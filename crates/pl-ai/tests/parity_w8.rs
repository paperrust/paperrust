#![cfg(feature = "server")]
//! Parity of the AI routes against an **own** Django instance with a local mock LLM
//! (tests/parity/pl-ai/w8-*.sh: DB `pl_w8_ref`, Django :8180, mock :8181, Redis prefix `w8`).
//! Rust runs in-process (router from `pl_ai::routes()` + `pl_authz::Authz`), same DB, same
//! data directory (shared vector index), same Redis cache.
//!
//! ```sh
//! bash tests/parity/pl-ai/w8-reset.sh && bash tests/parity/pl-ai/w8-start.sh
//! source tests/parity/pl-ai/w8-env.sh   # + Index: manage.py document_llmindex rebuild
//! cargo test -p pl-ai -j 1 --test parity_w8 -- --ignored --nocapture --test-threads 1
//! ```

use std::collections::BTreeMap;

use axum::body::Body;
use axum::http::Request;
use pl_authz::{AuthConfig, Authz};
use serde_json::Value;
use tower::ServiceExt;

const TOKENS: [(&str, &str); 4] = [
    ("admin", "Token a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001"),
    ("alice", "Token a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002"),
    ("bob", "Token b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b003"),
    ("carol", "Token c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c004"),
];

fn django() -> String {
    format!("http://127.0.0.1:{}", std::env::var("W8_DJ_PORT").unwrap_or_else(|_| "8180".into()))
}

fn mock_log() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("MOCK_LLM_LOG").expect("source tests/parity/pl-ai/w8-env.sh"))
}

fn take_log() -> Vec<Value> {
    let p = mock_log();
    let s = std::fs::read_to_string(&p).unwrap_or_default();
    let _ = std::fs::write(&p, "");
    s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Rust: process-local cache. Reference Django: cache in Garnet (`PAPERLESS_REDIS` of the W8 environment).
async fn flush_cache(state: &pl_core::AppState) {
    state.cache().local().clear();
    let url = std::env::var("PAPERLESS_REDIS").unwrap_or_else(|_| "redis://127.0.0.1:63799".into());
    let mut c = redis::Client::open(url).expect("redis url").get_multiplexed_async_connection().await.expect("redis");
    let keys: Vec<String> = redis::cmd("KEYS").arg("w8*").query_async(&mut c).await.unwrap_or_default();
    if !keys.is_empty() {
        let _: i64 = redis::cmd("DEL").arg(&keys).query_async(&mut c).await.unwrap_or(0);
    }
}

async fn app() -> (axum::Router, pl_core::AppState) {
    let state = pl_core::AppState::from_env().await.expect("state");
    let authz = Authz::new(state.db().clone(), AuthConfig::from_core(state.config()));
    let mut router: axum::Router<pl_core::AppState> = axum::Router::new();
    for r in pl_ai::routes() {
        router = router.route(r.path, r.handler);
    }
    (router.with_state(state.clone()).layer(axum::Extension(authz)), state)
}

struct Resp {
    status: u16,
    ct: String,
    cache: String,
    body: String,
}

fn norm(body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(v) => serde_json::to_string(&v).unwrap_or_default(),
        Err(_) => body.to_string(),
    }
}

async fn call_django(method: &str, path: &str, auth: Option<&str>, body: Option<&str>) -> Resp {
    let c = reqwest::Client::new();
    let mut rb = c.request(method.parse().unwrap(), format!("{}{path}", django())).header("Accept", "application/json");
    if let Some(a) = auth {
        rb = rb.header("Authorization", a);
    }
    if let Some(b) = body {
        rb = rb.header("Content-Type", "application/json").body(b.to_string());
    }
    let r = rb.send().await.expect("django");
    let h = |n: &str| r.headers().get(n).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let (status, ct, cache) = (r.status().as_u16(), h("content-type"), h("cache-control"));
    Resp { status, ct, cache, body: r.text().await.unwrap_or_default() }
}

