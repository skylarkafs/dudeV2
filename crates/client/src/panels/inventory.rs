use crate::map_canvas::format_bps;
use crate::platform::launch_external_command;
use eframe::egui;
use nextdude_client_core::config::ClientLocalConfig;
use nextdude_client_core::connection::ClientCommand;
use nextdude_client_core::i18n::{tr, Language, TextKey};
use nextdude_client_core::state::ClientStateCache;
use nextdude_client_core::theme::{state_badge_glyph, state_color, ThemeMode};
use nextdude_common::proto::v1;
use tokio::sync::mpsc;

#[derive(Default)]
pub struct InventoryPanelState {
    pub filter: String,
    pub edit_device: Option<v1::Device>,
    pub edit_community: String,
    pub edit_auth_pass: String,
    pub edit_priv_pass: String,
    pub edit_service: Option<v1::DeviceService>,
    pub edit_probe: Option<v1::ProbeDefinition>,
    pub edit_link: Option<v1::NetworkLink>,
    pub edit_link_type: Option<v1::LinkType>,
    pub edit_device_type: Option<v1::DeviceType>,
    pub status_msg: String,
}

pub fn render_devices_panel(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    lang: Language,
    theme: ThemeMode,
    state: &ClientStateCache,
    local_cfg: &ClientLocalConfig,
    inv: &mut InventoryPanelState,
    active_map_id: Option<i64>,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.horizontal(|ui| {
        ui.heading(tr(lang, TextKey::PanelDevices));
        ui.label(tr(lang, TextKey::Filter));
        ui.text_edit_singleline(&mut inv.filter);
        if ui.button(tr(lang, TextKey::MapToolAddDevice)).clicked() {
            open_new_device_editor(inv, state, active_map_id.unwrap_or(1), 200.0, 200.0);
        }
    });
    if !inv.status_msg.is_empty() {
        ui.label(&inv.status_msg);
    }

    let f = inv.filter.to_ascii_lowercase();
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("devices_table")
            .striped(true)
            .num_columns(7)
            .show(ui, |ui| {
                ui.strong(tr(lang, TextKey::Name));
                ui.strong(tr(lang, TextKey::Address));
                ui.strong(tr(lang, TextKey::Vendor));
                ui.strong(tr(lang, TextKey::Model));
                ui.strong(tr(lang, TextKey::Status));
                ui.strong("RTT");
                ui.strong(tr(lang, TextKey::Action));
                ui.end_row();

                for dev in state.devices.values() {
                    if !f.is_empty()
                        && !dev.name.to_ascii_lowercase().contains(&f)
                        && !dev.address.contains(&f)
                        && !dev.vendor.to_ascii_lowercase().contains(&f)
                    {
                        continue;
                    }
                    let st = v1::ProbeState::try_from(dev.aggregate_state).unwrap_or(v1::ProbeState::Unknown);
                    let col = state_color(theme, st);
                    let c32 = egui::Color32::from_rgb(col.r, col.g, col.b);
                    ui.label(&dev.name);
                    ui.label(&dev.address);
                    ui.label(&dev.vendor);
                    ui.label(&dev.model);
                    ui.colored_label(c32, state_badge_glyph(st));
                    ui.label(format!("{:.1} ms", dev.last_rtt_ms));
                    ui.horizontal(|ui| {
                        if ui.button("Edit").clicked() {
                            inv.edit_device = Some(dev.clone());
                            inv.edit_community.clear();
                            inv.edit_auth_pass.clear();
                            inv.edit_priv_pass.clear();
                        }
                        if ui.button("Winbox/SSH").clicked() {
                            let tpl = state
                                .device_types
                                .get(&dev.device_type_id)
                                .map(|dt| dt.launcher_command.clone())
                                .unwrap_or_default();
                            let cmd_str = if tpl.starts_with("winbox") {
                                format!(
                                    "{} {}",
                                    local_cfg.external_tools.get("winbox").map(String::as_str).unwrap_or("winbox"),
                                    dev.address
                                )
                            } else if tpl.starts_with("ssh") {
                                format!(
                                    "{} -ssh {}",
                                    local_cfg.external_tools.get("ssh").map(String::as_str).unwrap_or("putty"),
                                    dev.address
                                )
                            } else {
                                tpl
                            };
                            if let Err(e) = launch_external_command(&cmd_str, &dev.address) {
                                inv.status_msg = e;
                            }
                        }
                        if ui.button(tr(lang, TextKey::Delete)).clicked() {
                            let _ = cmd_tx.try_send(ClientCommand::DeleteDevice(v1::IdRequest {
                                id: dev.id,
                                expected_version: dev.version,
                            }));
                        }
                    });
                    ui.end_row();
                }
            });
    });

    render_device_editor_modal(ctx, lang, state, inv, cmd_tx);
}

