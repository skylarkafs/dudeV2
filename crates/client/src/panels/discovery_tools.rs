use eframe::egui;
use nextdude_client_core::connection::ClientCommand;
use nextdude_client_core::i18n::{tr, Language, TextKey};
use nextdude_client_core::state::ClientStateCache;
use nextdude_common::proto::v1;
use std::collections::HashSet;
use tokio::sync::mpsc;

pub struct DiscoveryToolsState {
    pub cidrs_text: String,
    pub concurrency: u32,
    pub rate_limit: u32,
    pub snmp_community: String,
    pub include_mndp: bool,
    pub include_lldp_cdp: bool,
    pub selected_candidates: HashSet<String>,
    pub target_map_id: Option<i64>,
    pub import_links: bool,
    pub auto_layout: bool,
    pub tool_kind: v1::ToolKind,
    pub tool_target: String,
    pub tool_port: u32,
    pub tool_oid: String,
    pub tool_device_id: Option<i64>,
    pub tool_output: Vec<String>,
}

impl Default for DiscoveryToolsState {
    fn default() -> Self {
        Self {
            cidrs_text: String::new(),
            concurrency: 32,
            rate_limit: 100,
            snmp_community: String::new(),
            include_mndp: true,
            include_lldp_cdp: true,
            selected_candidates: HashSet::new(),
            target_map_id: None,
            import_links: true,
            auto_layout: true,
            tool_kind: v1::ToolKind::Ping,
            tool_target: String::new(),
            tool_port: 80,
            tool_oid: "1.3.6.1.2.1.1.1.0".to_string(),
            tool_device_id: None,
            tool_output: Vec::new(),
        }
    }
}

pub fn render_discovery_panel(
    ui: &mut egui::Ui,
    lang: Language,
    state: &ClientStateCache,
    dt: &mut DiscoveryToolsState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.heading(tr(lang, TextKey::PanelDiscovery));
    ui.horizontal_wrapped(|ui| {
        ui.label("CIDR(s) comma-separated");
        ui.text_edit_singleline(&mut dt.cidrs_text);
        ui.label("Concurrency");
        ui.add(egui::DragValue::new(&mut dt.concurrency).range(1..=128));
        ui.label("Rate/s");
        ui.add(egui::DragValue::new(&mut dt.rate_limit).range(1..=500));
    });
    ui.horizontal_wrapped(|ui| {
        ui.label("SNMP Community (v2c)");
        ui.add(egui::TextEdit::singleline(&mut dt.snmp_community).password(true));
        ui.checkbox(&mut dt.include_mndp, "MNDP (UDP 5678)");
        ui.checkbox(&mut dt.include_lldp_cdp, "LLDP / CDP");
        if ui.button(tr(lang, TextKey::StartDiscovery)).clicked() {
            let cidrs = dt
                .cidrs_text
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>();
            let creds = if dt.snmp_community.is_empty() {
                Vec::new()
            } else {
                vec![v1::SnmpCredential {
                    version: v1::SnmpVersion::V2c as i32,
                    port: 161,
                    timeout_ms: 1500,
                    retries: 0,
                    community: Some(dt.snmp_community.clone()),
                    ..Default::default()
                }]
            };
            let _ = cmd_tx.try_send(ClientCommand::StartDiscovery(v1::StartDiscoveryRequest {
                cidrs,
                concurrency: dt.concurrency,
                rate_limit_per_sec: dt.rate_limit,
                credentials: creds,
                include_mndp: dt.include_mndp,
                include_lldp_cdp: dt.include_lldp_cdp,
            }));
        }
        if let Some(ref prog) = state.discovery_progress {
            if !prog.finished && ui.button(tr(lang, TextKey::CancelDiscovery)).clicked() {
                let _ = cmd_tx.try_send(ClientCommand::CancelDiscovery(prog.job_id.clone()));
            }
        }
    });

    if let Some(ref prog) = state.discovery_progress {
        let frac = if prog.total_hosts > 0 {
            (prog.scanned_hosts as f32) / (prog.total_hosts as f32)
        } else {
            1.0
        };
        ui.add(egui::ProgressBar::new(frac).text(&prog.status_text));
        ui.separator();
        ui.horizontal(|ui| {
            let default_map = dt.target_map_id.or_else(|| state.maps.keys().next().copied());
            dt.target_map_id = default_map;
            egui::ComboBox::from_label("Target Map")
                .selected_text(
                    dt.target_map_id
                        .and_then(|id| state.maps.get(&id))
                        .map_or("Select Map", |m| m.name.as_str()),
                )
                .show_ui(ui, |ui| {
                    for m in state.maps.values() {
                        ui.selectable_value(&mut dt.target_map_id, Some(m.id), &m.name);
                    }
                });
            ui.checkbox(&mut dt.import_links, "Import Links");
            ui.checkbox(&mut dt.auto_layout, "Auto Layout");
            if ui.button("Select New (Non-Dup)").clicked() {
                dt.selected_candidates.clear();
                for c in &prog.candidates {
                    if !c.duplicate_existing {
                        dt.selected_candidates.insert(c.temp_id.clone());
                    }
                }
            }
            if ui.button(tr(lang, TextKey::ImportSelected)).clicked() {
                if let Some(mid) = dt.target_map_id {
                    let _ = cmd_tx.try_send(ClientCommand::ImportDiscoveryCandidates(
                        v1::ImportDiscoveryRequest {
                            job_id: prog.job_id.clone(),
                            target_map_id: mid,
                            selected_temp_ids: dt.selected_candidates.iter().cloned().collect(),
                            import_links: dt.import_links,
                            auto_layout: dt.auto_layout,
                        },
                    ));
                }
            }
        });

        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("disc_cands").striped(true).num_columns(6).show(ui, |ui| {
                ui.strong("Sel");
                ui.strong("IP");
                ui.strong("sysName");
                ui.strong(tr(lang, TextKey::Vendor));
                ui.strong(tr(lang, TextKey::Model));
                ui.strong("Duplicate?");
                ui.end_row();
                for c in &prog.candidates {
                    let mut checked = dt.selected_candidates.contains(&c.temp_id);
                    if ui.checkbox(&mut checked, "").changed() {
                        if checked {
                            dt.selected_candidates.insert(c.temp_id.clone());
                        } else {
                            dt.selected_candidates.remove(&c.temp_id);
                        }
                    }
                    ui.label(&c.ip);
                    ui.label(&c.sys_name);
                    ui.label(&c.vendor);
                    ui.label(&c.model);
                    ui.label(if c.duplicate_existing { "Existing" } else { "New" });
                    ui.end_row();
                }
            });
        });
    }
}

