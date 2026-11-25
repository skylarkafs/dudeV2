use crate::api::ActiveSession;
use crate::discovery::DiscoveryManager;
use crate::engine::EngineContext;
use nextdude_common::proto::v1;
use nextdude_common::APP_NAME;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{RwLock, Semaphore};
use tracing::{info, warn};

const MAX_HTTP_HEADER_BYTES: usize = 8192;
const MAX_CONCURRENT_WEB_CONNS: usize = 32;

/// Lifecycle state for a supervised child task or daemon worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildState {
    Running,
    Restarting,
    Stopped,
}

/// Snapshot metadata for one registered child task or worker.
#[derive(Debug, Clone, Serialize)]
pub struct ChildTaskEntry {
    pub name: String,
    pub role: String,
    pub state: ChildState,
    pub restarts: u64,
    pub started_utc: i64,
    pub last_restart_utc: Option<i64>,
    pub last_note: String,
}

/// RAII guard that decrements an atomic active-child counter when dropped.
pub struct ActiveChildGuard {
    counter: Arc<AtomicUsize>,
}

impl Drop for ActiveChildGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Tracks supervised engine children and transient active worker jobs without async blocking.
#[derive(Debug)]
pub struct ChildTaskTracker {
    supervised: StdRwLock<BTreeMap<String, ChildTaskEntry>>,
    active_probe_tasks: Arc<AtomicUsize>,
    active_discovery_tasks: Arc<AtomicUsize>,
    active_tool_tasks: Arc<AtomicUsize>,
    total_restarts: AtomicU64,
    poll_cycles_total: AtomicU64,
    last_poll_cycle_ms: AtomicU64,
}

impl Default for ChildTaskTracker {
    fn default() -> Self {
        Self {
            supervised: StdRwLock::new(BTreeMap::new()),
            active_probe_tasks: Arc::new(AtomicUsize::new(0)),
            active_discovery_tasks: Arc::new(AtomicUsize::new(0)),
            active_tool_tasks: Arc::new(AtomicUsize::new(0)),
            total_restarts: AtomicU64::new(0),
            poll_cycles_total: AtomicU64::new(0),
            last_poll_cycle_ms: AtomicU64::new(0),
        }
    }
}

impl ChildTaskTracker {
    /// Creates a shared tracker instance wrapped in `Arc`.
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Marks a supervised child task as actively running.
    pub fn mark_running(&self, name: &str, role: &str) {
        let now = chrono::Utc::now().timestamp();
        if let Ok(mut guard) = self.supervised.write() {
            let entry = guard
                .entry(name.to_string())
                .or_insert_with(|| ChildTaskEntry {
                    name: name.to_string(),
                    role: role.to_string(),
                    state: ChildState::Running,
                    restarts: 0,
                    started_utc: now,
                    last_restart_utc: None,
                    last_note: "Active".to_string(),
                });
            entry.role = role.to_string();
            entry.state = ChildState::Running;
            entry.started_utc = now;
            entry.last_note = "Active".to_string();
        }
    }

