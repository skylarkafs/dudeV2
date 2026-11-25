use nextdude_common::proto::v1;
use nextdude_server::engine::dependency::{compute_dependency_suppression, would_create_cycle};
use nextdude_server::engine::state::{
    advance_probe_state, compute_counter_bps, remap_interface_indices, CounterSample,
    FlappingTracker,
};
use nextdude_server::snmp::profiles::{
    ModelImageRule, VendorProfile, VendorProfileRegistry,
};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

#[test]
fn state_machine_debounce_and_flapping() {
    let mut st = v1::ProbeState::Up;
    let mut fail = 0;
    let mut succ = 0;

    // Require 3 consecutive failures before Down
    for _ in 0..2 {
        let r = advance_probe_state(st, fail, succ, 3, 1, false, false);
        st = r.new_state;
        fail = r.consecutive_failures;
        succ = r.consecutive_successes;
        assert_eq!(st, v1::ProbeState::Up);
    }
    let r3 = advance_probe_state(st, fail, succ, 3, 1, false, false);
    assert_eq!(r3.new_state, v1::ProbeState::Down);
    assert!(r3.transitioned);

    // 1 success recovers to Up
    let rec = advance_probe_state(r3.new_state, r3.consecutive_failures, 0, 3, 1, true, false);
    assert_eq!(rec.new_state, v1::ProbeState::Up);
    assert!(rec.transitioned);

    // Flapping detection (> 3 changes in 5 min)
    let mut tracker = FlappingTracker::default();
    let base = Instant::now();
    for i in 0..3 {
        assert!(!tracker.record_change(base + Duration::from_secs(i * 10), 3, Duration::from_secs(300)));
    }
    assert!(tracker.record_change(base + Duration::from_secs(35), 3, Duration::from_secs(300)));
    assert!(tracker.is_flapping);
}

#[test]
fn dependency_cycle_guard_and_root_cause_suppression() {
    let deps = vec![
        v1::Dependency {
            id: 1,
            version: 1,
            map_id: 1,
            parent_device_id: 10,
            child_device_id: 20,
        },
        v1::Dependency {
            id: 2,
            version: 1,
            map_id: 1,
            parent_device_id: 20,
            child_device_id: 30,
        },
    ];
    assert!(would_create_cycle(&deps, 30, 10));
    assert!(!would_create_cycle(&deps, 30, 40));

    let mut down = HashSet::new();
    down.insert(10);
    down.insert(20);
    let summary = compute_dependency_suppression(&deps, &down);
    assert_eq!(summary.suppressed_children.get(&20), Some(&10));
    assert_eq!(summary.suppressed_children.get(&30), Some(&10));
    assert_eq!(summary.affected_counts_by_root.get(&10), Some(&2));
}

#[test]
fn counter_wrap_and_reboot_reset_and_interface_remap() {
    let t0 = Instant::now();
    let t1 = t0 + Duration::from_secs(10);

    // 32-bit wrap: 4_294_967_000 -> 1000 = 1296 bytes in 10s = 1036.8 bps
    let p32 = CounterSample {
        value: u64::from(u32::MAX) - 295,
        is_64bit: false,
        uptime_cs: Some(10_000),
        at: t0,
    };
    let c32 = CounterSample {
        value: 1000,
        is_64bit: false,
        uptime_cs: Some(11_000),
        at: t1,
    };
    let bps = compute_counter_bps(Some(p32), c32, 1_000_000_000).unwrap();
    assert!((bps - (1296.0 * 8.0 / 10.0)).abs() < 0.01);

    // Reboot detected via sysUpTime decrease -> returns None (no fake spike)
    let reboot = CounterSample {
        value: 500,
        is_64bit: true,
        uptime_cs: Some(200),
        at: t1,
    };
    assert!(compute_counter_bps(Some(p32), reboot, 1_000_000_000).is_none());

    // Interface index remapping by stable name
    let mut old = HashMap::new();
    old.insert("ether1".to_string(), 1);
    let remapped = remap_interface_indices(&old, &[(7, "ether1".to_string(), "ether1".to_string())]);
    assert_eq!(remapped.get("ether1"), Some(&7));
}

#[test]
fn vendor_profile_longest_prefix_and_model_image_match() {
    let reg = VendorProfileRegistry::from_profiles(vec![
        VendorProfile {
            id: "generic".to_string(),
            name: "Generic".to_string(),
            vendor: "Generic".to_string(),
            sys_object_id_prefixes: vec!["".to_string()],
            default_vendor_image_key: "generic".to_string(),
            sensors: Vec::new(),
            model_rules: Vec::new(),
        },
        VendorProfile {
            id: "mikrotik".to_string(),
            name: "MikroTik".to_string(),
            vendor: "MikroTik".to_string(),
            sys_object_id_prefixes: vec!["1.3.6.1.4.1.14988".to_string()],
            default_vendor_image_key: "router".to_string(),
            sensors: Vec::new(),
            model_rules: vec![ModelImageRule {
                descr_or_model_pattern: "crs".to_string(),
                model_name: "CRS Series".to_string(),
                image_key: "crs".to_string(),
                device_type_name: "CRS".to_string(),
            }],
        },
    ]);

    let matched = reg.match_sys_object_id(".1.3.6.1.4.1.14988.1").unwrap();
    assert_eq!(matched.vendor, "MikroTik");
    let (model, img, _) = reg.match_model_and_image(matched, "RouterOS CRS326-24G-2S+");
    assert_eq!(model, "CRS Series");
    assert_eq!(img, Some("crs"));
}
