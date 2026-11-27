use crate::panels::inventory::fmt_local_ts;
use crate::platform::{set_autostart_registry, set_prevent_sleep};
use crate::ui_kit::{
    card_frame, danger_button, empty_state_card, hairline_separator, muted_color, pick_lang,
    primary_button, section_heading, segmented_track, segmented_value, status_pill, table_card,
    toolbar_frame,
};
use eframe::egui;
use nextdude_client_core::config::ClientLocalConfig;
use nextdude_client_core::connection::ClientCommand;
use nextdude_client_core::i18n::{tr, Language, TextKey};
use nextdude_client_core::state::ClientStateCache;
use nextdude_client_core::theme::{state_color, ThemeMode};
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
    theme: ThemeMode,
    admin_st: &AdminPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.horizontal(|ui| {
        section_heading(ui, theme, tr(lang, TextKey::PanelHistoryActions), None);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(tr(lang, TextKey::Refresh)).clicked() {
                let now = chrono::Utc::now().timestamp();
                let _ = cmd_tx.try_send(ClientCommand::QueryHistoryActions(
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
        });
    });
    ui.add_space(6.0);

    if admin_st.history_actions.is_empty() {
        empty_state_card(
            ui,
            theme,
            pick_lang(lang, "Belum ada log audit", "No audit records loaded"),
            pick_lang(
                lang,
                "Klik Segarkan untuk mengambil riwayat aksi admin terbaru.",
                "Click Refresh to load recent administrator actions.",
            ),
        );
        return;
    }

    table_card(ui, theme, "history_scroll", |ui| {
        egui::Grid::new("history_grid")
            .spacing([16.0, 10.0])
            .num_columns(5)
            .show(ui, |ui| {
                ui.strong(tr(lang, TextKey::Timestamp));
                ui.strong(tr(lang, TextKey::Username));
                ui.strong(tr(lang, TextKey::Action));
                ui.strong("Target");
                ui.strong("Details");
                ui.end_row();
                for h in &admin_st.history_actions {
                    ui.monospace(fmt_local_ts(h.timestamp_utc));
                    ui.strong(&h.username);
                    ui.monospace(&h.action);
                    ui.monospace(format!("{}:{}", h.target_type, h.target_id));
                    ui.label(&h.details);
                    ui.end_row();
                }
            });
    });
}

pub fn render_active_admins_panel(
    ui: &mut egui::Ui,
    lang: Language,
    theme: ThemeMode,
    admin_st: &AdminPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.horizontal(|ui| {
        section_heading(ui, theme, tr(lang, TextKey::PanelActiveAdmins), None);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(tr(lang, TextKey::Refresh)).clicked() {
                let _ = cmd_tx.try_send(ClientCommand::ListActiveAdmins);
            }
        });
    });
    ui.add_space(6.0);

    if admin_st.active_sessions.is_empty() {
        empty_state_card(
            ui,
            theme,
            pick_lang(lang, "Tidak ada sesi aktif", "No active admin sessions"),
            pick_lang(
                lang,
                "Klik Segarkan untuk memuat daftar client gRPC yang terhubung.",
                "Click Refresh to query currently connected admin sessions.",
            ),
        );
        return;
    }

    table_card(ui, theme, "active_admins_scroll", |ui| {
        egui::Grid::new("active_admins_grid")
            .spacing([16.0, 10.0])
            .num_columns(5)
            .show(ui, |ui| {
                ui.strong(tr(lang, TextKey::Username));
                ui.strong("Remote Addr");
                ui.strong("Client");
                ui.strong("Connected");
                ui.strong(tr(lang, TextKey::Action));
                ui.end_row();
                for s in &admin_st.active_sessions {
                    ui.strong(&s.username);
                    ui.monospace(&s.remote_addr);
                    ui.label(&s.client_info);
                    ui.monospace(fmt_local_ts(s.connected_utc));
                    if danger_button(ui, theme, tr(lang, TextKey::KickSession)).clicked() {
                        let _ = cmd_tx
                            .try_send(ClientCommand::KickAdminSession(s.session_id.clone()));
                    }
                    ui.end_row();
                }
            });
    });
}

