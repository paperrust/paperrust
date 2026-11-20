//! The guard: watches `MEDIA_ROOT/documents/` and evaluates every change against the catalogue
//! (database). PaperRust itself changes files only in such a way that file and database afterwards
//! match (new version, renaming via storage path, deleting from the trash). What
//! does **not** match the database after a waiting time ([`GuardConfig::grace_secs`]) is a finding.
//!
//! Sources of changes:
//! 1. file system events (`notify`: inotify, ReadDirectoryChangesW, kqueue), where available;
//! 2. regular review (size + modification time, without content) against the last state;
//! 3. rotating sampling: fully hash a few documents per round.
//!
//! If the findings reach the threshold within the time window, [`Reporter::trigger`] reports the attack
//! (read-only mode, alarm). Afterwards further findings are assigned to the open event.

use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::BoxFuture;
use sha2::Digest;
use tokio::sync::{mpsc, watch};

use crate::config::GuardConfig;
use crate::detect::{self, Area, ContentVerdict, NameVerdict};

/// Reason for a finding (stored as `reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reason {
    /// File belongs to a document, content does not match the stored checksum.
    Modified,
    /// Content looks encrypted (header missing, high entropy).
    Encrypted,
    /// Name of a ransom note.
    RansomNote,
    /// Extension of known ransomware programs (`.locked`, `.encrypted`, …).
    SuspiciousExtension,
    /// File of a document is missing.
    Deleted,
    /// Extension that PaperRust never creates in this folder.
    UnknownExtension,
    /// Header does not match the extension (without high entropy).
    BadMagic,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Modified => "modified",
            Reason::Encrypted => "encrypted",
            Reason::RansomNote => "ransom_note",
            Reason::SuspiciousExtension => "suspicious_extension",
            Reason::Deleted => "deleted",
            Reason::UnknownExtension => "unknown_extension",
            Reason::BadMagic => "bad_magic",
        }
    }
}

/// A finding (one file).
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    /// path relative to `MEDIA_ROOT` with `/`.
    pub path: String,
    pub reason: Reason,
    pub detail: String,
}

/// What the database expects for a file.
#[derive(Debug, Clone, PartialEq)]
pub struct Expected {
    pub doc_id: i64,
    /// SHA-256 (hex) for originals/archives, `None` for thumbnails.
    pub checksum: Option<String>,
}

/// A document for the sampling.
#[derive(Debug, Clone)]
pub struct SampleDoc {
    pub id: i64,
    /// (path relative to `MEDIA_ROOT`, checksum)
    pub files: Vec<(String, String)>,
}

/// Catalogue = view of the database (tests: in memory).
pub trait Catalog: Send + Sync + 'static {
    /// Expectation for a file (`rel_in_area` relative to the area).
    fn lookup<'a>(&'a self, area: Area, rel_in_area: &'a str) -> BoxFuture<'a, Result<Option<Expected>, String>>;
    /// Documents with `id > after` (ascending, at most `n`).
    fn sample(&self, after: i64, n: i64) -> BoxFuture<'_, Result<Vec<SampleDoc>, String>>;
}

/// Receiver of the findings (server: database + alarm; tests: in memory).
pub trait Reporter: Send + Sync + 'static {
    fn finding<'a>(&'a self, f: &'a Finding) -> BoxFuture<'a, Result<(), String>>;
    /// Threshold reached: create event, read-only mode, alarm. `files` = number of findings in the window.
    fn trigger(&self, files: usize, window_minutes: i64) -> BoxFuture<'_, Result<(), String>>;
    /// Has an attack already been reported (read-only mode active)?
    fn is_triggered(&self) -> bool;
}

/// State of the last review: hash(path) → hash(size, modification time). ~20 bytes per file.
#[derive(Default)]
pub struct Snapshot {
    map: HashMap<u64, u64>,
}

fn h64(v: impl Hash) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

