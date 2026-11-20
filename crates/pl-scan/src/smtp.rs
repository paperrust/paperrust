//! SMTP intake for devices only (scan-to-email), RFC 5321 / 4954 (AUTH) / 3207 (STARTTLS).
//!
//! * Off unless `PAPERRUST_SMTP_INTAKE` is `on`; then nothing listens.
//! * **No relay:** nothing is ever forwarded or sent. A recipient is only
//!   accepted if the session is authenticated with a device token (AUTH PLAIN/LOGIN,
//!   token = password) or the recipient address carries a valid token (`scan-…@host`).
//! * Attachments (PDF, JPEG, TIFF – recognised by content, not by the type field) go one by one to the
//!   ingestion pipeline, owner = owner of the device. Other attachments are ignored; if a
//!   message has no matching attachment it is rejected (the device shows an error).
//! * Limits: size (`SIZE`), messages per minute and device, failed logins per IP,
//!   concurrent connections, timeouts. Sender IP list global and per device.
//! * STARTTLS if certificate and key are given; optionally mandatory.
//! * Tokens never appear in the log (only device id/name and IP).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncWrite, AsyncWriteExt as _, BufReader};
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, watch};
use tokio_rustls::rustls;

use crate::intake::{Intake, IntakeDoc};
use crate::net::{self, IpRule};
use crate::store::{ActiveDevice, DeviceStore};
use crate::token;

pub const SETTING_ENABLED: &str = "PAPERRUST_SMTP_INTAKE";
pub const SETTING_LISTEN: &str = "PAPERRUST_SMTP_INTAKE_LISTEN";
pub const SETTING_MAX_MB: &str = "PAPERRUST_SMTP_INTAKE_MAX_MB";
pub const SETTING_ALLOWED_IPS: &str = "PAPERRUST_SMTP_INTAKE_ALLOWED_IPS";
pub const SETTING_RATE: &str = "PAPERRUST_SMTP_INTAKE_RATE_PER_MINUTE";
pub const SETTING_TLS_CERT: &str = "PAPERRUST_SMTP_INTAKE_TLS_CERT";
pub const SETTING_TLS_KEY: &str = "PAPERRUST_SMTP_INTAKE_TLS_KEY";
pub const SETTING_REQUIRE_TLS: &str = "PAPERRUST_SMTP_INTAKE_REQUIRE_TLS";
pub const SETTING_IMPLICIT_TLS: &str = "PAPERRUST_SMTP_INTAKE_IMPLICIT_TLS";

pub const DEFAULT_LISTEN: &str = "0.0.0.0:2525";
pub const DEFAULT_MAX_MB: u64 = 50;
pub const DEFAULT_RATE: u32 = 30;

/// At most this many failed logins per IP within [`FAIL_WINDOW`], afterwards blocked until it expires.
pub const MAX_AUTH_FAILURES: usize = 5;
pub const FAIL_WINDOW: Duration = Duration::from_secs(600);
const MAX_CONNECTIONS: usize = 16;
const MAX_RECIPIENTS: usize = 20;
const MAX_BAD_COMMANDS: u32 = 10;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);
const SESSION_LIMIT: Duration = Duration::from_secs(1800);
const MAX_LINE: usize = 4096;

/// Settings of the intake.
#[derive(Clone)]
pub struct SmtpConfig {
    pub listen: SocketAddr,
    pub max_bytes: usize,
    pub allowed_ips: Vec<IpRule>,
    pub rate_per_minute: u32,
    pub tls: Option<Arc<rustls::ServerConfig>>,
    pub require_tls: bool,
    /// Direct TLS from the first byte (SMTPS, usually port 465) instead of STARTTLS.
    pub implicit_tls: bool,
    pub hostname: String,
    pub scratch: PathBuf,
}

impl std::fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("listen", &self.listen)
            .field("max_bytes", &self.max_bytes)
            .field("allowed_ips", &self.allowed_ips)
            .field("rate_per_minute", &self.rate_per_minute)
            .field("tls", &self.tls.is_some())
            .field("require_tls", &self.require_tls)
            .field("implicit_tls", &self.implicit_tls)
            .finish()
    }
}

fn on(v: Option<String>) -> bool {
    v.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "on" | "1" | "true" | "yes" | "y" | "t"))
}

/// TLS configuration from PEM files.
pub fn load_tls(cert: &str, key: &str) -> Result<Arc<rustls::ServerConfig>, String> {
    use rustls::pki_types::pem::PemObject as _;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let cert_pem = std::fs::read(cert).map_err(|e| format!("{SETTING_TLS_CERT}: {e}"))?;
    let key_pem = std::fs::read(key).map_err(|e| format!("{SETTING_TLS_KEY}: {e}"))?;
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&cert_pem)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{SETTING_TLS_CERT}: {e}"))?;
    if certs.is_empty() {
        return Err(format!("{SETTING_TLS_CERT}: no certificate found"));
    }
    let key = PrivateKeyDer::from_pem_slice(&key_pem).map_err(|e| format!("{SETTING_TLS_KEY}: {e}"))?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("TLS: {e}"))?;
    Ok(Arc::new(cfg))
}

