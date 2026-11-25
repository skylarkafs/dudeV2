pub mod v1 {
    tonic::include_proto!("nextdude.v1");
}

use crate::types::Permission;

pub fn default_state_palette(
    up: &str,
    down: &str,
    unreachable: &str,
    partial: &str,
    unknown: &str,
    flapping: &str,
    no_reply: &str,
) -> v1::StateColorPalette {
    v1::StateColorPalette {
        up_hex: up.to_string(),
        down_hex: down.to_string(),
        unreachable_hex: unreachable.to_string(),
        partial_hex: partial.to_string(),
        unknown_hex: unknown.to_string(),
        flapping_hex: flapping.to_string(),
        no_reply_hex: no_reply.to_string(),
    }
}

pub fn default_map_palettes() -> (
    v1::StateColorPalette,
    v1::StateColorPalette,
    v1::StateColorPalette,
    v1::StateColorPalette,
    v1::StateColorPalette,
) {
    let p = default_state_palette(
        "#22C55E",
        "#EF4444",
        "#F97316",
        "#EAB308",
        "#64748B",
        "#A855F7",
        "#F59E0B",
    );
    (p.clone(), p.clone(), p.clone(), p.clone(), p)
}

pub fn has_permission(permissions: &[String], required: Permission) -> bool {
    let key = required.as_str();
    permissions.iter().any(|p| p == key)
}
