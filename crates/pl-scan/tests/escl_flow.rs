//! Whole scan flow against a **test scanner** (small eSCL HTTP server on 127.0.0.1):
//! feed with several pages, double-sided, flatbed, jam, cancel, busy, feed empty,
//! foreign `Location`. The hand-over to the pipeline is recorded (no database needed).

mod support;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::future::BoxFuture;
use pl_scan::escl::{Client, ColorMode, ScanError, ScanSettings, Source, normalize_base};
use pl_scan::intake::{Intake, IntakeDoc};
use pl_scan::job::{JobSpec, JobState, Limits, Registry};
use support::{Fake, start_fake};

/// Records hand-overs (instead of the queue).
#[derive(Default)]
struct Captured {
    docs: Mutex<Vec<(IntakeDoc, Vec<u8>)>>,
}

impl Intake for Captured {
    fn submit(&self, doc: IntakeDoc) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            let data = std::fs::read(&doc.path).map_err(|e| e.to_string())?;
            self.docs.lock().unwrap().push((doc, data));
            Ok("task-1".into())
        })
    }
}

async fn run_job(
    f: Fake,
    source: Source,
    color: Option<ColorMode>,
    resolution: Option<u32>,
) -> (Arc<Fake>, Arc<Captured>, pl_scan::job::JobView, tempfile::TempDir) {
    run_job_limited(f, source, color, resolution, Limits::default()).await
}

async fn run_job_limited(
    f: Fake,
    source: Source,
    color: Option<ColorMode>,
    resolution: Option<u32>,
    limits: Limits,
) -> (Arc<Fake>, Arc<Captured>, pl_scan::job::JobView, tempfile::TempDir) {
    let (fake, client) = start_fake(f).await;
    let caps = client.capabilities().await.unwrap();
    let settings = ScanSettings::resolve(&caps, source, color, resolution).unwrap();
    let cap = Arc::new(Captured::default());
    let scratch = tempfile::tempdir().unwrap();
    let reg = Registry::default();
    let job = reg
        .start(JobSpec {
            scanner_id: 1,
            client,
            settings,
            user_id: 42,
            title: Some("Post".into()),
            scratch: scratch.path().to_path_buf(),
            intake: cap.clone(),
            limits,
        })
        .unwrap();
    let view = wait(&job).await;
    (fake, cap, view, scratch)
}

