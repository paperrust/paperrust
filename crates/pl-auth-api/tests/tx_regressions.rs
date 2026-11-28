//! Regression tests for the external review (2026-11-27):
//!
//! 1. Logout race: saving an existing session is an UPDATE only, a key deleted meanwhile
//!    (concurrent logout) is never re-created (`session::save_row` → `Saved::Interrupted`).
//! 2. Password change + device-token revocation commit together (`db::set_password_and_revoke`,
//!    `db::update_user_and_revoke`, `pl_authz::device::revoke_all_for_user_tx`).
//! 4. TOTP activation + recovery codes in one transaction (`db::activate_totp`).
//!
//! Failures are injected with PostgreSQL triggers that raise. Uses its own throw-away database
//! `pl_authapi_tx_test` (from `template0`, minimal schema); never `paperless`. Dropped at the end.
//!
//! ```text
//! cargo test -p pl-auth-api -j 2 --test tx_regressions -- --ignored --test-threads=1
//! ```

use chrono::{Duration, Utc};
use pl_auth_api::session::{Saved, save_row};
use sqlx::PgPool;

const ADMIN_URL: &str = "postgres://paperless:paperless@127.0.0.1:54329/postgres";

async fn fresh_db(name: &str) -> (PgPool, PgPool) {
    let admin = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(ADMIN_URL).await.expect("admin connect");
    sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)")).execute(&admin).await.expect("drop");
    sqlx::query(&format!("CREATE DATABASE {name} TEMPLATE template0")).execute(&admin).await.expect("create");
    let db = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&format!("postgres://paperless:paperless@127.0.0.1:54329/{name}"))
        .await
        .expect("db");
    sqlx::raw_sql(
        "CREATE TABLE django_session (session_key varchar(40) PRIMARY KEY, session_data text NOT NULL, \
           expire_date timestamptz NOT NULL);
         CREATE TABLE auth_user (id serial PRIMARY KEY, password varchar(128) NOT NULL);
         CREATE TABLE paperrust_device_token (id bigserial PRIMARY KEY, user_id int4 NOT NULL, revoked_at timestamptz);
         CREATE TABLE mfa_authenticator (id bigserial PRIMARY KEY, user_id int4 NOT NULL, type varchar(20) NOT NULL, \
           data jsonb NOT NULL, created_at timestamptz NOT NULL, last_used_at timestamptz);
         CREATE UNIQUE INDEX unique_authenticator_type ON mfa_authenticator (user_id, type) \
           WHERE type IN ('totp', 'recovery_codes');
         CREATE FUNCTION fail_always() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected failure'; END $$;
         INSERT INTO auth_user (id, password) VALUES (1, 'old');
         INSERT INTO paperrust_device_token (user_id) VALUES (1), (1);",
    )
    .execute(&db)
    .await
    .expect("schema");
    (admin, db)
}

async fn drop_db(admin: PgPool, db: PgPool, name: &str) {
    db.close().await;
    sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)")).execute(&admin).await.expect("drop");
}

async fn password(db: &PgPool) -> String {
    sqlx::query_scalar("SELECT password FROM auth_user WHERE id = 1").fetch_one(db).await.expect("password")
}

async fn active_tokens(db: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM paperrust_device_token WHERE revoked_at IS NULL").fetch_one(db).await.expect("tokens")
}

