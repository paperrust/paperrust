//! End-to-end via the HTTP API `/api/x-scan/…` with a real database (permissions, tables, token)
//! and a recording queue (`consume_file` kwargs as in production):
//! register a scanner (test scanner) → scan button (feed, double-sided) → `consume_file` with
//! owner = user; create a device → SMTP intake with the token (PgDevices) → `consume_file` with
//! owner = device owner; revocation takes effect immediately.
//!
//! Creates a **dedicated throwaway database** from `paperless_snapshot` and drops it afterwards:
//! ```text
//! PAPERRUST_SCAN_TEST_ADMIN_URL=postgres://paperless:paperless@127.0.0.1:54329/postgres \
//!   cargo test -p pl-scan --test api_db -j 2 -- --ignored --nocapture
//! ```

mod support;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::MethodRouter;
use base64::Engine as _;
use futures::future::BoxFuture;
use pl_core::AppState;
use pl_core::config::{Config, Env};
use pl_queue::{EnqueueOptions, QueueError, TaskInfo};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tower::ServiceExt;

const SU: &str = "plscanapitestsu0000000000000000000000000";
const ADDER: &str = "plscanapitestad0000000000000000000000000";
const VIEWER: &str = "plscanapitestvw0000000000000000000000000";

type Call = (String, Map<String, Value>, Option<String>);

#[derive(Default)]
struct RecordingQueue {
    calls: Mutex<Vec<Call>>,
}

impl pl_queue::TaskQueue for RecordingQueue {
    fn enqueue<'a>(&'a self, task_name: &'a str, kwargs: Map<String, Value>, opts: EnqueueOptions) -> BoxFuture<'a, Result<uuid::Uuid, QueueError>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push((task_name.to_string(), kwargs, opts.trigger_source.clone()));
            Ok(uuid::Uuid::new_v4())
        })
    }
    fn status(&self, _task_id: uuid::Uuid) -> BoxFuture<'_, Result<Option<TaskInfo>, QueueError>> {
        Box::pin(async { Ok(None) })
    }
}

async fn user(pool: &sqlx::PgPool, name: &str, su: bool, perms: &[&str], token: &str) -> i64 {
    let id: i32 = sqlx::query_scalar(
        "INSERT INTO auth_user (password, is_superuser, username, first_name, last_name, email, is_staff, is_active, date_joined)
         VALUES ('!', $1, $2, '', '', '', $1, true, now()) RETURNING id",
    )
    .bind(su)
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap();
    for p in perms {
        sqlx::query(
            "INSERT INTO auth_user_user_permissions (user_id, permission_id)
             SELECT $1, p.id FROM auth_permission p JOIN django_content_type ct ON ct.id = p.content_type_id
             WHERE ct.app_label = 'documents' AND p.codename = $2",
        )
        .bind(id)
        .bind(p)
        .execute(pool)
        .await
        .unwrap();
    }
    sqlx::query("INSERT INTO authtoken_token (key, created, user_id) VALUES ($1, now(), $2)").bind(token).bind(id).execute(pool).await.unwrap();
    i64::from(id)
}

fn router(state: AppState, pool: sqlx::PgPool, cfg: &Config) -> Router {
    let mut grouped: BTreeMap<&'static str, MethodRouter<AppState>> = BTreeMap::new();
    for r in pl_scan::routes() {
        let e = grouped.remove(r.path).unwrap_or_default();
        grouped.insert(r.path, e.merge(r.handler));
    }
    let mut router: Router<AppState> = Router::new();
    for (p, mr) in grouped {
        router = router.route(p, mr);
    }
    let authz = pl_authz::Authz::new(pool, pl_authz::AuthConfig::from_core(cfg));
    router.with_state(state).layer(axum::Extension(authz))
}

