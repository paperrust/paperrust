//! Parity comparison Django ↔ pl-documents (in-process, with `pl_authz::Authz` like the server),
//! read-only requests only.
//!
//! ```sh
//! source reference/env.sh
//! cargo test -p pl-documents -j 1 --test parity -- --ignored --nocapture
//! ```
//! Compares status, content type (main type) and JSON text (key order counts).
//!
//! Sets (environment `PARITY_SETS`, comma-separated, default all):
//! * `routes`    – all read routes (list, detail, root, notes, share_links, history, next_asn,
//!   selection_data) for all users and API v9/v10,
//! * `docfilter` – `tests/parity/pl-docfilter/cases.base.json`,
//! * `search`    – `tests/parity/pl-search/cases.json` + `cases.base.json` (`/api/documents/` only).
//!
//! `PARITY_ONLY=<substring>` restricts to paths that contain it.
//! Deviations where only the order of `results` differs while the response is otherwise identical
//! are counted as `tie` (tie without a unique sort order).

use std::collections::HashMap;

use axum::body::Body;
use axum::http::Request;
use pl_authz::{AuthConfig, Authz};
use serde_json::Value;
use tower::ServiceExt;

const DJANGO: &str = "http://127.0.0.1:8000";
const HOST: &str = "127.0.0.1:8000";

struct Resp {
    status: u16,
    ctype: String,
    body: String,
}

/// Role name of the parity catalogs → Authorization header (None = anonymous).
fn user_auth(name: &str) -> Option<String> {
    let token = match name {
        "admin" => "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001",
        "user1" | "alice" => "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002",
        "user2" | "bob" => "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b003",
        "noperm" | "carol" => "c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c004",
        "inactive" | "dave" => "d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d005",
        "badtoken" => "nope",
        "basic-admin" => return Some("Basic YWRtaW46YWRtaW4=".into()),
        "basic-wrong" => return Some("Basic YWxpY2U6d3Jvbmc=".into()),
        _ => return None,
    };
    Some(format!("Token {token}"))
}

fn normalize(body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(v) => serde_json::to_string(&v).unwrap_or_default(),
        Err(_) => body.to_string(),
    }
}

/// Only the order of `results` (and `all`) differs?
fn tie_only(a: &str, b: &str) -> bool {
    let (Ok(mut x), Ok(mut y)) = (
        serde_json::from_str::<Value>(a),
        serde_json::from_str::<Value>(b),
    ) else {
        return false;
    };
    let sorted = |v: &mut Value| -> bool {
        let Some(r) = v.get_mut("results").and_then(Value::as_array_mut) else {
            return false;
        };
        r.sort_by_key(|e| e.to_string());
        if let Some(all) = v.get_mut("all").and_then(Value::as_array_mut) {
            all.sort_by_key(|e| e.to_string());
        }
        true
    };
    sorted(&mut x) && sorted(&mut y) && x == y
}

async fn django(
    client: &reqwest::Client,
    method: &str,
    path: &str,
    auth: Option<&str>,
    accept: &str,
    body: Option<&str>,
) -> Resp {
    let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut rb = client
        .request(m, format!("{DJANGO}{path}"))
        .header("Accept", accept);
    if let Some(a) = auth {
        rb = rb.header("Authorization", a);
    }
    if let Some(b) = body {
        rb = rb
            .header("Content-Type", "application/json")
            .body(b.to_string());
    }
    let r = match rb.send().await {
        Ok(r) => r,
        Err(e) => {
            return Resp {
                status: 0,
                ctype: String::new(),
                body: format!("django error: {e}"),
            };
        }
    };
    let status = r.status().as_u16();
    let ctype = r
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    Resp {
        status,
        ctype,
        body: r.text().await.unwrap_or_default(),
    }
}

