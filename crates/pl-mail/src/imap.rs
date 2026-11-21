//! IMAP connection test for `POST /api/mail_accounts/test/`.
//!
//! upstream: src/paperless_mail/mail.py@v3.4.2 (get_mailbox, PinnedIMAP4, Pinned*MailBox,
//! mailbox_login, MailError), src/paperless/network.py@v3.4.2 (is_public_ip, resolve_hostname_ips),
//! imap_tools/mailbox.py@1.15.1 (BaseMailBox.login/login_utf8/xoauth2/logout, MailBoxStartTls),
//! imap_tools/folder.py (MailBoxFolderManager.set), Python 3.12 imaplib (IMAP4._connect,
//! _get_capabilities, _command_complete, starttls, authenticate, logout).
//!
//! Protocol parser: `imap-proto`. The flow (command sequence, error classes) is re-built by hand
//! here, so success/400/500 arise exactly like upstream:
//! * `TestError::Mail`  → upstream `MailError` → 400 "Unable to connect to server"
//! * `TestError::Other` → upstream other exception → 500

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::{self, pki_types::ServerName};

use imap_proto::{Capability, Response, ResponseCode};

/// Own copy of `imap_proto::Status` (not `Copy` there).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    No,
    Bad,
    PreAuth,
    Bye,
}

fn st(s: &imap_proto::Status) -> Status {
    match s {
        imap_proto::Status::Ok => Status::Ok,
        imap_proto::Status::No => Status::No,
        imap_proto::Status::Bad => Status::Bad,
        imap_proto::Status::PreAuth => Status::PreAuth,
        imap_proto::Status::Bye => Status::Bye,
    }
}

pub const SECURITY_NONE: i16 = 1;
pub const SECURITY_SSL: i16 = 2;
pub const SECURITY_STARTTLS: i16 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestError {
    /// `MailError` → 400
    Mail(String),
    /// other exception → 500
    Other(String),
}

/// Account data for the test (like `MailAccount(**validated_data)`).
#[derive(Debug, Clone)]
pub struct TestAccount {
    pub imap_server: String,
    pub imap_port: u16,
    pub imap_security: i16,
    pub username: String,
    pub password: String,
    pub is_token: bool,
}

#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub allow_internal_hosts: bool,
    pub certificate_file: Option<std::path::PathBuf>,
    pub timeout: Duration,
    /// Tests only: additional trusted certificates (DER).
    pub extra_roots: Vec<Vec<u8>>,
}

// ---------------------------------------------------------------------------------------------
// paperless.network

fn v4_in(ip: std::net::Ipv4Addr, net: [u8; 4], bits: u32) -> bool {
    let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
    (u32::from(ip) & mask) == (u32::from(std::net::Ipv4Addr::from(net)) & mask)
}

fn v6_in(ip: std::net::Ipv6Addr, net: [u16; 8], bits: u32) -> bool {
    let mask = if bits == 0 { 0 } else { u128::MAX << (128 - bits) };
    (u128::from(ip) & mask) == (u128::from(std::net::Ipv6Addr::from(net)) & mask)
}

fn v4_is_private(ip: std::net::Ipv4Addr) -> bool {
    // Python 3.12 ipaddress._private_networks (IPv4)
    const NETS: &[([u8; 4], u32)] = &[
        ([0, 0, 0, 0], 8),
        ([10, 0, 0, 0], 8),
        ([127, 0, 0, 0], 8),
        ([169, 254, 0, 0], 16),
        ([172, 16, 0, 0], 12),
        ([192, 0, 0, 0], 24),
        ([192, 0, 2, 0], 24),
        ([192, 168, 0, 0], 16),
        ([198, 18, 0, 0], 15),
        ([198, 51, 100, 0], 24),
        ([203, 0, 113, 0], 24),
        ([240, 0, 0, 0], 4),
        ([255, 255, 255, 255], 32),
    ];
    // Exceptions (globally reachable): 192.0.0.9/32, 192.0.0.10/32
    if ip == std::net::Ipv4Addr::new(192, 0, 0, 9) || ip == std::net::Ipv4Addr::new(192, 0, 0, 10) {
        return false;
    }
    NETS.iter().any(|(n, b)| v4_in(ip, *n, *b))
}

