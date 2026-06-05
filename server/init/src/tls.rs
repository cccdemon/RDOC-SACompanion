//! Self-signed TLS for the signaling endpoint. Generates + persists a cert on
//! first run (so the pinned fingerprint stays stable across restarts) and
//! reports its SHA-256 so clients can pin it. PEM-pin, no CA needed.

use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};
use std::path::Path;

pub struct Tls {
    pub cert_pem: String,
    pub key_pem: String,
    /// SHA-256 of the cert DER, hex — the value clients pass as CERT_SHA256.
    pub fingerprint: String,
}

/// Load existing cert/key, or generate a self-signed pair and persist it.
/// SANs: localhost + 127.0.0.1, plus any comma-separated TLS_SAN entries.
pub fn ensure(cert_path: &str, key_path: &str) -> Result<Tls> {
    if Path::new(cert_path).exists() && Path::new(key_path).exists() {
        let cert_pem = std::fs::read_to_string(cert_path)?;
        let key_pem = std::fs::read_to_string(key_path)?;
        let fingerprint = fingerprint_of(&cert_pem)?;
        return Ok(Tls { cert_pem, key_pem, fingerprint });
    }

    let mut sans = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    if let Ok(extra) = std::env::var("TLS_SAN") {
        for s in extra.split(',') {
            let s = s.trim();
            if !s.is_empty() {
                sans.push(s.to_string());
            }
        }
    }
    let key = rcgen::generate_simple_self_signed(sans)?;
    let cert_pem = key.cert.pem();
    let key_pem = key.key_pair.serialize_pem();
    std::fs::write(cert_path, &cert_pem)?;
    std::fs::write(key_path, &key_pem)?;
    let fingerprint = fingerprint_of(&cert_pem)?;
    Ok(Tls { cert_pem, key_pem, fingerprint })
}

fn fingerprint_of(cert_pem: &str) -> Result<String> {
    let mut rd = std::io::Cursor::new(cert_pem.as_bytes());
    let der = rustls_pemfile::certs(&mut rd)
        .next()
        .ok_or_else(|| anyhow!("no CERTIFICATE in PEM"))??;
    let mut h = Sha256::new();
    h.update(der.as_ref());
    Ok(hex::encode(h.finalize()))
}
