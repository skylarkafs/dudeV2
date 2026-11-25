use nextdude_server::snmp::ber::{
    decode_message, encode_integer, encode_oid, encode_unsigned, encode_v1_or_v2_message,
    Pdu, PduType, SnmpMessage, SnmpValue, TrapV1Pdu, V1OrV2Pdu, Varbind, TAG_COUNTER32,
    TAG_COUNTER64,
};
use nextdude_server::snmp::usm::{
    decode_and_verify_v3, encode_v3_message, AuthProto, PrivProto, UsmCredentials,
    UsmSecurityParams,
};
use nextdude_server::snmp::{ResolvedCredential, SnmpClient, SnmpTarget};
use nextdude_server::trap::decode_trap_packet;
use nextdude_common::proto::v1;
use std::time::Duration;
use tokio::net::UdpSocket;

#[test]
fn ber_oid_and_integer_test_vectors() {
    let mut oid_buf = Vec::new();
    encode_oid(&[1, 3, 6, 1, 2, 1, 1, 1, 0], &mut oid_buf).unwrap();
    assert_eq!(oid_buf, vec![0x06, 0x08, 0x2b, 0x06, 0x01, 0x02, 0x01, 0x01, 0x01, 0x00]);

    let mut int_zero = Vec::new();
    encode_integer(0, &mut int_zero);
    assert_eq!(int_zero, vec![0x02, 0x01, 0x00]);

    let mut int_neg = Vec::new();
    encode_integer(-129, &mut int_neg);
    assert_eq!(int_neg, vec![0x02, 0x02, 0xff, 0x7f]);

    // Counter32 >= 0x8000_0000 requires leading 0x00 byte so BER treats it as unsigned
    let mut c32 = Vec::new();
    encode_unsigned(TAG_COUNTER32, 0x8000_0001, &mut c32);
    assert_eq!(c32, vec![0x41, 0x05, 0x00, 0x80, 0x00, 0x00, 0x01]);

    let mut c64 = Vec::new();
    encode_unsigned(TAG_COUNTER64, 0x8000_0000_0000_0005, &mut c64);
    assert_eq!(c64.len(), 11);
    assert_eq!(c64[0..3], [0x46, 0x09, 0x00]);
}

#[test]
fn snmp_v2c_and_v3_usm_auth_priv_roundtrip() {
    let pdu = Pdu {
        pdu_type: PduType::GetRequest,
        request_id: 4242,
        error_status: 0,
        error_index: 0,
        varbinds: vec![Varbind {
            oid: vec![1, 3, 6, 1, 2, 1, 1, 5, 0],
            value: SnmpValue::Null,
        }],
    };
    let wire_v2 = encode_v1_or_v2_message(&SnmpMessage::V2c {
        community: b"public".to_vec(),
        pdu: pdu.clone(),
    })
    .unwrap();
    let dec_v2 = decode_message(&wire_v2).unwrap();
    assert_eq!(
        dec_v2,
        SnmpMessage::V2c {
            community: b"public".to_vec(),
            pdu: pdu.clone(),
        }
    );

    // Test both MD5+DES and SHA1+AES128
    for (auth, priv_p) in [
        (AuthProto::Md5, PrivProto::Des),
        (AuthProto::Sha1, PrivProto::Aes128),
    ] {
        let creds = UsmCredentials {
            username: "noc_v3".to_string(),
            auth_proto: auth,
            auth_password: "AuthPassword123".to_string(),
            priv_proto: priv_p,
            priv_password: "PrivPassword456".to_string(),
        };
        let mut sec = UsmSecurityParams {
            engine_id: vec![0x80, 0x00, 0x1f, 0x88, 0x04, 0x74, 0x65, 0x73, 0x74],
            engine_boots: 7,
            engine_time: 12345,
            salt_counter: 99,
        };
        let wire_v3 = encode_v3_message(77, true, &creds, &mut sec, &sec.engine_id.clone(), b"", &pdu).unwrap();
        let parsed = decode_message(&wire_v3).unwrap();
        let (_, dec_pdu) = decode_and_verify_v3(&wire_v3, parsed, Some(&creds)).unwrap();
        assert_eq!(dec_pdu, pdu);

        // Tamper detection check
        let mut tampered = wire_v3.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xFF;
        let parsed_t = decode_message(&tampered).unwrap();
        assert!(decode_and_verify_v3(&tampered, parsed_t, Some(&creds)).is_err());
    }
}

