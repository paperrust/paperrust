//! Write parity Django ↔ Rust for pl-documents-write (own reference copy, see
//! `docs/contract/pl-documents-write.md`, section parity tests).
//!
//! Per case: reset → Django (127.0.0.1:8130) → capture state → reset → Rust (in-process) →
//! capture state → compare. State = HTTP response + DB rows (incl. auditlog,
//! PaperlessTask) + file tree under MEDIA_ROOT (path + SHA-256).
//!
//! ```sh
//! source tests/parity/pl-documents-write/w4env.sh
//! tests/parity/pl-documents-write/django-start.sh
//! cargo test -p pl-documents-write -j 1 --test write_parity -- --ignored --nocapture
//! # only specific cases: W4_CASES=name1,name2
//! ```

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::Request;
use chrono::{DateTime, NaiveDateTime, Utc};
use pl_authz::{AuthConfig, Authz};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::Connection;
use tower::ServiceExt;

const DJANGO: &str = "http://127.0.0.1:8130";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn w4_dir() -> PathBuf {
    repo().join("tests/parity/pl-documents-write")
}

fn token(user: &str) -> Option<String> {
    let t = match user {
        "admin" => "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001",
        "alice" => "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002",
        "bob" => "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b003",
        "carol" => "c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c004",
        "dave" => "d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d005",
        _ => return None,
    };
    Some(format!("Token {t}"))
}

fn reset() {
    let st =
        std::process::Command::new("bash").arg(w4_dir().join("reset.sh")).stdout(std::process::Stdio::null()).status().expect("reset.sh");
    assert!(st.success(), "reset failed");
}

fn db_url() -> String {
    format!(
        "postgres://{}:{}@{}:{}/{}",
        std::env::var("PAPERLESS_DBUSER").unwrap_or_else(|_| "paperless".into()),
        std::env::var("PAPERLESS_DBPASS").unwrap_or_else(|_| "paperless".into()),
        std::env::var("PAPERLESS_DBHOST").unwrap_or_else(|_| "127.0.0.1".into()),
        std::env::var("PAPERLESS_DBPORT").unwrap_or_else(|_| "54329".into()),
        std::env::var("PAPERLESS_DBNAME").unwrap_or_else(|_| "pl_w4_ref".into()),
    )
}

async fn run_setup(sql: &str) {
    if sql.is_empty() {
        return;
    }
    let mut c = sqlx::PgConnection::connect(&db_url()).await.expect("db");
    sqlx::raw_sql(sql).execute(&mut c).await.expect("setup sql");
}

/// Tables and their sorting; `drop` = columns that are not compared.
const TABLES: &[(&str, &str, &[&str])] = &[
    ("documents_document", "id", &[]),
    ("documents_document_tags", "document_id, tag_id", &["id"]),
    ("documents_customfieldinstance", "id", &[]),
    ("documents_note", "id", &[]),
    ("documents_sharelink", "id", &[]),
    ("documents_workflowrun", "id", &[]),
    ("documents_documentbarcode", "id", &[]),
    ("guardian_userobjectpermission", "object_pk, permission_id, user_id", &["id"]),
    ("guardian_groupobjectpermission", "object_pk, permission_id, group_id", &["id"]),
    ("auditlog_logentry", "id", &["id"]),
    ("documents_paperlesstask", "id", &["id"]),
];

struct Norm {
    t0: DateTime<Utc>,
    uuids: HashMap<String, String>,
}

