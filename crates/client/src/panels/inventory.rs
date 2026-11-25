use crate::map_canvas::format_bps;
use crate::platform::launch_external_command;
use crate::ui_kit::{
    card_frame, empty_state_card, link_status_pill, muted_color, pick_lang, probe_kind_label,
    probe_state_pill, section_heading, toolbar_frame,
};
use eframe::egui;
use nextdude_client_core::config::ClientLocalConfig;
use nextdude_client_core::connection::ClientCommand;
use nextdude_client_core::i18n::{tr, Language, TextKey};
use nextdude_client_core::state::ClientStateCache;
use nextdude_client_core::theme::ThemeMode;
use nextdude_common::proto::v1;
use tokio::sync::mpsc;

#[derive(Default)]
pub struct InventoryPanelState {
    pub filter: String,
    pub edit_device: Option<v1::Device>,
    pub edit_community: String,
    pub edit_auth_pass: String,
    pub edit_priv_pass: String,
    pub edit_tags_csv: String,
    pub edit_service: Option<v1::DeviceService>,
    pub edit_probe: Option<v1::ProbeDefinition>,
    pub edit_link: Option<v1::NetworkLink>,
    pub edit_link_type: Option<v1::LinkType>,
    pub edit_device_type: Option<v1::DeviceType>,
    pub status_msg: String,
}

#[allow(clippy::too_many_arguments)]
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
        section_heading(
            ui,
            theme,
            tr(lang, TextKey::PanelDevices),
            Some(pick_lang(
                lang,
                "Daftar seluruh perangkat termonitor lintas peta",
                "All monitored devices across maps",
            )),
        );
    });
    ui.add_space(6.0);

    toolbar_frame(theme).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(tr(lang, TextKey::Filter));
            ui.add(
                egui::TextEdit::singleline(&mut inv.filter).hint_text(pick_lang(
                    lang,
                    "Cari nama, IP, atau vendor...",
                    "Filter by name, IP, or vendor...",
                )),
            );
            if !inv.filter.is_empty() && ui.small_button("x").clicked() {
                inv.filter.clear();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(format!("+ {}", tr(lang, TextKey::MapToolAddDevice))).clicked() {
                    open_new_device_editor(inv, state, active_map_id.unwrap_or(1), 200.0, 200.0);
                }
            });
        });
    });

    if !inv.status_msg.is_empty() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(&inv.status_msg).color(muted_color(theme)));
    }
    ui.add_space(6.0);

    let f = inv.filter.to_ascii_lowercase();
    let filtered: Vec<&v1::Device> = state
        .devices
        .values()
        .filter(|dev| {
            f.is_empty()
                || dev.name.to_ascii_lowercase().contains(&f)
                || dev.address.contains(&f)
                || dev.vendor.to_ascii_lowercase().contains(&f)
                || dev.model.to_ascii_lowercase().contains(&f)
        })
        .collect();

    if filtered.is_empty() {
        empty_state_card(
            ui,
            theme,
            pick_lang(lang, "Belum ada perangkat ditemukan", "No devices found"),
            pick_lang(
                lang,
                "Tambahkan perangkat baru atau ubah kata kunci filter.",
                "Add a new device or clear your search filter.",
            ),
        );
    } else {
        card_frame(theme).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("devices_table")
                    .striped(true)
                    .spacing([14.0, 7.0])
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

                        for dev in filtered {
                            let st = v1::ProbeState::try_from(dev.aggregate_state)
                                .unwrap_or(v1::ProbeState::Unknown);
                            ui.strong(&dev.name);
                            ui.monospace(&dev.address);
                            ui.label(&dev.vendor);
                            ui.label(&dev.model);
                            probe_state_pill(ui, lang, theme, st);
                            ui.monospace(format!("{:.1} ms", dev.last_rtt_ms));
                            ui.horizontal(|ui| {
                                if ui.button("Edit").clicked() {
                                    inv.edit_tags_csv = dev.tags.join(", ");
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
                                            local_cfg
                                                .external_tools
                                                .get("winbox")
                                                .map(String::as_str)
                                                .unwrap_or("winbox"),
                                            dev.address
                                        )
                                    } else if tpl.starts_with("ssh") {
                                        format!(
                                            "{} -ssh {}",
                                            local_cfg
                                                .external_tools
                                                .get("ssh")
                                                .map(String::as_str)
                                                .unwrap_or("putty"),
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
                                    let _ = cmd_tx.try_send(ClientCommand::DeleteDevice(
                                        v1::IdRequest {
                                            id: dev.id,
                                            expected_version: dev.version,
                                        },
                                    ));
                                }
                            });
                            ui.end_row();
                        }
                    });
            });
        });
    }

    render_device_editor_modal(ctx, lang, theme, state, inv, cmd_tx);
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
            auth_protocol: v1::SnmpAuthProtocol::SnmpAuthSha1 as i32,
            priv_protocol: v1::SnmpPrivProtocol::SnmpPrivAes128 as i32,
            ..Default::default()
        }),
        ..Default::default()
    });
    inv.edit_community.clear();
    inv.edit_auth_pass.clear();
    inv.edit_priv_pass.clear();
    inv.edit_tags_csv.clear();
}

