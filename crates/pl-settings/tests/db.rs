//! DB tests (WP2 storage, WP3 live reload) against a **dedicated** copy (never `paperless`):
//!
//! ```text
//! psql -h 127.0.0.1 -p 54329 -U paperless -d postgres -c "CREATE DATABASE pl_settings_test TEMPLATE paperless_snapshot"
//! PAPERRUST_SETTINGS_TEST_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/pl_settings_test \
//!   cargo test -p pl-settings -j 2 -- --ignored --test-threads 1
//! ```
//! The tests only change `paperrust_setting*` and the AC row of the copy.

use std::time::{Duration, Instant};

use pl_core::config::Env;
use pl_settings::SettingsHandle;
use pl_settings::crypto::SecretBox;
use pl_settings::layer::{DbValue, Source};
use pl_settings::store::{self, Action, Actor, ApplyOptions, Input};
use serde_json::json;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tokio::sync::Mutex;

static LOCK: Mutex<()> = Mutex::const_new(());

fn url() -> String {
    let url = std::env::var("PAPERRUST_SETTINGS_TEST_DATABASE_URL").expect("PAPERRUST_SETTINGS_TEST_DATABASE_URL");
    let db = url.trim_end_matches('/').rsplit('/').next().unwrap_or("").split('?').next().unwrap_or("").to_string();
    assert!(db.starts_with("pl_settings_test"), "refusing to run against a non-test database ({db})");
    url
}

async fn pool() -> PgPool {
    let pool = PgPoolOptions::new().max_connections(4).connect(&url()).await.expect("connect");
    let mut c = pool.acquire().await.unwrap();
    store::migrate(&mut c).await.unwrap();
    pool
}

async fn clean(pool: &PgPool) {
    sqlx::raw_sql("DELETE FROM paperrust_setting; DELETE FROM paperrust_setting_history;").execute(pool).await.unwrap();
}

fn actor() -> Actor {
    Actor { user_id: Some(1), user_name: Some("admin".into()), remote_addr: "127.0.0.1".parse().ok() }
}

fn opts<'a>() -> ApplyOptions<'a> {
    ApplyOptions::default()
}

#[tokio::test]
#[ignore]
async fn migration_idempotent() {
    let _g = LOCK.lock().await;
    let pool = pool().await;
    let mut c = pool.acquire().await.unwrap();
    store::migrate(&mut c).await.unwrap();
    store::migrate(&mut c).await.unwrap();
    assert!(store::installed(&mut c).await.unwrap());
    // no Django table changed: AC table still has exactly its columns
    let cols: i64 =
        sqlx::query_scalar("SELECT count(*) FROM information_schema.columns WHERE table_name = 'paperless_applicationconfiguration'")
            .fetch_one(&mut *c)
            .await
            .unwrap();
    assert_eq!(cols, 46);
}

