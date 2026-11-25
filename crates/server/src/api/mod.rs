use crate::crypto::generate_session_token;
use crate::discovery::{auto_layout_map, DiscoveryManager};
use crate::engine::{EngineContext, ServerBroadcastEvent};
use crate::error::ServerError;
use crate::images::{
    build_image_meta, delete_stored_image_files, download_image_https, load_stored_image_blob,
    store_processed_image, validate_and_process_image,
};
use crate::storage::auth::{
    authenticate_admin, list_admins_and_groups, load_user_preferences, record_history_action,
    save_admin_group, save_admin_user, save_user_preferences,
};
use crate::storage::entities::{self, delete_entity_versioned};
use crate::storage::telemetry;
use crate::tools::execute_tool_stream;
use nextdude_common::proto::has_permission;
use nextdude_common::proto::v1;
use nextdude_common::proto::v1::next_dude_service_server::NextDudeService;
use nextdude_common::types::{ImageSizeVariant, Permission};
use nextdude_common::{API_VERSION, APP_NAME};
use std::collections::{HashMap, HashSet};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;
use tonic::{Request, Response, Status};

#[derive(Debug, Clone)]
pub enum UndoPayload {
    Device {
        before: Option<v1::Device>,
        after: Option<v1::Device>,
    },
    Link {
        before: Option<v1::NetworkLink>,
        after: Option<v1::NetworkLink>,
    },
    Map {
        before: Option<v1::NetworkMap>,
        after: Option<v1::NetworkMap>,
    },
    Network {
        before: Option<v1::MapNetwork>,
        after: Option<v1::MapNetwork>,
    },
    Submap {
        before: Option<v1::SubmapItem>,
        after: Option<v1::SubmapItem>,
    },
    StaticItem {
        before: Option<v1::StaticItem>,
        after: Option<v1::StaticItem>,
    },
}

#[derive(Debug, Clone)]
pub struct UndoEntry {
    pub description: String,
    pub payload: UndoPayload,
}

#[derive(Debug, Clone)]
pub struct ActiveSession {
    pub session_id: String,
    pub token: String,
    pub admin_id: i64,
    pub username: String,
    pub permissions: Vec<String>,
    pub remote_addr: String,
    pub client_info: String,
    pub connected_utc: i64,
    pub last_active_utc: i64,
    pub undo_stack: Vec<UndoEntry>,
    pub redo_stack: Vec<UndoEntry>,
    pub revoked: bool,
}

#[derive(Clone)]
pub struct NextDudeGrpcService {
    pub ctx: EngineContext,
    pub discovery: DiscoveryManager,
    pub sessions: Arc<RwLock<HashMap<String, ActiveSession>>>,
    pub syslog_stats: Arc<(AtomicU64, AtomicU64)>,
}

impl NextDudeGrpcService {
    pub fn new(
        ctx: EngineContext,
        discovery: DiscoveryManager,
        syslog_stats: Arc<(AtomicU64, AtomicU64)>,
    ) -> Self {
        Self {
            ctx,
            discovery,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            syslog_stats,
        }
    }

    async fn authorize<T>(
        &self,
        req: &Request<T>,
        perm: Permission,
    ) -> Result<ActiveSession, Status> {
        let auth_val = req
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("missing authorization token"))?;
        let token = auth_val
            .strip_prefix("Bearer ")
            .ok_or_else(|| Status::unauthenticated("invalid authorization header"))?;

        let mut map = self.sessions.write().await;
        let session = map
            .get_mut(token)
            .ok_or_else(|| Status::unauthenticated("invalid or expired session"))?;
        if session.revoked {
            return Err(Status::unauthenticated("session terminated by administrator"));
        }
        session.last_active_utc = chrono::Utc::now().timestamp();
        if !has_permission(&session.permissions, perm) {
            return Err(Status::permission_denied(format!(
                "missing permission: {}",
                perm.as_str()
            )));
        }
        Ok(session.clone())
    }

    async fn audit(&self, session: &ActiveSession, action: &str, target_type: &str, target_id: &str, details: &str) {
        let aid = session.admin_id;
        let uname = session.username.clone();
        let a = action.to_string();
        let tt = target_type.to_string();
        let tid = target_id.to_string();
        let det = details.to_string();
        let _ = self
            .ctx
            .storage
            .write(move |c| record_history_action(c, aid, &uname, &a, &tt, &tid, &det))
            .await;
    }

    async fn push_undo(&self, token: &str, entry: UndoEntry) {
        let mut map = self.sessions.write().await;
        if let Some(s) = map.get_mut(token) {
            if s.undo_stack.len() >= 100 {
                s.undo_stack.remove(0);
            }
            s.undo_stack.push(entry);
            s.redo_stack.clear();
        }
    }
}

fn map_err(e: ServerError) -> Status {
    match e {
        ServerError::VersionConflict { expected, current } => Status::aborted(format!(
            "optimistic concurrency conflict: expected version {expected}, current {current}"
        )),
        ServerError::AuthFailed(m) => Status::unauthenticated(m),
        ServerError::AccountLocked(ts) => {
            Status::resource_exhausted(format!("account locked until UTC {ts}"))
        }
        ServerError::PermissionDenied(m) => Status::permission_denied(m),
        ServerError::NotFound(m) => Status::not_found(m),
        ServerError::Validation(v) => Status::invalid_argument(v.to_string()),
        ServerError::Image(m) | ServerError::Config(m) => Status::invalid_argument(m),
        other => Status::internal(other.to_string()),
    }
}

#[tonic::async_trait]
impl NextDudeService for NextDudeGrpcService {
    type SubscribeStream = Pin<Box<dyn Stream<Item = Result<v1::StreamEvent, Status>> + Send>>;
    type RunToolStream = Pin<Box<dyn Stream<Item = Result<v1::ToolOutputChunk, Status>> + Send>>;

