//! Parity against W3's own Django instance (tests/parity/pl-accounts/w3-django.sh):
//! Django 127.0.0.1:8120 on DB `pl_w3_ref` (copy of `paperless_snapshot`), Redis prefix
//! `w3ref`, SMTP sink 127.0.0.1:8125. The Rust router runs in-process on the same DB and
//! the same cache. Compared are status, location, content type, cache-control, cookies
//! (names/attributes, value "same as request"/"new"/"empty"), authenticated marker and the
//! HTML (CSRF token normalized). Plus session exchange Rust↔Django and security cases.
//!
//! Start: `bash tests/parity/pl-accounts/w3-django.sh reset && bash tests/parity/pl-accounts/w3-django.sh start`
//! Run:   `cargo test -p pl-accounts -j 1 --test parity -- --ignored --test-threads=1`

mod common;

use std::collections::HashMap;
use std::net::SocketAddr;

use axum::Router;
use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::Request;
use pl_core::config::Env;
use pl_core::{AppState, Config};
use tower::ServiceExt as _;

const DJANGO: &str = "http://127.0.0.1:8120";
const HOST: &str = "127.0.0.1:8120";
const SECRET: &str = "paperless-rs-reference-secret-key-local-only-0123456789";
const TOTP_SECRET: &str = "JBSWY3DPEHPK3PXP";

fn config() -> Config {
    let providers = r#"{"openid_connect":{"OAUTH_PKCE_ENABLED":true,"APPS":[{"provider_id":"fakeidp","name":"Fake IdP","client_id":"w3client","secret":"w3secret","settings":{"server_url":"http://127.0.0.1:8126"}}]}}"#;
    Config::from_env_map(Env::from_pairs([
        ("PAPERLESS_DBHOST", "127.0.0.1"),
        ("PAPERLESS_DBPORT", "54329"),
        ("PAPERLESS_DBNAME", "pl_w3_ref"),
        ("PAPERLESS_DBUSER", "paperless"),
        ("PAPERLESS_DBPASS", "paperless"),
        ("PAPERLESS_SECRET_KEY", SECRET),
        ("PAPERLESS_TIME_ZONE", "Europe/Zurich"),
        ("PAPERLESS_URL", DJANGO),
        ("PAPERLESS_ALLOWED_HOSTS", "127.0.0.1,localhost"),
        ("PAPERLESS_CSRF_TRUSTED_ORIGINS", "http://127.0.0.1:8120,http://localhost:8120"),
        ("PAPERLESS_DEBUG", "false"),
        ("PAPERLESS_EMAIL_HOST", "127.0.0.1"),
        ("PAPERLESS_EMAIL_PORT", "8125"),
        ("PAPERLESS_EMAIL_FROM", "paperless@example.invalid"),
        ("PAPERLESS_ACCOUNT_ALLOW_SIGNUPS", "true"),
        ("PAPERLESS_SOCIALACCOUNT_PROVIDERS", providers),
        ("PAPERLESS_APPS", "allauth.socialaccount.providers.openid_connect"),
    ]))
    .expect("w3 config")
}

/// Rust side: cache is process-local (no shared Redis with Django any more, 2026-11-14);
/// `clear_rl` therefore clears both sides.
static RUST_STATE: std::sync::OnceLock<AppState> = std::sync::OnceLock::new();

fn app() -> (Router, AppState) {
    let state = AppState::new(config()).expect("state");
    let _ = RUST_STATE.set(state.clone());
    let mut router: Router<AppState> = Router::new();
    for r in pl_accounts::routes() {
        router = router.route(r.path, r.handler);
    }
    (router.with_state(state.clone()), state)
}

#[derive(Debug, Clone)]
struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
    authenticated: bool,
}

impl Resp {
    fn header(&self, n: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.as_str())
    }
    fn cookies(&self) -> Vec<String> {
        self.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie")).map(|(_, v)| v.clone()).collect()
    }
}

#[derive(Default, Clone, Debug)]
struct Jar(HashMap<String, String>);

impl Jar {
    fn header(&self) -> String {
        let mut v: Vec<_> = self.0.iter().filter(|(_, v)| !v.is_empty() && v.as_str() != "\"\"").map(|(k, v)| format!("{k}={v}")).collect();
        v.sort();
        v.join("; ")
    }
    fn update(&mut self, r: &Resp) {
        for c in r.cookies() {
            let first = c.split(';').next().unwrap_or("");
            if let Some((k, v)) = first.split_once('=') {
                if v == "\"\"" {
                    self.0.remove(k);
                } else {
                    self.0.insert(k.to_string(), v.to_string());
                }
            }
        }
    }
}

enum Side<'a> {
    Django,
    Rust(&'a Router),
}