impl SmtpConfig {
    /// From the settings (`get` = environment + UI). `Ok(None)` = off.
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>, scratch: PathBuf) -> Result<Option<SmtpConfig>, String> {
        if !on(get(SETTING_ENABLED)) {
            return Ok(None);
        }
        let listen_s = get(SETTING_LISTEN).filter(|s| !s.trim().is_empty()).unwrap_or_else(|| DEFAULT_LISTEN.into());
        let listen: SocketAddr = listen_s.trim().parse().map_err(|_| format!("{SETTING_LISTEN}: invalid address {listen_s:?} (expected ip:port)"))?;
        let max_mb = match get(SETTING_MAX_MB).filter(|s| !s.trim().is_empty()) {
            Some(v) => v.trim().parse::<u64>().ok().filter(|v| (1..=1024).contains(v)).ok_or_else(|| format!("{SETTING_MAX_MB}: expected 1–1024"))?,
            None => DEFAULT_MAX_MB,
        };
        let allowed_ips = net::parse_list(&get(SETTING_ALLOWED_IPS).unwrap_or_default()).map_err(|e| format!("{SETTING_ALLOWED_IPS}: invalid entry {e:?}"))?;
        let rate = match get(SETTING_RATE).filter(|s| !s.trim().is_empty()) {
            Some(v) => v.trim().parse::<u32>().ok().filter(|v| (1..=10_000).contains(v)).ok_or_else(|| format!("{SETTING_RATE}: expected 1–10000"))?,
            None => DEFAULT_RATE,
        };
        let cert = get(SETTING_TLS_CERT).filter(|s| !s.trim().is_empty());
        let key = get(SETTING_TLS_KEY).filter(|s| !s.trim().is_empty());
        let tls = match (cert, key) {
            (Some(c), Some(k)) => Some(load_tls(c.trim(), k.trim())?),
            (None, None) => None,
            _ => return Err(format!("{SETTING_TLS_CERT} and {SETTING_TLS_KEY} must be set together")),
        };
        let require_tls = on(get(SETTING_REQUIRE_TLS));
        if require_tls && tls.is_none() {
            return Err(format!("{SETTING_REQUIRE_TLS} needs {SETTING_TLS_CERT} and {SETTING_TLS_KEY}"));
        }
        let implicit_tls = on(get(SETTING_IMPLICIT_TLS));
        if implicit_tls && tls.is_none() {
            return Err(format!("{SETTING_IMPLICIT_TLS} needs {SETTING_TLS_CERT} and {SETTING_TLS_KEY}"));
        }
        Ok(Some(SmtpConfig {
            listen,
            max_bytes: (max_mb * 1024 * 1024) as usize,
            allowed_ips,
            rate_per_minute: rate,
            tls,
            require_tls,
            implicit_tls,
            hostname: pl_core::brand::BIN_NAME.into(),
            scratch,
        }))
    }
}

// ------------------------------------------------------------------------------ Limits

/// In-memory counters (failed logins per IP, messages per device).
#[derive(Default)]
pub struct Limits {
    failures: Mutex<HashMap<IpAddr, Vec<Instant>>>,
    messages: Mutex<HashMap<i64, Vec<Instant>>>,
}

impl Limits {
    fn prune(v: &mut Vec<Instant>, window: Duration) {
        v.retain(|t| t.elapsed() < window);
    }
    pub fn blocked(&self, ip: IpAddr) -> bool {
        let Ok(mut m) = self.failures.lock() else { return true };
        let v = m.entry(ip).or_default();
        Self::prune(v, FAIL_WINDOW);
        v.len() >= MAX_AUTH_FAILURES
    }
    pub fn fail(&self, ip: IpAddr) {
        if let Ok(mut m) = self.failures.lock() {
            if m.len() > 10_000 {
                m.retain(|_, v| v.iter().any(|t| t.elapsed() < FAIL_WINDOW));
            }
            m.entry(ip).or_default().push(Instant::now());
        }
    }
    /// Count a message; `false` if the device is over the limit.
    pub fn allow_message(&self, device: i64, per_minute: u32) -> bool {
        let Ok(mut m) = self.messages.lock() else { return false };
        let v = m.entry(device).or_default();
        Self::prune(v, Duration::from_secs(60));
        if v.len() >= per_minute as usize {
            return false;
        }
        v.push(Instant::now());
        true
    }
}

// ------------------------------------------------------------------------------ Server