/// `paperless.network.is_public_ip`.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let non_public = v4_is_private(v4)
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_unspecified()
                || v4_in(v4, [100, 64, 0, 0], 10);
            !non_public
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                // Python ≥ 3.12.4 evaluates IPv4-mapped by the IPv4 address
                if !is_public_ip(IpAddr::V4(mapped)) {
                    return false;
                }
            }
            const NETS: &[([u16; 8], u32)] = &[
                ([0, 0, 0, 0, 0, 0, 0, 1], 128),
                ([0, 0, 0, 0, 0, 0, 0, 0], 128),
                ([0x64, 0xff9b, 1, 0, 0, 0, 0, 0], 48),
                ([0x100, 0, 0, 0, 0, 0, 0, 0], 64),
                ([0x2001, 0, 0, 0, 0, 0, 0, 0], 23),
                ([0x2001, 0xdb8, 0, 0, 0, 0, 0, 0], 32),
                ([0x2001, 0x10, 0, 0, 0, 0, 0, 0], 28),
                ([0xfc00, 0, 0, 0, 0, 0, 0, 0], 7),
                ([0xfe80, 0, 0, 0, 0, 0, 0, 0], 10),
                ([0xff00, 0, 0, 0, 0, 0, 0, 0], 8),
                ([0x64, 0xff9b, 0, 0, 0, 0, 0, 0], 96),
            ];
            !NETS.iter().any(|(n, b)| v6_in(v6, *n, *b))
        }
    }
}

/// `resolve_hostname_ips` (getaddrinfo, all families).
pub async fn resolve_hostname_ips(host: &str) -> Result<Vec<IpAddr>, String> {
    let addrs = tokio::net::lookup_host((host, 0)).await.map_err(|_| format!("Could not resolve hostname: {host}"))?;
    let ips: Vec<IpAddr> = addrs.map(|a| a.ip()).collect();
    if ips.is_empty() {
        return Err(format!("Could not resolve hostname: {host}"));
    }
    Ok(ips)
}

// ---------------------------------------------------------------------------------------------
// Connection

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    NonAuth,
    Auth,
    Selected,
}

struct Conn {
    stream: Box<dyn Stream>,
    buf: Vec<u8>,
    tag_no: u32,
    capabilities: Vec<String>,
    /// `'BYE' in self.untagged_responses`
    bye: bool,
    /// Response code READ-ONLY seen
    read_only: bool,
    state: State,
    timeout: Duration,
}

/// Result of a command.
struct Done {
    status: Status,
    /// Untagged CAPABILITY data (last), if received.
    capabilities: Option<Vec<String>>,
}

