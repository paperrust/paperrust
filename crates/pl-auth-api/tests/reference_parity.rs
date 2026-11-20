//! Read-only parity against the running reference (`reference/README.md`): the same request is
//! sent to Django (127.0.0.1:8000) and to this crate's router (in-process, reference PostgreSQL),
//! and status + JSON (incl. key order) are compared. Token auth only (no password rehash, no
//! writes). Lists that upstream builds from Python sets are compared sorted (contract #7).
//!
//! Run: `cargo test -p pl-auth-api -j 2 --test reference_parity -- --ignored --nocapture`

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use pl_core::config::Env;
use pl_core::{AppState, Config};
use serde_json::Value;
use tower::ServiceExt as _;

const DJANGO: &str = "http://127.0.0.1:8000";
const SECRET: &str = "paperless-rs-reference-secret-key-local-only-0123456789";
const ADMIN: &str = "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001";
const ALICE: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002";
const BOB: &str = "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b003";
const CAROL: &str = "c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c004";

fn config() -> Config {
    Config::from_env_map(Env::from_pairs([
        ("PAPERLESS_DBHOST", "127.0.0.1"),
        ("PAPERLESS_DBPORT", "54329"),
        ("PAPERLESS_DBNAME", "paperless"),
        ("PAPERLESS_DBUSER", "paperless"),
        ("PAPERLESS_DBPASS", "paperless"),
        ("PAPERLESS_REDIS", "redis://127.0.0.1:63799"),
        ("PAPERLESS_SECRET_KEY", SECRET),
        ("PAPERLESS_TIME_ZONE", "Europe/Zurich"),
        ("PAPERLESS_URL", "http://127.0.0.1:8000"),
        ("PAPERLESS_ALLOWED_HOSTS", "127.0.0.1,localhost"),
        ("PAPERLESS_CSRF_TRUSTED_ORIGINS", "http://127.0.0.1:8000,http://localhost:8000"),
        ("PAPERLESS_DEBUG", "false"),
    ]))
    .expect("reference config")
}

fn app() -> Router {
    let cfg = config();
    let state = AppState::new(cfg).expect("state");
    let mut auth_cfg = pl_authz::AuthConfig::for_tests(SECRET);
    auth_cfg.allowed_hosts = vec!["127.0.0.1".into(), "localhost".into()];
    let authz = pl_authz::Authz::new(state.db().clone(), auth_cfg);
    let mut router: Router<AppState> = Router::new();
    for r in pl_auth_api::routes() {
        router = router.route(r.path, r.handler);
    }
    router.layer(axum::Extension(authz)).with_state(state)
}

fn normalise(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                normalise(x);
                if matches!(k.as_str(), "inherited_permissions" | "permissions" | "groups")
                    && let Value::Array(a) = x
                    && a.iter().all(|e| e.is_string() || e.is_number())
                {
                    a.sort_by_key(|e| e.to_string());
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(normalise),
        _ => {}
    }
}

async fn compare(app: &Router, token: &str, path: &str, version: &str) -> Result<(), String> {
    compare_with(app, "GET", token, path, version, None).await
}