/// Everything a session needs.
pub struct Context {
    pub cfg: SmtpConfig,
    pub devices: Arc<dyn DeviceStore>,
    pub intake: Arc<dyn Intake>,
    pub limits: Limits,
}

/// Listen until `stop`. Returns only after the stop (or an error on bind).
pub async fn serve(listener: TcpListener, ctx: Arc<Context>, mut stop: watch::Receiver<bool>) {
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        tokio::select! {
            r = listener.accept() => {
                let (stream, peer) = match r {
                    Ok(v) => v,
                    Err(e) => {
                        tracing::warn!(target: "paperrust.smtp", error = %e, "accept failed");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        continue;
                    }
                };
                let ctx = ctx.clone();
                let permit = slots.clone().try_acquire_owned();
                tokio::spawn(async move {
                    let mut stream = stream;
                    let Ok(_permit) = permit else {
                        let _ = stream.write_all(b"421 4.7.0 Too many connections, try again later\r\n").await;
                        return;
                    };
                    let ip = peer.ip();
                    // Direct TLS (SMTPS, e.g. port 465): handshake first, then SMTP
                    if ctx.cfg.implicit_tls
                        && let Some(tls) = ctx.cfg.tls.clone()
                    {
                        if !net::allowed(&ctx.cfg.allowed_ips, ip) || ctx.limits.blocked(ip) {
                            return;
                        }
                        let acceptor = tokio_rustls::TlsAcceptor::from(tls);
                        let Ok(Ok(s)) = tokio::time::timeout(COMMAND_TIMEOUT, acceptor.accept(stream)).await else { return };
                        let _ = tokio::time::timeout(SESSION_LIMIT, session(Box::new(s), ip, ctx, true)).await;
                        return;
                    }
                    let _ = tokio::time::timeout(SESSION_LIMIT, session(Box::new(stream), ip, ctx, false)).await;
                });
            }
            _ = stop.changed() => {
                if *stop.borrow() { break; }
            }
        }
    }
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

struct Conn {
    io: BufReader<Box<dyn Io>>,
}

enum Line {
    Text(Vec<u8>),
    TooLong,
    Eof,
}

impl Conn {
    async fn send(&mut self, s: &str) -> std::io::Result<()> {
        let w = self.io.get_mut();
        w.write_all(s.as_bytes()).await?;
        w.write_all(b"\r\n").await?;
        w.flush().await
    }

    /// One line (without CRLF), at most `max` bytes. Longer lines are read to the end.
    async fn line(&mut self, max: usize) -> std::io::Result<Line> {
        let mut out = Vec::new();
        let mut too_long = false;
        loop {
            let buf = self.io.fill_buf().await?;
            if buf.is_empty() {
                return Ok(if out.is_empty() && !too_long { Line::Eof } else if too_long { Line::TooLong } else { Line::Text(out) });
            }
            let (chunk, done) = match buf.iter().position(|b| *b == b'\n') {
                Some(i) => (&buf[..=i], true),
                None => (buf, false),
            };
            let n = chunk.len();
            if !too_long {
                if out.len() + n > max + 2 {
                    too_long = true;
                    out.clear();
                } else {
                    out.extend_from_slice(chunk);
                }
            }
            self.io.consume(n);
            if done {
                if too_long {
                    return Ok(Line::TooLong);
                }
                while out.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
                    out.pop();
                }
                return Ok(Line::Text(out));
            }
        }
    }

    async fn line_timeout(&mut self, max: usize) -> Option<Line> {
        match tokio::time::timeout(COMMAND_TIMEOUT, self.line(max)).await {
            Ok(Ok(l)) => Some(l),
            _ => None,
        }
    }
}

#[derive(Default)]
struct Tx {
    mail_from: bool,
    rcpt: usize,
}

struct Session {
    ip: IpAddr,
    helo: bool,
    tls: bool,
    device: Option<ActiveDevice>,
    authed: bool,
    tx: Tx,
    bad: u32,
}

fn b64(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD.decode(s.trim()).ok()
}

/// Address from `MAIL FROM:<…> …` / `RCPT TO:<…>` (text after the colon).
fn path_arg(rest: &str) -> (String, &str) {
    let rest = rest.trim_start();
    if let Some(r) = rest.strip_prefix('<')
        && let Some(end) = r.find('>')
    {
        return (r[..end].to_string(), &r[end + 1..]);
    }
    let mut it = rest.splitn(2, ' ');
    (it.next().unwrap_or("").to_string(), it.next().unwrap_or(""))
}

