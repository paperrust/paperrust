//! PDF parity Django ↔ Rust for the PDF methods of bulk_edit (W13, own reference copy
//! `pl_w13_ref`, see `docs/contract/pl-documents-write.md`, section "PDF methods").
//!
//! Per case: reset → (file setup) → Django (127.0.0.1:8150, overlay records Celery tasks)
//! → capture state → reset → Rust (in-process, PgQueue) → capture state → compare.
//! State = HTTP response, DB rows (`documents_document`, `documents_paperlesstask`,
//! `auditlog_logentry`), enqueued tasks as a plan (single task with link/link_error,
//! group, chord with body/on_error; kwargs incl. `ConsumableDocument`/overrides) and the
//! generated PDFs (page count, rotation and text per page instead of bytes).
//!
//! ```sh
//! source tests/parity/pl-documents-write/w13/w13env.sh
//! tests/parity/pl-documents-write/w13/django-start.sh
//! cargo test -p pl-documents-write -j 1 --test pdf_parity -- --ignored --nocapture
//! # only specific cases: W13_CASES=name1,name2
//! ```

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use chrono::{DateTime, NaiveDateTime, Utc};
use pl_authz::{AuthConfig, Authz};
use serde_json::{Value, json};
use sqlx::Connection;
use tower::ServiceExt;

const DJANGO: &str = "http://127.0.0.1:8150";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn w13_dir() -> PathBuf {
    repo().join("tests/parity/pl-documents-write/w13")
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
    let st = std::process::Command::new("bash")
        .arg(w13_dir().join("reset.sh"))
        .stdout(std::process::Stdio::null())
        .status()
        .expect("reset.sh");
    assert!(st.success(), "reset failed");
}

fn db_url() -> String {
    format!(
        "postgres://{}:{}@{}:{}/{}",
        std::env::var("PAPERLESS_DBUSER").unwrap_or_else(|_| "paperless".into()),
        std::env::var("PAPERLESS_DBPASS").unwrap_or_else(|_| "paperless".into()),
        std::env::var("PAPERLESS_DBHOST").unwrap_or_else(|_| "127.0.0.1".into()),
        std::env::var("PAPERLESS_DBPORT").unwrap_or_else(|_| "54329".into()),
        std::env::var("PAPERLESS_DBNAME").unwrap_or_else(|_| "pl_w13_ref".into()),
    )
}

fn media_root() -> PathBuf {
    PathBuf::from(std::env::var("PAPERLESS_MEDIA_ROOT").expect("PAPERLESS_MEDIA_ROOT (source w13env.sh)"))
}

fn task_log() -> PathBuf {
    PathBuf::from(std::env::var("W13_TASK_LOG").expect("W13_TASK_LOG (source w13env.sh)"))
}

/// `setup_sql` and `setup_files` (`[{"from": "<repo-relative>", "doc": id}]`: replace the original).
async fn run_setup(case: &Value) {
    let mut c = sqlx::PgConnection::connect(&db_url()).await.expect("db");
    if let Some(sql) = case["setup_sql"].as_str() {
        sqlx::raw_sql(sql).execute(&mut c).await.expect("setup sql");
    }
    for f in case["setup_files"].as_array().cloned().unwrap_or_default() {
        let doc = f["doc"].as_i64().expect("doc");
        let name: Option<String> = sqlx::query_scalar("SELECT filename FROM documents_document WHERE id = $1")
            .bind(doc as i32)
            .fetch_one(&mut c)
            .await
            .expect("filename");
        let dst = media_root().join("documents/originals").join(name.expect("filename set"));
        std::fs::copy(repo().join(f["from"].as_str().expect("from")), &dst).expect("copy setup file");
    }
}

const TABLES: &[(&str, &str, &[&str])] = &[
    ("documents_document", "id", &[]),
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
                Value::String(s.replace('\\', "/"))
            }
            Value::Array(a) => Value::Array(a.iter().map(|x| self.value(x)).collect()),
            Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), self.value(x))).collect()),
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