/// Result of a review.
#[derive(Debug, Default)]
pub struct ScanResult {
    pub files: usize,
    /// new or changed files (relative to `MEDIA_ROOT`)
    pub changed: Vec<String>,
    /// number of vanished files (names are not stored, see contract)
    pub removed: usize,
}

/// Path relative to `root` with `/`.
pub fn rel_path(root: &Path, p: &Path) -> Option<String> {
    let r = p.strip_prefix(root).ok()?;
    let mut s = String::new();
    for c in r.components() {
        if !s.is_empty() {
            s.push('/');
        }
        s.push_str(&c.as_os_str().to_string_lossy());
    }
    (!s.is_empty()).then_some(s)
}

fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, u64, i128)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        if ft.is_dir() {
            walk(&p, root, out);
        } else if ft.is_file() {
            let Ok(m) = e.metadata() else { continue };
            let mt = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_nanos() as i128)
                .unwrap_or(0);
            if let Some(rel) = rel_path(root, &p) {
                out.push((rel, m.len(), mt));
            }
        }
    }
}

impl Snapshot {
    /// Review `MEDIA_ROOT/documents` and compare with the last state. The first time
    /// (`baseline`) there are no changes.
    pub fn scan(&mut self, media_root: &Path, baseline: bool) -> ScanResult {
        let mut files = Vec::new();
        walk(&media_root.join("documents"), media_root, &mut files);
        let mut next = HashMap::with_capacity(files.len());
        let mut res = ScanResult { files: files.len(), ..Default::default() };
        let mut matched = 0usize;
        for (rel, size, mt) in files {
            let k = h64(&rel);
            let fp = h64((size, mt));
            match self.map.get(&k) {
                Some(old) => {
                    matched += 1;
                    if *old != fp && !baseline {
                        res.changed.push(rel);
                    }
                }
                None if !baseline => res.changed.push(rel),
                None => {}
            }
            next.insert(k, fp);
        }
        res.removed = self.map.len().saturating_sub(matched);
        self.map = next;
        res
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// SHA-256 (hex) of a file.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn verdict_finding(path: &str, v: &ContentVerdict) -> Option<Finding> {
    match v {
        ContentVerdict::Ok => None,
        ContentVerdict::Encrypted { entropy } => Some(Finding { path: path.into(), reason: Reason::Encrypted, detail: format!("content looks encrypted (entropy {entropy:.2} bit/byte)") }),
        ContentVerdict::BadMagic { expected, entropy } => {
            Some(Finding { path: path.into(), reason: Reason::BadMagic, detail: format!("header is not {expected} (entropy {entropy:.2} bit/byte)") })
        }
    }
}

/// Evaluate a file (without waiting time). At most one finding per file.
pub async fn evaluate(media_root: &Path, cfg: &GuardConfig, catalog: &dyn Catalog, rel: &str) -> Result<Option<Finding>, String> {
    if detect::is_system_file(rel) {
        return Ok(None);
    }
    let (area, inner) = detect::area(rel);
    if area == Area::Other {
        return Ok(None);
    }
    let abs = media_root.join(rel);
    let meta = std::fs::symlink_metadata(&abs).ok();
    if meta.as_ref().is_some_and(|m| m.is_dir()) {
        return Ok(None);
    }
    let exists = meta.as_ref().is_some_and(|m| m.is_file());
    let name_finding = if exists {
        match detect::check_name(area, inner, &cfg.extra_extensions) {
            NameVerdict::Ok => None,
            NameVerdict::RansomNote => Some(Finding { path: rel.into(), reason: Reason::RansomNote, detail: "file name of a ransom note".into() }),
            NameVerdict::SuspiciousExtension(e) => Some(Finding { path: rel.into(), reason: Reason::SuspiciousExtension, detail: format!("extension .{e}") }),
            NameVerdict::UnknownExtension(e) => Some(Finding {
                path: rel.into(),
                reason: Reason::UnknownExtension,
                detail: if e.is_empty() { "file without extension".into() } else { format!("extension .{e} is never written here") },
            }),
        }
    } else {
        None
    };
    let expected = catalog.lookup(area, inner).await?;
    let content = |want_hash: bool| {
        let abs = abs.clone();
        let inner = inner.to_string();
        async move {
            tokio::task::spawn_blocking(move || -> std::io::Result<(ContentVerdict, Option<String>)> {
                let head = detect::read_head(&abs)?;
                let v = detect::check_content(&inner, &head);
                let sha = if want_hash { Some(sha256_file(&abs)?) } else { None };
                Ok((v, sha))
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
        }
    };
    let found = match (expected, exists) {
        (Some(exp), true) => match &exp.checksum {
            Some(want) => match content(true).await {
                Ok((v, Some(sha))) if !sha.eq_ignore_ascii_case(want) => {
                    let extra = match v {
                        ContentVerdict::Encrypted { entropy } => format!("; content looks encrypted (entropy {entropy:.2} bit/byte)"),
                        ContentVerdict::BadMagic { expected, .. } => format!("; header is not {expected}"),
                        ContentVerdict::Ok => String::new(),
                    };
                    let reason = if matches!(v, ContentVerdict::Encrypted { .. }) { Reason::Encrypted } else { Reason::Modified };
                    Some(Finding { path: rel.into(), reason, detail: format!("document {}: checksum differs from the database{extra}", exp.doc_id) })
                }
                Ok(_) => None,
                // just renamed/deleted: evaluate again on the next event
                Err(_) => None,
            },
            None => match content(false).await {
                Ok((v, _)) => verdict_finding(rel, &v),
                Err(_) => None,
            },
        },
        (Some(exp), false) => Some(Finding { path: rel.into(), reason: Reason::Deleted, detail: format!("file of document {} is missing", exp.doc_id) }),
        (None, true) => {
            let c = content(false).await.ok().and_then(|(v, _)| verdict_finding(rel, &v));
            match (name_finding, c) {
                (Some(n), Some(c)) if n.reason == Reason::UnknownExtension && c.reason == Reason::Encrypted => Some(c),
                (Some(n), _) => Some(n),
                (None, c) => c,
            }
        }
        (None, false) => None,
    };
    Ok(found)
}

/// Guard with state (waiting list, time window, review, sampling).
pub struct Engine {
    pub media_root: PathBuf,
    pub cfg: GuardConfig,
    catalog: Arc<dyn Catalog>,
    reporter: Arc<dyn Reporter>,
    pending: HashMap<String, Instant>,
    window: VecDeque<Instant>,
    /// last reported (path, reason) → time; do not report the same file continuously
    reported: HashMap<(String, Reason), Instant>,
    snapshot: Snapshot,
    scanned_once: bool,
    sample_cursor: i64,
}

/// Statistics of a processing step.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub evaluated: usize,
    pub findings: usize,
    pub triggered: bool,
}

impl Engine {
    pub fn new(media_root: PathBuf, cfg: GuardConfig, catalog: Arc<dyn Catalog>, reporter: Arc<dyn Reporter>) -> Self {
        Self {
            media_root,
            cfg,
            catalog,
            reporter,
            pending: HashMap::new(),
            window: VecDeque::new(),
            reported: HashMap::new(),
            snapshot: Snapshot::default(),
            scanned_once: false,
            sample_cursor: 0,
        }
    }

    /// Note a change; it is evaluated after the waiting time (a repeated change postpones it).
    pub fn note_change(&mut self, rel: String, now: Instant) {
        if detect::is_system_file(&rel) || detect::area(&rel).0 == Area::Other {
            return;
        }
        self.pending.insert(rel, now + Duration::from_secs(self.cfg.grace_secs));
    }

    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Evaluate due changes.
    pub async fn process_due(&mut self, now: Instant) -> Step {
        let due: Vec<String> = self.pending.iter().filter(|(_, t)| **t <= now).map(|(p, _)| p.clone()).collect();
        let mut step = Step::default();
        for rel in due {
            self.pending.remove(&rel);
            step.evaluated += 1;
            match evaluate(&self.media_root, &self.cfg, self.catalog.as_ref(), &rel).await {
                Ok(Some(f)) => {
                    let s = self.record(f, now).await;
                    step.findings += s.findings;
                    step.triggered |= s.triggered;
                }
                Ok(None) => {}
                Err(e) => tracing::warn!(target: "paperless.guard", path = %rel, error = %e, "could not evaluate change"),
            }
        }
        step
    }

    /// Report a finding, maintain the time window, trigger the alarm if needed.
    pub async fn record(&mut self, f: Finding, now: Instant) -> Step {
        let key = (f.path.clone(), f.reason);
        if self.reported.get(&key).is_some_and(|t| now.duration_since(*t) < Duration::from_secs(24 * 3600)) {
            return Step::default();
        }
        if self.reported.len() > 200_000 {
            self.reported.retain(|_, t| now.duration_since(*t) < Duration::from_secs(3600));
        }
        self.reported.insert(key, now);
        tracing::warn!(target: "paperless.guard", path = %f.path, reason = f.reason.as_str(), detail = %f.detail, "suspicious change in media folder");
        if let Err(e) = self.reporter.finding(&f).await {
            tracing::error!(target: "paperless.guard", error = %e, "could not store guard finding");
        }
        let window = Duration::from_secs(self.cfg.window_minutes.max(1) as u64 * 60);
        self.window.push_back(now);
        while self.window.front().is_some_and(|t| now.duration_since(*t) > window) {
            self.window.pop_front();
        }
        let mut step = Step { evaluated: 0, findings: 1, triggered: false };
        if !self.reporter.is_triggered() && self.window.len() as i64 >= self.cfg.threshold {
            match self.reporter.trigger(self.window.len(), self.cfg.window_minutes).await {
                Ok(()) => step.triggered = true,
                Err(e) => tracing::error!(target: "paperless.guard", error = %e, "could not switch to read-only mode"),
            }
        }
        step
    }

    /// Review (size/modification time); changes go onto the waiting list.
    pub async fn scan(&mut self, now: Instant) -> ScanResult {
        let root = self.media_root.clone();
        let baseline = !self.scanned_once;
        let mut snap = std::mem::take(&mut self.snapshot);
        let (snap, res) = match tokio::task::spawn_blocking(move || {
            let r = snap.scan(&root, baseline);
            (snap, r)
        })
        .await
        {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(target: "paperless.guard", error = %e, "media scan failed");
                return ScanResult::default();
            }
        };
        self.snapshot = snap;
        self.scanned_once = true;
        for rel in &res.changed {
            self.note_change(rel.clone(), now);
        }
        if res.removed > 0 {
            tracing::info!(target: "paperless.guard", removed = res.removed, "files removed since the last media scan");
        }
        res
    }

    /// Sampling: fully hash `sample_files` documents (rotating over all IDs).
    pub async fn sample(&mut self, now: Instant) -> Step {
        let mut step = Step::default();
        if self.cfg.sample_files <= 0 {
            return step;
        }
        let docs = match self.catalog.sample(self.sample_cursor, self.cfg.sample_files).await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(target: "paperless.guard", error = %e, "guard sample query failed");
                return step;
            }
        };
        if docs.is_empty() {
            self.sample_cursor = 0;
            return step;
        }
        for d in docs {
            self.sample_cursor = self.sample_cursor.max(d.id);
            for (rel, want) in d.files {
                let abs = self.media_root.join(&rel);
                let sha = tokio::task::spawn_blocking(move || sha256_file(&abs)).await;
                let bad = match sha {
                    Ok(Ok(s)) => !s.eq_ignore_ascii_case(&want),
                    Ok(Err(e)) => e.kind() == std::io::ErrorKind::NotFound,
                    Err(_) => false,
                };
                if bad {
                    step.evaluated += 1;
                    // check again via the database (document might just be being reprocessed)
                    if let Ok(Some(f)) = evaluate(&self.media_root, &self.cfg, self.catalog.as_ref(), &rel).await {
                        let s = self.record(f, now).await;
                        step.findings += s.findings;
                        step.triggered |= s.triggered;
                    }
                }
            }
        }
        step
    }

    /// Main loop until the stop signal.
    pub async fn run(mut self, mut stop: watch::Receiver<bool>) {
        let (tx, mut rx) = mpsc::channel::<PathBuf>(65536);
        let overflow = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let _watcher = if self.cfg.watch { start_watcher(&self.media_root, tx, overflow.clone()) } else { None };
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let scan_every = (self.cfg.scan_interval_secs > 0).then(|| Duration::from_secs(self.cfg.scan_interval_secs));
        // first review (initial state) shortly after the start
        let mut next_scan = Instant::now() + Duration::from_secs(5);
        tracing::info!(
            target: "paperless.guard",
            media = %self.media_root.display(),
            watch = _watcher.is_some(),
            scan_interval_secs = self.cfg.scan_interval_secs,
            threshold = self.cfg.threshold,
            window_minutes = self.cfg.window_minutes,
            "ransomware guard started"
        );
        loop {
            tokio::select! {
                _ = stop.changed() => if *stop.borrow() { break },
                Some(p) = rx.recv() => {
                    if let Some(rel) = rel_path(&self.media_root, &p) {
                        self.note_change(rel, Instant::now());
                    }
                }
                _ = tick.tick() => {
                    let now = Instant::now();
                    let forced = overflow.swap(false, std::sync::atomic::Ordering::Relaxed);
                    if forced {
                        tracing::warn!(target: "paperless.guard", "too many file events, scanning the media folder");
                    }
                    // first review = initial state, even without regular review (for the sampling)
                    let due = now >= next_scan && (scan_every.is_some() || !self.scanned_once);
                    if forced || due {
                        self.scan(now).await;
                        self.sample(now).await;
                        next_scan = now + scan_every.unwrap_or(Duration::from_secs(900));
                    }
                    self.process_due(now).await;
                }
            }
        }
        tracing::info!(target: "paperless.guard", "ransomware guard stopped");
    }
}

