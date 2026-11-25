use crate::tofu::{TofuDecision, TofuVerifier};
use nextdude_common::proto::v1;
use nextdude_common::proto::v1::next_dude_service_client::NextDudeServiceClient;
use nextdude_common::API_VERSION;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;
use tonic::codec::CompressionEncoding;
use tonic::metadata::MetadataValue;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::Request;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionPhase {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting(u32),
}

#[derive(Debug, Clone)]
pub struct ConnectParams {
    pub server_id: String,
    pub server_name: String,
    pub address: String,
    pub port: u16,
    pub username: String,
    pub password: String,
}

#[derive(Debug)]
pub enum ClientCommand {
    Connect(ConnectParams),
    Disconnect,
    TrustFingerprint { host_key: String, fingerprint: String },
    UpdateSubscription(v1::SubscribeRequest),
    SaveUserPreferences(v1::UserPreferences),
    SaveMap(v1::NetworkMap),
    DeleteMap(v1::IdRequest),
    AutoLayoutMap(v1::AutoLayoutRequest),
    CopyPasteItems(v1::CopyPasteRequest),
    Undo,
    Redo,
    SaveDevice(v1::Device),
    DeleteDevice(v1::IdRequest),
    SaveService(v1::DeviceService),
    DeleteService(v1::IdRequest),
    SaveProbe(v1::ProbeDefinition),
    DeleteProbe(v1::IdRequest),
    SaveDeviceType(v1::DeviceType),
    DeleteDeviceType(v1::IdRequest),
    SaveLinkType(v1::LinkType),
    DeleteLinkType(v1::IdRequest),
    SaveLink(v1::NetworkLink),
    DeleteLink(v1::IdRequest),
    SaveNetwork(v1::MapNetwork),
    DeleteNetwork(v1::IdRequest),
    SaveSubmap(v1::SubmapItem),
    DeleteSubmap(v1::IdRequest),
    SaveStaticItem(v1::StaticItem),
    DeleteStaticItem(v1::IdRequest),
    SaveDependency(v1::Dependency),
    DeleteDependency(v1::IdRequest),
    AcknowledgeAlert(i64),
    SaveAlertRule(v1::AlertRule),
    DeleteAlertRule(v1::IdRequest),
    SaveMaintenanceWindow(v1::MaintenanceWindow),
    DeleteMaintenanceWindow(v1::IdRequest),
    SaveNotificationChannel(v1::NotificationChannel),
    DeleteNotificationChannel(v1::IdRequest),
    TestNotificationChannel(i64),
    StartDiscovery(v1::StartDiscoveryRequest),
    CancelDiscovery(String),
    ImportDiscoveryCandidates(v1::ImportDiscoveryRequest),
    RunTool(v1::RunToolRequest),
    SaveChart(v1::ChartDefinition),
    DeleteChart(v1::IdRequest),
    QueryChartSeries(v1::QueryChartSeriesRequest),
    QueryEvents(v1::PageQueryRequest),
    QuerySyslog(v1::PageQueryRequest),
    QueryTraps(v1::PageQueryRequest),
    QueryOutages(v1::PageQueryRequest),
    QueryHistoryActions(v1::PageQueryRequest),
    UploadImage(v1::UploadImageRequest),
    ImportImageFromUrl(v1::ImportImageUrlRequest),
    DeleteImage(v1::IdRequest),
    GetImageBlob(v1::GetImageBlobRequest),
    ListAdmins,
    SaveAdmin(v1::AdminUser),
    DeleteAdmin(v1::IdRequest),
    SaveAdminGroup(v1::AdminGroup),
    DeleteAdminGroup(v1::IdRequest),
    ListActiveAdmins,
    KickAdminSession(String),
    UpdateServerSettings(v1::ServerSettings),
    ListVendorProfiles,
}

#[derive(Debug)]
pub enum ClientEvent {
    PhaseChanged(ConnectionPhase),
    TofuPrompt(TofuDecision),
    LoggedIn(v1::LoginResponse),
    StreamFrame(v1::StreamEvent),
    ToolChunk(v1::ToolOutputChunk),
    ChartSeriesLoaded(v1::QueryChartSeriesResponse),
    EventsLoaded(v1::QueryEventsResponse),
    SyslogLoaded(v1::QuerySyslogResponse),
    TrapsLoaded(v1::QueryTrapsResponse),
    OutagesLoaded(v1::QueryOutagesResponse),
    HistoryLoaded(v1::QueryHistoryActionsResponse),
    ImageBlobLoaded {
        image_id: i64,
        variant: String,
        mime_type: String,
        data: Vec<u8>,
    },
    AdminsLoaded(v1::ListAdminsResponse),
    ActiveAdminsLoaded(v1::ListActiveAdminsResponse),
    VendorProfilesLoaded(v1::ListVendorProfilesResponse),
    InfoNotice(String),
    ErrorNotice(String),
}