pub fn render_admins_groups_panel(
    ui: &mut egui::Ui,
    lang: Language,
    theme: ThemeMode,
    admin_st: &mut AdminPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.horizontal(|ui| {
        section_heading(ui, theme, tr(lang, TextKey::PanelAdminsGroups), None);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(tr(lang, TextKey::Refresh)).clicked() {
                let _ = cmd_tx.try_send(ClientCommand::ListAdmins);
            }
        });
    });
    ui.add_space(6.0);

    toolbar_frame(theme).show(ui, |ui| {
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
            let valid = !admin_st.new_admin_username.trim().is_empty()
                && admin_st.new_admin_password.len() >= 8;
            if primary_button(ui, theme, valid, "+ Admin").clicked() {
                let _ = cmd_tx.try_send(ClientCommand::SaveAdmin(v1::AdminUser {
                    id: 0,
                    version: 1,
                    username: admin_st.new_admin_username.trim().to_string(),
                    group_id: admin_st.new_admin_group_id,
                    group_name: String::new(),
                    enabled: true,
                    password: Some(admin_st.new_admin_password.clone()),
                    locked_until_utc: 0,
                }));
                admin_st.new_admin_password.clear();
            }
        });
    });
    ui.add_space(6.0);

    table_card(ui, theme, "admins_list_scroll", |ui| {
        egui::Grid::new("admins_list_grid")
            .spacing([16.0, 10.0])
            .num_columns(4)
            .show(ui, |ui| {
                ui.strong(tr(lang, TextKey::Username));
                ui.strong("Group");
                ui.strong("Enabled");
                ui.strong(tr(lang, TextKey::Action));
                ui.end_row();
                for a in &admin_st.admins {
                    ui.strong(&a.username);
                    ui.label(&a.group_name);
                    if a.enabled {
                        status_pill(
                            ui,
                            tr(lang, TextKey::Yes),
                            state_color(theme, v1::ProbeState::Up),
                        );
                    } else {
                        status_pill(
                            ui,
                            tr(lang, TextKey::No),
                            state_color(theme, v1::ProbeState::Down),
                        );
                    }
                    if danger_button(ui, theme, tr(lang, TextKey::Delete)).clicked() {
                        let _ = cmd_tx.try_send(ClientCommand::DeleteAdmin(v1::IdRequest {
                            id: a.id,
                            expected_version: a.version,
                        }));
                    }
                    ui.end_row();
                }
            });
    });

    ui.add_space(8.0);
    card_frame(theme).show(ui, |ui| {
        ui.strong("Admin Groups & Permissions");
        ui.add_space(6.0);
        let group_count = admin_st.groups.len();
        for (idx, g) in admin_st.groups.iter_mut().enumerate() {
            let group_name = g.name.clone();
            ui.collapsing(&group_name, |ui| {
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
                ui.add_space(4.0);
                if primary_button(ui, theme, true, tr(lang, TextKey::Save)).clicked() {
                    let _ = cmd_tx.try_send(ClientCommand::SaveAdminGroup(g.clone()));
                }
            });
            if idx + 1 < group_count {
                hairline_separator(ui, theme);
            }
        }
    });
}

