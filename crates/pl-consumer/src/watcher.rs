// Watch the input folder (replacement for `manage.py document_consumer`).
//
// upstream: src/documents/management/commands/document_consumer.py@v3.4.2 (TrackedFile,
//           FileStabilityTracker, ConsumerFilter, _tags_from_path, _consume_file, Command.handle,
//           _process_existing_files, _rescan_existing_files, _watch_directory),
//           watchfiles 1.x filters.py (BaseFilter.__call__)
//
// Watching like upstream (watchfiles = Rust `notify`): `CONSUMER_POLLING_INTERVAL` > 0 ->
// directory snapshots at that interval (watchfiles `force_polling`); 0 -> native events
// (inotify/ReadDirectoryChangesW/kqueue via `notify`), falling back to 1 s polling if they are not
// available (e.g. inotify limit). Service switch `PAPERLESS_CONSUMER_DISABLE` like
// svc-consumer/run.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use futures::future::BoxFuture;

use crate::data::{ConsumableDocument, DocumentMetadataOverrides, DocumentSource, absolute};

/// `ConsumerFilter.DEFAULT_IGNORE_PATTERNS`.
pub const DEFAULT_IGNORE_PATTERNS: &[&str] = &[r"^\.DS_Store$", r"^\.DS_STORE$", r"^\._.*", r"^desktop\.ini$", r"^Thumbs\.db$"];
/// `ConsumerFilter.DEFAULT_IGNORE_DIRS`.
pub const DEFAULT_IGNORE_DIRS: &[&str] = &[".stfolder", ".stversions", ".localized", "@eaDir", ".Spotlight-V100", ".Trashes", "__MACOSX"];

/// `watchfiles.Change`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Added,
    Modified,
    Deleted,
}

/// `ConsumerFilter`.
#[derive(Debug, Clone)]
pub struct ConsumerFilter {
    pub supported_extensions: BTreeSet<String>,
    pub ignore_patterns: Vec<String>,
    pub ignore_dirs: HashSet<String>,
}

impl ConsumerFilter {
    pub fn new(supported_extensions: BTreeSet<String>, ignore_patterns: &[String], ignore_dirs: &[String]) -> Self {
        let mut pats: Vec<String> = DEFAULT_IGNORE_PATTERNS.iter().map(|s| s.to_string()).collect();
        pats.extend(ignore_patterns.iter().cloned());
        let mut dirs: HashSet<String> = DEFAULT_IGNORE_DIRS.iter().map(|s| s.to_string()).collect();
        dirs.extend(ignore_dirs.iter().cloned());
        Self { supported_extensions, ignore_patterns: pats, ignore_dirs: dirs }
    }

    /// With the extensions of the default registry (`get_supported_file_extensions()`).
    pub fn with_registry(ignore_patterns: &[String], ignore_dirs: &[String]) -> Self {
        let reg = pl_parsers::ParserRegistry::new(pl_parsers::RegistryConfig::default());
        Self::new(reg.get_supported_file_extensions(), ignore_patterns, ignore_dirs)
    }

    /// `ConsumerFilter.__call__(change, path)`.
    pub fn accepts(&self, path: &Path) -> bool {
        let s = path.to_string_lossy();
        let parts: Vec<&str> = s.trim_start_matches(std::path::MAIN_SEPARATOR).split(std::path::MAIN_SEPARATOR).collect();
        if parts.iter().any(|p| self.ignore_dirs.contains(*p)) {
            return false;
        }
        let entity = parts.last().copied().unwrap_or("");
        if self.ignore_patterns.iter().any(|p| pl_matching::regex_compat::safe_regex_search(p, entity, false).is_some()) {
            return false;
        }
        if path.is_dir() {
            return true;
        }
        let suffix = path.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default();
        self.supported_extensions.contains(&suffix)
    }
}

#[derive(Debug, Clone)]
struct TrackedFile {
    last_event_time: Instant,
    last_mtime: Option<SystemTime>,
    last_size: Option<u64>,
}

