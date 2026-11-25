use crate::error::ServerError;
use std::fmt;

pub const TAG_INTEGER: u8 = 0x02;
pub const TAG_OCTET_STRING: u8 = 0x04;
pub const TAG_NULL: u8 = 0x05;
pub const TAG_OID: u8 = 0x06;
pub const TAG_SEQUENCE: u8 = 0x30;
pub const TAG_IP_ADDRESS: u8 = 0x40;
pub const TAG_COUNTER32: u8 = 0x41;
pub const TAG_GAUGE32: u8 = 0x42;
pub const TAG_TIMETICKS: u8 = 0x43;
pub const TAG_OPAQUE: u8 = 0x44;
pub const TAG_COUNTER64: u8 = 0x46;

pub const TAG_NO_SUCH_OBJECT: u8 = 0x80;
pub const TAG_NO_SUCH_INSTANCE: u8 = 0x81;
pub const TAG_END_OF_MIB_VIEW: u8 = 0x82;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PduType {
    GetRequest = 0xA0,
    GetNextRequest = 0xA1,
    Response = 0xA2,
    SetRequest = 0xA3,
    TrapV1 = 0xA4,
    GetBulkRequest = 0xA5,
    InformRequest = 0xA6,
    TrapV2 = 0xA7,
    Report = 0xA8,
}

