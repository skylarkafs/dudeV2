use crate::error::CommonError;
use ipnet::IpNet;
use std::net::IpAddr;

pub fn validate_entity_name(name: &str) -> Result<String, CommonError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > 128 {
        return Err(CommonError::InvalidName(
            "name must be 1..128 characters".to_string(),
        ));
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(CommonError::InvalidName(
            "name contains control characters".to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

pub fn validate_host_or_ip(addr: &str) -> Result<String, CommonError> {
    let trimmed = addr.trim();
    if trimmed.is_empty() || trimmed.len() > 253 {
        return Err(CommonError::InvalidAddress(addr.to_string()));
    }
    if trimmed.parse::<IpAddr>().is_ok() {
        return Ok(trimmed.to_string());
    }
    // RFC 1123 hostname validation (allows underscore for internal records)
    let valid_labels = trimmed.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    });
    if valid_labels {
        Ok(trimmed.to_string())
    } else {
        Err(CommonError::InvalidAddress(addr.to_string()))
    }
}

pub fn validate_cidr(cidr: &str) -> Result<IpNet, CommonError> {
    let trimmed = cidr.trim();
    trimmed
        .parse::<IpNet>()
        .map_err(|_| CommonError::InvalidCidr(cidr.to_string()))
}

pub fn validate_oid(oid: &str) -> Result<Vec<u32>, CommonError> {
    let trimmed = oid.trim().trim_start_matches('.');
    if trimmed.is_empty() {
        return Err(CommonError::InvalidOid(oid.to_string()));
    }
    let mut arcs = Vec::new();
    for part in trimmed.split('.') {
        let arc = part
            .parse::<u32>()
            .map_err(|_| CommonError::InvalidOid(oid.to_string()))?;
        arcs.push(arc);
    }
    if arcs.len() < 2 || arcs[0] > 2 || (arcs[0] < 2 && arcs[1] > 39) {
        return Err(CommonError::InvalidOid(oid.to_string()));
    }
    Ok(arcs)
}

pub fn validate_port(port: u32) -> Result<u16, CommonError> {
    if port == 0 || port > u32::from(u16::MAX) {
        return Err(CommonError::InvalidPort(port));
    }
    Ok(port as u16)
}

pub fn validate_hex_color(color: &str) -> Result<String, CommonError> {
    let trimmed = color.trim();
    let hex_part = trimmed.strip_prefix('#').unwrap_or(trimmed);
    if (hex_part.len() == 6 || hex_part.len() == 8)
        && hex_part.chars().all(|c| c.is_ascii_hexdigit())
    {
        Ok(format!("#{}", hex_part.to_ascii_uppercase()))
    } else {
        Err(CommonError::InvalidColor(color.to_string()))
    }
}

pub fn validate_https_url(url: &str) -> Result<String, CommonError> {
    let trimmed = url.trim();
    if !trimmed.starts_with("https://") || trimmed.len() > 2048 || trimmed.contains(char::is_whitespace) {
        return Err(CommonError::InvalidUrl(url.to_string()));
    }
    let after_scheme = &trimmed["https://".len()..];
    let host_port = after_scheme.split('/').next().unwrap_or("");
    if host_port.is_empty() {
        return Err(CommonError::InvalidUrl(url.to_string()));
    }
    Ok(trimmed.to_string())
}

pub fn validate_password(password: &str) -> Result<(), CommonError> {
    if password.len() < 8 {
        return Err(CommonError::InvalidPassword(
            "password must be at least 8 characters".to_string(),
        ));
    }
    if password.len() > 128 {
        return Err(CommonError::InvalidPassword(
            "password must be at most 128 characters".to_string(),
        ));
    }
    Ok(())
}
