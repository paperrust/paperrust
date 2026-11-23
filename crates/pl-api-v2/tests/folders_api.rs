//! Folder view `/rs/api/v2/folders` (results/ideas/2026-11-22-papermerge-ordneransicht.md, stage 1) against a
//! **dedicated copy** of the reference DB (never `paperless`):
//! ```text
//! psql -h 127.0.0.1 -p 54329 -U paperless -d postgres -c "CREATE DATABASE pl_folders_api_test TEMPLATE paperless_snapshot"
//! PAPERRUST_FOLDERS_TEST_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/pl_folders_api_test \
//!   cargo test -p pl-api-v2 --test folders_api -j 2 -- --ignored --test-threads 1
//! ```
//! The test creates the folder tables itself (same migration as `manage migrate`) and cleans up its own rows.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use pl_core::AppState;
use pl_core::config::{Config, Env};
use serde_json::{Value, json};
use tower::ServiceExt;

const SU_TOKEN: &str = "plfoldersapitestsu0000000000000000000000";
const EDIT_TOKEN: &str = "plfoldersapitested0000000000000000000000";
const READ_TOKEN: &str = "plfoldersapitestrd0000000000000000000000";

fn url() -> String {
    let url = std::env::var("PAPERRUST_FOLDERS_TEST_DATABASE_URL").expect("PAPERRUST_FOLDERS_TEST_DATABASE_URL");
    let db = url.trim_end_matches('/').rsplit('/').next().unwrap_or("").split('?').next().unwrap_or("").to_string();
    assert!(db.starts_with("pl_folders"), "refusing to run against a non-test database ({db})");
    url
}

async fn user(pool: &sqlx::PgPool, name: &str, su: bool, perms: &[&str]) -> i64 {
    let id: i32 = sqlx::query_scalar(
        "INSERT INTO auth_user (password, is_superuser, username, first_name, last_name, email, is_staff, is_active, date_joined)
         VALUES ('!', $2, $1, '', '', '', $2, true, now()) RETURNING id",
    )
    .bind(name)
    .bind(su)
    .fetch_one(pool)
    .await
    .unwrap();
    let perms: Vec<String> = perms.iter().map(|s| s.to_string()).collect();
    sqlx::query(
        "INSERT INTO auth_user_user_permissions (user_id, permission_id)
         SELECT $1, p.id FROM auth_permission p JOIN django_content_type ct ON ct.id = p.content_type_id
         WHERE ct.app_label || '.' || p.codename = ANY($2)",
    )
    .bind(id)
    .bind(&perms)
    .execute(pool)
    .await
    .unwrap();
    i64::from(id)
}

