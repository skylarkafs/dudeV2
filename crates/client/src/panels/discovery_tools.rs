use crate::ui_kit::{
    accent_rgb, card_frame, empty_state_card, pick_lang, primary_button, section_heading,
    status_pill, table_card, toolbar_frame, warning_rgb,
};
use eframe::egui;
use nextdude_client_core::connection::ClientCommand;
use nextdude_client_core::i18n::{tr, Language, TextKey};
use nextdude_client_core::state::ClientStateCache;
use nextdude_client_core::theme::ThemeMode;
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
            import_links: false,
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
    theme: ThemeMode,
    state: &ClientStateCache,
    dt: &mut DiscoveryToolsState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    section_heading(
        ui,
        theme,
        tr(lang, TextKey::PanelDiscovery),
        Some(pick_lang(
            lang,
            "Pemindaian CIDR ICMP/SNMP serta tetangga LLDP/CDP/MNDP",
            "ICMP/SNMP CIDR sweep with LLDP, CDP, and MNDP neighbor detection",
        )),
    );
    ui.add_space(6.0);

    card_frame(theme).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label("CIDR(s)");
            ui.add(
                egui::TextEdit::singleline(&mut dt.cidrs_text)
                    .hint_text("192.168.88.0/24, 10.0.0.0/24"),
            );
            ui.label("Concurrency");
            ui.add(egui::DragValue::new(&mut dt.concurrency).range(1..=128));
            ui.label("Rate/s");
            ui.add(egui::DragValue::new(&mut dt.rate_limit).range(1..=500));
        });
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("SNMP Community (v2c)");
            ui.add(egui::TextEdit::singleline(&mut dt.snmp_community).password(true));
            ui.checkbox(&mut dt.include_mndp, "MNDP (UDP 5678)");
            ui.checkbox(&mut dt.include_lldp_cdp, "LLDP / CDP");
            let can_scan = !dt.cidrs_text.trim().is_empty() || dt.include_mndp;
            if primary_button(ui, theme, can_scan, tr(lang, TextKey::StartDiscovery)).clicked() {
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
    });
    ui.add_space(6.0);

    if let Some(ref prog) = state.discovery_progress {
        let frac = if prog.total_hosts > 0 {
            (prog.scanned_hosts as f32) / (prog.total_hosts as f32)
        } else {
            1.0
        };
        ui.add(egui::ProgressBar::new(frac).text(&prog.status_text));
        ui.add_space(6.0);

        toolbar_frame(theme).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
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
                ui.checkbox(
                    &mut dt.import_links,
                    pick_lang(lang, "Impor Link (LLDP/CDP)", "Import Links (LLDP/CDP)"),
                );
                ui.checkbox(&mut dt.auto_layout, "Auto Layout");
                if ui.button("Select New (Non-Dup)").clicked() {
                    dt.selected_candidates.clear();
                    for c in &prog.candidates {
                        if !c.duplicate_existing {
                            dt.selected_candidates.insert(c.temp_id.clone());
                        }
                    }
                }
                let can_import = !dt.selected_candidates.is_empty() && dt.target_map_id.is_some();
                if primary_button(ui, theme, can_import, tr(lang, TextKey::ImportSelected))
                    .clicked()
                {
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
        });
        ui.add_space(6.0);

        table_card(ui, theme, "disc_cands_scroll", |ui| {
            egui::Grid::new("disc_cands")
                .spacing([16.0, 10.0])
                .num_columns(6)
                .show(ui, |ui| {
                    ui.strong("Sel");
                    ui.strong("IP");
                    ui.strong("sysName");
                    ui.strong(tr(lang, TextKey::Vendor));
                    ui.strong(tr(lang, TextKey::Model));
                    ui.strong("Status");
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
                        ui.monospace(&c.ip);
                        ui.strong(&c.sys_name);
                        ui.label(&c.vendor);
                        ui.label(&c.model);
                        if c.duplicate_existing {
                            status_pill(ui, "Existing", warning_rgb(theme));
                        } else {
                            status_pill(ui, "New", accent_rgb(theme));
                        }
                        ui.end_row();
                    }
                });
        });
    } else {
        empty_state_card(
            ui,
            theme,
            pick_lang(lang, "Siap memindai jaringan", "Ready to scan network"),
            pick_lang(
                lang,
                "Masukkan satu atau beberapa subnet CIDR lalu klik Mulai Pemindaian.",
                "Enter one or more CIDR ranges above and start a scan. Discovered devices are reviewed before import.",
            ),
        );
    }
}

