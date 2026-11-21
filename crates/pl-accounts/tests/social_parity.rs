//! Social/OIDC parity against W12's own Django instance
//! (tests/parity/pl-accounts/w12-django.sh): Django 127.0.0.1:8140 on DB `pl_w12_ref`
//! (copy of `paperless_snapshot`), Redis prefix `w12`, fake OIDC IdP 127.0.0.1:8141,
//! SMTP sink 127.0.0.1:8142. The Rust router runs in-process on the same DB/the same cache.
//!
//! Each case runs once completely through Django and once through Rust (or mixed: start on
//! one side, callback/signup/MFA on the other). Compared are status, location
//! (state/code_challenge/scope order normalized), cookies (classes), HTML (CSRF
//! normalized), authenticated marker, created DB rows (auth_user, groups,
//! socialaccount_socialaccount, account_emailaddress, socialtoken) and sent mails.
//!
//! Start: `bash tests/parity/pl-accounts/w12-django.sh reset && bash tests/parity/pl-accounts/w12-django.sh start a`
//! Run:   `cargo test -p pl-accounts -j 1 --test social_parity -- --ignored --test-threads=1`
//! (Variants b, c: restart Django with `start b` or `start c`; each test checks the variant.)

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

const DJANGO: &str = "http://127.0.0.1:8140";
const HOST: &str = "127.0.0.1:8140";
const SECRET: &str = "paperless-rs-reference-secret-key-local-only-0123456789";
const TOTP_SECRET: &str = "JBSWY3DPEHPK3PXP";
// Working folder of the w12 reference run (repo target/w12-ref)
const WORK: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/w12-ref");

fn variant() -> String {
    std::fs::read_to_string(format!("{WORK}/variant")).unwrap_or_default().trim().to_string()
}

fn config(v: &str) -> Config {
    let providers = r#"{"openid_connect":{"OAUTH_PKCE_ENABLED":true,"APPS":[{"provider_id":"fakeidp","name":"Fake IdP","client_id":"w12client","secret":"w12secret","settings":{"server_url":"http://127.0.0.1:8141"}}]}}"#;
    let mut pairs: Vec<(&str, &str)> = vec![
        ("PAPERLESS_DBHOST", "127.0.0.1"),
        ("PAPERLESS_DBPORT", "54329"),
        ("PAPERLESS_DBNAME", "pl_w12_ref"),
        ("PAPERLESS_DBUSER", "paperless"),
        ("PAPERLESS_DBPASS", "paperless"),
        ("PAPERLESS_SECRET_KEY", SECRET),
        ("PAPERLESS_TIME_ZONE", "Europe/Zurich"),
        ("PAPERLESS_URL", DJANGO),
        ("PAPERLESS_ALLOWED_HOSTS", "127.0.0.1,localhost"),
        ("PAPERLESS_CSRF_TRUSTED_ORIGINS", "http://127.0.0.1:8140,http://localhost:8140"),
        ("PAPERLESS_DEBUG", "false"),
        ("PAPERLESS_EMAIL_HOST", "127.0.0.1"),
        ("PAPERLESS_EMAIL_PORT", "8142"),
        ("PAPERLESS_EMAIL_FROM", "paperless@example.invalid"),
        ("PAPERLESS_ACCOUNT_ALLOW_SIGNUPS", "true"),
        ("PAPERLESS_SOCIALACCOUNT_PROVIDERS", providers),
        ("PAPERLESS_APPS", "allauth.socialaccount.providers.openid_connect"),
        ("PAPERLESS_TRUSTED_PROXIES", ""),
    ];
    match v {
        "b" => pairs.extend([
            ("PAPERLESS_SOCIAL_AUTO_SIGNUP", "true"),
            ("PAPERLESS_SOCIAL_ACCOUNT_SYNC_GROUPS", "true"),
            ("PAPERLESS_SOCIAL_ACCOUNT_DEFAULT_GROUPS", "viewers"),
            ("PAPERLESS_ACCOUNT_DEFAULT_GROUPS", "auditors"),
            ("PAPERLESS_SOCIAL_ACCOUNT_SYNC_SUPERUSER_GROUP", "idp-admins"),
            ("PAPERLESS_REDIRECT_LOGIN_TO_SSO", "true"),
            ("PAPERLESS_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR", "pwd"),
        ]),
        "c" => pairs.extend([
            ("PAPERLESS_SOCIAL_AUTO_SIGNUP", "true"),
            ("PAPERLESS_SOCIALACCOUNT_ALLOW_SIGNUPS", "false"),
            ("PAPERLESS_DISABLE_REGULAR_LOGIN", "true"),
            ("PAPERLESS_ACCOUNT_SESSION_REMEMBER", "false"),
        ]),
        "d" => pairs.extend([("PAPERLESS_SOCIAL_AUTO_SIGNUP", "true"), ("PAPERLESS_ACCOUNT_EMAIL_VERIFICATION", "mandatory")]),
        _ => pairs.push(("PAPERLESS_SOCIAL_AUTO_SIGNUP", "false")),
    }
    Config::from_env_map(Env::from_pairs(pairs)).expect("w12 config")
}

fn app(v: &str) -> (Router, AppState) {
    let state = AppState::new(config(v)).expect("state");
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
    delegated: bool,
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

#[derive(Clone, Copy)]
enum Side<'a> {
    Django,
    Rust(&'a Router),
}

async fn send(side: Side<'_>, method: &str, path: &str, jar: &Jar, form: Option<&[(&str, &str)]>) -> Resp {
    let body = form.map(|f| url::form_urlencoded::Serializer::new(String::new()).extend_pairs(f.iter()).finish());
    let cookie = jar.header();
    match side {
        Side::Django => {
            let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
            let mut rb =
                client.request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), format!("{DJANGO}{path}")).header("Host", HOST);
            if !cookie.is_empty() {
                rb = rb.header("Cookie", cookie);
            }
            if let Some(b) = body {
                rb = rb.header("Content-Type", "application/x-www-form-urlencoded").body(b);
            }
            let r = rb.send().await.expect("django reachable");
            let status = r.status().as_u16();
            let authenticated = r.headers().contains_key("x-api-version");
            let headers = r.headers().iter().map(|(k, v)| (k.to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
            Resp { status, headers, body: r.text().await.unwrap_or_default(), authenticated, delegated: false }
        }
        Side::Rust(app) => {
            let mut rb = Request::builder().method(method).uri(path).header("host", HOST);
            if !cookie.is_empty() {
                rb = rb.header("cookie", cookie);
            }
            let mut req = match body {
                Some(b) => rb.header("content-type", "application/x-www-form-urlencoded").body(Body::from(b)).unwrap(),
                None => rb.body(Body::empty()).unwrap(),
            };
            req.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 50000))));
            let r = app.clone().oneshot(req).await.unwrap();
            let status = r.status().as_u16();
            let authenticated = r.extensions().get::<pl_core::Authenticated>().is_some();
            let delegated = pl_core::markers::is_delegation(&r);
            let headers = r.headers().iter().map(|(k, v)| (k.to_string(), String::from_utf8_lossy(v.as_bytes()).into_owned())).collect();
            let bytes = axum::body::to_bytes(r.into_body(), usize::MAX).await.unwrap();
            Resp { status, headers, body: String::from_utf8_lossy(&bytes).into_owned(), authenticated, delegated }
        }
    }
}

