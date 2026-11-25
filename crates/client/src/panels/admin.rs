use crate::panels::inventory::fmt_local_ts;
use crate::platform::{set_autostart_registry, set_prevent_sleep};
use eframe::egui;
use nextdude_client_core::config::ClientLocalConfig;
use nextdude_client_core::connection::ClientCommand;
use nextdude_client_core::i18n::{tr, Language, TextKey};
use nextdude_client_core::state::ClientStateCache;
use nextdude_client_core::theme::ThemeMode;
use nextdude_common::proto::v1;
use nextdude_common::types::Permission;
use tokio::sync::mpsc;

#[derive(Default)]
pub struct AdminPanelState {
    pub admins: Vec<v1::AdminUser>,
    pub groups: Vec<v1::AdminGroup>,
    pub active_sessions: Vec<v1::ActiveAdminSession>,
    pub history_actions: Vec<v1::HistoryAction>,
    pub new_admin_username: String,
    pub new_admin_password: String,
    pub new_admin_group_id: i64,
    pub new_tg_name: String,
    pub new_tg_chat_id: String,
    pub new_tg_token: String,
    pub settings_draft: Option<v1::ServerSettings>,
}

pub fn render_history_actions_panel(
    ui: &mut egui::Ui,
    lang: Language,
    admin_st: &AdminPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.horizontal(|ui| {
        ui.heading(tr(lang, TextKey::PanelHistoryActions));
        if ui.button(tr(lang, TextKey::Refresh)).clicked() {
            let now = chrono::Utc::now().timestamp();
            let _ = cmd_tx.try_send(ClientCommand::QueryHistoryActions(v1::PageQueryRequest {
                start_utc: now - 30 * 86400,
                end_utc: now + 60,
                device_id: None,
                text_filter: String::new(),
                min_severity: None,
                limit: 200,
                offset: 0,
            }));
        }
    });
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("history_grid").striped(true).num_columns(5).show(ui, |ui| {
            ui.strong(tr(lang, TextKey::Timestamp));
            ui.strong(tr(lang, TextKey::Username));
            ui.strong(tr(lang, TextKey::Action));
            ui.strong("Target");
            ui.strong("Details");
            ui.end_row();
            for h in &admin_st.history_actions {
                ui.label(fmt_local_ts(h.timestamp_utc));
                ui.label(&h.username);
                ui.label(&h.action);
                ui.label(format!("{}:{}", h.target_type, h.target_id));
                ui.label(&h.details);
                ui.end_row();
            }
        });
    });
}

pub fn render_active_admins_panel(
    ui: &mut egui::Ui,
    lang: Language,
    admin_st: &AdminPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.horizontal(|ui| {
        ui.heading(tr(lang, TextKey::PanelActiveAdmins));
        if ui.button(tr(lang, TextKey::Refresh)).clicked() {
            let _ = cmd_tx.try_send(ClientCommand::ListActiveAdmins);
        }
    });
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("active_admins_grid").striped(true).num_columns(5).show(ui, |ui| {
            ui.strong(tr(lang, TextKey::Username));
            ui.strong("Remote Addr");
            ui.strong("Client");
            ui.strong("Connected");
            ui.strong(tr(lang, TextKey::Action));
            ui.end_row();
            for s in &admin_st.active_sessions {
                ui.label(&s.username);
                ui.label(&s.remote_addr);
                ui.label(&s.client_info);
                ui.label(fmt_local_ts(s.connected_utc));
                if ui.button(tr(lang, TextKey::KickSession)).clicked() {
                    let _ = cmd_tx.try_send(ClientCommand::KickAdminSession(s.session_id.clone()));
                }
                ui.end_row();
            }
        });
    });
}

