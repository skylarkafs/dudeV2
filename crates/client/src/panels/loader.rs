use crate::platform::{dpapi_protect_b64, dpapi_unprotect_b64};
use crate::ui_kit::{
    accent_rgb, card_frame, empty_state_card, muted_color, notice_frame, pick_lang,
    section_heading, status_pill,
};
use eframe::egui;
use nextdude_client_core::config::{ClientLocalConfig, SavedServerEntry};
use nextdude_client_core::connection::{ClientCommand, ConnectParams, ConnectionPhase};
use nextdude_client_core::i18n::{tr, Language, TextKey};
use nextdude_client_core::theme::{state_color, RgbColor, ThemeMode};
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
}

#[allow(clippy::too_many_arguments)]
pub fn render_loader_panel(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    lang: Language,
    theme: ThemeMode,
    phase: &ConnectionPhase,
    local_cfg: &mut ClientLocalConfig,
    panel: &mut LoaderPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
    save_cfg_cb: &mut dyn FnMut(&ClientLocalConfig),
) {
    ui.horizontal(|ui| {
        section_heading(
            ui,
            theme,
            tr(lang, TextKey::AppTitle),
            Some(pick_lang(
                lang,
                "Pilih server NextDude Linux tujuan (gRPC TLS 1.3 + TOFU)",
                "Select a target NextDude Linux server (gRPC TLS 1.3 + TOFU)",
            )),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (status_text, status_rgb) = match phase {
                ConnectionPhase::Connected => (
                    tr(lang, TextKey::StatusConnected).to_string(),
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
                    format!("{} (#{n})", tr(lang, TextKey::StatusReconnecting)),
                    RgbColor::new(245, 158, 11),
                ),
            };
            status_pill(ui, &status_text, status_rgb);
        });
    });
    ui.add_space(6.0);

    if !panel.status_banner.is_empty() {
        let is_err = panel.status_banner.starts_with("Error:");
        let col = if is_err {
            state_color(theme, v1::ProbeState::Down)
        } else {
            accent_rgb(theme)
        };
        notice_frame(theme, col).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(&panel.status_banner);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("x").clicked() {
                        panel.status_banner.clear();
                    }
                });
            });
        });
        ui.add_space(6.0);
    }

    card_frame(theme).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui.button(format!("+ {}", tr(lang, TextKey::AddServer))).clicked() {
                panel.is_editing = true;
                panel.show_password = false;
                panel.edit_entry = SavedServerEntry {
                    id: format!("srv-{}", chrono::Utc::now().timestamp_millis()),
                    port: nextdude_common::DEFAULT_GRPC_PORT,
                    ..Default::default()
                };
                panel.edit_password_plain.clear();
            }
            if ui
                .checkbox(&mut local_cfg.auto_connect_last, tr(lang, TextKey::AutoConnectLast))
                .changed()
            {
                save_cfg_cb(local_cfg);
            }
            if *phase != ConnectionPhase::Disconnected && ui.button(tr(lang, TextKey::Disconnect)).clicked() {
                let _ = cmd_tx.try_send(ClientCommand::Disconnect);
            }
        });
    });

    ui.add_space(8.0);
    if local_cfg.servers.is_empty() {
        empty_state_card(
            ui,
            theme,
            pick_lang(lang, "Belum ada profil server tersimpan", "No saved server profiles yet"),
            pick_lang(
                lang,
                "Klik '+ Tambah Server' untuk menambahkan alamat IP/hostname server NextDude Linux beserta kredensial admin.",
                "Click '+ Add Server' to add your NextDude Linux server IP/hostname and admin credentials.",
            ),
        );
    } else {
        let mut to_delete: Option<String> = None;
        let mut to_connect: Option<SavedServerEntry> = None;
        let mut to_edit: Option<SavedServerEntry> = None;

        card_frame(theme).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("server_loader_grid")
                    .striped(true)
                    .spacing([14.0, 8.0])
                    .num_columns(6)
                    .show(ui, |ui| {
                        ui.strong(tr(lang, TextKey::ServerName));
                        ui.strong(tr(lang, TextKey::ServerAddress));
                        ui.strong(tr(lang, TextKey::ServerPort));
                        ui.strong(tr(lang, TextKey::Username));
                        ui.strong(tr(lang, TextKey::Comment));
                        ui.strong(tr(lang, TextKey::Action));
                        ui.end_row();

                        for srv in &local_cfg.servers {
                            let is_last = local_cfg.last_server_id.as_deref() == Some(srv.id.as_str());
                            ui.horizontal(|ui| {
                                ui.strong(&srv.name);
                                if is_last && *phase == ConnectionPhase::Connected {
                                    status_pill(
                                        ui,
                                        pick_lang(lang, "AKTIF", "ACTIVE"),
                                        state_color(theme, v1::ProbeState::Up),
                                    );
                                }
                            });
                            ui.monospace(&srv.address);
                            ui.monospace(srv.port.to_string());
                            ui.label(&srv.username);
                            ui.label(egui::RichText::new(&srv.comment).color(muted_color(theme)));
                            ui.horizontal(|ui| {
                                if ui.button(tr(lang, TextKey::Connect)).clicked() {
                                    to_connect = Some(srv.clone());
                                }
                                if ui.button(tr(lang, TextKey::EditServer)).clicked() {
                                    to_edit = Some(srv.clone());
                                }
                                if ui.button(tr(lang, TextKey::DeleteServer)).clicked() {
                                    to_delete = Some(srv.id.clone());
                                }
                            });
                            ui.end_row();
                        }
                    });
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
            panel.edit_password_plain = if srv.remember_password && !srv.encrypted_password_b64.is_empty() {
                dpapi_unprotect_b64(&srv.encrypted_password_b64).unwrap_or_default()
            } else {
                String::new()
            };
            panel.edit_entry = srv;
        }
        if let Some(srv) = to_connect {
            connect_to_entry(srv, local_cfg, panel, cmd_tx, save_cfg_cb);
        }
    }

    if panel.is_editing {
        egui::Window::new(tr(lang, TextKey::EditServer))
            .collapsible(false)
            .resizable(false)
            .min_width(420.0)
            .show(ctx, |ui| {
                egui::Grid::new("server_edit_form")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(tr(lang, TextKey::ServerName));
                        ui.text_edit_singleline(&mut panel.edit_entry.name);
                        ui.end_row();

                        ui.label(tr(lang, TextKey::ServerAddress));
                        ui.text_edit_singleline(&mut panel.edit_entry.address);
                        ui.end_row();

                        ui.label(tr(lang, TextKey::ServerPort));
                        ui.add(egui::DragValue::new(&mut panel.edit_entry.port).range(1..=65535));
                        ui.end_row();

                        ui.label(tr(lang, TextKey::Username));
                        ui.text_edit_singleline(&mut panel.edit_entry.username);
                        ui.end_row();

                        ui.label(tr(lang, TextKey::Password));
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut panel.edit_password_plain)
                                    .password(!panel.show_password),
                            );
                            ui.checkbox(
                                &mut panel.show_password,
                                pick_lang(lang, "Tampilkan", "Show"),
                            );
                        });
                        ui.end_row();

                        ui.label(tr(lang, TextKey::Comment));
                        ui.text_edit_singleline(&mut panel.edit_entry.comment);
                        ui.end_row();
                    });

                ui.add_space(6.0);
                ui.checkbox(
                    &mut panel.edit_entry.remember_password,
                    tr(lang, TextKey::RememberPasswordDpapi),
                );
                ui.add_space(8.0);
                let valid = !panel.edit_entry.name.trim().is_empty()
                    && !panel.edit_entry.address.trim().is_empty()
                    && !panel.edit_entry.username.trim().is_empty();
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(valid, egui::Button::new(tr(lang, TextKey::Save)))
                        .clicked()
                    {
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
                    }
                    if ui.button(tr(lang, TextKey::Cancel)).clicked() {
                        panel.is_editing = false;
                    }
                });
            });
    }

    // TOFU Verification / Mismatch Modal with severity visual distinction
    if let Some(decision) = panel.pending_tofu.clone() {
        match decision {
            TofuDecision::Trusted { .. } => {
                panel.pending_tofu = None;
            }
            TofuDecision::FirstUse { host_key, fingerprint } => {
                egui::Window::new(tr(lang, TextKey::TofuFirstUseTitle))
                    .collapsible(false)
                    .resizable(false)
                    .min_width(480.0)
                    .show(ctx, |ui| {
                        notice_frame(theme, accent_rgb(theme)).show(ui, |ui| {
                            ui.label(tr(lang, TextKey::TofuFirstUseBody));
                            ui.add_space(6.0);
                            ui.monospace(format!("Server  : {host_key}"));
                            ui.monospace(format!("SHA-256 : {fingerprint}"));
                        });
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui.button(tr(lang, TextKey::TofuTrustAndContinue)).clicked() {
                                local_cfg.known_fingerprints.insert(host_key.clone(), fingerprint.clone());
                                save_cfg_cb(local_cfg);
                                let _ = cmd_tx.try_send(ClientCommand::TrustFingerprint {
                                    host_key,
                                    fingerprint,
                                });
                                panel.pending_tofu = None;
                            }
                            if ui.button(tr(lang, TextKey::Cancel)).clicked() {
                                panel.pending_tofu = None;
                            }
                        });
                    });
            }
            TofuDecision::Mismatch { host_key, previous, current } => {
                egui::Window::new(tr(lang, TextKey::TofuMismatchTitle))
                    .collapsible(false)
                    .resizable(false)
                    .min_width(500.0)
                    .show(ctx, |ui| {
                        notice_frame(theme, state_color(theme, v1::ProbeState::Down)).show(ui, |ui| {
                            ui.strong(tr(lang, TextKey::TofuMismatchBody));
                            ui.add_space(6.0);
                            ui.monospace(format!("Server   : {host_key}"));
                            ui.monospace(format!("Previous : {previous}"));
                            ui.monospace(format!("Current  : {current}"));
                        });
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui.button(tr(lang, TextKey::TofuTrustAndContinue)).clicked() {
                                local_cfg.known_fingerprints.insert(host_key.clone(), current.clone());
                                save_cfg_cb(local_cfg);
                                let _ = cmd_tx.try_send(ClientCommand::TrustFingerprint {
                                    host_key,
                                    fingerprint: current,
                                });
                                panel.pending_tofu = None;
                            }
                            if ui.button(tr(lang, TextKey::Cancel)).clicked() {
                                panel.pending_tofu = None;
                            }
                        });
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
