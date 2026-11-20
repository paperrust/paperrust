//! TLS pinning (docs/REMOTE-APP.md §8 "Unit (Kern)", docs/SYNC.md §9 "Sicherheit"): test certificates
//! with rcgen at runtime; correct pin ok, other pin hard error **without** a request
//! (or a token) leaving the client, expired error, without pin system trust.

mod common;

use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use common::*;
use pl_sync_client::Error;
use pl_sync_client::remote::PaperRustRemote;
use pl_sync_client::tls::{TlsOptions, sha256_hex, spki_pin};
use rcgen::{CertificateParams, KeyPair, SanType, date_time_ymd};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

struct TestCert {
    der: CertificateDer<'static>,
    key: KeyPair,
}

fn cert(not_after_year: i32) -> TestCert {
    let key = KeyPair::generate().unwrap();
    let mut p = CertificateParams::new(vec!["nas.local".to_string()]).unwrap();
    p.subject_alt_names.push(SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    p.not_before = date_time_ymd(2020, 1, 1);
    p.not_after = date_time_ymd(not_after_year, 1, 1);
    let c = p.self_signed(&key).unwrap();
    TestCert { der: c.der().clone(), key }
}

fn acceptor(c: &TestCert) -> tokio_rustls::TlsAcceptor {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![c.der.clone()], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(c.key.serialize_der())))
        .unwrap();
    tokio_rustls::TlsAcceptor::from(Arc::new(cfg))
}

fn opts(pin: Option<String>, roots: Vec<CertificateDer<'static>>) -> TlsOptions {
    TlsOptions { pin, extra_roots: roots, skip_native_roots: true }
}

#[tokio::test]
async fn spki_pin_matches_rcgen_public_key() {
    let c = cert(2099);
    assert_eq!(spki_pin(&c.der).unwrap(), sha256_hex(&c.key.public_key_der()));
}

#[tokio::test]
async fn pinning() {
    let mock = Mock::new(true);
    let good = cert(2099);
    let srv = serve(router(mock.clone()), Some(acceptor(&good))).await;
    let url = srv.url(true);
    let pin = spki_pin(&good.der).unwrap();

    // correct pin (name "nas.local" ≠ 127.0.0.1 does not matter with a pin)
    let r = PaperRustRemote::new(&url, &opts(Some(pin.clone()), vec![])).unwrap();
    assert_eq!(r.server().await.unwrap().product, "paperrust");
    assert_eq!(mock.requests().len(), 1);

    // other pin → hard error in the handshake, no HTTP request, no token sent
    mock.clear_log();
    let other = spki_pin(&cert(2099).der).unwrap();
    let r = PaperRustRemote::new(&url, &opts(Some(other), vec![])).unwrap();
    r.set_token(Some(zeroize::Zeroizing::new("prt_secret_should_not_leave".into())));
    let e = r.server().await.unwrap_err();
    assert!(matches!(e, Error::PinMismatch), "{e:?}");
    let e = r.create_device(USER, PASS, None, "x").await.unwrap_err();
    assert!(matches!(e, Error::PinMismatch), "{e:?}");
    assert!(mock.requests().is_empty(), "no request may pass a pin mismatch");
    assert!(mock.st.lock().unwrap().auth_headers.is_empty());

    // without pin, certificate not trusted → TLS error
    let r = PaperRustRemote::new(&url, &opts(None, vec![])).unwrap_or_else(|_| {
        // no roots → configuration rejects; check with a foreign root
        PaperRustRemote::new(&url, &opts(None, vec![cert(2099).der])).unwrap()
    });
    let e = r.server().await.unwrap_err();
    assert!(matches!(e, Error::Tls(_)), "{e:?}");

    // without pin, certificate as trust anchor (own CA) → standard check ok (SAN 127.0.0.1)
    let r = PaperRustRemote::new(&url, &opts(None, vec![good.der.clone()])).unwrap();
    assert_eq!(r.server().await.unwrap().api, 2);
}

#[tokio::test]
async fn pinned_but_expired_is_rejected() {
    let mock = Mock::new(true);
    let old = cert(2021);
    let srv = serve(router(mock.clone()), Some(acceptor(&old))).await;
    let r = PaperRustRemote::new(&srv.url(true), &opts(Some(spki_pin(&old.der).unwrap()), vec![])).unwrap();
    let e = r.server().await.unwrap_err();
    assert!(matches!(e, Error::Tls(ref m) if m.to_ascii_lowercase().contains("expired")), "{e:?}");
    assert!(mock.requests().is_empty());
}

#[tokio::test]
async fn http_without_tls_still_works_for_loopback() {
    // http:// only after explicit confirmation (shell); the core itself speaks it
    let mock = Mock::new(true);
    let srv = serve(router(mock.clone()), None).await;
    let r = PaperRustRemote::new(&srv.url(false), &TlsOptions::default()).unwrap();
    assert_eq!(r.server().await.unwrap().api, 2);
}
