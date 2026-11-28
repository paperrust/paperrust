//! IMAP client for mail fetching (Python-free).
//!
//! upstream: src/paperless_mail/mail.py@v3.4.2 (get_mailbox, PinnedIMAP4, Pinned*MailBox,
//! mailbox_login), imap_tools 1.15.1 `mailbox.py` (BaseMailBox.login/login_utf8/xoauth2/logout/
//! uids/fetch/_fetch_in_bulk/delete/copy/move/flag/expunge), `folder.py` (set/status/list),
//! `utils.py` (quote, encode_folder, clean_flags), `imap_utf7.py`, Python 3.12 `imaplib`
//! (_connect, _get_capabilities, _get_response with literal reading, _untagged_response, uid,
//! select/READ-ONLY, starttls, authenticate, logout).
//!
//! Responses are collected like imaplib (`untagged_responses[typ]`, literals as a pair
//! `(head, data)`), so imap_tools' pairing on FETCH stays the same.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::{self, pki_types::ServerName};

use crate::imap::{SECURITY_NONE, SECURITY_SSL, SECURITY_STARTTLS, is_public_ip, resolve_hostname_ips};

/// Error of an IMAP step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImapError {
    /// upstream `MailError` (DNS block, login …).
    Mail(String),
    /// imap_tools `UnexpectedCommandStatusError` subclass (name, e.g. `MailboxFolderSelectError`).
    Status { kind: &'static str, message: String },
    /// imaplib `abort`/`error`/socket/TLS (other exception).
    Other(String),
}

impl std::fmt::Display for ImapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImapError::Mail(m) | ImapError::Other(m) => f.write_str(m),
            ImapError::Status { kind, message } => write!(f, "{kind}: {message}"),
        }
    }
}

impl std::error::Error for ImapError {}

/// Response element like imaplib (`bytes` or `(bytes, bytes)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Line(Vec<u8>),
    Literal(Vec<u8>, Vec<u8>),
}

/// Result of a command: status (`OK`/`NO`) + data.
#[derive(Debug, Clone)]
pub struct CmdResult {
    pub typ: String,
    pub data: Vec<Option<Item>>,
}

#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub allow_internal_hosts: bool,
    pub certificate_file: Option<std::path::PathBuf>,
    pub timeout: Duration,
    pub extra_roots: Vec<Vec<u8>>,
}

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

/// A connected mailbox (`MailBox`).
pub struct MailBox {
    stream: Box<dyn Stream>,
    buf: Vec<u8>,
    tag_no: u32,
    /// `client.capabilities` (upper case, after connect/STARTTLS).
    pub capabilities: Vec<String>,
    untagged: HashMap<String, Vec<Item>>,
    bye: bool,
    read_only: bool,
    authenticated: bool,
    timeout: Duration,
    /// Recording of the sent commands (without tags, passwords masked) – tests/diagnostics.
    pub transcript: Vec<String>,
    /// `_current_folder`
    pub current_folder: Option<String>,
    /// Capabilities announced after authentication (`[CAPABILITY ...]` in the tagged LOGIN/
    /// AUTHENTICATE reply or an untagged `CAPABILITY`). Not used by imaplib; PaperRust only
    /// consults it for `UIDPLUS`/`MOVE` so the safe commands are found on servers that list
    /// them post-login only (Dovecot).
    pub auth_capabilities: Vec<String>,
    /// `UIDVALIDITY` from the untagged `OK [UIDVALIDITY n]` of the last successful SELECT.
    pub selected_uidvalidity: Option<i64>,
}

const MAX_LITERAL: usize = 512 * 1024 * 1024;
const MAX_LINE: usize = 1_000_000; // imaplib _MAXLINE

