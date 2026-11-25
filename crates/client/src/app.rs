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
    open_new_device_editor, render_device_editor_modal, render_device_types_panel,
    render_devices_panel, render_link_types_panel, render_links_panel, render_networks_and_submaps,
    render_outages_panel, render_probes_panel, render_services_panel, InventoryPanelState,
};
use crate::panels::loader::{connect_to_entry, render_loader_panel, LoaderPanelState};
use crate::panels::observability::{
    render_charts_panel, render_events_panel, render_syslog_panel, render_traps_panel,
    ObservabilityPanelState,
};
use crate::panels::ActiveWorkspaceTab;
use crate::platform::{show_windows_toast, start_alarm_sound, stop_alarm_sound};
use crate::ui_kit::{
    accent_rgb, apply_app_theme, card_frame, muted_color, notice_frame, pick_lang,
    severity_pill, status_pill,
};
use eframe::egui;
use muda::{Menu, MenuEvent, MenuItem};
use nextdude_client_core::config::ClientLocalConfig;
use nextdude_client_core::connection::{
    spawn_client_worker, ClientCommand, ClientEvent, ClientWorkerHandle, ConnectionPhase,
};
use nextdude_client_core::i18n::{tr, Language, TextKey};
use nextdude_client_core::state::ClientStateCache;
use nextdude_client_core::theme::{state_color, RgbColor, ThemeMode};
use nextdude_common::proto::v1;
use nextdude_common::APP_NAME;
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{Options, Tree};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tray_icon::{Icon as TrayIconImg, TrayIcon, TrayIconBuilder};

pub struct NextDudeApp {
    local_cfg: ClientLocalConfig,
    applied_theme: Option<ThemeMode>,
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
    focus_global_search: bool,
    global_search_query: String,
    show_help_dialog: bool,
    alarm_wav_buf: Option<Vec<u8>>,
    alarm_popup_open: bool,
    _tray_icon: Option<TrayIcon>,
    tray_open_id: Option<muda::MenuId>,
    tray_quit_id: Option<muda::MenuId>,
    allow_close: bool,
    current_admin_id: i64,
    connected_server_label: String,
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

        apply_app_theme(&cc.egui_ctx, local_cfg.theme);

        // Preload built-in geometric SVG icons into egui textures
        let mut textures = HashMap::new();
        preload_builtin_icons(&cc.egui_ctx, &mut textures);

        // Setup Windows System Tray
        let (tray_icon, tray_open_id, tray_quit_id) = setup_tray(&icon_rgba);

        let initial_theme = local_cfg.theme;
        let mut app = Self {
            local_cfg,
            applied_theme: Some(initial_theme),
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
            focus_global_search: false,
            global_search_query: String::new(),
            show_help_dialog: false,
            alarm_wav_buf: None,
            alarm_popup_open: false,
            _tray_icon: tray_icon,
            tray_open_id,
            tray_quit_id,
            allow_close: false,
            current_admin_id: 0,
            connected_server_label: String::new(),
        };

