//! SMTP intake with a local SMTP client (raw SMTP over TCP, 127.0.0.1):
//! wrong/revoked token, too large, attachment without PDF, several attachments, no relay,
//! token as recipient address, AUTH LOGIN, lock-out after failed attempts, volume limit,
//! IP list, STARTTLS (test certificate). Devices and pipeline are simulated in memory.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use futures::future::BoxFuture;
use pl_scan::intake::{Intake, IntakeDoc};
use pl_scan::smtp::{Context, Limits, SmtpConfig};
use pl_scan::store::{ActiveDevice, DeviceStore};
use pl_scan::token;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::watch;

// ------------------------------------------------------------------------- Simulations

#[derive(Default)]
struct Devices {
    /// hash → (device, revoked)
    by_hash: Mutex<HashMap<String, (ActiveDevice, bool)>>,
    used: Mutex<Vec<(i64, i64)>>,
}

impl Devices {
    fn add(&self, id: i64, owner: i64, allowed_ips: &str) -> String {
        let t = token::generate();
        self.by_hash.lock().unwrap().insert(
            token::hash(&t),
            (ActiveDevice { id, name: format!("dev{id}"), owner_id: owner, allowed_ips: allowed_ips.into() }, false),
        );
        t
    }
    fn revoke(&self, t: &str) {
        if let Some(e) = self.by_hash.lock().unwrap().get_mut(&token::hash(t)) {
            e.1 = true;
        }
    }
}

impl DeviceStore for Devices {
    fn find_active(&self, token_hash: &str) -> BoxFuture<'_, Result<Option<ActiveDevice>, String>> {
        let r = self.by_hash.lock().unwrap().get(token_hash).filter(|(_, revoked)| !revoked).map(|(d, _)| d.clone());
        Box::pin(async move { Ok(r) })
    }
    fn record_use(&self, id: i64, _ip: &str, documents: i64) -> BoxFuture<'_, ()> {
        self.used.lock().unwrap().push((id, documents));
        Box::pin(async {})
    }
}

#[derive(Default)]
struct Captured {
    docs: Mutex<Vec<(IntakeDoc, Vec<u8>)>>,
}

impl Intake for Captured {
    fn submit(&self, doc: IntakeDoc) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            let data = std::fs::read(&doc.path).map_err(|e| e.to_string())?;
            self.docs.lock().unwrap().push((doc, data));
            Ok("t".into())
        })
    }
}

struct Server {
    addr: SocketAddr,
    devices: Arc<Devices>,
    cap: Arc<Captured>,
    _scratch: tempfile::TempDir,
    _stop: watch::Sender<bool>,
}

fn data_path(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name).to_string_lossy().into_owned()
}

async fn server_with(tune: impl FnOnce(&mut SmtpConfig)) -> Server {
    let scratch = tempfile::tempdir().unwrap();
    let mut cfg =
        SmtpConfig::from_lookup(|k| (k == pl_scan::smtp::SETTING_ENABLED).then(|| "on".to_string()), scratch.path().to_path_buf())
            .unwrap()
            .unwrap();
    cfg.max_bytes = 64 * 1024;
    tune(&mut cfg);
    let devices = Arc::new(Devices::default());
    let cap = Arc::new(Captured::default());
    let ctx = Arc::new(Context { cfg, devices: devices.clone(), intake: cap.clone(), limits: Limits::default() });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let (tx, rx) = watch::channel(false);
    tokio::spawn(pl_scan::smtp::serve(l, ctx, rx));
    Server { addr, devices, cap, _scratch: scratch, _stop: tx }
}

async fn server() -> Server {
    server_with(|_| {}).await
}

// ------------------------------------------------------------------------- Test-Client

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

struct C {
    io: BufReader<Box<dyn Io>>,
}