    async fn login(&self, req: Request<v1::LoginRequest>) -> Result<Response<v1::LoginResponse>, Status> {
        let remote_addr = req
            .remote_addr()
            .map(|a| a.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let body = req.into_inner();
        if body.api_version != API_VERSION {
            return Err(Status::failed_precondition(format!(
                "incompatible API version: client={}, server={API_VERSION}",
                body.api_version
            )));
        }

        let max_attempts = self.ctx.config.login_max_attempts;
        let lockout_sec = self.ctx.config.login_lockout_sec;
        let now_utc = chrono::Utc::now().timestamp();
        let uname = body.username.clone();
        let pass = body.password.clone();

        let auth = self
            .ctx
            .storage
            .write(move |c| {
                authenticate_admin(c, &uname, &pass, max_attempts, lockout_sec, now_utc)
            })
            .await
            .map_err(map_err)?;

        let aid = auth.admin_id;
        let prefs = self
            .ctx
            .storage
            .read(move |c| load_user_preferences(c, aid))
            .await
            .map_err(map_err)?;

        let token = generate_session_token();
        let session_id = format!("sess-{}", &token[..12]);
        let session = ActiveSession {
            session_id,
            token: token.clone(),
            admin_id: auth.admin_id,
            username: auth.username.clone(),
            permissions: auth.permissions.clone(),
            remote_addr,
            client_info: body.client_info,
            connected_utc: now_utc,
            last_active_utc: now_utc,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            revoked: false,
        };
        self.sessions.write().await.insert(token.clone(), session.clone());
        self.audit(&session, "login", "session", &session.session_id, "admin logged in")
            .await;

        Ok(Response::new(v1::LoginResponse {
            session_token: token,
            admin_id: auth.admin_id,
            username: auth.username,
            group_name: auth.group_name,
            permissions: auth.permissions,
            api_version: API_VERSION,
            app_name: APP_NAME.to_string(),
            preferences: Some(prefs),
        }))
    }

    async fn logout(&self, req: Request<v1::Empty>) -> Result<Response<v1::Empty>, Status> {
        if let Some(tok) = req
            .metadata()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
        {
            if let Some(s) = self.sessions.write().await.remove(tok) {
                self.audit(&s, "logout", "session", &s.session_id, "admin logged out")
                    .await;
            }
        }
        Ok(Response::new(v1::Empty {}))
    }

    async fn subscribe(
        &self,
        req: Request<v1::SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        let session = self.authorize(&req, Permission::ReadMonitoring).await?;
        let sub = req.into_inner();
        let icmp_report = self.ctx.executor.icmp.report().clone();
        let syslog_rx = self.syslog_stats.0.load(Ordering::Relaxed);
        let syslog_drop = self.syslog_stats.1.load(Ordering::Relaxed);

        let active_map_id = sub.active_map_id;
        let active_device_id = sub.active_device_id;
        let include_events = sub.include_events;
        let include_syslog = sub.include_syslog;
        let include_traps = sub.include_traps;
        let include_outages = sub.include_outages;

        let snapshot = self
            .ctx
            .storage
            .read(move |c| {
                let maps = entities::load_maps(c)?;
                let all_devices = entities::load_devices(c)?;
                let all_services = entities::load_services(c)?;
                let probes = entities::load_probes(c)?;
                let device_types = entities::load_device_types(c)?;
                let link_types = entities::load_link_types(c)?;
                let all_links = entities::load_links(c)?;
                let all_networks = entities::load_networks(c)?;
                let all_submaps = entities::load_submaps(c)?;
                let all_statics = entities::load_static_items(c)?;
                let all_deps = entities::load_dependencies(c)?;
                let all_ifaces = entities::load_interfaces(c)?;
                let alert_rules = telemetry::load_alert_rules(c)?;
                let maintenance_windows = telemetry::load_maintenance_windows(c)?;
                let charts = telemetry::load_charts(c)?;
                let images = entities::load_images(c)?.into_iter().map(|(m, _)| m).collect();
                let notification_channels = telemetry::load_notification_channels(c)?;

                let visible_dev_ids: HashSet<i64> = all_devices
                    .iter()
                    .filter(|d| active_map_id.map_or(true, |m| d.map_id == m) || active_device_id == Some(d.id))
                    .map(|d| d.id)
                    .collect();

                let devices = all_devices;
                let services = all_services
                    .into_iter()
                    .filter(|s| visible_dev_ids.contains(&s.device_id))
                    .collect();
                let interfaces = all_ifaces
                    .into_iter()
                    .filter(|i| active_device_id.map_or_else(|| visible_dev_ids.contains(&i.device_id), |did| i.device_id == did))
                    .collect();
                let links = all_links
                    .into_iter()
                    .filter(|l| active_map_id.map_or(true, |m| l.map_id == m))
                    .collect();
                let networks = all_networks
                    .into_iter()
                    .filter(|n| active_map_id.map_or(true, |m| n.map_id == m))
                    .collect();
                let submaps = all_submaps
                    .into_iter()
                    .filter(|s| active_map_id.map_or(true, |m| s.map_id == m))
                    .collect();
                let static_items = all_statics
                    .into_iter()
                    .filter(|s| active_map_id.map_or(true, |m| s.map_id == m))
                    .collect();
                let dependencies = all_deps
                    .into_iter()
                    .filter(|d| active_map_id.map_or(true, |m| d.map_id == m))
                    .collect();

                let now = chrono::Utc::now().timestamp();
                let recent_q = v1::PageQueryRequest {
                    start_utc: now - 86400,
                    end_utc: now + 60,
                    device_id: active_device_id,
                    text_filter: String::new(),
                    min_severity: None,
                    limit: 200,
                    offset: 0,
                };
                let recent_events = if include_events {
                    telemetry::query_events(c, &recent_q)?.items
                } else {
                    Vec::new()
                };
                let recent_syslog = if include_syslog {
                    telemetry::query_syslog(c, &recent_q)?.items
                } else {
                    Vec::new()
                };
                let recent_traps = if include_traps {
                    telemetry::query_traps(c, &recent_q)?.items
                } else {
                    Vec::new()
                };
                let outages = if include_outages {
                    telemetry::query_outages(c, &recent_q)?.items
                } else {
                    Vec::new()
                };

                let settings = telemetry::load_server_settings(
                    c,
                    icmp_report.mode as u32,
                    &icmp_report.message,
                )?;

                Ok(v1::SnapshotPayload {
                    maps,
                    devices,
                    services,
                    probes,
                    device_types,
                    link_types,
                    links,
                    networks,
                    submaps,
                    static_items,
                    dependencies,
                    interfaces,
                    alert_rules,
                    maintenance_windows,
                    outages,
                    charts,
                    images,
                    notification_channels,
                    recent_events,
                    recent_syslog,
                    recent_traps,
                    syslog_stats: Some(v1::SyslogDropStats {
                        received_total: syslog_rx,
                        dropped_total: syslog_drop,
                    }),
                    server_settings: Some(settings),
                })
            })
            .await
            .map_err(map_err)?;

        let (tx, rx) = mpsc::channel::<Result<v1::StreamEvent, Status>>(256);
        let mut b_rx = self.ctx.events_tx.subscribe();
        let sessions = Arc::clone(&self.sessions);
        let token = session.token.clone();

        let _ = tx
            .send(Ok(v1::StreamEvent {
                seq: 1,
                payload: Some(v1::stream_event::Payload::Snapshot(snapshot)),
            }))
            .await;

        tokio::spawn(async move {
            let mut seq = 2u64;
            let mut hb = tokio::time::interval(Duration::from_secs(15));
            loop {
                tokio::select! {
                    ev = b_rx.recv() => {
                        match ev {
                            Ok(event) => {
                                if let Some(msg) = to_stream_payload(&sub, event) {
                                    let frame = v1::StreamEvent { seq, payload: Some(msg) };
                                    seq += 1;
                                    if tx.try_send(Ok(frame)).is_err() {
                                        // Slow client backpressure protection: notify resync or close
                                        let _ = tx.try_send(Ok(v1::StreamEvent {
                                            seq,
                                            payload: Some(v1::stream_event::Payload::ResyncRequired("client queue overflow".to_string())),
                                        }));
                                        break;
                                    }
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                                let _ = tx.send(Ok(v1::StreamEvent {
                                    seq,
                                    payload: Some(v1::stream_event::Payload::ResyncRequired(format!("missed {skipped} updates"))),
                                })).await;
                                break;
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    _ = hb.tick() => {
                        let kicked = sessions
                            .read()
                            .await
                            .get(&token)
                            .map_or(true, |s| s.revoked);
                        if kicked {
                            break;
                        }
                        if tx.try_send(Ok(v1::StreamEvent {
                            seq,
                            payload: Some(v1::stream_event::Payload::HeartbeatUtc(chrono::Utc::now().timestamp())),
                        })).is_err() {
                            break;
                        }
                        seq += 1;
                    }
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn save_user_preferences(
        &self,
        req: Request<v1::UserPreferences>,
    ) -> Result<Response<v1::UserPreferences>, Status> {
        let session = self.authorize(&req, Permission::ReadMonitoring).await?;
        let mut prefs = req.into_inner();
        prefs.admin_id = session.admin_id;
        let saved = self
            .ctx
            .storage
            .write(move |c| save_user_preferences(c, &prefs))
            .await
            .map_err(map_err)?;
        Ok(Response::new(saved))
    }

    async fn save_map(&self, req: Request<v1::NetworkMap>) -> Result<Response<v1::NetworkMap>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let map = req.into_inner();
        let before = if map.id > 0 {
            self.ctx
                .storage
                .read(move |c| Ok(entities::load_maps(c)?.into_iter().find(|m| m.id == map.id)))
                .await
                .unwrap_or(None)
        } else {
            None
        };
        let m_clone = map.clone();
        let saved = self
            .ctx
            .storage
            .write(move |c| entities::save_map(c, &m_clone))
            .await
            .map_err(map_err)?;
        self.push_undo(
            &session.token,
            UndoEntry {
                description: format!("Save map {}", saved.name),
                payload: UndoPayload::Map {
                    before,
                    after: Some(saved.clone()),
                },
            },
        )
        .await;
        self.audit(&session, "save", "map", &saved.id.to_string(), &saved.name).await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "map".to_string(),
            deleted: false,
            entity_id: saved.id,
            map: Some(saved.clone()),
            ..Default::default()
        }));
        Ok(Response::new(saved))
    }

    async fn delete_map(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let id_req = req.into_inner();
        let before = self
            .ctx
            .storage
            .read(move |c| Ok(entities::load_maps(c)?.into_iter().find(|m| m.id == id_req.id)))
            .await
            .unwrap_or(None);
        self.ctx
            .storage
            .write(move |c| delete_entity_versioned(c, "maps", id_req.id, id_req.expected_version))
            .await
            .map_err(map_err)?;
        self.push_undo(
            &session.token,
            UndoEntry {
                description: format!("Delete map {}", id_req.id),
                payload: UndoPayload::Map {
                    before,
                    after: None,
                },
            },
        )
        .await;
        self.audit(&session, "delete", "map", &id_req.id.to_string(), "").await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "map".to_string(),
            deleted: true,
            entity_id: id_req.id,
            ..Default::default()
        }));
        Ok(Response::new(v1::Empty {}))
    }

    async fn auto_layout_map(&self, req: Request<v1::AutoLayoutRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let body = req.into_inner();
        auto_layout_map(&self.ctx, body.map_id, body.iterations)
            .await
            .map_err(map_err)?;
        self.audit(&session, "auto_layout", "map", &body.map_id.to_string(), "").await;
        Ok(Response::new(v1::Empty {}))
    }

    async fn copy_paste_items(
        &self,
        req: Request<v1::CopyPasteRequest>,
    ) -> Result<Response<v1::CopyPasteResponse>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let body = req.into_inner();
        let secrets = self.ctx.storage.secrets().clone();
        let (created_devices, created_nets, created_statics) = self
            .ctx
            .storage
            .write(move |c| {
                let devs = entities::load_devices(c)?;
                let nets = entities::load_networks(c)?;
                let statics = entities::load_static_items(c)?;
                let mut new_devs = Vec::new();
                for did in &body.device_ids {
                    if let Some(mut d) = devs.iter().find(|x| x.id == *did).cloned() {
                        let old_sec = entities::load_device_secrets(c, &secrets, d.id)?;
                        d.id = 0;
                        d.map_id = body.target_map_id;
                        d.pos_x += body.offset_x;
                        d.pos_y += body.offset_y;
                        d.name = format!("{} (copy)", d.name);
                        if let Some(ref mut sn) = d.snmp {
                            sn.community = Some(old_sec.community);
                            sn.auth_password = Some(old_sec.auth_password);
                            sn.priv_password = Some(old_sec.priv_password);
                        }
                        new_devs.push(entities::save_device(c, &secrets, &d)?);
                    }
                }
                let mut new_nets = Vec::new();
                for nid in &body.network_ids {
                    if let Some(mut n) = nets.iter().find(|x| x.id == *nid).cloned() {
                        n.id = 0;
                        n.map_id = body.target_map_id;
                        n.pos_x += body.offset_x;
                        n.pos_y += body.offset_y;
                        new_nets.push(entities::save_network(c, &n)?);
                    }
                }
                let mut new_statics = Vec::new();
                for sid in &body.static_ids {
                    if let Some(mut s) = statics.iter().find(|x| x.id == *sid).cloned() {
                        s.id = 0;
                        s.map_id = body.target_map_id;
                        s.pos_x += body.offset_x;
                        s.pos_y += body.offset_y;
                        new_statics.push(entities::save_static_item(c, &s)?);
                    }
                }
                Ok((new_devs, new_nets, new_statics))
            })
            .await
            .map_err(map_err)?;

        let mut created_ids = Vec::new();
        for d in created_devices {
            created_ids.push(d.id);
            self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
                entity_type: "device".to_string(),
                deleted: false,
                entity_id: d.id,
                device: Some(d),
                ..Default::default()
            }));
        }
        for n in created_nets {
            self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
                entity_type: "network".to_string(),
                deleted: false,
                entity_id: n.id,
                network: Some(n),
                ..Default::default()
            }));
        }
        for s in created_statics {
            self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
                entity_type: "static_item".to_string(),
                deleted: false,
                entity_id: s.id,
                static_item: Some(s),
                ..Default::default()
            }));
        }
        self.ctx.refresh_device_ip_index().await;
        self.audit(
            &session,
            "copy_paste",
            "map",
            &body.target_map_id.to_string(),
            &format!("{} device(s) pasted", created_ids.len()),
        )
        .await;
        Ok(Response::new(v1::CopyPasteResponse {
            created_device_ids: created_ids,
        }))
    }

    async fn undo(&self, req: Request<v1::Empty>) -> Result<Response<v1::UndoRedoResponse>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let entry = {
            let mut map = self.sessions.write().await;
            let Some(s) = map.get_mut(&session.token) else {
                return Ok(Response::new(v1::UndoRedoResponse {
                    applied: false,
                    description: String::new(),
                }));
            };
            s.undo_stack.pop()
        };
        let Some(entry) = entry else {
            return Ok(Response::new(v1::UndoRedoResponse {
                applied: false,
                description: "Nothing to undo".to_string(),
            }));
        };
        apply_undo_payload(&self.ctx, &entry.payload, true)
            .await
            .map_err(map_err)?;
        let desc = entry.description.clone();
        if let Some(s) = self.sessions.write().await.get_mut(&session.token) {
            s.redo_stack.push(entry);
        }
        Ok(Response::new(v1::UndoRedoResponse {
            applied: true,
            description: desc,
        }))
    }

    async fn redo(&self, req: Request<v1::Empty>) -> Result<Response<v1::UndoRedoResponse>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let entry = {
            let mut map = self.sessions.write().await;
            let Some(s) = map.get_mut(&session.token) else {
                return Ok(Response::new(v1::UndoRedoResponse {
                    applied: false,
                    description: String::new(),
                }));
            };
            s.redo_stack.pop()
        };
        let Some(entry) = entry else {
            return Ok(Response::new(v1::UndoRedoResponse {
                applied: false,
                description: "Nothing to redo".to_string(),
            }));
        };
        apply_undo_payload(&self.ctx, &entry.payload, false)
            .await
            .map_err(map_err)?;
        let desc = entry.description.clone();
        if let Some(s) = self.sessions.write().await.get_mut(&session.token) {
            s.undo_stack.push(entry);
        }
        Ok(Response::new(v1::UndoRedoResponse {
            applied: true,
            description: desc,
        }))
    }

    async fn save_device(&self, req: Request<v1::Device>) -> Result<Response<v1::Device>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let mut dev = req.into_inner();
        let before = if dev.id > 0 {
            self.ctx
                .storage
                .read(move |c| Ok(entities::load_devices(c)?.into_iter().find(|d| d.id == dev.id)))
                .await
                .unwrap_or(None)
        } else {
            None
        };

        // Resolve automatic vendor/model/image mapping from TOML profiles
        if !dev.sys_object_id.is_empty() {
            let guard = self.ctx.profiles.read().await;
            if let Some(prof) = guard.match_sys_object_id(&dev.sys_object_id) {
                if dev.vendor.is_empty() {
                    dev.vendor = prof.vendor.clone();
                }
                let (model, img_key, _) = guard.match_model_and_image(prof, &dev.sys_descr);
                if dev.model.is_empty() && !model.is_empty() {
                    dev.model = model;
                }
                if dev.resolved_image_key.is_none() {
                    dev.resolved_image_key = img_key.map(str::to_string);
                }
            }
        }

        let secrets = self.ctx.storage.secrets().clone();
        let d_clone = dev.clone();
        let saved = self
            .ctx
            .storage
            .write(move |c| entities::save_device(c, &secrets, &d_clone))
            .await
            .map_err(map_err)?;

        self.ctx.refresh_device_ip_index().await;
        self.push_undo(
            &session.token,
            UndoEntry {
                description: format!("Save device {}", saved.name),
                payload: UndoPayload::Device {
                    before,
                    after: Some(saved.clone()),
                },
            },
        )
        .await;
        self.audit(&session, "save", "device", &saved.id.to_string(), &saved.name).await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "device".to_string(),
            deleted: false,
            entity_id: saved.id,
            device: Some(saved.clone()),
            ..Default::default()
        }));
        Ok(Response::new(saved))
    }

    async fn delete_device(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let id_req = req.into_inner();
        let before = self
            .ctx
            .storage
            .read(move |c| Ok(entities::load_devices(c)?.into_iter().find(|d| d.id == id_req.id)))
            .await
            .unwrap_or(None);
        self.ctx
            .storage
            .write(move |c| delete_entity_versioned(c, "devices", id_req.id, id_req.expected_version))
            .await
            .map_err(map_err)?;
        self.ctx.refresh_device_ip_index().await;
        self.push_undo(
            &session.token,
            UndoEntry {
                description: format!("Delete device {}", id_req.id),
                payload: UndoPayload::Device {
                    before,
                    after: None,
                },
            },
        )
        .await;
        self.audit(&session, "delete", "device", &id_req.id.to_string(), "").await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "device".to_string(),
            deleted: true,
            entity_id: id_req.id,
            ..Default::default()
        }));
        Ok(Response::new(v1::Empty {}))
    }

    async fn save_service(
        &self,
        req: Request<v1::DeviceService>,
    ) -> Result<Response<v1::DeviceService>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let svc = req.into_inner();
        let saved = self
            .ctx
            .storage
            .write(move |c| entities::save_service(c, &svc))
            .await
            .map_err(map_err)?;
        self.audit(&session, "save", "service", &saved.id.to_string(), &saved.name).await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "service".to_string(),
            deleted: false,
            entity_id: saved.id,
            service: Some(saved.clone()),
            ..Default::default()
        }));
        Ok(Response::new(saved))
    }

    async fn delete_service(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let id_req = req.into_inner();
        self.ctx
            .storage
            .write(move |c| delete_entity_versioned(c, "device_services", id_req.id, id_req.expected_version))
            .await
            .map_err(map_err)?;
        self.audit(&session, "delete", "service", &id_req.id.to_string(), "").await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "service".to_string(),
            deleted: true,
            entity_id: id_req.id,
            ..Default::default()
        }));
        Ok(Response::new(v1::Empty {}))
    }

    async fn save_probe(
        &self,
        req: Request<v1::ProbeDefinition>,
    ) -> Result<Response<v1::ProbeDefinition>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let probe = req.into_inner();
        let saved = self
            .ctx
            .storage
            .write(move |c| entities::save_probe(c, &probe))
            .await
            .map_err(map_err)?;
        self.audit(&session, "save", "probe", &saved.id.to_string(), &saved.name).await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "probe".to_string(),
            deleted: false,
            entity_id: saved.id,
            probe: Some(saved.clone()),
            ..Default::default()
        }));
        Ok(Response::new(saved))
    }

    async fn delete_probe(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let id_req = req.into_inner();
        self.ctx
            .storage
            .write(move |c| delete_entity_versioned(c, "probes", id_req.id, id_req.expected_version))
            .await
            .map_err(map_err)?;
        self.audit(&session, "delete", "probe", &id_req.id.to_string(), "").await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "probe".to_string(),
            deleted: true,
            entity_id: id_req.id,
            ..Default::default()
        }));
        Ok(Response::new(v1::Empty {}))
    }

    async fn save_device_type(
        &self,
        req: Request<v1::DeviceType>,
    ) -> Result<Response<v1::DeviceType>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let dt = req.into_inner();
        let saved = self
            .ctx
            .storage
            .write(move |c| entities::save_device_type(c, &dt))
            .await
            .map_err(map_err)?;
        self.audit(&session, "save", "device_type", &saved.id.to_string(), &saved.name).await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "device_type".to_string(),
            deleted: false,
            entity_id: saved.id,
            device_type: Some(saved.clone()),
            ..Default::default()
        }));
        Ok(Response::new(saved))
    }

    async fn delete_device_type(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let id_req = req.into_inner();
        self.ctx
            .storage
            .write(move |c| delete_entity_versioned(c, "device_types", id_req.id, id_req.expected_version))
            .await
            .map_err(map_err)?;
        self.audit(&session, "delete", "device_type", &id_req.id.to_string(), "").await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "device_type".to_string(),
            deleted: true,
            entity_id: id_req.id,
            ..Default::default()
        }));
        Ok(Response::new(v1::Empty {}))
    }

    async fn save_link_type(&self, req: Request<v1::LinkType>) -> Result<Response<v1::LinkType>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let lt = req.into_inner();
        let saved = self
            .ctx
            .storage
            .write(move |c| entities::save_link_type(c, &lt))
            .await
            .map_err(map_err)?;
        self.audit(&session, "save", "link_type", &saved.id.to_string(), &saved.name).await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "link_type".to_string(),
            deleted: false,
            entity_id: saved.id,
            link_type: Some(saved.clone()),
            ..Default::default()
        }));
        Ok(Response::new(saved))
    }

    async fn delete_link_type(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let id_req = req.into_inner();
        self.ctx
            .storage
            .write(move |c| delete_entity_versioned(c, "link_types", id_req.id, id_req.expected_version))
            .await
            .map_err(map_err)?;
        self.audit(&session, "delete", "link_type", &id_req.id.to_string(), "").await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "link_type".to_string(),
            deleted: true,
            entity_id: id_req.id,
            ..Default::default()
        }));
        Ok(Response::new(v1::Empty {}))
    }

    async fn save_link(&self, req: Request<v1::NetworkLink>) -> Result<Response<v1::NetworkLink>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let link = req.into_inner();
        let before = if link.id > 0 {
            self.ctx
                .storage
                .read(move |c| Ok(entities::load_links(c)?.into_iter().find(|l| l.id == link.id)))
                .await
                .unwrap_or(None)
        } else {
            None
        };
        let l_clone = link.clone();
        let saved = self
            .ctx
            .storage
            .write(move |c| entities::save_link(c, &l_clone))
            .await
            .map_err(map_err)?;
        self.push_undo(
            &session.token,
            UndoEntry {
                description: format!("Save link {}", saved.id),
                payload: UndoPayload::Link {
                    before,
                    after: Some(saved.clone()),
                },
            },
        )
        .await;
        self.audit(&session, "save", "link", &saved.id.to_string(), &saved.interface_key).await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "link".to_string(),
            deleted: false,
            entity_id: saved.id,
            link: Some(saved.clone()),
            ..Default::default()
        }));
        Ok(Response::new(saved))
    }

    async fn delete_link(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let id_req = req.into_inner();
        let before = self
            .ctx
            .storage
            .read(move |c| Ok(entities::load_links(c)?.into_iter().find(|l| l.id == id_req.id)))
            .await
            .unwrap_or(None);
        self.ctx
            .storage
            .write(move |c| delete_entity_versioned(c, "links", id_req.id, id_req.expected_version))
            .await
            .map_err(map_err)?;
        self.push_undo(
            &session.token,
            UndoEntry {
                description: format!("Delete link {}", id_req.id),
                payload: UndoPayload::Link {
                    before,
                    after: None,
                },
            },
        )
        .await;
        self.audit(&session, "delete", "link", &id_req.id.to_string(), "").await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "link".to_string(),
            deleted: true,
            entity_id: id_req.id,
            ..Default::default()
        }));
        Ok(Response::new(v1::Empty {}))
    }

    async fn save_network(&self, req: Request<v1::MapNetwork>) -> Result<Response<v1::MapNetwork>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let net = req.into_inner();
        let before = if net.id > 0 {
            self.ctx
                .storage
                .read(move |c| Ok(entities::load_networks(c)?.into_iter().find(|n| n.id == net.id)))
                .await
                .unwrap_or(None)
        } else {
            None
        };
        let n_clone = net.clone();
        let saved = self
            .ctx
            .storage
            .write(move |c| entities::save_network(c, &n_clone))
            .await
            .map_err(map_err)?;
        self.push_undo(
            &session.token,
            UndoEntry {
                description: format!("Save network {}", saved.name),
                payload: UndoPayload::Network {
                    before,
                    after: Some(saved.clone()),
                },
            },
        )
        .await;
        self.audit(&session, "save", "network", &saved.id.to_string(), &saved.cidr).await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "network".to_string(),
            deleted: false,
            entity_id: saved.id,
            network: Some(saved.clone()),
            ..Default::default()
        }));
        Ok(Response::new(saved))
    }

    async fn delete_network(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let id_req = req.into_inner();
        self.ctx
            .storage
            .write(move |c| delete_entity_versioned(c, "map_networks", id_req.id, id_req.expected_version))
            .await
            .map_err(map_err)?;
        self.audit(&session, "delete", "network", &id_req.id.to_string(), "").await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "network".to_string(),
            deleted: true,
            entity_id: id_req.id,
            ..Default::default()
        }));
        Ok(Response::new(v1::Empty {}))
    }

    async fn save_submap(&self, req: Request<v1::SubmapItem>) -> Result<Response<v1::SubmapItem>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let sub = req.into_inner();
        let saved = self
            .ctx
            .storage
            .write(move |c| entities::save_submap(c, &sub))
            .await
            .map_err(map_err)?;
        self.audit(&session, "save", "submap", &saved.id.to_string(), &saved.label).await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "submap".to_string(),
            deleted: false,
            entity_id: saved.id,
            submap: Some(saved.clone()),
            ..Default::default()
        }));
        Ok(Response::new(saved))
    }

    async fn delete_submap(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let id_req = req.into_inner();
        self.ctx
            .storage
            .write(move |c| delete_entity_versioned(c, "submaps", id_req.id, id_req.expected_version))
            .await
            .map_err(map_err)?;
        self.audit(&session, "delete", "submap", &id_req.id.to_string(), "").await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "submap".to_string(),
            deleted: true,
            entity_id: id_req.id,
            ..Default::default()
        }));
        Ok(Response::new(v1::Empty {}))
    }

    async fn save_static_item(
        &self,
        req: Request<v1::StaticItem>,
    ) -> Result<Response<v1::StaticItem>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let item = req.into_inner();
        let saved = self
            .ctx
            .storage
            .write(move |c| entities::save_static_item(c, &item))
            .await
            .map_err(map_err)?;
        self.audit(&session, "save", "static_item", &saved.id.to_string(), &saved.label).await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "static_item".to_string(),
            deleted: false,
            entity_id: saved.id,
            static_item: Some(saved.clone()),
            ..Default::default()
        }));
        Ok(Response::new(saved))
    }

    async fn delete_static_item(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let id_req = req.into_inner();
        self.ctx
            .storage
            .write(move |c| delete_entity_versioned(c, "static_items", id_req.id, id_req.expected_version))
            .await
            .map_err(map_err)?;
        self.audit(&session, "delete", "static_item", &id_req.id.to_string(), "").await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "static_item".to_string(),
            deleted: true,
            entity_id: id_req.id,
            ..Default::default()
        }));
        Ok(Response::new(v1::Empty {}))
    }

    async fn save_dependency(&self, req: Request<v1::Dependency>) -> Result<Response<v1::Dependency>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let dep = req.into_inner();
        let saved = self
            .ctx
            .storage
            .write(move |c| entities::save_dependency(c, &dep))
            .await
            .map_err(map_err)?;
        self.audit(&session, "save", "dependency", &saved.id.to_string(), "").await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "dependency".to_string(),
            deleted: false,
            entity_id: saved.id,
            dependency: Some(saved.clone()),
            ..Default::default()
        }));
        Ok(Response::new(saved))
    }

    async fn delete_dependency(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let id_req = req.into_inner();
        self.ctx
            .storage
            .write(move |c| delete_entity_versioned(c, "dependencies", id_req.id, id_req.expected_version))
            .await
            .map_err(map_err)?;
        self.audit(&session, "delete", "dependency", &id_req.id.to_string(), "").await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "dependency".to_string(),
            deleted: true,
            entity_id: id_req.id,
            ..Default::default()
        }));
        Ok(Response::new(v1::Empty {}))
    }

    async fn acknowledge_alert(&self, req: Request<v1::AcknowledgeAlertRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self.authorize(&req, Permission::AcknowledgeAlerts).await?;
        let body = req.into_inner();
        let uname = session.username.clone();
        self.ctx
            .storage
            .write(move |c| telemetry::acknowledge_event(c, body.event_id, &uname))
            .await
            .map_err(map_err)?;
        self.audit(&session, "acknowledge", "event", &body.event_id.to_string(), "").await;
        Ok(Response::new(v1::Empty {}))
    }

    async fn save_alert_rule(&self, req: Request<v1::AlertRule>) -> Result<Response<v1::AlertRule>, Status> {
        let session = self
            .authorize(&req, Permission::ManageNotificationsAndAlerts)
            .await?;
        let rule = req.into_inner();
        let saved = self
            .ctx
            .storage
            .write(move |c| telemetry::save_alert_rule(c, &rule))
            .await
            .map_err(map_err)?;
        self.audit(&session, "save", "alert_rule", &saved.id.to_string(), &saved.name).await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "alert_rule".to_string(),
            deleted: false,
            entity_id: saved.id,
            alert_rule: Some(saved.clone()),
            ..Default::default()
        }));
        Ok(Response::new(saved))
    }

    async fn delete_alert_rule(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self
            .authorize(&req, Permission::ManageNotificationsAndAlerts)
            .await?;
        let id_req = req.into_inner();
        self.ctx
            .storage
            .write(move |c| delete_entity_versioned(c, "alert_rules", id_req.id, id_req.expected_version))
            .await
            .map_err(map_err)?;
        self.audit(&session, "delete", "alert_rule", &id_req.id.to_string(), "").await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "alert_rule".to_string(),
            deleted: true,
            entity_id: id_req.id,
            ..Default::default()
        }));
        Ok(Response::new(v1::Empty {}))
    }

    async fn save_maintenance_window(
        &self,
        req: Request<v1::MaintenanceWindow>,
    ) -> Result<Response<v1::MaintenanceWindow>, Status> {
        let session = self
            .authorize(&req, Permission::ManageNotificationsAndAlerts)
            .await?;
        let mw = req.into_inner();
        let saved = self
            .ctx
            .storage
            .write(move |c| telemetry::save_maintenance_window(c, &mw))
            .await
            .map_err(map_err)?;
        self.audit(&session, "save", "maintenance", &saved.id.to_string(), &saved.name).await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "maintenance".to_string(),
            deleted: false,
            entity_id: saved.id,
            maintenance: Some(saved.clone()),
            ..Default::default()
        }));
        Ok(Response::new(saved))
    }

    async fn delete_maintenance_window(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self
            .authorize(&req, Permission::ManageNotificationsAndAlerts)
            .await?;
        let id_req = req.into_inner();
        self.ctx
            .storage
            .write(move |c| delete_entity_versioned(c, "maintenance_windows", id_req.id, id_req.expected_version))
            .await
            .map_err(map_err)?;
        self.audit(&session, "delete", "maintenance", &id_req.id.to_string(), "").await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "maintenance".to_string(),
            deleted: true,
            entity_id: id_req.id,
            ..Default::default()
        }));
        Ok(Response::new(v1::Empty {}))
    }

    async fn save_notification_channel(
        &self,
        req: Request<v1::NotificationChannel>,
    ) -> Result<Response<v1::NotificationChannel>, Status> {
        let session = self
            .authorize(&req, Permission::ManageNotificationsAndAlerts)
            .await?;
        let ch = req.into_inner();
        let secrets = self.ctx.storage.secrets().clone();
        let saved = self
            .ctx
            .storage
            .write(move |c| telemetry::save_notification_channel(c, &secrets, &ch))
            .await
            .map_err(map_err)?;
        self.audit(&session, "save", "notification_channel", &saved.id.to_string(), &saved.name).await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "notification_channel".to_string(),
            deleted: false,
            entity_id: saved.id,
            notification_channel: Some(saved.clone()),
            ..Default::default()
        }));
        Ok(Response::new(saved))
    }

    async fn delete_notification_channel(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self
            .authorize(&req, Permission::ManageNotificationsAndAlerts)
            .await?;
        let id_req = req.into_inner();
        self.ctx
            .storage
            .write(move |c| delete_entity_versioned(c, "notification_channels", id_req.id, id_req.expected_version))
            .await
            .map_err(map_err)?;
        self.audit(&session, "delete", "notification_channel", &id_req.id.to_string(), "").await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "notification_channel".to_string(),
            deleted: true,
            entity_id: id_req.id,
            ..Default::default()
        }));
        Ok(Response::new(v1::Empty {}))
    }

    async fn test_notification_channel(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self
            .authorize(&req, Permission::ManageNotificationsAndAlerts)
            .await?;
        let id_req = req.into_inner();
        self.ctx
            .notifications
            .test_channel(id_req.id)
            .await
            .map_err(map_err)?;
        self.audit(&session, "test", "notification_channel", &id_req.id.to_string(), "").await;
        Ok(Response::new(v1::Empty {}))
    }

    async fn start_discovery(
        &self,
        req: Request<v1::StartDiscoveryRequest>,
    ) -> Result<Response<v1::StartDiscoveryResponse>, Status> {
        let session = self.authorize(&req, Permission::ManageDiscovery).await?;
        let job_id = self
            .discovery
            .start_job(self.ctx.clone(), req.into_inner())
            .await
            .map_err(map_err)?;
        self.audit(&session, "start", "discovery", &job_id, "").await;
        Ok(Response::new(v1::StartDiscoveryResponse { job_id }))
    }

    async fn cancel_discovery(
        &self,
        req: Request<v1::StartDiscoveryResponse>,
    ) -> Result<Response<v1::Empty>, Status> {
        let _ = self.authorize(&req, Permission::ManageDiscovery).await?;
        self.discovery.cancel_job(&req.into_inner().job_id).await;
        Ok(Response::new(v1::Empty {}))
    }

    async fn import_discovery_candidates(
        &self,
        req: Request<v1::ImportDiscoveryRequest>,
    ) -> Result<Response<v1::ImportDiscoveryResponse>, Status> {
        let session = self.authorize(&req, Permission::ManageDiscovery).await?;
        let resp = self
            .discovery
            .import_candidates(&self.ctx, req.into_inner())
            .await
            .map_err(map_err)?;
        self.audit(
            &session,
            "import",
            "discovery",
            "",
            &format!("devices={}, links={}", resp.imported_devices, resp.imported_links),
        )
        .await;
        Ok(Response::new(resp))
    }

    async fn run_tool(
        &self,
        req: Request<v1::RunToolRequest>,
    ) -> Result<Response<Self::RunToolStream>, Status> {
        let session = self.authorize(&req, Permission::RunTools).await?;
        let body = req.into_inner();
        self.audit(&session, "run_tool", "tool", &body.target, &format!("kind={}", body.kind))
            .await;
        let (tx, rx) = mpsc::channel(64);
        let ctx = self.ctx.clone();
        tokio::spawn(async move {
            execute_tool_stream(ctx, body, tx).await;
        });
        Ok(Response::new(Box::pin(ReceiverStream::new(rx))))
    }

    async fn save_chart(&self, req: Request<v1::ChartDefinition>) -> Result<Response<v1::ChartDefinition>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let chart = req.into_inner();
        let saved = self
            .ctx
            .storage
            .write(move |c| telemetry::save_chart(c, &chart))
            .await
            .map_err(map_err)?;
        self.audit(&session, "save", "chart", &saved.id.to_string(), &saved.name).await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "chart".to_string(),
            deleted: false,
            entity_id: saved.id,
            chart: Some(saved.clone()),
            ..Default::default()
        }));
        Ok(Response::new(saved))
    }

    async fn delete_chart(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self.authorize(&req, Permission::EditMapsAndDevices).await?;
        let id_req = req.into_inner();
        self.ctx
            .storage
            .write(move |c| delete_entity_versioned(c, "charts", id_req.id, id_req.expected_version))
            .await
            .map_err(map_err)?;
        self.audit(&session, "delete", "chart", &id_req.id.to_string(), "").await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "chart".to_string(),
            deleted: true,
            entity_id: id_req.id,
            ..Default::default()
        }));
        Ok(Response::new(v1::Empty {}))
    }

    async fn query_chart_series(
        &self,
        req: Request<v1::QueryChartSeriesRequest>,
    ) -> Result<Response<v1::QueryChartSeriesResponse>, Status> {
        let _ = self.authorize(&req, Permission::ReadMonitoring).await?;
        let q = req.into_inner();
        let res = self
            .ctx
            .storage
            .read(move |c| telemetry::query_chart_series(c, q.chart_id, q.start_utc, q.end_utc, q.max_points))
            .await
            .map_err(map_err)?;
        Ok(Response::new(res))
    }

    async fn query_events(&self, req: Request<v1::PageQueryRequest>) -> Result<Response<v1::QueryEventsResponse>, Status> {
        let _ = self.authorize(&req, Permission::ReadMonitoring).await?;
        let q = req.into_inner();
        let res = self
            .ctx
            .storage
            .read(move |c| telemetry::query_events(c, &q))
            .await
            .map_err(map_err)?;
        Ok(Response::new(res))
    }

    async fn query_syslog(&self, req: Request<v1::PageQueryRequest>) -> Result<Response<v1::QuerySyslogResponse>, Status> {
        let _ = self.authorize(&req, Permission::ReadMonitoring).await?;
        let q = req.into_inner();
        let res = self
            .ctx
            .storage
            .read(move |c| telemetry::query_syslog(c, &q))
            .await
            .map_err(map_err)?;
        Ok(Response::new(res))
    }

    async fn query_traps(&self, req: Request<v1::PageQueryRequest>) -> Result<Response<v1::QueryTrapsResponse>, Status> {
        let _ = self.authorize(&req, Permission::ReadMonitoring).await?;
        let q = req.into_inner();
        let res = self
            .ctx
            .storage
            .read(move |c| telemetry::query_traps(c, &q))
            .await
            .map_err(map_err)?;
        Ok(Response::new(res))
    }

    async fn query_outages(&self, req: Request<v1::PageQueryRequest>) -> Result<Response<v1::QueryOutagesResponse>, Status> {
        let _ = self.authorize(&req, Permission::ReadMonitoring).await?;
        let q = req.into_inner();
        let res = self
            .ctx
            .storage
            .read(move |c| telemetry::query_outages(c, &q))
            .await
            .map_err(map_err)?;
        Ok(Response::new(res))
    }

    async fn query_history_actions(
        &self,
        req: Request<v1::PageQueryRequest>,
    ) -> Result<Response<v1::QueryHistoryActionsResponse>, Status> {
        let _ = self.authorize(&req, Permission::ReadMonitoring).await?;
        let q = req.into_inner();
        let res = self
            .ctx
            .storage
            .read(move |c| telemetry::query_history_actions(c, &q))
            .await
            .map_err(map_err)?;
        Ok(Response::new(res))
    }

    async fn upload_image(&self, req: Request<v1::UploadImageRequest>) -> Result<Response<v1::ImageAssetMeta>, Status> {
        let session = self.authorize(&req, Permission::ManageImages).await?;
        let body = req.into_inner();
        let processed = validate_and_process_image(
            &body.raw_data,
            self.ctx.config.max_image_upload_bytes,
            self.ctx.config.max_image_dimension_px,
        )
        .map_err(map_err)?;

        let storage_key = format!("img-{}", &generate_session_token()[..16]);
        store_processed_image(&self.ctx.config.images_dir(), &storage_key, &processed)
            .map_err(map_err)?;
        let meta = build_image_meta(&body.name, &body.vendor, &body.model, body.tags, &processed);
        let sk_clone = storage_key.clone();
        let saved = self
            .ctx
            .storage
            .write(move |c| entities::insert_image_meta(c, &meta, &sk_clone))
            .await
            .map_err(map_err)?;

        self.audit(&session, "upload", "image", &saved.id.to_string(), &saved.name).await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "image".to_string(),
            deleted: false,
            entity_id: saved.id,
            image: Some(saved.clone()),
            ..Default::default()
        }));
        Ok(Response::new(saved))
    }

    async fn import_image_from_url(
        &self,
        req: Request<v1::ImportImageUrlRequest>,
    ) -> Result<Response<v1::ImageAssetMeta>, Status> {
        let session = self.authorize(&req, Permission::ManageImages).await?;
        let allowed = self
            .ctx
            .storage
            .read(|c| Ok(telemetry::load_server_settings(c, 0, "")?.allow_image_url_import))
            .await
            .map_err(map_err)?;
        if !allowed {
            return Err(Status::permission_denied(
                "image import from URL is disabled in server settings",
            ));
        }
        let body = req.into_inner();
        let raw = download_image_https(&body.url, self.ctx.config.max_image_upload_bytes)
            .await
            .map_err(map_err)?;
        let processed = validate_and_process_image(
            &raw,
            self.ctx.config.max_image_upload_bytes,
            self.ctx.config.max_image_dimension_px,
        )
        .map_err(map_err)?;

        let storage_key = format!("img-{}", &generate_session_token()[..16]);
        store_processed_image(&self.ctx.config.images_dir(), &storage_key, &processed)
            .map_err(map_err)?;
        let meta = build_image_meta(&body.name, &body.vendor, &body.model, body.tags, &processed);
        let saved = self
            .ctx
            .storage
            .write(move |c| entities::insert_image_meta(c, &meta, &storage_key))
            .await
            .map_err(map_err)?;
        self.audit(&session, "import_url", "image", &saved.id.to_string(), &body.url).await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "image".to_string(),
            deleted: false,
            entity_id: saved.id,
            image: Some(saved.clone()),
            ..Default::default()
        }));
        Ok(Response::new(saved))
    }

    async fn delete_image(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self.authorize(&req, Permission::ManageImages).await?;
        let id_req = req.into_inner();
        let key: Option<String> = self
            .ctx
            .storage
            .read(move |c| {
                Ok(entities::load_images(c)?
                    .into_iter()
                    .find(|(m, _)| m.id == id_req.id)
                    .map(|(_, k)| k))
            })
            .await
            .map_err(map_err)?;
        self.ctx
            .storage
            .write(move |c| delete_entity_versioned(c, "images", id_req.id, id_req.expected_version))
            .await
            .map_err(map_err)?;
        if let Some(k) = key {
            delete_stored_image_files(&self.ctx.config.images_dir(), &k);
        }
        self.audit(&session, "delete", "image", &id_req.id.to_string(), "").await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "image".to_string(),
            deleted: true,
            entity_id: id_req.id,
            ..Default::default()
        }));
        Ok(Response::new(v1::Empty {}))
    }

    async fn get_image_blob(
        &self,
        req: Request<v1::GetImageBlobRequest>,
    ) -> Result<Response<v1::GetImageBlobResponse>, Status> {
        let _ = self.authorize(&req, Permission::ReadMonitoring).await?;
        let q = req.into_inner();
        if let Some(builtin_key) = q.variant.strip_prefix("builtin:") {
            if let Some(svg) = crate::images::builtin_svg_by_key(builtin_key) {
                return Ok(Response::new(v1::GetImageBlobResponse {
                    mime_type: "image/svg+xml".to_string(),
                    data: svg.as_bytes().to_vec(),
                }));
            }
        }
        let images = self
            .ctx
            .storage
            .read(entities::load_images)
            .await
            .map_err(map_err)?;
        let (meta, key) = images
            .into_iter()
            .find(|(m, _)| m.id == q.image_id)
            .ok_or_else(|| Status::not_found(format!("image {}", q.image_id)))?;
        let variant = ImageSizeVariant::parse(&q.variant);
        let (mime_type, data) =
            load_stored_image_blob(&self.ctx.config.images_dir(), &key, &meta.mime_type, variant)
                .map_err(map_err)?;
        Ok(Response::new(v1::GetImageBlobResponse { mime_type, data }))
    }

    async fn list_admins(&self, req: Request<v1::Empty>) -> Result<Response<v1::ListAdminsResponse>, Status> {
        let _ = self.authorize(&req, Permission::ManageAdmins).await?;
        let (users, groups) = self
            .ctx
            .storage
            .read(list_admins_and_groups)
            .await
            .map_err(map_err)?;
        Ok(Response::new(v1::ListAdminsResponse { users, groups }))
    }

    async fn save_admin(&self, req: Request<v1::AdminUser>) -> Result<Response<v1::AdminUser>, Status> {
        let session = self.authorize(&req, Permission::ManageAdmins).await?;
        let user = req.into_inner();
        let saved = self
            .ctx
            .storage
            .write(move |c| save_admin_user(c, &user))
            .await
            .map_err(map_err)?;
        self.audit(&session, "save", "admin", &saved.id.to_string(), &saved.username).await;
        Ok(Response::new(saved))
    }

    async fn delete_admin(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self.authorize(&req, Permission::ManageAdmins).await?;
        let id_req = req.into_inner();
        if id_req.id == session.admin_id {
            return Err(Status::invalid_argument("cannot delete your own active admin account"));
        }
        self.ctx
            .storage
            .write(move |c| delete_entity_versioned(c, "admins", id_req.id, id_req.expected_version))
            .await
            .map_err(map_err)?;
        self.audit(&session, "delete", "admin", &id_req.id.to_string(), "").await;
        Ok(Response::new(v1::Empty {}))
    }

    async fn save_admin_group(&self, req: Request<v1::AdminGroup>) -> Result<Response<v1::AdminGroup>, Status> {
        let session = self.authorize(&req, Permission::ManageAdmins).await?;
        let group = req.into_inner();
        let saved = self
            .ctx
            .storage
            .write(move |c| save_admin_group(c, &group))
            .await
            .map_err(map_err)?;
        self.audit(&session, "save", "admin_group", &saved.id.to_string(), &saved.name).await;
        Ok(Response::new(saved))
    }

    async fn delete_admin_group(&self, req: Request<v1::IdRequest>) -> Result<Response<v1::Empty>, Status> {
        let session = self.authorize(&req, Permission::ManageAdmins).await?;
        let id_req = req.into_inner();
        self.ctx
            .storage
            .write(move |c| delete_entity_versioned(c, "admin_groups", id_req.id, id_req.expected_version))
            .await
            .map_err(map_err)?;
        self.audit(&session, "delete", "admin_group", &id_req.id.to_string(), "").await;
        Ok(Response::new(v1::Empty {}))
    }

    async fn list_active_admins(
        &self,
        req: Request<v1::Empty>,
    ) -> Result<Response<v1::ListActiveAdminsResponse>, Status> {
        let _ = self.authorize(&req, Permission::ManageAdmins).await?;
        let sessions = self
            .sessions
            .read()
            .await
            .values()
            .filter(|s| !s.revoked)
            .map(|s| v1::ActiveAdminSession {
                session_id: s.session_id.clone(),
                admin_id: s.admin_id,
                username: s.username.clone(),
                remote_addr: s.remote_addr.clone(),
                client_info: s.client_info.clone(),
                connected_utc: s.connected_utc,
                last_active_utc: s.last_active_utc,
            })
            .collect();
        Ok(Response::new(v1::ListActiveAdminsResponse { sessions }))
    }

    async fn kick_admin_session(
        &self,
        req: Request<v1::KickSessionRequest>,
    ) -> Result<Response<v1::Empty>, Status> {
        let session = self.authorize(&req, Permission::ManageAdmins).await?;
        let sid = req.into_inner().session_id;
        let mut map = self.sessions.write().await;
        for s in map.values_mut() {
            if s.session_id == sid {
                s.revoked = true;
            }
        }
        drop(map);
        self.audit(&session, "kick", "session", &sid, "session disconnected").await;
        Ok(Response::new(v1::Empty {}))
    }

    async fn get_server_settings(&self, req: Request<v1::Empty>) -> Result<Response<v1::ServerSettings>, Status> {
        let _ = self.authorize(&req, Permission::ReadMonitoring).await?;
        let icmp = self.ctx.executor.icmp.report().clone();
        let settings = self
            .ctx
            .storage
            .read(move |c| telemetry::load_server_settings(c, icmp.mode as u32, &icmp.message))
            .await
            .map_err(map_err)?;
        Ok(Response::new(settings))
    }

    async fn update_server_settings(
        &self,
        req: Request<v1::ServerSettings>,
    ) -> Result<Response<v1::ServerSettings>, Status> {
        let session = self.authorize(&req, Permission::ManageServerSettings).await?;
        let body = req.into_inner();
        let saved = self
            .ctx
            .storage
            .write(move |c| telemetry::update_server_settings(c, &body))
            .await
            .map_err(map_err)?;
        self.audit(&session, "update", "server_settings", "1", "").await;
        self.ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "server_settings".to_string(),
            deleted: false,
            entity_id: 1,
            server_settings: Some(saved.clone()),
            ..Default::default()
        }));
        Ok(Response::new(saved))
    }

    async fn list_vendor_profiles(
        &self,
        req: Request<v1::Empty>,
    ) -> Result<Response<v1::ListVendorProfilesResponse>, Status> {
        let _ = self.authorize(&req, Permission::ReadMonitoring).await?;
        let profiles = self
            .ctx
            .profiles
            .read()
            .await
            .all()
            .iter()
            .map(|p| v1::VendorProfileSummary {
                name: p.name.clone(),
                vendor: p.vendor.clone(),
                sys_object_id_prefixes: p.sys_object_id_prefixes.clone(),
                sensor_count: p.sensors.len() as u32,
                model_rule_count: p.model_rules.len() as u32,
            })
            .collect();
        Ok(Response::new(v1::ListVendorProfilesResponse { profiles }))
    }
}

