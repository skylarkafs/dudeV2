pub mod mndp;

use crate::engine::{EngineContext, ServerBroadcastEvent};
use crate::error::ServerError;
use crate::snmp::ber::{parse_oid, SnmpValue, Varbind};
use crate::snmp::{AuthProto, PrivProto, ResolvedCredential, SnmpTarget, UsmCredentials};
use crate::storage::entities;
use ipnet::IpNet;
use mndp::MndpCache;
use nextdude_common::proto::v1;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, Semaphore};

#[derive(Debug, Clone)]
pub struct DiscoveryJobResult {
    pub job_id: String,
    pub candidates: Vec<v1::DiscoveryCandidateDevice>,
    pub candidate_links: Vec<v1::DiscoveryCandidateLink>,
    pub cancelled: Arc<AtomicBool>,
}

#[derive(Clone, Default)]
pub struct DiscoveryManager {
    pub jobs: Arc<RwLock<HashMap<String, DiscoveryJobResult>>>,
    pub mndp_cache: MndpCache,
}

#[derive(Debug, Clone)]
pub struct DiscoveredNeighbor {
    pub remote_sys_name: String,
    pub remote_port: String,
    pub remote_ip: Option<String>,
    pub protocol: String,
}

pub fn decode_lldp_neighbors(
    rem_sys_names: &[Varbind],
    rem_port_ids: &[Varbind],
) -> Vec<DiscoveredNeighbor> {
    // lldpRemSysName OID suffix: <timeMark>.<localPortNum>.<remIndex>
    let mut port_by_key: HashMap<Vec<u32>, String> = HashMap::new();
    for vb in rem_port_ids {
        if vb.oid.len() >= 3 {
            let key = vb.oid[vb.oid.len() - 3..].to_vec();
            port_by_key.insert(key, vb.value.as_string_lossy());
        }
    }
    let mut out = Vec::new();
    for vb in rem_sys_names {
        if vb.oid.len() >= 3 {
            let key = vb.oid[vb.oid.len() - 3..].to_vec();
            let remote_sys_name = vb.value.as_string_lossy();
            if !remote_sys_name.is_empty() {
                out.push(DiscoveredNeighbor {
                    remote_sys_name,
                    remote_port: port_by_key.get(&key).cloned().unwrap_or_default(),
                    remote_ip: None,
                    protocol: "LLDP".to_string(),
                });
            }
        }
    }
    out
}

pub fn decode_cdp_neighbors(
    device_ids: &[Varbind],
    device_ports: &[Varbind],
    addresses: &[Varbind],
) -> Vec<DiscoveredNeighbor> {
    // cdpCache* OID suffix: <ifIndex>.<deviceIndex>
    let mut ports: HashMap<(u32, u32), String> = HashMap::new();
    for vb in device_ports {
        if vb.oid.len() >= 2 {
            let k = (vb.oid[vb.oid.len() - 2], vb.oid[vb.oid.len() - 1]);
            ports.insert(k, vb.value.as_string_lossy());
        }
    }
    let mut addrs: HashMap<(u32, u32), String> = HashMap::new();
    for vb in addresses {
        if vb.oid.len() >= 2 {
            let k = (vb.oid[vb.oid.len() - 2], vb.oid[vb.oid.len() - 1]);
            if let SnmpValue::OctetString(ref b) = vb.value {
                if b.len() == 4 {
                    addrs.insert(k, format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3]));
                }
            }
        }
    }
    let mut out = Vec::new();
    for vb in device_ids {
        if vb.oid.len() >= 2 {
            let k = (vb.oid[vb.oid.len() - 2], vb.oid[vb.oid.len() - 1]);
            let name = vb.value.as_string_lossy();
            if !name.is_empty() {
                out.push(DiscoveredNeighbor {
                    remote_sys_name: name,
                    remote_port: ports.get(&k).cloned().unwrap_or_default(),
                    remote_ip: addrs.get(&k).cloned(),
                    protocol: "CDP".to_string(),
                });
            }
        }
    }
    out
}