fn stat(p: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

impl TrackedFile {
    fn update_stats(&mut self, p: &Path) -> bool {
        match stat(p) {
            Some((t, s)) => {
                self.last_mtime = Some(t);
                self.last_size = Some(s);
                true
            }
            None => false,
        }
    }
    fn is_unchanged(&self, p: &Path) -> bool {
        stat(p).is_some_and(|(t, s)| Some(t) == self.last_mtime && Some(s) == self.last_size)
    }
}

/// `FileStabilityTracker`.
#[derive(Debug, Default)]
pub struct FileStabilityTracker {
    pub stability_delay: Duration,
    tracked: HashMap<PathBuf, TrackedFile>,
}

impl FileStabilityTracker {
    pub fn new(stability_delay: Duration) -> Self {
        Self { stability_delay, tracked: HashMap::new() }
    }

    pub fn track(&mut self, path: &Path, change: Change) {
        let path = absolute(path);
        match change {
            Change::Deleted => {
                self.tracked.remove(&path);
                tracing::debug!(target: "paperless.management.consumer", "Stopped tracking deleted file: {}", path.display());
            }
            Change::Added | Change::Modified => {
                let now = Instant::now();
                if let Some(t) = self.tracked.get_mut(&path) {
                    t.last_event_time = now;
                    t.update_stats(&path);
                } else {
                    let mut t = TrackedFile { last_event_time: now, last_mtime: None, last_size: None };
                    if t.update_stats(&path) {
                        self.tracked.insert(path.clone(), t);
                        tracing::debug!(target: "paperless.management.consumer", "Started tracking: {}", path.display());
                    }
                }
            }
        }
    }

    /// `get_stable_files()`.
    pub fn take_stable_files(&mut self) -> Vec<PathBuf> {
        let now = Instant::now();
        let mut remove = Vec::new();
        let mut stable = Vec::new();
        for (p, t) in self.tracked.iter_mut() {
            if now.duration_since(t.last_event_time) < self.stability_delay {
                continue;
            }
            if !t.is_unchanged(p) {
                if t.update_stats(p) {
                    t.last_event_time = now;
                    tracing::debug!(target: "paperless.management.consumer", "File changed during stability check: {}", p.display());
                } else {
                    remove.push(p.clone());
                }
                continue;
            }
            if t.last_size.unwrap_or(0) == 0 {
                remove.push(p.clone());
                tracing::debug!(target: "paperless.management.consumer", "Ignoring stable but empty file: {}", p.display());
                continue;
            }
            tracing::info!(target: "paperless.management.consumer", "File is stable: {}", p.display());
            stable.push(p.clone());
        }
        for p in remove.iter().chain(stable.iter()) {
            self.tracked.remove(p);
        }
        stable.sort();
        stable
    }

    pub fn is_tracking(&self, path: &Path) -> bool {
        self.tracked.contains_key(&absolute(path))
    }
    pub fn has_pending_files(&self) -> bool {
        !self.tracked.is_empty()
    }
}

/// What happens to a stable file (`consume_file.apply_async` or ingest directly).
pub trait ConsumeSink: Send + Sync {
    /// `true` if handed over (otherwise the file is retried on the next pass).
    fn submit<'a>(&'a self, doc: ConsumableDocument, overrides: DocumentMetadataOverrides) -> BoxFuture<'a, bool>;
}

/// Via the queue (`consume_file.apply_async`).
pub struct QueueSink(pub Arc<dyn pl_queue::TaskQueue>);
impl ConsumeSink for QueueSink {
    fn submit<'a>(&'a self, doc: ConsumableDocument, overrides: DocumentMetadataOverrides) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            match crate::task::enqueue_consume(self.0.as_ref(), &doc, &overrides).await {
                Ok(_) => true,
                Err(e) => {
                    tracing::error!(target: "paperless.management.consumer", "Error while queuing document {}: {e}", doc.original_file.display());
                    false
                }
            }
        })
    }
}

/// Ingest directly in the process (without a queue, e.g. desktop/tests).
pub struct DirectSink(pub Arc<crate::Consumer>);
impl ConsumeSink for DirectSink {
    fn submit<'a>(&'a self, doc: ConsumableDocument, overrides: DocumentMetadataOverrides) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let name = doc.original_file.display().to_string();
            if let Err(e) = self.0.consume_file(doc, Some(overrides), None).await {
                tracing::error!(target: "paperless.management.consumer", "Consuming {name} failed: {e}");
            }
            true
        })
    }
}

