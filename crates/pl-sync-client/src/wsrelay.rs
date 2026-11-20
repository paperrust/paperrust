//! WebSocket relay for `/ws/status/` in mode 2b (processing/upload progress).
//!
//! Own URI schemes (`prapi://`, WebView2 `http://prapi.localhost`) cannot answer
//! WebSocket connections. The core therefore listens on `127.0.0.1:<random>`; the
//! light UI fetches the address via `prapi …/local/ws` (app origin only) and connects to
//! `ws://127.0.0.1:<port>/ws/status/?k=<secret>`.
//!
//! Per connection:
//! * check **before** anything else: path `/ws/status/`, key `k` (random per start,
//!   constant-time comparison), `Origin` = app origin, `Upgrade: websocket` – otherwise `403`;
//! * offline / not logged in / paperless-ngx (socket only with session cookie) → `503`, the
//!   light UI tries again later (gently, without an error message);
//! * online: new connection to the server (`<base>ws/status/`, TLS with the same check/pinning as
//!   the core), handshake with the key/version of the window and **`Authorization: Bearer
//!   prt_…`** (token stays in the core; the window never sees it), no cookies, no `Origin`.
//!   If the server answers with `101`, only bytes are copied from then on (WebSocket frames
//!   unchanged in both directions; `Sec-WebSocket-Accept` matches because the key is passed
//!   through). Other responses → `502` to the window.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;

use crate::gateway::Gateway;

/// Path of the status socket (as on the server).
pub const PATH: &str = "/ws/status/";

/// Upper limit for handshake heads (window and server).
const MAX_HEAD: usize = 16 * 1024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Address of the relay (for `/local/ws` and the CSP of the app pages).
#[derive(Debug, Clone)]
pub struct RelayInfo {
    pub port: u16,
    secret: String,
}

impl RelayInfo {
    pub fn new(port: u16, secret: String) -> RelayInfo {
        RelayInfo { port, secret }
    }

    /// `ws://127.0.0.1:<port>/ws/status/?k=<geheim>`
    pub fn url(&self) -> String {
        format!("ws://127.0.0.1:{}{PATH}?k={}", self.port, self.secret)
    }

    /// Source for `connect-src` of the app pages.
    pub fn csp_source(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }

    fn key_ok(&self, k: &str) -> bool {
        let (a, b) = (self.secret.as_bytes(), k.as_bytes());
        a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
    }
}

/// Random secret (2 × UUIDv4 = 244 bits of randomness).
fn secret() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

/// Auf `127.0.0.1:0` lauschen.
pub async fn bind() -> std::io::Result<(TcpListener, RelayInfo)> {
    let l = TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = l.local_addr()?.port();
    Ok((l, RelayInfo::new(port, secret())))
}

/// Accept loop until `stop` (true) or the sender is gone.
pub async fn serve(listener: TcpListener, info: RelayInfo, gw: Arc<Gateway>, origins: Vec<String>, mut stop: watch::Receiver<bool>) {
    let origins = Arc::new(origins);
    let info = Arc::new(info);
    loop {
        tokio::select! {
            r = stop.changed() => {
                if r.is_err() || *stop.borrow() {
                    break;
                }
            }
            a = listener.accept() => match a {
                Ok((sock, _)) => {
                    let (gw, origins, info) = (gw.clone(), origins.clone(), info.clone());
                    tokio::spawn(async move {
                        if let Err(e) = connection(sock, &info, &gw, &origins).await {
                            tracing::debug!("ws relay: {e}");
                        }
                    });
                }
                Err(e) => {
                    tracing::warn!("ws relay accept: {e}");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            },
        }
    }
}

/// Read the head of an HTTP/1.1 message up to `\r\n\r\n`; return the rest (already read frames) as well.
async fn read_head<S: AsyncRead + Unpin>(s: &mut S) -> std::io::Result<(Vec<u8>, Vec<u8>)> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(i + 4);
            return Ok((buf, rest));
        }
        if buf.len() > MAX_HEAD {
            return Err(std::io::Error::other("head too large"));
        }
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "closed during handshake"));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Request of the window (only what the relay needs).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Handshake {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
}