    /// Records a restart cycle (clean exit or caught panic) for a supervised child task.
    pub fn mark_restarting(&self, name: &str, panicked: bool) {
        let now = chrono::Utc::now().timestamp();
        self.total_restarts.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut guard) = self.supervised.write() {
            if let Some(entry) = guard.get_mut(name) {
                entry.state = ChildState::Restarting;
                entry.restarts = entry.restarts.saturating_add(1);
                entry.last_restart_utc = Some(now);
                entry.last_note = if panicked {
                    "Caught panic; restarting".to_string()
                } else {
                    "Exited cleanly; restarting".to_string()
                };
            }
        }
    }

    /// Marks an optional background child task as intentionally stopped/disabled.
    pub fn mark_stopped(&self, name: &str, role: &str, reason: &str) {
        let now = chrono::Utc::now().timestamp();
        if let Ok(mut guard) = self.supervised.write() {
            guard.insert(
                name.to_string(),
                ChildTaskEntry {
                    name: name.to_string(),
                    role: role.to_string(),
                    state: ChildState::Stopped,
                    restarts: 0,
                    started_utc: now,
                    last_restart_utc: None,
                    last_note: reason.to_string(),
                },
            );
        }
    }

    /// Acquires an RAII guard counting an active probe worker child task.
    pub fn acquire_probe_guard(&self) -> ActiveChildGuard {
        self.active_probe_tasks.fetch_add(1, Ordering::Relaxed);
        ActiveChildGuard {
            counter: Arc::clone(&self.active_probe_tasks),
        }
    }

    /// Acquires an RAII guard counting an active discovery scan child job.
    pub fn acquire_discovery_guard(&self) -> ActiveChildGuard {
        self.active_discovery_tasks.fetch_add(1, Ordering::Relaxed);
        ActiveChildGuard {
            counter: Arc::clone(&self.active_discovery_tasks),
        }
    }

    /// Acquires an RAII guard counting an active interactive diagnostic stream child task.
    pub fn acquire_tool_guard(&self) -> ActiveChildGuard {
        self.active_tool_tasks.fetch_add(1, Ordering::Relaxed);
        ActiveChildGuard {
            counter: Arc::clone(&self.active_tool_tasks),
        }
    }

    /// Updates poll scheduler cycle count and last observed cycle duration in milliseconds.
    pub fn record_poll_cycle(&self, duration_ms: u64) {
        self.poll_cycles_total.fetch_add(1, Ordering::Relaxed);
        self.last_poll_cycle_ms
            .store(duration_ms, Ordering::Relaxed);
    }

    /// Returns a cloned snapshot of all registered child tasks sorted by name.
    pub fn snapshot_children(&self) -> Vec<ChildTaskEntry> {
        self.supervised
            .read()
            .map(|g| g.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Returns `(supervised_running, active_probe, active_discovery, active_tool)`.
    pub fn active_counts(&self) -> (usize, usize, usize, usize) {
        let supervised_running = self
            .supervised
            .read()
            .map(|g| {
                g.values()
                    .filter(|e| e.state == ChildState::Running)
                    .count()
            })
            .unwrap_or(0);
        (
            supervised_running,
            self.active_probe_tasks.load(Ordering::Relaxed),
            self.active_discovery_tasks.load(Ordering::Relaxed),
            self.active_tool_tasks.load(Ordering::Relaxed),
        )
    }

    /// Returns `(total_restarts, poll_cycles_total, last_poll_cycle_ms)`.
    pub fn engine_counters(&self) -> (u64, u64, u64) {
        (
            self.total_restarts.load(Ordering::Relaxed),
            self.poll_cycles_total.load(Ordering::Relaxed),
            self.last_poll_cycle_ms.load(Ordering::Relaxed),
        )
    }
}

/// Process-level resource telemetry read from Linux `/proc` (or basic metadata on non-Linux).
#[derive(Debug, Clone, Serialize)]
pub struct ProcessHealthStats {
    pub pid: u32,
    pub rss_kib: Option<u64>,
    pub threads: Option<u64>,
    pub load_avg_1m: Option<f64>,
    pub load_avg_5m: Option<f64>,
    pub load_avg_15m: Option<f64>,
    pub db_size_bytes: u64,
    pub wal_size_bytes: u64,
}

/// Read-only aggregate inventory and monitoring counts (redacts names/IPs/secrets).
#[derive(Debug, Clone, Default, Serialize)]
pub struct MonitoringCounts {
    pub maps_total: u64,
    pub devices_total: u64,
    pub devices_up: u64,
    pub devices_down: u64,
    pub devices_unreachable: u64,
    pub devices_partial: u64,
    pub devices_unknown: u64,
    pub services_total: u64,
    pub services_up: u64,
    pub services_down: u64,
    pub links_total: u64,
    pub links_up: u64,
    pub links_down: u64,
    pub links_flapping: u64,
    pub dependencies_total: u64,
    pub active_outages: u64,
}

/// Complete JSON payload served on `GET /api/stats` and `GET /api/status`.
#[derive(Debug, Clone, Serialize)]
pub struct ServerStatusSnapshot {
    pub app_name: String,
    pub version: String,
    pub generated_utc: i64,
    pub started_utc: i64,
    pub uptime_sec: i64,
    pub grpc_port: u16,
    pub status_web_port: u16,
    pub running_children_total: usize,
    pub supervised_tasks_running: usize,
    pub supervised_tasks_total: usize,
    pub active_probe_tasks: usize,
    pub active_discovery_tasks: usize,
    pub active_tool_tasks: usize,
    pub total_child_restarts: u64,
    pub poll_cycles_total: u64,
    pub last_poll_cycle_ms: u64,
    pub active_client_sessions: usize,
    pub discovery_jobs_cached: usize,
    pub syslog_received: u64,
    pub syslog_dropped: u64,
    pub icmp_mode: String,
    pub children: Vec<ChildTaskEntry>,
    pub monitoring: MonitoringCounts,
    pub process: ProcessHealthStats,
}

/// Parses Linux `/proc/self/status` text for `VmRSS` (KiB) and `Threads`.
pub fn parse_proc_status(raw: &str) -> (Option<u64>, Option<u64>) {
    let mut rss_kib = None;
    let mut threads = None;
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            rss_kib = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<u64>().ok());
        } else if let Some(rest) = line.strip_prefix("Threads:") {
            threads = rest
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<u64>().ok());
        }
    }
    (rss_kib, threads)
}

/// Parses Linux `/proc/loadavg` text into `(1m, 5m, 15m)` load averages.
pub fn parse_proc_loadavg(raw: &str) -> (Option<f64>, Option<f64>, Option<f64>) {
    let mut parts = raw.split_whitespace();
    let l1 = parts.next().and_then(|s| s.parse::<f64>().ok());
    let l5 = parts.next().and_then(|s| s.parse::<f64>().ok());
    let l15 = parts.next().and_then(|s| s.parse::<f64>().ok());
    (l1, l5, l15)
}

/// Collects real process memory/load and SQLite DB/WAL byte sizes.
pub fn read_process_stats(db_path: &Path) -> ProcessHealthStats {
    #[cfg(target_os = "linux")]
    let (rss_kib, threads, load_avg_1m, load_avg_5m, load_avg_15m) = {
        let status_raw = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let (rss, thr) = parse_proc_status(&status_raw);
        let load_raw = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
        let (l1, l5, l15) = parse_proc_loadavg(&load_raw);
        (rss, thr, l1, l5, l15)
    };
    #[cfg(not(target_os = "linux"))]
    let (rss_kib, threads, load_avg_1m, load_avg_5m, load_avg_15m) =
        (None, None, None, None, None);

    let db_size_bytes = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
    let mut wal_os = db_path.as_os_str().to_os_string();
    wal_os.push("-wal");
    let wal_size_bytes = std::fs::metadata(Path::new(&wal_os))
        .map(|m| m.len())
        .unwrap_or(0);

    ProcessHealthStats {
        pid: std::process::id(),
        rss_kib,
        threads,
        load_avg_1m,
        load_avg_5m,
        load_avg_15m,
        db_size_bytes,
        wal_size_bytes,
    }
}