pub fn open_new_device_editor(
    inv: &mut InventoryPanelState,
    state: &ClientStateCache,
    map_id: i64,
    x: f64,
    y: f64,
) {
    let default_type = state.device_types.keys().next().copied().unwrap_or(1);
    inv.edit_device = Some(v1::Device {
        id: 0,
        version: 1,
        map_id,
        name: String::new(),
        address: String::new(),
        device_type_id: default_type,
        pos_x: x,
        pos_y: y,
        shape: v1::NodeShape::RoundedRect as i32,
        size: v1::NodeSize::Medium as i32,
        custom_size_px: 48.0,
        label_template: "{name}\n{ip}".to_string(),
        snmp: Some(v1::SnmpCredential {
            version: v1::SnmpVersion::V2c as i32,
            port: 161,
            timeout_ms: 2500,
            retries: 1,
            ..Default::default()
        }),
        ..Default::default()
    });
    inv.edit_community.clear();
    inv.edit_auth_pass.clear();
    inv.edit_priv_pass.clear();
}

pub fn render_device_editor_modal(
    ctx: &egui::Context,
    lang: Language,
    state: &ClientStateCache,
    inv: &mut InventoryPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    let Some(mut dev) = inv.edit_device.clone() else {
        return;
    };
    let mut keep_open = true;
    let mut do_save = false;

    egui::Window::new(tr(lang, TextKey::PanelDevices))
        .open(&mut keep_open)
        .resizable(true)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(480.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr(lang, TextKey::Name));
                    ui.text_edit_singleline(&mut dev.name);
                });
                ui.horizontal(|ui| {
                    ui.label(tr(lang, TextKey::Address));
                    ui.text_edit_singleline(&mut dev.address);
                });
                egui::ComboBox::from_label(tr(lang, TextKey::PanelDeviceTypes))
                    .selected_text(
                        state
                            .device_types
                            .get(&dev.device_type_id)
                            .map_or("Select", |dt| dt.name.as_str()),
                    )
                    .show_ui(ui, |ui| {
                        for dt in state.device_types.values() {
                            ui.selectable_value(&mut dev.device_type_id, dt.id, &dt.name);
                        }
                    });
                egui::ComboBox::from_label("Custom Image")
                    .selected_text(
                        dev.custom_image_id
                            .and_then(|id| state.images.get(&id))
                            .map_or("Auto (Model / Vendor / Type)", |im| im.name.as_str()),
                    )
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut dev.custom_image_id, None, "Auto");
                        for im in state.images.values() {
                            ui.selectable_value(&mut dev.custom_image_id, Some(im.id), &im.name);
                        }
                    });

                let mut poll_ov = dev.poll_interval_sec_override.unwrap_or(0);
                ui.horizontal(|ui| {
                    ui.label("Poll Override (s, 0=inherit)");
                    ui.add(egui::DragValue::new(&mut poll_ov).range(0..=3600));
                });
                dev.poll_interval_sec_override = if poll_ov == 0 { None } else { Some(poll_ov) };

                ui.separator();
                ui.strong("SNMP Credentials (Write-Only Secrets)");
                let mut snmp = dev.snmp.clone().unwrap_or_default();
                egui::ComboBox::from_label("SNMP Version")
                    .selected_text(format!("{:?}", v1::SnmpVersion::try_from(snmp.version).unwrap_or(v1::SnmpVersion::V2c)))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut snmp.version, v1::SnmpVersion::V1 as i32, "v1");
                        ui.selectable_value(&mut snmp.version, v1::SnmpVersion::V2c as i32, "v2c");
                        ui.selectable_value(&mut snmp.version, v1::SnmpVersion::V3 as i32, "v3");
                    });
                ui.horizontal(|ui| {
                    ui.label("Port");
                    ui.add(egui::DragValue::new(&mut snmp.port).range(1..=65535));
                });
                if snmp.version == v1::SnmpVersion::V3 as i32 {
                    ui.horizontal(|ui| {
                        ui.label("v3 Username");
                        ui.text_edit_singleline(&mut snmp.username);
                    });
                    ui.horizontal(|ui| {
                        ui.label("Auth Pass");
                        ui.add(egui::TextEdit::singleline(&mut inv.edit_auth_pass).password(true));
                    });
                    ui.label(if snmp.has_auth_password {
                        tr(lang, TextKey::SecretAlreadySet)
                    } else {
                        tr(lang, TextKey::SecretNotSet)
                    });
                    ui.horizontal(|ui| {
                        ui.label("Priv Pass");
                        ui.add(egui::TextEdit::singleline(&mut inv.edit_priv_pass).password(true));
                    });
                } else {
                    ui.horizontal(|ui| {
                        ui.label("Community");
                        ui.add(egui::TextEdit::singleline(&mut inv.edit_community).password(true));
                    });
                    ui.label(if snmp.has_community {
                        tr(lang, TextKey::SecretAlreadySet)
                    } else {
                        tr(lang, TextKey::SecretNotSet)
                    });
                }
                dev.snmp = Some(snmp);

                if dev.id > 0 {
                    ui.separator();
                    ui.strong(tr(lang, TextKey::PanelServices));
                    for s in state.services.values().filter(|s| s.device_id == dev.id) {
                        ui.horizontal(|ui| {
                            ui.label(format!("{} => {}", s.name, s.last_text));
                            if ui.button(tr(lang, TextKey::Delete)).clicked() {
                                let _ = cmd_tx.try_send(ClientCommand::DeleteService(v1::IdRequest {
                                    id: s.id,
                                    expected_version: s.version,
                                }));
                            }
                        });
                    }
                    if ui.button("+ Service").clicked() {
                        inv.edit_service = Some(v1::DeviceService {
                            id: 0,
                            version: 1,
                            device_id: dev.id,
                            probe_id: state.probes.keys().next().copied().unwrap_or(1),
                            name: "New Service".to_string(),
                            enabled: true,
                            ..Default::default()
                        });
                    }
                }

                ui.add_space(6.0);
                if ui.button(tr(lang, TextKey::Save)).clicked() {
                    if let Some(ref mut sn) = dev.snmp {
                        if !inv.edit_community.is_empty() {
                            sn.community = Some(inv.edit_community.clone());
                        }
                        if !inv.edit_auth_pass.is_empty() {
                            sn.auth_password = Some(inv.edit_auth_pass.clone());
                        }
                        if !inv.edit_priv_pass.is_empty() {
                            sn.priv_password = Some(inv.edit_priv_pass.clone());
                        }
                    }
                    do_save = true;
                }
            });
        });

    if do_save {
        let _ = cmd_tx.try_send(ClientCommand::SaveDevice(dev));
        inv.edit_device = None;
    } else if !keep_open {
        inv.edit_device = None;
    } else {
        inv.edit_device = Some(dev);
    }

    // Sub-modal for adding/editing service
    if let Some(mut svc) = inv.edit_service.clone() {
        let mut open = true;
        let mut save_svc = false;
        egui::Window::new(tr(lang, TextKey::PanelServices))
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(tr(lang, TextKey::Name));
                    ui.text_edit_singleline(&mut svc.name);
                });
                egui::ComboBox::from_label("Probe")
                    .selected_text(state.probes.get(&svc.probe_id).map_or("Probe", |p| p.name.as_str()))
                    .show_ui(ui, |ui| {
                        for p in state.probes.values() {
                            ui.selectable_value(&mut svc.probe_id, p.id, &p.name);
                        }
                    });
                ui.checkbox(&mut svc.enabled, "Enabled");
                if ui.button(tr(lang, TextKey::Save)).clicked() {
                    save_svc = true;
                }
            });
        if save_svc {
            let _ = cmd_tx.try_send(ClientCommand::SaveService(svc));
            inv.edit_service = None;
        } else if !open {
            inv.edit_service = None;
        } else {
            inv.edit_service = Some(svc);
        }
    }
}

