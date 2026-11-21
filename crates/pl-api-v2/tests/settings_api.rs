//! WP5 acceptance (docs/SETTINGS.md §6): permissions (superuser / AC permission / loopback), 409 locked,
//! 409 rev_conflict, 422 validation, secrets write-only, parity v1 `/api/config/` ↔ v2.
//!
//! Against a **dedicated copy** (never `paperless`):
//! ```text
//! psql -h 127.0.0.1 -p 54329 -U paperless -d postgres -c "CREATE DATABASE pl_settings_api_test TEMPLATE paperless_snapshot"
//! PAPERRUST_SETTINGS_TEST_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/pl_settings_api_test \
//!   cargo test -p pl-api-v2 --test settings_api -j 2 -- --ignored --test-threads 1
//! ```

use std::net::SocketAddr;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use pl_core::AppState;
use pl_core::config::{Config, Env};
use serde_json::{Value, json};
use tower::ServiceExt;

const SU_TOKEN: &str = "plsettingsapitestsu000000000000000000000";
const VIEW_TOKEN: &str = "plsettingsapitestvw000000000000000000000";
const PLAIN_TOKEN: &str = "plsettingsapitestpl000000000000000000000";

fn url() -> String {
    let url = std::env::var("PAPERRUST_SETTINGS_TEST_DATABASE_URL").expect("PAPERRUST_SETTINGS_TEST_DATABASE_URL");
    let db = url.trim_end_matches('/').rsplit('/').next().unwrap_or("").split('?').next().unwrap_or("").to_string();
    assert!(db.starts_with("pl_settings"), "refusing to run against a non-test database ({db})");
    url
}

async fn setup(env_extra: &[(&str, &str)]) -> (Router, sqlx::PgPool) {
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(4).connect(&url()).await.expect("connect");
    {
        let mut c = pool.acquire().await.unwrap();
        pl_settings::store::migrate(&mut c).await.unwrap();
    }
    sqlx::raw_sql(
        "DELETE FROM paperrust_setting; DELETE FROM paperrust_setting_history;
         DELETE FROM authtoken_token WHERE key LIKE 'plsettingsapitest%';
         DELETE FROM auth_user_user_permissions WHERE user_id IN (SELECT id FROM auth_user WHERE username LIKE 'pl_settings_api_%');
         DELETE FROM auth_user WHERE username LIKE 'pl_settings_api_%';",
    )
    .execute(&pool)
    .await
    .unwrap();
    let su: i32 = sqlx::query_scalar(
        "INSERT INTO auth_user (password, is_superuser, username, first_name, last_name, email, is_staff, is_active, date_joined)
         VALUES ('!', true, 'pl_settings_api_su', '', '', '', true, true, now()) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let viewer: i32 = sqlx::query_scalar(
        "INSERT INTO auth_user (password, is_superuser, username, first_name, last_name, email, is_staff, is_active, date_joined)
         VALUES ('!', false, 'pl_settings_api_viewer', '', '', '', false, true, now()) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let plain: i32 = sqlx::query_scalar(
        "INSERT INTO auth_user (password, is_superuser, username, first_name, last_name, email, is_staff, is_active, date_joined)
         VALUES ('!', false, 'pl_settings_api_plain', '', '', '', false, true, now()) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO auth_user_user_permissions (user_id, permission_id)
         SELECT $1, p.id FROM auth_permission p JOIN django_content_type ct ON ct.id = p.content_type_id
         WHERE ct.app_label = 'paperless' AND p.codename = 'view_applicationconfiguration'",
    )
    .bind(viewer)
    .execute(&pool)
    .await
    .unwrap();
    for (k, u) in [(SU_TOKEN, su), (VIEW_TOKEN, viewer), (PLAIN_TOKEN, plain)] {
        sqlx::query("INSERT INTO authtoken_token (key, created, user_id) VALUES ($1, now(), $2)")
            .bind(k)
            .bind(u)
            .execute(&pool)
            .await
            .unwrap();
    }
    let mut pairs =
        vec![("PAPERLESS_SECRET_KEY", "pl-settings-api-tests-only-0123456789abcdefghijklmnop"), ("PAPERLESS_DBHOST", "127.0.0.1")];
    pairs.extend_from_slice(env_extra);
    let cfg = Config::from_env_map(Env::from_pairs(pairs)).expect("config");
    let state = AppState::with_pool(cfg.clone(), pool.clone()).expect("state");
    let authz = pl_authz::Authz::new(pool.clone(), pl_authz::AuthConfig::from_core(&cfg));
    let mut r: Router<AppState> = pl_api_v2::settings::routes(Router::new());
    for route in pl_system::routes().into_iter().filter(|r| r.id == "config.list") {
        r = r.route(route.path, route.handler);
    }
    (r.with_state(state).layer(axum::Extension(authz)), pool)
}

async fn call(router: &Router, method: &str, uri: &str, token: &str, body: Option<Value>, loopback: bool) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri).header("host", "localhost").header("authorization", format!("Token {token}"));
    let body = match body {
        Some(v) => {
            b = b.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let mut req = b.body(body).unwrap();
    let peer: SocketAddr = if loopback { "127.0.0.1:5000".parse().unwrap() } else { "192.0.2.7:5000".parse().unwrap() };
    req.extensions_mut().insert(ConnectInfo(peer));
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 24).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn find<'a>(list: &'a Value, key: &str) -> Option<&'a Value> {
    list["items"].as_array()?.iter().find(|i| i["key"] == key)
}