/// Settings of the watcher.
#[derive(Debug, Clone)]
pub struct WatchOptions {
    pub directory: PathBuf,
    pub recursive: bool,
    pub subdirs_as_tags: bool,
    /// Seconds; 0 = upstream "native events" (here: polling at 1 s)
    pub polling_interval: f64,
    pub stability_delay: f64,
    /// `rescan_interval_s` (300)
    pub rescan_interval: Duration,
}

/// The watcher.
pub struct FolderWatcher {
    pub opts: WatchOptions,
    pub filter: ConsumerFilter,
    pub pool: sqlx::PgPool,
    pub sink: Arc<dyn ConsumeSink>,
}

impl FolderWatcher {
    /// `Command.handle` checks: directory exists and is a directory.
    pub fn check_directory(dir: &Path) -> Result<PathBuf, String> {
        let d = absolute(dir);
        if !d.exists() {
            return Err(format!("Consumption directory does not exist: {}", d.display()));
        }
        if !d.is_dir() {
            return Err(format!("Consumption path is not a directory: {}", d.display()));
        }
        Ok(d)
    }

    fn list_files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![self.opts.directory.clone()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    if self.opts.recursive {
                        stack.push(p);
                    }
                } else if p.is_file() {
                    out.push(p);
                }
            }
        }
        out.sort();
        out
    }

    /// `_tags_from_path`: one tag per directory part below the input folder (get_or_create).
    pub async fn tags_from_path(&self, filepath: &Path) -> Result<Vec<i64>, String> {
        let rel = filepath.strip_prefix(&self.opts.directory).map_err(|e| e.to_string())?;
        let mut conn = self.pool.acquire().await.map_err(|e| e.to_string())?;
        let mut ids = BTreeSet::new();
        if let Some(parent) = rel.parent() {
            for part in parent.components() {
                let name = part.as_os_str().to_string_lossy();
                if name.is_empty() {
                    continue;
                }
                ids.insert(crate::db::get_or_create_tag(&mut conn, &name).await.map_err(|e| e.to_string())?);
            }
        }
        Ok(ids.into_iter().collect())
    }

    /// `_consume_file`.
    pub async fn consume_path(&self, filepath: &Path) -> bool {
        if !filepath.is_file() {
            tracing::debug!(target: "paperless.management.consumer", "Not consuming {}: not a file or doesn't exist", filepath.display());
            return false;
        }
        let mut tag_ids = None;
        if self.opts.subdirs_as_tags {
            match self.tags_from_path(filepath).await {
                Ok(t) => tag_ids = Some(t),
                Err(e) => {
                    tracing::error!(target: "paperless.management.consumer", "Error creating tags from path for {}: {e}", filepath.display())
                }
            }
        }
        tracing::info!(target: "paperless.management.consumer", "Adding {} to the task queue", filepath.display());
        let doc = match ConsumableDocument::new(DocumentSource::ConsumeFolder, filepath) {
            Ok(d) => d,
            Err(e) => {
                tracing::error!(target: "paperless.management.consumer", "Error while queuing document {}: {e}", filepath.display());
                return false;
            }
        };
        self.sink.submit(doc, DocumentMetadataOverrides { tag_ids, ..Default::default() }).await
    }

    /// `_process_existing_files` → set of the handed-over paths.
    pub async fn process_existing(&self) -> HashSet<PathBuf> {
        tracing::info!(target: "paperless.management.consumer", "Processing existing files in {}", self.opts.directory.display());
        let mut queued = HashSet::new();
        for p in self.list_files() {
            if !self.filter.accepts(&p) {
                continue;
            }
            if self.consume_path(&p).await {
                queued.insert(absolute(&p));
            }
        }
        queued
    }

    /// `--oneshot` + watch loop until `stop` becomes true – as a service in the worker: with
    /// `PAPERLESS_CONSUMER_DISABLE` (non-empty) the watching does not start (upstream
    /// docker/rootfs/etc/s6-overlay/s6-rc.d/svc-consumer/run: `[[ -n "${PAPERLESS_CONSUMER_DISABLE}" ]]`).
    pub async fn run(&self, oneshot: bool, stop: tokio::sync::watch::Receiver<bool>) {
        if consumer_disabled(&|k| pl_core::config::var(k)) {
            tracing::info!(target: "paperless.management.consumer", "Consumer is disabled, exiting");
            return;
        }
        self.run_command(oneshot, stop).await;
    }

    /// `Command.handle` without the service check `CONSUMER_DISABLE` (command `document_consumer`).
    pub async fn run_command(&self, oneshot: bool, stop: tokio::sync::watch::Receiver<bool>) {
        let queued = self.process_existing().await;
        if oneshot {
            tracing::info!(target: "paperless.management.consumer", "Oneshot mode: processed existing files, exiting");
            return;
        }
        if self.opts.polling_interval <= 0.0 {
            match NativeEvents::start(&self.opts.directory, self.opts.recursive) {
                Ok(ev) => {
                    tracing::info!(
                        target: "paperless.management.consumer",
                        "Watching {} using native file system events",
                        self.opts.directory.display()
                    );
                    self.watch_loop(Some(ev), 1.0, queued, stop).await;
                    tracing::debug!(target: "paperless.management.consumer", "Consumer exiting");
                    return;
                }
                Err(e) => tracing::warn!(
                    target: "paperless.management.consumer",
                    "Native file system events unavailable for {} ({e}); falling back to polling every 1s",
                    self.opts.directory.display()
                ),
            }
        }
        let interval = if self.opts.polling_interval > 0.0 { self.opts.polling_interval } else { 1.0 };
        tracing::info!(
            target: "paperless.management.consumer",
            "Watching {} using polling (interval: {interval}s)",
            self.opts.directory.display()
        );
        self.watch_loop(None, interval, queued, stop).await;
        tracing::debug!(target: "paperless.management.consumer", "Consumer exiting");
    }

    /// `_watch_directory`: with `events` (native events) or via directory snapshot every
    /// `interval` seconds.
    async fn watch_loop(
        &self,
        mut events: Option<NativeEvents>,
        interval: f64,
        mut queued: HashSet<PathBuf>,
        mut stop: tokio::sync::watch::Receiver<bool>,
    ) {
        let stability = Duration::from_secs_f64(self.opts.stability_delay.max(0.0));
        let mut tracker = FileStabilityTracker::new(stability);
        let mut snapshot = if events.is_none() { self.snapshot() } else { BTreeMap::new() };
        let mut last_rescan = Instant::now();
        loop {
            if *stop.borrow() {
                break;
            }
            let changes: Vec<(Change, PathBuf)> = match events.as_mut() {
                Some(ev) => {
                    // upstream: rust_timeout = stability delay with open files, otherwise infinite,
                    // each capped to the rescan interval (cap_for_rescan).
                    let wait = if tracker.has_pending_files() { stability.max(Duration::from_millis(50)) } else { Duration::MAX };
                    let wait = cap_for_rescan(wait, self.opts.rescan_interval);
                    let mut batch = Vec::new();
                    tokio::select! {
                        got = ev.next_batch(wait, &mut batch) => {
                            if !got {
                                tracing::warn!(target: "paperless.management.consumer", "File system event stream ended; falling back to polling every 1s");
                                events = None;
                                snapshot = self.snapshot();
                            }
                        }
                        _ = stop.changed() => { if *stop.borrow() { break; } }
                    }
                    batch.into_iter().filter(|(_, p)| self.filter.accepts(p)).collect()
                }
                None => {
                    let wait = if tracker.has_pending_files() {
                        Duration::from_secs_f64(interval.min(self.opts.stability_delay.max(0.05)))
                    } else {
                        Duration::from_secs_f64(interval)
                    };
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = stop.changed() => { if *stop.borrow() { break; } }
                    }
                    let now_snap = self.snapshot();
                    let d = diff(&snapshot, &now_snap);
                    snapshot = now_snap;
                    d
                }
            };
            for (change, path) in changes {
                let path = absolute(&path);
                if change == Change::Deleted {
                    queued.remove(&path);
                }
                if !path.is_file() {
                    continue;
                }
                if queued.contains(&path) {
                    tracing::debug!(target: "paperless.management.consumer", "Ignoring event for queued file: {}", path.display());
                    continue;
                }
                tracing::debug!(target: "paperless.management.consumer", "Event: {change:?} for {}", path.display());
                tracker.track(&path, change);
            }
            for p in tracker.take_stable_files() {
                if self.consume_path(&p).await {
                    queued.insert(p);
                }
            }
            let forced = events.as_mut().is_some_and(|e| std::mem::take(&mut e.needs_rescan));
            if forced || (!self.opts.rescan_interval.is_zero() && last_rescan.elapsed() >= self.opts.rescan_interval) {
                self.rescan(&mut tracker, &mut queued);
                last_rescan = Instant::now();
            }
        }
    }

    /// `_rescan_existing_files`.
    fn rescan(&self, tracker: &mut FileStabilityTracker, queued: &mut HashSet<PathBuf>) {
        queued.retain(|p| p.exists());
        for p in self.list_files() {
            if !self.filter.accepts(&p) {
                continue;
            }
            let r = absolute(&p);
            if tracker.is_tracking(&r) || queued.contains(&r) {
                continue;
            }
            tracing::debug!(target: "paperless.management.consumer", "Rescan found untracked file: {}", r.display());
            tracker.track(&r, Change::Added);
        }
    }

    fn snapshot(&self) -> BTreeMap<PathBuf, (Option<SystemTime>, u64)> {
        self.list_files()
            .into_iter()
            .filter(|p| self.filter.accepts(p))
            .map(|p| {
                let m = std::fs::metadata(&p).ok();
                let v = (m.as_ref().and_then(|m| m.modified().ok()), m.map_or(0, |m| m.len()));
                (absolute(&p), v)
            })
            .collect()
    }
}