impl Handshake {
    pub fn parse(head: &[u8]) -> Option<Handshake> {
        let text = std::str::from_utf8(head).ok()?;
        let mut lines = text.split("\r\n");
        let mut first = lines.next()?.split(' ');
        let method = first.next()?.to_string();
        let target = first.next()?.to_string();
        if !first.next()?.starts_with("HTTP/1.1") {
            return None;
        }
        let headers = lines
            .filter(|l| !l.is_empty())
            .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())))
            .collect();
        Some(Handshake { method, target, headers })
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// Result of checking the window request.
#[derive(Debug, PartialEq, Eq)]
pub enum Check {
    Ok,
    Forbidden,
}

/// Check path, key, origin, upgrade (before any connection to the server).
pub fn check(h: &Handshake, info: &RelayInfo, origins: &[String]) -> Check {
    let (path, query) = h.target.split_once('?').unwrap_or((h.target.as_str(), ""));
    let key = url::form_urlencoded::parse(query.as_bytes()).find(|(k, _)| k == "k").map(|(_, v)| v.into_owned());
    let origin = h.header("origin").map(|o| o.trim_end_matches('/'));
    let upgrade = h.header("upgrade").is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
    let ok = h.method == "GET"
        && path == PATH
        && key.as_deref().is_some_and(|k| info.key_ok(k))
        && origin.is_some_and(|o| origins.iter().any(|a| a.trim_end_matches('/').eq_ignore_ascii_case(o)))
        && upgrade
        && h.header("sec-websocket-key").is_some();
    if ok { Check::Ok } else { Check::Forbidden }
}

/// Handshake to the server (without cookies/origin; `authorization` only here).
pub fn upstream_request(h: &Handshake, base: &url::Url, authorization: &str) -> String {
    let host = match base.port() {
        Some(p) => format!("{}:{p}", base.host_str().unwrap_or("")),
        None => base.host_str().unwrap_or("").to_string(),
    };
    let mut r = format!(
        "GET {}ws/status/ HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nUser-Agent: {}\r\n",
        base.path(),
        crate::remote::paperrust::USER_AGENT
    );
    for name in ["sec-websocket-key", "sec-websocket-version", "sec-websocket-extensions", "sec-websocket-protocol"] {
        if let Some(v) = h.header(name).filter(|v| !v.contains(['\r', '\n'])) {
            r.push_str(&format!("{name}: {v}\r\n"));
        }
    }
    r.push_str(&format!("Authorization: {authorization}\r\n\r\n"));
    r
}

fn status_of(head: &[u8]) -> Option<u16> {
    let line = head.split(|b| *b == b'\r').next()?;
    let text = std::str::from_utf8(line).ok()?;
    let mut parts = text.split(' ');
    if !parts.next()?.starts_with("HTTP/1.") {
        return None;
    }
    parts.next()?.parse().ok()
}

async fn reply<S: AsyncWrite + Unpin>(s: &mut S, status: &str) -> std::io::Result<()> {
    let msg = format!("HTTP/1.1 {status}\r\nContent-Length: 0\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n");
    s.write_all(msg.as_bytes()).await?;
    s.shutdown().await
}

trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

/// Connection to the server (http or https with the TLS configuration of the core).
async fn connect_upstream(base: &url::Url, tls: &crate::tls::TlsOptions) -> std::io::Result<Box<dyn Io>> {
    let host = base.host_str().ok_or_else(|| std::io::Error::other("no host"))?.to_string();
    let port = base.port_or_known_default().unwrap_or(80);
    let tcp = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect((host.trim_matches(['[', ']']), port)))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "connect timeout"))??;
    let _ = tcp.set_nodelay(true);
    if base.scheme() != "https" {
        return Ok(Box::new(tcp));
    }
    let mut cfg = crate::tls::client_config(tls).map_err(std::io::Error::other)?;
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let name = rustls::pki_types::ServerName::try_from(host.trim_matches(['[', ']']).to_string()).map_err(std::io::Error::other)?;
    let s = tokio_rustls::TlsConnector::from(Arc::new(cfg)).connect(name, tcp).await?;
    Ok(Box::new(s))
}

