use crate::panels::inventory::fmt_local_ts;
use crate::ui_kit::{
    card_frame, empty_state_card, muted_color, pick_lang, section_heading, severity_pill,
    toolbar_frame,
};
use eframe::egui;
use egui_plot::{Legend, Line, Plot, PlotPoints};
use nextdude_client_core::connection::ClientCommand;
use nextdude_client_core::i18n::{tr, Language, TextKey};
use nextdude_client_core::state::ClientStateCache;
use nextdude_client_core::theme::ThemeMode;
use nextdude_common::proto::v1;
use regex::Regex;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartRangePreset {
    Hour1,
    Hour6,
    Hour24,
    Day7,
    Day30,
    CustomHours,
}

pub struct ObservabilityPanelState {
    pub event_filter: String,
    pub syslog_text_filter: String,
    pub syslog_use_regex: bool,
    pub syslog_device_filter: Option<i64>,
    pub syslog_max_severity: u32,
    pub syslog_facility_filter: Option<u32>,
    pub selected_chart_id: Option<i64>,
    pub chart_range: ChartRangePreset,
    pub custom_hours: u32,
    pub show_min_max: bool,
    pub loaded_chart: Option<v1::QueryChartSeriesResponse>,
    pub new_chart_name: String,
    pub new_chart_service_id: Option<i64>,
    pub new_chart_link_id: Option<i64>,
    pub new_chart_metric: String,
    pub show_new_chart_modal: bool,
}

impl Default for ObservabilityPanelState {
    fn default() -> Self {
        Self {
            event_filter: String::new(),
            syslog_text_filter: String::new(),
            syslog_use_regex: false,
            syslog_device_filter: None,
            syslog_max_severity: 7,
            syslog_facility_filter: None,
            selected_chart_id: None,
            chart_range: ChartRangePreset::Hour1,
            custom_hours: 12,
            show_min_max: true,
            loaded_chart: None,
            new_chart_name: String::new(),
            new_chart_service_id: None,
            new_chart_link_id: None,
            new_chart_metric: "value".to_string(),
            show_new_chart_modal: false,
        }
    }
}

pub fn render_events_panel(
    ui: &mut egui::Ui,
    lang: Language,
    theme: ThemeMode,
    state: &ClientStateCache,
    obs: &mut ObservabilityPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    section_heading(ui, theme, tr(lang, TextKey::PanelEvents), None);
    ui.add_space(6.0);

    toolbar_frame(theme).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(tr(lang, TextKey::Filter));
            ui.text_edit_singleline(&mut obs.event_filter);
            if !obs.event_filter.is_empty() && ui.small_button("x").clicked() {
                obs.event_filter.clear();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(tr(lang, TextKey::ExportCsv)).clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_file_name("nextdude_events.csv")
                        .add_filter("CSV", &["csv"])
                        .save_file()
                    {
                        if let Ok(mut wtr) = csv::Writer::from_path(path) {
                            let _ = wtr.write_record([
                                "timestamp",
                                "severity",
                                "category",
                                "device",
                                "message",
                                "acknowledged",
                            ]);
                            for e in &state.events {
                                let _ = wtr.write_record([
                                    fmt_local_ts(e.timestamp_utc),
                                    e.severity.to_string(),
                                    e.category.clone(),
                                    e.device_name.clone(),
                                    e.message.clone(),
                                    e.acknowledged.to_string(),
                                ]);
                            }
                            let _ = wtr.flush();
                        }
                    }
                }
            });
        });
    });
    ui.add_space(6.0);

    let f = obs.event_filter.to_ascii_lowercase();
    let rows: Vec<&v1::EventLogEntry> = state
        .events
        .iter()
        .filter(|e| {
            f.is_empty()
                || e.message.to_ascii_lowercase().contains(&f)
                || e.device_name.to_ascii_lowercase().contains(&f)
                || e.category.to_ascii_lowercase().contains(&f)
        })
        .collect();

    if rows.is_empty() {
        empty_state_card(
            ui,
            theme,
            pick_lang(lang, "Belum ada log event", "No events recorded"),
            pick_lang(
                lang,
                "Peristiwa perubahan state, alert, dan restart engine tampil di sini.",
                "State transitions, alert dispatches, and server lifecycle events appear here.",
            ),
        );
        return;
    }

    card_frame(theme).show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("events_grid")
                .striped(true)
                .spacing([12.0, 7.0])
                .num_columns(6)
                .show(ui, |ui| {
                    ui.strong(tr(lang, TextKey::Timestamp));
                    ui.strong(tr(lang, TextKey::Severity));
                    ui.strong("Device");
                    ui.strong("Category");
                    ui.strong(tr(lang, TextKey::Message));
                    ui.strong(tr(lang, TextKey::Action));
                    ui.end_row();
                    for e in rows {
                        let sev = v1::Severity::try_from(e.severity).unwrap_or(v1::Severity::Info);
                        ui.monospace(fmt_local_ts(e.timestamp_utc));
                        severity_pill(ui, lang, theme, sev);
                        ui.strong(&e.device_name);
                        ui.monospace(&e.category);
                        ui.label(&e.message);
                        if e.acknowledged {
                            ui.label(
                                egui::RichText::new(format!(
                                    "Ack: {}",
                                    e.ack_by.as_deref().unwrap_or("")
                                ))
                                .small()
                                .color(muted_color(theme)),
                            );
                        } else if ui.button(tr(lang, TextKey::Acknowledge)).clicked() {
                            let _ = cmd_tx.try_send(ClientCommand::AcknowledgeAlert(e.id));
                        }
                        ui.end_row();
                    }
                });
        });
    });
}