/// Detected file type of an attachment (by content).
pub fn sniff(data: &[u8]) -> Option<(&'static str, &'static str)> {
    if data.starts_with(b"%PDF-") || (data.len() > 1024 && data[..1024].windows(5).any(|w| w == b"%PDF-")) {
        Some(("application/pdf", "pdf"))
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(("image/jpeg", "jpg"))
    } else if data.starts_with(b"II*\0") || data.starts_with(b"MM\0*") {
        Some(("image/tiff", "tif"))
    } else {
        None
    }
}

/// Result of a message.
#[derive(Debug, PartialEq, Eq)]
pub enum Delivery {
    Queued(usize),
    NoAttachment,
    TempFail,
}

/// Hand the attachments of a message to the pipeline.
pub async fn deliver(ctx: &Context, device: &ActiveDevice, raw: &[u8]) -> Delivery {
    let mail = pl_parsers::mail::MailMessage::from_bytes(raw, 100);
    let mut queued = 0usize;
    let mut skipped = 0usize;
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    for (n, a) in mail.attachments().into_iter().enumerate() {
        let Some((_, ext)) = sniff(&a.payload) else {
            skipped += 1;
            continue;
        };
        let fallback = format!("scan-{stamp}-{}.{ext}", n + 1);
        let mut name = crate::intake::safe_name(&a.filename, &fallback);
        if !name.to_ascii_lowercase().ends_with(&format!(".{ext}")) && !(ext == "tif" && name.to_ascii_lowercase().ends_with(".tiff")) && !(ext == "jpg" && name.to_ascii_lowercase().ends_with(".jpeg")) {
            name = format!("{name}.{ext}");
        }
        let path = match crate::intake::spool(&ctx.cfg.scratch, &name, &a.payload) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(target: "paperrust.smtp", device = device.id, error = %e, "spooling failed");
                return if queued > 0 { Delivery::Queued(queued) } else { Delivery::TempFail };
            }
        };
        let doc = IntakeDoc { path: path.clone(), filename: name, title: None, owner_id: device.owner_id, source: pl_consumer::DocumentSource::ApiUpload };
        match ctx.intake.submit(doc).await {
            Ok(task) => {
                queued += 1;
                tracing::info!(target: "paperrust.smtp", device = device.id, task = %task, "attachment handed to consumer");
            }
            Err(e) => {
                crate::intake::discard(&path);
                tracing::error!(target: "paperrust.smtp", device = device.id, error = %e, "enqueue failed");
                return if queued > 0 { Delivery::Queued(queued) } else { Delivery::TempFail };
            }
        }
    }
    if skipped > 0 {
        tracing::info!(target: "paperrust.smtp", device = device.id, skipped, "attachments of other types ignored");
    }
    if queued == 0 { Delivery::NoAttachment } else { Delivery::Queued(queued) }
}

async fn lookup(ctx: &Context, candidate: &str) -> Option<ActiveDevice> {
    if !token::is_well_formed(candidate) {
        return None;
    }
    match ctx.devices.find_active(&token::hash(candidate)).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(target: "paperrust.smtp", error = %e, "device lookup failed");
            None
        }
    }
}

fn device_ip_ok(d: &ActiveDevice, ip: IpAddr) -> bool {
    match net::parse_list(&d.allowed_ips) {
        Ok(rules) => net::allowed(&rules, ip),
        // Broken list: better to block
        Err(_) => false,
    }
}

enum Step {
    Continue,
    Close,
    StartTls,
}

async fn session(stream: Box<dyn Io>, ip: IpAddr, ctx: Arc<Context>, tls: bool) {
    let mut conn = Conn { io: BufReader::new(stream) };
    if !net::allowed(&ctx.cfg.allowed_ips, ip) {
        tracing::info!(target: "paperrust.smtp", %ip, "connection from address outside the allow-list refused");
        let _ = conn.send("554 5.7.1 Access denied").await;
        return;
    }
    if ctx.limits.blocked(ip) {
        let _ = conn.send("421 4.7.0 Too many failed attempts, try again later").await;
        return;
    }
    if conn.send(&format!("220 {} ESMTP {} intake", ctx.cfg.hostname, pl_core::brand::PRODUCT_NAME)).await.is_err() {
        return;
    }
    let mut s = Session { ip, helo: false, tls, device: None, authed: false, tx: Tx::default(), bad: 0 };
    loop {
        match command(&mut conn, &mut s, &ctx).await {
            Step::Continue => {}
            Step::Close => return,
            Step::StartTls => {
                let Some(tls) = ctx.cfg.tls.clone() else { return };
                // Buffered data after STARTTLS = command injection (CVE-2011-0411 class)
                if !conn.io.buffer().is_empty() {
                    return;
                }
                let inner = conn.io.into_inner();
                let acceptor = tokio_rustls::TlsAcceptor::from(tls);
                let stream = match tokio::time::timeout(COMMAND_TIMEOUT, acceptor.accept(inner)).await {
                    Ok(Ok(s)) => s,
                    _ => return,
                };
                conn = Conn { io: BufReader::new(Box::new(stream)) };
                // RFC 3207: forget state
                s = Session { ip, helo: false, tls: true, device: None, authed: false, tx: Tx::default(), bad: s.bad };
            }
        }
    }
}

