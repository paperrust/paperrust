//! upstream fixes after v3.4.2 (docs/UPSTREAM-SYNC.md, "Übernommene Einzel-Fixes"):
//!
//! - 53d9aa34a (#14709): reading `/api/config/` needs `paperless.view_applicationconfiguration`
//!   (`PaperlessModelPermissions`). Mirrors `test_api_read_config_requires_view_permission` /
//!   `test_api_read_config_with_view_permission` for the routes PaperRust serves (GET/HEAD;
//!   OPTIONS and the writes stay on Django).
//! - 0736c07d8 (#14710): `current_asn` of `/api/statistics/` over the visible documents. Mirrors
//!   `test_statistics_current_asn_*`.
//!
//! Each test runs against its own **private copy** (TEMPLATE `paperless_snapshot`), never against
//! `paperless`; the copies are dropped at the end.
//!
//! ```text
//! cargo test -p pl-system -j 2 --test upstream_fixes -- --ignored --test-threads 1
//! ```

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use pl_authz::{AuthConfig, Authz};
use pl_core::AppState;
use pl_core::config::Env;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;

const DB_CONFIG: &str = "pl_upsync_system_test";
const DB_STATS: &str = "pl_upsync_system_stats";
const ADMIN_URL: &str = "postgres://paperless:paperless@127.0.0.1:54329/postgres";

async fn admin() -> PgPool {
    sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(ADMIN_URL).await.expect("admin connect")
}

async fn copy_db(name: &str) -> PgPool {
    let a = admin().await;
    sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)")).execute(&a).await.expect("drop");
    sqlx::query(&format!("CREATE DATABASE {name} TEMPLATE paperless_snapshot")).execute(&a).await.expect("create");
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&format!("postgres://paperless:paperless@127.0.0.1:54329/{name}"))
        .await
        .expect("db")
}

async fn drop_db(pool: PgPool, name: &str) {
    pool.close().await;
    let a = admin().await;
    sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)")).execute(&a).await.expect("drop");
}

fn app(pool: &PgPool, db: &str, prefix: &str) -> Router {
    let cfg = pl_core::Config::from_env_map(Env::from_pairs([
        ("PAPERLESS_SECRET_KEY", "paperless-rs-reference-secret-key-local-only-0123456789"),
        ("PAPERLESS_DBHOST", "127.0.0.1"),
        ("PAPERLESS_DBPORT", "54329"),
        ("PAPERLESS_DBNAME", db),
        ("PAPERLESS_DBUSER", "paperless"),
        ("PAPERLESS_DBPASS", "paperless"),
        ("PAPERLESS_CACHE_BACKEND", "django.core.cache.backends.locmem.LocMemCache"),
    ]))
    .expect("config");
    let state = AppState::with_pool(cfg, pool.clone()).expect("state");
    let authz = Authz::new(pool.clone(), AuthConfig::from_core(state.config()));
    let mut app: Router<AppState> = Router::new();
    for r in pl_system::routes().into_iter().filter(|r| r.path.starts_with(prefix)) {
        app = app.route(r.path, r.handler);
    }
    app.with_state(state).layer(axum::Extension(authz))
}