pub type RepaintCallback = Arc<dyn Fn() + Send + Sync>;

pub struct ClientWorkerHandle {
    pub cmd_tx: mpsc::Sender<ClientCommand>,
    pub event_rx: mpsc::Receiver<ClientEvent>,
}

pub fn spawn_client_worker(
    initial_fingerprints: HashMap<String, String>,
    repaint: RepaintCallback,
) -> ClientWorkerHandle {
    let (cmd_tx, cmd_rx) = mpsc::channel::<ClientCommand>(256);
    let (event_tx, event_rx) = mpsc::channel::<ClientEvent>(1024);

    std::thread::Builder::new()
        .name("nextdude-client-net".to_string())
        .spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .worker_threads(2)
                .build()
            else {
                return;
            };
            rt.block_on(worker_loop(cmd_rx, event_tx, initial_fingerprints, repaint));
        })
        .ok();

    ClientWorkerHandle { cmd_tx, event_rx }
}

fn emit(tx: &mpsc::Sender<ClientEvent>, repaint: &RepaintCallback, ev: ClientEvent) {
    let _ = tx.try_send(ev);
    repaint();
}

async fn worker_loop(
    mut cmd_rx: mpsc::Receiver<ClientCommand>,
    event_tx: mpsc::Sender<ClientEvent>,
    initial_fingerprints: HashMap<String, String>,
    repaint: RepaintCallback,
) {
    let known_fps = Arc::new(RwLock::new(initial_fingerprints));
    let mut active_params: Option<ConnectParams> = None;
    let mut active_client: Option<NextDudeServiceClient<Channel>> = None;
    let mut active_token: Option<String> = None;
    let mut current_sub = v1::SubscribeRequest {
        active_map_id: None,
        active_device_id: None,
        active_chart_ids: Vec::new(),
        include_events: true,
        include_syslog: true,
        include_traps: true,
        include_outages: true,
    };

    let (stream_tx, mut stream_rx) = mpsc::channel::<Result<v1::StreamEvent, String>>(512);
    let mut stream_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut reconnect_attempt = 0u32;
    let mut reconnect_deadline: Option<tokio::time::Instant> = None;

    loop {
        tokio::select! {
            maybe_cmd = cmd_rx.recv() => {
                let Some(cmd) = maybe_cmd else { break };
                match cmd {
                    ClientCommand::Connect(params) => {
                        if let Some(h) = stream_task.take() {
                            h.abort();
                        }
                        active_params = Some(params.clone());
                        reconnect_attempt = 0;
                        reconnect_deadline = None;
                        emit(&event_tx, &repaint, ClientEvent::PhaseChanged(ConnectionPhase::Connecting));
                        match establish_session(&params, &known_fps, &current_sub, &stream_tx).await {
                            Ok((client, login_resp, task)) => {
                                active_client = Some(client);
                                active_token = Some(login_resp.session_token.clone());
                                stream_task = Some(task);
                                emit(&event_tx, &repaint, ClientEvent::LoggedIn(login_resp));
                                emit(&event_tx, &repaint, ClientEvent::PhaseChanged(ConnectionPhase::Connected));
                            }
                            Err(ConnectError::Tofu(decision)) => {
                                active_client = None;
                                active_token = None;
                                emit(&event_tx, &repaint, ClientEvent::PhaseChanged(ConnectionPhase::Disconnected));
                                emit(&event_tx, &repaint, ClientEvent::TofuPrompt(decision));
                            }
                            Err(ConnectError::Failed(msg)) => {
                                active_client = None;
                                active_token = None;
                                emit(&event_tx, &repaint, ClientEvent::ErrorNotice(msg));
                                emit(&event_tx, &repaint, ClientEvent::PhaseChanged(ConnectionPhase::Disconnected));
                            }
                        }
                    }
                    ClientCommand::TrustFingerprint { host_key, fingerprint } => {
                        if let Ok(mut g) = known_fps.write() {
                            g.insert(host_key, fingerprint);
                        }
                        if let Some(params) = active_params.clone() {
                            emit(&event_tx, &repaint, ClientEvent::PhaseChanged(ConnectionPhase::Connecting));
                            match establish_session(&params, &known_fps, &current_sub, &stream_tx).await {
                                Ok((client, login_resp, task)) => {
                                    active_client = Some(client);
                                    active_token = Some(login_resp.session_token.clone());
                                    stream_task = Some(task);
                                    emit(&event_tx, &repaint, ClientEvent::LoggedIn(login_resp));
                                    emit(&event_tx, &repaint, ClientEvent::PhaseChanged(ConnectionPhase::Connected));
                                }
                                Err(ConnectError::Tofu(d)) => {
                                    emit(&event_tx, &repaint, ClientEvent::TofuPrompt(d));
                                }
                                Err(ConnectError::Failed(msg)) => {
                                    emit(&event_tx, &repaint, ClientEvent::ErrorNotice(msg));
                                    emit(&event_tx, &repaint, ClientEvent::PhaseChanged(ConnectionPhase::Disconnected));
                                }
                            }
                        }
                    }
                    ClientCommand::Disconnect => {
                        if let (Some(mut c), Some(tok)) = (active_client.take(), active_token.take()) {
                            let _ = c.logout(authed_req(v1::Empty {}, &tok)).await;
                        }
                        if let Some(h) = stream_task.take() {
                            h.abort();
                        }
                        active_params = None;
                        reconnect_deadline = None;
                        emit(&event_tx, &repaint, ClientEvent::PhaseChanged(ConnectionPhase::Disconnected));
                    }
                    ClientCommand::UpdateSubscription(sub) => {
                        current_sub = sub.clone();
                        if let (Some(c), Some(tok)) = (active_client.clone(), active_token.clone()) {
                            if let Some(h) = stream_task.take() {
                                h.abort();
                            }
                            match start_subscription(c, tok, sub, stream_tx.clone()).await {
                                Ok(h) => stream_task = Some(h),
                                Err(e) => emit(&event_tx, &repaint, ClientEvent::ErrorNotice(e)),
                            }
                        }
                    }
                    other => {
                        if let (Some(client), Some(token)) = (active_client.clone(), active_token.clone()) {
                            let ev_tx = event_tx.clone();
                            let rep = Arc::clone(&repaint);
                            tokio::spawn(async move {
                                handle_rpc_command(client, token, other, ev_tx, rep).await;
                            });
                        } else {
                            emit(&event_tx, &repaint, ClientEvent::ErrorNotice("Not connected to server".to_string()));
                        }
                    }
                }
            }
            maybe_frame = stream_rx.recv() => {
                match maybe_frame {
                    Some(Ok(frame)) => {
                        let is_resync = matches!(
                            frame.payload,
                            Some(v1::stream_event::Payload::ResyncRequired(_))
                        );
                        emit(&event_tx, &repaint, ClientEvent::StreamFrame(frame));
                        if is_resync {
                            if let (Some(c), Some(tok)) = (active_client.clone(), active_token.clone()) {
                                if let Some(h) = stream_task.take() {
                                    h.abort();
                                }
                                if let Ok(h) = start_subscription(c, tok, current_sub.clone(), stream_tx.clone()).await {
                                    stream_task = Some(h);
                                }
                            }
                        }
                    }
                    Some(Err(err_msg)) => {
                        if active_params.is_some() {
                            active_client = None;
                            active_token = None;
                            reconnect_attempt = reconnect_attempt.saturating_add(1);
                            let delay_sec = (1u64 << reconnect_attempt.min(5)).min(30);
                            reconnect_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(delay_sec));
                            emit(&event_tx, &repaint, ClientEvent::PhaseChanged(ConnectionPhase::Reconnecting(reconnect_attempt)));
                            emit(&event_tx, &repaint, ClientEvent::ErrorNotice(format!("Stream lost: {err_msg}; reconnecting in {delay_sec}s")));
                        }
                    }
                    None => break,
                }
            }
            _ = async {
                match reconnect_deadline {
                    Some(dl) => tokio::time::sleep_until(dl).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                reconnect_deadline = None;
                if let Some(params) = active_params.clone() {
                    match establish_session(&params, &known_fps, &current_sub, &stream_tx).await {
                        Ok((client, login_resp, task)) => {
                            active_client = Some(client);
                            active_token = Some(login_resp.session_token.clone());
                            stream_task = Some(task);
                            reconnect_attempt = 0;
                            emit(&event_tx, &repaint, ClientEvent::LoggedIn(login_resp));
                            emit(&event_tx, &repaint, ClientEvent::PhaseChanged(ConnectionPhase::Connected));
                        }
                        Err(ConnectError::Tofu(d)) => {
                            emit(&event_tx, &repaint, ClientEvent::TofuPrompt(d));
                            emit(&event_tx, &repaint, ClientEvent::PhaseChanged(ConnectionPhase::Disconnected));
                        }
                        Err(ConnectError::Failed(msg)) => {
                            reconnect_attempt = reconnect_attempt.saturating_add(1);
                            let delay_sec = (1u64 << reconnect_attempt.min(5)).min(30);
                            reconnect_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(delay_sec));
                            emit(&event_tx, &repaint, ClientEvent::PhaseChanged(ConnectionPhase::Reconnecting(reconnect_attempt)));
                            emit(&event_tx, &repaint, ClientEvent::ErrorNotice(msg));
                        }
                    }
                }
            }
        }
    }
}

