use crate::error::ServerError;
use crate::probe::{LocalProbeExecutor, ProbeOutcome, ProbeTarget};
use crate::snmp::ber::parse_oid;
use crate::snmp::SnmpTarget;
use nextdude_common::proto::v1;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{lookup_host, TcpStream, UdpSocket};
use tokio::time::timeout;

pub async fn run_probe(
    exec: &LocalProbeExecutor,
    target: &ProbeTarget,
    probe: &v1::ProbeDefinition,
    interface_key: &str,
) -> Result<ProbeOutcome, ServerError> {
    let wait = Duration::from_millis(u64::from(probe.timeout_ms.max(200)));
    let kind = v1::ProbeKind::try_from(probe.kind).unwrap_or(v1::ProbeKind::Unspecified);

    match kind {
        v1::ProbeKind::Icmp => {
            let ip = resolve_target_ip(exec, target).await?;
            let rtt = exec.icmp.ping(ip, wait).await?;
            let rtt_ms = rtt.as_secs_f64() * 1000.0;
            Ok(ProbeOutcome {
                success: true,
                rtt_ms,
                value: rtt_ms,
                text: format!("{rtt_ms:.2} ms"),
                force_partial: false,
            })
        }
        v1::ProbeKind::Tcp => {
            let ip = resolve_target_ip(exec, target).await?;
            let port = if probe.port == 0 { 80 } else { probe.port as u16 };
            let start = Instant::now();
            timeout(wait, TcpStream::connect(SocketAddr::new(ip, port)))
                .await
                .map_err(|_| ServerError::Probe("TCP connect timeout".to_string()))?
                .map_err(|e| ServerError::Probe(format!("TCP connect failed: {e}")))?;
            let rtt_ms = start.elapsed().as_secs_f64() * 1000.0;
            Ok(ProbeOutcome {
                success: true,
                rtt_ms,
                value: rtt_ms,
                text: format!("open ({rtt_ms:.1} ms)"),
                force_partial: false,
            })
        }
        v1::ProbeKind::Http | v1::ProbeKind::Https => {
            let scheme = if kind == v1::ProbeKind::Https {
                "https"
            } else {
                "http"
            };
            let default_port = if kind == v1::ProbeKind::Https { 443 } else { 80 };
            let port = if probe.port == 0 {
                default_port
            } else {
                probe.port as u16
            };
            let path = if probe.target_path_or_query.is_empty() {
                "/"
            } else {
                probe.target_path_or_query.as_str()
            };
            let url = format!("{scheme}://{}:{port}{path}", target.host);
            let start = Instant::now();
            let resp = timeout(wait, exec.http.get(&url).send())
                .await
                .map_err(|_| ServerError::Probe("HTTP timeout".to_string()))?
                .map_err(|e| ServerError::Probe(format!("HTTP request failed: {e}")))?;
            let status = resp.status();
            let body = timeout(wait, resp.text())
                .await
                .map_err(|_| ServerError::Probe("HTTP body timeout".to_string()))?
                .map_err(|e| ServerError::Probe(format!("HTTP read failed: {e}")))?;
            let rtt_ms = start.elapsed().as_secs_f64() * 1000.0;
            let pattern_ok = probe.expected_pattern.is_empty() || body.contains(&probe.expected_pattern);
            let ok = status.is_success() && pattern_ok;
            Ok(ProbeOutcome {
                success: ok,
                rtt_ms,
                value: f64::from(status.as_u16()),
                text: format!("HTTP {} ({rtt_ms:.1} ms)", status.as_u16()),
                force_partial: false,
            })
        }
        v1::ProbeKind::Dns => {
            let ip = resolve_target_ip(exec, target).await?;
            let port = if probe.port == 0 { 53 } else { probe.port as u16 };
            let qname = if probe.target_path_or_query.is_empty() {
                "example.com"
            } else {
                probe.target_path_or_query.as_str()
            };
            let start = Instant::now();
            let answers = query_dns_udp(SocketAddr::new(ip, port), qname, wait).await?;
            let rtt_ms = start.elapsed().as_secs_f64() * 1000.0;
            Ok(ProbeOutcome {
                success: answers > 0,
                rtt_ms,
                value: rtt_ms,
                text: format!("{answers} ans ({rtt_ms:.1} ms)"),
                force_partial: false,
            })
        }
        v1::ProbeKind::Ftp => {
            banner_probe(exec, target, probe, 21, "220", Some("QUIT\r\n"), wait).await
        }
        v1::ProbeKind::Smtp => {
            banner_probe(exec, target, probe, 25, "220", Some("QUIT\r\n"), wait).await
        }
        v1::ProbeKind::Imap => {
            banner_probe(exec, target, probe, 143, "* OK", Some("a1 LOGOUT\r\n"), wait).await
        }
        v1::ProbeKind::Pop3 => {
            banner_probe(exec, target, probe, 110, "+OK", Some("QUIT\r\n"), wait).await
        }
        v1::ProbeKind::Ssh => {
            banner_probe(exec, target, probe, 22, "SSH-", None, wait).await
        }
        v1::ProbeKind::Telnet => {
            let ip = resolve_target_ip(exec, target).await?;
            let port = if probe.port == 0 { 23 } else { probe.port as u16 };
            let start = Instant::now();
            let mut stream = timeout(wait, TcpStream::connect(SocketAddr::new(ip, port)))
                .await
                .map_err(|_| ServerError::Probe("Telnet connect timeout".to_string()))?
                .map_err(|e| ServerError::Probe(format!("Telnet connect failed: {e}")))?;
            let mut buf = [0u8; 256];
            let _ = timeout(Duration::from_millis(800), stream.read(&mut buf)).await;
            let rtt_ms = start.elapsed().as_secs_f64() * 1000.0;
            Ok(ProbeOutcome {
                success: true,
                rtt_ms,
                value: rtt_ms,
                text: format!("telnet up ({rtt_ms:.1} ms)"),
                force_partial: false,
            })
        }
        v1::ProbeKind::Ntp => {
            let ip = resolve_target_ip(exec, target).await?;
            let port = if probe.port == 0 { 123 } else { probe.port as u16 };
            let start = Instant::now();
            let stratum = query_ntp_udp(SocketAddr::new(ip, port), wait).await?;
            let rtt_ms = start.elapsed().as_secs_f64() * 1000.0;
            Ok(ProbeOutcome {
                success: stratum > 0 && stratum < 16,
                rtt_ms,
                value: f64::from(stratum),
                text: format!("stratum {stratum} ({rtt_ms:.1} ms)"),
                force_partial: false,
            })
        }
        v1::ProbeKind::SnmpOid | v1::ProbeKind::SnmpCustom => {
            let ip = resolve_target_ip(exec, target).await?;
            let oid = parse_oid(&probe.snmp_oid)?;
            let snmp_target = SnmpTarget {
                addr: SocketAddr::new(ip, target.snmp_port),
                timeout: target.snmp_timeout,
                retries: target.snmp_retries,
                credential: target.snmp_credential.clone(),
            };
            let start = Instant::now();
            let vbs = exec.snmp.get(&snmp_target, &[oid]).await?;
            let rtt_ms = start.elapsed().as_secs_f64() * 1000.0;
            let Some(vb) = vbs.into_iter().next() else {
                return Err(ServerError::Snmp("empty SNMP response".to_string()));
            };
            let raw_num = vb.value.as_f64().unwrap_or(0.0);
            let scale = if probe.snmp_scale == 0.0 { 1.0 } else { probe.snmp_scale };
            let scaled = raw_num * scale;
            let mut text = if probe.unit.is_empty() {
                vb.value.as_string_lossy()
            } else {
                format!("{scaled:.2} {}", probe.unit)
            };
            if !probe.expected_pattern.is_empty() && !vb.value.as_string_lossy().contains(&probe.expected_pattern) {
                return Ok(ProbeOutcome {
                    success: false,
                    rtt_ms,
                    value: scaled,
                    text: format!("mismatch: {text}"),
                    force_partial: false,
                });
            }
            let mut force_partial = false;
            if let Some(crit) = probe.crit_threshold {
                if scaled >= crit {
                    return Ok(ProbeOutcome {
                        success: false,
                        rtt_ms,
                        value: scaled,
                        text: format!("CRIT {text}"),
                        force_partial: false,
                    });
                }
            }
            if let Some(warn) = probe.warn_threshold {
                if scaled >= warn {
                    force_partial = true;
                    text = format!("WARN {text}");
                }
            }
            Ok(ProbeOutcome {
                success: true,
                rtt_ms,
                value: scaled,
                text,
                force_partial,
            })
        }
        v1::ProbeKind::SnmpInterface => {
            let ip = resolve_target_ip(exec, target).await?;
            let snmp_target = SnmpTarget {
                addr: SocketAddr::new(ip, target.snmp_port),
                timeout: target.snmp_timeout,
                retries: target.snmp_retries,
                credential: target.snmp_credential.clone(),
            };
            let start = Instant::now();
            // Walk ifName (1.3.6.1.2.1.31.1.1.1.1) or ifDescr (1.3.6.1.2.1.2.2.1.2)
            let name_oid = parse_oid("1.3.6.1.2.1.31.1.1.1.1")?;
            let names = exec.snmp.walk(&snmp_target, &name_oid).await?;
            let matched_idx = names
                .iter()
                .find(|vb| vb.value.as_string_lossy() == interface_key)
                .and_then(|vb| vb.oid.last().copied());
            let Some(if_index) = matched_idx else {
                return Err(ServerError::Probe(format!(
                    "interface '{interface_key}' not found on device"
                )));
            };
            let oper_oid = parse_oid(&format!("1.3.6.1.2.1.2.2.1.8.{if_index}"))?;
            let resp = exec.snmp.get(&snmp_target, &[oper_oid]).await?;
            let rtt_ms = start.elapsed().as_secs_f64() * 1000.0;
            let oper = resp.first().and_then(|v| v.value.as_i64()).unwrap_or(2);
            Ok(ProbeOutcome {
                success: oper == 1,
                rtt_ms,
                value: oper as f64,
                text: if oper == 1 {
                    format!("{interface_key} up")
                } else {
                    format!("{interface_key} down (oper={oper})")
                },
                force_partial: false,
            })
        }
        v1::ProbeKind::Unspecified => Err(ServerError::Probe("unspecified probe kind".to_string())),
    }
}