impl DiscoveryManager {
    pub async fn start_job(
        &self,
        ctx: EngineContext,
        req: v1::StartDiscoveryRequest,
    ) -> Result<String, ServerError> {
        let mut hosts: Vec<IpAddr> = Vec::new();
        for cidr_str in &req.cidrs {
            let net: IpNet = nextdude_common::validation::validate_cidr(cidr_str)?;
            for ip in net.hosts().take(4096) {
                hosts.push(ip);
            }
            if hosts.len() >= 8192 {
                break;
            }
        }
        if hosts.is_empty() && !req.include_mndp {
            return Err(ServerError::Config("no hosts in CIDR range".to_string()));
        }

        let job_id = format!("disc-{}", chrono::Utc::now().timestamp_millis());
        let cancelled = Arc::new(AtomicBool::new(false));
        self.jobs.write().await.insert(
            job_id.clone(),
            DiscoveryJobResult {
                job_id: job_id.clone(),
                candidates: Vec::new(),
                candidate_links: Vec::new(),
                cancelled: Arc::clone(&cancelled),
            },
        );

        let mgr = self.clone();
        let job_id_clone = job_id.clone();
        tokio::spawn(async move {
            mgr.run_discovery_job(ctx, job_id_clone, hosts, req, cancelled)
                .await;
        });

        Ok(job_id)
    }

    pub async fn cancel_job(&self, job_id: &str) {
        if let Some(j) = self.jobs.read().await.get(job_id) {
            j.cancelled.store(true, Ordering::Relaxed);
        }
    }

