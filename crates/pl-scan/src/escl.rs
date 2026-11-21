//! eSCL/AirScan client (Mopria "eSCL Scan Technical Specification" 2.x, like sane-airscan).
//!
//! | Request | Purpose |
//! |---|---|
//! | `GET  {base}ScannerCapabilities` | sources, resolutions, colour modes, formats, size |
//! | `GET  {base}ScannerStatus` | `pwg:State`, `scan:AdfState`, job states |
//! | `POST {base}ScanJobs` (ScanSettings XML) | `201` + `Location` = job; `503` = busy |
//! | `GET  {job}/NextDocument` | `200` = next page, `503` = not ready yet, `404` = no further page |
//! | `DELETE {job}` | cancel job |
//!
//! `base` always ends with `/` (e.g. `http://192.168.1.20:8080/eSCL/`). `Location` may only point to
//! the same host/port (otherwise an error, so that a device cannot redirect the server).

use std::time::Duration;

use serde::Serialize;
use url::Url;

use crate::xml::{self, Node};

/// Upper limit per page (600 dpi colour A4 as JPEG is ~10–30 MB).
pub const MAX_PAGE_BYTES: usize = 200 * 1024 * 1024;
/// Upper limit for XML responses.
const MAX_XML_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, thiserror::Error)]
pub enum ScanError {
    #[error("scanner not reachable: {0}")]
    Unreachable(String),
    #[error("scanner is busy")]
    Busy,
    #[error("document feeder is empty")]
    AdfEmpty,
    #[error("paper jam")]
    Jam,
    #[error("scan cancelled")]
    Cancelled,
    #[error("scanner does not support this setting: {0}")]
    Unsupported(String),
    #[error("unexpected scanner answer: {0}")]
    Protocol(String),
    #[error("page too large")]
    TooLarge,
    #[error("could not build the PDF: {0}")]
    Pdf(String),
    #[error("could not hand the scan to the consumer: {0}")]
    Intake(String),
}

impl ScanError {
    /// Fixed code for the UI (translation there).
    pub fn code(&self) -> &'static str {
        match self {
            ScanError::Unreachable(_) => "unreachable",
            ScanError::Busy => "busy",
            ScanError::AdfEmpty => "adf_empty",
            ScanError::Jam => "jam",
            ScanError::Cancelled => "cancelled",
            ScanError::Unsupported(_) => "unsupported",
            ScanError::Protocol(_) => "protocol",
            ScanError::TooLarge => "too_large",
            ScanError::Pdf(_) => "pdf",
            ScanError::Intake(_) => "intake",
        }
    }
}

// ------------------------------------------------------------------------------------ Model

/// Input source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Flatbed (`Platen`).
    Flatbed,
    /// Feeder single-sided (`Feeder`).
    Adf,
    /// Feeder double-sided (`Feeder` + `Duplex=true`).
    AdfDuplex,
}

impl Source {
    pub fn parse(s: &str) -> Option<Source> {
        match s.trim() {
            "flatbed" | "platen" => Some(Source::Flatbed),
            "adf" | "feeder" => Some(Source::Adf),
            "adf_duplex" | "duplex" => Some(Source::AdfDuplex),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Flatbed => "flatbed",
            Source::Adf => "adf",
            Source::AdfDuplex => "adf_duplex",
        }
    }
    fn input_source(self) -> &'static str {
        match self {
            Source::Flatbed => "Platen",
            Source::Adf | Source::AdfDuplex => "Feeder",
        }
    }
    pub fn is_feeder(self) -> bool {
        !matches!(self, Source::Flatbed)
    }
}

/// Colour mode (simplified names for the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorMode {
    Color,
    Grayscale,
    Bw,
}

impl ColorMode {
    pub fn parse(s: &str) -> Option<ColorMode> {
        match s.trim() {
            "color" | "RGB24" => Some(ColorMode::Color),
            "grayscale" | "gray" | "Grayscale8" => Some(ColorMode::Grayscale),
            "bw" | "BlackAndWhite1" => Some(ColorMode::Bw),
            _ => None,
        }
    }
    fn from_escl(s: &str) -> Option<ColorMode> {
        match s.trim() {
            "RGB24" => Some(ColorMode::Color),
            "Grayscale8" => Some(ColorMode::Grayscale),
            "BlackAndWhite1" => Some(ColorMode::Bw),
            _ => None,
        }
    }
    pub fn escl(self) -> &'static str {
        match self {
            ColorMode::Color => "RGB24",
            ColorMode::Grayscale => "Grayscale8",
            ColorMode::Bw => "BlackAndWhite1",
        }
    }
}

