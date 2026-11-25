use nextdude_client_core::geometry::{
    compute_elastic_link_path, distance_to_polyline, node_boundary_point, parallel_link_offset,
    Vec2d,
};
use nextdude_client_core::i18n::{tr, Language, TextKey};
use nextdude_client_core::state::ClientStateCache;
use nextdude_client_core::tofu::TofuVerifier;
use nextdude_common::proto::v1;
use rustls::client::danger::ServerCertVerifier;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[test]
fn i18n_catalogs_cover_all_keys_with_non_empty_text() {
    for &key in TextKey::ALL {
        let id_str = tr(Language::Id, key);
        let en_str = tr(Language::En, key);
        assert!(!id_str.trim().is_empty(), "empty ID translation for {key:?}");
        assert!(!en_str.trim().is_empty(), "empty EN translation for {key:?}");
    }
}

#[test]
fn link_geometry_boundary_fanout_and_curve_hittest() {
    // Circle node boundary intersection
    let b = node_boundary_point(
        Vec2d::new(0.0, 0.0),
        20.0,
        20.0,
        v1::NodeShape::Circle,
        Vec2d::new(100.0, 0.0),
    );
    assert!((b.x - 20.0).abs() < 1e-6);
    assert!(b.y.abs() < 1e-6);

    // Symmetric parallel link fan-out
    let o0 = parallel_link_offset(0, 2, 26.0);
    let o1 = parallel_link_offset(1, 2, 26.0);
    assert!((o0 + o1).abs() < 1e-6);
    assert!(o0 < 0.0 && o1 > 0.0);

    // Curved link with bend handle and hit-testing
    let path = compute_elastic_link_path(
        Vec2d::new(0.0, 0.0),
        20.0,
        20.0,
        v1::NodeShape::RoundedRect,
        Vec2d::new(200.0, 0.0),
        20.0,
        20.0,
        v1::NodeShape::RoundedRect,
        v1::LinkStyle::Curved,
        &[Vec2d::new(100.0, 60.0)],
        0,
        1,
    );
    assert!(path.full_points.len() > 10);
    let dist_mid = distance_to_polyline(path.midpoint, &path.full_points);
    assert!(dist_mid < 1.0);
    let dist_far = distance_to_polyline(Vec2d::new(100.0, -200.0), &path.full_points);
    assert!(dist_far > 150.0);
}

#[test]
fn client_state_cache_detects_sequence_gap_for_resync() {
    let mut cache = ClientStateCache::default();
    cache.apply_stream_event(v1::StreamEvent {
        seq: 1,
        payload: Some(v1::stream_event::Payload::Snapshot(v1::SnapshotPayload::default())),
    });
    assert!(!cache.needs_resync);
    // Skipping seq 2 -> seq 3 triggers automatic resync flag
    cache.apply_stream_event(v1::StreamEvent {
        seq: 3,
        payload: Some(v1::stream_event::Payload::HeartbeatUtc(100)),
    });
    assert!(cache.needs_resync);
}

#[test]
fn tofu_verifier_enforces_first_use_and_detects_mismatch() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let store = Arc::new(RwLock::new(HashMap::new()));
    let verifier = TofuVerifier::new("10.0.0.1:9443".to_string(), Arc::clone(&store));
    let cert_a = CertificateDer::from(vec![0x30, 0x82, 0x01, 0x0a]);
    let cert_b = CertificateDer::from(vec![0x30, 0x82, 0x01, 0x0b]);
    let sname = ServerName::try_from("localhost").unwrap();

    // First use rejects handshake & emits FirstUse challenge
    assert!(verifier
        .verify_server_cert(&cert_a, &[], &sname, &[], UnixTime::now())
        .is_err());
    let dec = verifier.take_last_decision().unwrap();
    let fp = match dec {
        nextdude_client_core::tofu::TofuDecision::FirstUse { fingerprint, .. } => fingerprint,
        _ => panic!("expected FirstUse"),
    };

    // Trusting cert A allows connection
    store
        .write()
        .unwrap()
        .insert("10.0.0.1:9443".to_string(), fp);
    assert!(verifier
        .verify_server_cert(&cert_a, &[], &sname, &[], UnixTime::now())
        .is_ok());

    // Presenting cert B triggers Mismatch warning
    assert!(verifier
        .verify_server_cert(&cert_b, &[], &sname, &[], UnixTime::now())
        .is_err());
    assert!(matches!(
        verifier.take_last_decision().unwrap(),
        nextdude_client_core::tofu::TofuDecision::Mismatch { .. }
    ));
}
