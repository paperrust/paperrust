//! Access log (`paperrust::access`): requests aborted by the client are logged too
//! (E2E 2026-11-08: a hanging `POST` left no line).
//! Own test file, because the test sets the global tracing subscriber.

use std::collections::HashMap;
use std::io::Write;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::Method;
use axum::routing::{get, post};
use pl_core::ApiRoute;
use pl_server::config::ServerConfig;
use pl_server::routing;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

mod support;
use support::test_state;

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);

impl Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Buf {
    fn lines(&self) -> Vec<String> {
        String::from_utf8_lossy(&self.0.lock().unwrap()).lines().map(str::to_owned).collect()
    }
}

fn install_log() -> Buf {
    let buf = Buf::default();
    let w = buf.clone();
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_env_filter("paperrust::access=info,pl_server=warn")
        .with_writer(move || w.clone())
        .init();
    buf
}

async fn start() -> SocketAddr {
    let m: HashMap<String, String> = HashMap::new();
    let cfg = ServerConfig::from_lookup(|k| m.get(k).cloned()).unwrap();
    // POST /slow/ answers only after 5 s, GET /fast/ immediately.
    let routes = vec![
        ApiRoute::new("t.fast", Method::GET, "/fast/", "test", get(|| async { "ok" })),
        ApiRoute::new(
            "t.slow",
            Method::POST,
            "/slow/",
            "test",
            post(|| async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                "ok"
            }),
        ),
    ];
    let plan = routing::plan(routes, &routing::parse_routes_toml("").unwrap()).unwrap();
    let app = pl_server::build_app(&cfg, plan, test_state()).unwrap();
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(pl_server::serve::serve(l, app, std::future::pending()));
    addr
}

async fn wait_line(buf: &Buf, needle: &str) -> String {
    for _ in 0..200 {
        if let Some(l) = buf.lines().into_iter().find(|l| l.contains(needle)) {
            return l;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no log line with {needle:?}; log:\n{}", buf.lines().join("\n"));
}

#[tokio::test]
async fn aborted_and_completed_requests_are_logged() {
    let log = install_log();
    let app = start().await;

    // 1. normal: Status, via
    let mut s = TcpStream::connect(app).await.unwrap();
    s.write_all(b"GET /fast/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").await.unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    let line = wait_line(&log, "path=/fast/").await;
    assert!(line.contains("status=200"), "{line}");
    assert!(line.contains("via=\"rust\""), "{line}");
    assert!(line.contains("outcome=\"ok\""), "{line}");
    assert!(!line.contains("upstream_ms"), "{line}");

    // 2. Client aborts while the handler is still working → line with 499 / client closed
    let mut s = TcpStream::connect(app).await.unwrap();
    s.write_all(b"POST /slow/ HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}").await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    drop(s);
    let line = wait_line(&log, "path=/slow/").await;
    assert!(line.contains("method=POST"), "{line}");
    assert!(line.contains("status=499"), "{line}");
    assert!(line.contains("via=\"rust\""), "{line}");
    assert!(line.contains("outcome=\"client closed\""), "{line}");
    // e2e/compare.mjs still reads the line
    assert!(regex::Regex::new(r#"method=(\w+) path=(\S+) status=(\d+) ms=\d+ via="(\w+)""#).unwrap().is_match(&line), "{line}");
}