async fn call_rust(app: &axum::Router, method: &str, path: &str, auth: Option<&str>, body: Option<&str>) -> Resp {
    let mut req = Request::builder().method(method).uri(path).header("host", "127.0.0.1:8180").header("accept", "application/json");
    if let Some(a) = auth {
        req = req.header("authorization", a);
    }
    let b = match body {
        Some(b) => {
            req = req.header("content-type", "application/json");
            Body::from(b.to_string())
        }
        None => Body::empty(),
    };
    let r = app.clone().oneshot(req.body(b).unwrap()).await.expect("rust");
    let h = |n: &str| r.headers().get(n).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let (status, ct, cache) = (r.status().as_u16(), h("content-type"), h("cache-control"));
    let bytes = axum::body::to_bytes(r.into_body(), 64 << 20).await.unwrap_or_default();
    Resp { status, ct, cache, body: String::from_utf8_lossy(&bytes).into_owned() }
}

fn same(d: &Resp, r: &Resp) -> bool {
    d.status == r.status && d.ct.split(';').next() == r.ct.split(';').next() && norm(&d.body) == norm(&r.body) && d.cache == r.cache
}

/// LLM/embedding requests without regard to the order of the JSON keys.
fn reqs(v: &[Value]) -> Vec<String> {
    v.iter().map(|e| serde_json::to_string(&serde_json::json!({"path": e["path"], "body": e["body"]})).unwrap_or_default()).collect()
}

/// Chat against the v3.4.2 reference with the adopted upstream fixes (after v3.4.2):
/// e0c33b3d3 – up to 5 instead of 3 references in the trailer (both sides compared truncated to 3);
/// 45a2ac487 – only one retrieval per question (Django's second, identical embedding request is dropped).
fn chat_same(d: &Resp, r: &Resp) -> bool {
    fn cut(body: &str) -> String {
        const DELIM: &str = "\n\n__PAPERLESS_CHAT_METADATA__";
        let Some((text, trailer)) = body.split_once(DELIM) else { return body.to_string() };
        let Ok(mut v) = serde_json::from_str::<Value>(trailer) else { return body.to_string() };
        if let Some(a) = v.get_mut("references").and_then(Value::as_array_mut) {
            a.truncate(3);
        }
        format!("{text}{DELIM}{v}")
    }
    d.status == r.status && d.ct.split(';').next() == r.ct.split(';').next() && d.cache == r.cache && cut(&d.body) == cut(&r.body)
}

/// Django v3.4.2 embeds the question twice (retriever + RetrieverQueryEngine); since 45a2ac487
/// once → remove the first exact repetition of an earlier request.
fn chat_reqs_v342(v: &[Value]) -> Vec<String> {
    let mut out = reqs(v);
    if let Some(i) = (1..out.len()).find(|&i| out[..i].contains(&out[i])) {
        out.remove(i);
    }
    out
}

#[tokio::test]
#[ignore = "braucht die W8-Kopie (tests/parity/pl-ai/w8-start.sh)"]
async fn ai_suggestions_match_django() {
    let (app, state) = app().await;
    let docs = ["1", "2", "3", "7", "16", "56", "66", "100", "150", "201", "196", "999"];
    let (mut ok, mut bad, mut wire_ok, mut wire_bad) = (0, 0, 0, 0);
    for (name, tok) in TOKENS.iter().copied().map(|(n, t)| (n, Some(t))).chain([("anon", None)]) {
        for d in docs {
            let path = format!("/api/documents/{d}/ai_suggestions/");
            flush_cache(&state).await;
            take_log();
            let dj = call_django("GET", &path, tok, None).await;
            let dj_log = take_log();
            flush_cache(&state).await;
            let rs = call_rust(&app, "GET", &path, tok, None).await;
            let rs_log = take_log();
            if same(&dj, &rs) {
                ok += 1;
            } else {
                bad += 1;
                println!("DIFF {name} {path}\n  django {} {} [{}] {}\n  rust   {} {} [{}] {}", dj.status, dj.ct, dj.cache, dj.body.chars().take(600).collect::<String>(), rs.status, rs.ct, rs.cache, rs.body.chars().take(600).collect::<String>());
            }
            if reqs(&dj_log) == reqs(&rs_log) {
                wire_ok += 1;
            } else {
                wire_bad += 1;
                println!("WIRE DIFF {name} {path}: django {} requests, rust {}", dj_log.len(), rs_log.len());
                for (a, b) in reqs(&dj_log).iter().zip(reqs(&rs_log).iter()) {
                    if a != b {
                        let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
                        let p = a.iter().zip(&b).position(|(x, y)| x != y).unwrap_or(a.len().min(b.len()));
                        let s = p.saturating_sub(120);
                        println!("  dj: …{}…\n  rs: …{}…", a[s..(p + 200).min(a.len())].iter().collect::<String>(), b[s..(p + 200).min(b.len())].iter().collect::<String>());
                        break;
                    }
                }
            }
        }
    }
    // shared cache: Django writes, Rust reads (no LLM call) – and vice versa
    let mut cache_ok = 0;
    for d in ["1", "7", "100"] {
        let path = format!("/api/documents/{d}/ai_suggestions/");
        flush_cache(&state).await;
        let dj = call_django("GET", &path, Some(TOKENS[0].1), None).await;
        take_log();
        let rs = call_rust(&app, "GET", &path, Some(TOKENS[0].1), None).await;
        let calls = take_log().len();
        let rs2 = call_rust(&app, "GET", &path, Some(TOKENS[1].1), None).await;
        take_log();
        let dj2 = call_django("GET", &path, Some(TOKENS[1].1), None).await;
        let calls2 = take_log().len();
        if same(&dj, &rs) && calls == 0 && same(&dj2, &rs2) && calls2 == 0 {
            cache_ok += 1;
        } else {
            println!("CACHE DIFF {path}: rust calls {calls}, django calls {calls2}");
        }
    }
    println!("PARITY ai_suggestions same={ok} different={bad} wire_same={wire_ok} wire_different={wire_bad} cache_shared={cache_ok}/3");
    assert_eq!(bad, 0);
    assert_eq!(wire_bad, 0);
    assert_eq!(cache_ok, 3);
}

