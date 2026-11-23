//! API v2 (P3/P3b) against a **copy** of the reference data: own DB `pl_p3_v2_test`
//! (TEMPLATE `paperless_snapshot`) with applied sync capture (P2 migration) and
//! device token migration, media in a temp directory. Never against `paperless` itself.
//!
//! ```text
//! PL_P3_TEST_DB=1 cargo test -p pl-server -j 2 --test api_v2_db -- --ignored --test-threads 1
//! ```
//! The DB copy is dropped at the end (`DROP DATABASE … WITH (FORCE)`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use http_body_util::BodyExt;
use pl_core::AppState;
use pl_core::config::Env;
use pl_server::config::ServerConfig;
use pl_server::routing::{self, Backend};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const DB: &str = "pl_p3_v2_test";
const ADMIN_URL: &str = "postgres://paperless:paperless@127.0.0.1:54329/postgres";
const SECRET: &str = "paperless-rs-reference-secret-key-local-only-0123456789";

const ADMIN: &str = "Token a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001";
const ALICE: &str = "Token a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002";
const BOB: &str = "Token b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b003";
const CAROL: &str = "Token c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c004";
const DAVE: &str = "Token d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d005";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

async fn recreate_db() -> PgPool {
    let admin = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(ADMIN_URL).await.expect("admin connect");
    sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&admin).await.expect("drop");
    sqlx::query(&format!("CREATE DATABASE {DB} TEMPLATE paperless_snapshot")).execute(&admin).await.expect("create");
    admin.close().await;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&format!("postgres://paperless:paperless@127.0.0.1:54329/{DB}"))
        .await
        .expect("db");
    // Tag tree as an adjacency list (pl_objects.0001, L6): without it the v3.4.2 copy requires the
    // treenode columns that PaperRust no longer writes
    for (_, sql) in pl_objects::migrations::ALL {
        sqlx::raw_sql(sql).execute(&pool).await.expect("pl_objects migration");
    }
    // documents_note.format (paperless-ngx 3.5 #14626): the notes routes read and write it
    for (_, sql) in pl_documents_write::migrations::ALL {
        sqlx::raw_sql(sql).execute(&pool).await.expect("note format migration");
    }
    pl_sync::migrations::apply(&pool).await.expect("sync migration (P2)");
    pl_authz::device::migrations::apply(&pool).await.expect("device token migration");
    pool
}

async fn drop_db(pool: PgPool) {
    pool.close().await;
    let admin = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(ADMIN_URL).await.expect("admin connect");
    sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&admin).await.expect("drop");
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), &t).unwrap();
        }
    }
}

fn state(pool: &PgPool, media: &Path, data: &Path, throttle: &str) -> AppState {
    let media_s = media.to_string_lossy().to_string();
    let data_s = data.to_string_lossy().to_string();
    let cfg = pl_core::Config::from_env_map(Env::from_pairs([
        ("PAPERLESS_SECRET_KEY", SECRET),
        ("PAPERLESS_DBHOST", "127.0.0.1"),
        ("PAPERLESS_DBPORT", "54329"),
        ("PAPERLESS_DBNAME", DB),
        ("PAPERLESS_DBUSER", "paperless"),
        ("PAPERLESS_DBPASS", "paperless"),
        ("PAPERLESS_TIME_ZONE", "Europe/Zurich"),
        ("PAPERLESS_MEDIA_ROOT", media_s.as_str()),
        ("PAPERLESS_DATA_DIR", data_s.as_str()),
        ("PAPERLESS_TOKEN_THROTTLE_RATE", throttle),
        ("PAPERLESS_ALLOWED_HOSTS", "localhost,127.0.0.1"),
    ]))
    .expect("config");
    AppState::with_pool(cfg, pool.clone()).expect("state")
}

/// App with all `/api/…` routes from routes.toml (v1) + API v2.
fn build(st: AppState, api_v2: bool) -> Router {
    let toml = routing::load_routes_file(&repo().join("routes.toml"), true).expect("routes.toml");
    let all = pl_server::all_routes();
    let api: BTreeMap<String, Backend> = all
        .iter()
        .map(|r| {
            let on = r.path.starts_with("/api/") && toml.get(r.id) != Some(&Backend::Off);
            (r.id.to_string(), if on { Backend::Rust } else { Backend::Off })
        })
        .collect();
    let plan = routing::plan(all, &api).expect("plan");
    let mut env: BTreeMap<&str, &str> = BTreeMap::new();
    env.insert("PAPERLESS_ALLOWED_HOSTS", "localhost,127.0.0.1");
    if !api_v2 {
        env.insert("PAPERRUST_API_V2", "off");
    }
    let cfg = ServerConfig::from_lookup(|k| env.get(k).map(|v| v.to_string())).expect("server config");
    pl_server::build_app(&cfg, plan, st).expect("app")
}