fn query_scalar_u64(conn: &rusqlite::Connection, sql: &str) -> u64 {
    conn.query_row(sql, [], |r| r.get::<_, i64>(0))
        .unwrap_or(0)
        .max(0) as u64
}

fn query_grouped_counts(conn: &rusqlite::Connection, sql: &str) -> Vec<(i32, u64)> {
    let Ok(mut stmt) = conn.prepare(sql) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))) else {
        return Vec::new();
    };
    rows.flatten()
        .map(|(k, c)| (k as i32, c.max(0) as u64))
        .collect()
}

/// Builds a live read-only status snapshot from server atomics, session map, and SQLite.
pub async fn collect_status_snapshot(
    ctx: &EngineContext,
    sessions: &Arc<RwLock<HashMap<String, ActiveSession>>>,
    discovery: &DiscoveryManager,
    syslog_stats: &Arc<(AtomicU64, AtomicU64)>,
) -> ServerStatusSnapshot {
    let now_utc = chrono::Utc::now().timestamp();
    let uptime_sec = now_utc.saturating_sub(ctx.server_started_utc);
    let children = ctx.tracker.snapshot_children();
    let (supervised_running, active_probe, active_discovery, active_tool) =
        ctx.tracker.active_counts();
    let (total_restarts, poll_cycles_total, last_poll_cycle_ms) = ctx.tracker.engine_counters();

    let active_client_sessions = sessions
        .read()
        .await
        .values()
        .filter(|s| !s.revoked)
        .count();
    let discovery_jobs_cached = discovery.jobs.read().await.len();

    let monitoring = ctx
        .storage
        .read(|conn| {
            let mut devices_total = 0u64;
            let mut devices_up = 0u64;
            let mut devices_down = 0u64;
            let mut devices_unreachable = 0u64;
            let mut devices_partial = 0u64;
            let mut devices_unknown = 0u64;
            for (state, c) in query_grouped_counts(
                conn,
                "SELECT aggregate_state, COUNT(*) FROM devices GROUP BY aggregate_state",
            ) {
                devices_total += c;
                match state {
                    x if x == v1::ProbeState::Up as i32 => devices_up += c,
                    x if x == v1::ProbeState::Down as i32 => devices_down += c,
                    x if x == v1::ProbeState::Unreachable as i32 => devices_unreachable += c,
                    x if x == v1::ProbeState::Partial as i32 => devices_partial += c,
                    _ => devices_unknown += c,
                }
            }

            let mut services_total = 0u64;
            let mut services_up = 0u64;
            let mut services_down = 0u64;
            for (state, c) in query_grouped_counts(
                conn,
                "SELECT state, COUNT(*) FROM device_services GROUP BY state",
            ) {
                services_total += c;
                if state == v1::ProbeState::Up as i32 {
                    services_up += c;
                } else if state == v1::ProbeState::Down as i32 {
                    services_down += c;
                }
            }

            let mut links_total = 0u64;
            let mut links_up = 0u64;
            let mut links_down = 0u64;
            let mut links_flapping = 0u64;
            for (status, c) in
                query_grouped_counts(conn, "SELECT status, COUNT(*) FROM links GROUP BY status")
            {
                links_total += c;
                match status {
                    x if x == v1::LinkStatus::Up as i32 => links_up += c,
                    x if x == v1::LinkStatus::Down as i32 => links_down += c,
                    x if x == v1::LinkStatus::Flapping as i32 => links_flapping += c,
                    _ => {}
                }
            }

            Ok(MonitoringCounts {
                maps_total: query_scalar_u64(conn, "SELECT COUNT(*) FROM maps"),
                devices_total,
                devices_up,
                devices_down,
                devices_unreachable,
                devices_partial,
                devices_unknown,
                services_total,
                services_up,
                services_down,
                links_total,
                links_up,
                links_down,
                links_flapping,
                dependencies_total: query_scalar_u64(conn, "SELECT COUNT(*) FROM dependencies"),
                active_outages: query_scalar_u64(
                    conn,
                    "SELECT COUNT(*) FROM outages WHERE ended_utc IS NULL",
                ),
            })
        })
        .await
        .unwrap_or_default();

    let process = read_process_stats(&ctx.config.db_path());

    ServerStatusSnapshot {
        app_name: APP_NAME.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        generated_utc: now_utc,
        started_utc: ctx.server_started_utc,
        uptime_sec,
        grpc_port: ctx.config.grpc_port,
        status_web_port: ctx.config.status_web_port,
        running_children_total: supervised_running + active_probe + active_discovery + active_tool,
        supervised_tasks_running: supervised_running,
        supervised_tasks_total: children.len(),
        active_probe_tasks: active_probe,
        active_discovery_tasks: active_discovery,
        active_tool_tasks: active_tool,
        total_child_restarts: total_restarts,
        poll_cycles_total,
        last_poll_cycle_ms,
        active_client_sessions,
        discovery_jobs_cached,
        syslog_received: syslog_stats.0.load(Ordering::Relaxed),
        syslog_dropped: syslog_stats.1.load(Ordering::Relaxed),
        icmp_mode: ctx.executor.icmp.report().message.clone(),
        children,
        monitoring,
        process,
    }
}

