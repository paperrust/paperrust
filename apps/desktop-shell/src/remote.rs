//! Remote mode 2a (docs/REMOTE-APP.md §3, §4, §7): check server address, detect PaperRust/paperless-ngx,
//! check reachability, fetch certificate and check it against the confirmed fingerprint.
//!
//! All domain logic is testable without a network: [`detect`] receives the HTTP query as a function
//! (unit tests with recorded responses), [`Fetcher`] is the real implementation with `reqwest`
//! (rustls, webpki + operating system certificates).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::Url;
use url::Host;

use crate::mode::Flavor;

// ---------------------------------------------------------------------------------------------
// Adresse

/// Where the server is – decides whether `http://` is offered at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HostScope {
    /// `127.0.0.0/8`, `::1`, `localhost`
    Loopback,
    /// RFC 1918, `fc00::/7`, `*.local`
    Private,
    Public,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerUrl {
    /// Origin + subpath, with `/` at the end (e.g. `https://docs.example.ch/` or
    /// `https://example.ch/paperless/`).
    pub base: String,
    /// Scheme + host + port (navigation allow list).
    pub origin: String,
    pub https: bool,
    pub scope: HostScope,
    /// Host in ASCII (punycode for IDN).
    pub host: String,
    /// IDN: hint "angezeigt als Punycode".
    pub idn: bool,
}

fn v4_scope(ip: Ipv4Addr) -> HostScope {
    if ip.is_loopback() {
        HostScope::Loopback
    } else if ip.is_private() {
        HostScope::Private
    } else {
        HostScope::Public
    }
}

fn ip_scope(ip: IpAddr) -> HostScope {
    match ip {
        IpAddr::V4(v4) => v4_scope(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                v4_scope(v4)
            } else if v6 == Ipv6Addr::LOCALHOST {
                HostScope::Loopback
            } else if (v6.segments()[0] & 0xfe00) == 0xfc00 {
                HostScope::Private
            } else {
                HostScope::Public
            }
        }
    }
}

pub fn host_scope(host: &Host<&str>) -> HostScope {
    match host {
        Host::Ipv4(ip) => v4_scope(*ip),
        Host::Ipv6(ip) => ip_scope(IpAddr::V6(*ip)),
        Host::Domain(d) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            if d == "localhost" || d.ends_with(".localhost") {
                HostScope::Loopback
            } else if d.ends_with(".local") {
                HostScope::Private
            } else {
                HostScope::Public
            }
        }
    }
}

/// Check and normalize input (REMOTE-APP §3.1). Without a scheme `https://` is assumed.
pub fn parse_server_url(input: &str) -> Result<ServerUrl, String> {
    let t = input.trim();
    if t.is_empty() {
        return Err("Bitte die Adresse des Servers angeben.".into());
    }
    let with_scheme = if t.contains("://") {
        t.to_string()
    } else {
        format!("https://{t}")
    };
    let u = Url::parse(&with_scheme).map_err(|e| format!("Ungültige Adresse: {e}"))?;
    let https = match u.scheme() {
        "https" => true,
        "http" => false,
        s => {
            return Err(format!(
                "Nur https:// (oder http:// im eigenen Netz), nicht {s}://."
            ));
        }
    };
    if !u.username().is_empty() || u.password().is_some() {
        return Err("Die Adresse darf keinen Benutzernamen/Passwort enthalten (user@…). Die Anmeldung folgt im Fenster.".into());
    }
    if u.query().is_some() || u.fragment().is_some() {
        return Err("Die Adresse darf kein ?… oder #… enthalten.".into());
    }
    let host = u.host().ok_or("Die Adresse enthält keinen Servernamen.")?;
    let scope = host_scope(&host);
    let host_ascii = u.host_str().unwrap_or_default().to_string();
    let mut path = u.path().to_string();
    if !path.ends_with('/') {
        path.push('/');
    }
    let origin = u.origin().ascii_serialization();
    Ok(ServerUrl {
        base: format!("{origin}{path}"),
        origin,
        https,
        scope,
        idn: host_ascii.split('.').any(|l| l.starts_with("xn--")),
        host: host_ascii,
    })
}