#[tokio::test]
#[ignore]
async fn secrets_encrypted_history_masked_wrong_key_unreadable() {
    let _g = LOCK.lock().await;
    let pool = pool().await;
    clean(&pool).await;
    let sb = SecretBox::from_secret_key("test-secret-key-A");
    let rev = store::apply(
        &pool,
        &sb,
        &[
            ("PAPERLESS_EMAIL_HOST_PASSWORD".into(), Input::Json(json!("hunter2-plain"))),
            ("PAPERLESS_EMAIL_HOST".into(), Input::Json(json!("smtp.example.ch"))),
        ],
        &opts(),
        &actor(),
        Action::Set,
    )
    .await
    .unwrap();
    assert!(rev > 0);
    // Plaintext appears nowhere in the DB
    let leaks: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM paperrust_setting WHERE coalesce(value,'') LIKE '%hunter2%' OR position('hunter2'::bytea in coalesce(secret_ct,''::bytea)) > 0)
              + (SELECT count(*) FROM paperrust_setting_history WHERE coalesce(old_value,'') || coalesce(new_value,'') LIKE '%hunter2%')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(leaks, 0);
    let masked: String = sqlx::query_scalar("SELECT new_value FROM paperrust_setting_history WHERE key = 'PAPERLESS_EMAIL_HOST_PASSWORD'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(masked, "***");
    let mut c = pool.acquire().await.unwrap();
    let db = store::load_db(&mut c, &sb).await.unwrap();
    assert!(db["PAPERLESS_EMAIL_HOST_PASSWORD"].value == DbValue::Plain("hunter2-plain".into()));
    // different SECRET_KEY → unreadable, counts as not set
    let other = SecretBox::from_secret_key("test-secret-key-B");
    let db = store::load_db(&mut c, &other).await.unwrap();
    assert!(db["PAPERLESS_EMAIL_HOST_PASSWORD"].value == DbValue::Unreadable);
    let s = store::load_settings(&pool, Env::from_pairs([("X", "1")]), &other, None).await.unwrap();
    let e = s.get("PAPERLESS_EMAIL_HOST_PASSWORD").unwrap();
    assert!(e.unreadable);
    assert_eq!(e.source, Source::Default);
    // rekey makes it readable again
    let n = store::rekey(&pool, &sb, &other, &actor()).await.unwrap();
    assert_eq!(n, 1);
    let db = store::load_db(&mut c, &other).await.unwrap();
    assert!(db["PAPERLESS_EMAIL_HOST_PASSWORD"].value == DbValue::Plain("hunter2-plain".into()));
    // Export without secrets
    let ex = store::export(&mut c, &other, false).await.unwrap();
    assert!(!ex.to_string().contains("hunter2"));
    assert_eq!(ex["settings"]["PAPERLESS_EMAIL_HOST"], json!("smtp.example.ch"));
}

#[tokio::test]
#[ignore]
async fn rev_conflict_lock_and_reset() {
    let _g = LOCK.lock().await;
    let pool = pool().await;
    clean(&pool).await;
    let sb = SecretBox::from_secret_key("k");
    let r1 = store::apply(&pool, &sb, &[("PAPERLESS_EMAIL_PORT".into(), Input::Json(json!(587)))], &opts(), &actor(), Action::Set)
        .await
        .unwrap();
    // outdated state → 409
    let o = ApplyOptions { expected_rev: Some(r1 - 1), ..opts() };
    let e =
        store::apply(&pool, &sb, &[("PAPERLESS_EMAIL_PORT".into(), Input::Json(json!(25)))], &o, &actor(), Action::Set).await.unwrap_err();
    assert_eq!(e.code(), "rev_conflict");
    // Environment locks
    let env = Env::from_pairs([("PAPERLESS_EMAIL_PORT", "2525")]);
    let o = ApplyOptions { expected_rev: Some(r1), env: Some(&env), ..opts() };
    let e =
        store::apply(&pool, &sb, &[("PAPERLESS_EMAIL_PORT".into(), Input::Json(json!(25)))], &o, &actor(), Action::Set).await.unwrap_err();
    assert_eq!(e.code(), "locked_by_environment");
    // --if-unset leaves existing values alone
    let o = ApplyOptions { if_unset: true, ..opts() };
    store::apply(&pool, &sb, &[("PAPERLESS_EMAIL_PORT".into(), Input::Raw("1".into()))], &o, &actor(), Action::Cli).await.unwrap();
    let s = store::load_settings(&pool, Env::default(), &sb, None).await.unwrap();
    assert_eq!(s.value("PAPERLESS_EMAIL_PORT"), Some("587"));
    // Reset deletes the row
    let r2 = store::apply(&pool, &sb, &[("PAPERLESS_EMAIL_PORT".into(), Input::Reset)], &opts(), &actor(), Action::Set).await.unwrap();
    assert!(r2 > r1);
    let s = store::load_settings(&pool, Env::default(), &sb, None).await.unwrap();
    assert_eq!((s.value("PAPERLESS_EMAIL_PORT"), s.get("PAPERLESS_EMAIL_PORT").unwrap().source), (Some("25"), Source::Default));
    assert_eq!(s.rev, r2);
    let mut c = pool.acquire().await.unwrap();
    let h = store::history_list(&mut c, Some("PAPERLESS_EMAIL_PORT"), None, 10).await.unwrap();
    assert_eq!(h.iter().map(|r| r.action.as_str()).collect::<Vec<_>>(), ["reset", "set"]);
}

