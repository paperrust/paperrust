//! `POST /api/documents/email/` success path against a local SMTP sink (no real mails).
//!
//! `#[ignore]`: needs the reference PostgreSQL (read only) and its media directory:
//! `PL_MAIL_TEST_DB=postgres://paperless:paperless@127.0.0.1:54329/paperless
//!  PL_MAIL_TEST_MEDIA=<repo>/reference/data/media cargo test -p pl-mail --test documents_email -- --ignored`.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use pl_authz::{AuthConfig, Authz};
use pl_core::AppState;
use pl_core::config::{Config, Env};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tower::ServiceExt;

const ADMIN_TOKEN: &str = "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001";
const USER2_TOKEN: &str = "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b003";

#[derive(Default, Clone, Debug)]
struct Log {
    commands: Vec<String>,
    data: Vec<Vec<u8>>,
}

async fn sink() -> (u16, Arc<Mutex<Log>>) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let log = Arc::new(Mutex::new(Log::default()));
    let log2 = log.clone();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            let log = log2.clone();
            tokio::spawn(async move {
                let (r, mut w) = s.into_split();
                let mut r = BufReader::new(r);
                w.write_all(b"220 sink ESMTP\r\n").await.unwrap();
                let mut line = String::new();
                loop {
                    line.clear();
                    if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let cmd = line.trim_end().to_string();
                    log.lock().unwrap().commands.push(cmd.clone());
                    let up = cmd.to_uppercase();
                    if up.starts_with("EHLO") {
                        w.write_all(b"250-sink\r\n250 SIZE 100000000\r\n").await.unwrap();
                    } else if up == "DATA" {
                        w.write_all(b"354 go\r\n").await.unwrap();
                        let mut data = Vec::new();
                        loop {
                            let mut b = [0u8; 4096];
                            let n = r.read(&mut b).await.unwrap();
                            data.extend_from_slice(&b[..n]);
                            if n == 0 || data.ends_with(b"\r\n.\r\n") {
                                break;
                            }
                        }
                        log.lock().unwrap().data.push(data);
                        w.write_all(b"250 queued\r\n").await.unwrap();
                    } else if up == "QUIT" {
                        w.write_all(b"221 bye\r\n").await.unwrap();
                        return;
                    } else {
                        w.write_all(b"250 ok\r\n").await.unwrap();
                    }
                }
            });
        }
    });
    (port, log)
}

async fn app(smtp_port: Option<u16>) -> Router {
    let url = std::env::var("PL_MAIL_TEST_DB").expect("PL_MAIL_TEST_DB");
    let media = std::env::var("PL_MAIL_TEST_MEDIA").expect("PL_MAIL_TEST_MEDIA");
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.expect("db");
    let port = smtp_port.map(|p| p.to_string()).unwrap_or_else(|| "1".into());
    let cfg = Config::from_env_map(Env::from_pairs([
        ("PAPERLESS_SECRET_KEY", "paperless-rs-reference-secret-key-local-only-0123456789"),
        ("PAPERLESS_MEDIA_ROOT", media.as_str()),
        ("PAPERLESS_TIME_ZONE", "Europe/Zurich"),
        ("PAPERLESS_EMAIL_HOST", "127.0.0.1"),
        ("PAPERLESS_EMAIL_PORT", port.as_str()),
        ("PAPERLESS_EMAIL_FROM", "paperless@example.org"),
    ]))
    .expect("config");
    let state = AppState::with_pool(cfg.clone(), pool.clone()).expect("state");
    let authz = Authz::new(pool, AuthConfig::from_core(&cfg));
    let mut r: Router<AppState> = Router::new();
    for route in pl_mail::documents_email::routes() {
        r = r.route(route.path, route.handler);
    }
    r.with_state(state).layer(axum::Extension(authz))
}

async fn post(app: &Router, path: &str, token: &str, ct: &str, body: &str) -> (u16, String) {
    let req = Request::post(path)
        .header("Host", "127.0.0.1:8000")
        .header("Authorization", format!("Token {token}"))
        .header("Content-Type", ct)
        .body(Body::from(body.to_string()))
        .unwrap();
    let r = app.clone().oneshot(req).await.unwrap();
    let s = r.status().as_u16();
    let b = axum::body::to_bytes(r.into_body(), 1 << 24).await.unwrap();
    (s, String::from_utf8_lossy(&b).into_owned())
}

#[tokio::test]
#[ignore = "braucht die Referenz-PostgreSQL (PL_MAIL_TEST_DB) und PL_MAIL_TEST_MEDIA"]
async fn sends_documents_to_sink() {
    let (port, log) = sink().await;
    let app = app(Some(port)).await;
    let body = r#"{"documents":[1,4],"addresses":"a@example.com, b@example.org","subject":"Zwei\nDokumente","message":"Hallo"}"#;
    let (s, b) = post(&app, "/api/documents/email/", ADMIN_TOKEN, "application/json", body).await;
    assert_eq!((s, b.as_str()), (200, r#"{"message":"Email sent"}"#));
    let l = log.lock().unwrap().clone();
    assert!(l.commands.contains(&"rcpt TO:<a@example.com>".to_string()), "{:?}", l.commands);
    assert!(l.commands.contains(&"rcpt TO:<b@example.org>".to_string()), "{:?}", l.commands);
    assert_eq!(l.data.len(), 1);
    let data = String::from_utf8_lossy(&l.data[0]);
    assert!(data.contains("Subject: Zwei Dokumente\r\n"), "{data}");
    assert_eq!(data.matches("Content-Disposition: attachment;").count(), 2, "{data}");

    // deprecated detail route with form body
    let (s, b) = post(
        &app,
        "/api/documents/1/email/",
        ADMIN_TOKEN,
        "application/x-www-form-urlencoded",
        "addresses=c%40example.com&subject=s&message=m&use_archive_version=false",
    )
    .await;
    assert_eq!((s, b.as_str()), (200, r#"{"message":"Email sent"}"#));
    // ... and JSON (upstream AttributeError → 500; PaperRust accepts it like the form body)
    let (s, b) = post(
        &app,
        "/api/documents/1/email/",
        ADMIN_TOKEN,
        "application/json",
        r#"{"addresses":"c@example.com","subject":"s","message":"m"}"#,
    )
    .await;
    assert_eq!((s, b.as_str()), (200, r#"{"message":"Email sent"}"#));
}

#[tokio::test]
#[ignore = "braucht die Referenz-PostgreSQL (PL_MAIL_TEST_DB) und PL_MAIL_TEST_MEDIA"]
async fn smtp_failure_and_permissions() {
    let app = app(None).await;
    let body = r#"{"documents":[1],"addresses":"a@example.com","subject":"s","message":"m"}"#;
    let (s, b) = post(&app, "/api/documents/email/", ADMIN_TOKEN, "application/json", body).await;
    // upstream 500; PaperRust 400 (B-313)
    assert_eq!((s, b.as_str()), (400, "Error emailing documents, check logs for more detail."));
    // document 2 belongs to alice; bob may not view it
    let body2 = r#"{"documents":[2],"addresses":"a@example.com","subject":"s","message":"m"}"#;
    let (s, b) = post(&app, "/api/documents/email/", USER2_TOKEN, "application/json", body2).await;
    assert_eq!((s, b.as_str()), (403, "Insufficient permissions"));
}