fn to_stream_payload(
    sub: &v1::SubscribeRequest,
    event: ServerBroadcastEvent,
) -> Option<v1::stream_event::Payload> {
    match event {
        ServerBroadcastEvent::LiveMetrics(b) => Some(v1::stream_event::Payload::LiveMetrics(b)),
        ServerBroadcastEvent::EntityDelta(d) => Some(v1::stream_event::Payload::EntityDelta(d)),
        ServerBroadcastEvent::EventLog(e) if sub.include_events => {
            Some(v1::stream_event::Payload::EventLog(e))
        }
        ServerBroadcastEvent::Syslog(s) if sub.include_syslog => {
            Some(v1::stream_event::Payload::SyslogEntry(s))
        }
        ServerBroadcastEvent::Trap(t) if sub.include_traps => {
            Some(v1::stream_event::Payload::TrapEntry(t))
        }
        ServerBroadcastEvent::Alert(a) => Some(v1::stream_event::Payload::Alert(a)),
        ServerBroadcastEvent::Outage(o) if sub.include_outages => {
            Some(v1::stream_event::Payload::Outage(o))
        }
        ServerBroadcastEvent::DiscoveryProgress(dp) => {
            Some(v1::stream_event::Payload::Discovery(dp))
        }
        ServerBroadcastEvent::SyslogStats(st) => {
            Some(v1::stream_event::Payload::SyslogStats(st))
        }
        _ => None,
    }
}