/// AC fields: the same Django row, no copy in `paperrust_setting` (E3, §2.8).
#[tokio::test]
#[ignore]
async fn appconfig_same_row() {
    let _g = LOCK.lock().await;
    let pool = pool().await;
    clean(&pool).await;
    let before: Option<serde_json::Value> =
        sqlx::query_scalar("SELECT to_jsonb(c) FROM paperless_applicationconfiguration c ORDER BY id LIMIT 1")
            .fetch_optional(&pool)
            .await
            .unwrap();
    let sb = SecretBox::from_secret_key("k");
    store::apply(
        &pool,
        &sb,
        &[("PAPERLESS_OCR_LANGUAGE".into(), Input::Json(json!("deu+fra"))), ("PAPERLESS_OCR_PAGES".into(), Input::Json(json!(3)))],
        &opts(),
        &actor(),
        Action::Set,
    )
    .await
    .unwrap();
    let (lang, pages): (Option<String>, Option<i16>) =
        sqlx::query_as("SELECT language, pages FROM paperless_applicationconfiguration ORDER BY id LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((lang.as_deref(), pages), (Some("deu+fra"), Some(3)));
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM paperrust_setting").fetch_one(&pool).await.unwrap();
    assert_eq!(n, 0);
    let hk: Vec<String> = sqlx::query_scalar("SELECT key FROM paperrust_setting_history ORDER BY key").fetch_all(&pool).await.unwrap();
    assert_eq!(hk, ["AC:language", "AC:pages"]);
    let s = store::load_settings(&pool, Env::from_pairs([("PAPERLESS_OCR_LANGUAGE", "eng")]), &sb, None).await.unwrap();
    let e = s.get("PAPERLESS_OCR_LANGUAGE").unwrap();
    assert_eq!((e.value.as_deref(), e.source, e.locked), (Some("deu+fra"), Source::AppConfig, false));
    // Reset = NULL; afterwards the environment applies
    store::apply(&pool, &sb, &[("PAPERLESS_OCR_LANGUAGE".into(), Input::Reset)], &opts(), &actor(), Action::Set).await.unwrap();
    let s = store::load_settings(&pool, Env::from_pairs([("PAPERLESS_OCR_LANGUAGE", "eng")]), &sb, None).await.unwrap();
    assert_eq!(s.value("PAPERLESS_OCR_LANGUAGE"), Some("eng"));
    // Restore copy
    if let Some(serde_json::Value::Object(m)) = before {
        let lang = m.get("language").and_then(|v| v.as_str()).map(String::from);
        let pages = m.get("pages").and_then(|v| v.as_i64()).map(|v| v as i16);
        sqlx::query("UPDATE paperless_applicationconfiguration SET language = $1, pages = $2")
            .bind(lang)
            .bind(pages)
            .execute(&pool)
            .await
            .unwrap();
    }
}

/// WP3: two independent holders (own pools/LISTEN connections, like two processes) – a
/// change via A is visible in B in < 2 s; S key sets `restart_pending`.
#[tokio::test]
#[ignore]
async fn live_reload_two_processes() {
    let _g = LOCK.lock().await;
    let pa = pool().await;
    let pb = pool().await;
    clean(&pa).await;
    let sb = SecretBox::from_secret_key("k");
    let env = Env::from_pairs([("X", "1")]);
    let a = SettingsHandle::load_initial(&pa, env.clone(), sb.clone()).await.unwrap();
    let b = SettingsHandle::load_initial(&pb, env.clone(), sb.clone()).await.unwrap();
    let (stop_tx, stop) = tokio::sync::watch::channel(false);
    let ja = a.spawn_listener(pa.clone(), stop.clone());
    // B with a long fallback: only NOTIFY can deliver the change in time
    let jb = b.spawn_listener_with(pb.clone(), stop.clone(), Duration::from_secs(3600));
    let mut sub = b.subscribe();
    tokio::time::sleep(Duration::from_millis(300)).await; // LISTEN is in place
    let t0 = Instant::now();
    let rev = store::apply(
        &pa,
        &sb,
        &[
            ("PAPERLESS_EMAIL_HOST".into(), Input::Raw("live.example".into())),
            ("PAPERLESS_TIME_ZONE".into(), Input::Raw("Europe/Zurich".into())),
        ],
        &opts(),
        &actor(),
        Action::Set,
    )
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while sub.borrow_and_update().rev != rev {
            sub.changed().await.unwrap();
        }
    })
    .await
    .expect("change not visible in B within 2 s");
    let took = t0.elapsed();
    let s = b.load();
    assert_eq!(s.value("PAPERLESS_EMAIL_HOST"), Some("live.example"));
    assert_eq!(s.env().get("PAPERLESS_EMAIL_HOST"), Some("live.example"));
    assert!(s.get("PAPERLESS_TIME_ZONE").unwrap().restart_pending);
    assert!(!s.get("PAPERLESS_EMAIL_HOST").unwrap().restart_pending);
    eprintln!("visible in B after {took:?}");
    // A sees it as well
    tokio::time::timeout(Duration::from_secs(2), async {
        while a.load().rev != rev {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    // Reset → restart_pending off again
    let rev2 = store::apply(&pa, &sb, &[("PAPERLESS_TIME_ZONE".into(), Input::Reset)], &opts(), &actor(), Action::Set).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while b.load().rev != rev2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(b.load().restart_pending_count(), 0);
    let _ = stop_tx.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        let _ = ja.await;
        let _ = jb.await;
    })
    .await;
    clean(&pa).await;
}