/// Origin of a confirmed additional address (SSO): only `https://host[:port]`, otherwise `None`.
pub fn parse_origin(s: &str) -> Option<String> {
    let u = Url::parse(s.trim()).ok()?;
    (u.scheme() == "https" && u.host_str().is_some()).then(|| u.origin().ascii_serialization())
}

/// May this address be used – and does it need the http confirmation?
pub fn http_policy(u: &ServerUrl) -> Result<bool, String> {
    if u.https {
        return Ok(false);
    }
    match u.scope {
        HostScope::Public => Err(
            "Unverschlüsseltes http:// ist nur für diesen Computer und private Netze (192.168.…, 10.…, 172.16–31.…, *.local) erlaubt. Bitte https:// verwenden."
                .into(),
        ),
        HostScope::Loopback | HostScope::Private => Ok(true),
    }
}

// ---------------------------------------------------------------------------------------------
// Certificate

/// SHA-256 as hex (lower case, without separator) – format of `tls_pin`.
pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Display fingerprint as in the browser/`openssl x509 -fingerprint -sha256` (AA:BB:…).
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

pub fn valid_pin(pin: &str) -> bool {
    pin.len() == 64 && pin.bytes().all(|b| b.is_ascii_hexdigit())
}

/// A DER element: (tag, content, rest).
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

/// Fields of an X.509 certificate, as far as the shell needs them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CertInfo {
    /// SHA-256 of the whole certificate, display `AA:BB:…`.
    pub fingerprint: String,
    /// SHA-256 of the SubjectPublicKeyInfo (hex) → `tls_pin`.
    pub spki_sha256: String,
    pub subject_cn: String,
    pub issuer_cn: String,
    pub not_after: String,
    /// Issuer = subject (self-signed).
    pub self_signed: bool,
}

fn common_name(name: &[u8]) -> String {
    // Name ::= SEQUENCE OF RelativeDistinguishedName (SET OF AttributeTypeAndValue)
    const CN: &[u8] = &[0x55, 0x04, 0x03];
    let mut rest = name;
    while let Some((_, set, r, _)) = der_next(rest) {
        rest = r;
        let mut s = set;
        while let Some((_, atv, r2, _)) = der_next(s) {
            s = r2;
            if let Some((0x06, oid, val, _)) = der_next(atv)
                && oid == CN
                && let Some((_, v, _, _)) = der_next(val)
            {
                return String::from_utf8_lossy(v).into_owned();
            }
        }
    }
    String::new()
}

fn der_time(t: &[u8]) -> String {
    // UTCTime YYMMDDhhmmssZ / GeneralizedTime YYYYMMDDhhmmssZ → YYYY-MM-DD hh:mm UTC
    let s = String::from_utf8_lossy(t);
    let s = s.trim_end_matches('Z');
    let full = if s.len() == 12 {
        let yy: u32 = s[..2].parse().unwrap_or(0);
        format!("{}{s}", if yy >= 50 { "19" } else { "20" })
    } else {
        s.to_string()
    };
    if full.len() >= 12 && full.is_ascii() {
        format!(
            "{}-{}-{} {}:{} UTC",
            &full[..4],
            &full[4..6],
            &full[6..8],
            &full[8..10],
            &full[10..12]
        )
    } else {
        full
    }
}

