use crate::platform::{dpapi_protect_b64, dpapi_unprotect_b64};
use eframe::egui;
use nextdude_client_core::config::{ClientLocalConfig, SavedServerEntry};
use nextdude_client_core::connection::{ClientCommand, ConnectParams, ConnectionPhase};
use nextdude_client_core::i18n::{tr, Language, TextKey};
use nextdude_client_core::tofu::TofuDecision;
use tokio::sync::mpsc;

#[derive(Default)]
pub struct LoaderPanelState {
    pub edit_entry: SavedServerEntry,
    pub edit_password_plain: String,
    pub is_editing: bool,
    pub status_banner: String,
    pub pending_tofu: Option<TofuDecision>,
}

pub fn render_loader_panel(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    lang: Language,
    phase: &ConnectionPhase,
    local_cfg: &mut ClientLocalConfig,
    panel: &mut LoaderPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
    save_cfg_cb: &mut dyn FnMut(&ClientLocalConfig),
) {
    ui.heading(tr(lang, TextKey::AppTitle));
    let status_text = match phase {
        ConnectionPhase::Connected => tr(lang, TextKey::StatusConnected).to_string(),
        ConnectionPhase::Connecting => tr(lang, TextKey::StatusConnecting).to_string(),
        ConnectionPhase::Disconnected => tr(lang, TextKey::StatusDisconnected).to_string(),
        ConnectionPhase::Reconnecting(n) => format!("{} (#{n})", tr(lang, TextKey::StatusReconnecting)),
    };
    ui.horizontal(|ui| {
        ui.strong(format!("{}: {}", tr(lang, TextKey::Status), status_text));
        if !panel.status_banner.is_empty() {
            ui.label(format!(" | {}", panel.status_banner));
        }
    });
    ui.separator();

    ui.horizontal(|ui| {
        if ui.button(tr(lang, TextKey::AddServer)).clicked() {
            panel.is_editing = true;
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

    ui.add_space(6.0);
    let mut to_delete: Option<String> = None;
    let mut to_connect: Option<SavedServerEntry> = None;
    let mut to_edit: Option<SavedServerEntry> = None;

    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("server_loader_grid")
            .striped(true)
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
                    ui.label(&srv.name);
                    ui.label(&srv.address);
                    ui.label(srv.port.to_string());
                    ui.label(&srv.username);
                    ui.label(&srv.comment);
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

    if let Some(id) = to_delete {
        local_cfg.servers.retain(|s| s.id != id);
        if local_cfg.last_server_id.as_deref() == Some(&id) {
            local_cfg.last_server_id = None;
        }
        save_cfg_cb(local_cfg);
    }
    if let Some(srv) = to_edit {
        panel.is_editing = true;
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

    if panel.is_editing {
        egui::Window::new(tr(lang, TextKey::EditServer))
            .collapsible(false)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr(lang, TextKey::ServerName));
                    ui.text_edit_singleline(&mut panel.edit_entry.name);
                });
                ui.horizontal(|ui| {
                    ui.label(tr(lang, TextKey::ServerAddress));
                    ui.text_edit_singleline(&mut panel.edit_entry.address);
                });
                ui.horizontal(|ui| {
                    ui.label(tr(lang, TextKey::ServerPort));
                    ui.add(egui::DragValue::new(&mut panel.edit_entry.port).range(1..=65535));
                });
                ui.horizontal(|ui| {
                    ui.label(tr(lang, TextKey::Username));
                    ui.text_edit_singleline(&mut panel.edit_entry.username);
                });
                ui.horizontal(|ui| {
                    ui.label(tr(lang, TextKey::Password));
                    ui.add(egui::TextEdit::singleline(&mut panel.edit_password_plain).password(true));
                });
                ui.horizontal(|ui| {
                    ui.label(tr(lang, TextKey::Comment));
                    ui.text_edit_singleline(&mut panel.edit_entry.comment);
                });
                ui.checkbox(
                    &mut panel.edit_entry.remember_password,
                    tr(lang, TextKey::RememberPasswordDpapi),
                );
                ui.horizontal(|ui| {
                    if ui.button(tr(lang, TextKey::Save)).clicked() {
                        let mut entry = panel.edit_entry.clone();
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

    // TOFU Verification / Mismatch Modal
    if let Some(decision) = panel.pending_tofu.clone() {
        let (title, body, host_key, fp) = match decision {
            TofuDecision::FirstUse { host_key, fingerprint } => (
                tr(lang, TextKey::TofuFirstUseTitle),
                format!("{}\n\nServer: {}\nSHA-256: {}", tr(lang, TextKey::TofuFirstUseBody), host_key, fingerprint),
                host_key,
                fingerprint,
            ),
            TofuDecision::Mismatch { host_key, previous, current } => (
                tr(lang, TextKey::TofuMismatchTitle),
                format!("{}\n\nServer: {}\nPrevious: {}\nNew: {}", tr(lang, TextKey::TofuMismatchBody), host_key, previous, current),
                host_key,
                current,
            ),
            TofuDecision::Trusted { .. } => {
                panel.pending_tofu = None;
                return;
            }
        };

        egui::Window::new(title).collapsible(false).show(ctx, |ui| {
            ui.label(body);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button(tr(lang, TextKey::TofuTrustAndContinue)).clicked() {
                    local_cfg.known_fingerprints.insert(host_key.clone(), fp.clone());
                    save_cfg_cb(local_cfg);
                    let _ = cmd_tx.try_send(ClientCommand::TrustFingerprint {
                        host_key,
                        fingerprint: fp,
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
