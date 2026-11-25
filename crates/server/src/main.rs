#![forbid(unsafe_code)]

use anyhow::Context;
use clap::Parser;
use nextdude_common::proto::v1::next_dude_service_server::NextDudeServiceServer;
use nextdude_common::APP_NAME;
use nextdude_server::api::NextDudeGrpcService;
use nextdude_server::cli::{AdminAction, Cli, Command};
use nextdude_server::config::ServerConfig;
use nextdude_server::crypto::{load_or_create_tls_material, SecretBox};
use nextdude_server::discovery::mndp::{run_mndp_listener, MndpCache};
use nextdude_server::discovery::DiscoveryManager;
use nextdude_server::engine::scheduler::{polling_loop, rollup_and_retention_loop, spawn_supervised};
use nextdude_server::engine::{EngineContext, ServerBroadcastEvent};
use nextdude_server::notify::NotificationManager;
use nextdude_server::probe::icmp::IcmpProber;
use nextdude_server::probe::LocalProbeExecutor;
use nextdude_server::snmp::profiles::VendorProfileRegistry;
use nextdude_server::snmp::SnmpClient;
use nextdude_server::storage::auth::{create_admin, reset_admin_password};
use nextdude_server::storage::StorageHandle;
use nextdude_server::syslog::run_syslog_receiver;
use nextdude_server::trap::run_trap_receiver;
use nextdude_server::web_status::{run_status_web_server, ChildTaskTracker};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, RwLock};
use tonic::codec::CompressionEncoding;
use tonic::transport::{Identity, Server, ServerTlsConfig};
use tracing::info;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let cli = Cli::parse();

    match cli.command {
        Command::CheckConfig => {
            let cfg = ServerConfig::load_from_path(&cli.config)?;
            let icmp = IcmpProber::initialize();
            let profiles = VendorProfileRegistry::load(
                &cfg.system_profiles_dir,
                &cfg.override_profiles_dir,
            )?;
            let status_web = if cfg.status_web_enabled {
                format!("http://{}:{}/", cfg.status_web_addr, cfg.status_web_port)
            } else {
                "disabled".to_string()
            };
            println!(
                "{} config OK: listen={}:{}, status_web={}, data_dir={}, profiles={}, icmp={}",
                APP_NAME,
                cfg.listen_addr,
                cfg.grpc_port,
                status_web,
                cfg.data_dir.display(),
                profiles.all().len(),
                icmp.report().message
            );
            Ok(())
        }
        Command::Admin(cmd) => {
            let cfg = ServerConfig::load_from_path(&cli.config)?;
            let secrets = SecretBox::load_or_create(&cfg.master_key_path())?;
            let storage = StorageHandle::open(&cfg.db_path(), secrets)?;
            match cmd.action {
                AdminAction::Create {
                    username,
                    password,
                    group,
                } => {
                    let u = username.clone();
                    let id = storage
                        .write(move |c| create_admin(c, &u, &password, &group))
                        .await?;
                    println!("Created admin '{username}' with id={id}");
                }
                AdminAction::ResetPassword { username, password } => {
                    let u = username.clone();
                    storage
                        .write(move |c| reset_admin_password(c, &u, &password))
                        .await?;
                    println!("Password updated for admin '{username}'");
                }
            }
            Ok(())
        }
        Command::Backup(args) => {
            let cfg = ServerConfig::load_from_path(&cli.config)?;
            let secrets = SecretBox::load_or_create(&cfg.master_key_path())?;
            let storage = StorageHandle::open(&cfg.db_path(), secrets)?;
            storage.backup_to(&args.path)?;
            println!("Database backed up to {}", args.path.display());
            Ok(())
        }
        Command::Restore(args) => {
            let cfg = ServerConfig::load_from_path(&cli.config)?;
            StorageHandle::restore_from(&cfg.db_path(), &args.path)?;
            println!("Database restored from {}", args.path.display());
            Ok(())
        }
        Command::Run => run_server(&cli.config).await,
    }
}

fn init_logging(cfg: &ServerConfig) -> anyhow::Result<tracing_appender::non_blocking::WorkerGuard> {
    std::fs::create_dir_all(&cfg.log_dir)?;
    let file_appender = tracing_appender::rolling::daily(&cfg.log_dir, "nextdude-server.log");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,tonic=warn,h2=warn,hyper=warn"));

    let file_layer = fmt::layer().with_writer(non_blocking).with_ansi(false);
    let stdout_layer = fmt::layer().with_ansi(false);

    #[cfg(target_os = "linux")]
    {
        let journal_layer = tracing_journald::layer().ok();
        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .with(stdout_layer)
            .with(journal_layer)
            .init();
    }
    #[cfg(not(target_os = "linux"))]
    {
        tracing_subscriber::registry()
            .with(filter)
            .with(file_layer)
            .with(stdout_layer)
            .init();
    }

    Ok(guard)
}

