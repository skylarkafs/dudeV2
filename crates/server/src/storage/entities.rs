use crate::crypto::SecretBox;
use crate::error::ServerError;
use nextdude_common::proto::{default_map_palettes, v1};
use nextdude_common::validation::{validate_cidr, validate_entity_name, validate_host_or_ip};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredPalettes {
    device: StoredPalette,
    network: StoredPalette,
    submap: StoredPalette,
    static_item: StoredPalette,
    link: StoredPalette,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredPalette {
    up: String,
    down: String,
    unreachable: String,
    partial: String,
    unknown: String,
    flapping: String,
    no_reply: String,
}

impl From<&v1::StateColorPalette> for StoredPalette {
    fn from(p: &v1::StateColorPalette) -> Self {
        Self {
            up: p.up_hex.clone(),
            down: p.down_hex.clone(),
            unreachable: p.unreachable_hex.clone(),
            partial: p.partial_hex.clone(),
            unknown: p.unknown_hex.clone(),
            flapping: p.flapping_hex.clone(),
            no_reply: p.no_reply_hex.clone(),
        }
    }
}

impl From<StoredPalette> for v1::StateColorPalette {
    fn from(p: StoredPalette) -> Self {
        Self {
            up_hex: p.up,
            down_hex: p.down,
            unreachable_hex: p.unreachable,
            partial_hex: p.partial,
            unknown_hex: p.unknown,
            flapping_hex: p.flapping,
            no_reply_hex: p.no_reply,
        }
    }
}

pub fn check_version(
    conn: &Connection,
    table: &str,
    id: i64,
    expected_version: u64,
) -> Result<(), ServerError> {
    let sql = format!("SELECT version FROM {table} WHERE id = ?1");
    let current: Option<i64> = conn
        .query_row(&sql, params![id], |r| r.get(0))
        .optional()
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let Some(curr) = current else {
        return Err(ServerError::NotFound(format!("{table} id={id}")));
    };
    if curr as u64 != expected_version {
        return Err(ServerError::VersionConflict {
            expected: expected_version,
            current: curr as u64,
        });
    }
    Ok(())
}

pub fn delete_entity_versioned(
    conn: &mut Connection,
    table: &str,
    id: i64,
    expected_version: u64,
) -> Result<(), ServerError> {
    if expected_version > 0 {
        check_version(conn, table, id, expected_version)?;
    }
    let sql = format!("DELETE FROM {table} WHERE id = ?1");
    let affected = conn
        .execute(&sql, params![id])
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    if affected == 0 {
        return Err(ServerError::NotFound(format!("{table} id={id}")));
    }
    Ok(())
}

pub fn load_maps(conn: &Connection) -> Result<Vec<v1::NetworkMap>, ServerError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, version, name, description, poll_interval_sec_override, label_refresh_sec,
                    palettes_json, default_node_shape, default_link_thickness, default_link_style,
                    background_image_id, background_scale, background_tile, locked, default_layer
             FROM maps ORDER BY id",
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            let pal_json: String = r.get(6)?;
            let (d, n, s, st, l) = match serde_json::from_str::<StoredPalettes>(&pal_json) {
                Ok(p) => (
                    p.device.into(),
                    p.network.into(),
                    p.submap.into(),
                    p.static_item.into(),
                    p.link.into(),
                ),
                Err(_) => default_map_palettes(),
            };
            Ok(v1::NetworkMap {
                id: r.get(0)?,
                version: r.get::<_, i64>(1)? as u64,
                name: r.get(2)?,
                description: r.get(3)?,
                poll_interval_sec_override: r.get::<_, Option<i64>>(4)?.map(|v| v as u32),
                label_refresh_sec: r.get::<_, i64>(5)? as u32,
                device_colors: Some(d),
                network_colors: Some(n),
                submap_colors: Some(s),
                static_colors: Some(st),
                link_colors: Some(l),
                default_node_shape: r.get(7)?,
                default_link_thickness: r.get(8)?,
                default_link_style: r.get(9)?,
                background_image_id: r.get(10)?,
                background_scale: r.get(11)?,
                background_tile: r.get::<_, i64>(12)? != 0,
                locked: r.get::<_, i64>(13)? != 0,
                default_layer: r.get(14)?,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))
}

