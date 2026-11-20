//! PaperRust UI (design 2): life areas, timeline, upcoming, reminders,
//! done/snooze, `/ask` (AI off and with mock LLM) against a **copy** of the
//! reference data (`pl_design2_test`, TEMPLATE `paperless_snapshot`), media/index in a
//! temp directory. Never against `paperless` itself; no real LLM (local Ollama mock).
//!
//! ```text
//! PL_DESIGN2_TEST_DB=1 cargo test -p pl-server -j 4 --test design2_db -- --ignored --test-threads 1
//! ```
//! The DB copy is dropped at the end (`DROP DATABASE … WITH (FORCE)`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use chrono::{Duration, NaiveDate, Utc};
use http_body_util::BodyExt;
use pl_core::AppState;
use pl_core::config::Env;
use pl_server::config::ServerConfig;
use pl_server::routing::{self, Backend};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const DB: &str = "pl_design2_test";
const ADMIN_URL: &str = "postgres://paperless:paperless@127.0.0.1:54329/postgres";
const SECRET: &str = "paperless-rs-reference-secret-key-local-only-0123456789";

const ADMIN: &str = "Token a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001";
const ALICE: &str = "Token a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002";
const BOB: &str = "Token b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b003";
const CAROL: &str = "Token c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c004";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

async fn recreate_db() -> PgPool {
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(ADMIN_URL)
        .await
        .expect("admin connect");
    sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)"))
        .execute(&admin)
        .await
        .expect("drop");
    sqlx::query(&format!("CREATE DATABASE {DB} TEMPLATE paperless_snapshot"))
        .execute(&admin)
        .await
        .expect("create");
    admin.close().await;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .connect(&format!(
            "postgres://paperless:paperless@127.0.0.1:54329/{DB}"
        ))
        .await
        .expect("db");
    pl_sync::migrations::apply(&pool)
        .await
        .expect("sync migration (P2)");
    pl_authz::device::migrations::apply(&pool)
        .await
        .expect("device token migration");
    pool
}

async fn drop_db(pool: PgPool) {
    pool.close().await;
    let admin = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(ADMIN_URL)
        .await
        .expect("admin connect");
    sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)"))
        .execute(&admin)
        .await
        .expect("drop");
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

fn state(pool: &PgPool, media: &Path, data: &Path) -> AppState {
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
        ("PAPERLESS_TOKEN_THROTTLE_RATE", "1000/min"),
        ("PAPERLESS_ALLOWED_HOSTS", "localhost,127.0.0.1"),
    ]))
    .expect("config");
    AppState::with_pool(cfg, pool.clone()).expect("state")
}

fn build(st: AppState) -> Router {
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
    let cfg =
        ServerConfig::from_lookup(|k| env.get(k).map(|v| v.to_string())).expect("server config");
    pl_server::build_app(&cfg, plan, st).expect("app")
}

struct Resp {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|_| {
            panic!(
                "not json ({}): {}",
                self.status,
                String::from_utf8_lossy(&self.body)
            )
        })
    }
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

async fn call(
    app: &Router,
    method: Method,
    path: &str,
    auth: &str,
    body: Option<Value>,
    extra: &[(&str, &str)],
) -> Resp {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost")
        .header("authorization", auth);
    for (k, v) in extra {
        b = b.header(*k, *v);
    }
    let req = match body {
        Some(v) => b
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    };
    let r = app.clone().oneshot(req).await.unwrap();
    let status = r.status();
    let headers = r.headers().clone();
    let body = r.into_body().collect().await.unwrap().to_bytes().to_vec();
    Resp {
        status,
        headers,
        body,
    }
}

async fn get(app: &Router, path: &str, auth: &str) -> Resp {
    call(app, Method::GET, path, auth, None, &[]).await
}

/// All visible documents (v2 list): legacy_id → uuid.
async fn v2_docs(app: &Router, auth: &str) -> Option<BTreeMap<i64, String>> {
    let mut out = BTreeMap::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut p = "/rs/api/v2/documents?limit=1000".to_string();
        if let Some(c) = &cursor {
            p.push_str(&format!("&cursor={c}"));
        }
        let r = get(app, &p, auth).await;
        if r.status == StatusCode::FORBIDDEN {
            return None;
        }
        assert_eq!(r.status, StatusCode::OK, "{}", r.text());
        let v = r.json();
        for d in v["items"].as_array().unwrap() {
            out.insert(
                d["legacy_id"].as_i64().unwrap(),
                d["uuid"].as_str().unwrap().to_string(),
            );
        }
        match v["next_cursor"].as_str() {
            Some(c) => cursor = Some(c.to_string()),
            None => break,
        }
    }
    Some(out)
}

