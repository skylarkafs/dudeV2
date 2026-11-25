use crate::map_canvas::{render_map_workspace, MapCanvasState};
use crate::panels::admin::{
    render_active_admins_panel, render_admins_groups_panel, render_client_settings_panel,
    render_history_actions_panel, render_notifications_panel, render_server_settings_panel,
    AdminPanelState,
};
use crate::panels::discovery_tools::{
    render_discovery_panel, render_tools_panel, DiscoveryToolsState,
};
use crate::panels::images::{render_images_panel, ImagesPanelState};
use crate::panels::inventory::{
    open_new_device_editor, render_device_types_panel, render_devices_panel,
    render_link_types_panel, render_links_panel, render_networks_and_submaps, render_outages_panel,
    render_probes_panel, render_services_panel, InventoryPanelState,
};
use crate::panels::loader::{connect_to_entry, render_loader_panel, LoaderPanelState};
use crate::panels::observability::{
    render_charts_panel, render_events_panel, render_syslog_panel, render_traps_panel,
    ObservabilityPanelState,
};
use crate::panels::ActiveWorkspaceTab;
use crate::platform::{
    show_windows_toast, start_alarm_sound, stop_alarm_sound,
};
use eframe::egui;
use muda::{Menu, MenuEvent, MenuItem};
use nextdude_client_core::config::ClientLocalConfig;
use nextdude_client_core::connection::{
    spawn_client_worker, ClientCommand, ClientEvent, ClientWorkerHandle, ConnectionPhase,
};
use nextdude_client_core::i18n::{tr, TextKey};
use nextdude_client_core::state::ClientStateCache;
use nextdude_client_core::theme::ThemeMode;
use nextdude_common::proto::v1;
use nextdude_common::APP_NAME;
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{Options, Tree};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tray_icon::{Icon as TrayIconImg, TrayIcon, TrayIconBuilder};

pub struct NextDudeApp {
    local_cfg: ClientLocalConfig,
    worker: ClientWorkerHandle,
    phase: ConnectionPhase,
    state: ClientStateCache,
    active_tab: ActiveWorkspaceTab,
    loader_panel: LoaderPanelState,
    map_canvas: MapCanvasState,
    inventory: InventoryPanelState,
    observability: ObservabilityPanelState,
    discovery_tools: DiscoveryToolsState,
    images_panel: ImagesPanelState,
    admin_panel: AdminPanelState,
    textures: HashMap<String, egui::TextureHandle>,
    requested_images: HashSet<String>,
    show_global_search: bool,
    global_search_query: String,
    alarm_wav_buf: Option<Vec<u8>>,
    alarm_popup_open: bool,
    _tray_icon: Option<TrayIcon>,
    tray_open_id: Option<muda::MenuId>,
    tray_quit_id: Option<muda::MenuId>,
    allow_close: bool,
    current_admin_id: i64,
}

