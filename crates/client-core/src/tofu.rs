use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error as RustlsError, SignatureScheme};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TofuDecision {
    FirstUse { host_key: String, fingerprint: String },
    Mismatch {
        host_key: String,
        previous: String,
        current: String,
    },
    Trusted { host_key: String, fingerprint: String },
}

/// Rustls ServerCertVerifier implementing Trust-On-First-Use (TOFU) certificate pinning.
#[derive(Debug)]
pub struct TofuVerifier {
    host_key: String,
    known_fingerprints: Arc<RwLock<HashMap<String, String>>>,
    last_decision: Arc<RwLock<Option<TofuDecision>>>,
    supported_algs: rustls::crypto::WebPkiSupportedAlgorithms,
}

impl TofuVerifier {
    pub fn new(
        host_key: String,
        known_fingerprints: Arc<RwLock<HashMap<String, String>>>,
    ) -> Self {
        let provider = rustls::crypto::ring::default_provider();
        Self {
            host_key,
            known_fingerprints,
            last_decision: Arc::new(RwLock::new(None)),
            supported_algs: provider.signature_verification_algorithms,
        }
    }

    pub fn take_last_decision(&self) -> Option<TofuDecision> {
        self.last_decision
            .write()
            .ok()
            .and_then(|mut g| g.take())
    }
}

pub fn sha256_fingerprint(der: &[u8]) -> String {
    let digest = Sha256::digest(der);
    let hex_str = hex::encode(digest);
    hex_str
        .as_bytes()
        .chunks(2)
        .map(|c| std::str::from_utf8(c).unwrap_or("00").to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join(":")
}

impl ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        let fp = sha256_fingerprint(end_entity.as_ref());
        let stored = self
            .known_fingerprints
            .read()
            .ok()
            .and_then(|g| g.get(&self.host_key).cloned());

        match stored {
            Some(prev) if prev == fp => {
                if let Ok(mut d) = self.last_decision.write() {
                    *d = Some(TofuDecision::Trusted {
                        host_key: self.host_key.clone(),
                        fingerprint: fp,
                    });
                }
                Ok(ServerCertVerified::assertion())
            }
            Some(prev) => {
                if let Ok(mut d) = self.last_decision.write() {
                    *d = Some(TofuDecision::Mismatch {
                        host_key: self.host_key.clone(),
                        previous: prev,
                        current: fp,
                    });
                }
                Err(RustlsError::General("TOFU_MISMATCH".to_string()))
            }
            None => {
                if let Ok(mut d) = self.last_decision.write() {
                    *d = Some(TofuDecision::FirstUse {
                        host_key: self.host_key.clone(),
                        fingerprint: fp,
                    });
                }
                Err(RustlsError::General("TOFU_FIRST_USE".to_string()))
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.supported_algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.supported_algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.supported_algs.supported_schemes()
    }
}