#[tokio::test]
#[ignore = "needs the reference PostgreSQL on 127.0.0.1:54329"]
async fn saving_a_deleted_session_does_not_resurrect_it() {
    let name = "pl_authapi_tx_test_session";
    let (admin, db) = fresh_db(name).await;
    let expire = Utc::now() + Duration::days(14);
    let Saved::Key(key) = save_row(&db, None, "logged-in-data", expire).await.expect("create") else { panic!("create interrupted") };
    // Request A loaded `key` earlier; request B logs out (deletes the row) ...
    sqlx::query("DELETE FROM django_session WHERE session_key = $1").bind(&key).execute(&db).await.expect("logout");
    // ... then A saves its modified session.
    let r = save_row(&db, Some(&key), "logged-in-data-modified", expire).await.expect("save");
    assert_eq!(r, Saved::Interrupted);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM django_session").fetch_one(&db).await.expect("count");
    assert_eq!(n, 0, "the logged-out session key must not come back");
    // An existing row is still updated in place.
    let Saved::Key(k2) = save_row(&db, None, "a", expire).await.expect("create") else { panic!("create interrupted") };
    assert_eq!(save_row(&db, Some(&k2), "b", expire).await.expect("update"), Saved::Key(k2.clone()));
    let data: String =
        sqlx::query_scalar("SELECT session_data FROM django_session WHERE session_key = $1").bind(&k2).fetch_one(&db).await.expect("row");
    assert_eq!(data, "b");
    drop_db(admin, db, name).await;
}

#[tokio::test]
#[ignore = "needs the reference PostgreSQL on 127.0.0.1:54329"]
async fn password_change_and_token_revocation_are_atomic() {
    let name = "pl_authapi_tx_test_password";
    let (admin, db) = fresh_db(name).await;
    // revocation fails → the new password must not be stored either
    sqlx::query("CREATE TRIGGER t BEFORE UPDATE ON paperrust_device_token FOR EACH ROW EXECUTE FUNCTION fail_always()")
        .execute(&db)
        .await
        .expect("trigger");
    assert!(pl_auth_api::db::set_password_and_revoke(&db, 1, "new").await.is_err());
    assert_eq!(password(&db).await, "old", "password stored although the device tokens stayed valid");
    assert_eq!(active_tokens(&db).await, 2);
    sqlx::query("DROP TRIGGER t ON paperrust_device_token").execute(&db).await.expect("drop trigger");

    // normal case: both happen
    pl_auth_api::db::set_password_and_revoke(&db, 1, "new").await.expect("change");
    assert_eq!(password(&db).await, "new");
    assert_eq!(active_tokens(&db).await, 0);

    // missing table (migration not applied): password change still works (savepoint)
    sqlx::query("DROP TABLE paperrust_device_token").execute(&db).await.expect("drop table");
    pl_auth_api::db::set_password_and_revoke(&db, 1, "newer").await.expect("change without table");
    assert_eq!(password(&db).await, "newer");
    drop_db(admin, db, name).await;
}

#[tokio::test]
#[ignore = "needs the reference PostgreSQL on 127.0.0.1:54329"]
async fn totp_activation_with_recovery_codes_is_atomic() {
    let name = "pl_authapi_tx_test_totp";
    let (admin, db) = fresh_db(name).await;
    sqlx::raw_sql(
        "CREATE FUNCTION fail_rc() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN \
           IF NEW.type = 'recovery_codes' THEN RAISE EXCEPTION 'injected failure'; END IF; RETURN NEW; END $$;
         CREATE TRIGGER t BEFORE INSERT ON mfa_authenticator FOR EACH ROW EXECUTE FUNCTION fail_rc();",
    )
    .execute(&db)
    .await
    .expect("trigger");
    assert!(pl_auth_api::db::activate_totp(&db, 1, "JBSWY3DPEHPK3PXP", "seed").await.is_err());
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM mfa_authenticator").fetch_one(&db).await.expect("count");
    assert_eq!(n, 0, "TOTP must not stay active without recovery codes");

    // retry after the failure works and stores both
    sqlx::query("DROP TRIGGER t ON mfa_authenticator").execute(&db).await.expect("drop trigger");
    let rc = pl_auth_api::db::activate_totp(&db, 1, "JBSWY3DPEHPK3PXP", "seed").await.expect("activate");
    assert_eq!(rc, Some(serde_json::json!({ "seed": "seed", "used_mask": 0 })));
    let kinds: Vec<String> = sqlx::query_scalar("SELECT type FROM mfa_authenticator ORDER BY id").fetch_all(&db).await.expect("kinds");
    assert_eq!(kinds, ["totp", "recovery_codes"]);
    drop_db(admin, db, name).await;
}