pub fn render_device_editor_modal(
    ctx: &egui::Context,
    lang: Language,
    theme: ThemeMode,
    state: &ClientStateCache,
    inv: &mut InventoryPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    let Some(mut dev) = inv.edit_device.clone() else {
        return;
    };
    let mut keep_open = true;
    let mut do_save = false;

    egui::Window::new(if dev.id == 0 {
        tr(lang, TextKey::MapToolAddDevice)
    } else {
        tr(lang, TextKey::PanelDevices)
    })
    .open(&mut keep_open)
    .resizable(true)
    .min_width(480.0)
    .show(ctx, |ui| {
        egui::ScrollArea::vertical().max_height(540.0).show(ui, |ui| {
            egui::Grid::new("dev_editor_form")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    ui.label(tr(lang, TextKey::Name));
                    ui.text_edit_singleline(&mut dev.name);
                    ui.end_row();

                    ui.label(tr(lang, TextKey::Address));
                    ui.text_edit_singleline(&mut dev.address);
                    ui.end_row();

                    ui.label(tr(lang, TextKey::PanelDeviceTypes));
                    egui::ComboBox::from_id_salt("dev_type_cb")
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
                    ui.end_row();

                    ui.label("Custom Image");
                    egui::ComboBox::from_id_salt("dev_img_cb")
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
                    ui.end_row();

                    ui.label(pick_lang(lang, "Bentuk Node", "Node Shape"));
                    let curr_shape = v1::NodeShape::try_from(dev.shape)
                        .unwrap_or(v1::NodeShape::RoundedRect);
                    egui::ComboBox::from_id_salt("dev_shape_cb")
                        .selected_text(match curr_shape {
                            v1::NodeShape::Circle => "Circle",
                            v1::NodeShape::Frameless => "Frameless",
                            _ => "Rounded Box",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut dev.shape,
                                v1::NodeShape::RoundedRect as i32,
                                "Rounded Box",
                            );
                            ui.selectable_value(
                                &mut dev.shape,
                                v1::NodeShape::Circle as i32,
                                "Circle",
                            );
                            ui.selectable_value(
                                &mut dev.shape,
                                v1::NodeShape::Frameless as i32,
                                "Frameless",
                            );
                        });
                    ui.end_row();

                    ui.label(pick_lang(lang, "Ukuran Node", "Node Size"));
                    let curr_sz =
                        v1::NodeSize::try_from(dev.size).unwrap_or(v1::NodeSize::Medium);
                    egui::ComboBox::from_id_salt("dev_size_cb")
                        .selected_text(match curr_sz {
                            v1::NodeSize::Small => "Small",
                            v1::NodeSize::Large => "Large",
                            v1::NodeSize::Custom => "Custom",
                            _ => "Medium",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut dev.size,
                                v1::NodeSize::Small as i32,
                                "Small",
                            );
                            ui.selectable_value(
                                &mut dev.size,
                                v1::NodeSize::Medium as i32,
                                "Medium",
                            );
                            ui.selectable_value(
                                &mut dev.size,
                                v1::NodeSize::Large as i32,
                                "Large",
                            );
                            ui.selectable_value(
                                &mut dev.size,
                                v1::NodeSize::Custom as i32,
                                "Custom",
                            );
                        });
                    ui.end_row();

                    if dev.size == v1::NodeSize::Custom as i32 {
                        ui.label("Custom Size (px)");
                        ui.add(egui::DragValue::new(&mut dev.custom_size_px).range(24.0..=192.0));
                        ui.end_row();
                    }

                    ui.label("Label Template");
                    ui.text_edit_singleline(&mut dev.label_template);
                    ui.end_row();

                    let mut poll_ov = dev.poll_interval_sec_override.unwrap_or(0);
                    ui.label("Poll Override (s, 0=inherit)");
                    ui.add(egui::DragValue::new(&mut poll_ov).range(0..=3600));
                    dev.poll_interval_sec_override =
                        if poll_ov == 0 { None } else { Some(poll_ov) };
                    ui.end_row();

                    ui.label("Tags (comma)");
                    ui.text_edit_singleline(&mut inv.edit_tags_csv);
                    ui.end_row();

                    ui.label("Notes");
                    ui.add(egui::TextEdit::multiline(&mut dev.notes).desired_rows(2));
                    ui.end_row();
                });

            ui.add_space(6.0);
            ui.separator();
            ui.strong("SNMP Credentials (Write-Only Secrets)");
            let mut snmp = dev.snmp.clone().unwrap_or_default();
            egui::Grid::new("dev_snmp_grid")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    ui.label("SNMP Version");
                    let sn_ver = v1::SnmpVersion::try_from(snmp.version)
                        .unwrap_or(v1::SnmpVersion::V2c);
                    egui::ComboBox::from_id_salt("dev_snmp_ver")
                        .selected_text(match sn_ver {
                            v1::SnmpVersion::V1 => "SNMPv1",
                            v1::SnmpVersion::V3 => "SNMPv3",
                            _ => "SNMPv2c",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut snmp.version, v1::SnmpVersion::V1 as i32, "SNMPv1");
                            ui.selectable_value(&mut snmp.version, v1::SnmpVersion::V2c as i32, "SNMPv2c");
                            ui.selectable_value(&mut snmp.version, v1::SnmpVersion::V3 as i32, "SNMPv3");
                        });
                    ui.end_row();

                    ui.label("UDP Port");
                    ui.add(egui::DragValue::new(&mut snmp.port).range(1..=65535));
                    ui.end_row();

                    if snmp.version == v1::SnmpVersion::V3 as i32 {
                        ui.label("v3 Username");
                        ui.text_edit_singleline(&mut snmp.username);
                        ui.end_row();

                        ui.label("Auth Protocol");
                        let ap = v1::SnmpAuthProtocol::try_from(snmp.auth_protocol)
                            .unwrap_or(v1::SnmpAuthProtocol::SnmpAuthNone);
                        egui::ComboBox::from_id_salt("dev_snmp_auth_proto")
                            .selected_text(match ap {
                                v1::SnmpAuthProtocol::SnmpAuthMd5 => "HMAC-MD5",
                                v1::SnmpAuthProtocol::SnmpAuthSha1 => "HMAC-SHA1",
                                _ => "None",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut snmp.auth_protocol, v1::SnmpAuthProtocol::SnmpAuthNone as i32, "None");
                                ui.selectable_value(&mut snmp.auth_protocol, v1::SnmpAuthProtocol::SnmpAuthMd5 as i32, "HMAC-MD5");
                                ui.selectable_value(&mut snmp.auth_protocol, v1::SnmpAuthProtocol::SnmpAuthSha1 as i32, "HMAC-SHA1");
                            });
                        ui.end_row();

                        ui.label("Auth Pass");
                        ui.add(egui::TextEdit::singleline(&mut inv.edit_auth_pass).password(true));
                        ui.end_row();

                        ui.label("Priv Protocol");
                        let pp = v1::SnmpPrivProtocol::try_from(snmp.priv_protocol)
                            .unwrap_or(v1::SnmpPrivProtocol::SnmpPrivNone);
                        egui::ComboBox::from_id_salt("dev_snmp_priv_proto")
                            .selected_text(match pp {
                                v1::SnmpPrivProtocol::SnmpPrivDes => "DES-CBC",
                                v1::SnmpPrivProtocol::SnmpPrivAes128 => "AES-128-CFB",
                                _ => "None",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut snmp.priv_protocol, v1::SnmpPrivProtocol::SnmpPrivNone as i32, "None");
                                ui.selectable_value(&mut snmp.priv_protocol, v1::SnmpPrivProtocol::SnmpPrivDes as i32, "DES-CBC");
                                ui.selectable_value(&mut snmp.priv_protocol, v1::SnmpPrivProtocol::SnmpPrivAes128 as i32, "AES-128-CFB");
                            });
                        ui.end_row();

                        ui.label("Priv Pass");
                        ui.add(egui::TextEdit::singleline(&mut inv.edit_priv_pass).password(true));
                        ui.end_row();

                        ui.label("Status Secret");
                        ui.label(
                            egui::RichText::new(if snmp.has_auth_password {
                                tr(lang, TextKey::SecretAlreadySet)
                            } else {
                                tr(lang, TextKey::SecretNotSet)
                            })
                            .small()
                            .color(muted_color(theme)),
                        );
                        ui.end_row();
                    } else {
                        ui.label("Community");
                        ui.add(egui::TextEdit::singleline(&mut inv.edit_community).password(true));
                        ui.end_row();

                        ui.label("Status Secret");
                        ui.label(
                            egui::RichText::new(if snmp.has_community {
                                tr(lang, TextKey::SecretAlreadySet)
                            } else {
                                tr(lang, TextKey::SecretNotSet)
                            })
                            .small()
                            .color(muted_color(theme)),
                        );
                        ui.end_row();
                    }
                });
            dev.snmp = Some(snmp);

            if dev.id > 0 {
                ui.add_space(6.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.strong(tr(lang, TextKey::PanelServices));
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
                });
                for s in state.services.values().filter(|s| s.device_id == dev.id) {
                    let sst =
                        v1::ProbeState::try_from(s.state).unwrap_or(v1::ProbeState::Unknown);
                    ui.horizontal(|ui| {
                        probe_state_pill(ui, lang, theme, sst);
                        ui.label(format!("{} — {}", s.name, s.last_text));
                        if ui.button(tr(lang, TextKey::Delete)).clicked() {
                            let _ = cmd_tx.try_send(ClientCommand::DeleteService(v1::IdRequest {
                                id: s.id,
                                expected_version: s.version,
                            }));
                        }
                    });
                }
            }

            ui.add_space(10.0);
            let can_save = !dev.name.trim().is_empty() && !dev.address.trim().is_empty();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(can_save, egui::Button::new(tr(lang, TextKey::Save)))
                    .clicked()
                {
                    dev.tags = inv
                        .edit_tags_csv
                        .split(',')
                        .map(|t| t.trim().to_string())
                        .filter(|t| !t.is_empty())
                        .collect();
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
                if ui.button(tr(lang, TextKey::Cancel)).clicked() {
                    keep_open = false;
                }
            });
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
            .min_width(340.0)
            .show(ctx, |ui| {
                egui::Grid::new("svc_modal_form")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(tr(lang, TextKey::Name));
                        ui.text_edit_singleline(&mut svc.name);
                        ui.end_row();

                        ui.label("Probe");
                        egui::ComboBox::from_id_salt("svc_probe_cb")
                            .selected_text(
                                state
                                    .probes
                                    .get(&svc.probe_id)
                                    .map_or("Probe", |p| p.name.as_str()),
                            )
                            .show_ui(ui, |ui| {
                                for p in state.probes.values() {
                                    ui.selectable_value(&mut svc.probe_id, p.id, &p.name);
                                }
                            });
                        ui.end_row();
                    });
                ui.checkbox(&mut svc.enabled, "Enabled");
                ui.add_space(6.0);
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
    section_heading(
        ui,
        theme,
        tr(lang, TextKey::PanelServices),
        Some(pick_lang(
            lang,
            "Instance probe dan sensor pada setiap perangkat",
            "Active probe and sensor instances across devices",
        )),
    );
    ui.add_space(6.0);
    if state.services.is_empty() {
        empty_state_card(
            ui,
            theme,
            pick_lang(lang, "Belum ada layanan termonitor", "No monitored services"),
            pick_lang(
                lang,
                "Tambahkan service dari dialog Edit Perangkat.",
                "Add a service from any Device Editor dialog.",
            ),
        );
        return;
    }

    card_frame(theme).show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("services_grid")
                .striped(true)
                .spacing([14.0, 7.0])
                .num_columns(6)
                .show(ui, |ui| {
                    ui.strong("Device");
                    ui.strong(tr(lang, TextKey::Name));
                    ui.strong(tr(lang, TextKey::Status));
                    ui.strong("Value / Info");
                    ui.strong("RTT");
                    ui.strong(tr(lang, TextKey::Action));
                    ui.end_row();
                    for s in state.services.values() {
                        let dname = state.devices.get(&s.device_id).map_or("-", |d| d.name.as_str());
                        let st =
                            v1::ProbeState::try_from(s.state).unwrap_or(v1::ProbeState::Unknown);
                        ui.label(dname);
                        ui.strong(&s.name);
                        probe_state_pill(ui, lang, theme, st);
                        ui.label(&s.last_text);
                        ui.monospace(format!("{:.1} ms", s.last_rtt_ms));
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
    });
}