impl NextDudeApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        local_cfg: ClientLocalConfig,
        icon_rgba: Vec<u8>,
    ) -> Self {
        let egui_ctx = cc.egui_ctx.clone();
        let repaint = Arc::new(move || {
            egui_ctx.request_repaint();
        });
        let worker = spawn_client_worker(local_cfg.known_fingerprints.clone(), repaint);

        // Preload built-in geometric SVG icons into egui textures
        let mut textures = HashMap::new();
        preload_builtin_icons(&cc.egui_ctx, &mut textures);

        // Setup Windows System Tray
        let (tray_icon, tray_open_id, tray_quit_id) = setup_tray(&icon_rgba);

        let mut app = Self {
            local_cfg,
            worker,
            phase: ConnectionPhase::Disconnected,
            state: ClientStateCache::default(),
            active_tab: ActiveWorkspaceTab::Loader,
            loader_panel: LoaderPanelState::default(),
            map_canvas: MapCanvasState::default(),
            inventory: InventoryPanelState::default(),
            observability: ObservabilityPanelState::default(),
            discovery_tools: DiscoveryToolsState::default(),
            images_panel: ImagesPanelState::default(),
            admin_panel: AdminPanelState::default(),
            textures,
            requested_images: HashSet::new(),
            show_global_search: false,
            global_search_query: String::new(),
            alarm_wav_buf: None,
            alarm_popup_open: false,
            _tray_icon: tray_icon,
            tray_open_id,
            tray_quit_id,
            allow_close: false,
            current_admin_id: 0,
        };

        // Auto-connect to last server on startup if configured
        if app.local_cfg.auto_connect_last {
            if let Some(last_id) = app.local_cfg.last_server_id.clone() {
                if let Some(srv) = app.local_cfg.servers.iter().find(|s| s.id == last_id).cloned() {
                    let mut cfg_copy = app.local_cfg.clone();
                    connect_to_entry(
                        srv,
                        &mut cfg_copy,
                        &mut app.loader_panel,
                        &app.worker.cmd_tx,
                        &mut |c| {
                            let _ = c.save_to(&ClientLocalConfig::config_file_path());
                        },
                    );
                }
            }
        }

        app
    }

    fn drain_worker_events(&mut self, ctx: &egui::Context) {
        while let Ok(ev) = self.worker.event_rx.try_recv() {
            match ev {
                ClientEvent::PhaseChanged(p) => {
                    if p == ConnectionPhase::Disconnected {
                        stop_alarm_sound();
                    }
                    self.phase = p;
                }
                ClientEvent::TofuPrompt(decision) => {
                    self.loader_panel.pending_tofu = Some(decision);
                }
                ClientEvent::LoggedIn(resp) => {
                    self.current_admin_id = resp.admin_id;
                    self.loader_panel.status_banner =
                        format!("{} ({})", resp.username, resp.group_name);
                    self.active_tab = ActiveWorkspaceTab::Map;
                    if let Some(prefs) = resp.preferences {
                        self.map_canvas.active_map_id = prefs.last_map_id;
                        if let Ok(saved_tab) = serde_json::from_str::<serde_json::Value>(&prefs.panel_layout_json) {
                            if let Some(find_q) = saved_tab.get("map_find").and_then(|v| v.as_str()) {
                                self.map_canvas.find_query = find_q.to_string();
                            }
                        }
                    }
                }
                ClientEvent::StreamFrame(frame) => {
                    if let Some(alarm) = self.state.apply_stream_event(frame) {
                        self.alarm_popup_open = true;
                        show_windows_toast(&alarm.title, &alarm.description);
                        if !self.local_cfg.global_mute {
                            start_alarm_sound(&mut self.alarm_wav_buf);
                        }
                    }
                    if self.state.needs_resync {
                        self.state.needs_resync = false;
                        let _ = self.worker.cmd_tx.try_send(ClientCommand::UpdateSubscription(
                            v1::SubscribeRequest {
                                active_map_id: self.map_canvas.active_map_id,
                                active_device_id: None,
                                active_chart_ids: Vec::new(),
                                include_events: true,
                                include_syslog: true,
                                include_traps: true,
                                include_outages: true,
                            },
                        ));
                    }
                }
                ClientEvent::ToolChunk(chunk) => {
                    self.discovery_tools.tool_output.push(chunk.line);
                }
                ClientEvent::ChartSeriesLoaded(res) => {
                    self.observability.loaded_chart = Some(res);
                }
                ClientEvent::EventsLoaded(res) => {
                    self.state.events = res.items;
                }
                ClientEvent::SyslogLoaded(res) => {
                    self.state.syslogs = res.items;
                }
                ClientEvent::TrapsLoaded(res) => {
                    self.state.traps = res.items;
                }
                ClientEvent::OutagesLoaded(res) => {
                    self.state.outages = res.items;
                }
                ClientEvent::HistoryLoaded(res) => {
                    self.admin_panel.history_actions = res.items;
                }
                ClientEvent::ImageBlobLoaded {
                    image_id,
                    variant,
                    mime_type,
                    data,
                } => {
                    let key = format!("img:{image_id}:{variant}");
                    if let Some(color_img) = decode_image_to_egui(&mime_type, &data) {
                        let tex = ctx.load_texture(key.clone(), color_img, egui::TextureOptions::LINEAR);
                        self.textures.insert(key, tex);
                    }
                }
                ClientEvent::AdminsLoaded(res) => {
                    self.admin_panel.admins = res.users;
                    self.admin_panel.groups = res.groups;
                    if self.admin_panel.new_admin_group_id == 0 {
                        if let Some(g) = self.admin_panel.groups.last() {
                            self.admin_panel.new_admin_group_id = g.id;
                        }
                    }
                }
                ClientEvent::ActiveAdminsLoaded(res) => {
                    self.admin_panel.active_sessions = res.sessions;
                }
                ClientEvent::VendorProfilesLoaded(_) => {}
                ClientEvent::InfoNotice(msg) => {
                    self.loader_panel.status_banner = msg;
                }
                ClientEvent::ErrorNotice(err) => {
                    self.loader_panel.status_banner = format!("Error: {err}");
                }
            }
        }
    }

    fn save_preferences_to_server(&self) {
        if self.phase != ConnectionPhase::Connected {
            return;
        }
        let layout_json = serde_json::json!({
            "map_find": self.map_canvas.find_query,
            "zoom": self.map_canvas.zoom,
        })
        .to_string();
        let filters_json = serde_json::json!({
            "devices": self.inventory.filter,
            "events": self.observability.event_filter,
            "syslog": self.observability.syslog_text_filter,
        })
        .to_string();
        let _ = self.worker.cmd_tx.try_send(ClientCommand::SaveUserPreferences(
            v1::UserPreferences {
                admin_id: self.current_admin_id,
                last_map_id: self.map_canvas.active_map_id,
                panel_layout_json: layout_json,
                filters_json,
            },
        ));
    }
}

