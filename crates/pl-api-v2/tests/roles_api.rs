//! Roles `/rs/api/v2/roles` (docs/DESIGN2-ENTWIRREN.md §1) against a **dedicated copy** of the
//! reference DB (never `paperless`):
//! ```text
//! psql -h 127.0.0.1 -p 54329 -U paperless -d postgres -c "CREATE DATABASE pl_roles_api_test TEMPLATE paperless_snapshot"
//! PAPERRUST_ROLES_TEST_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/pl_roles_api_test \
//!   cargo test -p pl-api-v2 --test roles_api -j 2 -- --ignored --test-threads 1
//! ```

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use pl_core::AppState;
use pl_core::config::{Config, Env};
use serde_json::{Value, json};
use tower::ServiceExt;

const SU_TOKEN: &str = "plrolesapitestsu000000000000000000000000";
const MGR_TOKEN: &str = "plrolesapitestmg000000000000000000000000";
const PLAIN_TOKEN: &str = "plrolesapitestpl000000000000000000000000";

fn url() -> String {
    let url = std::env::var("PAPERRUST_ROLES_TEST_DATABASE_URL").expect("PAPERRUST_ROLES_TEST_DATABASE_URL");
    let db = url.trim_end_matches('/').rsplit('/').next().unwrap_or("").split('?').next().unwrap_or("").to_string();
    assert!(db.starts_with("pl_roles"), "refusing to run against a non-test database ({db})");
    url
}

struct Ids {
    su: i64,
    mgr: i64,
    plain: i64,
    other_su: i64,
    group: i64,
}

async fn user(pool: &sqlx::PgPool, name: &str, su: bool) -> i64 {
    let id: i32 = sqlx::query_scalar(
        "INSERT INTO auth_user (password, is_superuser, username, first_name, last_name, email, is_staff, is_active, date_joined)
         VALUES ('!', $2, $1, '', '', '', $2, true, now()) RETURNING id",
    )
    .bind(name)
    .bind(su)
    .fetch_one(pool)
    .await
    .unwrap();
    i64::from(id)
}