/// `[[ -n "${PAPERLESS_CONSUMER_DISABLE}" ]]` (svc-consumer/run): any non-empty value switches off
/// (also `false`, like upstream).
pub fn consumer_disabled(get: &dyn Fn(&str) -> Option<String>) -> bool {
    get("PAPERLESS_CONSUMER_DISABLE").is_some_and(|v| !v.is_empty())
}

/// `cap_for_rescan`: wait at most until the next rescan (rescan off -> only an upper bound against
/// overflow of `Instant + Duration`).
fn cap_for_rescan(wait: Duration, rescan: Duration) -> Duration {
    const FAR: Duration = Duration::from_secs(24 * 3600);
    let w = if rescan.is_zero() { wait } else { wait.min(rescan) };
    w.min(FAR)
}

/// Native file system events (`notify`, like the Rust core of watchfiles).
pub struct NativeEvents {
    _watcher: notify::RecommendedWatcher,
    rx: tokio::sync::mpsc::UnboundedReceiver<notify::Result<notify::Event>>,
    /// Events lost (overflow/error) -> immediate rescan.
    pub needs_rescan: bool,
}

impl NativeEvents {
    /// Start watching; error -> the caller falls back to polling.
    pub fn start(dir: &Path, recursive: bool) -> Result<Self, String> {
        use notify::Watcher;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut w = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })
        .map_err(|e| e.to_string())?;
        let mode = if recursive { notify::RecursiveMode::Recursive } else { notify::RecursiveMode::NonRecursive };
        w.watch(dir, mode).map_err(|e| e.to_string())?;
        Ok(Self { _watcher: w, rx, needs_rescan: false })
    }

    /// Waits at most `wait` for the first event and then keeps collecting for 50 ms (watchfiles
    /// `step`). `false` if the event stream has ended.
    pub async fn next_batch(&mut self, wait: Duration, out: &mut Vec<(Change, PathBuf)>) -> bool {
        let first = match tokio::time::timeout(wait, self.rx.recv()).await {
            Err(_) => return true,
            Ok(None) => return false,
            Ok(Some(r)) => r,
        };
        self.take(first, out);
        tokio::time::sleep(Duration::from_millis(50)).await;
        loop {
            match self.rx.try_recv() {
                Ok(r) => self.take(r, out),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => return true,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => return !out.is_empty(),
            }
        }
    }

    fn take(&mut self, r: notify::Result<notify::Event>, out: &mut Vec<(Change, PathBuf)>) {
        match r {
            Ok(ev) => {
                if ev.need_rescan() {
                    self.needs_rescan = true;
                }
                out.extend(map_event(&ev));
            }
            Err(e) => {
                tracing::debug!(target: "paperless.management.consumer", "File system event error: {e}");
                self.needs_rescan = true;
            }
        }
    }
}

