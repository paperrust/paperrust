//! Automatic updates of the desktop app (Z4, docs/DESKTOP.md §23).
//!
//! Flow:
//! 1. **Search** (at start after a short pause, then every `interval_hours`, default 24 h, or
//!    "Jetzt suchen"): fetch a static manifest in Tauri format `latest.json` per channel from the
//!    update address ([`endpoint_url`], `{{channel}}` = `stable` | `beta`). Channel `beta`
//!    reads both manifests and takes the highest version; `stable` never takes a pre-release
//!    ([`choose`]). Never back to an older version.
//! 2. **Download + check** in the background with `tauri-plugin-updater`: [`to_update`] turns the
//!    manifest into an `Update` (`restore_update` enforces that the signature contains the announced version
//!    – a manipulated manifest cannot slip in an old, validly signed version),
//!    `Update::download` downloads and checks the minisign/Ed25519 signature against the built-in
//!    public key (`tauri.conf.json` → `plugins.updater.pubkey`). Only then does the
//!    installer land in `<data folder>/updates/` ([`Store`]).
//! 3. **Install** only after confirmation ("Jetzt neu starten und installieren", also offered at the next
//!    start): check the file **again** ([`prepare_install`]), stop server + PostgreSQL locally
//!    in an orderly way (the shell does that), then start the NSIS installer outside the job object
//!    (`/P /UPDATE /D=<program folder>`, via a helper: [`launch_installer`],
//!    [`helper_main`]) and quit. The installer replaces only the program folder, the helper then restarts
//!    the app with the same environment; data stays untouched, schema migrations run
//!    at start as before (`manage migrate`).
//!
//! Disabling: settings (`app.toml` `[updates] enabled = false`), environment
//! `PAPERRUST_DESKTOP_UPDATES=off`, or built without a public key (developer build).
//! Remote mode: the same (the app updates itself; server updates are the server's business).

use std::path::{Path, PathBuf};
use std::time::Duration;

use semver::Version;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime, Url};
use tauri_plugin_updater::UpdaterExt;

/// Default update address (placeholder until the user decides on the location): a GitHub release
/// `updater` of the repo `paperrust/paperrust` with `stable.json` and `beta.json` as assets.
/// Can be overridden with `app.toml` `[updates] endpoint` or `PAPERRUST_DESKTOP_UPDATE_URL`.
pub const DEFAULT_ENDPOINT: &str = "https://github.com/paperrust/paperrust/releases/download/updater/{{channel}}.json";
/// `off` / `0` / `false` / `no` / `aus` switches updates off (managed installations, tests).
pub const ENV_UPDATES: &str = "PAPERRUST_DESKTOP_UPDATES";
pub const ENV_UPDATE_URL: &str = "PAPERRUST_DESKTOP_UPDATE_URL";
pub const ENV_UPDATE_CHANNEL: &str = "PAPERRUST_DESKTOP_UPDATE_CHANNEL";
pub const DEFAULT_INTERVAL_HOURS: u32 = 24;
const MAX_INTERVAL_HOURS: u32 = 24 * 30;
const MAX_MANIFEST: usize = 256 * 1024;
const MAX_NOTES: usize = 20_000;
const UA: &str = concat!("PaperRust-Desktop/", env!("CARGO_PKG_VERSION"), " (updater)");

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    #[default]
    Stable,
    Beta,
}

impl Channel {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "stable" | "stabil" => Some(Self::Stable),
            "beta" => Some(Self::Beta),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
        }
    }

    /// May a version of this channel be offered? `stable`: only without a pre-release part.
    pub fn accepts(self, v: &Version) -> bool {
        self == Self::Beta || v.pre.is_empty()
    }
}

/// `app.toml` → `[updates]` (missing fields → default).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    pub enabled: bool,
    pub channel: Channel,
    pub interval_hours: u32,
    /// Empty = [`DEFAULT_ENDPOINT`].
    pub endpoint: String,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self { enabled: true, channel: Channel::Stable, interval_hours: DEFAULT_INTERVAL_HOURS, endpoint: String::new() }
    }
}

/// Effective settings after environment, key and check of the address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effective {
    pub enabled: bool,
    /// Reason if disabled (display in settings/update window).
    pub reason: Option<String>,
    pub channel: Channel,
    pub channel_from_env: bool,
    pub interval: Duration,
    /// Template with `{{channel}}` etc.
    pub endpoint: String,
    pub endpoint_from_env: bool,
}

fn is_off(v: &str) -> bool {
    matches!(v.trim().to_ascii_lowercase().as_str(), "off" | "0" | "false" | "no" | "aus" | "disabled")
}

pub fn effective(s: &UpdateSettings, get: impl Fn(&str) -> Option<String>, pubkey: &str) -> Effective {
    let get = |k: &str| get(k).filter(|v| !v.trim().is_empty());
    let (channel, channel_from_env) = match get(ENV_UPDATE_CHANNEL).and_then(|c| Channel::parse(&c)) {
        Some(c) => (c, true),
        None => (s.channel, false),
    };
    let (endpoint, endpoint_from_env) = match get(ENV_UPDATE_URL) {
        Some(u) => (u.trim().to_string(), true),
        None if !s.endpoint.trim().is_empty() => (s.endpoint.trim().to_string(), false),
        None => (DEFAULT_ENDPOINT.to_string(), false),
    };
    let hours = match s.interval_hours {
        0 => DEFAULT_INTERVAL_HOURS,
        h => h.min(MAX_INTERVAL_HOURS),
    };
    let reason = if get(ENV_UPDATES).is_some_and(|v| is_off(&v)) {
        Some(format!("abgeschaltet ({ENV_UPDATES}=off)"))
    } else if pubkey.trim().is_empty() {
        Some("Diese Version ist ohne Update-Schlüssel gebaut (Entwickler-Version).".into())
    } else if !s.enabled {
        Some("in den Einstellungen abgeschaltet".into())
    } else if let Err(e) = endpoint_url(&endpoint, channel, &Version::new(0, 0, 0)) {
        Some(format!("Update-Adresse ungültig: {e}"))
    } else {
        None
    };
    Effective {
        enabled: reason.is_none(),
        reason,
        channel,
        channel_from_env,
        interval: Duration::from_secs(u64::from(hours) * 3600),
        endpoint,
        endpoint_from_env,
    }
}