async fn call(r: &Router, method: &str, uri: &str, token: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri).header("host", "localhost");
    if let Some(t) = token {
        b = b.header("authorization", format!("Token {t}"));
    }
    let body = match body {
        Some(v) => {
            b = b.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = r.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let st = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 24).await.unwrap();
    (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn smtp_reply(io: &mut BufReader<tokio::net::TcpStream>) -> String {
    let mut all = String::new();
    loop {
        let mut l = String::new();
        if io.read_line(&mut l).await.unwrap_or(0) == 0 {
            return all;
        }
        all.push_str(&l);
        if l.len() < 4 || l.as_bytes()[3] == b' ' {
            return all;
        }
    }
}

async fn smtp(io: &mut BufReader<tokio::net::TcpStream>, line: &str) -> String {
    io.get_mut().write_all(format!("{line}\r\n").as_bytes()).await.unwrap();
    smtp_reply(io).await
}

/// Send a message with a PDF attachment via SMTP → response to AUTH and to DATA.
async fn send_scan_mail(addr: std::net::SocketAddr, token: &str) -> (String, String) {
    let mut io = BufReader::new(tokio::net::TcpStream::connect(addr).await.unwrap());
    assert!(smtp_reply(&mut io).await.starts_with("220"));
    assert!(smtp(&mut io, "EHLO hp-officejet").await.starts_with("250"));
    let auth = smtp(&mut io, &format!("AUTH PLAIN {}", base64::engine::general_purpose::STANDARD.encode(format!("\0hp\0{token}")))).await;
    if !auth.starts_with("235") {
        return (auth, String::new());
    }
    assert!(smtp(&mut io, "MAIL FROM:<hp@printer>").await.starts_with("250"));
    assert!(smtp(&mut io, "RCPT TO:<paperrust@nas>").await.starts_with("250"));
    assert!(smtp(&mut io, "DATA").await.starts_with("354"));
    let pdf = base64::engine::general_purpose::STANDARD.encode(b"%PDF-1.4\n%%EOF\n");
    let msg = format!(
        "Subject: Scan\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=B\r\n\r\n--B\r\nContent-Type: application/pdf; name=\"scan0001.pdf\"\r\nContent-Disposition: attachment; filename=\"scan0001.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{pdf}\r\n--B--\r\n.\r\n"
    );
    io.get_mut().write_all(msg.as_bytes()).await.unwrap();
    let data = smtp_reply(&mut io).await;
    let _ = smtp(&mut io, "QUIT").await;
    (auth, data)
}

#[tokio::test]
#[ignore = "braucht PostgreSQL (PAPERRUST_SCAN_TEST_ADMIN_URL), legt eine Wegwerf-DB an"]
async fn scan_and_smtp_end_to_end() {
    let admin_url = std::env::var("PAPERRUST_SCAN_TEST_ADMIN_URL").expect("PAPERRUST_SCAN_TEST_ADMIN_URL");
    assert!(admin_url.contains("127.0.0.1") || admin_url.contains("localhost"), "local test databases only");
    let db = format!("pl_scan_e2e_{}", std::process::id());
    let admin = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(&admin_url).await.expect("admin connect");
    sqlx::query(&format!("DROP DATABASE IF EXISTS {db}")).execute(&admin).await.unwrap();
    sqlx::query(&format!("CREATE DATABASE {db} TEMPLATE paperless_snapshot")).execute(&admin).await.expect("create test db");
    let url = format!("{}/{db}", admin_url.trim_end_matches('/').rsplit_once('/').unwrap().0);
    let result = tokio::spawn(run(url)).await;
    // always clean up
    sqlx::query(&format!("DROP DATABASE IF EXISTS {db} WITH (FORCE)")).execute(&admin).await.expect("drop test db");
    if let Err(e) = result {
        std::panic::resume_unwind(e.into_panic());
    }
}

async fn run(url: String) {
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(4).connect(&url).await.expect("connect");
    let su = user(&pool, "pl_scan_su", true, &[], SU).await;
    let adder = user(&pool, "pl_scan_adder", false, &["view_document", "add_document"], ADDER).await;
    let viewer = user(&pool, "pl_scan_viewer", false, &["view_document"], VIEWER).await;
    let scratch = tempfile::tempdir().unwrap();
    let cfg = Config::from_env_map(Env::from_pairs([
        ("PAPERLESS_SECRET_KEY", "pl-scan-api-tests-only-0123456789abcdefghijklmnopqrstu"),
        ("PAPERLESS_DBHOST", "127.0.0.1"),
        ("PAPERLESS_SCRATCH_DIR", scratch.path().to_str().unwrap()),
    ]))
    .expect("config");
    let state = AppState::with_pool(cfg.clone(), pool.clone()).expect("state");
    let queue = Arc::new(RecordingQueue::default());
    state.set_service::<Arc<dyn pl_queue::TaskQueue>>(queue.clone());
    let r = router(state.clone(), pool.clone(), &cfg);

    // --- Permissions ---
    assert_eq!(call(&r, "GET", "/api/x-scan/scanners/", None, None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&r, "GET", "/api/x-scan/scanners/", Some(VIEWER), None).await.0, StatusCode::FORBIDDEN);
    let (st, v) = call(&r, "GET", "/api/x-scan/scanners/", Some(ADDER), None).await;
    assert_eq!((st, v["count"].clone()), (StatusCode::OK, json!(0)));
    assert_eq!(call(&r, "GET", "/api/x-scan/devices/", Some(ADDER), None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&r, "GET", "/api/x-scan/discover/", Some(ADDER), None).await.0, StatusCode::FORBIDDEN);

    // --- Register a scanner (superuser only, address is checked) ---
    let (_fake, client) = support::start_fake(support::Fake { sheets: 2, ..Default::default() }).await;
    let base = support::base_url(&client);
    assert_eq!(call(&r, "POST", "/api/x-scan/scanners/", Some(ADDER), Some(json!({"url": base}))).await.0, StatusCode::FORBIDDEN);
    let (st, v) = call(&r, "POST", "/api/x-scan/scanners/", Some(SU), Some(json!({"url": "127.0.0.1:9"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    let (st, s) = call(&r, "POST", "/api/x-scan/scanners/", Some(SU), Some(json!({"url": base}))).await;
    assert_eq!(st, StatusCode::CREATED, "{s}");
    assert_eq!(s["name"], "HP OfficeJet Pro 9010 series");
    let sid = s["id"].as_i64().unwrap();
    assert_eq!(call(&r, "POST", "/api/x-scan/scanners/", Some(SU), Some(json!({"url": base}))).await.0, StatusCode::BAD_REQUEST, "duplicate");
    // users do not see addresses
    let (_, v) = call(&r, "GET", "/api/x-scan/scanners/", Some(ADDER), None).await;
    assert!(v["results"][0].get("base_url").is_none(), "{v}");
    let (_, v) = call(&r, "GET", "/api/x-scan/scanners/", Some(SU), None).await;
    assert_eq!(v["results"][0]["base_url"], json!(base));

    // --- Capabilities + scan button ---
    let (st, caps) = call(&r, "GET", &format!("/api/x-scan/scanners/{sid}/capabilities/"), Some(ADDER), None).await;
    assert_eq!(st, StatusCode::OK, "{caps}");
    assert_eq!(caps["sources"].as_array().unwrap().len(), 3);
    let (st, v) = call(&r, "POST", "/api/x-scan/jobs/", Some(VIEWER), Some(json!({"scanner": sid}))).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{v}");
    let (st, v) = call(&r, "POST", "/api/x-scan/jobs/", Some(ADDER), Some(json!({"scanner": sid, "source": "flatbed", "duplex": true}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    let (st, v) = call(&r, "POST", "/api/x-scan/jobs/", Some(ADDER), Some(json!({"scanner": sid, "source": "adf", "resolution": 9999}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    let (st, job) = call(&r, "POST", "/api/x-scan/jobs/", Some(ADDER), Some(json!({"scanner": sid, "source": "adf", "duplex": true, "resolution": 200, "color_mode": "grayscale", "title": "Brief"}))).await;
    assert_eq!(st, StatusCode::CREATED, "{job}");
    let jid = job["id"].as_str().unwrap().to_string();
    let mut view = job.clone();
    for _ in 0..200 {
        view = call(&r, "GET", &format!("/api/x-scan/jobs/{jid}/"), Some(ADDER), None).await.1;
        if ["done", "failed", "cancelled"].contains(&view["state"].as_str().unwrap_or("")) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(view["state"], "done", "{view}");
    assert_eq!(view["pages"], 4);
    // a different user does not see the job, a superuser does
    assert_eq!(call(&r, "GET", &format!("/api/x-scan/jobs/{jid}/"), Some(VIEWER), None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&r, "GET", &format!("/api/x-scan/jobs/{jid}/"), Some(SU), None).await.0, StatusCode::OK);
    {
        let calls = queue.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        let (task, kw, trig) = &calls[0];
        assert_eq!(task, "documents.tasks.consume_file");
        assert_eq!(trig.as_deref(), Some("web_ui"));
        assert_eq!(kw["overrides"]["owner_id"], json!(adder), "owned by the user who pressed the button");
        assert_eq!(kw["overrides"]["title"], "Brief");
        assert_eq!(kw["input_doc"]["mime_type"], "application/pdf");
        let path = kw["input_doc"]["original_file"].as_str().unwrap();
        let doc = lopdf::Document::load(path).expect("spooled PDF");
        assert_eq!(doc.get_pages().len(), 4);
        assert!(std::path::Path::new(path).starts_with(scratch.path()));
    }

    // --- Device tokens ---
    assert_eq!(call(&r, "POST", "/api/x-scan/devices/", Some(ADDER), Some(json!({"name": "HP", "owner": adder}))).await.0, StatusCode::FORBIDDEN);
    let (st, v) = call(&r, "POST", "/api/x-scan/devices/", Some(SU), Some(json!({"name": "HP", "owner": viewer}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "owner without add_document: {v}");
    let (st, v) = call(&r, "POST", "/api/x-scan/devices/", Some(SU), Some(json!({"name": "HP", "owner": adder, "allowed_ips": "nope"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{v}");
    let (st, dev) = call(&r, "POST", "/api/x-scan/devices/", Some(SU), Some(json!({"name": "HP OfficeJet Pro 9010", "owner": adder, "allowed_ips": "127.0.0.1"}))).await;
    assert_eq!(st, StatusCode::CREATED, "{dev}");
    let token = dev["token"].as_str().unwrap().to_string();
    assert!(pl_scan::token::is_well_formed(&token));
    let did = dev["id"].as_i64().unwrap();
    // stored only as a hash
    let stored: String = sqlx::query_scalar("SELECT token_hash FROM paperrust_scan_device WHERE id = $1").bind(did).fetch_one(&pool).await.unwrap();
    assert_eq!(stored, pl_scan::token::hash(&token));
    let raw_hits: i64 = sqlx::query_scalar("SELECT count(*) FROM paperrust_scan_device WHERE row_to_json(paperrust_scan_device)::text LIKE '%' || $1 || '%'")
        .bind(&token)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(raw_hits, 0, "plain token must not be stored");
    let (_, list) = call(&r, "GET", "/api/x-scan/devices/", Some(SU), None).await;
    assert!(list["results"][0].get("token").is_none() && list["results"][0].get("token_hash").is_none(), "{list}");
    assert_eq!(list["smtp"]["enabled"], false);

    // --- SMTP intake with real device storage ---
    let smtp_cfg = pl_scan::smtp::SmtpConfig::from_lookup(|k| (k == pl_scan::smtp::SETTING_ENABLED).then(|| "on".into()), scratch.path().to_path_buf()).unwrap().unwrap();
    let ctx = Arc::new(pl_scan::smtp::Context {
        cfg: smtp_cfg,
        devices: Arc::new(pl_scan::store::PgDevices(pool.clone())),
        intake: Arc::new(pl_scan::intake::QueueIntake(queue.clone())),
        limits: pl_scan::smtp::Limits::default(),
    });
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(pl_scan::smtp::serve(l, ctx, stop_rx));
    let (auth, data) = send_scan_mail(addr, &token).await;
    assert!(auth.starts_with("235"), "{auth}");
    assert!(data.starts_with("250"), "{data}");
    {
        let calls = queue.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        let (_, kw, trig) = &calls[1];
        assert_eq!(trig.as_deref(), Some("api_upload"));
        assert_eq!(kw["overrides"]["owner_id"], json!(adder));
        assert_eq!(kw["overrides"]["filename"], "scan0001.pdf");
    }
    let (n, ip): (i64, Option<String>) = sqlx::query_as("SELECT documents_received, last_ip FROM paperrust_scan_device WHERE id = $1").bind(did).fetch_one(&pool).await.unwrap();
    assert_eq!((n, ip.as_deref()), (1, Some("127.0.0.1")));

    // --- Revocation takes effect immediately ---
    assert_eq!(call(&r, "DELETE", &format!("/api/x-scan/devices/{did}/"), Some(SU), None).await.0, StatusCode::NOT_FOUND, "only revoked devices can be deleted");
    assert_eq!(call(&r, "POST", &format!("/api/x-scan/devices/{did}/revoke/"), Some(SU), None).await.0, StatusCode::OK);
    let (auth, _) = send_scan_mail(addr, &token).await;
    assert!(auth.starts_with("535"), "revoked token: {auth}");
    assert_eq!(queue.calls.lock().unwrap().len(), 2);
    assert_eq!(call(&r, "DELETE", &format!("/api/x-scan/devices/{did}/"), Some(SU), None).await.0, StatusCode::NO_CONTENT);

    // --- deactivated owner: token ineffective ---
    let (_, dev2) = call(&r, "POST", "/api/x-scan/devices/", Some(SU), Some(json!({"name": "Zweites", "owner": adder}))).await;
    let t2 = dev2["token"].as_str().unwrap().to_string();
    sqlx::query("UPDATE auth_user SET is_active = false WHERE id = $1").bind(adder as i32).execute(&pool).await.unwrap();
    let (auth, _) = send_scan_mail(addr, &t2).await;
    assert!(auth.starts_with("535"), "inactive owner: {auth}");

    // --- Rename/remove scanner ---
    let (st, v) = call(&r, "PATCH", &format!("/api/x-scan/scanners/{sid}/"), Some(SU), Some(json!({"name": "Büro"}))).await;
    assert_eq!((st, v["name"].clone()), (StatusCode::OK, json!("Büro")));
    assert_eq!(call(&r, "DELETE", &format!("/api/x-scan/scanners/{sid}/"), Some(SU), None).await.0, StatusCode::NO_CONTENT);
    assert_eq!(call(&r, "DELETE", &format!("/api/x-scan/scanners/{sid}/"), Some(SU), None).await.0, StatusCode::NOT_FOUND);

    let _ = stop_tx.send(true);
    let _ = server.await;
    let _ = su;
    pool.close().await;
}