/// Read X.509 DER (only the fields above; without signature check – the trust decision is made by
/// the user via the fingerprint).
pub fn cert_info(der: &[u8]) -> Option<CertInfo> {
    let (0x30, cert, _, _) = der_next(der)? else {
        return None;
    };
    let (0x30, tbs, _, _) = der_next(cert)? else {
        return None;
    };
    let mut rest = tbs;
    // [0] version (optional)
    if rest.first() == Some(&0xa0) {
        rest = der_next(rest)?.2;
    }
    let (_, _, r, _) = der_next(rest)?; // serialNumber
    let (_, _, r, _) = der_next(r)?; // signature
    let (_, issuer, r, _) = der_next(r)?;
    let (_, validity, r, _) = der_next(r)?;
    let (_, subject, r, _) = der_next(r)?;
    let (0x30, _, _, spki_len) = der_next(r)? else {
        return None;
    };
    let spki = &r[..spki_len];
    let (_, _nb, after, _) = der_next(validity)?;
    let (_, na, _, _) = der_next(after)?;
    Some(CertInfo {
        fingerprint: fingerprint(der),
        spki_sha256: sha256_hex(spki),
        subject_cn: common_name(subject),
        issuer_cn: common_name(issuer),
        not_after: der_time(na),
        self_signed: issuer == subject,
    })
}

/// SPKI pin of a DER certificate.
pub fn spki_pin(der: &[u8]) -> Option<String> {
    cert_info(der).map(|c| c.spki_sha256)
}

/// PEM (WebView2 `ToPemEncoding`) → DER of the first certificate.
pub fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let start = pem.find("-----BEGIN CERTIFICATE-----")? + "-----BEGIN CERTIFICATE-----".len();
    let end = start + pem[start..].find("-----END CERTIFICATE-----")?;
    let b64: String = pem[start..end]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    base64::engine::general_purpose::STANDARD.decode(b64).ok()
}

// ---------------------------------------------------------------------------------------------
// HTTP

#[derive(Debug, Clone, Default)]
pub struct Resp {
    pub status: u16,
    pub content_type: String,
    pub body: String,
    pub x_version: Option<String>,
}

/// Why the certificate was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TlsKind {
    /// Unknown issuer / self-signed → offer confirmation.
    Untrusted,
    /// Name does not match → offer confirmation (pin secures the identity).
    Name,
    /// Expired/not yet valid/revoked → hard error.
    Expired,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// No connection (DNS, refused, timeout).
    Connect(String),
    Tls(TlsKind, String),
    /// Certificate ≠ confirmed fingerprint.
    PinMismatch {
        got: String,
    },
    Other(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "Server nicht erreichbar: {e}"),
            Self::Tls(TlsKind::Expired, e) => write!(f, "Zertifikat abgelaufen oder ungültig: {e}"),
            Self::Tls(_, e) => write!(f, "Zertifikat nicht vertrauenswürdig: {e}"),
            Self::PinMismatch { .. } => write!(
                f,
                "Das Zertifikat des Servers hat sich geändert und passt nicht mehr zum bestätigten Fingerabdruck. Verbindung abgebrochen."
            ),
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

/// Error chain of reqwest/hyper/rustls as text.
fn chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        let t = c.to_string();
        if !s.contains(&t) {
            s.push_str(": ");
            s.push_str(&t);
        }
        cur = c.source();
    }
    s
}

/// rustls reports certificate errors as `invalid peer certificate: <kind>` (Display of
/// `rustls::Error::InvalidCertificate`); the kind is only in the text (io::Error chain).
pub fn classify_error(text: &str, is_connect: bool, is_timeout: bool) -> FetchError {
    let t = text.to_ascii_lowercase();
    if let Some(i) = t.find("invalid peer certificate") {
        let kind = &t[i..];
        let k =
            if kind.contains("expired") || kind.contains("notvalidyet") || kind.contains("revoked")
            {
                TlsKind::Expired
            } else if kind.contains("unknownissuer")
                || kind.contains("selfsigned")
                || kind.contains("unknown issuer")
            {
                TlsKind::Untrusted
            } else if kind.contains("notvalidforname") || kind.contains("not valid for name") {
                TlsKind::Name
            } else {
                TlsKind::Other
            };
        return FetchError::Tls(k, text.to_string());
    }
    if is_connect || is_timeout {
        FetchError::Connect(text.to_string())
    } else {
        FetchError::Other(text.to_string())
    }
}

