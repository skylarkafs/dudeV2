use nextdude_server::discovery::mndp::parse_mndp_packet;
use nextdude_server::discovery::{decode_cdp_neighbors, decode_lldp_neighbors};
use nextdude_server::snmp::ber::{SnmpValue, Varbind};
use nextdude_server::syslog::parse_syslog_line;
use std::net::{IpAddr, Ipv4Addr};

#[test]
fn syslog_parses_rfc3164_rfc5424_mikrotik_cisco_huawei() {
    let p_3164 = parse_syslog_line("<34>Oct 11 22:14:15 core-sw01 sshd: Failed password for root");
    assert_eq!(p_3164.facility, 4);
    assert_eq!(p_3164.severity, 2);
    assert_eq!(p_3164.hostname, "core-sw01");
    assert_eq!(p_3164.app_name, "sshd");
    assert_eq!(p_3164.message, "Failed password for root");

    let p_5424 = parse_syslog_line("<165>1 2026-11-25T10:00:00Z edge-gw - - - [meta id=\"1\"] Interface ge-0/0/0 down");
    assert_eq!(p_5424.format, "rfc5424");
    assert_eq!(p_5424.hostname, "edge-gw");
    assert_eq!(p_5424.message, "Interface ge-0/0/0 down");

    let p_mkt = parse_syslog_line("<30>system,info,account: user admin logged in from 10.0.0.5");
    assert_eq!(p_mkt.app_name, "system,info,account");
    assert!(p_mkt.message.contains("logged in"));

    let p_cisco = parse_syslog_line("<187>124: *Mar  1 00:02:15.100: %LINK-3-UPDOWN: Interface GigabitEthernet0/1, changed state to down");
    assert_eq!(p_cisco.format, "cisco_3164");
    assert_eq!(p_cisco.app_name, "LINK-3-UPDOWN");
    assert!(p_cisco.message.contains("changed state to down"));
}

#[test]
fn mndp_tlv_parser_decodes_mac_identity_little_endian_uptime_and_ipv4() {
    let mut pkt = vec![0x00, 0x00, 0x00, 0x01]; // 4-byte MNDP header
    // MAC (0x0001)
    pkt.extend_from_slice(&[0x00, 0x01, 0x00, 0x06, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0x01]);
    // Identity (0x0005)
    let id_b = b"MKT-CORE";
    pkt.extend_from_slice(&0x0005u16.to_be_bytes());
    pkt.extend_from_slice(&(id_b.len() as u16).to_be_bytes());
    pkt.extend_from_slice(id_b);
    // Uptime (0x000a) little-endian 3600s
    pkt.extend_from_slice(&[0x00, 0x0a, 0x00, 0x04]);
    pkt.extend_from_slice(&3600u32.to_le_bytes());
    // Interface (0x0010)
    let if_b = b"ether1";
    pkt.extend_from_slice(&0x0010u16.to_be_bytes());
    pkt.extend_from_slice(&(if_b.len() as u16).to_be_bytes());
    pkt.extend_from_slice(if_b);
    // IPv4 (0x0011)
    pkt.extend_from_slice(&[0x00, 0x11, 0x00, 0x04, 192, 168, 88, 1]);

    let parsed = parse_mndp_packet(IpAddr::V4(Ipv4Addr::new(192, 168, 88, 1)), &pkt, 1000).unwrap();
    assert_eq!(parsed.mac_address, "AA:BB:CC:DD:EE:01");
    assert_eq!(parsed.identity, "MKT-CORE");
    assert_eq!(parsed.uptime_sec, 3600);
    assert_eq!(parsed.interface_name, "ether1");
    assert_eq!(parsed.ipv4, Some(Ipv4Addr::new(192, 168, 88, 1)));
}

#[test]
fn lldp_and_cdp_neighbor_decoders_extract_links() {
    let lldp = decode_lldp_neighbors(
        &[Varbind {
            oid: vec![1, 0, 8802, 1, 1, 2, 1, 4, 1, 1, 9, 100, 2, 1],
            value: SnmpValue::OctetString(b"dist-sw-02".to_vec()),
        }],
        &[Varbind {
            oid: vec![1, 0, 8802, 1, 1, 2, 1, 4, 1, 1, 7, 100, 2, 1],
            value: SnmpValue::OctetString(b"sfp-sfpplus1".to_vec()),
        }],
    );
    assert_eq!(lldp.len(), 1);
    assert_eq!(lldp[0].remote_sys_name, "dist-sw-02");
    assert_eq!(lldp[0].remote_port, "sfp-sfpplus1");

    let cdp = decode_cdp_neighbors(
        &[Varbind {
            oid: vec![1, 3, 6, 1, 4, 1, 9, 9, 23, 1, 2, 1, 1, 6, 4, 1],
            value: SnmpValue::OctetString(b"access-sw-09".to_vec()),
        }],
        &[Varbind {
            oid: vec![1, 3, 6, 1, 4, 1, 9, 9, 23, 1, 2, 1, 1, 7, 4, 1],
            value: SnmpValue::OctetString(b"Gi0/24".to_vec()),
        }],
        &[Varbind {
            oid: vec![1, 3, 6, 1, 4, 1, 9, 9, 23, 1, 2, 1, 1, 4, 4, 1],
            value: SnmpValue::OctetString(vec![10, 1, 2, 9]),
        }],
    );
    assert_eq!(cdp.len(), 1);
    assert_eq!(cdp[0].remote_sys_name, "access-sw-09");
    assert_eq!(cdp[0].remote_ip.as_deref(), Some("10.1.2.9"));
}
