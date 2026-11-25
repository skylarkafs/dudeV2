use crate::error::ServerError;
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use rand::rngs::OsRng;
use rand::RngCore;
use rcgen::{CertificateParams, DnType, KeyPair, SanType};
use sha2::{Digest, Sha256};
use std::fs;
use std::net::IpAddr;
use std::path::Path;
use zeroize::Zeroizing;

#[derive(Clone)]
pub struct SecretBox {
    key: Zeroizing<[u8; 32]>,
}

impl SecretBox {
    pub fn load_or_create(path: &Path) -> Result<Self, ServerError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if path.exists() {
            let raw = fs::read(path)?;
            if raw.len() != 32 {
                return Err(ServerError::Crypto(format!(
                    "master key {} must be 32 bytes, found {}",
                    path.display(),
                    raw.len()
                )));
            }
            let mut key = Zeroizing::new([0u8; 32]);
            key.copy_from_slice(&raw);
            ensure_strict_file_mode(path)?;
            Ok(Self { key })
        } else {
            let mut key = Zeroizing::new([0u8; 32]);
            OsRng.fill_bytes(key.as_mut());
            write_strict_file(path, key.as_ref())?;
            Ok(Self { key })
        }
    }

    pub fn from_raw_key(raw: [u8; 32]) -> Self {
        Self {
            key: Zeroizing::new(raw),
        }
    }

    pub fn encrypt(&self, plaintext: &str) -> Result<String, ServerError> {
        if plaintext.is_empty() {
            return Ok(String::new());
        }
        let cipher = ChaCha20Poly1305::new(self.key.as_ref().into());
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| ServerError::Crypto(format!("AEAD encrypt failed: {e}")))?;

        let mut combined = Vec::with_capacity(12 + ciphertext.len());
        combined.extend_from_slice(&nonce_bytes);
        combined.extend_from_slice(&ciphertext);
        Ok(format!("v1:{}", B64.encode(&combined)))
    }

    pub fn decrypt(&self, encoded: &str) -> Result<String, ServerError> {
        if encoded.is_empty() {
            return Ok(String::new());
        }
        let payload = encoded
            .strip_prefix("v1:")
            .ok_or_else(|| ServerError::Crypto("unsupported secret cipher version".to_string()))?;
        let raw = B64
            .decode(payload)
            .map_err(|e| ServerError::Crypto(format!("invalid base64 secret: {e}")))?;
        if raw.len() < 12 + 16 {
            return Err(ServerError::Crypto("truncated ciphertext".to_string()));
        }
        let (nonce_bytes, ciphertext) = raw.split_at(12);
        let cipher = ChaCha20Poly1305::new(self.key.as_ref().into());
        let plain = cipher
            .decrypt(Nonce::from_slice(nonce_bytes), ciphertext)
            .map_err(|e| ServerError::Crypto(format!("AEAD decrypt failed: {e}")))?;
        String::from_utf8(plain)
            .map_err(|e| ServerError::Crypto(format!("decrypted secret not UTF-8: {e}")))
    }
}

pub fn hash_password(password: &str) -> Result<String, ServerError> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| ServerError::Crypto(format!("argon2id hash failed: {e}")))?;
    Ok(hash.to_string())
}

pub fn verify_password(password: &str, phc_hash: &str) -> Result<bool, ServerError> {
    let parsed = PasswordHash::new(phc_hash)
        .map_err(|e| ServerError::Crypto(format!("invalid password hash format: {e}")))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

pub fn generate_session_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub struct TlsMaterial {
    pub cert_pem: String,
    pub key_pem: String,
    pub sha256_fingerprint: String,
}

pub fn load_or_create_tls_material(
    cert_path: &Path,
    key_path: &Path,
    extra_san_addrs: &[String],
) -> Result<TlsMaterial, ServerError> {
    if cert_path.exists() && key_path.exists() {
        let cert_pem = fs::read_to_string(cert_path)?;
        let key_pem = fs::read_to_string(key_path)?;
        let fp = compute_pem_cert_fingerprint(&cert_pem)?;
        return Ok(TlsMaterial {
            cert_pem,
            key_pem,
            sha256_fingerprint: fp,
        });
    }

    if let Some(p) = cert_path.parent() {
        fs::create_dir_all(p)?;
    }
    if let Some(p) = key_path.parent() {
        fs::create_dir_all(p)?;
    }

    let mut params = CertificateParams::new(vec!["localhost".to_string()])
        .map_err(|e| ServerError::Crypto(format!("cert params failed: {e}")))?;
    params
        .distinguished_name
        .push(DnType::CommonName, nextdude_common::APP_NAME);
    params
        .subject_alt_names
        .push(SanType::IpAddress(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)));
    params
        .subject_alt_names
        .push(SanType::IpAddress(IpAddr::V6(std::net::Ipv6Addr::LOCALHOST)));
    for s in extra_san_addrs {
        if let Ok(ip) = s.parse::<IpAddr>() {
            if !ip.is_unspecified() {
                params.subject_alt_names.push(SanType::IpAddress(ip));
            }
        } else if let Ok(dns) = s.clone().try_into() {
            params.subject_alt_names.push(SanType::DnsName(dns));
        }
    }

    let key_pair =
        KeyPair::generate().map_err(|e| ServerError::Crypto(format!("keypair gen failed: {e}")))?;
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| ServerError::Crypto(format!("self-signed cert failed: {e}")))?;
    let cert_pem = cert.pem();
    let key_pem = key_pair.serialize_pem();
    let fp = compute_der_fingerprint(cert.der().as_ref());

    fs::write(cert_path, &cert_pem)?;
    write_strict_file(key_path, key_pem.as_bytes())?;

    Ok(TlsMaterial {
        cert_pem,
        key_pem,
        sha256_fingerprint: fp,
    })
}

pub fn compute_der_fingerprint(der: &[u8]) -> String {
    let digest = Sha256::digest(der);
    let hex_str = hex::encode(digest);
    hex_str
        .as_bytes()
        .chunks(2)
        .map(|chunk| std::str::from_utf8(chunk).unwrap_or("00").to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join(":")
}

pub fn compute_pem_cert_fingerprint(cert_pem: &str) -> Result<String, ServerError> {
    let mut b64 = String::new();
    let mut in_cert = false;
    for line in cert_pem.lines() {
        let t = line.trim();
        if t == "-----BEGIN CERTIFICATE-----" {
            in_cert = true;
            continue;
        }
        if t == "-----END CERTIFICATE-----" {
            break;
        }
        if in_cert {
            b64.push_str(t);
        }
    }
    if b64.is_empty() {
        return Err(ServerError::Crypto(
            "PEM file did not contain CERTIFICATE block".to_string(),
        ));
    }
    let der = B64
        .decode(b64.as_bytes())
        .map_err(|e| ServerError::Crypto(format!("invalid PEM base64: {e}")))?;
    Ok(compute_der_fingerprint(&der))
}

#[cfg(unix)]
fn write_strict_file(path: &Path, data: &[u8]) -> Result<(), ServerError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(data)?;
    f.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn write_strict_file(path: &Path, data: &[u8]) -> Result<(), ServerError> {
    fs::write(path, data)?;
    Ok(())
}

#[cfg(unix)]
fn ensure_strict_file_mode(path: &Path) -> Result<(), ServerError> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::metadata(path)?;
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(ServerError::Crypto(format!(
            "secret file {} has overly permissive mode {:o}; expected 0600",
            path.display(),
            mode
        )));
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_strict_file_mode(_path: &Path) -> Result<(), ServerError> {
    Ok(())
}