/// Page count, effective rotation and text per page of a generated PDF.
fn pdf_props(path: &Path) -> Value {
    let doc = match pl_pdfops::pdf::load(path, None) {
        Ok(d) => d,
        Err(e) => return json!({"error": e.kind()}),
    };
    let ids = pl_pdfops::pdf::page_ids(&doc);
    let rotations: Vec<i64> = ids.iter().map(|p| pl_pdfops::pdf::effective_rotation(&doc, *p)).collect();
    let texts: Vec<String> = (1..=ids.len() as u32)
        .map(|n| doc.extract_text(&[n]).map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_else(|_| "<no text>".into()))
        .collect();
    json!({"pages": ids.len(), "rotations": rotations, "texts": texts})
}

/// `original_file` → file name; properties of the file under `files[name]`.
fn norm_task_kwargs(kwargs: &Value, files: &mut BTreeMap<String, Value>) -> Value {
    let mut k = kwargs.clone();
    if let Some(f) = k.pointer_mut("/input_doc/original_file")
        && let Some(s) = f.as_str().map(str::to_string)
    {
        let p = PathBuf::from(&s);
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        files.insert(name.clone(), pdf_props(&p));
        *f = Value::String(format!("<scratch>/{name}"));
    }
    k
}

fn reduce_sig(s: &Value) -> Value {
    json!({
        "task": s["task"],
        "args": s.get("args").cloned().unwrap_or(json!([])),
        "kwargs": s.get("kwargs").cloned().unwrap_or(json!({})),
        "immutable": s.get("immutable").and_then(Value::as_bool).unwrap_or(false),
    })
}

fn sigs(v: &Value) -> Value {
    Value::Array(v.as_array().map(|a| a.iter().map(reduce_sig).collect()).unwrap_or_default())
}

/// Plan from the overlay recordings of the Django side.
fn django_plan(files: &mut BTreeMap<String, Value>) -> Value {
    let text = std::fs::read_to_string(task_log()).unwrap_or_default();
    let mut plan: Vec<Value> = Vec::new();
    let mut group_pos: HashMap<String, usize> = HashMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let e: Value = serde_json::from_str(line).expect("task log line");
        let task = json!({
            "task": e["task"],
            "args": e["args"],
            "kwargs": norm_task_kwargs(&e["kwargs"], files),
            "trigger_source": e["trigger_source"],
        });
        match e["group"].as_str() {
            None => {
                let mut t = task;
                t["link"] = sigs(&e["callbacks"]);
                t["link_error"] = sigs(&e["errbacks"]);
                plan.push(json!({"kind": "task", "task": t}));
            }
            Some(g) => {
                let pos = *group_pos.entry(g.to_string()).or_insert_with(|| {
                    let entry = if e["chord"].is_null() {
                        json!({"kind": "group", "header": []})
                    } else {
                        json!({
                            "kind": "chord", "header": [], "body": reduce_sig(&e["chord"]),
                            "on_error": e["chord"]["link_error"].as_array().and_then(|a| a.first()).map(reduce_sig).unwrap_or(Value::Null),
                        })
                    };
                    plan.push(entry);
                    plan.len() - 1
                });
                if let Some(h) = plan[pos]["header"].as_array_mut() {
                    h.push(task);
                }
            }
        }
    }
    Value::Array(plan)
}

/// task_name, args, kwargs, headers, link, link_error, group_id
type QueueRow = (String, Value, Value, Value, Value, Value, Option<String>);