/// `notify` event -> watchfiles `Change` (watchfiles src/lib.rs: Create -> added, Remove ->
/// deleted, rename from/to -> deleted/added, other change -> modified, access -> nothing).
pub fn map_event(ev: &notify::Event) -> Vec<(Change, PathBuf)> {
    use notify::EventKind;
    use notify::event::{ModifyKind, RenameMode};
    let all = |c: Change| ev.paths.iter().map(|p| (c, p.clone())).collect::<Vec<_>>();
    let by_existence =
        |present: Change| ev.paths.iter().map(|p| (if p.exists() { present } else { Change::Deleted }, p.clone())).collect::<Vec<_>>();
    match ev.kind {
        EventKind::Access(_) => Vec::new(),
        EventKind::Create(_) => all(Change::Added),
        EventKind::Remove(_) => all(Change::Deleted),
        EventKind::Modify(ModifyKind::Name(RenameMode::From)) => all(Change::Deleted),
        EventKind::Modify(ModifyKind::Name(RenameMode::To)) => all(Change::Added),
        EventKind::Modify(ModifyKind::Name(RenameMode::Both)) => {
            let mut v = Vec::new();
            if let Some(f) = ev.paths.first() {
                v.push((Change::Deleted, f.clone()));
            }
            v.extend(ev.paths.iter().skip(1).map(|p| (Change::Added, p.clone())));
            v
        }
        EventKind::Modify(ModifyKind::Name(_)) => by_existence(Change::Added),
        EventKind::Modify(_) | EventKind::Any | EventKind::Other => by_existence(Change::Modified),
    }
}