impl Norm {
    fn ts(&self, s: &str) -> Option<String> {
        let parsed: Option<DateTime<Utc>> = DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Utc))
            .or_else(|| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f").ok().map(|n| n.and_utc()))
            .or_else(|| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").ok().map(|n| n.and_utc()));
        parsed.filter(|d| *d >= self.t0).map(|_| "<NOW>".to_string())
    }
    fn uuid(&mut self, s: &str) -> Option<String> {
        if s.len() == 36 && uuid::Uuid::parse_str(s).is_ok() {
            let n = self.uuids.len();
            Some(self.uuids.entry(s.to_string()).or_insert_with(|| format!("<uuid{n}>")).clone())
        } else {
            None
        }
    }
    fn value(&mut self, v: &Value) -> Value {
        match v {
            Value::String(s) => {
                if let Some(t) = self.ts(s) {
                    return Value::String(t);
                }
                if let Some(u) = self.uuid(s) {
                    return Value::String(u);
                }
                Value::String(s.clone())
            }
            Value::Array(a) => Value::Array(a.iter().map(|x| self.value(x)).collect()),
            Value::Object(o) => {
                let mut m = Map::new();
                for (k, x) in o {
                    let mut nv = self.value(x);
                    // Windows clock (~1 ms) produces equal `created` values upstream → order
                    // of the custom fields (Meta.ordering created) is random there; sort by field.
                    if k == "custom_fields"
                        && let Value::Array(items) = &mut nv
                        && items.iter().all(|i| i.get("field").is_some())
                    {
                        items.sort_by_key(|i| i.get("field").and_then(Value::as_i64).unwrap_or(0));
                    }
                    if (k == "filename" || k == "archive_filename")
                        && let Value::String(s) = &nv
                    {
                        // upstream on Windows stores str(Path) with backslash
                        nv = Value::String(s.replace('\\', "/"));
                    }
                    m.insert(k.clone(), nv);
                }
                Value::Object(m)
            }
            other => other.clone(),
        }
    }
}

async fn capture_db(norm: &mut Norm) -> BTreeMap<String, Value> {
    let mut c = sqlx::PgConnection::connect(&db_url()).await.expect("db");
    let mut out = BTreeMap::new();
    for (table, order, drop) in TABLES {
        let sql = format!("SELECT to_jsonb(t) FROM {table} t ORDER BY {order}");
        let rows: Vec<Value> = sqlx::query_scalar(&sql).fetch_all(&mut c).await.expect("capture");
        let rows: Vec<Value> = rows
            .into_iter()
            .map(|mut r| {
                if let Some(o) = r.as_object_mut() {
                    for d in *drop {
                        o.remove(*d);
                    }
                }
                norm.value(&r)
            })
            .collect();
        out.insert((*table).to_string(), Value::Array(rows));
    }
    out
}