async fn cleanup(pool: &sqlx::PgPool) {
    sqlx::raw_sql(
        "DELETE FROM paperrust_document_folder; DELETE FROM paperrust_folder_grant;
         UPDATE paperrust_folder SET parent_id = NULL; DELETE FROM paperrust_folder;
         DELETE FROM authtoken_token WHERE key LIKE 'plfoldersapitest%';
         DELETE FROM auth_user_user_permissions WHERE user_id IN (SELECT id FROM auth_user WHERE username LIKE 'pl_folders_api_%');
         DELETE FROM auth_user WHERE username LIKE 'pl_folders_api_%';",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn setup() -> (Router, sqlx::PgPool) {
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(4).connect(&url()).await.expect("connect");
    sqlx::raw_sql(pl_api_v2::folders::MIGRATION.1).execute(&pool).await.unwrap();
    cleanup(&pool).await;
    let su = user(&pool, "pl_folders_api_su", true, &[]).await;
    let ed = user(&pool, "pl_folders_api_ed", false, &["documents.view_document", "documents.change_document"]).await;
    let rd = user(&pool, "pl_folders_api_rd", false, &["documents.view_document"]).await;
    for (k, u) in [(SU_TOKEN, su), (EDIT_TOKEN, ed), (READ_TOKEN, rd)] {
        sqlx::query("INSERT INTO authtoken_token (key, created, user_id) VALUES ($1, now(), $2)")
            .bind(k)
            .bind(u as i32)
            .execute(&pool)
            .await
            .unwrap();
    }
    let cfg = Config::from_env_map(Env::from_pairs(vec![
        ("PAPERLESS_SECRET_KEY", "pl-folders-api-tests-only-0123456789abcdefghijklmn"),
        ("PAPERLESS_DBHOST", "127.0.0.1"),
    ]))
    .expect("config");
    let state = AppState::with_pool(cfg.clone(), pool.clone()).expect("state");
    let authz = pl_authz::Authz::new(pool.clone(), pl_authz::AuthConfig::from_core(&cfg));
    let r: Router<AppState> = pl_api_v2::folders::routes(Router::new());
    (r.with_state(state).layer(axum::Extension(authz)), pool)
}

async fn call(router: &Router, method: &str, uri: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder().method(method).uri(uri).header("host", "localhost").header("authorization", format!("Token {token}"));
    let body = match body {
        Some(v) => {
            b = b.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = router.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 24).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn count_of(list: &Value, id: i64) -> i64 {
    list["items"].as_array().unwrap().iter().find(|f| f["id"] == json!(id)).map(|f| f["document_count"].as_i64().unwrap()).unwrap_or(-1)
}

#[tokio::test]
#[ignore = "needs the reference PostgreSQL (copy pl_folders_api_test of paperless_snapshot), see the module comment"]
async fn folder_lifecycle_and_permissions() {
    let (r, pool) = setup().await;
    let docs: Vec<i64> = sqlx::query_scalar(
        "SELECT id::int8 FROM documents_document WHERE root_document_id IS NULL AND deleted_at IS NULL ORDER BY id LIMIT 3",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(docs.len(), 3, "reference data has documents");

    // create A, A/B; duplicate name on the same level (case-insensitive) → 400
    let (s, a) = call(&r, "POST", "/folders", SU_TOKEN, Some(json!({"name": " Rechnungen "}))).await;
    assert_eq!(s, StatusCode::CREATED, "{a}");
    assert_eq!(a["name"], "Rechnungen");
    let a_id = a["id"].as_i64().unwrap();
    let (s, b) = call(&r, "POST", "/folders", SU_TOKEN, Some(json!({"name": "2026", "parent": a_id}))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let b_id = b["id"].as_i64().unwrap();
    assert_eq!(b["path"], json!([{"id": a_id, "name": "Rechnungen"}, {"id": b_id, "name": "2026"}]));
    let (s, e) = call(&r, "POST", "/folders", SU_TOKEN, Some(json!({"name": "rechnungen"}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");
    assert!(e["error"]["fields"]["name"].is_array());

    // counts before/after moving
    let (_, before) = call(&r, "GET", "/folders", SU_TOKEN, None).await;
    let unsorted0 = before["unsorted"].as_i64().unwrap();
    let (s, m) =
        call(&r, "POST", "/folders/move-documents", SU_TOKEN, Some(json!({"documents": [docs[0], docs[1], docs[0]], "folder": b_id})))
            .await;
    assert_eq!(s, StatusCode::OK, "{m}");
    assert_eq!(m["moved"], 2);
    let (_, after) = call(&r, "GET", "/folders", SU_TOKEN, None).await;
    assert_eq!(after["unsorted"].as_i64().unwrap(), unsorted0 - 2);
    assert_eq!(count_of(&after, b_id), 2);
    assert_eq!(count_of(&after, a_id), 0);
    // moving again into the same folder changes nothing
    let (_, m) = call(&r, "POST", "/folders/move-documents", SU_TOKEN, Some(json!({"documents": [docs[0]], "folder": b_id}))).await;
    assert_eq!(m["moved"], 0);

    // cycle → 400; If-Match with an old rev → 412
    let (s, e) = call(&r, "PATCH", &format!("/folders/{a_id}"), SU_TOKEN, Some(json!({"parent": b_id}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{e}");

    // reader: sees nothing owned by the superuser, may not create or move
    let (s, l) = call(&r, "GET", "/folders", READ_TOKEN, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(l["items"], json!([]));
    assert_eq!(l["can_organize"], false);
    let (s, _) = call(&r, "POST", "/folders", READ_TOKEN, Some(json!({"name": "x"}))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(&r, "GET", &format!("/folders/{a_id}"), READ_TOKEN, None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // share A with the editor (view+change): visible and changeable, B stays invisible
    let ed: i64 = sqlx::query_scalar("SELECT id::int8 FROM auth_user WHERE username = 'pl_folders_api_ed'").fetch_one(&pool).await.unwrap();
    let (s, _) =
        call(&r, "PATCH", &format!("/folders/{a_id}"), SU_TOKEN, Some(json!({"set_permissions": {"change": {"users": [ed]}}}))).await;
    assert_eq!(s, StatusCode::OK);
    let (_, l) = call(&r, "GET", "/folders", EDIT_TOKEN, None).await;
    let ids: Vec<i64> = l["items"].as_array().unwrap().iter().map(|f| f["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, vec![a_id]);
    let (s, _) = call(&r, "DELETE", &format!("/folders/{a_id}"), EDIT_TOKEN, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "no delete bit");
    let (s, own) = call(&r, "POST", "/folders", EDIT_TOKEN, Some(json!({"name": "Mine", "parent": a_id}))).await;
    assert_eq!(s, StatusCode::CREATED, "{own}");
    assert_eq!(own["owner"], json!(ed));

    // delete B → documents to the parent A
    let (s, d) = call(&r, "DELETE", &format!("/folders/{b_id}?contents=parent"), SU_TOKEN, None).await;
    assert_eq!(s, StatusCode::OK, "{d}");
    assert_eq!(d["moved_documents"], 2);
    let in_a: i64 = sqlx::query_scalar("SELECT count(*)::int8 FROM paperrust_document_folder WHERE folder_id = $1")
        .bind(a_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(in_a, 2);
    // delete A → documents unsorted, subfolder "Mine" to the top level
    let (s, d) = call(&r, "DELETE", &format!("/folders/{a_id}?contents=unsorted"), SU_TOKEN, None).await;
    assert_eq!(s, StatusCode::OK, "{d}");
    assert_eq!(d["moved_folders"], 1);
    let mapped: i64 = sqlx::query_scalar("SELECT count(*)::int8 FROM paperrust_document_folder").fetch_one(&pool).await.unwrap();
    assert_eq!(mapped, 0);
    let (_, l) = call(&r, "GET", "/folders", SU_TOKEN, None).await;
    assert_eq!(l["unsorted"].as_i64().unwrap(), unsorted0);
    assert_eq!(l["items"][0]["parent"], Value::Null);

    cleanup(&pool).await;
}

/// Stage 2: upload into a folder (`folder` field of post_document, checked by `check_upload_target`).
#[tokio::test]
#[ignore = "needs the reference PostgreSQL (copy pl_folders_api_test of paperless_snapshot), see the module comment"]
async fn upload_target_rules() {
    use pl_api_v2::folders::{UPLOAD_FOLDER_FORBIDDEN, check_upload_target, upload_folder_missing};
    let (r, pool) = setup().await;
    let load = |name: &'static str| {
        let pool = pool.clone();
        async move {
            let id: i32 = sqlx::query_scalar("SELECT id FROM auth_user WHERE username = $1").bind(name).fetch_one(&pool).await.unwrap();
            pl_authz::user::load_user_by_id(&pool, i64::from(id)).await.unwrap().unwrap()
        }
    };
    let su = load("pl_folders_api_su").await;
    let ed = load("pl_folders_api_ed").await;
    let rd = load("pl_folders_api_rd").await;
    // superuser's own folder: only the superuser may file into it; an ownerless folder is open to editors
    let (_, own) = call(&r, "POST", "/folders", SU_TOKEN, Some(json!({"name": "Private"}))).await;
    let own_id = own["id"].as_i64().unwrap();
    let (_, open) = call(&r, "POST", "/folders", SU_TOKEN, Some(json!({"name": "Open", "owner": null}))).await;
    let open_id = open["id"].as_i64().unwrap();
    assert_eq!(check_upload_target(&pool, &su, own_id).await.unwrap(), Ok(()));
    assert_eq!(check_upload_target(&pool, &ed, own_id).await.unwrap(), Err(upload_folder_missing(own_id)), "invisible");
    assert_eq!(check_upload_target(&pool, &ed, open_id).await.unwrap(), Ok(()));
    assert_eq!(check_upload_target(&pool, &rd, open_id).await.unwrap(), Err(UPLOAD_FOLDER_FORBIDDEN.to_string()), "no change_document");
    assert_eq!(check_upload_target(&pool, &su, 999_999_999).await.unwrap(), Err(upload_folder_missing(999_999_999)));
    cleanup(&pool).await;
}