async fn send(side: &Side<'_>, method: &str, path: &str, jar: &Jar, form: Option<&[(&str, &str)]>, extra: &[(&str, &str)]) -> Resp {
    let body = form.map(|f| url::form_urlencoded::Serializer::new(String::new()).extend_pairs(f.iter()).finish());
    let cookie = jar.header();
    match side {
        Side::Django => {
            let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
            let mut rb = client.request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), format!("{DJANGO}{path}")).header("Host", HOST);
            if !cookie.is_empty() {
                rb = rb.header("Cookie", cookie);
            }
            for (k, v) in extra {
                rb = rb.header(*k, *v);
            }
            if let Some(b) = body {
                rb = rb.header("Content-Type", "application/x-www-form-urlencoded").body(b);
            }
            let r = rb.send().await.expect("django reachable");
            let status = r.status().as_u16();
            let authenticated = r.headers().contains_key("x-api-version");
            let headers = r.headers().iter().map(|(k, v)| (k.to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
            Resp { status, headers, body: r.text().await.unwrap_or_default(), authenticated }
        }
        Side::Rust(app) => {
            let mut rb = Request::builder().method(method).uri(path).header("host", HOST);
            if !cookie.is_empty() {
                rb = rb.header("cookie", cookie);
            }
            for (k, v) in extra {
                rb = rb.header(*k, *v);
            }
            let mut req = match body {
                Some(b) => rb.header("content-type", "application/x-www-form-urlencoded").body(Body::from(b)).unwrap(),
                None => rb.body(Body::empty()).unwrap(),
            };
            req.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 50000))));
            let r = (*app).clone().oneshot(req).await.unwrap();
            let status = r.status().as_u16();
            let authenticated = r.extensions().get::<pl_core::Authenticated>().is_some();
            let headers = r.headers().iter().map(|(k, v)| (k.to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
            let bytes = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
            Resp { status, headers, body: String::from_utf8_lossy(&bytes).into_owned(), authenticated }
        }
    }
}

/// Own templates: the interface of the page is compared, not the wording.
fn norm_html(s: &str) -> String {
    common::page_skeleton(s)
}

fn norm_cookie(c: &str, jar: &Jar) -> String {
    let mut parts: Vec<String> = c.split("; ").map(str::to_string).collect();
    if let Some((k, v)) = parts[0].clone().split_once('=') {
        let class = if v == "\"\"" {
            "EMPTY".to_string()
        } else if jar.0.get(k).is_some_and(|old| old == v) {
            "SAME".to_string()
        } else if k == "messages" {
            // Own message texts: only compare presence
            "MSG".to_string()
        } else {
            "NEW".to_string()
        };
        parts[0] = format!("{k}={class}");
    }
    for p in parts.iter_mut() {
        if p.starts_with("expires=") && !p.contains("1970") {
            *p = "expires=D".into();
        }
    }
    parts.join("; ")
}

/// Comparison of two responses (same request jar).
fn diff(case: &str, d: &Resp, r: &Resp, jar: &Jar) -> Vec<String> {
    let mut out = Vec::new();
    if d.status != r.status {
        out.push(format!("{case}: status django={} rust={}", d.status, r.status));
    }
    for h in ["location", "content-type", "cache-control", "allow"] {
        if d.header(h) != r.header(h) {
            out.push(format!("{case}: header {h} django={:?} rust={:?}", d.header(h), r.header(h)));
        }
    }
    if d.header("expires").is_some() != r.header("expires").is_some() {
        out.push(format!("{case}: expires presence differs"));
    }
    if d.authenticated != r.authenticated {
        out.push(format!("{case}: authenticated django={} rust={}", d.authenticated, r.authenticated));
    }
    let dc: Vec<String> = d.cookies().iter().map(|c| norm_cookie(c, jar)).collect();
    let rc: Vec<String> = r.cookies().iter().map(|c| norm_cookie(c, jar)).collect();
    if dc != rc {
        out.push(format!("{case}: cookies\n  django={dc:?}\n  rust  ={rc:?}"));
    }
    let (db, rb) = (norm_html(&d.body), norm_html(&r.body));
    if db != rb {
        let pos = db.bytes().zip(rb.bytes()).position(|(a, b)| a != b).unwrap_or(db.len().min(rb.len()));
        let ctx = |s: &str| s.get(pos.saturating_sub(80)..(pos + 120).min(s.len())).unwrap_or("").to_string();
        out.push(format!("{case}: body differs at {pos}\n  django: {:?}\n  rust  : {:?}", ctx(&db), ctx(&rb)));
    }
    out
}

