//! Test scanner: small eSCL HTTP server on 127.0.0.1 (for escl_flow and api_db).
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use pl_scan::escl::{Client, normalize_base};

const CAPS: &str = include_str!("../data/caps_officejet.xml");

/// Behaviour of the test scanner.
#[derive(Default)]
pub struct Fake {
    /// sheets in the feeder
    pub sheets: usize,
    /// `503` this many times before each page
    pub not_ready_each: usize,
    /// jam after this many pages
    pub jam_after: Option<usize>,
    /// `POST ScanJobs` → 503
    pub busy_post: bool,
    /// ScannerStatus reports "Processing"
    pub busy_status: bool,
    /// feed empty
    pub adf_empty: bool,
    /// foreign host in Location
    pub evil_location: bool,
    /// wait this long before the final `404` (no more pages) – window for a late cancel
    pub done_delay_ms: u64,
    // State
    pub duplex: AtomicBool,
    pub feeder: AtomicBool,
    pub delivered: AtomicUsize,
    pub not_ready_left: AtomicUsize,
    pub jammed: AtomicBool,
    pub cancelled: AtomicBool,
    pub settings_xml: Mutex<String>,
}

type S = State<Arc<Fake>>;

async fn caps() -> Response {
    ([(header::CONTENT_TYPE, "text/xml")], CAPS).into_response()
}

async fn status(State(f): S) -> Response {
    let state = if f.busy_status { "Processing" } else { "Idle" };
    let adf = if f.jammed.load(Ordering::SeqCst) {
        "ScannerAdfJam"
    } else if f.adf_empty {
        "ScannerAdfEmpty"
    } else {
        "ScannerAdfLoaded"
    };
    let job = if f.jammed.load(Ordering::SeqCst) {
        "<scan:Jobs><scan:JobInfo><pwg:JobUri>/eSCL/ScanJobs/job1</pwg:JobUri><pwg:JobState>Aborted</pwg:JobState><pwg:JobStateReasons><pwg:JobStateReason>AdfJam</pwg:JobStateReason></pwg:JobStateReasons></scan:JobInfo></scan:Jobs>"
    } else {
        ""
    };
    let body = format!(
        r#"<?xml version="1.0"?><scan:ScannerStatus xmlns:scan="http://schemas.hp.com/imaging/escl/2011/05/03" xmlns:pwg="http://www.pwg.org/schemas/2010/12/sm"><pwg:Version>2.63</pwg:Version><pwg:State>{state}</pwg:State><scan:AdfState>{adf}</scan:AdfState>{job}</scan:ScannerStatus>"#
    );
    ([(header::CONTENT_TYPE, "text/xml")], body).into_response()
}

async fn create(State(f): S, body: String) -> Response {
    *f.settings_xml.lock().unwrap() = body.clone();
    if f.busy_post {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    f.duplex.store(body.contains("<scan:Duplex>true</scan:Duplex>"), Ordering::SeqCst);
    f.feeder.store(body.contains("<pwg:InputSource>Feeder</pwg:InputSource>"), Ordering::SeqCst);
    f.not_ready_left.store(f.not_ready_each, Ordering::SeqCst);
    let loc = if f.evil_location { "http://198.51.100.7/eSCL/ScanJobs/job1" } else { "/eSCL/ScanJobs/job1" };
    let mut h = HeaderMap::new();
    h.insert(header::LOCATION, loc.parse().unwrap());
    (StatusCode::CREATED, h).into_response()
}

pub fn page_jpeg(n: usize) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    // pages distinguishable: width = 100 + n
    let img = image::DynamicImage::ImageLuma8(image::GrayImage::from_pixel(100 + n as u32, 140, image::Luma([200])));
    img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
    out.into_inner()
}

async fn next(State(f): S, Path(_job): Path<String>) -> Response {
    if f.cancelled.load(Ordering::SeqCst) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if f.not_ready_left.load(Ordering::SeqCst) > 0 {
        f.not_ready_left.fetch_sub(1, Ordering::SeqCst);
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let delivered = f.delivered.load(Ordering::SeqCst);
    if let Some(j) = f.jam_after
        && delivered >= j
    {
        f.jammed.store(true, Ordering::SeqCst);
        return StatusCode::NOT_FOUND.into_response();
    }
    let total = if !f.feeder.load(Ordering::SeqCst) {
        usize::MAX // Flatbed: the client has to stop by itself
    } else if f.duplex.load(Ordering::SeqCst) {
        f.sheets * 2
    } else {
        f.sheets
    };
    if delivered >= total {
        if f.done_delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(f.done_delay_ms)).await;
        }
        return StatusCode::NOT_FOUND.into_response();
    }
    f.delivered.fetch_add(1, Ordering::SeqCst);
    f.not_ready_left.store(f.not_ready_each, Ordering::SeqCst);
    ([(header::CONTENT_TYPE, "image/jpeg")], page_jpeg(delivered)).into_response()
}

async fn cancel(State(f): S, Path(_job): Path<String>) -> Response {
    f.cancelled.store(true, Ordering::SeqCst);
    StatusCode::OK.into_response()
}

pub async fn start_fake(f: Fake) -> (Arc<Fake>, Client) {
    let f = Arc::new(f);
    let app = Router::new()
        .route("/eSCL/ScannerCapabilities", get(caps))
        .route("/eSCL/ScannerStatus", get(status))
        .route("/eSCL/ScanJobs", post(create))
        .route("/eSCL/ScanJobs/{job}/NextDocument", get(next))
        .route("/eSCL/ScanJobs/{job}", delete(cancel))
        .with_state(f.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    let client = Client::new(normalize_base(&format!("http://127.0.0.1:{port}/eSCL/")).unwrap()).unwrap();
    (f, client)
}

/// Base URL of the test scanner.
pub fn base_url(c: &Client) -> String {
    c.base().as_str().to_string()
}