#[tokio::test]
#[ignore]
async fn settings_api_end_to_end() {
    let (r, pool) = setup(&[("PAPERLESS_EMAIL_PORT", "2525")]).await;

    // --- Permissions ---
    let (st, _) = call(&r, "GET", "/settings", PLAIN_TOKEN, None, true).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, l) = call(&r, "GET", "/settings", VIEW_TOKEN, None, true).await;
    assert_eq!(st, StatusCode::OK);
    assert!(l["items"].as_array().unwrap().iter().all(|i| !i["appconfig_field"].is_null()), "viewer sees only AC keys");
    assert_eq!(l["items"].as_array().unwrap().len(), 45);
    let (st, _) = call(&r, "PATCH", "/settings", VIEW_TOKEN, Some(json!({"values": {"PAPERLESS_OCR_LANGUAGE": "deu"}})), true).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "view right is not change right");
    let (st, l) = call(&r, "GET", "/settings", SU_TOKEN, None, false).await;
    assert_eq!(st, StatusCode::OK);
    assert!(find(&l, "PAPERLESS_EMAIL_HOST").is_some());
    assert!(find(&l, "desktop.server.lan_sharing").is_none(), "desktop keys only from loopback");
    let (_, l) = call(&r, "GET", "/settings?category=desktop", SU_TOKEN, None, true).await;
    assert!(find(&l, "desktop.server.lan_sharing").is_some());
    let (st, _) = call(&r, "PATCH", "/settings", SU_TOKEN, Some(json!({"values": {"desktop.server.lan_sharing": true}})), false).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // --- locked / class C / validation ---
    let rev0 = l["rev"].as_i64().unwrap();
    let (st, e) = call(&r, "PATCH", "/settings", SU_TOKEN, Some(json!({"values": {"PAPERLESS_EMAIL_PORT": 25}})), false).await;
    assert_eq!((st, e["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("locked_by_environment")));
    let (st, e) = call(&r, "PATCH", "/settings", SU_TOKEN, Some(json!({"values": {"PAPERLESS_DBHOST": "x"}})), false).await;
    assert_eq!((st, e["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("not_writable")));
    let (st, e) = call(
        &r,
        "PATCH",
        "/settings",
        SU_TOKEN,
        Some(json!({"values": {"PAPERLESS_EMAIL_USE_TLS": "vielleicht", "PAPERLESS_TRAIN_TASK_CRON": "x y"}})),
        false,
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        e["error"]["fields"]["PAPERLESS_EMAIL_USE_TLS"].is_array() && e["error"]["fields"]["PAPERLESS_TRAIN_TASK_CRON"].is_array(),
        "{e}"
    );
    let (st, e) = call(&r, "PATCH", "/settings", SU_TOKEN, Some(json!({"values": {"PAPERLESS_DISABLE_REGULAR_LOGIN": true}})), false).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "lockout protection: {e}");

    // --- set, secret, rev ---
    let (st, w) = call(
        &r,
        "PATCH",
        "/settings",
        SU_TOKEN,
        Some(json!({"values": {"PAPERLESS_EMAIL_HOST": "smtp.example.ch", "PAPERLESS_EMAIL_HOST_PASSWORD": "hunter2-api"}, "expected_rev": rev0})),
        false,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{w}");
    assert!(!w.to_string().contains("hunter2"));
    let rev1 = w["rev"].as_i64().unwrap();
    assert!(rev1 > rev0);
    let (st, e) =
        call(&r, "PATCH", "/settings", SU_TOKEN, Some(json!({"values": {"PAPERLESS_EMAIL_HOST": "x"}, "expected_rev": rev0})), false).await;
    assert_eq!((st, e["error"]["code"].as_str()), (StatusCode::CONFLICT, Some("rev_conflict")));
    let (_, one) = call(&r, "GET", "/settings/PAPERLESS_EMAIL_HOST_PASSWORD", SU_TOKEN, None, false).await;
    assert_eq!((one["is_set"].clone(), one["value"].clone(), one["source"].clone()), (json!(true), Value::Null, json!("db")));
    let (_, one) = call(&r, "GET", "/settings/PAPERRUST_EMAIL_HOST", SU_TOKEN, None, false).await;
    assert_eq!((one["key"].clone(), one["value"].clone()), (json!("PAPERLESS_EMAIL_HOST"), json!("smtp.example.ch")));
    let (_, l) = call(&r, "GET", "/settings?q=email_host", SU_TOKEN, None, false).await;
    assert!(!l.to_string().contains("hunter2"));

    // --- AC: parity v2 → v1 and v1 → v2 ---
    let (st, w) = call(
        &r,
        "PATCH",
        "/settings",
        SU_TOKEN,
        Some(json!({"values": {"PAPERLESS_OCR_LANGUAGE": "deu+fra", "PAPERLESS_OCR_PAGES": 4}})),
        false,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{w}");
    let (st, v1) = call(&r, "GET", "/api/config/", SU_TOKEN, None, false).await;
    assert_eq!(st, StatusCode::OK, "{v1}");
    assert_eq!((v1[0]["language"].clone(), v1[0]["pages"].clone()), (json!("deu+fra"), json!(4)));
    // v1 write path (Django) = same row
    sqlx::query("UPDATE paperless_applicationconfiguration SET language = 'ita'").execute(&pool).await.unwrap();
    let (_, one) = call(&r, "GET", "/settings/PAPERLESS_OCR_LANGUAGE", VIEW_TOKEN, None, false).await;
    assert_eq!((one["value"].clone(), one["source"].clone(), one["locked"].clone()), (json!("ita"), json!("appconfig"), json!(false)));
    let (st, e) = call(&r, "PATCH", "/settings", SU_TOKEN, Some(json!({"values": {"PAPERLESS_OCR_PAGES": 0}})), false).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "serializer rule pages >= 1: {e}");
    let (st, _) = call(&r, "DELETE", "/settings/PAPERLESS_OCR_LANGUAGE", SU_TOKEN, None, false).await;
    assert_eq!(st, StatusCode::OK);
    let (_, v1) = call(&r, "GET", "/api/config/", SU_TOKEN, None, false).await;
    assert_eq!(v1[0]["language"], Value::Null);
    let (st, _) = call(&r, "DELETE", "/settings/PAPERLESS_OCR_PAGES", SU_TOKEN, None, false).await;
    assert_eq!(st, StatusCode::OK);

    // --- History, schema, restart ---
    let (st, h) = call(&r, "GET", "/settings/history", SU_TOKEN, None, false).await;
    assert_eq!(st, StatusCode::OK);
    assert!(!h.to_string().contains("hunter2"));
    assert!(h["items"].as_array().unwrap().iter().any(|i| i["key"] == "AC:language"));
    let (_, hv) = call(&r, "GET", "/settings/history", VIEW_TOKEN, None, false).await;
    assert!(hv["items"].as_array().unwrap().iter().all(|i| i["key"].as_str().unwrap().starts_with("AC:")));
    let (st, s) = call(&r, "GET", "/settings/schema", SU_TOKEN, None, false).await;
    assert_eq!(st, StatusCode::OK);
    assert!(s["items"].as_array().unwrap().len() > 250);
    // English help text additively next to the German `help`
    let host = find(&s, "PAPERLESS_EMAIL_HOST").unwrap();
    assert_eq!((host["help"].as_str(), host["help_en"].as_str()), (Some("SMTP-Server"), Some("SMTP server")));
    let (st, e) = call(&r, "POST", "/settings/restart", SU_TOKEN, Some(json!({})), false).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "password required: {e}");
    let (st, _) = call(&r, "POST", "/settings/restart", VIEW_TOKEN, Some(json!({"password": "x"})), false).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    // --- Reset ---
    let (st, _) = call(&r, "DELETE", "/settings/PAPERLESS_EMAIL_HOST", SU_TOKEN, None, false).await;
    assert_eq!(st, StatusCode::OK);
    let (_, one) = call(&r, "GET", "/settings/PAPERLESS_EMAIL_HOST", SU_TOKEN, None, false).await;
    assert_eq!((one["value"].clone(), one["source"].clone()), (json!("localhost"), json!("default")));

    // clean up
    sqlx::raw_sql(
        "DELETE FROM paperrust_setting; DELETE FROM paperrust_setting_history;
         DELETE FROM authtoken_token WHERE key LIKE 'plsettingsapitest%';
         DELETE FROM auth_user_user_permissions WHERE user_id IN (SELECT id FROM auth_user WHERE username LIKE 'pl_settings_api_%');
         DELETE FROM auth_user WHERE username LIKE 'pl_settings_api_%';",
    )
    .execute(&pool)
    .await
    .unwrap();
}

/// WP8: origin → adopt → slim env; check file; export with passphrase → import.
#[tokio::test]
#[ignore]
async fn settings_migration_end_to_end() {
    let (r, pool) = setup(&[("PAPERLESS_EMAIL_PORT", "2525"), ("PAPERLESS_TIKA_ENABLED", "1")]).await;
    let pw = pl_authz::hashers::make_password("Mig-pass-123");
    sqlx::query("UPDATE auth_user SET password = $1 WHERE username = 'pl_settings_api_su'").bind(pw).execute(&pool).await.unwrap();

    let (st, o) = call(&r, "GET", "/settings/origin", SU_TOKEN, None, false).await;
    assert_eq!(st, StatusCode::OK, "{o}");
    let item = |o: &Value, n: &str| o["items"].as_array().unwrap().iter().find(|i| i["name"] == n).cloned();
    assert_eq!(item(&o, "PAPERLESS_EMAIL_PORT").unwrap()["verdict"], "adoptable");
    assert_eq!(item(&o, "PAPERLESS_TIKA_ENABLED").unwrap()["verdict"], "no_effect");
    assert_eq!(item(&o, "PAPERLESS_DBHOST").unwrap()["verdict"], "must_stay");
    assert!(item(&o, "PAPERLESS_SECRET_KEY").unwrap()["value"].is_null());
    let (st, _) = call(&r, "GET", "/settings/origin", VIEW_TOKEN, None, false).await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    let (st, a) =
        call(&r, "POST", "/settings/adopt", SU_TOKEN, Some(json!({"keys": ["PAPERLESS_EMAIL_PORT", "PAPERLESS_DBHOST"]})), false).await;
    assert_eq!(st, StatusCode::OK, "{a}");
    assert_eq!(a["adopted"], json!(["PAPERLESS_EMAIL_PORT"]));
    assert_eq!(a["skipped"][0]["name"], "PAPERLESS_DBHOST");
    let (_, o) = call(&r, "GET", "/settings/origin", SU_TOKEN, None, false).await;
    assert_eq!(item(&o, "PAPERLESS_EMAIL_PORT").unwrap()["verdict"], "adopted");
    // adopted, but still locked (the environment sets it)
    let (_, one) = call(&r, "GET", "/settings/PAPERLESS_EMAIL_PORT", SU_TOKEN, None, false).await;
    assert_eq!((one["locked"].clone(), one["in_db"].clone()), (json!(true), json!(true)));

    let req = Request::builder()
        .method("GET")
        .uri("/settings/slim-env")
        .header("authorization", format!("Token {SU_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    let resp = r.clone().oneshot(req).await.unwrap();
    let txt = String::from_utf8(axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap().to_vec()).unwrap();
    assert!(txt.contains("# PAPERLESS_EMAIL_PORT – already in the database"), "{txt}");
    assert!(txt.contains("# PAPERLESS_TIKA_ENABLED – no effect"));
    assert!(txt.contains("PAPERLESS_DBHOST=127.0.0.1"));
    assert!(!txt.contains("pl-settings-api-tests-only"), "secret leaked into slim env");

    // check file (text body)
    let compose = "services:\n  webserver:\n    env_file: docker-compose.env\n    environment:\n      PAPERLESS_REDIS: redis://broker:6379\n      PAPERLESS_OCR_LANGUAGE: deu\n      PAPERLESS_CONSUMER_INOTIFY_DELAY: \"2\"\n";
    let req = Request::builder()
        .method("POST")
        .uri("/settings/check-file")
        .header("authorization", format!("Token {SU_TOKEN}"))
        .header("content-type", "text/plain")
        .body(Body::from(compose))
        .unwrap();
    let resp = r.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let c: Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 1 << 20).await.unwrap()).unwrap();
    assert_eq!((c["kind"].clone(), c["service"].clone()), (json!("compose"), json!("webserver")));
    let v = |n: &str| c["items"].as_array().unwrap().iter().find(|i| i["name"] == n).unwrap()["verdict"].clone();
    assert_eq!(
        (v("PAPERLESS_REDIS"), v("PAPERLESS_OCR_LANGUAGE"), v("PAPERLESS_CONSUMER_INOTIFY_DELAY")),
        (json!("must_stay"), json!("adoptable"), json!("obsolete"))
    );
    let (st, a) = call(
        &r,
        "POST",
        "/settings/adopt",
        SU_TOKEN,
        Some(json!({"values": {"PAPERLESS_OCR_LANGUAGE": "deu", "PAPERLESS_REDIS": "x"}})),
        false,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{a}");
    assert_eq!(a["adopted"], json!(["PAPERLESS_OCR_LANGUAGE"]));
    let lang: Option<String> =
        sqlx::query_scalar("SELECT language FROM paperless_applicationconfiguration ORDER BY id LIMIT 1").fetch_one(&pool).await.unwrap();
    assert_eq!(lang.as_deref(), Some("deu"));

    // export with secret: password + passphrase required, result encrypted
    call(&r, "PATCH", "/settings", SU_TOKEN, Some(json!({"values": {"PAPERLESS_EMAIL_HOST_PASSWORD": "exp-secret-1"}})), false).await;
    let (st, _) =
        call(&r, "POST", "/settings/export", SU_TOKEN, Some(json!({"include_secrets": true, "passphrase": "long enough"})), false).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "password required");
    let (st, enc) = call(
        &r,
        "POST",
        "/settings/export",
        SU_TOKEN,
        Some(json!({"include_secrets": true, "password": "Mig-pass-123", "passphrase": "long enough"})),
        false,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{enc}");
    assert_eq!(enc["format"], "paperrust-settings-encrypted");
    assert!(!enc.to_string().contains("exp-secret-1"));
    let (_, plain) = call(&r, "POST", "/settings/export", SU_TOKEN, Some(json!({})), false).await;
    assert!(!plain.to_string().contains("exp-secret-1"));
    // change a value, then import (dry run, then real) restores it
    call(&r, "DELETE", "/settings/PAPERLESS_EMAIL_HOST_PASSWORD", SU_TOKEN, None, false).await;
    let (st, e) = call(&r, "POST", "/settings/import", SU_TOKEN, Some(json!({"data": enc, "passphrase": "wrong phrase"})), false).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{e}");
    let (st, d) =
        call(&r, "POST", "/settings/import", SU_TOKEN, Some(json!({"data": enc, "passphrase": "long enough", "dry_run": true})), false)
            .await;
    assert_eq!(st, StatusCode::OK, "{d}");
    let row = d["items"].as_array().unwrap().iter().find(|i| i["key"] == "PAPERLESS_EMAIL_HOST_PASSWORD").unwrap().clone();
    assert_eq!((row["status"].clone(), row["new"].clone()), (json!("change"), json!("***")));
    assert!(d["items"].as_array().unwrap().iter().any(|i| i["key"] == "PAPERLESS_EMAIL_PORT" && i["status"] == "locked_by_environment"));
    let (st, d) = call(&r, "POST", "/settings/import", SU_TOKEN, Some(json!({"data": enc, "passphrase": "long enough"})), false).await;
    assert_eq!(st, StatusCode::OK, "{d}");
    assert!(d["applied"].as_u64().unwrap() >= 1);
    let (_, one) = call(&r, "GET", "/settings/PAPERLESS_EMAIL_HOST_PASSWORD", SU_TOKEN, None, false).await;
    assert_eq!((one["is_set"].clone(), one["source"].clone()), (json!(true), json!("db")));

    sqlx::query("UPDATE paperless_applicationconfiguration SET language = NULL").execute(&pool).await.unwrap();
    sqlx::raw_sql(
        "DELETE FROM paperrust_setting; DELETE FROM paperrust_setting_history;
         DELETE FROM authtoken_token WHERE key LIKE 'plsettingsapitest%';
         DELETE FROM auth_user_user_permissions WHERE user_id IN (SELECT id FROM auth_user WHERE username LIKE 'pl_settings_api_%');
         DELETE FROM auth_user WHERE username LIKE 'pl_settings_api_%';",
    )
    .execute(&pool)
    .await
    .unwrap();
}

/// WP7: desktop keys via the supervisor (dummy), superuser + loopback only.
#[tokio::test]
#[ignore]
async fn desktop_settings_via_supervisor() {
    let dir = std::env::temp_dir().join(format!("pl_settings_api_desktop_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("desktop-settings.json"), r#"{"desktop.server.lan_sharing":"false","desktop.server.port":"58080"}"#).unwrap();
    let ctl = dir.display().to_string();
    let (r, pool) = setup(&[("PAPERRUST_SUPERVISOR_CONTROL", ctl.as_str())]).await;
    let (_, one) = call(&r, "GET", "/settings/desktop.server.lan_sharing", SU_TOKEN, None, true).await;
    assert_eq!((one["value"].clone(), one["source"].clone(), one["locked"].clone()), (json!(false), json!("desktop"), json!(false)));
    let (_, sk) = call(&r, "GET", "/settings/desktop.server.secret_key", SU_TOKEN, None, true).await;
    assert_eq!(sk["locked"], json!(true));
    // Supervisor dummy
    let d2 = dir.clone();
    let sup = tokio::spawn(async move {
        for _ in 0..200 {
            if let Ok(t) = std::fs::read_to_string(d2.join("settings.request.json")) {
                let req: Value = serde_json::from_str(&t).unwrap();
                std::fs::remove_file(d2.join("settings.request.json")).unwrap();
                assert_eq!(req["changes"]["desktop.server.lan_sharing"], "true");
                let out = json!({"id": req["id"], "ok": true, "error": null, "restart": true});
                std::fs::write(d2.join("settings.result.json"), out.to_string()).unwrap();
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        false
    });
    let (st, _) = call(&r, "PATCH", "/settings", SU_TOKEN, Some(json!({"values": {"desktop.server.lan_sharing": true}})), false).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "not from the LAN");
    let (st, _) = call(
        &r,
        "PATCH",
        "/settings",
        SU_TOKEN,
        Some(json!({"values": {"desktop.server.lan_sharing": true, "PAPERLESS_EMAIL_HOST": "x"}})),
        true,
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "mixed request");
    let (st, _) = call(&r, "PATCH", "/settings", SU_TOKEN, Some(json!({"values": {"desktop.server.secret_key": "x"}})), true).await;
    assert_eq!(st, StatusCode::CONFLICT, "not writable from the UI");
    let (st, b) = call(&r, "PATCH", "/settings", SU_TOKEN, Some(json!({"values": {"desktop.server.lan_sharing": true}})), true).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{b}");
    assert_eq!(b["restarting"], json!(true));
    assert!(sup.await.unwrap());
    let _ = std::fs::remove_dir_all(&dir);
    sqlx::raw_sql(
        "DELETE FROM authtoken_token WHERE key LIKE 'plsettingsapitest%';
         DELETE FROM auth_user_user_permissions WHERE user_id IN (SELECT id FROM auth_user WHERE username LIKE 'pl_settings_api_%');
         DELETE FROM auth_user WHERE username LIKE 'pl_settings_api_%';",
    )
    .execute(&pool)
    .await
    .unwrap();
}