/// Own templates: the interface of the page is compared, not the wording.
fn norm_html(s: &str) -> String {
    let key = regex::Regex::new(r"/accounts/confirm-email/[-:\w]+/").unwrap();
    common::page_skeleton(&key.replace_all(s, "/accounts/confirm-email/KEY/"))
}

/// IdP redirect: replace `state`, `code_challenge`, sort `scope`.
fn norm_location(l: &str) -> String {
    let Some((base, q)) = l.split_once('?') else { return l.to_string() };
    let pairs: Vec<String> = url::form_urlencoded::parse(q.as_bytes())
        .map(|(k, v)| match k.as_ref() {
            "state" => format!("state=STATE{}", v.len()),
            "code_challenge" => format!("code_challenge=CH{}", v.len()),
            "scope" => {
                let mut s: Vec<&str> = v.split(' ').collect();
                s.sort();
                format!("scope={}", s.join(" "))
            }
            _ => format!("{k}={v}"),
        })
        .collect();
    format!("{base}?{}", pairs.join("&"))
}

fn norm_cookie(c: &str, jar: &Jar) -> String {
    let mut parts: Vec<String> = c.split("; ").map(str::to_string).collect();
    if let Some((k, v)) = parts[0].clone().split_once('=') {
        let class = if v == "\"\"" {
            "EMPTY".to_string()
        } else if jar.0.get(k).is_some_and(|old| old == v) {
            "SAME".to_string()
        } else if k == "messages" {
            let payload = v.split(':').next().unwrap_or("");
            format!("MSG({})", common::message_levels(&messages_text(payload)))
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

/// Content of a messages cookie (zlib output of Python and Rust differs).
fn messages_text(payload: &str) -> String {
    use base64::Engine as _;
    use std::io::Read as _;
    let (comp, b) = match payload.strip_prefix('.') {
        Some(r) => (true, r),
        None => (false, payload),
    };
    let Ok(raw) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(b) else { return payload.into() };
    if !comp {
        return String::from_utf8_lossy(&raw).into_owned();
    }
    let mut out = String::new();
    let _ = flate2::read::ZlibDecoder::new(&raw[..]).read_to_string(&mut out);
    out
}

/// Comparison of two responses (cookies per own jar).
fn diff(case: &str, d: &Resp, dj: &Jar, r: &Resp, rj: &Jar) -> Vec<String> {
    let mut out = Vec::new();
    if r.delegated {
        out.push(format!("{case}: rust delegated to django"));
        return out;
    }
    if d.status != r.status {
        out.push(format!("{case}: status django={} rust={}", d.status, r.status));
    }
    for h in ["location", "content-type", "cache-control", "allow"] {
        let (a, b) = (d.header(h).map(norm_location), r.header(h).map(norm_location));
        if a != b {
            out.push(format!("{case}: header {h}\n  django={a:?}\n  rust  ={b:?}"));
        }
    }
    if d.header("expires").is_some() != r.header("expires").is_some() {
        out.push(format!("{case}: expires presence differs"));
    }
    if d.authenticated != r.authenticated {
        out.push(format!("{case}: authenticated django={} rust={}", d.authenticated, r.authenticated));
    }
    let dc: Vec<String> = d.cookies().iter().map(|c| norm_cookie(c, dj)).collect();
    let rc: Vec<String> = r.cookies().iter().map(|c| norm_cookie(c, rj)).collect();
    if dc != rc {
        out.push(format!("{case}: cookies\n  django={dc:?}\n  rust  ={rc:?}"));
    }
    let (db, rb) = (norm_html(&d.body), norm_html(&r.body));
    if db != rb {
        let pos = db.bytes().zip(rb.bytes()).position(|(a, b)| a != b).unwrap_or(db.len().min(rb.len()));
        let ctx = |s: &str| s.get(pos.saturating_sub(100)..(pos + 120).min(s.len())).unwrap_or("").to_string();
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

fn totp_now() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    if now % 30 > 26 {
        std::thread::sleep(std::time::Duration::from_secs(4));
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    pl_auth_api::totp::format_hotp_value(pl_auth_api::totp::hotp_value(TOTP_SECRET, now / 30).unwrap())
}

/// Test state: remove all social accounts, new users (id > 6) and their rows,
/// clear limits/MFA locks, empty the mail folder.
async fn reset_state(db: &sqlx::PgPool) {
    for q in [
        "DELETE FROM socialaccount_socialtoken",
        "DELETE FROM socialaccount_socialaccount",
        "DELETE FROM account_emailaddress WHERE user_id > 6",
        "DELETE FROM auth_user_groups WHERE user_id > 6",
        "DELETE FROM mfa_authenticator WHERE user_id > 6",
        "DELETE FROM authtoken_token WHERE user_id > 6",
        "DELETE FROM auth_user WHERE id > 6",
    ] {
        sqlx::query(q).execute(db).await.unwrap();
    }
    redis_del("w12:1:allauth*").await;
    redis_del("w12:1:jwt:*").await;
    let _ = std::fs::remove_dir_all(format!("{WORK}/mail"));
    let _ = std::fs::create_dir_all(format!("{WORK}/mail"));
}

/// User row of the summary.
type UserRow = (i64, String, String, String, String, bool, bool, bool, bool, bool);

/// DB summary of the social users (without ids/timestamps/random values).
async fn db_summary(db: &sqlx::PgPool) -> String {
    let users: Vec<UserRow> = sqlx::query_as(
        "SELECT id::int8, username, first_name, last_name, email, is_staff, is_superuser, is_active, password LIKE '!%', last_login IS NOT NULL FROM auth_user WHERE id > 6 ORDER BY id",
    )
    .fetch_all(db)
    .await
    .unwrap();
    let mut out = String::new();
    for (id, u, f, l, e, st, su, ac, unusable, logged) in users {
        let groups: Vec<(String,)> = sqlx::query_as(
            "SELECT g.name FROM auth_user_groups ug JOIN auth_group g ON g.id = ug.group_id WHERE ug.user_id = $1 ORDER BY g.name",
        )
        .bind(id)
        .fetch_all(db)
        .await
        .unwrap();
        let mails: Vec<(String, bool, bool)> =
            sqlx::query_as("SELECT email, verified, \"primary\" FROM account_emailaddress WHERE user_id = $1 ORDER BY id")
                .bind(id)
                .fetch_all(db)
                .await
                .unwrap();
        out.push_str(&format!(
            "user {u}|{f}|{l}|{e} staff={st} su={su} active={ac} unusable={unusable} logged={logged} groups={:?} mails={mails:?}\n",
            groups.into_iter().map(|g| g.0).collect::<Vec<_>>()
        ));
    }
    let accs: Vec<(String, String, String, serde_json::Value)> = sqlx::query_as(
        "SELECT a.provider, a.uid, u.username, a.extra_data FROM socialaccount_socialaccount a JOIN auth_user u ON u.id = a.user_id ORDER BY a.uid",
    )
    .fetch_all(db)
    .await
    .unwrap();
    for (p, uid, u, mut extra) in accs {
        if let Some(t) = extra.get_mut("id_token").and_then(|v| v.as_object_mut()) {
            t.remove("iat");
            t.remove("exp");
        }
        out.push_str(&format!("account {p}|{uid}|{u}|{extra}\n"));
    }
    let (tokens,): (i64,) = sqlx::query_as("SELECT count(*) FROM socialaccount_socialtoken").fetch_one(db).await.unwrap();
    let base: Vec<(String, bool, bool)> =
        sqlx::query_as("SELECT u.username, u.is_staff, u.is_superuser FROM auth_user u WHERE u.id BETWEEN 2 AND 6 ORDER BY u.id")
            .fetch_all(db)
            .await
            .unwrap();
    out.push_str(&format!("tokens={tokens} base={base:?}\n"));
    out
}

/// Sent mails (Date/Message-ID/confirmation key normalized).
fn mails() -> Vec<String> {
    let mut files: Vec<_> =
        std::fs::read_dir(format!("{WORK}/mail")).map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).collect()).unwrap_or_default();
    files.sort();
    let re_key = regex::Regex::new(r"confirm-email/[^/\s]+/").unwrap();
    files
        .iter()
        .map(|p| {
            let s = std::fs::read_to_string(p).unwrap_or_default();
            let s: String = s
                .lines()
                .filter(|l| !l.starts_with("Date:") && !l.starts_with("Message-ID:") && !l.starts_with(" <"))
                .collect::<Vec<_>>()
                .join("\n");
            re_key.replace_all(&s, "confirm-email/KEY/").into_owned()
        })
        .collect()
}

/// Key of the last confirmation link in the mail sink.
fn last_confirm_key() -> Option<String> {
    let mut files: Vec<_> = std::fs::read_dir(format!("{WORK}/mail")).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    files.sort();
    let s = std::fs::read_to_string(files.last()?).ok()?;
    let re = regex::Regex::new(r"confirm-email/([^/\s]+)/").ok()?;
    re.captures(&s).map(|c| c[1].to_string())
}

/// Result of a flow.
#[derive(Debug, Clone)]
struct Run {
    steps: Vec<(String, Resp, Jar)>,
    jar: Jar,
}

impl Run {
    fn last(&self) -> &Resp {
        &self.steps.last().expect("step").1
    }
}

/// Start page (CSRF cookie), POST to `/accounts/oidc/fakeidp/login/…` (side `start`),
/// IdP, callback (side `cb`), then optionally follow redirects.
async fn oidc_run(start: Side<'_>, cb: Side<'_>, hint: &str, next: Option<&str>, mut jar: Jar) -> Run {
    let mut steps = Vec::new();
    let page = send(start, "GET", "/accounts/password/reset/", &jar, None).await;
    jar.update(&page);
    let tok = csrf_from(&page.body);
    let mut q = format!("/accounts/oidc/fakeidp/login/?process=&auth_params={}", oidc_q(&format!("login_hint={}", oidc_q(hint))));
    if let Some(n) = next {
        q.push_str(&format!("&next={}", oidc_q(n)));
    }
    let before = jar.clone();
    let r = send(start, "POST", &q, &jar, Some(&[("csrfmiddlewaretoken", &tok)])).await;
    jar.update(&r);
    steps.push(("start".into(), r.clone(), before));
    let Some(loc) = r.header("location").map(str::to_string).filter(|l| l.starts_with("http://127.0.0.1:8141/")) else {
        return Run { steps, jar };
    };
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let idp = client.get(&loc).send().await.expect("idp");
    let cbloc = idp.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let path = cbloc.split_once("://").map(|(_, r)| r).and_then(|r| r.split_once('/')).map(|(_, p)| format!("/{p}")).unwrap_or_default();
    let before = jar.clone();
    let r = send(cb, "GET", &path, &jar, None).await;
    jar.update(&r);
    steps.push(("callback".into(), r, before));
    Run { steps, jar }
}

fn oidc_q(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Follow a redirect within the account pages (GET).
async fn follow(side: Side<'_>, run: &mut Run, label: &str) {
    let Some(loc) = run.last().header("location").map(str::to_string) else { return };
    if !loc.starts_with("/accounts/") {
        return;
    }
    let before = run.jar.clone();
    let r = send(side, "GET", &loc, &run.jar, None).await;
    run.jar.update(&r);
    run.steps.push((label.into(), r, before));
}

fn compare(case: &str, d: &Run, r: &Run) -> Vec<String> {
    let mut out = Vec::new();
    if d.steps.len() != r.steps.len() {
        out.push(format!("{case}: steps django={} rust={}", d.steps.len(), r.steps.len()));
    }
    for ((name, dr, dj), (_, rr, rj)) in d.steps.iter().zip(r.steps.iter()) {
        out.extend(diff(&format!("{case}.{name}"), dr, dj, rr, rj));
    }
    out
}

/// Is the jar logged in at Django (`/api/ui_settings/`: 200/403 = logged in, 401 = anonymous)?
async fn django_user(jar: &Jar) -> Option<String> {
    let r = send(Side::Django, "GET", "/api/ui_settings/", jar, None).await;
    if r.status == 401 {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&r.body).unwrap_or_default();
    Some(v.pointer("/user/username").and_then(|u| u.as_str()).unwrap_or("?").to_string())
}

/// Case for both sides (same flow), incl. DB and mails.
macro_rules! both {
    ($fails:ident, $n:ident, $db:ident, $case:expr, |$side:ident| $body:expr) => {{
        reset_state(&$db).await;
        let d = {
            let $side = Side::Django;
            $body
        };
        let dsum = db_summary(&$db).await;
        let dmail = mails();
        reset_state(&$db).await;
        let r = {
            let $side = Side::Rust(APP.get().unwrap());
            $body
        };
        let rsum = db_summary(&$db).await;
        let rmail = mails();
        if std::env::var("W12_VERBOSE").is_ok() {
            eprintln!(
                "db {}:
{dsum}mails={}",
                $case,
                dmail.len()
            );
        }
        $n += 1;
        eprintln!(
            "case {}: {}",
            $case,
            d.steps
                .iter()
                .map(|(n, r, _)| format!("{n}={} {}", r.status, r.header("location").map(|l| &l[..l.len().min(60)]).unwrap_or("")))
                .collect::<Vec<_>>()
                .join(" | ")
        );
        let v = compare($case, &d, &r);
        if !v.is_empty() {
            eprintln!("FAIL {}", $case);
        }
        $fails.extend(v);
        if dsum != rsum {
            $fails.push(format!("{}: db\n  django={dsum}\n  rust  ={rsum}", $case));
        }
        if dmail != rmail {
            $fails.push(format!("{}: mails\n  django={dmail:?}\n  rust  ={rmail:?}", $case));
        }
        (d, r)
    }};
}

static APP: std::sync::OnceLock<Router> = std::sync::OnceLock::new();

async fn setup(v: &str) -> Option<(AppState, sqlx::PgPool)> {
    if variant() != v {
        eprintln!("Django läuft nicht in Variante {v} (gefunden: {:?}) – übersprungen", variant());
        return None;
    }
    let (router, state) = app(v);
    let _ = APP.set(router);
    let db = state.db().clone();
    Some((state, db))
}

fn report(fails: &[String], n: usize) {
    for f in fails {
        eprintln!("{f}\n");
    }
    eprintln!("{n} Fälle, {} Abweichungen", fails.len());
    assert!(fails.is_empty(), "{} Abweichungen", fails.len());
}

#[tokio::test]
#[ignore = "needs W12 Django (variant a) on 127.0.0.1:8140: tests/parity/pl-accounts/w12-django.sh start a"]
async fn social_parity_a() {
    let Some((_state, db)) = setup("a").await else { return };
    let mut fails: Vec<String> = Vec::new();
    let mut n = 0usize;

    // Pages without a flow
    for path in [
        "/accounts/oidc/fakeidp/login/",
        "/accounts/oidc/fakeidp/login/?process=connect&next=/documents/",
        "/accounts/oidc/nope/login/",
        "/accounts/oidc/nope/login/callback/?code=x&state=y",
        "/accounts/oidc/fakeidp/login/callback/",
        "/accounts/oidc/fakeidp/login/callback/?code=x&state=bad",
        "/accounts/oidc/fakeidp/login/callback/?error=access_denied&state=bad",
        "/accounts/3rdparty/login/cancelled/",
        "/accounts/3rdparty/login/error/",
        "/accounts/3rdparty/signup/",
        "/accounts/login/",
        "/accounts/signup/",
    ] {
        reset_state(&db).await;
        let mut jar = Jar::default();
        let p = send(Side::Django, "GET", "/accounts/password/reset/", &jar, None).await;
        jar.update(&p);
        let d = send(Side::Django, "GET", path, &jar, None).await;
        let r = send(Side::Rust(APP.get().unwrap()), "GET", path, &jar, None).await;
        n += 1;
        fails.extend(diff(&format!("get {path}"), &d, &jar, &r, &jar));
        // the same page in German (language cookie)
        jar.0.insert("django_language".into(), "de".into());
        let d = send(Side::Django, "GET", path, &jar, None).await;
        let r = send(Side::Rust(APP.get().unwrap()), "GET", path, &jar, None).await;
        n += 1;
        fails.extend(diff(&format!("get.de {path}"), &d, &jar, &r, &jar));
    }
    // Callback via POST (form_post) without CSRF token → 403 as Django; intermediate page German
    {
        let mut jar = Jar::default();
        let p = send(Side::Django, "GET", "/accounts/login/", &jar, None).await;
        jar.update(&p);
        let path = "/accounts/oidc/fakeidp/login/callback/";
        let d = send(Side::Django, "POST", path, &jar, Some(&[("code", "x"), ("state", "y")])).await;
        let r = send(Side::Rust(APP.get().unwrap()), "POST", path, &jar, Some(&[("code", "x"), ("state", "y")])).await;
        n += 1;
        fails.extend(diff("post.callback_no_csrf", &d, &jar, &r, &jar));
    }
    // POST without CSRF token → 403
    {
        let mut jar = Jar::default();
        let p = send(Side::Django, "GET", "/accounts/login/", &jar, None).await;
        jar.update(&p);
        let d = send(Side::Django, "POST", "/accounts/oidc/fakeidp/login/", &jar, Some(&[("x", "y")])).await;
        let r = send(Side::Rust(APP.get().unwrap()), "POST", "/accounts/oidc/fakeidp/login/", &jar, Some(&[("x", "y")])).await;
        n += 1;
        fails.extend(diff("post.no_csrf", &d, &jar, &r, &jar));
    }

    // New user → signup form → account
    let (d, r) = both!(fails, n, db, "signup.form", |side| {
        let mut run = oidc_run(side, side, "newbie", Some("/documents/"), Jar::default()).await;
        follow(side, &mut run, "signup_page").await;
        run
    });
    // Submit form (error, then success) – per side with its own intermediate state
    for (label, form) in [
        ("signup.empty", vec![("username", ""), ("email", "")]),
        ("signup.taken", vec![("username", "Alice"), ("email", "")]),
        ("signup.bad_email", vec![("username", "newbie"), ("email", "nope")]),
        ("signup.email_taken", vec![("username", "newbie"), ("email", "alice@example.invalid")]),
        ("signup.invalid_name", vec![("username", "new bie"), ("email", "")]),
    ] {
        let mut pair = Vec::new();
        for (side, run) in [(Side::Django, &d), (Side::Rust(APP.get().unwrap()), &r)] {
            let tok = csrf_from(&run.last().body);
            let mut f: Vec<(&str, &str)> = vec![("csrfmiddlewaretoken", &tok)];
            f.extend(form.iter().cloned());
            pair.push(send(side, "POST", "/accounts/3rdparty/signup/", &run.jar, Some(&f)).await);
        }
        n += 1;
        fails.extend(diff(label, &pair[0], &d.jar, &pair[1], &r.jar));
    }

    // Successful signup (without / with email), login afterwards, return visit
    for (case, email) in [("signup.ok_noemail", ""), ("signup.ok_email", "newbie@example.invalid")] {
        both!(fails, n, db, case, |side| {
            let mut run = oidc_run(side, side, "newbie", Some("/documents/"), Jar::default()).await;
            follow(side, &mut run, "signup_page").await;
            let tok = csrf_from(&run.last().body);
            let before = run.jar.clone();
            let p = send(
                side,
                "POST",
                "/accounts/3rdparty/signup/",
                &run.jar,
                Some(&[("csrfmiddlewaretoken", &tok), ("username", "newbie"), ("email", email)]),
            )
            .await;
            run.jar.update(&p);
            run.steps.push(("submit".into(), p, before));
            follow(side, &mut run, "after").await;
            // repeated login: existing account
            let again = oidc_run(side, side, "newbie", None, Jar::default()).await;
            run.steps.extend(again.steps);
            run
        });
    }

    // Switch mid-flow: start Rust → callback Django and vice versa; signup crossed
    for (case, s1, s2) in [("cross.rust_start", 1, 0), ("cross.django_start", 0, 1)] {
        reset_state(&db).await;
        let sides = [Side::Django, Side::Rust(APP.get().unwrap())];
        let mut run = oidc_run(sides[s1], sides[s2], "crossy", Some("/documents/"), Jar::default()).await;
        // signup form on the start side, submit on the other
        follow(sides[s1], &mut run, "signup_page").await;
        let tok = csrf_from(&run.last().body);
        let p = send(
            sides[s2],
            "POST",
            "/accounts/3rdparty/signup/",
            &run.jar,
            Some(&[("csrfmiddlewaretoken", &tok), ("username", "crossy"), ("email", "")]),
        )
        .await;
        run.jar.update(&p);
        n += 1;
        let cb = &run.steps[1].1;
        if cb.status != 302 || cb.header("location") != Some("/accounts/3rdparty/signup/") || cb.delegated {
            fails.push(format!("{case}: callback {} {:?}", cb.status, cb.header("location")));
        }
        if p.status != 302 || p.header("location") != Some("/documents/") || p.delegated {
            fails.push(format!("{case}: signup submit {} {:?} {}", p.status, p.header("location"), &p.body[..p.body.len().min(300)]));
        }
        let api = send(Side::Django, "GET", "/api/ui_settings/", &run.jar, None).await;
        if api.status == 401 {
            fails.push(format!(
                "{case}: session not accepted by django: {} {} jar={:?}",
                api.status,
                &api.body[..api.body.len().min(200)],
                run.jar
            ));
        }
        let back = send(sides[s1], "GET", "/accounts/login/", &run.jar, None).await;
        if back.status != 302 {
            fails.push(format!("{case}: start side does not see login ({})", back.status));
        }
    }

    // Existing account (alice), groups/permissions unchanged (no sync in variant a)
    let link = |db: sqlx::PgPool, user: i64, uid: &'static str| async move {
        sqlx::query("INSERT INTO socialaccount_socialaccount (provider, uid, last_login, date_joined, extra_data, user_id) VALUES ('fakeidp', $1, now(), now(), '{}', $2)")
            .bind(uid)
            .bind(user)
            .execute(&db)
            .await
            .unwrap();
    };
    both!(fails, n, db, "existing.alice", |side| {
        link(db.clone(), 3, "sub-alice").await;
        let mut run = oidc_run(side, side, "alice~g=viewers", Some("/documents/?a=1"), Jar::default()).await;
        follow(side, &mut run, "after").await;
        run
    });
    // inactive user (dave) → account_inactive
    both!(fails, n, db, "existing.inactive", |side| {
        link(db.clone(), 6, "sub-dave").await;
        let mut run = oidc_run(side, side, "dave", None, Jar::default()).await;
        follow(side, &mut run, "after").await;
        run
    });
    // already logged in (bob) → logged out, logged in as alice
    both!(fails, n, db, "existing.switch_user", |side| {
        link(db.clone(), 3, "sub-alice").await;
        let (mut jar, tok) = {
            let mut j = Jar::default();
            let p = send(side, "GET", "/accounts/login/", &j, None).await;
            j.update(&p);
            (j, csrf_from(&p.body))
        };
        let l = send(side, "POST", "/accounts/login/", &jar, Some(&[("csrfmiddlewaretoken", &tok), ("login", "bob"), ("password", "bob")]))
            .await;
        jar.update(&l);
        oidc_run(side, side, "alice", None, jar).await
    });

    // MFA: carol with TOTP; without amr → 2FA page, with amr=mfa → logged in directly
    let mfa_setup = |db: sqlx::PgPool| async move {
        sqlx::query("DELETE FROM mfa_authenticator WHERE user_id = 5").execute(&db).await.unwrap();
        sqlx::query(
            "INSERT INTO mfa_authenticator(user_id,type,data,created_at) VALUES (5,'totp','{\"secret\":\"JBSWY3DPEHPK3PXP\"}',now())",
        )
        .execute(&db)
        .await
        .unwrap();
        sqlx::query("INSERT INTO socialaccount_socialaccount (provider, uid, last_login, date_joined, extra_data, user_id) VALUES ('fakeidp', 'sub-carol', now(), now(), '{}', 5)").execute(&db).await.unwrap();
    };
    both!(fails, n, db, "mfa.required", |side| {
        mfa_setup(db.clone()).await;
        let mut run = oidc_run(side, side, "carol", None, Jar::default()).await;
        follow(side, &mut run, "mfa_page").await;
        run
    });
    // upstream fix cb3513c6f (after v3.4.2): default SOCIAL_ACCOUNT_MFA_TRUSTED_AMR = ["mfa"].
    // Django v3.4.2 (default: empty list) shows the 2FA page here; PaperRust logs in directly.
    // Deliberate deviation → check the Rust side only.
    {
        reset_state(&db).await;
        mfa_setup(db.clone()).await;
        let rust = Side::Rust(APP.get().unwrap());
        let run = oidc_run(rust, rust, "carol~mfa", None, Jar::default()).await;
        n += 1;
        let loc = run.last().header("location").unwrap_or("").to_string();
        if run.last().status != 302 || loc.contains("2fa") {
            fails.push(format!("mfa.amr_trusted (rust only, fix cb3513c6f): expected direct login, got {} {loc}", run.last().status));
        }
    }
    // MFA intermediate state from Rust, code at Django (and vice versa)
    for (case, s1, s2) in [("mfa.cross_rust_to_django", 1, 0), ("mfa.cross_django_to_rust", 0, 1)] {
        reset_state(&db).await;
        mfa_setup(db.clone()).await;
        let sides = [Side::Django, Side::Rust(APP.get().unwrap())];
        let mut run = oidc_run(sides[s1], sides[s1], "carol", None, Jar::default()).await;
        follow(sides[s2], &mut run, "mfa_page").await;
        let tok = csrf_from(&run.last().body);
        let code = totp_now();
        redis_del("w12:1:allauth.mfa*").await;
        let p =
            send(sides[s2], "POST", "/accounts/2fa/authenticate/", &run.jar, Some(&[("csrfmiddlewaretoken", &tok), ("code", &code)])).await;
        run.jar.update(&p);
        n += 1;
        if p.status != 302 || p.header("location") != Some("/dashboard") {
            fails.push(format!("{case}: {} {:?} {}", p.status, p.header("location"), &p.body[..p.body.len().min(200)]));
        }
        if django_user(&run.jar).await.is_none() {
            fails.push(format!("{case}: not logged in"));
        }
    }
    sqlx::query("DELETE FROM mfa_authenticator WHERE user_id = 5").execute(&db).await.unwrap();

    // Error cases at the IdP / token
    for (case, hint) in [
        ("err.badaud", "erik~badaud"),
        ("err.badiss", "erik~badiss"),
        ("err.expired", "erik~expired"),
        ("err.tokenfail", "erik~tokenfail"),
        ("err.nouserinfo", "erik~nouserinfo"),
        ("err.deny", "erik~deny"),
        ("err.server_error", "erik~err"),
        ("ok.badsig_accepted", "erik~badsig"),
        ("ok.listaud", "erik~listaud"),
        ("ok.noid", "erik~noid"),
        ("ok.noemail", "erik~noemail"),
        ("ok.verified", "erik~verified"),
    ] {
        both!(fails, n, db, case, |side| {
            let mut run = oidc_run(side, side, hint, None, Jar::default()).await;
            follow(side, &mut run, "after").await;
            run
        });
    }
    // Replay (jti): a second token with the same jti is rejected
    both!(fails, n, db, "err.jti_replay", |side| {
        let mut run = oidc_run(side, side, "jerry~jti", None, Jar::default()).await;
        let again = oidc_run(side, side, "jerry~jti", None, Jar::default()).await;
        run.steps.extend(again.steps);
        run
    });
    // Wrong / reused state
    both!(fails, n, db, "err.state_reuse", |side| {
        let run = oidc_run(side, side, "stan", None, Jar::default()).await;
        let cbpath = run.steps.get(1).map(|_| ()).is_some();
        let mut run2 = run.clone();
        if cbpath {
            // the same callback again (state consumed)
            let p = send(side, "GET", "/accounts/oidc/fakeidp/login/callback/?code=zzz&state=zzzzzzzzzzzzzzzz", &run.jar, None).await;
            run2.steps.push(("again".into(), p, run.jar.clone()));
        }
        run2
    });
    // Expired state (timestamp in the session set back by 700 s)
    both!(fails, n, db, "err.state_expired", |side| {
        let mut jar = Jar::default();
        let page = send(side, "GET", "/accounts/login/", &jar, None).await;
        jar.update(&page);
        let tok = csrf_from(&page.body);
        let r = send(
            side,
            "POST",
            "/accounts/oidc/fakeidp/login/?process=&auth_params=login_hint%3Dold",
            &jar,
            Some(&[("csrfmiddlewaretoken", &tok)]),
        )
        .await;
        jar.update(&r);
        let key = jar.0.get("sessionid").cloned().unwrap_or_default();
        age_session_states(&db, &key, 700.0).await;
        let loc = r.header("location").unwrap_or("").to_string();
        let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let idp = client.get(&loc).send().await.expect("idp");
        let cbloc = idp.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let path = format!("/{}", cbloc.splitn(4, '/').nth(3).unwrap_or(""));
        let before = jar.clone();
        let c = send(side, "GET", &path, &jar, None).await;
        Run { steps: vec![("start".into(), r, Jar::default()), ("callback".into(), c, before)], jar }
    });

    // process=connect (logged in: add account; anonymous: redirect)
    both!(fails, n, db, "connect.logged_in", |side| {
        let mut jar = Jar::default();
        let p = send(side, "GET", "/accounts/login/", &jar, None).await;
        jar.update(&p);
        let tok = csrf_from(&p.body);
        let l = send(side, "POST", "/accounts/login/", &jar, Some(&[("csrfmiddlewaretoken", &tok), ("login", "bob"), ("password", "bob")]))
            .await;
        jar.update(&l);
        let p = send(side, "GET", "/accounts/password/reset/", &jar, None).await;
        jar.update(&p);
        let tok = csrf_from(&p.body);
        let r = send(
            side,
            "POST",
            "/accounts/oidc/fakeidp/login/?process=connect&auth_params=login_hint%3Dbobby",
            &jar,
            Some(&[("csrfmiddlewaretoken", &tok)]),
        )
        .await;
        jar.update(&r);
        let loc = r.header("location").unwrap_or("").to_string();
        let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let idp = client.get(&loc).send().await.expect("idp");
        let cbloc = idp.headers().get("location").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let path = format!("/{}", cbloc.splitn(4, '/').nth(3).unwrap_or(""));
        let before = jar.clone();
        let c = send(side, "GET", &path, &jar, None).await;
        jar.update(&c);
        Run { steps: vec![("start".into(), r, Jar::default()), ("callback".into(), c, before)], jar }
    });

    // Registration with email (/accounts/signup/) + email confirmation (/accounts/confirm-email/)
    for (case, email, next) in [
        ("signup_email.ok", "quinn@example.invalid", ""),
        ("signup_email.taken", "alice@example.invalid", ""),
        ("signup_email.invalid", "nope@", ""),
        ("signup_email.ok_next", "quinn@example.invalid", "?next=/documents/"),
        ("signup_email.none", "", ""),
    ] {
        both!(fails, n, db, case, |side| {
            let mut jar = Jar::default();
            let p = send(side, "GET", "/accounts/signup/", &jar, None).await;
            jar.update(&p);
            let tok = csrf_from(&p.body);
            let before = jar.clone();
            let path = format!("/accounts/signup/{next}");
            let r = send(
                side,
                "POST",
                &path,
                &jar,
                Some(&[
                    ("csrfmiddlewaretoken", &tok),
                    ("username", "quinn"),
                    ("email", email),
                    ("password1", "Vexing-Quartz-77"),
                    ("password2", "Vexing-Quartz-77"),
                ]),
            )
            .await;
            jar.update(&r);
            let mut run = Run { steps: vec![("signup".into(), r, before)], jar };
            // Confirmation link from the mail: view anonymously, view logged in, confirm
            if let Some(key) = last_confirm_key() {
                let url = format!("/accounts/confirm-email/{key}/");
                let anon = send(side, "GET", &format!("{url}?next=/x/%3Cy%3E"), &Jar::default(), None).await;
                run.steps.push(("confirm_anon".into(), anon, Jar::default()));
                let before = run.jar.clone();
                let page = send(side, "GET", &url, &run.jar, None).await;
                run.jar.update(&page);
                let tok = csrf_from(&page.body);
                run.steps.push(("confirm_page".into(), page, before));
                let before = run.jar.clone();
                let post = send(side, "POST", &url, &run.jar, Some(&[("csrfmiddlewaretoken", &tok)])).await;
                run.jar.update(&post);
                run.steps.push(("confirm_post".into(), post, before));
                let before = run.jar.clone();
                let again = send(side, "GET", &url, &run.jar, None).await;
                run.steps.push(("confirm_again".into(), again, before));
                let before = run.jar.clone();
                let bad = send(side, "POST", &url, &run.jar, Some(&[("csrfmiddlewaretoken", &tok)])).await;
                run.steps.push(("confirm_post_again".into(), bad, before));
            }
            run
        });
    }
    // Confirmation by another logged-in user (logout_other_user) and conflict
    both!(fails, n, db, "confirm.other_user_and_conflict", |side| {
        let mut jar = Jar::default();
        let p = send(side, "GET", "/accounts/signup/", &jar, None).await;
        jar.update(&p);
        let tok = csrf_from(&p.body);
        let r = send(
            side,
            "POST",
            "/accounts/signup/",
            &jar,
            Some(&[
                ("csrfmiddlewaretoken", &tok),
                ("username", "rita"),
                ("email", "rita@example.invalid"),
                ("password1", "Vexing-Quartz-77"),
                ("password2", "Vexing-Quartz-77"),
            ]),
        )
        .await;
        let key = last_confirm_key().unwrap_or_default();
        let url = format!("/accounts/confirm-email/{key}/");
        // bob logged in
        let mut bj = Jar::default();
        let p = send(side, "GET", "/accounts/login/", &bj, None).await;
        bj.update(&p);
        let t = csrf_from(&p.body);
        let l =
            send(side, "POST", "/accounts/login/", &bj, Some(&[("csrfmiddlewaretoken", &t), ("login", "bob"), ("password", "bob")])).await;
        bj.update(&l);
        let before = bj.clone();
        let page = send(side, "GET", &url, &bj, None).await;
        bj.update(&page);
        let mut run = Run { steps: vec![("signup".into(), r, Jar::default()), ("other_user_page".into(), page, before)], jar: bj };
        // Conflict: the same address confirmed at alice
        sqlx::query(
            "INSERT INTO account_emailaddress (email, verified, \"primary\", user_id) VALUES ('rita@example.invalid', true, false, 3)",
        )
        .execute(&db)
        .await
        .unwrap();
        let before = run.jar.clone();
        let c = send(side, "GET", &url, &run.jar, None).await;
        run.jar.update(&c);
        let tok = csrf_from(&send(side, "GET", "/accounts/password/reset/", &run.jar, None).await.body);
        run.steps.push(("conflict_page".into(), c, before));
        let before = run.jar.clone();
        let post = send(side, "POST", &url, &run.jar, Some(&[("csrfmiddlewaretoken", &tok)])).await;
        run.jar.update(&post);
        run.steps.push(("conflict_post".into(), post, before));
        follow(side, &mut run, "after").await;
        sqlx::query("DELETE FROM account_emailaddress WHERE user_id = 3 AND email = 'rita@example.invalid'").execute(&db).await.unwrap();
        run
    });
    for path in ["/accounts/confirm-email/abc/", "/accounts/confirm-email/NDQ:1xQ0mE:iM0t1DmRCDKGs0OcgOJ-7Pv__dyUZGona4Y3KhyxxWU/"] {
        let d = send(Side::Django, "GET", path, &Jar::default(), None).await;
        let r = send(Side::Rust(APP.get().unwrap()), "GET", path, &Jar::default(), None).await;
        n += 1;
        fails.extend(diff(&format!("get {path}"), &d, &Jar::default(), &r, &Jar::default()));
    }

    // disconnect_social_account (profile API, Django) → next login is a signup again
    {
        reset_state(&db).await;
        link(db.clone(), 3, "sub-alice").await;
        let (aid,): (i64,) =
            sqlx::query_as("SELECT id::int8 FROM socialaccount_socialaccount WHERE uid='sub-alice'").fetch_one(&db).await.unwrap();
        let client = reqwest::Client::new();
        let r = client
            .post(format!("{DJANGO}/api/profile/disconnect_social_account/"))
            .header("Authorization", "Token a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002")
            .json(&serde_json::json!({"id": aid}))
            .send()
            .await
            .unwrap();
        let st = r.status().as_u16();
        let run = oidc_run(Side::Rust(APP.get().unwrap()), Side::Rust(APP.get().unwrap()), "alice", None, Jar::default()).await;
        n += 1;
        if st != 200 || run.last().header("location") != Some("/accounts/3rdparty/signup/") {
            fails.push(format!("disconnect: api={st} callback={:?}", run.last().header("location")));
        }
    }
    reset_state(&db).await;
    report(&fails, n);
}

/// Set back the timestamps of all `socialaccount_states` of a session by `secs` (DB + cache).
async fn age_session_states(db: &sqlx::PgPool, key: &str, secs: f64) {
    let (data,): (String,) =
        sqlx::query_as("SELECT session_data FROM django_session WHERE session_key = $1").bind(key).fetch_one(db).await.unwrap();
    let mut m = pl_authz::signing::decode_session(&data, SECRET);
    if let Some(serde_json::Value::Object(states)) = m.get_mut("socialaccount_states") {
        for (_, v) in states.iter_mut() {
            if let Some(ts) = v.get(1).and_then(|t| t.as_f64()) {
                v[1] = serde_json::json!(ts - secs);
            }
        }
    }
    let json = pl_authz::signing::python_json_dumps(&serde_json::Value::Object(m));
    let enc = pl_authz::signing::encode_session(&json, SECRET, chrono::Utc::now().timestamp());
    sqlx::query("UPDATE django_session SET session_data = $1 WHERE session_key = $2").bind(enc).bind(key).execute(db).await.unwrap();
    redis_del(&format!("w12:1:django.contrib.sessions.cached_db{key}")).await;
}

#[tokio::test]
#[ignore = "needs W12 Django (variant b) on 127.0.0.1:8140: tests/parity/pl-accounts/w12-django.sh start b"]
async fn social_parity_b() {
    let Some((_state, db)) = setup("b").await else { return };
    let mut fails: Vec<String> = Vec::new();
    let mut n = 0usize;
    // Login page with SSO redirect
    for path in ["/accounts/login/", "/accounts/login/?loggedout=1", "/accounts/oidc/fakeidp/login/"] {
        let mut jar = Jar::default();
        let p = send(Side::Django, "GET", "/accounts/password/reset/", &jar, None).await;
        jar.update(&p);
        let d = send(Side::Django, "GET", path, &jar, None).await;
        let r = send(Side::Rust(APP.get().unwrap()), "GET", path, &jar, None).await;
        n += 1;
        fails.extend(diff(&format!("get {path}"), &d, &jar, &r, &jar));
    }
    // Auto-signup with group sync (default groups are replaced by the sync), superuser group
    for (case, hint) in [
        ("auto.groups", "gina~g=editors~g=idp-admins"),
        ("auto.no_groups", "hank"),
        ("auto.verified", "ivy~verified~g=editors"),
        ("auto.email_taken", "alice2~g=editors"),
        ("auto.bad_username", "bad name~g=editors"),
        ("auto.generated_username", "x y~noemail"),
    ] {
        both!(fails, n, db, case, |side| {
            if case == "auto.email_taken" {
                sqlx::query("UPDATE auth_user SET email = 'alice2@example.invalid' WHERE id = 3").execute(&db).await.unwrap();
            }
            let mut run = oidc_run(side, side, hint, Some("/documents/"), Jar::default()).await;
            follow(side, &mut run, "after").await;
            if case == "auto.email_taken" {
                sqlx::query("UPDATE auth_user SET email = 'alice@example.invalid' WHERE id = 3").execute(&db).await.unwrap();
            }
            run
        });
    }
    // MFA with trusted amr (PAPERLESS_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR=pwd; v3.4.2 split on "mfa",
    // since upstream fix cb3513c6f on "," – identical for "pwd")
    for (case, hint) in [("mfa.amr_trusted", "carol~mfa"), ("mfa.amr_missing", "carol")] {
        both!(fails, n, db, case, |side| {
            sqlx::query("DELETE FROM mfa_authenticator WHERE user_id = 5").execute(&db).await.unwrap();
            sqlx::query(
                "INSERT INTO mfa_authenticator(user_id,type,data,created_at) VALUES (5,'totp','{\"secret\":\"JBSWY3DPEHPK3PXP\"}',now())",
            )
            .execute(&db)
            .await
            .unwrap();
            sqlx::query("INSERT INTO socialaccount_socialaccount (provider, uid, last_login, date_joined, extra_data, user_id) VALUES ('fakeidp', 'sub-carol', now(), now(), '{}', 5)").execute(&db).await.unwrap();
            oidc_run(side, side, hint, None, Jar::default()).await
        });
    }
    sqlx::query("DELETE FROM mfa_authenticator WHERE user_id = 5").execute(&db).await.unwrap();
    // existing account: group sync + superuser revocation
    both!(fails, n, db, "existing.sync", |side| {
        sqlx::query("INSERT INTO socialaccount_socialaccount (provider, uid, last_login, date_joined, extra_data, user_id) VALUES ('fakeidp', 'sub-alice', now(), now(), '{}', 3)").execute(&db).await.unwrap();
        let run = oidc_run(side, side, "alice~g=viewers~g=idp-admins", None, Jar::default()).await;
        let after = db_summary(&db).await;
        sqlx::query("UPDATE auth_user SET is_superuser = false, is_staff = false WHERE id = 3").execute(&db).await.unwrap();
        sqlx::query("DELETE FROM auth_user_groups WHERE user_id = 3").execute(&db).await.unwrap();
        sqlx::query("INSERT INTO auth_user_groups (user_id, group_id) VALUES (3, 1)").execute(&db).await.unwrap();
        let mut run = run;
        run.steps.push((
            "summary".into(),
            Resp { status: 0, headers: vec![], body: after, authenticated: false, delegated: false },
            Jar::default(),
        ));
        run
    });
    reset_state(&db).await;
    report(&fails, n);
}

#[tokio::test]
#[ignore = "needs W12 Django (variant c) on 127.0.0.1:8140: tests/parity/pl-accounts/w12-django.sh start c"]
async fn social_parity_c() {
    let Some((_state, db)) = setup("c").await else { return };
    let mut fails: Vec<String> = Vec::new();
    let mut n = 0usize;
    // DISABLE_REGULAR_LOGIN: login page without form
    for path in ["/accounts/login/", "/accounts/oidc/fakeidp/login/"] {
        let mut jar = Jar::default();
        let p = send(Side::Django, "GET", "/accounts/password/reset/", &jar, None).await;
        jar.update(&p);
        let d = send(Side::Django, "GET", path, &jar, None).await;
        let r = send(Side::Rust(APP.get().unwrap()), "GET", path, &jar, None).await;
        n += 1;
        fails.extend(diff(&format!("get {path}"), &d, &jar, &r, &jar));
    }
    // SOCIALACCOUNT_ALLOW_SIGNUPS=false → signup_closed (allauth layout)
    both!(fails, n, db, "closed.new_user", |side| oidc_run(side, side, "nina", None, Jar::default()).await);
    // existing user, session until browser close (ACCOUNT_SESSION_REMEMBER=false)
    both!(fails, n, db, "remember_false.existing", |side| {
        sqlx::query("INSERT INTO socialaccount_socialaccount (provider, uid, last_login, date_joined, extra_data, user_id) VALUES ('fakeidp', 'sub-bob', now(), now(), '{}', 4)").execute(&db).await.unwrap();
        oidc_run(side, side, "bob", None, Jar::default()).await
    });
    reset_state(&db).await;
    report(&fails, n);
}

#[tokio::test]
#[ignore = "needs W12 Django (variant d) on 127.0.0.1:8140: tests/parity/pl-accounts/w12-django.sh start d"]
async fn social_parity_d() {
    let Some((_state, db)) = setup("d").await else { return };
    let mut fails: Vec<String> = Vec::new();
    let mut n = 0usize;
    // mandatory (gap S1): callback now in Rust (EmailVerificationStage), result as Django
    reset_state(&db).await;
    let run = oidc_run(Side::Rust(APP.get().unwrap()), Side::Rust(APP.get().unwrap()), "mona", None, Jar::default()).await;
    n += 1;
    if run.steps.get(1).is_none_or(|s| s.1.delegated) {
        fails.push("mandatory: callback delegated or missing".into());
    }
    reset_state(&db).await;
    let d = oidc_run(Side::Django, Side::Django, "mona", None, Jar::default()).await;
    n += 1;
    eprintln!(
        "mandatory: rust {} {:?} / django {} {:?}",
        run.last().status,
        run.last().header("location"),
        d.last().status,
        d.last().header("location")
    );
    if (run.last().status, run.last().header("location")) != (d.last().status, d.last().header("location")) {
        fails.push("mandatory: callback result differs from django".into());
    }
    reset_state(&db).await;
    report(&fails, n);
}