impl eframe::App for NextDudeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_worker_events(ctx);

        // Apply selected theme
        match self.local_cfg.theme {
            ThemeMode::Dark => ctx.set_visuals(egui::Visuals::dark()),
            ThemeMode::Light => ctx.set_visuals(egui::Visuals::light()),
        }

        // Handle System Tray Menu clicks
        while let Ok(ev) = MenuEvent::receiver().try_recv() {
            if Some(&ev.id) == self.tray_open_id.as_ref() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            } else if Some(&ev.id) == self.tray_quit_id.as_ref() {
                self.allow_close = true;
                stop_alarm_sound();
                self.save_preferences_to_server();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        // Intercept window close button for close-to-tray
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_close {
            if self.local_cfg.close_to_tray {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            } else {
                stop_alarm_sound();
                self.save_preferences_to_server();
            }
        }

        // Global Keyboard Shortcuts (Ctrl+K, Ctrl+Z, Ctrl+Y)
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::K)) {
            self.show_global_search = !self.show_global_search;
        }
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Z)) {
            let _ = self.worker.cmd_tx.try_send(ClientCommand::Undo);
        }
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Y)) {
            let _ = self.worker.cmd_tx.try_send(ClientCommand::Redo);
        }

        let lang = self.local_cfg.language;

        // Top Menu Bar
        egui::TopBottomPanel::top("top_menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button(tr(lang, TextKey::MenuFile), |ui| {
                    if ui.button(tr(lang, TextKey::AppTitle)).clicked() {
                        self.active_tab = ActiveWorkspaceTab::Loader;
                        ui.close_menu();
                    }
                    if ui.button(tr(lang, TextKey::PanelClientSettings)).clicked() {
                        self.active_tab = ActiveWorkspaceTab::ClientSettings;
                        ui.close_menu();
                    }
                    if ui.button(tr(lang, TextKey::Disconnect)).clicked() {
                        let _ = self.worker.cmd_tx.try_send(ClientCommand::Disconnect);
                        ui.close_menu();
                    }
                    if ui.button("Exit").clicked() {
                        self.allow_close = true;
                        stop_alarm_sound();
                        self.save_preferences_to_server();
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button(tr(lang, TextKey::MenuServer), |ui| {
                    if ui.button(tr(lang, TextKey::PanelNotifications)).clicked() {
                        self.active_tab = ActiveWorkspaceTab::Notifications;
                        ui.close_menu();
                    }
                    if ui.button(tr(lang, TextKey::PanelAdminsGroups)).clicked() {
                        let _ = self.worker.cmd_tx.try_send(ClientCommand::ListAdmins);
                        self.active_tab = ActiveWorkspaceTab::AdminsGroups;
                        ui.close_menu();
                    }
                    if ui.button(tr(lang, TextKey::PanelActiveAdmins)).clicked() {
                        let _ = self.worker.cmd_tx.try_send(ClientCommand::ListActiveAdmins);
                        self.active_tab = ActiveWorkspaceTab::ActiveAdmins;
                        ui.close_menu();
                    }
                    if ui.button(tr(lang, TextKey::PanelHistoryActions)).clicked() {
                        self.active_tab = ActiveWorkspaceTab::HistoryActions;
                        ui.close_menu();
                    }
                    if ui.button(tr(lang, TextKey::PanelServerSettings)).clicked() {
                        self.active_tab = ActiveWorkspaceTab::ServerSettings;
                        ui.close_menu();
                    }
                });
                ui.menu_button(tr(lang, TextKey::MenuTools), |ui| {
                    if ui.button(tr(lang, TextKey::PanelDiscovery)).clicked() {
                        self.active_tab = ActiveWorkspaceTab::Discovery;
                        ui.close_menu();
                    }
                    if ui.button(tr(lang, TextKey::PanelTools)).clicked() {
                        self.active_tab = ActiveWorkspaceTab::Tools;
                        ui.close_menu();
                    }
                });

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let mut muted = self.local_cfg.global_mute;
                    if ui.checkbox(&mut muted, tr(lang, TextKey::MuteAlarm)).changed() {
                        self.local_cfg.global_mute = muted;
                        if muted {
                            stop_alarm_sound();
                        }
                        let _ = self.local_cfg.save_to(&ClientLocalConfig::config_file_path());
                    }
                    if ui.button("Ctrl+K").clicked() {
                        self.show_global_search = true;
                    }
                    let conn_label = match &self.phase {
                        ConnectionPhase::Connected => tr(lang, TextKey::StatusConnected).to_string(),
                        ConnectionPhase::Connecting => tr(lang, TextKey::StatusConnecting).to_string(),
                        ConnectionPhase::Disconnected => tr(lang, TextKey::StatusDisconnected).to_string(),
                        ConnectionPhase::Reconnecting(n) => format!("{} ({n})", tr(lang, TextKey::StatusReconnecting)),
                    };
                    ui.label(conn_label);
                });
            });
        });

        // Left Dude-style Navigation Tree
        egui::SidePanel::left("nav_sidebar").resizable(true).default_width(200.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                let items: &[(ActiveWorkspaceTab, TextKey)] = &[
                    (ActiveWorkspaceTab::Loader, TextKey::Connect),
                    (ActiveWorkspaceTab::Map, TextKey::TabMap),
                    (ActiveWorkspaceTab::Devices, TextKey::PanelDevices),
                    (ActiveWorkspaceTab::Services, TextKey::PanelServices),
                    (ActiveWorkspaceTab::Probes, TextKey::PanelProbes),
                    (ActiveWorkspaceTab::DeviceTypes, TextKey::PanelDeviceTypes),
                    (ActiveWorkspaceTab::Links, TextKey::PanelLinks),
                    (ActiveWorkspaceTab::LinkTypes, TextKey::PanelLinkTypes),
                    (ActiveWorkspaceTab::Networks, TextKey::PanelNetworks),
                    (ActiveWorkspaceTab::Submaps, TextKey::PanelSubmaps),
                    (ActiveWorkspaceTab::Outages, TextKey::PanelOutages),
                    (ActiveWorkspaceTab::Events, TextKey::PanelEvents),
                    (ActiveWorkspaceTab::Syslog, TextKey::PanelSyslog),
                    (ActiveWorkspaceTab::Traps, TextKey::PanelTraps),
                    (ActiveWorkspaceTab::Charts, TextKey::PanelCharts),
                    (ActiveWorkspaceTab::Discovery, TextKey::PanelDiscovery),
                    (ActiveWorkspaceTab::Files, TextKey::PanelFiles),
                    (ActiveWorkspaceTab::Tools, TextKey::PanelTools),
                    (ActiveWorkspaceTab::HistoryActions, TextKey::PanelHistoryActions),
                    (ActiveWorkspaceTab::ActiveAdmins, TextKey::PanelActiveAdmins),
                    (ActiveWorkspaceTab::AdminsGroups, TextKey::PanelAdminsGroups),
                    (ActiveWorkspaceTab::Notifications, TextKey::PanelNotifications),
                    (ActiveWorkspaceTab::ServerSettings, TextKey::PanelServerSettings),
                    (ActiveWorkspaceTab::ClientSettings, TextKey::PanelClientSettings),
                ];
                for &(tab, key) in items {
                    if ui.selectable_label(self.active_tab == tab, tr(lang, key)).clicked() {
                        self.active_tab = tab;
                    }
                }
            });
        });

        // Main Central Panel
        egui::CentralPanel::default().show(ctx, |ui| {
            let cmd_tx = &self.worker.cmd_tx;
            let requested = &mut self.requested_images;
            let mut request_img = |img_id: i64, variant: &str| {
                let k = format!("img:{img_id}:{variant}");
                if requested.insert(k) {
                    let _ = cmd_tx.try_send(ClientCommand::GetImageBlob(v1::GetImageBlobRequest {
                        image_id: img_id,
                        variant: variant.to_string(),
                    }));
                }
            };

            match self.active_tab {
                ActiveWorkspaceTab::Loader => render_loader_panel(
                    ui,
                    ctx,
                    lang,
                    &self.phase,
                    &mut self.local_cfg,
                    &mut self.loader_panel,
                    cmd_tx,
                    &mut |c| {
                        let _ = c.save_to(&ClientLocalConfig::config_file_path());
                    },
                ),
                ActiveWorkspaceTab::Map => {
                    render_map_workspace(
                        ui,
                        ctx,
                        lang,
                        self.local_cfg.theme,
                        &mut self.state,
                        &mut self.map_canvas,
                        &self.textures,
                        cmd_tx,
                        &mut request_img,
                    );
                    if let Some((nx, ny)) = self.map_canvas.pending_new_device_pos.take() {
                        open_new_device_editor(
                            &mut self.inventory,
                            &self.state,
                            self.map_canvas.active_map_id.unwrap_or(1),
                            nx,
                            ny,
                        );
                    }
                    if let Some(did) = self.map_canvas.open_device_dialog.take() {
                        if let Some(dev) = self.state.devices.get(&did).cloned() {
                            self.inventory.edit_device = Some(dev);
                        }
                    }
                    if let Some(lid) = self.map_canvas.open_link_dialog.take() {
                        if let Some(link) = self.state.links.get(&lid).cloned() {
                            self.inventory.edit_link = Some(link);
                            self.active_tab = ActiveWorkspaceTab::Links;
                        }
                    }
                    crate::panels::inventory::render_device_editor_modal(
                        ctx,
                        lang,
                        &self.state,
                        &mut self.inventory,
                        cmd_tx,
                    );
                }
                ActiveWorkspaceTab::Devices => render_devices_panel(
                    ui,
                    ctx,
                    lang,
                    self.local_cfg.theme,
                    &self.state,
                    &self.local_cfg,
                    &mut self.inventory,
                    self.map_canvas.active_map_id,
                    cmd_tx,
                ),
                ActiveWorkspaceTab::Services => {
                    render_services_panel(ui, lang, self.local_cfg.theme, &self.state, cmd_tx)
                }
                ActiveWorkspaceTab::Probes => {
                    render_probes_panel(ui, ctx, lang, &self.state, &mut self.inventory, cmd_tx)
                }
                ActiveWorkspaceTab::DeviceTypes => {
                    render_device_types_panel(ui, ctx, lang, &self.state, &mut self.inventory, cmd_tx)
                }
                ActiveWorkspaceTab::Links => {
                    render_links_panel(ui, ctx, lang, &self.state, &mut self.inventory, cmd_tx)
                }
                ActiveWorkspaceTab::LinkTypes => {
                    render_link_types_panel(ui, ctx, lang, &self.state, &mut self.inventory, cmd_tx)
                }
                ActiveWorkspaceTab::Networks => {
                    render_networks_and_submaps(ui, lang, &self.state, false, cmd_tx)
                }
                ActiveWorkspaceTab::Submaps => {
                    render_networks_and_submaps(ui, lang, &self.state, true, cmd_tx)
                }
                ActiveWorkspaceTab::Outages => render_outages_panel(ui, lang, &self.state),
                ActiveWorkspaceTab::Events => {
                    render_events_panel(ui, lang, &self.state, &mut self.observability, cmd_tx)
                }
                ActiveWorkspaceTab::Syslog => {
                    render_syslog_panel(ui, lang, &self.state, &mut self.observability)
                }
                ActiveWorkspaceTab::Traps => render_traps_panel(ui, lang, &self.state),
                ActiveWorkspaceTab::Charts => {
                    render_charts_panel(ui, ctx, lang, &self.state, &mut self.observability, cmd_tx)
                }
                ActiveWorkspaceTab::Discovery => {
                    render_discovery_panel(ui, lang, &self.state, &mut self.discovery_tools, cmd_tx)
                }
                ActiveWorkspaceTab::Files => render_images_panel(
                    ui,
                    ctx,
                    lang,
                    &self.state,
                    &mut self.images_panel,
                    &self.textures,
                    cmd_tx,
                    &mut request_img,
                ),
                ActiveWorkspaceTab::Tools => {
                    render_tools_panel(ui, lang, &self.state, &mut self.discovery_tools, cmd_tx)
                }
                ActiveWorkspaceTab::HistoryActions => {
                    render_history_actions_panel(ui, lang, &self.admin_panel, cmd_tx)
                }
                ActiveWorkspaceTab::ActiveAdmins => {
                    render_active_admins_panel(ui, lang, &self.admin_panel, cmd_tx)
                }
                ActiveWorkspaceTab::AdminsGroups => {
                    render_admins_groups_panel(ui, lang, &mut self.admin_panel, cmd_tx)
                }
                ActiveWorkspaceTab::Notifications => render_notifications_panel(
                    ui,
                    lang,
                    &self.state,
                    &mut self.admin_panel,
                    cmd_tx,
                ),
                ActiveWorkspaceTab::ServerSettings => render_server_settings_panel(
                    ui,
                    lang,
                    &self.state,
                    &mut self.admin_panel,
                    cmd_tx,
                ),
                ActiveWorkspaceTab::ClientSettings => {
                    render_client_settings_panel(ui, &mut self.local_cfg, &mut |c| {
                        let _ = c.save_to(&ClientLocalConfig::config_file_path());
                    })
                }
            }
        });

        // Global Search Modal (Ctrl+K)
        if self.show_global_search {
            let mut open = true;
            egui::Window::new("Global Search (Ctrl+K)").open(&mut open).show(ctx, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.global_search_query)
                        .hint_text(tr(lang, TextKey::SearchGlobalPlaceholder)),
                );
                let q = self.global_search_query.to_ascii_lowercase();
                if !q.is_empty() {
                    for d in self.state.devices.values().take(30) {
                        if d.name.to_ascii_lowercase().contains(&q) || d.address.contains(&q) {
                            if ui.button(format!("[Device] {} ({})", d.name, d.address)).clicked() {
                                self.map_canvas.active_map_id = Some(d.map_id);
                                self.map_canvas.selected_devices.clear();
                                self.map_canvas.selected_devices.insert(d.id);
                                self.active_tab = ActiveWorkspaceTab::Map;
                                self.show_global_search = false;
                            }
                        }
                    }
                    for m in self.state.maps.values() {
                        if m.name.to_ascii_lowercase().contains(&q) {
                            if ui.button(format!("[Map] {}", m.name)).clicked() {
                                self.map_canvas.active_map_id = Some(m.id);
                                self.active_tab = ActiveWorkspaceTab::Map;
                                self.show_global_search = false;
                            }
                        }
                    }
                }
            });
            if !open {
                self.show_global_search = false;
            }
        }

        // Active Alarm Popup + Sound Acknowledge
        if self.alarm_popup_open && !self.state.active_alarms.is_empty() {
            let mut ack_all = false;
            egui::Window::new("Active Network Alarms")
                .collapsible(false)
                .show(ctx, |ui| {
                    for alarm in self.state.active_alarms.iter().take(6) {
                        ui.label(format!("[{}] {} — {}", alarm.device_name, alarm.title, alarm.description));
                    }
                    ui.horizontal(|ui| {
                        if ui.button(tr(lang, TextKey::Acknowledge)).clicked() {
                            ack_all = true;
                        }
                        if ui.button(tr(lang, TextKey::MuteAlarm)).clicked() {
                            stop_alarm_sound();
                            self.alarm_popup_open = false;
                        }
                    });
                });
            if ack_all {
                stop_alarm_sound();
                for alarm in self.state.active_alarms.drain(..) {
                    let _ = self
                        .worker
                        .cmd_tx
                        .try_send(ClientCommand::AcknowledgeAlert(alarm.event_id));
                }
                self.alarm_popup_open = false;
            }
        }
    }
}

