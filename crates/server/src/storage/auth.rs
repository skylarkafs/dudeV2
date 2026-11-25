use crate::crypto::{hash_password, verify_password};
use crate::error::ServerError;
use nextdude_common::proto::v1;
use nextdude_common::validation::{validate_entity_name, validate_password};
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, Clone)]
pub struct AuthenticatedAdmin {
    pub admin_id: i64,
    pub username: String,
    pub group_name: String,
    pub permissions: Vec<String>,
}

pub fn create_admin(
    conn: &mut Connection,
    username: &str,
    password: &str,
    group_name: &str,
) -> Result<i64, ServerError> {
    let username = validate_entity_name(username)?;
    validate_password(password)?;
    let hash = hash_password(password)?;

    let group_id: i64 = conn
        .query_row(
            "SELECT id FROM admin_groups WHERE name = ?1",
            params![group_name],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| ServerError::Storage(e.to_string()))?
        .ok_or_else(|| ServerError::NotFound(format!("admin group '{group_name}'")))?;

    conn.execute(
        "INSERT INTO admins (username, password_hash, group_id, enabled) VALUES (?1, ?2, ?3, 1)",
        params![username, hash, group_id],
    )
    .map_err(|e| ServerError::Storage(format!("create admin failed: {e}")))?;
    let id = conn.last_insert_rowid();
    conn.execute(
        "INSERT OR IGNORE INTO user_preferences (admin_id) VALUES (?1)",
        params![id],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    Ok(id)
}

pub fn reset_admin_password(
    conn: &mut Connection,
    username: &str,
    new_password: &str,
) -> Result<(), ServerError> {
    validate_password(new_password)?;
    let hash = hash_password(new_password)?;
    let changed = conn
        .execute(
            "UPDATE admins SET password_hash = ?1, failed_attempts = 0, locked_until_utc = 0, version = version + 1 WHERE username = ?2",
            params![hash, username],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    if changed == 0 {
        return Err(ServerError::NotFound(format!("admin '{username}'")));
    }
    Ok(())
}

type AdminAuthRow = (i64, String, String, String, String, bool, i64, i64);

pub fn authenticate_admin(
    conn: &mut Connection,
    username: &str,
    password: &str,
    max_attempts: u32,
    lockout_sec: i64,
    now_utc: i64,
) -> Result<AuthenticatedAdmin, ServerError> {
    let row: Option<AdminAuthRow> = conn
        .query_row(
            "SELECT a.id, a.username, a.password_hash, g.name, g.permissions_json, a.enabled, a.failed_attempts, a.locked_until_utc
             FROM admins a JOIN admin_groups g ON a.group_id = g.id
             WHERE a.username = ?1",
            params![username],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get::<_, i64>(5)? != 0,
                    r.get(6)?,
                    r.get(7)?,
                ))
            },
        )
        .optional()
        .map_err(|e| ServerError::Storage(e.to_string()))?;

    let Some((id, uname, hash, group_name, perms_json, enabled, failed, locked_until)) = row else {
        return Err(ServerError::AuthFailed("invalid username or password".to_string()));
    };

    if !enabled {
        return Err(ServerError::AuthFailed("account is disabled".to_string()));
    }
    if locked_until > now_utc {
        return Err(ServerError::AccountLocked(locked_until));
    }

    if !verify_password(password, &hash)? {
        let next_failed = failed + 1;
        let next_lock = if next_failed >= i64::from(max_attempts) {
            now_utc + lockout_sec
        } else {
            0
        };
        conn.execute(
            "UPDATE admins SET failed_attempts = ?1, locked_until_utc = ?2 WHERE id = ?3",
            params![next_failed, next_lock, id],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
        if next_lock > 0 {
            return Err(ServerError::AccountLocked(next_lock));
        }
        return Err(ServerError::AuthFailed("invalid username or password".to_string()));
    }

    conn.execute(
        "UPDATE admins SET failed_attempts = 0, locked_until_utc = 0 WHERE id = ?1",
        params![id],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;

    let permissions: Vec<String> = serde_json::from_str(&perms_json).unwrap_or_default();
    Ok(AuthenticatedAdmin {
        admin_id: id,
        username: uname,
        group_name,
        permissions,
    })
}

pub fn load_user_preferences(conn: &Connection, admin_id: i64) -> Result<v1::UserPreferences, ServerError> {
    let row = conn
        .query_row(
            "SELECT last_map_id, panel_layout_json, filters_json FROM user_preferences WHERE admin_id = ?1",
            params![admin_id],
            |r| {
                Ok(v1::UserPreferences {
                    admin_id,
                    last_map_id: r.get(0)?,
                    panel_layout_json: r.get(1)?,
                    filters_json: r.get(2)?,
                })
            },
        )
        .optional()
        .map_err(|e| ServerError::Storage(e.to_string()))?;

    Ok(row.unwrap_or(v1::UserPreferences {
        admin_id,
        last_map_id: None,
        panel_layout_json: "{}".to_string(),
        filters_json: "{}".to_string(),
    }))
}

pub fn save_user_preferences(
    conn: &mut Connection,
    prefs: &v1::UserPreferences,
) -> Result<v1::UserPreferences, ServerError> {
    conn.execute(
        "INSERT INTO user_preferences (admin_id, last_map_id, panel_layout_json, filters_json)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(admin_id) DO UPDATE SET
           last_map_id = excluded.last_map_id,
           panel_layout_json = excluded.panel_layout_json,
           filters_json = excluded.filters_json",
        params![
            prefs.admin_id,
            prefs.last_map_id,
            prefs.panel_layout_json,
            prefs.filters_json
        ],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    Ok(prefs.clone())
}

pub fn list_admins_and_groups(
    conn: &Connection,
) -> Result<(Vec<v1::AdminUser>, Vec<v1::AdminGroup>), ServerError> {
    let mut g_stmt = conn
        .prepare("SELECT id, version, name, permissions_json FROM admin_groups ORDER BY id")
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let groups = g_stmt
        .query_map([], |r| {
            let perms_json: String = r.get(3)?;
            let permissions: Vec<String> = serde_json::from_str(&perms_json).unwrap_or_default();
            Ok(v1::AdminGroup {
                id: r.get(0)?,
                version: r.get::<_, i64>(1)? as u64,
                name: r.get(2)?,
                permissions,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))?;

    let mut u_stmt = conn
        .prepare(
            "SELECT a.id, a.version, a.username, a.group_id, g.name, a.enabled, a.locked_until_utc
             FROM admins a JOIN admin_groups g ON a.group_id = g.id ORDER BY a.id",
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    let users = u_stmt
        .query_map([], |r| {
            Ok(v1::AdminUser {
                id: r.get(0)?,
                version: r.get::<_, i64>(1)? as u64,
                username: r.get(2)?,
                group_id: r.get(3)?,
                group_name: r.get(4)?,
                enabled: r.get::<_, i64>(5)? != 0,
                password: None,
                locked_until_utc: r.get(6)?,
            })
        })
        .map_err(|e| ServerError::Storage(e.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| ServerError::Storage(e.to_string()))?;

    Ok((users, groups))
}

pub fn save_admin_user(conn: &mut Connection, user: &v1::AdminUser) -> Result<v1::AdminUser, ServerError> {
    let uname = validate_entity_name(&user.username)?;
    if user.id <= 0 {
        let pass = user
            .password
            .as_deref()
            .ok_or_else(|| ServerError::Validation(nextdude_common::error::CommonError::InvalidPassword("password required for new admin".to_string())))?;
        validate_password(pass)?;
        let hash = hash_password(pass)?;
        conn.execute(
            "INSERT INTO admins (username, password_hash, group_id, enabled) VALUES (?1, ?2, ?3, ?4)",
            params![uname, hash, user.group_id, i64::from(user.enabled)],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
        let id = conn.last_insert_rowid();
        conn.execute(
            "INSERT OR IGNORE INTO user_preferences (admin_id) VALUES (?1)",
            params![id],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
        let mut out = user.clone();
        out.id = id;
        out.version = 1;
        out.password = None;
        return Ok(out);
    }

    let current_ver: i64 = conn
        .query_row("SELECT version FROM admins WHERE id = ?1", params![user.id], |r| r.get(0))
        .optional()
        .map_err(|e| ServerError::Storage(e.to_string()))?
        .ok_or_else(|| ServerError::NotFound(format!("admin {}", user.id)))?;
    if current_ver as u64 != user.version {
        return Err(ServerError::VersionConflict {
            expected: user.version,
            current: current_ver as u64,
        });
    }

    if let Some(ref pass) = user.password {
        if !pass.is_empty() {
            validate_password(pass)?;
            let hash = hash_password(pass)?;
            conn.execute(
                "UPDATE admins SET username = ?1, group_id = ?2, enabled = ?3, password_hash = ?4, locked_until_utc = 0, failed_attempts = 0, version = version + 1 WHERE id = ?5",
                params![uname, user.group_id, i64::from(user.enabled), hash, user.id],
            )
            .map_err(|e| ServerError::Storage(e.to_string()))?;
        } else {
            conn.execute(
                "UPDATE admins SET username = ?1, group_id = ?2, enabled = ?3, version = version + 1 WHERE id = ?4",
                params![uname, user.group_id, i64::from(user.enabled), user.id],
            )
            .map_err(|e| ServerError::Storage(e.to_string()))?;
        }
    } else {
        conn.execute(
            "UPDATE admins SET username = ?1, group_id = ?2, enabled = ?3, version = version + 1 WHERE id = ?4",
            params![uname, user.group_id, i64::from(user.enabled), user.id],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
    }

    let mut out = user.clone();
    out.version += 1;
    out.password = None;
    Ok(out)
}

pub fn save_admin_group(
    conn: &mut Connection,
    group: &v1::AdminGroup,
) -> Result<v1::AdminGroup, ServerError> {
    let name = validate_entity_name(&group.name)?;
    for p in &group.permissions {
        nextdude_common::types::Permission::parse(p)?;
    }
    let perms_json = serde_json::to_string(&group.permissions).unwrap_or_else(|_| "[]".to_string());
    if group.id <= 0 {
        conn.execute(
            "INSERT INTO admin_groups (name, permissions_json) VALUES (?1, ?2)",
            params![name, perms_json],
        )
        .map_err(|e| ServerError::Storage(e.to_string()))?;
        let mut out = group.clone();
        out.id = conn.last_insert_rowid();
        out.version = 1;
        return Ok(out);
    }

    let current_ver: i64 = conn
        .query_row(
            "SELECT version FROM admin_groups WHERE id = ?1",
            params![group.id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| ServerError::Storage(e.to_string()))?
        .ok_or_else(|| ServerError::NotFound(format!("admin group {}", group.id)))?;
    if current_ver as u64 != group.version {
        return Err(ServerError::VersionConflict {
            expected: group.version,
            current: current_ver as u64,
        });
    }
    conn.execute(
        "UPDATE admin_groups SET name = ?1, permissions_json = ?2, version = version + 1 WHERE id = ?3",
        params![name, perms_json, group.id],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    let mut out = group.clone();
    out.version += 1;
    Ok(out)
}

pub fn record_history_action(
    conn: &mut Connection,
    admin_id: i64,
    username: &str,
    action: &str,
    target_type: &str,
    target_id: &str,
    details: &str,
) -> Result<(), ServerError> {
    let now = chrono::Utc::now().timestamp();
    conn.execute(
        "INSERT INTO history_actions (timestamp_utc, admin_id, username, action, target_type, target_id, details)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![now, admin_id, username, action, target_type, target_id, details],
    )
    .map_err(|e| ServerError::Storage(e.to_string()))?;
    Ok(())
}