/// `{{target}}`/`{{arch}}` values as in the Tauri updater.
pub fn os_arch() -> (&'static str, &'static str) {
    let os = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86") {
        "i686"
    } else {
        "x86_64"
    };
    (os, arch)
}

/// Keys in `platforms` that this app accepts (order as in the plugin).
pub fn platform_keys() -> Vec<String> {
    let (os, arch) = os_arch();
    vec![format!("{os}-{arch}-nsis"), format!("{os}-{arch}")]
}

/// https everywhere, http only on this machine (tests, local mirrors). No user info.
pub fn url_allowed(u: &Url) -> bool {
    if !u.username().is_empty() || u.password().is_some() {
        return false;
    }
    match (u.scheme(), u.host()) {
        ("https", Some(_)) => true,
        ("http", Some(url::Host::Domain(d))) => d.eq_ignore_ascii_case("localhost"),
        ("http", Some(url::Host::Ipv4(ip))) => ip.is_loopback(),
        ("http", Some(url::Host::Ipv6(ip))) => ip.is_loopback(),
        _ => false,
    }
}

/// Address of the manifest for a channel.
pub fn endpoint_url(template: &str, channel: Channel, current: &Version) -> Result<Url, String> {
    let (os, arch) = os_arch();
    let s = template
        .trim()
        .replace("{{channel}}", channel.as_str())
        .replace("{{current_version}}", &current.to_string())
        .replace("{{target}}", os)
        .replace("{{arch}}", arch);
    let u = Url::parse(&s).map_err(|e| format!("{s}: {e}"))?;
    if !url_allowed(&u) {
        return Err(format!("{s}: nur https (http nur für 127.0.0.1/localhost)"));
    }
    Ok(u)
}

/// An announced version from a manifest.
#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: Version,
    pub notes: String,
    pub pub_date: Option<String>,
    /// Manifest unchanged (for `restore_update`).
    pub raw: serde_json::Value,
    pub source: Channel,
}

fn parse_version(s: &str) -> Result<Version, String> {
    Version::parse(s.trim().trim_start_matches('v')).map_err(|e| format!("Version {s:?}: {e}"))
}

/// Check `latest.json` (Tauri, static format): `version` (SemVer), `platforms` with an
/// entry for this platform (`url` allowed, `signature` not empty), optionally `notes`, `pub_date`.
pub fn parse_manifest(bytes: &[u8], source: Channel, keys: &[String]) -> Result<Release, String> {
    if bytes.len() > MAX_MANIFEST {
        return Err("Manifest zu groß".into());
    }
    let raw: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| format!("Manifest ist kein JSON: {e}"))?;
    let obj = raw.as_object().ok_or("Manifest ist kein JSON-Objekt")?;
    let version = parse_version(obj.get("version").and_then(|v| v.as_str()).ok_or("Manifest ohne `version`")?)?;
    let platforms = obj.get("platforms").and_then(|p| p.as_object()).ok_or("Manifest ohne `platforms`")?;
    let mut found = false;
    for k in keys {
        let Some(p) = platforms.get(k) else { continue };
        let url = p.get("url").and_then(|u| u.as_str()).ok_or_else(|| format!("{k}: `url` fehlt"))?;
        let url = Url::parse(url).map_err(|e| format!("{k}: url {e}"))?;
        if !url_allowed(&url) {
            return Err(format!("{k}: Download-Adresse {url} nicht erlaubt (nur https)"));
        }
        if p.get("signature").and_then(|s| s.as_str()).is_none_or(|s| s.trim().is_empty()) {
            return Err(format!("{k}: `signature` fehlt"));
        }
        found = true;
    }
    if !found {
        return Err(format!("kein Paket für diese Plattform ({})", keys.join(", ")));
    }
    let notes: String = obj.get("notes").and_then(|n| n.as_str()).unwrap_or("").chars().take(MAX_NOTES).collect();
    let pub_date = obj.get("pub_date").and_then(|d| d.as_str()).map(|d| d.chars().take(40).collect());
    Ok(Release { version, notes, pub_date, raw, source })
}

/// Best newer version for the channel (highest, never ≤ current, `stable` without pre-releases).
pub fn choose(current: &Version, channel: Channel, candidates: impl IntoIterator<Item = Release>) -> Option<Release> {
    candidates.into_iter().filter(|r| r.version > *current && channel.accepts(&r.version)).max_by(|a, b| a.version.cmp(&b.version))
}

/// Manifests that a channel reads.
pub fn channels_to_read(channel: Channel) -> &'static [Channel] {
    match channel {
        Channel::Stable => &[Channel::Stable],
        Channel::Beta => &[Channel::Beta, Channel::Stable],
    }
}

/// Due? Always at start; otherwise after the interval has elapsed (wall clock, also after standby)
/// or if the clock was set back.
pub fn due(first: bool, last_ms: u64, now_ms: u64, interval: Duration) -> bool {
    first || last_ms == 0 || now_ms < last_ms || now_ms - last_ms >= interval.as_millis() as u64
}

// ---------------------------------------------------------------------------------------------
// Store `<data folder>/updates/`

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub version: String,
    pub channel: Channel,
    pub notes: String,
    pub pub_date: Option<String>,
    /// Manifest the file comes from (evaluated again at installation).
    pub manifest: serde_json::Value,
    /// File name in `updates/`.
    pub file: String,
    pub size: u64,
    pub sha256: String,
    pub downloaded_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installing {
    pub version: String,
    pub from: String,
    pub started_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub last_check_ms: u64,
    pub last_error: Option<String>,
    /// Latest version found (even if not downloadable, e.g. portable installation).
    pub latest_seen: Option<String>,
    /// Installer started; the next start checks whether the version is right.
    pub installing: Option<Installing>,
    /// Installing this version failed → do not offer unasked again at start.
    pub failed_version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Store {
    pub dir: PathBuf,
}

/// Result of the check at start (after an installer run).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartNote {
    None,
    Installed(String),
    InstallFailed(String),
}

