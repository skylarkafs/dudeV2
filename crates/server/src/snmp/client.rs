use crate::error::ServerError;
use crate::snmp::ber::{
    decode_message, encode_v1_or_v2_message, format_oid, Pdu, PduType, SnmpMessage, SnmpValue,
    V1OrV2Pdu, Varbind,
};
use crate::snmp::usm::{
    decode_and_verify_v3, encode_v3_message, AuthProto, PrivProto, UsmCredentials,
    UsmSecurityParams,
};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::time::timeout;

#[derive(Debug, Clone)]
pub enum ResolvedCredential {
    V1 { community: String },
    V2c { community: String },
    V3(UsmCredentials),
}

#[derive(Debug, Clone)]
pub struct SnmpTarget {
    pub addr: SocketAddr,
    pub timeout: Duration,
    pub retries: u32,
    pub credential: ResolvedCredential,
}

#[derive(Clone)]
pub struct SnmpClient {
    next_req_id: Arc<AtomicI32>,
    v3_engines: Arc<Mutex<HashMap<SocketAddr, UsmSecurityParams>>>,
}

impl Default for SnmpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl SnmpClient {
    pub fn new() -> Self {
        Self {
            next_req_id: Arc::new(AtomicI32::new(1000)),
            v3_engines: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn alloc_id(&self) -> i32 {
        let id = self.next_req_id.fetch_add(1, Ordering::Relaxed);
        if id <= 0 {
            1
        } else {
            id & 0x7FFF_FFFF
        }
    }

    pub async fn get(&self, target: &SnmpTarget, oids: &[Vec<u32>]) -> Result<Vec<Varbind>, ServerError> {
        if oids.is_empty() {
            return Ok(Vec::new());
        }
        // Batch multiple OIDs per PDU (safe chunk size 24 to stay well below UDP MTU).
        let mut results = Vec::with_capacity(oids.len());
        for chunk in oids.chunks(24) {
            let vbs = chunk
                .iter()
                .map(|oid| Varbind {
                    oid: oid.clone(),
                    value: SnmpValue::Null,
                })
                .collect();
            let resp = self
                .send_pdu(target, PduType::GetRequest, 0, 0, vbs)
                .await?;
            if resp.error_status != 0 {
                return Err(ServerError::Snmp(format!(
                    "SNMP error_status={} error_index={}",
                    resp.error_status, resp.error_index
                )));
            }
            results.extend(resp.varbinds);
        }
        Ok(results)
    }

    pub async fn get_next(&self, target: &SnmpTarget, oid: &[u32]) -> Result<Varbind, ServerError> {
        let resp = self
            .send_pdu(
                target,
                PduType::GetNextRequest,
                0,
                0,
                vec![Varbind {
                    oid: oid.to_vec(),
                    value: SnmpValue::Null,
                }],
            )
            .await?;
        resp.varbinds
            .into_iter()
            .next()
            .ok_or_else(|| ServerError::Snmp("empty GetNext response".to_string()))
    }

    pub async fn get_bulk(
        &self,
        target: &SnmpTarget,
        oids: &[Vec<u32>],
        non_repeaters: i32,
        max_repetitions: i32,
    ) -> Result<Vec<Varbind>, ServerError> {
        let pdu_type = match target.credential {
            ResolvedCredential::V1 { .. } => PduType::GetNextRequest,
            _ => PduType::GetBulkRequest,
        };
        let vbs = oids
            .iter()
            .map(|oid| Varbind {
                oid: oid.clone(),
                value: SnmpValue::Null,
            })
            .collect();
        let resp = self
            .send_pdu(target, pdu_type, non_repeaters, max_repetitions, vbs)
            .await?;
        Ok(resp.varbinds)
    }

    /// Walks an OID subtree using GETBULK (v2c/v3) or GETNEXT (v1) with cycle protection.
    pub async fn walk(&self, target: &SnmpTarget, root_oid: &[u32]) -> Result<Vec<Varbind>, ServerError> {
        let mut results = Vec::new();
        let mut current = root_oid.to_vec();
        let max_iterations = 4096usize;

        for _ in 0..max_iterations {
            let batch = self
                .get_bulk(target, std::slice::from_ref(&current), 0, 20)
                .await?;
            if batch.is_empty() {
                break;
            }
            let mut progressed = false;
            let mut finished = false;
            for vb in batch {
                if matches!(vb.value, SnmpValue::EndOfMibView) || !vb.oid.starts_with(root_oid) {
                    finished = true;
                    break;
                }
                if vb.oid <= current {
                    finished = true;
                    break;
                }
                current = vb.oid.clone();
                results.push(vb);
                progressed = true;
            }
            if finished || !progressed {
                break;
            }
        }
        Ok(results)
    }

    async fn send_pdu(
        &self,
        target: &SnmpTarget,
        pdu_type: PduType,
        error_status: i32,
        error_index: i32,
        varbinds: Vec<Varbind>,
    ) -> Result<Pdu, ServerError> {
        let bind_addr: SocketAddr = match target.addr.ip() {
            IpAddr::V4(_) => SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
            IpAddr::V6(_) => SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0),
        };
        let socket = UdpSocket::bind(bind_addr).await?;
        socket.connect(target.addr).await?;

        let mut last_err = ServerError::Snmp("SNMP request failed".to_string());
        let attempts = target.retries.saturating_add(1);

        for _ in 0..attempts {
            let req_id = self.alloc_id();
            let pdu = Pdu {
                pdu_type,
                request_id: req_id,
                error_status,
                error_index,
                varbinds: varbinds.clone(),
            };

            let wire = match &target.credential {
                ResolvedCredential::V1 { community } => {
                    encode_v1_or_v2_message(&SnmpMessage::V1 {
                        community: community.as_bytes().to_vec(),
                        pdu: V1OrV2Pdu::Standard(pdu),
                    })?
                }
                ResolvedCredential::V2c { community } => {
                    encode_v1_or_v2_message(&SnmpMessage::V2c {
                        community: community.as_bytes().to_vec(),
                        pdu,
                    })?
                }
                ResolvedCredential::V3(creds) => {
                    let mut engine = self.get_or_discover_engine(&socket, target, creds).await?;
                    let msg_id = self.alloc_id();
                    let engine_id = engine.engine_id.clone();
                    let bytes = encode_v3_message(
                        msg_id,
                        true,
                        creds,
                        &mut engine,
                        &engine_id,
                        b"",
                        &pdu,
                    )?;
                    self.v3_engines.lock().await.insert(target.addr, engine);
                    bytes
                }
            };

            socket.send(&wire).await?;

            let mut buf = vec![0u8; 65535];
            match timeout(target.timeout, socket.recv(&mut buf)).await {
                Ok(Ok(n)) => {
                    let pkt = &buf[..n];
                    let decoded = decode_message(pkt)?;
                    match (&target.credential, decoded) {
                        (
                            ResolvedCredential::V1 { .. },
                            SnmpMessage::V1 {
                                pdu: V1OrV2Pdu::Standard(resp),
                                ..
                            },
                        )
                        | (ResolvedCredential::V2c { .. }, SnmpMessage::V2c { pdu: resp, .. }) => {
                            if resp.request_id == req_id {
                                return Ok(resp);
                            }
                            last_err = ServerError::Snmp("mismatched SNMP request_id".to_string());
                        }
                        (ResolvedCredential::V3(creds), msg @ SnmpMessage::V3 { .. }) => {
                            let (usm_hdr, resp) = decode_and_verify_v3(pkt, msg, Some(creds))?;
                            // Refresh engine boots/time from authoritative response.
                            {
                                let mut map = self.v3_engines.lock().await;
                                if let Some(entry) = map.get_mut(&target.addr) {
                                    if !usm_hdr.engine_id.is_empty() {
                                        entry.engine_id = usm_hdr.engine_id;
                                    }
                                    entry.engine_boots = usm_hdr.engine_boots;
                                    entry.engine_time = usm_hdr.engine_time;
                                }
                            }
                            if resp.pdu_type == PduType::Report {
                                // usmStatsNotInTimeWindows -> retry once with updated engine time
                                let is_time_window = resp.varbinds.iter().any(|vb| {
                                    format_oid(&vb.oid).starts_with("1.3.6.1.6.3.15.1.1.2")
                                });
                                if is_time_window {
                                    last_err = ServerError::Snmp(
                                        "SNMPv3 time window resync required".to_string(),
                                    );
                                    continue;
                                }
                                return Err(ServerError::Snmp("SNMPv3 Report error received".to_string()));
                            }
                            if resp.request_id == req_id {
                                return Ok(resp);
                            }
                            last_err = ServerError::Snmp("mismatched SNMPv3 request_id".to_string());
                        }
                        _ => {
                            last_err =
                                ServerError::Snmp("unexpected SNMP version/PDU in response".to_string());
                        }
                    }
                }
                Ok(Err(e)) => last_err = ServerError::Io(e),
                Err(_) => last_err = ServerError::Snmp("SNMP timeout".to_string()),
            }
        }
        Err(last_err)
    }

    async fn get_or_discover_engine(
        &self,
        socket: &UdpSocket,
        target: &SnmpTarget,
        _creds: &UsmCredentials,
    ) -> Result<UsmSecurityParams, ServerError> {
        if let Some(cached) = self.v3_engines.lock().await.get(&target.addr).cloned() {
            if !cached.engine_id.is_empty() {
                return Ok(cached);
            }
        }

        let probe_creds = UsmCredentials {
            username: String::new(),
            auth_proto: AuthProto::None,
            auth_password: String::new(),
            priv_proto: PrivProto::None,
            priv_password: String::new(),
        };
        let mut empty_sec = UsmSecurityParams {
            engine_id: Vec::new(),
            engine_boots: 0,
            engine_time: 0,
            salt_counter: 1,
        };
        let req_id = self.alloc_id();
        let probe_pdu = Pdu {
            pdu_type: PduType::GetRequest,
            request_id: req_id,
            error_status: 0,
            error_index: 0,
            varbinds: Vec::new(),
        };
        let wire = encode_v3_message(
            self.alloc_id(),
            true,
            &probe_creds,
            &mut empty_sec,
            b"",
            b"",
            &probe_pdu,
        )?;
        socket.send(&wire).await?;

        let mut buf = vec![0u8; 4096];
        let n = timeout(target.timeout, socket.recv(&mut buf))
            .await
            .map_err(|_| ServerError::Snmp("SNMPv3 engine discovery timeout".to_string()))??;
        let msg = decode_message(&buf[..n])?;
        let (usm_hdr, _) = decode_and_verify_v3(&buf[..n], msg, None)?;
        if usm_hdr.engine_id.is_empty() {
            return Err(ServerError::Snmp(
                "agent returned empty authoritativeEngineID".to_string(),
            ));
        }
        let params = UsmSecurityParams {
            engine_id: usm_hdr.engine_id,
            engine_boots: usm_hdr.engine_boots,
            engine_time: usm_hdr.engine_time,
            salt_counter: 1,
        };
        self.v3_engines
            .lock()
            .await
            .insert(target.addr, params.clone());
        Ok(params)
    }
}