/// WP3: fallback without NOTIFY (e.g. a change whose NOTIFY got lost) via the state comparison.
#[tokio::test]
#[ignore]
async fn fallback_poll() {
    let _g = LOCK.lock().await;
    let pool = pool().await;
    clean(&pool).await;
    let sb = SecretBox::from_secret_key("k");
    let h = SettingsHandle::load_initial(&pool, Env::default(), sb.clone()).await.unwrap();
    let (stop_tx, stop) = tokio::sync::watch::channel(false);
    let j = h.spawn_listener_with(pool.clone(), stop, Duration::from_millis(200));
    tokio::time::sleep(Duration::from_millis(300)).await;
    // write directly, without pg_notify
    sqlx::query(
        "INSERT INTO paperrust_setting (key, value, rev) VALUES ('PAPERLESS_EMAIL_HOST', 'poll.example', nextval('paperrust_setting_rev'))",
    )
    .execute(&pool)
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while h.load().value("PAPERLESS_EMAIL_HOST") != Some("poll.example") {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("fallback poll did not pick up the change");
    let _ = stop_tx.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(5), j).await;
    clean(&pool).await;
}

/// WP4: process-wide effect – overlay for `pl_core::config::var` and `LiveConfig`; S values
/// only after restart (`startup_overlay` at startup), L/W values immediately.
#[tokio::test]
#[ignore]
async fn publish_overlay_live_config_and_startup() {
    let _g = LOCK.lock().await;
    let pool = pool().await;
    clean(&pool).await;
    let sb = SecretBox::from_secret_key("k");
    // restart key already in the DB before the "start"
    store::apply(&pool, &sb, &[("PAPERLESS_COOKIE_PREFIX".into(), Input::Raw("boot_".into()))], &opts(), &actor(), Action::Set)
        .await
        .unwrap();
    let url = url();
    let db = url.rsplit('/').next().unwrap().to_string();
    let core = pl_core::Config::from_env_map(Env::from_pairs([
        ("PAPERLESS_SECRET_KEY", "k"),
        ("PAPERLESS_DBHOST", "127.0.0.1"),
        ("PAPERLESS_DBPORT", "54329"),
        ("PAPERLESS_DBUSER", "paperless"),
        ("PAPERLESS_DBPASS", "paperless"),
        ("PAPERLESS_DBNAME", db.as_str()),
    ]))
    .unwrap();
    assert!(pl_settings::reload::startup_overlay(&core).await);
    assert_eq!(pl_core::config::var("PAPERLESS_COOKIE_PREFIX").as_deref(), Some("boot_"));
    // Holder as in the server
    let state = pl_core::AppState::with_pool(core.clone(), pool.clone()).unwrap();
    let h = SettingsHandle::load_initial(&pool, Env::from_process_raw(), sb.clone()).await.unwrap();
    let live = pl_core::live::LiveConfig::new(core.clone());
    state.set_service(live.clone());
    h.publish_process_wide(live.clone(), core.clone(), core.env.clone());
    // change an L key and an S key
    store::apply(
        &pool,
        &sb,
        &[
            ("PAPERLESS_EMAIL_HOST".into(), Input::Raw("live.example".into())),
            ("PAPERLESS_COOKIE_PREFIX".into(), Input::Raw("after_".into())),
        ],
        &opts(),
        &actor(),
        Action::Set,
    )
    .await
    .unwrap();
    h.reload(&pool).await.unwrap();
    assert_eq!(pl_core::config::var("PAPERLESS_EMAIL_HOST").as_deref(), Some("live.example"));
    assert_eq!(state.live_config().email_host, "live.example");
    // S key: start value until restart, badge "restart needed"
    assert_eq!(pl_core::config::var("PAPERLESS_COOKIE_PREFIX").as_deref(), Some("boot_"));
    assert_eq!(state.live_config().cookie_prefix, "boot_");
    assert!(h.load().get("PAPERLESS_COOKIE_PREFIX").unwrap().restart_pending);
    // Layering stays on the pure environment: DB values are not locked as "from environment"
    assert!(!h.load().get("PAPERLESS_EMAIL_HOST").unwrap().locked);
    pl_core::config::set_live_overlay(Default::default());
    clean(&pool).await;
}