/// File system events; `None` if unavailable (then only review + sampling).
fn start_watcher(media_root: &Path, tx: mpsc::Sender<PathBuf>, overflow: Arc<std::sync::atomic::AtomicBool>) -> Option<notify::RecommendedWatcher> {
    use notify::{RecursiveMode, Watcher};
    let dir = media_root.join("documents");
    let dir = if dir.is_dir() { dir } else { media_root.to_path_buf() };
    let handler = move |res: notify::Result<notify::Event>| match res {
        Ok(ev) => {
            if matches!(ev.kind, notify::EventKind::Access(_)) {
                return;
            }
            if ev.need_rescan() {
                overflow.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            for p in ev.paths {
                if tx.try_send(p).is_err() {
                    overflow.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }
        Err(_) => overflow.store(true, std::sync::atomic::Ordering::Relaxed),
    };
    let mut w = match notify::recommended_watcher(handler) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(target: "paperless.guard", error = %e, "file system events unavailable; relying on periodic scans");
            return None;
        }
    };
    if let Err(e) = w.watch(&dir, RecursiveMode::Recursive) {
        tracing::warn!(target: "paperless.guard", dir = %dir.display(), error = %e, "cannot watch media folder; relying on periodic scans");
        return None;
    }
    Some(w)
}

// ------------------------------------------------------------------- Test implementations

/// Catalogue in memory (tests, simulation): path in the area → expectation.
#[derive(Default)]
pub struct MemoryCatalog {
    pub originals: std::sync::Mutex<HashMap<String, Expected>>,
    pub archive: std::sync::Mutex<HashMap<String, Expected>>,
    pub thumbnails: std::sync::Mutex<HashMap<String, Expected>>,
}

impl Catalog for MemoryCatalog {
    fn lookup<'a>(&'a self, area: Area, rel: &'a str) -> BoxFuture<'a, Result<Option<Expected>, String>> {
        let m = match area {
            Area::Originals => &self.originals,
            Area::Archive => &self.archive,
            Area::Thumbnails => &self.thumbnails,
            Area::Other => return Box::pin(async { Ok(None) }),
        };
        let v = m.lock().map_err(|e| e.to_string()).map(|g| g.get(rel).cloned());
        Box::pin(async move { v })
    }
    fn sample(&self, after: i64, n: i64) -> BoxFuture<'_, Result<Vec<SampleDoc>, String>> {
        let mut by_id: std::collections::BTreeMap<i64, Vec<(String, String)>> = Default::default();
        for (prefix, m) in [("documents/originals/", &self.originals), ("documents/archive/", &self.archive)] {
            if let Ok(g) = m.lock() {
                for (rel, e) in g.iter() {
                    if let Some(c) = &e.checksum {
                        by_id.entry(e.doc_id).or_default().push((format!("{prefix}{rel}"), c.clone()));
                    }
                }
            }
        }
        let v: Vec<SampleDoc> = by_id.into_iter().filter(|(id, _)| *id > after).take(n.max(0) as usize).map(|(id, files)| SampleDoc { id, files }).collect();
        Box::pin(async move { Ok(v) })
    }
}