pub fn save_map(conn: &mut Connection, map: &v1::NetworkMap) -> Result<v1::NetworkMap, ServerError> {
    let name = validate_entity_name(&map.name)?;
    let (def_d, def_n, def_s, def_st, def_l) = default_map_palettes();
    let stored_pal = StoredPalettes {
        device: StoredPalette::from(map.device_colors.as_ref().unwrap_or(&def_d)),
        network: StoredPalette::from(map.network_colors.as_ref().unwrap_or(&def_n)),
        submap: StoredPalette::from(map.submap_colors.as_ref().unwrap_or(&def_s)),
        static_item: StoredPalette::from(map.static_colors.as_ref().unwrap_or(&def_st)),
        link: StoredPalette::from(map.link_colors.as_ref().unwrap_or(&def_l)),
    };
    let pal_json = serde_json::to_string(&stored_pal).unwrap_or_default();

    if map.id <= 0 {
        conn.execute(
            "INSERT INTO maps (name, description, poll_interval_sec_override, label_refresh_sec,
              palettes_json, default_node_shape, default_link_thickness, default_link_style,
              background_image_id, background_scale, background_tile, locked, default_layer)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                name,
                map.description,
                map.poll_interval_sec_override.map(i64::from),
                i64::from(map.label_refresh_sec.max(1)),
                pal_json,
                map.default_node_shape.max(1),
                map.default_link_thickness.max(0.5),
                map.default_link_style.max(1),
                map.background_image_id,
                map.background_scale.max(0.1),
                i64::from(map.background_tile),
                i64::from(map.locked),
                map.default_layer.max(1)
            ],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
        let mut out = map.clone();
        out.id = conn.last_insert_rowid();
        out.version = 1;
        out.name = name;
        return Ok(out);
    }

    check_version(conn, "maps", map.id, map.version)?;
    conn.execute(
        "UPDATE maps SET name=?1, description=?2, poll_interval_sec_override=?3, label_refresh_sec=?4,
          palettes_json=?5, default_node_shape=?6, default_link_thickness=?7, default_link_style=?8,
          background_image_id=?9, background_scale=?10, background_tile=?11, locked=?12,
          default_layer=?13, version=version+1
         WHERE id=?14",
        params![
            name,
            map.description,
            map.poll_interval_sec_override.map(i64::from),
            i64::from(map.label_refresh_sec.max(1)),
            pal_json,
            map.default_node_shape.max(1),
            map.default_link_thickness.max(0.5),
            map.default_link_style.max(1),
            map.background_image_id,
            map.background_scale.max(0.1),
            i64::from(map.background_tile),
            i64::from(map.locked),
            map.default_layer.max(1),
            map.id
        ],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut out = map.clone();
    out.version += 1;
    out.name = name;
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct DeviceSecrets {
    pub community: String,
    pub auth_password: String,
    pub priv_password: String,
}

pub fn load_devices(conn: &Connection) -> Result<Vec<v1::Device>, ServerError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, version, map_id, name, address, device_type_id, vendor, model,
                    sys_object_id, sys_descr, sys_uptime_cs, tags_json, notes,
                    snmp_version, snmp_port, snmp_timeout_ms, snmp_retries,
                    snmp_community_enc, snmp_username, snmp_auth_proto, snmp_auth_pass_enc,
                    snmp_priv_proto, snmp_priv_pass_enc, pos_x, pos_y, shape, size,
                    custom_size_px, custom_image_id, resolved_image_key, resolved_image_id,
                    poll_interval_sec_override, poll_priority_override, label_template,
                    aggregate_state, last_rtt_ms, last_seen_utc, extra_ips_json, mac_addresses_json
             FROM devices ORDER BY id",
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;

    let rows = stmt
        .query_map([], |r| {
            let tags_json: String = r.get(11)?;
            let comm_enc: String = r.get(17)?;
            let auth_enc: String = r.get(20)?;
            let priv_enc: String = r.get(22)?;
            let extra_ips_json: String = r.get(37)?;
            let macs_json: String = r.get(38)?;
            Ok(v1::Device {
                id: r.get(0)?,
                version: r.get::<_, i64>(1)? as u64,
                map_id: r.get(2)?,
                name: r.get(3)?,
                address: r.get(4)?,
                device_type_id: r.get(5)?,
                vendor: r.get(6)?,
                model: r.get(7)?,
                sys_object_id: r.get(8)?,
                sys_descr: r.get(9)?,
                sys_uptime_cs: r.get::<_, i64>(10)? as u64,
                tags: serde_json::from_str(&tags_json).unwrap_or_default(),
                notes: r.get(12)?,
                snmp: Some(v1::SnmpCredential {
                    version: r.get(13)?,
                    port: r.get::<_, i64>(14)? as u32,
                    timeout_ms: r.get::<_, i64>(15)? as u32,
                    retries: r.get::<_, i64>(16)? as u32,
                    community: None,
                    has_community: !comm_enc.is_empty(),
                    username: r.get(18)?,
                    auth_protocol: r.get(19)?,
                    auth_password: None,
                    has_auth_password: !auth_enc.is_empty(),
                    priv_protocol: r.get(21)?,
                    priv_password: None,
                    has_priv_password: !priv_enc.is_empty(),
                }),
                pos_x: r.get(23)?,
                pos_y: r.get(24)?,
                shape: r.get(25)?,
                size: r.get(26)?,
                custom_size_px: r.get(27)?,
                custom_image_id: r.get(28)?,
                resolved_image_key: r.get(29)?,
                resolved_image_id: r.get(30)?,
                poll_interval_sec_override: r.get::<_, Option<i64>>(31)?.map(|v| v as u32),
                poll_priority_override: r.get::<_, Option<i64>>(32)?.map(|v| v as u32),
                label_template: r.get(33)?,
                aggregate_state: r.get(34)?,
                last_rtt_ms: r.get(35)?,
                last_seen_utc: r.get(36)?,
                extra_ips: serde_json::from_str(&extra_ips_json).unwrap_or_default(),
                mac_addresses: serde_json::from_str(&macs_json).unwrap_or_default(),
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))
}

pub fn load_device_secrets(
    conn: &Connection,
    secrets: &SecretBox,
    device_id: i64,
) -> Result<DeviceSecrets, ServerError> {
    let (c_enc, a_enc, p_enc): (String, String, String) = conn
        .query_row(
            "SELECT snmp_community_enc, snmp_auth_pass_enc, snmp_priv_pass_enc FROM devices WHERE id = ?1",
            params![device_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    Ok(DeviceSecrets {
        community: secrets.decrypt(&c_enc)?,
        auth_password: secrets.decrypt(&a_enc)?,
        priv_password: secrets.decrypt(&p_enc)?,
    })
}

pub fn save_device(
    conn: &mut Connection,
    secrets: &SecretBox,
    dev: &v1::Device,
) -> Result<v1::Device, ServerError> {
    let name = validate_entity_name(&dev.name)?;
    let address = validate_host_or_ip(&dev.address)?;
    let snmp = dev.snmp.clone().unwrap_or(v1::SnmpCredential {
        version: v1::SnmpVersion::V2c as i32,
        port: 161,
        timeout_ms: 2500,
        retries: 1,
        community: None,
        has_community: false,
        username: String::new(),
        auth_protocol: v1::SnmpAuthProtocol::SnmpAuthNone as i32,
        auth_password: None,
        has_auth_password: false,
        priv_protocol: v1::SnmpPrivProtocol::SnmpPrivNone as i32,
        priv_password: None,
        has_priv_password: false,
    });
    let tags_json = serde_json::to_string(&dev.tags).unwrap_or_else(|_| "[]".to_string());
    let extra_ips_json = serde_json::to_string(&dev.extra_ips).unwrap_or_else(|_| "[]".to_string());
    let macs_json = serde_json::to_string(&dev.mac_addresses).unwrap_or_else(|_| "[]".to_string());

    if dev.id <= 0 {
        let comm_enc = match snmp.community.as_deref() {
            Some(c) if !c.is_empty() => secrets.encrypt(c)?,
            _ => String::new(),
        };
        let auth_enc = match snmp.auth_password.as_deref() {
            Some(p) if !p.is_empty() => secrets.encrypt(p)?,
            _ => String::new(),
        };
        let priv_enc = match snmp.priv_password.as_deref() {
            Some(p) if !p.is_empty() => secrets.encrypt(p)?,
            _ => String::new(),
        };

        conn.execute(
            "INSERT INTO devices (
                map_id, name, address, device_type_id, vendor, model, sys_object_id, sys_descr,
                sys_uptime_cs, tags_json, notes, snmp_version, snmp_port, snmp_timeout_ms,
                snmp_retries, snmp_community_enc, snmp_username, snmp_auth_proto,
                snmp_auth_pass_enc, snmp_priv_proto, snmp_priv_pass_enc,
                pos_x, pos_y, shape, size, custom_size_px, custom_image_id,
                resolved_image_key, resolved_image_id, poll_interval_sec_override,
                poll_priority_override, label_template, extra_ips_json, mac_addresses_json
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33,?34)",
            params![
                dev.map_id,
                name,
                address,
                dev.device_type_id.max(1),
                dev.vendor,
                dev.model,
                dev.sys_object_id,
                dev.sys_descr,
                dev.sys_uptime_cs as i64,
                tags_json,
                dev.notes,
                snmp.version.max(1),
                i64::from(snmp.port.max(1)),
                i64::from(snmp.timeout_ms.max(200)),
                i64::from(snmp.retries),
                comm_enc,
                snmp.username,
                snmp.auth_protocol.max(1),
                auth_enc,
                snmp.priv_protocol.max(1),
                priv_enc,
                dev.pos_x,
                dev.pos_y,
                dev.shape.max(1),
                dev.size.max(1),
                dev.custom_size_px.max(16.0),
                dev.custom_image_id,
                dev.resolved_image_key,
                dev.resolved_image_id,
                dev.poll_interval_sec_override.map(i64::from),
                dev.poll_priority_override.map(i64::from),
                if dev.label_template.is_empty() { "{name}\n{ip}" } else { &dev.label_template },
                extra_ips_json,
                macs_json
            ],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
        let id = conn.last_insert_rowid();

        // Attach default probes from device_type automatically on creation.
        let default_probes_json: Option<String> = conn
            .query_row(
                "SELECT default_probe_ids_json FROM device_types WHERE id = ?1",
                params![dev.device_type_id.max(1)],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| ServerError::Storage(e.to_string()))?;
        if let Some(json) = default_probes_json {
            let probe_ids: Vec<i64> = serde_json::from_str(&json).unwrap_or_default();
            for pid in probe_ids {
                if let Ok(pname) = conn.query_row(
                    "SELECT name FROM probes WHERE id = ?1",
                    params![pid],
                    |r| r.get::<_, String>(0),
                ) {
                    let _ = conn.execute(
                        "INSERT INTO device_services (device_id, probe_id, name, enabled) VALUES (?1, ?2, ?3, 1)",
                        params![id, pid, pname],
                    );
                }
            }
        }

        let mut out = dev.clone();
        out.id = id;
        out.version = 1;
        out.name = name;
        out.address = address;
        if let Some(ref mut s) = out.snmp {
            s.has_community = !comm_enc.is_empty();
            s.has_auth_password = !auth_enc.is_empty();
            s.has_priv_password = !priv_enc.is_empty();
            s.community = None;
            s.auth_password = None;
            s.priv_password = None;
        }
        return Ok(out);
    }

    check_version(conn, "devices", dev.id, dev.version)?;
    let (old_c, old_a, old_p): (String, String, String) = conn
        .query_row(
            "SELECT snmp_community_enc, snmp_auth_pass_enc, snmp_priv_pass_enc FROM devices WHERE id = ?1",
            params![dev.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;

    let comm_enc = match snmp.community.as_deref() {
        Some(c) if !c.is_empty() => secrets.encrypt(c)?,
        _ => old_c,
    };
    let auth_enc = match snmp.auth_password.as_deref() {
        Some(p) if !p.is_empty() => secrets.encrypt(p)?,
        _ => old_a,
    };
    let priv_enc = match snmp.priv_password.as_deref() {
        Some(p) if !p.is_empty() => secrets.encrypt(p)?,
        _ => old_p,
    };

    conn.execute(
        "UPDATE devices SET
            map_id=?1, name=?2, address=?3, device_type_id=?4, vendor=?5, model=?6,
            sys_object_id=?7, sys_descr=?8, tags_json=?9, notes=?10,
            snmp_version=?11, snmp_port=?12, snmp_timeout_ms=?13, snmp_retries=?14,
            snmp_community_enc=?15, snmp_username=?16, snmp_auth_proto=?17,
            snmp_auth_pass_enc=?18, snmp_priv_proto=?19, snmp_priv_pass_enc=?20,
            pos_x=?21, pos_y=?22, shape=?23, size=?24, custom_size_px=?25,
            custom_image_id=?26, resolved_image_key=?27, resolved_image_id=?28,
            poll_interval_sec_override=?29, poll_priority_override=?30, label_template=?31,
            extra_ips_json=?32, mac_addresses_json=?33, version=version+1
         WHERE id=?34",
        params![
            dev.map_id,
            name,
            address,
            dev.device_type_id.max(1),
            dev.vendor,
            dev.model,
            dev.sys_object_id,
            dev.sys_descr,
            tags_json,
            dev.notes,
            snmp.version.max(1),
            i64::from(snmp.port.max(1)),
            i64::from(snmp.timeout_ms.max(200)),
            i64::from(snmp.retries),
            comm_enc,
            snmp.username,
            snmp.auth_protocol.max(1),
            auth_enc,
            snmp.priv_protocol.max(1),
            priv_enc,
            dev.pos_x,
            dev.pos_y,
            dev.shape.max(1),
            dev.size.max(1),
            dev.custom_size_px.max(16.0),
            dev.custom_image_id,
            dev.resolved_image_key,
            dev.resolved_image_id,
            dev.poll_interval_sec_override.map(i64::from),
            dev.poll_priority_override.map(i64::from),
            dev.label_template,
            extra_ips_json,
            macs_json,
            dev.id
        ],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;

    let mut out = dev.clone();
    out.version += 1;
    out.name = name;
    out.address = address;
    if let Some(ref mut s) = out.snmp {
        s.has_community = !comm_enc.is_empty();
        s.has_auth_password = !auth_enc.is_empty();
        s.has_priv_password = !priv_enc.is_empty();
        s.community = None;
        s.auth_password = None;
        s.priv_password = None;
    }
    Ok(out)
}

pub fn load_probes(conn: &Connection) -> Result<Vec<v1::ProbeDefinition>, ServerError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, version, name, kind, port, timeout_ms, down_threshold, up_threshold,
                    target_path_or_query, expected_pattern, snmp_oid, snmp_value_type, snmp_scale,
                    unit, warn_threshold, crit_threshold, builtin
             FROM probes ORDER BY id",
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(v1::ProbeDefinition {
                id: r.get(0)?,
                version: r.get::<_, i64>(1)? as u64,
                name: r.get(2)?,
                kind: r.get(3)?,
                port: r.get::<_, i64>(4)? as u32,
                timeout_ms: r.get::<_, i64>(5)? as u32,
                down_threshold: r.get::<_, i64>(6)? as u32,
                up_threshold: r.get::<_, i64>(7)? as u32,
                target_path_or_query: r.get(8)?,
                expected_pattern: r.get(9)?,
                snmp_oid: r.get(10)?,
                snmp_value_type: r.get(11)?,
                snmp_scale: r.get(12)?,
                unit: r.get(13)?,
                warn_threshold: r.get(14)?,
                crit_threshold: r.get(15)?,
                builtin: r.get::<_, i64>(16)? != 0,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))
}

pub fn save_probe(
    conn: &mut Connection,
    probe: &v1::ProbeDefinition,
) -> Result<v1::ProbeDefinition, ServerError> {
    let name = validate_entity_name(&probe.name)?;
    if !probe.snmp_oid.is_empty() {
        nextdude_common::validation::validate_oid(&probe.snmp_oid)?;
    }
    if probe.id <= 0 {
        conn.execute(
            "INSERT INTO probes (name, kind, port, timeout_ms, down_threshold, up_threshold,
              target_path_or_query, expected_pattern, snmp_oid, snmp_value_type, snmp_scale,
              unit, warn_threshold, crit_threshold, builtin)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,0)",
            params![
                name,
                probe.kind.max(1),
                i64::from(probe.port),
                i64::from(probe.timeout_ms.max(200)),
                i64::from(probe.down_threshold.max(1)),
                i64::from(probe.up_threshold.max(1)),
                probe.target_path_or_query,
                probe.expected_pattern,
                probe.snmp_oid,
                probe.snmp_value_type.max(1),
                if probe.snmp_scale == 0.0 { 1.0 } else { probe.snmp_scale },
                probe.unit,
                probe.warn_threshold,
                probe.crit_threshold
            ],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
        let mut out = probe.clone();
        out.id = conn.last_insert_rowid();
        out.version = 1;
        out.name = name;
        out.builtin = false;
        return Ok(out);
    }

    check_version(conn, "probes", probe.id, probe.version)?;
    conn.execute(
        "UPDATE probes SET name=?1, kind=?2, port=?3, timeout_ms=?4, down_threshold=?5, up_threshold=?6,
          target_path_or_query=?7, expected_pattern=?8, snmp_oid=?9, snmp_value_type=?10,
          snmp_scale=?11, unit=?12, warn_threshold=?13, crit_threshold=?14, version=version+1
         WHERE id=?15",
        params![
            name,
            probe.kind.max(1),
            i64::from(probe.port),
            i64::from(probe.timeout_ms.max(200)),
            i64::from(probe.down_threshold.max(1)),
            i64::from(probe.up_threshold.max(1)),
            probe.target_path_or_query,
            probe.expected_pattern,
            probe.snmp_oid,
            probe.snmp_value_type.max(1),
            if probe.snmp_scale == 0.0 { 1.0 } else { probe.snmp_scale },
            probe.unit,
            probe.warn_threshold,
            probe.crit_threshold,
            probe.id
        ],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut out = probe.clone();
    out.version += 1;
    out.name = name;
    Ok(out)
}

pub fn load_services(conn: &Connection) -> Result<Vec<v1::DeviceService>, ServerError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, version, device_id, probe_id, name, enabled,
                    poll_interval_sec_override, down_threshold_override, up_threshold_override,
                    state, consecutive_failures, consecutive_successes,
                    last_value, last_text, last_rtt_ms, last_check_utc, interface_key
             FROM device_services ORDER BY id",
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(v1::DeviceService {
                id: r.get(0)?,
                version: r.get::<_, i64>(1)? as u64,
                device_id: r.get(2)?,
                probe_id: r.get(3)?,
                name: r.get(4)?,
                enabled: r.get::<_, i64>(5)? != 0,
                poll_interval_sec_override: r.get::<_, Option<i64>>(6)?.map(|v| v as u32),
                down_threshold_override: r.get::<_, Option<i64>>(7)?.map(|v| v as u32),
                up_threshold_override: r.get::<_, Option<i64>>(8)?.map(|v| v as u32),
                state: r.get(9)?,
                consecutive_failures: r.get::<_, i64>(10)? as u32,
                consecutive_successes: r.get::<_, i64>(11)? as u32,
                last_value: r.get(12)?,
                last_text: r.get(13)?,
                last_rtt_ms: r.get(14)?,
                last_check_utc: r.get(15)?,
                interface_key: r.get(16)?,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))
}

pub fn save_service(
    conn: &mut Connection,
    svc: &v1::DeviceService,
) -> Result<v1::DeviceService, ServerError> {
    let name = validate_entity_name(&svc.name)?;
    if svc.id <= 0 {
        conn.execute(
            "INSERT INTO device_services (device_id, probe_id, name, enabled,
              poll_interval_sec_override, down_threshold_override, up_threshold_override, interface_key)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                svc.device_id,
                svc.probe_id,
                name,
                i64::from(svc.enabled),
                svc.poll_interval_sec_override.map(i64::from),
                svc.down_threshold_override.map(i64::from),
                svc.up_threshold_override.map(i64::from),
                svc.interface_key
            ],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
        let mut out = svc.clone();
        out.id = conn.last_insert_rowid();
        out.version = 1;
        out.name = name;
        return Ok(out);
    }

    check_version(conn, "device_services", svc.id, svc.version)?;
    conn.execute(
        "UPDATE device_services SET probe_id=?1, name=?2, enabled=?3,
          poll_interval_sec_override=?4, down_threshold_override=?5, up_threshold_override=?6,
          interface_key=?7, version=version+1
         WHERE id=?8",
        params![
            svc.probe_id,
            name,
            i64::from(svc.enabled),
            svc.poll_interval_sec_override.map(i64::from),
            svc.down_threshold_override.map(i64::from),
            svc.up_threshold_override.map(i64::from),
            svc.interface_key,
            svc.id
        ],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut out = svc.clone();
    out.version += 1;
    out.name = name;
    Ok(out)
}

pub fn load_device_types(conn: &Connection) -> Result<Vec<v1::DeviceType>, ServerError> {
    let mut stmt = conn
        .prepare("SELECT id, version, name, default_icon_key, default_image_id, default_probe_ids_json, launcher_command FROM device_types ORDER BY id")
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            let p_json: String = r.get(5)?;
            Ok(v1::DeviceType {
                id: r.get(0)?,
                version: r.get::<_, i64>(1)? as u64,
                name: r.get(2)?,
                default_icon_key: r.get(3)?,
                default_image_id: r.get(4)?,
                default_probe_ids: serde_json::from_str(&p_json).unwrap_or_default(),
                launcher_command: r.get(6)?,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))
}

pub fn save_device_type(
    conn: &mut Connection,
    dt: &v1::DeviceType,
) -> Result<v1::DeviceType, ServerError> {
    let name = validate_entity_name(&dt.name)?;
    let probes_json = serde_json::to_string(&dt.default_probe_ids).unwrap_or_else(|_| "[]".to_string());
    if dt.id <= 0 {
        conn.execute(
            "INSERT INTO device_types (name, default_icon_key, default_image_id, default_probe_ids_json, launcher_command)
             VALUES (?1,?2,?3,?4,?5)",
            params![name, dt.default_icon_key, dt.default_image_id, probes_json, dt.launcher_command],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
        let mut out = dt.clone();
        out.id = conn.last_insert_rowid();
        out.version = 1;
        out.name = name;
        return Ok(out);
    }
    check_version(conn, "device_types", dt.id, dt.version)?;
    conn.execute(
        "UPDATE device_types SET name=?1, default_icon_key=?2, default_image_id=?3, default_probe_ids_json=?4, launcher_command=?5, version=version+1 WHERE id=?6",
        params![name, dt.default_icon_key, dt.default_image_id, probes_json, dt.launcher_command, dt.id],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut out = dt.clone();
    out.version += 1;
    out.name = name;
    Ok(out)
}

pub fn load_link_types(conn: &Connection) -> Result<Vec<v1::LinkType>, ServerError> {
    let mut stmt = conn
        .prepare("SELECT id, version, name, max_speed_bps, thickness, style, color_hex, builtin FROM link_types ORDER BY id")
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(v1::LinkType {
                id: r.get(0)?,
                version: r.get::<_, i64>(1)? as u64,
                name: r.get(2)?,
                max_speed_bps: r.get::<_, i64>(3)? as u64,
                thickness: r.get(4)?,
                style: r.get(5)?,
                color_hex: r.get(6)?,
                builtin: r.get::<_, i64>(7)? != 0,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))
}

pub fn save_link_type(conn: &mut Connection, lt: &v1::LinkType) -> Result<v1::LinkType, ServerError> {
    let name = validate_entity_name(&lt.name)?;
    let color = nextdude_common::validation::validate_hex_color(&lt.color_hex)?;
    if lt.id <= 0 {
        conn.execute(
            "INSERT INTO link_types (name, max_speed_bps, thickness, style, color_hex, builtin)
             VALUES (?1,?2,?3,?4,?5,0)",
            params![name, lt.max_speed_bps as i64, lt.thickness.max(0.5), lt.style.max(1), color],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
        let mut out = lt.clone();
        out.id = conn.last_insert_rowid();
        out.version = 1;
        out.name = name;
        out.color_hex = color;
        out.builtin = false;
        return Ok(out);
    }
    check_version(conn, "link_types", lt.id, lt.version)?;
    conn.execute(
        "UPDATE link_types SET name=?1, max_speed_bps=?2, thickness=?3, style=?4, color_hex=?5, version=version+1 WHERE id=?6",
        params![name, lt.max_speed_bps as i64, lt.thickness.max(0.5), lt.style.max(1), color, lt.id],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut out = lt.clone();
    out.version += 1;
    out.name = name;
    out.color_hex = color;
    Ok(out)
}

#[derive(Serialize, Deserialize)]
struct StoredPt {
    x: f64,
    y: f64,
}

pub fn load_links(conn: &Connection) -> Result<Vec<v1::NetworkLink>, ServerError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, version, map_id, src_device_id, dst_device_id, master_device_id,
                    mastering_type, interface_key, link_type_id, max_speed_bps_override,
                    style_override, history_enabled, bend_points_json, status,
                    in_bps, out_bps, in_utilization_pct, out_utilization_pct, label
             FROM links ORDER BY id",
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            let bends_json: String = r.get(12)?;
            let bends = serde_json::from_str::<Vec<StoredPt>>(&bends_json)
                .unwrap_or_default()
                .into_iter()
                .map(|p| v1::Point2d { x: p.x, y: p.y })
                .collect();
            Ok(v1::NetworkLink {
                id: r.get(0)?,
                version: r.get::<_, i64>(1)? as u64,
                map_id: r.get(2)?,
                src_device_id: r.get(3)?,
                dst_device_id: r.get(4)?,
                master_device_id: r.get(5)?,
                mastering_type: r.get(6)?,
                interface_key: r.get(7)?,
                link_type_id: r.get(8)?,
                max_speed_bps_override: r.get::<_, Option<i64>>(9)?.map(|v| v as u64),
                style_override: r.get(10)?,
                history_enabled: r.get::<_, i64>(11)? != 0,
                bend_points: bends,
                status: r.get(13)?,
                in_bps: r.get(14)?,
                out_bps: r.get(15)?,
                in_utilization_pct: r.get(16)?,
                out_utilization_pct: r.get(17)?,
                label: r.get(18)?,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))
}

pub fn save_link(conn: &mut Connection, link: &v1::NetworkLink) -> Result<v1::NetworkLink, ServerError> {
    let bends_json = serde_json::to_string(
        &link
            .bend_points
            .iter()
            .map(|p| StoredPt { x: p.x, y: p.y })
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".to_string());

    if link.id <= 0 {
        conn.execute(
            "INSERT INTO links (map_id, src_device_id, dst_device_id, master_device_id, mastering_type,
              interface_key, link_type_id, max_speed_bps_override, style_override, history_enabled,
              bend_points_json, label)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                link.map_id,
                link.src_device_id,
                link.dst_device_id,
                if link.master_device_id > 0 { link.master_device_id } else { link.src_device_id },
                link.mastering_type.max(1),
                link.interface_key,
                link.link_type_id.max(1),
                link.max_speed_bps_override.map(|v| v as i64),
                link.style_override,
                i64::from(link.history_enabled),
                bends_json,
                link.label
            ],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
        let mut out = link.clone();
        out.id = conn.last_insert_rowid();
        out.version = 1;
        return Ok(out);
    }

    check_version(conn, "links", link.id, link.version)?;
    conn.execute(
        "UPDATE links SET master_device_id=?1, mastering_type=?2, interface_key=?3, link_type_id=?4,
          max_speed_bps_override=?5, style_override=?6, history_enabled=?7, bend_points_json=?8,
          label=?9, version=version+1
         WHERE id=?10",
        params![
            link.master_device_id,
            link.mastering_type.max(1),
            link.interface_key,
            link.link_type_id.max(1),
            link.max_speed_bps_override.map(|v| v as i64),
            link.style_override,
            i64::from(link.history_enabled),
            bends_json,
            link.label,
            link.id
        ],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut out = link.clone();
    out.version += 1;
    Ok(out)
}

pub fn load_networks(conn: &Connection) -> Result<Vec<v1::MapNetwork>, ServerError> {
    let mut stmt = conn
        .prepare("SELECT id, version, map_id, name, cidr, pos_x, pos_y, width, height, image_id, state FROM map_networks ORDER BY id")
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(v1::MapNetwork {
                id: r.get(0)?,
                version: r.get::<_, i64>(1)? as u64,
                map_id: r.get(2)?,
                name: r.get(3)?,
                cidr: r.get(4)?,
                pos_x: r.get(5)?,
                pos_y: r.get(6)?,
                width: r.get(7)?,
                height: r.get(8)?,
                image_id: r.get(9)?,
                state: r.get(10)?,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))
}

pub fn save_network(
    conn: &mut Connection,
    net: &v1::MapNetwork,
) -> Result<v1::MapNetwork, ServerError> {
    let name = validate_entity_name(&net.name)?;
    let parsed_cidr = validate_cidr(&net.cidr)?;
    let cidr_str = parsed_cidr.to_string();
    if net.id <= 0 {
        conn.execute(
            "INSERT INTO map_networks (map_id, name, cidr, pos_x, pos_y, width, height, image_id)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                net.map_id,
                name,
                cidr_str,
                net.pos_x,
                net.pos_y,
                net.width.max(80.0),
                net.height.max(40.0),
                net.image_id
            ],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
        let mut out = net.clone();
        out.id = conn.last_insert_rowid();
        out.version = 1;
        out.name = name;
        out.cidr = cidr_str;
        return Ok(out);
    }
    check_version(conn, "map_networks", net.id, net.version)?;
    conn.execute(
        "UPDATE map_networks SET name=?1, cidr=?2, pos_x=?3, pos_y=?4, width=?5, height=?6, image_id=?7, version=version+1 WHERE id=?8",
        params![
            name,
            cidr_str,
            net.pos_x,
            net.pos_y,
            net.width.max(80.0),
            net.height.max(40.0),
            net.image_id,
            net.id
        ],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut out = net.clone();
    out.version += 1;
    out.name = name;
    out.cidr = cidr_str;
    Ok(out)
}

pub fn load_submaps(conn: &Connection) -> Result<Vec<v1::SubmapItem>, ServerError> {
    let mut stmt = conn
        .prepare("SELECT id, version, map_id, target_map_id, label, pos_x, pos_y, image_id, aggregate_state FROM submaps ORDER BY id")
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(v1::SubmapItem {
                id: r.get(0)?,
                version: r.get::<_, i64>(1)? as u64,
                map_id: r.get(2)?,
                target_map_id: r.get(3)?,
                label: r.get(4)?,
                pos_x: r.get(5)?,
                pos_y: r.get(6)?,
                image_id: r.get(7)?,
                aggregate_state: r.get(8)?,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))
}

pub fn save_submap(
    conn: &mut Connection,
    sub: &v1::SubmapItem,
) -> Result<v1::SubmapItem, ServerError> {
    let label = validate_entity_name(&sub.label)?;
    if sub.map_id == sub.target_map_id {
        return Err(ServerError::Validation(
            nextdude_common::error::CommonError::InvalidName(
                "submap cannot link to itself".to_string(),
            ),
        ));
    }
    if sub.id <= 0 {
        conn.execute(
            "INSERT INTO submaps (map_id, target_map_id, label, pos_x, pos_y, image_id)
             VALUES (?1,?2,?3,?4,?5,?6)",
            params![sub.map_id, sub.target_map_id, label, sub.pos_x, sub.pos_y, sub.image_id],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
        let mut out = sub.clone();
        out.id = conn.last_insert_rowid();
        out.version = 1;
        out.label = label;
        return Ok(out);
    }
    check_version(conn, "submaps", sub.id, sub.version)?;
    conn.execute(
        "UPDATE submaps SET target_map_id=?1, label=?2, pos_x=?3, pos_y=?4, image_id=?5, version=version+1 WHERE id=?6",
        params![sub.target_map_id, label, sub.pos_x, sub.pos_y, sub.image_id, sub.id],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut out = sub.clone();
    out.version += 1;
    out.label = label;
    Ok(out)
}

pub fn load_static_items(conn: &Connection) -> Result<Vec<v1::StaticItem>, ServerError> {
    let mut stmt = conn
        .prepare("SELECT id, version, map_id, label, kind, pos_x, pos_y, width, height, color_hex, image_id, font_size FROM static_items ORDER BY id")
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(v1::StaticItem {
                id: r.get(0)?,
                version: r.get::<_, i64>(1)? as u64,
                map_id: r.get(2)?,
                label: r.get(3)?,
                kind: r.get(4)?,
                pos_x: r.get(5)?,
                pos_y: r.get(6)?,
                width: r.get(7)?,
                height: r.get(8)?,
                color_hex: r.get(9)?,
                image_id: r.get(10)?,
                font_size: r.get::<_, i64>(11)? as u32,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))
}

pub fn save_static_item(
    conn: &mut Connection,
    item: &v1::StaticItem,
) -> Result<v1::StaticItem, ServerError> {
    let color = nextdude_common::validation::validate_hex_color(&item.color_hex)?;
    if item.id <= 0 {
        conn.execute(
            "INSERT INTO static_items (map_id, label, kind, pos_x, pos_y, width, height, color_hex, image_id, font_size)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                item.map_id,
                item.label,
                item.kind,
                item.pos_x,
                item.pos_y,
                item.width.max(20.0),
                item.height.max(20.0),
                color,
                item.image_id,
                i64::from(item.font_size.clamp(8, 72))
            ],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
        let mut out = item.clone();
        out.id = conn.last_insert_rowid();
        out.version = 1;
        out.color_hex = color;
        return Ok(out);
    }
    check_version(conn, "static_items", item.id, item.version)?;
    conn.execute(
        "UPDATE static_items SET label=?1, kind=?2, pos_x=?3, pos_y=?4, width=?5, height=?6, color_hex=?7, image_id=?8, font_size=?9, version=version+1 WHERE id=?10",
        params![
            item.label,
            item.kind,
            item.pos_x,
            item.pos_y,
            item.width.max(20.0),
            item.height.max(20.0),
            color,
            item.image_id,
            i64::from(item.font_size.clamp(8, 72)),
            item.id
        ],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut out = item.clone();
    out.version += 1;
    out.color_hex = color;
    Ok(out)
}

pub fn load_dependencies(conn: &Connection) -> Result<Vec<v1::Dependency>, ServerError> {
    let mut stmt = conn
        .prepare("SELECT id, version, map_id, parent_device_id, child_device_id FROM dependencies ORDER BY id")
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(v1::Dependency {
                id: r.get(0)?,
                version: r.get::<_, i64>(1)? as u64,
                map_id: r.get(2)?,
                parent_device_id: r.get(3)?,
                child_device_id: r.get(4)?,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))
}

pub fn save_dependency(
    conn: &mut Connection,
    dep: &v1::Dependency,
) -> Result<v1::Dependency, ServerError> {
    if dep.parent_device_id == dep.child_device_id {
        return Err(ServerError::Validation(
            nextdude_common::error::CommonError::InvalidName(
                "device cannot depend on itself".to_string(),
            ),
        ));
    }
    let existing = load_dependencies(conn)?;
    if crate::engine::dependency::would_create_cycle(&existing, dep.parent_device_id, dep.child_device_id) {
        return Err(ServerError::Validation(
            nextdude_common::error::CommonError::InvalidName(
                "dependency would create a cycle".to_string(),
            ),
        ));
    }
    conn.execute(
        "INSERT INTO dependencies (map_id, parent_device_id, child_device_id) VALUES (?1,?2,?3)",
        params![dep.map_id, dep.parent_device_id, dep.child_device_id],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut out = dep.clone();
    out.id = conn.last_insert_rowid();
    out.version = 1;
    Ok(out)
}

pub fn load_interfaces(conn: &Connection) -> Result<Vec<v1::DeviceInterface>, ServerError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, device_id, if_index, stable_key, name, descr, alias, mac, speed_bps, admin_status, oper_status, in_bps, out_bps, updated_utc
             FROM device_interfaces ORDER BY device_id, if_index",
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok(v1::DeviceInterface {
                id: r.get(0)?,
                device_id: r.get(1)?,
                if_index: r.get::<_, i64>(2)? as u32,
                stable_key: r.get(3)?,
                name: r.get(4)?,
                descr: r.get(5)?,
                alias: r.get(6)?,
                mac: r.get(7)?,
                speed_bps: r.get::<_, i64>(8)? as u64,
                admin_status: r.get::<_, i64>(9)? as u32,
                oper_status: r.get::<_, i64>(10)? as u32,
                in_bps: r.get(11)?,
                out_bps: r.get(12)?,
                updated_utc: r.get(13)?,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))
}

pub fn upsert_device_interfaces(
    conn: &mut Connection,
    device_id: i64,
    ifaces: &[v1::DeviceInterface],
) -> Result<(), ServerError> {
    let tx = conn
        .transaction()
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    for iface in ifaces {
        tx.execute(
            "INSERT INTO device_interfaces (device_id, if_index, stable_key, name, descr, alias, mac, speed_bps, admin_status, oper_status, in_bps, out_bps, updated_utc)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
             ON CONFLICT(device_id, stable_key) DO UPDATE SET
               if_index=excluded.if_index,
               name=excluded.name,
               descr=excluded.descr,
               alias=excluded.alias,
               mac=excluded.mac,
               speed_bps=excluded.speed_bps,
               admin_status=excluded.admin_status,
               oper_status=excluded.oper_status,
               in_bps=excluded.in_bps,
               out_bps=excluded.out_bps,
               updated_utc=excluded.updated_utc",
            params![
                device_id,
                i64::from(iface.if_index),
                iface.stable_key,
                iface.name,
                iface.descr,
                iface.alias,
                iface.mac,
                iface.speed_bps as i64,
                i64::from(iface.admin_status),
                i64::from(iface.oper_status),
                iface.in_bps,
                iface.out_bps,
                iface.updated_utc
            ],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    }
    tx.commit()
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    Ok(())
}

pub fn load_images(conn: &Connection) -> Result<Vec<(v1::ImageAssetMeta, String)>, ServerError> {
    let mut stmt = conn
        .prepare("SELECT id, version, name, mime_type, vendor, model, tags_json, width, height, byte_size, builtin, storage_key FROM images ORDER BY id")
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let rows = stmt
        .query_map([], |r| {
            let tags_json: String = r.get(6)?;
            Ok((
                v1::ImageAssetMeta {
                    id: r.get(0)?,
                    version: r.get::<_, i64>(1)? as u64,
                    name: r.get(2)?,
                    mime_type: r.get(3)?,
                    vendor: r.get(4)?,
                    model: r.get(5)?,
                    tags: serde_json::from_str(&tags_json).unwrap_or_default(),
                    width: r.get::<_, i64>(7)? as u32,
                    height: r.get::<_, i64>(8)? as u32,
                    byte_size: r.get::<_, i64>(9)? as u64,
                    builtin: r.get::<_, i64>(10)? != 0,
                },
                r.get::<_, String>(11)?,
            ))
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))
}

pub fn insert_image_meta(
    conn: &mut Connection,
    meta: &v1::ImageAssetMeta,
    storage_key: &str,
) -> Result<v1::ImageAssetMeta, ServerError> {
    let name = validate_entity_name(&meta.name)?;
    let tags_json = serde_json::to_string(&meta.tags).unwrap_or_else(|_| "[]".to_string());
    conn.execute(
        "INSERT INTO images (name, mime_type, vendor, model, tags_json, width, height, byte_size, builtin, storage_key)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,0,?9)",
        params![
            name,
            meta.mime_type,
            meta.vendor,
            meta.model,
            tags_json,
            i64::from(meta.width),
            i64::from(meta.height),
            meta.byte_size as i64,
            storage_key
        ],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut out = meta.clone();
    out.id = conn.last_insert_rowid();
    out.version = 1;
    out.name = name;
    Ok(out)
}