pub fn render_notifications_panel(
    ui: &mut egui::Ui,
    lang: Language,
    theme: ThemeMode,
    state: &ClientStateCache,
    admin_st: &mut AdminPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    section_heading(ui, theme, tr(lang, TextKey::PanelNotifications), None);
    ui.add_space(6.0);

    toolbar_frame(theme).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label("Telegram Name");
            ui.text_edit_singleline(&mut admin_st.new_tg_name);
            ui.label("Chat ID");
            ui.text_edit_singleline(&mut admin_st.new_tg_chat_id);
            ui.label("Bot Token (write-only)");
            ui.add(egui::TextEdit::singleline(&mut admin_st.new_tg_token).password(true));
            let valid = !admin_st.new_tg_chat_id.trim().is_empty()
                && !admin_st.new_tg_token.trim().is_empty();
            if primary_button(ui, theme, valid, "+ Telegram Channel").clicked() {
                let _ = cmd_tx.try_send(ClientCommand::SaveNotificationChannel(
                    v1::NotificationChannel {
                        id: 0,
                        version: 1,
                        name: if admin_st.new_tg_name.trim().is_empty() {
                            "Telegram Ops".to_string()
                        } else {
                            admin_st.new_tg_name.trim().to_string()
                        },
                        kind: "telegram".to_string(),
                        enabled: true,
                        target: admin_st.new_tg_chat_id.trim().to_string(),
                        secret_token: Some(admin_st.new_tg_token.clone()),
                        has_secret_token: false,
                        rate_limit_per_min: 20,
                    },
                ));
                admin_st.new_tg_token.clear();
            }
        });
    });
    ui.add_space(6.0);

    table_card(ui, theme, "notif_channels_scroll", |ui| {
        egui::Grid::new("notif_channels_grid")
            .spacing([16.0, 10.0])
            .num_columns(5)
            .show(ui, |ui| {
                ui.strong(tr(lang, TextKey::Name));
                ui.strong(tr(lang, TextKey::Type));
                ui.strong("Target");
                ui.strong("Secret");
                ui.strong(tr(lang, TextKey::Action));
                ui.end_row();
                for ch in state.notification_channels.values() {
                    ui.strong(&ch.name);
                    ui.monospace(&ch.kind);
                    ui.label(&ch.target);
                    ui.label(
                        egui::RichText::new(if ch.has_secret_token {
                            tr(lang, TextKey::SecretAlreadySet)
                        } else {
                            tr(lang, TextKey::SecretNotSet)
                        })
                        .small()
                        .color(muted_color(theme)),
                    );
                    ui.horizontal(|ui| {
                        if ui.button(tr(lang, TextKey::TestSend)).clicked() {
                            let _ = cmd_tx.try_send(ClientCommand::TestNotificationChannel(ch.id));
                        }
                        if danger_button(ui, theme, tr(lang, TextKey::Delete)).clicked() {
                            let _ = cmd_tx.try_send(ClientCommand::DeleteNotificationChannel(
                                v1::IdRequest {
                                    id: ch.id,
                                    expected_version: ch.version,
                                },
                            ));
                        }
                    });
                    ui.end_row();
                }
            });
    });
}

pub fn render_server_settings_panel(
    ui: &mut egui::Ui,
    lang: Language,
    theme: ThemeMode,
    state: &ClientStateCache,
    admin_st: &mut AdminPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    section_heading(ui, theme, tr(lang, TextKey::PanelServerSettings), None);
    ui.add_space(6.0);

    if admin_st.settings_draft.is_none() {
        admin_st.settings_draft = state.server_settings.clone();
    }
    let Some(ref mut s) = admin_st.settings_draft else {
        empty_state_card(
            ui,
            theme,
            pick_lang(lang, "Belum terhubung", "Server settings unavailable"),
            pick_lang(
                lang,
                "Sambungkan ke server untuk membaca dan mengubah konfigurasi engine.",
                "Connect to a NextDude server to view and update runtime settings.",
            ),
        );
        return;
    };

    ui.vertical(|ui| {
        ui.set_max_width(720.0);
        card_frame(theme).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong("ICMP Socket:");
                ui.monospace(&s.icmp_status_message);
            });
            hairline_separator(ui, theme);

            egui::Grid::new("server_settings_grid")
                .num_columns(2)
                .spacing([18.0, 10.0])
                .show(ui, |ui| {
                    ui.label("Global Poll Interval (s)");
                    ui.add(egui::DragValue::new(&mut s.global_poll_interval_sec).range(5..=3600));
                    ui.end_row();

                    ui.label("Max Concurrent Probes");
                    ui.add(egui::DragValue::new(&mut s.max_concurrent_probes).range(1..=2048));
                    ui.end_row();

                    ui.label("Max Concurrent Per Host");
                    ui.add(egui::DragValue::new(&mut s.max_concurrent_per_host).range(1..=64));
                    ui.end_row();

                    ui.label("Flapping Changes / Window (min)");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut s.flapping_changes).range(2..=50));
                        ui.add(egui::DragValue::new(&mut s.flapping_window_min).range(1..=120));
                    });
                    ui.end_row();

                    ui.label("External Syslog Forward (host:port)");
                    ui.text_edit_singleline(&mut s.syslog_forward_addr);
                    ui.end_row();

                    ui.label("Retention Raw(h) / Rollup(d) / Events(d) / Syslog(d) / Audit(d)");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut s.retention_raw_hours).range(1..=720));
                        ui.add(egui::DragValue::new(&mut s.retention_rollup_days).range(1..=3650));
                        ui.add(egui::DragValue::new(&mut s.retention_events_days).range(1..=3650));
                        ui.add(egui::DragValue::new(&mut s.retention_syslog_days).range(1..=3650));
                        ui.add(egui::DragValue::new(&mut s.retention_audit_days).range(1..=3650));
                    });
                    ui.end_row();
                });

            ui.add_space(6.0);
            ui.checkbox(
                &mut s.allow_image_url_import,
                "Allow Image Import from HTTPS URL",
            );
            ui.add_space(10.0);

            if primary_button(ui, theme, true, tr(lang, TextKey::Save)).clicked() {
                let _ = cmd_tx.try_send(ClientCommand::UpdateServerSettings(s.clone()));
            }
        });
    });
}