/// Runs the lightweight HTTP status dashboard and `/api/stats` endpoint.
pub async fn run_status_web_server(
    ctx: EngineContext,
    sessions: Arc<RwLock<HashMap<String, ActiveSession>>>,
    discovery: DiscoveryManager,
    syslog_stats: Arc<(AtomicU64, AtomicU64)>,
) {
    if !ctx.config.status_web_enabled {
        ctx.tracker.mark_stopped(
            "status_web",
            "HTTP Status Page",
            "disabled by configuration",
        );
        info!("Server HTTP status page disabled by configuration");
        return;
    }

    let ip: IpAddr = match ctx.config.status_web_addr.parse() {
        Ok(ip) => ip,
        Err(e) => {
            warn!("Invalid status_web_addr '{}': {e}", ctx.config.status_web_addr);
            ctx.tracker
                .mark_stopped("status_web", "HTTP Status Page", "invalid bind address");
            return;
        }
    };
    let bind_addr = SocketAddr::new(ip, ctx.config.status_web_port);
    let listener = match TcpListener::bind(bind_addr).await {
        Ok(l) => l,
        Err(e) => {
            warn!("Failed binding server status web UI on {bind_addr}: {e}");
            ctx.tracker
                .mark_stopped("status_web", "HTTP Status Page", "bind failed");
            return;
        }
    };
    info!("Server status web UI listening on http://{bind_addr}/");

    let conn_limit = Arc::new(Semaphore::new(MAX_CONCURRENT_WEB_CONNS));
    loop {
        match listener.accept().await {
            Ok((stream, _peer)) => {
                let Ok(permit) = Arc::clone(&conn_limit).try_acquire_owned() else {
                    drop(stream);
                    continue;
                };
                let ctx_c = ctx.clone();
                let sessions_c = Arc::clone(&sessions);
                let disc_c = discovery.clone();
                let syslog_c = Arc::clone(&syslog_stats);
                tokio::spawn(async move {
                    let _permit = permit;
                    handle_http_connection(stream, &ctx_c, &sessions_c, &disc_c, &syslog_c).await;
                });
            }
            Err(e) => {
                warn!("Status web accept error: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

async fn handle_http_connection(
    mut stream: TcpStream,
    ctx: &EngineContext,
    sessions: &Arc<RwLock<HashMap<String, ActiveSession>>>,
    discovery: &DiscoveryManager,
    syslog_stats: &Arc<(AtomicU64, AtomicU64)>,
) {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];

    let read_res = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let n = stream.read(&mut chunk).await?;
            if n == 0 {
                break;
            }
            let remaining = MAX_HTTP_HEADER_BYTES.saturating_sub(buf.len());
            let take = n.min(remaining);
            buf.extend_from_slice(&chunk[..take]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() >= MAX_HTTP_HEADER_BYTES {
                break;
            }
        }
        Ok::<(), std::io::Error>(())
    })
    .await;

    if read_res.is_err() || matches!(read_res, Ok(Err(_))) || buf.is_empty() {
        return;
    }

    let response = route_http_bytes(&buf, ctx, sessions, discovery, syslog_stats).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), stream.write_all(&response)).await;
    let _ = stream.shutdown().await;
}

/// Parses the HTTP request line and routes `GET`/`HEAD` requests to HTML, JSON stats, or `/healthz`.
pub async fn route_http_bytes(
    raw: &[u8],
    ctx: &EngineContext,
    sessions: &Arc<RwLock<HashMap<String, ActiveSession>>>,
    discovery: &DiscoveryManager,
    syslog_stats: &Arc<(AtomicU64, AtomicU64)>,
) -> Vec<u8> {
    let Some((method, target)) = parse_http_request_line(raw) else {
        return build_http_response(
            "400 Bad Request",
            "text/plain; charset=utf-8",
            b"400 Bad Request\n",
            false,
        );
    };

    let head_only = match method {
        "GET" => false,
        "HEAD" => true,
        _ => {
            return build_http_response(
                "405 Method Not Allowed",
                "text/plain; charset=utf-8",
                b"405 Method Not Allowed\n",
                false,
            );
        }
    };

    let path = target.split('?').next().unwrap_or(target);
    match path {
        "/" | "/index.html" => {
            let html = build_status_html();
            build_http_response(
                "200 OK",
                "text/html; charset=utf-8",
                html.as_bytes(),
                head_only,
            )
        }
        "/api/stats" | "/api/status" => {
            let snap = collect_status_snapshot(ctx, sessions, discovery, syslog_stats).await;
            let json = serde_json::to_vec_pretty(&snap).unwrap_or_else(|_| b"{}".to_vec());
            build_http_response("200 OK", "application/json; charset=utf-8", &json, head_only)
        }
        "/healthz" => {
            let (sup_running, probes, disc, tools) = ctx.tracker.active_counts();
            let total = sup_running + probes + disc + tools;
            let body = serde_json::to_vec(&serde_json::json!({
                "status": "ok",
                "app": APP_NAME,
                "running_children_total": total,
                "supervised_tasks_running": sup_running,
            }))
            .unwrap_or_else(|_| b"{\"status\":\"ok\"}".to_vec());
            build_http_response("200 OK", "application/json; charset=utf-8", &body, head_only)
        }
        _ => build_http_response(
            "404 Not Found",
            "text/plain; charset=utf-8",
            b"404 Not Found\n",
            head_only,
        ),
    }
}

/// Extracts `(METHOD, TARGET)` from the first line of an HTTP/1.x request buffer.
pub fn parse_http_request_line(raw: &[u8]) -> Option<(&str, &str)> {
    let text = std::str::from_utf8(raw).ok()?;
    let line = text.lines().next()?.trim_end_matches('\r');
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    let version = parts.next()?;
    if !version.starts_with("HTTP/1.") {
        return None;
    }
    Some((method, target))
}

