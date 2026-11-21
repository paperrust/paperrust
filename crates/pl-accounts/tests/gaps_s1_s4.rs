//! Gaps S1–S4 (docs/LUECKEN-NGX.md) end-to-end through the account routes of pl-accounts:
//!
//! * S1 `ACCOUNT_EMAIL_VERIFICATION=mandatory`: login without a confirmed address → redirect to
//!   "confirm email" (instead of 404), after confirmation normal login; signup with a
//!   taken address → enumeration protection (no new user, notice page).
//! * S2 `PAPERLESS_APPS` with GitHub and an unknown provider: login page stays usable,
//!   GitHub button and redirect, unknown provider 404.
//! * S3 closed registration → notice page (200).
//! * S4 login throttle ends up in the shared table `paperrust_cache`.
//!
//! Own throwaway database `pl_gaps_accounts_test` on the reference PostgreSQL (schema from
//! `reference/schema/postgres.sql`), never `paperless`; dropped at the end. Mails go to a
//! closed local port (sending fails and is only logged).
//!
//! ```text
//! cargo test -p pl-accounts -j 2 --test gaps_s1_s4 -- --ignored
//! ```

use axum::body::Body;
use axum::http::{Request, StatusCode};
use sqlx::PgPool;
use tower::ServiceExt as _;

const DB: &str = "pl_gaps_accounts_test";
const ADMIN_URL: &str = "postgres://paperless:paperless@127.0.0.1:54329/postgres";

async fn admin() -> PgPool {
    sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(ADMIN_URL).await.expect("admin connect")
}

async fn fresh_db() -> PgPool {
    let a = admin().await;
    sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&a).await.expect("drop");
    sqlx::query(&format!("CREATE DATABASE {DB} TEMPLATE template0")).execute(&a).await.expect("create");
    let url = format!("postgres://paperless:paperless@127.0.0.1:54329/{DB}");
    // load the schema on a separate connection (the dump sets search_path to '').
    let schema: String =
        include_str!("../../../reference/schema/postgres.sql").lines().filter(|l| !l.starts_with('\\')).collect::<Vec<_>>().join("\n");
    {
        use sqlx::Connection as _;
        let mut c = sqlx::PgConnection::connect(&url).await.expect("connect");
        sqlx::raw_sql(&schema).execute(&mut c).await.expect("schema");
        c.close().await.expect("close");
    }
    sqlx::postgres::PgPoolOptions::new().max_connections(4).connect(&url).await.expect("db")
}

async fn drop_db() {
    let a = admin().await;
    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&a).await;
}

fn app(db: &PgPool, extra: &[(&str, &str)]) -> axum::Router {
    let mut pairs: Vec<(String, String)> = vec![
        ("PAPERLESS_SECRET_KEY".into(), "unit-test-only-secret-key-0123456789abcdef".into()),
        ("PAPERLESS_EMAIL_HOST".into(), "127.0.0.1".into()),
        ("PAPERLESS_EMAIL_PORT".into(), "1".into()),
    ];
    pairs.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    let cfg = pl_core::Config::from_env_map(pl_core::config::Env::from_pairs(pairs)).expect("config");
    let st = pl_core::AppState::with_pool(cfg, db.clone()).expect("state");
    let mut by_path: std::collections::BTreeMap<&str, axum::routing::MethodRouter<pl_core::AppState>> = Default::default();
    for r in pl_accounts::routes() {
        let h = r.handler;
        let merged = match by_path.remove(r.path) {
            Some(p) => p.merge(h),
            None => h,
        };
        by_path.insert(r.path, merged);
    }
    let mut router = axum::Router::new();
    for (p, h) in by_path {
        router = router.route(p, h);
    }
    router.with_state(st)
}

#[derive(Default, Clone)]
struct Jar(Vec<(String, String)>);

impl Jar {
    fn header(&self) -> String {
        self.0.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ")
    }
    fn update(&mut self, r: &Resp) {
        for c in &r.cookies {
            let first = c.split(';').next().unwrap_or("");
            if let Some((k, v)) = first.split_once('=') {
                self.0.retain(|(n, _)| n != k);
                if !v.is_empty() && v != "\"\"" {
                    self.0.push((k.to_string(), v.to_string()));
                }
            }
        }
    }
    fn get(&self, k: &str) -> Option<&str> {
        self.0.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }
}