fn csrf_from(body: &str) -> String {
    let re = regex::Regex::new(r#"name="csrfmiddlewaretoken" value="([A-Za-z0-9]{64})""#).unwrap();
    re.captures(body).map(|c| c[1].to_string()).unwrap_or_default()
}

async fn redis_del(pattern: &str) {
    let client = redis::Client::open("redis://127.0.0.1:63799").unwrap();
    let mut c = client.get_multiplexed_async_connection().await.unwrap();
    let keys: Vec<String> = redis::cmd("KEYS").arg(pattern).query_async(&mut c).await.unwrap_or_default();
    for k in keys {
        let _: Result<i64, _> = redis::cmd("DEL").arg(&k).query_async(&mut c).await;
    }
}

async fn clear_rl() {
    // Reference Django: cache in Garnet
    redis_del("w3ref:1:allauth:rl:*").await;
    if let Some(st) = RUST_STATE.get() {
        st.cache().local().clear();
        // S4: allauth rate limits are always in the shared table
        let _ = sqlx::query("DELETE FROM paperrust_cache WHERE cache_key LIKE ':1:allauth:rl:%'").execute(st.db()).await;
    }
}

fn totp_now() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    // not right at the window boundary
    if now % 30 > 26 {
        std::thread::sleep(std::time::Duration::from_secs(4));
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    pl_auth_api::totp::format_hotp_value(pl_auth_api::totp::hotp_value(TOTP_SECRET, now / 30).unwrap())
}

/// GET page (cookie jar with csrftoken from a previous GET), then comparison.
async fn same_get(app: &Router, case: &str, path: &str, jar: &Jar, extra: &[(&str, &str)]) -> Vec<String> {
    let d = send(&Side::Django, "GET", path, jar, None, extra).await;
    let r = send(&Side::Rust(app), "GET", path, jar, None, extra).await;
    diff(case, &d, &r, jar)
}

async fn fresh_jar(side: &Side<'_>) -> (Jar, String) {
    let mut jar = Jar::default();
    let r = send(side, "GET", "/accounts/login/", &jar, None, &[]).await;
    jar.update(&r);
    (jar, csrf_from(&r.body))
}

/// POST with own jar on both sides (CSRF token per side), comparison of the responses.
async fn same_post(app: &Router, case: &str, path: &str, form: &[(&str, &str)], extra: &[(&str, &str)]) -> (Vec<String>, (Resp, Jar), (Resp, Jar)) {
    clear_rl().await;
    let (mut dj, dt) = fresh_jar(&Side::Django).await;
    let (mut rj, rt) = fresh_jar(&Side::Rust(app)).await;
    let mut df: Vec<(&str, &str)> = vec![("csrfmiddlewaretoken", dt.as_str())];
    df.extend_from_slice(form);
    let mut rf: Vec<(&str, &str)> = vec![("csrfmiddlewaretoken", rt.as_str())];
    rf.extend_from_slice(form);
    let d = send(&Side::Django, "POST", path, &dj, Some(&df), extra).await;
    let r = send(&Side::Rust(app), "POST", path, &rj, Some(&rf), extra).await;
    // compare cookie classes relative to the respective jar
    let mut out = Vec::new();
    let dc: Vec<String> = d.cookies().iter().map(|c| norm_cookie(c, &dj)).collect();
    let rc: Vec<String> = r.cookies().iter().map(|c| norm_cookie(c, &rj)).collect();
    if dc != rc {
        out.push(format!("{case}: cookies\n  django={dc:?}\n  rust  ={rc:?}"));
    }
    let mut d2 = d.clone();
    let mut r2 = r.clone();
    d2.headers.retain(|(k, _)| k != "set-cookie");
    r2.headers.retain(|(k, _)| k != "set-cookie");
    out.extend(diff(case, &d2, &r2, &Jar::default()));
    dj.update(&d);
    rj.update(&r);
    (out, (d, dj), (r, rj))
}

#[tokio::test]
#[ignore = "needs W3 Django on 127.0.0.1:8120 (tests/parity/pl-accounts/w3-django.sh start)"]
async fn accounts_parity() {
    let (app, state) = app();
    let db = state.db().clone();
    // MFA user carol (TOTP + recovery codes), test state
    sqlx::query("DELETE FROM mfa_authenticator WHERE user_id = 5").execute(&db).await.unwrap();
    sqlx::query("INSERT INTO mfa_authenticator(user_id,type,data,created_at) VALUES (5,'totp','{\"secret\":\"JBSWY3DPEHPK3PXP\"}',now()),(5,'recovery_codes','{\"seed\":\"00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff0011223344556677\",\"used_mask\":0}',now())").execute(&db).await.unwrap();
    redis_del("w3ref:1:allauth*").await;
    let (orig_bob,): (String,) = sqlx::query_as("SELECT password FROM auth_user WHERE username = 'bob'").fetch_one(&db).await.unwrap();
    let mut fails: Vec<String> = Vec::new();
    let mut n = 0usize;
    let t0 = std::time::Instant::now();
    macro_rules! check {
        ($v:expr) => {{
            let v: Vec<String> = $v;
            n += 1;
            eprintln!("check {n} at {:?}", t0.elapsed());
            fails.extend(v);
        }};
    }

    // --- GET pages ---------------------------------------------------------------
    let (jar, _) = fresh_jar(&Side::Django).await;
    for (case, path, extra) in [
        ("login", "/accounts/login/", vec![]),
        ("login.de", "/accounts/login/", vec![("Accept-Language", "de-DE,de;q=0.9")]),
        ("login.fr_cookie", "/accounts/login/", vec![("Cookie", "django_language=fr")]),
        ("login.next", "/accounts/login/?next=/documents/%3Fa%3D1", vec![]),
        ("login.evil_next", "/accounts/login/?next=https://evil.example/", vec![]),
        ("login.sharelink", "/accounts/login/?sharelink_notfound=1", vec![]),
        ("login.loggedout", "/accounts/login/?loggedout=1", vec![]),
        ("api_auth.login", "/api/auth/login/", vec![]),
        ("signup", "/accounts/signup/", vec![]),
        ("signup.next", "/accounts/signup/?next=/documents/", vec![]),
        ("inactive", "/accounts/account_inactive/", vec![]),
        ("reset", "/accounts/password/reset/", vec![]),
        ("reset.de", "/accounts/password/reset/", vec![("Accept-Language", "de")]),
        ("reset_done", "/accounts/password/reset/done/", vec![]),
        ("key_done", "/accounts/password/reset/key/done/", vec![]),
        ("key_bad", "/accounts/password/reset/key/3-abc-def/", vec![]),
        ("key_nouid", "/accounts/password/reset/key/zz-1-2/", vec![]),
        ("mfa_no_stage", "/accounts/2fa/authenticate/", vec![]),
        ("logout_anon", "/accounts/logout/", vec![]),
        ("logout_next", "/accounts/logout/?next=/accounts/login/?next=/", vec![]),
        ("logout_evil", "/accounts/logout/?next=//evil.example/", vec![]),
    ] {
        let mut j = jar.clone();
        let mut ex: Vec<(&str, &str)> = Vec::new();
        for (k, v) in &extra {
            if *k == "Cookie" {
                let (a, b) = v.split_once('=').unwrap();
                j.0.insert(a.into(), b.into());
            } else {
                ex.push((k, v));
            }
        }
        check!(same_get(&app, case, path, &j, &ex).await);
    }
    // without csrftoken cookie (cookie new)
    check!(same_get(&app, "login.nocookie", "/accounts/login/", &Jar::default(), &[]).await);

    // --- Login form ---------------------------------------------------------------
    for (case, form) in [
        ("post.wrong_pw", vec![("login", "admin"), ("password", "wrong")]),
        ("post.unknown", vec![("login", "nobody-x"), ("password", "x")]),
        ("post.empty", vec![("login", ""), ("password", "")]),
        ("post.long", vec![("login", &*"a".repeat(151)), ("password", "x")]),
        ("post.inactive", vec![("login", "dave"), ("password", "dave")]),
        ("post.ok", vec![("login", "bob"), ("password", "bob")]),
        ("post.ok_case", vec![("login", "BOB"), ("password", "bob")]),
        ("post.mfa", vec![("login", "carol"), ("password", "carol")]),
    ] {
        let (v, _, _) = same_post(&app, case, "/accounts/login/", &form, &[]).await;
        check!(v);
    }
    for (case, path) in [("post.next", "/accounts/login/?next=/documents/"), ("post.evil_next", "/accounts/login/?next=https://evil.example/x"), ("post.slashes", "/accounts/login/?next=///evil.example")] {
        let (v, _, _) = same_post(&app, case, path, &[("login", "alice"), ("password", "alice")], &[]).await;
        check!(v);
    }

    // --- CSRF -------------------------------------------------------------------------
    for (case, cookie, token, origin) in [
        ("csrf.no_cookie", false, String::new(), None),
        ("csrf.no_token", true, String::new(), None),
        ("csrf.bad_token", true, "x".repeat(64), None),
        ("csrf.bad_origin", true, "VALID".to_string(), Some("http://evil.example")),
        ("csrf.good_origin", true, "VALID".to_string(), Some("http://127.0.0.1:8120")),
    ] {
        clear_rl().await;
        let mut results = Vec::new();
        for side in [Side::Django, Side::Rust(&app)] {
            let (jar, t) = if cookie { fresh_jar(&side).await } else { (Jar::default(), String::new()) };
            let tok = if token == "VALID" { t } else { token.clone() };
            let ex: Vec<(&str, &str)> = origin.map(|o| vec![("Origin", o)]).unwrap_or_default();
            let r = send(&side, "POST", "/accounts/login/", &jar, Some(&[("csrfmiddlewaretoken", &tok), ("login", "admin"), ("password", "wrong")]), &ex).await;
            results.push((r, jar));
        }
        let (d, dj) = &results[0];
        let (r, rj) = &results[1];
        let mut d2 = d.clone();
        let mut r2 = r.clone();
        let dc: Vec<String> = d.cookies().iter().map(|c| norm_cookie(c, dj)).collect();
        let rc: Vec<String> = r.cookies().iter().map(|c| norm_cookie(c, rj)).collect();
        if dc != rc {
            fails.push(format!("{case}: cookies {dc:?} vs {rc:?}"));
        }
        d2.headers.retain(|(k, _)| k != "set-cookie");
        r2.headers.retain(|(k, _)| k != "set-cookie");
        check!(diff(case, &d2, &r2, &Jar::default()));
    }

    // --- Sessions mutually accepted ------------------------------------------------
    clear_rl().await;
    {
        // Rust login → Django API
        let (mut rj, rt) = fresh_jar(&Side::Rust(&app)).await;
        let r = send(&Side::Rust(&app), "POST", "/accounts/login/", &rj, Some(&[("csrfmiddlewaretoken", &rt), ("login", "alice"), ("password", "alice")]), &[]).await;
        rj.update(&r);
        let api = send(&Side::Django, "GET", "/api/ui_settings/", &rj, None, &[]).await;
        if api.status != 200 || !api.body.contains("\"username\":\"alice\"") {
            fails.push(format!("session.rust_to_django: {} {}", api.status, &api.body[..api.body.len().min(200)]));
        }
        // same session at Rust: logged in → login page redirects
        let again = send(&Side::Rust(&app), "GET", "/accounts/login/", &rj, None, &[]).await;
        if again.status != 302 || again.header("location") != Some("/dashboard") {
            fails.push(format!("session.rust_reuse: {} {:?}", again.status, again.header("location")));
        }
        // Rust logout → Django rejects session
        let lo = send(&Side::Rust(&app), "GET", "/accounts/logout/", &rj, None, &[]).await;
        let mut after = rj.clone();
        after.update(&lo);
        let api2 = send(&Side::Django, "GET", "/api/ui_settings/", &rj, None, &[]).await;
        if api2.status == 200 {
            fails.push("session.rust_logout: Django still accepts the session".into());
        }
        // logout messages appear on the Django login page as on the Rust side
        let pd = send(&Side::Django, "GET", "/accounts/login/?loggedout=1", &after, None, &[]).await;
        let pr = send(&Side::Rust(&app), "GET", "/accounts/login/?loggedout=1", &after, None, &[]).await;
        check!(diff("session.logout_messages", &pd, &pr, &after));
        n += 3;
    }
    {
        // Django login → Rust recognizes user; Rust logout ends Django session
        let (mut dj, dt) = fresh_jar(&Side::Django).await;
        let d = send(&Side::Django, "POST", "/accounts/login/", &dj, Some(&[("csrfmiddlewaretoken", &dt), ("login", "bob"), ("password", "bob")]), &[]).await;
        dj.update(&d);
        let r = send(&Side::Rust(&app), "GET", "/accounts/login/?next=/documents/", &dj, None, &[]).await;
        if r.status != 302 || r.header("location") != Some("/documents/") || !r.authenticated {
            fails.push(format!("session.django_to_rust: {} {:?}", r.status, r.header("location")));
        }
        let lo = send(&Side::Rust(&app), "POST", "/accounts/logout/", &dj, Some(&[("csrfmiddlewaretoken", &csrf_from(&send(&Side::Django, "GET", "/accounts/password/reset/", &dj, None, &[]).await.body))]), &[]).await;
        if lo.status != 302 {
            fails.push(format!("session.rust_logout_post: {}", lo.status));
        }
        let api = send(&Side::Django, "GET", "/api/ui_settings/", &dj, None, &[]).await;
        if api.status == 200 {
            fails.push("session.django_session_after_rust_logout still valid".into());
        }
        n += 3;
    }

    // --- MFA ----------------------------------------------------------------------------
    clear_rl().await;
    {
        let mut pair = Vec::new();
        for side in [Side::Django, Side::Rust(&app)] {
            let (mut j, t) = fresh_jar(&side).await;
            let r = send(&side, "POST", "/accounts/login/", &j, Some(&[("csrfmiddlewaretoken", &t), ("login", "carol"), ("password", "carol")]), &[]).await;
            j.update(&r);
            // MFA bypass: intermediate state is not logged in
            let api = send(&Side::Django, "GET", "/api/ui_settings/", &j, None, &[]).await;
            if api.status == 200 {
                fails.push("mfa.bypass: password step alone authenticates".into());
            }
            let page = send(&side, "GET", "/accounts/2fa/authenticate/", &j, None, &[]).await;
            j.update(&page);
            pair.push((j, page));
        }
        let (dj, dp) = &pair[0];
        let (rj, rp) = &pair[1];
        check!(diff("mfa.page", dp, rp, dj).into_iter().filter(|l| !l.contains("cookies")).collect());
        let _ = rj;
        // wrong code
        let mut wrong = Vec::new();
        for (i, side) in [Side::Django, Side::Rust(&app)].into_iter().enumerate() {
            let j = &pair[i].0;
            let t = csrf_from(&pair[i].1.body);
            wrong.push(send(&side, "POST", "/accounts/2fa/authenticate/", j, Some(&[("csrfmiddlewaretoken", &t), ("code", "000000")]), &[]).await);
        }
        check!(diff("mfa.wrong", &wrong[0], &wrong[1], &pair[0].0).into_iter().filter(|l| !l.contains("cookies")).collect());
        // correct code at Rust, the same code afterwards at Django → used up (shared cache)
        let code = totp_now();
        let rt = csrf_from(&wrong[1].body);
        let ok = send(&Side::Rust(&app), "POST", "/accounts/2fa/authenticate/", &pair[1].0, Some(&[("csrfmiddlewaretoken", &rt), ("code", &code)]), &[]).await;
        if ok.status != 302 || ok.header("location") != Some("/dashboard") || !ok.authenticated {
            fails.push(format!("mfa.ok: {} {:?}", ok.status, ok.header("location")));
        }
        let mut rj2 = pair[1].0.clone();
        rj2.update(&ok);
        let api = send(&Side::Django, "GET", "/api/ui_settings/", &rj2, None, &[]).await;
        // carol has no model permissions: 403 = logged in, 401 = anonymous
        if api.status == 401 {
            fails.push(format!("mfa.session_to_django: {}", api.status));
        }
        let dt = csrf_from(&wrong[0].body);
        let replay = send(&Side::Django, "POST", "/accounts/2fa/authenticate/", &pair[0].0, Some(&[("csrfmiddlewaretoken", &dt), ("code", &code)]), &[]).await;
        if replay.status != 200 || !replay.body.contains("Incorrect code.") {
            fails.push(format!("mfa.replay_blocked: Django accepted a code already used in Rust ({})", replay.status));
        }
        // redeem recovery code (first code of the seed) at Django intermediate state via Rust
        let codes = pl_auth_api::totp::generate_recovery_codes("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff0011223344556677");
        let rc = send(&Side::Rust(&app), "POST", "/accounts/2fa/authenticate/", &pair[0].0, Some(&[("csrfmiddlewaretoken", &csrf_from(&replay.body)), ("code", &codes[0])]), &[]).await;
        if rc.status != 302 {
            fails.push(format!("mfa.recovery_code_cross: {}", rc.status));
        }
        let (mask,): (serde_json::Value,) = sqlx::query_as("SELECT data->'used_mask' FROM mfa_authenticator WHERE user_id=5 AND type='recovery_codes'").fetch_one(&db).await.unwrap();
        if mask != serde_json::json!(1) {
            fails.push(format!("mfa.recovery_mask: {mask}"));
        }
        n += 6;
    }

    // --- Brute force (login_failed 5/300s/key, shared) ---------------------------------
    clear_rl().await;
    {
        let (mut rj, mut rt) = fresh_jar(&Side::Rust(&app)).await;
        for _ in 0..5 {
            let r = send(&Side::Rust(&app), "POST", "/accounts/login/", &rj, Some(&[("csrfmiddlewaretoken", &rt), ("login", "admin"), ("password", "nope")]), &[]).await;
            rj.update(&r);
            rt = csrf_from(&r.body);
        }
        // sixth attempt – even with the correct password – blocked at Django
        let (dj, dt) = fresh_jar(&Side::Django).await;
        let d = send(&Side::Django, "POST", "/accounts/login/", &dj, Some(&[("csrfmiddlewaretoken", &dt), ("login", "Admin"), ("password", "admin")]), &[]).await;
        if !d.body.contains("Too many failed login attempts") {
            fails.push(format!("bruteforce.shared_django: {}", d.status));
        }
        let r = send(&Side::Rust(&app), "POST", "/accounts/login/", &rj, Some(&[("csrfmiddlewaretoken", &rt), ("login", "admin"), ("password", "admin")]), &[]).await;
        if !r.body.contains(common::en("auth.error.too_many")) || r.status != 200 {
            fails.push(format!("bruteforce.rust: {}", r.status));
        }
        n += 2;
    }
    // login 30/m/ip → 429
    clear_rl().await;
    {
        let (rj, rt) = fresh_jar(&Side::Rust(&app)).await;
        // 30 hits in the last minute (as allauth: newest first)
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
        let hist: Vec<f64> = (0..30).map(|i| now - i as f64).collect();
        let client = redis::Client::open("redis://127.0.0.1:63799").unwrap();
        let mut c = client.get_multiplexed_async_connection().await.unwrap();
        let _: () = redis::cmd("SET").arg("w3ref:1:allauth:rl:allauth.account:login:ip:127.0.0.1").arg(pl_auth_api::django_cache::pickle_float_list(&hist)).arg("EX").arg(60).query_async(&mut c).await.unwrap();
        // prefill the Rust side separately (own cache; keys carry the time window since 2026-11-15)
        pl_accounts::cache::Cache::new(&state).set("allauth:rl:allauth.account:login:60s:ip:127.0.0.1", &pl_auth_api::django_cache::pickle_float_list(&hist), 60).await.unwrap();
        let last = send(&Side::Rust(&app), "POST", "/accounts/login/", &rj, Some(&[("csrfmiddlewaretoken", &rt), ("login", "x"), ("password", "x")]), &[]).await.status;
        let (dj, dt) = fresh_jar(&Side::Django).await;
        let d = send(&Side::Django, "POST", "/accounts/login/", &dj, Some(&[("csrfmiddlewaretoken", &dt), ("login", "y"), ("password", "x")]), &[]).await;
        if last != 429 || d.status != 429 {
            fails.push(format!("ratelimit.login_429: rust={last} django={}", d.status));
        }
        let r = send(&Side::Rust(&app), "POST", "/accounts/login/", &rj, Some(&[("csrfmiddlewaretoken", &rt), ("login", "y"), ("password", "x")]), &[]).await;
        check!(diff("ratelimit.429_body", &d, &r, &Jar::default()).into_iter().filter(|l| !l.contains("cookies")).collect());
        n += 1;
    }
    clear_rl().await;

    // --- Password reset ----------------------------------------------------------
    {
        for (case, email) in [("reset.unknown", "nobody@example.invalid"), ("reset.invalid", "not-an-email"), ("reset.empty", "")] {
            let (v, _, _) = same_post(&app, case, "/accounts/password/reset/", &[("email", email)], &[]).await;
            check!(v);
        }
        // Rust generates the link, Django accepts it (and vice versa)
        let mail_dir = std::path::PathBuf::from(std::env::var("W3_WORK").unwrap_or_else(|_| "../../target/w3-ref".into())).join("mail");
        let before = std::fs::read_dir(&mail_dir).map(|d| d.count()).unwrap_or(0);
        let (_, _, (r, _)) = same_post(&app, "reset.known", "/accounts/password/reset/", &[("email", "bob@example.invalid")], &[]).await;
        if r.status != 302 {
            fails.push(format!("reset.known rust status {}", r.status));
        }
        let mut files: Vec<_> = std::fs::read_dir(&mail_dir).map(|d| d.filter_map(Result::ok).map(|e| e.path()).collect()).unwrap_or_default();
        files.sort();
        if files.len() < before + 2 {
            fails.push(format!("reset.mails: expected 2 new mails, got {}", files.len().saturating_sub(before)));
        } else {
            let dmail = std::fs::read_to_string(&files[files.len() - 2]).unwrap_or_default();
            let rmail = std::fs::read_to_string(&files[files.len() - 1]).unwrap_or_default();
            // Own mail texts: header lines (without subject/date) equal, text from the own catalog
            let head = |m: &str| m.lines().take_while(|l| !l.is_empty()).filter(|l| !l.starts_with("Date:") && !l.starts_with("Subject:") && !l.starts_with(' ')).map(str::to_string).collect::<Vec<_>>();
            if head(&dmail) != head(&rmail) {
                fails.push(format!("reset.mail_headers differ\n--django--\n{dmail}\n--rust--\n{rmail}"));
            }
            if !rmail.contains(common::en("auth.mail.reset_subject")) || !rmail.contains(common::en("auth.mail.reset_text")) || !rmail.contains("bob") {
                fails.push(format!("reset.mail_body (own text)\n{rmail}"));
            }
            let link_re = regex::Regex::new(r"http://127\.0\.0\.1:8120(/accounts/password/reset/key/[^\s]+)").unwrap();
            let rlink = link_re.captures(&rmail).map(|c| c[1].to_string()).unwrap_or_default();
            let dlink = link_re.captures(&dmail).map(|c| c[1].to_string()).unwrap_or_default();
            let viaj = send(&Side::Django, "GET", &rlink, &Jar::default(), None, &[]).await;
            if viaj.status != 302 || !viaj.header("location").unwrap_or("").ends_with("-set-password/") {
                fails.push(format!("reset.rust_token_in_django: {} {:?} ({rlink})", viaj.status, viaj.header("location")));
            }
            let viar = send(&Side::Rust(&app), "GET", &dlink, &Jar::default(), None, &[]).await;
            check!(diff("reset.django_token_in_rust", &viaj, &viar, &Jar::default()).into_iter().filter(|l| !l.contains("cookies") && !l.contains("location")).collect());
            // form after redirect + new password (too short, then valid) in Rust
            let mut rj = Jar::default();
            rj.update(&viar);
            let loc = viar.header("location").unwrap_or("").to_string();
            let form = send(&Side::Rust(&app), "GET", &loc, &rj, None, &[]).await;
            rj.update(&form);
            let mut dj = Jar::default();
            dj.update(&viaj);
            let dform = send(&Side::Django, "GET", viaj.header("location").unwrap_or(""), &dj, None, &[]).await;
            dj.update(&dform);
            check!(diff("reset.form", &dform, &form, &rj).into_iter().filter(|l| !l.contains("cookies")).collect());
            let short = send(&Side::Rust(&app), "POST", &loc, &rj, Some(&[("csrfmiddlewaretoken", &csrf_from(&form.body)), ("password1", "abc"), ("password2", "abc")]), &[]).await;
            let dj_csrf = csrf_from(&dform.body);
            let dshort = send(&Side::Django, "POST", viaj.header("location").unwrap_or(""), &dj, Some(&[("csrfmiddlewaretoken", &dj_csrf), ("password1", "abc"), ("password2", "abc")]), &[]).await;
            check!(diff("reset.too_short", &dshort, &short, &rj).into_iter().filter(|l| !l.contains("cookies")).collect());
            let done = send(&Side::Rust(&app), "POST", &loc, &rj, Some(&[("csrfmiddlewaretoken", &csrf_from(&short.body)), ("password1", "bob-new-pass-2026"), ("password2", "bob-new-pass-2026")]), &[]).await;
            if done.status != 302 || done.header("location") != Some("/accounts/password/reset/key/done/") {
                fails.push(format!("reset.done: {} {:?}", done.status, done.header("location")));
            }
            // Django accepts the new password; the link is invalid afterwards
            let (dj2, dt2) = fresh_jar(&Side::Django).await;
            let login = send(&Side::Django, "POST", "/accounts/login/", &dj2, Some(&[("csrfmiddlewaretoken", &dt2), ("login", "bob"), ("password", "bob-new-pass-2026")]), &[]).await;
            if login.status != 302 {
                fails.push(format!("reset.django_login_new_pw: {}", login.status));
            }
            let reuse = send(&Side::Django, "GET", &rlink, &Jar::default(), None, &[]).await;
            if reuse.status != 200 {
                fails.push(format!("reset.link_reuse: {}", reuse.status));
            }
            // restore password (for further runs)
            sqlx::query("UPDATE auth_user SET password = $1 WHERE username = 'bob'").bind(&orig_bob).execute(&db).await.unwrap();
            n += 6;
        }
    }

    // --- Signup ---------------------------------------------------------------------------
    for (case, form) in [
        ("signup.empty", vec![("username", ""), ("password1", ""), ("password2", "")]),
        ("signup.taken", vec![("username", "ALICE"), ("password1", "x-very-long-pass-1"), ("password2", "x-very-long-pass-1")]),
        ("signup.mismatch", vec![("username", "newbie"), ("password1", "x-very-long-pass-1"), ("password2", "x-very-long-pass-2")]),
        ("signup.weak", vec![("username", "newbie"), ("password1", "12345678"), ("password2", "12345678")]),
        ("signup.badname", vec![("username", "bad name!"), ("password1", "x-very-long-pass-1"), ("password2", "x-very-long-pass-1")]),
    ] {
        let (v, _, _) = same_post(&app, case, "/accounts/signup/", &form, &[]).await;
        check!(v);
    }
    {
        let (v, (d, _), (r, rj)) = same_post(&app, "signup.ok", "/accounts/signup/", &[("username", "w3signup"), ("password1", "x-very-long-pass-1"), ("password2", "x-very-long-pass-1")], &[]).await;
        // Django creates w3signup, Rust then fails with "already exists" – check separately
        let _ = v;
        if d.status != 302 {
            fails.push(format!("signup.ok django {}", d.status));
        }
        if r.status != 200 || !r.body.contains(common::en("auth.error.username_taken")) {
            fails.push(format!("signup.ok rust (duplicate) {}", r.status));
        }
        let _ = rj;
        sqlx::query("DELETE FROM auth_user WHERE username = 'w3signup'").execute(&db).await.unwrap();
        clear_rl().await;
        let (mut rj, rt) = fresh_jar(&Side::Rust(&app)).await;
        let r = send(&Side::Rust(&app), "POST", "/accounts/signup/", &rj, Some(&[("csrfmiddlewaretoken", &rt), ("username", "w3signup"), ("password1", "x-very-long-pass-1"), ("password2", "x-very-long-pass-1")]), &[]).await;
        rj.update(&r);
        if r.status != 302 || r.header("location") != Some("/dashboard") {
            fails.push(format!("signup.rust_ok {} {:?}", r.status, r.header("location")));
        }
        let api = send(&Side::Django, "GET", "/api/ui_settings/", &rj, None, &[]).await;
        if api.status == 401 {
            fails.push(format!("signup.session_to_django {}", api.status));
        }
        sqlx::query("DELETE FROM django_session WHERE session_data IS NULL").execute(&db).await.ok();
        sqlx::query("DELETE FROM auth_user WHERE username = 'w3signup'").execute(&db).await.ok();
        n += 3;
    }

    eprintln!("pl-accounts parity: {n} checks, {} differences", fails.len());
    for f in &fails {
        eprintln!("DIFF {f}");
    }
    assert!(fails.is_empty(), "{} differences", fails.len());
}

/// Regression W21: login with password rehash (`must_update`, here salt < 128 bit as in the
/// reference snapshot `pbkdf2_sha256$1000000$paperlessrsref<user>$…`). Django computes the
/// `_auth_user_hash` in `login()` from the **new** hash; Rust did it from the old one, the next
/// request discarded the session (flush → 401 "Authentication credentials were not provided.").
#[tokio::test]
#[ignore = "needs W3 Django on 127.0.0.1:8120 (tests/parity/pl-accounts/w3-django.sh start)"]
async fn login_with_rehash_keeps_session() {
    let (app, state) = app();
    let db = state.db().clone();
    let (orig,): (String,) = sqlx::query_as("SELECT password FROM auth_user WHERE username = 'alice'").fetch_one(&db).await.unwrap();
    let weak = pl_authz::hashers::encode_pbkdf2_sha256("alice", "paperlessrsrefalice", 1_000_000);
    assert!(pl_authz::hashers::verify_password("alice", &weak).must_update);
    sqlx::query("UPDATE auth_user SET password = $1 WHERE username = 'alice'").bind(&weak).execute(&db).await.unwrap();
    clear_rl().await;
    let mut fails = Vec::new();
    let (mut rj, rt) = fresh_jar(&Side::Rust(&app)).await;
    let r = send(&Side::Rust(&app), "POST", "/accounts/login/", &rj, Some(&[("csrfmiddlewaretoken", &rt), ("login", "alice"), ("password", "alice")]), &[]).await;
    rj.update(&r);
    if r.status != 302 {
        fails.push(format!("login {}", r.status));
    }
    let (now,): (String,) = sqlx::query_as("SELECT password FROM auth_user WHERE username = 'alice'").fetch_one(&db).await.unwrap();
    if now == weak {
        fails.push("password not rehashed".into());
    }
    let again = send(&Side::Rust(&app), "GET", "/accounts/login/", &rj, None, &[]).await;
    if again.status != 302 || again.header("location") != Some("/dashboard") {
        fails.push(format!("rust reuse after rehash: {} {:?}", again.status, again.header("location")));
    }
    let api = send(&Side::Django, "GET", "/api/ui_settings/", &rj, None, &[]).await;
    if api.status != 200 || !api.body.contains("\"username\":\"alice\"") {
        fails.push(format!("django api after rust rehash-login: {}", api.status));
    }
    sqlx::query("UPDATE auth_user SET password = $1 WHERE username = 'alice'").bind(&orig).execute(&db).await.unwrap();
    assert!(fails.is_empty(), "{fails:?}");
}