pub fn render_services_panel(
    ui: &mut egui::Ui,
    lang: Language,
    theme: ThemeMode,
    state: &ClientStateCache,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.heading(tr(lang, TextKey::PanelServices));
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("services_grid").striped(true).num_columns(6).show(ui, |ui| {
            ui.strong("Device");
            ui.strong(tr(lang, TextKey::Name));
            ui.strong(tr(lang, TextKey::Status));
            ui.strong("Value / Info");
            ui.strong("RTT");
            ui.strong(tr(lang, TextKey::Action));
            ui.end_row();
            for s in state.services.values() {
                let dname = state.devices.get(&s.device_id).map_or("-", |d| d.name.as_str());
                let st = v1::ProbeState::try_from(s.state).unwrap_or(v1::ProbeState::Unknown);
                let col = state_color(theme, st);
                ui.label(dname);
                ui.label(&s.name);
                ui.colored_label(egui::Color32::from_rgb(col.r, col.g, col.b), state_badge_glyph(st));
                ui.label(&s.last_text);
                ui.label(format!("{:.1} ms", s.last_rtt_ms));
                if ui.button(tr(lang, TextKey::Delete)).clicked() {
                    let _ = cmd_tx.try_send(ClientCommand::DeleteService(v1::IdRequest {
                        id: s.id,
                        expected_version: s.version,
                    }));
                }
                ui.end_row();
            }
        });
    });
}