fn root_store(opts: &ConnectOptions) -> Result<rustls::RootCertStore, String> {
    let mut roots = rustls::RootCertStore::empty();
    for c in rustls_native_certs::load_native_certs().certs {
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

/// imaplib `_quote` (for LOGIN).
fn imaplib_quote(s: &str) -> Result<Vec<u8>, ImapError> {
    if s.contains(['\r', '\n']) {
        return Err(ImapError::Other("line breaks in argument".into()));
    }
    Ok(format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")).into_bytes())
}

/// imap_tools `quote` (str).
pub fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// imap_tools `utf7_encode` (IMAP modified UTF-7, RFC 3501 5.1.3).
pub fn utf7_encode(s: &str) -> Vec<u8> {
    fn flush(out: &mut Vec<u8>, pending: &mut Vec<u16>) {
        if pending.is_empty() {
            return;
        }
        let bytes: Vec<u8> = pending.iter().flat_map(|u| u.to_be_bytes()).collect();
        let b64 = base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes).replace('/', ",");
        out.push(b'&');
        out.extend_from_slice(b64.as_bytes());
        out.push(b'-');
        pending.clear();
    }
    let mut out = Vec::new();
    let mut pending: Vec<u16> = Vec::new();
    for c in s.chars() {
        let o = c as u32;
        if (0x20..=0x7e).contains(&o) {
            flush(&mut out, &mut pending);
            if c == '&' {
                out.extend_from_slice(b"&-");
            } else {
                out.push(o as u8);
            }
        } else {
            let mut b = [0u16; 2];
            pending.extend_from_slice(c.encode_utf16(&mut b));
        }
    }
    flush(&mut out, &mut pending);
    out
}

/// imap_tools `encode_folder` = `quote(utf7_encode(folder))` (on bytes).
pub fn encode_folder(folder: &str) -> Vec<u8> {
    let raw = utf7_encode(folder);
    let mut out = vec![b'"'];
    for b in raw {
        if b == b'\\' || b == b'"' {
            out.push(b'\\');
        }
        out.push(b);
    }
    out.push(b'"');
    out
}

/// `clean_flags` – non-system flags must not start with `\`.
pub fn clean_flags(flags: &[String]) -> Result<(), ImapError> {
    const SYS: [&str; 6] = ["\\SEEN", "\\ANSWERED", "\\FLAGGED", "\\DELETED", "\\DRAFT", "\\RECENT"];
    for f in flags {
        if !SYS.contains(&f.to_uppercase().as_str()) && f.starts_with('\\') {
            return Err(ImapError::Other("Non system flag must not start with \"\\\"".into()));
        }
    }
    Ok(())
}

fn untagged_re_type(line: &[u8]) -> Option<(String, Vec<u8>)> {
    // `\* (?P<type>[A-Z-]+)( (?P<data>.*))?` or `\* (?P<data>\d+) (?P<type>[A-Z-]+)( (?P<data2>.*))?`
    let rest = line.strip_prefix(b"* ")?;
    let word_end = rest.iter().position(|&c| c == b' ').unwrap_or(rest.len());
    let first = &rest[..word_end];
    let after = rest.get(word_end + 1..);
    let is_type = |w: &[u8]| !w.is_empty() && w.iter().all(|&c| c.is_ascii_uppercase() || c == b'-');
    if is_type(first) {
        return Some((String::from_utf8_lossy(first).into_owned(), after.map(<[u8]>::to_vec).unwrap_or_default()));
    }
    if !first.is_empty() && first.iter().all(u8::is_ascii_digit) {
        let after = after.unwrap_or(&[]);
        let t_end = after.iter().position(|&c| c == b' ').unwrap_or(after.len());
        let t = &after[..t_end];
        if is_type(t) {
            let mut dat = first.to_vec();
            if let Some(d2) = after.get(t_end + 1..) {
                dat.push(b' ');
                dat.extend_from_slice(d2);
            }
            return Some((String::from_utf8_lossy(t).into_owned(), dat));
        }
    }
    None
}

/// `[UIDVALIDITY n]` response code of an untagged `OK` after SELECT (RFC 3501 7.1).
fn select_uidvalidity(item: &Item) -> Option<i64> {
    let Item::Line(l) = item else { return None };
    let text = String::from_utf8_lossy(l);
    let upper = text.to_ascii_uppercase();
    let start = upper.find("[UIDVALIDITY ")? + "[UIDVALIDITY ".len();
    let rest = &upper[start..];
    let end = rest.find(']')?;
    rest[..end].trim().parse().ok()
}

/// `Literal = br'.*{(?P<size>\d+)}$'`
fn literal_size(dat: &[u8]) -> Option<usize> {
    let s = dat.strip_suffix(b"}")?;
    let open = s.iter().rposition(|&c| c == b'{')?;
    let digits = &s[open + 1..];
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

impl MailBox {
    /// `get_mailbox(server, port, security)` + greeting + CAPABILITY (+ STARTTLS).
    pub async fn connect(server: &str, port: u16, security: i16, opts: &ConnectOptions) -> Result<MailBox, ImapError> {
        let mut pinned = None;
        if !opts.allow_internal_hosts {
            let ips = resolve_hostname_ips(server).await.map_err(ImapError::Mail)?;
            if ips.iter().any(|ip| !is_public_ip(*ip)) {
                return Err(ImapError::Mail(format!("Connection blocked: {server} resolves to a non-public address")));
            }
            pinned = Some(ips);
        }
        if security == SECURITY_STARTTLS && port == 993 {
            return Err(ImapError::Other("Port 993 requires IMAP4_SSL. Use MailBox class for SSL/TLS connection.".into()));
        }
        if security != SECURITY_NONE && security != SECURITY_SSL && security != SECURITY_STARTTLS {
            return Err(ImapError::Other("Unknown IMAP security".into()));
        }
        let tcp = tcp_connect(server, port, &pinned, opts.timeout).await.map_err(ImapError::Other)?;
        let mut stream: Box<dyn Stream> = Box::new(tcp);
        if security == SECURITY_SSL {
            stream = tls_wrap(stream, server, opts).await.map_err(ImapError::Other)?;
        }
        let mut mb = MailBox {
            stream,
            buf: Vec::new(),
            tag_no: 0,
            capabilities: Vec::new(),
            untagged: HashMap::new(),
            bye: false,
            read_only: false,
            authenticated: false,
            timeout: opts.timeout,
            transcript: Vec::new(),
            current_folder: None,
            auth_capabilities: Vec::new(),
            selected_uidvalidity: None,
        };
        mb.greeting().await?;
        mb.load_capabilities().await?;
        if security == SECURITY_STARTTLS {
            if !mb.capabilities.iter().any(|c| c == "STARTTLS") {
                return Err(ImapError::Other("TLS not supported by server".into()));
            }
            let r = mb.simple("STARTTLS", &[], None).await?;
            if r.typ != "OK" {
                return Err(ImapError::Status { kind: "MailboxStarttlsError", message: "Couldn't establish TLS session".into() });
            }
            let plain = std::mem::replace(&mut mb.stream, Box::new(empty::EmptyDuplex));
            mb.stream = tls_wrap(plain, server, opts).await.map_err(ImapError::Other)?;
            mb.buf.clear();
            mb.load_capabilities().await?;
        }
        Ok(mb)
    }

    async fn read_more(&mut self) -> Result<(), ImapError> {
        let mut chunk = [0u8; 16384];
        let n = match tokio::time::timeout(self.timeout, self.stream.read(&mut chunk)).await {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(ImapError::Other(format!("socket error: {e}"))),
            Err(_) => return Err(ImapError::Other("socket timeout".into())),
        };
        if n == 0 {
            return Err(ImapError::Other("socket error: EOF".into()));
        }
        self.buf.extend_from_slice(&chunk[..n]);
        Ok(())
    }

    /// `_get_line` (without CRLF).
    async fn get_line(&mut self) -> Result<Vec<u8>, ImapError> {
        loop {
            if let Some(p) = self.buf.windows(2).position(|w| w == b"\r\n") {
                let line = self.buf[..p].to_vec();
                self.buf.drain(..p + 2);
                return Ok(line);
            }
            if self.buf.len() > MAX_LINE {
                return Err(ImapError::Other(format!("got more than {MAX_LINE} bytes")));
            }
            self.read_more().await?;
        }
    }

    async fn read_exact(&mut self, n: usize) -> Result<Vec<u8>, ImapError> {
        if n > MAX_LITERAL {
            return Err(ImapError::Other("literal too large".into()));
        }
        while self.buf.len() < n {
            self.read_more().await?;
        }
        Ok(self.buf.drain(..n).collect())
    }

    async fn write_line(&mut self, line: &[u8]) -> Result<(), ImapError> {
        let mut data = line.to_vec();
        data.extend_from_slice(b"\r\n");
        let stream = &mut self.stream;
        let fut = async move {
            stream.write_all(&data).await?;
            stream.flush().await
        };
        match tokio::time::timeout(self.timeout, fut).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(ImapError::Other(format!("socket error: {e}"))),
            Err(_) => Err(ImapError::Other("socket timeout".into())),
        }
    }

    fn append_untagged(&mut self, typ: &str, item: Item) {
        if typ == "BYE" {
            self.bye = true;
        }
        if let Item::Line(l) = &item
            && (typ == "OK" || typ == "NO" || typ == "BAD")
            && l.windows(11).any(|w| w.eq_ignore_ascii_case(b"[READ-ONLY]"))
        {
            self.read_only = true;
        }
        self.untagged.entry(typ.to_string()).or_default().push(item);
    }

    /// Read one response: `Ok(Some((tag, typ, data)))` tagged, `Ok(None)` untagged/continuation.
    async fn get_response(&mut self) -> Result<Response, ImapError> {
        let resp = self.get_line().await?;
        // tagged response?
        if let Some(sp) = resp.iter().position(|&c| c == b' ')
            && resp[..sp].starts_with(b"PLRS")
        {
            let tag = String::from_utf8_lossy(&resp[..sp]).into_owned();
            let rest = &resp[sp + 1..];
            let t_end = rest.iter().position(|&c| c == b' ').unwrap_or(rest.len());
            let typ = String::from_utf8_lossy(&rest[..t_end]).to_uppercase();
            let data = rest.get(t_end + 1..).map(<[u8]>::to_vec).unwrap_or_default();
            if data.windows(11).any(|w| w.eq_ignore_ascii_case(b"[READ-ONLY]")) {
                self.read_only = true;
            }
            return Ok(Response::Tagged { tag, typ, data });
        }
        if let Some((typ, mut dat)) = untagged_re_type(&resp) {
            while let Some(size) = literal_size(&dat) {
                let data = self.read_exact(size).await?;
                self.append_untagged(&typ, Item::Literal(dat, data));
                dat = self.get_line().await?;
            }
            self.append_untagged(&typ, Item::Line(dat));
            return Ok(Response::Untagged);
        }
        if resp.starts_with(b"+") {
            return Ok(Response::Continuation);
        }
        Err(ImapError::Other(format!("unexpected response: {:?}", String::from_utf8_lossy(&resp))))
    }

    async fn greeting(&mut self) -> Result<(), ImapError> {
        self.get_response().await?;
        if self.untagged.contains_key("PREAUTH") {
            self.authenticated = true;
        } else if !self.untagged.contains_key("OK") {
            return Err(ImapError::Other("unexpected greeting".into()));
        }
        Ok(())
    }

    async fn load_capabilities(&mut self) -> Result<(), ImapError> {
        let r = self.simple("CAPABILITY", &[], None).await?;
        let caps = self.take_untagged(&r, "CAPABILITY");
        let Some(Some(Item::Line(l))) = caps.data.last() else {
            return Err(ImapError::Other("no CAPABILITY response from server".into()));
        };
        self.capabilities = String::from_utf8_lossy(l).to_uppercase().split_whitespace().map(str::to_string).collect();
        if !self.capabilities.iter().any(|c| c == "IMAP4REV1" || c == "IMAP4") {
            return Err(ImapError::Other("server not IMAP4 compliant".into()));
        }
        Ok(())
    }

    fn next_tag(&mut self) -> String {
        self.tag_no += 1;
        format!("PLRS{}", self.tag_no)
    }

    /// `_simple_command` + `_command_complete` (BAD → error, BYE → abort).
    /// `auth`: reply to continuations (AUTHENTICATE).
    async fn simple(&mut self, cmd: &str, args: &[&[u8]], auth: Option<&[u8]>) -> Result<CmdResult, ImapError> {
        if self.bye {
            return Err(ImapError::Other("socket error: server closed connection".into()));
        }
        let tag = self.next_tag();
        let mut line = format!("{tag} {cmd}").into_bytes();
        let mut logged = cmd.to_string();
        for a in args {
            line.push(b' ');
            line.extend_from_slice(a);
            if cmd != "LOGIN" {
                logged.push(' ');
                logged.push_str(&String::from_utf8_lossy(a));
            }
        }
        if cmd == "LOGIN" {
            logged.push_str(" <credentials>");
        }
        self.transcript.push(logged);
        // `untagged_responses` from before commands are kept (imaplib), only the OK/NO/BAD of the
        // last command are replaced (`_check_bye`/`_get_tagged_response`).
        self.write_line(&line).await?;
        loop {
            match self.get_response().await? {
                Response::Continuation => match auth {
                    Some(resp) => self.write_line(resp).await?,
                    None => return Err(ImapError::Other("unexpected continuation".into())),
                },
                Response::Tagged { tag: t, typ, data } if t == tag => {
                    if self.bye && cmd != "LOGOUT" {
                        return Err(ImapError::Other("socket error: server closed connection".into()));
                    }
                    if typ == "BAD" {
                        return Err(ImapError::Other(format!("{cmd} command error: BAD [{}]", String::from_utf8_lossy(&data))));
                    }
                    return Ok(CmdResult { typ, data: vec![Some(Item::Line(data))] });
                }
                Response::Tagged { .. } => {}
                Response::Untagged => {
                    if self.bye && cmd != "LOGOUT" {
                        return Err(ImapError::Other("socket error: server closed connection".into()));
                    }
                }
            }
        }
    }

    /// `_untagged_response(typ, dat, name)`.
    fn take_untagged(&mut self, r: &CmdResult, name: &str) -> CmdResult {
        if r.typ == "NO" {
            return r.clone();
        }
        match self.untagged.remove(name) {
            Some(items) => CmdResult { typ: r.typ.clone(), data: items.into_iter().map(Some).collect() },
            None => CmdResult { typ: r.typ.clone(), data: vec![None] },
        }
    }

    /// imaplib `uid(command, *args)`.
    async fn uid(&mut self, command: &str, args: &[&[u8]]) -> Result<CmdResult, ImapError> {
        let command = command.to_uppercase();
        let mut all: Vec<&[u8]> = vec![command.as_bytes()];
        all.extend_from_slice(args);
        let r = self.simple("UID", &all, None).await?;
        let name = if matches!(command.as_str(), "SEARCH" | "SORT" | "THREAD") { command.as_str() } else { "FETCH" };
        Ok(self.take_untagged(&r, name))
    }

    fn need_nonauth(&self, cmd: &str) -> Result<(), ImapError> {
        if self.authenticated {
            return Err(ImapError::Other(format!("command {cmd} illegal in state AUTH, only allowed in states NONAUTH")));
        }
        Ok(())
    }

    // ---------------------------------------------------------------------------------------
    // Login (mail.py::mailbox_login)

    /// `mailbox_login(mailbox, account)`; all errors → `MailError`.
    pub async fn login(&mut self, username: &str, password: &str, is_token: bool) -> Result<(), ImapError> {
        let r = self.login_inner(username, password, is_token).await;
        r.map_err(|e| {
            tracing::error!(target: "paperless_mail", "Error while authenticating account: {}", match &e {
                ImapError::Status { kind, .. } => kind.to_string(),
                _ => "login failed".into(),
            });
            ImapError::Mail("Error while authenticating account".into())
        })
    }

    async fn login_inner(&mut self, username: &str, password: &str, is_token: bool) -> Result<(), ImapError> {
        let r = self.authenticate(username, password, is_token).await?;
        self.authenticated = true;
        self.auth_capabilities = self.post_auth_capabilities(&r);
        // initial_folder='INBOX'
        self.folder_set("INBOX").await
    }

    /// Capabilities from `[CAPABILITY ...]` in the tagged reply or an untagged `CAPABILITY`
    /// that arrived with the login.
    fn post_auth_capabilities(&mut self, r: &CmdResult) -> Vec<String> {
        let mut lines: Vec<String> = Vec::new();
        if let Some(Some(Item::Line(l))) = r.data.first() {
            let text = String::from_utf8_lossy(l).to_uppercase();
            if let Some(rest) = text.strip_prefix("[CAPABILITY ")
                && let Some(end) = rest.find(']')
            {
                lines.push(rest[..end].to_string());
            }
        }
        if let Some(items) = self.untagged.remove("CAPABILITY") {
            for it in items {
                if let Item::Line(l) = it {
                    lines.push(String::from_utf8_lossy(&l).to_uppercase());
                }
            }
        }
        lines.iter().flat_map(|l| l.split_whitespace().map(str::to_string)).collect()
    }

    /// Server capability, before or after authentication.
    pub fn has_capability(&self, name: &str) -> bool {
        self.capabilities.iter().chain(self.auth_capabilities.iter()).any(|c| c == name)
    }

    async fn authenticate(&mut self, username: &str, password: &str, is_token: bool) -> Result<CmdResult, ImapError> {
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b).into_bytes();
        if is_token {
            self.need_nonauth("AUTHENTICATE")?;
            let auth = format!("user={username}\x01auth=Bearer {password}\x01\x01");
            let resp = b64(auth.as_bytes());
            let r = self.simple("AUTHENTICATE", &[b"XOAUTH2"], Some(&resp)).await?;
            if r.typ != "OK" {
                return Err(ImapError::Status { kind: "MailboxLoginError", message: "AUTHENTICATE failed".into() });
            }
            Ok(r)
        } else if password.is_ascii() {
            self.need_nonauth("LOGIN")?;
            if !username.is_ascii() {
                return Err(ImapError::Other("'ascii' codec can't encode characters".into()));
            }
            let u = imaplib_quote(username)?;
            let p = imaplib_quote(password)?;
            let r = self.simple("LOGIN", &[&u, &p], None).await?;
            if r.typ != "OK" {
                return Err(ImapError::Status { kind: "MailboxLoginError", message: "LOGIN failed".into() });
            }
            Ok(r)
        } else {
            tracing::debug!(target: "paperless_mail", "Falling back to AUTH=PLAIN");
            self.need_nonauth("AUTHENTICATE")?;
            let mut raw = vec![0u8];
            raw.extend_from_slice(username.as_bytes());
            raw.push(0);
            raw.extend_from_slice(password.as_bytes());
            let resp = b64(&raw);
            let r = self.simple("AUTHENTICATE", &[b"PLAIN"], Some(&resp)).await?;
            if r.typ != "OK" {
                return Err(ImapError::Status { kind: "MailboxLoginError", message: "AUTHENTICATE failed".into() });
            }
            Ok(r)
        }
    }

    // ---------------------------------------------------------------------------------------
    // Folders

    /// `folder.set(folder)` → `SELECT` (READ-ONLY → imaplib `readonly` exception).
    pub async fn folder_set(&mut self, folder: &str) -> Result<(), ImapError> {
        self.untagged.clear();
        self.read_only = false;
        self.selected_uidvalidity = None;
        let enc = encode_folder(folder);
        let r = self.simple("SELECT", &[&enc], None).await?;
        self.selected_uidvalidity = self.untagged.get("OK").and_then(|items| items.iter().find_map(select_uidvalidity));
        if r.typ != "OK" {
            self.selected_uidvalidity = None;
            self.current_folder = None;
            return Err(ImapError::Status {
                kind: "MailboxFolderSelectError",
                message: format!("Response status \"OK\" expected, but \"{}\" received.", r.typ),
            });
        }
        if self.read_only {
            return Err(ImapError::Other(format!("{} is not writable", String::from_utf8_lossy(&enc))));
        }
        self.current_folder = Some(folder.to_string());
        Ok(())
    }

    /// `folder.status(folder, ["UIDVALIDITY"])`.
    pub async fn folder_status_uidvalidity(&mut self, folder: &str) -> Result<Option<i64>, ImapError> {
        let enc = encode_folder(folder);
        let r = self.simple("STATUS", &[&enc, b"(UIDVALIDITY)"], None).await?;
        if r.typ != "OK" {
            return Err(ImapError::Status { kind: "MailboxFolderStatusError", message: format!("\"{}\" received", r.typ) });
        }
        let res = self.take_untagged(&r, "STATUS");
        let line = res.data.iter().find_map(|i| match i {
            Some(Item::Line(l)) => Some(l.clone()),
            _ => None,
        });
        let Some(line) = line else {
            return Err(ImapError::Other("list index out of range".into()));
        };
        let text = String::from_utf8_lossy(&line).into_owned();
        // values = status_data.decode().split('(')[-1].split(')')[0].split(' ')
        let inner = text.rsplit('(').next().unwrap_or("").split(')').next().unwrap_or("");
        let vals: Vec<&str> = inner.split(' ').collect();
        if !vals.len().is_multiple_of(2) {
            return Err(ImapError::Other("An even-length array is expected".into()));
        }
        for pair in vals.chunks(2) {
            if pair[0] == "UIDVALIDITY" && !pair[1].is_empty() && pair[1].chars().all(|c| c.is_ascii_digit()) {
                return Ok(pair[1].parse().ok());
            }
        }
        Ok(None)
    }

    /// `folder.list()` – names only (for the error log).
    pub async fn folder_list(&mut self) -> Result<Vec<String>, ImapError> {
        let r = self.simple("LIST", &[b"\"\"", b"\"*\""], None).await?;
        let res = self.take_untagged(&r, "LIST");
        let mut out = Vec::new();
        for it in res.data.into_iter().flatten() {
            let raw = match it {
                Item::Line(l) => l,
                Item::Literal(h, _) => h,
            };
            let s = String::from_utf8_lossy(&raw).into_owned();
            // (?P<flags>…) (?P<delim>\S+) (?P<name>.+)
            if let Some(close) = s.find(") ") {
                let rest = &s[close + 2..];
                if let Some(sp) = rest.find(' ') {
                    let name = rest[sp + 1..].trim_matches('"').to_string();
                    out.push(name);
                }
            }
        }
        Ok(out)
    }

    // ---------------------------------------------------------------------------------------
    // Search/fetch

    /// `uids(criteria, charset)` → `UID SEARCH CHARSET <cs> <criteria>`.
    pub async fn uids(&mut self, criteria: &[u8], charset: Option<&str>) -> Result<Vec<String>, ImapError> {
        let r = match charset {
            None => self.uid("SEARCH", &[criteria]).await?,
            Some(cs) => self.uid("SEARCH", &[b"CHARSET", cs.as_bytes(), criteria]).await?,
        };
        if r.typ != "OK" {
            return Err(ImapError::Status { kind: "MailboxUidsError", message: format!("\"{}\" received", r.typ) });
        }
        match r.data.first() {
            Some(Some(Item::Line(l))) if !l.is_empty() => Ok(String::from_utf8_lossy(l).split_whitespace().map(str::to_string).collect()),
            _ => Ok(Vec::new()),
        }
    }

    /// `fetch(uid_list=…, mark_seen=False, bulk=n)` → list of response pairs (imap_tools pairing).
    pub async fn fetch_bulk(&mut self, uids: &[String], bulk: usize) -> Result<Vec<Vec<Item>>, ImapError> {
        let mut out = Vec::new();
        for chunk in uids.chunks(bulk.max(2)) {
            let set = chunk.join(",");
            let r = self.uid("FETCH", &[set.as_bytes(), b"(BODY.PEEK[] UID FLAGS RFC822.SIZE)"]).await?;
            if r.typ != "OK" {
                return Err(ImapError::Status { kind: "MailboxFetchError", message: format!("\"{}\" received", r.typ) });
            }
            if r.data.is_empty() || r.data[0].is_none() {
                return Ok(out);
            }
            let items: Vec<Item> = r.data.into_iter().flatten().collect();
            for pair in items.chunks(2) {
                out.push(pair.to_vec());
            }
        }
        Ok(out)
    }

    // ---------------------------------------------------------------------------------------
    // Actions

    pub async fn expunge(&mut self) -> Result<(), ImapError> {
        let r = self.simple("EXPUNGE", &[], None).await?;
        let _ = self.take_untagged(&r, "EXPUNGE");
        if r.typ != "OK" {
            return Err(ImapError::Status { kind: "MailboxExpungeError", message: format!("\"{}\" received", r.typ) });
        }
        Ok(())
    }

    /// `UID EXPUNGE uid` (RFC 4315, needs `UIDPLUS`): removes only this message.
    pub async fn uid_expunge(&mut self, uid: &str) -> Result<(), ImapError> {
        let r = self.uid("EXPUNGE", &[uid.as_bytes()]).await?;
        if r.typ != "OK" {
            return Err(ImapError::Status { kind: "MailboxExpungeError", message: format!("\"{}\" received", r.typ) });
        }
        Ok(())
    }

    /// `delete(uid)`: `UID STORE uid +FLAGS (\Deleted)`, then `UID EXPUNGE uid` when the server
    /// has `UIDPLUS`. Without it only the folder-wide `EXPUNGE` exists (imap_tools behaviour);
    /// that also removes messages other clients marked `\Deleted` (Known differences).
    pub async fn delete(&mut self, uid: &str) -> Result<(), ImapError> {
        let r = self.uid("STORE", &[uid.as_bytes(), b"+FLAGS", b"(\\Deleted)"]).await?;
        if r.typ != "OK" {
            return Err(ImapError::Status { kind: "MailboxDeleteError", message: format!("\"{}\" received", r.typ) });
        }
        if self.has_capability("UIDPLUS") {
            self.uid_expunge(uid).await
        } else {
            tracing::warn!(target: "paperless_mail", "Server lacks UIDPLUS; deleting mail {uid} with a folder-wide EXPUNGE");
            self.expunge().await
        }
    }

    /// `copy(uid, folder)`.
    pub async fn copy(&mut self, uid: &str, folder: &str) -> Result<(), ImapError> {
        let enc = encode_folder(folder);
        let r = self.uid("COPY", &[uid.as_bytes(), &enc]).await?;
        if r.typ != "OK" {
            return Err(ImapError::Status { kind: "MailboxCopyError", message: format!("\"{}\" received", r.typ) });
        }
        Ok(())
    }

    /// `move(uid, folder)`: `UID MOVE` with capability `MOVE` (also when only announced after
    /// login), otherwise COPY + [`Self::delete`].
    pub async fn move_to(&mut self, uid: &str, folder: &str) -> Result<(), ImapError> {
        if self.has_capability("MOVE") {
            let enc = encode_folder(folder);
            let r = self.uid("MOVE", &[uid.as_bytes(), &enc]).await?;
            if r.typ != "OK" {
                return Err(ImapError::Status { kind: "MailboxMoveError", message: format!("\"{}\" received", r.typ) });
            }
            Ok(())
        } else {
            self.copy(uid, folder).await?;
            self.delete(uid).await
        }
    }

    /// `flag(uid, flags, value)`: `UID STORE uid ±FLAGS (…)`. imap_tools sends a folder-wide
    /// `EXPUNGE` afterwards; PaperRust does not, because that would permanently remove every
    /// message in the folder that any client marked `\Deleted` (Known differences).
    pub async fn flag(&mut self, uid: &str, flags: &[String], value: bool) -> Result<(), ImapError> {
        clean_flags(flags)?;
        let op = if value { "+FLAGS" } else { "-FLAGS" };
        let list = format!("({})", flags.join(" "));
        let r = self.uid("STORE", &[uid.as_bytes(), op.as_bytes(), list.as_bytes()]).await?;
        if r.typ != "OK" {
            return Err(ImapError::Status { kind: "MailboxFlagError", message: format!("\"{}\" received", r.typ) });
        }
        Ok(())
    }

    /// Gmail: `client.uid("STORE", uid, "+X-GM-LABELS", keyword)` (`None` makes imaplib omit
    /// the argument). upstream ignores the reply status; here `NO` is an error (`BAD` already
    /// is one in [`Self::simple`]) so a rejected label is not recorded as SUCCESS
    /// (Known differences).
    pub async fn gmail_label(&mut self, uid: &str, keyword: Option<&str>) -> Result<(), ImapError> {
        let mut args: Vec<&[u8]> = vec![uid.as_bytes(), b"+X-GM-LABELS"];
        if let Some(k) = keyword {
            args.push(k.as_bytes());
        }
        let r = self.uid("STORE", &args).await?;
        if r.typ != "OK" {
            return Err(ImapError::Status { kind: "MailboxTaggingError", message: format!("\"{}\" received", r.typ) });
        }
        Ok(())
    }

    /// `__exit__` → `logout()`; `true` if `BYE` came.
    pub async fn logout(&mut self) -> bool {
        if self.bye {
            return true;
        }
        self.transcript.push("LOGOUT".into());
        let tag = self.next_tag();
        if self.write_line(format!("{tag} LOGOUT").as_bytes()).await.is_err() {
            return self.bye;
        }
        loop {
            match self.get_response().await {
                Ok(Response::Tagged { tag: t, .. }) if t == tag => return self.bye,
                Ok(_) => {}
                Err(_) => return self.bye,
            }
        }
    }
}

enum Response {
    Tagged { tag: String, typ: String, data: Vec<u8> },
    Untagged,
    Continuation,
}

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
    fn utf7() {
        assert_eq!(utf7_encode("INBOX"), b"INBOX");
        assert_eq!(utf7_encode("Entwürfe"), b"Entw&APw-rfe");
        assert_eq!(utf7_encode("A&B"), b"A&-B");
        assert_eq!(encode_folder("Arch\"iv"), b"\"Arch\\\"iv\"");
    }

    #[test]
    fn untagged_parse() {
        assert_eq!(untagged_re_type(b"* 3 FETCH (UID 5 {10}"), Some(("FETCH".into(), b"3 (UID 5 {10}".to_vec())));
        assert_eq!(untagged_re_type(b"* SEARCH 1 2"), Some(("SEARCH".into(), b"1 2".to_vec())));
        assert_eq!(untagged_re_type(b"* SEARCH"), Some(("SEARCH".into(), Vec::new())));
        assert_eq!(literal_size(b"3 (UID 5 BODY[] {123}"), Some(123));
        assert_eq!(literal_size(b"3 (UID 5)"), None);
    }

    #[test]
    fn select_uidvalidity_code() {
        assert_eq!(select_uidvalidity(&Item::Line(b"[UIDVALIDITY 3857529045] UIDs valid".to_vec())), Some(3_857_529_045));
        assert_eq!(select_uidvalidity(&Item::Line(b"[uidvalidity 7]".to_vec())), Some(7));
        assert_eq!(select_uidvalidity(&Item::Line(b"[UIDNEXT 4] Predicted next UID".to_vec())), None);
        assert_eq!(select_uidvalidity(&Item::Line(b"[UIDVALIDITY x]".to_vec())), None);
    }

    #[test]
    fn flags_check() {
        assert!(clean_flags(&["\\Seen".into(), "custom".into()]).is_ok());
        assert!(clean_flags(&["\\Custom".into()]).is_err());
    }
}