fn preload_builtin_icons(
    ctx: &egui::Context,
    textures: &mut HashMap<String, egui::TextureHandle>,
) {
    let icons: &[(&str, &str)] = &[
        ("router", include_str!("../../../assets/icons/router.svg")),
        ("switch", include_str!("../../../assets/icons/switch.svg")),
        ("crs", include_str!("../../../assets/icons/crs.svg")),
        ("olt", include_str!("../../../assets/icons/olt.svg")),
        ("onu", include_str!("../../../assets/icons/onu.svg")),
        ("ap", include_str!("../../../assets/icons/ap.svg")),
        ("radio", include_str!("../../../assets/icons/radio.svg")),
        ("server", include_str!("../../../assets/icons/server.svg")),
        ("firewall", include_str!("../../../assets/icons/firewall.svg")),
        ("generic", include_str!("../../../assets/icons/generic.svg")),
    ];
    for &(key, svg) in icons {
        if let Some(img) = decode_image_to_egui("image/svg+xml", svg.as_bytes()) {
            let tkey = format!("builtin:{key}");
            let tex = ctx.load_texture(tkey.clone(), img, egui::TextureOptions::LINEAR);
            textures.insert(tkey, tex);
        }
    }
}

fn decode_image_to_egui(mime: &str, data: &[u8]) -> Option<egui::ColorImage> {
    if mime == "image/svg+xml" {
        let tree = Tree::from_data(data, &Options::default()).ok()?;
        let sz = tree.size().to_int_size();
        let mut pixmap = Pixmap::new(64, 64)?;
        let sx = 64.0 / (sz.width().max(1) as f32);
        let sy = 64.0 / (sz.height().max(1) as f32);
        resvg::render(&tree, Transform::from_scale(sx, sy), &mut pixmap.as_mut());
        return Some(egui::ColorImage::from_rgba_unmultiplied(
            [64, 64],
            pixmap.data(),
        ));
    }
    let dyn_img = image::load_from_memory(data).ok()?;
    let rgba = dyn_img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [w as usize, h as usize],
        rgba.as_raw(),
    ))
}

fn setup_tray(icon_rgba: &[u8]) -> (Option<TrayIcon>, Option<muda::MenuId>, Option<muda::MenuId>) {
    let menu = Menu::new();
    let open_item = MenuItem::new(format!("Open {APP_NAME}"), true, None);
    let quit_item = MenuItem::new("Exit", true, None);
    let open_id = open_item.id().clone();
    let quit_id = quit_item.id().clone();
    let _ = menu.append_items(&[&open_item, &quit_item]);

    let Ok(tray_img) = TrayIconImg::from_rgba(icon_rgba.to_vec(), 32, 32) else {
        return (None, None, None);
    };
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip(APP_NAME)
        .with_icon(tray_img)
        .build()
        .ok();
    (tray, Some(open_id), Some(quit_id))
}