fn cap_strings(caps: &[Capability<'_>]) -> Vec<String> {
    caps.iter()
        .map(|c| match c {
            Capability::Imap4rev1 => "IMAP4REV1".to_string(),
            Capability::Auth(a) => format!("AUTH={}", a.to_uppercase()),
            Capability::Atom(a) => a.to_uppercase(),
        })
        .collect()
}

/// A parsed response element (self-contained, without lifetime).
enum Item {
    Continue,
    Tagged { tag: String, status: Status, read_only: bool },
    Untagged { status: Option<Status>, read_only: bool, caps: Option<Vec<String>> },
}

fn to_item(r: Response<'_>) -> Item {
    match r {
        Response::Continue { .. } => Item::Continue,
        Response::Done { tag, status, code, .. } => {
            Item::Tagged { tag: tag.0, status: st(&status), read_only: matches!(code, Some(ResponseCode::ReadOnly)) }
        }
        Response::Data { status, code, .. } => {
            let caps = match &code {
                Some(ResponseCode::Capabilities(c)) => Some(cap_strings(c)),
                _ => None,
            };
            Item::Untagged { status: Some(st(&status)), read_only: matches!(code, Some(ResponseCode::ReadOnly)), caps }
        }
        Response::Capabilities(c) => Item::Untagged { status: None, read_only: false, caps: Some(cap_strings(&c)) },
        _ => Item::Untagged { status: None, read_only: false, caps: None },
    }
}

/// Fallback for lines that imap-proto does not understand (imaplib is tolerant).
fn fallback_item(line: &str) -> Item {
    let mut it = line.splitn(3, ' ');
    let first = it.next().unwrap_or("");
    let second = it.next().unwrap_or("").to_ascii_uppercase();
    let status = match second.as_str() {
        "OK" => Some(Status::Ok),
        "NO" => Some(Status::No),
        "BAD" => Some(Status::Bad),
        "BYE" => Some(Status::Bye),
        "PREAUTH" => Some(Status::PreAuth),
        _ => None,
    };
    if first == "+" {
        return Item::Continue;
    }
    if first == "*" {
        return Item::Untagged { status, read_only: false, caps: None };
    }
    match status {
        Some(s) => Item::Tagged { tag: first.to_string(), status: s, read_only: false },
        None => Item::Untagged { status: None, read_only: false, caps: None },
    }
}

impl Conn {
    /// Reads a complete response element.
    async fn read_item(&mut self) -> Result<Item, String> {
        loop {
            if !self.buf.is_empty() {
                let parsed = match imap_proto::parser::parse_response(&self.buf) {
                    Ok((rest, resp)) => {
                        let consumed = self.buf.len() - rest.len();
                        Some((to_item(resp), consumed))
                    }
                    Err(e) if e.is_incomplete() => None,
                    Err(_) => {
                        // whole line present? → continue tolerantly
                        self.buf.windows(2).position(|w| w == b"\r\n").map(|p| {
                            let line = String::from_utf8_lossy(&self.buf[..p]).into_owned();
                            (fallback_item(&line), p + 2)
                        })
                    }
                };
                if let Some((item, consumed)) = parsed {
                    self.buf.drain(..consumed);
                    match &item {
                        Item::Untagged { status, read_only, .. } => {
                            if *read_only {
                                self.read_only = true;
                            }
                            if *status == Some(Status::Bye) {
                                self.bye = true;
                            }
                        }
                        Item::Tagged { read_only, .. } => {
                            if *read_only {
                                self.read_only = true;
                            }
                        }
                        Item::Continue => {}
                    }
                    return Ok(item);
                }
            }
            let mut chunk = [0u8; 8192];
            let n = {
                let stream = &mut self.stream;
                match tokio::time::timeout(self.timeout, stream.read(&mut chunk)).await {
                    Ok(Ok(n)) => n,
                    Ok(Err(e)) => return Err(format!("socket error: {e}")),
                    Err(_) => return Err("socket timeout".into()),
                }
            };
            if n == 0 {
                return Err("socket error: EOF".into());
            }
            if self.buf.len() + n > 20 * 1024 * 1024 {
                return Err("got more than 20Mb bytes".into());
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }
    }

    async fn write_line(&mut self, line: &[u8]) -> Result<(), String> {
        let mut data = line.to_vec();
        data.extend_from_slice(b"\r\n");
        let stream = &mut self.stream;
        let fut = async move {
            stream.write_all(&data).await?;
            stream.flush().await
        };
        match tokio::time::timeout(self.timeout, fut).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(format!("socket error: {e}")),
            Err(_) => Err("socket timeout".into()),
        }
    }

    fn next_tag(&mut self) -> String {
        self.tag_no += 1;
        format!("PLRS{}", self.tag_no)
    }

    /// `_simple_command` + `_command_complete` (BAD → error, BYE → abort).
    /// `auth`: replies to continuations (AUTHENTICATE).
    async fn command(&mut self, cmd: &str, args: &[&[u8]], auth: Option<&[u8]>) -> Result<Done, String> {
        if self.bye {
            return Err("server closed connection".into());
        }
        let tag = self.next_tag();
        let mut line = format!("{tag} {cmd}").into_bytes();
        for a in args {
            line.push(b' ');
            line.extend_from_slice(a);
        }
        self.write_line(&line).await?;
        let mut caps = None;
        loop {
            match self.read_item().await? {
                Item::Continue => match auth {
                    Some(resp) => self.write_line(resp).await?,
                    None => return Err("unexpected continuation".into()),
                },
                Item::Tagged { tag: t, status, .. } if t == tag => {
                    if self.bye {
                        return Err("server closed connection".into());
                    }
                    if status == Status::Bad {
                        return Err(format!("{cmd} command error: BAD"));
                    }
                    return Ok(Done { status, capabilities: caps });
                }
                Item::Tagged { .. } => {}
                Item::Untagged { caps: c, .. } => {
                    if c.is_some() {
                        caps = c;
                    }
                    if self.bye {
                        return Err("server closed connection".into());
                    }
                }
            }
        }
    }

    /// `_get_capabilities` + version check.
    async fn load_capabilities(&mut self) -> Result<(), String> {
        let done = self.command("CAPABILITY", &[], None).await?;
        let Some(caps) = done.capabilities else {
            return Err("no CAPABILITY response from server".into());
        };
        self.capabilities = caps;
        if !self.capabilities.iter().any(|c| c == "IMAP4REV1" || c == "IMAP4") {
            return Err("server not IMAP4 compliant".into());
        }
        Ok(())
    }

    /// Read the greeting (`IMAP4._connect`).
    async fn greeting(&mut self) -> Result<(), String> {
        loop {
            match self.read_item().await? {
                Item::Untagged { status: Some(Status::PreAuth), .. } => {
                    self.state = State::Auth;
                    return Ok(());
                }
                Item::Untagged { status: Some(Status::Ok), .. } => {
                    self.state = State::NonAuth;
                    return Ok(());
                }
                Item::Untagged { status: None, .. } => continue,
                _ => return Err("unexpected greeting".into()),
            }
        }
    }

    /// `IMAP4.logout` + imap_tools `check_command_status(..., expected='BYE')`.
    async fn logout(&mut self) -> bool {
        if self.bye {
            return true;
        }
        let tag = self.next_tag();
        if self.write_line(format!("{tag} LOGOUT").as_bytes()).await.is_err() {
            return self.bye;
        }
        loop {
            match self.read_item().await {
                Ok(Item::Tagged { tag: t, .. }) if t == tag => return self.bye,
                Ok(_) => {
                    if self.bye {
                        return true;
                    }
                }
                Err(_) => return self.bye,
            }
        }
    }
}

fn root_store(opts: &ConnectOptions) -> Result<rustls::RootCertStore, String> {
    let mut roots = rustls::RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    for c in native.certs {
        let _ = roots.add(c);
    }
    if let Some(path) = &opts.certificate_file {
        let data = std::fs::read(path).map_err(|e| format!("cafile: {e}"))?;
        for der in pem_certs(&data) {
            roots.add(rustls::pki_types::CertificateDer::from(der)).map_err(|e| format!("cafile: {e}"))?;
        }
    }
    for der in &opts.extra_roots {
        roots.add(rustls::pki_types::CertificateDer::from(der.clone())).map_err(|e| format!("extra root: {e}"))?;
    }
    Ok(roots)
}

/// Minimal PEM reader (`CERTIFICATE` blocks only).
fn pem_certs(data: &[u8]) -> Vec<Vec<u8>> {
    let text = String::from_utf8_lossy(data);
    let mut out = Vec::new();
    let mut cur: Option<String> = None;
    for line in text.lines() {
        let l = line.trim();
        if l == "-----BEGIN CERTIFICATE-----" {
            cur = Some(String::new());
        } else if l == "-----END CERTIFICATE-----" {
            if let Some(b) = cur.take()
                && let Ok(der) = base64::engine::general_purpose::STANDARD.decode(b)
            {
                out.push(der);
            }
        } else if let Some(b) = cur.as_mut() {
            b.push_str(l);
        }
    }
    out
}

async fn tls_wrap(stream: Box<dyn Stream>, host: &str, opts: &ConnectOptions) -> Result<Box<dyn Stream>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("tls config: {e}"))?
        .with_root_certificates(root_store(opts)?)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(config));
    let name = ServerName::try_from(host.to_string()).map_err(|e| format!("tls name: {e}"))?;
    match tokio::time::timeout(opts.timeout, connector.connect(name, stream)).await {
        Ok(Ok(s)) => Ok(Box::new(s)),
        Ok(Err(e)) => Err(format!("ssl error: {e}")),
        Err(_) => Err("socket timeout".into()),
    }
}

