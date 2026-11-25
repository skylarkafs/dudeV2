use crate::engine::{EngineContext, ServerBroadcastEvent};
use crate::error::ServerError;
use crate::notify::AlertDispatch;
use crate::snmp::ber::{
    decode_message, encode_v1_or_v2_message, format_oid, Pdu, PduType, SnmpMessage, V1OrV2Pdu,
    Varbind,
};
use crate::snmp::usm::{decode_and_verify_v3, encode_v3_message, AuthProto, PrivProto, UsmCredentials, UsmSecurityParams};
use crate::storage::entities;
use crate::storage::telemetry::insert_trap;
use nextdude_common::oid_dict::resolve_oid_name;
use nextdude_common::proto::v1;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::net::UdpSocket;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct DecodedTrap {
    pub snmp_version: v1::SnmpVersion,
    pub trap_oid: String,
    pub trap_name: String,
    pub varbinds: Vec<v1::TrapVarbind>,
    pub is_inform: bool,
    pub request_id: i32,
    pub community: Vec<u8>,
    pub raw_v2_varbinds: Vec<Varbind>,
}

pub fn decode_trap_packet(
    raw: &[u8],
    v3_creds: Option<&UsmCredentials>,
) -> Result<DecodedTrap, ServerError> {
    let msg = decode_message(raw)?;
    match msg {
        SnmpMessage::V1 { community, pdu } => match pdu {
            V1OrV2Pdu::TrapV1(t) => {
                let trap_oid = match t.generic_trap {
                    0 => "1.3.6.1.6.3.1.1.5.1".to_string(),
                    1 => "1.3.6.1.6.3.1.1.5.2".to_string(),
                    2 => "1.3.6.1.6.3.1.1.5.3".to_string(),
                    3 => "1.3.6.1.6.3.1.1.5.4".to_string(),
                    4 => "1.3.6.1.6.3.1.1.5.5".to_string(),
                    _ => format!("{}.0.{}", format_oid(&t.enterprise), t.specific_trap),
                };
                let trap_name = resolve_oid_name(&trap_oid);
                let varbinds = map_varbinds(&t.varbinds);
                Ok(DecodedTrap {
                    snmp_version: v1::SnmpVersion::V1,
                    trap_oid,
                    trap_name,
                    varbinds,
                    is_inform: false,
                    request_id: 0,
                    community,
                    raw_v2_varbinds: Vec::new(),
                })
            }
            V1OrV2Pdu::Standard(p) => Ok(from_v2_pdu(v1::SnmpVersion::V1, community, p)),
        },
        SnmpMessage::V2c { community, pdu } => {
            if !matches!(pdu.pdu_type, PduType::TrapV2 | PduType::InformRequest) {
                return Err(ServerError::Snmp("non-trap PDU received on trap port".to_string()));
            }
            Ok(from_v2_pdu(v1::SnmpVersion::V2c, community, pdu))
        }
        v3_msg @ SnmpMessage::V3 { .. } => {
            let (_, pdu) = decode_and_verify_v3(raw, v3_msg, v3_creds)?;
            if !matches!(pdu.pdu_type, PduType::TrapV2 | PduType::InformRequest) {
                return Err(ServerError::Snmp("non-trap SNMPv3 PDU on trap port".to_string()));
            }
            Ok(from_v2_pdu(v1::SnmpVersion::V3, Vec::new(), pdu))
        }
    }
}

fn from_v2_pdu(ver: v1::SnmpVersion, community: Vec<u8>, pdu: Pdu) -> DecodedTrap {
    let is_inform = pdu.pdu_type == PduType::InformRequest;
    let trap_oid = pdu
        .varbinds
        .iter()
        .find(|vb| format_oid(&vb.oid) == "1.3.6.1.6.3.1.1.4.1.0")
        .map(|vb| vb.value.as_string_lossy())
        .unwrap_or_else(|| "1.3.6.1.6.3.1.1.5".to_string());
    let trap_name = resolve_oid_name(&trap_oid);
    let varbinds = map_varbinds(&pdu.varbinds);
    DecodedTrap {
        snmp_version: ver,
        trap_oid,
        trap_name,
        varbinds,
        is_inform,
        request_id: pdu.request_id,
        community,
        raw_v2_varbinds: pdu.varbinds,
    }
}

fn map_varbinds(vbs: &[Varbind]) -> Vec<v1::TrapVarbind> {
    vbs.iter()
        .map(|vb| {
            let oid_str = format_oid(&vb.oid);
            v1::TrapVarbind {
                oid_name: resolve_oid_name(&oid_str),
                oid: oid_str,
                value_type: vb.value.type_name().to_string(),
                value: vb.value.as_string_lossy(),
            }
        })
        .collect()
}