    async fn run_discovery_job(
        &self,
        ctx: EngineContext,
        job_id: String,
        hosts: Vec<IpAddr>,
        req: v1::StartDiscoveryRequest,
        cancelled: Arc<AtomicBool>,
    ) {
        let _discovery_guard = ctx.tracker.acquire_discovery_guard();
        let existing_devices = ctx
            .storage
            .read(|c| entities::load_devices(c))
            .await
            .unwrap_or_default();

        let existing_ips: HashMap<String, i64> = existing_devices
            .iter()
            .flat_map(|d| {
                std::iter::once((d.address.clone(), d.id))
                    .chain(d.extra_ips.iter().map(move |e| (e.clone(), d.id)))
            })
            .collect();
        let existing_names: HashMap<String, i64> = existing_devices
            .iter()
            .map(|d| (d.name.to_ascii_lowercase(), d.id))
            .collect();
        let existing_macs: HashMap<String, i64> = existing_devices
            .iter()
            .flat_map(|d| {
                d.mac_addresses
                    .iter()
                    .map(move |m| (m.to_ascii_uppercase(), d.id))
            })
            .collect();

        let concurrency = req.concurrency.clamp(1, 128) as usize;
        let rate_per_sec = req.rate_limit_per_sec.clamp(1, 500);
        let pace_delay = Duration::from_millis(u64::from(1000 / rate_per_sec.max(1)));
        let total = hosts.len() as u32;
        let sem = Arc::new(Semaphore::new(concurrency));

        let resolved_creds: Vec<ResolvedCredential> = req
            .credentials
            .iter()
            .map(proto_cred_to_resolved)
            .collect();

        let mut candidates: Vec<v1::DiscoveryCandidateDevice> = Vec::new();
        let mut raw_neighbors: Vec<(String, Vec<DiscoveredNeighbor>)> = Vec::new();
        let mut scanned = 0u32;
        let mut alive = 0u32;

        for chunk in hosts.chunks(concurrency) {
            if cancelled.load(Ordering::Relaxed) {
                break;
            }
            let mut join_set = tokio::task::JoinSet::new();
            for &ip in chunk {
                let permit = Arc::clone(&sem);
                let exec = Arc::clone(&ctx.executor);
                let creds = resolved_creds.clone();
                let profiles = Arc::clone(&ctx.profiles);
                let include_lldp_cdp = req.include_lldp_cdp;
                tokio::time::sleep(pace_delay).await;
                join_set.spawn(async move {
                    let Ok(_p) = permit.acquire_owned().await else {
                        return None;
                    };
                    if exec.icmp.ping(ip, Duration::from_millis(1200)).await.is_err() {
                        return Some((ip, false, None, Vec::new()));
                    }
                    let (cand, neighs) =
                        inspect_alive_host(&exec, &profiles, ip, &creds, include_lldp_cdp).await;
                    Some((ip, true, cand, neighs))
                });
            }

            while let Some(res) = join_set.join_next().await {
                scanned += 1;
                if let Ok(Some((ip, is_alive, cand_opt, neighs))) = res {
                    if is_alive {
                        alive += 1;
                        if let Some(mut cand) = cand_opt {
                            let dup_id = existing_ips
                                .get(&cand.ip)
                                .or_else(|| existing_names.get(&cand.sys_name.to_ascii_lowercase()))
                                .or_else(|| {
                                    cand.macs
                                        .iter()
                                        .find_map(|m| existing_macs.get(&m.to_ascii_uppercase()))
                                })
                                .copied();
                            cand.duplicate_existing = dup_id.is_some();
                            cand.existing_device_id = dup_id;
                            let tid = cand.temp_id.clone();
                            candidates.push(cand);
                            if !neighs.is_empty() {
                                raw_neighbors.push((tid, neighs));
                            }
                        } else {
                            let ip_str = ip.to_string();
                            let dup_id = existing_ips.get(&ip_str).copied();
                            candidates.push(v1::DiscoveryCandidateDevice {
                                temp_id: format!("ip:{ip_str}"),
                                ip: ip_str.clone(),
                                sys_name: ip_str,
                                sys_descr: String::new(),
                                sys_object_id: String::new(),
                                vendor: String::new(),
                                model: String::new(),
                                macs: Vec::new(),
                                duplicate_existing: dup_id.is_some(),
                                existing_device_id: dup_id,
                                matched_cred_index: 0,
                            });
                        }
                    }
                }
            }

            ctx.broadcast(ServerBroadcastEvent::DiscoveryProgress(
                v1::DiscoveryProgressEvent {
                    job_id: job_id.clone(),
                    total_hosts: total,
                    scanned_hosts: scanned,
                    alive_hosts: alive,
                    finished: false,
                    status_text: format!("Scanned {scanned}/{total} hosts ({alive} alive)"),
                    candidates: candidates.clone(),
                    candidate_links: Vec::new(),
                },
            ));
        }

        // Merge MNDP neighbors if enabled
        let mut candidate_links: Vec<v1::DiscoveryCandidateLink> = Vec::new();
        if req.include_mndp {
            let mndp = self.mndp_cache.read().await;
            for (_, n) in mndp.iter() {
                let ip_s = n
                    .ipv4
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| n.source_ip.to_string());
                if !candidates.iter().any(|c| c.ip == ip_s) {
                    let dup_id = existing_ips
                        .get(&ip_s)
                        .or_else(|| existing_macs.get(&n.mac_address.to_ascii_uppercase()))
                        .copied();
                    candidates.push(v1::DiscoveryCandidateDevice {
                        temp_id: format!("mndp:{ip_s}"),
                        ip: ip_s,
                        sys_name: if n.identity.is_empty() {
                            n.source_ip.to_string()
                        } else {
                            n.identity.clone()
                        },
                        sys_descr: format!("{} {}", n.platform, n.version),
                        sys_object_id: String::new(),
                        vendor: n.platform.clone(),
                        model: n.board.clone(),
                        macs: if n.mac_address.is_empty() {
                            Vec::new()
                        } else {
                            vec![n.mac_address.clone()]
                        },
                        duplicate_existing: dup_id.is_some(),
                        existing_device_id: dup_id,
                        matched_cred_index: 0,
                    });
                }
            }
        }

