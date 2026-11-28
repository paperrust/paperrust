//! Timing of bulk folder moves/deletes with the locks of 2026-11-28 (one transaction, caller and groups
//! FOR SHARE, documents FOR SHARE, rights re-read). Not a pass/fail test: prints medians. Needs a
//! **dedicated copy** of a large DB (e.g. 100,000 documents):
//! ```text
//! psql -h 127.0.0.1 -p 54329 -U paperless -d postgres -c "CREATE DATABASE pl_folders_bench TEMPLATE pl_bulk_100k"
//! PAPERRUST_FOLDERS_BENCH_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/pl_folders_bench \
//!   cargo test -p pl-api-v2 --test folders_bench -j 2 -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use pl_core::AppState;
use pl_core::config::{Config, Env};
use serde_json::{Value, json};
use tower::ServiceExt;

const SU: &str = "plfoldersbenchsu000000000000000000000000";
const ED: &str = "plfoldersbenched000000000000000000000000";
const RUNS: usize = 3;

fn url() -> String {
    let url = std::env::var("PAPERRUST_FOLDERS_BENCH_DATABASE_URL").expect("PAPERRUST_FOLDERS_BENCH_DATABASE_URL");
    let db = url.trim_end_matches('/').rsplit('/').next().unwrap_or("").split('?').next().unwrap_or("").to_string();
    assert!(db.starts_with("pl_folders"), "refusing to run against a non-test database ({db})");
    url
}

