//! E-mail sending (workflow action "e-mail") – Django `EmailMessage` + SMTP backend.
//!
//! upstream: src/documents/mail.py@v3.4.2 (send_email, _get_unique_filename, EmailAttachment),
//! src/paperless/settings/__init__.py@v3.4.2 (EMAIL_HOST/PORT/HOST_USER/HOST_PASSWORD/USE_TLS/
//! USE_SSL, DEFAULT_FROM_EMAIL, EMAIL_TIMEOUT=30, EMAIL_ENABLED), Django 5.2
//! `core/mail/message.py` (EmailMessage.message, SafeMIMEText, _create_attachment,
//! sanitize_address), `core/mail/backends/smtp.py` (open/close/_send), Python 3.12 `smtplib`
//! (ehlo/helo, starttls, login: AUTH order, sendmail: SIZE, quotedata).
//!
//! The MIME text is equivalent in meaning but not byte-identical to Django (boundary, Message-ID, date,
//! encoding of the header lines) – see Known differences.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::{self, pki_types::ServerName};

/// Django `EMAIL_*` settings.
#[derive(Debug, Clone)]
pub struct EmailSettings {
    pub host: String,
    pub port: u16,
    pub host_user: String,
    pub host_password: String,
    pub from_email: String,
    pub use_tls: bool,
    pub use_ssl: bool,
    pub timeout: Duration,
    /// `EMAIL_ENABLED = EMAIL_HOST != "localhost" or EMAIL_HOST_USER != ""`
    pub enabled: bool,
    /// Tests only: additional trusted certificates (DER).
    pub extra_roots: Vec<Vec<u8>>,
}

impl EmailSettings {
    pub fn from_core(core: &pl_core::Config) -> Self {
        Self {
            host: core.email_host.clone(),
            port: u16::try_from(core.email_port).unwrap_or(25),
            host_user: core.email_host_user.clone(),
            host_password: core.email_host_password.clone(),
            from_email: core.default_from_email.clone(),
            use_tls: core.email_use_tls,
            use_ssl: core.email_use_ssl,
            timeout: Duration::from_secs(30),
            enabled: core.email_enabled,
            extra_roots: Vec::new(),
        }
    }
}

/// `EmailAttachment`.
#[derive(Debug, Clone)]
pub struct EmailAttachment {
    pub path: std::path::PathBuf,
    pub mime_type: String,
    pub friendly_name: String,
}

/// Finished message.
#[derive(Debug, Clone)]
pub struct OutgoingMail {
    pub from: String,
    pub recipients: Vec<String>,
    pub data: Vec<u8>,
}

/// `_get_unique_filename(friendly_name, used_names)`.
pub fn unique_filename(friendly: &str, used: &std::collections::HashSet<String>) -> String {
    if !used.contains(friendly) {
        return friendly.to_string();
    }
    // Path.stem + "".join(Path.suffixes)
    let (stem, suffixes) = stem_and_suffixes(friendly);
    let mut counter = 1;
    loop {
        let f = format!("{stem}_{counter:02}{suffixes}");
        if !used.contains(&f) {
            return f;
        }
        counter += 1;
    }
}

fn stem_and_suffixes(name: &str) -> (String, String) {
    let base = name.rsplit('/').next().unwrap_or(name);
    let stem = crate::fetch::handler::path_stem(base);
    // Path.suffixes: all ".x" parts after the first character, unless the name ends with "."
    let suffixes = if base.ends_with('.') {
        String::new()
    } else {
        let trimmed = base.trim_start_matches('.');
        let lead = base.len() - trimmed.len();
        match trimmed.find('.') {
            Some(i) => base[lead + i..].to_string(),
            None => String::new(),
        }
    };
    (stem, suffixes)
}

fn is_ascii_printable(s: &str) -> bool {
    s.bytes().all(|b| (0x20..0x7f).contains(&b))
}

/// RFC 2047 `=?utf-8?b?…?=` (Django: Header(value, "utf-8").encode(); shortest encoding).
fn encode_header_value(v: &str) -> String {
    if is_ascii_printable(v) {
        return v.to_string();
    }
    // in chunks ≤ 45 bytes (at character boundaries), one encoded-word each
    let mut words = Vec::new();
    let mut cur = String::new();
    for c in v.chars() {
        if cur.len() + c.len_utf8() > 45 {
            words.push(std::mem::take(&mut cur));
        }
        cur.push(c);
    }
    if !cur.is_empty() {
        words.push(cur);
    }
    words
        .iter()
        .map(|w| format!("=?utf-8?b?{}?=", base64::engine::general_purpose::STANDARD.encode(w.as_bytes())))
        .collect::<Vec<_>>()
        .join("\r\n ")
}