pub fn render_probes_panel(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    lang: Language,
    state: &ClientStateCache,
    inv: &mut InventoryPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.horizontal(|ui| {
        ui.heading(tr(lang, TextKey::PanelProbes));
        if ui.button("+ Custom Probe").clicked() {
            inv.edit_probe = Some(v1::ProbeDefinition {
                id: 0,
                version: 1,
                name: String::new(),
                kind: v1::ProbeKind::SnmpCustom as i32,
                port: 161,
                timeout_ms: 2500,
                down_threshold: 3,
                up_threshold: 1,
                snmp_value_type: v1::SnmpValueType::Gauge as i32,
                snmp_scale: 1.0,
                ..Default::default()
            });
        }
    });
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("probes_grid").striped(true).num_columns(5).show(ui, |ui| {
            ui.strong(tr(lang, TextKey::Name));
            ui.strong(tr(lang, TextKey::Type));
            ui.strong("Port / OID");
            ui.strong("Debounce (D/U)");
            ui.strong(tr(lang, TextKey::Action));
            ui.end_row();
            for p in state.probes.values() {
                ui.label(&p.name);
                ui.label(format!("{:?}", v1::ProbeKind::try_from(p.kind).unwrap_or(v1::ProbeKind::Unspecified)));
                ui.label(if p.snmp_oid.is_empty() { p.port.to_string() } else { p.snmp_oid.clone() });
                ui.label(format!("{}/{}", p.down_threshold, p.up_threshold));
                ui.horizontal(|ui| {
                    if ui.button("Edit").clicked() {
                        inv.edit_probe = Some(p.clone());
                    }
                    if !p.builtin && ui.button(tr(lang, TextKey::Delete)).clicked() {
                        let _ = cmd_tx.try_send(ClientCommand::DeleteProbe(v1::IdRequest {
                            id: p.id,
                            expected_version: p.version,
                        }));
                    }
                });
                ui.end_row();
            }
        });
    });

    if let Some(mut p) = inv.edit_probe.clone() {
        let mut open = true;
        let mut do_save = false;
        egui::Window::new(tr(lang, TextKey::PanelProbes)).open(&mut open).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(tr(lang, TextKey::Name));
                ui.text_edit_singleline(&mut p.name);
            });
            ui.horizontal(|ui| {
                ui.label("SNMP OID");
                ui.text_edit_singleline(&mut p.snmp_oid);
            });
            ui.horizontal(|ui| {
                ui.label("Scale");
                ui.add(egui::DragValue::new(&mut p.snmp_scale).speed(0.1));
            });
            ui.horizontal(|ui| {
                ui.label("Unit");
                ui.text_edit_singleline(&mut p.unit);
            });
            ui.horizontal(|ui| {
                ui.label("Down Threshold");
                ui.add(egui::DragValue::new(&mut p.down_threshold).range(1..=20));
            });
            ui.horizontal(|ui| {
                ui.label("Up Threshold");
                ui.add(egui::DragValue::new(&mut p.up_threshold).range(1..=20));
            });
            if ui.button(tr(lang, TextKey::Save)).clicked() {
                do_save = true;
            }
        });
        if do_save {
            let _ = cmd_tx.try_send(ClientCommand::SaveProbe(p));
            inv.edit_probe = None;
        } else if !open {
            inv.edit_probe = None;
        } else {
            inv.edit_probe = Some(p);
        }
    }
}

