//! Scan jobs: flow against the scanner and in-process management.
//!
//! Flow ([`run_scan`]):
//! 1. `ScannerStatus` (if present): not `Idle` → busy; on feed empty/jam → error.
//! 2. `POST ScanJobs` → job URL (`503` busy, `409` on feed = empty).
//! 3. `NextDocument` repeated: `200` = page, `503` = wait, `404` = done. Flatbed:
//!    done after the first page (like sane-airscan). Other responses → read the status
//!    (jam / aborted by the device) → error.
//! 4. Cancel by the user: checked before every request → `DELETE` on the job.
//!
//! Afterwards ([`ScanJob::spawn`]): pages → PDF → hand-over to the pipeline (owner = user).
//! Jobs live only in memory (removed one hour after the end); per scanner at most
//! one runs.

use std::collections::HashMap;
use std::path::PathBuf;
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
/// At most this many pages per job.
pub const MAX_PAGES: usize = 500;

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
    let mut not_ready = 0u32;
    // 3. Pages
    loop {
        if cancelled() {
            client.cancel(&job).await;
            return Err(ScanError::Cancelled);
        }
        match client.next_document(&job).await {
            Ok(Next::Page(p)) => {
                not_ready = 0;
                pages.push(p);
                progress.page(pages.len());
                if settings.source == Source::Flatbed {
                    break;
                }
                if pages.len() >= MAX_PAGES {
                    client.cancel(&job).await;
                    break;
                }
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

    /// Request cancellation. `false` if the job has already ended.
    pub fn cancel(&self) -> bool {
        if !self.is_running() {
            return false;
        }
        let _ = self.cancel.send(true);
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
    let pages = match run_scan(&spec.client, &spec.settings, &rx, &progress).await {
        Ok(p) => p,
        Err(ScanError::Cancelled) => return job.finish(JobState::Cancelled, Some(ScanError::Cancelled)),
        Err(e) => {
            tracing::info!(target: "paperrust.scan", scanner = spec.scanner_id, code = e.code(), "scan failed: {e}");
            return job.finish(JobState::Failed, Some(e));
        }
    };
    job.update(|v| v.state = JobState::Processing);
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