enum ConnectError {
    Tofu(TofuDecision),
    Failed(String),
}

async fn establish_session(
    params: &ConnectParams,
    known_fps: &Arc<RwLock<HashMap<String, String>>>,
    sub: &v1::SubscribeRequest,
    stream_tx: &mpsc::Sender<Result<v1::StreamEvent, String>>,
) -> Result<(NextDudeServiceClient<Channel>, v1::LoginResponse, tokio::task::JoinHandle<()>), ConnectError> {
    let host_key = format!("{}:{}", params.address, params.port);
    let verifier = Arc::new(TofuVerifier::new(host_key, Arc::clone(known_fps)));
    let mut rustls_cfg = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();
    rustls_cfg.alpn_protocols = vec![b"h2".to_vec()];

    let tls_cfg = ClientTlsConfig::new().rustls_client_config(rustls_cfg);
    let uri = format!("https://{}:{}", params.address, params.port);
    let endpoint = Endpoint::from_shared(uri)
        .map_err(|e| ConnectError::Failed(format!("invalid endpoint URI: {e}")))?
        .tls_config(tls_cfg)
        .map_err(|e| ConnectError::Failed(format!("TLS config failed: {e}")))?
        .connect_timeout(Duration::from_secs(8))
        .http2_keep_alive_interval(Duration::from_secs(20))
        .keep_alive_timeout(Duration::from_secs(10));

    let channel = match endpoint.connect().await {
        Ok(ch) => ch,
        Err(e) => {
            if let Some(dec) = verifier.take_last_decision() {
                if matches!(dec, TofuDecision::FirstUse { .. } | TofuDecision::Mismatch { .. }) {
                    return Err(ConnectError::Tofu(dec));
                }
            }
            return Err(ConnectError::Failed(format!("connection failed: {e}")));
        }
    };

    let mut client = NextDudeServiceClient::new(channel)
        .send_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Gzip)
        .max_decoding_message_size(32 * 1024 * 1024);

    let login_resp = client
        .login(v1::LoginRequest {
            username: params.username.clone(),
            password: params.password.clone(),
            api_version: API_VERSION,
            client_info: format!("{} Windows Client", nextdude_common::APP_NAME),
        })
        .await
        .map_err(|s| ConnectError::Failed(s.message().to_string()))?
        .into_inner();

    let mut effective_sub = sub.clone();
    if effective_sub.active_map_id.is_none() {
        effective_sub.active_map_id = login_resp.preferences.as_ref().and_then(|p| p.last_map_id);
    }

    let task = start_subscription(
        client.clone(),
        login_resp.session_token.clone(),
        effective_sub,
        stream_tx.clone(),
    )
    .await
    .map_err(ConnectError::Failed)?;

    Ok((client, login_resp, task))
}