async fn auth_fail(conn: &mut Conn, s: &mut Session, ctx: &Context) -> Step {
    ctx.limits.fail(s.ip);
    tracing::info!(target: "paperrust.smtp", ip = %s.ip, "authentication failed");
    if ctx.limits.blocked(s.ip) {
        let _ = conn.send("421 4.7.0 Too many failed attempts, closing connection").await;
        return Step::Close;
    }
    if conn.send("535 5.7.8 Authentication credentials invalid").await.is_err() {
        return Step::Close;
    }
    Step::Continue
}

async fn auth_ok(conn: &mut Conn, s: &mut Session, ctx: &Context, d: ActiveDevice) -> Step {
    if !device_ip_ok(&d, s.ip) {
        tracing::info!(target: "paperrust.smtp", ip = %s.ip, device = d.id, "device used from address outside its allow-list");
        return auth_fail(conn, s, ctx).await;
    }
    tracing::info!(target: "paperrust.smtp", ip = %s.ip, device = d.id, name = %d.name, "device authenticated");
    s.device = Some(d);
    s.authed = true;
    if conn.send("235 2.7.0 Authentication successful").await.is_err() {
        return Step::Close;
    }
    Step::Continue
}

async fn read_auth_line(conn: &mut Conn) -> Option<String> {
    match conn.line_timeout(MAX_LINE).await? {
        Line::Text(t) => Some(String::from_utf8_lossy(&t).into_owned()),
        _ => None,
    }
}