/// Capabilities of a source.
#[derive(Debug, Clone, Serialize)]
pub struct SourceCaps {
    pub source: Source,
    pub color_modes: Vec<ColorMode>,
    pub resolutions: Vec<u32>,
    pub formats: Vec<String>,
    /// in 1/300 inch
    pub max_width: u32,
    pub max_height: u32,
}

/// `ScannerCapabilities` (only what PaperRust needs).
#[derive(Debug, Clone, Serialize)]
pub struct Capabilities {
    pub make_and_model: Option<String>,
    pub uuid: Option<String>,
    pub version: Option<String>,
    pub sources: Vec<SourceCaps>,
}

impl Capabilities {
    pub fn source(&self, s: Source) -> Option<&SourceCaps> {
        self.sources.iter().find(|c| c.source == s)
    }
}

const COMMON_RESOLUTIONS: [u32; 8] = [75, 100, 150, 200, 300, 400, 600, 1200];

fn parse_u32(s: Option<String>) -> Option<u32> {
    s.and_then(|v| v.trim().parse::<u32>().ok())
}

fn input_caps(node: &Node, source: Source) -> SourceCaps {
    let mut color_modes = Vec::new();
    let mut formats = Vec::new();
    let mut resolutions = Vec::new();
    let mut profiles = Vec::new();
    node.find_all("SettingProfile", &mut profiles);
    for p in profiles {
        let mut modes = Vec::new();
        p.find_all("ColorMode", &mut modes);
        for m in modes {
            if let Some(c) = ColorMode::from_escl(&m.text)
                && !color_modes.contains(&c)
            {
                color_modes.push(c);
            }
        }
        let mut fmts = Vec::new();
        p.find_all("DocumentFormat", &mut fmts);
        p.find_all("DocumentFormatExt", &mut fmts);
        for f in fmts {
            let f = f.text.trim().to_ascii_lowercase();
            if !f.is_empty() && !formats.contains(&f) {
                formats.push(f);
            }
        }
        let mut discrete = Vec::new();
        p.find_all("DiscreteResolution", &mut discrete);
        for d in discrete {
            let x = parse_u32(d.text_of("XResolution"));
            let y = parse_u32(d.text_of("YResolution"));
            if let (Some(x), Some(y)) = (x, y)
                && x == y
                && !resolutions.contains(&x)
            {
                resolutions.push(x);
            }
        }
        let mut ranges = Vec::new();
        p.find_all("ResolutionRange", &mut ranges);
        for r in ranges {
            let range = r.find("XResolutionRange").unwrap_or(r);
            let (Some(min), Some(max)) = (parse_u32(range.text_of("Min")), parse_u32(range.text_of("Max"))) else {
                continue;
            };
            for v in COMMON_RESOLUTIONS {
                if v >= min && v <= max && !resolutions.contains(&v) {
                    resolutions.push(v);
                }
            }
        }
    }
    resolutions.sort_unstable();
    SourceCaps {
        source,
        color_modes,
        resolutions,
        formats,
        max_width: parse_u32(node.text_of("MaxWidth")).unwrap_or(2550),
        max_height: parse_u32(node.text_of("MaxHeight")).unwrap_or(3508),
    }
}

/// `ScannerCapabilities`-XML lesen.
pub fn parse_capabilities(data: &[u8]) -> Result<Capabilities, ScanError> {
    let root = xml::parse(data).ok_or_else(|| ScanError::Protocol("capabilities are not valid XML".into()))?;
    if root.name != "ScannerCapabilities" {
        return Err(ScanError::Protocol(format!("expected ScannerCapabilities, got {}", root.name)));
    }
    let mut sources = Vec::new();
    if let Some(platen) = root.find("PlatenInputCaps") {
        sources.push(input_caps(platen, Source::Flatbed));
    }
    if let Some(adf) = root.find("Adf") {
        if let Some(s) = adf.find("AdfSimplexInputCaps") {
            sources.push(input_caps(s, Source::Adf));
        }
        if let Some(d) = adf.find("AdfDuplexInputCaps") {
            sources.push(input_caps(d, Source::AdfDuplex));
        } else {
            // Older devices: only AdfOptions/Duplex, the same capabilities as single-sided
            let mut opts = Vec::new();
            adf.find_all("AdfOption", &mut opts);
            if opts.iter().any(|o| o.text.trim() == "Duplex")
                && let Some(s) = adf.find("AdfSimplexInputCaps")
            {
                sources.push(input_caps(s, Source::AdfDuplex));
            }
        }
    }
    Ok(Capabilities { make_and_model: root.text_of("MakeAndModel"), uuid: root.text_of("UUID"), version: root.text_of("Version"), sources })
}

