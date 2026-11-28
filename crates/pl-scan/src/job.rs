//! Scan jobs: flow against the scanner and in-process management.
//!
//! Flow ([`run_scan`]):
//! 1. `ScannerStatus` (if present): not `Idle` → busy; on feed empty/jam → error.
//! 2. `POST ScanJobs` → job URL (`503` busy, `409` on feed = empty).
//! 3. `NextDocument` repeated: `200` = page, `503` = wait, `404` = done. Flatbed:
//!    done after the first page (like sane-airscan). Other responses → read the status
//!    (jam / aborted by the device) → error.
//! 4. Cancel by the user: checked before every request → `DELETE` on the job.
//! 5. Limits ([`Limits`]): more than `max_pages` pages or more than `max_job_bytes` in total →
//!    `DELETE` on the job and an error; a truncated batch is never imported.
//!
//! Afterwards ([`execute`]): pages → PDF → hand-over to the pipeline (owner = user). The cancel
//! signal is checked again before the PDF is built and right before the hand-over; the hand-over
//! itself is the commit point (a cancel after it is refused, see [`ScanJob::cancel`]).
//! Jobs live only in memory of the process that started them (removed one hour after the end);
//! per scanner at most one runs (also only per process, see the contract "Known differences").

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::watch;
use url::Url;
use uuid::Uuid;

use crate::escl::{Client, Next, Page, ScanError, ScanSettings, Source};
use crate::intake::{Intake, IntakeDoc};

/// Wait between `503` responses of `NextDocument`.
pub const RETRY_DELAY: Duration = Duration::from_millis(500);
/// At most this many `503` in a row (≈ 5 min at 500 ms).
pub const MAX_NOT_READY: u32 = 600;
/// At most this many pages per job (more = the job fails with `too_many_pages`).
pub const MAX_PAGES: usize = 500;
/// Setting: total size of all pages of one job in MiB (they are held in memory until the PDF
/// is built).
pub const SETTING_MAX_JOB_MB: &str = "PAPERRUST_SCAN_MAX_JOB_MB";
/// Default for [`SETTING_MAX_JOB_MB`]: 1 GiB.
pub const DEFAULT_MAX_JOB_MB: u64 = 1024;
/// Allowed range for [`SETTING_MAX_JOB_MB`].
const MAX_JOB_MB_RANGE: std::ops::RangeInclusive<u64> = 1..=65536;

/// Limits of one scan job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Pages per job; one page more fails the job.
    pub max_pages: usize,
    /// Sum of all page sizes per job in bytes.
    pub max_job_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_pages: MAX_PAGES, max_job_bytes: DEFAULT_MAX_JOB_MB * 1024 * 1024 }
    }
}

impl Limits {
    /// Limits from a variable lookup (`pl_core::config::var` in production). Empty = default;
    /// an invalid value is an error (the caller decides how to report it).
    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Limits, String> {
        let mut l = Limits::default();
        if let Some(v) = get(SETTING_MAX_JOB_MB).filter(|v| !v.trim().is_empty()) {
            let mb = v
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|mb| MAX_JOB_MB_RANGE.contains(mb))
                .ok_or_else(|| format!("{SETTING_MAX_JOB_MB}: expected 1–65536 (MiB)"))?;
            l.max_job_bytes = mb * 1024 * 1024;
        }
        Ok(l)
    }

    /// Limits from the environment / saved settings; an invalid value is logged and the default
    /// is used (scanning stays possible, the limit stays conservative).
    pub fn from_env() -> Limits {
        Limits::from_lookup(pl_core::config::var).unwrap_or_else(|e| {
            tracing::warn!(target: "paperrust.scan", "invalid scan setting, using the default: {e}");
            Limits::default()
        })
    }
}

/// Progress while scanning.
pub trait Progress: Send + Sync {
    fn page(&self, n: usize);
}

impl<F: Fn(usize) + Send + Sync> Progress for F {
    fn page(&self, n: usize) {
        self(n)
    }
}

async fn status_error(client: &Client, job: &Url, code: u16) -> ScanError {
    match client.status().await {
        Ok(s) if s.adf_jam() => ScanError::Jam,
        Ok(s) => match s.job(job.as_str()) {
            Some((_, _, reasons)) if reasons.iter().any(|r| r.contains("Jam")) => ScanError::Jam,
            Some((_, state, _)) if state == "Canceled" || state == "Aborted" => ScanError::Cancelled,
            _ => ScanError::Protocol(format!("next page: HTTP {code}")),
        },
        Err(_) => ScanError::Protocol(format!("next page: HTTP {code}")),
    }
}

