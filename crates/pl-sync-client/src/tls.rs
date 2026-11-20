//! TLS for the client core (docs/SYNC.md §8, docs/REMOTE-APP.md §4).
//!
//! * without pin: standard check (rustls/webpki) against the system trust (`rustls-native-certs`);
//! * with pin: own [`ServerCertVerifier`] – the certificate is valid if the SHA-256 of its
//!   SubjectPublicKeyInfo (hex, lower case, 64 characters) equals the confirmed pin and it is
//!   valid in time. Chain/name are then not checked (self-signed, access by IP), the
//!   handshake signature however is (possession of the private key). The check runs **in the
//!   handshake** – with a wrong pin no request (and no token) leaves the client.
//!
//! Pin format = `tls_pin` of the shell (`apps/desktop-shell/src/remote.rs::spki_pin`).

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, DigitallySignedStruct, OtherError, RootCertStore, SignatureScheme};
use sha2::{Digest, Sha256};

/// Marker in the error text by which the core recognizes a pin error.
pub const PIN_MISMATCH_MARKER: &str = "paperrust-spki-pin-mismatch";

/// SHA-256 as hex (lower case, without separator).
pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// Valid pin format (64 hex characters).
pub fn valid_pin(pin: &str) -> bool {
    pin.len() == 64 && pin.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A DER element: (tag, content, rest, total length).
fn der_next(data: &[u8]) -> Option<(u8, &[u8], &[u8], usize)> {
    let tag = *data.first()?;
    let first = *data.get(1)?;
    let (len, hdr) = if first < 0x80 {
        (first as usize, 2)
    } else {
        let n = (first & 0x7f) as usize;
        if n == 0 || n > 4 {
            return None;
        }
        let mut len = 0usize;
        for i in 0..n {
            len = (len << 8) | *data.get(2 + i)? as usize;
        }
        (len, 2 + n)
    };
    let end = hdr.checked_add(len)?;
    if end > data.len() {
        return None;
    }
    Some((tag, &data[hdr..end], &data[end..], end))
}

/// Fields of a certificate that the pin check needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertFacts {
    /// SHA-256 of the SubjectPublicKeyInfo (hex).
    pub spki_sha256: String,
    /// Validity as Unix seconds.
    pub not_before: i64,
    pub not_after: i64,
}

/// UTCTime (YYMMDDhhmmssZ) / GeneralizedTime (YYYYMMDDhhmmssZ) → Unix-Sekunden.
fn der_time(tag: u8, t: &[u8]) -> Option<i64> {
    let s = std::str::from_utf8(t).ok()?.strip_suffix('Z')?;
    let full = match tag {
        0x17 if s.len() == 12 => {
            let yy: u32 = s[..2].parse().ok()?;
            format!("{}{s}", if yy >= 50 { "19" } else { "20" })
        }
        0x18 if s.len() == 14 => s.to_string(),
        _ => return None,
    };
    let dt = chrono::NaiveDateTime::parse_from_str(&full, "%Y%m%d%H%M%S").ok()?;
    Some(dt.and_utc().timestamp())
}

/// Read X.509 DER (SPKI + validity), without signature check.
pub fn cert_facts(der: &[u8]) -> Option<CertFacts> {
    let (0x30, cert, _, _) = der_next(der)? else { return None };
    let (0x30, tbs, _, _) = der_next(cert)? else { return None };
    let mut rest = tbs;
    if rest.first() == Some(&0xa0) {
        rest = der_next(rest)?.2; // [0] version
    }
    let (_, _, r, _) = der_next(rest)?; // serialNumber
    let (_, _, r, _) = der_next(r)?; // signature
    let (_, _, r, _) = der_next(r)?; // issuer
    let (_, validity, r, _) = der_next(r)?;
    let (_, _, r, _) = der_next(r)?; // subject
    let (0x30, _, _, spki_len) = der_next(r)? else { return None };
    let spki = &r[..spki_len];
    let (t1, nb, after, _) = der_next(validity)?;
    let (t2, na, _, _) = der_next(after)?;
    Some(CertFacts { spki_sha256: sha256_hex(spki), not_before: der_time(t1, nb)?, not_after: der_time(t2, na)? })
}