/// Snapshot comparison (watchfiles polling).
fn diff(old: &BTreeMap<PathBuf, (Option<SystemTime>, u64)>, new: &BTreeMap<PathBuf, (Option<SystemTime>, u64)>) -> Vec<(Change, PathBuf)> {
    let mut out = Vec::new();
    for (p, v) in new {
        match old.get(p) {
            None => out.push((Change::Added, p.clone())),
            Some(o) if o != v => out.push((Change::Modified, p.clone())),
            _ => {}
        }
    }
    for p in old.keys() {
        if !new.contains_key(p) {
            out.push((Change::Deleted, p.clone()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter() -> ConsumerFilter {
        ConsumerFilter::new(BTreeSet::from([".pdf".to_string(), ".txt".to_string()]), &[r"^ignore".into()], &["skipme".into()])
    }

    #[test]
    fn filter_rules() {
        let t = tempfile::tempdir().unwrap();
        let f = filter();
        let ok = t.path().join("a.PDF");
        std::fs::write(&ok, b"x").unwrap();
        assert!(f.accepts(&ok));
        for bad in ["._a.pdf", ".DS_Store", "Thumbs.db", "ignore-me.pdf", "x.docx"] {
            let p = t.path().join(bad);
            std::fs::write(&p, b"x").unwrap();
            assert!(!f.accepts(&p), "{bad}");
        }
        let d = t.path().join("@eaDir");
        std::fs::create_dir(&d).unwrap();
        let p = d.join("a.pdf");
        std::fs::write(&p, b"x").unwrap();
        assert!(!f.accepts(&p));
        let d = t.path().join("skipme");
        std::fs::create_dir(&d).unwrap();
        assert!(!f.accepts(&d.join("a.pdf")));
    }

    #[test]
    fn stability() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("a.pdf");
        std::fs::write(&p, b"abc").unwrap();
        let empty = t.path().join("e.pdf");
        std::fs::write(&empty, b"").unwrap();
        let mut tr = FileStabilityTracker::new(Duration::from_millis(0));
        tr.track(&p, Change::Added);
        tr.track(&empty, Change::Added);
        assert!(tr.has_pending_files());
        let s = tr.take_stable_files();
        assert_eq!(s, vec![absolute(&p)]);
        assert!(!tr.has_pending_files(), "empty file dropped");
        tr.track(&p, Change::Added);
        tr.track(&p, Change::Deleted);
        assert!(!tr.is_tracking(&p));
    }

    #[test]
    fn disable_switch_like_svc_consumer() {
        assert!(!consumer_disabled(&|_| None));
        assert!(!consumer_disabled(&|_| Some(String::new())));
        assert!(consumer_disabled(&|_| Some("1".into())));
        // upstream only checks "not empty"
        assert!(consumer_disabled(&|_| Some("false".into())));
    }

    #[test]
    fn rescan_cap() {
        let r = Duration::from_secs(300);
        assert_eq!(cap_for_rescan(Duration::MAX, r), r);
        assert_eq!(cap_for_rescan(Duration::from_secs(5), r), Duration::from_secs(5));
        assert_eq!(cap_for_rescan(Duration::MAX, Duration::ZERO), Duration::from_secs(24 * 3600));
    }

    #[test]
    fn notify_events_mapped_like_watchfiles() {
        use notify::event::{AccessKind, CreateKind, DataChange, ModifyKind, RemoveKind, RenameMode};
        use notify::{Event, EventKind};
        let ev = |k: EventKind, p: &[&str]| {
            let mut e = Event::new(k);
            for x in p {
                e = e.add_path(PathBuf::from(x));
            }
            e
        };
        assert_eq!(map_event(&ev(EventKind::Create(CreateKind::File), &["a"])), vec![(Change::Added, "a".into())]);
        assert_eq!(map_event(&ev(EventKind::Remove(RemoveKind::File), &["a"])), vec![(Change::Deleted, "a".into())]);
        assert!(map_event(&ev(EventKind::Access(AccessKind::Any), &["a"])).is_empty());
        assert_eq!(
            map_event(&ev(EventKind::Modify(ModifyKind::Name(RenameMode::Both)), &["a", "b"])),
            vec![(Change::Deleted, "a".into()), (Change::Added, "b".into())]
        );
        assert_eq!(map_event(&ev(EventKind::Modify(ModifyKind::Name(RenameMode::To)), &["b"])), vec![(Change::Added, "b".into())]);
        let t = tempfile::tempdir().unwrap();
        let f = t.path().join("x.pdf");
        std::fs::write(&f, b"1").unwrap();
        let fs = f.to_string_lossy().to_string();
        assert_eq!(map_event(&ev(EventKind::Modify(ModifyKind::Data(DataChange::Any)), &[&fs])), vec![(Change::Modified, f.clone())]);
        let gone = t.path().join("gone.pdf").to_string_lossy().to_string();
        assert_eq!(map_event(&ev(EventKind::Modify(ModifyKind::Any), &[&gone]))[0].0, Change::Deleted);
    }

    /// Native events: a new file is reported without polling.
    #[tokio::test]
    async fn native_events_report_new_file() {
        let t = tempfile::tempdir().unwrap();
        let Ok(mut ev) = NativeEvents::start(t.path(), true) else {
            eprintln!("no native events on this system - skipped");
            return;
        };
        let f = t.path().join("new.pdf");
        std::fs::write(&f, b"data").unwrap();
        let mut seen = false;
        for _ in 0..20 {
            let mut batch = Vec::new();
            assert!(ev.next_batch(Duration::from_millis(250), &mut batch).await);
            if batch.iter().any(|(c, p)| *c != Change::Deleted && p.file_name() == f.file_name()) {
                seen = true;
                break;
            }
        }
        assert!(seen, "no event for the new file");
    }

    #[test]
    fn snapshot_diff() {
        let a = BTreeMap::from([(PathBuf::from("a"), (None, 1)), (PathBuf::from("b"), (None, 1))]);
        let b = BTreeMap::from([(PathBuf::from("a"), (None, 2)), (PathBuf::from("c"), (None, 1))]);
        let d = diff(&a, &b);
        assert!(d.contains(&(Change::Modified, "a".into())));
        assert!(d.contains(&(Change::Added, "c".into())));
        assert!(d.contains(&(Change::Deleted, "b".into())));
    }
}