pub fn render_probes_panel(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    lang: Language,
    theme: ThemeMode,
    state: &ClientStateCache,
    inv: &mut InventoryPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.horizontal(|ui| {
        section_heading(ui, theme, tr(lang, TextKey::PanelProbes), None);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
    });
    ui.add_space(6.0);
    card_frame(theme).show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("probes_grid")
                .striped(true)
                .spacing([14.0, 7.0])
                .num_columns(5)
                .show(ui, |ui| {
                    ui.strong(tr(lang, TextKey::Name));
                    ui.strong(tr(lang, TextKey::Type));
                    ui.strong("Port / OID");
                    ui.strong("Debounce (D/U)");
                    ui.strong(tr(lang, TextKey::Action));
                    ui.end_row();
                    for p in state.probes.values() {
                        let pkind = v1::ProbeKind::try_from(p.kind)
                            .unwrap_or(v1::ProbeKind::Unspecified);
                        ui.strong(&p.name);
                        ui.label(probe_kind_label(pkind));
                        ui.monospace(if p.snmp_oid.is_empty() {
                            p.port.to_string()
                        } else {
                            p.snmp_oid.clone()
                        });
                        ui.monospace(format!("{}/{}", p.down_threshold, p.up_threshold));
                        ui.horizontal(|ui| {
                            if ui.button("Edit").clicked() {
                                inv.edit_probe = Some(p.clone());
                            }
                            if !p.builtin && ui.button(tr(lang, TextKey::Delete)).clicked() {
                                let _ =
                                    cmd_tx.try_send(ClientCommand::DeleteProbe(v1::IdRequest {
                                        id: p.id,
                                        expected_version: p.version,
                                    }));
                            }
                        });
                        ui.end_row();
                    }
                });
        });
    });

    if let Some(mut p) = inv.edit_probe.clone() {
        let mut open = true;
        let mut do_save = false;
        egui::Window::new(tr(lang, TextKey::PanelProbes))
            .open(&mut open)
            .min_width(380.0)
            .show(ctx, |ui| {
                egui::Grid::new("probe_edit_form")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(tr(lang, TextKey::Name));
                        ui.text_edit_singleline(&mut p.name);
                        ui.end_row();

                        ui.label(tr(lang, TextKey::Type));
                        let pk = v1::ProbeKind::try_from(p.kind)
                            .unwrap_or(v1::ProbeKind::SnmpCustom);
                        egui::ComboBox::from_id_salt("probe_kind_cb")
                            .selected_text(probe_kind_label(pk))
                            .show_ui(ui, |ui| {
                                for k in [
                                    v1::ProbeKind::Icmp,
                                    v1::ProbeKind::Tcp,
                                    v1::ProbeKind::Http,
                                    v1::ProbeKind::Https,
                                    v1::ProbeKind::Dns,
                                    v1::ProbeKind::SnmpOid,
                                    v1::ProbeKind::SnmpCustom,
                                    v1::ProbeKind::Ssh,
                                    v1::ProbeKind::Ntp,
                                ] {
                                    ui.selectable_value(&mut p.kind, k as i32, probe_kind_label(k));
                                }
                            });
                        ui.end_row();

                        ui.label("Port");
                        ui.add(egui::DragValue::new(&mut p.port).range(0..=65535));
                        ui.end_row();

                        ui.label("SNMP OID");
                        ui.text_edit_singleline(&mut p.snmp_oid);
                        ui.end_row();

                        ui.label("Scale");
                        ui.add(egui::DragValue::new(&mut p.snmp_scale).speed(0.1));
                        ui.end_row();

                        ui.label("Unit");
                        ui.text_edit_singleline(&mut p.unit);
                        ui.end_row();

                        ui.label("Down Threshold");
                        ui.add(egui::DragValue::new(&mut p.down_threshold).range(1..=20));
                        ui.end_row();

                        ui.label("Up Threshold");
                        ui.add(egui::DragValue::new(&mut p.up_threshold).range(1..=20));
                        ui.end_row();
                    });
                ui.add_space(6.0);
                if ui
                    .add_enabled(!p.name.trim().is_empty(), egui::Button::new(tr(lang, TextKey::Save)))
                    .clicked()
                {
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
    theme: ThemeMode,
    state: &ClientStateCache,
    inv: &mut InventoryPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    section_heading(ui, theme, tr(lang, TextKey::PanelLinks), None);
    ui.add_space(6.0);
    if state.links.is_empty() {
        empty_state_card(
            ui,
            theme,
            pick_lang(lang, "Belum ada link L1 antar perangkat", "No L1 links configured"),
            pick_lang(
                lang,
                "Gunakan alat 'Tambah Link' di Peta Jaringan atau impor dari Auto-Discovery.",
                "Use 'Add Link' on the Network Map or import links from Auto-Discovery.",
            ),
        );
    } else {
        card_frame(theme).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("links_grid")
                    .striped(true)
                    .spacing([14.0, 7.0])
                    .num_columns(7)
                    .show(ui, |ui| {
                        ui.strong("ID");
                        ui.strong("Src -> Dst");
                        ui.strong("Interface");
                        ui.strong(tr(lang, TextKey::Status));
                        ui.strong("In");
                        ui.strong("Out");
                        ui.strong(tr(lang, TextKey::Action));
                        ui.end_row();
                        for l in state.links.values() {
                            let sname = state
                                .devices
                                .get(&l.src_device_id)
                                .map_or("?", |d| d.name.as_str());
                            let dname = state
                                .devices
                                .get(&l.dst_device_id)
                                .map_or("?", |d| d.name.as_str());
                            let ls = v1::LinkStatus::try_from(l.status)
                                .unwrap_or(v1::LinkStatus::Unknown);
                            ui.monospace(l.id.to_string());
                            ui.strong(format!("{sname} <-> {dname}"));
                            ui.monospace(&l.interface_key);
                            link_status_pill(ui, lang, theme, ls);
                            ui.monospace(format!(
                                "{} ({:.1}%)",
                                format_bps(l.in_bps),
                                l.in_utilization_pct
                            ));
                            ui.monospace(format!(
                                "{} ({:.1}%)",
                                format_bps(l.out_bps),
                                l.out_utilization_pct
                            ));
                            ui.horizontal(|ui| {
                                if ui.button("Edit").clicked() {
                                    inv.edit_link = Some(l.clone());
                                }
                                if ui.button(tr(lang, TextKey::Delete)).clicked() {
                                    let _ = cmd_tx.try_send(ClientCommand::DeleteLink(
                                        v1::IdRequest {
                                            id: l.id,
                                            expected_version: l.version,
                                        },
                                    ));
                                }
                            });
                            ui.end_row();
                        }
                    });
            });
        });
    }

    if let Some(mut link) = inv.edit_link.clone() {
        let mut open = true;
        let mut do_save = false;
        egui::Window::new(tr(lang, TextKey::PanelLinks))
            .open(&mut open)
            .min_width(420.0)
            .show(ctx, |ui| {
                egui::Grid::new("link_edit_form")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Master Device");
                        egui::ComboBox::from_id_salt("link_master_cb")
                            .selected_text(
                                state
                                    .devices
                                    .get(&link.master_device_id)
                                    .map_or("Select", |d| d.name.as_str()),
                            )
                            .show_ui(ui, |ui| {
                                if let Some(s) = state.devices.get(&link.src_device_id) {
                                    ui.selectable_value(&mut link.master_device_id, s.id, &s.name);
                                }
                                if let Some(d) = state.devices.get(&link.dst_device_id) {
                                    ui.selectable_value(&mut link.master_device_id, d.id, &d.name);
                                }
                            });
                        ui.end_row();

                        ui.label("Mastering Type");
                        let mt = v1::MasteringType::try_from(link.mastering_type)
                            .unwrap_or(v1::MasteringType::SnmpIfTraffic);
                        egui::ComboBox::from_id_salt("link_master_type_cb")
                            .selected_text(match mt {
                                v1::MasteringType::ProbeLatency => "Probe Latency",
                                v1::MasteringType::Manual => "Manual",
                                _ => "SNMP Interface Traffic",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut link.mastering_type, v1::MasteringType::SnmpIfTraffic as i32, "SNMP Interface Traffic");
                                ui.selectable_value(&mut link.mastering_type, v1::MasteringType::ProbeLatency as i32, "Probe Latency");
                                ui.selectable_value(&mut link.mastering_type, v1::MasteringType::Manual as i32, "Manual");
                            });
                        ui.end_row();

                        ui.label("Interface Key");
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(&mut link.interface_key);
                            if let Some(ifaces) = state.interfaces_by_device.get(&link.master_device_id) {
                                if !ifaces.is_empty() {
                                    egui::ComboBox::from_id_salt("link_iface_picker")
                                        .selected_text("Pick")
                                        .show_ui(ui, |ui| {
                                            for iface in ifaces {
                                                ui.selectable_value(
                                                    &mut link.interface_key,
                                                    iface.stable_key.clone(),
                                                    format!("{} ({})", iface.name, format_bps(iface.speed_bps as f64)),
                                                );
                                            }
                                        });
                                }
                            }
                        });
                        ui.end_row();

                        ui.label(tr(lang, TextKey::PanelLinkTypes));
                        egui::ComboBox::from_id_salt("link_type_cb")
                            .selected_text(
                                state
                                    .link_types
                                    .get(&link.link_type_id)
                                    .map_or("Select", |lt| lt.name.as_str()),
                            )
                            .show_ui(ui, |ui| {
                                for lt in state.link_types.values() {
                                    ui.selectable_value(&mut link.link_type_id, lt.id, &lt.name);
                                }
                            });
                        ui.end_row();

                        ui.label("Style Override");
                        let curr_style = link
                            .style_override
                            .and_then(|v| v1::LinkStyle::try_from(v).ok());
                        egui::ComboBox::from_id_salt("link_style_ov")
                            .selected_text(match curr_style {
                                Some(v1::LinkStyle::Straight) => tr(lang, TextKey::LinkStyleStraight),
                                Some(v1::LinkStyle::Curved) => tr(lang, TextKey::LinkStyleCurved),
                                Some(v1::LinkStyle::OrthogonalRounded) => {
                                    tr(lang, TextKey::LinkStyleOrthogonal)
                                }
                                _ => "Inherit Link Type",
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut link.style_override, None, "Inherit Link Type");
                                ui.selectable_value(
                                    &mut link.style_override,
                                    Some(v1::LinkStyle::Straight as i32),
                                    tr(lang, TextKey::LinkStyleStraight),
                                );
                                ui.selectable_value(
                                    &mut link.style_override,
                                    Some(v1::LinkStyle::Curved as i32),
                                    tr(lang, TextKey::LinkStyleCurved),
                                );
                                ui.selectable_value(
                                    &mut link.style_override,
                                    Some(v1::LinkStyle::OrthogonalRounded as i32),
                                    tr(lang, TextKey::LinkStyleOrthogonal),
                                );
                            });
                        ui.end_row();

                        let mut speed_mbps = link.max_speed_bps_override.unwrap_or(0) / 1_000_000;
                        ui.label("Speed Override (Mbps, 0=auto)");
                        ui.add(egui::DragValue::new(&mut speed_mbps).range(0..=1_000_000));
                        link.max_speed_bps_override = if speed_mbps == 0 {
                            None
                        } else {
                            Some(speed_mbps * 1_000_000)
                        };
                        ui.end_row();

                        ui.label("Label");
                        ui.text_edit_singleline(&mut link.label);
                        ui.end_row();
                    });
                ui.add_space(4.0);
                ui.checkbox(&mut link.history_enabled, "Record Traffic History");
                ui.add_space(6.0);
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
    theme: ThemeMode,
    state: &ClientStateCache,
    inv: &mut InventoryPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.horizontal(|ui| {
        section_heading(ui, theme, tr(lang, TextKey::PanelLinkTypes), None);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
    });
    ui.add_space(6.0);
    card_frame(theme).show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("ltypes_grid")
                .striped(true)
                .spacing([14.0, 7.0])
                .num_columns(5)
                .show(ui, |ui| {
                    ui.strong(tr(lang, TextKey::Name));
                    ui.strong("Max Speed");
                    ui.strong("Thickness");
                    ui.strong("Style");
                    ui.strong(tr(lang, TextKey::Action));
                    ui.end_row();
                    for lt in state.link_types.values() {
                        let ls = v1::LinkStyle::try_from(lt.style).unwrap_or(v1::LinkStyle::Curved);
                        let style_lbl = match ls {
                            v1::LinkStyle::Straight => tr(lang, TextKey::LinkStyleStraight),
                            v1::LinkStyle::OrthogonalRounded => {
                                tr(lang, TextKey::LinkStyleOrthogonal)
                            }
                            _ => tr(lang, TextKey::LinkStyleCurved),
                        };
                        ui.strong(&lt.name);
                        ui.monospace(format_bps(lt.max_speed_bps as f64));
                        ui.monospace(format!("{:.1}", lt.thickness));
                        ui.label(style_lbl);
                        ui.horizontal(|ui| {
                            if ui.button("Edit").clicked() {
                                inv.edit_link_type = Some(lt.clone());
                            }
                            if !lt.builtin && ui.button(tr(lang, TextKey::Delete)).clicked() {
                                let _ = cmd_tx.try_send(ClientCommand::DeleteLinkType(
                                    v1::IdRequest {
                                        id: lt.id,
                                        expected_version: lt.version,
                                    },
                                ));
                            }
                        });
                        ui.end_row();
                    }
                });
        });
    });

    if let Some(mut lt) = inv.edit_link_type.clone() {
        let mut open = true;
        let mut do_save = false;
        egui::Window::new(tr(lang, TextKey::PanelLinkTypes))
            .open(&mut open)
            .min_width(340.0)
            .show(ctx, |ui| {
                egui::Grid::new("ltype_form")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(tr(lang, TextKey::Name));
                        ui.text_edit_singleline(&mut lt.name);
                        ui.end_row();
                        ui.label("Max Speed (bps)");
                        ui.add(egui::DragValue::new(&mut lt.max_speed_bps).speed(1_000_000.0));
                        ui.end_row();
                        ui.label("Thickness");
                        ui.add(egui::DragValue::new(&mut lt.thickness).range(0.5..=12.0));
                        ui.end_row();
                        ui.label("Color Hex");
                        ui.text_edit_singleline(&mut lt.color_hex);
                        ui.end_row();
                    });
                ui.add_space(6.0);
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
    theme: ThemeMode,
    state: &ClientStateCache,
    inv: &mut InventoryPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.horizontal(|ui| {
        section_heading(ui, theme, tr(lang, TextKey::PanelDeviceTypes), None);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
    });
    ui.add_space(6.0);
    card_frame(theme).show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("dtypes_grid")
                .striped(true)
                .spacing([14.0, 7.0])
                .num_columns(4)
                .show(ui, |ui| {
                    ui.strong(tr(lang, TextKey::Name));
                    ui.strong("Icon Key");
                    ui.strong("Launcher");
                    ui.strong(tr(lang, TextKey::Action));
                    ui.end_row();
                    for dt in state.device_types.values() {
                        ui.strong(&dt.name);
                        ui.monospace(&dt.default_icon_key);
                        ui.monospace(&dt.launcher_command);
                        ui.horizontal(|ui| {
                            if ui.button("Edit").clicked() {
                                inv.edit_device_type = Some(dt.clone());
                            }
                            if ui.button(tr(lang, TextKey::Delete)).clicked() {
                                let _ = cmd_tx.try_send(ClientCommand::DeleteDeviceType(
                                    v1::IdRequest {
                                        id: dt.id,
                                        expected_version: dt.version,
                                    },
                                ));
                            }
                        });
                        ui.end_row();
                    }
                });
        });
    });

    if let Some(mut dt) = inv.edit_device_type.clone() {
        let mut open = true;
        let mut do_save = false;
        egui::Window::new(tr(lang, TextKey::PanelDeviceTypes))
            .open(&mut open)
            .min_width(340.0)
            .show(ctx, |ui| {
                egui::Grid::new("dtype_form")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(tr(lang, TextKey::Name));
                        ui.text_edit_singleline(&mut dt.name);
                        ui.end_row();
                        ui.label("Icon Key");
                        ui.text_edit_singleline(&mut dt.default_icon_key);
                        ui.end_row();
                        ui.label("Launcher");
                        ui.text_edit_singleline(&mut dt.launcher_command);
                        ui.end_row();
                    });
                ui.add_space(6.0);
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
    theme: ThemeMode,
    state: &ClientStateCache,
    is_submaps: bool,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    if is_submaps {
        section_heading(ui, theme, tr(lang, TextKey::PanelSubmaps), None);
        ui.add_space(6.0);
        if state.submaps.is_empty() {
            empty_state_card(
                ui,
                theme,
                pick_lang(lang, "Belum ada sub-peta", "No submaps placed"),
                pick_lang(
                    lang,
                    "Gunakan alat 'Tambah Sub-Peta' di Peta Jaringan untuk menautkan peta.",
                    "Use 'Add Submap' on the Network Map to link between maps.",
                ),
            );
            return;
        }
        card_frame(theme).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("submaps_grid")
                    .striped(true)
                    .spacing([14.0, 7.0])
                    .num_columns(4)
                    .show(ui, |ui| {
                        ui.strong("Map");
                        ui.strong("Target Map");
                        ui.strong("Label");
                        ui.strong(tr(lang, TextKey::Action));
                        ui.end_row();
                        for s in state.submaps.values() {
                            ui.label(state.maps.get(&s.map_id).map_or("-", |m| m.name.as_str()));
                            ui.label(
                                state
                                    .maps
                                    .get(&s.target_map_id)
                                    .map_or("-", |m| m.name.as_str()),
                            );
                            ui.strong(&s.label);
                            if ui.button(tr(lang, TextKey::Delete)).clicked() {
                                let _ = cmd_tx.try_send(ClientCommand::DeleteSubmap(
                                    v1::IdRequest {
                                        id: s.id,
                                        expected_version: s.version,
                                    },
                                ));
                            }
                            ui.end_row();
                        }
                    });
            });
        });
    } else {
        section_heading(ui, theme, tr(lang, TextKey::PanelNetworks), None);
        ui.add_space(6.0);
        if state.networks.is_empty() {
            empty_state_card(
                ui,
                theme,
                pick_lang(lang, "Belum ada segmen jaringan", "No network segments"),
                pick_lang(
                    lang,
                    "Gunakan alat 'Tambah Segmen' di Peta Jaringan.",
                    "Use 'Add Network' on the Network Map.",
                ),
            );
            return;
        }
        card_frame(theme).show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("networks_grid")
                    .striped(true)
                    .spacing([14.0, 7.0])
                    .num_columns(4)
                    .show(ui, |ui| {
                        ui.strong(tr(lang, TextKey::Name));
                        ui.strong("CIDR");
                        ui.strong("Map");
                        ui.strong(tr(lang, TextKey::Action));
                        ui.end_row();
                        for n in state.networks.values() {
                            ui.strong(&n.name);
                            ui.monospace(&n.cidr);
                            ui.label(state.maps.get(&n.map_id).map_or("-", |m| m.name.as_str()));
                            if ui.button(tr(lang, TextKey::Delete)).clicked() {
                                let _ = cmd_tx.try_send(ClientCommand::DeleteNetwork(
                                    v1::IdRequest {
                                        id: n.id,
                                        expected_version: n.version,
                                    },
                                ));
                            }
                            ui.end_row();
                        }
                    });
            });
        });
    }
}

