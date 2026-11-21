//! L6 step 1 (`pl_objects.0001_paperrust_tag_adjacency`, upstream #14612 reproduced): after the
//! migration the tag API returns the same hierarchy as the django-treenode columns did before.
//!
//! The expectation comes from the **unchanged** copy of the reference data (`tn_children_pks`/`tn_descendants_pks`
//! written by django-treenode), then it migrates and compares via the API;
//! plus validation (`Tag.clean`), parent change and deletion via the parent edge.
//!
//! Runs against a **private copy** (`pl_tag_adjacency_test`, TEMPLATE `paperless_snapshot`),
//! never against `paperless`; the copy is dropped at the end.
//!
//! ```text
//! cargo test -p pl-objects -j 2 --test tag_adjacency -- --ignored
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use pl_authz::{AuthConfig, Authz};
use pl_core::AppState;
use pl_core::config::Env;
use pl_core::tasks::TaskRequest;
use pl_core::tasks::{DispatchFuture, Dispatcher, TaskDispatch};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const DB: &str = "pl_tag_adjacency_test";

/// Accepts tasks (bulk_update_documents) instead of Celery/Redis.
#[derive(Default)]
struct Rec(Mutex<Vec<TaskRequest>>);

impl TaskDispatch for Rec {
    fn dispatch(&self, req: TaskRequest) -> DispatchFuture<'_> {
        if let Ok(mut v) = self.0.lock() {
            v.push(req);
        }
        Box::pin(async { Ok(uuid::Uuid::nil()) })
    }
}
const ADMIN_URL: &str = "postgres://paperless:paperless@127.0.0.1:54329/postgres";

async fn recreate_db() -> PgPool {
    let admin = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(ADMIN_URL).await.expect("admin connect");
    sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&admin).await.expect("drop");
    sqlx::query(&format!("CREATE DATABASE {DB} TEMPLATE paperless_snapshot")).execute(&admin).await.expect("create");
    admin.close().await;
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&format!("postgres://paperless:paperless@127.0.0.1:54329/{DB}"))
        .await
        .expect("db")
}

async fn drop_db(pool: PgPool) {
    pool.close().await;
    let admin = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(ADMIN_URL).await.expect("admin connect");
    sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&admin).await.expect("drop");
}

fn app(pool: &PgPool, rec: Arc<Rec>) -> Router {
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
    state.set_service(Dispatcher(rec));
    let authz = Authz::new(pool.clone(), AuthConfig::from_core(state.config()));
    let mut app: Router<AppState> = Router::new();
    for r in pl_objects::routes() {
        app = app.route(r.path, r.handler);
    }
    app.with_state(state).layer(axum::Extension(authz))
}

