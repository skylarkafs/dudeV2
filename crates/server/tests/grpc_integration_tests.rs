use nextdude_common::proto::v1;
use nextdude_common::proto::v1::next_dude_service_client::NextDudeServiceClient;
use nextdude_common::proto::v1::next_dude_service_server::NextDudeServiceServer;
use nextdude_common::API_VERSION;
use nextdude_server::api::NextDudeGrpcService;
use nextdude_server::config::ServerConfig;
use nextdude_server::crypto::{load_or_create_tls_material, SecretBox};
use nextdude_server::discovery::DiscoveryManager;
use nextdude_server::engine::{EngineContext, ServerBroadcastEvent};
use nextdude_server::notify::NotificationManager;
use nextdude_server::probe::icmp::IcmpProber;
use nextdude_server::probe::LocalProbeExecutor;
use nextdude_server::snmp::profiles::VendorProfileRegistry;
use nextdude_server::snmp::SnmpClient;
use nextdude_server::storage::auth::create_admin;
use nextdude_server::storage::StorageHandle;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, RwLock};
use tokio_stream::wrappers::TcpListenerStream;
use tokio_stream::StreamExt;
use tonic::metadata::MetadataValue;
use tonic::transport::{Certificate, ClientTlsConfig, Endpoint, Identity, Server, ServerTlsConfig};
use tonic::{Code, Request};

static SEQ: AtomicU64 = AtomicU64::new(1);

fn authed<T>(msg: T, token: &str) -> Request<T> {
    let mut r = Request::new(msg);
    r.metadata_mut().insert(
        "authorization",
        MetadataValue::try_from(format!("Bearer {token}")).unwrap(),
    );
    r
}

#[tokio::test]
async fn in_process_grpc_tls_auth_rbac_stream_version_and_reconnect() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let base_dir = std::env::temp_dir().join(format!("nextdude_grpc_it_{}_{n}", std::process::id()));
    std::fs::create_dir_all(&base_dir).unwrap();

    let mut cfg = ServerConfig::default();
    cfg.data_dir = base_dir.clone();
    cfg.log_dir = base_dir.join("logs");

    let secrets = SecretBox::load_or_create(&cfg.master_key_path()).unwrap();
    let storage = StorageHandle::open(&cfg.db_path(), secrets).unwrap();

    storage
        .write(|c| {
            create_admin(c, "admin", "AdminPass123!", "Admin")?;
            create_admin(c, "viewer", "ViewerPass123!", "Viewer")?;
            Ok(())
        })
        .await
        .unwrap();

    let tls = load_or_create_tls_material(
        &cfg.effective_tls_cert_path(),
        &cfg.effective_tls_key_path(),
        &["127.0.0.1".to_string()],
    )
    .unwrap();

    let icmp = IcmpProber::initialize();
    let snmp = SnmpClient::new();
    let exec = Arc::new(LocalProbeExecutor::new(icmp, snmp.clone()).unwrap());
    let profiles = Arc::new(RwLock::new(VendorProfileRegistry::default()));
    let (events_tx, _) = broadcast::channel::<ServerBroadcastEvent>(256);
    let notifications = NotificationManager::new(storage.clone(), events_tx.clone());

    let ctx = EngineContext {
        config: cfg.clone(),
        storage,
        snmp,
        executor: exec,
        profiles,
        notifications,
        events_tx,
        device_host_map: Arc::new(RwLock::new(HashMap::new())),
        link_flapping: Arc::new(RwLock::new(HashMap::new())),
        down_devices: Arc::new(RwLock::new(HashSet::new())),
        active_outages: Arc::new(RwLock::new(HashMap::new())),
        tracker: nextdude_server::web_status::ChildTaskTracker::new(),
        server_started_utc: chrono::Utc::now().timestamp(),
    };

    let svc = NextDudeGrpcService::new(
        ctx,
        DiscoveryManager::default(),
        Arc::new((AtomicU64::new(0), AtomicU64::new(0))),
    );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server_tls = ServerTlsConfig::new().identity(Identity::from_pem(&tls.cert_pem, &tls.key_pem));
    let incoming = TcpListenerStream::new(listener);

    let server_handle = tokio::spawn(async move {
        let _ = Server::builder()
            .tls_config(server_tls)
            .unwrap()
            .add_service(NextDudeServiceServer::new(svc))
            .serve_with_incoming(incoming)
            .await;
    });

    let client_tls = ClientTlsConfig::new()
        .domain_name("localhost")
        .ca_certificate(Certificate::from_pem(&tls.cert_pem));
    let ch = Endpoint::from_shared(format!("https://127.0.0.1:{port}"))
        .unwrap()
        .tls_config(client_tls)
        .unwrap()
        .connect()
        .await
        .unwrap();

    let mut client = NextDudeServiceClient::new(ch);

    // 1. Viewer login & RBAC denial on SaveMap
    let v_login = client
        .login(v1::LoginRequest {
            username: "viewer".to_string(),
            password: "ViewerPass123!".to_string(),
            api_version: API_VERSION,
            client_info: "test".to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    let viewer_err = client
        .save_map(authed(
            v1::NetworkMap {
                name: "ForbiddenMap".to_string(),
                ..Default::default()
            },
            &v_login.session_token,
        ))
        .await
        .unwrap_err();
    assert_eq!(viewer_err.code(), Code::PermissionDenied);

    // 2. Admin login + Subscribe snapshot + delta stream + optimistic concurrency conflict
    let a_login = client
        .login(v1::LoginRequest {
            username: "admin".to_string(),
            password: "AdminPass123!".to_string(),
            api_version: API_VERSION,
            client_info: "test".to_string(),
        })
        .await
        .unwrap()
        .into_inner();
    let tok = a_login.session_token;

    let mut stream = client
        .subscribe(authed(v1::SubscribeRequest::default(), &tok))
        .await
        .unwrap()
        .into_inner();

    let first = stream.next().await.unwrap().unwrap();
    assert_eq!(first.seq, 1);
    assert!(matches!(
        first.payload,
        Some(v1::stream_event::Payload::Snapshot(_))
    ));

    let saved_map = client
        .save_map(authed(
            v1::NetworkMap {
                id: 0,
                version: 1,
                name: "Core Backbone".to_string(),
                ..Default::default()
            },
            &tok,
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(saved_map.version, 1);

    let delta = stream.next().await.unwrap().unwrap();
    assert_eq!(delta.seq, 2);

    // Optimistic concurrency stale version must return Aborted
    let mut stale = saved_map.clone();
    stale.version = 999;
    let conflict = client.save_map(authed(stale, &tok)).await.unwrap_err();
    assert_eq!(conflict.code(), Code::Aborted);

    // 3. Reconnect & new subscription works seamlessly
    drop(stream);
    let mut stream2 = client
        .subscribe(authed(v1::SubscribeRequest::default(), &tok))
        .await
        .unwrap()
        .into_inner();
    let snap2 = stream2.next().await.unwrap().unwrap();
    if let Some(v1::stream_event::Payload::Snapshot(s)) = snap2.payload {
        assert_eq!(s.maps.len(), 1);
    } else {
        panic!("expected snapshot on reconnect");
    }

    server_handle.abort();
    let _ = std::fs::remove_dir_all(&base_dir);
}
