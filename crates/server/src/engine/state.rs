use nextdude_common::proto::v1;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct DebounceResult {
    pub new_state: v1::ProbeState,
    pub consecutive_failures: u32,
    pub consecutive_successes: u32,
    pub transitioned: bool,
}

/// Evaluates probe state transitions with configurable up/down debounce thresholds.
pub fn advance_probe_state(
    current: v1::ProbeState,
    failures: u32,
    successes: u32,
    down_threshold: u32,
    up_threshold: u32,
    probe_ok: bool,
    force_partial: bool,
) -> DebounceResult {
    let down_req = down_threshold.max(1);
    let up_req = up_threshold.max(1);

    if probe_ok {
        let next_succ = successes.saturating_add(1);
        let next_state = if force_partial {
            v1::ProbeState::Partial
        } else if current == v1::ProbeState::Up || next_succ >= up_req {
            v1::ProbeState::Up
        } else {
            current
        };
        DebounceResult {
            new_state: next_state,
            consecutive_failures: 0,
            consecutive_successes: next_succ,
            transitioned: next_state != current,
        }
    } else {
        let next_fail = failures.saturating_add(1);
        let next_state = if current == v1::ProbeState::Down || next_fail >= down_req {
            v1::ProbeState::Down
        } else if current == v1::ProbeState::Unknown {
            v1::ProbeState::Unknown
        } else {
            current
        };
        DebounceResult {
            new_state: next_state,
            consecutive_failures: next_fail,
            consecutive_successes: 0,
            transitioned: next_state != current,
        }
    }
}

pub fn aggregate_device_state(
    service_states: &[v1::ProbeState],
    suppressed_by_parent: bool,
) -> v1::ProbeState {
    if suppressed_by_parent {
        return v1::ProbeState::Unreachable;
    }
    if service_states.is_empty() {
        return v1::ProbeState::Unknown;
    }
    let total = service_states.len();
    let down_count = service_states
        .iter()
        .filter(|&&s| s == v1::ProbeState::Down)
        .count();
    let up_count = service_states
        .iter()
        .filter(|&&s| s == v1::ProbeState::Up)
        .count();
    let partial_count = service_states
        .iter()
        .filter(|&&s| s == v1::ProbeState::Partial)
        .count();

    if down_count == total {
        v1::ProbeState::Down
    } else if down_count > 0 || partial_count > 0 {
        v1::ProbeState::Partial
    } else if up_count > 0 {
        v1::ProbeState::Up
    } else {
        v1::ProbeState::Unknown
    }
}

#[derive(Debug, Clone)]
pub struct FlappingTracker {
    changes: VecDeque<Instant>,
    pub is_flapping: bool,
    pub notified_current_flap: bool,
}

impl Default for FlappingTracker {
    fn default() -> Self {
        Self {
            changes: VecDeque::new(),
            is_flapping: false,
            notified_current_flap: false,
        }
    }
}

impl FlappingTracker {
    pub fn record_change(
        &mut self,
        now: Instant,
        threshold_changes: u32,
        window: Duration,
    ) -> bool {
        self.changes.push_back(now);
        self.prune(now, window);
        let was_flapping = self.is_flapping;
        if self.changes.len() > threshold_changes as usize {
            self.is_flapping = true;
        }
        !was_flapping && self.is_flapping
    }

    pub fn refresh(&mut self, now: Instant, threshold_changes: u32, window: Duration) {
        self.prune(now, window);
        if self.changes.len() <= (threshold_changes as usize / 2) {
            self.is_flapping = false;
            self.notified_current_flap = false;
        }
    }

    fn prune(&mut self, now: Instant, window: Duration) {
        while let Some(&front) = self.changes.front() {
            if now.saturating_duration_since(front) > window {
                self.changes.pop_front();
            } else {
                break;
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CounterSample {
    pub value: u64,
    pub is_64bit: bool,
    pub uptime_cs: Option<u64>,
    pub at: Instant,
}

/// Computes delta bytes handling 64-bit/32-bit counter wraps and device reboot resets.
pub fn compute_counter_bps(
    prev: Option<CounterSample>,
    curr: CounterSample,
    max_plausible_speed_bps: u64,
) -> Option<f64> {
    let prev = prev?;
    let dt = curr.at.saturating_duration_since(prev.at).as_secs_f64();
    if dt <= 0.05 {
        return None;
    }

    // If device sysUpTime went backwards, counters were reset by a reboot.
    if let (Some(p_up), Some(c_up)) = (prev.uptime_cs, curr.uptime_cs) {
        if c_up < p_up {
            return None;
        }
    }

    let delta_bytes = if curr.value >= prev.value {
        curr.value - prev.value
    } else if !curr.is_64bit && prev.value <= u64::from(u32::MAX) && curr.value <= u64::from(u32::MAX) {
        // 32-bit counter wraparound
        (u64::from(u32::MAX) - prev.value)
            .saturating_add(1)
            .saturating_add(curr.value)
    } else {
        // 64-bit wraparound or sudden counter reset; sanity-check against plausible max traffic
        let wrapped = (u64::MAX - prev.value)
            .saturating_add(1)
            .saturating_add(curr.value);
        let max_bytes = ((max_plausible_speed_bps.max(10_000_000_000) as f64) / 8.0) * dt * 2.0;
        if (wrapped as f64) <= max_bytes {
            wrapped
        } else {
            return None;
        }
    };

    let bps = (delta_bytes as f64 * 8.0) / dt;
    // Reject obvious counter-reset spikes beyond 2x physical ceiling.
    let ceiling = (max_plausible_speed_bps.max(100_000_000_000) as f64) * 2.0;
    if bps > ceiling {
        return None;
    }
    Some(bps)
}

pub fn stable_interface_key(if_name: &str, if_descr: &str, if_index: u32) -> String {
    let n = if_name.trim();
    if !n.is_empty() {
        return n.to_string();
    }
    let d = if_descr.trim();
    if !d.is_empty() {
        return d.to_string();
    }
    format!("ifIndex:{if_index}")
}

pub fn remap_interface_indices(
    old_by_key: &HashMap<String, u32>,
    new_entries: &[(u32, String, String)],
) -> HashMap<String, u32> {
    let mut mapped = HashMap::with_capacity(new_entries.len());
    for (idx, name, descr) in new_entries {
        let key = stable_interface_key(name, descr, *idx);
        let _ = old_by_key.get(&key);
        mapped.insert(key, *idx);
    }
    mapped
}

pub fn compute_link_status_and_util(
    iface_oper_up: Option<bool>,
    remote_reachable: Option<bool>,
    neighbor_present: Option<bool>,
    is_flapping: bool,
    in_bps: f64,
    out_bps: f64,
    effective_max_speed_bps: u64,
) -> (v1::LinkStatus, f64, f64) {
    let speed = effective_max_speed_bps.max(1) as f64;
    let in_util = ((in_bps / speed) * 100.0).clamp(0.0, 100.0);
    let out_util = ((out_bps / speed) * 100.0).clamp(0.0, 100.0);

    let status = if is_flapping {
        v1::LinkStatus::Flapping
    } else if iface_oper_up == Some(false) {
        v1::LinkStatus::Down
    } else if iface_oper_up == Some(true)
        && (remote_reachable == Some(false) || neighbor_present == Some(false))
    {
        v1::LinkStatus::NoReply
    } else if iface_oper_up == Some(true) || remote_reachable == Some(true) {
        v1::LinkStatus::Up
    } else {
        v1::LinkStatus::Unknown
    };

    (status, in_util, out_util)
}