fn sanitize_address(a: &str) -> Result<String, String> {
    let a = a.trim();
    if a.is_empty() || a.contains(['\r', '\n']) {
        return Err(format!("Invalid address \"{a}\""));
    }
    Ok(a.to_string())
}

/// Address for `MAIL FROM`/`RCPT TO` (only the addr-spec part).
fn envelope_addr(a: &str) -> String {
    match (a.rfind('<'), a.rfind('>')) {
        (Some(s), Some(e)) if s < e => a[s + 1..e].trim().to_string(),
        _ => a.trim().to_string(),
    }
}

fn b64_lines(data: &[u8]) -> String {
    let enc = base64::engine::general_purpose::STANDARD.encode(data);
    let mut out = String::with_capacity(enc.len() + enc.len() / 76 * 2 + 2);
    for chunk in enc.as_bytes().chunks(76) {
        out.push_str(&String::from_utf8_lossy(chunk));
        out.push_str("\r\n");
    }
    out
}

fn filename_param(name: &str) -> String {
    if name.is_ascii() {
        format!("filename=\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        // RFC 2231: filename*=utf-8''…
        // RFC 2231 attribute-char: ALPHA / DIGIT / ! # $ & + - . ^ _ ` | ~
        const ATTR: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
            .remove(b'!')
            .remove(b'#')
            .remove(b'$')
            .remove(b'&')
            .remove(b'+')
            .remove(b'-')
            .remove(b'.')
            .remove(b'^')
            .remove(b'_')
            .remove(b'`')
            .remove(b'|')
            .remove(b'~');
        let enc: String = percent_encoding::utf8_percent_encode(name, ATTR).to_string();
        format!("filename*=utf-8''{enc}")
    }
}

fn text_part(body: &str) -> String {
    let long = body.lines().any(|l| l.len() > 998);
    let mut s = String::from("Content-Type: text/plain; charset=\"utf-8\"\r\nMIME-Version: 1.0\r\n");
    if long {
        // quoted-printable (utf8_charset_qp)
        s.push_str("Content-Transfer-Encoding: quoted-printable\r\n\r\n");
        s.push_str(&qp_encode(body));
    } else {
        let cte = if body.is_ascii() { "7bit" } else { "8bit" };
        s.push_str(&format!("Content-Transfer-Encoding: {cte}\r\n\r\n"));
        s.push_str(&crlf(body));
    }
    s
}

fn crlf(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    let mut prev_cr = false;
    for c in s.chars() {
        if c == '\n' && !prev_cr {
            out.push('\r');
        }
        prev_cr = c == '\r';
        out.push(c);
    }
    out
}

fn qp_encode(s: &str) -> String {
    let mut out = String::new();
    for line in s.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        let mut cur = String::new();
        for b in line.bytes() {
            let enc = if (33..=126).contains(&b) && b != b'=' || b == b' ' { (b as char).to_string() } else { format!("={b:02X}") };
            if cur.len() + enc.len() > 75 {
                out.push_str(&cur);
                out.push_str("=\r\n");
                cur.clear();
            }
            cur.push_str(&enc);
        }
        out.push_str(&cur);
        out.push_str("\r\n");
    }
    out
}