#[tokio::test]
#[ignore = "braucht die W8-Kopie (tests/parity/pl-ai/w8-start.sh)"]
async fn chat_matches_django() {
    let (app, _state) = app().await;
    let bodies = [
        r#"{"q": "Was steht in der Rechnung?"}"#,
        r#"{"q": "Steuern 2024", "document_id": 7}"#,
        r#"{"q": "Zusammenfassung", "document_id": 1}"#,
        r#"{"q": "x", "document_id": 196}"#,
        r#"{"q": "x", "document_id": 999}"#,
        r#"{"q": "x", "document_id": 0}"#,
        r#"{"q": "x", "document_id": "abc"}"#,
        r#"{"q": ""}"#,
        r#"{"q": "   "}"#,
        r#"{"q": null}"#,
        r#"{"q": 12}"#,
        r#"{"q": true}"#,
        r#"{}"#,
        r#"{"document_id": 1}"#,
    ];
    let long = format!(r#"{{"q": "{}"}}"#, "a".repeat(4001));
    let mut all: Vec<String> = bodies.iter().map(|s| s.to_string()).collect();
    all.push(long);
    let (mut ok, mut bad, mut wire_ok, mut wire_bad) = (0, 0, 0, 0);
    let mut seen: BTreeMap<u16, usize> = BTreeMap::new();
    for (name, tok) in TOKENS.iter().copied().map(|(n, t)| (n, Some(t))).chain([("anon", None)]) {
        for b in &all {
            take_log();
            let dj = call_django("POST", "/api/documents/chat/", tok, Some(b)).await;
            let dj_log = take_log();
            let rs = call_rust(&app, "POST", "/api/documents/chat/", tok, Some(b)).await;
            let rs_log = take_log();
            *seen.entry(dj.status).or_default() += 1;
            if chat_same(&dj, &rs) {
                ok += 1;
            } else {
                bad += 1;
                println!("DIFF {name} {b}\n  django {} {} [{}] {:?}\n  rust   {} {} [{}] {:?}", dj.status, dj.ct, dj.cache, dj.body.chars().take(400).collect::<String>(), rs.status, rs.ct, rs.cache, rs.body.chars().take(400).collect::<String>());
            }
            if chat_reqs_v342(&dj_log) == reqs(&rs_log) {
                wire_ok += 1;
            } else {
                wire_bad += 1;
                println!("WIRE DIFF {name} {b}: django {} requests, rust {}", dj_log.len(), rs_log.len());
            }
        }
    }
    println!("PARITY chat same={ok} different={bad} wire_same={wire_ok} wire_different={wire_bad} statuses={seen:?}");
    assert_eq!(bad, 0);
    assert_eq!(wire_bad, 0);
}

/// index nodes that Rust builds from the DB equal those Django wrote
/// (text, metadata, embedding; only the node UUIDs are random).
#[tokio::test]
#[ignore = "braucht die W8-Kopie mit Django-gebautem Index"]
async fn index_nodes_match_django_index() {
    let (_app, state) = app().await;
    let cfg = pl_ai::server::effective_config(&state).await.unwrap();
    let dir = pl_ai::server::index::index_dir(&state);
    let st = pl_ai::vector_store::VecStore::open(&dir, None).unwrap();
    let mut py: BTreeMap<i64, Vec<(String, String, Vec<u8>)>> = BTreeMap::new();
    {
        let mut q = st.conn().prepare("SELECT document_id, node_content, embedding FROM documents").unwrap();
        let rows = q.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Vec<u8>>(2)?))).unwrap();
        for r in rows {
            let (d, nc, e) = r.unwrap();
            let n = pl_ai::node::TextNode::from_node_content(&nc).unwrap();
            py.entry(d).or_default().push((n.text.clone(), serde_json::to_string(&n.metadata).unwrap(), e));
        }
    }
    let mt = st.get_modified_times().unwrap();
    let docs = pl_ai::server::index::load_index_docs(&state, None, 0, 100_000).await.unwrap();
    let (mut ok, mut bad) = (0, 0);
    for d in &docs {
        let nodes = pl_ai::node::build_document_nodes(&d.id.to_string(), &d.text, &d.metadata, cfg.llm_embedding_chunk_size as usize, |i| i.to_string()).unwrap();
        let mut got: Vec<(String, String)> = nodes.iter().map(|n| (n.text.clone(), serde_json::to_string(&n.metadata).unwrap())).collect();
        let mut exp: Vec<(String, String)> = py.get(&d.id).map(|v| v.iter().map(|(t, m, _)| (t.clone(), m.clone())).collect()).unwrap_or_default();
        got.sort();
        exp.sort();
        let modified_ok = mt.get(&d.id.to_string()) == Some(&d.modified_iso);
        if got == exp && modified_ok {
            ok += 1;
        } else {
            bad += 1;
            if bad < 5 {
                println!("DIFF doc {} modified_ok={modified_ok} rust {} nodes, django {}", d.id, got.len(), exp.len());
                for (a, b) in got.iter().zip(exp.iter()) {
                    if a != b {
                        println!("  rust  : {:?}\n  django: {:?}", a.0.chars().take(300).collect::<String>(), b.0.chars().take(300).collect::<String>());
                        println!("  meta rust  : {}\n  meta django: {}", a.1, b.1);
                        break;
                    }
                }
            }
        }
    }
    println!("PARITY llm-index nodes same={ok} different={bad} (docs {})", docs.len());
    assert_eq!(bad, 0);
}