/// SPKI pin of a DER certificate (same format as the shell).
pub fn spki_pin(der: &[u8]) -> Option<String> {
    cert_facts(der).map(|c| c.spki_sha256)
}

struct PinMismatch;

// rustls shows `OtherError` via Debug → marker there as well
impl std::fmt::Debug for PinMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(PIN_MISMATCH_MARKER)
    }
}

impl std::fmt::Display for PinMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(PIN_MISMATCH_MARKER)
    }
}

impl std::error::Error for PinMismatch {}

/// Verifier for a pinned certificate.
#[derive(Debug)]
pub struct PinnedVerifier {
    pin: String,
    algs: WebPkiSupportedAlgorithms,
}

impl PinnedVerifier {
    pub fn new(pin: &str, provider: &CryptoProvider) -> Self {
        Self { pin: pin.to_ascii_lowercase(), algs: provider.signature_verification_algorithms }
    }
}

impl ServerCertVerifier for PinnedVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let Some(f) = cert_facts(end_entity.as_ref()) else {
            return Err(rustls::Error::InvalidCertificate(CertificateError::BadEncoding));
        };
        if f.spki_sha256 != self.pin {
            return Err(rustls::Error::InvalidCertificate(CertificateError::Other(OtherError(Arc::new(PinMismatch)))));
        }
        let now = now.as_secs() as i64;
        if now > f.not_after {
            return Err(rustls::Error::InvalidCertificate(CertificateError::Expired));
        }
        if now < f.not_before {
            return Err(rustls::Error::InvalidCertificate(CertificateError::NotValidYet));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

/// TLS-Einstellungen eines Profils.
#[derive(Debug, Clone, Default)]
pub struct TlsOptions {
    /// Confirmed SPKI pin (`tls_pin` from `app.toml`); empty/None = system trust.
    pub pin: Option<String>,
    /// Additional trust anchors (tests; own CA without OS store).
    pub extra_roots: Vec<CertificateDer<'static>>,
    /// Systemzertifikate laden (Standard ja; Tests schalten ab).
    pub skip_native_roots: bool,
}

/// rustls configuration for reqwest (`use_preconfigured_tls`).
pub fn client_config(opts: &TlsOptions) -> Result<rustls::ClientConfig, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?;
    let pin = opts.pin.as_deref().map(str::trim).filter(|p| !p.is_empty());
    let config = match pin {
        Some(pin) => {
            if !valid_pin(pin) {
                return Err("invalid tls pin (expected 64 hex characters)".into());
            }
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(PinnedVerifier::new(pin, &provider)))
                .with_no_client_auth()
        }
        None => {
            let mut roots = RootCertStore::empty();
            if !opts.skip_native_roots {
                let native = rustls_native_certs::load_native_certs();
                roots.add_parsable_certificates(native.certs);
            }
            roots.add_parsable_certificates(opts.extra_roots.iter().cloned());
            if roots.is_empty() {
                return Err("no trusted root certificates available".into());
            }
            builder.with_root_certificates(roots).with_no_client_auth()
        }
    };
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn der_times() {
        assert_eq!(der_time(0x17, b"250106080000Z"), Some(1736150400));
        assert_eq!(der_time(0x18, b"20991231235959Z"), Some(4102444799));
        assert_eq!(der_time(0x17, b"bogus"), None);
    }

    #[test]
    fn pin_format() {
        assert!(valid_pin(&"a".repeat(64)));
        assert!(!valid_pin("abc"));
        assert!(client_config(&TlsOptions { pin: Some("xyz".into()), ..Default::default() }).is_err());
    }
}