/// Fetch the pages of a job from the scanner.
pub async fn run_scan(
    client: &Client,
    settings: &ScanSettings,
    limits: &Limits,
    cancel: &watch::Receiver<bool>,
    progress: &dyn Progress,
) -> Result<Vec<Page>, ScanError> {
    let cancelled = || *cancel.borrow();
    // 1. Status beforehand (devices without ScannerStatus: continue)
    if let Ok(s) = client.status().await {
        if !s.is_idle() {
            return Err(ScanError::Busy);
        }
        if settings.source.is_feeder() {
            if s.adf_jam() {
                return Err(ScanError::Jam);
            }
            if s.adf_empty() {
                return Err(ScanError::AdfEmpty);
            }
        }
    }
    if cancelled() {
        return Err(ScanError::Cancelled);
    }
    // 2. Job
    let job = client.create_job(settings).await?;
    let mut pages: Vec<Page> = Vec::new();
    let mut total_bytes: u64 = 0;
    let mut not_ready = 0u32;
    // 3. Pages
    loop {
        if cancelled() {
            client.cancel(&job).await;
            return Err(ScanError::Cancelled);
        }
        // Never read more than the rest of the job budget (+1 byte to detect the overflow).
        let remaining = limits.max_job_bytes.saturating_sub(total_bytes).saturating_add(1);
        let page_limit = usize::try_from(remaining).unwrap_or(usize::MAX).min(crate::escl::MAX_PAGE_BYTES);
        match client.next_document_limited(&job, page_limit).await {
            Ok(Next::Page(p)) => {
                not_ready = 0;
                // One page more than allowed: the device still has paper → fail instead of
                // importing a truncated batch.
                if pages.len() >= limits.max_pages {
                    client.cancel(&job).await;
                    return Err(ScanError::TooManyPages(limits.max_pages));
                }
                total_bytes = total_bytes.saturating_add(p.data.len() as u64);
                if total_bytes > limits.max_job_bytes {
                    client.cancel(&job).await;
                    return Err(ScanError::JobTooLarge(limits.max_job_bytes));
                }
                pages.push(p);
                progress.page(pages.len());
                if settings.source == Source::Flatbed {
                    break;
                }
            }
            // The page did not fit into the rest of the job budget (a single page above
            // MAX_PAGE_BYTES stays `too_large`).
            Err(ScanError::TooLarge) if page_limit < crate::escl::MAX_PAGE_BYTES => {
                client.cancel(&job).await;
                return Err(ScanError::JobTooLarge(limits.max_job_bytes));
            }
            Ok(Next::NotReady) => {
                not_ready += 1;
                if not_ready > MAX_NOT_READY {
                    client.cancel(&job).await;
                    return Err(ScanError::Protocol("scanner did not deliver a page in time".into()));
                }
                tokio::time::sleep(RETRY_DELAY).await;
            }
            Ok(Next::Done) => {
                if pages.is_empty() {
                    // Not a single page: read the cause from the status
                    return Err(match client.status().await {
                        Ok(s) if s.adf_jam() => ScanError::Jam,
                        Ok(s) if settings.source.is_feeder() && s.adf_empty() => ScanError::AdfEmpty,
                        _ => ScanError::Protocol("scanner delivered no page".into()),
                    });
                }
                // a jam after a few pages is reported by the device as job abort / AdfJam
                if let Ok(s) = client.status().await {
                    if s.adf_jam() {
                        return Err(ScanError::Jam);
                    }
                    if let Some((_, state, reasons)) = s.job(job.as_str())
                        && state == "Aborted"
                        && reasons.iter().any(|r| r.contains("Jam"))
                    {
                        return Err(ScanError::Jam);
                    }
                }
                break;
            }
            Ok(Next::Failed(code)) => {
                let e = status_error(client, &job, code).await;
                client.cancel(&job).await;
                return Err(e);
            }
            Err(e) => {
                client.cancel(&job).await;
                return Err(e);
            }
        }
    }
    Ok(pages)
}

// ------------------------------------------------------------------------------ Management

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Scanning,
    Processing,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobError {
    pub code: &'static str,
    pub message: String,
}

