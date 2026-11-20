//! Reconciliation (docs/SYNC.md §4.5): initial sync, running feed with long poll, full sync via
//! `/sync/ids`, selection via `/sync/resolve`, contents via `/objects/batch`, files.
//!
//! Without a feed (server without the `sync` feature, `/sync/changes` → 501) the core works in
//! **query mode**: metadata via `ids` + `batch`, documents via `resolve` + `batch`, every
//! [`POLL_INTERVAL`] seconds. As soon as the server offers the feed, it switches to long poll.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{Mutex, Notify, watch};
use zeroize::Zeroizing;

use crate::error::{Error, Result};
use crate::files::FileCache;
use crate::remote::{ChangePage, PaperRustRemote, ServerInfo};
use crate::secret::{SecretStore, target_for};
use crate::selection::{Limits, Plan, Wants, plan};
use crate::store::{META_KINDS, Replica, SelectionItem, now};
use crate::tls::TlsOptions;

/// Page size of the feed.
pub const FEED_LIMIT: u32 = 1000;
/// Long-poll wait time (server limits to 30 s).
pub const LONG_POLL_SECS: u64 = 25;
/// Query interval without feed.
pub const POLL_INTERVAL: Duration = Duration::from_secs(60);
/// `resolve` again at most this often (docs/SYNC.md §4.5).
pub const RESOLVE_THROTTLE: Duration = Duration::from_secs(30);
/// Full sync after 24 h at the latest.
pub const FULL_SYNC_EVERY: chrono::Duration = chrono::Duration::hours(24);
/// Documents no longer selected stay for 7 days.
pub const UNSELECTED_RETENTION: chrono::Duration = chrono::Duration::days(7);
/// Parallel file downloads.
pub const FILE_PARALLEL: usize = 4;
/// UUIDs per `objects/batch`.
pub const BATCH: usize = 500;

/// Profile of the client core.
#[derive(Debug, Clone)]
pub struct ProfileConfig {
    /// Profile ID (folder name, keychain entry `PaperRust/<id>`).
    pub profile_id: String,
    /// Server base URL (origin, possibly with a subpath).
    pub url: String,
    pub tls: TlsOptions,
    /// Profile folder; contains `replica.sqlite` and `files/`.
    pub dir: PathBuf,
    /// Device name for the device token.
    pub device_name: String,
    /// `paperrust` (API v2, full sync) | `ngx` (paperless-ngx via v1, restricted, SYNC.md §7)
    pub flavor: String,
}

impl ProfileConfig {
    pub fn is_ngx(&self) -> bool {
        self.flavor == "ngx"
    }
}

#[path = "engine_ngx.rs"]
mod ngx;

/// State for settings/banner (`/local/status`).
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct Status {
    /// `idle` | `syncing` | `offline` | `auth_required` | `limit_exceeded` | `error` | `pin_mismatch`
    pub state: String,
    pub online: bool,
    /// `feed` (long poll) | `polling` (server without feed)
    pub mode: String,
    pub logged_in: bool,
    pub last_sync: Option<String>,
    pub last_error: Option<String>,
    pub documents: i64,
    pub phase: Option<String>,
    pub done: u64,
    pub total: u64,
    /// Token permanently in the keychain (otherwise only in memory).
    pub token_persistent: bool,
}

/// Result of a reconciliation (for tests/measurements).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct SyncReport {
    pub full: bool,
    pub feed_changes: u64,
    pub objects_upserted: u64,
    pub objects_deleted: u64,
    pub documents_fetched: u64,
    pub documents_removed: u64,
    pub files_downloaded: u64,
    pub file_errors: u64,
    pub resolved: bool,
}

struct Inner {
    cfg: ProfileConfig,
    replica: Replica,
    remote: PaperRustRemote,
    /// only for `flavor = ngx`
    ngx: Option<crate::remote::NgxRemote>,
    files: FileCache,
    secrets: Arc<dyn SecretStore>,
    status: watch::Sender<Status>,
    run_lock: Mutex<()>,
    resolve_dirty: AtomicBool,
    last_resolve: std::sync::Mutex<Option<Instant>>,
    has_feed: AtomicBool,
    resolve_throttle: std::sync::Mutex<Duration>,
    /// paperless-ngx: interval of the fingerprint check (default 15 min)
    fingerprint_every: std::sync::Mutex<Duration>,
    wake: Notify,
}

impl Inner {
    fn source(&self) -> crate::files::Source<'_> {
        match &self.ngx {
            Some(n) => crate::files::Source::Ngx(n),
            None => crate::files::Source::PaperRust(&self.remote),
        }
    }
}

/// The client core of a profile.
#[derive(Clone)]
pub struct SyncClient {
    inner: Arc<Inner>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k).and_then(Value::as_str).map(str::to_string)
}