async fn rust(
    app: &axum::Router,
    method: &str,
    path: &str,
    auth: Option<&str>,
    accept: &str,
    body: Option<&str>,
) -> Resp {
    let mut rb = Request::builder()
        .method(method)
        .uri(path)
        .header("host", HOST)
        .header("accept", accept);
    if let Some(a) = auth {
        rb = rb.header("authorization", a);
    }
    if body.is_some() {
        rb = rb.header("content-type", "application/json");
    }
    let Ok(req) = rb.body(Body::from(body.unwrap_or("").to_string())) else {
        return Resp {
            status: 0,
            ctype: String::new(),
            body: "bad request".into(),
        };
    };
    let r = match tokio::time::timeout(
        std::time::Duration::from_secs(120),
        app.clone().oneshot(req),
    )
    .await
    {
        Ok(Ok(r)) => r,
        _ => {
            return Resp {
                status: 0,
                ctype: String::new(),
                body: "rust timeout".into(),
            };
        }
    };
    let status = r.status().as_u16();
    let ctype = r
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = axum::body::to_bytes(r.into_body(), 256 * 1024 * 1024)
        .await
        .unwrap_or_default();
    Resp {
        status,
        ctype,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

async fn pick(db: &sqlx::PgPool, sql: &str) -> Option<i64> {
    sqlx::query_scalar::<_, i32>(sql)
        .fetch_optional(db)
        .await
        .ok()
        .flatten()
        .map(i64::from)
}

/// A comparison case.
struct Case {
    set: &'static str,
    id: String,
    method: &'static str,
    path: String,
    body: Option<String>,
    users: Vec<String>,
    versions: Vec<String>,
}

fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        other => other.to_string(),
    }
}

fn substitute(s: &str, vars: &HashMap<String, String>) -> String {
    let mut out = s.to_string();
    for (k, v) in vars {
        out = out.replace(&format!("${{{k}}}"), v);
    }
    out
}

fn build_path(path: &str, query: Option<&Value>, vars: &HashMap<String, String>) -> String {
    let Some(Value::Object(q)) = query else {
        return path.to_string();
    };
    let mut url = match reqwest::Url::parse(&format!("http://x{path}")) {
        Ok(u) => u,
        Err(_) => return path.to_string(),
    };
    {
        let mut qp = url.query_pairs_mut();
        for (k, v) in q {
            qp.append_pair(k, &substitute(&py_str(v), vars));
        }
    }
    format!("{}?{}", url.path(), url.query().unwrap_or(""))
}

/// Resolve `vars` of a catalog via Django (admin or the given user).
async fn resolve_vars(client: &reqwest::Client, cat: &Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(vars) = cat.get("vars").and_then(Value::as_object) else {
        return out;
    };
    for (name, spec) in vars {
        let path = build_path(
            spec["get"].as_str().unwrap_or("/"),
            spec.get("query"),
            &HashMap::new(),
        );
        let user = spec.get("user").and_then(Value::as_str).unwrap_or("admin");
        let r = django(
            client,
            "GET",
            &path,
            user_auth(user).as_deref(),
            "application/json",
            None,
        )
        .await;
        let Ok(v) = serde_json::from_str::<Value>(&r.body) else {
            continue;
        };
        let mut cur = &v;
        for part in spec["json"].as_str().unwrap_or("").split('.') {
            cur = match part.parse::<usize>() {
                Ok(i) => &cur[i],
                Err(_) => &cur[part],
            };
        }
        out.insert(name.clone(), py_str(cur));
    }
    out
}

fn catalog_cases(
    set: &'static str,
    file: &str,
    vars: &HashMap<String, String>,
    cat: &Value,
) -> Vec<Case> {
    let defaults = cat.get("defaults");
    let mut out = Vec::new();
    let Some(cases) = cat.get("cases").and_then(Value::as_array) else {
        return out;
    };
    for c in cases {
        if c.get("path").and_then(Value::as_str) != Some("/api/documents/") {
            continue;
        }
        let list = |key: &str, dflt: &[&str]| -> Vec<String> {
            c.get(key)
                .or_else(|| defaults.and_then(|d| d.get(key)))
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_else(|| dflt.iter().map(|s| s.to_string()).collect())
        };
        out.push(Case {
            set,
            id: format!("{file}:{}", c["id"].as_str().unwrap_or("?")),
            method: "GET",
            path: build_path("/api/documents/", c.get("query"), vars),
            body: None,
            users: list("users", &["admin"]),
            versions: list("api_versions", &["10"]),
        });
    }
    out
}

fn accept_for(version: &str) -> String {
    if version == "10" {
        "application/json".into()
    } else {
        format!("application/json; version={version}")
    }
}

