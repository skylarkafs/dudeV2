use nextdude_common::proto::v1;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone)]
pub enum DeviceImageSource {
    UploadedImage(i64),
    BuiltinIconKey(String),
}

#[derive(Debug, Clone, Default)]
pub struct ClientStateCache {
    pub last_seq: u64,
    pub needs_resync: bool,
    pub maps: BTreeMap<i64, v1::NetworkMap>,
    pub devices: BTreeMap<i64, v1::Device>,
    pub services: BTreeMap<i64, v1::DeviceService>,
    pub probes: BTreeMap<i64, v1::ProbeDefinition>,
    pub device_types: BTreeMap<i64, v1::DeviceType>,
    pub link_types: BTreeMap<i64, v1::LinkType>,
    pub links: BTreeMap<i64, v1::NetworkLink>,
    pub networks: BTreeMap<i64, v1::MapNetwork>,
    pub submaps: BTreeMap<i64, v1::SubmapItem>,
    pub static_items: BTreeMap<i64, v1::StaticItem>,
    pub dependencies: BTreeMap<i64, v1::Dependency>,
    pub interfaces_by_device: HashMap<i64, Vec<v1::DeviceInterface>>,
    pub alert_rules: BTreeMap<i64, v1::AlertRule>,
    pub maintenance_windows: BTreeMap<i64, v1::MaintenanceWindow>,
    pub outages: Vec<v1::OutageRecord>,
    pub charts: BTreeMap<i64, v1::ChartDefinition>,
    pub images: BTreeMap<i64, v1::ImageAssetMeta>,
    pub notification_channels: BTreeMap<i64, v1::NotificationChannel>,
    pub events: Vec<v1::EventLogEntry>,
    pub syslogs: Vec<v1::SyslogMessage>,
    pub traps: Vec<v1::TrapMessage>,
    pub syslog_stats: v1::SyslogDropStats,
    pub server_settings: Option<v1::ServerSettings>,
    pub discovery_progress: Option<v1::DiscoveryProgressEvent>,
    pub active_alarms: Vec<v1::AlertEvent>,
}

impl ClientStateCache {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Applies an incoming stream frame and verifies strict sequence continuity.
    pub fn apply_stream_event(&mut self, ev: v1::StreamEvent) -> Option<v1::AlertEvent> {
        if self.last_seq > 0 && ev.seq != self.last_seq + 1 {
            self.needs_resync = true;
            return None;
        }
        self.last_seq = ev.seq;

        let mut new_alarm = None;
        match ev.payload {
            Some(v1::stream_event::Payload::Snapshot(snap)) => {
                self.apply_snapshot(snap);
            }
            Some(v1::stream_event::Payload::LiveMetrics(batch)) => {
                self.apply_live_batch(batch);
            }
            Some(v1::stream_event::Payload::EntityDelta(delta)) => {
                self.apply_entity_delta(delta);
            }
            Some(v1::stream_event::Payload::EventLog(e)) => {
                self.events.insert(0, e);
                self.events.truncate(2000);
            }
            Some(v1::stream_event::Payload::SyslogEntry(s)) => {
                self.syslogs.insert(0, s);
                self.syslogs.truncate(2000);
            }
            Some(v1::stream_event::Payload::TrapEntry(t)) => {
                self.traps.insert(0, t);
                self.traps.truncate(1000);
            }
            Some(v1::stream_event::Payload::Alert(a)) => {
                self.active_alarms.insert(0, a.clone());
                self.active_alarms.truncate(200);
                new_alarm = Some(a);
            }
            Some(v1::stream_event::Payload::Outage(o)) => {
                if let Some(pos) = self.outages.iter().position(|x| x.id == o.id) {
                    self.outages[pos] = o;
                } else {
                    self.outages.insert(0, o);
                    self.outages.truncate(1000);
                }
            }
            Some(v1::stream_event::Payload::Discovery(dp)) => {
                self.discovery_progress = Some(dp);
            }
            Some(v1::stream_event::Payload::SyslogStats(st)) => {
                self.syslog_stats = st;
            }
            Some(v1::stream_event::Payload::ResyncRequired(_)) => {
                self.needs_resync = true;
            }
            Some(v1::stream_event::Payload::HeartbeatUtc(_)) | None => {}
        }
        new_alarm
    }