pub fn render_links_panel(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    lang: Language,
    state: &ClientStateCache,
    inv: &mut InventoryPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.heading(tr(lang, TextKey::PanelLinks));
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("links_grid").striped(true).num_columns(7).show(ui, |ui| {
            ui.strong("ID");
            ui.strong("Src -> Dst");
            ui.strong("Interface");
            ui.strong(tr(lang, TextKey::Status));
            ui.strong("In");
            ui.strong("Out");
            ui.strong(tr(lang, TextKey::Action));
            ui.end_row();
            for l in state.links.values() {
                let sname = state.devices.get(&l.src_device_id).map_or("?", |d| d.name.as_str());
                let dname = state.devices.get(&l.dst_device_id).map_or("?", |d| d.name.as_str());
                ui.label(l.id.to_string());
                ui.label(format!("{sname} <-> {dname}"));
                ui.label(&l.interface_key);
                ui.label(format!("{:?}", v1::LinkStatus::try_from(l.status).unwrap_or(v1::LinkStatus::Unknown)));
                ui.label(format!("{} ({:.1}%)", format_bps(l.in_bps), l.in_utilization_pct));
                ui.label(format!("{} ({:.1}%)", format_bps(l.out_bps), l.out_utilization_pct));
                ui.horizontal(|ui| {
                    if ui.button("Edit").clicked() {
                        inv.edit_link = Some(l.clone());
                    }
                    if ui.button(tr(lang, TextKey::Delete)).clicked() {
                        let _ = cmd_tx.try_send(ClientCommand::DeleteLink(v1::IdRequest {
                            id: l.id,
                            expected_version: l.version,
                        }));
                    }
                });
                ui.end_row();
            }
        });
    });

    if let Some(mut link) = inv.edit_link.clone() {
        let mut open = true;
        let mut do_save = false;
        egui::Window::new(tr(lang, TextKey::PanelLinks)).open(&mut open).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Interface Key");
                ui.text_edit_singleline(&mut link.interface_key);
            });
            egui::ComboBox::from_label("Master Device")
                .selected_text(state.devices.get(&link.master_device_id).map_or("Select", |d| d.name.as_str()))
                .show_ui(ui, |ui| {
                    if let Some(s) = state.devices.get(&link.src_device_id) {
                        ui.selectable_value(&mut link.master_device_id, s.id, &s.name);
                    }
                    if let Some(d) = state.devices.get(&link.dst_device_id) {
                        ui.selectable_value(&mut link.master_device_id, d.id, &d.name);
                    }
                });
            egui::ComboBox::from_label(tr(lang, TextKey::PanelLinkTypes))
                .selected_text(state.link_types.get(&link.link_type_id).map_or("Select", |lt| lt.name.as_str()))
                .show_ui(ui, |ui| {
                    for lt in state.link_types.values() {
                        ui.selectable_value(&mut link.link_type_id, lt.id, &lt.name);
                    }
                });
            ui.checkbox(&mut link.history_enabled, "Record Traffic History");
            if ui.button(tr(lang, TextKey::Save)).clicked() {
                do_save = true;
            }
        });
        if do_save {
            let _ = cmd_tx.try_send(ClientCommand::SaveLink(link));
            inv.edit_link = None;
        } else if !open {
            inv.edit_link = None;
        } else {
            inv.edit_link = Some(link);
        }
    }
}

