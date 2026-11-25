use crate::error::ServerError;
use nextdude_common::types::IcmpCapabilityMode;
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{oneshot, Mutex};
use tokio::time::timeout;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct IcmpStartupReport {
    pub mode: IcmpCapabilityMode,
    pub ipv6_supported: bool,
    pub message: String,
}

type PendingMap = Arc<Mutex<HashMap<(IpAddr, u16), oneshot::Sender<Instant>>>>;

pub struct IcmpProber {
    report: IcmpStartupReport,
    v4_socket: Option<Arc<UdpSocket>>,
    v6_socket: Option<Arc<UdpSocket>>,
    pending: PendingMap,
    next_seq: AtomicU16,
    ident: u16,
}

impl IcmpProber {
    pub fn initialize() -> Arc<Self> {
        let (v4_sock, v4_raw, v4_mode, v4_msg) = probe_family(false);
        let (v6_sock, v6_raw, _, v6_msg) = probe_family(true);

        let message = match v4_mode {
            IcmpCapabilityMode::Datagram => {
                format!("ICMP active using unprivileged datagram sockets ({v6_msg})")
            }
            IcmpCapabilityMode::Raw => {
                format!("ICMP active using raw sockets via CAP_NET_RAW ({v6_msg})")
            }
            IcmpCapabilityMode::Unavailable => format!(
                "ICMP unavailable ({v4_msg}). Fix on Linux by either: \
                 (1) setting sysctl net.ipv4.ping_group_range=\"0 2147483647\", or \
                 (2) granting CAP_NET_RAW (AmbientCapabilities=CAP_NET_RAW in systemd or setcap cap_net_raw+ep)."
            ),
        };

        if v4_mode == IcmpCapabilityMode::Unavailable {
            warn!("{message}");
        } else {
            info!("{message}");
        }

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let ident = (std::process::id() & 0xFFFF) as u16;

        if let Some(ref s4) = v4_sock {
            tokio::spawn(recv_loop(Arc::clone(s4), Arc::clone(&pending), false, v4_raw));
        }
        if let Some(ref s6) = v6_sock {
            tokio::spawn(recv_loop(Arc::clone(s6), Arc::clone(&pending), true, v6_raw));
        }

        Arc::new(Self {
            report: IcmpStartupReport {
                mode: v4_mode,
                ipv6_supported: v6_sock.is_some(),
                message,
            },
            v4_socket: v4_sock,
            v6_socket: v6_sock,
            pending,
            next_seq: AtomicU16::new(1),
            ident,
        })
    }

    pub fn report(&self) -> &IcmpStartupReport {
        &self.report
    }

    pub async fn ping(&self, target: IpAddr, wait: Duration) -> Result<Duration, ServerError> {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self.pending.lock().await;
            guard.insert((target, seq), tx);
        }

        let send_res = match target {
            IpAddr::V4(v4) => {
                let Some(sock) = &self.v4_socket else {
                    self.pending.lock().await.remove(&(target, seq));
                    return Err(ServerError::Probe(self.report.message.clone()));
                };
                let pkt = build_icmp_v4_echo(self.ident, seq, b"NextDudePing");
                sock.send_to(&pkt, SocketAddr::new(IpAddr::V4(v4), 0)).await
            }
            IpAddr::V6(v6) => {
                let Some(sock) = &self.v6_socket else {
                    self.pending.lock().await.remove(&(target, seq));
                    return Err(ServerError::Probe("IPv6 ICMP socket unavailable".to_string()));
                };
                let pkt = build_icmp_v6_echo(self.ident, seq, b"NextDudePing");
                sock.send_to(&pkt, SocketAddr::new(IpAddr::V6(v6), 0)).await
            }
        };

        if let Err(e) = send_res {
            self.pending.lock().await.remove(&(target, seq));
            return Err(ServerError::Io(e));
        }

        let start = Instant::now();
        match timeout(wait, rx).await {
            Ok(Ok(done)) => Ok(done.saturating_duration_since(start)),
            Ok(Err(_)) => {
                self.pending.lock().await.remove(&(target, seq));
                Err(ServerError::Probe("ICMP reply channel closed".to_string()))
            }
            Err(_) => {
                self.pending.lock().await.remove(&(target, seq));
                Err(ServerError::Probe("ICMP request timed out".to_string()))
            }
        }
    }
}

