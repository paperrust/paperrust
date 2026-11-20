//! upstream fix fa83a4c5d (#14687, after v3.4.2): deleting a tag must check the delete permission
//! of every descendant (detail delete and `bulk_edit_objects` delete, with `objects` and `all`).
//! Mirrors `TestTagHierarchyPermissions.test_delete_checks_descendant_permissions`.
//!
//! Runs against a **private copy** (`pl_upsync_objects_test`, TEMPLATE `paperless_snapshot`),
//! never against `paperless`; the copy is dropped at the end.
//!
//! ```text
//! cargo test -p pl-objects -j 2 --test tag_delete_descendants -- --ignored
//! ```

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use pl_authz::{AuthConfig, Authz};
use pl_core::AppState;
use pl_core::config::Env;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const DB: &str = "pl_upsync_objects_test";
const ADMIN_URL: &str = "postgres://paperless:paperless@127.0.0.1:54329/postgres";

async fn recreate_db() -> PgPool {
    let admin = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(ADMIN_URL).await.expect("admin connect");
    sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&admin).await.expect("drop");
    sqlx::query(&format!("CREATE DATABASE {DB} TEMPLATE paperless_snapshot")).execute(&admin).await.expect("create");
    admin.close().await;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&format!("postgres://paperless:paperless@127.0.0.1:54329/{DB}"))
        .await
        .expect("db");
    // the snapshot is the plain v3.4.2 schema; PaperRust runs its own tag migration on start
    sqlx::raw_sql(pl_objects::migrations::TAG_ADJACENCY.1).execute(&pool).await.expect("tag adjacency migration");
    pool
}

async fn drop_db(pool: PgPool) {
    pool.close().await;
    let admin = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(ADMIN_URL).await.expect("admin connect");
    sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&admin).await.expect("drop");
}

fn app(pool: &PgPool) -> Router {
    let cfg = pl_core::Config::from_env_map(Env::from_pairs([
        ("PAPERLESS_SECRET_KEY", "paperless-rs-reference-secret-key-local-only-0123456789"),
        ("PAPERLESS_DBHOST", "127.0.0.1"),
        ("PAPERLESS_DBPORT", "54329"),
        ("PAPERLESS_DBNAME", DB),
        ("PAPERLESS_DBUSER", "paperless"),
        ("PAPERLESS_DBPASS", "paperless"),
        ("PAPERLESS_CACHE_BACKEND", "django.core.cache.backends.locmem.LocMemCache"),
    ]))
    .expect("config");
    let state = AppState::with_pool(cfg, pool.clone()).expect("state");
    let authz = Authz::new(pool.clone(), AuthConfig::from_core(state.config()));
    let mut app: Router<AppState> = Router::new();
    for r in pl_objects::routes() {
        app = app.route(r.path, r.handler);
    }
    app.with_state(state).layer(axum::Extension(authz))
}