        // Correlate LLDP/CDP neighbor names/IPs to discovered candidates
        let mut seen_pairs: HashSet<(String, String)> = HashSet::new();
        for (src_tid, neighs) in raw_neighbors {
            for n in neighs {
                let target_tid = candidates
                    .iter()
                    .find(|c| {
                        (!c.sys_name.is_empty()
                            && c.sys_name.eq_ignore_ascii_case(&n.remote_sys_name))
                            || n.remote_ip.as_deref() == Some(c.ip.as_str())
                    })
                    .map(|c| c.temp_id.clone());
                if let Some(dst_tid) = target_tid {
                    if src_tid == dst_tid {
                        continue;
                    }
                    let pair = if src_tid < dst_tid {
                        (src_tid.clone(), dst_tid.clone())
                    } else {
                        (dst_tid.clone(), src_tid.clone())
                    };
                    if seen_pairs.insert(pair) {
                        candidate_links.push(v1::DiscoveryCandidateLink {
                            src_temp_id: src_tid.clone(),
                            dst_temp_id: dst_tid,
                            src_interface: String::new(),
                            dst_interface: n.remote_port,
                            protocol: n.protocol,
                        });
                    }
                }
            }
        }

        if let Some(job) = self.jobs.write().await.get_mut(&job_id) {
            job.candidates = candidates.clone();
            job.candidate_links = candidate_links.clone();
        }

        ctx.broadcast(ServerBroadcastEvent::DiscoveryProgress(
            v1::DiscoveryProgressEvent {
                job_id,
                total_hosts: total,
                scanned_hosts: scanned,
                alive_hosts: alive,
                finished: true,
                status_text: format!("Finished: {} candidate(s), {} link(s)", candidates.len(), candidate_links.len()),
                candidates,
                candidate_links,
            },
        ));
    }

    pub async fn import_candidates(
        &self,
        ctx: &EngineContext,
        req: v1::ImportDiscoveryRequest,
    ) -> Result<v1::ImportDiscoveryResponse, ServerError> {
        let job = self
            .jobs
            .read()
            .await
            .get(&req.job_id)
            .cloned()
            .ok_or_else(|| ServerError::NotFound(format!("discovery job {}", req.job_id)))?;
        let selected: HashSet<String> = req.selected_temp_ids.into_iter().collect();

        let mut temp_to_device_id: HashMap<String, i64> = HashMap::new();
        let mut imported_devices = 0u32;
        let mut idx = 0usize;

        for cand in &job.candidates {
            if !selected.contains(&cand.temp_id) {
                continue;
            }
            if let Some(existing_id) = cand.existing_device_id {
                temp_to_device_id.insert(cand.temp_id.clone(), existing_id);
                continue;
            }
            let col = (idx % 10) as f64;
            let row = (idx / 10) as f64;
            idx += 1;

            let dev = v1::Device {
                id: 0,
                version: 1,
                map_id: req.target_map_id,
                name: if cand.sys_name.is_empty() {
                    cand.ip.clone()
                } else {
                    cand.sys_name.clone()
                },
                address: cand.ip.clone(),
                device_type_id: 1,
                vendor: cand.vendor.clone(),
                model: cand.model.clone(),
                sys_object_id: cand.sys_object_id.clone(),
                sys_descr: cand.sys_descr.clone(),
                mac_addresses: cand.macs.clone(),
                pos_x: 120.0 + col * 140.0,
                pos_y: 120.0 + row * 120.0,
                shape: v1::NodeShape::RoundedRect as i32,
                size: v1::NodeSize::Medium as i32,
                custom_size_px: 48.0,
                label_template: "{name}\n{ip}".to_string(),
                ..Default::default()
            };
            let secrets = ctx.storage.secrets().clone();
            let saved = ctx
                .storage
                .write(move |c| entities::save_device(c, &secrets, &dev))
                .await?;
            temp_to_device_id.insert(cand.temp_id.clone(), saved.id);
            imported_devices += 1;
            ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
                entity_type: "device".to_string(),
                deleted: false,
                entity_id: saved.id,
                device: Some(saved),
                ..Default::default()
            }));
        }

        let mut imported_links = 0u32;
        if req.import_links {
            for cl in &job.candidate_links {
                if let (Some(&src_id), Some(&dst_id)) = (
                    temp_to_device_id.get(&cl.src_temp_id),
                    temp_to_device_id.get(&cl.dst_temp_id),
                ) {
                    let link = v1::NetworkLink {
                        id: 0,
                        version: 1,
                        map_id: req.target_map_id,
                        src_device_id: src_id,
                        dst_device_id: dst_id,
                        master_device_id: src_id,
                        mastering_type: v1::MasteringType::SnmpIfTraffic as i32,
                        interface_key: cl.dst_interface.clone(),
                        link_type_id: 2, // GigabitEthernet default
                        history_enabled: true,
                        label: cl.protocol.clone(),
                        ..Default::default()
                    };
                    let saved = ctx.storage.write(move |c| entities::save_link(c, &link)).await?;
                    imported_links += 1;
                    ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
                        entity_type: "link".to_string(),
                        deleted: false,
                        entity_id: saved.id,
                        link: Some(saved),
                        ..Default::default()
                    }));
                }
            }
        }

        if req.auto_layout {
            let _ = auto_layout_map(ctx, req.target_map_id, 60).await;
        }
        ctx.refresh_device_ip_index().await;

        Ok(v1::ImportDiscoveryResponse {
            imported_devices,
            imported_links,
        })
    }
}

