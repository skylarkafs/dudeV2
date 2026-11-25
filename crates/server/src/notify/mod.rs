use crate::engine::ServerBroadcastEvent;
use crate::error::ServerError;
use crate::storage::telemetry::{insert_event_log, load_channel_secret, load_notification_channels};
use crate::storage::StorageHandle;
use nextdude_common::proto::v1;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::warn;

#[derive(Debug, Clone)]
pub struct AlertDispatch {
    pub severity: v1::Severity,
    pub device_id: Option<i64>,
    pub device_name: String,
    pub title: String,
    pub message: String,
    pub channel_ids: Vec<i64>,
}

#[derive(Debug, Clone)]
struct TelegramJob {
    bot_token: String,
    chat_id: String,
    text: String,
}

pub struct NotificationManager {
    storage: StorageHandle,
    events_tx: broadcast::Sender<ServerBroadcastEvent>,
    telegram_tx: mpsc::Sender<TelegramJob>,
    rate_windows: Arc<Mutex<HashMap<i64, Vec<Instant>>>>,
}

impl NotificationManager {
    pub fn new(
        storage: StorageHandle,
        events_tx: broadcast::Sender<ServerBroadcastEvent>,
    ) -> Arc<Self> {
        let (telegram_tx, telegram_rx) = mpsc::channel::<TelegramJob>(512);
        tokio::spawn(telegram_worker(telegram_rx));
        Arc::new(Self {
            storage,
            events_tx,
            telegram_tx,
            rate_windows: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub async fn dispatch(&self, alert: AlertDispatch) -> Result<(), ServerError> {
        let sev_i32 = alert.severity as i32;
        let dname = alert.device_name.clone();
        let msg = alert.message.clone();
        let did = alert.device_id;

        // Always record into server Event Log and broadcast live alert for connected Windows clients.
        let event = self
            .storage
            .write(move |c| insert_event_log(c, sev_i32, "alert", did, &dname, &msg))
            .await?;

        let _ = self
            .events_tx
            .send(ServerBroadcastEvent::EventLog(event.clone()));
        if alert.severity != v1::Severity::Info {
            let _ = self.events_tx.send(ServerBroadcastEvent::Alert(v1::AlertEvent {
                event_id: event.id,
                device_id: alert.device_id.unwrap_or(0),
                device_name: alert.device_name.clone(),
                title: alert.title.clone(),
                description: alert.message.clone(),
                severity: sev_i32,
                timestamp_utc: event.timestamp_utc,
            }));
        }

        let channels = self
            .storage
            .read(load_notification_channels)
            .await
            .unwrap_or_default();

        for ch in channels {
            if !ch.enabled {
                continue;
            }
            if !alert.channel_ids.is_empty() && !alert.channel_ids.contains(&ch.id) {
                continue;
            }
            if !self.check_rate_limit(ch.id, ch.rate_limit_per_min).await {
                continue;
            }
            match ch.kind.as_str() {
                "syslog_forward" => {
                    if !ch.target.is_empty() {
                        let target = ch.target.clone();
                        let line = format!(
                            "<14>{} {}: {}",
                            nextdude_common::APP_NAME,
                            alert.title,
                            alert.message
                        );
                        tokio::spawn(async move {
                            if let Ok(sock) = UdpSocket::bind("0.0.0.0:0").await {
                                let _ = sock.send_to(line.as_bytes(), &target).await;
                            }
                        });
                    }
                }
                "telegram" => {
                    let secrets = self.storage.secrets().clone();
                    let ch_id = ch.id;
                    if let Ok(token) = self
                        .storage
                        .read(move |c| load_channel_secret(c, &secrets, ch_id))
                        .await
                    {
                        if !token.is_empty() && !ch.target.is_empty() {
                            let job = TelegramJob {
                                bot_token: token,
                                chat_id: ch.target.clone(),
                                text: format!("[{}] {}\n{}", nextdude_common::APP_NAME, alert.title, alert.message),
                            };
                            if self.telegram_tx.try_send(job).is_err() {
                                warn!("Telegram notification queue full; dropping message");
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub async fn test_channel(&self, channel_id: i64) -> Result<(), ServerError> {
        let channels = self.storage.read(load_notification_channels).await?;
        let ch = channels
            .into_iter()
            .find(|c| c.id == channel_id)
            .ok_or_else(|| ServerError::NotFound(format!("notification channel {channel_id}")))?;
        match ch.kind.as_str() {
            "telegram" => {
                let secrets = self.storage.secrets().clone();
                let token = self
                    .storage
                    .read(move |c| load_channel_secret(c, &secrets, channel_id))
                    .await?;
                if token.is_empty() || ch.target.is_empty() {
                    return Err(ServerError::Config(
                        "Telegram bot token and chat_id must be configured".to_string(),
                    ));
                }
                send_telegram_once(
                    &reqwest::Client::new(),
                    &token,
                    &ch.target,
                    &format!("{} test notification OK", nextdude_common::APP_NAME),
                )
                .await
            }
            "syslog_forward" => {
                if ch.target.is_empty() {
                    return Err(ServerError::Config("syslog target host:port required".to_string()));
                }
                let sock = UdpSocket::bind("0.0.0.0:0").await?;
                sock.send_to(
                    format!("<14>{} test notification", nextdude_common::APP_NAME).as_bytes(),
                    &ch.target,
                )
                .await?;
                Ok(())
            }
            _ => {
                let _ = self
                    .storage
                    .write(|c| {
                        insert_event_log(
                            c,
                            v1::Severity::Info as i32,
                            "notification_test",
                            None,
                            "Server",
                            "Notification test event",
                        )
                    })
                    .await?;
                Ok(())
            }
        }
    }

    async fn check_rate_limit(&self, channel_id: i64, max_per_min: u32) -> bool {
        let now = Instant::now();
        let mut map = self.rate_windows.lock().await;
        let list = map.entry(channel_id).or_default();
        list.retain(|&t| now.saturating_duration_since(t) < Duration::from_secs(60));
        if list.len() >= max_per_min.max(1) as usize {
            return false;
        }
        list.push(now);
        true
    }
}

async fn telegram_worker(mut rx: mpsc::Receiver<TelegramJob>) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    while let Some(job) = rx.recv().await {
        let mut delay = Duration::from_secs(1);
        for attempt in 0..4 {
            match send_telegram_once(&client, &job.bot_token, &job.chat_id, &job.text).await {
                Ok(()) => break,
                Err(e) => {
                    warn!("Telegram send failed (attempt {}): {e}", attempt + 1);
                    if attempt < 3 {
                        tokio::time::sleep(delay).await;
                        delay *= 2;
                    }
                }
            }
        }
        // Respect Telegram per-bot burst limits
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn send_telegram_once(
    client: &reqwest::Client,
    token: &str,
    chat_id: &str,
    text: &str,
) -> Result<(), ServerError> {
    let url = format!("https://api.telegram.org/bot{token}/sendMessage");
    let resp = client
        .post(url)
        .json(&serde_json::json!({
            "chat_id": chat_id,
            "text": text,
        }))
        .send()
        .await
        .map_err(|e| ServerError::Probe(format!("Telegram HTTP error: {e}")))?;
    if !resp.status().is_success() {
        return Err(ServerError::Probe(format!(
            "Telegram API returned status {}",
            resp.status()
        )));
    }
    Ok(())
}