async fn run_server(config_path: &std::path::Path) -> anyhow::Result<()> {
    let cfg = ServerConfig::load_from_path(config_path)
        .with_context(|| format!("failed loading config {}", config_path.display()))?;
    let _log_guard = init_logging(&cfg)?;
    info!("Starting {} server...", APP_NAME);

    std::fs::create_dir_all(&cfg.data_dir)?;
    std::fs::create_dir_all(cfg.images_dir())?;

    let secrets = SecretBox::load_or_create(&cfg.master_key_path())?;
    let storage = StorageHandle::open(&cfg.db_path(), secrets)?;

    let cfg_init = cfg.clone();
    storage
        .write(move |c| {
            c.execute(
                "UPDATE server_runtime_settings SET
                  global_poll_interval_sec=COALESCE(global_poll_interval_sec, ?1),
                  max_concurrent_probes=COALESCE(max_concurrent_probes, ?2),
                  max_concurrent_per_host=COALESCE(max_concurrent_per_host, ?3)
                 WHERE id=1",
                rusqlite::params![
                    i64::from(cfg_init.global_poll_interval_sec),
                    cfg_init.max_concurrent_probes as i64,
                    cfg_init.max_concurrent_per_host as i64
                ],
            )
            .map_err(|e| nextdude_server::error::ServerError::Storage(e.to_string()))?;
            Ok(())
        })
        .await?;

    let tls = load_or_create_tls_material(
        &cfg.effective_tls_cert_path(),
        &cfg.effective_tls_key_path(),
        std::slice::from_ref(&cfg.listen_addr),
    )?;
    info!(
        "TLS 1.3 server certificate SHA-256 fingerprint: {}",
        tls.sha256_fingerprint
    );

    let icmp = IcmpProber::initialize();
    let snmp = SnmpClient::new();
    let executor = Arc::new(LocalProbeExecutor::new(icmp, snmp.clone())?);
    let profiles = Arc::new(RwLock::new(VendorProfileRegistry::load(
        &cfg.system_profiles_dir,
        &cfg.override_profiles_dir,
    )?));
    info!(
        "Loaded {} SNMP vendor profile(s)",
        profiles.read().await.all().len()
    );

    let (events_tx, _) = broadcast::channel::<ServerBroadcastEvent>(2048);
    let notifications = NotificationManager::new(storage.clone(), events_tx.clone());

    let tracker = ChildTaskTracker::new();
    let server_started_utc = chrono::Utc::now().timestamp();

    let ctx = EngineContext {
        config: cfg.clone(),
        storage: storage.clone(),
        snmp,
        executor,
        profiles,
        notifications,
        events_tx,
        device_host_map: Arc::new(RwLock::new(HashMap::new())),
        link_flapping: Arc::new(RwLock::new(HashMap::new())),
        down_devices: Arc::new(RwLock::new(HashSet::new())),
        active_outages: Arc::new(RwLock::new(HashMap::new())),
        tracker,
        server_started_utc,
    };

    let syslog_stats = Arc::new((AtomicU64::new(0), AtomicU64::new(0)));
    let mndp_cache: MndpCache = Arc::new(RwLock::new(HashMap::new()));
    let discovery = DiscoveryManager {
        jobs: Arc::new(RwLock::new(HashMap::new())),
        mndp_cache: Arc::clone(&mndp_cache),
    };
    let service = NextDudeGrpcService::new(
        ctx.clone(),
        discovery.clone(),
        Arc::clone(&syslog_stats),
    );

    spawn_supervised(
        "polling_scheduler",
        "Monitoring Polling Engine",
        ctx.clone(),
        polling_loop,
    );
    spawn_supervised(
        "rollup_retention",
        "Telemetry Rollups & Retention",
        ctx.clone(),
        rollup_and_retention_loop,
    );
    if cfg.syslog_enabled {
        let stats_c = Arc::clone(&syslog_stats);
        spawn_supervised(
            "syslog_receiver",
            "UDP Syslog Ingestion",
            ctx.clone(),
            move |c| run_syslog_receiver(c, Arc::clone(&stats_c)),
        );
    } else {
        ctx.tracker
            .mark_stopped("syslog_receiver", "UDP Syslog Ingestion", "disabled by config");
    }
    if cfg.trap_enabled {
        spawn_supervised(
            "trap_receiver",
            "UDP SNMP Trap / Inform Receiver",
            ctx.clone(),
            run_trap_receiver,
        );
    } else {
        ctx.tracker.mark_stopped(
            "trap_receiver",
            "UDP SNMP Trap / Inform Receiver",
            "disabled by config",
        );
    }
    if cfg.mndp_enabled {
        let mndp_c = Arc::clone(&mndp_cache);
        spawn_supervised(
            "mndp_listener",
            "UDP MNDP Discovery Listener",
            ctx.clone(),
            move |c| run_mndp_listener(c, Arc::clone(&mndp_c)),
        );
    } else {
        ctx.tracker.mark_stopped(
            "mndp_listener",
            "UDP MNDP Discovery Listener",
            "disabled by config",
        );
    }
    if cfg.status_web_enabled {
        let sessions_c = Arc::clone(&service.sessions);
        let disc_c = discovery;
        let syslog_c = syslog_stats;
        spawn_supervised("status_web", "HTTP Status Page", ctx.clone(), move |c| {
            run_status_web_server(c, Arc::clone(&sessions_c), disc_c.clone(), Arc::clone(&syslog_c))
        });
    } else {
        ctx.tracker
            .mark_stopped("status_web", "HTTP Status Page", "disabled by config");
    }

    let grpc_addr: SocketAddr = format!("{}:{}", cfg.listen_addr, cfg.grpc_port).parse()?;
    let svc_server = NextDudeServiceServer::new(service)
        .accept_compressed(CompressionEncoding::Gzip)
        .send_compressed(CompressionEncoding::Gzip)
        .max_decoding_message_size(32 * 1024 * 1024);

    let tls_cfg = ServerTlsConfig::new().identity(Identity::from_pem(&tls.cert_pem, &tls.key_pem));
    info!("gRPC TLS server listening on {}", grpc_addr);

    Server::builder()
        .tls_config(tls_cfg)?
        .http2_keepalive_interval(Some(Duration::from_secs(20)))
        .http2_keepalive_timeout(Some(Duration::from_secs(10)))
        .add_service(svc_server)
        .serve_with_shutdown(grpc_addr, shutdown_signal())
        .await?;

    info!("{} server stopped cleanly", APP_NAME);
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}