async fn cleanup(pool: &sqlx::PgPool) {
    sqlx::raw_sql(
        "DELETE FROM paperrust_document_folder; DELETE FROM paperrust_folder_grant;
         UPDATE paperrust_folder SET parent_id = NULL; DELETE FROM paperrust_folder;
         DELETE FROM authtoken_token WHERE key LIKE 'plfoldersbench%';
         DELETE FROM auth_user_user_permissions WHERE user_id IN (SELECT id FROM auth_user WHERE username LIKE 'pl_folders_bench_%');
         DELETE FROM auth_user WHERE username LIKE 'pl_folders_bench_%';",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn user(pool: &sqlx::PgPool, name: &str, su: bool, token: &str) {
    let id: i32 = sqlx::query_scalar(
        "INSERT INTO auth_user (password, is_superuser, username, first_name, last_name, email, is_staff, is_active, date_joined)
         VALUES ('!', $2, $1, '', '', '', $2, true, now()) RETURNING id",
    )
    .bind(name)
    .bind(su)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO auth_user_user_permissions (user_id, permission_id)
         SELECT $1, p.id FROM auth_permission p JOIN django_content_type ct ON ct.id = p.content_type_id
         WHERE ct.app_label || '.' || p.codename IN ('documents.view_document', 'documents.change_document')",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO authtoken_token (key, created, user_id) VALUES ($1, now(), $2)")
        .bind(token)
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
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

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

async fn timed_move(r: &Router, token: &str, ids: &[i64], folder: Option<i64>) -> Duration {
    let t = Instant::now();
    let (s, b) = call(r, "POST", "/folders/move-documents", token, Some(json!({"documents": ids, "folder": folder}))).await;
    let d = t.elapsed();
    assert_eq!(s, StatusCode::OK, "{b}");
    d
}

async fn folder(r: &Router, token: &str, name: &str) -> i64 {
    let (s, f) = call(r, "POST", "/folders", token, Some(json!({"name": name}))).await;
    assert_eq!(s, StatusCode::CREATED, "{f}");
    f["id"].as_i64().unwrap()
}

#[tokio::test]
#[ignore = "benchmark; needs a large DB copy pl_folders_bench*, see the module comment"]
async fn bulk_move_and_delete_timings() {
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(4).connect(&url()).await.expect("connect");
    sqlx::raw_sql(pl_api_v2::folders::MIGRATION.1).execute(&pool).await.unwrap();
    cleanup(&pool).await;
    user(&pool, "pl_folders_bench_su", true, SU).await;
    user(&pool, "pl_folders_bench_ed", false, ED).await;
    let cfg = Config::from_env_map(Env::from_pairs(vec![
        ("PAPERLESS_SECRET_KEY", "pl-folders-bench-only-0123456789abcdefghijklmnopq"),
        ("PAPERLESS_DBHOST", "127.0.0.1"),
    ]))
    .expect("config");
    let state = AppState::with_pool(cfg.clone(), pool.clone()).expect("state");
    let authz = pl_authz::Authz::new(pool.clone(), pl_authz::AuthConfig::from_core(&cfg));
    let r: Router<AppState> = pl_api_v2::folders::routes(Router::new());
    let r = r.with_state(state).layer(axum::Extension(authz));

    let all: Vec<i64> = sqlx::query_scalar(&format!(
        "SELECT id::int8 FROM documents_document WHERE root_document_id IS NULL AND deleted_at IS NULL ORDER BY id LIMIT {}",
        pl_api_v2::folders::MOVE_MAX
    ))
    .fetch_all(&pool)
    .await
    .unwrap();
    let open: Vec<i64> = sqlx::query_scalar(
        "SELECT id::int8 FROM documents_document WHERE root_document_id IS NULL AND deleted_at IS NULL AND owner_id IS NULL ORDER BY id LIMIT 10000",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    println!("documents: superuser set {}, editor set (ownerless) {}", all.len(), open.len());

    let a = folder(&r, SU, "BenchA").await;
    let b = folder(&r, SU, "BenchB").await;
    let e = folder(&r, ED, "BenchEd").await;
    let _ = timed_move(&r, SU, &all[..1000], Some(a)).await; // warm-up
    let _ = timed_move(&r, SU, &all[..1000], None).await;

    let (mut ins, mut upd, mut del) = (vec![], vec![], vec![]);
    for _ in 0..RUNS {
        ins.push(timed_move(&r, SU, &all, Some(a)).await);
        upd.push(timed_move(&r, SU, &all, Some(b)).await);
        del.push(timed_move(&r, SU, &all, None).await);
    }
    println!(
        "superuser move {} docs: into folder {:?}, folder->folder {:?}, to Unsorted {:?}",
        all.len(),
        median(ins),
        median(upd),
        median(del)
    );

    let (mut eins, mut edel) = (vec![], vec![]);
    for _ in 0..RUNS {
        eins.push(timed_move(&r, ED, &open, Some(e)).await);
        edel.push(timed_move(&r, ED, &open, None).await);
    }
    println!("editor move {} docs (object-permission check): into folder {:?}, to Unsorted {:?}", open.len(), median(eins), median(edel));

    // share of the document row lock alone
    let table = {
        let mut c = pool.acquire().await.unwrap();
        pl_authz::grants::document_lock_table(&mut c).await.unwrap()
    };
    let ids32: Vec<i32> = all.iter().map(|i| *i as i32).collect();
    let mut lk = vec![];
    for _ in 0..RUNS {
        let mut tx = pool.begin().await.unwrap();
        let t = Instant::now();
        sqlx::query(&format!("SELECT id FROM {table} WHERE id = ANY($1::int4[]) ORDER BY id FOR SHARE"))
            .bind(&ids32)
            .execute(&mut *tx)
            .await
            .unwrap();
        lk.push(t.elapsed());
        tx.rollback().await.unwrap();
    }
    println!("lock step alone ({} rows FOR SHARE): {:?}", ids32.len(), median(lk));

    // folder delete with all documents inside
    let mut fd = vec![];
    for i in 0..RUNS {
        let f = folder(&r, SU, &format!("BenchDel{i}")).await;
        let _ = timed_move(&r, SU, &all, Some(f)).await;
        let t = Instant::now();
        let (s, body) = call(&r, "DELETE", &format!("/folders/{f}?contents=unsorted"), SU, None).await;
        fd.push(t.elapsed());
        assert_eq!(s, StatusCode::OK, "{body}");
    }
    println!("superuser delete folder with {} docs: {:?}", all.len(), median(fd));
    let mut efd = vec![];
    for i in 0..RUNS {
        let f = folder(&r, ED, &format!("BenchEdDel{i}")).await;
        let _ = timed_move(&r, ED, &open, Some(f)).await;
        let t = Instant::now();
        let (s, body) = call(&r, "DELETE", &format!("/folders/{f}?contents=unsorted"), ED, None).await;
        efd.push(t.elapsed());
        assert_eq!(s, StatusCode::OK, "{body}");
    }
    println!("editor delete folder with {} docs (object-permission check): {:?}", open.len(), median(efd));
    cleanup(&pool).await;
}