fn repo() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn sh(script: &str) {
    let st = std::process::Command::new("bash").arg(repo().join(script)).status().expect("bash");
    assert!(st.success(), "{script}");
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    let _ = std::fs::remove_dir_all(to);
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_file() {
            std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
}

async fn doc_snapshot(db: &sqlx::PgPool, doc: i64, max_ids: (i32, i32, i32)) -> Value {
    use sqlx::Row;
    let r = sqlx::query(
        "SELECT title, correspondent_id, document_type_id, storage_path_id, created::text AS created, \
         COALESCE((SELECT array_agg(tag_id ORDER BY tag_id) FROM documents_document_tags WHERE document_id = $1), '{}') AS tags \
         FROM documents_document WHERE id = $1",
    )
    .bind(doc as i32)
    .fetch_one(db)
    .await
    .unwrap();
    let new = |t: &'static str, m: i32| async move {
        sqlx::query(&format!("SELECT name, owner_id FROM {t} WHERE id > $1 ORDER BY id"))
            .bind(m)
            .fetch_all(db)
            .await
            .unwrap()
            .iter()
            .map(|r| serde_json::json!([r.get::<String, _>(0), r.get::<Option<i32>, _>(1)]))
            .collect::<Vec<_>>()
    };
    serde_json::json!({
        "title": r.get::<String, _>("title"),
        "correspondent": r.get::<Option<i32>, _>("correspondent_id"),
        "document_type": r.get::<Option<i32>, _>("document_type_id"),
        "storage_path": r.get::<Option<i32>, _>("storage_path_id"),
        "created": r.get::<String, _>("created"),
        "tags": r.get::<Vec<i32>, _>("tags"),
        "new_tags": new("documents_tag", max_ids.0).await,
        "new_correspondents": new("documents_correspondent", max_ids.1).await,
        "new_document_types": new("documents_documenttype", max_ids.2).await,
    })
}