impl Store {
    pub fn new(home: &Path) -> Self {
        Self { dir: home.join("updates") }
    }

    fn state_file(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    fn pending_file(&self) -> PathBuf {
        self.dir.join("pending.json")
    }

    pub fn file_path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// File name of the installer (only safe characters of the version).
    pub fn installer_name(version: &Version) -> String {
        let v: String =
            version.to_string().chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
        format!("PaperRust-{v}-setup.exe")
    }

    fn write(&self, path: &Path, data: &[u8]) -> Result<(), String> {
        paperrust_desktop::perms::create_private_dir(&self.dir).map_err(|e| format!("{}: {e}", self.dir.display()))?;
        let tmp = path.with_extension("tmp");
        paperrust_desktop::config::write_private(&tmp, data).map_err(|e| format!("{} schreiben: {e}", tmp.display()))?;
        std::fs::rename(&tmp, path).map_err(|e| format!("{} ersetzen: {e}", path.display()))
    }

    fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
        let t = std::fs::read(path).ok()?;
        match serde_json::from_slice(&t) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(error = %e, "{} unlesbar – ignoriert", path.display());
                None
            }
        }
    }

    pub fn load_state(&self) -> State {
        Self::read_json(&self.state_file()).unwrap_or_default()
    }

    pub fn save_state(&self, s: &State) -> Result<(), String> {
        let t = serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?;
        self.write(&self.state_file(), &t)
    }

    pub fn update_state(&self, f: impl FnOnce(&mut State)) {
        let mut s = self.load_state();
        f(&mut s);
        if let Err(e) = self.save_state(&s) {
            tracing::warn!("Update-Zustand: {e}");
        }
    }

    /// Ready update (only if the file is still there).
    pub fn load_pending(&self) -> Option<Pending> {
        let p: Pending = Self::read_json(&self.pending_file())?;
        let ok = !p.file.contains(['/', '\\']) && self.file_path(&p.file).is_file();
        ok.then_some(p)
    }

    pub fn save_pending(&self, p: &Pending) -> Result<(), String> {
        let t = serde_json::to_vec_pretty(p).map_err(|e| e.to_string())?;
        self.write(&self.pending_file(), &t)
    }

    /// Delete the ready update and all downloaded installers.
    pub fn clear_pending(&self) {
        let _ = std::fs::remove_file(self.pending_file());
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.starts_with("PaperRust-") && (n.ends_with("-setup.exe") || n.ends_with(".part")) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }

    /// At start: evaluate the result of an installer run, clean up outdated downloads.
    pub fn reconcile(&self, current: &Version) -> StartNote {
        // Copy of the helper from the last install (may be missing or still running briefly).
        let _ = std::fs::remove_file(self.dir.join(HELPER_EXE));
        let mut st = self.load_state();
        let mut note = StartNote::None;
        if let Some(i) = st.installing.take() {
            match parse_version(&i.version) {
                Ok(v) if *current >= v => {
                    note = StartNote::Installed(i.version.clone());
                    st.failed_version = None;
                }
                _ => {
                    note = StartNote::InstallFailed(i.version.clone());
                    st.failed_version = Some(i.version.clone());
                }
            }
            let _ = self.save_state(&st);
        }
        if let Some(p) = self.load_pending()
            && parse_version(&p.version).is_ok_and(|v| v <= *current)
        {
            self.clear_pending();
        }
        note
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

// ---------------------------------------------------------------------------------------------
// tauri-plugin-updater

/// Public key from the built-in configuration (`plugins.updater.pubkey`).
pub fn pubkey<R: Runtime>(app: &AppHandle<R>) -> String {
    app.config().plugins.0.get("updater").and_then(|u| u.get("pubkey")).and_then(|k| k.as_str()).unwrap_or("").trim().to_string()
}

/// Manifest → `Update` of the plugin (target platform, version comparison, signature must carry the
/// version). `None` = not newer than the running version.
pub fn to_update<R: Runtime>(app: &AppHandle<R>, manifest: &serde_json::Value) -> Result<Option<tauri_plugin_updater::Update>, String> {
    // The plugin requires an address; it is never queried (the manifest is fetched by [`fetch`]).
    let placeholder = Url::parse(DEFAULT_ENDPOINT).map_err(|e| e.to_string())?;
    let u = app
        .updater_builder()
        .endpoints(vec![placeholder])
        .map_err(|e| e.to_string())?
        .timeout(Duration::from_secs(30 * 60))
        .build()
        .map_err(|e| format!("Updater: {e}"))?;
    u.restore_update(manifest.clone()).map_err(|e| format!("Manifest: {e}"))
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .use_rustls_tls()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .user_agent(UA)
        // GitHub redirects release assets – only to allowed addresses.
        .redirect(reqwest::redirect::Policy::custom(
            |a| {
                if a.previous().len() >= 5 || !url_allowed(a.url()) { a.stop() } else { a.follow() }
            },
        ))
        .build()
        .map_err(|e| e.to_string())
}

/// Fetch the manifest of a channel. `Ok(None)` = channel has (yet) no release (404/204).
async fn fetch(c: &reqwest::Client, template: &str, channel: Channel, current: &Version) -> Result<Option<Release>, String> {
    let url = endpoint_url(template, channel, current)?;
    let r = c.get(url.clone()).header("Accept", "application/json").send().await.map_err(|e| format!("{url}: {e}"))?;
    match r.status().as_u16() {
        204 | 404 => return Ok(None),
        s if !(200..300).contains(&s) => return Err(format!("{url}: HTTP {s}")),
        _ => {}
    }
    if r.content_length().is_some_and(|l| l > MAX_MANIFEST as u64) {
        return Err(format!("{url}: Manifest zu groß"));
    }
    let body = r.bytes().await.map_err(|e| format!("{url}: {e}"))?;
    parse_manifest(&body, channel, &platform_keys()).map(Some).map_err(|e| format!("{url}: {e}"))
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    UpToDate,
    /// Newer version found, but not downloadable (portable installation): hint only.
    Available(Release),
    /// Checked and stored (new or already present).
    Ready(Pending),
    Failed(String),
}

/// Search, download, check, store. `installable` = NSIS installation (otherwise hint only).
pub async fn check<R: Runtime>(
    app: &AppHandle<R>,
    store: &Store,
    eff: &Effective,
    current: &Version,
    installable: bool,
    now_ms: u64,
) -> Outcome {
    let c = match client() {
        Ok(c) => c,
        Err(e) => return Outcome::Failed(e),
    };
    let mut found = Vec::new();
    let mut errors = Vec::new();
    for ch in channels_to_read(eff.channel) {
        match fetch(&c, &eff.endpoint, *ch, current).await {
            Ok(Some(r)) => {
                if !ch.accepts(&r.version) {
                    tracing::warn!(version = %r.version, "Vorab-Version im Kanal stable ignoriert");
                }
                found.push(r);
            }
            Ok(None) => {}
            Err(e) => errors.push(e),
        }
    }
    if found.is_empty() && !errors.is_empty() {
        return Outcome::Failed(errors.join("; "));
    }
    for e in &errors {
        tracing::warn!("Update-Suche: {e}");
    }
    let pending = store.load_pending();
    let Some(rel) = choose(current, eff.channel, found) else {
        // Channel switch beta → stable or version already installed: discard the download.
        if pending.is_some() {
            store.clear_pending();
        }
        return Outcome::UpToDate;
    };
    if let Some(p) = pending {
        if parse_version(&p.version).is_ok_and(|v| v == rel.version) {
            return Outcome::Ready(p);
        }
        store.clear_pending();
    }
    if !installable {
        return Outcome::Available(rel);
    }
    let update = match to_update(app, &rel.raw) {
        Ok(Some(u)) => u,
        Ok(None) => return Outcome::UpToDate,
        Err(e) => return Outcome::Failed(e),
    };
    if !url_allowed(&update.download_url) {
        return Outcome::Failed(format!("Download-Adresse {} nicht erlaubt", update.download_url));
    }
    // The plugin builds its download client without a crypto provider (only `check` sets one).
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    tracing::info!(version = %rel.version, url = %update.download_url, "Update wird geladen");
    // Downloads and checks the signature (incl. signed version); on error not a byte goes to disk.
    let bytes = match update.download(|_, _| {}, || {}).await {
        Ok(b) => b,
        Err(e) => {
            return Outcome::Failed(match e {
                tauri_plugin_updater::Error::Minisign(_)
                | tauri_plugin_updater::Error::SignedVersionMismatch { .. }
                | tauri_plugin_updater::Error::MissingSignedVersion
                | tauri_plugin_updater::Error::Base64(_)
                | tauri_plugin_updater::Error::SignatureUtf8(_) => {
                    format!("Signatur ungültig – Update {} verworfen ({e})", rel.version)
                }
                e => format!("Download {}: {e}", update.download_url),
            });
        }
    };
    let name = Store::installer_name(&rel.version);
    if let Err(e) = store.write(&store.file_path(&name), &bytes) {
        return Outcome::Failed(e);
    }
    let p = Pending {
        version: rel.version.to_string(),
        channel: rel.source,
        notes: rel.notes.clone(),
        pub_date: rel.pub_date.clone(),
        manifest: rel.raw.clone(),
        file: name,
        size: bytes.len() as u64,
        sha256: sha256_hex(&bytes),
        downloaded_ms: now_ms,
    };
    if let Err(e) = store.save_pending(&p) {
        store.clear_pending();
        return Outcome::Failed(e);
    }
    tracing::info!(version = %p.version, size = p.size, sha256 = %p.sha256, "Update geprüft und abgelegt");
    Outcome::Ready(p)
}

/// Before installing: check the file again against manifest + key. Invalid → discard.
pub fn verify_pending<R: Runtime>(app: &AppHandle<R>, store: &Store) -> Result<(Pending, PathBuf), String> {
    let p = store.load_pending().ok_or("Kein heruntergeladenes Update vorhanden.")?;
    let path = store.file_path(&p.file);
    let update = match to_update(app, &p.manifest) {
        Ok(Some(u)) if u.version == p.version => u,
        Ok(_) => {
            store.clear_pending();
            return Err("Das Update ist nicht neuer als die installierte Version.".into());
        }
        Err(e) => {
            store.clear_pending();
            return Err(e);
        }
    };
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Err(e) = update.verify(&bytes) {
        store.clear_pending();
        return Err(format!("Signatur ungültig – Update {} verworfen ({e})", p.version));
    }
    Ok((p, path))
}

/// Program folder of the running app.
pub fn install_dir() -> Option<PathBuf> {
    std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf))
}

