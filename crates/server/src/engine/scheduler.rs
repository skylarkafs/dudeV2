use crate::config::ServerConfig;
use crate::engine::dependency::compute_dependency_suppression;
use crate::engine::state::{
    advance_probe_state, aggregate_device_state, compute_counter_bps, compute_link_status_and_util,
    stable_interface_key, CounterSample, FlappingTracker,
};
use crate::notify::{AlertDispatch, NotificationManager};
use crate::probe::{LocalProbeExecutor, ProbeExecutor, ProbeTarget};
use crate::web_status::ChildTaskTracker;
use crate::snmp::ber::parse_oid;
use crate::snmp::profiles::VendorProfileRegistry;
use crate::snmp::{AuthProto, PrivProto, ResolvedCredential, SnmpClient, SnmpTarget, UsmCredentials};
use crate::storage::entities::{load_device_secrets, upsert_device_interfaces};
use crate::storage::telemetry::{
    enforce_retention_and_vacuum, insert_event_log, insert_raw_samples_batch, run_rollups,
    RawMetricSample, SOURCE_LINK_IN_BPS, SOURCE_LINK_OUT_BPS, SOURCE_LINK_UTIL_PCT,
    SOURCE_SERVICE_RTT, SOURCE_SERVICE_VALUE,
};
use crate::storage::{entities, telemetry, StorageHandle};
use chrono::{Datelike, Timelike};
use futures::FutureExt;
use nextdude_common::proto::v1;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, RwLock, Semaphore};
use tracing::{error, info, warn};

#[derive(Debug, Clone)]
pub enum ServerBroadcastEvent {
    LiveMetrics(v1::LiveMetricsBatch),
    EntityDelta(v1::EntityDelta),
    EventLog(v1::EventLogEntry),
    Syslog(v1::SyslogMessage),
    Trap(v1::TrapMessage),
    Alert(v1::AlertEvent),
    Outage(v1::OutageRecord),
    DiscoveryProgress(v1::DiscoveryProgressEvent),
    SyslogStats(v1::SyslogDropStats),
}

#[derive(Clone)]
pub struct EngineContext {
    pub config: ServerConfig,
    pub storage: StorageHandle,
    pub snmp: SnmpClient,
    pub executor: Arc<LocalProbeExecutor>,
    pub profiles: Arc<RwLock<VendorProfileRegistry>>,
    pub notifications: Arc<NotificationManager>,
    pub events_tx: broadcast::Sender<ServerBroadcastEvent>,
    pub device_host_map: Arc<RwLock<HashMap<IpAddr, (i64, String)>>>,
    pub link_flapping: Arc<RwLock<HashMap<i64, FlappingTracker>>>,
    pub down_devices: Arc<RwLock<HashSet<i64>>>,
    pub active_outages: Arc<RwLock<HashMap<i64, i64>>>, // service_id -> outage_id
    pub tracker: Arc<ChildTaskTracker>,
    pub server_started_utc: i64,
}

impl EngineContext {
    pub fn broadcast(&self, ev: ServerBroadcastEvent) {
        let _ = self.events_tx.send(ev);
    }

    pub async fn refresh_device_ip_index(&self) {
        if let Ok(devs) = self.storage.read(|c| entities::load_devices(c)).await {
            let mut map = HashMap::with_capacity(devs.len() * 2);
            for d in devs {
                if let Ok(ip) = d.address.parse::<IpAddr>() {
                    map.insert(ip, (d.id, d.name.clone()));
                }
                for extra in &d.extra_ips {
                    if let Ok(ip) = extra.parse::<IpAddr>() {
                        map.insert(ip, (d.id, d.name.clone()));
                    }
                }
            }
            *self.device_host_map.write().await = map;
        }
    }

    /// Handles an immediate SNMP linkDown/linkUp trap without waiting for the polling cycle.
    pub async fn handle_interface_trap_immediate(
        &self,
        device_id: i64,
        if_index: u32,
        is_up: bool,
    ) {
        let now_utc = chrono::Utc::now().timestamp();
        let oper = if is_up { 1i64 } else { 2i64 };
        let res = self
            .storage
            .write(move |conn| {
                conn.execute(
                    "UPDATE device_interfaces SET oper_status=?1, updated_utc=?2 WHERE device_id=?3 AND if_index=?4",
                    rusqlite::params![oper, now_utc, device_id, i64::from(if_index)],
                )
                .map_err(|e| crate::error::ServerError::Storage(e.to_string()))?;
                Ok(())
            })
            .await;
        if let Err(e) = res {
            warn!("failed updating interface state from trap: {e}");
        }
    }
}

/// Runs an engine task inside `catch_unwind` and restarts automatically on panic.
pub fn spawn_supervised<F, Fut>(
    name: &'static str,
    role: &'static str,
    ctx: EngineContext,
    factory: F,
)
where
    F: Fn(EngineContext) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        loop {
            ctx.tracker.mark_running(name, role);
            let fut = factory(ctx.clone());
            let result = AssertUnwindSafe(fut).catch_unwind().await;
            match result {
                Ok(()) => {
                    warn!("supervised task '{name}' exited cleanly; restarting in 2s");
                    ctx.tracker.mark_restarting(name, false);
                }
                Err(_) => {
                    error!("supervised task '{name}' panicked! Catching unwind, logging event, and restarting.");
                    ctx.tracker.mark_restarting(name, true);
                    let msg = format!("Server task '{name}' panicked and was automatically restarted");
                    let _ = ctx
                        .storage
                        .write(move |c| {
                            insert_event_log(
                                c,
                                v1::Severity::Critical as i32,
                                "system",
                                None,
                                "Server",
                                &msg,
                            )
                        })
                        .await;
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });
}