async fn compare_with(app: &Router, method: &str, token: &str, path: &str, version: &str, body: Option<&str>) -> Result<(), String> {
    let accept = format!("application/json; version={version}");
    let client = reqwest::Client::new();
    let m = reqwest::Method::from_bytes(method.as_bytes()).map_err(|e| e.to_string())?;
    let mut rb = client
        .request(m, format!("{DJANGO}{path}"))
        .header("Accept", &accept);
    if !token.is_empty() {
        rb = rb.header("Authorization", format!("Token {token}"));
    }
    if let Some(b) = body {
        rb = rb.header("Content-Type", "application/json").body(b.to_string());
    }
    let d = rb.send().await.map_err(|e| e.to_string())?;
    let d_status = d.status().as_u16();
    let d_body = d.text().await.map_err(|e| e.to_string())?;

    let mut req = Request::builder().method(method).uri(path).header("host", "127.0.0.1:8000").header("accept", &accept);
    if !token.is_empty() {
        req = req.header("authorization", format!("Token {token}"));
    }
    let req = match body {
        Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .map_err(|e| e.to_string())?;
    let r = app.clone().oneshot(req).await.map_err(|e| e.to_string())?;
    let r_status = r.status().as_u16();
    let bytes = axum::body::to_bytes(r.into_body(), usize::MAX).await.map_err(|e| e.to_string())?;
    let r_body = String::from_utf8_lossy(&bytes).to_string();

    if d_status != r_status {
        return Err(format!("{path} v{version}: status django={d_status} rust={r_status}\n  django: {d_body:.300}\n  rust:   {r_body:.300}"));
    }
    match (serde_json::from_str::<Value>(&d_body), serde_json::from_str::<Value>(&r_body)) {
        (Ok(mut a), Ok(mut b)) => {
            normalise(&mut a);
            normalise(&mut b);
            // key order matters: compare serialised forms (preserve_order)
            let (sa, sb) = (a.to_string(), b.to_string());
            if sa != sb {
                return Err(format!("{path} v{version}: body differs\n  django: {sa:.1500}\n  rust:   {sb:.1500}"));
            }
            eprintln!("same {d_status} {:>6}B v{version} {method} {path}", sa.len());
        }
        _ => {
            if d_body != r_body {
                return Err(format!("{path} v{version}: raw body differs\n  django: {d_body:.300}\n  rust:   {r_body:.300}"));
            }
        }
    }
    Ok(())
}

#[tokio::test]
#[ignore = "needs the reference environment (PostgreSQL :54329, Django :8000)"]
async fn read_only_routes_match_django() {
    let app = app();
    let cases: &[(&str, &str)] = &[
        (ADMIN, "/api/users/"),
        (ADMIN, "/api/users/?page_size=2"),
        (ADMIN, "/api/users/?page_size=2&page=2"),
        (ADMIN, "/api/users/?ordering=-username"),
        (ADMIN, "/api/users/?ordering=username,bogus"),
        (ADMIN, "/api/users/?username__icontains=A"),
        (ADMIN, "/api/users/?username__iexact=ALICE"),
        (ADMIN, "/api/users/?username__istartswith=b&username__iendswith=b"),
        (ADMIN, "/api/users/?page=999"),
        (ADMIN, "/api/users/?page=abc"),
        (ADMIN, "/api/users/2/"),
        (ADMIN, "/api/users/3/"),
        (ADMIN, "/api/users/1/"),
        (ADMIN, "/api/users/abc/"),
        (ADMIN, "/api/users/999999/"),
        (ALICE, "/api/users/"),
        (CAROL, "/api/users/"),
        (ADMIN, "/api/groups/"),
        (ADMIN, "/api/groups/?ordering=-name"),
        (ADMIN, "/api/groups/?name__icontains=OR"),
        (ADMIN, "/api/groups/abc/"),
        (ADMIN, "/api/groups/999999/"),
        (CAROL, "/api/groups/"),
        (ADMIN, "/api/profile/"),
        (ALICE, "/api/profile/"),
        (CAROL, "/api/profile/"),
        (ADMIN, "/api/ui_settings/"),
        (ALICE, "/api/ui_settings/"),
        (BOB, "/api/ui_settings/"),
        (CAROL, "/api/ui_settings/"),
        ("ffffffffffffffffffffffffffffffffffffffff", "/api/users/"),
    ];
    let mut failures = Vec::new();
    let mut n = 0;
    for version in ["10", "9"] {
        for (token, path) in cases {
            n += 1;
            if let Err(e) = compare(&app, token, path, version).await {
                failures.push(e);
            }
        }
    }
    for f in &failures {
        eprintln!("DIFF {f}\n");
    }
    eprintln!("compared {n}, different {}", failures.len());
    assert!(failures.is_empty(), "{} of {n} cases differ", failures.len());
}

#[tokio::test]
#[ignore = "needs the reference environment (PostgreSQL :54329)"]
async fn anonymous_and_bad_token() {
    let app = app();
    let req = Request::get("/api/users/").header("host", "127.0.0.1:8000").body(Body::empty()).unwrap();
    let r = app.clone().oneshot(req).await.unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(r.headers().get("www-authenticate").unwrap(), "Token");
}

#[tokio::test]
#[ignore = "needs the reference environment (PostgreSQL :54329, Django :8000)"]
async fn non_writing_mutations_match_django() {
    let app = app();
    // (method, token, path, json body) – every case fails validation/permission or is a no-op,
    // so neither Django nor Rust writes to the reference database.
    let cases: &[(&str, &str, &str, Option<&str>)] = &[
        ("POST", ADMIN, "/api/users/", Some(r#"{}"#)),
        ("POST", ADMIN, "/api/users/", Some(r#"[1, 2]"#)),
        ("POST", ADMIN, "/api/users/", Some(r#"{"username": "admin", "email": "nope", "password": "1234", "groups": [999], "user_permissions": ["no_such_perm"], "is_staff": "maybe"}"#)),
        ("POST", ADMIN, "/api/users/", Some(r#"{"username": "bad name!", "groups": "x", "first_name": "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"}"#)),
        ("POST", ADMIN, "/api/users/", Some(r#"{"username": "x", "user_permissions": "view_user", "groups": [true]}"#)),
        ("POST", ADMIN, "/api/users/", Some(r#"{"username": "x", "password": "admin123", "date_joined": "garbage"}"#)),
        ("POST", ALICE, "/api/users/", Some(r#"{"username": "x", "is_superuser": true}"#)),
        ("PATCH", ADMIN, "/api/users/3/", Some(r#"{"email": "invalid", "password": "alice"}"#)),
        ("PUT", ADMIN, "/api/users/3/", Some(r#"{"email": "a@b.ch"}"#)),
        ("PATCH", ADMIN, "/api/users/999999/", Some(r#"{"email": "a@b.ch"}"#)),
        ("PATCH", ADMIN, "/api/users/abc/", Some(r#"{}"#)),
        ("PATCH", CAROL, "/api/users/3/", Some(r#"{}"#)),
        ("POST", ADMIN, "/api/users/4/deactivate_totp/", None),
        ("POST", CAROL, "/api/users/4/deactivate_totp/", None),
        ("POST", ADMIN, "/api/groups/", Some(r#"{}"#)),
        ("POST", ADMIN, "/api/groups/", Some(r#"{"name": "editors", "permissions": ["view_document", "nope"]}"#)),
        ("POST", ADMIN, "/api/groups/", Some(r#"{"name": "", "permissions": "x"}"#)),
        ("PUT", ADMIN, "/api/groups/1/", Some(r#"{"name": "viewers"}"#)),
        ("PATCH", ADMIN, "/api/groups/999999/", Some(r#"{}"#)),
        ("DELETE", CAROL, "/api/groups/1/", None),
        ("DELETE", ADMIN, "/api/groups/999999/", None),
        ("PATCH", ALICE, "/api/profile/", Some(r#"{"email": "x", "password": "123"}"#)),
        ("PATCH", ALICE, "/api/profile/", Some(r#"{"password": "Alice Reference"}"#)),
        ("PATCH", ALICE, "/api/profile/", Some(r#"{"password": ""}"#)),
        ("PATCH", ALICE, "/api/profile/", Some(r#"{"first_name": "Alice", "password": "**********"}"#)),
        ("POST", ALICE, "/api/profile/disconnect_social_account/", Some(r#"{"id": -1}"#)),
        ("POST", ALICE, "/api/profile/totp/", Some(r#"{"secret": "JBSWY3DPEHPK3PXP", "code": "000000x"}"#)),
        ("DELETE", ALICE, "/api/profile/totp/", None),
        ("POST", ALICE, "/api/ui_settings/", Some(r#"{"settings": "not a dict"}"#)),
        ("POST", ALICE, "/api/ui_settings/", Some(r#"{"settings": [1]}"#)),
        ("POST", CAROL, "/api/ui_settings/", Some(r#"{"settings": {}}"#)),
        ("POST", "", "/api/token/", Some(r#"{}"#)),
        ("POST", "", "/api/token/", Some(r#"{"username": "alice", "password": "wrong"}"#)),
        ("POST", "", "/api/token/", Some(r#"{"username": "", "password": ""}"#)),
    ];
    let mut failures = Vec::new();
    for (method, token, path, body) in cases {
        if let Err(e) = compare_with(&app, method, token, path, "10", *body).await {
            failures.push(e);
        }
    }
    for f in &failures {
        eprintln!("DIFF {f}
");
    }
    eprintln!("compared {}, different {}", cases.len(), failures.len());
    assert!(failures.is_empty(), "{} of {} cases differ", failures.len(), cases.len());
}
