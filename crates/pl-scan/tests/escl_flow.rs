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
use pl_scan::job::{JobSpec, JobState, Registry};
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
