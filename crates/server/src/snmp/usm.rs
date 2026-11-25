use crate::error::ServerError;
use crate::snmp::ber::{
    decode_scoped_pdu, encode_integer, encode_length, encode_scoped_pdu, encode_tlv,
    encode_usm_security_params, Pdu, ScopedPduPayload, SnmpMessage, UsmHeader, TAG_OCTET_STRING,
    TAG_SEQUENCE,
};
use aes::Aes128;
use cfb_mode::cipher::{AsyncStreamCipher, KeyIvInit as StreamKeyIvInit};
use cipher::block_padding::NoPadding;
use cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit as BlockKeyIvInit};
use des::Des;
use hmac::{Hmac, Mac};
use md5::{Digest as Md5Digest, Md5};
use sha1::Sha1;

type HmacMd5 = Hmac<Md5>;
type HmacSha1 = Hmac<Sha1>;
type DesCbcEnc = cbc::Encryptor<Des>;
type DesCbcDec = cbc::Decryptor<Des>;
type Aes128CfbEnc = cfb_mode::Encryptor<Aes128>;
type Aes128CfbDec = cfb_mode::Decryptor<Aes128>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthProto {
    None,
    Md5,
    Sha1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivProto {
    None,
    Des,
    Aes128,
}

#[derive(Debug, Clone)]
pub struct UsmCredentials {
    pub username: String,
    pub auth_proto: AuthProto,
    pub auth_password: String,
    pub priv_proto: PrivProto,
    pub priv_password: String,
}

#[derive(Debug, Clone)]
pub struct UsmSecurityParams {
    pub engine_id: Vec<u8>,
    pub engine_boots: i32,
    pub engine_time: i32,
    pub salt_counter: u64,
}

/// RFC 3414 Appendix A.2.1 password-to-key expansion (1 MiB stream).
pub fn password_to_key(proto: AuthProto, password: &[u8]) -> Vec<u8> {
    if password.is_empty() || proto == AuthProto::None {
        return Vec::new();
    }
    const EXPAND_LEN: usize = 1_048_576;
    let mut block = [0u8; 64];
    let mut p_idx = 0usize;
    let plen = password.len();

    match proto {
        AuthProto::Md5 => {
            let mut hasher = Md5::new();
            let mut count = 0usize;
            while count < EXPAND_LEN {
                for b in &mut block {
                    *b = password[p_idx % plen];
                    p_idx += 1;
                }
                hasher.update(block);
                count += 64;
            }
            hasher.finalize().to_vec()
        }
        AuthProto::Sha1 => {
            let mut hasher = Sha1::new();
            let mut count = 0usize;
            while count < EXPAND_LEN {
                for b in &mut block {
                    *b = password[p_idx % plen];
                    p_idx += 1;
                }
                hasher.update(block);
                count += 64;
            }
            hasher.finalize().to_vec()
        }
        AuthProto::None => Vec::new(),
    }
}

/// RFC 3414 Section 2.6 key localization: Hash(Ku || engineID || Ku).
pub fn localize_key(proto: AuthProto, ku: &[u8], engine_id: &[u8]) -> Vec<u8> {
    match proto {
        AuthProto::Md5 => {
            let mut h = Md5::new();
            h.update(ku);
            h.update(engine_id);
            h.update(ku);
            h.finalize().to_vec()
        }
        AuthProto::Sha1 => {
            let mut h = Sha1::new();
            h.update(ku);
            h.update(engine_id);
            h.update(ku);
            h.finalize().to_vec()
        }
        AuthProto::None => Vec::new(),
    }
}

pub fn compute_hmac_96(proto: AuthProto, kul: &[u8], message: &[u8]) -> Result<[u8; 12], ServerError> {
    let mut out = [0u8; 12];
    match proto {
        AuthProto::Md5 => {
            let mut mac = HmacMd5::new_from_slice(kul)
                .map_err(|e| ServerError::Snmp(format!("HMAC-MD5 key error: {e}")))?;
            mac.update(message);
            let digest = mac.finalize().into_bytes();
            out.copy_from_slice(&digest[..12]);
        }
        AuthProto::Sha1 => {
            let mut mac = HmacSha1::new_from_slice(kul)
                .map_err(|e| ServerError::Snmp(format!("HMAC-SHA1 key error: {e}")))?;
            mac.update(message);
            let digest = mac.finalize().into_bytes();
            out.copy_from_slice(&digest[..12]);
        }
        AuthProto::None => {
            return Err(ServerError::Snmp("cannot compute HMAC with AuthProto::None".to_string()));
        }
    }
    Ok(out)
}

fn ct_eq_12(a: &[u8], b: &[u8; 12]) -> bool {
    if a.len() != 12 {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..12 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

pub fn encrypt_scoped_pdu(
    priv_proto: PrivProto,
    priv_kul: &[u8],
    engine_boots: i32,
    engine_time: i32,
    salt_counter: u64,
    plain_scoped: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), ServerError> {
    match priv_proto {
        PrivProto::None => Err(ServerError::Snmp("encrypt called with PrivProto::None".to_string())),
        PrivProto::Des => {
            if priv_kul.len() < 16 {
                return Err(ServerError::Snmp("DES localized key must be >= 16 bytes".to_string()));
            }
            let key = &priv_kul[..8];
            let pre_iv = &priv_kul[8..16];
            let mut salt = [0u8; 8];
            salt[..4].copy_from_slice(&engine_boots.to_be_bytes());
            salt[4..8].copy_from_slice(&(salt_counter as u32).to_be_bytes());

            let mut iv = [0u8; 8];
            for i in 0..8 {
                iv[i] = pre_iv[i] ^ salt[i];
            }
            let mut buf = plain_scoped.to_vec();
            while buf.len() % 8 != 0 {
                buf.push(0);
            }
            let enc = DesCbcEnc::new_from_slices(key, &iv)
                .map_err(|e| ServerError::Snmp(format!("DES init error: {e}")))?;
            let msg_len = buf.len();
            enc.encrypt_padded_mut::<NoPadding>(&mut buf, msg_len)
                .map_err(|e| ServerError::Snmp(format!("DES encrypt error: {e}")))?;
            Ok((buf, salt.to_vec()))
        }
        PrivProto::Aes128 => {
            if priv_kul.len() < 16 {
                return Err(ServerError::Snmp("AES localized key must be >= 16 bytes".to_string()));
            }
            let key = &priv_kul[..16];
            let salt = salt_counter.to_be_bytes();
            let mut iv = [0u8; 16];
            iv[..4].copy_from_slice(&engine_boots.to_be_bytes());
            iv[4..8].copy_from_slice(&engine_time.to_be_bytes());
            iv[8..16].copy_from_slice(&salt);

            let mut buf = plain_scoped.to_vec();
            let enc = Aes128CfbEnc::new_from_slices(key, &iv)
                .map_err(|e| ServerError::Snmp(format!("AES init error: {e}")))?;
            enc.encrypt(&mut buf);
            Ok((buf, salt.to_vec()))
        }
    }
}

pub fn decrypt_scoped_pdu(
    priv_proto: PrivProto,
    priv_kul: &[u8],
    engine_boots: i32,
    engine_time: i32,
    priv_params: &[u8],
    encrypted: &[u8],
) -> Result<Vec<u8>, ServerError> {
    match priv_proto {
        PrivProto::None => Err(ServerError::Snmp("decrypt called with PrivProto::None".to_string())),
        PrivProto::Des => {
            if priv_kul.len() < 16 || priv_params.len() != 8 {
                return Err(ServerError::Snmp("invalid DES key or salt length".to_string()));
            }
            if encrypted.is_empty() || encrypted.len() % 8 != 0 {
                return Err(ServerError::Snmp(
                    "DES ciphertext length must be non-zero multiple of 8".to_string(),
                ));
            }
            let key = &priv_kul[..8];
            let pre_iv = &priv_kul[8..16];
            let mut iv = [0u8; 8];
            for i in 0..8 {
                iv[i] = pre_iv[i] ^ priv_params[i];
            }
            let mut buf = encrypted.to_vec();
            let dec = DesCbcDec::new_from_slices(key, &iv)
                .map_err(|e| ServerError::Snmp(format!("DES init error: {e}")))?;
            dec.decrypt_padded_mut::<NoPadding>(&mut buf)
                .map_err(|e| ServerError::Snmp(format!("DES decrypt error: {e}")))?;
            Ok(buf)
        }
        PrivProto::Aes128 => {
            if priv_kul.len() < 16 || priv_params.len() != 8 {
                return Err(ServerError::Snmp("invalid AES key or salt length".to_string()));
            }
            let key = &priv_kul[..16];
            let mut iv = [0u8; 16];
            iv[..4].copy_from_slice(&engine_boots.to_be_bytes());
            iv[4..8].copy_from_slice(&engine_time.to_be_bytes());
            iv[8..16].copy_from_slice(priv_params);

            let mut buf = encrypted.to_vec();
            let dec = Aes128CfbDec::new_from_slices(key, &iv)
                .map_err(|e| ServerError::Snmp(format!("AES init error: {e}")))?;
            dec.decrypt(&mut buf);
            Ok(buf)
        }
    }
}

pub fn encode_v3_message(
    msg_id: i32,
    reportable: bool,
    creds: &UsmCredentials,
    sec: &mut UsmSecurityParams,
    context_engine_id: &[u8],
    context_name: &[u8],
    pdu: &Pdu,
) -> Result<Vec<u8>, ServerError> {
    let auth_enabled = creds.auth_proto != AuthProto::None && !sec.engine_id.is_empty();
    let priv_enabled =
        auth_enabled && creds.priv_proto != PrivProto::None && !creds.priv_password.is_empty();

    let mut flags = 0u8;
    if auth_enabled {
        flags |= 0x01;
    }
    if priv_enabled {
        flags |= 0x02;
    }
    if reportable {
        flags |= 0x04;
    }

    let scoped_bytes = encode_scoped_pdu(context_engine_id, context_name, pdu)?;
    let (payload_bytes, priv_params) = if priv_enabled {
        let ku = password_to_key(creds.auth_proto, creds.priv_password.as_bytes());
        let kul = localize_key(creds.auth_proto, &ku, &sec.engine_id);
        sec.salt_counter = sec.salt_counter.wrapping_add(1);
        let (enc, salt) = encrypt_scoped_pdu(
            creds.priv_proto,
            &kul,
            sec.engine_boots,
            sec.engine_time,
            sec.salt_counter,
            &scoped_bytes,
        )?;
        let mut out = Vec::with_capacity(enc.len() + 4);
        encode_tlv(TAG_OCTET_STRING, &enc, &mut out);
        (out, salt)
    } else {
        (scoped_bytes, Vec::new())
    };

    let auth_placeholder = if auth_enabled { vec![0u8; 12] } else { Vec::new() };
    let usm = UsmHeader {
        engine_id: sec.engine_id.clone(),
        engine_boots: sec.engine_boots,
        engine_time: sec.engine_time,
        username: creds.username.as_bytes().to_vec(),
        auth_params: auth_placeholder,
        priv_params,
    };
    let (usm_seq, auth_offset_in_usm) = encode_usm_security_params(&usm);

    let mut global_body = Vec::with_capacity(24);
    encode_integer(i64::from(msg_id), &mut global_body);
    encode_integer(65507, &mut global_body);
    encode_tlv(TAG_OCTET_STRING, &[flags], &mut global_body);
    encode_integer(3, &mut global_body); // USM securityModel = 3

    let mut msg_body = Vec::with_capacity(128 + payload_bytes.len());
    encode_integer(3, &mut msg_body);
    encode_tlv(TAG_SEQUENCE, &global_body, &mut msg_body);

    // Encode msgSecurityParameters as OCTET STRING wrapping `usm_seq`.
    msg_body.push(TAG_OCTET_STRING);
    encode_length(usm_seq.len(), &mut msg_body);
    let usm_start_in_msg_body = msg_body.len();
    msg_body.extend_from_slice(&usm_seq);
    msg_body.extend_from_slice(&payload_bytes);

    let mut full_msg = Vec::with_capacity(msg_body.len() + 4);
    full_msg.push(TAG_SEQUENCE);
    encode_length(msg_body.len(), &mut full_msg);
    let outer_hdr_len = full_msg.len();
    full_msg.extend_from_slice(&msg_body);

    if auth_enabled {
        let auth_offset = outer_hdr_len + usm_start_in_msg_body + auth_offset_in_usm;
        let ku = password_to_key(creds.auth_proto, creds.auth_password.as_bytes());
        let kul = localize_key(creds.auth_proto, &ku, &sec.engine_id);
        let mac = compute_hmac_96(creds.auth_proto, &kul, &full_msg)?;
        full_msg[auth_offset..auth_offset + 12].copy_from_slice(&mac);
    }

    Ok(full_msg)
}

pub fn decode_and_verify_v3(
    raw: &[u8],
    msg: SnmpMessage,
    creds: Option<&UsmCredentials>,
) -> Result<(UsmHeader, Pdu), ServerError> {
    let SnmpMessage::V3 {
        flags,
        usm,
        payload,
        auth_param_offset,
        ..
    } = msg
    else {
        return Err(ServerError::Snmp("expected SNMPv3 message".to_string()));
    };

    let auth_flag = (flags & 0x01) != 0;
    let priv_flag = (flags & 0x02) != 0;

    if auth_flag {
        let Some(creds) = creds else {
            return Err(ServerError::Snmp("authenticated v3 message but no credentials".to_string()));
        };
        if creds.auth_proto == AuthProto::None {
            return Err(ServerError::Snmp("credentials lack auth protocol".to_string()));
        }
        let offset = auth_param_offset
            .ok_or_else(|| ServerError::Snmp("missing auth_params in v3 packet".to_string()))?;
        if offset + 12 > raw.len() {
            return Err(ServerError::Snmp("auth_params offset out of bounds".to_string()));
        }
        let mut copy = raw.to_vec();
        copy[offset..offset + 12].fill(0);
        let ku = password_to_key(creds.auth_proto, creds.auth_password.as_bytes());
        let kul = localize_key(creds.auth_proto, &ku, &usm.engine_id);
        let expected = compute_hmac_96(creds.auth_proto, &kul, &copy)?;
        if !ct_eq_12(&usm.auth_params, &expected) {
            return Err(ServerError::Snmp("SNMPv3 HMAC verification failed".to_string()));
        }
    }

    let pdu = match payload {
        ScopedPduPayload::Plain { pdu, .. } => {
            if priv_flag {
                return Err(ServerError::Snmp("v3 priv flag set but payload unencrypted".to_string()));
            }
            pdu
        }
        ScopedPduPayload::Encrypted(enc) => {
            if !auth_flag {
                return Err(ServerError::Snmp("v3 privacy requires authentication".to_string()));
            }
            let Some(creds) = creds else {
                return Err(ServerError::Snmp("encrypted v3 message without credentials".to_string()));
            };
            let ku = password_to_key(creds.auth_proto, creds.priv_password.as_bytes());
            let kul = localize_key(creds.auth_proto, &ku, &usm.engine_id);
            let plain = decrypt_scoped_pdu(
                creds.priv_proto,
                &kul,
                usm.engine_boots,
                usm.engine_time,
                &usm.priv_params,
                &enc,
            )?;
            let (_, _, pdu) = decode_scoped_pdu(&plain)?;
            pdu
        }
    };

    Ok((usm, pdu))
}