async fn banner_probe(
    exec: &LocalProbeExecutor,
    target: &ProbeTarget,
    probe: &v1::ProbeDefinition,
    default_port: u16,
    default_expect: &str,
    quit_cmd: Option<&str>,
    wait: Duration,
) -> Result<ProbeOutcome, ServerError> {
    let ip = resolve_target_ip(exec, target).await?;
    let port = if probe.port == 0 {
        default_port
    } else {
        probe.port as u16
    };
    let start = Instant::now();
    let mut stream = timeout(wait, TcpStream::connect(SocketAddr::new(ip, port)))
        .await
        .map_err(|_| ServerError::Probe("banner connect timeout".to_string()))?
        .map_err(|e| ServerError::Probe(format!("connect failed: {e}")))?;

    let mut buf = [0u8; 512];
    let n = timeout(wait, stream.read(&mut buf))
        .await
        .map_err(|_| ServerError::Probe("banner read timeout".to_string()))?
        .map_err(|e| ServerError::Probe(format!("banner read failed: {e}")))?;
    let banner = String::from_utf8_lossy(&buf[..n]).trim().to_string();
    if let Some(cmd) = quit_cmd {
        let _ = stream.write_all(cmd.as_bytes()).await;
    }
    let rtt_ms = start.elapsed().as_secs_f64() * 1000.0;
    let expect = if probe.expected_pattern.is_empty() {
        default_expect
    } else {
        probe.expected_pattern.as_str()
    };
    let ok = banner.contains(expect);
    Ok(ProbeOutcome {
        success: ok,
        rtt_ms,
        value: rtt_ms,
        text: banner,
        force_partial: false,
    })
}