async fn send(app: &Router, method: &str, path: &str, token: &str) -> (u16, Value) {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header("Host", "localhost")
        .header("Authorization", format!("Token {token}"))
        .header("Accept", "application/json; version=9")
        .body(Body::empty())
        .unwrap();
    let r = app.clone().oneshot(req).await.unwrap();
    let st = r.status().as_u16();
    let bytes = axum::body::to_bytes(r.into_body(), 10_000_000).await.unwrap();
    (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn user_with(db: &PgPool, name: &str, perms: &[(&str, &str)]) -> (i32, String) {
    let id: i32 = sqlx::query_scalar(
        "INSERT INTO auth_user (password, is_superuser, username, first_name, last_name, email, is_staff, is_active, date_joined) \
         VALUES ('!', false, $1, '', '', '', false, true, now()) RETURNING id",
    )
    .bind(name)
    .fetch_one(db)
    .await
    .unwrap();
    for (app_label, codename) in perms {
        sqlx::query(
            "INSERT INTO auth_user_user_permissions (user_id, permission_id) \
             SELECT $1, p.id FROM auth_permission p JOIN django_content_type c ON c.id = p.content_type_id \
             WHERE c.app_label = $2 AND p.codename = $3",
        )
        .bind(id)
        .bind(app_label)
        .bind(codename)
        .execute(db)
        .await
        .unwrap();
    }
    let token = format!("{:0>40}", format!("{id:x}ee"));
    sqlx::query("INSERT INTO authtoken_token (key, created, user_id) VALUES ($1, now(), $2)")
        .bind(&token)
        .bind(id)
        .execute(db)
        .await
        .unwrap();
    (id, token)
}

#[tokio::test]
#[ignore = "needs the reference PostgreSQL (127.0.0.1:54329) with the template paperless_snapshot"]
async fn read_config_requires_view_permission() {
    let db = copy_db(DB_CONFIG).await;
    let app = app(&db, DB_CONFIG, "/api/config/");
    let cfg_id: i32 =
        sqlx::query_scalar("SELECT id FROM paperless_applicationconfiguration ORDER BY id LIMIT 1").fetch_one(&db).await.unwrap();
    let endpoints = ["/api/config/".to_string(), format!("/api/config/{cfg_id}/")];

    let denied: [&[(&str, &str)]; 3] = [&[], &[("paperless", "change_applicationconfiguration")], &[("documents", "view_document")]];
    for (i, perms) in denied.iter().enumerate() {
        let (_, token) = user_with(&db, &format!("cfgdenied{i}"), perms).await;
        for ep in &endpoints {
            for method in ["GET", "HEAD"] {
                assert_eq!(send(&app, method, ep, &token).await.0, 403, "{perms:?} {method} {ep}");
            }
        }
    }

    let (_, token) = user_with(&db, "cfgviewer", &[("paperless", "view_applicationconfiguration")]).await;
    for ep in &endpoints {
        for method in ["GET", "HEAD"] {
            assert_eq!(send(&app, method, ep, &token).await.0, 200, "view perm {method} {ep}");
        }
    }

    drop_db(db, DB_CONFIG).await;
}

async fn current_asn(app: &Router, token: &str) -> i64 {
    let (st, v) = send(app, "GET", "/api/statistics/", token).await;
    assert_eq!(st, 200, "{v}");
    v["current_asn"].as_i64().expect("current_asn")
}

#[tokio::test]
#[ignore = "needs the reference PostgreSQL (127.0.0.1:54329) with the template paperless_snapshot"]
async fn statistics_current_asn_respects_document_permissions() {
    let db = copy_db(DB_STATS).await;
    let app = app(&db, DB_STATS, "/api/statistics/");
    let admin_id: i32 = sqlx::query_scalar("SELECT id FROM auth_user WHERE is_superuser ORDER BY id LIMIT 1").fetch_one(&db).await.unwrap();
    let doc_ct: i32 = sqlx::query_scalar("SELECT id FROM django_content_type WHERE app_label = 'documents' AND model = 'document'")
        .fetch_one(&db)
        .await
        .unwrap();
    let view_doc: i32 = sqlx::query_scalar("SELECT id FROM auth_permission WHERE content_type_id = $1 AND codename = 'view_document'")
        .bind(doc_ct)
        .fetch_one(&db)
        .await
        .unwrap();
    // Only these documents carry an ASN: the viewer's (12), an admin document shared with the
    // viewer (23), an admin document (99) and a version of the viewer's document (500, not
    // counted: versions are excluded from the statistics queryset).
    let ids: Vec<i32> =
        sqlx::query_scalar("SELECT id FROM documents_document WHERE root_document_id IS NULL AND deleted_at IS NULL ORDER BY id LIMIT 4")
            .fetch_all(&db)
            .await
            .unwrap();
    sqlx::query("UPDATE documents_document SET archive_serial_number = NULL").execute(&db).await.unwrap();
    let (viewer_id, viewer) = user_with(&db, "asnviewer", &[("documents", "view_document")]).await;
    for (id, owner, asn, root) in
        [(ids[0], viewer_id, 12, None), (ids[1], admin_id, 23, None), (ids[2], admin_id, 99, None), (ids[3], viewer_id, 500, Some(ids[0]))]
    {
        sqlx::query("UPDATE documents_document SET owner_id = $2, archive_serial_number = $3, root_document_id = $4 WHERE id = $1")
            .bind(id)
            .bind(owner)
            .bind(asn)
            .bind(root)
            .execute(&db)
            .await
            .unwrap();
    }
    sqlx::query("INSERT INTO guardian_userobjectpermission (object_pk, content_type_id, permission_id, user_id) VALUES ($1, $2, $3, $4)")
        .bind(ids[1].to_string())
        .bind(doc_ct)
        .bind(view_doc)
        .bind(viewer_id)
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(current_asn(&app, &viewer).await, 23);

    // test_statistics_current_asn_without_visible_documents
    let no_perms: [&[(&str, &str)]; 2] = [&[], &[("documents", "view_document")]];
    for (i, perms) in no_perms.iter().enumerate() {
        let (_, token) = user_with(&db, &format!("asnnone{i}"), perms).await;
        assert_eq!(current_asn(&app, &token).await, 0, "{perms:?}");
    }

    // test_statistics_current_asn_with_global_statistics_permission
    let (_, token) = user_with(&db, "asnglobal", &[("paperless", "view_global_statistics")]).await;
    assert_eq!(current_asn(&app, &token).await, 99);

    drop_db(db, DB_STATS).await;
}