async fn command(conn: &mut Conn, s: &mut Session, ctx: &Context) -> Step {
    let line = match conn.line_timeout(MAX_LINE).await {
        Some(Line::Text(t)) => String::from_utf8_lossy(&t).into_owned(),
        Some(Line::TooLong) => {
            s.bad += 1;
            return if conn.send("500 5.5.2 Line too long").await.is_err() { Step::Close } else { Step::Continue };
        }
        Some(Line::Eof) => return Step::Close,
        None => {
            let _ = conn.send("421 4.4.2 Timeout, closing connection").await;
            return Step::Close;
        }
    };
    if s.bad >= MAX_BAD_COMMANDS {
        let _ = conn.send("421 4.7.0 Too many errors, closing connection").await;
        return Step::Close;
    }
    let (verb, rest) = match line.split_once(' ') {
        Some((v, r)) => (v.to_ascii_uppercase(), r.to_string()),
        None => (line.trim().to_ascii_uppercase(), String::new()),
    };
    let tls_missing = ctx.cfg.require_tls && !s.tls;
    let reply = |c: &'static str| c.to_string();
    let r: String = match verb.as_str() {
        "EHLO" | "HELO" => {
            s.helo = true;
            s.tx = Tx::default();
            if verb == "HELO" {
                format!("250 {}", ctx.cfg.hostname)
            } else {
                let mut lines = vec![format!("250-{} greets {}", ctx.cfg.hostname, rest.trim()), format!("250-SIZE {}", ctx.cfg.max_bytes), "250-8BITMIME".into()];
                if ctx.cfg.tls.is_some() && !s.tls {
                    lines.push("250-STARTTLS".into());
                }
                if !tls_missing {
                    lines.push("250-AUTH PLAIN LOGIN".into());
                }
                lines.push("250 ENHANCEDSTATUSCODES".into());
                lines.join("\r\n")
            }
        }
        "STARTTLS" => {
            if ctx.cfg.tls.is_none() {
                reply("502 5.5.1 STARTTLS not available")
            } else if s.tls {
                reply("503 5.5.1 TLS already active")
            } else {
                if conn.send("220 2.0.0 Ready to start TLS").await.is_err() {
                    return Step::Close;
                }
                return Step::StartTls;
            }
        }
        "AUTH" => {
            if !s.helo {
                reply("503 5.5.1 Send EHLO first")
            } else if s.authed {
                reply("503 5.5.1 Already authenticated")
            } else if tls_missing {
                reply("530 5.7.0 Must issue a STARTTLS command first")
            } else if s.tx.mail_from {
                reply("503 5.5.1 AUTH not allowed during a mail transaction")
            } else {
                let mut parts = rest.split_whitespace();
                let mech = parts.next().unwrap_or("").to_ascii_uppercase();
                let initial = parts.next().map(str::to_string);
                match mech.as_str() {
                    "PLAIN" => {
                        let data = match initial {
                            Some(i) if i != "=" => Some(i),
                            Some(_) => Some(String::new()),
                            None => {
                                if conn.send("334 ").await.is_err() {
                                    return Step::Close;
                                }
                                read_auth_line(conn).await
                            }
                        };
                        let Some(data) = data else { return Step::Close };
                        if data.trim() == "*" {
                            reply("501 5.0.0 Authentication cancelled")
                        } else {
                            let pass = b64(&data).and_then(|raw| {
                                let mut f = raw.split(|b| *b == 0);
                                let _authz = f.next();
                                let _user = f.next();
                                f.next().map(|p| String::from_utf8_lossy(p).into_owned())
                            });
                            let dev = match pass {
                                Some(p) => lookup(ctx, &p).await,
                                None => None,
                            };
                            return match dev {
                                Some(d) => auth_ok(conn, s, ctx, d).await,
                                None => auth_fail(conn, s, ctx).await,
                            };
                        }
                    }
                    "LOGIN" => {
                        let user = match initial {
                            Some(i) => b64(&i),
                            None => {
                                if conn.send("334 VXNlcm5hbWU6").await.is_err() {
                                    return Step::Close;
                                }
                                match read_auth_line(conn).await {
                                    Some(l) if l.trim() == "*" => return if conn.send("501 5.0.0 Authentication cancelled").await.is_err() { Step::Close } else { Step::Continue },
                                    Some(l) => b64(&l),
                                    None => return Step::Close,
                                }
                            }
                        };
                        if conn.send("334 UGFzc3dvcmQ6").await.is_err() {
                            return Step::Close;
                        }
                        let pass = match read_auth_line(conn).await {
                            Some(l) if l.trim() == "*" => return if conn.send("501 5.0.0 Authentication cancelled").await.is_err() { Step::Close } else { Step::Continue },
                            Some(l) => b64(&l),
                            None => return Step::Close,
                        };
                        // token in the password; some devices only allow one field → also the user name
                        let mut dev = None;
                        for cand in [pass, user].into_iter().flatten() {
                            dev = lookup(ctx, &String::from_utf8_lossy(&cand)).await;
                            if dev.is_some() {
                                break;
                            }
                        }
                        return match dev {
                            Some(d) => auth_ok(conn, s, ctx, d).await,
                            None => auth_fail(conn, s, ctx).await,
                        };
                    }
                    _ => reply("504 5.5.4 Unrecognized authentication type"),
                }
            }
        }
        "MAIL" => {
            let up = rest.to_ascii_uppercase();
            if !s.helo {
                reply("503 5.5.1 Send EHLO first")
            } else if tls_missing {
                reply("530 5.7.0 Must issue a STARTTLS command first")
            } else if s.tx.mail_from {
                reply("503 5.5.1 Nested MAIL command")
            } else if !up.starts_with("FROM:") {
                reply("501 5.5.4 Syntax: MAIL FROM:<address>")
            } else {
                let (_, params) = path_arg(&rest[5..]);
                let size = params.split_whitespace().find_map(|p| p.to_ascii_uppercase().strip_prefix("SIZE=").and_then(|v| v.parse::<u64>().ok()));
                if size.is_some_and(|n| n > ctx.cfg.max_bytes as u64) {
                    reply("552 5.3.4 Message size exceeds fixed limit")
                } else {
                    s.tx = Tx { mail_from: true, rcpt: 0 };
                    if !s.authed {
                        s.device = None;
                    }
                    reply("250 2.1.0 OK")
                }
            }
        }
        "RCPT" => {
            if !s.tx.mail_from {
                reply("503 5.5.1 Need MAIL command first")
            } else if !rest.to_ascii_uppercase().starts_with("TO:") {
                reply("501 5.5.4 Syntax: RCPT TO:<address>")
            } else if s.tx.rcpt >= MAX_RECIPIENTS {
                reply("452 4.5.3 Too many recipients")
            } else {
                let (addr, _) = path_arg(&rest[3..]);
                if s.authed {
                    // Authenticated: every address is just a name for this intake (no relay)
                    s.tx.rcpt += 1;
                    reply("250 2.1.5 OK")
                } else {
                    match token::from_address(&addr) {
                        Some(t) => match lookup(ctx, &t).await {
                            Some(d) if device_ip_ok(&d, s.ip) => {
                                if s.device.as_ref().is_some_and(|x| x.id != d.id) {
                                    reply("452 4.5.3 One device per message")
                                } else {
                                    tracing::info!(target: "paperrust.smtp", ip = %s.ip, device = d.id, name = %d.name, "device identified by recipient address");
                                    s.device = Some(d);
                                    s.tx.rcpt += 1;
                                    reply("250 2.1.5 OK")
                                }
                            }
                            _ => {
                                return auth_fail_rcpt(conn, s, ctx).await;
                            }
                        },
                        None => reply("550 5.7.1 Relaying denied"),
                    }
                }
            }
        }
        "DATA" => {
            if !s.tx.mail_from || s.tx.rcpt == 0 {
                reply("503 5.5.1 Need RCPT command first")
            } else if let Some(device) = s.device.clone() {
                if conn.send("354 End data with <CR><LF>.<CR><LF>").await.is_err() {
                    return Step::Close;
                }
                let r = receive(conn, s, ctx, &device).await;
                s.tx = Tx::default();
                if !s.authed {
                    s.device = None;
                }
                match r {
                    Some(r) => r,
                    None => return Step::Close,
                }
            } else {
                reply("503 5.5.1 Need RCPT command first")
            }
        }
        "RSET" => {
            s.tx = Tx::default();
            if !s.authed {
                s.device = None;
            }
            reply("250 2.0.0 OK")
        }
        "NOOP" => reply("250 2.0.0 OK"),
        "VRFY" => reply("252 2.5.2 Cannot verify"),
        "QUIT" => {
            let _ = conn.send("221 2.0.0 Bye").await;
            return Step::Close;
        }
        _ => {
            s.bad += 1;
            reply("500 5.5.2 Command not recognized")
        }
    };
    if conn.send(&r).await.is_err() {
        return Step::Close;
    }
    Step::Continue
}