async fn start_subscription(
    mut client: NextDudeServiceClient<Channel>,
    token: String,
    sub: v1::SubscribeRequest,
    stream_tx: mpsc::Sender<Result<v1::StreamEvent, String>>,
) -> Result<tokio::task::JoinHandle<()>, String> {
    let req = authed_req(sub, &token).map_err(|s| s.message().to_string())?;
    let mut stream = client
        .subscribe(req)
        .await
        .map_err(|s| s.message().to_string())?
        .into_inner();

    Ok(tokio::spawn(async move {
        loop {
            match stream.next().await {
                Some(Ok(ev)) => {
                    if stream_tx.send(Ok(ev)).await.is_err() {
                        break;
                    }
                }
                Some(Err(status)) => {
                    let _ = stream_tx.send(Err(status.message().to_string())).await;
                    break;
                }
                None => {
                    let _ = stream_tx
                        .send(Err("server closed subscription stream".to_string()))
                        .await;
                    break;
                }
            }
        }
    }))
}

fn authed_req<T>(body: T, token: &str) -> Result<Request<T>, tonic::Status> {
    let mut req = Request::new(body);
    let val = MetadataValue::try_from(format!("Bearer {token}"))
        .map_err(|_| tonic::Status::internal("invalid token metadata"))?;
    req.metadata_mut().insert("authorization", val);
    Ok(req)
}