async fn connection(mut client: TcpStream, info: &RelayInfo, gw: &Gateway, origins: &[String]) -> std::io::Result<()> {
    let _ = client.set_nodelay(true);
    let (head, early) = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_head(&mut client))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "handshake timeout"))??;
    let Some(h) = Handshake::parse(&head) else {
        return reply(&mut client, "400 Bad Request").await;
    };
    if check(&h, info, origins) != Check::Ok {
        return reply(&mut client, "403 Forbidden").await;
    }
    let auth = gw.client().ws_authorization();
    let Some(auth) = auth.filter(|_| gw.online()) else {
        return reply(&mut client, "503 Service Unavailable").await;
    };
    let base = gw.client().base_url().clone();
    let mut up = match connect_upstream(&base, &gw.client().config().tls).await {
        Ok(s) => s,
        Err(e) => {
            tracing::debug!("ws relay upstream: {e}");
            return reply(&mut client, "503 Service Unavailable").await;
        }
    };
    let req = zeroize::Zeroizing::new(upstream_request(&h, &base, &auth));
    up.write_all(req.as_bytes()).await?;
    if !early.is_empty() {
        up.write_all(&early).await?;
    }
    let (resp, rest) = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_head(&mut up))
        .await
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "upstream handshake timeout"))??;
    match status_of(&resp) {
        Some(101) => {}
        Some(401 | 403) => {
            tracing::info!("ws relay: Server lehnt den Status-Socket ab (Geräte-Token am Socket nicht unterstützt?)");
            return reply(&mut client, "502 Bad Gateway").await;
        }
        other => {
            tracing::debug!("ws relay: upstream status {other:?}");
            return reply(&mut client, "502 Bad Gateway").await;
        }
    }
    client.write_all(&resp).await?;
    if !rest.is_empty() {
        client.write_all(&rest).await?;
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut up).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hs(target: &str, origin: Option<&str>) -> Handshake {
        let mut head = format!("GET {target} HTTP/1.1\r\nHost: 127.0.0.1:1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nCookie: sessionid=x\r\n");
        if let Some(o) = origin {
            head.push_str(&format!("Origin: {o}\r\n"));
        }
        head.push_str("\r\n");
        Handshake::parse(head.as_bytes()).unwrap()
    }

    #[test]
    fn checks_path_key_and_origin() {
        let info = RelayInfo::new(4711, "s3cr3t".into());
        let o = vec!["http://prapi.localhost".to_string(), "prapi://localhost".to_string()];
        assert_eq!(check(&hs("/ws/status/?k=s3cr3t", Some("http://prapi.localhost")), &info, &o), Check::Ok);
        assert_eq!(check(&hs("/ws/status/?k=s3cr3t", Some("prapi://localhost/")), &info, &o), Check::Ok);
        assert_eq!(check(&hs("/ws/status/?k=wrong!", Some("http://prapi.localhost")), &info, &o), Check::Forbidden);
        assert_eq!(check(&hs("/ws/status/", Some("http://prapi.localhost")), &info, &o), Check::Forbidden);
        assert_eq!(check(&hs("/ws/status/?k=s3cr3t", Some("https://evil.example")), &info, &o), Check::Forbidden);
        assert_eq!(check(&hs("/ws/status/?k=s3cr3t", None), &info, &o), Check::Forbidden);
        assert_eq!(check(&hs("/api/documents/?k=s3cr3t", Some("http://prapi.localhost")), &info, &o), Check::Forbidden);
        assert!(info.url().starts_with("ws://127.0.0.1:4711/ws/status/?k="));
        assert_eq!(info.csp_source(), "ws://127.0.0.1:4711");
    }

    #[test]
    fn upstream_handshake_carries_token_but_no_cookie_or_origin() {
        let h = hs("/ws/status/?k=x", Some("http://prapi.localhost"));
        let base = url::Url::parse("https://docs.example.ch:8443/paperless/").unwrap();
        let r = upstream_request(&h, &base, "Bearer prt_abc");
        assert!(r.starts_with("GET /paperless/ws/status/ HTTP/1.1\r\nHost: docs.example.ch:8443\r\n"));
        assert!(r.contains("sec-websocket-key: dGhlIHNhbXBsZSBub25jZQ==\r\n"));
        assert!(r.contains("Authorization: Bearer prt_abc\r\n"));
        assert!(!r.to_ascii_lowercase().contains("cookie"));
        assert!(!r.to_ascii_lowercase().contains("origin"));
        assert!(!r.contains("k=x"));
        assert!(r.ends_with("\r\n\r\n"));
        assert_eq!(status_of(b"HTTP/1.1 101 Switching Protocols\r\n"), Some(101));
        assert_eq!(status_of(b"garbage"), None);
    }
}