async fn inspect_alive_host(
    exec: &crate::probe::LocalProbeExecutor,
    profiles: &Arc<RwLock<crate::snmp::profiles::VendorProfileRegistry>>,
    ip: IpAddr,
    creds: &[ResolvedCredential],
    include_lldp_cdp: bool,
) -> (Option<v1::DiscoveryCandidateDevice>, Vec<DiscoveredNeighbor>) {
    let oid_descr = parse_oid("1.3.6.1.2.1.1.1.0").unwrap_or_default();
    let oid_obj = parse_oid("1.3.6.1.2.1.1.2.0").unwrap_or_default();
    let oid_name = parse_oid("1.3.6.1.2.1.1.5.0").unwrap_or_default();

    for (idx, cred) in creds.iter().enumerate() {
        let target = SnmpTarget {
            addr: SocketAddr::new(ip, 161),
            timeout: Duration::from_millis(1500),
            retries: 0,
            credential: cred.clone(),
        };
        if let Ok(vbs) = exec
            .snmp
            .get(&target, &[oid_descr.clone(), oid_obj.clone(), oid_name.clone()])
            .await
        {
            let sys_descr = vbs.first().map(|v| v.value.as_string_lossy()).unwrap_or_default();
            let sys_object_id = vbs.get(1).map(|v| v.value.as_string_lossy()).unwrap_or_default();
            let sys_name = vbs.get(2).map(|v| v.value.as_string_lossy()).unwrap_or_default();

            let (vendor, model) = {
                let guard = profiles.read().await;
                if let Some(p) = guard.match_sys_object_id(&sys_object_id) {
                    let (m, _, _) = guard.match_model_and_image(p, &sys_descr);
                    (p.vendor.clone(), m)
                } else {
                    (String::new(), String::new())
                }
            };

            let mut macs = Vec::new();
            if let Ok(phys_oid) = parse_oid("1.3.6.1.2.1.2.2.1.6") {
                if let Ok(mac_vbs) = exec.snmp.walk(&target, &phys_oid).await {
                    for vb in mac_vbs {
                        if let SnmpValue::OctetString(b) = vb.value {
                            if b.len() == 6 && b.iter().any(|&x| x != 0) {
                                let mac = format!(
                                    "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                                    b[0], b[1], b[2], b[3], b[4], b[5]
                                );
                                if !macs.contains(&mac) {
                                    macs.push(mac);
                                }
                            }
                        }
                    }
                }
            }

            let mut neighs = Vec::new();
            if include_lldp_cdp {
                if let (Ok(lldp_name), Ok(lldp_port)) = (
                    parse_oid("1.0.8802.1.1.2.1.4.1.1.9"),
                    parse_oid("1.0.8802.1.1.2.1.4.1.1.7"),
                ) {
                    let n_vbs = exec.snmp.walk(&target, &lldp_name).await.unwrap_or_default();
                    let p_vbs = exec.snmp.walk(&target, &lldp_port).await.unwrap_or_default();
                    neighs.extend(decode_lldp_neighbors(&n_vbs, &p_vbs));
                }
                if let (Ok(cdp_id), Ok(cdp_port), Ok(cdp_addr)) = (
                    parse_oid("1.3.6.1.4.1.9.9.23.1.2.1.1.6"),
                    parse_oid("1.3.6.1.4.1.9.9.23.1.2.1.1.7"),
                    parse_oid("1.3.6.1.4.1.9.9.23.1.2.1.1.4"),
                ) {
                    let i_vbs = exec.snmp.walk(&target, &cdp_id).await.unwrap_or_default();
                    let p_vbs = exec.snmp.walk(&target, &cdp_port).await.unwrap_or_default();
                    let a_vbs = exec.snmp.walk(&target, &cdp_addr).await.unwrap_or_default();
                    neighs.extend(decode_cdp_neighbors(&i_vbs, &p_vbs, &a_vbs));
                }
            }

            let ip_s = ip.to_string();
            return (
                Some(v1::DiscoveryCandidateDevice {
                    temp_id: format!("ip:{ip_s}"),
                    ip: ip_s.clone(),
                    sys_name: if sys_name.is_empty() { ip_s } else { sys_name },
                    sys_descr,
                    sys_object_id,
                    vendor,
                    model,
                    macs,
                    duplicate_existing: false,
                    existing_device_id: None,
                    matched_cred_index: idx as u32,
                }),
                neighs,
            );
        }
    }
    (None, Vec::new())
}