impl SyncClient {
    /// Open profile (create/migrate replica, load token from the keychain).
    pub async fn open(cfg: ProfileConfig, secrets: Arc<dyn SecretStore>) -> Result<SyncClient> {
        std::fs::create_dir_all(&cfg.dir)?;
        let replica = Replica::open(&cfg.dir.join("replica.sqlite")).await?;
        let remote = PaperRustRemote::new(&cfg.url, &cfg.tls)?;
        let ngx = if cfg.is_ngx() { Some(crate::remote::NgxRemote::new(&cfg.url, &cfg.tls)?) } else { None };
        replica.ensure_remote(remote.base().as_str(), if cfg.is_ngx() { "ngx" } else { "paperrust" }).await?;
        let limits = Self::limits_of(&replica).await?;
        let files = FileCache::new(cfg.dir.join("files"), cache_limit(limits));
        let (token, device) = split_secret(secrets.get(&target_for(&cfg.profile_id))?);
        let logged_in = token.is_some();
        if let Some(n) = &ngx {
            n.set_token(token.clone());
        }
        remote.set_token(token);
        // Replica new (deleted/copied), token still in the keychain → take device UUID from there
        if let Some(d) = device
            && replica.remote().await?.and_then(|r| r.device_uuid).is_none()
        {
            replica.set_remote_field("device_uuid", Some(d.as_str())).await?;
        }
        let documents = replica.count_documents(true).await?;
        let last_sync = replica.remote().await?.and_then(|r| r.last_contact);
        let (tx, _) = watch::channel(Status {
            state: "idle".into(),
            mode: "feed".into(),
            online: logged_in,
            logged_in,
            documents,
            last_sync,
            token_persistent: secrets.persistent(),
            ..Default::default()
        });
        Ok(SyncClient {
            inner: Arc::new(Inner {
                cfg,
                replica,
                remote,
                ngx,
                files,
                secrets,
                status: tx,
                run_lock: Mutex::new(()),
                resolve_dirty: AtomicBool::new(true),
                last_resolve: std::sync::Mutex::new(None),
                has_feed: AtomicBool::new(true),
                resolve_throttle: std::sync::Mutex::new(RESOLVE_THROTTLE),
                fingerprint_every: std::sync::Mutex::new(ngx::FINGERPRINT_EVERY),
                wake: Notify::new(),
            }),
        })
    }

    pub fn replica(&self) -> &Replica {
        &self.inner.replica
    }

    /// paperless-ngx profile (restricted sync via v1)?
    pub fn is_ngx(&self) -> bool {
        self.inner.ngx.is_some()
    }

    /// Raw forwarding to the server (gateway), with device or v1 token depending on the flavor.
    pub async fn raw(&self, method: &str, path_and_query: &str, headers: &[(String, String)], body: Vec<u8>) -> Result<crate::remote::RawResponse> {
        match &self.inner.ngx {
            Some(n) => n.raw(method, path_and_query, headers, body).await,
            None => self.inner.remote.raw(method, path_and_query, headers, body).await,
        }
    }

    /// Base URL of the server (normalized, with `/` at the end).
    pub fn base_url(&self) -> &url::Url {
        self.inner.remote.base()
    }

    /// Credentials for `/ws/status/` on the server: only PaperRust (device token, `Bearer prt_…`);
    /// paperless-ngx accepts only the session cookie on the socket → `None`.
    pub(crate) fn ws_authorization(&self) -> Option<zeroize::Zeroizing<String>> {
        if self.is_ngx() { None } else { self.inner.remote.bearer() }
    }

    pub fn remote(&self) -> &PaperRustRemote {
        &self.inner.remote
    }

    pub fn files(&self) -> &FileCache {
        &self.inner.files
    }

    pub fn config(&self) -> &ProfileConfig {
        &self.inner.cfg
    }

    /// Minimum interval between two `resolve` runs (default [`RESOLVE_THROTTLE`]; tests 0).
    pub fn set_resolve_throttle(&self, d: Duration) {
        if let Ok(mut t) = self.inner.resolve_throttle.lock() {
            *t = d;
        }
    }

    fn resolve_throttle(&self) -> Duration {
        self.inner.resolve_throttle.lock().map(|t| *t).unwrap_or(RESOLVE_THROTTLE)
    }

    /// Remaining time until `resolve` is allowed again, if one is pending.
    fn resolve_pending_in(&self) -> Option<Duration> {
        if !self.inner.resolve_dirty.load(Ordering::SeqCst) {
            return None;
        }
        let last = self.inner.last_resolve.lock().ok().and_then(|l| *l);
        Some(match last {
            Some(t) => self.resolve_throttle().saturating_sub(t.elapsed()),
            None => Duration::ZERO,
        })
    }

