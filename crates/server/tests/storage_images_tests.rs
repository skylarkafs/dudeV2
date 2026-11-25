use nextdude_common::proto::v1;
use nextdude_server::crypto::SecretBox;
use nextdude_server::images::validate_and_process_image;
use nextdude_server::storage::telemetry::{
    downsample_min_avg_max, enforce_retention_and_vacuum, insert_raw_samples_batch, run_rollups,
    RawMetricSample, SOURCE_SERVICE_VALUE,
};
use nextdude_server::storage::StorageHandle;
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_SEQ: AtomicU64 = AtomicU64::new(1);

fn temp_db_path(name: &str) -> std::path::PathBuf {
    let n = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir()
        .join(format!("nextdude_test_{name}_{}_{n}", std::process::id()))
        .join("test.db")
}

#[tokio::test]
async fn sqlite_migrations_rollups_and_retention() {
    let db = temp_db_path("rollup");
    let storage = StorageHandle::open(&db, SecretBox::from_raw_key([0x42; 32])).unwrap();
    let now = chrono::Utc::now().timestamp();

    storage
        .write(move |c| {
            let samples = vec![
                RawMetricSample {
                    source_kind: SOURCE_SERVICE_VALUE,
                    source_id: 1,
                    ts_utc: now - 30,
                    val: 10.0,
                },
                RawMetricSample {
                    source_kind: SOURCE_SERVICE_VALUE,
                    source_id: 1,
                    ts_utc: now - 10,
                    val: 30.0,
                },
                RawMetricSample {
                    source_kind: SOURCE_SERVICE_VALUE,
                    source_id: 1,
                    ts_utc: now - 100 * 3600, // older than 48h raw retention
                    val: 99.0,
                },
            ];
            insert_raw_samples_batch(c, &samples)?;
            run_rollups(c, now)?;
            enforce_retention_and_vacuum(c, now, 48, 90, 180, 30, 365, false)?;

            let raw_cnt: i64 = c
                .query_row("SELECT COUNT(*) FROM ts_raw", [], |r| r.get(0))
                .unwrap();
            assert_eq!(raw_cnt, 2);

            let (avg_v, min_v, max_v): (f64, f64, f64) = c
                .query_row(
                    "SELECT avg_val, min_val, max_val FROM ts_1m WHERE source_id=1 LIMIT 1",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .unwrap();
            assert!((avg_v - 20.0).abs() < 0.01);
            assert!((min_v - 10.0).abs() < 0.01);
            assert!((max_v - 30.0).abs() < 0.01);
            Ok(())
        })
        .await
        .unwrap();

    let ds = downsample_min_avg_max(
        &(0..100)
            .map(|i| v1::ChartSample {
                timestamp_utc: i,
                avg: i as f64,
                min: i as f64,
                max: i as f64,
            })
            .collect::<Vec<_>>(),
        10,
    );
    assert_eq!(ds.len(), 10);
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

#[test]
fn image_upload_validation_rejects_oversized_spoofed_and_unsafe_svg() {
    // Oversized
    let big = vec![0u8; 512];
    assert!(validate_and_process_image(&big, 128, 256).is_err());

    // Spoofed PNG magic
    let fake = b"not really a png file";
    assert!(validate_and_process_image(fake, 4096, 256).is_err());

    // Malicious SVG with script & event handler
    let evil_svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><script>alert(1)</script><rect onload="x()" width="10" height="10"/></svg>"#;
    assert!(validate_and_process_image(evil_svg, 65536, 512).is_err());

    // External href SVG
    let ext_svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><image href="https://evil.invalid/a.png"/></svg>"#;
    assert!(validate_and_process_image(ext_svg, 65536, 512).is_err());

    // Valid safe SVG produces 4 PNG variants
    let good_svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><circle cx="32" cy="32" r="20" fill="green"/></svg>"#;
    let proc_img = validate_and_process_image(good_svg, 65536, 512).unwrap();
    assert_eq!(proc_img.variants_png.len(), 4);
}