pub fn render_tools_panel(
    ui: &mut egui::Ui,
    lang: Language,
    state: &ClientStateCache,
    dt: &mut DiscoveryToolsState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    ui.heading(tr(lang, TextKey::PanelTools));
    ui.horizontal_wrapped(|ui| {
        egui::ComboBox::from_label("Tool")
            .selected_text(format!("{:?}", dt.tool_kind))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut dt.tool_kind, v1::ToolKind::Ping, "Ping");
                ui.selectable_value(&mut dt.tool_kind, v1::ToolKind::Traceroute, "Traceroute");
                ui.selectable_value(&mut dt.tool_kind, v1::ToolKind::SnmpGet, "SNMP Get");
                ui.selectable_value(&mut dt.tool_kind, v1::ToolKind::SnmpWalk, "SNMP Walk");
                ui.selectable_value(&mut dt.tool_kind, v1::ToolKind::TcpCheck, "TCP Port Check");
                ui.selectable_value(&mut dt.tool_kind, v1::ToolKind::DnsLookup, "DNS Lookup");
            });
        egui::ComboBox::from_label("Device (optional)")
            .selected_text(
                dt.tool_device_id
                    .and_then(|id| state.devices.get(&id))
                    .map_or("Custom Target", |d| d.name.as_str()),
            )
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut dt.tool_device_id, None, "Custom Target");
                for d in state.devices.values() {
                    if ui.selectable_value(&mut dt.tool_device_id, Some(d.id), &d.name).clicked() {
                        dt.tool_target = d.address.clone();
                    }
                }
            });
        ui.label("Target");
        ui.text_edit_singleline(&mut dt.tool_target);
        if dt.tool_kind == v1::ToolKind::TcpCheck {
            ui.label("Port");
            ui.add(egui::DragValue::new(&mut dt.tool_port).range(1..=65535));
        }
        if matches!(dt.tool_kind, v1::ToolKind::SnmpGet | v1::ToolKind::SnmpWalk) {
            ui.label("OID");
            ui.text_edit_singleline(&mut dt.tool_oid);
        }
        if ui.button(tr(lang, TextKey::RunTool)).clicked() {
            dt.tool_output.clear();
            let _ = cmd_tx.try_send(ClientCommand::RunTool(v1::RunToolRequest {
                kind: dt.tool_kind as i32,
                target: dt.tool_target.clone(),
                port: dt.tool_port,
                oid: dt.tool_oid.clone(),
                device_id: dt.tool_device_id,
                count: 5,
            }));
        }
    });
    ui.separator();
    egui::ScrollArea::vertical().stick_to_bottom(true).show(ui, |ui| {
        for line in &dt.tool_output {
            ui.monospace(line);
        }
    });
}