#[tokio::test]
#[ignore = "braucht Referenz-DB und Django (reference/README.md)"]
async fn parity_documents() {
    let state = pl_core::AppState::from_env().await.expect("state");
    let db = state.db().clone();
    let authz = Authz::new(db.clone(), AuthConfig::from_core(state.config()));
    let mut router: axum::Router<pl_core::AppState> = axum::Router::new();
    for r in pl_documents::routes() {
        router = router.route(r.path, r.handler);
    }
    let app: axum::Router = router.with_state(state).layer(axum::Extension(authz));
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .expect("client");
    let only = std::env::var("PARITY_ONLY").ok();
    let sets: Vec<String> = std::env::var("PARITY_SETS")
        .unwrap_or_else(|_| "routes,docfilter,search".into())
        .split(',')
        .map(str::to_string)
        .collect();
    let repo = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");

    let mut cases: Vec<Case> = Vec::new();
    if sets.iter().any(|s| s == "routes") {
        let mut docs: Vec<i64> = Vec::new();
        for sql in [
            "SELECT id FROM documents_document WHERE root_document_id IS NULL AND deleted_at IS NULL ORDER BY id LIMIT 1",
            "SELECT document_id FROM documents_note WHERE deleted_at IS NULL ORDER BY id LIMIT 1",
            "SELECT root_document_id FROM documents_document WHERE root_document_id IS NOT NULL ORDER BY id LIMIT 1",
            "SELECT id FROM documents_document WHERE root_document_id IS NOT NULL ORDER BY id LIMIT 1",
            "SELECT document_id FROM documents_sharelink ORDER BY id LIMIT 1",
            "SELECT id FROM documents_document WHERE deleted_at IS NOT NULL ORDER BY id LIMIT 1",
            "SELECT d.id FROM documents_document d WHERE EXISTS (SELECT 1 FROM documents_document x WHERE x.id <> d.id AND x.checksum = d.checksum) LIMIT 1",
            "SELECT object_id::int FROM auditlog_logentry WHERE object_id IS NOT NULL ORDER BY id DESC LIMIT 1",
            "SELECT id FROM documents_document WHERE owner_id IS NOT NULL AND root_document_id IS NULL ORDER BY owner_id DESC, id LIMIT 1",
            "SELECT id FROM documents_document WHERE owner_id = 4 AND root_document_id IS NULL AND deleted_at IS NULL ORDER BY id LIMIT 1",
        ] {
            if let Some(id) = pick(&db, sql).await
                && !docs.contains(&id)
            {
                docs.push(id);
            }
        }
        let mut paths: Vec<(String, Option<String>)> = [
            "/api/documents/",
            "/api/documents/?page_size=3",
            "/api/documents/?page=2&page_size=2",
            "/api/documents/?page=last&page_size=7",
            "/api/documents/?page=999",
            "/api/documents/?page=abc",
            "/api/documents/?page_size=0",
            "/api/documents/?fields=id,title,tags",
            "/api/documents/?fields=dgfhs",
            "/api/documents/?fields=id,is_shared_by_requester&full_perms=true",
            "/api/documents/?truncate_content=true&page_size=5",
            "/api/documents/?full_perms=true&page_size=5",
            "/api/documents/?ordering=title,id&page_size=5",
            "/api/documents/?ordering=-num_notes,id&page_size=5",
            "/api/documents/?ordering=correspondent__name,-id&page_size=5",
            "/api/documents/?ordering=archive_serial_number,id&page_size=5",
            "/api/documents/?ordering=owner,-id&page_size=5",
            "/api/documents/?ordering=bogus&page_size=5",
            "/api/documents/?ordering=custom_field_999",
            "/api/documents/?search=invoice&page_size=5",
            "/api/documents/?search=%22ACME%20Corp%22&page_size=5",
            "/api/documents/?id__in=1,2,3",
            "/api/documents/?id=2",
            "/api/documents/?id=abc",
            "/api/documents/?tags__id__all=1&page_size=5",
            "/api/documents/?include_selection_data=true&page_size=2",
            "/api/documents/?include_selection_data=true&title__icontains=a&page_size=2",
            "/api/documents/next_asn/",
            "/api/documents/999999/",
            "/api/documents/abc/",
            "/api/documents/abc/?ordering=custom_field_999",
            "/api/documents/abc/root/",
            "/api/documents/abc/notes/",
            "/api/documents/abc/history/",
        ]
        .iter()
        .map(|p| (p.to_string(), None))
        .collect();
        for id in &docs {
            for suffix in [
                "",
                "?full_perms=true",
                "?version=",
                "?fields=id,content,page_count&version=",
                "?title__icontains=zzzzzz",
                "root/",
                "notes/",
                "share_links/",
                "history/",
            ] {
                paths.push((format!("/api/documents/{id}/{suffix}"), None));
            }
        }
        let first = docs.first().copied().unwrap_or(1);
        for b in [
            format!("{{\"documents\":[{first}]}}"),
            "{\"documents\":[]}".to_string(),
            "{\"documents\":\"x\"}".to_string(),
            "{\"documents\":[\"a\",2.5]}".to_string(),
            "{\"documents\":[999999]}".to_string(),
            "{}".to_string(),
            "{\"all\":true}".to_string(),
            format!("{{\"all\":true,\"excluded_documents\":[{first}]}}"),
            "{\"excluded_documents\":[1]}".to_string(),
            "{\"all\":\"maybe\"}".to_string(),
            "[]".to_string(),
        ] {
            paths.push(("/api/documents/selection_data/".to_string(), Some(b)));
        }
        for (i, (path, body)) in paths.into_iter().enumerate() {
            // Basic auth is expensive (PBKDF2): only on some paths
            let mut users: Vec<String> = [
                "admin", "user1", "user2", "noperm", "inactive", "badtoken", "anon",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            if i < 3 {
                users.push("basic-admin".into());
                users.push("basic-wrong".into());
            }
            cases.push(Case {
                set: "routes",
                id: path.clone(),
                method: if body.is_some() { "POST" } else { "GET" },
                path,
                body,
                users,
                versions: vec!["10".into(), "9".into()],
            });
        }
    }
    for (set, files) in [
        (
            "docfilter",
            &["tests/parity/pl-docfilter/cases.base.json"][..],
        ),
        (
            "search",
            &[
                "tests/parity/pl-search/cases.json",
                "tests/parity/pl-search/cases.base.json",
            ][..],
        ),
    ] {
        if !sets.iter().any(|s| s == set) {
            continue;
        }
        for f in files {
            let raw = std::fs::read_to_string(format!("{repo}/{f}")).expect("catalog");
            let cat: Value = serde_json::from_str(&raw).expect("catalog json");
            let vars = resolve_vars(&client, &cat).await;
            cases.extend(catalog_cases(set, f, &vars, &cat));
        }
    }

    let mut stats: HashMap<&'static str, (u32, u32, u32)> = HashMap::new();
    for case in &cases {
        if let Some(o) = &only
            && !case.path.contains(o.as_str())
        {
            continue;
        }
        for who in &case.users {
            let auth = user_auth(who);
            for v in &case.versions {
                let accept = accept_for(v);
                let d = django(
                    &client,
                    case.method,
                    &case.path,
                    auth.as_deref(),
                    &accept,
                    case.body.as_deref(),
                )
                .await;
                let r = rust(
                    &app,
                    case.method,
                    &case.path,
                    auth.as_deref(),
                    &accept,
                    case.body.as_deref(),
                )
                .await;
                let e = stats.entry(case.set).or_insert((0, 0, 0));
                let same_meta =
                    d.status == r.status && d.ctype.split(';').next() == r.ctype.split(';').next();
                if same_meta && normalize(&d.body) == normalize(&r.body) {
                    e.0 += 1;
                } else if same_meta && tie_only(&d.body, &r.body) {
                    e.1 += 1;
                    println!("TIE  {} {who} v{v} {} {}", case.set, case.method, case.path);
                } else {
                    e.2 += 1;
                    // PARITY_FULL=1: print whole bodies (debugging)
                    let lim = if std::env::var("PARITY_FULL").is_ok() { usize::MAX } else { 900 };
                    println!(
                        "DIFF {} [{}] {who} v{v} {} {} {}",
                        case.set,
                        case.id,
                        case.method,
                        case.path.chars().take(300).collect::<String>(),
                        case.body.as_deref().unwrap_or("")
                    );
                    println!(
                        "  django {} {}: {}",
                        d.status,
                        d.ctype,
                        d.body.chars().take(lim).collect::<String>()
                    );
                    println!(
                        "  rust   {} {}: {}",
                        r.status,
                        r.ctype,
                        r.body.chars().take(lim).collect::<String>()
                    );
                }
            }
        }
    }
    let mut total_diff = 0;
    for (set, (same, tie, diff)) in &stats {
        println!("PARITY {set}: same={same} tie_only={tie} different={diff}");
        total_diff += diff;
    }
    assert_eq!(total_diff, 0);
}