pub async fn run_trap_receiver(ctx: EngineContext) {
    if !ctx.config.trap_enabled {
        info!("SNMP trap receiver disabled by configuration");
        return;
    }
    let bind_ip: IpAddr = ctx
        .config
        .trap_bind_addr
        .parse()
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let addr = SocketAddr::new(bind_ip, ctx.config.trap_port);
    let sock = match UdpSocket::bind(addr).await {
        Ok(s) => s,
        Err(e) => {
            warn!("failed binding SNMP trap UDP {addr}: {e}");
            return;
        }
    };
    info!("SNMP Trap/Inform UDP receiver listening on {addr}");

    let mut buf = [0u8; 65535];
    loop {
        let Ok((n, src)) = sock.recv_from(&mut buf).await else {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            continue;
        };
        let pkt = &buf[..n];
        let dev_match = ctx.device_host_map.read().await.get(&src.ip()).cloned();
        let (dev_id, dev_name) = match dev_match {
            Some((id, name)) => (Some(id), name),
            None => (None, String::new()),
        };

        // Load device SNMP credentials if known (to verify community or SNMPv3 auth/priv).
        let mut device_comm: Option<String> = None;
        let mut v3_creds: Option<UsmCredentials> = None;
        if let Some(id) = dev_id {
            let secrets = ctx.storage.secrets().clone();
            if let Ok((devs, sec)) = ctx
                .storage
                .read(move |c| {
                    let devs = entities::load_devices(c)?;
                    let s = entities::load_device_secrets(c, &secrets, id)?;
                    Ok((devs, s))
                })
                .await
            {
                if let Some(d) = devs.into_iter().find(|d| d.id == id) {
                    if !sec.community.is_empty() {
                        device_comm = Some(sec.community);
                    }
                    if let Some(sn) = d.snmp {
                        if sn.version == v1::SnmpVersion::V3 as i32 && !sn.username.is_empty() {
                            v3_creds = Some(UsmCredentials {
                                username: sn.username,
                                auth_proto: match v1::SnmpAuthProtocol::try_from(sn.auth_protocol) {
                                    Ok(v1::SnmpAuthProtocol::SnmpAuthMd5) => AuthProto::Md5,
                                    Ok(v1::SnmpAuthProtocol::SnmpAuthSha1) => AuthProto::Sha1,
                                    _ => AuthProto::None,
                                },
                                auth_password: sec.auth_password,
                                priv_proto: match v1::SnmpPrivProtocol::try_from(sn.priv_protocol) {
                                    Ok(v1::SnmpPrivProtocol::SnmpPrivDes) => PrivProto::Des,
                                    Ok(v1::SnmpPrivProtocol::SnmpPrivAes128) => PrivProto::Aes128,
                                    _ => PrivProto::None,
                                },
                                priv_password: sec.priv_password,
                            });
                        }
                    }
                }
            }
        }

        let decoded = match decode_trap_packet(pkt, v3_creds.as_ref()) {
            Ok(d) => d,
            Err(e) => {
                warn!("failed decoding SNMP trap from {}: {e}", src);
                continue;
            }
        };

        // Enforce community match when configured on the matched device
        if let Some(ref expected_comm) = device_comm {
            if matches!(decoded.snmp_version, v1::SnmpVersion::V1 | v1::SnmpVersion::V2c)
                && decoded.community != expected_comm.as_bytes()
            {
                warn!("SNMP trap community mismatch from {}", src);
                continue;
            }
        }

        // Send InformResponse immediately when InformRequest is received
        if decoded.is_inform {
            let resp_pdu = Pdu {
                pdu_type: PduType::Response,
                request_id: decoded.request_id,
                error_status: 0,
                error_index: 0,
                varbinds: decoded.raw_v2_varbinds.clone(),
            };
            let reply_wire = match decoded.snmp_version {
                v1::SnmpVersion::V3 => {
                    if let Some(ref creds) = v3_creds {
                        let mut sec = UsmSecurityParams {
                            engine_id: Vec::new(),
                            engine_boots: 0,
                            engine_time: 0,
                            salt_counter: 1,
                        };
                        encode_v3_message(
                            decoded.request_id,
                            false,
                            creds,
                            &mut sec,
                            b"",
                            b"",
                            &resp_pdu,
                        )
                        .ok()
                    } else {
                        None
                    }
                }
                _ => encode_v1_or_v2_message(&SnmpMessage::V2c {
                    community: decoded.community.clone(),
                    pdu: resp_pdu,
                })
                .ok(),
            };
            if let Some(wire) = reply_wire {
                let _ = sock.send_to(&wire, src).await;
            }
        }

        let now = chrono::Utc::now().timestamp();
        let trap_msg = v1::TrapMessage {
            id: 0,
            timestamp_utc: now,
            source_ip: src.ip().to_string(),
            device_id: dev_id,
            device_name: dev_name.clone(),
            snmp_version: decoded.snmp_version as i32,
            trap_oid: decoded.trap_oid.clone(),
            trap_name: decoded.trap_name.clone(),
            varbinds: decoded.varbinds.clone(),
        };

        if let Ok(saved) = ctx.storage.write(move |c| insert_trap(c, &trap_msg)).await {
            ctx.broadcast(ServerBroadcastEvent::Trap(saved));
        }

        // Immediate linkDown / linkUp handling without waiting for polling
        let is_link_down = decoded.trap_oid == "1.3.6.1.6.3.1.1.5.3";
        let is_link_up = decoded.trap_oid == "1.3.6.1.6.3.1.1.5.4";
        if (is_link_down || is_link_up) && dev_id.is_some() {
            let if_idx = decoded
                .varbinds
                .iter()
                .find(|vb| vb.oid.starts_with("1.3.6.1.2.1.2.2.1.1"))
                .and_then(|vb| vb.value.parse::<u32>().ok());
            if let (Some(did), Some(idx)) = (dev_id, if_idx) {
                ctx.handle_interface_trap_immediate(did, idx, is_link_up).await;
            }
            let _ = ctx
                .notifications
                .dispatch(AlertDispatch {
                    severity: if is_link_down {
                        v1::Severity::Critical
                    } else {
                        v1::Severity::Info
                    },
                    device_id: dev_id,
                    device_name: if dev_name.is_empty() {
                        src.ip().to_string()
                    } else {
                        dev_name
                    },
                    title: format!("SNMP Trap: {}", decoded.trap_name),
                    message: format!("Trap {} received from {}", decoded.trap_name, src.ip()),
                    channel_ids: Vec::new(),
                })
                .await;
        }
    }
}