async fn tcp_connect(host: &str, port: u16, pinned: &Option<Vec<IpAddr>>, timeout: Duration) -> Result<TcpStream, String> {
    let fut = async {
        match pinned {
            Some(ips) => {
                let mut last = None;
                for ip in ips {
                    match TcpStream::connect((*ip, port)).await {
                        Ok(s) => return Ok(s),
                        Err(e) => last = Some(e),
                    }
                }
                Err(last.unwrap_or_else(|| std::io::Error::other(format!("Could not connect to {host}"))))
            }
            None => TcpStream::connect((host, port)).await,
        }
    };
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(s)) => Ok(s),
        Ok(Err(e)) => Err(format!("connect error: {e}")),
        Err(_) => Err("connect timeout".into()),
    }
}

/// `get_mailbox(server, port, security)` → connected client after greeting/CAPABILITY/STARTTLS.
async fn get_mailbox(acc: &TestAccount, opts: &ConnectOptions) -> Result<Conn, TestError> {
    let host = acc.imap_server.as_str();
    let mut pinned = None;
    if !opts.allow_internal_hosts {
        let ips = resolve_hostname_ips(host).await.map_err(TestError::Mail)?;
        if ips.iter().any(|ip| !is_public_ip(*ip)) {
            return Err(TestError::Mail(format!("Connection blocked: {host} resolves to a non-public address")));
        }
        pinned = Some(ips);
    }
    if acc.imap_security == SECURITY_STARTTLS && acc.imap_port == 993 {
        return Err(TestError::Other("Port 993 requires IMAP4_SSL. Use MailBox class for SSL/TLS connection.".into()));
    }
    let tcp = tcp_connect(host, acc.imap_port, &pinned, opts.timeout).await.map_err(TestError::Other)?;
    let mut stream: Box<dyn Stream> = Box::new(tcp);
    if acc.imap_security == SECURITY_SSL {
        stream = tls_wrap(stream, host, opts).await.map_err(TestError::Other)?;
    } else if acc.imap_security != SECURITY_NONE && acc.imap_security != SECURITY_STARTTLS {
        return Err(TestError::Other("Unknown IMAP security".into()));
    }
    let mut conn = Conn {
        stream,
        buf: Vec::new(),
        tag_no: 0,
        capabilities: Vec::new(),
        bye: false,
        read_only: false,
        state: State::NonAuth,
        timeout: opts.timeout,
    };
    conn.greeting().await.map_err(TestError::Other)?;
    conn.load_capabilities().await.map_err(TestError::Other)?;
    if acc.imap_security == SECURITY_STARTTLS {
        if !conn.capabilities.iter().any(|c| c == "STARTTLS") {
            return Err(TestError::Other("TLS not supported by server".into()));
        }
        let done = conn.command("STARTTLS", &[], None).await.map_err(TestError::Other)?;
        if done.status != Status::Ok {
            return Err(TestError::Other("Couldn't establish TLS session".into()));
        }
        let plain = std::mem::replace(&mut conn.stream, Box::new(empty::EmptyDuplex));
        conn.stream = tls_wrap(plain, host, opts).await.map_err(TestError::Other)?;
        conn.buf.clear();
        conn.load_capabilities().await.map_err(TestError::Other)?;
    }
    Ok(conn)
}

