//! Parity tests against the reference database (read-only).
//!
//! Expected values: `tests/django_expected.json`, produced by `tests/django_expect.py` with the
//! **upstream** paperless code (permitted_object_ids, permitted_document_ids,
//! get_permission_grantees, get_shared_object_pks, guardian ObjectPermissionChecker,
//! User.get_all_permissions) on the reference data:
//!
//! ```sh
//! source reference/env.sh && cd upstream/src && "$PY" ../../crates/pl-authz/tests/django_expect.py \
//!     ../../crates/pl-authz/tests/django_expected.json
//! ```
//!
//! Run: `cargo test -p pl-authz -j 2 -- --ignored` (needs PostgreSQL of `reference/`, URL via
//! `PL_AUTHZ_TEST_DB`, default `postgres://paperless:paperless@127.0.0.1:54329/paperless`).
//! Ignored by default because the reference DB is not always running; regenerate the fixture
//! after the reference data changes.

use pl_authz::models::{self, ObjectModel};
use pl_authz::objects::{self, PermOpts};
use pl_authz::{owned, perms, user};
use pl_core::sqlbind::SqlxBinder as _;
use sea_query::PostgresQueryBuilder;
use serde_json::Value;
use sqlx::PgPool;

fn expected() -> Value {
    serde_json::from_str(include_str!("django_expected.json")).unwrap()
}

async fn pool() -> PgPool {
    // L5: `PL_AUTHZ_ENGINE=grants` checks the same upstream expectations via paperrust_grant
    // (DB copy with migration pl_authz.0002 required).
    if std::env::var("PL_AUTHZ_ENGINE").as_deref() == Ok("grants") {
        pl_authz::grants::set_override(Some(pl_authz::grants::Engine::Grants));
    }
    let url = std::env::var("PL_AUTHZ_TEST_DB").unwrap_or_else(|_| "postgres://paperless:paperless@127.0.0.1:54329/paperless".into());
    sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap()
}

fn model(key: &str) -> ObjectModel {
    match key {
        "tag" => models::TAG,
        "correspondent" => models::CORRESPONDENT,
        "documenttype" => models::DOCUMENT_TYPE,
        "storagepath" => models::STORAGE_PATH,
        "savedview" => models::SAVED_VIEW,
        "mailaccount" => models::MAIL_ACCOUNT,
        "mailrule" => models::MAIL_RULE,
        "document" => models::DOCUMENT,
        other => panic!("unknown model {other}"),
    }
}

fn ids(v: &Value) -> Vec<i64> {
    v.as_array().unwrap().iter().map(|x| x.as_i64().unwrap()).collect()
}

async fn fetch_ids(db: &PgPool, q: sea_query::SelectStatement) -> Vec<i64> {
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows: Vec<(i32,)> = sqlx::query_as_with(&sql, values).fetch_all(db).await.unwrap();
    let mut v: Vec<i64> = rows.into_iter().map(|(i,)| i as i64).collect();
    v.sort_unstable();
    v
}

#[tokio::test]
#[ignore = "needs the reference PostgreSQL (reference/env.sh)"]
async fn users_groups_and_model_perms() {
    let db = pool().await;
    let exp = expected();
    for (name, e) in exp["users"].as_object().unwrap() {
        let u = user::load_user_by_username(&db, name).await.unwrap().unwrap();
        assert_eq!(u.id, e["id"].as_i64().unwrap(), "{name}");
        assert_eq!(u.groups, ids(&e["groups"]), "{name}");
        let want: Vec<String> = e["perms"].as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_string()).collect();
        assert_eq!(u.all_permissions_sorted(), want, "{name}");
    }
}

#[tokio::test]
#[ignore = "needs the reference PostgreSQL (reference/env.sh)"]
async fn permitted_object_ids_match_django() {
    let db = pool().await;
    let exp = expected();
    for (name, per_model) in exp["permitted"].as_object().unwrap() {
        let u = if name == "<anon>" { None } else { Some(user::load_user_by_username(&db, name).await.unwrap().unwrap()) };
        for (key, want) in per_model.as_object().unwrap() {
            let (m, action) = key.split_once('.').unwrap();
            let q = if m == "document" {
                objects::permitted_document_ids(u.as_ref(), &format!("{action}_document"), false)
            } else {
                let om = model(m);
                objects::permitted_object_ids(u.as_ref(), &om, &om.codename(action), &PermOpts::default())
            };
            assert_eq!(fetch_ids(&db, q).await, ids(want), "user {name} {key}");
        }
    }
}

