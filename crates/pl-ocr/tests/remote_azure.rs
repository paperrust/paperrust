//! E7: Remote OCR (Azure AI Document Intelligence) against a local mock server (127.0.0.1).
//! No third-party services, key is only a test value.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};

use pl_ocr::config::{RemoteOcrConfig, RemoteOcrMode};

const KEY: &str = "test-key-123";
const PDF: &[u8] = b"%PDF-1.7\n% mock archive\n";

#[derive(Default)]
struct Seen {
    requests: Vec<(String, String, Option<String>, String)>, // method, path, key header, body
    polls: usize,
}

/// Minimal HTTP/1.1 server: every connection one request (`Connection: close`).
fn serve(op_location_host: Option<String>) -> (String, Arc<Mutex<Seen>>) {
    serve_with(op_location_host, false)
}

/// `huge_pdf`: the archive PDF reports `Content-Length` above the maximum size (without body).
fn serve_with(op_location_host: Option<String>, huge_pdf: bool) -> (String, Arc<Mutex<Seen>>) {
    serve_listener(TcpListener::bind("127.0.0.1:0").unwrap(), op_location_host, huge_pdf)
}

fn serve_on(port: u16, op_location_host: Option<String>) -> (String, Arc<Mutex<Seen>>) {
    serve_listener(TcpListener::bind(("127.0.0.1", port)).unwrap(), op_location_host, false)
}

fn serve_listener(l: TcpListener, op_location_host: Option<String>, huge_pdf: bool) -> (String, Arc<Mutex<Seen>>) {
    let port = l.local_addr().unwrap().port();
    let base = format!("http://127.0.0.1:{port}");
    let seen = Arc::new(Mutex::new(Seen::default()));
    let s2 = seen.clone();
    let b2 = base.clone();
    std::thread::spawn(move || {
        for conn in l.incoming() {
            let Ok(c) = conn else { continue };
            handle(c, &b2, op_location_host.as_deref(), &s2, huge_pdf);
        }
    });
    (base, seen)
}