pub fn render_link_types_panel(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    lang: Language,
    state: &ClientStateCache,
    inv: &mut InventoryPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.horizontal(|ui| {
        ui.heading(tr(lang, TextKey::PanelLinkTypes));
        if ui.button("+ Link Type").clicked() {
            inv.edit_link_type = Some(v1::LinkType {
                id: 0,
                version: 1,
                name: String::new(),
                max_speed_bps: 1_000_000_000,
                thickness: 2.5,
                style: v1::LinkStyle::Curved as i32,
                color_hex: "#38BDF8".to_string(),
                builtin: false,
            });
        }
    });
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("ltypes_grid").striped(true).num_columns(5).show(ui, |ui| {
            ui.strong(tr(lang, TextKey::Name));
            ui.strong("Max Speed");
            ui.strong("Thickness");
            ui.strong("Style");
            ui.strong(tr(lang, TextKey::Action));
            ui.end_row();
            for lt in state.link_types.values() {
                ui.label(&lt.name);
                ui.label(format_bps(lt.max_speed_bps as f64));
                ui.label(format!("{:.1}", lt.thickness));
                ui.label(format!("{:?}", v1::LinkStyle::try_from(lt.style).unwrap_or(v1::LinkStyle::Curved)));
                ui.horizontal(|ui| {
                    if ui.button("Edit").clicked() {
                        inv.edit_link_type = Some(lt.clone());
                    }
                    if !lt.builtin && ui.button(tr(lang, TextKey::Delete)).clicked() {
                        let _ = cmd_tx.try_send(ClientCommand::DeleteLinkType(v1::IdRequest {
                            id: lt.id,
                            expected_version: lt.version,
                        }));
                    }
                });
                ui.end_row();
            }
        });
    });

    if let Some(mut lt) = inv.edit_link_type.clone() {
        let mut open = true;
        let mut do_save = false;
        egui::Window::new(tr(lang, TextKey::PanelLinkTypes)).open(&mut open).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(tr(lang, TextKey::Name));
                ui.text_edit_singleline(&mut lt.name);
            });
            ui.horizontal(|ui| {
                ui.label("Max Speed (bps)");
                ui.add(egui::DragValue::new(&mut lt.max_speed_bps).speed(1_000_000.0));
            });
            ui.horizontal(|ui| {
                ui.label("Thickness");
                ui.add(egui::DragValue::new(&mut lt.thickness).range(0.5..=12.0));
            });
            ui.horizontal(|ui| {
                ui.label("Color Hex");
                ui.text_edit_singleline(&mut lt.color_hex);
            });
            if ui.button(tr(lang, TextKey::Save)).clicked() {
                do_save = true;
            }
        });
        if do_save {
            let _ = cmd_tx.try_send(ClientCommand::SaveLinkType(lt));
            inv.edit_link_type = None;
        } else if !open {
            inv.edit_link_type = None;
        } else {
            inv.edit_link_type = Some(lt);
        }
    }
}

pub fn render_device_types_panel(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    lang: Language,
    state: &ClientStateCache,
    inv: &mut InventoryPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.horizontal(|ui| {
        ui.heading(tr(lang, TextKey::PanelDeviceTypes));
        if ui.button("+ Device Type").clicked() {
            inv.edit_device_type = Some(v1::DeviceType {
                id: 0,
                version: 1,
                name: String::new(),
                default_icon_key: "generic".to_string(),
                ..Default::default()
            });
        }
    });
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("dtypes_grid").striped(true).num_columns(4).show(ui, |ui| {
            ui.strong(tr(lang, TextKey::Name));
            ui.strong("Icon Key");
            ui.strong("Launcher");
            ui.strong(tr(lang, TextKey::Action));
            ui.end_row();
            for dt in state.device_types.values() {
                ui.label(&dt.name);
                ui.label(&dt.default_icon_key);
                ui.label(&dt.launcher_command);
                ui.horizontal(|ui| {
                    if ui.button("Edit").clicked() {
                        inv.edit_device_type = Some(dt.clone());
                    }
                    if ui.button(tr(lang, TextKey::Delete)).clicked() {
                        let _ = cmd_tx.try_send(ClientCommand::DeleteDeviceType(v1::IdRequest {
                            id: dt.id,
                            expected_version: dt.version,
                        }));
                    }
                });
                ui.end_row();
            }
        });
    });

    if let Some(mut dt) = inv.edit_device_type.clone() {
        let mut open = true;
        let mut do_save = false;
        egui::Window::new(tr(lang, TextKey::PanelDeviceTypes)).open(&mut open).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(tr(lang, TextKey::Name));
                ui.text_edit_singleline(&mut dt.name);
            });
            ui.horizontal(|ui| {
                ui.label("Icon Key");
                ui.text_edit_singleline(&mut dt.default_icon_key);
            });
            ui.horizontal(|ui| {
                ui.label("Launcher");
                ui.text_edit_singleline(&mut dt.launcher_command);
            });
            if ui.button(tr(lang, TextKey::Save)).clicked() {
                do_save = true;
            }
        });
        if do_save {
            let _ = cmd_tx.try_send(ClientCommand::SaveDeviceType(dt));
            inv.edit_device_type = None;
        } else if !open {
            inv.edit_device_type = None;
        } else {
            inv.edit_device_type = Some(dt);
        }
    }
}