async fn prepare(fields: &str, create: bool, overwrite: bool) -> (pl_core::AppState, i64, (i32, i32, i32)) {
    let state = pl_core::AppState::from_env().await.expect("state");
    let db = state.db();
    let id: i32 = sqlx::query_scalar(
        "INSERT INTO documents_workflowaction (type, \"order\", remove_all_tags, remove_all_document_types, remove_all_correspondents, \
         remove_all_storage_paths, remove_all_owners, remove_all_permissions, remove_all_custom_fields, ai_create_missing, ai_overwrite_existing, ai_suggestion_fields) \
         VALUES (8, 0, false, false, false, false, false, false, false, $1, $2, $3::jsonb) RETURNING id",
    )
    .bind(create)
    .bind(overwrite)
    .bind(fields)
    .fetch_one(db)
    .await
    .unwrap();
    let m = |t: &'static str| async move { sqlx::query_scalar::<_, i32>(&format!("SELECT COALESCE(max(id), 0) FROM {t}")).fetch_one(db).await.unwrap() };
    let max = (m("documents_tag").await, m("documents_correspondent").await, m("documents_documenttype").await);
    flush_cache(&state).await;
    take_log();
    (state, i64::from(id), max)
}

/// Workflow action APPLY_AI_SUGGESTIONS: the Django Celery task (synchronous via manage.py shell) and
/// Rust change the same document identically (fields, tags, newly created objects, LLM requests).
#[tokio::test]
#[ignore = "setzt die W8-Kopie mehrfach zurück (w8-reset.sh)"]
async fn workflow_apply_ai_suggestions_matches_django() {
    let idx = repo().join("tests/parity/pl-ai/.w8/data/data/llm_index");
    let backup = repo().join("tests/parity/pl-ai/.w8/llm_index_backup");
    if !backup.exists() {
        copy_dir(&idx, &backup);
    }
    let py = std::env::var("PY").expect("PY");
    let cases = [
        (7, r#"["title","tags","correspondent","document_type","storage_path","created"]"#, true, false),
        (56, r#"["title","tags","correspondent","document_type","storage_path","created"]"#, true, true),
        (100, r#"["tags","created"]"#, false, true),
        (150, r#"[]"#, true, true),
    ];
    let (mut ok, mut bad) = (0, 0);
    for (doc, fields, create, overwrite) in cases {
        sh("tests/parity/pl-ai/w8-reset.sh");
        copy_dir(&backup, &idx);
        let (state, action, max) = prepare(fields, create, overwrite).await;
        let code = format!("from documents.tasks import apply_ai_suggestions; apply_ai_suggestions({action}, {doc})");
        let out = std::process::Command::new(&py)
            .args([repo().join("upstream/src/manage.py").to_str().unwrap(), "shell", "-c", &code])
            .current_dir(repo().join("upstream/src"))
            .output()
            .expect("manage.py");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let dj = doc_snapshot(state.db(), doc, max).await;
        let dj_log = take_log();
        drop(state);

        sh("tests/parity/pl-ai/w8-reset.sh");
        copy_dir(&backup, &idx);
        let (state, action2, max2) = prepare(fields, create, overwrite).await;
        assert_eq!(action, action2);
        pl_ai::server::workflow::apply_ai_suggestions(&state, action2, doc).await.expect("rust apply");
        let rs = doc_snapshot(state.db(), doc, max2).await;
        let rs_log = take_log();
        if dj == rs && reqs(&dj_log) == reqs(&rs_log) {
            ok += 1;
        } else {
            bad += 1;
            println!("DIFF doc {doc} fields {fields}\n  django {dj}\n  rust   {rs}\n  wire django {} rust {} equal={}", dj_log.len(), rs_log.len(), reqs(&dj_log) == reqs(&rs_log));
        }
    }
    sh("tests/parity/pl-ai/w8-reset.sh");
    copy_dir(&backup, &idx);
    println!("PARITY workflow.apply_ai_suggestions same={ok} different={bad}");
    assert_eq!(bad, 0);
}

/// Rust rebuilds the index (same file, locks like upstream); then Django reads it:
/// chat and ai_suggestions stay equal between Django and Rust.
#[tokio::test]
#[ignore = "überschreibt den W8-Index (Sicherung .w8/llm_index_backup)"]
async fn rust_built_index_is_read_by_django() {
    let idx = repo().join("tests/parity/pl-ai/.w8/data/data/llm_index");
    let backup = repo().join("tests/parity/pl-ai/.w8/llm_index_backup");
    if !backup.exists() {
        copy_dir(&idx, &backup);
    }
    let (app, state) = app().await;
    let cfg = pl_ai::server::effective_config(&state).await.unwrap();
    take_log();
    let msg = pl_ai::server::index::update_llm_index(&state, &cfg, true, None).await.expect("rebuild");
    assert_eq!(msg, "LLM index rebuilt successfully.");
    let embeds = take_log().len();
    let msg = pl_ai::server::index::update_llm_index(&state, &cfg, false, None).await.expect("update");
    assert_eq!(msg, "No changes detected in LLM index.");
    let (mut ok, mut bad) = (0, 0);
    for (tok, body) in [(TOKENS[0].1, r#"{"q": "Rechnung"}"#), (TOKENS[1].1, r#"{"q": "Steuern", "document_id": 7}"#), (TOKENS[2].1, r#"{"q": "Arzt"}"#)] {
        take_log();
        let dj = call_django("POST", "/api/documents/chat/", Some(tok), Some(body)).await;
        let dl = take_log();
        let rs = call_rust(&app, "POST", "/api/documents/chat/", Some(tok), Some(body)).await;
        let rl = take_log();
        if chat_same(&dj, &rs) && chat_reqs_v342(&dl) == reqs(&rl) && dj.status == 200 && dj.body.contains("__PAPERLESS_CHAT_METADATA__") {
            ok += 1;
        } else {
            bad += 1;
            println!("DIFF chat {body}: {} {:?} / {} {:?}", dj.status, dj.body.chars().take(200).collect::<String>(), rs.status, rs.body.chars().take(200).collect::<String>());
        }
    }
    for d in ["7", "100"] {
        let path = format!("/api/documents/{d}/ai_suggestions/");
        flush_cache(&state).await;
        take_log();
        let dj = call_django("GET", &path, Some(TOKENS[0].1), None).await;
        let dl = take_log();
        flush_cache(&state).await;
        let rs = call_rust(&app, "GET", &path, Some(TOKENS[0].1), None).await;
        let rl = take_log();
        if same(&dj, &rs) && reqs(&dl) == reqs(&rl) && dj.status == 200 {
            ok += 1;
        } else {
            bad += 1;
            println!("DIFF {path}");
        }
    }
    copy_dir(&backup, &idx);
    println!("PARITY rust-built index read by django: same={ok} different={bad} (embedding requests {embeds})");
    assert_eq!(bad, 0);
}

/// PaperRust extension: run (suggest/apply), review list, apply, undo,
/// conflict, permissions. Applies migration 0001_paperrust_ai.sql only to the W8 copy.
#[tokio::test]
#[ignore = "schreibt in die W8-Kopie (Migration + Dokumentänderungen); danach w8-reset.sh"]
async fn review_api_flow() {
    // Security wave 3 M1: `mode=apply` only with PAPERRUST_AI_AUTO_MODE=apply (set before the first
    // read of the environment; the test runs only individually with --ignored).
    if std::env::var("PAPERRUST_AI_AUTO_MODE").is_err() {
        // SAFETY: before any access by other threads to the AI configuration in this test.
        unsafe { std::env::set_var("PAPERRUST_AI_AUTO_MODE", "apply") };
    }
    let idx = repo().join("tests/parity/pl-ai/.w8/data/data/llm_index");
    let backup = repo().join("tests/parity/pl-ai/.w8/llm_index_backup");
    if !backup.exists() {
        copy_dir(&idx, &backup);
    }
    sh("tests/parity/pl-ai/w8-reset.sh");
    copy_dir(&backup, &idx);
    let (app, state) = app().await;
    let db = state.db().clone();
    sqlx::raw_sql(include_str!("../migrations/0001_paperrust_ai.sql")).execute(&db).await.expect("migration");
    flush_cache(&state).await;
    let admin = TOKENS[0].1;
    let post = |path: String, body: &'static str, tok: &'static str| {
        let app = app.clone();
        async move { call_rust(&app, "POST", &path, Some(tok), Some(body)).await }
    };
    let before = doc_snapshot(&db, 7, (i32::MAX, i32::MAX, i32::MAX)).await;
    // 1) suggest only
    let r = post("/api/x-ai/run/".into(), r#"{"documents": [7], "mode": "suggest"}"#, admin).await;
    assert_eq!(r.status, 200, "{}", r.body);
    let v: Value = serde_json::from_str(&r.body).unwrap();
    let res = &v["results"][0];
    assert_eq!(res["status"], "pending", "{v}");
    let rid = res["review"].as_i64().unwrap();
    assert_eq!(doc_snapshot(&db, 7, (i32::MAX, i32::MAX, i32::MAX)).await, before, "suggest ändert nichts");
    let l = call_rust(&app, "GET", "/api/x-ai/review/?status=pending", Some(admin), None).await;
    let lv: Value = serde_json::from_str(&l.body).unwrap();
    assert!(lv["results"].as_array().unwrap().iter().any(|x| x["id"] == rid), "{}", l.body);
    // alice may not change document 56 (upstream ai_suggestions: 403)
    let r56 = post("/api/x-ai/run/".into(), r#"{"documents": [56], "mode": "suggest"}"#, admin).await;
    let v56: Value = serde_json::from_str(&r56.body).unwrap();
    let rid56 = v56["results"][0]["review"].as_i64().unwrap();
    let b = post(format!("/api/x-ai/review/{rid56}/apply/"), "{}", TOKENS[1].1).await;
    assert!(b.status == 404 || b.status == 403, "{} {}", b.status, b.body);
    let b = post("/api/x-ai/run/".into(), r#"{"documents": [56], "mode": "apply"}"#, TOKENS[1].1).await;
    assert_eq!(b.status, 403, "{}", b.body);
    // 2) apply
    let a = post(format!("/api/x-ai/review/{rid}/apply/"), "{}", admin).await;
    assert_eq!(a.status, 200, "{}", a.body);
    let av: Value = serde_json::from_str(&a.body).unwrap();
    assert_eq!(av["status"], "applied");
    let after = doc_snapshot(&db, 7, (i32::MAX, i32::MAX, i32::MAX)).await;
    assert_ne!(after, before);
    let audits: i64 = sqlx::query_scalar("SELECT count(*) FROM paperrust_ai_audit WHERE object_id = 7").fetch_one(&db).await.unwrap();
    assert!(audits >= 1);
    // 3) undo
    let u = post(format!("/api/x-ai/review/{rid}/undo/"), "{}", admin).await;
    assert_eq!(u.status, 200, "{}", u.body);
    assert_eq!(doc_snapshot(&db, 7, (i32::MAX, i32::MAX, i32::MAX)).await, before, "undo stellt wieder her");
    // 4) apply automatically from confidence, then conflict on undo
    flush_cache(&state).await;
    let r = post("/api/x-ai/run/".into(), r#"{"documents": [7], "mode": "apply"}"#, admin).await;
    let v: Value = serde_json::from_str(&r.body).unwrap();
    let res = &v["results"][0];
    assert!(res["applied"].as_u64().unwrap() >= 1, "{v}");
    let rid2 = res["review"].as_i64().unwrap();
    sqlx::query("UPDATE documents_document SET correspondent_id = NULL WHERE id = 7").execute(&db).await.unwrap();
    let u = post(format!("/api/x-ai/review/{rid2}/undo/"), "{}", admin).await;
    assert_eq!(u.status, 409, "{}", u.body);
    // 5) without permissions: carol 403
    let c = call_rust(&app, "GET", "/api/x-ai/review/", Some(TOKENS[3].1), None).await;
    assert_eq!(c.status, 403);
    println!("REVIEW FLOW ok: suggest→apply→undo, apply-mode, conflict 409, permissions");
}