    pub fn status(&self) -> Status {
        self.inner.status.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Status> {
        self.inner.status.subscribe()
    }

    /// Online request failed on the network (gateway) → show offline, background keeps checking.
    pub fn mark_offline(&self) {
        self.set_status(|s| {
            s.online = false;
            s.state = "offline".into();
        });
        self.wake();
    }

    /// Server reported 401 (token revoked) → log in again.
    pub fn mark_unauthenticated(&self) {
        self.set_status(|s| {
            s.logged_in = false;
            s.state = "auth_required".into();
        });
    }

    fn set_status(&self, f: impl FnOnce(&mut Status)) {
        self.inner.status.send_modify(f);
    }

    /// Abort the running long poll/wait and reconcile immediately.
    pub fn wake(&self) {
        self.inner.wake.notify_waiters();
        self.inner.wake.notify_one();
    }

    // ------------------------------------------------------------------ limits/settings

    async fn limits_of(r: &Replica) -> Result<Limits> {
        let d = Limits::default();
        let get = |v: Option<String>, def: u64| v.and_then(|s| s.parse::<u64>().ok()).unwrap_or(def);
        Ok(Limits {
            max_bytes: get(r.setting("max_bytes").await?, d.max_bytes),
            max_documents: get(r.setting("max_documents").await?, d.max_documents),
        })
    }

    pub async fn limits(&self) -> Result<Limits> {
        Self::limits_of(&self.inner.replica).await
    }

    pub async fn set_limits(&self, l: Limits) -> Result<()> {
        self.inner.replica.set_setting("max_bytes", &l.max_bytes.to_string()).await?;
        self.inner.replica.set_setting("max_documents", &l.max_documents.to_string()).await?;
        self.inner.files.set_max_bytes(cache_limit(l));
        Ok(())
    }

    // ------------------------------------------------------------------ login

    /// Fetch a device token and store it in the keychain. The password stays only in `password`
    /// (overwritten when leaving).
    pub async fn login(&self, username: &str, password: Zeroizing<String>, code: Option<&str>) -> Result<()> {
        if self.inner.ngx.is_some() {
            return self.login_ngx(username, password, code).await;
        }
        let grant = self.inner.remote.create_device(username, &password, code, &self.inner.cfg.device_name).await;
        drop(password);
        let grant = grant?;
        // Keychain: token + device UUID (for revocation even without a replica)
        let secret = Zeroizing::new(format!("{}\n{}", grant.token.as_str(), grant.device_uuid));
        self.inner.secrets.set(&target_for(&self.inner.cfg.profile_id), &secret)?;
        self.inner.remote.set_token(Some(grant.token.clone()));
        let r = &self.inner.replica;
        r.set_remote_field("user_name", Some(username)).await?;
        r.set_remote_field("device_uuid", Some(&grant.device_uuid)).await?;
        self.set_status(|s| {
            s.logged_in = true;
            if s.state == "auth_required" {
                s.state = "idle".into();
            }
        });
        self.wake();
        Ok(())
    }

    /// Log out: revoke the device on the server (if reachable), delete the keychain entry; with
    /// `wipe` additionally remove replica and file cache (remove profile).
    pub async fn logout(&self, wipe: bool) -> Result<bool> {
        let mut revoked = false;
        if let Some(n) = &self.inner.ngx {
            // paperless-ngx: one token per user, not revocable per device (SYNC.md §7)
            n.set_token(None);
        }
        if let Some(dev) = self.inner.replica.remote().await?.and_then(|r| r.device_uuid)
            && self.inner.ngx.is_none()
            && self.inner.remote.has_token()
        {
            revoked = self.inner.remote.revoke_device(&dev).await.is_ok();
        }
        self.inner.secrets.delete(&target_for(&self.inner.cfg.profile_id))?;
        self.inner.remote.set_token(None);
        self.inner.replica.set_remote_field("device_uuid", None).await?;
        if wipe {
            self.wipe_data().await?;
        }
        self.set_status(|s| {
            s.logged_in = false;
            s.state = "auth_required".into();
        });
        Ok(revoked)
    }

    async fn wipe_data(&self) -> Result<()> {
        self.inner.replica.reset_data().await?;
        self.inner.resolve_dirty.store(true, Ordering::SeqCst);
        if let Ok(mut l) = self.inner.last_resolve.lock() {
            *l = None;
        }
        let root = self.inner.files.root().to_path_buf();
        if root.is_dir() {
            tokio::fs::remove_dir_all(&root).await?;
        }
        self.set_status(|s| s.documents = 0);
        Ok(())
    }

    // ------------------------------------------------------------------ selection

    /// Resolve the selection and check it against the limits (without adopting).
    pub async fn preview_selection(&self, items: &[SelectionItem]) -> Result<Plan> {
        if self.inner.ngx.is_some() {
            return self.preview_selection_ngx(items).await;
        }
        for it in items {
            if !matches!(it.kind.as_str(), "document" | "tag" | "saved_view") {
                return Err(Error::Invalid(format!("selection type {}", it.kind)));
            }
            if !matches!(it.files.as_str(), "preferred" | "original" | "both" | "none") {
                return Err(Error::Invalid(format!("files mode {}", it.files)));
            }
        }
        let mut as_of = None;
        // Resolve individual documents together (one request per ≤ 5,000), tags/views individually
        let doc_keys: Vec<String> = items.iter().filter(|i| i.kind == "document").map(|i| i.key.clone()).collect();
        let mut by_uuid: HashMap<String, crate::remote::ResolvedDoc> = HashMap::new();
        for chunk in doc_keys.chunks(5_000).map(<[String]>::to_vec).collect::<Vec<_>>() {
            let r = self.inner.remote.resolve(&chunk, &[], &[], Some("any")).await?;
            as_of = as_of.max(r.as_of_seq);
            by_uuid.extend(r.documents.into_iter().map(|d| (d.uuid.clone(), d)));
        }
        let mut resolved = Vec::new();
        for it in items.iter().cloned() {
            let docs = match it.kind.as_str() {
                "document" => by_uuid.get(&it.key).cloned().into_iter().collect(),
                kind => {
                    let one = vec![it.key.clone()];
                    let (tags, views): (&[String], &[String]) = if kind == "tag" { (&one, &[]) } else { (&[], &one) };
                    let r = self.inner.remote.resolve(&[], tags, views, Some("any")).await?;
                    as_of = as_of.max(r.as_of_seq);
                    r.documents
                }
            };
            resolved.push((it, docs));
        }
        Ok(plan(resolved, self.limits().await?, as_of))
    }

    /// Adopt the selection; over the limit → [`Error::LimitExceeded`] (nothing changed).
    pub async fn apply_selection(&self, items: &[SelectionItem]) -> Result<Plan> {
        let p = self.preview_selection(items).await?;
        if !p.within_limits {
            return Err(Error::LimitExceeded(p.message.clone().unwrap_or_default()));
        }
        self.inner.replica.set_selection(items).await?;
        self.inner.resolve_dirty.store(true, Ordering::SeqCst);
        if let Ok(mut l) = self.inner.last_resolve.lock() {
            *l = None;
        }
        self.wake();
        Ok(p)
    }

    // ------------------------------------------------------------------ reconciliation

    /// Check the server; foreign `server_uuid` → reset the replica.
    async fn check_server(&self) -> Result<ServerInfo> {
        let info = self.inner.remote.server().await?;
        if info.product != "paperrust" || info.api != 2 {
            return Err(Error::Protocol(format!("not a PaperRust v2 server ({} api {})", info.product, info.api)));
        }
        let has_feed = info.has("sync") && info.server_uuid.is_some();
        self.inner.has_feed.store(has_feed, Ordering::SeqCst);
        let r = &self.inner.replica;
        let row = r.remote().await?.unwrap_or_default();
        if let Some(new) = &info.server_uuid {
            match &row.server_uuid {
                Some(old) if old != new => {
                    tracing::warn!("server identity changed; resetting the local replica");
                    self.wipe_data().await?;
                    r.set_remote_field("server_uuid", Some(new)).await?;
                }
                None => r.set_remote_field("server_uuid", Some(new)).await?,
                _ => {}
            }
        }
        Ok(info)
    }

    /// One complete reconciliation run (without long poll). Serialized via a lock.
    ///
    /// Returned as a boxed `Send` future (otherwise `tokio::spawn(run)` fails with
    /// rust-lang/rust#102211 – "Send is not general enough").
    pub fn sync_once(&self) -> BoxFuture<'_, Result<SyncReport>> {
        Box::pin(async move {
            let _g = self.inner.run_lock.lock().await;
            self.set_status(|s| {
                s.state = "syncing".into();
                s.phase = Some("server".into());
            });
            let res = self.sync_inner().await;
            self.after_run(outcome(&res)).await;
            res
        })
    }