pub fn render_syslog_panel(
    ui: &mut egui::Ui,
    lang: Language,
    theme: ThemeMode,
    state: &ClientStateCache,
    obs: &mut ObservabilityPanelState,
) {
    ui.horizontal(|ui| {
        section_heading(ui, theme, tr(lang, TextKey::PanelSyslog), None);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.monospace(format!(
                "RX: {} | Dropped: {}",
                state.syslog_stats.received_total, state.syslog_stats.dropped_total
            ));
        });
    });
    ui.add_space(6.0);

    toolbar_frame(theme).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(tr(lang, TextKey::Filter));
            ui.text_edit_singleline(&mut obs.syslog_text_filter);
            ui.checkbox(&mut obs.syslog_use_regex, "Regex");
            ui.label("Max Sev (0..7)");
            ui.add(egui::DragValue::new(&mut obs.syslog_max_severity).range(0..=7));
            egui::ComboBox::from_label("Device")
                .selected_text(
                    obs.syslog_device_filter
                        .and_then(|id| state.devices.get(&id))
                        .map_or("All", |d| d.name.as_str()),
                )
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut obs.syslog_device_filter, None, "All");
                    for d in state.devices.values() {
                        ui.selectable_value(&mut obs.syslog_device_filter, Some(d.id), &d.name);
                    }
                });
        });
    });
    ui.add_space(6.0);

    let compiled_regex = if obs.syslog_use_regex && !obs.syslog_text_filter.is_empty() {
        Regex::new(&obs.syslog_text_filter).ok()
    } else {
        None
    };
    let lower_filter = obs.syslog_text_filter.to_ascii_lowercase();

    let rows: Vec<&v1::SyslogMessage> = state
        .syslogs
        .iter()
        .filter(|s| {
            if let Some(did) = obs.syslog_device_filter {
                if s.device_id != Some(did) {
                    return false;
                }
            }
            if s.severity > obs.syslog_max_severity {
                return false;
            }
            if let Some(fac) = obs.syslog_facility_filter {
                if s.facility != fac {
                    return false;
                }
            }
            if !obs.syslog_text_filter.is_empty() {
                if let Some(ref re) = compiled_regex {
                    return re.is_match(&s.message) || re.is_match(&s.app_name);
                }
                return s.message.to_ascii_lowercase().contains(&lower_filter)
                    || s.app_name.to_ascii_lowercase().contains(&lower_filter);
            }
            true
        })
        .collect();

    if rows.is_empty() {
        empty_state_card(
            ui,
            theme,
            pick_lang(lang, "Belum ada pesan syslog", "No matching syslog messages"),
            pick_lang(
                lang,
                "Arahkan syslog UDP 514 perangkat Anda ke alamat IP server NextDude.",
                "Point device syslog UDP 514 output to the NextDude server IP.",
            ),
        );
        return;
    }

    card_frame(theme).show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("syslog_grid")
                .striped(true)
                .spacing([12.0, 6.0])
                .num_columns(6)
                .show(ui, |ui| {
                    ui.strong(tr(lang, TextKey::Timestamp));
                    ui.strong("Source IP");
                    ui.strong("Device");
                    ui.strong("Fac/Sev");
                    ui.strong("App/Tag");
                    ui.strong(tr(lang, TextKey::Message));
                    ui.end_row();
                    for s in rows {
                        ui.monospace(fmt_local_ts(s.timestamp_utc));
                        ui.monospace(&s.source_ip);
                        ui.strong(&s.device_name);
                        ui.monospace(format!("{}/{}", s.facility, s.severity));
                        ui.monospace(&s.app_name);
                        ui.label(&s.message);
                        ui.end_row();
                    }
                });
        });
    });
}