impl PduType {
    pub fn from_tag(tag: u8) -> Result<Self, ServerError> {
        match tag {
            0xA0 => Ok(Self::GetRequest),
            0xA1 => Ok(Self::GetNextRequest),
            0xA2 => Ok(Self::Response),
            0xA3 => Ok(Self::SetRequest),
            0xA4 => Ok(Self::TrapV1),
            0xA5 => Ok(Self::GetBulkRequest),
            0xA6 => Ok(Self::InformRequest),
            0xA7 => Ok(Self::TrapV2),
            0xA8 => Ok(Self::Report),
            _ => Err(ServerError::Snmp(format!("unsupported PDU tag 0x{tag:02x}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnmpValue {
    Integer(i64),
    OctetString(Vec<u8>),
    Null,
    Oid(Vec<u32>),
    IpAddress([u8; 4]),
    Counter32(u32),
    Gauge32(u32),
    TimeTicks(u32),
    Opaque(Vec<u8>),
    Counter64(u64),
    NoSuchObject,
    NoSuchInstance,
    EndOfMibView,
}

impl SnmpValue {
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(v) => Some(*v),
            Self::Counter32(v) | Self::Gauge32(v) | Self::TimeTicks(v) => Some(i64::from(*v)),
            Self::Counter64(v) => i64::try_from(*v).ok(),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Integer(v) => Some(*v as f64),
            Self::Counter32(v) | Self::Gauge32(v) | Self::TimeTicks(v) => Some(f64::from(*v)),
            Self::Counter64(v) => Some(*v as f64),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Integer(v) if *v >= 0 => Some(*v as u64),
            Self::Counter32(v) | Self::Gauge32(v) | Self::TimeTicks(v) => Some(u64::from(*v)),
            Self::Counter64(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_string_lossy(&self) -> String {
        match self {
            Self::Integer(v) => v.to_string(),
            Self::OctetString(bytes) => {
                if bytes.iter().all(|b| b.is_ascii_graphic() || *b == b' ' || *b == b'\t') {
                    String::from_utf8_lossy(bytes).into_owned()
                } else if let Ok(s) = std::str::from_utf8(bytes) {
                    if !s.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
                        s.to_string()
                    } else {
                        hex::encode(bytes)
                    }
                } else {
                    hex::encode(bytes)
                }
            }
            Self::Null => "NULL".to_string(),
            Self::Oid(arcs) => format_oid(arcs),
            Self::IpAddress(ip) => format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]),
            Self::Counter32(v) | Self::Gauge32(v) | Self::TimeTicks(v) => v.to_string(),
            Self::Opaque(b) => hex::encode(b),
            Self::Counter64(v) => v.to_string(),
            Self::NoSuchObject => "noSuchObject".to_string(),
            Self::NoSuchInstance => "noSuchInstance".to_string(),
            Self::EndOfMibView => "endOfMibView".to_string(),
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Integer(_) => "Integer",
            Self::OctetString(_) => "OctetString",
            Self::Null => "Null",
            Self::Oid(_) => "OID",
            Self::IpAddress(_) => "IpAddress",
            Self::Counter32(_) => "Counter32",
            Self::Gauge32(_) => "Gauge32",
            Self::TimeTicks(_) => "TimeTicks",
            Self::Opaque(_) => "Opaque",
            Self::Counter64(_) => "Counter64",
            Self::NoSuchObject => "NoSuchObject",
            Self::NoSuchInstance => "NoSuchInstance",
            Self::EndOfMibView => "EndOfMibView",
        }
    }
}

impl fmt::Display for SnmpValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_string_lossy())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Varbind {
    pub oid: Vec<u32>,
    pub value: SnmpValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pdu {
    pub pdu_type: PduType,
    pub request_id: i32,
    /// For GetBulkRequest this carries `non_repeaters`.
    pub error_status: i32,
    /// For GetBulkRequest this carries `max_repetitions`.
    pub error_index: i32,
    pub varbinds: Vec<Varbind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrapV1Pdu {
    pub enterprise: Vec<u32>,
    pub agent_addr: [u8; 4],
    pub generic_trap: i32,
    pub specific_trap: i32,
    pub time_stamp: u32,
    pub varbinds: Vec<Varbind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum V1OrV2Pdu {
    Standard(Pdu),
    TrapV1(TrapV1Pdu),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsmHeader {
    pub engine_id: Vec<u8>,
    pub engine_boots: i32,
    pub engine_time: i32,
    pub username: Vec<u8>,
    pub auth_params: Vec<u8>,
    pub priv_params: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopedPduPayload {
    Plain {
        context_engine_id: Vec<u8>,
        context_name: Vec<u8>,
        pdu: Pdu,
    },
    Encrypted(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnmpMessage {
    V1 {
        community: Vec<u8>,
        pdu: V1OrV2Pdu,
    },
    V2c {
        community: Vec<u8>,
        pdu: Pdu,
    },
    V3 {
        msg_id: i32,
        max_size: i32,
        flags: u8,
        security_model: i32,
        usm: UsmHeader,
        payload: ScopedPduPayload,
        /// Byte offset of the 12-byte auth digest inside the raw packet when decoded.
        auth_param_offset: Option<usize>,
    },
}

pub fn format_oid(arcs: &[u32]) -> String {
    arcs.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

pub fn parse_oid(oid: &str) -> Result<Vec<u32>, ServerError> {
    nextdude_common::validation::validate_oid(oid).map_err(ServerError::from)
}

pub fn encode_length(len: usize, out: &mut Vec<u8>) {
    if len < 128 {
        out.push(len as u8);
    } else {
        let bytes = len.to_be_bytes();
        let first_nonzero = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len() - 1);
        let slice = &bytes[first_nonzero..];
        out.push(0x80 | (slice.len() as u8));
        out.extend_from_slice(slice);
    }
}

pub fn encode_tlv(tag: u8, content: &[u8], out: &mut Vec<u8>) {
    out.push(tag);
    encode_length(content.len(), out);
    out.extend_from_slice(content);
}

pub fn encode_integer(val: i64, out: &mut Vec<u8>) {
    let bytes = val.to_be_bytes();
    let mut start = 0;
    if val >= 0 {
        while start < 7 && bytes[start] == 0 && (bytes[start + 1] & 0x80) == 0 {
            start += 1;
        }
    } else {
        while start < 7 && bytes[start] == 0xFF && (bytes[start + 1] & 0x80) != 0 {
            start += 1;
        }
    }
    encode_tlv(TAG_INTEGER, &bytes[start..], out);
}

pub fn encode_unsigned(tag: u8, val: u64, out: &mut Vec<u8>) {
    let bytes = val.to_be_bytes();
    let first_nonzero = bytes.iter().position(|&b| b != 0).unwrap_or(7);
    let slice = &bytes[first_nonzero..];
    let mut buf = Vec::with_capacity(slice.len() + 1);
    if slice.is_empty() || (slice[0] & 0x80) != 0 {
        buf.push(0);
    }
    buf.extend_from_slice(slice);
    encode_tlv(tag, &buf, out);
}

pub fn encode_oid(arcs: &[u32], out: &mut Vec<u8>) -> Result<(), ServerError> {
    if arcs.len() < 2 {
        return Err(ServerError::Snmp("OID must have at least 2 arcs".to_string()));
    }
    let mut content = Vec::with_capacity(arcs.len() * 2);
    let first = u64::from(arcs[0]) * 40 + u64::from(arcs[1]);
    encode_vlq(first, &mut content);
    for &arc in &arcs[2..] {
        encode_vlq(u64::from(arc), &mut content);
    }
    encode_tlv(TAG_OID, &content, out);
    Ok(())
}

fn encode_vlq(mut val: u64, out: &mut Vec<u8>) {
    let mut buf = [0u8; 10];
    let mut idx = buf.len();
    loop {
        idx -= 1;
        buf[idx] = (val & 0x7F) as u8;
        val >>= 7;
        if val == 0 {
            break;
        }
    }
    for byte in &mut buf[idx..buf.len() - 1] {
        *byte |= 0x80;
    }
    out.extend_from_slice(&buf[idx..]);
}

pub fn encode_value(val: &SnmpValue, out: &mut Vec<u8>) -> Result<(), ServerError> {
    match val {
        SnmpValue::Integer(v) => encode_integer(*v, out),
        SnmpValue::OctetString(b) => encode_tlv(TAG_OCTET_STRING, b, out),
        SnmpValue::Null => encode_tlv(TAG_NULL, &[], out),
        SnmpValue::Oid(arcs) => encode_oid(arcs, out)?,
        SnmpValue::IpAddress(ip) => encode_tlv(TAG_IP_ADDRESS, ip, out),
        SnmpValue::Counter32(v) => encode_unsigned(TAG_COUNTER32, u64::from(*v), out),
        SnmpValue::Gauge32(v) => encode_unsigned(TAG_GAUGE32, u64::from(*v), out),
        SnmpValue::TimeTicks(v) => encode_unsigned(TAG_TIMETICKS, u64::from(*v), out),
        SnmpValue::Opaque(b) => encode_tlv(TAG_OPAQUE, b, out),
        SnmpValue::Counter64(v) => encode_unsigned(TAG_COUNTER64, *v, out),
        SnmpValue::NoSuchObject => encode_tlv(TAG_NO_SUCH_OBJECT, &[], out),
        SnmpValue::NoSuchInstance => encode_tlv(TAG_NO_SUCH_INSTANCE, &[], out),
        SnmpValue::EndOfMibView => encode_tlv(TAG_END_OF_MIB_VIEW, &[], out),
    }
    Ok(())
}

pub fn encode_pdu(pdu: &Pdu, out: &mut Vec<u8>) -> Result<(), ServerError> {
    let mut body = Vec::with_capacity(64);
    encode_integer(i64::from(pdu.request_id), &mut body);
    encode_integer(i64::from(pdu.error_status), &mut body);
    encode_integer(i64::from(pdu.error_index), &mut body);

    let mut vbl = Vec::with_capacity(pdu.varbinds.len() * 24);
    for vb in &pdu.varbinds {
        let mut pair = Vec::with_capacity(24);
        encode_oid(&vb.oid, &mut pair)?;
        encode_value(&vb.value, &mut pair)?;
        encode_tlv(TAG_SEQUENCE, &pair, &mut vbl);
    }
    encode_tlv(TAG_SEQUENCE, &vbl, &mut body);
    encode_tlv(pdu.pdu_type as u8, &body, out);
    Ok(())
}

pub fn encode_scoped_pdu(
    context_engine_id: &[u8],
    context_name: &[u8],
    pdu: &Pdu,
) -> Result<Vec<u8>, ServerError> {
    let mut body = Vec::with_capacity(96);
    encode_tlv(TAG_OCTET_STRING, context_engine_id, &mut body);
    encode_tlv(TAG_OCTET_STRING, context_name, &mut body);
    encode_pdu(pdu, &mut body)?;
    let mut out = Vec::with_capacity(body.len() + 4);
    encode_tlv(TAG_SEQUENCE, &body, &mut out);
    Ok(out)
}

pub fn encode_v1_or_v2_message(msg: &SnmpMessage) -> Result<Vec<u8>, ServerError> {
    match msg {
        SnmpMessage::V1 { community, pdu } => {
            let mut body = Vec::with_capacity(96);
            encode_integer(0, &mut body);
            encode_tlv(TAG_OCTET_STRING, community, &mut body);
            match pdu {
                V1OrV2Pdu::Standard(p) => encode_pdu(p, &mut body)?,
                V1OrV2Pdu::TrapV1(t) => encode_trap_v1(t, &mut body)?,
            }
            let mut out = Vec::with_capacity(body.len() + 4);
            encode_tlv(TAG_SEQUENCE, &body, &mut out);
            Ok(out)
        }
        SnmpMessage::V2c { community, pdu } => {
            let mut body = Vec::with_capacity(96);
            encode_integer(1, &mut body);
            encode_tlv(TAG_OCTET_STRING, community, &mut body);
            encode_pdu(pdu, &mut body)?;
            let mut out = Vec::with_capacity(body.len() + 4);
            encode_tlv(TAG_SEQUENCE, &body, &mut out);
            Ok(out)
        }
        SnmpMessage::V3 { .. } => Err(ServerError::Snmp(
            "use usm::encode_v3_message for SNMPv3 messages".to_string(),
        )),
    }
}

fn encode_trap_v1(t: &TrapV1Pdu, out: &mut Vec<u8>) -> Result<(), ServerError> {
    let mut body = Vec::with_capacity(96);
    encode_oid(&t.enterprise, &mut body)?;
    encode_tlv(TAG_IP_ADDRESS, &t.agent_addr, &mut body);
    encode_integer(i64::from(t.generic_trap), &mut body);
    encode_integer(i64::from(t.specific_trap), &mut body);
    encode_unsigned(TAG_TIMETICKS, u64::from(t.time_stamp), &mut body);
    let mut vbl = Vec::new();
    for vb in &t.varbinds {
        let mut pair = Vec::new();
        encode_oid(&vb.oid, &mut pair)?;
        encode_value(&vb.value, &mut pair)?;
        encode_tlv(TAG_SEQUENCE, &pair, &mut vbl);
    }
    encode_tlv(TAG_SEQUENCE, &vbl, &mut body);
    encode_tlv(PduType::TrapV1 as u8, &body, out);
    Ok(())
}

pub fn encode_usm_security_params(usm: &UsmHeader) -> (Vec<u8>, usize) {
    let mut body = Vec::with_capacity(64);
    encode_tlv(TAG_OCTET_STRING, &usm.engine_id, &mut body);
    encode_integer(i64::from(usm.engine_boots), &mut body);
    encode_integer(i64::from(usm.engine_time), &mut body);
    encode_tlv(TAG_OCTET_STRING, &usm.username, &mut body);

    // Record offset of the auth_params content inside `body`.
    body.push(TAG_OCTET_STRING);
    encode_length(usm.auth_params.len(), &mut body);
    let auth_content_offset_in_body = body.len();
    body.extend_from_slice(&usm.auth_params);

    encode_tlv(TAG_OCTET_STRING, &usm.priv_params, &mut body);

    let mut usm_seq = Vec::with_capacity(body.len() + 4);
    usm_seq.push(TAG_SEQUENCE);
    encode_length(body.len(), &mut usm_seq);
    let header_len = usm_seq.len();
    usm_seq.extend_from_slice(&body);
    (usm_seq, header_len + auth_content_offset_in_body)
}

pub fn decode_message(data: &[u8]) -> Result<SnmpMessage, ServerError> {
    let mut reader = BerReader::new(data);
    let (tag, seq_slice, _) = reader.read_tlv()?;
    if tag != TAG_SEQUENCE {
        return Err(ServerError::Snmp(format!(
            "expected outer SEQUENCE 0x30, got 0x{tag:02x}"
        )));
    }
    let mut inner = BerReader::new(seq_slice);
    let version = inner.read_integer()?;
    match version {
        0 => {
            let community = inner.read_octet_string()?.to_vec();
            let (pdu_tag, pdu_slice, _) = inner.read_tlv()?;
            let pdu_type = PduType::from_tag(pdu_tag)?;
            let pdu = if pdu_type == PduType::TrapV1 {
                V1OrV2Pdu::TrapV1(decode_trap_v1(pdu_slice)?)
            } else {
                V1OrV2Pdu::Standard(decode_pdu_body(pdu_type, pdu_slice)?)
            };
            Ok(SnmpMessage::V1 { community, pdu })
        }
        1 => {
            let community = inner.read_octet_string()?.to_vec();
            let (pdu_tag, pdu_slice, _) = inner.read_tlv()?;
            let pdu_type = PduType::from_tag(pdu_tag)?;
            let pdu = decode_pdu_body(pdu_type, pdu_slice)?;
            Ok(SnmpMessage::V2c { community, pdu })
        }
        3 => {
            let base_ptr = data.as_ptr() as usize;
            let global_slice = inner.read_sequence()?;
            let mut global = BerReader::new(global_slice);
            let msg_id = global.read_integer()? as i32;
            let max_size = global.read_integer()? as i32;
            let flags_bytes = global.read_octet_string()?;
            if flags_bytes.len() != 1 {
                return Err(ServerError::Snmp("invalid msgFlags length".to_string()));
            }
            let flags = flags_bytes[0];
            let security_model = global.read_integer()? as i32;

            let sec_params_bytes = inner.read_octet_string()?;
            let sec_params_abs_start = sec_params_bytes.as_ptr() as usize - base_ptr;
            let mut sec_seq_reader = BerReader::new(sec_params_bytes);
            let usm_seq = sec_seq_reader.read_sequence()?;
            let usm_seq_abs_start = usm_seq.as_ptr() as usize - base_ptr;
            let _ = sec_params_abs_start;

            let mut usm_reader = BerReader::new(usm_seq);
            let engine_id = usm_reader.read_octet_string()?.to_vec();
            let engine_boots = usm_reader.read_integer()? as i32;
            let engine_time = usm_reader.read_integer()? as i32;
            let username = usm_reader.read_octet_string()?.to_vec();
            let (auth_tag, auth_slice, _) = usm_reader.read_tlv()?;
            if auth_tag != TAG_OCTET_STRING {
                return Err(ServerError::Snmp("expected auth_params OCTET STRING".to_string()));
            }
            let auth_param_offset = if auth_slice.is_empty() {
                None
            } else {
                let slice_offset_in_usm = auth_slice.as_ptr() as usize - (usm_seq.as_ptr() as usize);
                Some(usm_seq_abs_start + slice_offset_in_usm)
            };
            let auth_params = auth_slice.to_vec();
            let priv_params = usm_reader.read_octet_string()?.to_vec();

            let payload = if (flags & 0x02) != 0 {
                let enc = inner.read_octet_string()?.to_vec();
                ScopedPduPayload::Encrypted(enc)
            } else {
                let scoped_slice = inner.read_sequence()?;
                let (context_engine_id, context_name, pdu) = decode_scoped_pdu_body(scoped_slice)?;
                ScopedPduPayload::Plain {
                    context_engine_id,
                    context_name,
                    pdu,
                }
            };

            Ok(SnmpMessage::V3 {
                msg_id,
                max_size,
                flags,
                security_model,
                usm: UsmHeader {
                    engine_id,
                    engine_boots,
                    engine_time,
                    username,
                    auth_params,
                    priv_params,
                },
                payload,
                auth_param_offset,
            })
        }
        other => Err(ServerError::Snmp(format!(
            "unsupported SNMP version {other}"
        ))),
    }
}

pub fn decode_scoped_pdu(data: &[u8]) -> Result<(Vec<u8>, Vec<u8>, Pdu), ServerError> {
    let mut r = BerReader::new(data);
    let seq = r.read_sequence()?;
    decode_scoped_pdu_body(seq)
}

fn decode_scoped_pdu_body(seq: &[u8]) -> Result<(Vec<u8>, Vec<u8>, Pdu), ServerError> {
    let mut r = BerReader::new(seq);
    let context_engine_id = r.read_octet_string()?.to_vec();
    let context_name = r.read_octet_string()?.to_vec();
    let (pdu_tag, pdu_slice, _) = r.read_tlv()?;
    let pdu_type = PduType::from_tag(pdu_tag)?;
    let pdu = decode_pdu_body(pdu_type, pdu_slice)?;
    Ok((context_engine_id, context_name, pdu))
}

fn decode_pdu_body(pdu_type: PduType, slice: &[u8]) -> Result<Pdu, ServerError> {
    let mut r = BerReader::new(slice);
    let request_id = r.read_integer()? as i32;
    let error_status = r.read_integer()? as i32;
    let error_index = r.read_integer()? as i32;
    let vbl_slice = r.read_sequence()?;
    let varbinds = decode_varbind_list(vbl_slice)?;
    Ok(Pdu {
        pdu_type,
        request_id,
        error_status,
        error_index,
        varbinds,
    })
}

fn decode_trap_v1(slice: &[u8]) -> Result<TrapV1Pdu, ServerError> {
    let mut r = BerReader::new(slice);
    let enterprise = r.read_oid()?;
    let (ip_tag, ip_slice, _) = r.read_tlv()?;
    if ip_tag != TAG_IP_ADDRESS || ip_slice.len() != 4 {
        return Err(ServerError::Snmp("invalid TrapV1 agent_addr".to_string()));
    }
    let agent_addr = [ip_slice[0], ip_slice[1], ip_slice[2], ip_slice[3]];
    let generic_trap = r.read_integer()? as i32;
    let specific_trap = r.read_integer()? as i32;
    let (ts_tag, ts_slice, _) = r.read_tlv()?;
    if ts_tag != TAG_TIMETICKS {
        return Err(ServerError::Snmp("expected TimeTicks in TrapV1".to_string()));
    }
    let time_stamp = decode_unsigned_slice(ts_slice)? as u32;
    let vbl_slice = r.read_sequence()?;
    let varbinds = decode_varbind_list(vbl_slice)?;
    Ok(TrapV1Pdu {
        enterprise,
        agent_addr,
        generic_trap,
        specific_trap,
        time_stamp,
        varbinds,
    })
}

fn decode_varbind_list(slice: &[u8]) -> Result<Vec<Varbind>, ServerError> {
    let mut r = BerReader::new(slice);
    let mut out = Vec::new();
    while !r.is_empty() {
        let pair_slice = r.read_sequence()?;
        let mut pair = BerReader::new(pair_slice);
        let oid = pair.read_oid()?;
        let value = pair.read_value()?;
        out.push(Varbind { oid, value });
    }
    Ok(out)
}

fn decode_signed_slice(bytes: &[u8]) -> Result<i64, ServerError> {
    if bytes.is_empty() || bytes.len() > 8 {
        return Err(ServerError::Snmp(format!(
            "invalid BER integer length {}",
            bytes.len()
        )));
    }
    let sign_fill: u8 = if (bytes[0] & 0x80) != 0 { 0xFF } else { 0x00 };
    let mut buf = [sign_fill; 8];
    let offset = 8 - bytes.len();
    buf[offset..].copy_from_slice(bytes);
    Ok(i64::from_be_bytes(buf))
}

fn decode_unsigned_slice(bytes: &[u8]) -> Result<u64, ServerError> {
    if bytes.is_empty() {
        return Err(ServerError::Snmp("empty unsigned BER integer".to_string()));
    }
    let trimmed = if bytes.len() > 1 && bytes[0] == 0 {
        &bytes[1..]
    } else {
        bytes
    };
    if trimmed.len() > 8 {
        return Err(ServerError::Snmp(format!(
            "BER unsigned integer exceeds 64 bits ({} bytes)",
            trimmed.len()
        )));
    }
    let mut buf = [0u8; 8];
    let offset = 8 - trimmed.len();
    buf[offset..].copy_from_slice(trimmed);
    Ok(u64::from_be_bytes(buf))
}

fn decode_oid_slice(bytes: &[u8]) -> Result<Vec<u32>, ServerError> {
    if bytes.is_empty() {
        return Err(ServerError::Snmp("empty OID".to_string()));
    }
    let mut idx = 0usize;
    let first = read_vlq(bytes, &mut idx)?;
    let mut arcs = Vec::with_capacity(bytes.len() + 1);
    if first < 80 {
        arcs.push((first / 40) as u32);
        arcs.push((first % 40) as u32);
    } else {
        arcs.push(2);
        arcs.push(
            u32::try_from(first - 80)
                .map_err(|_| ServerError::Snmp("OID second arc overflow".to_string()))?,
        );
    }
    while idx < bytes.len() {
        let sub = read_vlq(bytes, &mut idx)?;
        let arc =
            u32::try_from(sub).map_err(|_| ServerError::Snmp("OID arc overflow".to_string()))?;
        arcs.push(arc);
    }
    Ok(arcs)
}

fn read_vlq(bytes: &[u8], idx: &mut usize) -> Result<u64, ServerError> {
    let mut val = 0u64;
    for _ in 0..10 {
        if *idx >= bytes.len() {
            return Err(ServerError::Snmp("truncated OID VLQ".to_string()));
        }
        let b = bytes[*idx];
        *idx += 1;
        val = (val << 7) | u64::from(b & 0x7F);
        if (b & 0x80) == 0 {
            return Ok(val);
        }
    }
    Err(ServerError::Snmp("OID VLQ exceeds 10 bytes".to_string()))
}

pub struct BerReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BerReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.pos >= self.data.len()
    }

    pub fn read_tlv(&mut self) -> Result<(u8, &'a [u8], usize), ServerError> {
        if self.pos >= self.data.len() {
            return Err(ServerError::Snmp("unexpected end of BER stream".to_string()));
        }
        let tlv_start = self.pos;
        let tag = self.data[self.pos];
        self.pos += 1;
        if self.pos >= self.data.len() {
            return Err(ServerError::Snmp("missing BER length".to_string()));
        }
        let len_byte = self.data[self.pos];
        self.pos += 1;
        let len = if (len_byte & 0x80) == 0 {
            usize::from(len_byte)
        } else {
            let count = usize::from(len_byte & 0x7F);
            if count == 0 || count > std::mem::size_of::<usize>() {
                return Err(ServerError::Snmp(
                    "invalid or indefinite BER length".to_string(),
                ));
            }
            if self.pos + count > self.data.len() {
                return Err(ServerError::Snmp("truncated BER length bytes".to_string()));
            }
            let mut val = 0usize;
            for &b in &self.data[self.pos..self.pos + count] {
                val = (val << 8) | usize::from(b);
            }
            self.pos += count;
            val
        };
        if self.pos.saturating_add(len) > self.data.len() {
            return Err(ServerError::Snmp("BER value exceeds packet bounds".to_string()));
        }
        let slice = &self.data[self.pos..self.pos + len];
        self.pos += len;
        Ok((tag, slice, tlv_start))
    }

    pub fn read_integer(&mut self) -> Result<i64, ServerError> {
        let (tag, slice, _) = self.read_tlv()?;
        if tag != TAG_INTEGER {
            return Err(ServerError::Snmp(format!(
                "expected INTEGER 0x02, got 0x{tag:02x}"
            )));
        }
        decode_signed_slice(slice)
    }

    pub fn read_octet_string(&mut self) -> Result<&'a [u8], ServerError> {
        let (tag, slice, _) = self.read_tlv()?;
        if tag != TAG_OCTET_STRING {
            return Err(ServerError::Snmp(format!(
                "expected OCTET STRING 0x04, got 0x{tag:02x}"
            )));
        }
        Ok(slice)
    }

    pub fn read_sequence(&mut self) -> Result<&'a [u8], ServerError> {
        let (tag, slice, _) = self.read_tlv()?;
        if tag != TAG_SEQUENCE {
            return Err(ServerError::Snmp(format!(
                "expected SEQUENCE 0x30, got 0x{tag:02x}"
            )));
        }
        Ok(slice)
    }

    pub fn read_oid(&mut self) -> Result<Vec<u32>, ServerError> {
        let (tag, slice, _) = self.read_tlv()?;
        if tag != TAG_OID {
            return Err(ServerError::Snmp(format!(
                "expected OID 0x06, got 0x{tag:02x}"
            )));
        }
        decode_oid_slice(slice)
    }

    pub fn read_value(&mut self) -> Result<SnmpValue, ServerError> {
        let (tag, slice, _) = self.read_tlv()?;
        match tag {
            TAG_INTEGER => Ok(SnmpValue::Integer(decode_signed_slice(slice)?)),
            TAG_OCTET_STRING => Ok(SnmpValue::OctetString(slice.to_vec())),
            TAG_NULL => Ok(SnmpValue::Null),
            TAG_OID => Ok(SnmpValue::Oid(decode_oid_slice(slice)?)),
            TAG_IP_ADDRESS => {
                if slice.len() != 4 {
                    return Err(ServerError::Snmp("invalid IpAddress length".to_string()));
                }
                Ok(SnmpValue::IpAddress([slice[0], slice[1], slice[2], slice[3]]))
            }
            TAG_COUNTER32 => Ok(SnmpValue::Counter32(decode_unsigned_slice(slice)? as u32)),
            TAG_GAUGE32 => Ok(SnmpValue::Gauge32(decode_unsigned_slice(slice)? as u32)),
            TAG_TIMETICKS => Ok(SnmpValue::TimeTicks(decode_unsigned_slice(slice)? as u32)),
            TAG_OPAQUE => Ok(SnmpValue::Opaque(slice.to_vec())),
            TAG_COUNTER64 => Ok(SnmpValue::Counter64(decode_unsigned_slice(slice)?)),
            TAG_NO_SUCH_OBJECT => Ok(SnmpValue::NoSuchObject),
            TAG_NO_SUCH_INSTANCE => Ok(SnmpValue::NoSuchInstance),
            TAG_END_OF_MIB_VIEW => Ok(SnmpValue::EndOfMibView),
            _ => Err(ServerError::Snmp(format!(
                "unsupported SNMP value tag 0x{tag:02x}"
            ))),
        }
    }
}
