use nextdude_common::oid_dict::resolve_oid_name;
use nextdude_common::types::{Permission, RedactSecret};
use nextdude_common::validation::{
    validate_cidr, validate_entity_name, validate_hex_color, validate_host_or_ip,
    validate_https_url, validate_oid, validate_password, validate_port,
};

#[test]
fn secret_redaction_never_leaks_value() {
    let secret = RedactSecret::new("super_secret_community");
    assert_eq!(format!("{secret:?}"), "[REDACTED]");
    assert_eq!(format!("{secret}"), "[REDACTED]");
    assert_eq!(secret.expose(), "super_secret_community");
}

#[test]
fn validators_accept_and_reject_expected_inputs() {
    assert!(validate_entity_name("Core-Router-01").is_ok());
    assert!(validate_entity_name("   ").is_err());
    assert!(validate_entity_name("bad\nname").is_err());

    assert!(validate_host_or_ip("192.168.88.1").is_ok());
    assert!(validate_host_or_ip("2001:db8::1").is_ok());
    assert!(validate_host_or_ip("core-sw01.corp.local").is_ok());
    assert!(validate_host_or_ip("-bad.host").is_err());

    assert!(validate_cidr("10.10.0.0/24").is_ok());
    assert!(validate_cidr("10.10.0.0/33").is_err());

    assert_eq!(
        validate_oid(".1.3.6.1.2.1.1.1.0").expect("valid OID"),
        vec![1, 3, 6, 1, 2, 1, 1, 1, 0]
    );
    assert!(validate_oid("3.1.2").is_err());
    assert!(validate_oid("1.abc").is_err());

    assert_eq!(validate_port(9443).expect("valid port"), 9443);
    assert!(validate_port(0).is_err());
    assert!(validate_port(70000).is_err());

    assert_eq!(validate_hex_color("#22c55e").expect("hex"), "#22C55E");
    assert!(validate_hex_color("#zzzzzz").is_err());

    assert!(validate_https_url("https://example.com/icon.png").is_ok());
    assert!(validate_https_url("http://example.com/icon.png").is_err());

    assert!(validate_password("StrongPass123").is_ok());
    assert!(validate_password("short").is_err());
}

#[test]
fn oid_dictionary_resolves_standard_and_vendor_oids() {
    assert_eq!(
        resolve_oid_name(".1.3.6.1.2.1.1.5.0"),
        "SNMPv2-MIB::sysName.0"
    );
    assert_eq!(
        resolve_oid_name("1.3.6.1.2.1.31.1.1.1.6.12"),
        "IF-MIB::ifHCInOctets.12"
    );
    assert_eq!(
        resolve_oid_name("1.3.6.1.4.1.14988.1.1.3.10.0"),
        "MIKROTIK-MIB::mtxrHlTemperature.0"
    );
    assert_eq!(resolve_oid_name("1.3.999.1"), "1.3.999.1");
}

#[test]
fn permission_round_trip() {
    for perm in Permission::ALL {
        let parsed = Permission::parse(perm.as_str()).expect("known perm");
        assert_eq!(parsed, perm);
    }
    assert!(Permission::parse("unknown_perm").is_err());
}