/// `ScannerStatus` (excerpt).
#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    pub state: String,
    pub adf_state: Option<String>,
    /// (job URI, JobState, reasons)
    pub jobs: Vec<(String, String, Vec<String>)>,
}

impl Status {
    pub fn is_idle(&self) -> bool {
        self.state.is_empty() || self.state == "Idle"
    }
    pub fn adf_jam(&self) -> bool {
        self.adf_state.as_deref().is_some_and(|s| s.contains("Jam") || s.contains("Misfeed") || s.contains("Mispick"))
    }
    pub fn adf_empty(&self) -> bool {
        self.adf_state.as_deref() == Some("ScannerAdfEmpty")
    }
    /// State of a job (the path end must match).
    pub fn job(&self, job_url: &str) -> Option<&(String, String, Vec<String>)> {
        let tail = job_url.trim_end_matches('/').rsplit('/').next().unwrap_or("");
        self.jobs.iter().find(|(uri, _, _)| !tail.is_empty() && uri.trim_end_matches('/').ends_with(tail))
    }
}

pub fn parse_status(data: &[u8]) -> Result<Status, ScanError> {
    let root = xml::parse(data).ok_or_else(|| ScanError::Protocol("status is not valid XML".into()))?;
    let mut jobs = Vec::new();
    let mut infos = Vec::new();
    root.find_all("JobInfo", &mut infos);
    for j in infos {
        let mut reasons = Vec::new();
        j.find_all("JobStateReason", &mut reasons);
        jobs.push((
            j.text_of("JobUri").unwrap_or_default(),
            j.text_of("JobState").unwrap_or_default(),
            reasons.into_iter().map(|r| r.text.trim().to_string()).collect(),
        ));
    }
    Ok(Status { state: root.text_of("State").unwrap_or_default(), adf_state: root.text_of("AdfState"), jobs })
}

/// Desired settings of a scan.
#[derive(Debug, Clone)]
pub struct ScanSettings {
    pub source: Source,
    pub color: ColorMode,
    pub resolution: u32,
    /// MIME type (`image/jpeg` preferred, otherwise `application/pdf`)
    pub format: String,
    /// Area in 1/300 inch
    pub width: u32,
    pub height: u32,
}

/// A4 in 1/300 inch (210 × 297 mm).
pub const A4_300: (u32, u32) = (2480, 3508);

impl ScanSettings {
    /// Check the settings against the capabilities and complete them.
    pub fn resolve(
        caps: &Capabilities,
        source: Source,
        color: Option<ColorMode>,
        resolution: Option<u32>,
    ) -> Result<ScanSettings, ScanError> {
        let sc = caps.source(source).ok_or_else(|| ScanError::Unsupported(format!("source {}", source.as_str())))?;
        let color = match color {
            Some(c) if sc.color_modes.is_empty() || sc.color_modes.contains(&c) => c,
            Some(c) => return Err(ScanError::Unsupported(format!("color mode {}", c.escl()))),
            None => *sc.color_modes.iter().find(|c| **c == ColorMode::Color).or(sc.color_modes.first()).unwrap_or(&ColorMode::Color),
        };
        let resolution = match resolution {
            Some(r) if sc.resolutions.is_empty() || sc.resolutions.contains(&r) => r,
            Some(r) => return Err(ScanError::Unsupported(format!("resolution {r}"))),
            None => {
                // 300 dpi, otherwise the next smaller, otherwise the smallest
                if sc.resolutions.is_empty() || sc.resolutions.contains(&300) {
                    300
                } else {
                    sc.resolutions.iter().rev().find(|r| **r < 300).or(sc.resolutions.first()).copied().unwrap_or(300)
                }
            }
        };
        let format = if sc.formats.is_empty() || sc.formats.iter().any(|f| f == "image/jpeg") {
            "image/jpeg"
        } else if sc.formats.iter().any(|f| f == "application/pdf") {
            "application/pdf"
        } else {
            return Err(ScanError::Unsupported("no JPEG or PDF output".into()));
        };
        Ok(ScanSettings {
            source,
            color,
            resolution,
            format: format.into(),
            width: sc.max_width.min(A4_300.0).max(1),
            height: sc.max_height.min(A4_300.1).max(1),
        })
    }