pub fn render_admins_groups_panel(
    ui: &mut egui::Ui,
    lang: Language,
    admin_st: &mut AdminPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.horizontal(|ui| {
        ui.heading(tr(lang, TextKey::PanelAdminsGroups));
        if ui.button(tr(lang, TextKey::Refresh)).clicked() {
            let _ = cmd_tx.try_send(ClientCommand::ListAdmins);
        }
    });

    ui.horizontal_wrapped(|ui| {
        ui.label(tr(lang, TextKey::Username));
        ui.text_edit_singleline(&mut admin_st.new_admin_username);
        ui.label(tr(lang, TextKey::Password));
        ui.add(egui::TextEdit::singleline(&mut admin_st.new_admin_password).password(true));
        egui::ComboBox::from_label("Group")
            .selected_text(
                admin_st
                    .groups
                    .iter()
                    .find(|g| g.id == admin_st.new_admin_group_id)
                    .map_or("Select", |g| g.name.as_str()),
            )
            .show_ui(ui, |ui| {
                for g in &admin_st.groups {
                    ui.selectable_value(&mut admin_st.new_admin_group_id, g.id, &g.name);
                }
            });
        if ui.button("+ Admin").clicked() {
            let _ = cmd_tx.try_send(ClientCommand::SaveAdmin(v1::AdminUser {
                id: 0,
                version: 1,
                username: admin_st.new_admin_username.clone(),
                group_id: admin_st.new_admin_group_id,
                group_name: String::new(),
                enabled: true,
                password: Some(admin_st.new_admin_password.clone()),
                locked_until_utc: 0,
            }));
            admin_st.new_admin_password.clear();
        }
    });

    ui.separator();
    egui::Grid::new("admins_list_grid").striped(true).num_columns(4).show(ui, |ui| {
        ui.strong(tr(lang, TextKey::Username));
        ui.strong("Group");
        ui.strong("Enabled");
        ui.strong(tr(lang, TextKey::Action));
        ui.end_row();
        for a in &admin_st.admins {
            ui.label(&a.username);
            ui.label(&a.group_name);
            ui.label(if a.enabled { tr(lang, TextKey::Yes) } else { tr(lang, TextKey::No) });
            if ui.button(tr(lang, TextKey::Delete)).clicked() {
                let _ = cmd_tx.try_send(ClientCommand::DeleteAdmin(v1::IdRequest {
                    id: a.id,
                    expected_version: a.version,
                }));
            }
            ui.end_row();
        }
    });

    ui.separator();
    ui.strong("Admin Groups & Permissions");
    for g in &mut admin_st.groups {
        ui.collapsing(&g.name, |ui| {
            for perm in Permission::ALL {
                let key = perm.as_str();
                let mut has = g.permissions.iter().any(|p| p == key);
                if ui.checkbox(&mut has, key).changed() {
                    if has {
                        g.permissions.push(key.to_string());
                    } else {
                        g.permissions.retain(|p| p != key);
                    }
                }
            }
            if ui.button(tr(lang, TextKey::Save)).clicked() {
                let _ = cmd_tx.try_send(ClientCommand::SaveAdminGroup(g.clone()));
            }
        });
    }
}

pub fn render_notifications_panel(
    ui: &mut egui::Ui,
    lang: Language,
    state: &ClientStateCache,
    admin_st: &mut AdminPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.heading(tr(lang, TextKey::PanelNotifications));
    ui.horizontal_wrapped(|ui| {
        ui.label("Telegram Name");
        ui.text_edit_singleline(&mut admin_st.new_tg_name);
        ui.label("Chat ID");
        ui.text_edit_singleline(&mut admin_st.new_tg_chat_id);
        ui.label("Bot Token (write-only)");
        ui.add(egui::TextEdit::singleline(&mut admin_st.new_tg_token).password(true));
        if ui.button("+ Telegram Channel").clicked() {
            let _ = cmd_tx.try_send(ClientCommand::SaveNotificationChannel(
                v1::NotificationChannel {
                    id: 0,
                    version: 1,
                    name: if admin_st.new_tg_name.is_empty() {
                        "Telegram Ops".to_string()
                    } else {
                        admin_st.new_tg_name.clone()
                    },
                    kind: "telegram".to_string(),
                    enabled: true,
                    target: admin_st.new_tg_chat_id.clone(),
                    secret_token: Some(admin_st.new_tg_token.clone()),
                    has_secret_token: false,
                    rate_limit_per_min: 20,
                },
            ));
            admin_st.new_tg_token.clear();
        }
    });
    ui.separator();

    egui::Grid::new("notif_channels_grid").striped(true).num_columns(5).show(ui, |ui| {
        ui.strong(tr(lang, TextKey::Name));
        ui.strong(tr(lang, TextKey::Type));
        ui.strong("Target");
        ui.strong("Secret");
        ui.strong(tr(lang, TextKey::Action));
        ui.end_row();
        for ch in state.notification_channels.values() {
            ui.label(&ch.name);
            ui.label(&ch.kind);
            ui.label(&ch.target);
            ui.label(if ch.has_secret_token {
                tr(lang, TextKey::SecretAlreadySet)
            } else {
                tr(lang, TextKey::SecretNotSet)
            });
            ui.horizontal(|ui| {
                if ui.button(tr(lang, TextKey::TestSend)).clicked() {
                    let _ = cmd_tx.try_send(ClientCommand::TestNotificationChannel(ch.id));
                }
                if ui.button(tr(lang, TextKey::Delete)).clicked() {
                    let _ = cmd_tx.try_send(ClientCommand::DeleteNotificationChannel(v1::IdRequest {
                        id: ch.id,
                        expected_version: ch.version,
                    }));
                }
            });
            ui.end_row();
        }
    });
}