pub async fn polling_loop(ctx: EngineContext) {
    info!("Monitoring polling scheduler started");
    ctx.refresh_device_ip_index().await;

    let mut next_due: HashMap<i64, Instant> = HashMap::new();
    let mut backoff_mult: HashMap<i64, u32> = HashMap::new();
    let mut last_if_poll: HashMap<i64, Instant> = HashMap::new();
    let mut prev_counters: HashMap<(i64, String), (CounterSample, CounterSample)> = HashMap::new();
    let mut root_notified: HashMap<i64, usize> = HashMap::new();
    let mut host_semaphores: HashMap<i64, Arc<Semaphore>> = HashMap::new();
    let mut last_live_flush = Instant::now();
    let mut pending_device_live: HashMap<i64, v1::LiveDeviceSample> = HashMap::new();
    let mut pending_service_live: Vec<v1::LiveServiceSample> = Vec::new();
    let mut pending_link_live: HashMap<i64, v1::LiveLinkSample> = HashMap::new();
    let mut pending_ts: Vec<RawMetricSample> = Vec::new();

    loop {
        let cycle_start = Instant::now();
        let snapshot = match ctx
            .storage
            .read(|conn| {
                let settings = telemetry::load_server_settings(conn, 0, "")?;
                let maps = entities::load_maps(conn)?;
                let devices = entities::load_devices(conn)?;
                let services = entities::load_services(conn)?;
                let probes = entities::load_probes(conn)?;
                let deps = entities::load_dependencies(conn)?;
                let links = entities::load_links(conn)?;
                let link_types = entities::load_link_types(conn)?;
                let rules = telemetry::load_alert_rules(conn)?;
                let maint = telemetry::load_maintenance_windows(conn)?;
                Ok((
                    settings, maps, devices, services, probes, deps, links, link_types, rules,
                    maint,
                ))
            })
            .await
        {
            Ok(s) => s,
            Err(e) => {
                warn!("scheduler snapshot read failed: {e}");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };

        let (settings, maps, devices, services, probes, deps, links, link_types, rules, maint) =
            snapshot;

        let map_by_id: HashMap<i64, v1::NetworkMap> = maps.into_iter().map(|m| (m.id, m)).collect();
        let dev_by_id: HashMap<i64, v1::Device> = devices.iter().cloned().map(|d| (d.id, d)).collect();
        let probe_by_id: HashMap<i64, v1::ProbeDefinition> =
            probes.into_iter().map(|p| (p.id, p)).collect();
        let ltype_by_id: HashMap<i64, v1::LinkType> =
            link_types.into_iter().map(|lt| (lt.id, lt)).collect();

        let current_down: HashSet<i64> = devices
            .iter()
            .filter(|d| d.aggregate_state == v1::ProbeState::Down as i32)
            .map(|d| d.id)
            .collect();
        *ctx.down_devices.write().await = current_down.clone();

        let suppression = compute_dependency_suppression(&deps, &current_down);
        for (&root_id, &count) in &suppression.affected_counts_by_root {
            if root_notified.get(&root_id).copied() != Some(count) {
                root_notified.insert(root_id, count);
                if let Some(root_dev) = dev_by_id.get(&root_id) {
                    let msg = format!(
                        "Root cause outage: {} ({}) is DOWN; {} dependent device(s) marked unreachable and suppressed",
                        root_dev.name, root_dev.address, count
                    );
                    let _ = ctx
                        .notifications
                        .dispatch(AlertDispatch {
                            severity: v1::Severity::Critical,
                            device_id: Some(root_id),
                            device_name: root_dev.name.clone(),
                            title: "Root Cause Outage".to_string(),
                            message: msg,
                            channel_ids: Vec::new(),
                        })
                        .await;
                }
            }
        }
        root_notified.retain(|id, _| suppression.affected_counts_by_root.contains_key(id));

        let global_sem = Arc::new(Semaphore::new(settings.max_concurrent_probes.max(1) as usize));
        let per_host_limit = settings.max_concurrent_per_host.max(1) as usize;
        let now_mono = Instant::now();
        let mut join_set = tokio::task::JoinSet::new();

        for svc in &services {
            if !svc.enabled {
                continue;
            }
            if suppression.suppressed_children.contains_key(&svc.device_id) {
                continue;
            }
            let Some(dev) = dev_by_id.get(&svc.device_id) else {
                continue;
            };
            let Some(probe) = probe_by_id.get(&svc.probe_id) else {
                continue;
            };

            let base_interval = svc
                .poll_interval_sec_override
                .or(dev.poll_interval_sec_override)
                .or_else(|| {
                    map_by_id
                        .get(&dev.map_id)
                        .and_then(|m| m.poll_interval_sec_override)
                })
                .unwrap_or(settings.global_poll_interval_sec)
                .max(5);

            let mult = backoff_mult.get(&dev.id).copied().unwrap_or(1);
            let effective_sec = (base_interval.saturating_mul(mult)).min(300);

            let due = next_due.entry(svc.id).or_insert_with(|| {
                // Spread initial startup polls deterministically across the interval.
                let jitter_ms = ((svc.id.unsigned_abs() * 7919) % u64::from(base_interval * 1000))
                    .min(u64::from(base_interval * 1000));
                now_mono + Duration::from_millis(jitter_ms)
            });

            if now_mono < *due {
                continue;
            }
            let jitter_extra = ((svc.id.unsigned_abs() * 131) % 750) as u64;
            *due = now_mono + Duration::from_secs(u64::from(effective_sec)) + Duration::from_millis(jitter_extra);

            let host_sem = Arc::clone(
                host_semaphores
                    .entry(dev.id)
                    .or_insert_with(|| Arc::new(Semaphore::new(per_host_limit))),
            );
            let g_sem = Arc::clone(&global_sem);
            let exec = Arc::clone(&ctx.executor);
            let storage = ctx.storage.clone();
            let tracker = Arc::clone(&ctx.tracker);
            let svc_clone = svc.clone();
            let dev_clone = dev.clone();
            let probe_clone = probe.clone();

            join_set.spawn(async move {
                let Ok(_g_permit) = g_sem.acquire_owned().await else {
                    return None;
                };
                let Ok(_h_permit) = host_sem.acquire_owned().await else {
                    return None;
                };
                let _probe_guard = tracker.acquire_probe_guard();
                let target = build_probe_target(&storage, &dev_clone).await;
                let outcome = exec
                    .execute(&target, &probe_clone, &svc_clone.interface_key)
                    .await;
                Some((svc_clone, dev_clone, probe_clone, outcome))
            });
        }

        let mut changed_devices: HashSet<i64> = HashSet::new();
        let now_utc = chrono::Utc::now().timestamp();

        while let Some(res) = join_set.join_next().await {
            let Ok(Some((svc, dev, probe, outcome))) = res else {
                continue;
            };
            let (ok, rtt_ms, val, text, force_partial) = match outcome {
                Ok(o) => (o.success, o.rtt_ms, o.value, o.text, o.force_partial),
                Err(e) => (false, 0.0, 0.0, e.to_string(), false),
            };

            let curr_state =
                v1::ProbeState::try_from(svc.state).unwrap_or(v1::ProbeState::Unknown);
            let down_thr = svc.down_threshold_override.unwrap_or(probe.down_threshold);
            let up_thr = svc.up_threshold_override.unwrap_or(probe.up_threshold);
            let deb = advance_probe_state(
                curr_state,
                svc.consecutive_failures,
                svc.consecutive_successes,
                down_thr,
                up_thr,
                ok,
                force_partial,
            );

            let state_num = deb.new_state as i32;
            let txt_clone = text.clone();
            let svc_id = svc.id;
            let _ = ctx
                .storage
                .write(move |c| {
                    c.execute(
                        "UPDATE device_services SET state=?1, consecutive_failures=?2, consecutive_successes=?3, last_value=?4, last_text=?5, last_rtt_ms=?6, last_check_utc=?7 WHERE id=?8",
                        rusqlite::params![
                            state_num,
                            i64::from(deb.consecutive_failures),
                            i64::from(deb.consecutive_successes),
                            val,
                            txt_clone,
                            rtt_ms,
                            now_utc,
                            svc_id
                        ],
                    )
                    .map_err(|e| crate::error::ServerError::Storage(e.to_string()))?;
                    Ok(())
                })
                .await;

            pending_ts.push(RawMetricSample {
                source_kind: SOURCE_SERVICE_VALUE,
                source_id: svc.id,
                ts_utc: now_utc,
                val,
            });
            if ok {
                pending_ts.push(RawMetricSample {
                    source_kind: SOURCE_SERVICE_RTT,
                    source_id: svc.id,
                    ts_utc: now_utc,
                    val: rtt_ms,
                });
            }

            pending_service_live.push(v1::LiveServiceSample {
                service_id: svc.id,
                device_id: dev.id,
                state: state_num,
                value: val,
                text: text.clone(),
                rtt_ms,
                checked_utc: now_utc,
            });
            changed_devices.insert(dev.id);

            if deb.transitioned {
                // Outage record lifecycle
                if deb.new_state == v1::ProbeState::Down {
                    let _ = open_outage(&ctx, dev.id, svc.id, now_utc, false).await;
                } else if curr_state == v1::ProbeState::Down && deb.new_state == v1::ProbeState::Up {
                    let _ = close_outage(&ctx, svc.id, now_utc).await;
                }

                // Maintenance suppression still records event log, only silences notifications
                let silenced = is_in_maintenance(&maint, dev.id, dev.map_id, now_utc);
                evaluate_alert_rules(
                    &ctx,
                    &rules,
                    &dev,
                    &svc,
                    &probe,
                    deb.new_state,
                    val,
                    &text,
                    silenced,
                    true,
                )
                .await;
            } else {
                let silenced = is_in_maintenance(&maint, dev.id, dev.map_id, now_utc);
                evaluate_alert_rules(
                    &ctx,
                    &rules,
                    &dev,
                    &svc,
                    &probe,
                    deb.new_state,
                    val,
                    &text,
                    silenced,
                    false,
                )
                .await;
            }

            let entry = backoff_mult.entry(dev.id).or_insert(1);
            if ok {
                *entry = 1;
            } else if deb.new_state == v1::ProbeState::Down {
                *entry = (*entry * 2).min(4);
            }
        }

        // Recompute aggregate device states (including dependency suppression)
        if !changed_devices.is_empty() || !suppression.suppressed_children.is_empty() {
            if let Ok(fresh_services) = ctx.storage.read(|c| entities::load_services(c)).await {
                let mut by_dev: HashMap<i64, Vec<v1::ProbeState>> = HashMap::new();
                let mut rtt_by_dev: HashMap<i64, f64> = HashMap::new();
                for s in fresh_services {
                    let st = v1::ProbeState::try_from(s.state).unwrap_or(v1::ProbeState::Unknown);
                    by_dev.entry(s.device_id).or_default().push(st);
                    if s.last_rtt_ms > 0.0 {
                        rtt_by_dev.entry(s.device_id).or_insert(s.last_rtt_ms);
                    }
                }
                for dev in &devices {
                    let suppressed = suppression.suppressed_children.contains_key(&dev.id);
                    let states = by_dev.get(&dev.id).map(Vec::as_slice).unwrap_or(&[]);
                    let agg = aggregate_device_state(states, suppressed) as i32;
                    let rtt = rtt_by_dev.get(&dev.id).copied().unwrap_or(dev.last_rtt_ms);
                    if agg != dev.aggregate_state || suppressed {
                        let dev_id = dev.id;
                        let seen = if agg == v1::ProbeState::Up as i32 {
                            now_utc
                        } else {
                            dev.last_seen_utc
                        };
                        let _ = ctx
                            .storage
                            .write(move |c| {
                                c.execute(
                                    "UPDATE devices SET aggregate_state=?1, last_rtt_ms=?2, last_seen_utc=?3 WHERE id=?4",
                                    rusqlite::params![agg, rtt, seen, dev_id],
                                )
                                .map_err(|e| crate::error::ServerError::Storage(e.to_string()))?;
                                Ok(())
                            })
                            .await;
                        pending_device_live.insert(
                            dev.id,
                            v1::LiveDeviceSample {
                                device_id: dev.id,
                                aggregate_state: agg,
                                last_rtt_ms: rtt,
                                last_seen_utc: seen,
                                sys_uptime_cs: dev.sys_uptime_cs,
                            },
                        );
                    }
                }
            }
        }

        // Poll SNMP interface counters and update mastered link status/utilization
        poll_links_and_interfaces(
            &ctx,
            &devices,
            &links,
            &ltype_by_id,
            &suppression.suppressed_children,
            &mut last_if_poll,
            &mut prev_counters,
            &mut pending_link_live,
            &mut pending_ts,
            &settings,
        )
        .await;

        // Flush batch time-series writes + ~1Hz live metrics broadcast
        if !pending_ts.is_empty() {
            let batch = std::mem::take(&mut pending_ts);
            let _ = ctx
                .storage
                .write(move |c| insert_raw_samples_batch(c, &batch))
                .await;
        }

        if last_live_flush.elapsed() >= Duration::from_secs(1)
            && (!pending_device_live.is_empty()
                || !pending_service_live.is_empty()
                || !pending_link_live.is_empty())
        {
            let batch = v1::LiveMetricsBatch {
                devices: pending_device_live.drain().map(|(_, v)| v).collect(),
                services: std::mem::take(&mut pending_service_live),
                links: pending_link_live.drain().map(|(_, v)| v).collect(),
            };
            ctx.broadcast(ServerBroadcastEvent::LiveMetrics(batch));
            last_live_flush = Instant::now();
        }

        let elapsed = cycle_start.elapsed();
        ctx.tracker.record_poll_cycle(elapsed.as_millis() as u64);
        if elapsed < Duration::from_millis(500) {
            tokio::time::sleep(Duration::from_millis(500) - elapsed).await;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn poll_links_and_interfaces(
    ctx: &EngineContext,
    devices: &[v1::Device],
    links: &[v1::NetworkLink],
    ltype_by_id: &HashMap<i64, v1::LinkType>,
    suppressed_devices: &HashMap<i64, i64>,
    last_if_poll: &mut HashMap<i64, Instant>,
    prev_counters: &mut HashMap<(i64, String), (CounterSample, CounterSample)>,
    pending_link_live: &mut HashMap<i64, v1::LiveLinkSample>,
    pending_ts: &mut Vec<RawMetricSample>,
    settings: &v1::ServerSettings,
) {
    let mut devices_to_poll: HashSet<i64> = HashSet::new();
    for l in links {
        if l.mastering_type == v1::MasteringType::SnmpIfTraffic as i32 && l.master_device_id > 0 {
            devices_to_poll.insert(l.master_device_id);
        }
    }
    let dev_map: HashMap<i64, &v1::Device> = devices.iter().map(|d| (d.id, d)).collect();
    let now_mono = Instant::now();
    let now_utc = chrono::Utc::now().timestamp();

    for dev_id in devices_to_poll {
        if suppressed_devices.contains_key(&dev_id) {
            continue;
        }
        let Some(&dev) = dev_map.get(&dev_id) else {
            continue;
        };
        let last = last_if_poll.get(&dev_id).copied();
        if last.map_or(false, |t| now_mono.saturating_duration_since(t) < Duration::from_secs(15)) {
            continue;
        }
        last_if_poll.insert(dev_id, now_mono);

        let target = build_probe_target(&ctx.storage, dev).await;
        let Ok(ip) = crate::probe::network::resolve_target_ip(&ctx.executor, &target).await else {
            continue;
        };
        let snmp_target = SnmpTarget {
            addr: SocketAddr::new(ip, target.snmp_port),
            timeout: target.snmp_timeout,
            retries: target.snmp_retries,
            credential: target.snmp_credential,
        };

        let Ok(uptime_oid) = parse_oid("1.3.6.1.2.1.1.3.0") else {
            continue;
        };
        let uptime_cs = ctx
            .snmp
            .get(&snmp_target, &[uptime_oid])
            .await
            .ok()
            .and_then(|vbs| vbs.first().and_then(|v| v.value.as_u64()));

        let Ok(if_descr_oid) = parse_oid("1.3.6.1.2.1.2.2.1.2") else {
            continue;
        };
        let Ok(if_name_oid) = parse_oid("1.3.6.1.2.1.31.1.1.1.1") else {
            continue;
        };
        let Ok(if_oper_oid) = parse_oid("1.3.6.1.2.1.2.2.1.8") else {
            continue;
        };
        let Ok(if_admin_oid) = parse_oid("1.3.6.1.2.1.2.2.1.7") else {
            continue;
        };
        let Ok(if_speed_oid) = parse_oid("1.3.6.1.2.1.31.1.1.1.15") else {
            continue;
        };
        let Ok(if_hc_in_oid) = parse_oid("1.3.6.1.2.1.31.1.1.1.6") else {
            continue;
        };
        let Ok(if_hc_out_oid) = parse_oid("1.3.6.1.2.1.31.1.1.1.10") else {
            continue;
        };
        let Ok(if_in32_oid) = parse_oid("1.3.6.1.2.1.2.2.1.10") else {
            continue;
        };
        let Ok(if_out32_oid) = parse_oid("1.3.6.1.2.1.2.2.1.16") else {
            continue;
        };

        let descr_walk = ctx.snmp.walk(&snmp_target, &if_descr_oid).await.unwrap_or_default();
        if descr_walk.is_empty() {
            continue;
        }
        let name_walk = ctx.snmp.walk(&snmp_target, &if_name_oid).await.unwrap_or_default();
        let oper_walk = ctx.snmp.walk(&snmp_target, &if_oper_oid).await.unwrap_or_default();
        let admin_walk = ctx.snmp.walk(&snmp_target, &if_admin_oid).await.unwrap_or_default();
        let high_speed_walk = ctx.snmp.walk(&snmp_target, &if_speed_oid).await.unwrap_or_default();
        let hc_in_walk = ctx.snmp.walk(&snmp_target, &if_hc_in_oid).await.unwrap_or_default();
        let hc_out_walk = ctx.snmp.walk(&snmp_target, &if_hc_out_oid).await.unwrap_or_default();
        let in32_walk = if hc_in_walk.is_empty() {
            ctx.snmp.walk(&snmp_target, &if_in32_oid).await.unwrap_or_default()
        } else {
            Vec::new()
        };
        let out32_walk = if hc_out_walk.is_empty() {
            ctx.snmp.walk(&snmp_target, &if_out32_oid).await.unwrap_or_default()
        } else {
            Vec::new()
        };

        let to_u64_map = |vbs: Vec<crate::snmp::Varbind>| -> HashMap<u32, u64> {
            vbs.into_iter()
                .filter_map(|v| v.oid.last().copied().and_then(|idx| v.value.as_u64().map(|val| (idx, val))))
                .collect()
        };
        let to_str_map = |vbs: Vec<crate::snmp::Varbind>| -> HashMap<u32, String> {
            vbs.into_iter()
                .filter_map(|v| v.oid.last().copied().map(|idx| (idx, v.value.as_string_lossy())))
                .collect()
        };

        let descrs = to_str_map(descr_walk);
        let names = to_str_map(name_walk);
        let opers = to_u64_map(oper_walk);
        let admins = to_u64_map(admin_walk);
        let speeds_mbps = to_u64_map(high_speed_walk);
        let hc_in = to_u64_map(hc_in_walk);
        let hc_out = to_u64_map(hc_out_walk);
        let in32 = to_u64_map(in32_walk);
        let out32 = to_u64_map(out32_walk);

        let mut upserts = Vec::new();
        let mut by_key_stats: HashMap<String, (bool, u64, f64, f64)> = HashMap::new();

        for (idx, descr) in &descrs {
            let name = names.get(idx).cloned().unwrap_or_default();
            let stable_key = stable_interface_key(&name, descr, *idx);
            let oper = opers.get(idx).copied().unwrap_or(2) as u32;
            let admin = admins.get(idx).copied().unwrap_or(1) as u32;
            let speed_bps = speeds_mbps.get(idx).copied().unwrap_or(0).saturating_mul(1_000_000);

            let (in_val, is_64) = match hc_in.get(idx) {
                Some(&v) => (v, true),
                None => (in32.get(idx).copied().unwrap_or(0), false),
            };
            let out_val = match hc_out.get(idx) {
                Some(&v) => v,
                None => out32.get(idx).copied().unwrap_or(0),
            };

            let curr_in = CounterSample {
                value: in_val,
                is_64bit: is_64,
                uptime_cs,
                at: now_mono,
            };
            let curr_out = CounterSample {
                value: out_val,
                is_64bit: is_64,
                uptime_cs,
                at: now_mono,
            };
            let prev = prev_counters.insert((dev_id, stable_key.clone()), (curr_in, curr_out));
            let in_bps = compute_counter_bps(prev.map(|p| p.0), curr_in, speed_bps).unwrap_or(0.0);
            let out_bps = compute_counter_bps(prev.map(|p| p.1), curr_out, speed_bps).unwrap_or(0.0);

            by_key_stats.insert(stable_key.clone(), (oper == 1, speed_bps, in_bps, out_bps));
            upserts.push(v1::DeviceInterface {
                id: 0,
                device_id: dev_id,
                if_index: *idx,
                stable_key,
                name: if name.is_empty() { descr.clone() } else { name },
                descr: descr.clone(),
                alias: String::new(),
                mac: String::new(),
                speed_bps,
                admin_status: admin,
                oper_status: oper,
                in_bps,
                out_bps,
                updated_utc: now_utc,
            });
        }

        let _ = ctx
            .storage
            .write(move |c| upsert_device_interfaces(c, dev_id, &upserts))
            .await;

        let mut flap_guard = ctx.link_flapping.write().await;
        for link in links.iter().filter(|l| l.master_device_id == dev_id) {
            let Some(&(oper_up, if_speed, in_bps, out_bps)) = by_key_stats.get(&link.interface_key) else {
                continue;
            };
            let type_speed = ltype_by_id
                .get(&link.link_type_id)
                .map_or(1_000_000_000, |lt| lt.max_speed_bps);
            let max_speed = link
                .max_speed_bps_override
                .unwrap_or_else(|| if if_speed > 0 { if_speed } else { type_speed });

            let remote_id = if link.src_device_id == dev_id {
                link.dst_device_id
            } else {
                link.src_device_id
            };
            let remote_up = dev_map
                .get(&remote_id)
                .map(|d| d.aggregate_state == v1::ProbeState::Up as i32);

            let tracker = flap_guard.entry(link.id).or_default();
            tracker.refresh(
                now_mono,
                settings.flapping_changes,
                Duration::from_secs(u64::from(settings.flapping_window_min) * 60),
            );
            let (mut status, in_util, out_util) = compute_link_status_and_util(
                Some(oper_up),
                remote_up,
                None,
                tracker.is_flapping,
                in_bps,
                out_bps,
                max_speed,
            );
            if (status as i32) != link.status {
                let became_flapping = tracker.record_change(
                    now_mono,
                    settings.flapping_changes,
                    Duration::from_secs(u64::from(settings.flapping_window_min) * 60),
                );
                if tracker.is_flapping {
                    status = v1::LinkStatus::Flapping;
                }
                if became_flapping && !tracker.notified_current_flap {
                    tracker.notified_current_flap = true;
                    let _ = ctx
                        .notifications
                        .dispatch(AlertDispatch {
                            severity: v1::Severity::Warning,
                            device_id: Some(dev_id),
                            device_name: dev.name.clone(),
                            title: "Link Flapping".to_string(),
                            message: format!(
                                "Link #{} ({}) on {} is flapping (>{} changes in {} min)",
                                link.id,
                                link.interface_key,
                                dev.name,
                                settings.flapping_changes,
                                settings.flapping_window_min
                            ),
                            channel_ids: Vec::new(),
                        })
                        .await;
                }
            }

            let status_num = status as i32;
            let lid = link.id;
            let _ = ctx
                .storage
                .write(move |c| {
                    c.execute(
                        "UPDATE links SET status=?1, in_bps=?2, out_bps=?3, in_utilization_pct=?4, out_utilization_pct=?5 WHERE id=?6",
                        rusqlite::params![status_num, in_bps, out_bps, in_util, out_util, lid],
                    )
                    .map_err(|e| crate::error::ServerError::Storage(e.to_string()))?;
                    Ok(())
                })
                .await;

            if link.history_enabled {
                pending_ts.push(RawMetricSample {
                    source_kind: SOURCE_LINK_IN_BPS,
                    source_id: link.id,
                    ts_utc: now_utc,
                    val: in_bps,
                });
                pending_ts.push(RawMetricSample {
                    source_kind: SOURCE_LINK_OUT_BPS,
                    source_id: link.id,
                    ts_utc: now_utc,
                    val: out_bps,
                });
                pending_ts.push(RawMetricSample {
                    source_kind: SOURCE_LINK_UTIL_PCT,
                    source_id: link.id,
                    ts_utc: now_utc,
                    val: in_util.max(out_util),
                });
            }

            pending_link_live.insert(
                link.id,
                v1::LiveLinkSample {
                    link_id: link.id,
                    status: status_num,
                    in_bps,
                    out_bps,
                    in_util_pct: in_util,
                    out_util_pct: out_util,
                },
            );
        }
    }
}

pub async fn rollup_and_retention_loop(ctx: EngineContext) {
    let mut last_vacuum = Instant::now();
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let now_utc = chrono::Utc::now().timestamp();
        let do_vacuum = last_vacuum.elapsed() >= Duration::from_secs(86400);
        if do_vacuum {
            last_vacuum = Instant::now();
        }
        let cfg = ctx.config.clone();
        let res = ctx
            .storage
            .write(move |conn| {
                let s = telemetry::load_server_settings(conn, 0, "").unwrap_or(v1::ServerSettings {
                    retention_raw_hours: cfg.retention_raw_hours,
                    retention_rollup_days: cfg.retention_rollup_days,
                    retention_events_days: cfg.retention_events_days,
                    retention_syslog_days: cfg.retention_syslog_days,
                    retention_audit_days: cfg.retention_audit_days,
                    ..Default::default()
                });
                run_rollups(conn, now_utc)?;
                enforce_retention_and_vacuum(
                    conn,
                    now_utc,
                    s.retention_raw_hours,
                    s.retention_rollup_days,
                    s.retention_events_days,
                    s.retention_syslog_days,
                    s.retention_audit_days,
                    do_vacuum,
                )?;
                Ok(())
            })
            .await;
        if let Err(e) = res {
            warn!("rollup/retention job error: {e}");
        }
    }
}

pub async fn build_probe_target(storage: &StorageHandle, dev: &v1::Device) -> ProbeTarget {
    let snmp_cfg = dev.snmp.clone().unwrap_or_default();
    let secrets_box = storage.secrets().clone();
    let dev_id = dev.id;
    let sec = storage
        .read(move |c| load_device_secrets(c, &secrets_box, dev_id))
        .await
        .ok();
    let (comm, auth_pass, priv_pass) = match sec {
        Some(s) => (s.community, s.auth_password, s.priv_password),
        None => (String::new(), String::new(), String::new()),
    };
    let cred = match v1::SnmpVersion::try_from(snmp_cfg.version).unwrap_or(v1::SnmpVersion::V2c) {
        v1::SnmpVersion::V1 => ResolvedCredential::V1 { community: comm },
        v1::SnmpVersion::V3 => ResolvedCredential::V3(UsmCredentials {
            username: snmp_cfg.username,
            auth_proto: match v1::SnmpAuthProtocol::try_from(snmp_cfg.auth_protocol) {
                Ok(v1::SnmpAuthProtocol::SnmpAuthMd5) => AuthProto::Md5,
                Ok(v1::SnmpAuthProtocol::SnmpAuthSha1) => AuthProto::Sha1,
                _ => AuthProto::None,
            },
            auth_password: auth_pass,
            priv_proto: match v1::SnmpPrivProtocol::try_from(snmp_cfg.priv_protocol) {
                Ok(v1::SnmpPrivProtocol::SnmpPrivDes) => PrivProto::Des,
                Ok(v1::SnmpPrivProtocol::SnmpPrivAes128) => PrivProto::Aes128,
                _ => PrivProto::None,
            },
            priv_password: priv_pass,
        }),
        _ => ResolvedCredential::V2c { community: comm },
    };

    ProbeTarget {
        device_id: dev.id,
        host: dev.address.clone(),
        resolved_ip: dev.address.parse::<IpAddr>().ok(),
        snmp_port: if snmp_cfg.port == 0 { 161 } else { snmp_cfg.port as u16 },
        snmp_timeout: Duration::from_millis(u64::from(snmp_cfg.timeout_ms.max(300))),
        snmp_retries: snmp_cfg.retries,
        snmp_credential: cred,
    }
}

async fn open_outage(
    ctx: &EngineContext,
    device_id: i64,
    service_id: i64,
    started_utc: i64,
    suppressed: bool,
) -> Result<(), crate::error::ServerError> {
    let rec = ctx
        .storage
        .write(move |conn| {
            conn.execute(
                "INSERT INTO outages (device_id, service_id, started_utc, suppressed_by_dependency) VALUES (?1,?2,?3,?4)",
                rusqlite::params![device_id, service_id, started_utc, i64::from(suppressed)],
            )
            .map_err(|e| crate::error::ServerError::Storage(e.to_string()))?;
            let id = conn.last_insert_rowid();
            let (dname, sname): (String, String) = conn
                .query_row(
                    "SELECT COALESCE((SELECT name FROM devices WHERE id=?1),''), COALESCE((SELECT name FROM device_services WHERE id=?2),'')",
                    rusqlite::params![device_id, service_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap_or_default();
            Ok(v1::OutageRecord {
                id,
                device_id,
                device_name: dname,
                service_id,
                service_name: sname,
                started_utc,
                ended_utc: None,
                root_cause_note: String::new(),
                suppressed_by_dependency: suppressed,
            })
        })
        .await?;
    ctx.active_outages.write().await.insert(service_id, rec.id);
    ctx.broadcast(ServerBroadcastEvent::Outage(rec));
    Ok(())
}

async fn close_outage(
    ctx: &EngineContext,
    service_id: i64,
    ended_utc: i64,
) -> Result<(), crate::error::ServerError> {
    let outage_id = ctx.active_outages.write().await.remove(&service_id);
    if let Some(oid) = outage_id {
        let rec = ctx
            .storage
            .write(move |conn| {
                conn.execute(
                    "UPDATE outages SET ended_utc=?1 WHERE id=?2",
                    rusqlite::params![ended_utc, oid],
                )
                .map_err(|e| crate::error::ServerError::Storage(e.to_string()))?;
                conn.query_row(
                    "SELECT o.id, o.device_id, COALESCE(d.name,''), o.service_id, COALESCE(s.name,''), o.started_utc, o.ended_utc, o.root_cause_note, o.suppressed_by_dependency
                     FROM outages o LEFT JOIN devices d ON o.device_id=d.id LEFT JOIN device_services s ON o.service_id=s.id WHERE o.id=?1",
                    rusqlite::params![oid],
                    |r| {
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
                    },
                )
                .map_err(|e| crate::error::ServerError::Storage(e.to_string()))
            })
            .await?;
        ctx.broadcast(ServerBroadcastEvent::Outage(rec));
    }
    Ok(())
}

pub fn is_in_maintenance(
    windows: &[v1::MaintenanceWindow],
    device_id: i64,
    map_id: i64,
    now_utc: i64,
) -> bool {
    let Some(dt) = chrono::DateTime::from_timestamp(now_utc, 0) else {
        return false;
    };
    let weekday_bit = 1u32 << dt.weekday().num_days_from_monday();
    let tod_sec = i64::from(dt.num_seconds_from_midnight());

    for mw in windows {
        if !mw.enabled {
            continue;
        }
        let target_matches = match (mw.device_id, mw.map_id) {
            (Some(d), _) => d == device_id,
            (None, Some(m)) => m == map_id,
            (None, None) => true,
        };
        if !target_matches {
            continue;
        }
        if !mw.recurring_weekly {
            if now_utc >= mw.start_utc && now_utc <= mw.end_utc {
                return true;
            }
        } else if (mw.weekday_mask & weekday_bit) != 0 {
            let start_tod = mw.start_utc.rem_euclid(86400);
            let end_tod = mw.end_utc.rem_euclid(86400);
            if start_tod <= end_tod {
                if tod_sec >= start_tod && tod_sec <= end_tod {
                    return true;
                }
            } else if tod_sec >= start_tod || tod_sec <= end_tod {
                return true;
            }
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
async fn evaluate_alert_rules(
    ctx: &EngineContext,
    rules: &[v1::AlertRule],
    dev: &v1::Device,
    svc: &v1::DeviceService,
    probe: &v1::ProbeDefinition,
    state: v1::ProbeState,
    value: f64,
    text: &str,
    silenced_by_maintenance: bool,
    transitioned: bool,
) {
    let mut matched = false;
    for rule in rules {
        if !rule.enabled {
            continue;
        }
        if rule.device_id.map_or(false, |id| id != dev.id) {
            continue;
        }
        if rule.probe_id.map_or(false, |id| id != probe.id) {
            continue;
        }
        let state_hit = rule.trigger_on_state_change && transitioned;
        let threshold_hit = rule.value_threshold.map_or(false, |thr| {
            if rule.threshold_greater_than {
                value >= thr
            } else {
                value <= thr
            }
        });
        if state_hit || threshold_hit {
            matched = true;
            let sev = v1::Severity::try_from(rule.severity).unwrap_or(v1::Severity::Warning);
            let msg = format!(
                "{} / {} state={:?} value={:.2} ({})",
                dev.name, svc.name, state, value, text
            );
            if silenced_by_maintenance {
                let dname = dev.name.clone();
                let did = dev.id;
                let _ = ctx
                    .storage
                    .write(move |c| {
                        insert_event_log(
                            c,
                            sev as i32,
                            "maintenance_suppressed",
                            Some(did),
                            &dname,
                            &msg,
                        )
                    })
                    .await;
            } else {
                let _ = ctx
                    .notifications
                    .dispatch(AlertDispatch {
                        severity: sev,
                        device_id: Some(dev.id),
                        device_name: dev.name.clone(),
                        title: format!("{}: {}", rule.name, dev.name),
                        message: msg,
                        channel_ids: rule.notification_channel_ids.clone(),
                    })
                    .await;
            }
        }
    }
    if !matched && transitioned {
        let sev = match state {
            v1::ProbeState::Down => v1::Severity::Critical,
            v1::ProbeState::Partial => v1::Severity::Warning,
            _ => v1::Severity::Info,
        };
        let msg = format!("{} service '{}' changed to {:?}: {}", dev.name, svc.name, state, text);
        if silenced_by_maintenance {
            let dname = dev.name.clone();
            let did = dev.id;
            let _ = ctx
                .storage
                .write(move |c| {
                    insert_event_log(c, sev as i32, "maintenance_suppressed", Some(did), &dname, &msg)
                })
                .await;
        } else {
            let _ = ctx
                .notifications
                .dispatch(AlertDispatch {
                    severity: sev,
                    device_id: Some(dev.id),
                    device_name: dev.name.clone(),
                    title: format!("{} ({:?})", dev.name, state),
                    message: msg,
                    channel_ids: Vec::new(),
                })
                .await;
        }
    }
}