/// `send_email(subject, body, to, attachments)` → finished message (not yet sent).
pub fn build_message(
    s: &EmailSettings,
    subject: &str,
    body: &str,
    to: &[String],
    attachments: &[EmailAttachment],
) -> Result<OutgoingMail, String> {
    // collapse line breaks in the subject
    let subject = if subject.contains(['\r', '\n']) {
        pl_parsers::pystr::py_splitlines(subject).iter().map(|l| l.trim_matches([' ', '\t'])).collect::<Vec<_>>().join(" ")
    } else {
        subject.to_string()
    };
    let from = sanitize_address(&s.from_email)?;
    let recipients: Vec<String> = to.iter().map(|a| sanitize_address(a)).collect::<Result<_, _>>()?;
    let mut head = String::new();
    head.push_str(&format!("Subject: {}\r\n", encode_header_value(&subject)));
    head.push_str(&format!("From: {}\r\n", encode_header_value(&from)));
    head.push_str(&format!("To: {}\r\n", recipients.iter().map(|r| encode_header_value(r)).collect::<Vec<_>>().join(", ")));
    head.push_str(&format!("Date: {}\r\n", chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S -0000")));
    let domain = gethostname_fqdn();
    head.push_str(&format!("Message-ID: <{}@{domain}>\r\n", uuid::Uuid::new_v4().simple()));
    let mut data = String::new();
    let mut raw: Vec<u8> = Vec::new();
    if attachments.is_empty() {
        let part = text_part(body);
        // MIME-Version already occurs in the part; prepend header
        data.push_str(&part.replacen("\r\n\r\n", &format!("\r\n{head}\r\n"), 1));
        raw.extend_from_slice(data.as_bytes());
    } else {
        let boundary = format!("==============={}==", uuid::Uuid::new_v4().as_u128() % 10u128.pow(19));
        raw.extend_from_slice(
            format!("Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\nMIME-Version: 1.0\r\n{head}\r\n").as_bytes(),
        );
        raw.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        raw.extend_from_slice(text_part(body).as_bytes());
        raw.extend_from_slice(b"\r\n");
        let mut used = std::collections::HashSet::new();
        for a in attachments {
            let filename = unique_filename(&a.friendly_name, &used);
            used.insert(filename.clone());
            let content = std::fs::read(&a.path).map_err(|e| format!("{e}"))?;
            let (main, sub) = a.mime_type.split_once('/').unwrap_or(("application", "octet-stream"));
            raw.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            let disp = format!("Content-Disposition: attachment; {}\r\n", filename_param(&filename));
            if main == "message" && sub == "rfc822" {
                raw.extend_from_slice(b"Content-Type: message/rfc822\r\nMIME-Version: 1.0\r\n");
                raw.extend_from_slice(disp.as_bytes());
                raw.extend_from_slice(b"\r\n");
                raw.extend_from_slice(&content);
                if !content.ends_with(b"\n") {
                    raw.extend_from_slice(b"\r\n");
                }
            } else if main == "text"
                && let Ok(t) = String::from_utf8(content.clone())
            {
                let mut p = text_part(&t).replacen("text/plain", &format!("text/{sub}"), 1);
                p = p.replacen("\r\n\r\n", &format!("\r\n{disp}\r\n"), 1);
                raw.extend_from_slice(p.as_bytes());
            } else {
                let mt = if main == "text" { "application/octet-stream".to_string() } else { a.mime_type.clone() };
                raw.extend_from_slice(
                    format!("Content-Type: {mt}\r\nMIME-Version: 1.0\r\nContent-Transfer-Encoding: base64\r\n{disp}\r\n").as_bytes(),
                );
                raw.extend_from_slice(b64_lines(&content).as_bytes());
            }
        }
        raw.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    }
    Ok(OutgoingMail { from: envelope_addr(&from), recipients: recipients.iter().map(|r| envelope_addr(r)).collect(), data: raw })
}

/// `DNS_NAME` (socket.getfqdn) – here the host name.
fn gethostname_fqdn() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "localhost".into()).to_lowercase()
}

// ---------------------------------------------------------------------------------------------
// SMTP

trait Stream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Stream for T {}

struct Smtp {
    stream: Box<dyn Stream>,
    buf: Vec<u8>,
    timeout: Duration,
    esmtp: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmtpError(pub String);

impl std::fmt::Display for SmtpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl Smtp {
    async fn read_reply(&mut self) -> Result<(u16, Vec<String>), SmtpError> {
        let mut lines = Vec::new();
        loop {
            let line = loop {
                if let Some(p) = self
                    .buf
                    .windows(2)
                    .position(|w| w == b"\r\n")
                    .or_else(|| self.buf.iter().position(|&c| c == b'\n').map(|p| p.saturating_sub(0)))
                {
                    let end = if self.buf.get(p) == Some(&b'\r') { p + 2 } else { p + 1 };
                    let l = String::from_utf8_lossy(&self.buf[..p]).trim_end_matches('\r').to_string();
                    self.buf.drain(..end.min(self.buf.len()));
                    break l;
                }
                if self.buf.len() > 8192 {
                    return Err(SmtpError("line too long".into()));
                }
                let mut chunk = [0u8; 4096];
                let n = match tokio::time::timeout(self.timeout, self.stream.read(&mut chunk)).await {
                    Ok(Ok(0)) => return Err(SmtpError("Connection unexpectedly closed".into())),
                    Ok(Ok(n)) => n,
                    Ok(Err(e)) => return Err(SmtpError(e.to_string())),
                    Err(_) => return Err(SmtpError("timed out".into())),
                };
                self.buf.extend_from_slice(&chunk[..n]);
            };
            let code: u16 = line.get(..3).and_then(|c| c.parse().ok()).ok_or_else(|| SmtpError("Connection unexpectedly closed".into()))?;
            let more = line.as_bytes().get(3) == Some(&b'-');
            lines.push(line.get(4..).unwrap_or("").to_string());
            if !more {
                return Ok((code, lines));
            }
        }
    }