impl C {
    async fn connect(addr: SocketAddr) -> (C, String) {
        let s = TcpStream::connect(addr).await.unwrap();
        let mut c = C { io: BufReader::new(Box::new(s)) };
        let greet = c.reply().await;
        (c, greet)
    }
    /// Read a response (multi-line until `NNN `). Empty if the connection is closed.
    async fn reply(&mut self) -> String {
        let mut all = String::new();
        loop {
            let mut l = String::new();
            if self.io.read_line(&mut l).await.unwrap_or(0) == 0 {
                return all;
            }
            all.push_str(&l);
            if l.len() < 4 || l.as_bytes()[3] == b' ' {
                return all;
            }
        }
    }
    async fn cmd(&mut self, line: &str) -> String {
        self.io.get_mut().write_all(format!("{line}\r\n").as_bytes()).await.unwrap();
        self.reply().await
    }
    async fn raw(&mut self, data: &[u8]) {
        self.io.get_mut().write_all(data).await.unwrap();
    }
    async fn ehlo(&mut self) -> String {
        let r = self.cmd("EHLO printer.local").await;
        assert!(r.starts_with("250"), "{r}");
        r
    }
    async fn auth_plain(&mut self, secret: &str) -> String {
        let b = base64::engine::general_purpose::STANDARD.encode(format!("\0scanner\0{secret}"));
        self.cmd(&format!("AUTH PLAIN {b}")).await
    }
    /// MAIL/RCPT/DATA with a finished message → response to the end of DATA.
    async fn send_mail(&mut self, rcpt: &str, msg: &str) -> String {
        let r = self.cmd("MAIL FROM:<scanner@printer.local>").await;
        assert!(r.starts_with("250"), "MAIL: {r}");
        let r = self.cmd(&format!("RCPT TO:<{rcpt}>")).await;
        if !r.starts_with("250") {
            return r;
        }
        let r = self.cmd("DATA").await;
        assert!(r.starts_with("354"), "DATA: {r}");
        // dot-stuffing like a real client
        let mut body = String::new();
        for line in msg.split("\r\n") {
            if line.starts_with('.') {
                body.push('.');
            }
            body.push_str(line);
            body.push_str("\r\n");
        }
        body.push_str(".\r\n");
        self.raw(body.as_bytes()).await;
        self.reply().await
    }
}

/// MIME message with attachments (name, type, content).
fn message(parts: &[(&str, &str, &[u8])]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut m = String::from(
        "From: scanner@printer.local\r\nTo: paperrust@nas.local\r\nSubject: Scan\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"XYZ\"\r\n\r\n--XYZ\r\nContent-Type: text/plain\r\n\r\n.Scan from printer\r\n",
    );
    for (name, ct, data) in parts {
        m.push_str(&format!("--XYZ\r\nContent-Type: {ct}; name=\"{name}\"\r\nContent-Disposition: attachment; filename=\"{name}\"\r\nContent-Transfer-Encoding: base64\r\n\r\n"));
        let enc = b64.encode(data);
        for chunk in enc.as_bytes().chunks(76) {
            m.push_str(std::str::from_utf8(chunk).unwrap());
            m.push_str("\r\n");
        }
    }
    m.push_str("--XYZ--\r\n");
    m
}

const PDF: &[u8] = b"%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n";
const JPEG: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0, 16, b'J', b'F', b'I', b'F', 0, 1, 0xFF, 0xD9];
const TIFF: &[u8] = b"II*\0\x08\0\0\0\0\0";

// ------------------------------------------------------------------------- Tests

#[tokio::test]
async fn auth_plain_delivers_pdf_owned_by_device_owner() {
    let s = server().await;
    let t = s.devices.add(1, 99, "");
    let (mut c, greet) = C::connect(s.addr).await;
    assert!(greet.starts_with("220"));
    let e = c.ehlo().await;
    assert!(e.contains("AUTH PLAIN LOGIN") && e.contains("SIZE 65536") && !e.contains("STARTTLS"), "{e}");
    assert!(c.auth_plain(&t).await.starts_with("235"));
    let r = c.send_mail("anything@paperrust.local", &message(&[("Rechnung.pdf", "application/pdf", PDF)])).await;
    assert!(r.starts_with("250") && r.contains("1 document"), "{r}");
    {
        let docs = s.cap.docs.lock().unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].0.owner_id, 99);
        assert_eq!(docs[0].0.filename, "Rechnung.pdf");
        assert_eq!(docs[0].0.source, pl_consumer::DocumentSource::ApiUpload);
        assert_eq!(docs[0].1, PDF);
        assert_eq!(s.devices.used.lock().unwrap().as_slice(), &[(1, 1)]);
    }
    assert!(c.cmd("QUIT").await.starts_with("221"));
}

#[tokio::test]
async fn token_as_recipient_address() {
    let s = server().await;
    let t = s.devices.add(2, 5, "127.0.0.1");
    let (mut c, _) = C::connect(s.addr).await;
    c.ehlo().await;
    let r = c.send_mail(&format!("{t}@paperrust.local"), &message(&[("a.pdf", "application/pdf", PDF)])).await;
    assert!(r.starts_with("250"), "{r}");
    assert_eq!(s.cap.docs.lock().unwrap()[0].0.owner_id, 5);
    // plus form, upper case
    let r = c.send_mail(&format!("scans+{}@x", t.to_ascii_uppercase()), &message(&[("b.pdf", "application/pdf", PDF)])).await;
    assert!(r.starts_with("250"), "{r}");
}

