//! Email delivery via SMTP with the upstream settings.
//!
//! upstream: src/paperless/settings/__init__.py@v3.4.2 (`EMAIL_HOST`, `EMAIL_PORT`,
//! `EMAIL_HOST_USER`, `EMAIL_HOST_PASSWORD`, `EMAIL_USE_TLS`, `EMAIL_USE_SSL`,
//! `DEFAULT_FROM_EMAIL`, `EMAIL_TIMEOUT = 30`), django/core/mail/backends/smtp.py@5.2
//! (`open`: SSL or STARTTLS, `login`), django/core/mail/message.py (`EmailMessage.message`:
//! header order Content-Type, MIME-Version, Content-Transfer-Encoding, Subject, From,
//! To, Date, Message-ID). No `EMAIL_CERTIFICATE_FILE`/client certificate (contract, KD).

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::settings::Settings;

/// A text mail.
#[derive(Debug, Clone)]
pub struct Mail {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body: String,
}

fn encode_header(v: &str) -> String {
    if v.is_ascii() {
        v.to_string()
    } else {
        format!("=?utf-8?b?{}?=", base64::engine::general_purpose::STANDARD.encode(v.as_bytes()))
    }
}

/// RFC 5322 message like `EmailMessage.message().as_bytes(linesep="\r\n")`.
pub fn render(m: &Mail, date: chrono::DateTime<chrono::Utc>, msgid: &str) -> String {
    let (cte, body) = if m.body.is_ascii() { ("7bit", m.body.clone()) } else { ("8bit", m.body.clone()) };
    let body = body.replace("\r\n", "\n").replace('\n', "\r\n");
    // SMTP dot-stuffing
    let body = body.split("\r\n").map(|l| if l.starts_with('.') { format!(".{l}") } else { l.to_string() }).collect::<Vec<_>>().join("\r\n");
    format!(
        "Content-Type: text/plain; charset=\"utf-8\"\r\nMIME-Version: 1.0\r\nContent-Transfer-Encoding: {cte}\r\nSubject: {}\r\nFrom: {}\r\nTo: {}\r\nDate: {}\r\nMessage-ID: \r\n <{msgid}>\r\n\r\n{body}\r\n",
        encode_header(&m.subject),
        m.from,
        m.to,
        date.format("%a, %d %b %Y %H:%M:%S -0000"),
    )
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

async fn reply<S: AsyncRead + Unpin>(r: &mut BufReader<S>) -> Result<(u16, String), String> {
    let mut all = String::new();
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("connection closed".into());
        }
        all.push_str(&line);
        if line.len() < 4 || line.as_bytes()[3] != b'-' {
            let code = line.get(..3).and_then(|c| c.parse().ok()).ok_or("bad reply")?;
            return Ok((code, all));
        }
    }
}

async fn cmd<S: AsyncRead + AsyncWrite + Unpin>(s: &mut BufReader<S>, line: &str, expect: &[u16]) -> Result<String, String> {
    s.get_mut().write_all(format!("{line}\r\n").as_bytes()).await.map_err(|e| e.to_string())?;
    let (code, text) = reply(s).await?;
    if expect.contains(&code) { Ok(text) } else { Err(format!("SMTP {code}")) }
}

fn tls_connector() -> tokio_rustls::TlsConnector {
    let mut roots = tokio_rustls::rustls::RootCertStore::empty();
    for c in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(c);
    }
    let cfg = tokio_rustls::rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
    tokio_rustls::TlsConnector::from(Arc::new(cfg))
}

async fn session<S: Io>(stream: S, s: &Settings, m: &Mail, data: &str, starttls: bool) -> Result<(), String> {
    let mut r = BufReader::new(stream);
    let (code, _) = reply(&mut r).await?;
    if code != 220 {
        return Err(format!("SMTP greeting {code}"));
    }
    let ehlo = format!("EHLO {}", local_hostname());
    let caps = cmd(&mut r, &ehlo, &[250]).await?;
    if starttls {
        cmd(&mut r, "STARTTLS", &[220]).await?;
        let name = tokio_rustls::rustls::pki_types::ServerName::try_from(s.email_host.clone()).map_err(|e| e.to_string())?;
        let tls = tls_connector().connect(name, r.into_inner()).await.map_err(|e| e.to_string())?;
        return Box::pin(session_after_tls(tls, s, m, data)).await;
    }
    finish(&mut r, s, m, data, &caps).await
}