async fn setup() -> (Router, sqlx::PgPool, Ids) {
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(4).connect(&url()).await.expect("connect");
    sqlx::raw_sql(pl_api_v2::roles::MIGRATION.1).execute(&pool).await.unwrap();
    sqlx::raw_sql(
        "DELETE FROM paperrust_role WHERE NOT builtin; DELETE FROM paperrust_role_assignment;
         UPDATE paperrust_role_config SET mode = 'classic';
         DELETE FROM authtoken_token WHERE key LIKE 'plrolesapitest%';
         DELETE FROM auth_user_user_permissions WHERE user_id IN (SELECT id FROM auth_user WHERE username LIKE 'pl_roles_api_%');
         DELETE FROM auth_user WHERE username LIKE 'pl_roles_api_%';
         DELETE FROM auth_group_permissions WHERE group_id IN (SELECT id FROM auth_group WHERE name = 'pl_roles_api_team');
         DELETE FROM auth_group WHERE name = 'pl_roles_api_team';",
    )
    .execute(&pool)
    .await
    .unwrap();
    let ids = Ids {
        su: user(&pool, "pl_roles_api_su", true).await,
        mgr: user(&pool, "pl_roles_api_mgr", false).await,
        plain: user(&pool, "pl_roles_api_plain", false).await,
        other_su: user(&pool, "pl_roles_api_su2", true).await,
        group: i64::from(
            sqlx::query_scalar::<_, i32>("INSERT INTO auth_group (name) VALUES ('pl_roles_api_team') RETURNING id")
                .fetch_one(&pool)
                .await
                .unwrap(),
        ),
    };
    // mgr: may see/change users and has the permissions of "Read only"
    let mut mgr_perms: Vec<String> = pl_api_v2::roles::READ.iter().map(|s| s.to_string()).collect();
    mgr_perms.extend(["auth.view_user".to_string(), "auth.change_user".to_string()]);
    sqlx::query(
        "INSERT INTO auth_user_user_permissions (user_id, permission_id)
         SELECT $1, p.id FROM auth_permission p JOIN django_content_type ct ON ct.id = p.content_type_id
         WHERE ct.app_label || '.' || p.codename = ANY($2)",
    )
    .bind(ids.mgr as i32)
    .bind(&mgr_perms)
    .execute(&pool)
    .await
    .unwrap();
    for (k, u) in [(SU_TOKEN, ids.su), (MGR_TOKEN, ids.mgr), (PLAIN_TOKEN, ids.plain)] {
        sqlx::query("INSERT INTO authtoken_token (key, created, user_id) VALUES ($1, now(), $2)")
            .bind(k)
            .bind(u as i32)
            .execute(&pool)
            .await
            .unwrap();
    }
    let cfg = Config::from_env_map(Env::from_pairs(vec![
        ("PAPERLESS_SECRET_KEY", "pl-roles-api-tests-only-0123456789abcdefghijklmnop"),
        ("PAPERLESS_DBHOST", "127.0.0.1"),
    ]))
    .expect("config");
    let state = AppState::with_pool(cfg.clone(), pool.clone()).expect("state");
    let authz = pl_authz::Authz::new(pool.clone(), pl_authz::AuthConfig::from_core(&cfg));
    let r: Router<AppState> = pl_api_v2::roles::routes(Router::new());
    (r.with_state(state).layer(axum::Extension(authz)), pool, ids)
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

fn role_id(overview: &Value, key: &str) -> i64 {
    overview["roles"].as_array().unwrap().iter().find(|r| r["key"] == key).unwrap()["id"].as_i64().unwrap()
}

async fn flags(pool: &sqlx::PgPool, id: i64) -> (bool, bool) {
    sqlx::query_as("SELECT is_superuser, is_staff FROM auth_user WHERE id = $1").bind(id as i32).fetch_one(pool).await.unwrap()
}

#[tokio::test]
#[ignore = "braucht eine Kopie der Referenz-DB (PAPERRUST_ROLES_TEST_DATABASE_URL), siehe Kopfkommentar"]
async fn roles_end_to_end() {
    let (r, pool, ids) = setup().await;

    // Read: without view_user 403, with 200
    assert_eq!(call(&r, "GET", "/roles", PLAIN_TOKEN, None).await.0, StatusCode::FORBIDDEN);
    let (st, ov) = call(&r, "GET", "/roles", MGR_TOKEN, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ov["mode"], "classic");
    assert_eq!(ov["roles"].as_array().unwrap().len(), 4);
    assert_eq!(ov["topics"].as_array().unwrap().len(), 5);
    let (read, edit, admin) = (role_id(&ov, "read"), role_id(&ov, "edit"), role_id(&ov, "admin"));

    // mgr has READ ∪ view/change_user → individual; superuser → admin
    let (_, res) = call(&r, "GET", &format!("/roles/resolve?users={},{}", ids.mgr, ids.su), MGR_TOKEN, None).await;
    assert_eq!(res["users"][ids.mgr.to_string()], "individual");
    assert_eq!(res["users"][ids.su.to_string()], admin);

    // Assign "Read only" to plain + group (superuser)
    let (st, out) =
        call(&r, "POST", &format!("/roles/{read}/assign"), SU_TOKEN, Some(json!({"users":[ids.plain],"groups":[ids.group]}))).await;
    assert_eq!(st, StatusCode::OK, "{out}");
    assert_eq!(out["users"][0]["role_id"], read);
    let (_, res) = call(&r, "GET", &format!("/roles/resolve?users={}&groups={}", ids.plain, ids.group), SU_TOKEN, None).await;
    assert_eq!(res["users"][ids.plain.to_string()], read);
    assert_eq!(res["groups"][ids.group.to_string()], read);
    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_user_user_permissions up JOIN auth_permission p ON p.id = up.permission_id
         WHERE up.user_id = $1 AND p.codename = 'view_uisettings'",
    )
    .bind(ids.plain as i32)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1, "always mitgegeben");

    // mgr (change_user, only READ permissions): "Edit" would be privilege escalation → 403; admin → 403
    assert_eq!(
        call(&r, "POST", &format!("/roles/{edit}/assign"), MGR_TOKEN, Some(json!({"users":[ids.plain]}))).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&r, "POST", &format!("/roles/{admin}/assign"), MGR_TOKEN, Some(json!({"users":[ids.plain]}))).await.0,
        StatusCode::FORBIDDEN
    );

    // Admin role: flags; for a group 400
    let (st, _) = call(&r, "POST", &format!("/roles/{admin}/assign"), SU_TOKEN, Some(json!({"users":[ids.plain]}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(flags(&pool, ids.plain).await, (true, true));
    let (st, _) = call(&r, "POST", &format!("/roles/{admin}/assign"), SU_TOKEN, Some(json!({"groups":[ids.group]}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // Demoting another superuser; never oneself
    let (st, _) = call(&r, "POST", &format!("/roles/{edit}/assign"), SU_TOKEN, Some(json!({"users":[ids.other_su]}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(flags(&pool, ids.other_su).await, (false, false));
    let (st, e) = call(&r, "POST", &format!("/roles/{edit}/assign"), SU_TOKEN, Some(json!({"users":[ids.su]}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(e["error"]["message"], "You cannot remove your own administrator role");
    assert_eq!(flags(&pool, ids.su).await, (true, true));

    // Unknown ids → 400, nothing changed
    let (st, _) = call(&r, "POST", &format!("/roles/{read}/assign"), SU_TOKEN, Some(json!({"users":[ids.other_su, 999_999]}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (_, res) = call(&r, "GET", &format!("/roles/resolve?users={}", ids.other_su), SU_TOKEN, None).await;
    assert_eq!(res["users"][ids.other_su.to_string()], edit);

    // Custom roles: in classic mode 400, mode only superuser
    let body = json!({"name":"Buchhaltung","description":"x","permissions":["documents.view_document","documents.view_tag"]});
    assert_eq!(call(&r, "POST", "/roles", SU_TOKEN, Some(body.clone())).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&r, "PUT", "/roles/mode", MGR_TOKEN, Some(json!({"mode":"custom"}))).await.0, StatusCode::FORBIDDEN);
    let (st, ov) = call(&r, "PUT", "/roles/mode", SU_TOKEN, Some(json!({"mode":"custom"}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(ov["mode"], "custom");
    let (st, role) = call(&r, "POST", "/roles", SU_TOKEN, Some(body.clone())).await;
    assert_eq!(st, StatusCode::CREATED, "{role}");
    let cid = role["id"].as_i64().unwrap();
    assert_eq!(role["builtin"], false);
    assert_eq!(call(&r, "POST", "/roles", SU_TOKEN, Some(body)).await.0, StatusCode::BAD_REQUEST, "doppelter Name");
    let (st, _) = call(&r, "POST", "/roles", SU_TOKEN, Some(json!({"name":"X","permissions":["documents.fly_document"]}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, role) = call(&r, "PATCH", &format!("/roles/{cid}"), SU_TOKEN, Some(json!({"description":"Belege"}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(role["description"], "Belege");
    assert_eq!(call(&r, "PATCH", &format!("/roles/{read}"), SU_TOKEN, Some(json!({"name":"Y"}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&r, "DELETE", &format!("/roles/{read}"), SU_TOKEN, None).await.0, StatusCode::BAD_REQUEST);
    let (st, _) = call(&r, "POST", &format!("/roles/{cid}/assign"), SU_TOKEN, Some(json!({"users":[ids.other_su]}))).await;
    assert_eq!(st, StatusCode::OK);
    let (_, res) = call(&r, "GET", &format!("/roles/resolve?users={}", ids.other_su), SU_TOKEN, None).await;
    assert_eq!(res["users"][ids.other_su.to_string()], cid);
    assert_eq!(call(&r, "DELETE", &format!("/roles/{cid}"), SU_TOKEN, None).await.0, StatusCode::NO_CONTENT);
    let (_, res) = call(&r, "GET", &format!("/roles/resolve?users={}", ids.other_su), SU_TOKEN, None).await;
    assert_eq!(res["users"][ids.other_su.to_string()], "individual");

    // clean up
    sqlx::raw_sql("UPDATE paperrust_role_config SET mode = 'classic'").execute(&pool).await.unwrap();
}