pub fn render_networks_and_submaps(
    ui: &mut egui::Ui,
    lang: Language,
    state: &ClientStateCache,
    is_submaps: bool,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    if is_submaps {
        ui.heading(tr(lang, TextKey::PanelSubmaps));
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("submaps_grid").striped(true).num_columns(4).show(ui, |ui| {
                ui.strong("Map");
                ui.strong("Target Map");
                ui.strong("Label");
                ui.strong(tr(lang, TextKey::Action));
                ui.end_row();
                for s in state.submaps.values() {
                    ui.label(state.maps.get(&s.map_id).map_or("-", |m| m.name.as_str()));
                    ui.label(state.maps.get(&s.target_map_id).map_or("-", |m| m.name.as_str()));
                    ui.label(&s.label);
                    if ui.button(tr(lang, TextKey::Delete)).clicked() {
                        let _ = cmd_tx.try_send(ClientCommand::DeleteSubmap(v1::IdRequest {
                            id: s.id,
                            expected_version: s.version,
                        }));
                    }
                    ui.end_row();
                }
            });
        });
    } else {
        ui.heading(tr(lang, TextKey::PanelNetworks));
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("networks_grid").striped(true).num_columns(4).show(ui, |ui| {
                ui.strong(tr(lang, TextKey::Name));
                ui.strong("CIDR");
                ui.strong("Map");
                ui.strong(tr(lang, TextKey::Action));
                ui.end_row();
                for n in state.networks.values() {
                    ui.label(&n.name);
                    ui.label(&n.cidr);
                    ui.label(state.maps.get(&n.map_id).map_or("-", |m| m.name.as_str()));
                    if ui.button(tr(lang, TextKey::Delete)).clicked() {
                        let _ = cmd_tx.try_send(ClientCommand::DeleteNetwork(v1::IdRequest {
                            id: n.id,
                            expected_version: n.version,
                        }));
                    }
                    ui.end_row();
                }
            });
        });
    }
}

pub fn render_outages_panel(ui: &mut egui::Ui, lang: Language, state: &ClientStateCache) {
    ui.heading(tr(lang, TextKey::PanelOutages));
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("outages_grid").striped(true).num_columns(5).show(ui, |ui| {
            ui.strong("Device");
            ui.strong("Service");
            ui.strong("Started (Local)");
            ui.strong("Ended (Local)");
            ui.strong("Suppressed");
            ui.end_row();
            for o in &state.outages {
                ui.label(&o.device_name);
                ui.label(&o.service_name);
                ui.label(fmt_local_ts(o.started_utc));
                ui.label(o.ended_utc.map_or("ACTIVE".to_string(), fmt_local_ts));
                ui.label(if o.suppressed_by_dependency { tr(lang, TextKey::Yes) } else { tr(lang, TextKey::No) });
                ui.end_row();
            }
        });
    });
}

pub fn fmt_local_ts(utc_sec: i64) -> String {
    chrono::DateTime::from_timestamp(utc_sec, 0)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| utc_sec.to_string())
}
