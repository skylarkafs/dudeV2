use crate::engine::EngineContext;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::RwLock;
use tracing::{info, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MndpNeighbor {
    pub source_ip: IpAddr,
    pub mac_address: String,
    pub identity: String,
    pub version: String,
    pub platform: String,
    pub uptime_sec: u32,
    pub software_id: String,
    pub board: String,
    pub interface_name: String,
    pub ipv4: Option<Ipv4Addr>,
    pub ipv6: Option<Ipv6Addr>,
    pub last_seen_utc: i64,
}

/// Parses a MikroTik Neighbor Discovery Protocol (MNDP) UDP/5678 TLV payload.
/// Note: TLV headers are big-endian (Type: u16, Length: u16), while Uptime (0x000a)
/// is 4-byte little-endian per the MikroTik MNDP wire format.
pub fn parse_mndp_packet(source_ip: IpAddr, data: &[u8], now_utc: i64) -> Option<MndpNeighbor> {
    if data.len() < 4 {
        return None;
    }
    // 4-byte MNDP header (type/seq/flags) precedes the TLV sequence.
    let mut pos = 4usize;
    let mut neighbor = MndpNeighbor {
        source_ip,
        mac_address: String::new(),
        identity: String::new(),
        version: String::new(),
        platform: String::new(),
        uptime_sec: 0,
        software_id: String::new(),
        board: String::new(),
        interface_name: String::new(),
        ipv4: None,
        ipv6: None,
        last_seen_utc: now_utc,
    };

    let mut found_any = false;
    while pos + 4 <= data.len() {
        let tlv_type = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let tlv_len = usize::from(u16::from_be_bytes([data[pos + 2], data[pos + 3]]));
        pos += 4;
        if pos + tlv_len > data.len() {
            break;
        }
        let val = &data[pos..pos + tlv_len];
        pos += tlv_len;
        found_any = true;

        match tlv_type {
            0x0001 if val.len() == 6 => {
                neighbor.mac_address = format!(
                    "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
                    val[0], val[1], val[2], val[3], val[4], val[5]
                );
            }
            0x0005 => {
                neighbor.identity = String::from_utf8_lossy(val).trim_end_matches('\0').to_string();
            }
            0x0007 => {
                neighbor.version = String::from_utf8_lossy(val).trim_end_matches('\0').to_string();
            }
            0x0008 => {
                neighbor.platform = String::from_utf8_lossy(val).trim_end_matches('\0').to_string();
            }
            0x000a if val.len() == 4 => {
                neighbor.uptime_sec = u32::from_le_bytes([val[0], val[1], val[2], val[3]]);
            }
            0x000b => {
                neighbor.software_id = String::from_utf8_lossy(val).trim_end_matches('\0').to_string();
            }
            0x000c => {
                neighbor.board = String::from_utf8_lossy(val).trim_end_matches('\0').to_string();
            }
            0x000f if val.len() == 16 => {
                let mut arr = [0u8; 16];
                arr.copy_from_slice(val);
                neighbor.ipv6 = Some(Ipv6Addr::from(arr));
            }
            0x0010 => {
                neighbor.interface_name =
                    String::from_utf8_lossy(val).trim_end_matches('\0').to_string();
            }
            0x0011 if val.len() == 4 => {
                neighbor.ipv4 = Some(Ipv4Addr::new(val[0], val[1], val[2], val[3]));
            }
            _ => {}
        }
    }

    if found_any && (!neighbor.mac_address.is_empty() || !neighbor.identity.is_empty()) {
        Some(neighbor)
    } else {
        None
    }
}

pub type MndpCache = Arc<RwLock<HashMap<String, MndpNeighbor>>>;

pub async fn run_mndp_listener(ctx: EngineContext, cache: MndpCache) {
    if !ctx.config.mndp_enabled {
        info!("MNDP listener disabled by configuration");
        return;
    }
    let bind_addr: SocketAddr = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        ctx.config.mndp_port,
    );
    let sock = match UdpSocket::bind(bind_addr).await {
        Ok(s) => s,
        Err(e) => {
            warn!("failed binding MNDP UDP {}: {e}", bind_addr);
            return;
        }
    };
    let _ = sock.set_broadcast(true);
    info!("MNDP UDP listener bound on {}", bind_addr);

    let mut buf = [0u8; 4096];
    loop {
        match sock.recv_from(&mut buf).await {
            Ok((n, src)) => {
                let now = chrono::Utc::now().timestamp();
                if let Some(neigh) = parse_mndp_packet(src.ip(), &buf[..n], now) {
                    let key = if !neigh.mac_address.is_empty() {
                        neigh.mac_address.clone()
                    } else {
                        src.ip().to_string()
                    };
                    cache.write().await.insert(key, neigh);
                }
            }
            Err(e) => {
                warn!("MNDP recv error: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        }
    }
}