    async fn send(&mut self, data: &[u8]) -> Result<(), SmtpError> {
        let stream = &mut self.stream;
        let fut = async move {
            stream.write_all(data).await?;
            stream.flush().await
        };
        match tokio::time::timeout(self.timeout, fut).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(SmtpError(e.to_string())),
            Err(_) => Err(SmtpError("timed out".into())),
        }
    }

    async fn cmd(&mut self, line: &str) -> Result<(u16, Vec<String>), SmtpError> {
        self.send(format!("{line}\r\n").as_bytes()).await?;
        self.read_reply().await
    }

    async fn ehlo_or_helo(&mut self, name: &str) -> Result<(), SmtpError> {
        let (code, lines) = self.cmd(&format!("ehlo {name}")).await?;
        if code == 250 {
            self.esmtp = lines.iter().skip(1).map(|l| l.to_lowercase()).collect();
            return Ok(());
        }
        let (code, _) = self.cmd(&format!("helo {name}")).await?;
        if code != 250 {
            return Err(SmtpError(format!("SMTPHeloError({code})")));
        }
        self.esmtp.clear();
        Ok(())
    }

    fn has_extn(&self, name: &str) -> Option<String> {
        self.esmtp.iter().find_map(|l| {
            let mut it = l.splitn(2, [' ', '=']);
            let k = it.next()?;
            (k == name).then(|| it.next().unwrap_or("").to_string())
        })
    }
}

fn root_store(extra: &[Vec<u8>]) -> rustls::RootCertStore {
    let mut roots = rustls::RootCertStore::empty();
    for c in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(c);
    }
    for der in extra {
        let _ = roots.add(rustls::pki_types::CertificateDer::from(der.clone()));
    }
    roots
}

async fn tls(stream: Box<dyn Stream>, host: &str, s: &EmailSettings) -> Result<Box<dyn Stream>, SmtpError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| SmtpError(format!("tls config: {e}")))?
        .with_root_certificates(root_store(&s.extra_roots))
        .with_no_client_auth();
    let name = ServerName::try_from(host.to_string()).map_err(|e| SmtpError(format!("tls name: {e}")))?;
    match tokio::time::timeout(s.timeout, TlsConnector::from(Arc::new(config)).connect(name, stream)).await {
        Ok(Ok(t)) => Ok(Box::new(t)),
        Ok(Err(e)) => Err(SmtpError(format!("ssl error: {e}"))),
        Err(_) => Err(SmtpError("timed out".into())),
    }
}

/// `quotedata`: line endings → CRLF, double leading dots, terminator `\r\n.\r\n`.
fn quotedata(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 64);
    let mut at_line_start = true;
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        if at_line_start && b == b'.' {
            out.push(b'.');
        }
        match b {
            b'\r' => {
                out.extend_from_slice(b"\r\n");
                if data.get(i + 1) == Some(&b'\n') {
                    i += 1;
                }
                at_line_start = true;
            }
            b'\n' => {
                out.extend_from_slice(b"\r\n");
                at_line_start = true;
            }
            _ => {
                out.push(b);
                at_line_start = false;
            }
        }
        i += 1;
    }
    if !out.ends_with(b"\r\n") {
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b".\r\n");
    out
}

