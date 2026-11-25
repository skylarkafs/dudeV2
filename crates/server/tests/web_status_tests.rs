use nextdude_common::proto::v1;
use nextdude_server::api::ActiveSession;
use nextdude_server::config::ServerConfig;
use nextdude_server::crypto::SecretBox;
use nextdude_server::discovery::DiscoveryManager;
use nextdude_server::engine::{EngineContext, ServerBroadcastEvent};
use nextdude_server::notify::NotificationManager;
use nextdude_server::probe::icmp::IcmpProber;
use nextdude_server::probe::LocalProbeExecutor;
use nextdude_server::snmp::profiles::VendorProfileRegistry;
use nextdude_server::snmp::SnmpClient;
use nextdude_server::storage::{entities, StorageHandle};
use nextdude_server::web_status::{
    parse_http_request_line, parse_proc_loadavg, parse_proc_status, route_http_bytes,
    ChildState, ChildTaskTracker,
};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

static TEST_SEQ: AtomicU64 = AtomicU64::new(1);

#[test]
fn proc_parsers_and_child_tracker_track_running_children() {
    let sample_status = "\
Name:\tnextdude-server
VmRSS:\t  49152 kB
Threads:\t12
";
    assert_eq!(parse_proc_status(sample_status), (Some(49152), Some(12)));
    assert_eq!(
        parse_proc_loadavg("0.42 0.35 0.28 2/180 12345"),
        (Some(0.42), Some(0.35), Some(0.28))
    );

    let tracker = ChildTaskTracker::new();
    tracker.mark_running("polling_scheduler", "Monitoring Polling Engine");
    tracker.mark_running("syslog_receiver", "UDP 514 Syslog Ingestion");
    tracker.mark_stopped("mndp_listener", "UDP 5678 MNDP Discovery", "disabled");

    {
        let _p1 = tracker.acquire_probe_guard();
        let _p2 = tracker.acquire_probe_guard();
        let _d1 = tracker.acquire_discovery_guard();
        let _t1 = tracker.acquire_tool_guard();
        assert_eq!(tracker.active_counts(), (2, 2, 1, 1));
    }
    assert_eq!(tracker.active_counts(), (2, 0, 0, 0));

    tracker.mark_restarting("syslog_receiver", true);
    let children = tracker.snapshot_children();
    let syslog = children
        .iter()
        .find(|c| c.name == "syslog_receiver")
        .unwrap();
    assert_eq!(syslog.state, ChildState::Restarting);
    assert_eq!(syslog.restarts, 1);

    tracker.record_poll_cycle(37);
    assert_eq!(tracker.engine_counters(), (1, 1, 37));
}