struct Resp {
    status: StatusCode,
    location: Option<String>,
    cookies: Vec<String>,
    body: String,
}

async fn send(app: &axum::Router, method: &str, path: &str, jar: &mut Jar, form: Option<&[(&str, &str)]>) -> Resp {
    let peer: std::net::SocketAddr = ([127, 0, 0, 1], 40000).into();
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("host", "localhost")
        .header("cookie", jar.header())
        .extension(axum::extract::ConnectInfo(peer));
    let body = match form {
        Some(f) => {
            b = b.header("content-type", "application/x-www-form-urlencoded");
            Body::from(url::form_urlencoded::Serializer::new(String::new()).extend_pairs(f.iter()).finish())
        }
        None => Body::empty(),
    };
    let r = app.clone().oneshot(b.body(body).expect("request")).await.expect("response");
    let status = r.status();
    let location = r.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_string);
    let cookies = r.headers().get_all("set-cookie").iter().filter_map(|v| v.to_str().ok()).map(str::to_string).collect();
    let bytes = axum::body::to_bytes(r.into_body(), 1 << 22).await.expect("body");
    let resp = Resp { status, location, cookies, body: String::from_utf8_lossy(&bytes).into_owned() };
    jar.update(&resp);
    resp
}

fn csrf_from(body: &str) -> String {
    let re = regex::Regex::new(r#"name="csrfmiddlewaretoken" value="([^"]+)""#).expect("re");
    re.captures(body).map(|c| c[1].to_string()).expect("csrf token in page")
}

async fn add_user(db: &PgPool, name: &str, email: &str, pw: &str) -> i64 {
    let pw = pw.to_string();
    let enc = tokio::task::spawn_blocking(move || pl_authz::hashers::make_password(&pw)).await.expect("hash");
    let (id,): (i64,) = sqlx::query_as(
        "INSERT INTO auth_user (password, is_superuser, username, first_name, last_name, email, is_staff, is_active, date_joined) \
         VALUES ($1, false, $2, '', '', $3, false, true, now()) RETURNING id::int8",
    )
    .bind(enc)
    .bind(name)
    .bind(email)
    .fetch_one(db)
    .await
    .expect("user");
    id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs the reference PostgreSQL on 127.0.0.1:54329"]
async fn gaps_s1_to_s4() {
    let db = fresh_db().await;
    let uid = add_user(&db, "mona", "mona@example.invalid", "geheim-123").await;
    let providers = r#"{"github": {"APPS": [{"client_id": "gh-cid", "secret": "gh-secret", "key": ""}]}}"#;
    let app = app(
        &db,
        &[
            ("PAPERLESS_ACCOUNT_EMAIL_VERIFICATION", "mandatory"),
            ("PAPERLESS_ACCOUNT_ALLOW_SIGNUPS", "true"),
            ("PAPERLESS_APPS", "allauth.socialaccount.providers.github,allauth.socialaccount.providers.facebook"),
            ("PAPERLESS_SOCIALACCOUNT_PROVIDERS", providers),
        ],
    );

    // --- S2: login page with GitHub button, unknown provider skipped
    let mut jar = Jar::default();
    let page = send(&app, "GET", "/accounts/login/", &mut jar, None).await;
    assert_eq!(page.status, StatusCode::OK, "login page must not be a 404 (S1/S2)");
    assert!(page.body.contains("action=\"/accounts/github/login/?process="), "GitHub button");
    let tok = csrf_from(&page.body);
    let gh = send(&app, "POST", "/accounts/github/login/", &mut jar, Some(&[("csrfmiddlewaretoken", tok.as_str())])).await;
    assert_eq!(gh.status, StatusCode::FOUND);
    let loc = gh.location.clone().unwrap_or_default();
    assert!(loc.starts_with("https://github.com/login/oauth/authorize?client_id=gh-cid&redirect_uri=https%3A%2F%2Flocalhost%2Faccounts%2Fgithub%2Flogin%2Fcallback%2F&scope=&response_type=code&state="), "{loc}");
    let fb = send(&app, "GET", "/accounts/facebook/login/", &mut jar, None).await;
    assert_eq!(fb.status, StatusCode::NOT_FOUND);
    // callback without valid state → error page (401), no 500
    let cb = send(&app, "GET", "/accounts/github/login/callback/?state=nope&code=x", &mut jar, None).await;
    assert_eq!(cb.status, StatusCode::UNAUTHORIZED);

    // --- S1: login without confirmed address → "confirm email"
    let mut jar = Jar::default();
    let page = send(&app, "GET", "/accounts/login/", &mut jar, None).await;
    let tok = csrf_from(&page.body);
    let r = send(
        &app,
        "POST",
        "/accounts/login/",
        &mut jar,
        Some(&[("csrfmiddlewaretoken", tok.as_str()), ("login", "mona"), ("password", "geheim-123")]),
    )
    .await;
    assert_eq!(r.status, StatusCode::FOUND, "{}", r.body);
    assert_eq!(r.location.as_deref(), Some("/accounts/confirm-email/"));
    let (rows, verified): (i64, bool) = sqlx::query_as("SELECT count(*), bool_or(verified) FROM account_emailaddress WHERE user_id = $1")
        .bind(uid)
        .fetch_one(&db)
        .await
        .expect("addr");
    assert_eq!((rows, verified), (1, false), "address synced from auth_user, still unverified");
    let sent = send(&app, "GET", "/accounts/confirm-email/", &mut jar, None).await;
    assert_eq!(sent.status, StatusCode::OK);
    assert!(sent.body.contains("<form") || sent.body.contains("href=\"/accounts/login/\""));
    // not logged in
    let again = send(&app, "GET", "/accounts/login/", &mut jar, None).await;
    assert_eq!(again.status, StatusCode::OK);

    // after confirmation: normal login
    sqlx::query("UPDATE account_emailaddress SET verified = true WHERE user_id = $1").bind(uid).execute(&db).await.expect("verify");
    let tok = csrf_from(&again.body);
    let r = send(
        &app,
        "POST",
        "/accounts/login/",
        &mut jar,
        Some(&[("csrfmiddlewaretoken", tok.as_str()), ("login", "mona"), ("password", "geheim-123")]),
    )
    .await;
    assert_eq!(r.status, StatusCode::FOUND);
    assert_eq!(r.location.as_deref(), Some("/dashboard"));
    assert!(jar.get("sessionid").is_some());

    // --- S1: signup with taken address → enumeration protection
    let mut jar = Jar::default();
    let page = send(&app, "GET", "/accounts/signup/", &mut jar, None).await;
    assert_eq!(page.status, StatusCode::OK);
    let tok = csrf_from(&page.body);
    let r = send(
        &app,
        "POST",
        "/accounts/signup/",
        &mut jar,
        Some(&[
            ("csrfmiddlewaretoken", tok.as_str()),
            ("username", "mona2"),
            ("email", "MONA@example.invalid"),
            ("password1", "Sehr-langes-Passwort-42"),
            ("password2", "Sehr-langes-Passwort-42"),
        ]),
    )
    .await;
    assert_eq!(r.status, StatusCode::FOUND, "{}", r.body);
    assert_eq!(r.location.as_deref(), Some("/accounts/confirm-email/"));
    let (n,): (i64,) = sqlx::query_as("SELECT count(*) FROM auth_user WHERE username = 'mona2'").fetch_one(&db).await.expect("count");
    assert_eq!(n, 0, "no account for a taken address");

    // --- S4: failed logins count in the shared table
    let mut jar = Jar::default();
    let page = send(&app, "GET", "/accounts/login/", &mut jar, None).await;
    let tok = csrf_from(&page.body);
    let r = send(
        &app,
        "POST",
        "/accounts/login/",
        &mut jar,
        Some(&[("csrfmiddlewaretoken", tok.as_str()), ("login", "mona"), ("password", "falsch")]),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let (rl,): (i64,) = sqlx::query_as("SELECT count(*) FROM paperrust_cache WHERE cache_key LIKE ':1:allauth:rl:%'")
        .fetch_one(&db)
        .await
        .expect("rl rows");
    assert!(rl >= 1, "rate-limit rows in paperrust_cache");

    // --- S3: closed registration → notice page
    let closed = app_closed(&db);
    let mut jar = Jar::default();
    let r = send(&closed, "GET", "/accounts/signup/", &mut jar, None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains(pl_accounts::i18n::t("en-us", "auth.signup_closed.text")), "signup closed text");

    // --- S2: complete GitHub flow against a local mock service (GITHUB_URL)
    let fake = fake_github().await;
    let providers =
        format!(r#"{{"github": {{"GITHUB_URL": "{fake}", "APPS": [{{"client_id": "gh-cid", "secret": "gh-secret", "key": ""}}]}}}}"#);
    let gh_app = self::app(
        &db,
        &[
            ("PAPERLESS_APPS", "allauth.socialaccount.providers.github"),
            ("PAPERLESS_SOCIALACCOUNT_PROVIDERS", providers.as_str()),
            ("PAPERLESS_SOCIAL_AUTO_SIGNUP", "true"),
            ("PAPERLESS_SOCIALACCOUNT_ALLOW_SIGNUPS", "true"),
        ],
    );
    let mut jar = Jar::default();
    let page = send(&gh_app, "GET", "/accounts/login/", &mut jar, None).await;
    let tok = csrf_from(&page.body);
    let start = send(&gh_app, "POST", "/accounts/github/login/", &mut jar, Some(&[("csrfmiddlewaretoken", tok.as_str())])).await;
    let loc = start.location.clone().unwrap_or_default();
    assert!(loc.starts_with(&format!("{fake}/login/oauth/authorize?")), "{loc}");
    let state =
        url::Url::parse(&loc).ok().and_then(|u| u.query_pairs().find(|(k, _)| k == "state").map(|(_, v)| v.into_owned())).expect("state");
    let cb = send(&gh_app, "GET", &format!("/accounts/github/login/callback/?code=good&state={state}"), &mut jar, None).await;
    assert_eq!(cb.status, StatusCode::FOUND, "{}", cb.body);
    assert_eq!(cb.location.as_deref(), Some("/dashboard"));
    let (uname, prov, suid): (String, String, String) =
        sqlx::query_as("SELECT u.username, s.provider, s.uid FROM socialaccount_socialaccount s JOIN auth_user u ON u.id = s.user_id")
            .fetch_one(&db)
            .await
            .expect("social account");
    assert_eq!((uname.as_str(), prov.as_str(), suid.as_str()), ("octocat", "github", "583231"));

    db.close().await;
    drop_db().await;
}

/// Mock GitHub: token endpoint (form response like GitHub) and `/api/v3/user`.
async fn fake_github() -> String {
    let app = axum::Router::new()
        .route(
            "/login/oauth/access_token",
            axum::routing::post(|body: String| async move {
                let ok = body.contains("code=good") && body.contains("client_secret=gh-secret");
                let text = if ok { "access_token=tok123&scope=&token_type=bearer" } else { "error=bad_verification_code" };
                ([("content-type", "application/x-www-form-urlencoded")], text)
            }),
        )
        .route(
            "/api/v3/user",
            axum::routing::get(|h: axum::http::HeaderMap| async move {
                if h.get("authorization").and_then(|v| v.to_str().ok()) != Some("token tok123") {
                    return (StatusCode::UNAUTHORIZED, axum::Json(serde_json::json!({"message": "Bad credentials"})));
                }
                (
                    StatusCode::OK,
                    axum::Json(
                        serde_json::json!({"id": 583231, "login": "octocat", "name": "The Octocat", "email": "octo@example.invalid"}),
                    ),
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

fn app_closed(db: &PgPool) -> axum::Router {
    app(db, &[("PAPERLESS_ACCOUNT_ALLOW_SIGNUPS", "false")])
}