/// imaplib `_quote` + ASCII encoding of the arguments.
fn quote_ascii(s: &str) -> Result<Vec<u8>, String> {
    if !s.is_ascii() {
        return Err("'ascii' codec can't encode characters".into());
    }
    if s.contains(['\r', '\n']) {
        return Err("line breaks in argument".into());
    }
    Ok(format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")).into_bytes())
}

/// `mailbox_login(mailbox, account)` – all errors → MailError.
async fn mailbox_login(conn: &mut Conn, acc: &TestAccount) -> Result<(), String> {
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b).into_bytes();
    let status = if acc.is_token {
        if conn.state != State::NonAuth {
            return Err("command AUTHENTICATE illegal in state AUTH".into());
        }
        let auth = format!("user={}\x01auth=Bearer {}\x01\x01", acc.username, acc.password);
        let resp = b64(auth.as_bytes());
        let d = conn.command("AUTHENTICATE", &[b"XOAUTH2"], Some(&resp)).await?;
        if d.status != Status::Ok {
            return Err("AUTHENTICATE failed".into());
        }
        d.status
    } else if acc.password.is_ascii() {
        if conn.state != State::NonAuth {
            return Err("command LOGIN illegal in state AUTH".into());
        }
        let u = quote_ascii(&acc.username)?;
        let p = quote_ascii(&acc.password)?;
        conn.command("LOGIN", &[&u, &p], None).await?.status
    } else {
        if conn.state != State::NonAuth {
            return Err("command AUTHENTICATE illegal in state AUTH".into());
        }
        let mut raw = vec![0u8];
        raw.extend_from_slice(acc.username.as_bytes());
        raw.push(0);
        raw.extend_from_slice(acc.password.as_bytes());
        let resp = b64(&raw);
        let d = conn.command("AUTHENTICATE", &[b"PLAIN"], Some(&resp)).await?;
        if d.status != Status::Ok {
            return Err("AUTHENTICATE failed".into());
        }
        d.status
    };
    if status != Status::Ok {
        return Err("login failed".into());
    }
    conn.state = State::Auth;
    // folder.set('INBOX'): SELECT "INBOX"
    conn.read_only = false;
    let d = conn.command("SELECT", &[b"\"INBOX\""], None).await?;
    if d.status != Status::Ok {
        return Err("SELECT failed".into());
    }
    if conn.read_only {
        return Err("INBOX is not writable".into());
    }
    conn.state = State::Selected;
    Ok(())
}