/// Plan from `paperrust_task_queue` / `paperrust_task_group` (Rust side).
async fn rust_plan(files: &mut BTreeMap<String, Value>) -> Value {
    let mut c = sqlx::PgConnection::connect(&db_url()).await.expect("db");
    let rows: Vec<QueueRow> = sqlx::query_as(
        "SELECT task_name, args, kwargs, headers, link, link_error, group_id FROM paperrust_task_queue ORDER BY id",
    )
    .fetch_all(&mut c)
    .await
    .expect("queue rows");
    let groups: Vec<(String, Option<Value>, Option<Value>)> =
        sqlx::query_as("SELECT group_id, body, on_error FROM paperrust_task_group").fetch_all(&mut c).await.expect("groups");
    let groups: HashMap<String, (Option<Value>, Option<Value>)> = groups.into_iter().map(|(g, b, o)| (g, (b, o))).collect();
    let mut plan: Vec<Value> = Vec::new();
    let mut group_pos: HashMap<String, usize> = HashMap::new();
    for (name, args, kwargs, headers, link, link_error, group) in rows {
        let task = json!({
            "task": name,
            "args": args,
            "kwargs": norm_task_kwargs(&kwargs, files),
            "trigger_source": headers.get("trigger_source").cloned().unwrap_or(Value::Null),
        });
        match group {
            None => {
                let mut t = task;
                t["link"] = sigs(&link);
                t["link_error"] = sigs(&link_error);
                plan.push(json!({"kind": "task", "task": t}));
            }
            Some(g) => {
                let pos = *group_pos.entry(g.clone()).or_insert_with(|| {
                    let entry = match groups.get(&g) {
                        Some((Some(body), on_error)) => json!({
                            "kind": "chord", "header": [], "body": reduce_sig(body),
                            "on_error": on_error.as_ref().map(reduce_sig).unwrap_or(Value::Null),
                        }),
                        _ => json!({"kind": "group", "header": []}),
                    };
                    plan.push(entry);
                    plan.len() - 1
                });
                if let Some(h) = plan[pos]["header"].as_array_mut() {
                    h.push(task);
                }
            }
        }
    }
    Value::Array(plan)
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

fn request_body(case: &Value) -> Vec<u8> {
    serde_json::to_vec(&case["body"]).unwrap_or_default()
}

async fn django_call(case: &Value, norm: &mut Norm) -> Resp {
    let client = reqwest::Client::new();
    let mut rb = client.post(format!("{DJANGO}/api/documents/bulk_edit/")).header("Accept", "application/json");
    if let Some(t) = case["user"].as_str().and_then(token) {
        rb = rb.header("Authorization", t);
    }
    let r = rb.header("Content-Type", "application/json").body(request_body(case)).send().await.expect("django request");
    let status = r.status().as_u16();
    let ct = main_ct(r.headers().get("content-type").and_then(|v| v.to_str().ok()));
    let text = r.text().await.unwrap_or_default();
    Resp { status, body: parse_body(&ct, &text, norm), content_type: ct }
}

async fn rust_call(case: &Value, norm: &mut Norm) -> Resp {
    let state = pl_core::AppState::from_env().await.expect("state");
    pl_queue::migrations::apply(state.db()).await.expect("queue migration");
    let queue: Arc<dyn pl_queue::TaskQueue> = Arc::new(pl_queue::PgQueue::new(state.db().clone(), 5));
    state.set_service::<Arc<dyn pl_queue::TaskQueue>>(queue);
    pl_documents_write::hooks::set_pdf_ops(Arc::new(pl_documents_write::pdf_edit::NativePdfOps));
    let authz = Authz::new(state.db().clone(), AuthConfig::from_core(state.config()));
    let route = pl_documents_write::routes().into_iter().find(|r| r.id == "documents.bulk_edit").expect("route");
    let app: axum::Router = axum::Router::new()
        .route(route.path, route.handler)
        .with_state(state)
        .layer(axum::Extension(authz));
    let mut req = Request::builder()
        .method("POST")
        .uri("/api/documents/bulk_edit/")
        .header("host", "127.0.0.1:8150")
        .header("accept", "application/json")
        .header("content-type", "application/json");
    if let Some(t) = case["user"].as_str().and_then(token) {
        req = req.header("authorization", t);
    }
    let mut req = req.body(Body::from(request_body(case))).expect("request");
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

struct Captured {
    resp: Resp,
    db: BTreeMap<String, Value>,
    plan: Value,
    files: BTreeMap<String, Value>,
}

#[tokio::test]
#[ignore = "braucht die W13-Referenz-Kopie (tests/parity/pl-documents-write/w13/django-start.sh) und w13env.sh"]
async fn pdf_methods_match_django() {
    assert_eq!(std::env::var("PAPERLESS_DBNAME").as_deref(), Ok("pl_w13_ref"), "source w13env.sh first");
    let cases: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(w13_dir().join("pdf_cases.json")).expect("pdf_cases.json"))
        .expect("cases json");
    let only: Option<Vec<String>> = std::env::var("W13_CASES").ok().map(|s| s.split(',').map(str::to_string).collect());
    let t0 = Utc::now() - chrono::Duration::seconds(5);
    let mut report = Vec::new();
    let (mut same, mut different) = (0, 0);
    for case in &cases {
        let name = case["name"].as_str().unwrap_or("?").to_string();
        if only.as_ref().is_some_and(|o| !o.contains(&name)) {
            continue;
        }
        let mut sides: Vec<Captured> = Vec::new();
        for django in [true, false] {
            reset();
            run_setup(case).await;
            let mut n = Norm { t0, uuids: HashMap::new() };
            let resp = if django { django_call(case, &mut n).await } else { rust_call(case, &mut n).await };
            let mut files = BTreeMap::new();
            let plan = if django { django_plan(&mut files) } else { rust_plan(&mut files).await };
            let plan = n.value(&plan);
            let db = capture_db(&mut n).await;
            sides.push(Captured { resp, db, plan, files });
        }
        let (d, r) = (&sides[0], &sides[1]);
        let mut diffs = Vec::new();
        if d.resp.status != r.resp.status {
            diffs.push(format!("status django={} rust={}", d.resp.status, r.resp.status));
        }
        if d.resp.content_type != r.resp.content_type {
            diffs.push(format!("content-type django={} rust={}", d.resp.content_type, r.resp.content_type));
        }
        diff_values("body", &d.resp.body, &r.resp.body, &mut diffs);
        for (t, dv) in &d.db {
            diff_values(&format!("db.{t}"), dv, r.db.get(t).unwrap_or(&Value::Null), &mut diffs);
        }
        diff_values("plan", &d.plan, &r.plan, &mut diffs);
        let (df, rf) = (serde_json::to_value(&d.files).unwrap_or_default(), serde_json::to_value(&r.files).unwrap_or_default());
        diff_values("files", &df, &rf, &mut diffs);
        // Known upstream deviation (contract, Known differences): diffs under these prefixes do not count.
        let accept: Vec<String> = case["accept"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
        let accepted = diffs.iter().filter(|d| accept.iter().any(|p| d.starts_with(p.as_str()))).count();
        diffs.retain(|d| !accept.iter().any(|p| d.starts_with(p.as_str())));
        if accepted > 0 {
            println!("      ({accepted} akzeptierte Abweichung(en) unter {accept:?})");
        }
        if std::env::var("W13_DUMP").is_ok() {
            let out_dir = w13_dir().join("out");
            let _ = std::fs::create_dir_all(&out_dir);
            let dump = json!({
                "django": {"status": d.resp.status, "body": d.resp.body, "plan": d.plan, "files": df},
                "rust": {"status": r.resp.status, "body": r.resp.body, "plan": r.plan, "files": rf},
            });
            let _ = std::fs::write(out_dir.join(format!("{name}.json")), serde_json::to_string_pretty(&dump).unwrap_or_default());
        }
        if diffs.is_empty() {
            same += 1;
            println!("SAME  {name}  ({} {})", d.resp.status, d.plan.as_array().map(Vec::len).unwrap_or(0));
        } else {
            different += 1;
            println!("DIFF  {name}");
            for x in diffs.iter().take(25) {
                println!("      {x}");
            }
        }
        report.push(json!({"name": name, "status": d.resp.status, "tasks": d.plan, "diffs": diffs}));
    }
    let out_dir = w13_dir().join("out");
    let _ = std::fs::create_dir_all(&out_dir);
    let _ = std::fs::write(
        out_dir.join("report.json"),
        serde_json::to_string_pretty(&json!({"same": same, "different": different, "cases": report})).unwrap_or_default(),
    );
    println!("same={same} different={different}");
    reset();
    assert_eq!(different, 0, "Unterschiede, siehe tests/parity/pl-documents-write/w13/out/report.json");
}