pub fn render_tools_panel(
    ui: &mut egui::Ui,
    lang: Language,
    theme: ThemeMode,
    state: &ClientStateCache,
    dt: &mut DiscoveryToolsState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    section_heading(
        ui,
        theme,
        tr(lang, TextKey::PanelTools),
        Some(pick_lang(
            lang,
            "Diagnostik jaringan langsung dijalankan dari server Linux",
            "Server-side diagnostic tools streamed live",
        )),
    );
    ui.add_space(6.0);

    toolbar_frame(theme).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            let tool_name = match dt.tool_kind {
                v1::ToolKind::Ping => "Ping",
                v1::ToolKind::Traceroute => "Traceroute",
                v1::ToolKind::SnmpGet => "SNMP Get",
                v1::ToolKind::SnmpWalk => "SNMP Walk",
                v1::ToolKind::TcpCheck => "TCP Port Check",
                v1::ToolKind::DnsLookup => "DNS Lookup",
                _ => "Ping",
            };
            egui::ComboBox::from_label("Tool")
                .selected_text(tool_name)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut dt.tool_kind, v1::ToolKind::Ping, "Ping");
                    ui.selectable_value(&mut dt.tool_kind, v1::ToolKind::Traceroute, "Traceroute");
                    ui.selectable_value(&mut dt.tool_kind, v1::ToolKind::SnmpGet, "SNMP Get");
                    ui.selectable_value(&mut dt.tool_kind, v1::ToolKind::SnmpWalk, "SNMP Walk");
                    ui.selectable_value(&mut dt.tool_kind, v1::ToolKind::TcpCheck, "TCP Port Check");
                    ui.selectable_value(&mut dt.tool_kind, v1::ToolKind::DnsLookup, "DNS Lookup");
                });
            egui::ComboBox::from_label("Device")
                .selected_text(
                    dt.tool_device_id
                        .and_then(|id| state.devices.get(&id))
                        .map_or("Custom Target", |d| d.name.as_str()),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut dt.tool_device_id, None, "Custom Target");
                    for d in state.devices.values() {
                        if ui
                            .selectable_value(&mut dt.tool_device_id, Some(d.id), &d.name)
                            .clicked()
                        {
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
            let can_run = !dt.tool_target.trim().is_empty();
            if primary_button(ui, theme, can_run, tr(lang, TextKey::RunTool)).clicked() {
                dt.tool_output.clear();
                let _ = cmd_tx.try_send(ClientCommand::RunTool(v1::RunToolRequest {
                    kind: dt.tool_kind as i32,
                    target: dt.tool_target.trim().to_string(),
                    port: dt.tool_port,
                    oid: dt.tool_oid.clone(),
                    device_id: dt.tool_device_id,
                    count: 5,
                }));
            }
            if !dt.tool_output.is_empty()
                && ui
                    .button(pick_lang(lang, "Bersihkan", "Clear"))
                    .clicked()
            {
                dt.tool_output.clear();
            }
        });
    });
    ui.add_space(6.0);

    card_frame(theme).show(ui, |ui| {
        if dt.tool_output.is_empty() {
            ui.label(pick_lang(
                lang,
                "Pilih tool dan masukkan IP/hostname target lalu klik Jalankan.",
                "Pick a diagnostic tool, enter target IP/hostname, and click Run.",
            ));
        } else {
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &dt.tool_output {
                        ui.monospace(line);
                    }
                });
        }
    });
}