pub fn render_server_settings_panel(
    ui: &mut egui::Ui,
    lang: Language,
    state: &ClientStateCache,
    admin_st: &mut AdminPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.heading(tr(lang, TextKey::PanelServerSettings));
    if admin_st.settings_draft.is_none() {
        admin_st.settings_draft = state.server_settings.clone();
    }
    let Some(ref mut s) = admin_st.settings_draft else {
        ui.label("Connect to a server to view server settings.");
        return;
    };
    ui.label(format!("ICMP Status: {}", s.icmp_status_message));
    ui.separator();

    ui.horizontal(|ui| {
        ui.label("Global Poll Interval (s)");
        ui.add(egui::DragValue::new(&mut s.global_poll_interval_sec).range(5..=3600));
    });
    ui.horizontal(|ui| {
        ui.label("Max Concurrent Probes");
        ui.add(egui::DragValue::new(&mut s.max_concurrent_probes).range(1..=2048));
    });
    ui.horizontal(|ui| {
        ui.label("Max Concurrent Per Host");
        ui.add(egui::DragValue::new(&mut s.max_concurrent_per_host).range(1..=64));
    });
    ui.horizontal(|ui| {
        ui.label("Flapping Changes / Window (min)");
        ui.add(egui::DragValue::new(&mut s.flapping_changes).range(2..=50));
        ui.add(egui::DragValue::new(&mut s.flapping_window_min).range(1..=120));
    });
    ui.checkbox(&mut s.allow_image_url_import, "Allow Image Import from HTTPS URL");
    ui.horizontal(|ui| {
        ui.label("External Syslog Forward (host:port)");
        ui.text_edit_singleline(&mut s.syslog_forward_addr);
    });
    ui.horizontal(|ui| {
        ui.label("Retention Raw(h) / Rollup(d) / Events(d) / Syslog(d) / Audit(d)");
        ui.add(egui::DragValue::new(&mut s.retention_raw_hours).range(1..=720));
        ui.add(egui::DragValue::new(&mut s.retention_rollup_days).range(1..=3650));
        ui.add(egui::DragValue::new(&mut s.retention_events_days).range(1..=3650));
        ui.add(egui::DragValue::new(&mut s.retention_syslog_days).range(1..=3650));
        ui.add(egui::DragValue::new(&mut s.retention_audit_days).range(1..=3650));
    });

    if ui.button(tr(lang, TextKey::Save)).clicked() {
        let _ = cmd_tx.try_send(ClientCommand::UpdateServerSettings(s.clone()));
    }
}

pub fn render_client_settings_panel(
    ui: &mut egui::Ui,
    local_cfg: &mut ClientLocalConfig,
    save_cfg_cb: &mut dyn FnMut(&ClientLocalConfig),
) {
    let lang = local_cfg.language;
    ui.heading(tr(lang, TextKey::PanelClientSettings));

    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(tr(lang, TextKey::LanguageLabel));
        changed |= ui
            .selectable_value(&mut local_cfg.language, Language::Id, Language::Id.label())
            .changed();
        changed |= ui
            .selectable_value(&mut local_cfg.language, Language::En, Language::En.label())
            .changed();
    });

    ui.horizontal(|ui| {
        ui.label("Theme");
        changed |= ui
            .selectable_value(&mut local_cfg.theme, ThemeMode::Dark, tr(lang, TextKey::ThemeDark))
            .changed();
        changed |= ui
            .selectable_value(&mut local_cfg.theme, ThemeMode::Light, tr(lang, TextKey::ThemeLight))
            .changed();
    });

    changed |= ui
        .checkbox(&mut local_cfg.close_to_tray, tr(lang, TextKey::CloseToTray))
        .changed();
    if ui
        .checkbox(&mut local_cfg.prevent_sleep, tr(lang, TextKey::PreventSleep))
        .changed()
    {
        set_prevent_sleep(local_cfg.prevent_sleep);
        changed = true;
    }
    if ui
        .checkbox(&mut local_cfg.autostart_on_login, tr(lang, TextKey::AutostartLogin))
        .changed()
    {
        let _ = set_autostart_registry(local_cfg.autostart_on_login);
        changed = true;
    }
    changed |= ui
        .checkbox(&mut local_cfg.global_mute, tr(lang, TextKey::MuteAlarm))
        .changed();

    ui.separator();
    ui.strong(tr(lang, TextKey::ExternalToolsTitle));
    for key in ["winbox", "ssh", "telnet"] {
        let val = local_cfg.external_tools.entry(key.to_string()).or_default();
        ui.horizontal(|ui| {
            ui.label(key);
            if ui.text_edit_singleline(val).changed() {
                changed = true;
            }
        });
    }

    if changed {
        save_cfg_cb(local_cfg);
    }
}