pub fn render_traps_panel(
    ui: &mut egui::Ui,
    lang: Language,
    theme: ThemeMode,
    state: &ClientStateCache,
) {
    section_heading(ui, theme, tr(lang, TextKey::PanelTraps), None);
    ui.add_space(6.0);
    if state.traps.is_empty() {
        empty_state_card(
            ui,
            theme,
            pick_lang(lang, "Belum ada SNMP Trap diterima", "No SNMP traps received"),
            pick_lang(
                lang,
                "Server mendengarkan SNMPv1/v2c/v3 Trap dan Inform pada UDP port 162.",
                "Server listens for SNMPv1/v2c/v3 Trap and Inform packets on UDP 162.",
            ),
        );
        return;
    }

    card_frame(theme).show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("traps_grid")
                .striped(true)
                .spacing([12.0, 6.0])
                .num_columns(6)
                .show(ui, |ui| {
                    ui.strong(tr(lang, TextKey::Timestamp));
                    ui.strong("Source IP");
                    ui.strong("Device");
                    ui.strong("Version");
                    ui.strong("Trap OID / Name");
                    ui.strong("Varbinds");
                    ui.end_row();
                    for t in &state.traps {
                        let ver = v1::SnmpVersion::try_from(t.snmp_version)
                            .unwrap_or(v1::SnmpVersion::V2c);
                        let ver_str = match ver {
                            v1::SnmpVersion::V1 => "v1",
                            v1::SnmpVersion::V3 => "v3",
                            _ => "v2c",
                        };
                        ui.monospace(fmt_local_ts(t.timestamp_utc));
                        ui.monospace(&t.source_ip);
                        ui.strong(&t.device_name);
                        ui.monospace(ver_str);
                        ui.label(format!("{} ({})", t.trap_name, t.trap_oid));
                        let summary = t
                            .varbinds
                            .iter()
                            .map(|vb| format!("{}={}", vb.oid_name, vb.value))
                            .collect::<Vec<_>>()
                            .join("; ");
                        ui.label(summary);
                        ui.end_row();
                    }
                });
        });
    });
}

fn dispatch_chart_query(
    obs: &ObservabilityPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    if let Some(cid) = obs.selected_chart_id {
        let now = chrono::Utc::now().timestamp();
        let secs = match obs.chart_range {
            ChartRangePreset::Hour1 => 3600,
            ChartRangePreset::Hour6 => 6 * 3600,
            ChartRangePreset::Hour24 => 24 * 3600,
            ChartRangePreset::Day7 => 7 * 86400,
            ChartRangePreset::Day30 => 30 * 86400,
            ChartRangePreset::CustomHours => i64::from(obs.custom_hours) * 3600,
        };
        let _ = cmd_tx.try_send(ClientCommand::QueryChartSeries(v1::QueryChartSeriesRequest {
            chart_id: cid,
            start_utc: now - secs,
            end_utc: now,
            max_points: 600,
        }));
    }
}