async fn call(app: &Router, method: &str, path: &str, token: &str, body: Option<Value>) -> (u16, Value) {
    let mut b = Request::builder()
        .method(method)
        .uri(path)
        .header("Host", "localhost")
        .header("Authorization", format!("Token {token}"))
        .header("Accept", "application/json; version=10");
    let body = match body {
        Some(v) => {
            b = b.header("Content-Type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let r = app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = r.status().as_u16();
    let bytes = axum::body::to_bytes(r.into_body(), 10_000_000).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn superuser(db: &PgPool) -> String {
    let id: i32 = sqlx::query_scalar(
        "INSERT INTO auth_user (password, is_superuser, username, first_name, last_name, email, is_staff, is_active, date_joined) \
         VALUES ('!', true, 'adjacency-admin', '', '', '', true, true, now()) RETURNING id",
    )
    .fetch_one(db)
    .await
    .unwrap();
    let token = format!("{:0>40}", format!("{id:x}ad"));
    sqlx::query("INSERT INTO authtoken_token (key, created, user_id) VALUES ($1, now(), $2)")
        .bind(&token)
        .bind(id)
        .execute(db)
        .await
        .unwrap();
    token
}

fn split(s: &str) -> BTreeSet<i64> {
    s.split(',').filter_map(|p| p.trim().parse().ok()).collect()
}

/// Child ids per tag from the nested `children` of an API response (recursive).
fn collect(v: &Value, parent_of: &mut BTreeMap<i64, Option<i64>>, children: &mut BTreeMap<i64, BTreeSet<i64>>) {
    let id = v["id"].as_i64().unwrap();
    parent_of.insert(id, v["parent"].as_i64());
    let kids = v["children"].as_array().cloned().unwrap_or_default();
    let set = children.entry(id).or_default();
    for k in &kids {
        set.insert(k["id"].as_i64().unwrap());
    }
    for k in &kids {
        collect(k, parent_of, children);
    }
}

#[tokio::test]
#[ignore = "needs the reference PostgreSQL (127.0.0.1:54329) with the template paperless_snapshot"]
async fn api_hierarchy_unchanged_after_migration() {
    let db = recreate_db().await;

    // Expectation: values written by django-treenode (v3.4.2 schema, not yet migrated)
    let rows: Vec<(i32, Option<i32>, String, String)> =
        sqlx::query_as("SELECT id, tn_parent_id, tn_children_pks, tn_descendants_pks FROM documents_tag ORDER BY id")
            .fetch_all(&db)
            .await
            .unwrap();
    assert!(rows.iter().any(|r| r.1.is_some()), "snapshot without nested tags");
    let exp_parent: BTreeMap<i64, Option<i64>> = rows.iter().map(|r| (i64::from(r.0), r.1.map(i64::from))).collect();
    let exp_children: BTreeMap<i64, BTreeSet<i64>> = rows.iter().map(|r| (i64::from(r.0), split(&r.2))).collect();
    let exp_desc: BTreeMap<i64, BTreeSet<i64>> = rows.iter().map(|r| (i64::from(r.0), split(&r.3))).collect();

    // Migration (twice: idempotent)
    for _ in 0..2 {
        sqlx::raw_sql(pl_objects::migrations::TAG_ADJACENCY.1).execute(&db).await.expect("migration");
    }
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM information_schema.columns WHERE table_name = 'documents_tag' AND column_name LIKE 'tn\\_%' AND column_name <> 'tn_parent_id'",
    )
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(left, 0);

    // hierarchy in Rust equals the treenode values
    let tree = pl_objects::tagtree::TagTree::load_pool(&db).await.unwrap();
    for (id, desc) in &exp_desc {
        assert_eq!(&tree.descendants([*id]).into_iter().collect::<BTreeSet<_>>(), desc, "descendants of {id}");
    }

    let rec = Arc::new(Rec::default());
    let app = app(&db, rec.clone());
    let token = superuser(&db).await;

    // list: parent + children (nested) as before
    let (st, list) = call(&app, "GET", "/api/tags/?page_size=100000", &token, None).await;
    assert_eq!(st, 200);
    let (mut parent_of, mut children) = (BTreeMap::new(), BTreeMap::new());
    for t in list["results"].as_array().unwrap() {
        collect(t, &mut parent_of, &mut children);
    }
    assert_eq!(parent_of, exp_parent);
    assert_eq!(children, exp_children);
    assert_eq!(list["display_count"], json!(exp_parent.len()));

    // detail of a root tag with grandchildren: nested children complete
    let root = exp_desc.iter().max_by_key(|(_, d)| d.len()).map(|(i, _)| *i).unwrap();
    let (st, one) = call(&app, "GET", &format!("/api/tags/{root}/"), &token, None).await;
    assert_eq!(st, 200);
    let (mut p1, mut c1) = (BTreeMap::new(), BTreeMap::new());
    collect(&one, &mut p1, &mut c1);
    let shown: BTreeSet<i64> = p1.keys().copied().filter(|k| *k != root).collect();
    assert_eq!(shown, exp_desc[&root]);

    // Filter is_root
    let roots = exp_parent.values().filter(|p| p.is_none()).count();
    let (_, r) = call(&app, "GET", "/api/tags/?is_root=true&page_size=100000", &token, None).await;
    assert_eq!(r["count"], json!(roots));

    // Tag.clean(): own parent, descendant, depth
    let leaf_deepest = exp_parent.keys().copied().max_by_key(|k| tree.ancestors(*k).len()).unwrap();
    let (st, e) = call(&app, "PATCH", &format!("/api/tags/{root}/"), &token, Some(json!({"parent": root}))).await;
    assert_eq!((st, e), (400, json!({"parent": ["Cannot set itself as parent."]})));
    let (st, e) = call(&app, "PATCH", &format!("/api/tags/{root}/"), &token, Some(json!({"parent": leaf_deepest}))).await;
    assert_eq!((st, e), (400, json!({"parent": ["Cannot set parent to a descendant."]})));
    // fill the chain up to level 5, then level 6 is forbidden
    let mut parent = leaf_deepest;
    let mut level = tree.ancestors(leaf_deepest).len() + 1;
    while level < 5 {
        let (st, t) = call(&app, "POST", "/api/tags/", &token, Some(json!({"name": format!("lvl{level}"), "parent": parent}))).await;
        assert_eq!(st, 201, "{t}");
        assert_eq!(t["parent"], json!(parent));
        parent = t["id"].as_i64().unwrap();
        level += 1;
    }
    let (st, e) = call(&app, "POST", "/api/tags/", &token, Some(json!({"name": "too-deep", "parent": parent}))).await;
    assert_eq!((st, e), (400, json!({"parent": ["Maximum nesting depth exceeded."]})));

    // parent change: documents of the tag get the new parent + its ancestors
    let (st, moved) = call(&app, "POST", "/api/tags/", &token, Some(json!({"name": "move-me"}))).await;
    assert_eq!(st, 201);
    let moved = moved["id"].as_i64().unwrap();
    let doc: i32 =
        sqlx::query_scalar("SELECT id FROM documents_document WHERE deleted_at IS NULL ORDER BY id LIMIT 1").fetch_one(&db).await.unwrap();
    sqlx::query("INSERT INTO documents_document_tags (document_id, tag_id) VALUES ($1, $2)")
        .bind(doc)
        .bind(moved as i32)
        .execute(&db)
        .await
        .unwrap();
    let target = leaf_deepest;
    let (st, t) = call(&app, "PATCH", &format!("/api/tags/{moved}/"), &token, Some(json!({"parent": target}))).await;
    assert_eq!(st, 200, "{t}");
    assert_eq!(t["parent"], json!(target));
    let have: Vec<i32> =
        sqlx::query_scalar("SELECT tag_id FROM documents_document_tags WHERE document_id = $1").bind(doc).fetch_all(&db).await.unwrap();
    let have: BTreeSet<i64> = have.into_iter().map(i64::from).collect();
    for a in std::iter::once(target).chain(tree.ancestors(target)) {
        assert!(have.contains(&a), "document {doc} misses ancestor {a}");
    }
    // bulk_update_documents for exactly this document
    let sent: Vec<TaskRequest> = rec.0.lock().map(|v| v.clone()).unwrap_or_default();
    assert!(sent.iter().any(|t| t.name == "documents.tasks.bulk_update_documents" && t.kwargs["document_ids"] == json!([doc])), "{sent:?}");
    // the new parent shows the child
    let (_, p) = call(&app, "GET", &format!("/api/tags/{target}/"), &token, None).await;
    assert!(p["children"].as_array().unwrap().iter().any(|c| c["id"] == json!(moved)));

    // deleting cascades via the parent edge
    let (st, _) = call(&app, "DELETE", &format!("/api/tags/{target}/"), &token, None).await;
    assert_eq!(st, 204);
    let gone: i64 =
        sqlx::query_scalar("SELECT count(*) FROM documents_tag WHERE id = ANY($1)").bind(vec![target, moved]).fetch_one(&db).await.unwrap();
    assert_eq!(gone, 0);

    drop_db(db).await;
}