#[tokio::test]
async fn web_status_routes_serve_html_stats_and_healthz() {
    let n = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
    let base_dir =
        std::env::temp_dir().join(format!("nextdude_web_status_{}_{n}", std::process::id()));
    std::fs::create_dir_all(&base_dir).unwrap();

    let cfg = ServerConfig {
        data_dir: base_dir.clone(),
        log_dir: base_dir.join("logs"),
        ..Default::default()
    };

    let storage = StorageHandle::open(&cfg.db_path(), SecretBox::from_raw_key([0x11; 32])).unwrap();
    storage
        .write(|c| {
            let map = entities::save_map(
                c,
                &v1::NetworkMap {
                    id: 0,
                    version: 1,
                    name: "NOC Map".to_string(),
                    ..Default::default()
                },
            )?;
            let mut dev = v1::Device {
                id: 0,
                version: 1,
                map_id: map.id,
                name: "SecretDeviceNameNotExposed".to_string(),
                address: "10.99.99.1".to_string(),
                device_type_id: 1,
                aggregate_state: v1::ProbeState::Up as i32,
                ..Default::default()
            };
            dev = entities::save_device(c, &dev)?;
            c.execute(
                "UPDATE devices SET aggregate_state=?1 WHERE id=?2",
                rusqlite::params![v1::ProbeState::Up as i32, dev.id],
            )
            .unwrap();
            Ok(())
        })
        .await
        .unwrap();

    let icmp = IcmpProber::initialize();
    let snmp = SnmpClient::new();
    let exec = Arc::new(LocalProbeExecutor::new(icmp, snmp.clone()).unwrap());
    let (events_tx, _) = broadcast::channel::<ServerBroadcastEvent>(64);
    let notifications = NotificationManager::new(storage.clone(), events_tx.clone());
    let tracker = ChildTaskTracker::new();
    tracker.mark_running("polling_scheduler", "Monitoring Polling Engine");
    tracker.mark_running("status_web", "HTTP Status Page");
    let _probe_guard = tracker.acquire_probe_guard();

    let ctx = EngineContext {
        config: cfg,
        storage,
        snmp,
        executor: exec,
        profiles: Arc::new(RwLock::new(VendorProfileRegistry::default())),
        notifications,
        events_tx,
        device_host_map: Arc::new(RwLock::new(HashMap::new())),
        link_flapping: Arc::new(RwLock::new(HashMap::new())),
        down_devices: Arc::new(RwLock::new(HashSet::new())),
        active_outages: Arc::new(RwLock::new(HashMap::new())),
        tracker,
        server_started_utc: chrono::Utc::now().timestamp() - 45,
    };

    let mut sess_map = HashMap::new();
    sess_map.insert(
        "tok-1".to_string(),
        ActiveSession {
            session_id: "sess-1".to_string(),
            token: "tok-1".to_string(),
            admin_id: 1,
            username: "SecretAdminNotExposed".to_string(),
            permissions: vec!["read_monitoring".to_string()],
            remote_addr: "127.0.0.1".to_string(),
            client_info: "test".to_string(),
            connected_utc: 1,
            last_active_utc: 1,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            revoked: false,
        },
    );
    let sessions = Arc::new(RwLock::new(sess_map));
    let discovery = DiscoveryManager::default();
    let syslog_stats = Arc::new((AtomicU64::new(42), AtomicU64::new(3)));

    assert_eq!(
        parse_http_request_line(b"GET /api/stats?x=1 HTTP/1.1\r\nHost: localhost\r\n\r\n"),
        Some(("GET", "/api/stats?x=1"))
    );

    let html_resp =
        route_http_bytes(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n", &ctx, &sessions, &discovery, &syslog_stats).await;
    let html_str = String::from_utf8_lossy(&html_resp);
    assert!(html_str.starts_with("HTTP/1.1 200 OK"));
    assert!(html_str.contains("X-Content-Type-Options: nosniff"));
    assert!(html_str.contains("X-Frame-Options: DENY"));
    assert!(html_str.contains("Children Running"));

    let stats_resp = route_http_bytes(
        b"GET /api/stats HTTP/1.1\r\nHost: localhost\r\n\r\n",
        &ctx,
        &sessions,
        &discovery,
        &syslog_stats,
    )
    .await;
    let stats_str = String::from_utf8_lossy(&stats_resp);
    let body = stats_str.split("\r\n\r\n").nth(1).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(parsed["running_children_total"], 3);
    assert_eq!(parsed["supervised_tasks_running"], 2);
    assert_eq!(parsed["active_probe_tasks"], 1);
    assert_eq!(parsed["active_client_sessions"], 1);
    assert_eq!(parsed["syslog_received"], 42);
    assert_eq!(parsed["syslog_dropped"], 3);
    assert_eq!(parsed["monitoring"]["devices_up"], 1);
    assert!(!stats_str.contains("SecretDeviceNameNotExposed"));
    assert!(!stats_str.contains("SecretAdminNotExposed"));
    assert!(!stats_str.contains("10.99.99.1"));

    let head_resp = route_http_bytes(
        b"HEAD /healthz HTTP/1.1\r\nHost: localhost\r\n\r\n",
        &ctx,
        &sessions,
        &discovery,
        &syslog_stats,
    )
    .await;
    let head_str = String::from_utf8_lossy(&head_resp);
    assert!(head_str.starts_with("HTTP/1.1 200 OK"));
    assert!(head_str.ends_with("\r\n\r\n"));

    let bad_method = route_http_bytes(
        b"POST /api/stats HTTP/1.1\r\nHost: localhost\r\n\r\n",
        &ctx,
        &sessions,
        &discovery,
        &syslog_stats,
    )
    .await;
    assert!(String::from_utf8_lossy(&bad_method).starts_with("HTTP/1.1 405 Method Not Allowed"));

    let _ = std::fs::remove_dir_all(&base_dir);
}