pub fn render_client_settings_panel(
    ui: &mut egui::Ui,
    local_cfg: &mut ClientLocalConfig,
    save_cfg_cb: &mut dyn FnMut(&ClientLocalConfig),
) {
    let lang = local_cfg.language;
    let theme = local_cfg.theme;
    section_heading(ui, theme, tr(lang, TextKey::PanelClientSettings), None);
    ui.add_space(6.0);

    let mut changed = false;
    ui.vertical(|ui| {
        ui.set_max_width(720.0);
        card_frame(theme).show(ui, |ui| {
            egui::Grid::new("client_settings_grid")
                .num_columns(2)
                .spacing([18.0, 10.0])
                .show(ui, |ui| {
                    ui.label(tr(lang, TextKey::LanguageLabel));
                    segmented_track(ui, theme, |ui| {
                        changed |= segmented_value(
                            ui,
                            theme,
                            &mut local_cfg.language,
                            Language::Id,
                            Language::Id.label(),
                        );
                        changed |= segmented_value(
                            ui,
                            theme,
                            &mut local_cfg.language,
                            Language::En,
                            Language::En.label(),
                        );
                    });
                    ui.end_row();

                    ui.label("Theme");
                    segmented_track(ui, theme, |ui| {
                        changed |= segmented_value(
                            ui,
                            theme,
                            &mut local_cfg.theme,
                            ThemeMode::Dark,
                            tr(lang, TextKey::ThemeDark),
                        );
                        changed |= segmented_value(
                            ui,
                            theme,
                            &mut local_cfg.theme,
                            ThemeMode::Light,
                            tr(lang, TextKey::ThemeLight),
                        );
                    });
                    ui.end_row();
                });

            hairline_separator(ui, theme);
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
                .checkbox(
                    &mut local_cfg.autostart_on_login,
                    tr(lang, TextKey::AutostartLogin),
                )
                .changed()
            {
                let _ = set_autostart_registry(local_cfg.autostart_on_login);
                changed = true;
            }
            changed |= ui
                .checkbox(&mut local_cfg.global_mute, tr(lang, TextKey::MuteAlarm))
                .changed();

            hairline_separator(ui, theme);
            ui.strong(tr(lang, TextKey::ExternalToolsTitle));
            ui.add_space(6.0);
            egui::Grid::new("external_tools_grid")
                .num_columns(2)
                .spacing([16.0, 8.0])
                .show(ui, |ui| {
                    for key in ["winbox", "ssh", "telnet"] {
                        let val = local_cfg.external_tools.entry(key.to_string()).or_default();
                        ui.monospace(key);
                        if ui.text_edit_singleline(val).changed() {
                            changed = true;
                        }
                        ui.end_row();
                    }
                });
        });
    });

    if changed {
        save_cfg_cb(local_cfg);
    }
}