fn probe_family(
    ipv6: bool,
) -> (Option<Arc<UdpSocket>>, bool, IcmpCapabilityMode, String) {
    let domain = if ipv6 { Domain::IPV6 } else { Domain::IPV4 };
    let proto = if ipv6 {
        Protocol::ICMPV6
    } else {
        Protocol::ICMPV4
    };
    let bind_addr: SocketAddr = if ipv6 {
        SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)
    } else {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
    };

    if let Ok(sock) = open_async_icmp_socket(domain, Type::DGRAM, proto, bind_addr) {
        return (
            Some(Arc::new(sock)),
            false,
            IcmpCapabilityMode::Datagram,
            if ipv6 { "IPv6 dgram OK" } else { "IPv4 dgram OK" }.to_string(),
        );
    }

    match open_async_icmp_socket(domain, Type::RAW, proto, bind_addr) {
        Ok(sock) => (
            Some(Arc::new(sock)),
            true,
            IcmpCapabilityMode::Raw,
            if ipv6 { "IPv6 raw OK" } else { "IPv4 raw OK" }.to_string(),
        ),
        Err(e) => (
            None,
            false,
            IcmpCapabilityMode::Unavailable,
            format!("{e}"),
        ),
    }
}

fn open_async_icmp_socket(
    domain: Domain,
    sock_type: Type,
    proto: Protocol,
    bind_addr: SocketAddr,
) -> Result<UdpSocket, std::io::Error> {
    let sock = Socket::new(domain, sock_type, Some(proto))?;
    sock.set_nonblocking(true)?;
    sock.bind(&bind_addr.into())?;
    let std_udp: std::net::UdpSocket = sock.into();
    UdpSocket::from_std(std_udp)
}

async fn recv_loop(sock: Arc<UdpSocket>, pending: PendingMap, ipv6: bool, raw_mode: bool) {
    let mut buf = [0u8; 2048];
    loop {
        match sock.recv_from(&mut buf).await {
            Ok((n, src)) => {
                let now = Instant::now();
                let seq = if ipv6 {
                    parse_icmp_v6_echo_reply(&buf[..n]).map(|(_, seq)| seq)
                } else {
                    parse_icmp_v4_echo_reply(&buf[..n], raw_mode).map(|(_, seq)| seq)
                };
                if let Some(seq) = seq {
                    let key = (src.ip(), seq);
                    if let Some(tx) = pending.lock().await.remove(&key) {
                        let _ = tx.send(now);
                    }
                }
            }
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

pub fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut chunks = data.chunks_exact(2);
    for chunk in &mut chunks {
        let word = u16::from_be_bytes([chunk[0], chunk[1]]);
        sum = sum.wrapping_add(u32::from(word));
    }
    if let Some(&rem) = chunks.remainder().first() {
        sum = sum.wrapping_add(u32::from(rem) << 8);
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

pub fn build_icmp_v4_echo(ident: u16, seq: u16, payload: &[u8]) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(8 + payload.len());
    pkt.push(8); // Echo Request
    pkt.push(0); // Code 0
    pkt.extend_from_slice(&[0, 0]);
    pkt.extend_from_slice(&ident.to_be_bytes());
    pkt.extend_from_slice(&seq.to_be_bytes());
    pkt.extend_from_slice(payload);
    let csum = internet_checksum(&pkt);
    pkt[2..4].copy_from_slice(&csum.to_be_bytes());
    pkt
}

pub fn parse_icmp_v4_echo_reply(data: &[u8], strip_ipv4_header: bool) -> Option<(u16, u16)> {
    let payload = if strip_ipv4_header {
        if data.len() < 20 {
            return None;
        }
        let ihl = usize::from(data[0] & 0x0F) * 4;
        if ihl < 20 || data.len() < ihl + 8 {
            return None;
        }
        &data[ihl..]
    } else {
        if data.len() < 8 {
            return None;
        }
        data
    };
    if payload[0] != 0 || payload[1] != 0 {
        return None;
    }
    let ident = u16::from_be_bytes([payload[4], payload[5]]);
    let seq = u16::from_be_bytes([payload[6], payload[7]]);
    Some((ident, seq))
}

pub fn build_icmp_v6_echo(ident: u16, seq: u16, payload: &[u8]) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(8 + payload.len());
    pkt.push(128); // ICMPv6 Echo Request
    pkt.push(0);
    pkt.extend_from_slice(&[0, 0]);
    pkt.extend_from_slice(&ident.to_be_bytes());
    pkt.extend_from_slice(&seq.to_be_bytes());
    pkt.extend_from_slice(payload);
    pkt
}

pub fn parse_icmp_v6_echo_reply(data: &[u8]) -> Option<(u16, u16)> {
    if data.len() < 8 || data[0] != 129 || data[1] != 0 {
        return None;
    }
    let ident = u16::from_be_bytes([data[4], data[5]]);
    let seq = u16::from_be_bytes([data[6], data[7]]);
    Some((ident, seq))
}