async fn session_after_tls<S: Io>(stream: S, s: &Settings, m: &Mail, data: &str) -> Result<(), String> {
    let mut r = BufReader::new(stream);
    let caps = cmd(&mut r, &format!("EHLO {}", local_hostname()), &[250]).await?;
    finish(&mut r, s, m, data, &caps).await
}

async fn finish<S: AsyncRead + AsyncWrite + Unpin>(r: &mut BufReader<S>, s: &Settings, m: &Mail, data: &str, caps: &str) -> Result<(), String> {
    if !s.email_host_user.is_empty() {
        let b64 = |v: &str| base64::engine::general_purpose::STANDARD.encode(v.as_bytes());
        let auth_line = caps.lines().find(|l| l[4.min(l.len())..].to_ascii_uppercase().starts_with("AUTH")).unwrap_or("").to_ascii_uppercase();
        if auth_line.contains("PLAIN") || !auth_line.contains("LOGIN") {
            let token = b64(&format!("\0{}\0{}", s.email_host_user, s.email_host_password));
            cmd(r, &format!("AUTH PLAIN {token}"), &[235]).await?;
        } else {
            cmd(r, "AUTH LOGIN", &[334]).await?;
            cmd(r, &b64(&s.email_host_user), &[334]).await?;
            cmd(r, &b64(&s.email_host_password), &[235]).await?;
        }
    }
    cmd(r, &format!("MAIL FROM:<{}>", m.from), &[250]).await?;
    cmd(r, &format!("RCPT TO:<{}>", m.to), &[250, 251]).await?;
    cmd(r, "DATA", &[354]).await?;
    r.get_mut().write_all(data.as_bytes()).await.map_err(|e| e.to_string())?;
    cmd(r, ".", &[250]).await?;
    let _ = cmd(r, "QUIT", &[221]).await;
    Ok(())
}

fn local_hostname() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "localhost".into()).to_lowercase()
}

/// `EmailMessage.send()` (error → exception → 500 in Django).
pub async fn send(s: &Settings, m: &Mail) -> Result<(), String> {
    let now = chrono::Utc::now();
    let msgid = format!("{}{}.{}@{}", now.timestamp_millis() / 10, std::process::id(), rand::random::<u64>(), local_hostname());
    let data = render(m, now, &msgid);
    let addr = format!("{}:{}", s.email_host, s.email_port);
    let fut = async {
        let tcp = TcpStream::connect(&addr).await.map_err(|e| e.to_string())?;
        if s.email_use_ssl {
            let name = tokio_rustls::rustls::pki_types::ServerName::try_from(s.email_host.clone()).map_err(|e| e.to_string())?;
            let tls = tls_connector().connect(name, tcp).await.map_err(|e| e.to_string())?;
            session(tls, s, m, &data, false).await
        } else {
            session(tcp, s, m, &data, s.email_use_tls).await
        }
    };
    tokio::time::timeout(Duration::from_secs(30), fut).await.map_err(|_| "SMTP timeout".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_layout() {
        let m = Mail { from: "a@x".into(), to: "b@y".into(), subject: "[PaperRust] Reset your password".into(), body: "Hi\n.dot".into() };
        let t = chrono::DateTime::parse_from_rfc3339("2026-11-08T07:22:19Z").unwrap().with_timezone(&chrono::Utc);
        let r = render(&m, t, "1.2@h");
        assert!(r.starts_with("Content-Type: text/plain; charset=\"utf-8\"\r\nMIME-Version: 1.0\r\nContent-Transfer-Encoding: 7bit\r\nSubject: [PaperRust] Reset your password\r\nFrom: a@x\r\nTo: b@y\r\nDate: Sun, 08 Nov 2026 07:22:19 -0000\r\n"));
        assert!(r.ends_with("\r\n\r\nHi\r\n..dot\r\n"));
        assert_eq!(encode_header("Passwort zurücksetzen"), "=?utf-8?b?UGFzc3dvcnQgenVyw7xja3NldHplbg==?=");
    }
}