fn proto_cred_to_resolved(c: &v1::SnmpCredential) -> ResolvedCredential {
    match v1::SnmpVersion::try_from(c.version).unwrap_or(v1::SnmpVersion::V2c) {
        v1::SnmpVersion::V1 => ResolvedCredential::V1 {
            community: c.community.clone().unwrap_or_else(|| "public".to_string()),
        },
        v1::SnmpVersion::V3 => ResolvedCredential::V3(UsmCredentials {
            username: c.username.clone(),
            auth_proto: match v1::SnmpAuthProtocol::try_from(c.auth_protocol) {
                Ok(v1::SnmpAuthProtocol::SnmpAuthMd5) => AuthProto::Md5,
                Ok(v1::SnmpAuthProtocol::SnmpAuthSha1) => AuthProto::Sha1,
                _ => AuthProto::None,
            },
            auth_password: c.auth_password.clone().unwrap_or_default(),
            priv_proto: match v1::SnmpPrivProtocol::try_from(c.priv_protocol) {
                Ok(v1::SnmpPrivProtocol::SnmpPrivDes) => PrivProto::Des,
                Ok(v1::SnmpPrivProtocol::SnmpPrivAes128) => PrivProto::Aes128,
                _ => PrivProto::None,
            },
            priv_password: c.priv_password.clone().unwrap_or_default(),
        }),
        _ => ResolvedCredential::V2c {
            community: c.community.clone().unwrap_or_else(|| "public".to_string()),
        },
    }
}

