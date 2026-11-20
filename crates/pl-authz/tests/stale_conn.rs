//! W14: after a DB restart (here: `pg_terminate_backend` on the **own** pool connections)
//! the next authentication returns no error (`retry_stale_once` around user/token/
//! session lookup in `Authz::authenticate`).
//!
//! Runs against its own DB copy (never against `paperless`):
//!
//! ```sh
//! psql -h 127.0.0.1 -p 54329 -U postgres -c "CREATE DATABASE pl_w14_test TEMPLATE paperless_snapshot OWNER paperless"
//! PL_W14_TEST_DB=pl_w14_test cargo test -p pl-authz -j 1 --test stale_conn -- --ignored
//! ```

use std::time::Duration;

use axum::http::{HeaderMap, HeaderValue, Method, header};
use pl_authz::authn::{AuthMethod, Authz};
use pl_authz::config::AuthConfig;
use pl_core::Config;
use pl_core::config::Env;
use pl_core::state::{pg_connect_options, pool_options};
use sqlx::{Connection, PgPool};

const SECRET: &str = "paperless-rs-reference-secret-key-local-only-0123456789";
const ALICE_TOKEN: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002";

fn config() -> Config {
    let db = std::env::var("PL_W14_TEST_DB").unwrap_or_else(|_| "pl_w14_test".into());
    assert_ne!(db, "paperless", "never against the reference DB");
    Config::from_env_map(Env::from_pairs([
        ("PAPERLESS_SECRET_KEY", SECRET),
        ("PAPERLESS_DBHOST", "127.0.0.1"),
        ("PAPERLESS_DBPORT", "54329"),
        ("PAPERLESS_DBNAME", db.as_str()),
        ("PAPERLESS_DBUSER", "paperless"),
        ("PAPERLESS_DBPASS", "paperless"),
    ]))
    .unwrap()
}

async fn own_pids(pool: &PgPool, n: usize) -> Vec<i32> {
    let mut held = vec![];
    for _ in 0..n {
        held.push(pool.acquire().await.unwrap());
    }
    let mut out = vec![];
    for c in held.iter_mut() {
        out.push(sqlx::query_scalar::<_, i32>("SELECT pg_backend_pid()").fetch_one(&mut **c).await.unwrap());
    }
    out
}

async fn kill(cfg: &Config, pids: &[i32]) {
    let mut c = sqlx::PgConnection::connect_with(&pg_connect_options(&cfg.db)).await.unwrap();
    for p in pids {
        let ok: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)").bind(p).fetch_one(&mut c).await.unwrap();
        assert!(ok);
    }
    c.close().await.ok();
    tokio::time::sleep(Duration::from_millis(200)).await;
}

fn token_headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(header::AUTHORIZATION, HeaderValue::from_str(&format!("Token {ALICE_TOKEN}")).unwrap());
    h
}

#[tokio::test]
#[ignore = "braucht die DB-Kopie pl_w14_test (TEMPLATE paperless_snapshot) auf 127.0.0.1:54329"]
async fn authenticate_survives_terminated_connections() {
    let cfg = config();
    let pool = pool_options(&cfg).max_connections(2).connect_lazy_with(pg_connect_options(&cfg.db));
    let authz = Authz::new(pool.clone(), AuthConfig::for_tests(SECRET));

    for round in 0..3 {
        let ctx = authz.authenticate(&Method::GET, "/api/tags/", &token_headers()).await.unwrap();
        assert_eq!(ctx.user().map(|u| u.username.as_str()), Some("alice"), "round {round}");
        assert_eq!(ctx.method, Some(AuthMethod::Token));

        let pids = own_pids(&pool, 2).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        kill(&cfg, &pids).await;

        // the next request after the "restart": no error (DRF would answer 200)
        let ctx = authz.authenticate(&Method::GET, "/api/tags/", &token_headers()).await.unwrap();
        assert_eq!(ctx.user().map(|u| u.username.as_str()), Some("alice"), "round {round} after kill");

        // session lookup (unknown key → anonymous, but no 500)
        let pids = own_pids(&pool, 2).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        kill(&cfg, &pids).await;
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_static("sessionid=doesnotexist0123456789"));
        let ctx = authz.authenticate(&Method::GET, "/api/tags/", &h).await.unwrap();
        assert!(ctx.user().is_none());
    }
}