    /// `ScanSettings` XML for `POST ScanJobs`.
    pub fn to_xml(&self) -> String {
        let duplex = matches!(self.source, Source::AdfDuplex);
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<scan:ScanSettings xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03" xmlns:pwg="http://www.pwg.org/schemas/2010/12/sm">
  <pwg:Version>2.0</pwg:Version>
  <scan:Intent>Document</scan:Intent>
  <pwg:ScanRegions>
    <pwg:ScanRegion>
      <pwg:ContentRegionUnits>escl:ThreeHundredthsOfInches</pwg:ContentRegionUnits>
      <pwg:XOffset>0</pwg:XOffset>
      <pwg:YOffset>0</pwg:YOffset>
      <pwg:Width>{w}</pwg:Width>
      <pwg:Height>{h}</pwg:Height>
    </pwg:ScanRegion>
  </pwg:ScanRegions>
  <pwg:InputSource>{src}</pwg:InputSource>
  {duplex}<scan:ColorMode>{color}</scan:ColorMode>
  <scan:XResolution>{r}</scan:XResolution>
  <scan:YResolution>{r}</scan:YResolution>
  <pwg:DocumentFormat>{fmt}</pwg:DocumentFormat>
  <scan:DocumentFormatExt>{fmt}</scan:DocumentFormatExt>
</scan:ScanSettings>
"#,
            w = self.width,
            h = self.height,
            src = self.source.input_source(),
            duplex = if self.source.is_feeder() { format!("<scan:Duplex>{duplex}</scan:Duplex>\n  ") } else { String::new() },
            color = self.color.escl(),
            r = self.resolution,
            fmt = xml::escape(&self.format),
        )
    }
}

// ------------------------------------------------------------------------------------ Client

/// A fetched page (or a document if the device delivers several pages as PDF).
#[derive(Debug, Clone)]
pub struct Page {
    pub content_type: String,
    pub data: Vec<u8>,
}

/// Result of `NextDocument`.
#[derive(Debug)]
pub enum Next {
    Page(Page),
    /// `503`: not ready yet
    NotReady,
    /// `404`: no further page
    Done,
    /// other status (e.g. 409/410/500 on abort/jam)
    Failed(u16),
}

/// Normalise the base URL (`http(s)://host[:port]/path/`). Only http/https, no user/password.
pub fn normalize_base(input: &str) -> Result<Url, String> {
    let s = input.trim();
    let with_scheme = if s.contains("://") { s.to_string() } else { format!("http://{s}") };
    let mut u = Url::parse(&with_scheme).map_err(|e| format!("invalid address: {e}"))?;
    if u.scheme() != "http" && u.scheme() != "https" {
        return Err("only http and https are supported".into());
    }
    if u.host_str().is_none_or(str::is_empty) {
        return Err("address without host".into());
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err("address must not contain credentials".into());
    }
    u.set_query(None);
    u.set_fragment(None);
    // Only host given → default path /eSCL/
    if u.path() == "/" || u.path().is_empty() {
        u.set_path("/eSCL/");
    }
    if !u.path().ends_with('/') {
        let p = format!("{}/", u.path());
        u.set_path(&p);
    }
    Ok(u)
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: Url,
}

fn net_err(e: reqwest::Error) -> ScanError {
    // No URL in the text (could carry internal addresses in error pages) – only the kind.
    let kind = if e.is_timeout() {
        "timeout"
    } else if e.is_connect() {
        "connection failed"
    } else {
        "request failed"
    };
    ScanError::Unreachable(kind.into())
}

