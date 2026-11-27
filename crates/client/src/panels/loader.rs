use crate::platform::{dpapi_protect_b64, dpapi_unprotect_b64};
use crate::ui_kit::{
    accent_rgb, card_frame, centered_modal, danger_button, empty_state_card, hairline_separator,
    modal_footer_buttons, muted_color, notice_frame, pick_lang, primary_button, section_heading,
    status_pill, toolbar_frame, warning_rgb,
};
use eframe::egui;
use nextdude_client_core::config::{ClientLocalConfig, SavedServerEntry};
use nextdude_client_core::connection::{ClientCommand, ConnectParams, ConnectionPhase};
use nextdude_client_core::i18n::{tr, TextKey};
use nextdude_client_core::theme::{state_color, RgbColor};
use nextdude_client_core::tofu::TofuDecision;
use nextdude_common::proto::v1;
use tokio::sync::mpsc;

#[derive(Default)]
pub struct LoaderPanelState {
    pub edit_entry: SavedServerEntry,
    pub edit_password_plain: String,
    pub show_password: bool,
    pub is_editing: bool,
    pub status_banner: String,
    pub pending_tofu: Option<TofuDecision>,
    pub pending_password_entry: Option<SavedServerEntry>,
    pub prompt_password: String,
}

pub fn render_loader_panel(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    phase: &ConnectionPhase,
    local_cfg: &mut ClientLocalConfig,
    panel: &mut LoaderPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
    save_cfg_cb: &mut dyn FnMut(&ClientLocalConfig),
) {
    let lang = local_cfg.language;
    let theme = local_cfg.theme;
    ui.horizontal(|ui| {
        section_heading(
            ui,
            theme,
            tr(lang, TextKey::AppTitle),
            Some(pick_lang(
                lang,
                "Pilih atau tambahkan server NextDude Linux (gRPC TLS 1.3 + TOFU)",
                "Select or add a NextDude Linux server (gRPC TLS 1.3 + TOFU)",
            )),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if primary_button(
                ui,
                theme,
                true,
                &format!("+ {}", tr(lang, TextKey::AddServer)),
            )
            .clicked()
            {
                panel.is_editing = true;
                panel.show_password = false;
                panel.edit_entry = SavedServerEntry {
                    id: format!("srv-{}", chrono::Utc::now().timestamp_millis()),
                    port: nextdude_common::DEFAULT_GRPC_PORT,
                    ..Default::default()
                };
                panel.edit_password_plain.clear();
            }
        });
    });
    ui.add_space(10.0);

    if !panel.status_banner.is_empty() {
        let is_err = panel.status_banner.starts_with("Error:");
        let col = if is_err {
            state_color(theme, v1::ProbeState::Down)
        } else {
            accent_rgb(theme)
        };
        notice_frame(theme, col).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(&panel.status_banner);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("x").clicked() {
                        panel.status_banner.clear();
                    }
                });
            });
        });
        ui.add_space(10.0);
    }

    toolbar_frame(theme).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal(|ui| {
            let (status_text, status_rgb) = match phase {
                ConnectionPhase::Connected => (
                    tr(lang, TextKey::StatusConnected).to_string(),
                    state_color(theme, v1::ProbeState::Up),
                ),
                ConnectionPhase::Connecting => (
                    tr(lang, TextKey::StatusConnecting).to_string(),
                    warning_rgb(theme),
                ),
                ConnectionPhase::Disconnected => (
                    tr(lang, TextKey::StatusDisconnected).to_string(),
                    RgbColor::new(152, 152, 157),
                ),
                ConnectionPhase::Reconnecting(n) => (
                    format!("{} (#{n})", tr(lang, TextKey::StatusReconnecting)),
                    warning_rgb(theme),
                ),
            };
            status_pill(ui, &status_text, status_rgb);
            ui.add_space(6.0);
            if ui
                .checkbox(&mut local_cfg.auto_connect_last, tr(lang, TextKey::AutoConnectLast))
                .changed()
            {
                save_cfg_cb(local_cfg);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if *phase != ConnectionPhase::Disconnected
                    && ui.button(tr(lang, TextKey::Disconnect)).clicked()
                {
                    let _ = cmd_tx.try_send(ClientCommand::Disconnect);
                }
            });
        });
    });

    ui.add_space(10.0);
    if local_cfg.servers.is_empty() {
        empty_state_card(
            ui,
            theme,
            pick_lang(lang, "Belum ada profil server tersimpan", "No saved server profiles yet"),
            pick_lang(
                lang,
                "Klik '+ Tambah Server' di kanan atas untuk menambahkan alamat server NextDude Linux Anda.",
                "Click '+ Add Server' above to add your NextDude Linux server IP or hostname.",
            ),
        );
    } else {
        let mut to_delete: Option<String> = None;
        let mut to_connect: Option<SavedServerEntry> = None;
        let mut to_edit: Option<SavedServerEntry> = None;

        card_frame(theme).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            egui::ScrollArea::vertical()
                .id_salt("loader_servers_list")
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    let total = local_cfg.servers.len();
                    for (idx, srv) in local_cfg.servers.iter().enumerate() {
                        if idx > 0 {
                            ui.add_space(6.0);
                            hairline_separator(ui, theme);
                            ui.add_space(6.0);
                        }
                        let is_last =
                            local_cfg.last_server_id.as_deref() == Some(srv.id.as_str());
                        let is_active = is_last && *phase == ConnectionPhase::Connected;

                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(&srv.name).size(15.0).strong());
                                    if is_active {
                                        status_pill(
                                            ui,
                                            pick_lang(lang, "Aktif", "Active"),
                                            state_color(theme, v1::ProbeState::Up),
                                        );
                                    }
                                });
                                let subtitle = if srv.comment.trim().is_empty() {
                                    format!("{}:{} | {}", srv.address, srv.port, srv.username)
                                } else {
                                    format!(
                                        "{}:{} | {} | {}",
                                        srv.address, srv.port, srv.username, srv.comment
                                    )
                                };
                                ui.label(
                                    egui::RichText::new(subtitle)
                                        .small()
                                        .color(muted_color(theme)),
                                );
                            });

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if danger_button(ui, theme, tr(lang, TextKey::DeleteServer))
                                        .clicked()
                                    {
                                        to_delete = Some(srv.id.clone());
                                    }
                                    if ui.button(tr(lang, TextKey::EditServer)).clicked() {
                                        to_edit = Some(srv.clone());
                                    }
                                    if primary_button(
                                        ui,
                                        theme,
                                        true,
                                        tr(lang, TextKey::Connect),
                                    )
                                    .clicked()
                                    {
                                        to_connect = Some(srv.clone());
                                    }
                                },
                            );
                        });
                    }
                    if total == 0 {
                        ui.label("");
                    }
                });
        });

        if let Some(id) = to_delete {
            local_cfg.servers.retain(|s| s.id != id);
            if local_cfg.last_server_id.as_deref() == Some(&id) {
                local_cfg.last_server_id = None;
            }
            save_cfg_cb(local_cfg);
        }
        if let Some(srv) = to_edit {
            panel.is_editing = true;
            panel.show_password = false;
            panel.edit_password_plain =
                if srv.remember_password && !srv.encrypted_password_b64.is_empty() {
                    dpapi_unprotect_b64(&srv.encrypted_password_b64).unwrap_or_default()
                } else {
                    String::new()
                };
            panel.edit_entry = srv;
        }
        if let Some(srv) = to_connect {
            if srv.remember_password && !srv.encrypted_password_b64.is_empty() {
                connect_to_entry(srv, local_cfg, panel, cmd_tx, save_cfg_cb);
            } else {
                panel.prompt_password.clear();
                panel.pending_password_entry = Some(srv);
            }
        }
    }

    // Password prompt when server profile doesn't persist DPAPI credentials
    if let Some(srv) = panel.pending_password_entry.clone() {
        let mut submit = false;
        let mut cancel = false;
        centered_modal(
            pick_lang(lang, "Masuk ke Server", "Sign in to Server"),
            380.0,
        )
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(format!("{}  ({}:{})", srv.name, srv.address, srv.port))
                    .strong(),
            );
            ui.label(
                egui::RichText::new(format!("User: {}", srv.username))
                    .small()
                    .color(muted_color(theme)),
            );
            ui.add_space(8.0);
            let pw_resp = ui.add(
                egui::TextEdit::singleline(&mut panel.prompt_password)
                    .desired_width(f32::INFINITY)
                    .password(true)
                    .hint_text(tr(lang, TextKey::Password)),
            );
            pw_resp.request_focus();
            if pw_resp.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit = true;
            }
            let (s, c) = modal_footer_buttons(
                ui,
                theme,
                !panel.prompt_password.is_empty(),
                tr(lang, TextKey::Connect),
                tr(lang, TextKey::Cancel),
            );
            submit |= s;
            cancel |= c;
        });

        if submit && !panel.prompt_password.is_empty() {
            panel.edit_password_plain = panel.prompt_password.clone();
            panel.prompt_password.clear();
            panel.pending_password_entry = None;
            connect_to_entry(srv, local_cfg, panel, cmd_tx, save_cfg_cb);
        } else if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            panel.prompt_password.clear();
            panel.pending_password_entry = None;
        }
    }

    // Add / Edit Server Modal
    if panel.is_editing {
        centered_modal(tr(lang, TextKey::EditServer), 440.0).show(ctx, |ui| {
            egui::Grid::new("server_edit_form")
                .num_columns(2)
                .spacing([14.0, 9.0])
                .show(ui, |ui| {
                    ui.label(tr(lang, TextKey::ServerName));
                    ui.add(
                        egui::TextEdit::singleline(&mut panel.edit_entry.name)
                            .desired_width(260.0),
                    );
                    ui.end_row();

                    ui.label(tr(lang, TextKey::ServerAddress));
                    ui.add(
                        egui::TextEdit::singleline(&mut panel.edit_entry.address)
                            .desired_width(260.0)
                            .hint_text("192.168.1.10 / noc.corp.local"),
                    );
                    ui.end_row();

                    ui.label(tr(lang, TextKey::ServerPort));
                    ui.add(egui::DragValue::new(&mut panel.edit_entry.port).range(1..=65535));
                    ui.end_row();

                    ui.label(tr(lang, TextKey::Username));
                    ui.add(
                        egui::TextEdit::singleline(&mut panel.edit_entry.username)
                            .desired_width(260.0),
                    );
                    ui.end_row();

                    ui.label(tr(lang, TextKey::Password));
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut panel.edit_password_plain)
                                .desired_width(195.0)
                                .password(!panel.show_password),
                        );
                        ui.checkbox(
                            &mut panel.show_password,
                            pick_lang(lang, "Lihat", "Show"),
                        );
                    });
                    ui.end_row();

                    ui.label(tr(lang, TextKey::Comment));
                    ui.add(
                        egui::TextEdit::singleline(&mut panel.edit_entry.comment)
                            .desired_width(260.0),
                    );
                    ui.end_row();
                });

            ui.add_space(8.0);
            ui.checkbox(
                &mut panel.edit_entry.remember_password,
                tr(lang, TextKey::RememberPasswordDpapi),
            );

            let valid = !panel.edit_entry.name.trim().is_empty()
                && !panel.edit_entry.address.trim().is_empty()
                && !panel.edit_entry.username.trim().is_empty();
            let (save_clicked, cancel_clicked) = modal_footer_buttons(
                ui,
                theme,
                valid,
                tr(lang, TextKey::Save),
                tr(lang, TextKey::Cancel),
            );
            if save_clicked {
                let mut entry = panel.edit_entry.clone();
                entry.name = entry.name.trim().to_string();
                entry.address = entry.address.trim().to_string();
                entry.username = entry.username.trim().to_string();
                if entry.remember_password && !panel.edit_password_plain.is_empty() {
                    match dpapi_protect_b64(&panel.edit_password_plain) {
                        Ok(b64) => entry.encrypted_password_b64 = b64,
                        Err(e) => {
                            panel.status_banner = e;
                            entry.remember_password = false;
                        }
                    }
                } else if !entry.remember_password {
                    entry.encrypted_password_b64.clear();
                }
                if let Some(pos) = local_cfg.servers.iter().position(|s| s.id == entry.id) {
                    local_cfg.servers[pos] = entry;
                } else {
                    local_cfg.servers.push(entry);
                }
                save_cfg_cb(local_cfg);
                panel.is_editing = false;
            } else if cancel_clicked {
                panel.is_editing = false;
            }
        });
    }

    // TOFU Verification / Mismatch Modal
    if let Some(decision) = panel.pending_tofu.clone() {
        match decision {
            TofuDecision::Trusted { .. } => {
                panel.pending_tofu = None;
            }
            TofuDecision::FirstUse {
                host_key,
                fingerprint,
            } => {
                centered_modal(tr(lang, TextKey::TofuFirstUseTitle), 500.0).show(ctx, |ui| {
                    notice_frame(theme, accent_rgb(theme)).show(ui, |ui| {
                        ui.label(tr(lang, TextKey::TofuFirstUseBody));
                        ui.add_space(6.0);
                        ui.monospace(format!("Server  : {host_key}"));
                        ui.monospace(format!("SHA-256 : {fingerprint}"));
                    });
                    let (trust, cancel) = modal_footer_buttons(
                        ui,
                        theme,
                        true,
                        tr(lang, TextKey::TofuTrustAndContinue),
                        tr(lang, TextKey::Cancel),
                    );
                    if trust {
                        local_cfg
                            .known_fingerprints
                            .insert(host_key.clone(), fingerprint.clone());
                        save_cfg_cb(local_cfg);
                        let _ = cmd_tx.try_send(ClientCommand::TrustFingerprint {
                            host_key,
                            fingerprint,
                        });
                        panel.pending_tofu = None;
                    } else if cancel {
                        panel.pending_tofu = None;
                    }
                });
            }
            TofuDecision::Mismatch {
                host_key,
                previous,
                current,
            } => {
                centered_modal(tr(lang, TextKey::TofuMismatchTitle), 520.0).show(ctx, |ui| {
                    notice_frame(theme, state_color(theme, v1::ProbeState::Down)).show(ui, |ui| {
                        ui.strong(tr(lang, TextKey::TofuMismatchBody));
                        ui.add_space(6.0);
                        ui.monospace(format!("Server   : {host_key}"));
                        ui.monospace(format!("Previous : {previous}"));
                        ui.monospace(format!("Current  : {current}"));
                    });
                    let (trust, cancel) = modal_footer_buttons(
                        ui,
                        theme,
                        true,
                        tr(lang, TextKey::TofuTrustAndContinue),
                        tr(lang, TextKey::Cancel),
                    );
                    if trust {
                        local_cfg
                            .known_fingerprints
                            .insert(host_key.clone(), current.clone());
                        save_cfg_cb(local_cfg);
                        let _ = cmd_tx.try_send(ClientCommand::TrustFingerprint {
                            host_key,
                            fingerprint: current,
                        });
                        panel.pending_tofu = None;
                    } else if cancel {
                        panel.pending_tofu = None;
                    }
                });
            }
        }
    }
}

pub fn connect_to_entry(
    srv: SavedServerEntry,
    local_cfg: &mut ClientLocalConfig,
    panel: &mut LoaderPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
    save_cfg_cb: &mut dyn FnMut(&ClientLocalConfig),
) {
    let password = if srv.remember_password && !srv.encrypted_password_b64.is_empty() {
        dpapi_unprotect_b64(&srv.encrypted_password_b64).unwrap_or_default()
    } else {
        panel.edit_password_plain.clone()
    };
    local_cfg.last_server_id = Some(srv.id.clone());
    save_cfg_cb(local_cfg);
    let _ = cmd_tx.try_send(ClientCommand::Connect(ConnectParams {
        server_id: srv.id,
        server_name: srv.name,
        address: srv.address,
        port: srv.port,
        username: srv.username,
        password,
    }));
}