    pub fn apply_snapshot(&mut self, snap: v1::SnapshotPayload) {
        self.needs_resync = false;
        self.maps = snap.maps.into_iter().map(|x| (x.id, x)).collect();
        self.devices = snap.devices.into_iter().map(|x| (x.id, x)).collect();
        self.services = snap.services.into_iter().map(|x| (x.id, x)).collect();
        self.probes = snap.probes.into_iter().map(|x| (x.id, x)).collect();
        self.device_types = snap.device_types.into_iter().map(|x| (x.id, x)).collect();
        self.link_types = snap.link_types.into_iter().map(|x| (x.id, x)).collect();
        self.links = snap.links.into_iter().map(|x| (x.id, x)).collect();
        self.networks = snap.networks.into_iter().map(|x| (x.id, x)).collect();
        self.submaps = snap.submaps.into_iter().map(|x| (x.id, x)).collect();
        self.static_items = snap.static_items.into_iter().map(|x| (x.id, x)).collect();
        self.dependencies = snap.dependencies.into_iter().map(|x| (x.id, x)).collect();

        self.interfaces_by_device.clear();
        for iface in snap.interfaces {
            self.interfaces_by_device
                .entry(iface.device_id)
                .or_default()
                .push(iface);
        }

        self.alert_rules = snap.alert_rules.into_iter().map(|x| (x.id, x)).collect();
        self.maintenance_windows = snap
            .maintenance_windows
            .into_iter()
            .map(|x| (x.id, x))
            .collect();
        self.outages = snap.outages;
        self.charts = snap.charts.into_iter().map(|x| (x.id, x)).collect();
        self.images = snap.images.into_iter().map(|x| (x.id, x)).collect();
        self.notification_channels = snap
            .notification_channels
            .into_iter()
            .map(|x| (x.id, x))
            .collect();
        self.events = snap.recent_events;
        self.syslogs = snap.recent_syslog;
        self.traps = snap.recent_traps;
        if let Some(st) = snap.syslog_stats {
            self.syslog_stats = st;
        }
        self.server_settings = snap.server_settings;
    }

    fn apply_live_batch(&mut self, batch: v1::LiveMetricsBatch) {
        for d in batch.devices {
            if let Some(dev) = self.devices.get_mut(&d.device_id) {
                dev.aggregate_state = d.aggregate_state;
                dev.last_rtt_ms = d.last_rtt_ms;
                dev.last_seen_utc = d.last_seen_utc;
                if d.sys_uptime_cs > 0 {
                    dev.sys_uptime_cs = d.sys_uptime_cs;
                }
            }
        }
        for s in batch.services {
            if let Some(svc) = self.services.get_mut(&s.service_id) {
                svc.state = s.state;
                svc.last_value = s.value;
                svc.last_text = s.text;
                svc.last_rtt_ms = s.rtt_ms;
                svc.last_check_utc = s.checked_utc;
            }
        }
        for l in batch.links {
            if let Some(link) = self.links.get_mut(&l.link_id) {
                link.status = l.status;
                link.in_bps = l.in_bps;
                link.out_bps = l.out_bps;
                link.in_utilization_pct = l.in_util_pct;
                link.out_utilization_pct = l.out_util_pct;
            }
        }
    }