fn build_http_response(
    status: &str,
    content_type: &str,
    body: &[u8],
    head_only: bool,
) -> Vec<u8> {
    let header = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         Cache-Control: no-store\r\n\
         X-Content-Type-Options: nosniff\r\n\
         X-Frame-Options: DENY\r\n\
         Referrer-Policy: no-referrer\r\n\
         Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'\r\n\r\n",
        body.len()
    );
    let mut out = Vec::with_capacity(header.len() + if head_only { 0 } else { body.len() });
    out.extend_from_slice(header.as_bytes());
    if !head_only {
        out.extend_from_slice(body);
    }
    out
}

/// Returns the static offline-capable HTML5 status page that hydrates from `/api/stats`.
pub fn build_status_html() -> &'static str {
    r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>NextDude Server Status</title>
  <style>
    :root {
      color-scheme: light dark;
      --bg: #f1f5f9;
      --surface: #ffffff;
      --surface-subtle: #f8fafc;
      --border: #cbd5e1;
      --text: #0f172a;
      --muted: #475569;
      --ok: #15803d;
      --ok-bg: #dcfce7;
      --warn: #b45309;
      --warn-bg: #fef3c7;
      --unreach: #c2410c;
      --down: #b91c1c;
      --down-bg: #fee2e2;
      --unk: #64748b;
      --accent: #0284c7;
      --accent-soft: #e0f2fe;
      font-family: system-ui, -apple-system, "Segoe UI", Roboto, sans-serif;
    }
    @media (prefers-color-scheme: dark) {
      :root {
        --bg: #0b1120;
        --surface: #131c2e;
        --surface-subtle: #182338;
        --border: #26334a;
        --text: #e2e8f0;
        --muted: #94a3b8;
        --ok: #4ade80;
        --ok-bg: #052e16;
        --warn: #fbbf24;
        --warn-bg: #451a03;
        --unreach: #fb923c;
        --down: #f87171;
        --down-bg: #450a0a;
        --unk: #64748b;
        --accent: #38bdf8;
        --accent-soft: #0c4a6e;
      }
    }
    * { box-sizing: border-box; }
    body {
      margin: 0;
      background: var(--bg);
      color: var(--text);
      line-height: 1.5;
      -webkit-font-smoothing: antialiased;
    }
    .container {
      max-width: 1120px;
      margin: 0 auto;
      padding: 22px 18px 44px;
    }
    header {
      display: flex;
      flex-wrap: wrap;
      justify-content: space-between;
      align-items: center;
      gap: 14px;
      margin-bottom: 18px;
      padding-bottom: 14px;
      border-bottom: 1px solid var(--border);
    }
    .brand {
      display: flex;
      align-items: center;
      gap: 12px;
    }
    .brand-mark {
      width: 34px;
      height: 34px;
      border-radius: 8px;
      border: 1px solid var(--border);
      background: var(--surface);
      display: grid;
      place-items: center;
      flex-shrink: 0;
    }
    h1 {
      margin: 0;
      font-size: 1.25rem;
      font-weight: 700;
      letter-spacing: -0.015em;
    }
    .subtitle {
      margin: 2px 0 0;
      color: var(--muted);
      font-size: 0.84rem;
    }
    .toolbar {
      display: flex;
      flex-wrap: wrap;
      align-items: center;
      gap: 8px;
    }
    .badge {
      display: inline-flex;
      align-items: center;
      gap: 6px;
      padding: 4px 10px;
      border-radius: 999px;
      font-size: 0.78rem;
      font-weight: 600;
      border: 1px solid var(--border);
      background: var(--surface);
      color: var(--text);
    }
    .dot {
      width: 7px;
      height: 7px;
      border-radius: 50%;
      background: currentColor;
    }
    .badge-ok { background: var(--ok-bg); color: var(--ok); border-color: transparent; }
    .badge-warn { background: var(--warn-bg); color: var(--warn); border-color: transparent; }
    .badge-down { background: var(--down-bg); color: var(--down); border-color: transparent; }
    .btn, a.btn {
      cursor: pointer;
      display: inline-flex;
      align-items: center;
      padding: 6px 12px;
      border: 1px solid var(--border);
      border-radius: 6px;
      background: var(--surface);
      color: var(--text);
      font-size: 0.82rem;
      font-weight: 600;
      text-decoration: none;
      transition: border-color 120ms ease, background-color 120ms ease;
    }
    .btn:hover { border-color: var(--accent); background: var(--surface-subtle); }
    .btn:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
    .cards {
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(225px, 1fr));
      gap: 12px;
      margin-bottom: 16px;
    }
    .card {
      background: var(--surface);
      border: 1px solid var(--border);
      border-radius: 10px;
      padding: 14px 16px;
    }
    .card-accent {
      border-color: var(--accent);
    }
    .card-label {
      color: var(--muted);
      font-size: 0.74rem;
      font-weight: 700;
      text-transform: uppercase;
      letter-spacing: 0.05em;
    }
    .card-value {
      margin-top: 6px;
      font-size: 1.7rem;
      font-weight: 700;
      line-height: 1.15;
      font-variant-numeric: tabular-nums;
    }
    .card-sub {
      margin-top: 6px;
      font-size: 0.8rem;
      color: var(--muted);
      font-variant-numeric: tabular-nums;
    }
    section {
      background: var(--surface);
      border: 1px solid var(--border);
      border-radius: 10px;
      padding: 16px;
      margin-bottom: 16px;
    }
    .section-head {
      display: flex;
      flex-wrap: wrap;
      justify-content: space-between;
      align-items: center;
      gap: 10px;
      margin-bottom: 12px;
    }
    h2 {
      margin: 0;
      font-size: 0.98rem;
      font-weight: 700;
    }
    .worker-pills {
      display: flex;
      flex-wrap: wrap;
      gap: 6px;
    }
    .chip {
      font-size: 0.76rem;
      font-weight: 600;
      padding: 3px 9px;
      border-radius: 6px;
      background: var(--surface-subtle);
      border: 1px solid var(--border);
      font-variant-numeric: tabular-nums;
    }
    .health-bar-wrap {
      margin-bottom: 14px;
    }
    .health-bar {
      display: flex;
      height: 10px;
      border-radius: 999px;
      overflow: hidden;
      background: var(--surface-subtle);
      border: 1px solid var(--border);
    }
    .seg-up { background: var(--ok); }
    .seg-partial { background: var(--warn); }
    .seg-unreach { background: var(--unreach); }
    .seg-down { background: var(--down); }
    .seg-unk { background: var(--unk); }
    .health-legend {
      display: flex;
      flex-wrap: wrap;
      gap: 14px;
      margin-top: 8px;
      font-size: 0.78rem;
      color: var(--muted);
      font-variant-numeric: tabular-nums;
    }
    .table-wrap { overflow-x: auto; }
    table {
      width: 100%;
      border-collapse: collapse;
      font-size: 0.86rem;
    }
    th, td {
      text-align: left;
      padding: 9px 10px;
      border-bottom: 1px solid var(--border);
      font-variant-numeric: tabular-nums;
    }
    tbody tr:last-child td { border-bottom: none; }
    tbody tr:hover { background: var(--surface-subtle); }
    th {
      color: var(--muted);
      font-weight: 600;
      font-size: 0.76rem;
      text-transform: uppercase;
      letter-spacing: 0.04em;
    }
    .stats-grid {
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
      gap: 8px 20px;
      margin: 0;
    }
    .stat-row {
      display: flex;
      justify-content: space-between;
      align-items: baseline;
      gap: 12px;
      padding: 7px 0;
      border-bottom: 1px dashed var(--border);
      font-size: 0.85rem;
    }
    .stat-row dt { color: var(--muted); }
    .stat-row dd {
      margin: 0;
      font-weight: 600;
      text-align: right;
      font-variant-numeric: tabular-nums;
    }
    .mono { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
    .sr-only {
      position: absolute;
      width: 1px;
      height: 1px;
      overflow: hidden;
      clip: rect(0, 0, 0, 0);
      white-space: nowrap;
    }
  </style>
</head>
<body>
  <div class="container">
    <header>
      <div class="brand">
        <div class="brand-mark" aria-hidden="true">
          <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <circle cx="12" cy="12" r="9" opacity="0.4"></circle>
            <circle cx="12" cy="12" r="4"></circle>
            <path d="M12 3v3M12 18v3M3 12h3M18 12h3"></path>
          </svg>
        </div>
        <div>
          <h1 id="page-title">NextDude Server Status</h1>
          <p class="subtitle" id="page-meta">Daemon runtime, supervised children &amp; aggregate monitoring stats</p>
        </div>
      </div>
      <div class="toolbar">
        <span id="live-badge" class="badge" aria-live="polite"><span class="dot"></span><span>Connecting</span></span>
        <button id="toggle-auto" class="btn" type="button">Pause Auto</button>
        <button id="refresh-now" class="btn" type="button">Refresh</button>
        <a class="btn mono" href="/api/stats">JSON</a>
      </div>
    </header>

    <noscript>
      <section><p>JavaScript is disabled. Live JSON telemetry is served at <a href="/api/stats">/api/stats</a>.</p></section>
    </noscript>

    <main>
      <div class="cards">
        <article class="card card-accent">
          <div class="card-label">Children Running</div>
          <div class="card-value" id="kpi-children">-</div>
          <div class="card-sub" id="kpi-children-sub">-</div>
        </article>
        <article class="card">
          <div class="card-label">Server Uptime</div>
          <div class="card-value" id="kpi-uptime">-</div>
          <div class="card-sub" id="kpi-uptime-sub">-</div>
        </article>
        <article class="card">
          <div class="card-label">Memory &amp; System Load</div>
          <div class="card-value" id="kpi-rss">-</div>
          <div class="card-sub" id="kpi-load-sub">-</div>
        </article>
        <article class="card">
          <div class="card-label">Devices &amp; Client Sessions</div>
          <div class="card-value" id="kpi-devices">-</div>
          <div class="card-sub" id="kpi-clients-sub">-</div>
        </article>
      </div>

      <section aria-labelledby="children-heading">
        <div class="section-head">
          <h2 id="children-heading">Running Children &amp; Worker Tasks</h2>
          <div class="worker-pills">
            <span class="chip" id="chip-probes">Probes: 0</span>
            <span class="chip" id="chip-discovery">Discovery: 0</span>
            <span class="chip" id="chip-tools">Tools: 0</span>
          </div>
        </div>
        <div class="table-wrap">
          <table>
            <caption class="sr-only">Supervised server child tasks and active worker counts</caption>
            <thead>
              <tr>
                <th scope="col">Task</th>
                <th scope="col">Role</th>
                <th scope="col">State</th>
                <th scope="col">Restarts</th>
                <th scope="col">Started (UTC)</th>
                <th scope="col">Status Note</th>
              </tr>
            </thead>
            <tbody id="children-tbody">
              <tr><td colspan="6">Loading...</td></tr>
            </tbody>
          </table>
        </div>
      </section>

      <section aria-labelledby="stats-heading">
        <div class="section-head">
          <h2 id="stats-heading">Server &amp; Monitoring Stats</h2>
          <span class="chip mono" id="st-icmp-chip">ICMP: -</span>
        </div>
        <div class="health-bar-wrap">
          <div class="health-bar" role="img" aria-label="Device status proportion">
            <div id="bar-up" class="seg-up" style="width:0%"></div>
            <div id="bar-partial" class="seg-partial" style="width:0%"></div>
            <div id="bar-unreach" class="seg-unreach" style="width:0%"></div>
            <div id="bar-down" class="seg-down" style="width:0%"></div>
            <div id="bar-unk" class="seg-unk" style="width:100%"></div>
          </div>
          <div class="health-legend" id="health-legend">No monitored devices yet</div>
        </div>
        <dl class="stats-grid">
          <div class="stat-row"><dt>Devices (Up / Down / Unreach / Partial)</dt><dd id="st-devices">-</dd></div>
          <div class="stat-row"><dt>Services (Total / Up / Down)</dt><dd id="st-services">-</dd></div>
          <div class="stat-row"><dt>Links (Total / Up / Down / Flapping)</dt><dd id="st-links">-</dd></div>
          <div class="stat-row"><dt>Maps / Dependencies / Active Outages</dt><dd id="st-topology">-</dd></div>
          <div class="stat-row"><dt>Poll Cycles / Last Cycle Duration</dt><dd id="st-cycles">-</dd></div>
          <div class="stat-row"><dt>Syslog Received / Dropped</dt><dd id="st-syslog">-</dd></div>
          <div class="stat-row"><dt>SQLite DB + WAL Size</dt><dd id="st-db">-</dd></div>
          <div class="stat-row"><dt>Active Admin Sessions</dt><dd id="st-sessions">-</dd></div>
          <div class="stat-row"><dt>Total Child Panic/Restarts</dt><dd id="st-restarts">-</dd></div>
          <div class="stat-row"><dt>Ports (gRPC TLS / Status HTTP)</dt><dd id="st-ports">-</dd></div>
        </dl>
      </section>
    </main>
  </div>

  <script>
    (function () {
      let autoRefresh = true;
      let timerId = null;
      let inFlight = null;

      function fmtDuration(sec) {
        sec = Math.max(0, Math.floor(sec || 0));
        const d = Math.floor(sec / 86400);
        const h = Math.floor((sec % 86400) / 3600);
        const m = Math.floor((sec % 3600) / 60);
        const s = sec % 60;
        if (d > 0) return d + "d " + h + "h " + m + "m";
        if (h > 0) return h + "h " + m + "m " + s + "s";
        if (m > 0) return m + "m " + s + "s";
        return s + "s";
      }

      function fmtBytes(bytes) {
        const b = Number(bytes || 0);
        if (b >= 1073741824) return (b / 1073741824).toFixed(2) + " GiB";
        if (b >= 1048576) return (b / 1048576).toFixed(2) + " MiB";
        if (b >= 1024) return (b / 1024).toFixed(1) + " KiB";
        return b + " B";
      }

      function fmtUtc(ts) {
        if (!ts) return "-";
        return new Date(ts * 1000).toISOString().replace("T", " ").slice(0, 19);
      }

      function setText(id, text) {
        const el = document.getElementById(id);
        if (el) el.textContent = text;
      }

      function setLiveBadge(cls, text) {
        const badge = document.getElementById("live-badge");
        if (!badge) return;
        badge.className = "badge " + cls;
        const dot = document.createElement("span");
        dot.className = "dot";
        const lbl = document.createElement("span");
        lbl.textContent = text;
        badge.replaceChildren(dot, lbl);
      }

      function renderHealthBar(m) {
        const tot = Number(m.devices_total || 0);
        if (tot <= 0) {
          document.getElementById("bar-up").style.width = "0%";
          document.getElementById("bar-partial").style.width = "0%";
          document.getElementById("bar-unreach").style.width = "0%";
          document.getElementById("bar-down").style.width = "0%";
          document.getElementById("bar-unk").style.width = "100%";
          setText("health-legend", "No monitored devices registered yet");
          return;
        }
        const pct = (n) => ((Number(n || 0) / tot) * 100).toFixed(2) + "%";
        document.getElementById("bar-up").style.width = pct(m.devices_up);
        document.getElementById("bar-partial").style.width = pct(m.devices_partial);
        document.getElementById("bar-unreach").style.width = pct(m.devices_unreachable);
        document.getElementById("bar-down").style.width = pct(m.devices_down);
        document.getElementById("bar-unk").style.width = pct(m.devices_unknown);
        setText(
          "health-legend",
          "Up " + m.devices_up +
            " | Partial " + m.devices_partial +
            " | Unreachable " + m.devices_unreachable +
            " | Down " + m.devices_down +
            " | Unknown " + m.devices_unknown
        );
      }

      function renderChildrenTable(children) {
        const tbody = document.getElementById("children-tbody");
        if (!tbody) return;
        const rows = [];
        for (const child of children) {
          const tr = document.createElement("tr");

          const tdName = document.createElement("td");
          tdName.className = "mono";
          tdName.textContent = child.name;

          const tdRole = document.createElement("td");
          tdRole.textContent = child.role;

          const tdState = document.createElement("td");
          const pill = document.createElement("span");
          pill.className = "badge " + (
            child.state === "running" ? "badge-ok" :
            child.state === "restarting" ? "badge-warn" : ""
          );
          pill.textContent = child.state;
          tdState.appendChild(pill);

          const tdRestarts = document.createElement("td");
          if (child.restarts > 0) {
            const rp = document.createElement("span");
            rp.className = "badge badge-warn";
            rp.textContent = String(child.restarts);
            tdRestarts.appendChild(rp);
          } else {
            tdRestarts.textContent = "0";
          }

          const tdStarted = document.createElement("td");
          tdStarted.className = "mono";
          tdStarted.textContent = fmtUtc(child.started_utc);

          const tdNote = document.createElement("td");
          tdNote.textContent = child.last_note || "-";

          tr.append(tdName, tdRole, tdState, tdRestarts, tdStarted, tdNote);
          rows.push(tr);
        }
        tbody.replaceChildren(...rows);
      }

      function renderSnapshot(data) {
        setText("page-title", data.app_name + " Server Status (v" + data.version + ")");
        setText("page-meta", "Last updated UTC: " + fmtUtc(data.generated_utc));

        setText("kpi-children", String(data.running_children_total));
        setText(
          "kpi-children-sub",
          data.supervised_tasks_running + "/" + data.supervised_tasks_total +
            " supervised + " +
            (data.active_probe_tasks + data.active_discovery_tasks + data.active_tool_tasks) +
            " active workers"
        );

        setText("kpi-uptime", fmtDuration(data.uptime_sec));
        setText(
          "kpi-uptime-sub",
          "PID " + data.process.pid +
            (data.process.threads != null ? " | Threads " + data.process.threads : "")
        );

        const rssText = data.process.rss_kib != null
          ? (data.process.rss_kib / 1024).toFixed(1) + " MiB"
          : "N/A";
        setText("kpi-rss", rssText);
        const loadText = data.process.load_avg_1m != null
          ? "Load " + data.process.load_avg_1m.toFixed(2) + " / " +
            data.process.load_avg_5m.toFixed(2) + " / " +
            data.process.load_avg_15m.toFixed(2)
          : "Linux /proc stats on host";
        setText("kpi-load-sub", loadText);

        const m = data.monitoring;
        setText("kpi-devices", m.devices_up + " / " + m.devices_total + " UP");
        setText(
          "kpi-clients-sub",
          data.active_client_sessions + " active client session(s) | " + m.active_outages + " outage(s)"
        );

        setText("chip-probes", "Probes: " + data.active_probe_tasks);
        setText(
          "chip-discovery",
          "Discovery: " + data.active_discovery_tasks + " (cached " + data.discovery_jobs_cached + ")"
        );
        setText("chip-tools", "Tools: " + data.active_tool_tasks);

        renderChildrenTable(data.children || []);
        renderHealthBar(m);

        setText(
          "st-devices",
          m.devices_up + " / " + m.devices_down + " / " + m.devices_unreachable + " / " + m.devices_partial
        );
        setText("st-services", m.services_total + " / " + m.services_up + " / " + m.services_down);
        setText(
          "st-links",
          m.links_total + " / " + m.links_up + " / " + m.links_down + " / " + m.links_flapping
        );
        setText("st-topology", m.maps_total + " / " + m.dependencies_total + " / " + m.active_outages);
        setText("st-cycles", data.poll_cycles_total + " / " + data.last_poll_cycle_ms + " ms");
        setText("st-syslog", data.syslog_received + " / " + data.syslog_dropped);
        setText(
          "st-db",
          fmtBytes(data.process.db_size_bytes) + " + " + fmtBytes(data.process.wal_size_bytes) + " WAL"
        );
        setText("st-sessions", String(data.active_client_sessions));
        setText("st-restarts", String(data.total_child_restarts));
        setText("st-ports", data.grpc_port + " / " + data.status_web_port);
        setText("st-icmp-chip", "ICMP: " + (data.icmp_mode || "-"));
      }

      async function fetchStats() {
        if (inFlight) inFlight.abort();
        inFlight = new AbortController();
        const timeoutId = setTimeout(() => inFlight.abort(), 3500);
        try {
          const resp = await fetch("/api/stats", {
            signal: inFlight.signal,
            headers: { "Accept": "application/json" }
          });
          if (!resp.ok) throw new Error("HTTP " + resp.status);
          const data = await resp.json();
          renderSnapshot(data);
          setLiveBadge(autoRefresh ? "badge-ok" : "badge-warn", autoRefresh ? "Live" : "Paused");
        } catch (err) {
          if (err && err.name === "AbortError") return;
          setLiveBadge("badge-down", "Disconnected");
        } finally {
          clearTimeout(timeoutId);
        }
      }

      function scheduleNext() {
        if (timerId) clearInterval(timerId);
        if (autoRefresh && document.visibilityState === "visible") {
          timerId = setInterval(fetchStats, 2000);
        }
      }

      document.addEventListener("DOMContentLoaded", () => {
        const toggleBtn = document.getElementById("toggle-auto");
        const refreshBtn = document.getElementById("refresh-now");
        if (toggleBtn) {
          toggleBtn.addEventListener("click", () => {
            autoRefresh = !autoRefresh;
            toggleBtn.textContent = autoRefresh ? "Pause Auto" : "Resume Auto";
            setLiveBadge(autoRefresh ? "badge-ok" : "badge-warn", autoRefresh ? "Live" : "Paused");
            scheduleNext();
          });
        }
        if (refreshBtn) {
          refreshBtn.addEventListener("click", fetchStats);
        }
        document.addEventListener("visibilitychange", () => {
          if (document.visibilityState === "visible" && autoRefresh) {
            fetchStats();
          }
          scheduleNext();
        });
        fetchStats();
        scheduleNext();
      });
    })();
  </script>
</body>
</html>
"#
}