fn fmt_plot_axis_ts(utc_sec: i64) -> String {
    chrono::DateTime::from_timestamp(utc_sec, 0)
        .map(|dt| {
            dt.with_timezone(&chrono::Local)
                .format("%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| utc_sec.to_string())
}

pub fn render_charts_panel(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    lang: Language,
    theme: ThemeMode,
    state: &ClientStateCache,
    obs: &mut ObservabilityPanelState,
    cmd_tx: &mpsc::Sender<ClientCommand>,
) {
    section_heading(ui, theme, tr(lang, TextKey::PanelCharts), None);
    ui.add_space(6.0);

    let mut trigger_reload = false;
    toolbar_frame(theme).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_label("Chart")
                .selected_text(
                    obs.selected_chart_id
                        .and_then(|id| state.charts.get(&id))
                        .map_or("Select Chart", |c| c.name.as_str()),
                )
                .show_ui(ui, |ui| {
                    for ch in state.charts.values() {
                        if ui
                            .selectable_value(&mut obs.selected_chart_id, Some(ch.id), &ch.name)
                            .changed()
                        {
                            trigger_reload = true;
                        }
                    }
                });
            for (preset, lbl) in [
                (ChartRangePreset::Hour1, "1h"),
                (ChartRangePreset::Hour6, "6h"),
                (ChartRangePreset::Hour24, "24h"),
                (ChartRangePreset::Day7, "7d"),
                (ChartRangePreset::Day30, "30d"),
                (ChartRangePreset::CustomHours, "Custom(h)"),
            ] {
                if ui.selectable_value(&mut obs.chart_range, preset, lbl).changed() {
                    trigger_reload = true;
                }
            }
            if obs.chart_range == ChartRangePreset::CustomHours {
                ui.add(egui::DragValue::new(&mut obs.custom_hours).range(1..=720));
            }
            ui.checkbox(&mut obs.show_min_max, "Min/Max");

            if ui.button(tr(lang, TextKey::Refresh)).clicked() {
                trigger_reload = true;
            }
            if ui.button("+ Chart").clicked() {
                obs.show_new_chart_modal = true;
                obs.new_chart_name.clear();
            }
            if ui
                .add_enabled(
                    obs.loaded_chart.is_some(),
                    egui::Button::new(tr(lang, TextKey::ExportCsv)),
                )
                .clicked()
            {
                if let Some(ref data) = obs.loaded_chart {
                    if let Some(path) = rfd::FileDialog::new()
                        .set_file_name("nextdude_chart.csv")
                        .add_filter("CSV", &["csv"])
                        .save_file()
                    {
                        if let Ok(mut wtr) = csv::Writer::from_path(path) {
                            let _ = wtr.write_record([
                                "series",
                                "timestamp_local",
                                "avg",
                                "min",
                                "max",
                            ]);
                            for s in &data.series {
                                for pt in &s.samples {
                                    let _ = wtr.write_record([
                                        s.label.clone(),
                                        fmt_local_ts(pt.timestamp_utc),
                                        pt.avg.to_string(),
                                        pt.min.to_string(),
                                        pt.max.to_string(),
                                    ]);
                                }
                            }
                            let _ = wtr.flush();
                        }
                    }
                }
            }
        });
    });

    if trigger_reload {
        dispatch_chart_query(obs, cmd_tx);
    }
    ui.add_space(6.0);

    if let Some(ref data) = obs.loaded_chart {
        card_frame(theme).show(ui, |ui| {
            Plot::new("nextdude_chart_plot")
                .legend(Legend::default())
                .x_axis_formatter(|mark, _range| fmt_plot_axis_ts(mark.value.round() as i64))
                .show(ui, |plot_ui| {
                    for s in &data.series {
                        let avg_pts: PlotPoints = s
                            .samples
                            .iter()
                            .map(|p| [p.timestamp_utc as f64, p.avg])
                            .collect();
                        plot_ui.line(Line::new(avg_pts).name(format!("{} (avg)", s.label)));
                        if obs.show_min_max {
                            let min_pts: PlotPoints = s
                                .samples
                                .iter()
                                .map(|p| [p.timestamp_utc as f64, p.min])
                                .collect();
                            let max_pts: PlotPoints = s
                                .samples
                                .iter()
                                .map(|p| [p.timestamp_utc as f64, p.max])
                                .collect();
                            plot_ui.line(Line::new(min_pts).name(format!("{} (min)", s.label)));
                            plot_ui.line(Line::new(max_pts).name(format!("{} (max)", s.label)));
                        }
                    }
                });
        });
    } else {
        empty_state_card(
            ui,
            theme,
            pick_lang(lang, "Pilih atau buat grafik", "Select or create a chart"),
            pick_lang(
                lang,
                "Grafik menampilkan riwayat latency, nilai sensor SNMP, atau bps interface.",
                "Charts render service RTT, SNMP sensor values, or interface traffic rollups.",
            ),
        );
    }

    if obs.show_new_chart_modal {
        let mut open = true;
        egui::Window::new(tr(lang, TextKey::PanelCharts))
            .open(&mut open)
            .min_width(380.0)
            .show(ctx, |ui| {
                egui::Grid::new("new_chart_form")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        ui.label(tr(lang, TextKey::Name));
                        ui.text_edit_singleline(&mut obs.new_chart_name);
                        ui.end_row();

                        ui.label("Service Source");
                        egui::ComboBox::from_id_salt("chart_svc_src")
                            .selected_text(
                                obs.new_chart_service_id
                                    .and_then(|id| state.services.get(&id))
                                    .map_or("None", |s| s.name.as_str()),
                            )
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut obs.new_chart_service_id, None, "None");
                                for s in state.services.values() {
                                    let dname = state
                                        .devices
                                        .get(&s.device_id)
                                        .map_or("?", |d| d.name.as_str());
                                    ui.selectable_value(
                                        &mut obs.new_chart_service_id,
                                        Some(s.id),
                                        format!("{dname}: {}", s.name),
                                    );
                                }
                            });
                        ui.end_row();

                        ui.label("Link Source");
                        egui::ComboBox::from_id_salt("chart_link_src")
                            .selected_text(
                                obs.new_chart_link_id
                                    .map_or("None".to_string(), |id| format!("Link #{id}")),
                            )
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut obs.new_chart_link_id, None, "None");
                                for l in state.links.values() {
                                    ui.selectable_value(
                                        &mut obs.new_chart_link_id,
                                        Some(l.id),
                                        format!("Link #{} ({})", l.id, l.interface_key),
                                    );
                                }
                            });
                        ui.end_row();

                        ui.label("Metric");
                        egui::ComboBox::from_id_salt("chart_metric_cb")
                            .selected_text(&obs.new_chart_metric)
                            .show_ui(ui, |ui| {
                                for m in ["value", "rtt", "in_bps", "out_bps", "util_pct"] {
                                    ui.selectable_value(
                                        &mut obs.new_chart_metric,
                                        m.to_string(),
                                        m,
                                    );
                                }
                            });
                        ui.end_row();
                    });
                ui.add_space(6.0);
                if ui
                    .add_enabled(
                        !obs.new_chart_name.trim().is_empty(),
                        egui::Button::new(tr(lang, TextKey::Save)),
                    )
                    .clicked()
                {
                    let chart = v1::ChartDefinition {
                        id: 0,
                        version: 1,
                        name: obs.new_chart_name.trim().to_string(),
                        sources: vec![v1::ChartSeriesSource {
                            id: 1,
                            label: obs.new_chart_name.trim().to_string(),
                            service_id: obs.new_chart_service_id,
                            link_id: obs.new_chart_link_id,
                            metric: obs.new_chart_metric.clone(),
                            color_hex: "#38BDF8".to_string(),
                        }],
                    };
                    let _ = cmd_tx.try_send(ClientCommand::SaveChart(chart));
                    obs.show_new_chart_modal = false;
                }
            });
        if !open {
            obs.show_new_chart_modal = false;
        }
    }
}