/// Receiver in memory (tests, simulation).
#[derive(Default)]
pub struct MemoryReporter {
    pub findings: std::sync::Mutex<Vec<Finding>>,
    pub triggered: std::sync::atomic::AtomicBool,
    pub trigger_count: std::sync::atomic::AtomicUsize,
}

impl Reporter for MemoryReporter {
    fn finding<'a>(&'a self, f: &'a Finding) -> BoxFuture<'a, Result<(), String>> {
        if let Ok(mut g) = self.findings.lock() {
            g.push(f.clone());
        }
        Box::pin(async { Ok(()) })
    }
    fn trigger(&self, _files: usize, _window: i64) -> BoxFuture<'_, Result<(), String>> {
        self.triggered.store(true, std::sync::atomic::Ordering::SeqCst);
        self.trigger_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }
    fn is_triggered(&self) -> bool {
        self.triggered.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_diff() {
        let t = tempfile::tempdir().unwrap();
        let o = t.path().join("documents/originals");
        std::fs::create_dir_all(&o).unwrap();
        std::fs::write(o.join("a.pdf"), b"%PDF-1").unwrap();
        std::fs::write(o.join("b.pdf"), b"%PDF-2").unwrap();
        let mut s = Snapshot::default();
        let r = s.scan(t.path(), true);
        assert_eq!((r.files, r.changed.len(), r.removed), (2, 0, 0));
        std::fs::write(o.join("a.pdf"), b"%PDF-1 changed").unwrap();
        std::fs::remove_file(o.join("b.pdf")).unwrap();
        std::fs::write(o.join("c.pdf.locked"), b"x").unwrap();
        let mut r = s.scan(t.path(), false);
        r.changed.sort();
        assert_eq!(r.changed, vec!["documents/originals/a.pdf", "documents/originals/c.pdf.locked"]);
        assert_eq!(r.removed, 1);
    }

    #[test]
    fn rel_paths() {
        let root = Path::new("/m");
        assert_eq!(rel_path(root, Path::new("/m/documents/originals/x.pdf")).as_deref(), Some("documents/originals/x.pdf"));
        assert_eq!(rel_path(root, Path::new("/other/x")), None);
    }
}