pub async fn resolve_target_ip(
    exec: &LocalProbeExecutor,
    target: &ProbeTarget,
) -> Result<IpAddr, ServerError> {
    if let Ok(ip) = target.host.parse::<IpAddr>() {
        return Ok(ip);
    }
    if let Some(ip) = target.resolved_ip {
        return Ok(ip);
    }
    if let Some(ref resolver) = exec.dns {
        if let Ok(resp) = resolver.lookup_ip(target.host.as_str()).await {
            if let Some(ip) = resp.iter().next() {
                return Ok(ip);
            }
        }
    }
    let mut addrs = lookup_host((target.host.as_str(), 0))
        .await
        .map_err(|e| ServerError::Probe(format!("DNS resolve failed for {}: {e}", target.host)))?;
    addrs
        .next()
        .map(|s| s.ip())
        .ok_or_else(|| ServerError::Probe(format!("no IP address resolved for {}", target.host)))
}

async fn query_dns_udp(server: SocketAddr, qname: &str, wait: Duration) -> Result<u16, ServerError> {
    let bind_addr: SocketAddr = match server {
        SocketAddr::V4(_) => "0.0.0.0:0".parse().map_err(|_| ServerError::Probe("bind parse".into()))?,
        SocketAddr::V6(_) => "[::]:0".parse().map_err(|_| ServerError::Probe("bind parse".into()))?,
    };
    let sock = UdpSocket::bind(bind_addr).await?;
    let tx_id: u16 = 0x4E44;
    let mut pkt = Vec::with_capacity(64);
    pkt.extend_from_slice(&tx_id.to_be_bytes());
    pkt.extend_from_slice(&0x0100u16.to_be_bytes()); // standard recursive query
    pkt.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    pkt.extend_from_slice(&[0u8; 6]); // AN/NS/AR counts
    for label in qname.trim_end_matches('.').split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(ServerError::Probe("invalid DNS query name".to_string()));
        }
        pkt.push(label.len() as u8);
        pkt.extend_from_slice(label.as_bytes());
    }
    pkt.push(0);
    pkt.extend_from_slice(&1u16.to_be_bytes()); // QTYPE = A
    pkt.extend_from_slice(&1u16.to_be_bytes()); // QCLASS = IN

    sock.send_to(&pkt, server).await?;
    let mut resp = [0u8; 512];
    let (n, _) = timeout(wait, sock.recv_from(&mut resp))
        .await
        .map_err(|_| ServerError::Probe("DNS query timeout".to_string()))??;
    if n < 12 {
        return Err(ServerError::Probe("truncated DNS response".to_string()));
    }
    let resp_id = u16::from_be_bytes([resp[0], resp[1]]);
    if resp_id != tx_id {
        return Err(ServerError::Probe("mismatched DNS transaction ID".to_string()));
    }
    let rcode = resp[3] & 0x0F;
    if rcode != 0 {
        return Err(ServerError::Probe(format!("DNS error RCODE={rcode}")));
    }
    Ok(u16::from_be_bytes([resp[6], resp[7]]))
}

async fn query_ntp_udp(server: SocketAddr, wait: Duration) -> Result<u8, ServerError> {
    let bind_addr: SocketAddr = match server {
        SocketAddr::V4(_) => "0.0.0.0:0".parse().map_err(|_| ServerError::Probe("bind parse".into()))?,
        SocketAddr::V6(_) => "[::]:0".parse().map_err(|_| ServerError::Probe("bind parse".into()))?,
    };
    let sock = UdpSocket::bind(bind_addr).await?;
    let mut req = [0u8; 48];
    req[0] = 0x23; // LI=0, VN=4, Mode=3 (client)
    sock.send_to(&req, server).await?;
    let mut resp = [0u8; 48];
    let (n, _) = timeout(wait, sock.recv_from(&mut resp))
        .await
        .map_err(|_| ServerError::Probe("NTP timeout".to_string()))??;
    if n < 48 {
        return Err(ServerError::Probe("truncated NTP response".to_string()));
    }
    let mode = resp[0] & 0x07;
    if mode != 4 {
        return Err(ServerError::Probe(format!("unexpected NTP mode {mode}")));
    }
    Ok(resp[1])
}
