use crate::engine::scheduler::build_probe_target;
use crate::engine::EngineContext;
use crate::probe::icmp::{build_icmp_v4_echo, parse_icmp_v4_echo_reply};
use crate::probe::network::resolve_target_ip;
use crate::probe::ProbeTarget;
use crate::snmp::ber::{format_oid, parse_oid};
use crate::snmp::{ResolvedCredential, SnmpTarget};
use crate::storage::entities;
use nextdude_common::oid_dict::resolve_oid_name;
use nextdude_common::proto::v1;
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::net::{lookup_host, TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tonic::Status;

pub async fn execute_tool_stream(
    ctx: EngineContext,
    req: v1::RunToolRequest,
    tx: mpsc::Sender<Result<v1::ToolOutputChunk, Status>>,
) {
    let _tool_guard = ctx.tracker.acquire_tool_guard();
    let kind = v1::ToolKind::try_from(req.kind).unwrap_or(v1::ToolKind::Unspecified);
    let send_line = |line: String, finished: bool, error: bool| {
        let tx = tx.clone();
        async move {
            let _ = tx
                .send(Ok(v1::ToolOutputChunk {
                    line,
                    finished,
                    error,
                }))
                .await;
        }
    };

    let target = if let Some(dev_id) = req.device_id {
        match ctx.storage.read(move |c| entities::load_devices(c)).await {
            Ok(devs) => match devs.into_iter().find(|d| d.id == dev_id) {
                Some(d) => build_probe_target(&ctx.storage, &d).await,
                None => default_target(&req.target),
            },
            Err(_) => default_target(&req.target),
        }
    } else {
        default_target(&req.target)
    };

    match kind {
        v1::ToolKind::Ping => {
            let ip = match resolve_target_ip(&ctx.executor, &target).await {
                Ok(ip) => ip,
                Err(e) => {
                    send_line(format!("Error: {e}"), true, true).await;
                    return;
                }
            };
            let count = req.count.clamp(1, 50);
            send_line(format!("Pinging {} ({ip}) x{count}...", target.host), false, false).await;
            let mut ok_cnt = 0u32;
            let mut sum_ms = 0.0;
            for i in 1..=count {
                match ctx.executor.icmp.ping(ip, Duration::from_secs(2)).await {
                    Ok(rtt) => {
                        let ms = rtt.as_secs_f64() * 1000.0;
                        ok_cnt += 1;
                        sum_ms += ms;
                        send_line(format!("seq={i}: reply from {ip} time={ms:.2} ms"), false, false)
                            .await;
                    }
                    Err(e) => {
                        send_line(format!("seq={i}: {e}"), false, false).await;
                    }
                }
                if i < count {
                    tokio::time::sleep(Duration::from_millis(800)).await;
                }
            }
            let avg = if ok_cnt > 0 {
                sum_ms / f64::from(ok_cnt)
            } else {
                0.0
            };
            send_line(
                format!("Summary: {ok_cnt}/{count} received, avg={avg:.2} ms"),
                true,
                false,
            )
            .await;
        }
        v1::ToolKind::Traceroute => {
            let ip = match resolve_target_ip(&ctx.executor, &target).await {
                Ok(ip) => ip,
                Err(e) => {
                    send_line(format!("Error: {e}"), true, true).await;
                    return;
                }
            };
            run_traceroute(ip, &tx).await;
        }
        v1::ToolKind::SnmpGet | v1::ToolKind::SnmpWalk => {
            let ip = match resolve_target_ip(&ctx.executor, &target).await {
                Ok(ip) => ip,
                Err(e) => {
                    send_line(format!("Error: {e}"), true, true).await;
                    return;
                }
            };
            let oid = match parse_oid(if req.oid.is_empty() {
                "1.3.6.1.2.1.1"
            } else {
                &req.oid
            }) {
                Ok(o) => o,
                Err(e) => {
                    send_line(format!("Invalid OID: {e}"), true, true).await;
                    return;
                }
            };
            let st = SnmpTarget {
                addr: SocketAddr::new(ip, target.snmp_port),
                timeout: target.snmp_timeout,
                retries: target.snmp_retries,
                credential: target.snmp_credential.clone(),
            };
            let vbs_res = if kind == v1::ToolKind::SnmpGet {
                ctx.snmp.get(&st, &[oid]).await
            } else {
                ctx.snmp.walk(&st, &oid).await
            };
            match vbs_res {
                Ok(vbs) => {
                    for vb in &vbs {
                        let oid_s = format_oid(&vb.oid);
                        let name = resolve_oid_name(&oid_s);
                        send_line(
                            format!(
                                "{name} ({oid_s}) [{}] = {}",
                                vb.value.type_name(),
                                vb.value.as_string_lossy()
                            ),
                            false,
                            false,
                        )
                        .await;
                    }
                    send_line(format!("Completed: {} varbind(s)", vbs.len()), true, false).await;
                }
                Err(e) => {
                    send_line(format!("SNMP error: {e}"), true, true).await;
                }
            }
        }
        v1::ToolKind::TcpCheck => {
            let ip = match resolve_target_ip(&ctx.executor, &target).await {
                Ok(ip) => ip,
                Err(e) => {
                    send_line(format!("Error: {e}"), true, true).await;
                    return;
                }
            };
            let port = if req.port == 0 { 80 } else { req.port as u16 };
            let start = Instant::now();
            match timeout(Duration::from_secs(4), TcpStream::connect(SocketAddr::new(ip, port)))
                .await
            {
                Ok(Ok(mut stream)) => {
                    let ms = start.elapsed().as_secs_f64() * 1000.0;
                    let mut buf = [0u8; 256];
                    let banner = match timeout(Duration::from_millis(600), stream.read(&mut buf)).await
                    {
                        Ok(Ok(n)) if n > 0 => {
                            format!(" | banner: {}", String::from_utf8_lossy(&buf[..n]).trim())
                        }
                        _ => String::new(),
                    };
                    send_line(
                        format!("TCP {ip}:{port} OPEN ({ms:.2} ms){banner}"),
                        true,
                        false,
                    )
                    .await;
                }
                Ok(Err(e)) => {
                    send_line(format!("TCP {ip}:{port} CLOSED/FAILED: {e}"), true, true).await;
                }
                Err(_) => {
                    send_line(format!("TCP {ip}:{port} TIMEOUT"), true, true).await;
                }
            }
        }
        v1::ToolKind::DnsLookup => {
            if let Some(ref resolver) = ctx.executor.dns {
                match resolver.lookup_ip(req.target.as_str()).await {
                    Ok(resp) => {
                        for ip in resp.iter() {
                            send_line(format!("Address: {ip}"), false, false).await;
                        }
                        send_line("DNS lookup complete".to_string(), true, false).await;
                        return;
                    }
                    Err(e) => {
                        send_line(format!("DNS resolver error: {e}"), true, true).await;
                        return;
                    }
                }
            }
            match lookup_host((req.target.as_str(), 0)).await {
                Ok(addrs) => {
                    for s in addrs {
                        send_line(format!("Address: {}", s.ip()), false, false).await;
                    }
                    send_line("DNS lookup complete".to_string(), true, false).await;
                }
                Err(e) => {
                    send_line(format!("DNS lookup failed: {e}"), true, true).await;
                }
            }
        }
        v1::ToolKind::Unspecified => {
            send_line("Unspecified tool kind".to_string(), true, true).await;
        }
    }
}

fn default_target(host: &str) -> ProbeTarget {
    ProbeTarget {
        device_id: 0,
        host: host.to_string(),
        resolved_ip: host.parse::<IpAddr>().ok(),
        snmp_port: 161,
        snmp_timeout: Duration::from_millis(2000),
        snmp_retries: 1,
        snmp_credential: ResolvedCredential::V2c {
            community: "public".to_string(),
        },
    }
}

async fn run_traceroute(dest: IpAddr, tx: &mpsc::Sender<Result<v1::ToolOutputChunk, Status>>) {
    let IpAddr::V4(dest_v4) = dest else {
        let _ = tx
            .send(Ok(v1::ToolOutputChunk {
                line: "IPv4 traceroute is active; IPv6 ICMP TTL traceroute requires raw ICMPv6 socket".to_string(),
                finished: true,
                error: false,
            }))
            .await;
        return;
    };

    let Ok(raw_sock) = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::ICMPV4)) else {
        let _ = tx
            .send(Ok(v1::ToolOutputChunk {
                line: "Traceroute requires CAP_NET_RAW to read intermediate ICMP TTL-exceeded replies".to_string(),
                finished: true,
                error: true,
            }))
            .await;
        return;
    };
    if raw_sock.set_nonblocking(true).is_err()
        || raw_sock
            .bind(&SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0).into())
            .is_err()
    {
        let _ = tx
            .send(Ok(v1::ToolOutputChunk {
                line: "Failed configuring raw ICMP socket for traceroute".to_string(),
                finished: true,
                error: true,
            }))
            .await;
        return;
    }

    let _ = tx
        .send(Ok(v1::ToolOutputChunk {
            line: format!("Traceroute to {dest_v4} (max 30 hops):"),
            finished: false,
            error: false,
        }))
        .await;

    let ident = (std::process::id() & 0xFFFF) as u16;
    let std_udp: std::net::UdpSocket = raw_sock.into();
    let Ok(async_sock) = UdpSocket::from_std(std_udp) else {
        return;
    };

    for ttl in 1u32..=30 {
        let sock_ref = socket2::SockRef::from(&async_sock);
        let _ = sock_ref.set_ttl_v4(ttl);

        let seq = ttl as u16;
        let pkt = build_icmp_v4_echo(ident, seq, b"NDTrace");
        let start = Instant::now();
        let _ = async_sock
            .send_to(&pkt, SocketAddr::new(IpAddr::V4(dest_v4), 0))
            .await;

        let mut buf = [0u8; 1024];
        let mut hop_ip: Option<IpAddr> = None;
        let mut reached = false;
        let deadline = Instant::now() + Duration::from_millis(1200);

        while Instant::now() < deadline {
            let rem = deadline.saturating_duration_since(Instant::now());
            match timeout(rem, async_sock.recv_from(&mut buf)).await {
                Ok(Ok((n, src))) => {
                    if let Some((rx_id, rx_seq)) = parse_icmp_v4_echo_reply(&buf[..n], true) {
                        if rx_id == ident && rx_seq == seq {
                            hop_ip = Some(src.ip());
                            reached = true;
                            break;
                        }
                    }
                    // Check ICMP Type 11 Time Exceeded wrapping original IPv4 + 8 bytes ICMP
                    if n >= 20 {
                        let ihl = usize::from(buf[0] & 0x0F) * 4;
                        if ihl >= 20 && n >= ihl + 8 && buf[ihl] == 11 {
                            hop_ip = Some(src.ip());
                            break;
                        }
                    }
                }
                _ => break,
            }
        }

        let rtt_ms = start.elapsed().as_secs_f64() * 1000.0;
        let line = match hop_ip {
            Some(ip) => format!("{ttl:>2}  {ip:<16}  {rtt_ms:.2} ms"),
            None => format!("{ttl:>2}  * * *"),
        };
        let _ = tx
            .send(Ok(v1::ToolOutputChunk {
                line,
                finished: false,
                error: false,
            }))
            .await;
        if reached {
            break;
        }
    }

    let _ = tx
        .send(Ok(v1::ToolOutputChunk {
            line: "Traceroute complete".to_string(),
            finished: true,
            error: false,
        }))
        .await;
}