fn handle(mut c: TcpStream, base: &str, op_host: Option<&str>, seen: &Mutex<Seen>, huge_pdf: bool) {
    let mut r = BufReader::new(c.try_clone().unwrap());
    let mut line = String::new();
    if r.read_line(&mut line).is_err() {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    let mut len = 0usize;
    let mut key = None;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).is_err() || h == "\r\n" || h.is_empty() {
            break;
        }
        let (k, v) = h.split_once(':').unwrap_or(("", ""));
        let k = k.trim().to_ascii_lowercase();
        let v = v.trim().to_string();
        if k == "content-length" {
            len = v.parse().unwrap_or(0);
        }
        if k == "ocp-apim-subscription-key" {
            key = Some(v);
        }
    }
    let mut body = vec![0; len];
    let _ = r.read_exact(&mut body);
    let body = String::from_utf8_lossy(&body).into_owned();
    let mut g = seen.lock().unwrap();
    g.requests.push((method.clone(), path.clone(), key.clone(), body.clone()));
    let respond = |c: &mut TcpStream, status: &str, headers: &[(&str, String)], body: &[u8]| {
        let mut out = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
        for (k, v) in headers {
            out.push_str(&format!("{k}: {v}\r\n"));
        }
        out.push_str("\r\n");
        let _ = c.write_all(out.as_bytes());
        let _ = c.write_all(body);
    };
    if key.as_deref() != Some(KEY) {
        respond(&mut c, "401 Unauthorized", &[("Content-Type", "application/json".into())], br#"{"error":{"code":"401","message":"Access denied due to invalid subscription key."}}"#);
        return;
    }
    let api = "api-version=2024-11-30";
    if method == "POST" && path.starts_with("/documentintelligence/documentModels/prebuilt-read:analyze?") {
        assert!(path.contains(api) && path.contains("outputContentFormat=text") && path.contains("output=pdf"), "{path}");
        let host = op_host.map(str::to_string).unwrap_or_else(|| base.to_string());
        respond(
            &mut c,
            "202 Accepted",
            &[("Operation-Location", format!("{host}/documentintelligence/documentModels/prebuilt-read/analyzeResults/res-42?{api}"))],
            b"",
        );
    } else if method == "GET" && path == format!("/documentintelligence/documentModels/prebuilt-read/analyzeResults/res-42?{api}") {
        g.polls += 1;
        if g.polls == 1 {
            respond(&mut c, "200 OK", &[("Retry-After", "1".into()), ("Content-Type", "application/json".into())], br#"{"status":"running"}"#);
        } else {
            respond(
                &mut c,
                "200 OK",
                &[("Content-Type", "application/json".into())],
                br#"{"status":"succeeded","analyzeResult":{"content":"Hallo Azure\nZeile 2"}}"#,
            );
        }
    } else if method == "GET" && path == format!("/documentintelligence/documentModels/prebuilt-read/analyzeResults/res-42/pdf?{api}") {
        if huge_pdf {
            let _ = c.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", 65 * 1024 * 1024).as_bytes());
            let _ = c.write_all(b"%PDF");
            return;
        }
        respond(&mut c, "200 OK", &[("Content-Type", "application/pdf".into())], PDF);
    } else {
        respond(&mut c, "404 Not Found", &[], b"");
    }
}

fn cfg(endpoint: &str, key: &str, allow_internal: bool) -> RemoteOcrConfig {
    RemoteOcrConfig {
        engine: Some("azureai".into()),
        api_key: Some(key.into()),
        endpoint: Some(endpoint.into()),
        mode: RemoteOcrMode::Always,
        allow_internal_endpoints: allow_internal,
    }
}

fn no_local(_: &Path) -> Option<String> {
    panic!("local extraction must not run")
}

#[test]
fn azure_flow_text_and_archive() {
    let (base, seen) = serve(None);
    let tmp = tempfile::tempdir().unwrap();
    let doc = tmp.path().join("scan.png");
    std::fs::write(&doc, b"\x89PNG fake").unwrap();
    let out = pl_ocr::remote::parse(&cfg(&format!("{base}/"), KEY, true), &tmp.path().join("work"), &doc, "image/png", true, &no_local).unwrap();
    assert_eq!(out.text, "Hallo Azure\nZeile 2");
    let archive = out.archive_path.expect("archive");
    assert_eq!(std::fs::read(archive).unwrap(), PDF);
    let g = seen.lock().unwrap();
    assert_eq!(g.polls, 2);
    // body: base64 of the document
    let (_, _, _, body) = &g.requests[0];
    let v: serde_json::Value = serde_json::from_str(body).unwrap();
    use base64::Engine as _;
    assert_eq!(base64::engine::general_purpose::STANDARD.decode(v["base64Source"].as_str().unwrap()).unwrap(), b"\x89PNG fake");
    assert!(g.requests.iter().all(|(_, _, k, _)| k.as_deref() == Some(KEY)));
}

#[test]
fn azure_errors_are_parse_errors_without_key() {
    let (base, _) = serve(None);
    let tmp = tempfile::tempdir().unwrap();
    let doc = tmp.path().join("a.pdf");
    std::fs::write(&doc, b"%PDF-1.4").unwrap();
    let err = pl_ocr::remote::parse(&cfg(&base, "wrong-secret-key", true), tmp.path(), &doc, "application/pdf", true, &no_local).unwrap_err();
    let msg = err.message();
    assert_eq!(err.class_name(), "ParseError");
    assert!(msg.starts_with("Azure AI Vision parsing failed: (401) Access denied"), "{msg}");
    assert!(!msg.contains("wrong-secret-key"));
}

#[test]
fn internal_endpoint_blocked_when_not_allowed() {
    let (base, seen) = serve(None);
    let tmp = tempfile::tempdir().unwrap();
    let doc = tmp.path().join("a.png");
    std::fs::write(&doc, b"x").unwrap();
    let err = pl_ocr::remote::parse(&cfg(&base, KEY, false), tmp.path(), &doc, "image/png", true, &no_local).unwrap_err();
    assert_eq!(err.message(), "Invalid remote OCR endpoint: Connection blocked: 127.0.0.1 resolves to a non-public address");
    assert!(seen.lock().unwrap().requests.is_empty());
}

#[test]
fn operation_location_on_other_host_is_refused() {
    let (base, seen) = serve(Some("http://localhost:1".into()));
    let tmp = tempfile::tempdir().unwrap();
    let doc = tmp.path().join("a.png");
    std::fs::write(&doc, b"x").unwrap();
    let err = pl_ocr::remote::parse(&cfg(&base, KEY, true), tmp.path(), &doc, "image/png", true, &no_local).unwrap_err();
    assert!(err.message().contains("another host"), "{}", err.message());
    assert_eq!(seen.lock().unwrap().requests.len(), 1);
}

/// Security review 2026-11-19 F10: the connection goes to exactly the checked address
/// (pinned), not to a new DNS resolution. `.invalid` never resolves; the call only succeeds
/// if the client uses the pinned address 127.0.0.1.
#[test]
fn connection_goes_to_pinned_address() {
    // Reserve the port beforehand so that Operation-Location can name the host name including port.
    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let host = "ocr-pin-test.invalid";
    let (base, seen) = serve_on(port, Some(format!("http://{host}:{port}")));
    assert!(base.ends_with(&format!(":{port}")));
    let endpoint = format!("http://{host}:{port}");
    let pin = (host.to_string(), std::net::SocketAddr::from(([127, 0, 0, 1], port)));
    let (text, pdf) = pl_ocr::remote::azure_analyze_pinned(&endpoint, KEY, b"x".to_vec(), Some(pin)).unwrap();
    assert_eq!(text, "Hallo Azure
Zeile 2");
    assert_eq!(pdf, PDF);
    assert!(seen.lock().unwrap().requests.len() >= 3);
}

/// Security review 2026-11-19 F10: an oversized response is aborted with a clear message.
#[test]
fn oversized_response_is_refused() {
    let (base, _) = serve_with(None, true);
    let tmp = tempfile::tempdir().unwrap();
    let doc = tmp.path().join("a.png");
    std::fs::write(&doc, b"x").unwrap();
    let err = pl_ocr::remote::parse(&cfg(&base, KEY, true), &tmp.path().join("w"), &doc, "image/png", true, &no_local).unwrap_err();
    assert!(err.message().contains("response from Azure exceeds"), "{}", err.message());
    assert_eq!(pl_ocr::remote::MAX_RESPONSE_BYTES, 64 * 1024 * 1024);
}

/// Security review 2026-11-19 F10: the checked DNS name yields the address to be pinned.
#[test]
fn validate_and_pin_returns_address() {
    let (_, pin) = pl_ocr::remote::validate_and_pin("http://localhost:1/", true).unwrap();
    assert!(pin.is_none(), "allow_internal: keine Prüfung, kein Festnageln");
    let (_, pin) = pl_ocr::remote::validate_and_pin("http://8.8.8.8/", false).unwrap();
    assert!(pin.is_none(), "IP-Literal braucht kein Festnageln");
    assert!(pl_ocr::remote::validate_and_pin("http://localhost:1/", false).is_err());
}

#[test]
fn decisions_without_azure_call() {
    let tmp = tempfile::tempdir().unwrap();
    let doc = tmp.path().join("a.pdf");
    std::fs::write(&doc, b"%PDF-1.4").unwrap();
    // PDF without archive request → local text
    let local = |_: &Path| Some("  lokaler   Text \n".to_string());
    let out = pl_ocr::remote::parse(&cfg("http://127.0.0.1:9", KEY, true), tmp.path(), &doc, "application/pdf", false, &local).unwrap();
    assert_eq!(out.text, "lokaler Text");
    assert!(out.archive_path.is_none());
    // No valid engine → empty
    let mut c = cfg("http://127.0.0.1:9", KEY, true);
    c.endpoint = None;
    let out = pl_ocr::remote::parse(&c, tmp.path(), &doc, "image/png", true, &no_local).unwrap();
    assert_eq!(out, pl_ocr::remote::RemoteOutcome::default());
}
