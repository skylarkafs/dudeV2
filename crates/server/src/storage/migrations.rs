use crate::error::ServerError;
use nextdude_common::types::Permission;
use rusqlite::{params, Connection};

const CURRENT_SCHEMA_VERSION: i64 = 3;

pub fn run_migrations(conn: &mut Connection) -> Result<(), ServerError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_utc INTEGER NOT NULL
        );",
    )
    .map_err(|e| ServerError::Storage(format!("create schema_migrations failed: {e}")))?;

    let applied: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(|e| ServerError::Storage(format!("query schema version failed: {e}")))?;

    if applied < 1 {
        migrate_v1(conn)?;
    }
    if applied < 2 {
        migrate_v2(conn)?;
    }
    if applied < 3 {
        migrate_v3(conn)?;
    }
    debug_assert_eq!(CURRENT_SCHEMA_VERSION, 3);
    Ok(())
}

fn migrate_v1(conn: &mut Connection) -> Result<(), ServerError> {
    let tx = conn
        .transaction()
        .map_err(|e| ServerError::Storage(format!("tx v1 start: {e}")))?;

    tx.execute_batch(
        "
        CREATE TABLE admin_groups (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL DEFAULT 1,
            name TEXT NOT NULL UNIQUE,
            permissions_json TEXT NOT NULL
        );

        CREATE TABLE admins (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL DEFAULT 1,
            username TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            group_id INTEGER NOT NULL REFERENCES admin_groups(id),
            enabled INTEGER NOT NULL DEFAULT 1,
            failed_attempts INTEGER NOT NULL DEFAULT 0,
            locked_until_utc INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE user_preferences (
            admin_id INTEGER PRIMARY KEY REFERENCES admins(id) ON DELETE CASCADE,
            last_map_id INTEGER,
            panel_layout_json TEXT NOT NULL DEFAULT '{}',
            filters_json TEXT NOT NULL DEFAULT '{}'
        );

        CREATE TABLE images (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL DEFAULT 1,
            name TEXT NOT NULL,
            mime_type TEXT NOT NULL,
            vendor TEXT NOT NULL DEFAULT '',
            model TEXT NOT NULL DEFAULT '',
            tags_json TEXT NOT NULL DEFAULT '[]',
            width INTEGER NOT NULL,
            height INTEGER NOT NULL,
            byte_size INTEGER NOT NULL,
            builtin INTEGER NOT NULL DEFAULT 0,
            storage_key TEXT NOT NULL
        );

        CREATE TABLE maps (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL DEFAULT 1,
            name TEXT NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            poll_interval_sec_override INTEGER,
            label_refresh_sec INTEGER NOT NULL DEFAULT 5,
            palettes_json TEXT NOT NULL,
            default_node_shape INTEGER NOT NULL DEFAULT 1,
            default_link_thickness REAL NOT NULL DEFAULT 2.0,
            default_link_style INTEGER NOT NULL DEFAULT 2,
            background_image_id INTEGER REFERENCES images(id) ON DELETE SET NULL,
            background_scale REAL NOT NULL DEFAULT 1.0,
            background_tile INTEGER NOT NULL DEFAULT 0,
            locked INTEGER NOT NULL DEFAULT 0,
            default_layer INTEGER NOT NULL DEFAULT 1
        );

        CREATE TABLE device_types (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL DEFAULT 1,
            name TEXT NOT NULL UNIQUE,
            default_icon_key TEXT NOT NULL,
            default_image_id INTEGER REFERENCES images(id) ON DELETE SET NULL,
            default_probe_ids_json TEXT NOT NULL DEFAULT '[]',
            launcher_command TEXT NOT NULL DEFAULT ''
        );

        CREATE TABLE probes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL DEFAULT 1,
            name TEXT NOT NULL UNIQUE,
            kind INTEGER NOT NULL,
            port INTEGER NOT NULL DEFAULT 0,
            timeout_ms INTEGER NOT NULL DEFAULT 3000,
            down_threshold INTEGER NOT NULL DEFAULT 3,
            up_threshold INTEGER NOT NULL DEFAULT 1,
            target_path_or_query TEXT NOT NULL DEFAULT '',
            expected_pattern TEXT NOT NULL DEFAULT '',
            snmp_oid TEXT NOT NULL DEFAULT '',
            snmp_value_type INTEGER NOT NULL DEFAULT 1,
            snmp_scale REAL NOT NULL DEFAULT 1.0,
            unit TEXT NOT NULL DEFAULT '',
            warn_threshold REAL,
            crit_threshold REAL,
            builtin INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE devices (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL DEFAULT 1,
            map_id INTEGER NOT NULL REFERENCES maps(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            address TEXT NOT NULL,
            device_type_id INTEGER NOT NULL REFERENCES device_types(id),
            vendor TEXT NOT NULL DEFAULT '',
            model TEXT NOT NULL DEFAULT '',
            sys_object_id TEXT NOT NULL DEFAULT '',
            sys_descr TEXT NOT NULL DEFAULT '',
            sys_uptime_cs INTEGER NOT NULL DEFAULT 0,
            tags_json TEXT NOT NULL DEFAULT '[]',
            notes TEXT NOT NULL DEFAULT '',
            snmp_version INTEGER NOT NULL DEFAULT 2,
            snmp_port INTEGER NOT NULL DEFAULT 161,
            snmp_timeout_ms INTEGER NOT NULL DEFAULT 2500,
            snmp_retries INTEGER NOT NULL DEFAULT 1,
            snmp_community_enc TEXT NOT NULL DEFAULT '',
            snmp_username TEXT NOT NULL DEFAULT '',
            snmp_auth_proto INTEGER NOT NULL DEFAULT 1,
            snmp_auth_pass_enc TEXT NOT NULL DEFAULT '',
            snmp_priv_proto INTEGER NOT NULL DEFAULT 1,
            snmp_priv_pass_enc TEXT NOT NULL DEFAULT '',
            pos_x REAL NOT NULL DEFAULT 0.0,
            pos_y REAL NOT NULL DEFAULT 0.0,
            shape INTEGER NOT NULL DEFAULT 1,
            size INTEGER NOT NULL DEFAULT 2,
            custom_size_px REAL NOT NULL DEFAULT 48.0,
            custom_image_id INTEGER REFERENCES images(id) ON DELETE SET NULL,
            resolved_image_key TEXT,
            resolved_image_id INTEGER REFERENCES images(id) ON DELETE SET NULL,
            poll_interval_sec_override INTEGER,
            poll_priority_override INTEGER,
            label_template TEXT NOT NULL DEFAULT '{name}\n{ip}',
            aggregate_state INTEGER NOT NULL DEFAULT 1,
            last_rtt_ms REAL NOT NULL DEFAULT 0.0,
            last_seen_utc INTEGER NOT NULL DEFAULT 0,
            extra_ips_json TEXT NOT NULL DEFAULT '[]',
            mac_addresses_json TEXT NOT NULL DEFAULT '[]'
        );

        CREATE TABLE device_services (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL DEFAULT 1,
            device_id INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
            probe_id INTEGER NOT NULL REFERENCES probes(id),
            name TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            poll_interval_sec_override INTEGER,
            down_threshold_override INTEGER,
            up_threshold_override INTEGER,
            state INTEGER NOT NULL DEFAULT 1,
            consecutive_failures INTEGER NOT NULL DEFAULT 0,
            consecutive_successes INTEGER NOT NULL DEFAULT 0,
            last_value REAL NOT NULL DEFAULT 0.0,
            last_text TEXT NOT NULL DEFAULT '',
            last_rtt_ms REAL NOT NULL DEFAULT 0.0,
            last_check_utc INTEGER NOT NULL DEFAULT 0,
            interface_key TEXT NOT NULL DEFAULT ''
        );

        CREATE TABLE device_interfaces (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            device_id INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
            if_index INTEGER NOT NULL,
            stable_key TEXT NOT NULL,
            name TEXT NOT NULL,
            descr TEXT NOT NULL DEFAULT '',
            alias TEXT NOT NULL DEFAULT '',
            mac TEXT NOT NULL DEFAULT '',
            speed_bps INTEGER NOT NULL DEFAULT 0,
            admin_status INTEGER NOT NULL DEFAULT 1,
            oper_status INTEGER NOT NULL DEFAULT 1,
            in_bps REAL NOT NULL DEFAULT 0.0,
            out_bps REAL NOT NULL DEFAULT 0.0,
            updated_utc INTEGER NOT NULL DEFAULT 0,
            UNIQUE(device_id, stable_key)
        );

        CREATE TABLE link_types (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL DEFAULT 1,
            name TEXT NOT NULL UNIQUE,
            max_speed_bps INTEGER NOT NULL,
            thickness REAL NOT NULL,
            style INTEGER NOT NULL,
            color_hex TEXT NOT NULL,
            builtin INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE links (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL DEFAULT 1,
            map_id INTEGER NOT NULL REFERENCES maps(id) ON DELETE CASCADE,
            src_device_id INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
            dst_device_id INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
            master_device_id INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
            mastering_type INTEGER NOT NULL DEFAULT 1,
            interface_key TEXT NOT NULL DEFAULT '',
            link_type_id INTEGER NOT NULL REFERENCES link_types(id),
            max_speed_bps_override INTEGER,
            style_override INTEGER,
            history_enabled INTEGER NOT NULL DEFAULT 1,
            bend_points_json TEXT NOT NULL DEFAULT '[]',
            status INTEGER NOT NULL DEFAULT 1,
            in_bps REAL NOT NULL DEFAULT 0.0,
            out_bps REAL NOT NULL DEFAULT 0.0,
            in_utilization_pct REAL NOT NULL DEFAULT 0.0,
            out_utilization_pct REAL NOT NULL DEFAULT 0.0,
            label TEXT NOT NULL DEFAULT ''
        );

        CREATE TABLE map_networks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL DEFAULT 1,
            map_id INTEGER NOT NULL REFERENCES maps(id) ON DELETE CASCADE,
            name TEXT NOT NULL,
            cidr TEXT NOT NULL,
            pos_x REAL NOT NULL,
            pos_y REAL NOT NULL,
            width REAL NOT NULL DEFAULT 180.0,
            height REAL NOT NULL DEFAULT 90.0,
            image_id INTEGER REFERENCES images(id) ON DELETE SET NULL,
            state INTEGER NOT NULL DEFAULT 1
        );

        CREATE TABLE submaps (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL DEFAULT 1,
            map_id INTEGER NOT NULL REFERENCES maps(id) ON DELETE CASCADE,
            target_map_id INTEGER NOT NULL REFERENCES maps(id) ON DELETE CASCADE,
            label TEXT NOT NULL,
            pos_x REAL NOT NULL,
            pos_y REAL NOT NULL,
            image_id INTEGER REFERENCES images(id) ON DELETE SET NULL,
            aggregate_state INTEGER NOT NULL DEFAULT 1
        );

        CREATE TABLE static_items (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL DEFAULT 1,
            map_id INTEGER NOT NULL REFERENCES maps(id) ON DELETE CASCADE,
            label TEXT NOT NULL,
            kind TEXT NOT NULL DEFAULT 'box',
            pos_x REAL NOT NULL,
            pos_y REAL NOT NULL,
            width REAL NOT NULL DEFAULT 160.0,
            height REAL NOT NULL DEFAULT 80.0,
            color_hex TEXT NOT NULL DEFAULT '#334155',
            image_id INTEGER REFERENCES images(id) ON DELETE SET NULL,
            font_size INTEGER NOT NULL DEFAULT 14
        );

        CREATE TABLE dependencies (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL DEFAULT 1,
            map_id INTEGER NOT NULL REFERENCES maps(id) ON DELETE CASCADE,
            parent_device_id INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
            child_device_id INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
            UNIQUE(parent_device_id, child_device_id)
        );
        ",
    )
    .map_err(|e| ServerError::Storage(format!("migrate_v1 tables failed: {e}")))?;

    // Seed built-in admin groups (Viewer, Operator, Admin)
    let viewer_perms = serde_json::to_string(
        &Permission::viewer_defaults()
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>(),
    )
    .unwrap_or_default();
    let operator_perms = serde_json::to_string(
        &Permission::operator_defaults()
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>(),
    )
    .unwrap_or_default();
    let admin_perms = serde_json::to_string(
        &Permission::admin_defaults()
            .iter()
            .map(|p| p.as_str())
            .collect::<Vec<_>>(),
    )
    .unwrap_or_default();

    tx.execute(
        "INSERT INTO admin_groups (name, permissions_json) VALUES (?1, ?2)",
        params!["Viewer", viewer_perms],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    tx.execute(
        "INSERT INTO admin_groups (name, permissions_json) VALUES (?1, ?2)",
        params!["Operator", operator_perms],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    tx.execute(
        "INSERT INTO admin_groups (name, permissions_json) VALUES (?1, ?2)",
        params!["Admin", admin_perms],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;

    // Seed built-in probes matching The Dude baseline probes
    let probes_seed: &[(&str, i32, u32)] = &[
        ("ICMP Ping", 1, 0),
        ("HTTP", 3, 80),
        ("HTTPS", 4, 443),
        ("DNS", 5, 53),
        ("FTP", 8, 21),
        ("SMTP", 9, 25),
        ("IMAP", 10, 143),
        ("POP3", 11, 110),
        ("SSH", 12, 22),
        ("Telnet", 13, 23),
        ("NTP", 14, 123),
        ("SNMP SysUpTime", 6, 161),
    ];
    for (name, kind, port) in probes_seed {
        let oid = if *kind == 6 {
            "1.3.6.1.2.1.1.3.0"
        } else {
            ""
        };
        tx.execute(
            "INSERT INTO probes (name, kind, port, snmp_oid, builtin) VALUES (?1, ?2, ?3, ?4, 1)",
            params![name, kind, port, oid],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    }

    // Seed built-in device types referencing original generic SVG icon keys
    let dtypes_seed: &[(&str, &str, &str)] = &[
        ("Router", "router", "winbox {address}"),
        ("Switch", "switch", "ssh {address}"),
        ("CRS", "crs", "winbox {address}"),
        ("OLT", "olt", "ssh {address}"),
        ("ONU", "onu", "https://{address}"),
        ("Access Point", "ap", "https://{address}"),
        ("Radio", "radio", "https://{address}"),
        ("Server", "server", "ssh {address}"),
        ("Firewall", "firewall", "https://{address}"),
        ("Generic", "generic", ""),
    ];
    for (name, icon_key, launcher) in dtypes_seed {
        tx.execute(
            "INSERT INTO device_types (name, default_icon_key, default_probe_ids_json, launcher_command)
             VALUES (?1, ?2, '[1]', ?3)",
            params![name, icon_key, launcher],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    }

    // Seed built-in link types (FastEthernet, Gigabit, 10GbE, 40GbE, 100GbE, Wireless, Fiber)
    let ltypes_seed: &[(&str, i64, f64, i32, &str)] = &[
        ("FastEthernet", 100_000_000, 1.5, 2, "#38BDF8"),
        ("GigabitEthernet", 1_000_000_000, 2.5, 2, "#22C55E"),
        ("10GbE", 10_000_000_000, 3.5, 2, "#A855F7"),
        ("40GbE", 40_000_000_000, 4.5, 2, "#EC4899"),
        ("100GbE", 100_000_000_000, 5.5, 2, "#F97316"),
        ("Wireless", 300_000_000, 2.0, 2, "#06B6D4"),
        ("Fiber Backbone", 10_000_000_000, 3.0, 3, "#14B8A6"),
    ];
    for (name, speed, thick, style, color) in ltypes_seed {
        tx.execute(
            "INSERT INTO link_types (name, max_speed_bps, thickness, style, color_hex, builtin)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)",
            params![name, speed, thick, style, color],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    }

    let now = chrono::Utc::now().timestamp();
    tx.execute(
        "INSERT INTO schema_migrations (version, applied_utc) VALUES (1, ?1)",
        params![now],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    tx.commit()
        .map_err(|e| ServerError::Storage(format!("tx v1 commit: {e}")))?;
    Ok(())
}

fn migrate_v2(conn: &mut Connection) -> Result<(), ServerError> {
    let tx = conn
        .transaction()
        .map_err(|e| ServerError::Storage(format!("tx v2 start: {e}")))?;

    tx.execute_batch(
        "
        CREATE TABLE notification_channels (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL DEFAULT 1,
            name TEXT NOT NULL UNIQUE,
            kind TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            target TEXT NOT NULL DEFAULT '',
            secret_token_enc TEXT NOT NULL DEFAULT '',
            rate_limit_per_min INTEGER NOT NULL DEFAULT 30
        );

        CREATE TABLE alert_rules (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL DEFAULT 1,
            name TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            device_id INTEGER REFERENCES devices(id) ON DELETE CASCADE,
            probe_id INTEGER REFERENCES probes(id) ON DELETE CASCADE,
            trigger_on_state_change INTEGER NOT NULL DEFAULT 1,
            value_threshold REAL,
            threshold_greater_than INTEGER NOT NULL DEFAULT 1,
            min_duration_sec INTEGER NOT NULL DEFAULT 0,
            severity INTEGER NOT NULL DEFAULT 2,
            channel_ids_json TEXT NOT NULL DEFAULT '[]'
        );

        CREATE TABLE maintenance_windows (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL DEFAULT 1,
            name TEXT NOT NULL,
            device_id INTEGER REFERENCES devices(id) ON DELETE CASCADE,
            map_id INTEGER REFERENCES maps(id) ON DELETE CASCADE,
            start_utc INTEGER NOT NULL,
            end_utc INTEGER NOT NULL,
            recurring_weekly INTEGER NOT NULL DEFAULT 0,
            weekday_mask INTEGER NOT NULL DEFAULT 0,
            enabled INTEGER NOT NULL DEFAULT 1
        );

        CREATE TABLE outages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            device_id INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
            service_id INTEGER NOT NULL REFERENCES device_services(id) ON DELETE CASCADE,
            started_utc INTEGER NOT NULL,
            ended_utc INTEGER,
            root_cause_note TEXT NOT NULL DEFAULT '',
            suppressed_by_dependency INTEGER NOT NULL DEFAULT 0
        );
        CREATE INDEX idx_outages_started ON outages(started_utc DESC);

        CREATE TABLE event_logs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp_utc INTEGER NOT NULL,
            severity INTEGER NOT NULL,
            category TEXT NOT NULL,
            device_id INTEGER,
            device_name TEXT NOT NULL DEFAULT '',
            message TEXT NOT NULL,
            acknowledged INTEGER NOT NULL DEFAULT 0,
            ack_by TEXT,
            ack_utc INTEGER
        );
        CREATE INDEX idx_event_logs_ts ON event_logs(timestamp_utc DESC);

        CREATE TABLE syslog_messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp_utc INTEGER NOT NULL,
            source_ip TEXT NOT NULL,
            device_id INTEGER,
            device_name TEXT NOT NULL DEFAULT '',
            facility INTEGER NOT NULL,
            severity INTEGER NOT NULL,
            hostname TEXT NOT NULL DEFAULT '',
            app_name TEXT NOT NULL DEFAULT '',
            message TEXT NOT NULL,
            raw_format TEXT NOT NULL DEFAULT 'rfc3164'
        );
        CREATE INDEX idx_syslog_ts ON syslog_messages(timestamp_utc DESC);

        CREATE TABLE trap_messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp_utc INTEGER NOT NULL,
            source_ip TEXT NOT NULL,
            device_id INTEGER,
            device_name TEXT NOT NULL DEFAULT '',
            snmp_version INTEGER NOT NULL,
            trap_oid TEXT NOT NULL,
            trap_name TEXT NOT NULL,
            varbinds_json TEXT NOT NULL
        );
        CREATE INDEX idx_traps_ts ON trap_messages(timestamp_utc DESC);

        CREATE TABLE history_actions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp_utc INTEGER NOT NULL,
            admin_id INTEGER NOT NULL,
            username TEXT NOT NULL,
            action TEXT NOT NULL,
            target_type TEXT NOT NULL,
            target_id TEXT NOT NULL,
            details TEXT NOT NULL DEFAULT ''
        );
        CREATE INDEX idx_history_ts ON history_actions(timestamp_utc DESC);

        CREATE TABLE charts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            version INTEGER NOT NULL DEFAULT 1,
            name TEXT NOT NULL,
            sources_json TEXT NOT NULL DEFAULT '[]'
        );

        -- Tiered time-series storage: raw + 1m + 5m + 1h rollups
        -- source_kind: 1 = service value, 2 = service rtt, 3 = link in_bps, 4 = link out_bps, 5 = link util_pct
        CREATE TABLE ts_raw (
            source_kind INTEGER NOT NULL,
            source_id INTEGER NOT NULL,
            ts_utc INTEGER NOT NULL,
            val REAL NOT NULL,
            PRIMARY KEY (source_kind, source_id, ts_utc)
        );
        CREATE INDEX idx_ts_raw_ts ON ts_raw(ts_utc);

        CREATE TABLE ts_1m (
            source_kind INTEGER NOT NULL,
            source_id INTEGER NOT NULL,
            bucket_utc INTEGER NOT NULL,
            avg_val REAL NOT NULL,
            min_val REAL NOT NULL,
            max_val REAL NOT NULL,
            sample_count INTEGER NOT NULL,
            PRIMARY KEY (source_kind, source_id, bucket_utc)
        );

        CREATE TABLE ts_5m (
            source_kind INTEGER NOT NULL,
            source_id INTEGER NOT NULL,
            bucket_utc INTEGER NOT NULL,
            avg_val REAL NOT NULL,
            min_val REAL NOT NULL,
            max_val REAL NOT NULL,
            sample_count INTEGER NOT NULL,
            PRIMARY KEY (source_kind, source_id, bucket_utc)
        );

        CREATE TABLE ts_1h (
            source_kind INTEGER NOT NULL,
            source_id INTEGER NOT NULL,
            bucket_utc INTEGER NOT NULL,
            avg_val REAL NOT NULL,
            min_val REAL NOT NULL,
            max_val REAL NOT NULL,
            sample_count INTEGER NOT NULL,
            PRIMARY KEY (source_kind, source_id, bucket_utc)
        );
        ",
    )
    .map_err(|e| ServerError::Storage(format!("migrate_v2 failed: {e}")))?;

    // Seed built-in Event Log notification channel
    tx.execute(
        "INSERT INTO notification_channels (name, kind, enabled, target, rate_limit_per_min)
         VALUES ('Server Event Log', 'event_log', 1, 'internal', 120)",
        [],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;

    let now = chrono::Utc::now().timestamp();
    tx.execute(
        "INSERT INTO schema_migrations (version, applied_utc) VALUES (2, ?1)",
        params![now],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    tx.commit()
        .map_err(|e| ServerError::Storage(format!("tx v2 commit: {e}")))?;
    Ok(())
}

fn migrate_v3(conn: &mut Connection) -> Result<(), ServerError> {
    let tx = conn
        .transaction()
        .map_err(|e| ServerError::Storage(format!("tx v3 start: {e}")))?;

    tx.execute_batch(
        "
        CREATE TABLE server_runtime_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            version INTEGER NOT NULL DEFAULT 1,
            global_poll_interval_sec INTEGER NOT NULL DEFAULT 30,
            max_concurrent_probes INTEGER NOT NULL DEFAULT 256,
            max_concurrent_per_host INTEGER NOT NULL DEFAULT 6,
            flapping_changes INTEGER NOT NULL DEFAULT 5,
            flapping_window_min INTEGER NOT NULL DEFAULT 10,
            syslog_enabled INTEGER NOT NULL DEFAULT 1,
            syslog_port INTEGER NOT NULL DEFAULT 514,
            syslog_forward_addr TEXT NOT NULL DEFAULT '',
            trap_enabled INTEGER NOT NULL DEFAULT 1,
            trap_port INTEGER NOT NULL DEFAULT 162,
            mndp_enabled INTEGER NOT NULL DEFAULT 1,
            allow_image_url_import INTEGER NOT NULL DEFAULT 0,
            retention_raw_hours INTEGER NOT NULL DEFAULT 48,
            retention_rollup_days INTEGER NOT NULL DEFAULT 90,
            retention_events_days INTEGER NOT NULL DEFAULT 180,
            retention_syslog_days INTEGER NOT NULL DEFAULT 30,
            retention_audit_days INTEGER NOT NULL DEFAULT 365
        );
        INSERT OR IGNORE INTO server_runtime_settings (id) VALUES (1);
        ",
    )
    .map_err(|e| ServerError::Storage(format!("migrate_v3 failed: {e}")))?;

    let now = chrono::Utc::now().timestamp();
    tx.execute(
        "INSERT INTO schema_migrations (version, applied_utc) VALUES (3, ?1)",
        params![now],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    tx.commit()
        .map_err(|e| ServerError::Storage(format!("tx v3 commit: {e}")))?;
    Ok(())
}