/// Computes a Fruchterman-Reingold force-directed layout for devices on a map.
pub async fn auto_layout_map(
    ctx: &EngineContext,
    map_id: i64,
    iterations: u32,
) -> Result<(), ServerError> {
    let (mut devs, links) = ctx
        .storage
        .read(move |c| {
            let devs: Vec<v1::Device> = entities::load_devices(c)?
                .into_iter()
                .filter(|d| d.map_id == map_id)
                .collect();
            let links: Vec<v1::NetworkLink> = entities::load_links(c)?
                .into_iter()
                .filter(|l| l.map_id == map_id)
                .collect();
            Ok((devs, links))
        })
        .await?;

    let n = devs.len();
    if n < 2 {
        return Ok(());
    }
    let idx_by_id: HashMap<i64, usize> = devs.iter().enumerate().map(|(i, d)| (d.id, i)).collect();
    let area = (n as f64) * 35000.0;
    let k = (area / (n as f64)).sqrt().max(120.0);
    let mut disp = vec![(0.0f64, 0.0f64); n];
    let iters = iterations.clamp(10, 200);

    for step in 0..iters {
        for d in &mut disp {
            *d = (0.0, 0.0);
        }
        // Repulsive forces between all device pairs
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = devs[i].pos_x - devs[j].pos_x;
                let dy = devs[i].pos_y - devs[j].pos_y;
                let dist = (dx * dx + dy * dy).sqrt().max(1.0);
                let force = (k * k) / dist;
                let fx = (dx / dist) * force;
                let fy = (dy / dist) * force;
                disp[i].0 += fx;
                disp[i].1 += fy;
                disp[j].0 -= fx;
                disp[j].1 -= fy;
            }
        }
        // Attractive forces along links
        for link in &links {
            if let (Some(&u), Some(&v)) = (
                idx_by_id.get(&link.src_device_id),
                idx_by_id.get(&link.dst_device_id),
            ) {
                let dx = devs[u].pos_x - devs[v].pos_x;
                let dy = devs[u].pos_y - devs[v].pos_y;
                let dist = (dx * dx + dy * dy).sqrt().max(1.0);
                let force = (dist * dist) / k;
                let fx = (dx / dist) * force;
                let fy = (dy / dist) * force;
                disp[u].0 -= fx;
                disp[u].1 -= fy;
                disp[v].0 += fx;
                disp[v].1 += fy;
            }
        }
        let temp = (k * 0.5) * (1.0 - (f64::from(step) / f64::from(iters)));
        for i in 0..n {
            let (dx, dy) = disp[i];
            let mag = (dx * dx + dy * dy).sqrt().max(1.0);
            let clamped = mag.min(temp.max(4.0));
            devs[i].pos_x = (devs[i].pos_x + (dx / mag) * clamped).clamp(-4000.0, 4000.0);
            devs[i].pos_y = (devs[i].pos_y + (dy / mag) * clamped).clamp(-4000.0, 4000.0);
        }
    }

    let positions: Vec<(i64, f64, f64)> = devs.iter().map(|d| (d.id, d.pos_x, d.pos_y)).collect();
    ctx.storage
        .write(move |c| {
            let tx = c.transaction().map_err(|e| ServerError::Storage(e.to_string()))?;
            for (id, x, y) in positions {
                tx.execute(
                    "UPDATE devices SET pos_x=?1, pos_y=?2, version=version+1 WHERE id=?3",
                    rusqlite::params![x, y, id],
                )
                .map_err(|e| ServerError::Storage(e.to_string()))?;
            }
            tx.commit().map_err(|e| ServerError::Storage(e.to_string()))?;
            Ok(())
        })
        .await?;

    let refreshed = ctx.storage.read(|c| entities::load_devices(c)).await?;
    for d in refreshed.into_iter().filter(|d| d.map_id == map_id) {
        ctx.broadcast(ServerBroadcastEvent::EntityDelta(v1::EntityDelta {
            entity_type: "device".to_string(),
            deleted: false,
            entity_id: d.id,
            device: Some(d),
            ..Default::default()
        }));
    }
    Ok(())
}