async fn apply_undo_payload(
    ctx: &EngineContext,
    payload: &UndoPayload,
    is_undo: bool,
) -> Result<(), ServerError> {
    let secrets = ctx.storage.secrets().clone();
    match payload {
        UndoPayload::Device { before, after } => {
            let target = if is_undo { before } else { after };
            let other = if is_undo { after } else { before };
            match target {
                Some(d) => {
                    let mut d_copy = d.clone();
                    d_copy.version = 0; // force re-insert/overwrite cleanly
                    let saved = if d_copy.id > 0 {
                        let id = d_copy.id;
                        let current_ver: u64 = ctx
                            .storage
                            .read(move |c| {
                                Ok(entities::load_devices(c)?
                                    .into_iter()
                                    .find(|x| x.id == id)
                                    .map_or(0, |x| x.version))
                            })
                            .await?;
                        if current_ver > 0 {
                            d_copy.version = current_ver;
                        } else {
                            d_copy.id = 0;
                        }
                        ctx.storage
                            .write(move |c| entities::save_device(c, &secrets, &d_copy))
                            .await?
                    } else {
                        ctx.storage
                            .write(move |c| entities::save_device(c, &secrets, &d_copy))
                            .await?
                    };
                    ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
                        entity_type: "device".to_string(),
                        deleted: false,
                        entity_id: saved.id,
                        device: Some(saved),
                        ..Default::default()
                    }));
                }
                None => {
                    if let Some(d) = other {
                        let id = d.id;
                        let _ = ctx
                            .storage
                            .write(move |c| delete_entity_versioned(c, "devices", id, 0))
                            .await;
                        ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
                            entity_type: "device".to_string(),
                            deleted: true,
                            entity_id: id,
                            ..Default::default()
                        }));
                    }
                }
            }
        }
        UndoPayload::Link { before, after } => {
            let target = if is_undo { before } else { after };
            let other = if is_undo { after } else { before };
            match target {
                Some(l) => {
                    let mut l_copy = l.clone();
                    if l_copy.id > 0 {
                        let id = l_copy.id;
                        let cur: u64 = ctx
                            .storage
                            .read(move |c| {
                                Ok(entities::load_links(c)?
                                    .into_iter()
                                    .find(|x| x.id == id)
                                    .map_or(0, |x| x.version))
                            })
                            .await?;
                        if cur > 0 {
                            l_copy.version = cur;
                        } else {
                            l_copy.id = 0;
                        }
                    }
                    let saved = ctx
                        .storage
                        .write(move |c| entities::save_link(c, &l_copy))
                        .await?;
                    ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
                        entity_type: "link".to_string(),
                        deleted: false,
                        entity_id: saved.id,
                        link: Some(saved),
                        ..Default::default()
                    }));
                }
                None => {
                    if let Some(l) = other {
                        let id = l.id;
                        let _ = ctx
                            .storage
                            .write(move |c| delete_entity_versioned(c, "links", id, 0))
                            .await;
                        ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
                            entity_type: "link".to_string(),
                            deleted: true,
                            entity_id: id,
                            ..Default::default()
                        }));
                    }
                }
            }
        }
        UndoPayload::Map { before, after } => {
            let target = if is_undo { before } else { after };
            let other = if is_undo { after } else { before };
            match target {
                Some(m) => {
                    let mut m_copy = m.clone();
                    if m_copy.id > 0 {
                        let id = m_copy.id;
                        let cur: u64 = ctx
                            .storage
                            .read(move |c| {
                                Ok(entities::load_maps(c)?
                                    .into_iter()
                                    .find(|x| x.id == id)
                                    .map_or(0, |x| x.version))
                            })
                            .await?;
                        if cur > 0 {
                            m_copy.version = cur;
                        } else {
                            m_copy.id = 0;
                        }
                    }
                    let saved = ctx.storage.write(move |c| entities::save_map(c, &m_copy)).await?;
                    ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
                        entity_type: "map".to_string(),
                        deleted: false,
                        entity_id: saved.id,
                        map: Some(saved),
                        ..Default::default()
                    }));
                }
                None => {
                    if let Some(m) = other {
                        let id = m.id;
                        let _ = ctx
                            .storage
                            .write(move |c| delete_entity_versioned(c, "maps", id, 0))
                            .await;
                        ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
                            entity_type: "map".to_string(),
                            deleted: true,
                            entity_id: id,
                            ..Default::default()
                        }));
                    }
                }
            }
        }
        UndoPayload::Network { before, after } => {
            let target = if is_undo { before } else { after };
            if let Some(n) = target {
                let mut copy = n.clone();
                copy.id = 0;
                let saved = ctx.storage.write(move |c| entities::save_network(c, &copy)).await?;
                ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
                    entity_type: "network".to_string(),
                    deleted: false,
                    entity_id: saved.id,
                    network: Some(saved),
                    ..Default::default()
                }));
            }
        }
        UndoPayload::Submap { before, after } => {
            let target = if is_undo { before } else { after };
            if let Some(s) = target {
                let mut copy = s.clone();
                copy.id = 0;
                let saved = ctx.storage.write(move |c| entities::save_submap(c, &copy)).await?;
                ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
                    entity_type: "submap".to_string(),
                    deleted: false,
                    entity_id: saved.id,
                    submap: Some(saved),
                    ..Default::default()
                }));
            }
        }
        UndoPayload::StaticItem { before, after } => {
            let target = if is_undo { before } else { after };
            if let Some(s) = target {
                let mut copy = s.clone();
                copy.id = 0;
                let saved = ctx.storage.write(move |c| entities::save_static_item(c, &copy)).await?;
                ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
                    entity_type: "static_item".to_string(),
                    deleted: false,
                    entity_id: saved.id,
                    static_item: Some(saved),
                    ..Default::default()
                }));
            }
        }
    }
    Ok(())
}