#[tokio::test]
async fn wrong_token_and_no_relay() {
    let s = server().await;
    let _t = s.devices.add(1, 1, "");
    let (mut c, _) = C::connect(s.addr).await;
    c.ehlo().await;
    assert!(c.auth_plain("scan-aaaaaaaaaaaaaaaaaaaaaaaaaa").await.starts_with("535"));
    assert!(c.auth_plain("not-a-token").await.starts_with("535"));
    // Without login: foreign address → no relay
    let r = c.send_mail("victim@example.com", "Subject: x\r\n\r\nhi").await;
    assert!(r.starts_with("550") && r.contains("Relaying denied"), "{r}");
    c.cmd("RSET").await;
    let r = c.send_mail("scan-bbbbbbbbbbbbbbbbbbbbbbbbbb@paperrust.local", "Subject: x\r\n\r\nhi").await;
    assert!(r.starts_with("550"), "{r}");
    // DATA without a recipient
    assert!(c.cmd("DATA").await.starts_with("503"));
    assert!(s.cap.docs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn revoked_token_is_refused_immediately() {
    let s = server().await;
    let t = s.devices.add(3, 1, "");
    let (mut c, _) = C::connect(s.addr).await;
    c.ehlo().await;
    assert!(c.auth_plain(&t).await.starts_with("235"));
    s.devices.revoke(&t);
    let (mut c2, _) = C::connect(s.addr).await;
    c2.ehlo().await;
    assert!(c2.auth_plain(&t).await.starts_with("535"));
    let r = c2.send_mail(&format!("{t}@x"), "Subject: x\r\n\r\nhi").await;
    assert!(r.starts_with("550"), "{r}");
}

#[tokio::test]
async fn oversize_is_rejected() {
    let s = server().await;
    let t = s.devices.add(1, 1, "");
    let (mut c, _) = C::connect(s.addr).await;
    c.ehlo().await;
    c.auth_plain(&t).await;
    // announced size
    assert!(c.cmd("MAIL FROM:<a@b> SIZE=999999999").await.starts_with("552"));
    // actual size (limit 64 KiB)
    let big = vec![b'%'; 100 * 1024];
    let mut pdf = PDF.to_vec();
    pdf.extend_from_slice(&big);
    let r = c.send_mail("x@y", &message(&[("big.pdf", "application/pdf", &pdf)])).await;
    assert!(r.starts_with("552"), "{r}");
    assert!(s.cap.docs.lock().unwrap().is_empty());
    // connection stays usable
    let r = c.send_mail("x@y", &message(&[("small.pdf", "application/pdf", PDF)])).await;
    assert!(r.starts_with("250"), "{r}");
}

#[tokio::test]
async fn non_pdf_attachment_is_rejected() {
    let s = server().await;
    let t = s.devices.add(1, 1, "");
    let (mut c, _) = C::connect(s.addr).await;
    c.ehlo().await;
    c.auth_plain(&t).await;
    // Announced as PDF, content is a ZIP → recognised by content and rejected
    let r =
        c.send_mail("x@y", &message(&[("fake.pdf", "application/pdf", b"PK\x03\x04zipzip"), ("notes.txt", "text/plain", b"hello")])).await;
    assert!(r.starts_with("554"), "{r}");
    let r = c.send_mail("x@y", "Subject: nur Text\r\n\r\nkein Anhang").await;
    assert!(r.starts_with("554"), "{r}");
    assert!(s.cap.docs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn several_attachments() {
    let s = server().await;
    let t = s.devices.add(1, 8, "");
    let (mut c, _) = C::connect(s.addr).await;
    c.ehlo().await;
    c.auth_plain(&t).await;
    let msg = message(&[
        ("seite1.pdf", "application/pdf", PDF),
        ("foto", "image/jpeg", JPEG),
        ("scan.tiff", "image/tiff", TIFF),
        ("readme.txt", "text/plain", b"ignored"),
        ("../../evil.pdf", "application/pdf", PDF),
    ]);
    let r = c.send_mail("x@y", &msg).await;
    assert!(r.starts_with("250") && r.contains("4 document"), "{r}");
    let docs = s.cap.docs.lock().unwrap();
    let names: Vec<&str> = docs.iter().map(|(d, _)| d.filename.as_str()).collect();
    assert_eq!(names, ["seite1.pdf", "foto.jpg", "scan.tiff", "evil.pdf"]);
    assert!(docs.iter().all(|(d, _)| d.owner_id == 8));
    // Each file in its own spool directory under SCRATCH_DIR
    let dirs: std::collections::HashSet<_> = docs.iter().map(|(d, _)| d.path.parent().unwrap().to_path_buf()).collect();
    assert_eq!(dirs.len(), 4);
}

#[tokio::test]
async fn auth_login_and_username_only() {
    let s = server().await;
    let t = s.devices.add(1, 1, "");
    let b = |x: &str| base64::engine::general_purpose::STANDARD.encode(x);
    let (mut c, _) = C::connect(s.addr).await;
    c.ehlo().await;
    assert!(c.cmd("AUTH LOGIN").await.starts_with("334"));
    assert!(c.cmd(&b("scanner")).await.starts_with("334"));
    assert!(c.cmd(&b(&t)).await.starts_with("235"));
    // token only in the user name (devices with one field)
    let (mut c2, _) = C::connect(s.addr).await;
    c2.ehlo().await;
    c2.cmd("AUTH LOGIN").await;
    c2.cmd(&b(&t)).await;
    assert!(c2.cmd(&b("")).await.starts_with("235"));
}

#[tokio::test]
async fn brute_force_lockout() {
    let s = server().await;
    let t = s.devices.add(1, 1, "");
    let (mut c, _) = C::connect(s.addr).await;
    c.ehlo().await;
    for _ in 0..4 {
        assert!(c.auth_plain("scan-aaaaaaaaaaaaaaaaaaaaaaaaaa").await.starts_with("535"));
    }
    assert!(c.auth_plain("scan-aaaaaaaaaaaaaaaaaaaaaaaaaa").await.starts_with("421"));
    // New connection from the same IP: blocked, even with the correct token
    let (_c2, greet) = C::connect(s.addr).await;
    assert!(greet.starts_with("421"), "{greet}");
    let _ = t;
}

#[tokio::test]
async fn rate_limit_per_device() {
    let s = server_with(|c| c.rate_per_minute = 1).await;
    let t = s.devices.add(1, 1, "");
    let (mut c, _) = C::connect(s.addr).await;
    c.ehlo().await;
    c.auth_plain(&t).await;
    assert!(c.send_mail("x@y", &message(&[("a.pdf", "application/pdf", PDF)])).await.starts_with("250"));
    let r = c.send_mail("x@y", &message(&[("b.pdf", "application/pdf", PDF)])).await;
    assert!(r.starts_with("451"), "{r}");
    assert_eq!(s.cap.docs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn ip_allow_lists() {
    // global: 127.0.0.1 not allowed
    let s = server_with(|c| c.allowed_ips = pl_scan::net::parse_list("192.168.0.0/16").unwrap()).await;
    let (_c, greet) = C::connect(s.addr).await;
    assert!(greet.starts_with("554"), "{greet}");
    // per device: token is valid only from 10.0.0.5
    let s = server().await;
    let t = s.devices.add(1, 1, "10.0.0.5");
    let (mut c, _) = C::connect(s.addr).await;
    c.ehlo().await;
    assert!(c.auth_plain(&t).await.starts_with("535"));
}

#[tokio::test]
async fn unknown_commands_and_pipelining_injection() {
    let s = server().await;
    let (mut c, _) = C::connect(s.addr).await;
    assert!(c.cmd("MAIL FROM:<a@b>").await.starts_with("503"), "EHLO first");
    c.ehlo().await;
    assert!(c.cmd("VRFY root").await.starts_with("252"));
    assert!(c.cmd("EXPN staff").await.starts_with("500"));
    assert!(c.cmd("STARTTLS").await.starts_with("502"), "no TLS configured");
    assert!(c.cmd("NOOP").await.starts_with("250"));
}

// ------------------------------------------------------------------------- STARTTLS

#[derive(Debug)]
struct TrustCert(rustls::pki_types::CertificateDer<'static>);

use tokio_rustls::rustls;

impl rustls::client::danger::ServerCertVerifier for TrustCert {
    fn verify_server_cert(
        &self,
        end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.0.as_ref() {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("unexpected certificate".into()))
        }
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &rustls::pki_types::CertificateDer<'_>,
        d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &rustls::crypto::ring::default_provider().signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &rustls::pki_types::CertificateDer<'_>,
        d: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &rustls::crypto::ring::default_provider().signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
    }
}

async fn starttls(c: C) -> C {
    use rustls::pki_types::pem::PemObject as _;
    let cert = rustls::pki_types::CertificateDer::from_pem_slice(&std::fs::read(data_path("test-cert.pem")).unwrap()).unwrap();
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(TrustCert(cert)))
        .with_no_client_auth();
    let conn = tokio_rustls::TlsConnector::from(Arc::new(cfg));
    let inner = c.io.into_inner();
    let tls = conn.connect(rustls::pki_types::ServerName::try_from("localhost").unwrap(), inner).await.unwrap();
    C { io: BufReader::new(Box::new(tls)) }
}

#[tokio::test]
async fn starttls_and_require_tls() {
    let tls = pl_scan::smtp::load_tls(&data_path("test-cert.pem"), &data_path("test-key.pem")).unwrap();
    let s = server_with(|c| {
        c.tls = Some(tls);
        c.require_tls = true;
    })
    .await;
    let t = s.devices.add(1, 3, "");
    let (mut c, _) = C::connect(s.addr).await;
    let e = c.ehlo().await;
    assert!(e.contains("STARTTLS") && !e.contains("AUTH"), "{e}");
    assert!(c.auth_plain(&t).await.starts_with("530"));
    assert!(c.cmd("MAIL FROM:<a@b>").await.starts_with("530"));
    assert!(c.cmd("STARTTLS").await.starts_with("220"));
    let mut c = starttls(c).await;
    let e = c.ehlo().await;
    assert!(e.contains("AUTH PLAIN") && !e.contains("STARTTLS"), "{e}");
    assert!(c.auth_plain(&t).await.starts_with("235"));
    let r = c.send_mail("x@y", &message(&[("tls.pdf", "application/pdf", PDF)])).await;
    assert!(r.starts_with("250"), "{r}");
    assert_eq!(s.cap.docs.lock().unwrap()[0].0.owner_id, 3);
}

#[tokio::test]
async fn starttls_with_pipelined_data_closes() {
    let tls = pl_scan::smtp::load_tls(&data_path("test-cert.pem"), &data_path("test-key.pem")).unwrap();
    let s = server_with(|c| c.tls = Some(tls)).await;
    let (mut c, _) = C::connect(s.addr).await;
    c.ehlo().await;
    // plaintext after STARTTLS in the same buffer (command injection) → connection closed
    c.raw(b"STARTTLS\r\nRSET\r\n").await;
    let r = c.reply().await;
    assert!(r.starts_with("220"), "{r}");
    let mut buf = String::new();
    let n = c.io.read_line(&mut buf).await.unwrap_or(0);
    assert_eq!(n, 0, "connection must be closed, got {buf:?}");
}

#[tokio::test]
async fn implicit_tls_smtps() {
    let tls = pl_scan::smtp::load_tls(&data_path("test-cert.pem"), &data_path("test-key.pem")).unwrap();
    let s = server_with(|c| {
        c.tls = Some(tls);
        c.implicit_tls = true;
        c.require_tls = true;
    })
    .await;
    let t = s.devices.add(1, 4, "");
    // TLS from the first byte, then SMTP
    let raw = TcpStream::connect(s.addr).await.unwrap();
    let mut c = starttls(C { io: BufReader::new(Box::new(raw)) }).await;
    assert!(c.reply().await.starts_with("220"));
    let e = c.ehlo().await;
    assert!(e.contains("AUTH PLAIN") && !e.contains("STARTTLS"), "{e}");
    assert!(c.auth_plain(&t).await.starts_with("235"));
    let r = c.send_mail("x@y", &message(&[("smtps.pdf", "application/pdf", PDF)])).await;
    assert!(r.starts_with("250"), "{r}");
    assert_eq!(s.cap.docs.lock().unwrap()[0].0.owner_id, 4);
    // Plaintext client gets no SMTP greeting: server waits for TLS, plaintext aborts
    let mut raw = TcpStream::connect(s.addr).await.unwrap();
    raw.write_all(b"EHLO plain\r\n").await.unwrap();
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), tokio::io::AsyncReadExt::read_to_end(&mut raw, &mut buf)).await;
    assert!(!buf.starts_with(b"220"), "{:?}", String::from_utf8_lossy(&buf));
}
