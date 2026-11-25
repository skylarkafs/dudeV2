pub mod auth;
pub mod entities;
pub mod migrations;
pub mod telemetry;

use crate::crypto::SecretBox;
use crate::error::ServerError;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use tokio::sync::oneshot;

pub type DbTask = Box<dyn FnOnce(&mut Connection) + Send>;

#[derive(Clone)]
pub struct StorageHandle {
    db_path: PathBuf,
    writer_tx: SyncSender<DbTask>,
    reader_conn: Arc<Mutex<Connection>>,
    secrets: SecretBox,
}

impl StorageHandle {
    pub fn open(db_path: &Path, secrets: SecretBox) -> Result<Self, ServerError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut writer_conn = open_configured_connection(db_path, false)?;
        migrations::run_migrations(&mut writer_conn)?;

        let reader_conn = open_configured_connection(db_path, true)?;
        let (writer_tx, writer_rx) = sync_channel::<DbTask>(4096);

        thread::Builder::new()
            .name("nextdude-sqlite-writer".to_string())
            .spawn(move || writer_loop(writer_conn, writer_rx))
            .map_err(|e| ServerError::Storage(format!("failed spawning sqlite writer thread: {e}")))?;

        Ok(Self {
            db_path: db_path.to_path_buf(),
            writer_tx,
            reader_conn: Arc::new(Mutex::new(reader_conn)),
            secrets,
        })
    }

    pub fn secrets(&self) -> &SecretBox {
        &self.secrets
    }

    pub async fn write<T, F>(&self, f: F) -> Result<T, ServerError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, ServerError> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let task: DbTask = Box::new(move |conn| {
            let res = f(conn);
            let _ = tx.send(res);
        });
        self.writer_tx
            .try_send(task)
            .map_err(|e| ServerError::Storage(format!("SQLite writer queue full or closed: {e}")))?;
        rx.await
            .map_err(|_| ServerError::Storage("SQLite writer task dropped".to_string()))?
    }

    pub async fn read<T, F>(&self, f: F) -> Result<T, ServerError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, ServerError> + Send + 'static,
    {
        let reader = Arc::clone(&self.reader_conn);
        tokio::task::spawn_blocking(move || {
            let guard = reader
                .lock()
                .map_err(|_| ServerError::Storage("SQLite reader mutex poisoned".to_string()))?;
            f(&guard)
        })
        .await
        .map_err(|e| ServerError::Storage(format!("SQLite read join error: {e}")))?
    }

    pub fn backup_to(&self, dest: &Path) -> Result<(), ServerError> {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let src = open_configured_connection(&self.db_path, true)?;
        let mut dst = Connection::open(dest)
            .map_err(|e| ServerError::Storage(format!("open backup dest failed: {e}")))?;
        let backup = rusqlite::backup::Backup::new(&src, &mut dst)
            .map_err(|e| ServerError::Storage(format!("init sqlite backup failed: {e}")))?;
        backup
            .run_to_completion(256, Duration::from_millis(10), None)
            .map_err(|e| ServerError::Storage(format!("sqlite backup failed: {e}")))?;
        Ok(())
    }

    pub fn restore_from(db_path: &Path, src_backup: &Path) -> Result<(), ServerError> {
        if !src_backup.exists() {
            return Err(ServerError::NotFound(format!(
                "backup file not found: {}",
                src_backup.display()
            )));
        }
        let src = Connection::open_with_flags(src_backup, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| ServerError::Storage(format!("open backup file failed: {e}")))?;
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut dst = Connection::open(db_path)
            .map_err(|e| ServerError::Storage(format!("open restore target failed: {e}")))?;
        let backup = rusqlite::backup::Backup::new(&src, &mut dst)
            .map_err(|e| ServerError::Storage(format!("init restore failed: {e}")))?;
        backup
            .run_to_completion(256, Duration::from_millis(10), None)
            .map_err(|e| ServerError::Storage(format!("sqlite restore failed: {e}")))?;
        Ok(())
    }
}

fn writer_loop(mut conn: Connection, rx: Receiver<DbTask>) {
    while let Ok(first) = rx.recv() {
        let mut batch = Vec::with_capacity(32);
        batch.push(first);
        while batch.len() < 64 {
            match rx.try_recv() {
                Ok(task) => batch.push(task),
                Err(_) => break,
            }
        }
        for task in batch {
            task(&mut conn);
        }
    }
}

fn open_configured_connection(path: &Path, read_only: bool) -> Result<Connection, ServerError> {
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
    };
    let conn = Connection::open_with_flags(path, flags)
        .map_err(|e| ServerError::Storage(format!("failed opening {}: {e}", path.display())))?;
    conn.busy_timeout(Duration::from_secs(10))
        .map_err(|e| ServerError::Storage(format!("failed setting busy_timeout: {e}")))?;
    if !read_only {
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| ServerError::Storage(format!("failed enabling WAL: {e}")))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| ServerError::Storage(format!("failed setting synchronous: {e}")))?;
    }
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| ServerError::Storage(format!("failed enabling foreign_keys: {e}")))?;
    Ok(conn)
}