impl Client {
    pub fn new(base: Url) -> Result<Client, ScanError> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(180))
            .redirect(reqwest::redirect::Policy::none())
            // Printers on the LAN have self-signed certificates (_uscans._tcp); the content is only
            // the scan result anyway. See contract "Sicherheit".
            .danger_accept_invalid_certs(true)
            .user_agent(pl_core::brand::USER_AGENT)
            .build()
            .map_err(|e| ScanError::Unreachable(e.to_string()))?;
        Ok(Client { http, base })
    }

    pub fn base(&self) -> &Url {
        &self.base
    }

    fn url(&self, rel: &str) -> Result<Url, ScanError> {
        self.base.join(rel).map_err(|e| ScanError::Protocol(e.to_string()))
    }

    async fn read_limited(resp: reqwest::Response, limit: usize) -> Result<Vec<u8>, ScanError> {
        let mut resp = resp;
        if resp.content_length().is_some_and(|l| l as usize > limit) {
            return Err(ScanError::TooLarge);
        }
        let mut out = Vec::new();
        while let Some(chunk) = resp.chunk().await.map_err(net_err)? {
            if out.len() + chunk.len() > limit {
                return Err(ScanError::TooLarge);
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }

    pub async fn capabilities(&self) -> Result<Capabilities, ScanError> {
        let resp = self.http.get(self.url("ScannerCapabilities")?).send().await.map_err(net_err)?;
        if !resp.status().is_success() {
            return Err(ScanError::Protocol(format!("capabilities: HTTP {}", resp.status().as_u16())));
        }
        parse_capabilities(&Self::read_limited(resp, MAX_XML_BYTES).await?)
    }

    pub async fn status(&self) -> Result<Status, ScanError> {
        let resp = self.http.get(self.url("ScannerStatus")?).send().await.map_err(net_err)?;
        if !resp.status().is_success() {
            return Err(ScanError::Protocol(format!("status: HTTP {}", resp.status().as_u16())));
        }
        parse_status(&Self::read_limited(resp, MAX_XML_BYTES).await?)
    }

    /// Create a job → job URL.
    pub async fn create_job(&self, settings: &ScanSettings) -> Result<Url, ScanError> {
        let resp = self
            .http
            .post(self.url("ScanJobs")?)
            .header(reqwest::header::CONTENT_TYPE, "text/xml")
            .body(settings.to_xml())
            .send()
            .await
            .map_err(net_err)?;
        let code = resp.status().as_u16();
        match code {
            200 | 201 => {}
            503 => return Err(ScanError::Busy),
            409 if settings.source.is_feeder() => return Err(ScanError::AdfEmpty),
            409 => return Err(ScanError::Busy),
            _ => return Err(ScanError::Protocol(format!("create job: HTTP {code}"))),
        }
        let loc = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ScanError::Protocol("create job: no Location header".into()))?;
        let job = self.base.join(loc.trim()).map_err(|e| ScanError::Protocol(format!("create job: bad Location ({e})")))?;
        if job.scheme() != self.base.scheme()
            || job.host_str() != self.base.host_str()
            || job.port_or_known_default() != self.base.port_or_known_default()
        {
            return Err(ScanError::Protocol("create job: Location points to another host".into()));
        }
        Ok(job)
    }

    pub async fn next_document(&self, job: &Url) -> Result<Next, ScanError> {
        let base = job.as_str().trim_end_matches('/');
        let url = Url::parse(&format!("{base}/NextDocument")).map_err(|e| ScanError::Protocol(e.to_string()))?;
        let resp = self.http.get(url).send().await.map_err(net_err)?;
        let code = resp.status().as_u16();
        match code {
            200 => {
                let ct = resp
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(|v| v.split(';').next().unwrap_or("").trim().to_ascii_lowercase())
                    .unwrap_or_default();
                let data = Self::read_limited(resp, MAX_PAGE_BYTES).await?;
                if data.is_empty() {
                    return Ok(Next::Failed(204));
                }
                Ok(Next::Page(Page { content_type: ct, data }))
            }
            503 => Ok(Next::NotReady),
            404 => Ok(Next::Done),
            other => Ok(Next::Failed(other)),
        }
    }

    /// Cancel the job (errors are ignored).
    pub async fn cancel(&self, job: &Url) {
        if let Err(e) = self.http.delete(job.clone()).send().await {
            tracing::debug!(target: "paperrust.scan", error = %e, "cancel request failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const CAPS: &str = include_str!("../tests/data/caps_officejet.xml");

    #[test]
    fn parses_officejet_like_caps() {
        let c = parse_capabilities(CAPS.as_bytes()).unwrap();
        assert_eq!(c.make_and_model.as_deref(), Some("HP OfficeJet Pro 9010 series"));
        let kinds: Vec<_> = c.sources.iter().map(|s| s.source).collect();
        assert_eq!(kinds, [Source::Flatbed, Source::Adf, Source::AdfDuplex]);
        let p = c.source(Source::Flatbed).unwrap();
        assert_eq!(p.resolutions, [75, 100, 200, 300, 600, 1200]);
        assert_eq!(p.color_modes, [ColorMode::Bw, ColorMode::Grayscale, ColorMode::Color]);
        assert!(p.formats.contains(&"image/jpeg".to_string()));
        assert_eq!(c.source(Source::AdfDuplex).unwrap().resolutions, [75, 100, 200, 300]);
    }

    #[test]
    fn resolve_defaults_and_rejects() {
        let c = parse_capabilities(CAPS.as_bytes()).unwrap();
        let s = ScanSettings::resolve(&c, Source::Adf, None, None).unwrap();
        assert_eq!((s.color, s.resolution, s.format.as_str()), (ColorMode::Color, 300, "image/jpeg"));
        assert_eq!((s.width, s.height), (2480, 3508));
        assert!(matches!(ScanSettings::resolve(&c, Source::AdfDuplex, None, Some(600)), Err(ScanError::Unsupported(_))));
        let x = ScanSettings::resolve(&c, Source::AdfDuplex, Some(ColorMode::Bw), Some(200)).unwrap().to_xml();
        assert!(x.contains("<pwg:InputSource>Feeder</pwg:InputSource>"));
        assert!(x.contains("<scan:Duplex>true</scan:Duplex>"));
        assert!(x.contains("<scan:ColorMode>BlackAndWhite1</scan:ColorMode>"));
        let f = ScanSettings::resolve(&c, Source::Flatbed, None, None).unwrap().to_xml();
        assert!(f.contains("Platen") && !f.contains("Duplex"));
        assert!(xml::parse(f.as_bytes()).is_some());
    }

    #[test]
    fn range_resolutions() {
        let x = r#"<scan:ScannerCapabilities xmlns:scan="s"><scan:Platen><scan:PlatenInputCaps><scan:SettingProfiles><scan:SettingProfile>
            <scan:ColorModes><scan:ColorMode>RGB24</scan:ColorMode></scan:ColorModes>
            <scan:SupportedResolutions><scan:ResolutionRange><scan:XResolutionRange><scan:Min>100</scan:Min><scan:Max>600</scan:Max><scan:Step>1</scan:Step></scan:XResolutionRange></scan:ResolutionRange></scan:SupportedResolutions>
            </scan:SettingProfile></scan:SettingProfiles></scan:PlatenInputCaps></scan:Platen></scan:ScannerCapabilities>"#;
        let c = parse_capabilities(x.as_bytes()).unwrap();
        assert_eq!(c.sources[0].resolutions, [100, 150, 200, 300, 400, 600]);
    }

    #[test]
    fn status_parsing() {
        let s = parse_status(br#"<scan:ScannerStatus xmlns:scan="s" xmlns:pwg="p"><pwg:State>Idle</pwg:State><scan:AdfState>ScannerAdfJam</scan:AdfState>
            <scan:Jobs><scan:JobInfo><pwg:JobUri>/eSCL/ScanJobs/abc</pwg:JobUri><pwg:JobState>Aborted</pwg:JobState><pwg:JobStateReasons><pwg:JobStateReason>AdfJam</pwg:JobStateReason></pwg:JobStateReasons></scan:JobInfo></scan:Jobs></scan:ScannerStatus>"#).unwrap();
        assert!(s.is_idle() && s.adf_jam() && !s.adf_empty());
        assert_eq!(s.job("http://h/eSCL/ScanJobs/abc").unwrap().1, "Aborted");
    }

    #[test]
    fn base_urls() {
        assert_eq!(normalize_base("192.168.1.20").unwrap().as_str(), "http://192.168.1.20/eSCL/");
        assert_eq!(normalize_base("https://printer:443/eSCL").unwrap().as_str(), "https://printer/eSCL/");
        assert_eq!(normalize_base("http://10.0.0.5:8080/escl/").unwrap().as_str(), "http://10.0.0.5:8080/escl/");
        assert!(normalize_base("ftp://x").is_err());
        assert!(normalize_base("http://u:p@x").is_err());
    }
}