struct Resp {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|_| panic!("not json ({}): {}", self.status, String::from_utf8_lossy(&self.body)))
    }
    fn header(&self, k: &str) -> Option<String> {
        self.headers.get(k).and_then(|v| v.to_str().ok()).map(str::to_string)
    }
}

async fn call(app: &Router, method: Method, path: &str, auth: Option<&str>, body: Option<Value>, extra: &[(&str, &str)]) -> Resp {
    let mut b = Request::builder().method(method).uri(path).header("host", "localhost");
    if let Some(a) = auth {
        b = b.header("authorization", a);
    }
    for (k, v) in extra {
        b = b.header(*k, *v);
    }
    let req = match body {
        Some(v) => b.header("content-type", "application/json").body(Body::from(v.to_string())).unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let r = app.clone().oneshot(req).await.unwrap();
    let status = r.status();
    let headers = r.headers().clone();
    let body = r.into_body().collect().await.unwrap().to_bytes().to_vec();
    Resp { status, headers, body }
}

async fn get(app: &Router, path: &str, auth: &str) -> Resp {
    call(app, Method::GET, path, Some(auth), None, &[]).await
}

/// All pages of a v2 list (legacy_ids in delivery order).
async fn v2_all(app: &Router, path: &str, auth: &str, limit: usize) -> Vec<Value> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let sep = if path.contains('?') { '&' } else { '?' };
        let mut p = format!("{path}{sep}limit={limit}");
        if let Some(c) = &cursor {
            p.push_str(&format!("&cursor={c}"));
        }
        let r = get(app, &p, auth).await;
        assert_eq!(r.status, StatusCode::OK, "{p}: {}", String::from_utf8_lossy(&r.body));
        let v = r.json();
        let items = v["items"].as_array().unwrap();
        assert!(items.len() <= limit);
        out.extend(items.iter().cloned());
        match v["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
    }
    out
}

fn ids(items: &[Value]) -> Vec<i64> {
    items.iter().map(|i| i["legacy_id"].as_i64().unwrap()).collect()
}

async fn v1_ids(app: &Router, path: &str, auth: &str) -> Option<BTreeSet<i64>> {
    let r = get(app, &format!("{path}?page_size=100000"), auth).await;
    if r.status != StatusCode::OK {
        return None;
    }
    Some(r.json()["results"].as_array().unwrap().iter().map(|x| x["id"].as_i64().unwrap()).collect())
}