#[test]
fn trap_v1_v2c_and_inform_decoding() {
    // SNMPv1 linkDown trap
    let trap_v1 = SnmpMessage::V1 {
        community: b"trapcomm".to_vec(),
        pdu: V1OrV2Pdu::TrapV1(TrapV1Pdu {
            enterprise: vec![1, 3, 6, 1, 4, 1, 14988],
            agent_addr: [10, 0, 0, 1],
            generic_trap: 2, // linkDown
            specific_trap: 0,
            time_stamp: 5000,
            varbinds: vec![Varbind {
                oid: vec![1, 3, 6, 1, 2, 1, 2, 2, 1, 1, 5],
                value: SnmpValue::Integer(5),
            }],
        }),
    };
    let wire1 = encode_v1_or_v2_message(&trap_v1).unwrap();
    let dec1 = decode_trap_packet(&wire1, None).unwrap();
    assert_eq!(dec1.snmp_version, v1::SnmpVersion::V1);
    assert_eq!(dec1.trap_oid, "1.3.6.1.6.3.1.1.5.3");
    assert_eq!(dec1.trap_name, "IF-MIB::linkDown");
    assert!(!dec1.is_inform);

    // SNMPv2c InformRequest
    let inform = SnmpMessage::V2c {
        community: b"public".to_vec(),
        pdu: Pdu {
            pdu_type: PduType::InformRequest,
            request_id: 901,
            error_status: 0,
            error_index: 0,
            varbinds: vec![Varbind {
                oid: vec![1, 3, 6, 1, 6, 3, 1, 1, 4, 1, 0],
                value: SnmpValue::Oid(vec![1, 3, 6, 1, 6, 3, 1, 1, 5, 4]),
            }],
        },
    };
    let wire2 = encode_v1_or_v2_message(&inform).unwrap();
    let dec2 = decode_trap_packet(&wire2, None).unwrap();
    assert!(dec2.is_inform);
    assert_eq!(dec2.request_id, 901);
    assert_eq!(dec2.trap_name, "IF-MIB::linkUp");
}

#[tokio::test]
async fn snmp_client_queries_local_fake_udp_agent() {
    let agent_sock = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let agent_addr = agent_sock.local_addr().unwrap();

    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        if let Ok((n, src)) = agent_sock.recv_from(&mut buf).await {
            if let Ok(SnmpMessage::V2c { community, pdu }) = decode_message(&buf[..n]) {
                let resp = SnmpMessage::V2c {
                    community,
                    pdu: Pdu {
                        pdu_type: PduType::Response,
                        request_id: pdu.request_id,
                        error_status: 0,
                        error_index: 0,
                        varbinds: vec![Varbind {
                            oid: vec![1, 3, 6, 1, 2, 1, 1, 5, 0],
                            value: SnmpValue::OctetString(b"core-rtr-01".to_vec()),
                        }],
                    },
                };
                let wire = encode_v1_or_v2_message(&resp).unwrap();
                let _ = agent_sock.send_to(&wire, src).await;
            }
        }
    });

    let client = SnmpClient::new();
    let target = SnmpTarget {
        addr: agent_addr,
        timeout: Duration::from_secs(2),
        retries: 0,
        credential: ResolvedCredential::V2c {
            community: "public".to_string(),
        },
    };
    let res = client
        .get(&target, &[vec![1, 3, 6, 1, 2, 1, 1, 5, 0]])
        .await
        .unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].value.as_string_lossy(), "core-rtr-01");
}