    fn after_run(&self, outcome: Option<Outcome>) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let docs = self.inner.replica.count_documents(true).await.unwrap_or(0);
            let ts = now();
            if outcome.is_none() {
                let _ = self.inner.replica.set_remote_field("last_contact", Some(ts.as_str())).await;
            }
            let feed = self.inner.has_feed.load(Ordering::SeqCst);
            self.set_status(move |s| {
                s.documents = docs;
                s.phase = None;
                s.mode = if feed { "feed".into() } else { "polling".into() };
                match outcome {
                    None => {
                        s.state = "idle".into();
                        s.online = true;
                        s.last_sync = Some(ts);
                        s.last_error = None;
                    }
                    Some(o) => {
                        s.state = o.state.into();
                        s.online = o.online;
                        if o.unauthenticated {
                            s.logged_in = false;
                        }
                        s.last_error = Some(o.message);
                    }
                }
            });
        })
    }

    async fn sync_inner(&self) -> Result<SyncReport> {
        if !self.inner.remote.has_token() {
            return Err(Error::Unauthenticated);
        }
        if self.inner.ngx.is_some() {
            return self.sync_ngx().await;
        }
        let info = sendable(self.check_server()).await?;
        let r = &self.inner.replica;
        let mut rep = SyncReport::default();
        let row = sendable(r.remote()).await?.unwrap_or_default();
        let feed = self.inner.has_feed.load(Ordering::SeqCst);

        if sendable(r.remote_text("me_json")).await?.is_none() || row.cursor.is_none() {
            sendable(self.refresh_profile_data()).await;
        }

        let full_due = row.last_full_sync.as_deref().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok()).is_none_or(|t| {
            chrono::Utc::now().signed_duration_since(t.with_timezone(&chrono::Utc)) > FULL_SYNC_EVERY
        });
        let epoch_changed = matches!((row.perm_epoch, info.perm_epoch), (Some(a), Some(b)) if a != b);

        if feed {
            match row.cursor.clone() {
                None => {
                    sendable(self.initial_metadata(&mut rep)).await?;
                }
                Some(c) => match sendable(self.follow_feed(Some(c), 0, &mut rep)).await {
                    Err(Error::CursorExpired) => {
                        r.set_remote_field("cursor", None).await?;
                        sendable(self.initial_metadata(&mut rep)).await?;
                        sendable(self.full_reconcile(&mut rep)).await?;
                    }
                    other => other?,
                },
            }
            if (full_due && row.cursor.is_some()) || epoch_changed {
                sendable(self.full_reconcile(&mut rep)).await?;
            }
            sendable(r.set_perm_epoch(info.perm_epoch)).await?;
        } else {
            // Query mode: metadata completely via ids + batch
            sendable(self.full_reconcile(&mut rep)).await?;
            self.inner.resolve_dirty.store(true, Ordering::SeqCst);
        }

        if self.take_resolve_due() {
            sendable(self.resolve_and_fetch(&mut rep)).await?;
        } else {
            sendable(self.fetch_stale_details(&mut rep)).await?;
        }
        sendable(self.housekeeping()).await?;
        Ok(rep)
    }

    async fn refresh_profile_data(&self) {
        let r = &self.inner.replica;
        if let Ok(me) = self.inner.remote.me().await {
            let _ = r.set_remote_field("me_json", Some(&me.to_string())).await;
        }
        if let Ok(ui) = self.inner.remote.ui_settings().await {
            let _ = r.set_remote_field("ui_settings", Some(&ui.to_string())).await;
        }
    }

    fn take_resolve_due(&self) -> bool {
        if !self.inner.resolve_dirty.load(Ordering::SeqCst) {
            return false;
        }
        let mut last = match self.inner.last_resolve.lock() {
            Ok(l) => l,
            Err(_) => return true,
        };
        let throttle = self.resolve_throttle();
        if last.is_some_and(|t| t.elapsed() < throttle) {
            return false;
        }
        *last = Some(Instant::now());
        self.inner.resolve_dirty.store(false, Ordering::SeqCst);
        true
    }

    /// Initial sync of the metadata: feed without cursor, metadata kinds only.
    async fn initial_metadata(&self, rep: &mut SyncReport) -> Result<()> {
        rep.full = true;
        self.set_status(|s| s.phase = Some("metadata".into()));
        let mut cursor: Option<String> = None;
        loop {
            let page = self.inner.remote.changes(cursor.as_deref(), FEED_LIMIT, Some(&META_KINDS), 0).await?;
            let more = page.has_more;
            cursor = Some(page.next_cursor.clone());
            self.apply_page(page, rep).await?;
            if !more {
                break;
            }
        }
        let r = &self.inner.replica;
        r.set_remote_field("cursor", cursor.as_deref()).await?;
        r.set_remote_field("last_full_sync", Some(&now())).await?;
        self.inner.resolve_dirty.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Read the feed from `cursor` until `has_more = false` (first request with `wait`).
    async fn follow_feed(&self, mut cursor: Option<String>, wait: u64, rep: &mut SyncReport) -> Result<()> {
        let mut first = true;
        loop {
            let page = self.inner.remote.changes(cursor.as_deref(), FEED_LIMIT, None, if first { wait } else { 0 }).await?;
            first = false;
            let more = page.has_more;
            let next = page.next_cursor.clone();
            if let Some(e) = page.perm_epoch {
                let old = self.inner.replica.remote().await?.and_then(|r| r.perm_epoch);
                if old.is_some_and(|o| o != e) {
                    self.full_reconcile(rep).await?;
                    self.inner.replica.set_perm_epoch(Some(e)).await?;
                }
            }
            self.apply_page(page, rep).await?;
            self.inner.replica.set_remote_field("cursor", Some(&next)).await?;
            cursor = Some(next);
            if !more {
                return Ok(());
            }
        }
    }

    /// Apply a feed page.
    async fn apply_page(&self, page: ChangePage, rep: &mut SyncReport) -> Result<()> {
        let r = &self.inner.replica;
        let selection = r.selection().await?;
        let dynamic = selection.iter().any(|s| s.kind != "document");
        let selected_docs: HashSet<String> = selection.iter().filter(|s| s.kind == "document").map(|s| s.key.clone()).collect();
        let mut by_kind: HashMap<String, Vec<Value>> = HashMap::new();
        for ch in page.changes {
            rep.feed_changes += 1;
            match (ch.kind.as_str(), ch.op.as_str()) {
                ("document", "delete") => {
                    let paths = r.delete_document(&ch.uuid).await?;
                    FileCache::remove_paths(&paths).await;
                    rep.documents_removed += 1;
                }
                ("document", _) => {
                    let local = r.document_state(&ch.uuid).await?;
                    match (local, ch.object) {
                        (Some((rev, sel, _)), Some(obj)) if Some(rev) != ch.rev => {
                            r.upsert_documents(&[obj], false, sel).await?;
                        }
                        (None, _) if dynamic || selected_docs.contains(&ch.uuid) => {
                            self.inner.resolve_dirty.store(true, Ordering::SeqCst);
                        }
                        _ => {}
                    }
                    if dynamic {
                        // A tag change can bring a document into or out of the selection
                        self.inner.resolve_dirty.store(true, Ordering::SeqCst);
                    }
                }
                (kind, "delete") => {
                    if r.delete_object(kind, &ch.uuid).await? {
                        rep.objects_deleted += 1;
                    }
                    if dynamic && matches!(kind, "tag" | "saved_view") {
                        self.inner.resolve_dirty.store(true, Ordering::SeqCst);
                    }
                }
                (kind, _) => {
                    if let Some(obj) = ch.object {
                        by_kind.entry(kind.to_string()).or_default().push(obj);
                    }
                    if dynamic && matches!(kind, "tag" | "saved_view") {
                        self.inner.resolve_dirty.store(true, Ordering::SeqCst);
                    }
                }
            }
        }
        for (kind, objs) in by_kind {
            rep.objects_upserted += objs.len() as u64;
            r.upsert_objects(&kind, &objs).await?;
        }
        Ok(())
    }

    /// Full sync via `/sync/ids` (deletions/revocation, missing/differing metadata).
    async fn full_reconcile(&self, rep: &mut SyncReport) -> Result<()> {
        rep.full = true;
        self.set_status(|s| s.phase = Some("reconcile".into()));
        let r = &self.inner.replica;
        for kind in META_KINDS {
            let ids = match self.inner.remote.ids(kind).await {
                Ok(i) => i,
                // Kind not readable for this user (e.g. users without auth.view_user)
                Err(Error::Api { status: 403 | 404, .. }) => crate::remote::Ids { items: vec![], as_of_seq: None },
                Err(e) => return Err(e),
            };
            let remote: HashMap<String, i64> = ids.items.into_iter().collect();
            let local: HashMap<String, i64> = r.object_revs(kind).await?.into_iter().collect();
            // first collect, then delete (no iterator with a closure across `await`, rust#102211)
            let gone: Vec<String> = local.keys().filter(|k| !remote.contains_key(*k)).cloned().collect();
            for k in gone {
                r.delete_object(kind, &k).await?;
                rep.objects_deleted += 1;
            }
            let fetch: Vec<String> = remote.iter().filter(|(k, rev)| local.get(*k) != Some(rev)).map(|(k, _)| k.clone()).collect();
            for chunk in owned_chunks(&fetch) {
                let b = self.inner.remote.batch(kind, &chunk, &[]).await?;
                rep.objects_upserted += b.items.len() as u64;
                r.upsert_objects(kind, &b.items).await?;
            }
        }
        let ids = self.inner.remote.ids("document").await?;
        let remote: HashMap<String, i64> = ids.items.into_iter().collect();
        for (key, rev, _sel, _) in r.document_states().await? {
            match remote.get(&key) {
                None => {
                    let paths = r.delete_document(&key).await?;
                    FileCache::remove_paths(&paths).await;
                    rep.documents_removed += 1;
                }
                Some(rv) if *rv != rev => self.inner.resolve_dirty.store(true, Ordering::SeqCst),
                _ => {}
            }
        }
        r.set_remote_field("last_full_sync", Some(&now())).await?;
        Ok(())
    }

    /// Resolve selection, fetch contents, remove deselected, load files.
    async fn resolve_and_fetch(&self, rep: &mut SyncReport) -> Result<()> {
        rep.resolved = true;
        self.set_status(|s| s.phase = Some("resolve".into()));
        let r = &self.inner.replica;
        let items = r.selection().await?;
        let p = self.preview_selection(&items).await?;
        if !p.within_limits {
            return Err(Error::LimitExceeded(p.message.unwrap_or_default()));
        }
        // deselected
        let at = now();
        let states = r.document_states().await?;
        let gone: Vec<String> = states.iter().filter(|(k, _, sel, _)| *sel && !p.docs.contains_key(k)).map(|(k, ..)| k.clone()).collect();
        for key in gone {
            r.mark_unselected(&key, &at).await?;
            self.inner.files.remove_doc(r, &key).await?;
        }
        // Fetch contents: new, different rev, without details or selected again
        let local: HashMap<String, (i64, bool, bool)> = states.into_iter().map(|(k, rev, sel, det)| (k, (rev, sel, det))).collect();
        let need: Vec<String> = p
            .docs
            .iter()
            .filter(|(k, d)| match local.get(k.as_str()) {
                Some((rev, sel, det)) => *rev != d.rev || !*sel || !*det,
                None => true,
            })
            .map(|(k, _)| k.clone())
            .collect();
        self.fetch_details(&need, rep).await?;
        // Dateien
        self.fetch_files(&p, rep).await?;
        Ok(())
    }

    /// Fetch documents with `content`/`notes` (in blocks of 500).
    async fn fetch_details(&self, keys: &[String], rep: &mut SyncReport) -> Result<()> {
        let total = keys.len() as u64;
        self.set_status(|s| {
            s.phase = Some("documents".into());
            s.done = 0;
            s.total = total;
        });
        let r = &self.inner.replica;
        let mut done = 0;
        for chunk in owned_chunks(keys) {
            let b = self.inner.remote.batch("document", &chunk, &["content", "notes"]).await?;
            r.upsert_documents(&b.items, true, true).await?;
            rep.documents_fetched += b.items.len() as u64;
            for m in b.missing {
                let paths = r.delete_document(&m).await?;
                FileCache::remove_paths(&paths).await;
            }
            done += chunk.len() as u64;
            self.set_status(|s| s.done = done);
        }
        Ok(())
    }

    /// Selected documents whose content is outdated (feed update without details).
    async fn fetch_stale_details(&self, rep: &mut SyncReport) -> Result<()> {
        let stale: Vec<String> = self
            .inner
            .replica
            .document_states()
            .await?
            .into_iter()
            .filter(|(_, _, sel, det)| *sel && !*det)
            .map(|(k, ..)| k)
            .collect();
        if stale.is_empty() {
            return Ok(());
        }
        self.fetch_details(&stale, rep).await?;
        // Files possibly changed (new checksum) → reload for these documents
        let items = self.inner.replica.selection().await?;
        if items.iter().all(|i| i.files == "none") {
            return Ok(());
        }
        let wants = items.iter().fold(Wants::default(), |w, i| w.union(Wants::from_mode(&i.files)));
        let mut jobs = Vec::new();
        for k in stale {
            if let Some(doc) = self.inner.replica.document_json(&k).await? {
                let has_archive = !doc["files"]["archive"].is_null();
                for v in wants.variants(has_archive) {
                    jobs.push((doc.clone(), v.to_string()));
                }
                jobs.push((doc, "thumb".to_string()));
            }
        }
        self.run_file_jobs(jobs, rep).await
    }

    async fn fetch_files(&self, p: &Plan, rep: &mut SyncReport) -> Result<()> {
        let mut jobs = Vec::new();
        let planned: Vec<(String, Wants)> = p.docs.iter().map(|(k, d)| (k.clone(), d.wants)).collect();
        for (key, wants) in planned {
            let key = key.as_str();
            let Some(doc) = self.inner.replica.document_json(key).await? else { continue };
            let has_archive = !doc["files"]["archive"].is_null();
            let variants = wants.variants(has_archive);
            // delete variants no longer wanted
            for f in self.inner.replica.files_of(key).await? {
                if f.variant != "thumb" && !variants.contains(&f.variant.as_str()) {
                    let _ = tokio::fs::remove_file(&f.path).await;
                    self.inner.replica.delete_file(key, &f.variant).await?;
                }
            }
            for v in variants {
                jobs.push((doc.clone(), v.to_string()));
            }
            jobs.push((doc, "thumb".to_string()));
        }
        self.run_file_jobs(jobs, rep).await
    }

    async fn run_file_jobs(&self, jobs: Vec<(Value, String)>, rep: &mut SyncReport) -> Result<()> {
        let total = jobs.len() as u64;
        self.set_status(|s| {
            s.phase = Some("files".into());
            s.done = 0;
            s.total = total;
        });
        // one Arc per future (no borrowed data in closure futures, rust#102211)
        let inner = self.inner.clone();
        let mut stream = futures::stream::iter(jobs.into_iter().map(move |(doc, v): (Value, String)| {
            let inner = inner.clone();
            let fut: BoxFuture<'static, (Option<String>, String, Result<crate::files::Fetched>)> = Box::pin(async move {
                let res = inner.files.fetch(&inner.replica, inner.source(), &doc, &v, true).await;
                (s(&doc, "uuid"), v, res)
            });
            fut
        }))
        .buffer_unordered(FILE_PARALLEL);
        let mut done = 0u64;
        let mut fatal = None;
        while let Some((key, v, res)) = stream.next().await {
            done += 1;
            match res {
                Ok(crate::files::Fetched::Unchanged) => {}
                Ok(_) => rep.files_downloaded += 1,
                // Network gone → abort the run (resume next time)
                Err(e @ (Error::Offline(_) | Error::Unauthenticated | Error::PinMismatch)) => {
                    fatal.get_or_insert(e);
                }
                Err(Error::Api { status: 404, .. }) => {
                    // e.g. thumbnail missing on the server – not an error of the reconciliation
                }
                Err(e) => {
                    rep.file_errors += 1;
                    tracing::warn!(doc = key.as_deref().unwrap_or(""), variant = v.as_str(), "file download failed: {e}");
                }
            }
            if done.is_multiple_of(16) || done == total {
                self.set_status(|s| s.done = done);
            }
        }
        match fatal {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn housekeeping(&self) -> Result<()> {
        let before = (chrono::Utc::now() - UNSELECTED_RETENTION).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let gone = self.inner.replica.purge_unselected(&before).await?;
        for k in gone {
            self.inner.files.remove_doc(&self.inner.replica, &k).await?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------ continuous operation

    /// Loop until `shutdown` becomes true: reconciliation, then long poll (or query mode), on
    /// network errors retry with backoff (2 s … 2 min), on 401 wait for [`Self::login`].
    pub async fn run(self, mut shutdown: watch::Receiver<bool>) {
        let mut backoff = Duration::from_secs(2);
        loop {
            if *shutdown.borrow() {
                return;
            }
            let res = self.sync_once().await;
            let pause = match &res {
                Ok(_) => {
                    backoff = Duration::from_secs(2);
                    if self.inner.has_feed.load(Ordering::SeqCst) {
                        // Long poll until something happens
                        match self.long_poll().await {
                            Ok(()) => Duration::ZERO,
                            Err(_) => backoff,
                        }
                    } else {
                        POLL_INTERVAL
                    }
                }
                Err(Error::Unauthenticated) | Err(Error::PinMismatch) => Duration::from_secs(3600),
                Err(_) => {
                    let b = backoff;
                    backoff = (backoff * 2).min(Duration::from_secs(120));
                    b
                }
            };
            if pause.is_zero() {
                continue;
            }
            let inner = self.inner.clone();
            let notified = inner.wake.notified();
            tokio::pin!(notified);
            tokio::select! {
                _ = tokio::time::sleep(pause) => {}
                _ = &mut notified => {}
                _ = shutdown.changed() => {}
            }
        }
    }

    /// One long-poll pass (applying the changes).
    fn long_poll(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(self.long_poll_inner())
    }

    async fn long_poll_inner(&self) -> Result<()> {
        let _g = self.inner.run_lock.lock().await;
        let cursor = self.inner.replica.remote().await?.and_then(|r| r.cursor);
        let Some(cursor) = cursor else { return Ok(()) };
        let mut rep = SyncReport::default();
        // pending (throttled) resolve → do not wait longer than necessary
        let wait = match self.resolve_pending_in() {
            Some(d) => d.as_secs().clamp(1, LONG_POLL_SECS),
            None => LONG_POLL_SECS,
        };
        // wake() aborts the wait (aborting in the middle of a page is harmless: the cursor
        // is only set after applying, applying is idempotent)
        let inner = self.inner.clone();
        let notified = inner.wake.notified();
        tokio::pin!(notified);
        let res = tokio::select! {
            r = self.follow_feed(Some(cursor), wait, &mut rep) => r,
            _ = &mut notified => Ok(()),
        };
        if let Err(e) = &res {
            self.after_run(Some(Outcome::of(e))).await;
        }
        res
    }
}

/// Keychain value `token + newline + device_uuid` → (token, device UUID).
fn split_secret(s: Option<Zeroizing<String>>) -> (Option<Zeroizing<String>>, Option<String>) {
    let Some(s) = s else { return (None, None) };
    match s.split_once('\n') {
        Some((t, d)) => (Some(Zeroizing::new(t.trim().to_string())), Some(d.trim().to_string()).filter(|d| !d.is_empty())),
        None => (Some(Zeroizing::new(s.trim().to_string())), None),
    }
}

/// Blocks as separate vectors (no borrowed slices across `await`, rust#102211).
fn owned_chunks(v: &[String]) -> Vec<Vec<String>> {
    v.chunks(BATCH).map(<[String]>::to_vec).collect()
}

fn sendable<'a, T>(f: impl std::future::Future<Output = T> + Send + 'a) -> BoxFuture<'a, T> {
    Box::pin(f)
}

type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// Result of a run for the status (own values, no references across `await`).
struct Outcome {
    state: &'static str,
    online: bool,
    unauthenticated: bool,
    message: String,
}

impl Outcome {
    fn of(e: &Error) -> Outcome {
        Outcome {
            state: match e {
                Error::Offline(_) => "offline",
                Error::Unauthenticated => "auth_required",
                Error::LimitExceeded(_) => "limit_exceeded",
                Error::PinMismatch => "pin_mismatch",
                _ => "error",
            },
            online: !matches!(e, Error::Offline(_) | Error::PinMismatch | Error::Tls(_)),
            unauthenticated: matches!(e, Error::Unauthenticated),
            message: e.to_string(),
        }
    }
}

fn outcome(res: &Result<SyncReport>) -> Option<Outcome> {
    res.as_ref().err().map(Outcome::of)
}

/// Cache limit = selection limit + room for thumbnails/opened files.
pub fn cache_limit(l: Limits) -> u64 {
    l.max_bytes.saturating_add(256 * 1024 * 1024)
}