/// Public state of a job (response of `GET /api/x-scan/jobs/{id}/`).
#[derive(Debug, Clone, Serialize)]
pub struct JobView {
    pub id: Uuid,
    pub scanner: i64,
    pub source: &'static str,
    pub state: JobState,
    pub pages: usize,
    pub error: Option<JobError>,
    /// Task of the pipeline (`/api/tasks/?task_id=…`)
    pub task_id: Option<String>,
    pub created: chrono::DateTime<chrono::Utc>,
}

pub struct ScanJob {
    pub user_id: i64,
    view: Mutex<JobView>,
    cancel: watch::Sender<bool>,
    /// Set (under the `view` lock) right before the hand-over to the pipeline; from then on a
    /// cancel is refused, because the document may already be queued.
    committed: AtomicBool,
    finished: Mutex<Option<Instant>>,
}

impl ScanJob {
    pub fn view(&self) -> JobView {
        match self.view.lock() {
            Ok(v) => v.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    fn update(&self, f: impl FnOnce(&mut JobView)) {
        match self.view.lock() {
            Ok(mut v) => f(&mut v),
            Err(p) => f(&mut p.into_inner()),
        }
    }

    fn finish(&self, state: JobState, error: Option<ScanError>) {
        self.update(|v| {
            v.state = state;
            v.error = error.map(|e| JobError { code: e.code(), message: e.to_string() });
        });
        if let Ok(mut f) = self.finished.lock() {
            *f = Some(Instant::now());
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.view().state, JobState::Scanning | JobState::Processing)
    }

    /// Request cancellation. `false` if the job has already ended or is being handed over to
    /// the pipeline (then the document will be imported). Decided under the `view` lock, like
    /// [`ScanJob::commit`], so a confirmed cancel never ends with an imported document.
    pub fn cancel(&self) -> bool {
        let guard = match self.view.lock() {
            Ok(v) => v,
            Err(p) => p.into_inner(),
        };
        if !matches!(guard.state, JobState::Scanning | JobState::Processing) || self.committed.load(Ordering::SeqCst) {
            return false;
        }
        let _ = self.cancel.send(true);
        true
    }

    fn cancel_requested(&self) -> bool {
        *self.cancel.borrow()
    }

    /// Commit point before the hand-over: `false` if a cancel came first, otherwise later
    /// cancels are refused.
    fn commit(&self) -> bool {
        let _guard = match self.view.lock() {
            Ok(v) => v,
            Err(p) => p.into_inner(),
        };
        if self.cancel_requested() {
            return false;
        }
        self.committed.store(true, Ordering::SeqCst);
        true
    }
}

/// Everything for a job.
pub struct JobSpec {
    pub scanner_id: i64,
    pub client: Client,
    pub settings: ScanSettings,
    pub user_id: i64,
    pub title: Option<String>,
    pub scratch: PathBuf,
    pub intake: Arc<dyn Intake>,
    /// Page and size limits ([`Limits::from_env`] in production).
    pub limits: Limits,
}

#[derive(Default)]
pub struct Registry {
    jobs: Mutex<HashMap<Uuid, Arc<ScanJob>>>,
}

/// Process-wide management.
pub fn registry() -> &'static Registry {
    static R: OnceLock<Registry> = OnceLock::new();
    R.get_or_init(Registry::default)
}

const KEEP_FINISHED: Duration = Duration::from_secs(3600);

impl Registry {
    pub fn get(&self, id: Uuid) -> Option<Arc<ScanJob>> {
        self.jobs.lock().ok()?.get(&id).cloned()
    }

    fn prune(map: &mut HashMap<Uuid, Arc<ScanJob>>) {
        map.retain(|_, j| match j.finished.lock().ok().and_then(|f| *f) {
            Some(t) => t.elapsed() < KEEP_FINISHED,
            None => true,
        });
    }