/// Django SMTP backend: connect, (STARTTLS), log in, send, QUIT. Returns: number sent.
pub async fn send(s: &EmailSettings, mail: &OutgoingMail) -> Result<usize, SmtpError> {
    if mail.recipients.is_empty() {
        return Ok(0);
    }
    let addr = (s.host.as_str(), s.port);
    let tcp = match tokio::time::timeout(s.timeout, TcpStream::connect(addr)).await {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => return Err(SmtpError(e.to_string())),
        Err(_) => return Err(SmtpError("timed out".into())),
    };
    let mut stream: Box<dyn Stream> = Box::new(tcp);
    if s.use_ssl {
        stream = tls(stream, &s.host, s).await?;
    }
    let mut c = Smtp { stream, buf: Vec::new(), timeout: s.timeout, esmtp: Vec::new() };
    let (code, _) = c.read_reply().await?;
    if code != 220 {
        return Err(SmtpError(format!("SMTPConnectError({code})")));
    }
    let name = gethostname_fqdn();
    c.ehlo_or_helo(&name).await?;
    if s.use_tls {
        if c.has_extn("starttls").is_none() {
            return Err(SmtpError("STARTTLS extension not supported by server.".into()));
        }
        let (code, _) = c.cmd("STARTTLS").await?;
        if code != 220 {
            return Err(SmtpError(format!("STARTTLS failed ({code})")));
        }
        let plain = std::mem::replace(&mut c.stream, Box::new(tokio::io::duplex(1).0));
        c.stream = tls(plain, &s.host, s).await?;
        c.buf.clear();
        c.ehlo_or_helo(&name).await?;
    }
    if !s.host_user.is_empty() && !s.host_password.is_empty() {
        let auth = c.has_extn("auth").ok_or_else(|| SmtpError("SMTP AUTH extension not supported by server.".into()))?;
        let methods: Vec<String> = auth.split_whitespace().map(|m| m.to_uppercase()).collect();
        let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
        // smtplib prefers CRAM-MD5, PLAIN, LOGIN; CRAM-MD5 is not offered here (no MD5).
        if methods.iter().any(|m| m == "PLAIN") {
            let raw = format!("\0{}\0{}", s.host_user, s.host_password);
            let (code, _) = c.cmd(&format!("AUTH PLAIN {}", b64(raw.as_bytes()))).await?;
            if code != 235 && code != 503 {
                return Err(SmtpError(format!("SMTPAuthenticationError({code})")));
            }
        } else if methods.iter().any(|m| m == "LOGIN") {
            let (code, _) = c.cmd(&format!("AUTH LOGIN {}", b64(s.host_user.as_bytes()))).await?;
            if code == 334 {
                let (code, _) = c.cmd(&b64(s.host_password.as_bytes())).await?;
                if code != 235 {
                    return Err(SmtpError(format!("SMTPAuthenticationError({code})")));
                }
            } else if code != 235 {
                return Err(SmtpError(format!("SMTPAuthenticationError({code})")));
            }
        } else {
            return Err(SmtpError("No suitable authentication method found.".into()));
        }
    }
    let mut mail_from = format!("mail FROM:<{}>", mail.from);
    if c.has_extn("size").is_some() {
        mail_from.push_str(&format!(" size={}", mail.data.len()));
    }
    let (code, _) = c.cmd(&mail_from).await?;
    if code != 250 {
        let _ = c.cmd("rset").await;
        return Err(SmtpError(format!("SMTPSenderRefused({code})")));
    }
    let mut accepted = 0;
    for r in &mail.recipients {
        let (code, _) = c.cmd(&format!("rcpt TO:<{r}>")).await?;
        if code == 250 || code == 251 {
            accepted += 1;
        }
    }
    if accepted == 0 {
        let _ = c.cmd("rset").await;
        return Err(SmtpError("SMTPRecipientsRefused".into()));
    }
    let (code, _) = c.cmd("data").await?;
    if code != 354 {
        return Err(SmtpError(format!("SMTPDataError({code})")));
    }
    c.send(&quotedata(&mail.data)).await?;
    let (code, _) = c.read_reply().await?;
    if code != 250 {
        return Err(SmtpError(format!("SMTPDataError({code})")));
    }
    let _ = c.cmd("QUIT").await;
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique_names() {
        let mut used = std::collections::HashSet::new();
        assert_eq!(unique_filename("a.tar.gz", &used), "a.tar.gz");
        used.insert("a.tar.gz".to_string());
        assert_eq!(unique_filename("a.tar.gz", &used), "a.tar_01.tar.gz");
        used.insert("doc.pdf".into());
        assert_eq!(unique_filename("doc.pdf", &used), "doc_01.pdf");
    }

    #[test]
    fn dot_stuffing() {
        assert_eq!(quotedata(b"a\n.b\r\nc"), b"a\r\n..b\r\nc\r\n.\r\n".to_vec());
    }

    #[test]
    fn header_encoding() {
        assert_eq!(encode_header_value("Hello"), "Hello");
        assert!(encode_header_value("Grüße").starts_with("=?utf-8?b?"));
    }
}