/// Document ids of all entries (all lanes).
fn timeline_docs(v: &Value) -> BTreeSet<i64> {
    v["lanes"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|l| {
            l["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| i["document"]["legacy_id"].as_i64().unwrap())
        })
        .collect()
}

fn items_of(v: &Value) -> Vec<Value> {
    v["lanes"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|l| l["items"].as_array().unwrap().clone())
        .collect()
}

/// SSE text → (event, data).
fn parse_sse(s: &str) -> Vec<(String, Value)> {
    s.split("\n\n")
        .filter(|b| !b.trim().is_empty())
        .map(|b| {
            let mut ev = String::new();
            let mut data = String::new();
            for line in b.lines() {
                if let Some(x) = line.strip_prefix("event: ") {
                    ev = x.to_string();
                } else if let Some(x) = line.strip_prefix("data: ") {
                    data.push_str(x);
                }
            }
            (
                ev,
                serde_json::from_str(&data).unwrap_or_else(|_| panic!("bad data: {data}")),
            )
        })
        .collect()
}

async fn mock_ollama(answer: &'static str) -> String {
    let app = Router::new().route(
        "/api/chat",
        axum::routing::post(move |axum::Json(body): axum::Json<Value>| async move {
            let prompt = body.to_string();
            assert!(
                prompt.contains("[1]"),
                "numbered sources in the prompt: {prompt}"
            );
            let lines = format!(
                "{}\n{}\n{}\n",
                json!({"message": {"role": "assistant", "content": answer}, "done": false}),
                json!({"message": {"role": "assistant", "content": " Ende."}, "done": false}),
                json!({"message": {"role": "assistant", "content": ""}, "done": true})
            );
            ([("content-type", "application/x-ndjson")], lines)
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

fn set_env(k: &str, v: Option<&str>) {
    // Test runs with --test-threads 1; settings are read live by pl_core::config::var.
    unsafe {
        match v {
            Some(v) => std::env::set_var(k, v),
            None => std::env::remove_var(k),
        }
    }
}

fn de(d: NaiveDate) -> String {
    d.format("%d.%m.%Y").to_string()
}

#[tokio::test]
#[ignore = "needs the reference PostgreSQL (127.0.0.1:54329) and creates the DB copy pl_design2_test"]
async fn design2_against_db_copy() {
    if std::env::var("PL_DESIGN2_TEST_DB").is_err() {
        eprintln!("PL_DESIGN2_TEST_DB not set – skipped");
        return;
    }
    set_env("PAPERRUST_TIMELINE_EXTRACT_DATES", None);
    set_env("PAPERRUST_TIMELINE_DATE_FIELDS", None);
    let pool = recreate_db().await;
    let tmp = tempfile::tempdir().unwrap();
    let media = tmp.path().join("media");
    let data = tmp.path().join("data");
    copy_dir(&repo().join("reference/data/media"), &media);
    copy_dir(&repo().join("reference/data/data"), &data);
    let st = state(&pool, &media, &data);
    let app = build(st.clone());
    let today = Utc::now().with_timezone(&st.config().tz).date_naive();
    let mut n = 0usize;
    macro_rules! check {
        ($name:expr, $body:expr) => {{
            $body;
            n += 1;
            eprintln!("ok {}", $name);
        }};
    }

    // ---------------------------------------------------------------- Data basis
    let users = [
        ("admin", ADMIN),
        ("alice", ALICE),
        ("bob", BOB),
        ("carol", CAROL),
    ];
    let mut visible: BTreeMap<&str, Option<BTreeMap<i64, String>>> = BTreeMap::new();
    for (u, a) in users {
        visible.insert(u, v2_docs(&app, a).await);
    }
    let all = visible["admin"].clone().unwrap();
    let alice = visible["alice"].clone().expect("alice sees documents");
    let hidden = *all
        .keys()
        .find(|i| !alice.contains_key(i))
        .expect("a document alice cannot see");
    let shown = *alice.keys().next().unwrap();
    let shown2 = *alice.keys().nth(1).expect("two documents for alice");
    let (min_c, max_c): (NaiveDate, NaiveDate) = sqlx::query_as(
        "SELECT min(created), max(created) FROM documents_document WHERE deleted_at IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    // Custom fields (date + amount) and content with deadlines
    let field = |name: &'static str, t: &'static str| {
        let pool = pool.clone();
        async move {
            let id: i32 = sqlx::query_scalar("INSERT INTO documents_customfield (created, name, data_type) VALUES (now(), $1, $2) RETURNING id")
                .bind(name)
                .bind(t)
                .fetch_one(&pool)
                .await
                .unwrap();
            id
        }
    };
    let f_due = field("Fällig am", "date").await;
    let f_until = field("Gültig bis", "date").await;
    let f_term = field("Termin", "date").await;
    let f_amount = field("Betrag", "monetary").await;
    let inst = |doc: i64, f: i32, date: Option<NaiveDate>, money: Option<&'static str>| {
        let pool = pool.clone();
        async move {
            sqlx::query("INSERT INTO documents_customfieldinstance (created, document_id, field_id, value_date, value_monetary) VALUES (now(), $1, $2, $3, $4)")
                .bind(doc as i32)
                .bind(f)
                .bind(date)
                .bind(money)
                .execute(&pool)
                .await
                .unwrap();
        }
    };
    let due_shown = today + Duration::days(10);
    let due_hidden = today + Duration::days(12);
    inst(shown, f_due, Some(due_shown), None).await;
    inst(shown, f_amount, None, Some("CHF89.90")).await;
    inst(shown2, f_until, Some(today + Duration::days(400)), None).await;
    inst(shown2, f_term, Some(today + Duration::days(5)), None).await;
    inst(hidden, f_due, Some(due_hidden), None).await;

    // ---------------------------------------------------------------- Permissions: timeline = document list
    for (u, a) in users {
        check!(format!("timeline visibility {u} equals document list"), {
            match &visible[u] {
                None => {
                    assert_eq!(
                        get(&app, "/rs/api/v2/timeline", a).await.status,
                        StatusCode::FORBIDDEN
                    );
                    assert_eq!(
                        get(&app, "/rs/api/v2/timeline/upcoming", a).await.status,
                        StatusCode::FORBIDDEN
                    );
                    let r = call(
                        &app,
                        Method::POST,
                        "/rs/api/v2/ask",
                        a,
                        Some(json!({"question": "x"})),
                        &[],
                    )
                    .await;
                    assert_eq!(r.status, StatusCode::FORBIDDEN);
                }
                Some(list) => {
                    let mut seen = BTreeSet::new();
                    let mut from = min_c;
                    while from <= max_c {
                        let to = from + Duration::days(3600);
                        let r = get(
                            &app,
                            &format!(
                                "/rs/api/v2/timeline?from={from}&to={to}&kinds=created&limit=10000"
                            ),
                            a,
                        )
                        .await;
                        assert_eq!(r.status, StatusCode::OK, "{}", r.text());
                        assert_eq!(r.json()["truncated"], false);
                        seen.extend(timeline_docs(&r.json()));
                        from = to + Duration::days(1);
                    }
                    let want: BTreeSet<i64> = list.keys().copied().collect();
                    assert_eq!(
                        seen, want,
                        "{u}: timeline (created) must show exactly the listed documents"
                    );
                    // Deadlines: the hidden document never for users without visibility
                    let up = get(&app, "/rs/api/v2/timeline/upcoming?days=60", a)
                        .await
                        .json();
                    let ids: BTreeSet<i64> = up["items"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|i| i["document"]["legacy_id"].as_i64().unwrap())
                        .collect();
                    assert_eq!(
                        ids.contains(&hidden),
                        list.contains_key(&hidden),
                        "{u}: {up}"
                    );
                }
            }
        });
    }
    let shown_uuid = alice[&shown].clone();
    let hidden_uuid = all[&hidden].clone();

    check!(
        "timeline items: custom field kinds, amount, names, spans",
        {
            let v = get(
                &app,
                &format!(
                    "/rs/api/v2/timeline?from={}&to={}",
                    today,
                    today + Duration::days(500)
                ),
                ALICE,
            )
            .await
            .json();
            let it = items_of(&v);
            let due = it
                .iter()
                .find(|i| i["document"]["legacy_id"] == shown && i["kind"] == "due")
                .expect("due item");
            assert_eq!(due["date"], due_shown.to_string());
            assert_eq!(due["source"], "custom_field");
            assert_eq!(
                due["id"],
                format!("{shown_uuid}.due.{}", due_shown.format("%Y%m%d"))
            );
            assert_eq!(due["amount"]["value"], "89.90");
            assert_eq!(due["amount"]["currency"], "CHF");
            assert_eq!(due["state"]["done"], false);
            let vu = it
                .iter()
                .find(|i| i["document"]["legacy_id"] == shown2 && i["kind"] == "valid_until")
                .expect("valid_until (Feldname Gültig bis)");
            assert!(vu["span"]["start"].is_string(), "Laufzeit-Spanne: {vu}");
            assert!(
                !it.iter().any(|i| i["document"]["legacy_id"] == shown2
                    && i["date"] == (today + Duration::days(5)).to_string()),
                "„Termin“ ist keine Frist"
            );
            assert_eq!(v["areas_source"], "none");
            assert_eq!(
                v["lanes"].as_array().unwrap().len(),
                1,
                "nur „other“ ohne Bereiche"
            );
            assert_eq!(v["lanes"][0]["key"], "other");
        }
    );

    check!("date field mapping via setting", {
        set_env(
            "PAPERRUST_TIMELINE_DATE_FIELDS",
            Some(r#"{"cancel_by":["Termin"]}"#),
        );
        let v = get(&app, "/rs/api/v2/timeline/upcoming?days=30", ALICE)
            .await
            .json();
        assert!(
            v["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["kind"] == "cancel_by" && i["document"]["legacy_id"] == shown2),
            "{v}"
        );
        let c = get(&app, "/rs/api/v2/timeline/config", ALICE).await.json();
        let term = c["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["legacy_id"] == f_term)
            .unwrap()
            .clone();
        assert_eq!(
            (term["kind"].as_str(), term["source"].as_str()),
            (Some("cancel_by"), Some("setting"))
        );
        let due = c["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["legacy_id"] == f_due)
            .unwrap()
            .clone();
        assert_eq!(due["source"], "heuristic");
        assert_eq!(c["extract_dates"], false);
        set_env("PAPERRUST_TIMELINE_DATE_FIELDS", None);
    });

    check!("input validation", {
        for p in [
            "/rs/api/v2/timeline?from=2026-13-01",
            "/rs/api/v2/timeline?from=2020-01-01&to=2019-01-01",
            "/rs/api/v2/timeline?from=2000-01-01&to=2020-01-01",
            "/rs/api/v2/timeline?kinds=due,nope",
            "/rs/api/v2/timeline?areas=xyz",
            "/rs/api/v2/timeline?limit=0",
            "/rs/api/v2/timeline/upcoming?days=0",
        ] {
            let r = get(&app, p, ALICE).await;
            assert_eq!(r.status, StatusCode::BAD_REQUEST, "{p}: {}", r.text());
            assert_eq!(r.json()["error"]["code"], "invalid");
        }
    });

    // ---------------------------------------------------------------- Done / snooze
    let due_id = format!("{shown_uuid}.due.{}", due_shown.format("%Y%m%d"));
    check!("mark done / snooze / reset", {
        let up_has = |v: &Value| {
            v["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["id"] == due_id)
        };
        let r = call(
            &app,
            Method::PUT,
            &format!("/rs/api/v2/timeline/items/{due_id}/state"),
            ALICE,
            Some(json!({"done": true})),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text());
        assert_eq!(r.json()["done"], true);
        assert!(!up_has(
            &get(&app, "/rs/api/v2/timeline/upcoming", ALICE)
                .await
                .json()
        ));
        let inc = get(
            &app,
            "/rs/api/v2/timeline/upcoming?include_done=true",
            ALICE,
        )
        .await
        .json();
        assert!(
            inc["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["id"] == due_id && i["state"]["done"] == true)
        );
        // per user: the admin still sees it open
        assert!(up_has(
            &get(&app, "/rs/api/v2/timeline/upcoming", ADMIN)
                .await
                .json()
        ));
        let r = call(
            &app,
            Method::PUT,
            &format!("/rs/api/v2/timeline/items/{due_id}/state"),
            ALICE,
            Some(json!({"done": false, "snooze_days": 30})),
            &[],
        )
        .await;
        assert_eq!(
            r.json()["snoozed_until"],
            (today + Duration::days(30)).to_string()
        );
        assert!(
            !up_has(
                &get(&app, "/rs/api/v2/timeline/upcoming", ALICE)
                    .await
                    .json()
            ),
            "zurückgestellt"
        );
        let r = call(
            &app,
            Method::DELETE,
            &format!("/rs/api/v2/timeline/items/{due_id}/state"),
            ALICE,
            None,
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::NO_CONTENT);
        assert!(up_has(
            &get(&app, "/rs/api/v2/timeline/upcoming", ALICE)
                .await
                .json()
        ));
        // Error
        let r = call(
            &app,
            Method::PUT,
            &format!("/rs/api/v2/timeline/items/{due_id}/state"),
            ALICE,
            Some(json!({})),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
        let hid = format!("{hidden_uuid}.due.{}", due_hidden.format("%Y%m%d"));
        let r = call(
            &app,
            Method::PUT,
            &format!("/rs/api/v2/timeline/items/{hid}/state"),
            ALICE,
            Some(json!({"done": true})),
            &[],
        )
        .await;
        assert_eq!(
            r.status,
            StatusCode::NOT_FOUND,
            "Objektebene: fremdes Dokument"
        );
        let r = call(
            &app,
            Method::PUT,
            "/rs/api/v2/timeline/items/garbage/state",
            ALICE,
            Some(json!({"done": true})),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND);
    });

    // ---------------------------------------------------------------- Reminders
    check!("reminders: private, idempotent, object-level", {
        let d = today + Duration::days(20);
        let body = json!({"document": shown_uuid, "kind": "cancel_by", "date": d.to_string(), "note": "Vertrag prüfen"});
        let r = call(
            &app,
            Method::POST,
            "/rs/api/v2/timeline/reminders",
            ALICE,
            Some(body.clone()),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
        let rem = r.json();
        assert_eq!(
            rem["item_id"],
            format!("{shown_uuid}.cancel_by.{}", d.format("%Y%m%d"))
        );
        let r = call(
            &app,
            Method::POST,
            "/rs/api/v2/timeline/reminders",
            ALICE,
            Some(body),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(r.json()["id"], rem["id"]);
        let up = get(&app, "/rs/api/v2/timeline/upcoming", ALICE)
            .await
            .json();
        let it = up["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == rem["item_id"])
            .expect("reminder in Demnächst")
            .clone();
        assert_eq!(it["source"], "user");
        assert_eq!(it["reminder"]["note"], "Vertrag prüfen");
        let up_admin = get(&app, "/rs/api/v2/timeline/upcoming", ADMIN)
            .await
            .json();
        assert!(
            !up_admin["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|i| i["id"] == rem["item_id"]),
            "Erinnerungen sind privat"
        );
        assert_eq!(
            get(&app, "/rs/api/v2/timeline/reminders", ALICE)
                .await
                .json()["items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let r = call(
            &app,
            Method::DELETE,
            &format!("/rs/api/v2/timeline/reminders/{}", rem["id"]),
            ADMIN,
            None,
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "fremde Erinnerung");
        let r = call(
            &app,
            Method::POST,
            "/rs/api/v2/timeline/reminders",
            ALICE,
            Some(json!({"document": hidden_uuid, "date": d.to_string()})),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "unsichtbares Dokument");
        let r = call(
            &app,
            Method::POST,
            "/rs/api/v2/timeline/reminders",
            ALICE,
            Some(json!({"document": shown_uuid, "kind": "created", "date": d.to_string()})),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
        let r = call(
            &app,
            Method::DELETE,
            &format!("/rs/api/v2/timeline/reminders/{}", rem["id"]),
            ALICE,
            None,
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::NO_CONTENT);
    });

    // ---------------------------------------------------------------- Life areas
    let alice_tags: BTreeMap<String, Value> = {
        let v = get(&app, "/rs/api/v2/tags?limit=1000", ALICE).await.json();
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| (t["uuid"].as_str().unwrap().to_string(), t.clone()))
            .collect()
    };
    let admin_tags: Vec<Value> =
        get(&app, "/rs/api/v2/tags?limit=1000", ADMIN).await.json()["items"]
            .as_array()
            .unwrap()
            .clone();
    // a tag with a visible document for alice
    let (tag_uuid, tag_id): (String, i64) = {
        let rows: Vec<(i32,)> =
            sqlx::query_as("SELECT tag_id FROM documents_document_tags WHERE document_id = $1")
                .bind(shown as i32)
                .fetch_all(&pool)
                .await
                .unwrap();
        let t = admin_tags
            .iter()
            .find(|t| {
                rows.iter()
                    .any(|r| i64::from(r.0) == t["legacy_id"].as_i64().unwrap())
                    && alice_tags.contains_key(t["uuid"].as_str().unwrap())
            })
            .or_else(|| {
                admin_tags
                    .iter()
                    .find(|t| alice_tags.contains_key(t["uuid"].as_str().unwrap()))
            })
            .expect("a tag alice can see")
            .clone();
        (
            t["uuid"].as_str().unwrap().to_string(),
            t["legacy_id"].as_i64().unwrap(),
        )
    };
    let secret_tag = admin_tags
        .iter()
        .find(|t| !alice_tags.contains_key(t["uuid"].as_str().unwrap()))
        .map(|t| t["uuid"].as_str().unwrap().to_string());
    check!("life areas: default none, suggestions never stored", {
        let v = get(&app, "/rs/api/v2/life-areas", ALICE).await.json();
        assert_eq!(v["source"], "none");
        let s = get(&app, "/rs/api/v2/life-areas/suggestions", ALICE).await;
        assert_eq!(s.status, StatusCode::OK, "{}", s.text());
        for it in s.json()["items"].as_array().unwrap() {
            for u in it["rules"]["tags"].as_array().unwrap() {
                assert!(
                    alice_tags.contains_key(u.as_str().unwrap()),
                    "Vorschlag nur aus sichtbaren Tags"
                );
            }
        }
        let cnt: i64 = sqlx::query_scalar("SELECT count(*) FROM paperrust_life_area")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(cnt, 0, "Vorschläge werden nie gespeichert");
        assert_eq!(
            get(&app, "/rs/api/v2/life-areas/suggestions?lang=fr", ALICE)
                .await
                .status,
            StatusCode::BAD_REQUEST
        );
    });
    let area_uuid: String;
    check!("life areas: CRUD, scopes, permissions", {
        let body = json!({"items": [{"name": "Wohnen", "icon": "home", "rules": {"tags": [tag_uuid]}}, {"name": "Leer"}]});
        let r = call(
            &app,
            Method::PUT,
            "/rs/api/v2/life-areas?scope=instance",
            ALICE,
            Some(body.clone()),
            &[],
        )
        .await;
        assert_eq!(
            r.status,
            StatusCode::FORBIDDEN,
            "nur Superuser schreibt die Vorgabe"
        );
        let r = call(
            &app,
            Method::PUT,
            "/rs/api/v2/life-areas?scope=instance",
            ADMIN,
            Some(json!({"items": [{"name": "Vorgabe", "color": "#112233"}]})),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text());
        assert_eq!(
            get(&app, "/rs/api/v2/life-areas", BOB).await.json()["source"],
            "instance"
        );
        let r = call(
            &app,
            Method::PUT,
            "/rs/api/v2/life-areas",
            ALICE,
            Some(body),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.text());
        let v = r.json();
        assert_eq!(v["items"].as_array().unwrap().len(), 2);
        area_uuid = v["items"][0]["uuid"].as_str().unwrap().to_string();
        assert_eq!(v["items"][0]["color"], "#B7410E", "Palette");
        let eff = get(&app, "/rs/api/v2/life-areas", ALICE).await.json();
        assert_eq!(eff["source"], "user");
        // PUT with uuid keeps the identity, missing ones are deleted
        let r = call(&app, Method::PUT, "/rs/api/v2/life-areas", ALICE, Some(json!({"items": [{"uuid": area_uuid, "name": "Wohnen & Haus", "rules": {"tags": [tag_uuid]}}]})), &[]).await;
        assert_eq!(r.json()["items"][0]["uuid"], area_uuid.as_str());
        assert_eq!(r.json()["items"].as_array().unwrap().len(), 1);
        // foreign area: 404; read default yes, change no
        assert_eq!(
            get(&app, &format!("/rs/api/v2/life-areas/{area_uuid}"), BOB)
                .await
                .status,
            StatusCode::NOT_FOUND
        );
        let inst = get(&app, "/rs/api/v2/life-areas?scope=instance", ALICE)
            .await
            .json()["items"][0]["uuid"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(
            get(&app, &format!("/rs/api/v2/life-areas/{inst}"), ALICE)
                .await
                .status,
            StatusCode::OK
        );
        let r = call(
            &app,
            Method::PATCH,
            &format!("/rs/api/v2/life-areas/{inst}"),
            ALICE,
            Some(json!({"name": "x"})),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
        // Rules with an invisible tag
        if let Some(s) = &secret_tag {
            let r = call(
                &app,
                Method::POST,
                "/rs/api/v2/life-areas",
                ALICE,
                Some(json!({"name": "Geheim", "rules": {"tags": [s]}})),
                &[],
            )
            .await;
            assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.text());
            assert!(r.json()["error"]["fields"]["rules.tags"].is_array());
        }
        let r = call(
            &app,
            Method::POST,
            "/rs/api/v2/life-areas",
            ALICE,
            Some(json!({"name": "Bad", "color": "red"})),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
        let r = call(
            &app,
            Method::POST,
            "/rs/api/v2/life-areas",
            ALICE,
            Some(json!({"name": "Auto", "icon": "car"})),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::CREATED);
        let auto = r.json()["uuid"].as_str().unwrap().to_string();
        let r = call(
            &app,
            Method::PATCH,
            &format!("/rs/api/v2/life-areas/{auto}"),
            ALICE,
            Some(json!({"order": 99})),
            &[],
        )
        .await;
        assert_eq!(r.json()["order"], 99);
        assert_eq!(
            call(
                &app,
                Method::DELETE,
                &format!("/rs/api/v2/life-areas/{auto}"),
                ALICE,
                None,
                &[]
            )
            .await
            .status,
            StatusCode::NO_CONTENT
        );
    });

    check!("timeline lanes follow the area rules", {
        let v = get(
            &app,
            &format!(
                "/rs/api/v2/timeline?from={min_c}&to={}&kinds=created",
                min_c + Duration::days(3600)
            ),
            ALICE,
        )
        .await
        .json();
        let lane = v["lanes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["key"] == area_uuid.as_str())
            .expect("area lane")
            .clone();
        let tagged: BTreeSet<i64> = sqlx::query_scalar::<_, i32>(
            "SELECT document_id FROM documents_document_tags WHERE tag_id = $1",
        )
        .bind(tag_id as i32)
        .fetch_all(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(i64::from)
        .filter(|d| alice.contains_key(d))
        .collect();
        let in_lane: BTreeSet<i64> = lane["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["document"]["legacy_id"].as_i64().unwrap())
            .collect();
        let in_range: BTreeSet<i64> = timeline_docs(&v);
        assert_eq!(
            in_lane,
            tagged
                .intersection(&in_range)
                .copied()
                .collect::<BTreeSet<_>>()
        );
        let other = v["lanes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["key"] == "other")
            .unwrap();
        for i in other["items"].as_array().unwrap() {
            assert!(
                !tagged.contains(&i["document"]["legacy_id"].as_i64().unwrap()),
                "kein getaggtes Dokument in „other“"
            );
        }
        let only = get(
            &app,
            &format!(
                "/rs/api/v2/timeline?areas=other&from={min_c}&to={}&kinds=created",
                min_c + Duration::days(3600)
            ),
            ALICE,
        )
        .await
        .json();
        assert_eq!(only["lanes"].as_array().unwrap().len(), 1);
        let up = get(&app, "/rs/api/v2/timeline/upcoming", ALICE)
            .await
            .json();
        let it = up["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == due_id)
            .unwrap()
            .clone();
        assert!(it["areas"].is_array());
        assert_eq!(it["overdue"], false);
        assert_eq!(it["days_left"], 10);
    });

    // ---------------------------------------------------------------- /ask without AI
    // Append content with a deadline to a visible document (search index stays unchanged)
    let ask_due = today + Duration::days(40);
    let (content,): (String,) =
        sqlx::query_as("SELECT content FROM documents_document WHERE id = $1")
            .bind(shown2 as i32)
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query("UPDATE documents_document SET content = $2, modified = now() WHERE id = $1")
        .bind(shown2 as i32)
        .bind(format!(
            "{content}\nBitte zahlbar bis {} überweisen.",
            de(ask_due)
        ))
        .execute(&pool)
        .await
        .unwrap();
    let word: String = {
        let (t,): (String,) = sqlx::query_as("SELECT title FROM documents_document WHERE id = $1")
            .bind(shown2 as i32)
            .fetch_one(&pool)
            .await
            .unwrap();
        t.split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.chars().count() >= 4)
            .max_by_key(|w| w.len())
            .unwrap_or("Rechnung")
            .to_string()
    };
    let conv: String;
    check!(
        "ask without AI: ranked passages only, permission-filtered",
        {
            let r = call(
                &app,
                Method::POST,
                "/rs/api/v2/ask",
                ALICE,
                Some(json!({"question": word})),
                &[],
            )
            .await;
            assert_eq!(r.status, StatusCode::OK, "{}", r.text());
            let v = r.json();
            conv = v["conversation"].as_str().unwrap().to_string();
            let m = &v["message"];
            assert!(m["answer"].is_null(), "{v}");
            assert_eq!(m["index_state"], "disabled");
            assert_eq!(m["retrieval"], "fulltext");
            let notes = m["notes"].to_string();
            assert!(notes.contains("No LLM is configured"), "{notes}");
            let cits = m["citations"].as_array().unwrap();
            assert!(!cits.is_empty(), "Volltext findet {word:?}: {v}");
            for (i, c) in cits.iter().enumerate() {
                assert_eq!(c["n"], i + 1);
                let id = c["legacy_id"].as_i64().unwrap();
                assert!(alice.contains_key(&id), "nur sichtbare Quellen");
                assert_eq!(c["document"], alice[&id].as_str());
                for k in ["title", "page", "snippet", "source", "score"] {
                    assert!(c.get(k).is_some(), "{k}");
                }
            }
            let acts = m["actions"].as_array().unwrap();
            assert_eq!(acts[0]["type"], "open_documents");
            assert!(
                cits.iter().any(|c| c["legacy_id"] == shown2),
                "Titelwort {word:?} findet Dokument {shown2}: {v}"
            );
            {
                let rem = acts
                    .iter()
                    .find(|a| a["type"] == "create_reminder" && a["legacy_id"] == shown2)
                    .expect("create_reminder aus Inhalt");
                assert_eq!(rem["kind"], "due");
                assert_eq!(rem["date"], ask_due.to_string());
                // Execute action → entry on the timeline
                let req = &rem["request"];
                let r = call(
                    &app,
                    Method::POST,
                    req["path"].as_str().unwrap(),
                    ALICE,
                    Some(req["body"].clone()),
                    &[],
                )
                .await;
                assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
            }
            // same question for a user with fewer permissions: no foreign sources
            for (u, a) in users {
                let Some(list) = &visible[u] else { continue };
                let r = call(
                    &app,
                    Method::POST,
                    "/rs/api/v2/ask",
                    a,
                    Some(json!({"question": word, "answer": false})),
                    &[],
                )
                .await;
                for c in r.json()["message"]["citations"].as_array().unwrap() {
                    assert!(
                        list.contains_key(&c["legacy_id"].as_i64().unwrap()),
                        "{u}: fremde Quelle"
                    );
                }
            }
            // Restriction to documents
            let r = call(
                &app,
                Method::POST,
                "/rs/api/v2/ask",
                ALICE,
                Some(json!({"question": word, "documents": [hidden_uuid]})),
                &[],
            )
            .await;
            assert!(
                r.json()["message"]["citations"]
                    .as_array()
                    .unwrap()
                    .is_empty(),
                "unsichtbares Dokument als Einschränkung trifft nichts"
            );
        }
    );

    check!("ask streaming format (SSE) and history", {
        let r = call(
            &app,
            Method::POST,
            "/rs/api/v2/ask",
            ALICE,
            Some(json!({"question": word, "conversation": conv})),
            &[("accept", "text/event-stream")],
        )
        .await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(
            r.headers
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("text/event-stream")
        );
        let ev = parse_sse(&r.text());
        let names: Vec<&str> = ev.iter().map(|e| e.0.as_str()).collect();
        assert_eq!(
            names,
            vec!["meta", "citations", "actions", "done"],
            "ohne LLM keine delta"
        );
        assert_eq!(ev[0].1["conversation"], conv.as_str());
        assert_eq!(ev[3].1["uuid"], ev[0].1["message"]);
        let list = get(&app, "/rs/api/v2/ask/conversations", ALICE)
            .await
            .json();
        assert_eq!(list["items"][0]["uuid"], conv.as_str());
        let c = get(&app, &format!("/rs/api/v2/ask/conversations/{conv}"), ALICE)
            .await
            .json();
        let roles: Vec<&str> = c["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, vec!["user", "assistant", "user", "assistant"]);
        assert_eq!(
            get(&app, &format!("/rs/api/v2/ask/conversations/{conv}"), BOB)
                .await
                .status,
            StatusCode::NOT_FOUND
        );
        let r = call(
            &app,
            Method::POST,
            "/rs/api/v2/ask",
            BOB,
            Some(json!({"question": "x", "conversation": conv})),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "fremdes Gespräch");
        let r = call(
            &app,
            Method::POST,
            "/rs/api/v2/ask",
            ALICE,
            Some(json!({"question": "   "})),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
        let r = call(
            &app,
            Method::POST,
            "/rs/api/v2/ask",
            ALICE,
            Some(json!({"question": "x", "max_passages": 50})),
            &[],
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
    });

    check!("ask history re-filters citations by current visibility", {
        // Make a source invisible afterwards: redirect saved citations to the hidden document
        sqlx::query(
            "UPDATE paperrust_ask_message SET citations = jsonb_build_array(jsonb_build_object('n',1,'legacy_id',$1::int8,'title','geheim'))
              WHERE role = 'assistant' AND conversation_id = (SELECT id FROM paperrust_ask_conversation WHERE uuid = ($2::text)::uuid)",
        )
        .bind(hidden)
        .bind(&conv)
        .execute(&pool)
        .await
        .unwrap();
        let c = get(&app, &format!("/rs/api/v2/ask/conversations/{conv}"), ALICE)
            .await
            .json();
        for m in c["messages"].as_array().unwrap() {
            assert!(m["citations"].as_array().unwrap().is_empty(), "{m}");
        }
    });

    // ---------------------------------------------------------------- /ask with mock LLM
    check!("ask with LLM (mock): deltas, answer, citations", {
        let ep = mock_ollama("Laut Quelle [1] ist es fällig.").await;
        sqlx::query("UPDATE paperless_applicationconfiguration SET ai_enabled = true, llm_backend = 'ollama', llm_endpoint = $1, llm_model = 'mock'")
            .bind(&ep)
            .execute(&pool)
            .await
            .unwrap();
        let r = call(
            &app,
            Method::POST,
            "/rs/api/v2/ask",
            ALICE,
            Some(json!({"question": word, "stream": true})),
            &[],
        )
        .await;
        let ev = parse_sse(&r.text());
        let names: Vec<&str> = ev.iter().map(|e| e.0.as_str()).collect();
        assert_eq!(names.first(), Some(&"meta"));
        assert_eq!(names.last(), Some(&"done"), "{names:?}");
        let deltas: String = ev
            .iter()
            .filter(|e| e.0 == "delta")
            .map(|e| e.1["text"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(deltas, "Laut Quelle [1] ist es fällig. Ende.");
        let done = &ev.last().unwrap().1;
        assert_eq!(done["answer"], deltas.as_str());
        assert!(done["answer_error"].is_null());
        let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
        assert!(pos("citations") < pos("delta") && pos("delta") < pos("actions"));
        // LLM unreachable → answer_error, sources stay
        sqlx::query(
            "UPDATE paperless_applicationconfiguration SET llm_endpoint = 'http://127.0.0.1:9'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let v = call(
            &app,
            Method::POST,
            "/rs/api/v2/ask",
            ALICE,
            Some(json!({"question": word})),
            &[],
        )
        .await
        .json();
        assert!(v["message"]["answer"].is_null());
        assert!(v["message"]["answer_error"].is_string(), "{v}");
        assert!(!v["message"]["citations"].as_array().unwrap().is_empty());
        sqlx::query("UPDATE paperless_applicationconfiguration SET ai_enabled = false")
            .execute(&pool)
            .await
            .unwrap();
    });

    // ---------------------------------------------------------------- Content recognition
    let content_due = today + Duration::days(25);
    sqlx::query("UPDATE documents_document SET content = $2, modified = now() WHERE id = $1")
        .bind(shown as i32)
        .bind(format!(
            "Rechnung\nGarantie bis {}\nZahlbar bis {}",
            de(today + Duration::days(700)),
            de(content_due)
        ))
        .execute(&pool)
        .await
        .unwrap();
    check!(
        "content extraction: off by default, on via setting, stale refresh",
        {
            let has = |v: &Value| {
                v["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|i| i["source"] == "content" && i["date"] == content_due.to_string())
            };
            assert!(
                !has(&get(&app, "/rs/api/v2/timeline/upcoming", ALICE)
                    .await
                    .json()),
                "aus per Standard"
            );
            set_env("PAPERRUST_TIMELINE_EXTRACT_DATES", Some("yes"));
            let v = get(&app, "/rs/api/v2/timeline/upcoming", ALICE)
                .await
                .json();
            assert!(
                has(&v),
                "veraltete Dokumente werden beim Lesen erkannt: {v}"
            );
            let it = v["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|i| i["source"] == "content" && i["document"]["legacy_id"] == shown)
                .unwrap()
                .clone();
            assert_eq!(it["kind"], "due");
            assert!(it["confidence"].as_f64().unwrap() >= 0.9);
            assert!(it["phrase"].as_str().unwrap().starts_with("Zahlbar bis"));
            let w = get(
                &app,
                &format!(
                    "/rs/api/v2/timeline?from={today}&to={}&kinds=warranty_until",
                    today + Duration::days(800)
                ),
                ALICE,
            )
            .await
            .json();
            let wi = items_of(&w);
            assert!(
                wi.iter().any(|i| i["document"]["legacy_id"] == shown
                    && i["span"]["end"] == (today + Duration::days(700)).to_string()),
                "{w}"
            );
            // hidden doc gets content → alice does not see it
            sqlx::query("UPDATE documents_document SET content = 'zahlbar bis ' || $2, modified = now() WHERE id = $1")
            .bind(hidden as i32)
            .bind(de(content_due))
            .execute(&pool)
            .await
            .unwrap();
            let v = get(&app, "/rs/api/v2/timeline/upcoming", ALICE)
                .await
                .json();
            assert!(
                !v["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|i| i["document"]["legacy_id"] == hidden)
            );
            // Backfill (library like command/task)
            let dp = pl_timeline::extract::DateParse::from_lookup(pl_core::config::var);
            let rep = pl_timeline::refresh::backfill(&pool, &dp, true)
                .await
                .unwrap()
                .unwrap();
            assert!(rep.documents >= alice.len(), "{rep:?}");
            let stats = get(&app, "/rs/api/v2/timeline/config", ADMIN).await.json();
            assert_eq!(stats["extraction"]["stale"], 0, "{stats}");
            assert_eq!(
                call(
                    &app,
                    Method::POST,
                    "/rs/api/v2/timeline/backfill",
                    ALICE,
                    None,
                    &[]
                )
                .await
                .status,
                StatusCode::FORBIDDEN
            );
            let r = call(
                &app,
                Method::POST,
                "/rs/api/v2/timeline/backfill",
                ADMIN,
                Some(json!({"force": false})),
                &[],
            )
            .await;
            assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.text());
            set_env("PAPERRUST_TIMELINE_EXTRACT_DATES", None);
            assert!(
                !has(&get(&app, "/rs/api/v2/timeline/upcoming", ALICE)
                    .await
                    .json()),
                "ausgeschaltet → nicht angezeigt"
            );
            let r = call(
                &app,
                Method::POST,
                "/rs/api/v2/timeline/backfill",
                ADMIN,
                None,
                &[],
            )
            .await;
            assert_eq!(
                r.status,
                StatusCode::BAD_REQUEST,
                "Rückfüllung nur bei eingeschalteter Erkennung"
            );
        }
    );

    check!("expand-only: no Django table changed by the side tables", {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM information_schema.table_constraints c JOIN information_schema.constraint_column_usage u
               ON u.constraint_name = c.constraint_name WHERE c.constraint_type = 'FOREIGN KEY'
               AND c.table_name IN ('paperrust_life_area','paperrust_doc_date','paperrust_doc_date_state','paperrust_timeline_state',
                                    'paperrust_ask_conversation','paperrust_ask_message')
               AND u.table_name NOT LIKE 'paperrust_%'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(n, 0, "keine Fremdschlüssel auf Django-Tabellen");
    });

    // Wait for the background backfill, then clean up
    for _ in 0..100 {
        if !pl_timeline::refresh::backfill_running() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    eprintln!("{n} checks passed");
    drop(app);
    drop(st);
    drop_db(pool).await;
}