    /// Start a job. `Err(Busy)` if one is already running on this scanner.
    pub fn start(&self, spec: JobSpec) -> Result<Arc<ScanJob>, ScanError> {
        let (tx, rx) = watch::channel(false);
        let id = Uuid::new_v4();
        let job = Arc::new(ScanJob {
            user_id: spec.user_id,
            view: Mutex::new(JobView {
                id,
                scanner: spec.scanner_id,
                source: spec.settings.source.as_str(),
                state: JobState::Scanning,
                pages: 0,
                error: None,
                task_id: None,
                created: chrono::Utc::now(),
            }),
            cancel: tx,
            committed: AtomicBool::new(false),
            finished: Mutex::new(None),
        });
        {
            let mut map = self.jobs.lock().map_err(|_| ScanError::Protocol("job registry poisoned".into()))?;
            Self::prune(&mut map);
            if map.values().any(|j| j.view().scanner == spec.scanner_id && j.is_running()) {
                return Err(ScanError::Busy);
            }
            map.insert(id, job.clone());
        }
        let j = job.clone();
        tokio::spawn(async move { execute(j, spec, rx).await });
        Ok(job)
    }
}

async fn execute(job: Arc<ScanJob>, spec: JobSpec, rx: watch::Receiver<bool>) {
    let j2 = job.clone();
    let progress = move |n: usize| j2.update(|v| v.pages = n);
    let pages = match run_scan(&spec.client, &spec.settings, &spec.limits, &rx, &progress).await {
        Ok(p) => p,
        Err(ScanError::Cancelled) => return job.finish(JobState::Cancelled, Some(ScanError::Cancelled)),
        Err(e) => {
            tracing::info!(target: "paperrust.scan", scanner = spec.scanner_id, code = e.code(), "scan failed: {e}");
            return job.finish(JobState::Failed, Some(e));
        }
    };
    job.update(|v| v.state = JobState::Processing);
    // A cancel can arrive while the last NextDocument request is running (the scan loop checks
    // only before each request).
    if job.cancel_requested() {
        return job.finish(JobState::Cancelled, Some(ScanError::Cancelled));
    }
    let name = format!("scan-{}.pdf", chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"));
    let dir = match crate::intake::spool_dir(&spec.scratch) {
        Ok(d) => d,
        Err(e) => return job.finish(JobState::Failed, Some(ScanError::Intake(e.to_string()))),
    };
    let path = dir.join(&name);
    let resolution = spec.settings.resolution;
    let p2 = path.clone();
    let built = tokio::task::spawn_blocking(move || crate::pdf::write_pdf(&pages, resolution, &p2)).await;
    match built {
        Ok(Ok(n)) => job.update(|v| v.pages = n),
        Ok(Err(e)) => {
            let _ = std::fs::remove_dir_all(&dir);
            return job.finish(JobState::Failed, Some(e));
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            return job.finish(JobState::Failed, Some(ScanError::Pdf(e.to_string())));
        }
    }
    // Last chance to cancel: the hand-over is the commit point.
    if !job.commit() {
        let _ = std::fs::remove_dir_all(&dir);
        return job.finish(JobState::Cancelled, Some(ScanError::Cancelled));
    }
    let doc = IntakeDoc {
        path: path.clone(),
        filename: name,
        title: spec.title.clone(),
        owner_id: spec.user_id,
        source: pl_consumer::DocumentSource::WebUI,
    };
    match spec.intake.submit(doc).await {
        Ok(task_id) => {
            tracing::info!(target: "paperrust.scan", scanner = spec.scanner_id, user = spec.user_id, pages = job.view().pages, task = %task_id, "scan handed to consumer");
            job.update(|v| v.task_id = Some(task_id));
            job.finish(JobState::Done, None);
        }
        Err(e) => {
            crate::intake::discard(&path);
            job.finish(JobState::Failed, Some(ScanError::Intake(e)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
    }

    #[test]
    fn limits_from_lookup() {
        let d = Limits::from_lookup(get(&[])).unwrap();
        assert_eq!(d, Limits { max_pages: 500, max_job_bytes: 1024 * 1024 * 1024 });
        assert_eq!(Limits::from_lookup(get(&[(SETTING_MAX_JOB_MB, " ")])).unwrap(), d);
        assert_eq!(Limits::from_lookup(get(&[(SETTING_MAX_JOB_MB, "256")])).unwrap().max_job_bytes, 256 * 1024 * 1024);
        for bad in ["0", "-1", "abc", "65537", "1.5"] {
            assert!(Limits::from_lookup(get(&[(SETTING_MAX_JOB_MB, bad)])).is_err(), "{bad}");
        }
    }

    #[test]
    fn limit_errors_have_own_codes() {
        assert_eq!(ScanError::TooManyPages(500).code(), "too_many_pages");
        assert_eq!(ScanError::JobTooLarge(1024 * 1024 * 1024).code(), "job_too_large");
        assert_eq!(ScanError::JobTooLarge(1024 * 1024 * 1024).to_string(), "scan is larger than 1024 MiB in total; nothing was imported");
        assert_eq!(ScanError::JobTooLarge(10).to_string(), "scan is larger than 1 MiB in total; nothing was imported");
    }
}