fn capture_files(root: &Path) -> BTreeMap<String, String> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.insert(format!("{}/", rel(base, &p)), "dir".into());
                walk(base, &p, out);
            } else {
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if name.ends_with(".lock") {
                    continue;
                }
                let h = std::fs::read(&p).map(|b| Sha256::digest(&b).iter().map(|x| format!("{x:02x}")).collect()).unwrap_or_default();
                out.insert(rel(base, &p), h);
            }
        }
    }
    fn rel(base: &Path, p: &Path) -> String {
        p.strip_prefix(base).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_default()
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

#[derive(Debug)]
struct Resp {
    status: u16,
    content_type: String,
    body: Value,
}

fn parse_body(ct: &str, text: &str, norm: &mut Norm) -> Value {
    if ct.starts_with("application/json") {
        match serde_json::from_str::<Value>(text) {
            Ok(v) => norm.value(&v),
            Err(_) => Value::String(text.to_string()),
        }
    } else if ct.starts_with("text/html") && text.len() > 200 {
        Value::String("<html page>".into())
    } else {
        Value::String(text.to_string())
    }
}

fn main_ct(h: Option<&str>) -> String {
    h.unwrap_or("").split(';').next().unwrap_or("").trim().to_string()
}

async fn django_call(case: &Value, norm: &mut Norm) -> Resp {
    let client = reqwest::Client::new();
    let method = reqwest::Method::from_bytes(case["method"].as_str().unwrap_or("GET").as_bytes()).expect("method");
    let mut rb = client.request(method, format!("{DJANGO}{}", case["path"].as_str().unwrap_or("/")));
    rb = rb.header("Accept", case["accept"].as_str().unwrap_or("application/json"));
    if let Some(t) = case["user"].as_str().and_then(token) {
        rb = rb.header("Authorization", t);
    }
    if let Some(b) = case.get("body") {
        rb = rb.header("Content-Type", "application/json").body(serde_json::to_vec(b).unwrap_or_default());
    }
    let r = rb.send().await.expect("django request");
    let status = r.status().as_u16();
    let ct = main_ct(r.headers().get("content-type").and_then(|v| v.to_str().ok()));
    let text = r.text().await.unwrap_or_default();
    Resp { status, body: parse_body(&ct, &text, norm), content_type: ct }
}

async fn rust_call(case: &Value, norm: &mut Norm) -> Resp {
    let state = pl_core::AppState::from_env().await.expect("state");
    let authz = Authz::new(state.db().clone(), AuthConfig::from_core(state.config()));
    let mut router: axum::Router<pl_core::AppState> = axum::Router::new();
    let mut by_path: BTreeMap<&str, axum::routing::MethodRouter<pl_core::AppState>> = BTreeMap::new();
    for r in pl_documents_write::routes() {
        let e = by_path.remove(r.path);
        by_path.insert(
            r.path,
            match e {
                Some(m) => m.merge(r.handler),
                None => r.handler,
            },
        );
    }
    for (p, h) in by_path {
        router = router.route(p, h);
    }
    let app: axum::Router = router.with_state(state).layer(axum::Extension(authz));
    let mut req = Request::builder()
        .method(case["method"].as_str().unwrap_or("GET"))
        .uri(case["path"].as_str().unwrap_or("/"))
        .header("host", "127.0.0.1:8130")
        .header("accept", case["accept"].as_str().unwrap_or("application/json"));
    if let Some(t) = case["user"].as_str().and_then(token) {
        req = req.header("authorization", t);
    }
    let body = match case.get("body") {
        Some(b) => {
            req = req.header("content-type", "application/json");
            Body::from(serde_json::to_vec(b).unwrap_or_default())
        }
        None => Body::empty(),
    };
    let mut req = req.body(body).expect("request");
    let peer: std::net::SocketAddr = "127.0.0.1:50000".parse().expect("addr");
    req.extensions_mut().insert(axum::extract::ConnectInfo(peer));
    let resp = app.oneshot(req).await.expect("rust response");
    let status = resp.status().as_u16();
    let delegated = pl_core::markers::is_delegation(&resp);
    let ct = main_ct(resp.headers().get("content-type").and_then(|v| v.to_str().ok()));
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes).to_string();
    if delegated {
        return Resp { status: 0, content_type: "delegated".into(), body: Value::String("DELEGATED".into()) };
    }
    Resp { status, body: parse_body(&ct, &text, norm), content_type: ct }
}

fn diff_values(path: &str, a: &Value, b: &Value, out: &mut Vec<String>) {
    if out.len() > 40 {
        return;
    }
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let xk: Vec<&String> = x.keys().collect();
            let yk: Vec<&String> = y.keys().collect();
            if xk != yk && !path.contains("changes") {
                out.push(format!("{path}: keys {xk:?} != {yk:?}"));
            }
            for (k, v) in x {
                match y.get(k) {
                    Some(w) => diff_values(&format!("{path}.{k}"), v, w, out),
                    None => out.push(format!("{path}.{k}: only django = {v}")),
                }
            }
            for (k, w) in y {
                if !x.contains_key(k) {
                    out.push(format!("{path}.{k}: only rust = {w}"));
                }
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                out.push(format!("{path}: len {} != {}", x.len(), y.len()));
            }
            for (i, (v, w)) in x.iter().zip(y.iter()).enumerate() {
                diff_values(&format!("{path}[{i}]"), v, w, out);
            }
            if x.len() != y.len() {
                let (long, who) = if x.len() > y.len() { (x, "django") } else { (y, "rust") };
                for v in long.iter().skip(x.len().min(y.len())).take(5) {
                    out.push(format!("{path}: extra {who}: {v}"));
                }
            }
        }
        _ => {
            if a != b {
                out.push(format!("{path}: django={a} rust={b}"));
            }
        }
    }
}

fn media_root() -> PathBuf {
    let raw = std::env::var("PAPERLESS_MEDIA_ROOT").expect("PAPERLESS_MEDIA_ROOT (source w4env.sh)");
    PathBuf::from(raw)
}