pub fn render_outages_panel(
    ui: &mut egui::Ui,
    lang: Language,
    theme: ThemeMode,
    state: &ClientStateCache,
) {
    section_heading(ui, theme, tr(lang, TextKey::PanelOutages), None);
    ui.add_space(6.0);
    if state.outages.is_empty() {
        empty_state_card(
            ui,
            theme,
            pick_lang(lang, "Tidak ada riwayat gangguan", "No recorded outages"),
            pick_lang(
                lang,
                "Semua layanan yang dipantau berjalan stabil.",
                "All monitored services are healthy.",
            ),
        );
        return;
    }

    card_frame(theme).show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("outages_grid")
                .striped(true)
                .spacing([14.0, 7.0])
                .num_columns(5)
                .show(ui, |ui| {
                    ui.strong("Device");
                    ui.strong("Service");
                    ui.strong("Started (Local)");
                    ui.strong("Ended (Local)");
                    ui.strong("Suppressed");
                    ui.end_row();
                    for o in &state.outages {
                        ui.strong(&o.device_name);
                        ui.label(&o.service_name);
                        ui.monospace(fmt_local_ts(o.started_utc));
                        if let Some(ended) = o.ended_utc {
                            ui.monospace(fmt_local_ts(ended));
                        } else {
                            probe_state_pill(ui, lang, theme, v1::ProbeState::Down);
                        }
                        ui.label(if o.suppressed_by_dependency {
                            tr(lang, TextKey::Yes)
                        } else {
                            tr(lang, TextKey::No)
                        });
                        ui.end_row();
                    }
                });
        });
    });
}

pub fn fmt_local_ts(utc_sec: i64) -> String {
    chrono::DateTime::from_timestamp(utc_sec, 0)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| utc_sec.to_string())
}