#[tokio::test]
#[ignore = "needs the reference PostgreSQL (reference/env.sh)"]
async fn grantees_shared_and_object_perms_match_django() {
    let db = pool().await;
    let exp = expected();
    for (key, want) in exp["grantees"].as_object().unwrap() {
        let m = model(key);
        let pks: Vec<i64> = want.as_object().unwrap().keys().map(|k| k.parse().unwrap()).collect();
        let got = owned::get_permission_grantees(&db, &m, &pks).await.unwrap();
        for (pk, g) in want.as_object().unwrap() {
            let pk: i64 = pk.parse().unwrap();
            let mine = got[&pk].to_json();
            for action in ["view", "change"] {
                for kind in ["users", "groups"] {
                    assert_eq!(mine[action][kind], g[action][kind], "{key} {pk} {action} {kind}");
                }
            }
        }
        let shared = owned::get_shared_object_pks(&db, &m, &pks).await.unwrap();
        let mut shared: Vec<i64> = shared.into_iter().collect();
        shared.sort_unstable();
        assert_eq!(shared, ids(&exp["shared"][key]), "shared {key}");
    }
    for (key, per_user) in exp["obj_perm"].as_object().unwrap() {
        let m = model(key);
        let all: Vec<i64> = exp["grantees"][key].as_object().unwrap().keys().map(|k| k.parse().unwrap()).collect();
        for (name, per_action) in per_user.as_object().unwrap() {
            let u = user::load_user_by_username(&db, name).await.unwrap().unwrap();
            for (action, want) in per_action.as_object().unwrap() {
                let want = ids(want);
                for id in &all {
                    let got = perms::guardian_has_perm(&db, &u, &m, *id, &m.codename(action)).await.unwrap();
                    assert_eq!(got, want.contains(id), "{key} {name} {action} {id}");
                }
            }
        }
    }
}

#[tokio::test]
#[ignore = "needs the reference PostgreSQL (reference/env.sh)"]
async fn token_and_unknown_users() {
    let db = pool().await;
    assert!(user::load_user_by_token(&db, "definitely-not-a-token").await.unwrap().is_none());
    assert!(user::load_user_by_username(&db, "no-such-user").await.unwrap().is_none());
    // allauth-style case-insensitive lookup
    let a = user::load_user_by_username_iexact(&db, "ALICE").await.unwrap().unwrap();
    assert_eq!(a.username, "alice");
    // query must run; seed data has no MFA authenticators for alice
    assert!(!user::is_mfa_enabled(&db, a.id).await.unwrap());
}

/// O1: `OwnedInfo::load` with grantees and shared pks takes both from one query; it must
/// equal the two separate helpers (which the Django fixture checks above).
#[tokio::test]
#[ignore = "needs the reference PostgreSQL (reference/env.sh)"]
async fn owned_info_single_query_equals_separate_helpers() {
    let db = pool().await;
    let exp = expected();
    for key in exp["grantees"].as_object().unwrap().keys() {
        let m = model(key);
        let mut pks: Vec<i64> = exp["grantees"][key].as_object().unwrap().keys().map(|k| k.parse().unwrap()).collect();
        pks.push(999_999); // unknown pk: entry without grants
        let all = owned::OwnedFields::new(false, true);
        let info = owned::OwnedInfo::load(&db, &m, &pks, all, true).await.unwrap();
        let grantees = owned::get_permission_grantees(&db, &m, &pks).await.unwrap();
        let shared = owned::get_shared_object_pks(&db, &m, &pks).await.unwrap();
        assert_eq!(info.grantees, grantees, "grantees {key}");
        assert_eq!(info.shared, shared, "shared {key}");
        let empty = owned::OwnedInfo::load(&db, &m, &[], all, true).await.unwrap();
        assert!(empty.grantees.is_empty() && empty.shared.is_empty());
    }
}

/// O1: superusers get every permission (no DISTINCT in SQL; the set removes duplicates).
#[tokio::test]
#[ignore = "needs the reference PostgreSQL (reference/env.sh)"]
async fn superuser_has_all_permissions() {
    let db = pool().await;
    let admin = user::load_user_by_username(&db, "admin").await.unwrap().unwrap();
    assert!(admin.is_superuser);
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(DISTINCT ct.app_label || '.' || p.codename) FROM auth_permission p \
         JOIN django_content_type ct ON ct.id = p.content_type_id",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(admin.permissions.len() as i64, n);
}