async fn auth_fail_rcpt(conn: &mut Conn, s: &mut Session, ctx: &Context) -> Step {
    ctx.limits.fail(s.ip);
    tracing::info!(target: "paperrust.smtp", ip = %s.ip, "recipient with unknown or revoked token");
    if ctx.limits.blocked(s.ip) {
        let _ = conn.send("421 4.7.0 Too many failed attempts, closing connection").await;
        return Step::Close;
    }
    if conn.send("550 5.7.1 Relaying denied").await.is_err() {
        return Step::Close;
    }
    Step::Continue
}

/// Read DATA (dot-unstuffing), check limits, deliver. `None` = connection closed.
async fn receive(conn: &mut Conn, s: &Session, ctx: &Context, device: &ActiveDevice) -> Option<String> {
    let mut raw: Vec<u8> = Vec::new();
    let mut too_big = false;
    loop {
        // Lines up to 1 MiB (devices send base64 with 76 characters; 8BITMIME can be longer)
        let line = match conn.line_timeout(1024 * 1024).await? {
            Line::Text(t) => t,
            Line::TooLong => {
                too_big = true;
                continue;
            }
            Line::Eof => return None,
        };
        if line == b"." {
            break;
        }
        if too_big {
            continue;
        }
        let l = if line.starts_with(b".") { &line[1..] } else { &line[..] };
        if raw.len() + l.len() + 2 > ctx.cfg.max_bytes {
            too_big = true;
            raw = Vec::new();
            continue;
        }
        raw.extend_from_slice(l);
        raw.extend_from_slice(b"\r\n");
    }
    if too_big {
        tracing::info!(target: "paperrust.smtp", device = device.id, "message too large, rejected");
        return Some("552 5.3.4 Message size exceeds fixed limit".into());
    }
    if !ctx.limits.allow_message(device.id, ctx.cfg.rate_per_minute) {
        tracing::info!(target: "paperrust.smtp", device = device.id, "rate limit reached");
        return Some("451 4.7.1 Too many messages, try again later".into());
    }
    Some(match deliver(ctx, device, &raw).await {
        Delivery::Queued(n) => {
            ctx.devices.record_use(device.id, &s.ip.to_string(), n as i64).await;
            format!("250 2.0.0 OK: {n} document(s) queued")
        }
        Delivery::NoAttachment => "554 5.6.1 No PDF, JPEG or TIFF attachment found".into(),
        Delivery::TempFail => "451 4.3.0 Temporary failure, try again later".into(),
    })
}

/// Start the intake if enabled (for pl-server). Runs until `stop`.
pub async fn run_if_enabled(state: pl_core::AppState, stop: watch::Receiver<bool>) {
    let scratch = state.config().scratch_dir.clone();
    let cfg = match SmtpConfig::from_lookup(pl_core::config::var, scratch) {
        Ok(Some(c)) => c,
        Ok(None) => return,
        Err(e) => {
            tracing::error!(target: "paperrust.smtp", "SMTP intake not started: {e}");
            return;
        }
    };
    let Some(intake) = crate::intake::from_state(&state) else {
        tracing::error!(target: "paperrust.smtp", "SMTP intake not started: no task queue attached");
        return;
    };
    let listener = match TcpListener::bind(cfg.listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(target: "paperrust.smtp", listen = %cfg.listen, "SMTP intake not started: cannot bind: {e}");
            return;
        }
    };
    if cfg.tls.is_none() {
        tracing::warn!(target: "paperrust.smtp", listen = %cfg.listen, "SMTP intake listening WITHOUT TLS – use only in a trusted LAN (docs/user/scanner.md)");
    } else {
        tracing::info!(target: "paperrust.smtp", listen = %cfg.listen, require_tls = cfg.require_tls, implicit_tls = cfg.implicit_tls, "SMTP intake listening with TLS");
    }
    let ctx = Arc::new(Context { cfg, devices: Arc::new(crate::store::PgDevices(state.db().clone())), intake, limits: Limits::default() });
    serve(listener, ctx, stop).await;
}