/// Real HTTP queries to a server (without cookies, without redirects).
pub struct Fetcher {
    client: reqwest::Client,
    base: Url,
    pin: Option<String>,
}

const UA: &str = concat!("PaperRust-Desktop/", env!("CARGO_PKG_VERSION"));

impl Fetcher {
    /// `pin` set: system certificate check off, instead the SPKI pin of the
    /// server certificate must match (every response is checked before it is used).
    pub fn new(base: &str, pin: Option<&str>) -> Result<Self, String> {
        let base = Url::parse(base).map_err(|e| e.to_string())?;
        let mut b = reqwest::Client::builder()
            .use_rustls_tls()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .user_agent(UA);
        let pin = pin.filter(|p| !p.is_empty()).map(str::to_string);
        if pin.is_some() {
            b = b.danger_accept_invalid_certs(true).tls_info(true);
        }
        Ok(Self {
            client: b.build().map_err(|e| e.to_string())?,
            base,
            pin,
        })
    }

    pub async fn get(&self, path: &str) -> Result<Resp, FetchError> {
        let url = self
            .base
            .join(path)
            .map_err(|e| FetchError::Other(e.to_string()))?;
        let r = self
            .client
            .get(url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| classify_error(&chain(&e), e.is_connect(), e.is_timeout()))?;
        if let Some(pin) = &self.pin {
            let got = r
                .extensions()
                .get::<reqwest::tls::TlsInfo>()
                .and_then(|t| t.peer_certificate())
                .and_then(spki_pin);
            match got {
                Some(g) if &g == pin => {}
                // http (no TLS) with pin: not possible, pins exist only for https.
                Some(g) => return Err(FetchError::PinMismatch { got: g }),
                None if self.base.scheme() == "https" => {
                    return Err(FetchError::PinMismatch { got: String::new() });
                }
                None => {}
            }
        }
        let status = r.status().as_u16();
        let hv = |n: &str| {
            r.headers()
                .get(n)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let content_type = hv("content-type").unwrap_or_default();
        let x_version = hv("x-version");
        let body = r.text().await.unwrap_or_default();
        Ok(Resp {
            status,
            content_type,
            body: body.chars().take(64 * 1024).collect(),
            x_version,
        })
    }

    /// Fetch the server's certificate (without check) – for the confirmation dialog.
    pub async fn peer_certificate(base: &str) -> Result<CertInfo, String> {
        let c = reqwest::Client::builder()
            .use_rustls_tls()
            .danger_accept_invalid_certs(true)
            .tls_info(true)
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .user_agent(UA)
            .build()
            .map_err(|e| e.to_string())?;
        let r = c.get(base).send().await.map_err(|e| chain(&e))?;
        let der = r
            .extensions()
            .get::<reqwest::tls::TlsInfo>()
            .and_then(|t| t.peer_certificate())
            .ok_or("Server hat kein Zertifikat geliefert")?;
        cert_info(der).ok_or_else(|| "Zertifikat nicht lesbar".to_string())
    }
}

// ---------------------------------------------------------------------------------------------
// Erkennung

/// An anonymous GET query relative to the base address (real: [`Fetcher`], tests: recorded).
pub trait Get {
    fn get(&self, path: &'static str) -> impl Future<Output = Result<Resp, FetchError>> + Send;
}

impl Get for Fetcher {
    fn get(&self, path: &'static str) -> impl Future<Output = Result<Resp, FetchError>> + Send {
        Fetcher::get(self, path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Detected {
    pub flavor: Flavor,
    pub version: Option<String>,
    /// Hint (e.g. "Version nicht geprüft").
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetectError {
    Fetch(FetchError),
    /// Reachable, but neither PaperRust nor paperless-ngx.
    Unknown(String),
}

impl std::fmt::Display for DetectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fetch(e) => e.fmt(f),
            Self::Unknown(why) => write!(
                f,
                "Unter dieser Adresse läuft kein PaperRust- oder paperless-ngx-Server ({why})."
            ),
        }
    }
}

fn json(r: &Resp) -> Option<serde_json::Value> {
    if !r.content_type.to_ascii_lowercase().contains("json") {
        return None;
    }
    serde_json::from_str::<serde_json::Value>(&r.body)
        .ok()
        .filter(serde_json::Value::is_object)
}

fn ngx_note(version: Option<&str>) -> Option<String> {
    let v = version?.trim_start_matches(['v', 'V']);
    let major: u32 = v.split('.').next()?.parse().ok()?;
    (major < 2).then(|| format!("paperless-ngx {v} ist älter als 2.x – nicht geprüft, einzelne Funktionen können fehlen."))
}

fn clean_version(v: Option<&str>) -> Option<String> {
    v.map(|s| s.trim().trim_start_matches(['v', 'V']).to_string())
        .filter(|s| !s.is_empty() && s != "0.0.0")
}

/// Detection (REMOTE-APP §3.2), in this order:
/// 1. `GET rs/api/v2/server` → 200 JSON → PaperRust (API v2, from P3).
/// 2. `GET api/remote_version/` (anonymous) → JSON with object `paperrust` → PaperRust (v1 server of
///    today); only `version` → paperless-ngx.
/// 3. `GET api/ui_settings/` → 401/403 with DRF JSON `{"detail": …}` (or 200 with `settings`) →
///    paperless-ngx (older versions where `remote_version` requires a login).
///
/// Connection/certificate errors of the first query abort immediately.
pub async fn detect<G: Get + Sync>(g: &G) -> Result<Detected, DetectError> {
    let v2 = g
        .get("rs/api/v2/server")
        .await
        .map_err(DetectError::Fetch)?;
    if v2.status == 200
        && let Some(j) = json(&v2)
    {
        let version = j.get("version").and_then(|v| v.as_str()).or_else(|| {
            j.pointer("/paperrust/current_version")
                .and_then(|v| v.as_str())
        });
        return Ok(Detected {
            flavor: Flavor::Paperrust,
            version: clean_version(version),
            note: None,
        });
    }
    let mut seen = vec![format!("rs/api/v2/server → {}", v2.status)];
    match g.get("api/remote_version/").await {
        Ok(r) => {
            if r.status == 200
                && let Some(j) = json(&r)
            {
                if let Some(p) = j.get("paperrust").filter(|p| p.is_object()) {
                    return Ok(Detected {
                        flavor: Flavor::Paperrust,
                        version: clean_version(p.get("current_version").and_then(|v| v.as_str())),
                        note: None,
                    });
                }
                if j.get("version").is_some() {
                    let v = clean_version(j.get("version").and_then(|v| v.as_str()))
                        .or_else(|| clean_version(r.x_version.as_deref()));
                    return Ok(Detected {
                        note: ngx_note(v.as_deref()),
                        flavor: Flavor::Ngx,
                        version: v,
                    });
                }
            }
            seen.push(format!("api/remote_version/ → {}", r.status));
        }
        Err(e @ (FetchError::Tls(..) | FetchError::PinMismatch { .. })) => {
            return Err(DetectError::Fetch(e));
        }
        Err(e) => seen.push(format!("api/remote_version/ → {e}")),
    }
    let r = g
        .get("api/ui_settings/")
        .await
        .map_err(DetectError::Fetch)?;
    if let Some(j) = json(&r) {
        let drf = matches!(r.status, 401 | 403) && j.get("detail").is_some_and(|d| d.is_string());
        if drf || (r.status == 200 && j.get("settings").is_some()) {
            let v = clean_version(r.x_version.as_deref());
            return Ok(Detected {
                note: ngx_note(v.as_deref()).or(Some(
                    "Version erst nach der Anmeldung bekannt (paperless-ngx < 2.x wird nicht geprüft).".into(),
                )),
                flavor: Flavor::Ngx,
                version: v,
            });
        }
    }
    seen.push(format!("api/ui_settings/ → {}", r.status));
    Err(DetectError::Unknown(seen.join(", ")))
}

/// Reachability (offline detection, REMOTE-APP §7): every HTTP response < 500 counts as "online"
/// (the login page answers anonymously); 502/503/504 of a proxy in front = server gone.
pub async fn probe(f: &Fetcher) -> Result<(), FetchError> {
    let r = f.get("accounts/login/").await?;
    if r.status >= 500 {
        return Err(FetchError::Connect(format!("HTTP {}", r.status)));
    }
    Ok(())
}

/// Next wait time on automatic retry: 15 s, doubled, at most 2 min.
pub fn backoff(attempt: u32) -> Duration {
    let s = 15u64.saturating_mul(1 << attempt.min(4));
    Duration::from_secs(s.min(120))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_checks() {
        let ok = |s: &str| parse_server_url(s).unwrap();
        let u = ok("docs.example.ch");
        assert_eq!(u.base, "https://docs.example.ch/");
        assert_eq!(u.origin, "https://docs.example.ch");
        assert!(u.https && u.scope == HostScope::Public && !u.idn);
        let u = ok(" https://Example.ch:8443/paperless ");
        assert_eq!(
            u.base, "https://example.ch:8443/paperless/",
            "Unterpfad bleibt Teil der Basis"
        );
        assert_eq!(u.origin, "https://example.ch:8443");
        assert_eq!(ok("https://example.ch:443/").origin, "https://example.ch");
        // IDN → punycode
        let u = ok("https://bücher.example/");
        assert_eq!(u.host, "xn--bcher-kva.example");
        assert!(u.idn);
        for bad in [
            "",
            "ftp://example.ch",
            "https://user@example.ch",
            "https://user:pw@example.ch",
            "https://example.ch/?x=1",
            "https://example.ch/#a",
            "javascript:alert(1)",
            "file:///C:/x",
            "https://",
        ] {
            assert!(parse_server_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn http_only_local_networks() {
        let pol = |s: &str| http_policy(&parse_server_url(s).unwrap());
        assert_eq!(pol("https://example.ch"), Ok(false));
        for local in [
            "http://127.0.0.1:8000",
            "http://127.8.9.10",
            "http://localhost:8000",
            "http://[::1]:8000",
            "http://192.168.1.20:8000",
            "http://10.1.2.3",
            "http://172.16.0.1",
            "http://172.31.255.255",
            "http://nas.local",
            "http://[fd12:3456::1]",
            "http://[::ffff:192.168.0.1]",
        ] {
            assert_eq!(pol(local), Ok(true), "{local}");
        }
        for public in [
            "http://example.ch",
            "http://8.8.8.8",
            "http://172.32.0.1",
            "http://100.64.0.1",
            "http://[2001:db8::1]",
            "http://0.0.0.0",
            "http://192.168.1.20.nip.io",
            "http://local.example.ch",
        ] {
            assert!(pol(public).is_err(), "{public}");
        }
        let s = |u: &str| parse_server_url(u).unwrap().scope;
        assert_eq!(s("http://127.0.0.1"), HostScope::Loopback);
        assert_eq!(s("http://192.168.0.1"), HostScope::Private);
    }

    #[test]
    fn origins() {
        assert_eq!(
            parse_origin("https://sso.example.ch/realms/x"),
            Some("https://sso.example.ch".into())
        );
        assert_eq!(parse_origin("http://sso.example.ch"), None);
        assert_eq!(parse_origin("javascript:alert(1)"), None);
    }

    fn resp(status: u16, ct: &str, body: &str) -> Resp {
        Resp {
            status,
            content_type: ct.into(),
            body: body.into(),
            x_version: None,
        }
    }

    /// Recorded responses (2026-11-09): PaperRust 0.1.0 (standalone, :8191) and
    /// paperless-ngx 3.4.2 (reference Django :8000), each anonymous.
    fn paperrust(path: &str) -> Result<Resp, FetchError> {
        Ok(match path {
            "rs/api/v2/server" => resp(302, "text/html; charset=utf-8", ""),
            "api/remote_version/" => resp(
                200,
                "application/json",
                r#"{"version":"0.0.0","update_available":false,"paperrust":{"current_version":"0.1.0","latest_version":null,"status":"unknown","releases_url":"https://github.com/x/y/releases"}}"#,
            ),
            "api/ui_settings/" => resp(
                401,
                "application/json",
                r#"{"detail":"Authentication credentials were not provided."}"#,
            ),
            _ => resp(404, "text/html", ""),
        })
    }

    fn ngx342(path: &str) -> Result<Resp, FetchError> {
        Ok(match path {
            "rs/api/v2/server" => resp(302, "text/html; charset=utf-8", ""),
            "api/remote_version/" => resp(
                200,
                "application/json",
                r#"{"version":"v3.4.2","update_available":false}"#,
            ),
            "api/ui_settings/" => resp(
                401,
                "application/json",
                r#"{"detail":"Authentication credentials were not provided."}"#,
            ),
            _ => resp(404, "text/html", ""),
        })
    }

    fn block<T>(f: impl std::future::Future<Output = T>) -> T {
        tauri::async_runtime::block_on(f)
    }

    /// Recorded responses as `Get`.
    struct Rec(fn(&str) -> Result<Resp, FetchError>);

    impl Get for Rec {
        fn get(&self, path: &'static str) -> impl Future<Output = Result<Resp, FetchError>> + Send {
            std::future::ready((self.0)(path))
        }
    }

    fn run(f: fn(&str) -> Result<Resp, FetchError>) -> Result<Detected, DetectError> {
        block(detect(&Rec(f)))
    }

    #[test]
    fn detects_paperrust_v1() {
        let d = run(|p: &str| paperrust(p)).unwrap();
        assert_eq!(d.flavor, Flavor::Paperrust);
        assert_eq!(d.version.as_deref(), Some("0.1.0"));
    }

    #[test]
    fn detects_paperrust_v2() {
        let d = run(|p: &str| {
            if p == "rs/api/v2/server" {
                Ok(resp(
                    200,
                    "application/json",
                    r#"{"product":"paperrust","version":"0.2.0"}"#,
                ))
            } else {
                panic!("nach v2 nichts mehr abfragen: {p}")
            }
        })
        .unwrap();
        assert_eq!(d.flavor, Flavor::Paperrust);
        assert_eq!(d.version.as_deref(), Some("0.2.0"));
    }

    #[test]
    fn detects_ngx() {
        let d = run(|p: &str| ngx342(p)).unwrap();
        assert_eq!(d.flavor, Flavor::Ngx);
        assert_eq!(d.version.as_deref(), Some("3.4.2"));
        assert!(d.note.is_none());
        // older version, remote_version only with login → detected via ui_settings
        let d = run(|p: &str| match p {
            "api/remote_version/" | "api/ui_settings/" => Ok(Resp {
                x_version: Some("1.17.4".into()),
                ..resp(
                    401,
                    "application/json",
                    r#"{"detail":"Anmeldedaten fehlen."}"#,
                )
            }),
            _ => Ok(resp(404, "text/html", "")),
        })
        .unwrap();
        assert_eq!(d.flavor, Flavor::Ngx);
        assert!(d.note.unwrap().contains("nicht geprüft"));
        assert!(ngx_note(Some("v1.17.4")).is_some());
        assert!(ngx_note(Some("2.0.0")).is_none());
    }

    #[test]
    fn foreign_server_is_rejected() {
        // any web server: HTML everywhere
        let e = run(|_p: &str| Ok(resp(200, "text/html", "<html>"))).unwrap_err();
        assert!(matches!(e, DetectError::Unknown(_)));
        assert!(e.to_string().contains("kein PaperRust"));
        // JSON, but not DRF
        let e = run(|_p: &str| Ok(resp(401, "application/json", r#"{"error":"x"}"#))).unwrap_err();
        assert!(matches!(e, DetectError::Unknown(_)));
    }

    #[test]
    fn fetch_errors_stop_detection() {
        let e = run(|_p: &str| Err(FetchError::Connect("refused".into()))).unwrap_err();
        assert!(matches!(e, DetectError::Fetch(FetchError::Connect(_))));
        let e = run(|p: &str| {
            if p == "rs/api/v2/server" {
                Ok(resp(404, "text/html", ""))
            } else {
                Err(FetchError::PinMismatch { got: "x".into() })
            }
        })
        .unwrap_err();
        assert!(matches!(
            e,
            DetectError::Fetch(FetchError::PinMismatch { .. })
        ));
    }

    #[test]
    fn rustls_error_texts() {
        let c = |t: &str| classify_error(t, true, false);
        assert!(matches!(
            c(
                "error sending request: client error (Connect): invalid peer certificate: UnknownIssuer"
            ),
            FetchError::Tls(TlsKind::Untrusted, _)
        ));
        assert!(matches!(
            c("invalid peer certificate: Expired"),
            FetchError::Tls(TlsKind::Expired, _)
        ));
        assert!(matches!(
            c("invalid peer certificate: ExpiredContext { time: .. }"),
            FetchError::Tls(TlsKind::Expired, _)
        ));
        assert!(matches!(
            c("invalid peer certificate: NotValidForNameContext { expected: .. }"),
            FetchError::Tls(TlsKind::Name, _)
        ));
        assert!(matches!(
            c("tcp connect error: refused"),
            FetchError::Connect(_)
        ));
        assert!(matches!(
            classify_error("x", false, false),
            FetchError::Other(_)
        ));
    }

    fn self_signed(name: &str, days_valid: i64) -> (Vec<u8>, Vec<u8>) {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut p = rcgen::CertificateParams::new(vec![name.to_string()]).unwrap();
        p.distinguished_name.push(rcgen::DnType::CommonName, name);
        let now = std::time::SystemTime::now();
        let off = std::time::Duration::from_secs(days_valid.unsigned_abs() * 86_400);
        let t = if days_valid >= 0 {
            now + off
        } else {
            now - off
        };
        p.not_after = t.into();
        p.not_before = (now - std::time::Duration::from_secs(2 * 86_400 * 400)).into();
        let cert = p.self_signed(&key).unwrap();
        (cert.der().to_vec(), key.public_key_der())
    }

    #[test]
    fn certificate_fields_and_pin() {
        // Test certificate generated at runtime (rcgen), REMOTE-APP §8.
        let (der, spki) = self_signed("nas.local", 30);
        let c = cert_info(&der).unwrap();
        assert_eq!(
            c.spki_sha256,
            sha256_hex(&spki),
            "Pin = SHA-256 des SubjectPublicKeyInfo"
        );
        assert!(valid_pin(&c.spki_sha256));
        assert_eq!(c.subject_cn, "nas.local");
        assert_eq!(c.issuer_cn, "nas.local");
        assert!(c.self_signed);
        assert_eq!(c.fingerprint.len(), 32 * 3 - 1);
        assert!(c.not_after.ends_with("UTC"), "{}", c.not_after);
        // other key pair → other pin
        let (der2, _) = self_signed("nas.local", 30);
        assert_ne!(spki_pin(&der2).unwrap(), c.spki_sha256);
        // PEM (WebView2) → DER
        let pem = format!(
            "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
            {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(&der)
            }
        );
        assert_eq!(pem_to_der(&pem).unwrap(), der);
        assert!(cert_info(b"\x30\x03\x02\x01").is_none());
        assert!(cert_info(&[]).is_none());
    }

    #[test]
    fn backoff_grows_to_two_minutes() {
        assert_eq!(backoff(0).as_secs(), 15);
        assert_eq!(backoff(1).as_secs(), 30);
        assert_eq!(backoff(2).as_secs(), 60);
        assert_eq!(backoff(3).as_secs(), 120);
        assert_eq!(backoff(40).as_secs(), 120);
    }
}