async fn call(app: &Router, method: &str, path: &str, token: &str, body: Option<Value>) -> u16 {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("Host", "localhost")
        .header("Authorization", format!("Token {token}"))
        .header("Accept", "application/json; version=9");
    let body = match body {
        Some(v) => {
            b = b.header("Content-Type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let r = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = r.status().as_u16();
    let bytes = axum::body::to_bytes(r.into_body(), 1_000_000).await.unwrap();
    if status >= 400 {
        eprintln!("{method} {path} -> {status} {}", String::from_utf8_lossy(&bytes));
    }
    status
}

async fn user(db: &PgPool, name: &str, superuser: bool) -> (i32, String) {
    let id: i32 = sqlx::query_scalar(
        "INSERT INTO auth_user (password, is_superuser, username, first_name, last_name, email, is_staff, is_active, date_joined) \
         VALUES ('!', $1, $2, '', '', '', false, true, now()) RETURNING id",
    )
    .bind(superuser)
    .bind(name)
    .fetch_one(db)
    .await
    .unwrap();
    let token = format!("{:0>40}", format!("{id:x}ff"));
    sqlx::query("INSERT INTO authtoken_token (key, created, user_id) VALUES ($1, now(), $2)").bind(&token).bind(id).execute(db).await.unwrap();
    (id, token)
}

async fn perm_id(db: &PgPool, codename: &str) -> i32 {
    sqlx::query_scalar(
        "SELECT p.id FROM auth_permission p JOIN django_content_type c ON c.id = p.content_type_id \
         WHERE c.app_label = 'documents' AND c.model = 'tag' AND p.codename = $1",
    )
    .bind(codename)
    .fetch_one(db)
    .await
    .unwrap()
}

async fn tag(db: &PgPool, name: &str, owner: Option<i32>, parent: Option<i32>) -> i32 {
    sqlx::query_scalar(
        "INSERT INTO documents_tag (name, match, matching_algorithm, is_insensitive, color, is_inbox_tag, owner_id, tn_parent_id) \
         VALUES ($1, '', 0, true, '#a6cee3', false, $2, $3) RETURNING id",
    )
    .bind(name)
    .bind(owner)
    .bind(parent)
    .fetch_one(db)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "needs the reference PostgreSQL (127.0.0.1:54329) with the template paperless_snapshot"]
async fn delete_checks_descendant_permissions() {
    let db = recreate_db().await;
    let app = app(&db);
    let (view, delete) = (perm_id(&db, "view_tag").await, perm_id(&db, "delete_tag").await);
    let tag_ct: i32 = sqlx::query_scalar("SELECT id FROM django_content_type WHERE app_label = 'documents' AND model = 'tag'")
        .fetch_one(&db)
        .await
        .unwrap();
    let doc: i32 = sqlx::query_scalar("SELECT id FROM documents_document ORDER BY id LIMIT 1").fetch_one(&db).await.unwrap();
    let mut n = 0;
    for mode in ["detail", "bulk", "all"] {
        for ownership in ["owned", "unowned", "private", "shared", "superuser"] {
            n += 1;
            let (requester, token) = user(&db, &format!("req{n}"), ownership == "superuser").await;
            for p in [view, delete] {
                sqlx::query("INSERT INTO auth_user_user_permissions (user_id, permission_id) VALUES ($1, $2)")
                    .bind(requester)
                    .bind(p)
                    .execute(&db)
                    .await
                    .unwrap();
            }
            let descendant_owner = match ownership {
                "owned" => Some(requester),
                "unowned" => None,
                _ => Some(user(&db, &format!("other{n}"), false).await.0),
            };
            let parent = tag(&db, &format!("parent{n}"), Some(requester), None).await;
            let child = tag(&db, &format!("child{n}"), None, Some(parent)).await;
            let grandchild = tag(&db, &format!("grandchild{n}"), descendant_owner, Some(child)).await;
            if ownership == "shared" {
                sqlx::query("INSERT INTO guardian_userobjectpermission (object_pk, content_type_id, permission_id, user_id) VALUES ($1, $2, $3, $4)")
                    .bind(grandchild.to_string())
                    .bind(tag_ct)
                    .bind(delete)
                    .bind(requester)
                    .execute(&db)
                    .await
                    .unwrap();
            }
            sqlx::query("INSERT INTO documents_document_tags (document_id, tag_id) VALUES ($1, $2)")
                .bind(doc)
                .bind(grandchild)
                .execute(&db)
                .await
                .unwrap();
            let ids = vec![parent, child, grandchild];

            let status = match mode {
                "detail" => call(&app, "DELETE", &format!("/api/tags/{parent}/"), &token, None).await,
                _ => {
                    let mut body = json!({"object_type": "tags", "operation": "delete"});
                    if mode == "all" {
                        body["all"] = json!(true);
                        body["filters"] = json!({"id__in": parent.to_string()});
                    } else {
                        body["objects"] = json!([parent]);
                    }
                    call(&app, "POST", "/api/bulk_edit_objects/", &token, Some(body)).await
                }
            };

            // Detail deletion accepts explicit delete grants; bulk deletion requires ownership
            // (or an unowned object).
            let allowed = matches!(ownership, "owned" | "unowned" | "superuser") || (ownership == "shared" && mode == "detail");
            let expected = match (allowed, mode) {
                (false, _) => 403,
                (true, "detail") => 204,
                (true, _) => 200,
            };
            assert_eq!(status, expected, "mode={mode} ownership={ownership}");
            let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM documents_tag WHERE id = ANY($1)").bind(&ids).fetch_one(&db).await.unwrap();
            assert_eq!(left, if allowed { 0 } else { 3 }, "mode={mode} ownership={ownership}");
            let tagged: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM documents_document_tags WHERE document_id = $1 AND tag_id = $2)")
                .bind(doc)
                .bind(grandchild)
                .fetch_one(&db)
                .await
                .unwrap();
            assert_eq!(tagged, !allowed, "mode={mode} ownership={ownership}");
        }
    }
    drop_db(db).await;
}