    fn apply_entity_delta(&mut self, delta: v1::EntityDelta) {
        if delta.deleted {
            match delta.entity_type.as_str() {
                "map" => {
                    self.maps.remove(&delta.entity_id);
                }
                "device" => {
                    self.devices.remove(&delta.entity_id);
                }
                "service" => {
                    self.services.remove(&delta.entity_id);
                }
                "probe" => {
                    self.probes.remove(&delta.entity_id);
                }
                "device_type" => {
                    self.device_types.remove(&delta.entity_id);
                }
                "link_type" => {
                    self.link_types.remove(&delta.entity_id);
                }
                "link" => {
                    self.links.remove(&delta.entity_id);
                }
                "network" => {
                    self.networks.remove(&delta.entity_id);
                }
                "submap" => {
                    self.submaps.remove(&delta.entity_id);
                }
                "static_item" => {
                    self.static_items.remove(&delta.entity_id);
                }
                "dependency" => {
                    self.dependencies.remove(&delta.entity_id);
                }
                "alert_rule" => {
                    self.alert_rules.remove(&delta.entity_id);
                }
                "maintenance" => {
                    self.maintenance_windows.remove(&delta.entity_id);
                }
                "notification_channel" => {
                    self.notification_channels.remove(&delta.entity_id);
                }
                "chart" => {
                    self.charts.remove(&delta.entity_id);
                }
                "image" => {
                    self.images.remove(&delta.entity_id);
                }
                _ => {}
            }
            return;
        }

        if let Some(v) = delta.map {
            self.maps.insert(v.id, v);
        }
        if let Some(v) = delta.device {
            self.devices.insert(v.id, v);
        }
        if let Some(v) = delta.service {
            self.services.insert(v.id, v);
        }
        if let Some(v) = delta.probe {
            self.probes.insert(v.id, v);
        }
        if let Some(v) = delta.device_type {
            self.device_types.insert(v.id, v);
        }
        if let Some(v) = delta.link_type {
            self.link_types.insert(v.id, v);
        }
        if let Some(v) = delta.link {
            self.links.insert(v.id, v);
        }
        if let Some(v) = delta.network {
            self.networks.insert(v.id, v);
        }
        if let Some(v) = delta.submap {
            self.submaps.insert(v.id, v);
        }
        if let Some(v) = delta.static_item {
            self.static_items.insert(v.id, v);
        }
        if let Some(v) = delta.dependency {
            self.dependencies.insert(v.id, v);
        }
        if let Some(v) = delta.alert_rule {
            self.alert_rules.insert(v.id, v);
        }
        if let Some(v) = delta.maintenance {
            self.maintenance_windows.insert(v.id, v);
        }
        if let Some(v) = delta.notification_channel {
            self.notification_channels.insert(v.id, v);
        }
        if let Some(v) = delta.chart {
            self.charts.insert(v.id, v);
        }
        if let Some(v) = delta.image {
            self.images.insert(v.id, v);
        }
        if let Some(v) = delta.server_settings {
            self.server_settings = Some(v);
        }
    }

    /// Resolves device image in exact priority order:
    /// 1) manual device image, 2) model-mapped image, 3) vendor-detected image, 4) device-type generic icon.
    pub fn resolve_device_image(&self, dev: &v1::Device) -> DeviceImageSource {
        if let Some(custom_id) = dev.custom_image_id {
            if self.images.contains_key(&custom_id) {
                return DeviceImageSource::UploadedImage(custom_id);
            }
        }
        if let Some(resolved_id) = dev.resolved_image_id {
            if self.images.contains_key(&resolved_id) {
                return DeviceImageSource::UploadedImage(resolved_id);
            }
        }
        if !dev.model.is_empty() {
            let model_l = dev.model.to_ascii_lowercase();
            if let Some(img) = self.images.values().find(|im| {
                !im.model.is_empty() && model_l.contains(&im.model.to_ascii_lowercase())
            }) {
                return DeviceImageSource::UploadedImage(img.id);
            }
        }
        if !dev.vendor.is_empty() {
            let vendor_l = dev.vendor.to_ascii_lowercase();
            if let Some(img) = self.images.values().find(|im| {
                im.model.is_empty()
                    && !im.vendor.is_empty()
                    && im.vendor.to_ascii_lowercase() == vendor_l
            }) {
                return DeviceImageSource::UploadedImage(img.id);
            }
        }
        if let Some(dt) = self.device_types.get(&dev.device_type_id) {
            if let Some(dt_img) = dt.default_image_id {
                if self.images.contains_key(&dt_img) {
                    return DeviceImageSource::UploadedImage(dt_img);
                }
            }
            if !dt.default_icon_key.is_empty() {
                return DeviceImageSource::BuiltinIconKey(dt.default_icon_key.clone());
            }
        }
        DeviceImageSource::BuiltinIconKey("generic".to_string())
    }
}