/// State for the admin page.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Info {
    pub enabled: bool,
    pub listen: Option<String>,
    pub port: Option<u16>,
    pub tls: bool,
    pub require_tls: bool,
    pub implicit_tls: bool,
    pub max_mb: Option<u64>,
    pub error: Option<String>,
}

pub fn info() -> Info {
    match SmtpConfig::from_lookup(pl_core::config::var, PathBuf::new()) {
        Ok(Some(c)) => Info {
            enabled: true,
            listen: Some(c.listen.to_string()),
            port: Some(c.listen.port()),
            tls: c.tls.is_some(),
            require_tls: c.require_tls,
            implicit_tls: c.implicit_tls,
            max_mb: Some((c.max_bytes / (1024 * 1024)) as u64),
            error: None,
        },
        Ok(None) => Info { enabled: false, listen: None, port: None, tls: false, require_tls: false, implicit_tls: false, max_mb: None, error: None },
        Err(e) => Info { enabled: true, listen: None, port: None, tls: false, require_tls: false, implicit_tls: false, max_mb: None, error: Some(e) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_from_lookup() {
        let get = |m: &'static [(&'static str, &'static str)]| move |k: &str| m.iter().find(|(a, _)| *a == k).map(|(_, v)| v.to_string());
        assert!(SmtpConfig::from_lookup(get(&[]), PathBuf::new()).unwrap().is_none());
        let c = SmtpConfig::from_lookup(get(&[(SETTING_ENABLED, "on")]), PathBuf::new()).unwrap().unwrap();
        assert_eq!(c.listen.to_string(), DEFAULT_LISTEN);
        assert_eq!(c.max_bytes, 50 * 1024 * 1024);
        assert_eq!(c.rate_per_minute, 30);
        assert!(SmtpConfig::from_lookup(get(&[(SETTING_ENABLED, "on"), (SETTING_LISTEN, "nope")]), PathBuf::new()).is_err());
        assert!(SmtpConfig::from_lookup(get(&[(SETTING_ENABLED, "on"), (SETTING_MAX_MB, "0")]), PathBuf::new()).is_err());
        assert!(SmtpConfig::from_lookup(get(&[(SETTING_ENABLED, "on"), (SETTING_ALLOWED_IPS, "1.2.3.400")]), PathBuf::new()).is_err());
        assert!(SmtpConfig::from_lookup(get(&[(SETTING_ENABLED, "on"), (SETTING_TLS_CERT, "x.pem")]), PathBuf::new()).is_err());
        assert!(SmtpConfig::from_lookup(get(&[(SETTING_ENABLED, "on"), (SETTING_REQUIRE_TLS, "on")]), PathBuf::new()).is_err());
        assert!(SmtpConfig::from_lookup(get(&[(SETTING_ENABLED, "on"), (SETTING_IMPLICIT_TLS, "on")]), PathBuf::new()).is_err());
    }

    #[test]
    fn sniffing() {
        assert_eq!(sniff(b"%PDF-1.7 ...").map(|s| s.1), Some("pdf"));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]).map(|s| s.1), Some("jpg"));
        assert_eq!(sniff(b"II*\0rest").map(|s| s.1), Some("tif"));
        assert_eq!(sniff(b"MM\0*rest").map(|s| s.1), Some("tif"));
        assert_eq!(sniff(b"PK\x03\x04"), None);
        assert_eq!(sniff(b"<html>"), None);
    }

    #[test]
    fn limits() {
        let l = Limits::default();
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        for _ in 0..MAX_AUTH_FAILURES - 1 {
            l.fail(ip);
        }
        assert!(!l.blocked(ip));
        l.fail(ip);
        assert!(l.blocked(ip));
        assert!(!l.blocked("10.0.0.2".parse().unwrap()));
        assert!(l.allow_message(1, 2) && l.allow_message(1, 2) && !l.allow_message(1, 2));
        assert!(l.allow_message(2, 2));
    }

    #[test]
    fn paths() {
        assert_eq!(path_arg("<a@b> SIZE=10").0, "a@b");
        assert_eq!(path_arg("<a@b> SIZE=10").1.trim(), "SIZE=10");
        assert_eq!(path_arg("a@b").0, "a@b");
        assert_eq!(path_arg("<>").0, "");
    }
}