/// Installed with the NSIS installer (otherwise portable/ZIP or developer build).
pub fn installable(dir: &Path) -> bool {
    cfg!(windows) && dir.join("uninstall.exe").is_file()
}

/// Command line of the installer: passive (progress, no questions), update mode, into exactly the
/// folder of the running app (`/D=` must come last, without quotes – NSIS rule).
///
/// Deliberately **without** `/R`: the Tauri template then starts the app via
/// `nsis_tauri_utils::RunAsUser` with the environment of Explorer – an environment set only in the process
/// (`PAPERRUST_DESKTOP_HOME`, managed settings) would be lost and the new version
/// might open a different data folder. The restart is done by [`helper_main`].
pub fn installer_args(dir: &Path) -> String {
    format!("/P /UPDATE /D={}", dir.display())
}

/// First argument of the helper mode (see [`helper_main`]).
pub const HELPER_ARG: &str = "--paperrust-apply-update";
const HELPER_EXE: &str = "paperrust-update-helper.exe";

/// Helper mode, at the very beginning of `main` (before job object, single instance and Tauri): start the
/// installer, wait for its end, then start the (new) app again with the **same environment** –
/// even if the installer aborts (then the old version). Runs as a copy of the app under
/// `<data folder>/updates/`, so that the installer can replace the program file.
/// `None` = normal start.
pub fn helper_main() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next()? != HELPER_ARG {
        return None;
    }
    let (Some(installer), Some(dir), Some(app)) = (args.next(), args.next(), args.next()) else {
        return Some(2);
    };
    let (installer, dir, app) = (PathBuf::from(installer), PathBuf::from(dir), PathBuf::from(app));
    let mut c = std::process::Command::new(&installer);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.raw_arg(installer_args(&dir));
    }
    #[cfg(not(windows))]
    c.arg(installer_args(&dir));
    let code = match c.status() {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("PaperRust-Update: {}: {e}", installer.display());
            1
        }
    };
    if app.is_file() {
        let _ = std::process::Command::new(&app).spawn();
    }
    Some(code)
}