#[tokio::test]
#[ignore = "needs the reference PostgreSQL (127.0.0.1:54329) and creates the DB copy pl_p3_v2_test"]
async fn api_v2_against_db_copy() {
    if std::env::var("PL_P3_TEST_DB").is_err() {
        eprintln!("PL_P3_TEST_DB not set – skipped");
        return;
    }
    let pool = recreate_db().await;
    let tmp = tempfile::tempdir().unwrap();
    let media = tmp.path().join("media");
    let data = tmp.path().join("data");
    copy_dir(&repo().join("reference/data/media"), &media);
    copy_dir(&repo().join("reference/data/data"), &data);
    let st = state(&pool, &media, &data, "1000/min");
    let app = build(st.clone(), true);
    let mut n = 0usize;
    macro_rules! check {
        ($name:expr, $body:expr) => {{
            $body;
            n += 1;
            eprintln!("ok {}", $name);
        }};
    }

    // ---------------------------------------------------------------- server, auth
    check!("server anonymous", {
        let r = call(&app, Method::GET, "/rs/api/v2/server", None, None, &[]).await;
        assert_eq!(r.status, StatusCode::OK);
        let v = r.json();
        assert_eq!(v["product"], "paperrust");
        assert_eq!(v["api"], 2);
        assert!(v.get("server_uuid").is_none() && v.get("version").is_none());
        assert!(v["features"].as_array().unwrap().contains(&json!("sync")), "feed wired (P2)");
    });
    check!("server authenticated", {
        let v = get(&app, "/rs/api/v2/server", ADMIN).await.json();
        assert!(v["version"].is_string());
    });
    check!("me via v1 token", {
        let v = get(&app, "/rs/api/v2/me", ALICE).await.json();
        assert_eq!(v["username"], "alice");
        assert_eq!(v["legacy_id"], 3);
        assert_eq!(v["auth"], "token");
        assert!(v["permissions"].as_array().unwrap().contains(&json!("documents.view_document")));
        assert!(v["uuid"].as_str().unwrap().len() == 36);
    });
    check!("unauthenticated / bad / inactive", {
        for a in [None, Some("Token ffffffffffffffffffffffffffffffffffffffff"), Some(DAVE), Some("Bearer prt_short")] {
            let r = call(&app, Method::GET, "/rs/api/v2/me", a, None, &[]).await;
            assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{a:?}");
            assert_eq!(r.json()["error"]["code"], "not_authenticated");
        }
    });
    check!("unknown path and method are v2 errors", {
        let r = get(&app, "/rs/api/v2/nope", ADMIN).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);
        assert_eq!(r.json()["error"]["code"], "not_found");
        let r = call(&app, Method::DELETE, "/rs/api/v2/me", Some(ADMIN), None, &[]).await;
        assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
    });
    check!("session + CSRF shared with v1", {
        let (pw,): (String,) = sqlx::query_as("SELECT password FROM auth_user WHERE id = 3").fetch_one(&pool).await.unwrap();
        let sess = json!({
            "_auth_user_id": "3",
            "_auth_user_backend": "django.contrib.auth.backends.ModelBackend",
            "_auth_user_hash": pl_authz::signing::session_auth_hash(&pw, SECRET),
        });
        let data = pl_authz::signing::encode_session(&sess.to_string(), SECRET, chrono::Utc::now().timestamp());
        sqlx::query("INSERT INTO django_session (session_key, session_data, expire_date) VALUES ('p3v2testsession0001', $1, now() + interval '1 hour')")
            .bind(&data)
            .execute(&pool)
            .await
            .unwrap();
        let csrf = "abcdefghijklmnopqrstuvwxyz012345";
        let cookie = format!("sessionid=p3v2testsession0001; csrftoken={csrf}");
        let r = call(&app, Method::GET, "/rs/api/v2/me", None, None, &[("cookie", &cookie)]).await;
        assert_eq!(r.json()["auth"], "session");
        let r = call(&app, Method::GET, "/api/documents/?page_size=1", None, None, &[("cookie", &cookie)]).await;
        assert_eq!(r.status, StatusCode::OK, "same session valid for v1");
        let body = json!({"settings": {"p3": 1}});
        let r = call(&app, Method::PUT, "/rs/api/v2/ui_settings", None, Some(body.clone()), &[("cookie", &cookie)]).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "unsafe method without CSRF header");
        assert!(r.json()["error"]["message"].as_str().unwrap().contains("CSRF"));
        let r = call(&app, Method::PUT, "/rs/api/v2/ui_settings", None, Some(body), &[("cookie", &cookie), ("x-csrftoken", csrf)]).await;
        assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
        assert_eq!(r.json()["settings"]["p3"], 1);
        let v = get(&app, "/rs/api/v2/ui_settings", ALICE).await.json();
        assert_eq!(v["settings"]["p3"], 1);
    });

    // ---------------------------------------------------------------- Permission matrix v1 ↔ v2
    for (user, auth) in [("admin", ADMIN), ("alice", ALICE), ("bob", BOB), ("carol", CAROL)] {
        for (v1, v2) in [
            ("/api/tags/", "/rs/api/v2/tags"),
            ("/api/correspondents/", "/rs/api/v2/correspondents"),
            ("/api/document_types/", "/rs/api/v2/document_types"),
            ("/api/storage_paths/", "/rs/api/v2/storage_paths"),
            ("/api/saved_views/", "/rs/api/v2/saved_views"),
            ("/api/custom_fields/", "/rs/api/v2/custom_fields"),
            ("/api/documents/", "/rs/api/v2/documents"),
        ] {
            check!(format!("visibility {user} {v2}"), {
                let a = v1_ids(&app, v1, auth).await;
                let r = get(&app, &format!("{v2}?limit=1000"), auth).await;
                match a {
                    None => assert_eq!(r.status, StatusCode::FORBIDDEN, "{user} {v2}: v1 denied, v2 must deny"),
                    Some(a) => {
                        let b: BTreeSet<i64> = ids(&v2_all(&app, v2, auth, 1000).await).into_iter().collect();
                        assert_eq!(a, b, "{user} {v2}");
                    }
                }
            });
        }
        if user != "carol" {
            check!(format!("can_change v1 ↔ v2 {user}"), {
                let r = get(&app, "/api/documents/?page_size=100000", auth).await.json();
                let v1: BTreeMap<i64, bool> = r["results"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|d| (d["id"].as_i64().unwrap(), d["user_can_change"].as_bool().unwrap_or(false)))
                    .collect();
                let me = get(&app, "/rs/api/v2/me", auth).await.json();
                let has_model = me["permissions"].as_array().unwrap().contains(&json!("documents.change_document"));
                for d in v2_all(&app, "/rs/api/v2/documents", auth, 1000).await {
                    let id = d["legacy_id"].as_i64().unwrap();
                    assert_eq!(d["access"]["can_change"].as_bool().unwrap(), v1[&id] && has_model, "{user} doc {id}");
                }
            });
        }
    }

    // ---------------------------------------------------------------- Object level
    let alice_docs: BTreeSet<i64> = v1_ids(&app, "/api/documents/", ALICE).await.unwrap();
    let all_docs = v2_all(&app, "/rs/api/v2/documents", ADMIN, 1000).await;
    let hidden = all_docs.iter().find(|d| !alice_docs.contains(&d["legacy_id"].as_i64().unwrap())).unwrap().clone();
    let shown = all_docs.iter().find(|d| alice_docs.contains(&d["legacy_id"].as_i64().unwrap())).unwrap().clone();
    let hidden_uuid = hidden["uuid"].as_str().unwrap().to_string();
    check!("object-level: hidden document is 404 everywhere", {
        for p in [
            format!("/rs/api/v2/documents/{hidden_uuid}"),
            format!("/rs/api/v2/documents/{hidden_uuid}/files/original"),
            format!("/rs/api/v2/documents/{hidden_uuid}/notes"),
        ] {
            let r = get(&app, &p, ALICE).await;
            assert_eq!(r.status, StatusCode::NOT_FOUND, "{p}");
        }
        let r = call(
            &app,
            Method::PATCH,
            &format!("/rs/api/v2/documents/{hidden_uuid}"),
            Some(ALICE),
            Some(json!({"title": "x"})),
            &[("if-match", "\"1\"")],
        )
        .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "no 412 leak for invisible objects");
        let r = call(
            &app,
            Method::POST,
            "/rs/api/v2/objects/batch",
            Some(ALICE),
            Some(json!({"kind":"document","uuids":[hidden_uuid, shown["uuid"]]})),
            &[],
        )
        .await;
        let v = r.json();
        assert_eq!(v["items"].as_array().unwrap().len(), 1);
        assert_eq!(v["missing"], json!([hidden_uuid]));
    });
    check!("document detail with include", {
        let u = shown["uuid"].as_str().unwrap();
        let r = get(&app, &format!("/rs/api/v2/documents/{u}?include=content,notes"), ADMIN).await;
        assert_eq!(r.status, StatusCode::OK);
        let v = r.json();
        assert_eq!(r.header("etag").unwrap(), format!("\"{}\"", v["rev"]));
        assert!(v["content"].is_string());
        assert!(v["notes"].is_array());
        assert!(v["files"]["original"]["checksum"].is_string());
        assert!(v["created"].as_str().unwrap().len() == 10);
        assert!(v["added"].as_str().unwrap().ends_with('Z'));
        let r = get(&app, &format!("/rs/api/v2/documents/{u}?include=bogus"), ADMIN).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
    });

    // ---------------------------------------------------------------- Pagination
    check!("cursor pagination documents (all sort orders)", {
        let full = ids(&all_docs);
        for sort in ["-created", "created", "title", "-added", "modified", "id", "-id"] {
            let paged = ids(&v2_all(&app, &format!("/rs/api/v2/documents?sort={sort}"), ADMIN, 7).await);
            let set: BTreeSet<i64> = paged.iter().copied().collect();
            assert_eq!(set.len(), paged.len(), "{sort}: duplicates");
            assert_eq!(set, full.iter().copied().collect::<BTreeSet<_>>(), "{sort}: same set");
            let one = ids(&v2_all(&app, &format!("/rs/api/v2/documents?sort={sort}"), ADMIN, 1000).await);
            assert_eq!(paged, one, "{sort}: same order regardless of page size");
        }
        let r = get(&app, "/rs/api/v2/documents?sort=title&cursor=k1.bogus", ADMIN).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
        let c = get(&app, "/rs/api/v2/documents?limit=2&count=true", ADMIN).await.json();
        assert_eq!(c["count"].as_i64().unwrap() as usize, all_docs.len());
        let other = get(&app, &format!("/rs/api/v2/documents?sort=title&cursor={}", c["next_cursor"].as_str().unwrap()), ADMIN).await;
        assert_eq!(other.status, StatusCode::BAD_REQUEST, "cursor bound to its sort order");
    });
    check!("cursor pagination tags", {
        let a = ids(&v2_all(&app, "/rs/api/v2/tags", ALICE, 3).await);
        let b = ids(&v2_all(&app, "/rs/api/v2/tags", ALICE, 1000).await);
        assert_eq!(a, b);
        let d = ids(&v2_all(&app, "/rs/api/v2/tags?sort=-name", ALICE, 2).await);
        let mut rev = b.clone();
        rev.reverse();
        assert_eq!(d.len(), rev.len());
    });

    // ---------------------------------------------------------------- documents/query
    check!("documents/query matches v1 filters", {
        let tags = v2_all(&app, "/rs/api/v2/tags", ADMIN, 1000).await;
        let t = &tags[0];
        let tid = t["legacy_id"].as_i64().unwrap();
        let r = get(&app, &format!("/api/documents/?tags__id__all={tid}&title_content=a&page_size=100000"), ADMIN).await.json();
        let v1: BTreeSet<i64> = r["results"].as_array().unwrap().iter().map(|x| x["id"].as_i64().unwrap()).collect();
        let q = json!({"filter": {"all": [{"tags": {"all": [t["uuid"]]}}, {"text": {"query": "a"}}]}, "limit": 1000});
        let r = call(&app, Method::POST, "/rs/api/v2/documents/query", Some(ADMIN), Some(q), &[]).await;
        assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
        let v2: BTreeSet<i64> = ids(r.json()["items"].as_array().unwrap()).into_iter().collect();
        assert_eq!(v1, v2);
        let q = json!({"filter": {"correspondent": {"is_null": true}}, "limit": 1000});
        let r = call(&app, Method::POST, "/rs/api/v2/documents/query", Some(ALICE), Some(q), &[]).await.json();
        let v1 = get(&app, "/api/documents/?correspondent__isnull=1&page_size=100000", ALICE).await.json();
        assert_eq!(r["items"].as_array().unwrap().len(), v1["results"].as_array().unwrap().len());
        // Since data model stage 7 only the AST path: `any`/`not` work, `any: []` matches nothing
        // (docs/contract/pl-api-v2.md; formerly 501 via the old query path, test log B-319).
        let r = call(&app, Method::POST, "/rs/api/v2/documents/query", Some(ADMIN), Some(json!({"filter": {"any": []}})), &[]).await;
        assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
        assert_eq!(r.json()["items"].as_array().map(Vec::len), Some(0));
        let r = call(&app, Method::POST, "/rs/api/v2/documents/query", Some(ADMIN), Some(json!({"filtr": {}})), &[]).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
    });

    // ---------------------------------------------------------------- Files
    let su = shown["uuid"].as_str().unwrap().to_string();
    check!("files: ETag, 304, Range, 416, If-Range, HEAD", {
        let p = format!("/rs/api/v2/documents/{su}/files/original");
        let r = get(&app, &p, ALICE).await;
        assert_eq!(r.status, StatusCode::OK);
        let etag = r.header("etag").unwrap();
        assert_eq!(etag, format!("\"{}\"", shown["files"]["original"]["checksum"].as_str().unwrap()));
        assert_eq!(r.header("accept-ranges").as_deref(), Some("bytes"));
        assert!(r.header("content-disposition").unwrap().starts_with("attachment; filename*=UTF-8''"));
        assert_eq!(r.header("content-length").unwrap(), r.body.len().to_string());
        assert_eq!(Some(r.body.len() as u64), shown["files"]["original"]["size"].as_u64());
        let full = r.body.clone();
        let r = call(&app, Method::GET, &p, Some(ALICE), None, &[("if-none-match", &etag)]).await;
        assert_eq!(r.status, StatusCode::NOT_MODIFIED);
        assert!(r.body.is_empty());
        let r = call(&app, Method::GET, &p, Some(ALICE), None, &[("range", "bytes=0-9")]).await;
        assert_eq!(r.status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(r.body, full[..10]);
        assert_eq!(r.header("content-range").unwrap(), format!("bytes 0-9/{}", full.len()));
        let r = call(&app, Method::GET, &p, Some(ALICE), None, &[("range", "bytes=-5")]).await;
        assert_eq!(r.body, full[full.len() - 5..]);
        let r = call(&app, Method::GET, &p, Some(ALICE), None, &[("range", "bytes=99999999-")]).await;
        assert_eq!(r.status, StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(r.header("content-range").unwrap(), format!("bytes */{}", full.len()));
        let r = call(&app, Method::GET, &p, Some(ALICE), None, &[("range", "bytes=0-9"), ("if-range", "\"other\"")]).await;
        assert_eq!(r.status, StatusCode::OK, "If-Range mismatch → whole file");
        assert_eq!(r.body.len(), full.len());
        let r = call(&app, Method::HEAD, &p, Some(ALICE), None, &[]).await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(r.body.is_empty());
        assert_eq!(r.header("content-length").unwrap(), full.len().to_string());
        let r = get(&app, &format!("/rs/api/v2/documents/{su}/files/thumbnail"), ALICE).await;
        assert_eq!(r.status, StatusCode::OK);
        let te = r.header("etag").unwrap();
        assert!(te.starts_with("W/"));
        let r =
            call(&app, Method::GET, &format!("/rs/api/v2/documents/{su}/files/thumbnail"), Some(ALICE), None, &[("if-none-match", &te)])
                .await;
        assert_eq!(r.status, StatusCode::NOT_MODIFIED);
        let r = get(&app, &format!("/rs/api/v2/documents/{su}/files/preview"), ALICE).await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(r.header("content-disposition").unwrap().starts_with("inline;"));
        let r = get(&app, &format!("/rs/api/v2/documents/{su}/files/bogus"), ALICE).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);
        // v1 file is the same
        let r = get(&app, &format!("/api/documents/{}/download/?original=true", shown["legacy_id"]), ALICE).await;
        assert_eq!(r.body, full);
    });

    // ---------------------------------------------------------------- Writing + If-Match
    check!("tag create / update with If-Match / delete", {
        let r = call(
            &app,
            Method::POST,
            "/rs/api/v2/tags",
            Some(ALICE),
            Some(json!({"name": "p3-v2-tag", "color": "#112233", "matching": {"algorithm": "literal", "pattern": "xyz"}})),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", String::from_utf8_lossy(&r.body));
        let t = r.json();
        assert_eq!(t["owner"]["username"], "alice");
        assert_eq!(t["matching"]["algorithm"], "literal");
        assert_eq!(t["text_color"], "#ffffff");
        let u = t["uuid"].as_str().unwrap().to_string();
        let rev = t["rev"].as_i64().unwrap();
        let r = call(&app, Method::POST, "/rs/api/v2/tags", Some(ALICE), Some(json!({"name": "p3-v2-tag"})), &[]).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "duplicate name like v1");
        let r = call(
            &app,
            Method::PATCH,
            &format!("/rs/api/v2/tags/{u}"),
            Some(ALICE),
            Some(json!({"name": "p3-v2-tag-2"})),
            &[("if-match", &format!("\"{}\"", rev + 100))],
        )
        .await;
        assert_eq!(r.status, StatusCode::PRECONDITION_FAILED);
        assert_eq!(r.json()["error"]["current_rev"], rev);
        let r = call(
            &app,
            Method::PATCH,
            &format!("/rs/api/v2/tags/{u}"),
            Some(ALICE),
            Some(json!({"name": "p3-v2-tag-2"})),
            &[("if-match", &format!("\"{rev}\""))],
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
        let t2 = r.json();
        assert_eq!(t2["name"], "p3-v2-tag-2");
        assert!(t2["rev"].as_i64().unwrap() > rev, "rev increases (sync trigger)");
        // v1 sees the change
        let v1 = get(&app, &format!("/api/tags/{}/", t2["legacy_id"]), ALICE).await.json();
        assert_eq!(v1["name"], "p3-v2-tag-2");
        // Bob does not see Alice's tag → 404
        let r = call(&app, Method::DELETE, &format!("/rs/api/v2/tags/{u}"), Some(BOB), None, &[]).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);
        let r =
            call(&app, Method::DELETE, &format!("/rs/api/v2/tags/{u}"), Some(ALICE), None, &[("if-match", &format!("\"{rev}\""))]).await;
        assert_eq!(r.status, StatusCode::PRECONDITION_FAILED);
        // Alice has no model permission delete_tag (group editors): 403 like v1, access says so beforehand
        assert_eq!(t2["access"]["can_delete"], false);
        assert_eq!(t2["access"]["can_change"], true);
        let r = call(&app, Method::DELETE, &format!("/rs/api/v2/tags/{u}"), Some(ALICE), None, &[]).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
        assert_eq!(r.json()["error"]["code"], "permission_denied");
        let r = call(&app, Method::DELETE, &format!("/rs/api/v2/tags/{u}"), Some(ADMIN), None, &[]).await;
        assert_eq!(r.status, StatusCode::NO_CONTENT);
        assert_eq!(get(&app, &format!("/rs/api/v2/tags/{u}"), ALICE).await.status, StatusCode::NOT_FOUND);
        // Groups/users are not writable
        let r = call(&app, Method::POST, "/rs/api/v2/custom_fields", Some(ADMIN), Some(json!({"name": "x"})), &[]).await;
        assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
    });
    check!("document PATCH with If-Match, notes", {
        let d = get(&app, &format!("/rs/api/v2/documents/{su}"), ALICE).await.json();
        let rev = d["rev"].as_i64().unwrap();
        let r = call(
            &app,
            Method::PATCH,
            &format!("/rs/api/v2/documents/{su}"),
            Some(ALICE),
            Some(json!({"title": "P3 title"})),
            &[("if-match", "\"999999\"")],
        )
        .await;
        assert_eq!(r.status, StatusCode::PRECONDITION_FAILED);
        let tag = v2_all(&app, "/rs/api/v2/tags", ALICE, 1000).await[0].clone();
        let r = call(
            &app,
            Method::PATCH,
            &format!("/rs/api/v2/documents/{su}"),
            Some(ALICE),
            Some(json!({"title": "P3 title", "tags": [tag["uuid"]]})),
            &[("if-match", &format!("\"{rev}\""))],
        )
        .await;
        if d["access"]["can_change"].as_bool().unwrap() {
            assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
            let v = r.json();
            assert_eq!(v["title"], "P3 title");
            assert_eq!(v["tags"], json!([tag["uuid"]]));
            assert!(v["rev"].as_i64().unwrap() > rev);
            let v1 = get(&app, &format!("/api/documents/{}/", d["legacy_id"]), ALICE).await.json();
            assert_eq!(v1["title"], "P3 title");
            let r = call(
                &app,
                Method::PATCH,
                &format!("/rs/api/v2/documents/{su}"),
                Some(ALICE),
                Some(json!({"tags": ["00000000-0000-0000-0000-000000000000"]})),
                &[],
            )
            .await;
            assert_eq!(r.status, StatusCode::BAD_REQUEST);
            assert!(r.json()["error"]["fields"]["tags"].is_array());
        } else {
            assert_eq!(r.status, StatusCode::FORBIDDEN);
        }
        let r =
            call(&app, Method::POST, &format!("/rs/api/v2/documents/{su}/notes"), Some(ALICE), Some(json!({"note": "p3 note"})), &[]).await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", String::from_utf8_lossy(&r.body));
        let notes = r.json()["items"].as_array().unwrap().clone();
        let mine = notes.iter().find(|n| n["note"] == "p3 note").unwrap();
        assert_eq!(mine["user"]["username"], "alice");
        let r = call(&app, Method::DELETE, &format!("/rs/api/v2/documents/{su}/notes/{}", mine["legacy_id"]), Some(ALICE), None, &[]).await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(!r.json()["items"].as_array().unwrap().iter().any(|n| n["note"] == "p3 note"));
    });

    // ---------------------------------------------------------------- Device tokens (P3b)
    check!("device tokens: issue, use on v1+v2, hash only, list, revoke immediately", {
        let r = call(
            &app,
            Method::POST,
            "/rs/api/v2/auth/devices",
            None,
            Some(json!({"username": "alice", "password": "wrong", "device_name": "Laptop"})),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
        let r = call(
            &app,
            Method::POST,
            "/rs/api/v2/auth/devices",
            None,
            Some(json!({"username": "alice", "password": "alice", "device_name": "Laptop"})),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", String::from_utf8_lossy(&r.body));
        let v = r.json();
        let token = v["token"].as_str().unwrap().to_string();
        assert!(token.starts_with("prt_") && token.len() == 47);
        let dev = v["device"]["uuid"].as_str().unwrap().to_string();
        let bearer = format!("Bearer {token}");
        let me = get(&app, "/rs/api/v2/me", &bearer).await.json();
        assert_eq!(me["username"], "alice");
        assert_eq!(me["auth"], "device_token");
        assert_eq!(get(&app, "/api/documents/?page_size=1", &bearer).await.status, StatusCode::OK, "valid for v1 too");
        let (stored,): (String,) = sqlx::query_as("SELECT token_hash FROM paperrust_device_token WHERE uuid = $1::uuid")
            .bind(&dev)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, pl_authz::device::hash(&token));
        let (plain,): (i64,) = sqlx::query_as("SELECT count(*) FROM paperrust_device_token WHERE token_hash = $1 OR name = $1")
            .bind(&token)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(plain, 0, "no plaintext token in the DB");
        let list = get(&app, "/rs/api/v2/auth/devices", ALICE).await.json();
        assert!(list["items"].as_array().unwrap().iter().any(|d| d["uuid"] == dev.as_str()));
        assert!(
            get(&app, "/rs/api/v2/auth/devices", BOB).await.json()["items"].as_array().unwrap().iter().all(|d| d["uuid"] != dev.as_str())
        );
        let r = call(&app, Method::DELETE, &format!("/rs/api/v2/auth/devices/{dev}"), Some(BOB), None, &[]).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "foreign device");
        let r = call(&app, Method::DELETE, &format!("/rs/api/v2/auth/devices/{dev}"), Some(&bearer), None, &[]).await;
        assert_eq!(r.status, StatusCode::NO_CONTENT);
        assert_eq!(get(&app, "/rs/api/v2/me", &bearer).await.status, StatusCode::UNAUTHORIZED, "revocation is immediate");
        assert_eq!(get(&app, "/api/documents/?page_size=1", &bearer).await.status, StatusCode::UNAUTHORIZED);
    });
    check!("device tokens: MFA required", {
        sqlx::query("INSERT INTO mfa_authenticator (user_id, type, data, created_at, last_used_at) VALUES (4, 'totp', '{\"secret\": \"JBSWY3DPEHPK3PXP\"}', now(), NULL)")
            .execute(&pool)
            .await
            .unwrap();
        let r = call(
            &app,
            Method::POST,
            "/rs/api/v2/auth/devices",
            None,
            Some(json!({"username": "bob", "password": "bob", "device_name": "Phone"})),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
        assert!(r.json()["error"]["fields"]["code"].is_array());
        let r = call(
            &app,
            Method::POST,
            "/rs/api/v2/auth/devices",
            None,
            Some(json!({"username": "bob", "password": "bob", "device_name": "Phone", "code": "000000"})),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
        sqlx::query("DELETE FROM mfa_authenticator WHERE user_id = 4").execute(&pool).await.unwrap();
    });
    check!("device tokens: login throttle like /api/token/", {
        let st2 = state(&pool, &media, &data, "2/min");
        let app2 = build(st2, true);
        let mut last = StatusCode::OK;
        for _ in 0..3 {
            last = call(
                &app2,
                Method::POST,
                "/rs/api/v2/auth/devices",
                None,
                Some(json!({"username": "alice", "password": "wrong", "device_name": "x"})),
                &[],
            )
            .await
            .status;
        }
        assert_eq!(last, StatusCode::TOO_MANY_REQUESTS);
    });

    // ---------------------------------------------------------------- Sync (P2 parts)
    check!("sync: changes (P2 feed), ids, resolve", {
        // number the captured changes (otherwise the sequencer in the worker does that)
        pl_sync::sequence_once(&pool, 50_000).await.unwrap();
        let r = get(&app, "/rs/api/v2/sync/changes?kinds=tag&limit=1000", ADMIN).await;
        assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
        let full = r.json();
        assert!(!full["changes"].as_array().unwrap().is_empty(), "initial state of all tags");
        let cursor = full["next_cursor"].as_str().unwrap().to_string();
        assert!(cursor.starts_with("c1."));
        // change bypassing the DB layer (like Django) → in the feed after the cursor
        let tag_id: i32 = sqlx::query_scalar("SELECT min(id) FROM documents_tag").fetch_one(&pool).await.unwrap();
        sqlx::query("UPDATE documents_tag SET name = name || ' (sync)' WHERE id = $1").bind(tag_id).execute(&pool).await.unwrap();
        pl_sync::sequence_once(&pool, 50_000).await.unwrap();
        let v = get(&app, &format!("/rs/api/v2/sync/changes?kinds=tag&cursor={cursor}"), ADMIN).await.json();
        let ch = v["changes"].as_array().unwrap();
        assert_eq!(ch.len(), 1, "{v}");
        assert_eq!(ch[0]["op"], "upsert");
        assert_eq!(ch[0]["object"]["legacy_id"], json!(tag_id));
        assert!(ch[0]["object"]["name"].as_str().unwrap().ends_with(" (sync)"));
        // expired cursor → 410
        sqlx::query("UPDATE paperrust_sync_meta SET value = to_jsonb(1000000) WHERE key = 'min_cursor'").execute(&pool).await.unwrap();
        let r = get(&app, &format!("/rs/api/v2/sync/changes?cursor={cursor}"), ADMIN).await;
        assert_eq!(r.status, StatusCode::GONE);
        sqlx::query("UPDATE paperrust_sync_meta SET value = to_jsonb(0) WHERE key = 'min_cursor'").execute(&pool).await.unwrap();
        let v = get(&app, "/rs/api/v2/sync/ids?kind=document", ALICE).await.json();
        assert_eq!(v["items"].as_array().unwrap().len(), alice_docs.len());
        assert!(v["as_of_seq"].as_i64().unwrap() > 0);
        let tag = v2_all(&app, "/rs/api/v2/tags", ALICE, 1000).await[0].clone();
        let r = call(
            &app,
            Method::POST,
            "/rs/api/v2/sync/resolve",
            Some(ALICE),
            Some(json!({"tags": [tag["uuid"]], "documents": [hidden_uuid]})),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
        let res = r.json();
        let v1 = get(&app, &format!("/api/documents/?tags__id__in={}&page_size=100000", tag["legacy_id"]), ALICE).await.json();
        assert_eq!(res["documents"].as_array().unwrap().len(), v1["results"].as_array().unwrap().len(), "hidden doc not added");
    });

    // ---------------------------------------------------------------- Switch
    check!("PAPERRUST_API_V2=off", {
        let off = build(st.clone(), false);
        let r = call(&off, Method::GET, "/rs/api/v2/server", None, None, &[]).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);
        assert!(serde_json::from_slice::<Value>(&r.body).map(|v| v.get("product").is_none()).unwrap_or(true));
    });

    eprintln!("{n} checks passed");
    drop(app);
    drop(st);
    drop_db(pool).await;
}
