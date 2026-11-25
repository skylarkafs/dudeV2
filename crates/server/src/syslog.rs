use crate::engine::{EngineContext, ServerBroadcastEvent};
use crate::notify::AlertDispatch;
use crate::storage::telemetry::insert_syslog_batch;
use nextdude_common::proto::v1;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{info, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSyslog {
    pub facility: u32,
    pub severity: u32,
    pub hostname: String,
    pub app_name: String,
    pub message: String,
    pub format: &'static str,
}

/// Tolerant parser for RFC 3164, RFC 5424, and MikroTik/Cisco/Juniper/Huawei syslog variants.
pub fn parse_syslog_line(raw: &str) -> ParsedSyslog {
    let trimmed = raw.trim_end_matches(['\r', '\n', '\0']);
    let (pri, rest) = if let Some(after_lt) = trimmed.strip_prefix('<') {
        if let Some(gt) = after_lt.find('>') {
            let p = after_lt[..gt].parse::<u32>().unwrap_or(13);
            (p, after_lt[gt + 1..].trim_start())
        } else {
            (13, trimmed)
        }
    } else {
        (13, trimmed)
    };
    let facility = pri >> 3;
    let severity = pri & 0x07;

    // RFC 5424 starts with version digit '1' followed by space + ISO timestamp
    if let Some(after_ver) = rest.strip_prefix("1 ") {
        let mut parts = after_ver.splitn(6, ' ');
        let _ts = parts.next().unwrap_or("-");
        let host = parts.next().unwrap_or("-");
        let app = parts.next().unwrap_or("-");
        let _procid = parts.next().unwrap_or("-");
        let _msgid = parts.next().unwrap_or("-");
        let mut msg_rest = parts.next().unwrap_or("");
        // Strip optional SD-ELEMENT [..]
        if msg_rest.starts_with('[') {
            if let Some(end_sd) = msg_rest.find("] ") {
                msg_rest = &msg_rest[end_sd + 2..];
            }
        } else if let Some(stripped) = msg_rest.strip_prefix("- ") {
            msg_rest = stripped;
        }
        return ParsedSyslog {
            facility,
            severity,
            hostname: clean_nil(host),
            app_name: clean_nil(app),
            message: msg_rest.to_string(),
            format: "rfc5424",
        };
    }

    // Cisco IOS optional seq/timestamp prefix e.g. "123: *Mar  1 00:00:01.000: %LINK-3-UPDOWN: ..."
    if rest.contains(": %") {
        let split_pct = rest.split_once(": %");
        if let Some((prefix, body)) = split_pct {
            let mnemonic = body.split(':').next().unwrap_or("IOS").to_string();
            return ParsedSyslog {
                facility,
                severity,
                hostname: prefix
                    .split_whitespace()
                    .next()
                    .filter(|s| !s.ends_with(':') && !s.starts_with('*'))
                    .unwrap_or("")
                    .to_string(),
                app_name: mnemonic,
                message: format!("%{body}"),
                format: "cisco_3164",
            };
        }
    }

    // Standard RFC 3164: "Mmm dd HH:MM:SS hostname tag: message"
    // MikroTik often omits timestamp and sends "<pri>topics: message" or "hostname topics: message"
    let after_ts = strip_bsd_timestamp(rest).unwrap_or(rest);
    if let Some((first_tok, remainder)) = after_ts.split_once(' ') {
        if let Some(tag_end) = remainder.find(": ") {
            let tag = &remainder[..tag_end];
            let msg = &remainder[tag_end + 2..];
            return ParsedSyslog {
                facility,
                severity,
                hostname: first_tok.trim_end_matches(':').to_string(),
                app_name: tag.to_string(),
                message: msg.to_string(),
                format: "rfc3164",
            };
        }
    }

    // Fallback (e.g. MikroTik "system,info,account user admin logged in")
    if let Some((tag, msg)) = after_ts.split_once(": ") {
        return ParsedSyslog {
            facility,
            severity,
            hostname: String::new(),
            app_name: tag.to_string(),
            message: msg.to_string(),
            format: "mikrotik_3164",
        };
    }

    ParsedSyslog {
        facility,
        severity,
        hostname: String::new(),
        app_name: String::new(),
        message: rest.to_string(),
        format: "rfc3164",
    }
}

fn clean_nil(s: &str) -> String {
    if s == "-" {
        String::new()
    } else {
        s.to_string()
    }
}

fn strip_bsd_timestamp(s: &str) -> Option<&str> {
    let months = [
        "Jan ", "Feb ", "Mar ", "Apr ", "May ", "Jun ", "Jul ", "Aug ", "Sep ", "Oct ", "Nov ",
        "Dec ",
    ];
    if !months.iter().any(|m| s.starts_with(m)) || s.len() < 16 {
        return None;
    }
    // Check HH:MM:SS at offset 7..15
    let bytes = s.as_bytes();
    if bytes.get(9) == Some(&b':') && bytes.get(12) == Some(&b':') && bytes.get(15) == Some(&b' ')
    {
        Some(s[16..].trim_start())
    } else {
        None
    }
}

pub async fn run_syslog_receiver(ctx: EngineContext, stats: Arc<(AtomicU64, AtomicU64)>) {
    if !ctx.config.syslog_enabled {
        info!("Syslog receiver disabled by configuration");
        return;
    }
    let bind_ip: IpAddr = ctx
        .config
        .syslog_bind_addr
        .parse()
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    let addr = SocketAddr::new(bind_ip, ctx.config.syslog_port);
    let sock = match UdpSocket::bind(addr).await {
        Ok(s) => s,
        Err(e) => {
            warn!("failed binding syslog UDP {addr}: {e}");
            return;
        }
    };
    info!("Syslog UDP receiver listening on {addr}");

    let (tx, mut rx) = mpsc::channel::<(IpAddr, Vec<u8>)>(ctx.config.syslog_queue_capacity);
    let stats_rx = Arc::clone(&stats);
    let ctx_worker = ctx.clone();

    tokio::spawn(async move {
        let forward_sock = UdpSocket::bind("0.0.0.0:0").await.ok();
        let mut batch = Vec::with_capacity(64);
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
        loop {
            tokio::select! {
                maybe_pkt = rx.recv() => {
                    let Some((src_ip, raw)) = maybe_pkt else { break };
                    let text = String::from_utf8_lossy(&raw).into_owned();
                    let parsed = parse_syslog_line(&text);
                    let dev_match = ctx_worker.device_host_map.read().await.get(&src_ip).cloned();
                    let (dev_id, dev_name) = match dev_match {
                        Some((id, name)) => (Some(id), name),
                        None => (None, String::new()),
                    };

                    if let Some(ref fsock) = forward_sock {
                        if !ctx_worker.config.syslog_forward_addr.is_empty() {
                            let _ = fsock.send_to(&raw, &ctx_worker.config.syslog_forward_addr).await;
                        }
                    }

                    let msg_lower = parsed.message.to_ascii_lowercase();
                    if msg_lower.contains("link down") || msg_lower.contains("login failure") || msg_lower.contains("authentication failure") {
                        let _ = ctx_worker.notifications.dispatch(AlertDispatch {
                            severity: v1::Severity::Warning,
                            device_id: dev_id,
                            device_name: if dev_name.is_empty() { src_ip.to_string() } else { dev_name.clone() },
                            title: "Syslog Pattern Alert".to_string(),
                            message: format!("[{}] {}", src_ip, parsed.message),
                            channel_ids: Vec::new(),
                        }).await;
                    }

                    batch.push(v1::SyslogMessage {
                        id: 0,
                        timestamp_utc: chrono::Utc::now().timestamp(),
                        source_ip: src_ip.to_string(),
                        device_id: dev_id,
                        device_name: dev_name,
                        facility: parsed.facility,
                        severity: parsed.severity,
                        hostname: parsed.hostname,
                        app_name: parsed.app_name,
                        message: parsed.message,
                        raw_format: parsed.format.to_string(),
                    });
                    if batch.len() >= 64 {
                        flush_syslog(&ctx_worker, &mut batch).await;
                    }
                }
                _ = interval.tick() => {
                    flush_syslog(&ctx_worker, &mut batch).await;
                    ctx_worker.broadcast(ServerBroadcastEvent::SyslogStats(v1::SyslogDropStats {
                        received_total: stats_rx.0.load(Ordering::Relaxed),
                        dropped_total: stats_rx.1.load(Ordering::Relaxed),
                    }));
                }
            }
        }
    });

    let mut buf = [0u8; 8192];
    loop {
        match sock.recv_from(&mut buf).await {
            Ok((n, src)) => {
                stats.0.fetch_add(1, Ordering::Relaxed);
                if tx.try_send((src.ip(), buf[..n].to_vec())).is_err() {
                    stats.1.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(e) => {
                warn!("syslog recv error: {e}");
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }
}

async fn flush_syslog(ctx: &EngineContext, batch: &mut Vec<v1::SyslogMessage>) {
    if batch.is_empty() {
        return;
    }
    let to_write = std::mem::take(batch);
    if let Ok(saved) = ctx
        .storage
        .write(move |c| insert_syslog_batch(c, &to_write))
        .await
    {
        for s in saved {
            ctx.broadcast(ServerBroadcastEvent::Syslog(s));
        }
    }
}