async fn handle_rpc_command(
    mut client: NextDudeServiceClient<Channel>,
    token: String,
    cmd: ClientCommand,
    ev_tx: mpsc::Sender<ClientEvent>,
    repaint: RepaintCallback,
) {
    let report_err = |st: tonic::Status| {
        emit(&ev_tx, &repaint, ClientEvent::ErrorNotice(st.message().to_string()));
    };

    match cmd {
        ClientCommand::SaveUserPreferences(p) => {
            if let Ok(req) = authed_req(p, &token) {
                if let Err(e) = client.save_user_preferences(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::SaveMap(m) => {
            if let Ok(req) = authed_req(m, &token) {
                if let Err(e) = client.save_map(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::DeleteMap(id) => {
            if let Ok(req) = authed_req(id, &token) {
                if let Err(e) = client.delete_map(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::AutoLayoutMap(r) => {
            if let Ok(req) = authed_req(r, &token) {
                if let Err(e) = client.auto_layout_map(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::CopyPasteItems(r) => {
            if let Ok(req) = authed_req(r, &token) {
                if let Err(e) = client.copy_paste_items(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::Undo => {
            if let Ok(req) = authed_req(v1::Empty {}, &token) {
                match client.undo(req).await {
                    Ok(r) => emit(&ev_tx, &repaint, ClientEvent::InfoNotice(r.into_inner().description)),
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::Redo => {
            if let Ok(req) = authed_req(v1::Empty {}, &token) {
                match client.redo(req).await {
                    Ok(r) => emit(&ev_tx, &repaint, ClientEvent::InfoNotice(r.into_inner().description)),
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::SaveDevice(d) => {
            if let Ok(req) = authed_req(d, &token) {
                if let Err(e) = client.save_device(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::DeleteDevice(id) => {
            if let Ok(req) = authed_req(id, &token) {
                if let Err(e) = client.delete_device(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::SaveService(s) => {
            if let Ok(req) = authed_req(s, &token) {
                if let Err(e) = client.save_service(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::DeleteService(id) => {
            if let Ok(req) = authed_req(id, &token) {
                if let Err(e) = client.delete_service(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::SaveProbe(p) => {
            if let Ok(req) = authed_req(p, &token) {
                if let Err(e) = client.save_probe(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::DeleteProbe(id) => {
            if let Ok(req) = authed_req(id, &token) {
                if let Err(e) = client.delete_probe(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::SaveDeviceType(dt) => {
            if let Ok(req) = authed_req(dt, &token) {
                if let Err(e) = client.save_device_type(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::DeleteDeviceType(id) => {
            if let Ok(req) = authed_req(id, &token) {
                if let Err(e) = client.delete_device_type(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::SaveLinkType(lt) => {
            if let Ok(req) = authed_req(lt, &token) {
                if let Err(e) = client.save_link_type(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::DeleteLinkType(id) => {
            if let Ok(req) = authed_req(id, &token) {
                if let Err(e) = client.delete_link_type(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::SaveLink(l) => {
            if let Ok(req) = authed_req(l, &token) {
                if let Err(e) = client.save_link(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::DeleteLink(id) => {
            if let Ok(req) = authed_req(id, &token) {
                if let Err(e) = client.delete_link(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::SaveNetwork(n) => {
            if let Ok(req) = authed_req(n, &token) {
                if let Err(e) = client.save_network(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::DeleteNetwork(id) => {
            if let Ok(req) = authed_req(id, &token) {
                if let Err(e) = client.delete_network(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::SaveSubmap(s) => {
            if let Ok(req) = authed_req(s, &token) {
                if let Err(e) = client.save_submap(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::DeleteSubmap(id) => {
            if let Ok(req) = authed_req(id, &token) {
                if let Err(e) = client.delete_submap(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::SaveStaticItem(s) => {
            if let Ok(req) = authed_req(s, &token) {
                if let Err(e) = client.save_static_item(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::DeleteStaticItem(id) => {
            if let Ok(req) = authed_req(id, &token) {
                if let Err(e) = client.delete_static_item(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::SaveDependency(d) => {
            if let Ok(req) = authed_req(d, &token) {
                if let Err(e) = client.save_dependency(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::DeleteDependency(id) => {
            if let Ok(req) = authed_req(id, &token) {
                if let Err(e) = client.delete_dependency(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::AcknowledgeAlert(event_id) => {
            if let Ok(req) = authed_req(v1::AcknowledgeAlertRequest { event_id }, &token) {
                if let Err(e) = client.acknowledge_alert(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::SaveAlertRule(r) => {
            if let Ok(req) = authed_req(r, &token) {
                if let Err(e) = client.save_alert_rule(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::DeleteAlertRule(id) => {
            if let Ok(req) = authed_req(id, &token) {
                if let Err(e) = client.delete_alert_rule(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::SaveMaintenanceWindow(mw) => {
            if let Ok(req) = authed_req(mw, &token) {
                if let Err(e) = client.save_maintenance_window(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::DeleteMaintenanceWindow(id) => {
            if let Ok(req) = authed_req(id, &token) {
                if let Err(e) = client.delete_maintenance_window(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::SaveNotificationChannel(ch) => {
            if let Ok(req) = authed_req(ch, &token) {
                if let Err(e) = client.save_notification_channel(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::DeleteNotificationChannel(id) => {
            if let Ok(req) = authed_req(id, &token) {
                if let Err(e) = client.delete_notification_channel(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::TestNotificationChannel(id) => {
            if let Ok(req) = authed_req(v1::IdRequest { id, expected_version: 0 }, &token) {
                match client.test_notification_channel(req).await {
                    Ok(_) => emit(&ev_tx, &repaint, ClientEvent::InfoNotice("Test notification sent".to_string())),
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::StartDiscovery(r) => {
            if let Ok(req) = authed_req(r, &token) {
                match client.start_discovery(req).await {
                    Ok(res) => emit(&ev_tx, &repaint, ClientEvent::InfoNotice(format!("Discovery started: {}", res.into_inner().job_id))),
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::CancelDiscovery(job_id) => {
            if let Ok(req) = authed_req(v1::StartDiscoveryResponse { job_id }, &token) {
                let _ = client.cancel_discovery(req).await;
            }
        }
        ClientCommand::ImportDiscoveryCandidates(r) => {
            if let Ok(req) = authed_req(r, &token) {
                match client.import_discovery_candidates(req).await {
                    Ok(res) => {
                        let r = res.into_inner();
                        emit(&ev_tx, &repaint, ClientEvent::InfoNotice(format!("Imported {} devices, {} links", r.imported_devices, r.imported_links)));
                    }
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::RunTool(r) => {
            if let Ok(req) = authed_req(r, &token) {
                match client.run_tool(req).await {
                    Ok(resp) => {
                        let mut stream = resp.into_inner();
                        while let Some(item) = stream.next().await {
                            match item {
                                Ok(chunk) => emit(&ev_tx, &repaint, ClientEvent::ToolChunk(chunk)),
                                Err(e) => {
                                    report_err(e);
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::SaveChart(c) => {
            if let Ok(req) = authed_req(c, &token) {
                if let Err(e) = client.save_chart(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::DeleteChart(id) => {
            if let Ok(req) = authed_req(id, &token) {
                if let Err(e) = client.delete_chart(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::QueryChartSeries(q) => {
            if let Ok(req) = authed_req(q, &token) {
                match client.query_chart_series(req).await {
                    Ok(res) => emit(&ev_tx, &repaint, ClientEvent::ChartSeriesLoaded(res.into_inner())),
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::QueryEvents(q) => {
            if let Ok(req) = authed_req(q, &token) {
                match client.query_events(req).await {
                    Ok(res) => emit(&ev_tx, &repaint, ClientEvent::EventsLoaded(res.into_inner())),
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::QuerySyslog(q) => {
            if let Ok(req) = authed_req(q, &token) {
                match client.query_syslog(req).await {
                    Ok(res) => emit(&ev_tx, &repaint, ClientEvent::SyslogLoaded(res.into_inner())),
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::QueryTraps(q) => {
            if let Ok(req) = authed_req(q, &token) {
                match client.query_traps(req).await {
                    Ok(res) => emit(&ev_tx, &repaint, ClientEvent::TrapsLoaded(res.into_inner())),
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::QueryOutages(q) => {
            if let Ok(req) = authed_req(q, &token) {
                match client.query_outages(req).await {
                    Ok(res) => emit(&ev_tx, &repaint, ClientEvent::OutagesLoaded(res.into_inner())),
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::QueryHistoryActions(q) => {
            if let Ok(req) = authed_req(q, &token) {
                match client.query_history_actions(req).await {
                    Ok(res) => emit(&ev_tx, &repaint, ClientEvent::HistoryLoaded(res.into_inner())),
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::UploadImage(u) => {
            if let Ok(req) = authed_req(u, &token) {
                match client.upload_image(req).await {
                    Ok(_) => emit(&ev_tx, &repaint, ClientEvent::InfoNotice("Image uploaded".to_string())),
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::ImportImageFromUrl(u) => {
            if let Ok(req) = authed_req(u, &token) {
                match client.import_image_from_url(req).await {
                    Ok(_) => emit(&ev_tx, &repaint, ClientEvent::InfoNotice("Image imported from URL".to_string())),
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::DeleteImage(id) => {
            if let Ok(req) = authed_req(id, &token) {
                if let Err(e) = client.delete_image(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::GetImageBlob(q) => {
            let iid = q.image_id;
            let var = q.variant.clone();
            if let Ok(req) = authed_req(q, &token) {
                match client.get_image_blob(req).await {
                    Ok(res) => {
                        let inner = res.into_inner();
                        emit(
                            &ev_tx,
                            &repaint,
                            ClientEvent::ImageBlobLoaded {
                                image_id: iid,
                                variant: var,
                                mime_type: inner.mime_type,
                                data: inner.data,
                            },
                        );
                    }
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::ListAdmins => {
            if let Ok(req) = authed_req(v1::Empty {}, &token) {
                match client.list_admins(req).await {
                    Ok(res) => emit(&ev_tx, &repaint, ClientEvent::AdminsLoaded(res.into_inner())),
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::SaveAdmin(u) => {
            if let Ok(req) = authed_req(u, &token) {
                if let Err(e) = client.save_admin(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::DeleteAdmin(id) => {
            if let Ok(req) = authed_req(id, &token) {
                if let Err(e) = client.delete_admin(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::SaveAdminGroup(g) => {
            if let Ok(req) = authed_req(g, &token) {
                if let Err(e) = client.save_admin_group(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::DeleteAdminGroup(id) => {
            if let Ok(req) = authed_req(id, &token) {
                if let Err(e) = client.delete_admin_group(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::ListActiveAdmins => {
            if let Ok(req) = authed_req(v1::Empty {}, &token) {
                match client.list_active_admins(req).await {
                    Ok(res) => emit(&ev_tx, &repaint, ClientEvent::ActiveAdminsLoaded(res.into_inner())),
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::KickAdminSession(session_id) => {
            if let Ok(req) = authed_req(v1::KickSessionRequest { session_id }, &token) {
                if let Err(e) = client.kick_admin_session(req).await {
                    report_err(e);
                }
            }
        }
        ClientCommand::UpdateServerSettings(s) => {
            if let Ok(req) = authed_req(s, &token) {
                match client.update_server_settings(req).await {
                    Ok(_) => emit(&ev_tx, &repaint, ClientEvent::InfoNotice("Server settings saved".to_string())),
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::ListVendorProfiles => {
            if let Ok(req) = authed_req(v1::Empty {}, &token) {
                match client.list_vendor_profiles(req).await {
                    Ok(res) => emit(&ev_tx, &repaint, ClientEvent::VendorProfilesLoaded(res.into_inner())),
                    Err(e) => report_err(e),
                }
            }
        }
        ClientCommand::Connect(_)
        | ClientCommand::Disconnect
        | ClientCommand::TrustFingerprint { .. }
        | ClientCommand::UpdateSubscription(_) => {}
    }
}