        // Auto-connect to last server on startup if configured
        if app.local_cfg.auto_connect_last {
            if let Some(last_id) = app.local_cfg.last_server_id.clone() {
                if let Some(srv) = app.local_cfg.servers.iter().find(|s| s.id == last_id).cloned() {
                    app.connected_server_label = format!("{} ({}:{})", srv.name, srv.address, srv.port);
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
                        self.connected_server_label.clear();
                    }
                    self.phase = p;
                }
                ClientEvent::TofuPrompt(decision) => {
                    self.loader_panel.pending_tofu = Some(decision);
                    self.active_tab = ActiveWorkspaceTab::Loader;
                }
                ClientEvent::LoggedIn(resp) => {
                    self.current_admin_id = resp.admin_id;
                    self.loader_panel.status_banner =
                        format!("{} ({})", resp.username, resp.group_name);
                    if let Some(ref last_id) = self.local_cfg.last_server_id {
                        if let Some(srv) = self.local_cfg.servers.iter().find(|s| &s.id == last_id) {
                            self.connected_server_label =
                                format!("{} ({}:{})", srv.name, srv.address, srv.port);
                        }
                    }
                    self.active_tab = ActiveWorkspaceTab::Map;
                    if let Some(prefs) = resp.preferences {
                        self.map_canvas.active_map_id = prefs.last_map_id;
                        if let Ok(saved_tab) = serde_json::from_str::<serde_json::Value>(&prefs.panel_layout_json) {
                            if let Some(find_q) = saved_tab.get("map_find").and_then(|v| v.as_str()) {
                                self.map_canvas.find_query = find_q.to_string();
                            }
                            if let Some(z) = saved_tab.get("zoom").and_then(|v| v.as_f64()) {
                                self.map_canvas.zoom = (z as f32).clamp(0.15, 4.0);
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

    fn switch_to_tab(&mut self, tab: ActiveWorkspaceTab) {
        self.active_tab = tab;
        match tab {
            ActiveWorkspaceTab::AdminsGroups if self.admin_panel.admins.is_empty() => {
                let _ = self.worker.cmd_tx.try_send(ClientCommand::ListAdmins);
            }
            ActiveWorkspaceTab::ActiveAdmins => {
                let _ = self.worker.cmd_tx.try_send(ClientCommand::ListActiveAdmins);
            }
            ActiveWorkspaceTab::HistoryActions if self.admin_panel.history_actions.is_empty() => {
                let now = chrono::Utc::now().timestamp();
                let _ = self.worker.cmd_tx.try_send(ClientCommand::QueryHistoryActions(
                    v1::PageQueryRequest {
                        start_utc: now - 30 * 86400,
                        end_utc: now + 60,
                        device_id: None,
                        text_filter: String::new(),
                        min_severity: None,
                        limit: 200,
                        offset: 0,
                    },
                ));
            }
            _ => {}
        }
    }
}

impl eframe::App for NextDudeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_worker_events(ctx);

        // Apply custom Slate/Cobalt NOC theme only when theme changes
        if self.applied_theme != Some(self.local_cfg.theme) {
            apply_app_theme(ctx, self.local_cfg.theme);
            self.applied_theme = Some(self.local_cfg.theme);
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
            self.focus_global_search = self.show_global_search;
        }
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Z)) {
            let _ = self.worker.cmd_tx.try_send(ClientCommand::Undo);
        }
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(egui::Key::Y)) {
            let _ = self.worker.cmd_tx.try_send(ClientCommand::Redo);
        }

        let lang = self.local_cfg.language;
        let theme = self.local_cfg.theme;

        // Top Menu & Status Bar
        egui::TopBottomPanel::top("top_menu_bar").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button(tr(lang, TextKey::MenuFile), |ui| {
                    if ui.button(tr(lang, TextKey::AppTitle)).clicked() {
                        self.switch_to_tab(ActiveWorkspaceTab::Loader);
                        ui.close_menu();
                    }
                    if ui.button(tr(lang, TextKey::PanelClientSettings)).clicked() {
                        self.switch_to_tab(ActiveWorkspaceTab::ClientSettings);
                        ui.close_menu();
                    }
                    if ui
                        .add_enabled(
                            self.phase != ConnectionPhase::Disconnected,
                            egui::Button::new(tr(lang, TextKey::Disconnect)),
                        )
                        .clicked()
                    {
                        let _ = self.worker.cmd_tx.try_send(ClientCommand::Disconnect);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button(pick_lang(lang, "Keluar", "Exit")).clicked() {
                        self.allow_close = true;
                        stop_alarm_sound();
                        self.save_preferences_to_server();
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });

                ui.menu_button(tr(lang, TextKey::MenuMap), |ui| {
                    if ui.button(pick_lang(lang, "Buka Peta Jaringan", "Open Network Map")).clicked() {
                        self.switch_to_tab(ActiveWorkspaceTab::Map);
                        ui.close_menu();
                    }
                    if ui.button(pick_lang(lang, "Peta Baru...", "New Map...")).clicked() {
                        self.switch_to_tab(ActiveWorkspaceTab::Map);
                        self.map_canvas.show_new_map_dialog = true;
                        self.map_canvas.new_map_name.clear();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button(pick_lang(lang, "Perbesar (+)", "Zoom In (+)")).clicked() {
                        self.map_canvas.zoom_in();
                        ui.close_menu();
                    }
                    if ui.button(pick_lang(lang, "Perkecil (-)", "Zoom Out (-)")).clicked() {
                        self.map_canvas.zoom_out();
                        ui.close_menu();
                    }
                    if ui.button(pick_lang(lang, "Reset Zoom (100%)", "Reset Zoom (100%)")).clicked() {
                        self.map_canvas.reset_view();
                        ui.close_menu();
                    }
                    if ui.button(pick_lang(lang, "Sesuaikan ke Layar", "Fit Map to Screen")).clicked() {
                        self.switch_to_tab(ActiveWorkspaceTab::Map);
                        self.map_canvas.fit_view_requested = true;
                        ui.close_menu();
                    }
                });

                ui.menu_button(tr(lang, TextKey::MenuServer), |ui| {
                    if ui.button(tr(lang, TextKey::PanelNotifications)).clicked() {
                        self.switch_to_tab(ActiveWorkspaceTab::Notifications);
                        ui.close_menu();
                    }
                    if ui.button(tr(lang, TextKey::PanelAdminsGroups)).clicked() {
                        self.switch_to_tab(ActiveWorkspaceTab::AdminsGroups);
                        ui.close_menu();
                    }
                    if ui.button(tr(lang, TextKey::PanelActiveAdmins)).clicked() {
                        self.switch_to_tab(ActiveWorkspaceTab::ActiveAdmins);
                        ui.close_menu();
                    }
                    if ui.button(tr(lang, TextKey::PanelHistoryActions)).clicked() {
                        self.switch_to_tab(ActiveWorkspaceTab::HistoryActions);
                        ui.close_menu();
                    }
                    if ui.button(tr(lang, TextKey::PanelServerSettings)).clicked() {
                        self.switch_to_tab(ActiveWorkspaceTab::ServerSettings);
                        ui.close_menu();
                    }
                });

                ui.menu_button(tr(lang, TextKey::MenuTools), |ui| {
                    if ui.button(tr(lang, TextKey::PanelDiscovery)).clicked() {
                        self.switch_to_tab(ActiveWorkspaceTab::Discovery);
                        ui.close_menu();
                    }
                    if ui.button(tr(lang, TextKey::PanelTools)).clicked() {
                        self.switch_to_tab(ActiveWorkspaceTab::Tools);
                        ui.close_menu();
                    }
                });

                ui.menu_button(tr(lang, TextKey::MenuView), |ui| {
                    let theme_btn = match self.local_cfg.theme {
                        ThemeMode::Dark => tr(lang, TextKey::ThemeLight),
                        ThemeMode::Light => tr(lang, TextKey::ThemeDark),
                    };
                    if ui.button(theme_btn).clicked() {
                        self.local_cfg.theme = match self.local_cfg.theme {
                            ThemeMode::Dark => ThemeMode::Light,
                            ThemeMode::Light => ThemeMode::Dark,
                        };
                        let _ = self.local_cfg.save_to(&ClientLocalConfig::config_file_path());
                        ui.close_menu();
                    }
                    let lang_btn = match self.local_cfg.language {
                        Language::Id => "Language: English",
                        Language::En => "Bahasa: Indonesia",
                    };
                    if ui.button(lang_btn).clicked() {
                        self.local_cfg.language = match self.local_cfg.language {
                            Language::Id => Language::En,
                            Language::En => Language::Id,
                        };
                        let _ = self.local_cfg.save_to(&ClientLocalConfig::config_file_path());
                        ui.close_menu();
                    }
                    if ui.button("Global Search (Ctrl+K)").clicked() {
                        self.show_global_search = true;
                        self.focus_global_search = true;
                        ui.close_menu();
                    }
                });

                ui.menu_button(tr(lang, TextKey::MenuHelp), |ui| {
                    if ui
                        .button(pick_lang(
                            lang,
                            "Pintasan Keyboard & Tentang",
                            "Keyboard Shortcuts & About",
                        ))
                        .clicked()
                    {
                        self.show_help_dialog = true;
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
                        self.focus_global_search = true;
                    }
                    let (conn_label, conn_rgb) = match &self.phase {
                        ConnectionPhase::Connected => (
                            if self.connected_server_label.is_empty() {
                                tr(lang, TextKey::StatusConnected).to_string()
                            } else {
                                format!(
                                    "{} • {}",
                                    tr(lang, TextKey::StatusConnected),
                                    self.connected_server_label
                                )
                            },
                            state_color(theme, v1::ProbeState::Up),
                        ),
                        ConnectionPhase::Connecting => (
                            tr(lang, TextKey::StatusConnecting).to_string(),
                            RgbColor::new(245, 158, 11),
                        ),
                        ConnectionPhase::Disconnected => (
                            tr(lang, TextKey::StatusDisconnected).to_string(),
                            state_color(theme, v1::ProbeState::Unknown),
                        ),
                        ConnectionPhase::Reconnecting(n) => (
                            format!("{} ({n})", tr(lang, TextKey::StatusReconnecting)),
                            RgbColor::new(245, 158, 11),
                        ),
                    };
                    status_pill(ui, &conn_label, conn_rgb);
                });
            });
        });