/// Race on reload (found in the overall check 2026-11-18, `live_reload_two_processes`
/// failed under load): `apply` draws `nextval('paperrust_setting_rev')` before the COMMIT, the
/// new state is visible immediately. A snapshot loaded in this window must not
/// combine the new state with the old values – otherwise the notification after the COMMIT counts
/// as "unchanged" and subscribers stay on the old values.
#[tokio::test]
#[ignore]
async fn snapshot_rev_matches_values_during_apply() {
    let _g = LOCK.lock().await;
    let pool = pool().await;
    clean(&pool).await;
    let sb = SecretBox::from_secret_key("k");
    // `apply` by hand until shortly before the COMMIT (same lock, same flow)
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('paperrust_setting'))").execute(&mut *tx).await.unwrap();
    let rev: i64 = sqlx::query_scalar("SELECT nextval('paperrust_setting_rev')").fetch_one(&mut *tx).await.unwrap();
    sqlx::query(
        "INSERT INTO paperrust_setting (key, value, rev, updated_at) VALUES ('PAPERLESS_EMAIL_HOST', 'race.example', $1, now())
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, rev = EXCLUDED.rev",
    )
    .bind(rev)
    .execute(&mut *tx)
    .await
    .unwrap();
    let p2 = pool.clone();
    let sb2 = sb.clone();
    let loader = tokio::spawn(async move { store::load_settings(&p2, Env::from_pairs([("X", "1")]), &sb2, None).await });
    tokio::time::sleep(Duration::from_millis(400)).await;
    tx.commit().await.unwrap();
    let s = loader.await.unwrap().unwrap();
    if s.rev == rev {
        assert_eq!(s.value("PAPERLESS_EMAIL_HOST"), Some("race.example"), "snapshot with new rev but old values");
    } else {
        // a snapshot with the old state is fine (the notification reloads afterwards)
        assert!(s.rev < rev);
    }
    clean(&pool).await;
}