/// Start the installer via the helper – outside the app's job object (otherwise it would end with
/// it). `dir` = program folder of the running app. Returns the PID of the helper.
pub fn launch_installer(installer: &Path, dir: &Path) -> Result<u32, String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        let exe = std::env::current_exe().map_err(|e| format!("Programmpfad: {e}"))?;
        let app = dir.join(exe.file_name().ok_or("Programmname unbekannt")?);
        let helper = installer.parent().ok_or("Update-Ordner unbekannt")?.join(HELPER_EXE);
        std::fs::copy(&exe, &helper).map_err(|e| format!("{}: {e}", helper.display()))?;
        let cmd = || {
            let mut c = std::process::Command::new(&helper);
            c.arg(HELPER_ARG).arg(installer).arg(dir).arg(&app);
            c
        };
        let child = cmd()
            .creation_flags(CREATE_BREAKAWAY_FROM_JOB)
            .spawn()
            .or_else(|_| cmd().spawn())
            .map_err(|e| format!("{}: {e}", helper.display()))?;
        Ok(child.id())
    }
    #[cfg(not(windows))]
    {
        let _ = (installer, dir);
        Err("Automatisches Einspielen gibt es bisher nur unter Windows.".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    const FIX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/updater/");

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{FIX}{name}")).unwrap()
    }

    fn fixture_str(name: &str) -> String {
        String::from_utf8(fixture(name)).unwrap().trim().to_string()
    }

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    // The platform of the machine running the tests (CI runs on Linux and Windows).
    fn keys() -> Vec<String> {
        platform_keys()
    }

    fn manifest(version: &str, url: &str, sig: &str) -> serde_json::Value {
        let mut platforms = serde_json::Map::new();
        platforms.insert("other-os-unknown".into(), serde_json::json!({ "url": "https://x.invalid/a.tar.gz", "signature": "x" }));
        platforms.insert(keys()[1].clone(), serde_json::json!({ "url": url, "signature": sig }));
        serde_json::json!({
            "version": version,
            "notes": "Neu: Updates",
            "pub_date": "2026-11-09T12:00:00Z",
            "platforms": platforms
        })
    }

    fn rel(version: &str, source: Channel) -> Release {
        Release { version: v(version), notes: String::new(), pub_date: None, raw: serde_json::Value::Null, source }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("paperrust-update-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn version_compare_and_channels() {
        let cur = v("0.1.0");
        let all = || {
            vec![
                rel("0.1.0", Channel::Stable),
                rel("0.2.0", Channel::Stable),
                rel("0.3.0-beta.1", Channel::Beta),
                rel("0.0.9", Channel::Stable),
            ]
        };
        assert_eq!(choose(&cur, Channel::Stable, all()).unwrap().version, v("0.2.0"));
        assert_eq!(choose(&cur, Channel::Beta, all()).unwrap().version, v("0.3.0-beta.1"));
        // A beta user gets a newer stable version if the beta is older.
        let b = vec![rel("0.2.0-rc.1", Channel::Beta), rel("0.2.0", Channel::Stable)];
        let c = choose(&cur, Channel::Beta, b).unwrap();
        assert_eq!((c.version, c.source), (v("0.2.0"), Channel::Stable));
        // Never equal/older; stable never a pre-release, even if it is in the stable manifest.
        assert!(choose(&cur, Channel::Stable, vec![rel("0.1.0", Channel::Stable)]).is_none());
        assert!(choose(&v("0.2.0"), Channel::Beta, all()).is_some());
        assert!(choose(&v("0.3.0"), Channel::Beta, all()).is_none(), "kein Downgrade");
        assert!(choose(&cur, Channel::Stable, vec![rel("0.1.1-test", Channel::Stable)]).is_none());
        assert!(v("0.1.1-test") > v("0.1.0") && v("0.1.1-test") < v("0.1.1"));
        assert_eq!(channels_to_read(Channel::Stable), &[Channel::Stable]);
        assert_eq!(channels_to_read(Channel::Beta), &[Channel::Beta, Channel::Stable]);
        assert_eq!(Channel::parse(" Beta "), Some(Channel::Beta));
        assert_eq!(Channel::parse("nightly"), None);
    }

    #[test]
    fn settings_and_disable_switches() {
        let env =
            |pairs: &'static [(&'static str, &'static str)]| move |k: &str| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        let s = UpdateSettings::default();
        let e = effective(&s, env(&[]), "KEY");
        assert!(e.enabled && e.reason.is_none());
        assert_eq!(e.endpoint, DEFAULT_ENDPOINT);
        assert_eq!(e.interval, Duration::from_secs(24 * 3600));
        assert_eq!(e.channel, Channel::Stable);
        for off in ["off", "0", "FALSE", "no", "aus"] {
            let pairs: &'static [(&str, &str)] = match off {
                "off" => &[(ENV_UPDATES, "off")],
                "0" => &[(ENV_UPDATES, "0")],
                "FALSE" => &[(ENV_UPDATES, "FALSE")],
                "no" => &[(ENV_UPDATES, "no")],
                _ => &[(ENV_UPDATES, "aus")],
            };
            let e = effective(&s, env(pairs), "KEY");
            assert!(!e.enabled, "{off}");
            assert!(e.reason.unwrap().contains(ENV_UPDATES));
        }
        assert!(effective(&s, env(&[(ENV_UPDATES, "on")]), "KEY").enabled);
        let e = effective(&s, env(&[]), " ");
        assert!(!e.enabled && e.reason.unwrap().contains("Update-Schlüssel"));
        let off = UpdateSettings { enabled: false, ..UpdateSettings::default() };
        assert!(!effective(&off, env(&[]), "KEY").enabled);
        // Environment before settings; interval limited.
        let custom = UpdateSettings {
            channel: Channel::Beta,
            interval_hours: 100_000,
            endpoint: "https://updates.example.ch/pr/{{channel}}.json".into(),
            ..UpdateSettings::default()
        };
        let e = effective(&custom, env(&[]), "KEY");
        assert_eq!(e.channel, Channel::Beta);
        assert_eq!(e.interval, Duration::from_secs(720 * 3600));
        assert!(e.endpoint.starts_with("https://updates.example.ch/"));
        let e = effective(&custom, env(&[(ENV_UPDATE_URL, "http://127.0.0.1:9/{{channel}}.json"), (ENV_UPDATE_CHANNEL, "stable")]), "KEY");
        assert!(e.enabled && e.endpoint_from_env && e.channel_from_env);
        assert_eq!(e.channel, Channel::Stable);
        let zero = UpdateSettings { interval_hours: 0, ..UpdateSettings::default() };
        assert_eq!(effective(&zero, env(&[]), "K").interval.as_secs(), 24 * 3600);
        // http only local
        let e = effective(&s, env(&[(ENV_UPDATE_URL, "http://updates.example.ch/x.json")]), "K");
        assert!(!e.enabled && e.reason.unwrap().contains("ungültig"));
        // app.toml section: missing fields → default
        let t: UpdateSettings = toml::from_str("channel = \"beta\"").unwrap();
        assert_eq!(t, UpdateSettings { channel: Channel::Beta, ..UpdateSettings::default() });
    }

    #[test]
    fn endpoint_template_and_url_rules() {
        let u = endpoint_url(DEFAULT_ENDPOINT, Channel::Beta, &v("0.1.0")).unwrap();
        assert_eq!(u.as_str(), "https://github.com/paperrust/paperrust/releases/download/updater/beta.json");
        let u = endpoint_url("https://u.example/{{target}}/{{arch}}/{{current_version}}/{{channel}}.json", Channel::Stable, &v("0.1.0"))
            .unwrap();
        let (os, arch) = os_arch();
        assert_eq!(u.as_str(), format!("https://u.example/{os}/{arch}/0.1.0/stable.json"));
        let ok = |s: &str| url_allowed(&Url::parse(s).unwrap());
        assert!(ok("https://github.com/x"));
        assert!(ok("http://127.0.0.1:8080/x") && ok("http://localhost/x") && ok("http://[::1]/x"));
        assert!(!ok("http://192.168.1.2/x") && !ok("http://example.com/x"));
        assert!(!ok("https://user:pw@example.com/x") && !ok("file:///C:/x") && !ok("ftp://x/y"));
        assert!(endpoint_url("kein url", Channel::Stable, &v("0.1.0")).is_err());
    }

    #[test]
    fn manifest_parsing() {
        let m = manifest("v0.2.0", "https://dl.example/PaperRust-0.2.0-setup.exe", "c2ln");
        let r = parse_manifest(m.to_string().as_bytes(), Channel::Stable, &keys()).unwrap();
        assert_eq!(r.version, v("0.2.0"));
        assert_eq!(r.notes, "Neu: Updates");
        assert_eq!(r.pub_date.as_deref(), Some("2026-11-09T12:00:00Z"));
        assert_eq!(r.raw, m);
        let bad = |j: serde_json::Value| parse_manifest(j.to_string().as_bytes(), Channel::Stable, &keys()).unwrap_err();
        assert!(bad(serde_json::json!([1])).contains("Objekt"));
        assert!(bad(serde_json::json!({"platforms": {}})).contains("version"));
        assert!(bad(manifest("zwei", "https://a/b", "s")).contains("Version"));
        assert!(bad(serde_json::json!({"version": "1.0.0"})).contains("platforms"));
        assert!(
            bad(serde_json::json!({"version": "1.0.0", "platforms": {
            "other-os-unknown": {"url": "https://a/b", "signature": "s"}}}))
            .contains("Plattform")
        );
        assert!(bad(manifest("1.0.0", "https://a/b", " ")).contains("signature"));
        assert!(bad(manifest("1.0.0", "http://example.com/a.exe", "s")).contains("nicht erlaubt"));
        assert!(parse_manifest(b"{", Channel::Stable, &keys()).unwrap_err().contains("JSON"));
        assert!(parse_manifest(&vec![b' '; MAX_MANIFEST + 1], Channel::Stable, &keys()).unwrap_err().contains("groß"));
        // Long release notes are shortened.
        let mut long = manifest("1.0.0", "https://a/b", "s");
        long["notes"] = serde_json::Value::String("x".repeat(MAX_NOTES + 50));
        let r = parse_manifest(long.to_string().as_bytes(), Channel::Beta, &keys()).unwrap();
        assert_eq!(r.notes.chars().count(), MAX_NOTES);
        assert_eq!(r.source, Channel::Beta);
    }

    #[test]
    fn due_rules() {
        let day = Duration::from_secs(86_400);
        assert!(due(true, 1_000, 1_000, day), "beim Start immer");
        assert!(due(false, 0, 5, day));
        assert!(!due(false, 1_000, 1_000 + 3_600_000, day));
        assert!(due(false, 1_000, 1_000 + 86_400_000, day));
        assert!(due(false, 10_000, 5_000, day), "Uhr zurückgestellt");
    }

    #[test]
    fn store_roundtrip_and_reconcile() {
        let home = tmp("store");
        let s = Store::new(&home);
        assert_eq!(s.load_state(), State::default());
        assert!(s.load_pending().is_none());
        let name = Store::installer_name(&v("0.1.1-test"));
        assert_eq!(name, "PaperRust-0.1.1-test-setup.exe");
        assert_eq!(Store::installer_name(&v("1.0.0+build.5")), "PaperRust-1.0.0_build.5-setup.exe");
        s.write(&s.file_path(&name), b"exe").unwrap();
        let p = Pending {
            version: "0.1.1-test".into(),
            channel: Channel::Beta,
            notes: "n".into(),
            pub_date: None,
            manifest: serde_json::json!({}),
            file: name.clone(),
            size: 3,
            sha256: sha256_hex(b"exe"),
            downloaded_ms: 1,
        };
        s.save_pending(&p).unwrap();
        assert_eq!(s.load_pending(), Some(p.clone()));
        // A path in the file name is not accepted.
        let mut evil = p.clone();
        evil.file = "../desktop.toml".into();
        s.save_pending(&evil).unwrap();
        assert!(s.load_pending().is_none());
        s.save_pending(&p).unwrap();

        // Installer ran, version does not match → failed, download stays.
        s.update_state(|st| st.installing = Some(Installing { version: "0.1.1-test".into(), from: "0.1.0".into(), started_ms: 2 }));
        assert_eq!(s.reconcile(&v("0.1.0")), StartNote::InstallFailed("0.1.1-test".into()));
        assert_eq!(s.load_state().failed_version.as_deref(), Some("0.1.1-test"));
        assert!(s.load_pending().is_some());
        // New run with matching version → installed, download gone.
        s.update_state(|st| st.installing = Some(Installing { version: "0.1.1-test".into(), from: "0.1.0".into(), started_ms: 3 }));
        assert_eq!(s.reconcile(&v("0.1.1-test")), StartNote::Installed("0.1.1-test".into()));
        let st = s.load_state();
        assert!(st.installing.is_none() && st.failed_version.is_none());
        assert!(s.load_pending().is_none());
        assert!(!s.file_path(&name).exists());
        assert_eq!(s.reconcile(&v("0.1.1-test")), StartNote::None);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn installer_command_line() {
        let a = installer_args(Path::new(r"C:\Users\x\AppData\Local\PaperRust Desktop"));
        assert_eq!(a, r"/P /UPDATE /D=C:\Users\x\AppData\Local\PaperRust Desktop");
        let d = tmp("inst");
        assert!(!installable(&d));
        std::fs::write(d.join("uninstall.exe"), b"").unwrap();
        assert_eq!(installable(&d), cfg!(windows));
        let _ = std::fs::remove_dir_all(&d);
    }

    // ---- with the real plugin (Tauri test runtime) -------------------------------------------

    fn mock_app(pubkey: &str) -> tauri::App<tauri::test::MockRuntime> {
        let mut ctx = tauri::test::mock_context(tauri::test::noop_assets());
        ctx.config_mut().version = Some("0.1.0".into());
        ctx.config_mut().plugins.0.insert("updater".into(), serde_json::json!({ "pubkey": pubkey, "requireSignedVersion": true }));
        tauri::test::mock_builder().plugin(tauri_plugin_updater::Builder::new().build()).build(ctx).unwrap()
    }

    /// Test data (`tests/fixtures/updater/`): `payload.bin`, signed with the local test key
    /// (`apps/desktop/pack/updater.ps1 -NewTestKey`, private under `results/pack/updater-test-key/`,
    /// not in the repo) for the version `0.1.1-test`; `payload.bin.noversion.sig` without version;
    /// `other.pub` a foreign key.
    #[test]
    fn plugin_verifies_signature_and_signed_version() {
        let app = mock_app(&fixture_str("test.pub"));
        let h = app.handle();
        assert_eq!(pubkey(h), fixture_str("test.pub"));
        let payload = fixture("payload.bin");
        let sig = fixture_str("payload.bin.sig");
        let m = manifest("0.1.1-test", "https://dl.example/p.exe", &sig);
        let u = to_update(h, &m).unwrap().expect("0.1.1-test > 0.1.0");
        assert_eq!(u.version, "0.1.1-test");
        u.verify(&payload).expect("gültige Signatur");

        let mut tampered = payload.clone();
        tampered[3] ^= 0x01;
        assert!(u.verify(&tampered).is_err(), "manipulierte Datei");
        let mut longer = payload.clone();
        longer.push(b'x');
        assert!(u.verify(&longer).is_err(), "angehängte Bytes");

        // Manifest promises a different version than signed (downgrade attack).
        let m2 = manifest("0.2.0", "https://dl.example/p.exe", &sig);
        let u2 = to_update(h, &m2).unwrap().unwrap();
        let e = u2.verify(&payload).unwrap_err().to_string();
        assert!(e.contains("0.1.1-test"), "{e}");
        // Signature without version is rejected.
        let m3 = manifest("0.1.1-test", "https://dl.example/p.exe", &fixture_str("payload.bin.noversion.sig"));
        assert!(to_update(h, &m3).unwrap().unwrap().verify(&payload).is_err());
        // Broken signature.
        let m4 = manifest("0.1.1-test", "https://dl.example/p.exe", "AAAA");
        assert!(to_update(h, &m4).unwrap().unwrap().verify(&payload).is_err());
        // Not newer → no update.
        assert!(to_update(h, &manifest("0.1.0", "https://a/b", &sig)).unwrap().is_none());

        // Foreign key.
        let other = mock_app(&fixture_str("other.pub"));
        let u = to_update(other.handle(), &m).unwrap().unwrap();
        assert!(u.verify(&payload).is_err(), "fremder Schlüssel");
    }

    type Routes = std::sync::Arc<std::sync::Mutex<Vec<(&'static str, Vec<u8>)>>>;

    /// Small HTTP server for [`check`] (loopback only); the routes can be swapped between the
    /// steps. Ends with `GET /quit`.
    fn serve(routes: Routes) -> (String, std::thread::JoinHandle<()>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { continue };
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                if path == "/quit" {
                    break;
                }
                let body = lock_routes(&routes).iter().find(|(p, _)| *p == path).map(|(_, b)| b.clone());
                let resp = match body {
                    Some(b) => {
                        let mut r = format!(
                            "HTTP/1.1 200 OK
Content-Length: {}
Connection: close

",
                            b.len()
                        )
                        .into_bytes();
                        r.extend(b);
                        r
                    }
                    None => b"HTTP/1.1 404 Not Found
Content-Length: 0
Connection: close

"
                    .to_vec(),
                };
                let _ = s.write_all(&resp);
            }
        });
        (base, h)
    }

    fn lock_routes(r: &Routes) -> std::sync::MutexGuard<'_, Vec<(&'static str, Vec<u8>)>> {
        r.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn eff(endpoint: &str, channel: Channel) -> Effective {
        let s = UpdateSettings { channel, endpoint: endpoint.into(), ..UpdateSettings::default() };
        effective(&s, |_| None, "KEY")
    }

    #[test]
    fn check_downloads_verifies_and_refuses_tampered() {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let app = mock_app(&fixture_str("test.pub"));
        let h = app.handle();
        let payload = fixture("payload.bin");
        let sig = fixture_str("payload.bin.sig");
        let mut tampered = payload.clone();
        tampered[0] ^= 0x20;
        let routes: Routes = Default::default();
        let (base, th) = serve(routes.clone());
        let url = format!("{base}/p.exe");
        let set_file = |file: Vec<u8>| {
            *lock_routes(&routes) = vec![
                ("/beta.json", manifest("0.1.1-test", &url, &sig).to_string().into_bytes()),
                ("/stable.json", manifest("0.1.0", &url, &sig).to_string().into_bytes()),
                ("/p.exe", file),
            ];
        };
        let home = tmp("check");
        let store = Store::new(&home);
        let ep = format!("{base}/{{{{channel}}}}.json");
        let cur = v("0.1.0");

        // 1) stable: nothing newer (0.1.1-test is a pre-release and only in the beta manifest).
        set_file(payload.clone());
        let o = rt.block_on(check(h, &store, &eff(&ep, Channel::Stable), &cur, true, 1));
        assert_eq!(o, Outcome::UpToDate);

        // 2) beta, file manipulated → rejected, nothing stored.
        set_file(tampered);
        let o = rt.block_on(check(h, &store, &eff(&ep, Channel::Beta), &cur, true, 2));
        match o {
            Outcome::Failed(e) => assert!(e.contains("Signatur ungültig"), "{e}"),
            o => panic!("erwartet Ablehnung, bekam {o:?}"),
        }
        assert!(store.load_pending().is_none());
        assert_eq!(std::fs::read_dir(&store.dir).map(|d| d.count()).unwrap_or(0), 0, "keine Datei abgelegt");

        // 3) portable installation: hint only, no download.
        let o = rt.block_on(check(h, &store, &eff(&ep, Channel::Beta), &cur, false, 3));
        assert!(matches!(o, Outcome::Available(ref r) if r.version == v("0.1.1-test")));

        // 4) correct file → checked and stored; renewed check before installing ok.
        set_file(payload.clone());
        let o = rt.block_on(check(h, &store, &eff(&ep, Channel::Beta), &cur, true, 4));
        let Outcome::Ready(p) = o else { panic!("erwartet Ready, bekam {o:?}") };
        assert_eq!(p.version, "0.1.1-test");
        assert_eq!(p.channel, Channel::Beta);
        assert_eq!(p.sha256, sha256_hex(&payload));
        assert_eq!(std::fs::read(store.file_path(&p.file)).unwrap(), payload);
        let (vp, path) = verify_pending(h, &store).unwrap();
        assert_eq!(vp.version, "0.1.1-test");
        assert_eq!(path, store.file_path(&p.file));
        // Already present → no second download.
        let o = rt.block_on(check(h, &store, &eff(&ep, Channel::Beta), &cur, true, 5));
        assert!(matches!(o, Outcome::Ready(ref q) if q.downloaded_ms == 4));
        // Back to stable → pre-release is discarded.
        let o = rt.block_on(check(h, &store, &eff(&ep, Channel::Stable), &cur, true, 6));
        assert_eq!(o, Outcome::UpToDate);
        assert!(store.load_pending().is_none());

        // 5) file manipulated on disk → installing refused, download discarded.
        let o = rt.block_on(check(h, &store, &eff(&ep, Channel::Beta), &cur, true, 7));
        let Outcome::Ready(p) = o else { panic!("erwartet Ready, bekam {o:?}") };
        let f = store.file_path(&p.file);
        let mut b = std::fs::read(&f).unwrap();
        b[5] ^= 0x01;
        std::fs::write(&f, b).unwrap();
        let e = verify_pending(h, &store).unwrap_err();
        assert!(e.contains("Signatur ungültig"), "{e}");
        assert!(store.load_pending().is_none() && !f.exists());

        // 6) server gone → error, no crash.
        if let Ok(mut s) = std::net::TcpStream::connect(base.trim_start_matches("http://")) {
            let _ = s.write_all(
                b"GET /quit HTTP/1.1

",
            );
        }
        th.join().unwrap();
        let o = rt.block_on(check(h, &store, &eff(&ep, Channel::Stable), &cur, true, 8));
        assert!(matches!(o, Outcome::Failed(_)), "{o:?}");
        let _ = std::fs::remove_dir_all(&home);
    }
}