        // Bottom Live Telemetry & Notice Bar
        let mut dev_up = 0usize;
        let mut dev_down = 0usize;
        let mut dev_unreach = 0usize;
        let mut dev_partial = 0usize;
        for d in self.state.devices.values() {
            match v1::ProbeState::try_from(d.aggregate_state).unwrap_or(v1::ProbeState::Unknown) {
                v1::ProbeState::Up => dev_up += 1,
                v1::ProbeState::Down => dev_down += 1,
                v1::ProbeState::Unreachable => dev_unreach += 1,
                v1::ProbeState::Partial => dev_partial += 1,
                _ => {}
            }
        }
        let active_outages = self
            .state
            .outages
            .iter()
            .filter(|o| o.ended_utc.is_none())
            .count();

        egui::TopBottomPanel::bottom("bottom_status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if !self.loader_panel.status_banner.is_empty() {
                    let is_err = self.loader_panel.status_banner.starts_with("Error:");
                    let col = if is_err {
                        state_color(theme, v1::ProbeState::Down)
                    } else {
                        accent_rgb(theme)
                    };
                    status_pill(ui, &self.loader_panel.status_banner, col);
                    if ui.small_button("x").clicked() {
                        self.loader_panel.status_banner.clear();
                    }
                } else {
                    ui.label(
                        egui::RichText::new(format!("{APP_NAME} v{}", env!("CARGO_PKG_VERSION")))
                            .small()
                            .color(muted_color(theme)),
                    );
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.monospace(format!("Zoom {:.0}%", self.map_canvas.zoom * 100.0));
                    if self.state.syslog_stats.dropped_total > 0 {
                        status_pill(
                            ui,
                            &format!("Syslog Drops: {}", self.state.syslog_stats.dropped_total),
                            RgbColor::new(245, 158, 11),
                        );
                    }
                    if active_outages > 0 {
                        status_pill(
                            ui,
                            &format!("Outages: {active_outages}"),
                            state_color(theme, v1::ProbeState::Down),
                        );
                    }
                    if dev_partial > 0 {
                        status_pill(
                            ui,
                            &format!("Partial: {dev_partial}"),
                            state_color(theme, v1::ProbeState::Partial),
                        );
                    }
                    if dev_unreach > 0 {
                        status_pill(
                            ui,
                            &format!("Unreach: {dev_unreach}"),
                            state_color(theme, v1::ProbeState::Unreachable),
                        );
                    }
                    if dev_down > 0 {
                        status_pill(
                            ui,
                            &format!("Down: {dev_down}"),
                            state_color(theme, v1::ProbeState::Down),
                        );
                    }
                    status_pill(
                        ui,
                        &format!("Up: {dev_up}"),
                        state_color(theme, v1::ProbeState::Up),
                    );
                });
            });
        });

        // Left Sectioned Navigation Sidebar
        egui::SidePanel::left("nav_sidebar")
            .resizable(true)
            .default_width(214.0)
            .min_width(180.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    type NavSection<'a> = (&'a str, &'a str, &'a [(ActiveWorkspaceTab, TextKey)]);
                    let sections: &[NavSection<'_>] = &[
                        (
                            "RUANG KERJA",
                            "WORKSPACE",
                            &[
                                (ActiveWorkspaceTab::Loader, TextKey::Connect),
                                (ActiveWorkspaceTab::Map, TextKey::TabMap),
                            ],
                        ),
                        (
                            "INVENTARIS",
                            "INVENTORY",
                            &[
                                (ActiveWorkspaceTab::Devices, TextKey::PanelDevices),
                                (ActiveWorkspaceTab::Services, TextKey::PanelServices),
                                (ActiveWorkspaceTab::Probes, TextKey::PanelProbes),
                                (ActiveWorkspaceTab::DeviceTypes, TextKey::PanelDeviceTypes),
                                (ActiveWorkspaceTab::Links, TextKey::PanelLinks),
                                (ActiveWorkspaceTab::LinkTypes, TextKey::PanelLinkTypes),
                                (ActiveWorkspaceTab::Networks, TextKey::PanelNetworks),
                                (ActiveWorkspaceTab::Submaps, TextKey::PanelSubmaps),
                            ],
                        ),
                        (
                            "OBSERVABILITAS",
                            "OBSERVABILITY",
                            &[
                                (ActiveWorkspaceTab::Outages, TextKey::PanelOutages),
                                (ActiveWorkspaceTab::Events, TextKey::PanelEvents),
                                (ActiveWorkspaceTab::Syslog, TextKey::PanelSyslog),
                                (ActiveWorkspaceTab::Traps, TextKey::PanelTraps),
                                (ActiveWorkspaceTab::Charts, TextKey::PanelCharts),
                            ],
                        ),
                        (
                            "OPERASIONAL",
                            "OPERATIONS",
                            &[
                                (ActiveWorkspaceTab::Discovery, TextKey::PanelDiscovery),
                                (ActiveWorkspaceTab::Files, TextKey::PanelFiles),
                                (ActiveWorkspaceTab::Tools, TextKey::PanelTools),
                            ],
                        ),
                        (
                            "ADMINISTRASI",
                            "ADMINISTRATION",
                            &[
                                (ActiveWorkspaceTab::HistoryActions, TextKey::PanelHistoryActions),
                                (ActiveWorkspaceTab::ActiveAdmins, TextKey::PanelActiveAdmins),
                                (ActiveWorkspaceTab::AdminsGroups, TextKey::PanelAdminsGroups),
                                (ActiveWorkspaceTab::Notifications, TextKey::PanelNotifications),
                                (ActiveWorkspaceTab::ServerSettings, TextKey::PanelServerSettings),
                                (ActiveWorkspaceTab::ClientSettings, TextKey::PanelClientSettings),
                            ],
                        ),
                    ];

                    let mut next_tab: Option<ActiveWorkspaceTab> = None;
                    for (idx, &(sec_id, sec_en, items)) in sections.iter().enumerate() {
                        if idx > 0 {
                            ui.add_space(6.0);
                        }
                        ui.label(
                            egui::RichText::new(pick_lang(lang, sec_id, sec_en))
                                .small()
                                .strong()
                                .color(muted_color(theme)),
                        );
                        ui.add_space(2.0);
                        for &(tab, key) in items {
                            let badge = match tab {
                                ActiveWorkspaceTab::Devices if !self.state.devices.is_empty() => {
                                    Some(format!(" ({})", self.state.devices.len()))
                                }
                                ActiveWorkspaceTab::Outages if active_outages > 0 => {
                                    Some(format!(" ({active_outages})"))
                                }
                                ActiveWorkspaceTab::Links if !self.state.links.is_empty() => {
                                    Some(format!(" ({})", self.state.links.len()))
                                }
                                _ => None,
                            };
                            let label = match badge {
                                Some(b) => format!("{}{b}", tr(lang, key)),
                                None => tr(lang, key).to_string(),
                            };
                            if ui
                                .add_sized(
                                    [ui.available_width(), 24.0],
                                    egui::SelectableLabel::new(self.active_tab == tab, label),
                                )
                                .clicked()
                            {
                                next_tab = Some(tab);
                            }
                        }
                    }
                    if let Some(tab) = next_tab {
                        self.switch_to_tab(tab);
                    }
                });
            });

        // Main Central Panel
        egui::CentralPanel::default().show(ctx, |ui| {
            let show_disconnected_guard = self.phase == ConnectionPhase::Disconnected
                && self.state.maps.is_empty()
                && !matches!(
                    self.active_tab,
                    ActiveWorkspaceTab::Loader | ActiveWorkspaceTab::ClientSettings
                );

            if show_disconnected_guard {
                notice_frame(theme, accent_rgb(theme)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.strong(pick_lang(
                            lang,
                            "Belum terhubung ke server NextDude.",
                            "Not connected to a NextDude server.",
                        ));
                        if ui
                            .button(pick_lang(
                                lang,
                                "Buka Pengelola Koneksi",
                                "Open Server Loader",
                            ))
                            .clicked()
                        {
                            self.switch_to_tab(ActiveWorkspaceTab::Loader);
                        }
                    });
                });
                ui.add_space(8.0);
            }

            let cmd_tx_owned = self.worker.cmd_tx.clone();
            let cmd_tx = &cmd_tx_owned;
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
                    theme,
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
                        theme,
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
                            self.inventory.edit_tags_csv = dev.tags.join(", ");
                            self.inventory.edit_device = Some(dev);
                            self.inventory.edit_community.clear();
                            self.inventory.edit_auth_pass.clear();
                            self.inventory.edit_priv_pass.clear();
                        }
                    }
                    if let Some(lid) = self.map_canvas.open_link_dialog.take() {
                        if let Some(link) = self.state.links.get(&lid).cloned() {
                            self.inventory.edit_link = Some(link);
                            self.switch_to_tab(ActiveWorkspaceTab::Links);
                        }
                    }
                    if let Some((did, tkind)) = self.map_canvas.open_tool_for_device.take() {
                        if let Some(dev) = self.state.devices.get(&did) {
                            self.discovery_tools.tool_kind = tkind;
                            self.discovery_tools.tool_device_id = Some(dev.id);
                            self.discovery_tools.tool_target = dev.address.clone();
                            self.discovery_tools.tool_output.clear();
                            let _ = cmd_tx.try_send(ClientCommand::RunTool(v1::RunToolRequest {
                                kind: tkind as i32,
                                target: dev.address.clone(),
                                port: 0,
                                oid: self.discovery_tools.tool_oid.clone(),
                                device_id: Some(dev.id),
                                count: 5,
                            }));
                            self.switch_to_tab(ActiveWorkspaceTab::Tools);
                        }
                    }
                    render_device_editor_modal(
                        ctx,
                        lang,
                        theme,
                        &self.state,
                        &mut self.inventory,
                        cmd_tx,
                    );
                }
                ActiveWorkspaceTab::Devices => render_devices_panel(
                    ui,
                    ctx,
                    lang,
                    theme,
                    &self.state,
                    &self.local_cfg,
                    &mut self.inventory,
                    self.map_canvas.active_map_id,
                    cmd_tx,
                ),
                ActiveWorkspaceTab::Services => {
                    render_services_panel(ui, lang, theme, &self.state, cmd_tx)
                }
                ActiveWorkspaceTab::Probes => render_probes_panel(
                    ui,
                    ctx,
                    lang,
                    theme,
                    &self.state,
                    &mut self.inventory,
                    cmd_tx,
                ),
                ActiveWorkspaceTab::DeviceTypes => render_device_types_panel(
                    ui,
                    ctx,
                    lang,
                    theme,
                    &self.state,
                    &mut self.inventory,
                    cmd_tx,
                ),
                ActiveWorkspaceTab::Links => render_links_panel(
                    ui,
                    ctx,
                    lang,
                    theme,
                    &self.state,
                    &mut self.inventory,
                    cmd_tx,
                ),
                ActiveWorkspaceTab::LinkTypes => render_link_types_panel(
                    ui,
                    ctx,
                    lang,
                    theme,
                    &self.state,
                    &mut self.inventory,
                    cmd_tx,
                ),
                ActiveWorkspaceTab::Networks => {
                    render_networks_and_submaps(ui, lang, theme, &self.state, false, cmd_tx)
                }
                ActiveWorkspaceTab::Submaps => {
                    render_networks_and_submaps(ui, lang, theme, &self.state, true, cmd_tx)
                }
                ActiveWorkspaceTab::Outages => render_outages_panel(ui, lang, theme, &self.state),
                ActiveWorkspaceTab::Events => render_events_panel(
                    ui,
                    lang,
                    theme,
                    &self.state,
                    &mut self.observability,
                    cmd_tx,
                ),
                ActiveWorkspaceTab::Syslog => render_syslog_panel(
                    ui,
                    lang,
                    theme,
                    &self.state,
                    &mut self.observability,
                ),
                ActiveWorkspaceTab::Traps => render_traps_panel(ui, lang, theme, &self.state),
                ActiveWorkspaceTab::Charts => render_charts_panel(
                    ui,
                    ctx,
                    lang,
                    theme,
                    &self.state,
                    &mut self.observability,
                    cmd_tx,
                ),
                ActiveWorkspaceTab::Discovery => render_discovery_panel(
                    ui,
                    lang,
                    theme,
                    &self.state,
                    &mut self.discovery_tools,
                    cmd_tx,
                ),
                ActiveWorkspaceTab::Files => render_images_panel(
                    ui,
                    ctx,
                    lang,
                    theme,
                    &self.state,
                    &mut self.images_panel,
                    &self.textures,
                    cmd_tx,
                    &mut request_img,
                ),
                ActiveWorkspaceTab::Tools => render_tools_panel(
                    ui,
                    lang,
                    theme,
                    &self.state,
                    &mut self.discovery_tools,
                    cmd_tx,
                ),
                ActiveWorkspaceTab::HistoryActions => {
                    render_history_actions_panel(ui, lang, theme, &self.admin_panel, cmd_tx)
                }
                ActiveWorkspaceTab::ActiveAdmins => {
                    render_active_admins_panel(ui, lang, theme, &self.admin_panel, cmd_tx)
                }
                ActiveWorkspaceTab::AdminsGroups => {
                    render_admins_groups_panel(ui, lang, theme, &mut self.admin_panel, cmd_tx)
                }
                ActiveWorkspaceTab::Notifications => render_notifications_panel(
                    ui,
                    lang,
                    theme,
                    &self.state,
                    &mut self.admin_panel,
                    cmd_tx,
                ),
                ActiveWorkspaceTab::ServerSettings => render_server_settings_panel(
                    ui,
                    lang,
                    theme,
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
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                open = false;
            }
            egui::Window::new(pick_lang(
                lang,
                "Pencarian Global (Ctrl+K)",
                "Global Search (Ctrl+K)",
            ))
            .open(&mut open)
            .resizable(true)
            .min_width(440.0)
            .show(ctx, |ui| {
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.global_search_query)
                        .desired_width(f32::INFINITY)
                        .hint_text(tr(lang, TextKey::SearchGlobalPlaceholder)),
                );
                if self.focus_global_search {
                    edit.request_focus();
                    self.focus_global_search = false;
                }
                ui.add_space(6.0);
                let q = self.global_search_query.trim().to_ascii_lowercase();
                if q.is_empty() {
                    ui.label(
                        egui::RichText::new(pick_lang(
                            lang,
                            "Ketik nama perangkat, IP, atau nama peta...",
                            "Type device name, IP address, or map name...",
                        ))
                        .small()
                        .color(muted_color(theme)),
                    );
                } else {
                    let mut count = 0usize;
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        for d in self.state.devices.values() {
                            if d.name.to_ascii_lowercase().contains(&q) || d.address.contains(&q) {
                                count += 1;
                                if ui
                                    .button(format!("[Device] {} ({})", d.name, d.address))
                                    .clicked()
                                {
                                    self.map_canvas.active_map_id = Some(d.map_id);
                                    self.map_canvas.selected_devices.clear();
                                    self.map_canvas.selected_devices.insert(d.id);
                                    self.active_tab = ActiveWorkspaceTab::Map;
                                    self.show_global_search = false;
                                }
                                if count >= 30 {
                                    break;
                                }
                            }
                        }
                        for m in self.state.maps.values() {
                            if m.name.to_ascii_lowercase().contains(&q) {
                                count += 1;
                                if ui.button(format!("[Map] {}", m.name)).clicked() {
                                    self.map_canvas.active_map_id = Some(m.id);
                                    self.active_tab = ActiveWorkspaceTab::Map;
                                    self.show_global_search = false;
                                }
                            }
                        }
                        if count == 0 {
                            ui.label(
                                egui::RichText::new(pick_lang(
                                    lang,
                                    "Tidak ada hasil yang cocok.",
                                    "No matching devices or maps.",
                                ))
                                .color(muted_color(theme)),
                            );
                        }
                    });
                }
            });
            if !open {
                self.show_global_search = false;
            }
        }

        // Active Alarm Popup + Sound Acknowledge
        if self.alarm_popup_open && !self.state.active_alarms.is_empty() {
            let mut ack_all = false;
            egui::Window::new(pick_lang(
                lang,
                "Alarm Jaringan Aktif",
                "Active Network Alarms",
            ))
            .collapsible(false)
            .min_width(460.0)
            .show(ctx, |ui| {
                card_frame(theme).show(ui, |ui| {
                    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                        for alarm in self.state.active_alarms.iter().take(10) {
                            let sev = v1::Severity::try_from(alarm.severity)
                                .unwrap_or(v1::Severity::Warning);
                            ui.horizontal(|ui| {
                                severity_pill(ui, lang, theme, sev);
                                ui.strong(&alarm.device_name);
                                ui.label(format!("— {}: {}", alarm.title, alarm.description));
                            });
                        }
                    });
                });
                ui.add_space(8.0);
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

        // Shortcuts & About Modal
        if self.show_help_dialog {
            let mut open = true;
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                open = false;
            }
            egui::Window::new(pick_lang(
                lang,
                "Pintasan & Informasi",
                "Shortcuts & About",
            ))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.strong(format!("{APP_NAME} Desktop Client v{}", env!("CARGO_PKG_VERSION")));
                ui.label(
                    egui::RichText::new(pick_lang(
                        lang,
                        "Seluruh polling dan konfigurasi tersimpan aman di server Linux.",
                        "All probing, alerting, and layout state runs on the Linux server.",
                    ))
                    .color(muted_color(theme)),
                );
                ui.separator();
                egui::Grid::new("shortcuts_help_grid")
                    .num_columns(2)
                    .spacing([16.0, 6.0])
                    .show(ui, |ui| {
                        ui.monospace("Ctrl + K");
                        ui.label(pick_lang(lang, "Pencarian global", "Global search"));
                        ui.end_row();
                        ui.monospace("Ctrl + Z / Ctrl + Y");
                        ui.label(pick_lang(lang, "Undo / Redo aksi peta", "Undo / Redo map actions"));
                        ui.end_row();
                        ui.monospace("Scroll / Middle Drag");
                        ui.label(pick_lang(lang, "Zoom / Geser kanvas peta", "Zoom / Pan map canvas"));
                        ui.end_row();
                        ui.monospace("Delete / Esc");
                        ui.label(pick_lang(lang, "Hapus item / Batal alat", "Delete selected / Cancel tool"));
                        ui.end_row();
                    });
            });
            if !open {
                self.show_help_dialog = false;
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