/// Preparation before login (OAuth refresh), runs inside the `with get_mailbox(...)` block.
pub enum BeforeLogin {
    Proceed(TestAccount),
    /// Refresh failed → MailError
    Fail(String),
    /// Exception outside MailError (e.g. DB) → 500
    Error(String),
}

/// Whole test: `with get_mailbox(...) as M: [refresh]; mailbox_login(M, account)`.
pub async fn test_connection<F, Fut>(acc: TestAccount, opts: &ConnectOptions, before_login: F) -> Result<(), TestError>
where
    F: FnOnce(TestAccount) -> Fut,
    Fut: std::future::Future<Output = BeforeLogin>,
{
    let mut conn = get_mailbox(&acc, opts).await?;
    let result = match before_login(acc).await {
        BeforeLogin::Proceed(acc) => mailbox_login(&mut conn, &acc).await.map_err(TestError::Mail),
        BeforeLogin::Fail(msg) => Err(TestError::Mail(msg)),
        BeforeLogin::Error(msg) => Err(TestError::Other(msg)),
    };
    // __exit__ → logout(); an error there replaces the result
    let logged_out = conn.logout().await;
    if !logged_out {
        return Err(TestError::Other("logout without BYE".into()));
    }
    result
}

/// Empty duplex stream as placeholder during the STARTTLS swap.
mod empty {
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

    pub struct EmptyDuplex;

    impl AsyncRead for EmptyDuplex {
        fn poll_read(self: Pin<&mut Self>, _: &mut Context<'_>, _: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
    impl AsyncWrite for EmptyDuplex {
        fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, b: &[u8]) -> Poll<std::io::Result<usize>> {
            Poll::Ready(Ok(b.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_ip_rules() {
        let p = |s: &str| is_public_ip(s.parse().unwrap());
        assert!(!p("127.0.0.1"));
        assert!(!p("10.1.2.3"));
        assert!(!p("192.168.1.1"));
        assert!(!p("100.64.0.1"));
        assert!(!p("169.254.1.1"));
        assert!(!p("::1"));
        assert!(!p("fe80::1"));
        assert!(!p("fd00::1"));
        assert!(!p("64:ff9b::7f00:1"));
        assert!(!p("::ffff:127.0.0.1"));
        assert!(p("8.8.8.8"));
        assert!(p("2a00:1450:4001::1"));
    }

    #[test]
    fn quoting() {
        assert_eq!(quote_ascii("a\"b\\c").unwrap(), b"\"a\\\"b\\\\c\"".to_vec());
        assert!(quote_ascii("ä").is_err());
    }
}