async fn wait(job: &pl_scan::job::ScanJob) -> pl_scan::job::JobView {
    for _ in 0..400 {
        if !job.is_running() {
            return job.view();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("job did not finish: {:?}", job.view());
}

fn pdf_pages(data: &[u8]) -> Vec<f32> {
    let doc = lopdf::Document::load_mem(data).unwrap();
    doc.get_pages()
        .values()
        .map(|id| {
            let mb = doc.get_dictionary(*id).unwrap().get(b"MediaBox").unwrap().as_array().unwrap().clone();
            mb[2].as_float().unwrap()
        })
        .collect()
}

#[tokio::test]
async fn adf_multi_page_to_one_pdf() {
    let (fake, cap, view, _t) = run_job(Fake { sheets: 3, not_ready_each: 2, ..Default::default() }, Source::Adf, None, Some(100)).await;
    assert_eq!(view.state, JobState::Done, "{view:?}");
    assert_eq!(view.pages, 3);
    assert_eq!(view.task_id.as_deref(), Some("task-1"));
    let docs = cap.docs.lock().unwrap();
    assert_eq!(docs.len(), 1);
    let (doc, data) = &docs[0];
    assert_eq!(doc.owner_id, 42, "owned by the user who pressed the button");
    assert_eq!(doc.source, pl_consumer::DocumentSource::WebUI);
    assert_eq!(doc.title.as_deref(), Some("Post"));
    assert!(doc.filename.starts_with("scan-") && doc.filename.ends_with(".pdf"));
    // order preserved: widths 100, 101, 102 px at 100 dpi → 72, 72.72, 73.44 pt
    let widths = pdf_pages(data);
    assert_eq!(widths.len(), 3);
    assert!(widths[0] < widths[1] && widths[1] < widths[2], "{widths:?}");
    let xml = fake.settings_xml.lock().unwrap().clone();
    assert!(xml.contains("<scan:Duplex>false</scan:Duplex>") && xml.contains("<scan:XResolution>100</scan:XResolution>"));
}

#[tokio::test]
async fn adf_duplex_delivers_both_sides() {
    let (fake, cap, view, _t) =
        run_job(Fake { sheets: 2, ..Default::default() }, Source::AdfDuplex, Some(ColorMode::Grayscale), Some(200)).await;
    assert_eq!(view.state, JobState::Done, "{view:?}");
    assert_eq!(view.pages, 4);
    assert_eq!(pdf_pages(&cap.docs.lock().unwrap()[0].1).len(), 4);
    let xml = fake.settings_xml.lock().unwrap().clone();
    assert!(xml.contains("<scan:Duplex>true</scan:Duplex>"));
    assert!(xml.contains("<scan:ColorMode>Grayscale8</scan:ColorMode>"));
}

#[tokio::test]
async fn flatbed_stops_after_one_page() {
    let (fake, cap, view, _t) = run_job(Fake::default(), Source::Flatbed, None, None).await;
    assert_eq!(view.state, JobState::Done, "{view:?}");
    assert_eq!(view.pages, 1);
    assert_eq!(fake.delivered.load(Ordering::SeqCst), 1);
    assert!(fake.settings_xml.lock().unwrap().contains("Platen"));
    assert_eq!(cap.docs.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn paper_jam_fails_without_document() {
    let (_fake, cap, view, _t) = run_job(Fake { sheets: 5, jam_after: Some(2), ..Default::default() }, Source::Adf, None, None).await;
    assert_eq!(view.state, JobState::Failed);
    assert_eq!(view.error.as_ref().map(|e| e.code), Some("jam"));
    assert!(cap.docs.lock().unwrap().is_empty());
}

#[tokio::test]
async fn busy_scanner() {
    let (_f, cap, view, _t) = run_job(Fake { busy_post: true, sheets: 1, ..Default::default() }, Source::Adf, None, None).await;
    assert_eq!(view.error.as_ref().map(|e| e.code), Some("busy"));
    assert!(cap.docs.lock().unwrap().is_empty());
    let (_f, _c, view, _t) = run_job(Fake { busy_status: true, sheets: 1, ..Default::default() }, Source::Adf, None, None).await;
    assert_eq!(view.error.as_ref().map(|e| e.code), Some("busy"));
}

#[tokio::test]
async fn empty_feeder() {
    let (_f, _c, view, _t) = run_job(Fake { adf_empty: true, ..Default::default() }, Source::Adf, None, None).await;
    assert_eq!(view.state, JobState::Failed);
    assert_eq!(view.error.as_ref().map(|e| e.code), Some("adf_empty"));
}

#[tokio::test]
async fn cancel_while_scanning() {
    let (fake, client) = start_fake(Fake { sheets: 10, not_ready_each: 1000, ..Default::default() }).await;
    let caps = client.capabilities().await.unwrap();
    let settings = ScanSettings::resolve(&caps, Source::Adf, None, None).unwrap();
    let cap = Arc::new(Captured::default());
    let scratch = tempfile::tempdir().unwrap();
    let reg = Registry::default();
    let job = reg
        .start(JobSpec {
            scanner_id: 7,
            client: client.clone(),
            settings: settings.clone(),
            user_id: 1,
            title: None,
            scratch: scratch.path().to_path_buf(),
            intake: cap.clone(),
            limits: Limits::default(),
        })
        .unwrap();
    // second job on the same scanner: busy (without a request to the device)
    let second = reg.start(JobSpec {
        scanner_id: 7,
        client,
        settings,
        user_id: 2,
        title: None,
        scratch: scratch.path().to_path_buf(),
        intake: cap.clone(),
        limits: Limits::default(),
    });
    assert!(matches!(second, Err(ScanError::Busy)));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(job.cancel());
    let view = wait(&job).await;
    assert_eq!(view.state, JobState::Cancelled);
    assert!(fake.cancelled.load(Ordering::SeqCst), "DELETE sent to the scanner");
    assert!(cap.docs.lock().unwrap().is_empty());
    assert!(!job.cancel(), "already finished");
}

#[tokio::test]
async fn foreign_location_is_refused() {
    let (_f, _c, view, _t) = run_job(Fake { evil_location: true, sheets: 1, ..Default::default() }, Source::Adf, None, None).await;
    assert_eq!(view.error.as_ref().map(|e| e.code), Some("protocol"));
}

#[tokio::test]
async fn unreachable_scanner() {
    // port 9 (discard) on 127.0.0.1 is closed → connection fails
    let client = Client::new(normalize_base("http://127.0.0.1:9/eSCL/").unwrap()).unwrap();
    assert!(matches!(client.capabilities().await, Err(ScanError::Unreachable(_))));
}

// ------------------------------------------------------------------ limits and late cancel

/// Review item 1: more pages than allowed must fail the job (DELETE to the device), never
/// import a silently truncated batch.
#[tokio::test]
async fn page_limit_fails_instead_of_truncating() {
    let limits = Limits { max_pages: 3, ..Limits::default() };
    let (fake, cap, view, _t) = run_job_limited(Fake { sheets: 5, ..Default::default() }, Source::Adf, None, None, limits).await;
    assert_eq!(view.state, JobState::Failed, "{view:?}");
    let err = view.error.as_ref().expect("error set");
    assert_eq!(err.code, "too_many_pages");
    assert!(err.message.contains("more than 3 pages"), "{}", err.message);
    assert!(cap.docs.lock().unwrap().is_empty(), "no truncated document handed over");
    assert!(fake.cancelled.load(Ordering::SeqCst), "DELETE sent to the scanner");
}

/// Exactly the limit is fine (the device answers 404 after the last allowed page).
#[tokio::test]
async fn page_limit_exactly_reached_is_ok() {
    let limits = Limits { max_pages: 3, ..Limits::default() };
    let (fake, cap, view, _t) = run_job_limited(Fake { sheets: 3, ..Default::default() }, Source::Adf, None, None, limits).await;
    assert_eq!(view.state, JobState::Done, "{view:?}");
    assert_eq!(view.pages, 3);
    assert_eq!(cap.docs.lock().unwrap().len(), 1);
    assert!(!fake.cancelled.load(Ordering::SeqCst));
}

/// Review item 3: total byte budget per job (pages are held in memory until the PDF is built).
#[tokio::test]
async fn job_byte_budget_fails_the_job() {
    // room for two test pages, not for three
    let budget = (support::page_jpeg(0).len() + support::page_jpeg(1).len() + 10) as u64;
    let limits = Limits { max_job_bytes: budget, ..Limits::default() };
    let (fake, cap, view, _t) = run_job_limited(Fake { sheets: 5, ..Default::default() }, Source::Adf, None, None, limits).await;
    assert_eq!(view.state, JobState::Failed, "{view:?}");
    let err = view.error.as_ref().expect("error set");
    assert_eq!(err.code, "job_too_large");
    assert!(err.message.contains("MiB"), "{}", err.message);
    assert!(cap.docs.lock().unwrap().is_empty());
    assert!(fake.cancelled.load(Ordering::SeqCst), "DELETE sent to the scanner");
    assert!(fake.delivered.load(Ordering::SeqCst) <= 3, "stopped right after the budget was exceeded");
}

/// Review item 2: a cancel confirmed while the last NextDocument request is still running must
/// not end with an imported document.
#[tokio::test]
async fn cancel_after_last_page_discards_the_scan() {
    let (_fake, client) = start_fake(Fake { sheets: 2, done_delay_ms: 800, ..Default::default() }).await;
    let caps = client.capabilities().await.unwrap();
    let settings = ScanSettings::resolve(&caps, Source::Adf, None, None).unwrap();
    let cap = Arc::new(Captured::default());
    let scratch = tempfile::tempdir().unwrap();
    let reg = Registry::default();
    let job = reg
        .start(JobSpec {
            scanner_id: 3,
            client,
            settings,
            user_id: 1,
            title: None,
            scratch: scratch.path().to_path_buf(),
            intake: cap.clone(),
            limits: Limits::default(),
        })
        .unwrap();
    // both pages are in; the final NextDocument (404) is delayed by the device
    for _ in 0..200 {
        if job.view().pages == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(job.view().pages, 2);
    assert!(job.cancel(), "cancel accepted while the job runs");
    let view = wait(&job).await;
    assert_eq!(view.state, JobState::Cancelled, "{view:?}");
    assert!(cap.docs.lock().unwrap().is_empty(), "no document after a confirmed cancel");
    assert!(view.task_id.is_none());
    // nothing left behind in the scratch directory
    assert_eq!(std::fs::read_dir(scratch.path()).map(|d| d.count()).unwrap_or(0), 0);
}

/// Hand-over that waits until the test releases it.
struct Gate {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    inner: Captured,
}

impl Intake for Gate {
    fn submit(&self, doc: IntakeDoc) -> BoxFuture<'_, Result<String, String>> {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
            self.inner.submit(doc).await
        })
    }
}

/// Once the hand-over has started, a cancel is refused (`cancel_requested: false`), so the API
/// never confirms a cancel for a document that gets imported.
#[tokio::test]
async fn cancel_during_handover_is_refused() {
    let (_fake, client) = start_fake(Fake { sheets: 1, ..Default::default() }).await;
    let caps = client.capabilities().await.unwrap();
    let settings = ScanSettings::resolve(&caps, Source::Adf, None, None).unwrap();
    let gate = Arc::new(Gate { entered: Default::default(), release: Default::default(), inner: Captured::default() });
    let scratch = tempfile::tempdir().unwrap();
    let reg = Registry::default();
    let job = reg
        .start(JobSpec {
            scanner_id: 4,
            client,
            settings,
            user_id: 1,
            title: None,
            scratch: scratch.path().to_path_buf(),
            intake: gate.clone(),
            limits: Limits::default(),
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), gate.entered.notified()).await.expect("hand-over started");
    assert_eq!(job.view().state, JobState::Processing);
    assert!(!job.cancel(), "too late: the document is being queued");
    gate.release.notify_one();
    let view = wait(&job).await;
    assert_eq!(view.state, JobState::Done, "{view:?}");
    assert_eq!(gate.inner.docs.lock().unwrap().len(), 1);
}