#[tokio::test]
#[ignore = "braucht die W4-Referenz-Kopie (tests/parity/pl-documents-write/django-start.sh) und w4env.sh"]
async fn write_paths_match_django() {
    assert_eq!(std::env::var("PAPERLESS_DBNAME").as_deref(), Ok("pl_w4_ref"), "source w4env.sh first");
    let cases: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(w4_dir().join("write_cases.json")).expect("write_cases.json")).expect("cases json");
    let only: Option<Vec<String>> = std::env::var("W4_CASES").ok().map(|s| s.split(',').map(str::to_string).collect());
    let t0 = Utc::now() - chrono::Duration::seconds(5);
    let mut report = Vec::new();
    let (mut same, mut different) = (0, 0);
    for case in &cases {
        let name = case["name"].as_str().unwrap_or("?").to_string();
        if only.as_ref().is_some_and(|o| !o.contains(&name)) {
            continue;
        }
        let setup = case["setup_sql"].as_str().unwrap_or("");
        // Django
        reset();
        run_setup(setup).await;
        let mut nd = Norm { t0, uuids: HashMap::new() };
        let d_resp = django_call(case, &mut nd).await;
        let d_db = capture_db(&mut nd).await;
        let d_files = capture_files(&media_root());
        // Rust
        reset();
        run_setup(setup).await;
        let mut nr = Norm { t0, uuids: HashMap::new() };
        let r_resp = rust_call(case, &mut nr).await;
        let r_db = capture_db(&mut nr).await;
        let r_files = capture_files(&media_root());

        if std::env::var("W4_DUMP").is_ok() {
            let out_dir = w4_dir().join("out");
            let _ = std::fs::create_dir_all(&out_dir);
            let tail = |db: &BTreeMap<String, Value>| {
                let mut m = Map::new();
                for (k, v) in db {
                    if let Value::Array(a) = v {
                        let n = a.len();
                        m.insert(k.clone(), Value::Array(a.iter().skip(n.saturating_sub(8)).cloned().collect()));
                    }
                }
                Value::Object(m)
            };
            let dump = json!({
                "django": {"status": d_resp.status, "body": d_resp.body, "db_tail": tail(&d_db)},
                "rust": {"status": r_resp.status, "body": r_resp.body, "db_tail": tail(&r_db)},
            });
            let _ = std::fs::write(out_dir.join(format!("{name}.json")), serde_json::to_string_pretty(&dump).unwrap_or_default());
        }
        let mut diffs = Vec::new();
        if d_resp.status != r_resp.status {
            diffs.push(format!("status django={} rust={}", d_resp.status, r_resp.status));
        }
        if d_resp.content_type != r_resp.content_type {
            diffs.push(format!("content-type django={} rust={}", d_resp.content_type, r_resp.content_type));
        }
        diff_values("body", &d_resp.body, &r_resp.body, &mut diffs);
        for (t, dv) in &d_db {
            let rv = r_db.get(t).cloned().unwrap_or(Value::Null);
            diff_values(&format!("db.{t}"), dv, &rv, &mut diffs);
        }
        let dfv = serde_json::to_value(&d_files).unwrap_or_default();
        let rfv = serde_json::to_value(&r_files).unwrap_or_default();
        diff_values("files", &dfv, &rfv, &mut diffs);
        if diffs.is_empty() {
            same += 1;
            println!("SAME  {name}");
        } else {
            different += 1;
            println!("DIFF  {name}");
            for d in diffs.iter().take(25) {
                println!("      {d}");
            }
        }
        report.push(json!({"name": name, "status": d_resp.status, "diffs": diffs}));
    }
    let out_dir = w4_dir().join("out");
    let _ = std::fs::create_dir_all(&out_dir);
    let _ = std::fs::write(
        out_dir.join("report.json"),
        serde_json::to_string_pretty(&json!({"same": same, "different": different, "cases": report})).unwrap_or_default(),
    );
    println!("same={same} different={different}");
    reset();
    assert_eq!(different, 0, "Unterschiede, siehe tests/parity/pl-documents-write/out/report.json");
}
