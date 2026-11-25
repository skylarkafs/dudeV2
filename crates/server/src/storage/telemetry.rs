use crate::crypto::SecretBox;
use crate::error::ServerError;
use crate::storage::entities::check_version;
use nextdude_common::proto::v1;
use nextdude_common::validation::validate_entity_name;
use rusqlite::{params, Connection, OptionalExtension};

pub const SOURCE_SERVICE_VALUE: i32 = 1;
pub const SOURCE_SERVICE_RTT: i32 = 2;
pub const SOURCE_LINK_IN_BPS: i32 = 3;
pub const SOURCE_LINK_OUT_BPS: i32 = 4;
pub const SOURCE_LINK_UTIL_PCT: i32 = 5;

#[derive(Debug, Clone)]
pub struct RawMetricSample {
    pub source_kind: i32,
    pub source_id: i64,
    pub ts_utc: i64,
    pub val: f64,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredVarbind {
    oid: String,
    oid_name: String,
    value_type: String,
    value: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredChartSource {
    id: i64,
    label: String,
    service_id: Option<i64>,
    link_id: Option<i64>,
    metric: String,
    color_hex: String,
}

pub fn insert_raw_samples_batch(
    conn: &mut Connection,
    samples: &[RawMetricSample],
) -> Result<(), ServerError> {
    if samples.is_empty() {
        return Ok(());
    }
    let tx = conn
        .transaction()
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    {
        let mut stmt = tx
            .prepare_cached(
                "INSERT OR REPLACE INTO ts_raw (source_kind, source_id, ts_utc, val) VALUES (?1,?2,?3,?4)",
            )
            .map_err(|e| ServerError::Storage(e.to_string()))?;
        for s in samples {
            stmt.execute(params![s.source_kind, s.source_id, s.ts_utc, s.val])
                .map_err(|e| ServerError::Storage(e.to_string()))?;
        }
    }
    tx.commit()
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    Ok(())
}

pub fn run_rollups(conn: &mut Connection, now_utc: i64) -> Result<(), ServerError> {
    let tx = conn
        .transaction()
        .map_err(|e| ServerError::Storage(e.to_string()))?;

    let cutoff_1m = now_utc - 7200;
    tx.execute(
        "INSERT OR REPLACE INTO ts_1m (source_kind, source_id, bucket_utc, avg_val, min_val, max_val, sample_count)
         SELECT source_kind, source_id, (ts_utc / 60) * 60 AS b,
                AVG(val), MIN(val), MAX(val), COUNT(*)
         FROM ts_raw WHERE ts_utc >= ?1
         GROUP BY source_kind, source_id, b",
        params![cutoff_1m],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;

    let cutoff_5m = now_utc - 86400;
    tx.execute(
        "INSERT OR REPLACE INTO ts_5m (source_kind, source_id, bucket_utc, avg_val, min_val, max_val, sample_count)
         SELECT source_kind, source_id, (bucket_utc / 300) * 300 AS b,
                SUM(avg_val * sample_count) / SUM(sample_count), MIN(min_val), MAX(max_val), SUM(sample_count)
         FROM ts_1m WHERE bucket_utc >= ?1
         GROUP BY source_kind, source_id, b",
        params![cutoff_5m],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;

    let cutoff_1h = now_utc - 7 * 86400;
    tx.execute(
        "INSERT OR REPLACE INTO ts_1h (source_kind, source_id, bucket_utc, avg_val, min_val, max_val, sample_count)
         SELECT source_kind, source_id, (bucket_utc / 3600) * 3600 AS b,
                SUM(avg_val * sample_count) / SUM(sample_count), MIN(min_val), MAX(max_val), SUM(sample_count)
         FROM ts_5m WHERE bucket_utc >= ?1
         GROUP BY source_kind, source_id, b",
        params![cutoff_1h],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;

    tx.commit()
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    Ok(())
}

pub fn enforce_retention_and_vacuum(
    conn: &mut Connection,
    now_utc: i64,
    raw_hours: u32,
    rollup_days: u32,
    events_days: u32,
    syslog_days: u32,
    audit_days: u32,
    do_vacuum: bool,
) -> Result<(), ServerError> {
    let raw_cutoff = now_utc - i64::from(raw_hours) * 3600;
    let rollup_cutoff = now_utc - i64::from(rollup_days) * 86400;
    let events_cutoff = now_utc - i64::from(events_days) * 86400;
    let syslog_cutoff = now_utc - i64::from(syslog_days) * 86400;
    let audit_cutoff = now_utc - i64::from(audit_days) * 86400;

    let tx = conn
        .transaction()
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    tx.execute("DELETE FROM ts_raw WHERE ts_utc < ?1", params![raw_cutoff])
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    tx.execute("DELETE FROM ts_1m WHERE bucket_utc < ?1", params![rollup_cutoff])
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    tx.execute("DELETE FROM ts_5m WHERE bucket_utc < ?1", params![rollup_cutoff])
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    tx.execute("DELETE FROM ts_1h WHERE bucket_utc < ?1", params![rollup_cutoff])
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    tx.execute(
        "DELETE FROM event_logs WHERE timestamp_utc < ?1",
        params![events_cutoff],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    tx.execute(
        "DELETE FROM syslog_messages WHERE timestamp_utc < ?1",
        params![syslog_cutoff],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    tx.execute(
        "DELETE FROM trap_messages WHERE timestamp_utc < ?1",
        params![events_cutoff],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    tx.execute(
        "DELETE FROM history_actions WHERE timestamp_utc < ?1",
        params![audit_cutoff],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    tx.commit()
        .map_err(|e| ServerError::Storage(e.to_string()))?;

    if do_vacuum {
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")
            .map_err(|e| ServerError::Storage(format!("VACUUM failed: {e}")))?;
    }
    Ok(())
}

pub fn query_chart_series(
    conn: &Connection,
    chart_id: i64,
    start_utc: i64,
    end_utc: i64,
    max_points: u32,
) -> Result<v1::QueryChartSeriesResponse, ServerError> {
    let sources_json: String = conn
        .query_row(
            "SELECT sources_json FROM charts WHERE id = ?1",
            params![chart_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| ServerError::Storage(e.to_string()))?
        .ok_or_else(|| ServerError::NotFound(format!("chart {chart_id}")))?;
    let sources: Vec<StoredChartSource> = serde_json::from_str(&sources_json).unwrap_or_default();

    let span = (end_utc - start_utc).max(60);
    let agg = if span <= 3 * 3600 {
        v1::ChartAggregate::Raw
    } else if span <= 24 * 3600 {
        v1::ChartAggregate::OneMinute
    } else if span <= 7 * 86400 {
        v1::ChartAggregate::FiveMinutes
    } else {
        v1::ChartAggregate::OneHour
    };

    let target_pts = max_points.clamp(16, 2000) as usize;
    let mut series_out = Vec::with_capacity(sources.len());

    for src in sources {
        let (kind, sid) = if let Some(svc_id) = src.service_id {
            let k = if src.metric == "rtt" {
                SOURCE_SERVICE_RTT
            } else {
                SOURCE_SERVICE_VALUE
            };
            (k, svc_id)
        } else if let Some(link_id) = src.link_id {
            let k = match src.metric.as_str() {
                "out_bps" => SOURCE_LINK_OUT_BPS,
                "util_pct" => SOURCE_LINK_UTIL_PCT,
                _ => SOURCE_LINK_IN_BPS,
            };
            (k, link_id)
        } else {
            continue;
        };

        let raw_points: Vec<v1::ChartSample> = match agg {
            v1::ChartAggregate::Raw => {
                let mut stmt = conn
                    .prepare("SELECT ts_utc, val FROM ts_raw WHERE source_kind=?1 AND source_id=?2 AND ts_utc BETWEEN ?3 AND ?4 ORDER BY ts_utc")
                    .map_err(|e| ServerError::Storage(e.to_string()))?;
                let rows = stmt
                    .query_map(params![kind, sid, start_utc, end_utc], |r| {
                        let v: f64 = r.get(1)?;
                        Ok(v1::ChartSample {
                            timestamp_utc: r.get(0)?,
                            avg: v,
                            min: v,
                            max: v,
                        })
                    })
                    .map_err(|e| ServerError::Storage(e.to_string()))?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(|e| ServerError::Storage(e.to_string()))?
            }
            _ => {
                let table = match agg {
                    v1::ChartAggregate::OneMinute => "ts_1m",
                    v1::ChartAggregate::FiveMinutes => "ts_5m",
                    _ => "ts_1h",
                };
                let sql = format!(
                    "SELECT bucket_utc, avg_val, min_val, max_val FROM {table} WHERE source_kind=?1 AND source_id=?2 AND bucket_utc BETWEEN ?3 AND ?4 ORDER BY bucket_utc"
                );
                let mut stmt = conn.prepare(&sql).map_err(|e| ServerError::Storage(e.to_string()))?;
                let rows = stmt
                    .query_map(params![kind, sid, start_utc, end_utc], |r| {
                        Ok(v1::ChartSample {
                            timestamp_utc: r.get(0)?,
                            avg: r.get(1)?,
                            min: r.get(2)?,
                            max: r.get(3)?,
                        })
                    })
                    .map_err(|e| ServerError::Storage(e.to_string()))?;
                rows.collect::<Result<Vec<_>, _>>()
                    .map_err(|e| ServerError::Storage(e.to_string()))?
            }
        };

        let downsampled = downsample_min_avg_max(&raw_points, target_pts);
        series_out.push(v1::ChartSeriesData {
            source_id: src.id,
            label: src.label,
            color_hex: src.color_hex,
            samples: downsampled,
        });
    }

    Ok(v1::QueryChartSeriesResponse {
        series: series_out,
        aggregate_used: agg as i32,
    })
}

pub fn downsample_min_avg_max(samples: &[v1::ChartSample], max_points: usize) -> Vec<v1::ChartSample> {
    if samples.len() <= max_points || max_points == 0 {
        return samples.to_vec();
    }
    let chunk_size = (samples.len() + max_points - 1) / max_points;
    samples
        .chunks(chunk_size)
        .map(|chunk| {
            let mut min_v = f64::INFINITY;
            let mut max_v = f64::NEG_INFINITY;
            let mut sum = 0.0;
            for p in chunk {
                min_v = min_v.min(p.min);
                max_v = max_v.max(p.max);
                sum += p.avg;
            }
            let mid = &chunk[chunk.len() / 2];
            v1::ChartSample {
                timestamp_utc: mid.timestamp_utc,
                avg: sum / (chunk.len() as f64),
                min: min_v,
                max: max_v,
            }
        })
        .collect()
}

pub fn load_charts(conn: &Connection) -> Result<Vec<v1::ChartDefinition>, ServerError> {
    let mut stmt = conn
        .prepare("SELECT id, version, name, sources_json FROM charts ORDER BY id")
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            let json: String = r.get(3)?;
            let sources = serde_json::from_str::<Vec<StoredChartSource>>(&json)
                .unwrap_or_default()
                .into_iter()
                .map(|s| v1::ChartSeriesSource {
                    id: s.id,
                    label: s.label,
                    service_id: s.service_id,
                    link_id: s.link_id,
                    metric: s.metric,
                    color_hex: s.color_hex,
                })
                .collect();
            Ok(v1::ChartDefinition {
                id: r.get(0)?,
                version: r.get::<_, i64>(1)? as u64,
                name: r.get(2)?,
                sources,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))
}

pub fn save_chart(
    conn: &mut Connection,
    chart: &v1::ChartDefinition,
) -> Result<v1::ChartDefinition, ServerError> {
    let name = validate_entity_name(&chart.name)?;
    let stored: Vec<StoredChartSource> = chart
        .sources
        .iter()
        .enumerate()
        .map(|(i, s)| StoredChartSource {
            id: if s.id > 0 { s.id } else { (i + 1) as i64 },
            label: s.label.clone(),
            service_id: s.service_id,
            link_id: s.link_id,
            metric: s.metric.clone(),
            color_hex: s.color_hex.clone(),
        })
        .collect();
    let json = serde_json::to_string(&stored).unwrap_or_else(|_| "[]".to_string());
    if chart.id <= 0 {
        conn.execute(
            "INSERT INTO charts (name, sources_json) VALUES (?1, ?2)",
            params![name, json],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
        let mut out = chart.clone();
        out.id = conn.last_insert_rowid();
        out.version = 1;
        out.name = name;
        return Ok(out);
    }
    check_version(conn, "charts", chart.id, chart.version)?;
    conn.execute(
        "UPDATE charts SET name=?1, sources_json=?2, version=version+1 WHERE id=?3",
        params![name, json, chart.id],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut out = chart.clone();
    out.version += 1;
    out.name = name;
    Ok(out)
}

pub fn insert_event_log(
    conn: &mut Connection,
    severity: i32,
    category: &str,
    device_id: Option<i64>,
    device_name: &str,
    message: &str,
) -> Result<v1::EventLogEntry, ServerError> {
    let ts = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO event_logs (timestamp_utc, severity, category, device_id, device_name, message)
         VALUES (?1,?2,?3,?4,?5,?6)",
        params![ts, severity, category, device_id, device_name, message],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    Ok(v1::EventLogEntry {
        id: conn.last_insert_rowid(),
        timestamp_utc: ts,
        severity,
        category: category.to_string(),
        device_id,
        device_name: device_name.to_string(),
        message: message.to_string(),
        acknowledged: false,
        ack_by: None,
        ack_utc: None,
    })
}

pub fn acknowledge_event(
    conn: &mut Connection,
    event_id: i64,
    username: &str,
) -> Result<(), ServerError> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "UPDATE event_logs SET acknowledged=1, ack_by=?1, ack_utc=?2 WHERE id=?3",
        params![username, now, event_id],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    Ok(())
}

pub fn insert_syslog_batch(
    conn: &mut Connection,
    entries: &[v1::SyslogMessage],
) -> Result<Vec<v1::SyslogMessage>, ServerError> {
    if entries.is_empty() {
        return Ok(Vec::new());
    }
    let tx = conn
        .transaction()
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut inserted = Vec::with_capacity(entries.len());
    for e in entries {
        tx.execute(
            "INSERT INTO syslog_messages (timestamp_utc, source_ip, device_id, device_name, facility, severity, hostname, app_name, message, raw_format)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                e.timestamp_utc,
                e.source_ip,
                e.device_id,
                e.device_name,
                i64::from(e.facility),
                i64::from(e.severity),
                e.hostname,
                e.app_name,
                e.message,
                e.raw_format
            ],
        )
        .map_err(|err| ServerError::Storage(err.to_string()))?;
        let mut copy = e.clone();
        copy.id = tx.last_insert_rowid();
        inserted.push(copy);
    }
    tx.commit()
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    Ok(inserted)
}

pub fn insert_trap(conn: &mut Connection, trap: &v1::TrapMessage) -> Result<v1::TrapMessage, ServerError> {
    let varbinds_json = serde_json::to_string(
        &trap
            .varbinds
            .iter()
            .map(|v| StoredVarbind {
                oid: v.oid.clone(),
                oid_name: v.oid_name.clone(),
                value_type: v.value_type.clone(),
                value: v.value.clone(),
            })
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        "INSERT INTO trap_messages (timestamp_utc, source_ip, device_id, device_name, snmp_version, trap_oid, trap_name, varbinds_json)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            trap.timestamp_utc,
            trap.source_ip,
            trap.device_id,
            trap.device_name,
            trap.snmp_version,
            trap.trap_oid,
            trap.trap_name,
            varbinds_json
        ],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut out = trap.clone();
    out.id = conn.last_insert_rowid();
    Ok(out)
}

pub fn load_alert_rules(conn: &Connection) -> Result<Vec<v1::AlertRule>, ServerError> {
    let mut stmt = conn
        .prepare("SELECT id, version, name, enabled, device_id, probe_id, trigger_on_state_change, value_threshold, threshold_greater_than, min_duration_sec, severity, channel_ids_json FROM alert_rules ORDER BY id")
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            let ch_json: String = r.get(11)?;
            Ok(v1::AlertRule {
                id: r.get(0)?,
                version: r.get::<_, i64>(1)? as u64,
                name: r.get(2)?,
                enabled: r.get::<_, i64>(3)? != 0,
                device_id: r.get(4)?,
                probe_id: r.get(5)?,
                trigger_on_state_change: r.get::<_, i64>(6)? != 0,
                value_threshold: r.get(7)?,
                threshold_greater_than: r.get::<_, i64>(8)? != 0,
                min_duration_sec: r.get::<_, i64>(9)? as u32,
                severity: r.get(10)?,
                notification_channel_ids: serde_json::from_str(&ch_json).unwrap_or_default(),
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))
}

pub fn save_alert_rule(conn: &mut Connection, rule: &v1::AlertRule) -> Result<v1::AlertRule, ServerError> {
    let name = validate_entity_name(&rule.name)?;
    let ch_json = serde_json::to_string(&rule.notification_channel_ids).unwrap_or_else(|_| "[]".to_string());
    if rule.id <= 0 {
        conn.execute(
            "INSERT INTO alert_rules (name, enabled, device_id, probe_id, trigger_on_state_change, value_threshold, threshold_greater_than, min_duration_sec, severity, channel_ids_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                name,
                i64::from(rule.enabled),
                rule.device_id,
                rule.probe_id,
                i64::from(rule.trigger_on_state_change),
                rule.value_threshold,
                i64::from(rule.threshold_greater_than),
                i64::from(rule.min_duration_sec),
                rule.severity.max(1),
                ch_json
            ],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
        let mut out = rule.clone();
        out.id = conn.last_insert_rowid();
        out.version = 1;
        out.name = name;
        return Ok(out);
    }
    check_version(conn, "alert_rules", rule.id, rule.version)?;
    conn.execute(
        "UPDATE alert_rules SET name=?1, enabled=?2, device_id=?3, probe_id=?4, trigger_on_state_change=?5, value_threshold=?6, threshold_greater_than=?7, min_duration_sec=?8, severity=?9, channel_ids_json=?10, version=version+1 WHERE id=?11",
        params![
            name,
            i64::from(rule.enabled),
            rule.device_id,
            rule.probe_id,
            i64::from(rule.trigger_on_state_change),
            rule.value_threshold,
            i64::from(rule.threshold_greater_than),
            i64::from(rule.min_duration_sec),
            rule.severity.max(1),
            ch_json,
            rule.id
        ],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut out = rule.clone();
    out.version += 1;
    out.name = name;
    Ok(out)
}

pub fn load_maintenance_windows(conn: &Connection) -> Result<Vec<v1::MaintenanceWindow>, ServerError> {
    let mut stmt = conn
        .prepare("SELECT id, version, name, device_id, map_id, start_utc, end_utc, recurring_weekly, weekday_mask, enabled FROM maintenance_windows ORDER BY id")
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(v1::MaintenanceWindow {
                id: r.get(0)?,
                version: r.get::<_, i64>(1)? as u64,
                name: r.get(2)?,
                device_id: r.get(3)?,
                map_id: r.get(4)?,
                start_utc: r.get(5)?,
                end_utc: r.get(6)?,
                recurring_weekly: r.get::<_, i64>(7)? != 0,
                weekday_mask: r.get::<_, i64>(8)? as u32,
                enabled: r.get::<_, i64>(9)? != 0,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))
}

pub fn save_maintenance_window(
    conn: &mut Connection,
    mw: &v1::MaintenanceWindow,
) -> Result<v1::MaintenanceWindow, ServerError> {
    let name = validate_entity_name(&mw.name)?;
    if mw.id <= 0 {
        conn.execute(
            "INSERT INTO maintenance_windows (name, device_id, map_id, start_utc, end_utc, recurring_weekly, weekday_mask, enabled)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                name,
                mw.device_id,
                mw.map_id,
                mw.start_utc,
                mw.end_utc,
                i64::from(mw.recurring_weekly),
                i64::from(mw.weekday_mask),
                i64::from(mw.enabled)
            ],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
        let mut out = mw.clone();
        out.id = conn.last_insert_rowid();
        out.version = 1;
        out.name = name;
        return Ok(out);
    }
    check_version(conn, "maintenance_windows", mw.id, mw.version)?;
    conn.execute(
        "UPDATE maintenance_windows SET name=?1, device_id=?2, map_id=?3, start_utc=?4, end_utc=?5, recurring_weekly=?6, weekday_mask=?7, enabled=?8, version=version+1 WHERE id=?9",
        params![
            name,
            mw.device_id,
            mw.map_id,
            mw.start_utc,
            mw.end_utc,
            i64::from(mw.recurring_weekly),
            i64::from(mw.weekday_mask),
            i64::from(mw.enabled),
            mw.id
        ],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut out = mw.clone();
    out.version += 1;
    out.name = name;
    Ok(out)
}

pub fn load_notification_channels(
    conn: &Connection,
) -> Result<Vec<v1::NotificationChannel>, ServerError> {
    let mut stmt = conn
        .prepare("SELECT id, version, name, kind, enabled, target, secret_token_enc, rate_limit_per_min FROM notification_channels ORDER BY id")
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            let enc: String = r.get(6)?;
            Ok(v1::NotificationChannel {
                id: r.get(0)?,
                version: r.get::<_, i64>(1)? as u64,
                name: r.get(2)?,
                kind: r.get(3)?,
                enabled: r.get::<_, i64>(4)? != 0,
                target: r.get(5)?,
                secret_token: None,
                has_secret_token: !enc.is_empty(),
                rate_limit_per_min: r.get::<_, i64>(7)? as u32,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))
}

pub fn load_channel_secret(
    conn: &Connection,
    secrets: &SecretBox,
    channel_id: i64,
) -> Result<String, ServerError> {
    let enc: String = conn
        .query_row(
            "SELECT secret_token_enc FROM notification_channels WHERE id = ?1",
            params![channel_id],
            |r| r.get(0),
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    secrets.decrypt(&enc)
}

pub fn save_notification_channel(
    conn: &mut Connection,
    secrets: &SecretBox,
    ch: &v1::NotificationChannel,
) -> Result<v1::NotificationChannel, ServerError> {
    let name = validate_entity_name(&ch.name)?;
    if ch.id <= 0 {
        let enc = match ch.secret_token.as_deref() {
            Some(t) if !t.is_empty() => secrets.encrypt(t)?,
            _ => String::new(),
        };
        conn.execute(
            "INSERT INTO notification_channels (name, kind, enabled, target, secret_token_enc, rate_limit_per_min)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                name,
                ch.kind,
                i64::from(ch.enabled),
                ch.target,
                enc,
                i64::from(ch.rate_limit_per_min.max(1))
            ],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
        let mut out = ch.clone();
        out.id = conn.last_insert_rowid();
        out.version = 1;
        out.name = name;
        out.secret_token = None;
        out.has_secret_token = !enc.is_empty();
        return Ok(out);
    }

    check_version(conn, "notification_channels", ch.id, ch.version)?;
    let old_enc: String = conn
        .query_row(
            "SELECT secret_token_enc FROM notification_channels WHERE id = ?1",
            params![ch.id],
            |r| r.get(0),
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let enc = match ch.secret_token.as_deref() {
        Some(t) if !t.is_empty() => secrets.encrypt(t)?,
        _ => old_enc,
    };
    conn.execute(
        "UPDATE notification_channels SET name=?1, kind=?2, enabled=?3, target=?4, secret_token_enc=?5, rate_limit_per_min=?6, version=version+1 WHERE id=?7",
        params![
            name,
            ch.kind,
            i64::from(ch.enabled),
            ch.target,
            enc,
            i64::from(ch.rate_limit_per_min.max(1)),
            ch.id
        ],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut out = ch.clone();
    out.version += 1;
    out.name = name;
    out.secret_token = None;
    out.has_secret_token = !enc.is_empty();
    Ok(out)
}

pub fn load_server_settings(
    conn: &Connection,
    icmp_mode: u32,
    icmp_msg: &str,
) -> Result<v1::ServerSettings, ServerError> {
    conn.query_row(
        "SELECT version, global_poll_interval_sec, max_concurrent_probes, max_concurrent_per_host,
                flapping_changes, flapping_window_min, syslog_enabled, syslog_port, syslog_forward_addr,
                trap_enabled, trap_port, mndp_enabled, allow_image_url_import,
                retention_raw_hours, retention_rollup_days, retention_events_days,
                retention_syslog_days, retention_audit_days
         FROM server_runtime_settings WHERE id = 1",
        [],
        |r| {
            Ok(v1::ServerSettings {
                version: r.get::<_, i64>(0)? as u64,
                global_poll_interval_sec: r.get::<_, i64>(1)? as u32,
                max_concurrent_probes: r.get::<_, i64>(2)? as u32,
                max_concurrent_per_host: r.get::<_, i64>(3)? as u32,
                flapping_changes: r.get::<_, i64>(4)? as u32,
                flapping_window_min: r.get::<_, i64>(5)? as u32,
                syslog_enabled: r.get::<_, i64>(6)? != 0,
                syslog_port: r.get::<_, i64>(7)? as u32,
                syslog_forward_addr: r.get(8)?,
                trap_enabled: r.get::<_, i64>(9)? != 0,
                trap_port: r.get::<_, i64>(10)? as u32,
                mndp_enabled: r.get::<_, i64>(11)? != 0,
                allow_image_url_import: r.get::<_, i64>(12)? != 0,
                retention_raw_hours: r.get::<_, i64>(13)? as u32,
                retention_rollup_days: r.get::<_, i64>(14)? as u32,
                retention_events_days: r.get::<_, i64>(15)? as u32,
                retention_syslog_days: r.get::<_, i64>(16)? as u32,
                retention_audit_days: r.get::<_, i64>(17)? as u32,
                icmp_mode,
                icmp_status_message: icmp_msg.to_string(),
            })
        },
    )
    .map_err(|e| ServerError::Storage(e.to_string()))
}

pub fn update_server_settings(
    conn: &mut Connection,
    s: &v1::ServerSettings,
) -> Result<v1::ServerSettings, ServerError> {
    check_version(conn, "server_runtime_settings", 1, s.version)?;
    conn.execute(
        "UPDATE server_runtime_settings SET
          global_poll_interval_sec=?1, max_concurrent_probes=?2, max_concurrent_per_host=?3,
          flapping_changes=?4, flapping_window_min=?5, syslog_enabled=?6, syslog_port=?7,
          syslog_forward_addr=?8, trap_enabled=?9, trap_port=?10, mndp_enabled=?11,
          allow_image_url_import=?12, retention_raw_hours=?13, retention_rollup_days=?14,
          retention_events_days=?15, retention_syslog_days=?16, retention_audit_days=?17,
          version=version+1
         WHERE id = 1",
        params![
            i64::from(s.global_poll_interval_sec.max(5)),
            i64::from(s.max_concurrent_probes.max(1)),
            i64::from(s.max_concurrent_per_host.max(1)),
            i64::from(s.flapping_changes.max(2)),
            i64::from(s.flapping_window_min.max(1)),
            i64::from(s.syslog_enabled),
            i64::from(s.syslog_port.max(1)),
            s.syslog_forward_addr,
            i64::from(s.trap_enabled),
            i64::from(s.trap_port.max(1)),
            i64::from(s.mndp_enabled),
            i64::from(s.allow_image_url_import),
            i64::from(s.retention_raw_hours.max(1)),
            i64::from(s.retention_rollup_days.max(1)),
            i64::from(s.retention_events_days.max(1)),
            i64::from(s.retention_syslog_days.max(1)),
            i64::from(s.retention_audit_days.max(1))
        ],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut out = s.clone();
    out.version += 1;
    Ok(out)
}

pub fn query_events(conn: &Connection, q: &v1::PageQueryRequest) -> Result<v1::QueryEventsResponse, ServerError> {
    let limit = q.limit.clamp(1, 500) as i64;
    let offset = q.offset as i64;
    let like = format!("%{}%", q.text_filter);
    let min_sev = q.min_severity.unwrap_or(0);
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM event_logs
             WHERE timestamp_utc BETWEEN ?1 AND ?2
               AND (?3 IS NULL OR device_id = ?3)
               AND severity >= ?4
               AND (message LIKE ?5 OR device_name LIKE ?5 OR category LIKE ?5)",
            params![q.start_utc, q.end_utc, q.device_id, min_sev, like],
            |r| r.get(0),
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, timestamp_utc, severity, category, device_id, device_name, message, acknowledged, ack_by, ack_utc
             FROM event_logs
             WHERE timestamp_utc BETWEEN ?1 AND ?2
               AND (?3 IS NULL OR device_id = ?3)
               AND severity >= ?4
               AND (message LIKE ?5 OR device_name LIKE ?5 OR category LIKE ?5)
             ORDER BY timestamp_utc DESC, id DESC LIMIT ?6 OFFSET ?7",
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let items = stmt
        .query_map(params![q.start_utc, q.end_utc, q.device_id, min_sev, like, limit, offset], |r| {
            Ok(v1::EventLogEntry {
                id: r.get(0)?,
                timestamp_utc: r.get(1)?,
                severity: r.get(2)?,
                category: r.get(3)?,
                device_id: r.get(4)?,
                device_name: r.get(5)?,
                message: r.get(6)?,
                acknowledged: r.get::<_, i64>(7)? != 0,
                ack_by: r.get(8)?,
                ack_utc: r.get(9)?,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    Ok(v1::QueryEventsResponse {
        items,
        total: total as u64,
    })
}

pub fn query_syslog(conn: &Connection, q: &v1::PageQueryRequest) -> Result<v1::QuerySyslogResponse, ServerError> {
    let limit = q.limit.clamp(1, 500) as i64;
    let offset = q.offset as i64;
    let like = format!("%{}%", q.text_filter);
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM syslog_messages
             WHERE timestamp_utc BETWEEN ?1 AND ?2
               AND (?3 IS NULL OR device_id = ?3)
               AND (message LIKE ?4 OR hostname LIKE ?4 OR app_name LIKE ?4 OR source_ip LIKE ?4)",
            params![q.start_utc, q.end_utc, q.device_id, like],
            |r| r.get(0),
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, timestamp_utc, source_ip, device_id, device_name, facility, severity, hostname, app_name, message, raw_format
             FROM syslog_messages
             WHERE timestamp_utc BETWEEN ?1 AND ?2
               AND (?3 IS NULL OR device_id = ?3)
               AND (message LIKE ?4 OR hostname LIKE ?4 OR app_name LIKE ?4 OR source_ip LIKE ?4)
             ORDER BY timestamp_utc DESC, id DESC LIMIT ?5 OFFSET ?6",
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let items = stmt
        .query_map(params![q.start_utc, q.end_utc, q.device_id, like, limit, offset], |r| {
            Ok(v1::SyslogMessage {
                id: r.get(0)?,
                timestamp_utc: r.get(1)?,
                source_ip: r.get(2)?,
                device_id: r.get(3)?,
                device_name: r.get(4)?,
                facility: r.get::<_, i64>(5)? as u32,
                severity: r.get::<_, i64>(6)? as u32,
                hostname: r.get(7)?,
                app_name: r.get(8)?,
                message: r.get(9)?,
                raw_format: r.get(10)?,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    Ok(v1::QuerySyslogResponse {
        items,
        total: total as u64,
    })
}

pub fn query_traps(conn: &Connection, q: &v1::PageQueryRequest) -> Result<v1::QueryTrapsResponse, ServerError> {
    let limit = q.limit.clamp(1, 500) as i64;
    let offset = q.offset as i64;
    let like = format!("%{}%", q.text_filter);
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM trap_messages
             WHERE timestamp_utc BETWEEN ?1 AND ?2
               AND (?3 IS NULL OR device_id = ?3)
               AND (trap_oid LIKE ?4 OR trap_name LIKE ?4 OR varbinds_json LIKE ?4 OR source_ip LIKE ?4)",
            params![q.start_utc, q.end_utc, q.device_id, like],
            |r| r.get(0),
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, timestamp_utc, source_ip, device_id, device_name, snmp_version, trap_oid, trap_name, varbinds_json
             FROM trap_messages
             WHERE timestamp_utc BETWEEN ?1 AND ?2
               AND (?3 IS NULL OR device_id = ?3)
               AND (trap_oid LIKE ?4 OR trap_name LIKE ?4 OR varbinds_json LIKE ?4 OR source_ip LIKE ?4)
             ORDER BY timestamp_utc DESC, id DESC LIMIT ?5 OFFSET ?6",
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let items = stmt
        .query_map(params![q.start_utc, q.end_utc, q.device_id, like, limit, offset], |r| {
            let vb_json: String = r.get(8)?;
            let varbinds = serde_json::from_str::<Vec<StoredVarbind>>(&vb_json)
                .unwrap_or_default()
                .into_iter()
                .map(|v| v1::TrapVarbind {
                    oid: v.oid,
                    oid_name: v.oid_name,
                    value_type: v.value_type,
                    value: v.value,
                })
                .collect();
            Ok(v1::TrapMessage {
                id: r.get(0)?,
                timestamp_utc: r.get(1)?,
                source_ip: r.get(2)?,
                device_id: r.get(3)?,
                device_name: r.get(4)?,
                snmp_version: r.get(5)?,
                trap_oid: r.get(6)?,
                trap_name: r.get(7)?,
                varbinds,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    Ok(v1::QueryTrapsResponse {
        items,
        total: total as u64,
    })
}

pub fn query_outages(conn: &Connection, q: &v1::PageQueryRequest) -> Result<v1::QueryOutagesResponse, ServerError> {
    let limit = q.limit.clamp(1, 500) as i64;
    let offset = q.offset as i64;
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM outages WHERE started_utc BETWEEN ?1 AND ?2 AND (?3 IS NULL OR device_id = ?3)",
            params![q.start_utc, q.end_utc, q.device_id],
            |r| r.get(0),
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;

    let mut stmt = conn
        .prepare(
            "SELECT o.id, o.device_id, COALESCE(d.name, ''), o.service_id, COALESCE(s.name, ''),
                    o.started_utc, o.ended_utc, o.root_cause_note, o.suppressed_by_dependency
             FROM outages o
             LEFT JOIN devices d ON o.device_id = d.id
             LEFT JOIN device_services s ON o.service_id = s.id
             WHERE o.started_utc BETWEEN ?1 AND ?2 AND (?3 IS NULL OR o.device_id = ?3)
             ORDER BY o.started_utc DESC, o.id DESC LIMIT ?4 OFFSET ?5",
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let items = stmt
        .query_map(params![q.start_utc, q.end_utc, q.device_id, limit, offset], |r| {
            Ok(v1::OutageRecord {
                id: r.get(0)?,
                device_id: r.get(1)?,
                device_name: r.get(2)?,
                service_id: r.get(3)?,
                service_name: r.get(4)?,
                started_utc: r.get(5)?,
                ended_utc: r.get(6)?,
                root_cause_note: r.get(7)?,
                suppressed_by_dependency: r.get::<_, i64>(8)? != 0,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    Ok(v1::QueryOutagesResponse {
        items,
        total: total as u64,
    })
}

pub fn query_history_actions(
    conn: &Connection,
    q: &v1::PageQueryRequest,
) -> Result<v1::QueryHistoryActionsResponse, ServerError> {
    let limit = q.limit.clamp(1, 500) as i64;
    let offset = q.offset as i64;
    let like = format!("%{}%", q.text_filter);
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM history_actions
             WHERE timestamp_utc BETWEEN ?1 AND ?2
               AND (username LIKE ?3 OR action LIKE ?3 OR target_type LIKE ?3 OR details LIKE ?3)",
            params![q.start_utc, q.end_utc, like],
            |r| r.get(0),
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, timestamp_utc, admin_id, username, action, target_type, target_id, details
             FROM history_actions
             WHERE timestamp_utc BETWEEN ?1 AND ?2
               AND (username LIKE ?3 OR action LIKE ?3 OR target_type LIKE ?3 OR details LIKE ?3)
             ORDER BY timestamp_utc DESC, id DESC LIMIT ?4 OFFSET ?5",
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let items = stmt
        .query_map(params![q.start_utc, q.end_utc, like, limit, offset], |r| {
            Ok(v1::HistoryAction {
                id: r.get(0)?,
                timestamp_utc: r.get(1)?,
                admin_id: r.get(2)?,
                username: r.get(3)?,
                action: r.get(4)?,
                target_type: r.get(5)?,
                target_id: r.get(6)?,
                details: r.get(7)?,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    Ok(v1::QueryHistoryActionsResponse {
        items,
        total: total as u64,
    })
}
