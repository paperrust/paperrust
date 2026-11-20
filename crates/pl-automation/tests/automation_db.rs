//! Automation API and workflow extensions against an **own throwaway copy** of the reference DB
//! (`pl_automation_test` from `paperless_snapshot`; never `paperless`, `paperless_snapshot`, `pl_scale_1m`):
//!
//! ```text
//! PAPERRUST_AUTOMATION_TEST_SERVER=postgres://paperless:paperless@127.0.0.1:54329 \
//!   cargo test -p pl-automation -j 2 --test automation_db -- --ignored --test-threads 1
//! ```
//! Creates `pl_automation_test` anew (`DROP … WITH (FORCE)` beforehand) and deletes it again at the end.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use pl_core::AppState;
use pl_core::config::{Config, Env};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

const DB: &str = "pl_automation_test";
const ADMIN: &str = "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001";
const ALICE: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002";
const CAROL: &str = "c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c004";

fn server() -> String {
    std::env::var("PAPERRUST_AUTOMATION_TEST_SERVER").expect("PAPERRUST_AUTOMATION_TEST_SERVER").trim_end_matches('/').to_string()
}

async fn fresh_db() -> PgPool {
    // "Tables ready" applies per process; every throwaway DB starts without them
    pl_consumer::workflow_ext::forget_ready_for_tests();
    let admin = PgPool::connect(&format!("{}/postgres", server())).await.expect("connect postgres");
    drop_force(&admin).await.expect("drop");
    sqlx::query(&format!("CREATE DATABASE {DB} TEMPLATE paperless_snapshot")).execute(&admin).await.expect("create");
    admin.close().await;
    sqlx::postgres::PgPoolOptions::new().max_connections(6).connect(&format!("{}/{DB}", server())).await.expect("connect test db")
}

async fn drop_db(pool: PgPool) {
    pool.close().await;
    let admin = PgPool::connect(&format!("{}/postgres", server())).await.expect("connect postgres");
    drop_force(&admin).await.expect("drop");
}

/// `DROP … WITH (FORCE)` as the non-superuser `paperless` fails with 42501 while an autovacuum
/// worker (superuser backend) is busy in the fresh copy; that is transient, so retry for a while
/// instead of failing (or, as before in `drop_db`, silently leaving the DB behind for the next test).
async fn drop_force(admin: &PgPool) -> Result<(), sqlx::Error> {
    let sql = format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)");
    let mut tries = 0;
    loop {
        match sqlx::query(&sql).execute(admin).await {
            Ok(_) => return Ok(()),
            Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("42501") && tries < 60 => {
                tries += 1;
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            Err(e) => return Err(e),
        }
    }
}

fn app(pool: &PgPool) -> Router {
    let cfg = Config::from_env_map(Env::from_pairs(vec![
        ("PAPERLESS_SECRET_KEY", "pl-automation-tests-only-0123456789abcdefghijklmnop"),
        ("PAPERLESS_DBHOST", "127.0.0.1"),
        ("PAPERLESS_TIME_ZONE", "Europe/Zurich"),
    ]))
    .expect("config");
    let state = AppState::with_pool(cfg.clone(), pool.clone()).expect("state");
    let authz = pl_authz::Authz::new(pool.clone(), pl_authz::AuthConfig::from_core(&cfg));
    Router::new().nest(pl_automation::PREFIX, pl_automation::router()).with_state(state).layer(axum::Extension(authz))
}

async fn call(r: &Router, method: &str, path: &str, token: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut b = Request::builder()
        .method(method)
        .uri(format!("{}{path}", pl_automation::PREFIX))
        .header("host", "localhost")
        .header("authorization", format!("Token {token}"));
    let body = match body {
        Some(v) => {
            b = b.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = r.clone().oneshot(b.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 26).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn scalar_i64(pool: &PgPool, sql: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(sql).fetch_one(pool).await.unwrap()
}

struct Fixture {
    tag_x: i64,
    tag_y: i64,
    corr: i64,
}

async fn fixture(pool: &PgPool) -> Fixture {
    // Delete seed workflows so that only ours run
    sqlx::query("UPDATE documents_workflow SET enabled = false").execute(pool).await.unwrap();
    let tag_x = scalar_i64(
        pool,
        "SELECT t.tag_id::bigint FROM documents_document_tags t JOIN documents_document d ON d.id = t.document_id \
         JOIN documents_tag g ON g.id = t.tag_id WHERE d.deleted_at IS NULL AND d.root_document_id IS NULL AND NOT g.is_inbox_tag \
         GROUP BY t.tag_id HAVING count(*) BETWEEN 5 AND 120 ORDER BY count(*) DESC, t.tag_id LIMIT 1",
    )
    .await;
    let tag_y = scalar_i64(pool, &format!("SELECT id::bigint FROM documents_tag WHERE id <> {tag_x} AND tn_parent_id IS NULL AND NOT is_inbox_tag ORDER BY id LIMIT 1")).await;
    let corr = scalar_i64(pool, "SELECT id::bigint FROM documents_correspondent WHERE owner_id IS NULL ORDER BY id LIMIT 1").await;
    Fixture { tag_x, tag_y, corr }
}

fn workflow_body(name: &str, f: &Fixture) -> Value {
    json!({
        "name": name,
        "order": 1,
        "enabled": true,
        "triggers": [{"type": 3, "filter_has_tags": [f.tag_x], "matching_algorithm": 0, "match": "", "is_insensitive": true}],
        "actions": [{"type": 1, "assign_correspondent": f.corr}],
        "else_actions": [{"type": 1, "assign_tags": [f.tag_y]}],
        "ai_condition": null,
    })
}

#[tokio::test]
#[ignore = "braucht die Referenz-PostgreSQL (PAPERRUST_AUTOMATION_TEST_SERVER), siehe Kopfkommentar"]
async fn automation_end_to_end() {
    let pool = fresh_db().await;
    pl_consumer::workflow_ext::install_ai_judge(None);
    let r = app(&pool);
    let f = fixture(&pool).await;

    // --- Permissions
    assert_eq!(call(&r, "GET", "/workflows", CAROL, None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&r, "GET", "/workflows", "nope0000000000000000000000000000000000000", None).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&r, "POST", "/workflows", ALICE, Some(workflow_body("x", &f))).await.0, StatusCode::FORBIDDEN);

    // --- Validation like /api/workflows/ (DRF errors under fields)
    let (s, b) = call(&r, "POST", "/workflows", ADMIN, Some(json!({"name": "bad", "order": 0, "enabled": true, "triggers": [{"type": 99}], "actions": []}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    assert!(b["error"]["fields"]["triggers"].is_object(), "{b}");
    let (s, b) = call(&r, "POST", "/workflows", ADMIN, Some(json!({"name": "bad2", "order": 0, "enabled": true,
        "triggers": [{"type": 3}], "actions": [], "else_actions": [{"type": 77}]}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    assert!(b["error"]["fields"]["else_actions"].is_object(), "{b}");
    // nothing half-saved
    assert_eq!(scalar_i64(&pool, "SELECT count(*) FROM documents_workflow WHERE name LIKE 'bad%'").await, 0);

    // --- Create (together)
    let (s, b) = call(&r, "POST", "/workflows", ADMIN, Some(workflow_body("Auto test", &f))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let wf_id = b["workflow"]["id"].as_i64().unwrap();
    assert_eq!(b["else_actions"].as_array().unwrap().len(), 1);
    assert_eq!(b["summary"]["triggers"][0]["conditions"][0]["kind"], "tags_any");
    assert_eq!(b["summary"]["actions"][0]["items"][0]["kind"], "assign_correspondent");
    assert_eq!(b["summary"]["else_actions"][0]["items"][0]["kind"], "assign_tags");
    let else_action = b["else_actions"][0]["id"].as_i64().unwrap();

    // Overview (alice may read)
    let (s, b) = call(&r, "GET", "/workflows", ALICE, None).await;
    assert_eq!(s, StatusCode::OK);
    let item = b["items"].as_array().unwrap().iter().find(|i| i["id"] == wf_id).cloned().unwrap();
    assert_eq!(item["has_else"], true);
    assert_eq!(item["runs_this_week"], 0);

    // --- Dry run: nothing may remain
    let wfrun_before = scalar_i64(&pool, "SELECT count(*) FROM documents_workflowrun").await;
    let audit_before = scalar_i64(&pool, "SELECT count(*) FROM auditlog_logentry").await;
    let (s, dr) = call(&r, "POST", "/dry-run", ADMIN, Some(json!({"workflow_id": wf_id, "limit": 200}))).await;
    assert_eq!(s, StatusCode::OK, "{dr}");
    assert_eq!(dr["trigger_type"], 3);
    let main_n = dr["counts"]["main"].as_i64().unwrap();
    let else_n = dr["counts"]["else"].as_i64().unwrap();
    assert!(main_n > 0 && else_n > 0, "{}", dr["counts"]);
    let main_doc = dr["documents"].as_array().unwrap().iter().find(|d| d["result"] == "main").cloned().unwrap();
    let else_doc = dr["documents"].as_array().unwrap().iter().find(|d| d["result"] == "else").cloned().unwrap();
    assert!(else_doc["reason"].as_str().is_some(), "{else_doc}");
    assert_eq!(scalar_i64(&pool, "SELECT count(*) FROM documents_workflowrun").await, wfrun_before);
    assert_eq!(scalar_i64(&pool, "SELECT count(*) FROM auditlog_logentry").await, audit_before);
    assert_eq!(scalar_i64(&pool, &format!("SELECT count(*) FROM paperrust_workflow_run WHERE workflow_id = {wf_id}")).await, 0);
    let main_id = main_doc["id"].as_i64().unwrap();
    let else_id = else_doc["id"].as_i64().unwrap();
    let corr_now: Option<i32> = sqlx::query_scalar("SELECT correspondent_id FROM documents_document WHERE id = $1").bind(main_id as i32).fetch_one(&pool).await.unwrap();
    assert_eq!(main_doc["before"]["correspondent"]["id"].as_i64(), corr_now.map(i64::from));

    // --- Equality dry run ↔ real execution (same core, same state afterwards)
    let mut conn = pool.acquire().await.unwrap();
    let wf = pl_consumer::workflows::workflow_by_id(&mut conn, wf_id).await.unwrap().unwrap();
    assert_eq!(wf.else_actions.len(), 1);
    let opts = pl_consumer::workflow_dryrun::DryRunOptions {
        trigger_type: 3,
        ai_mode: pl_consumer::workflow_ext::AiMode::NotChecked,
        time_zone: chrono_tz::Europe::Zurich,
        source: None,
        mailrule_id: None,
    };
    let mut tx = sqlx::Connection::begin(&mut *conn).await.unwrap();
    let dry = pl_consumer::workflow_dryrun::dry_run(&mut tx, &wf, &[main_id, else_id], &opts).await.unwrap();
    tx.rollback().await.unwrap();
    let audit = pl_consumer::audit::AuditLog { enabled: true, actor_id: None };
    let effects = pl_consumer::native::LoggingEffects;
    let env = pl_consumer::workflows::RunEnv {
        audit: &audit,
        effects: &effects,
        time_zone: chrono_tz::Europe::Zurich,
        original_file: None,
        settings: None,
        deleter: &pl_consumer::workflows::NativeSoftDelete,
        ext: pl_consumer::workflow_ext::ExtOptions { ai_mode: pl_consumer::workflow_ext::AiMode::Inline, log_skips: true },
    };
    for (i, id) in [main_id, else_id].into_iter().enumerate() {
        let row = pl_consumer::db::load_document(&mut conn, id).await.unwrap().unwrap();
        let mut doc = pl_consumer::docops::DocInstance::from_db(row);
        let out = pl_consumer::workflows::run_workflow_as(&mut conn, &env, &mut doc, &wf, false, 3).await.unwrap();
        assert_eq!(out.runs.len(), 1);
        let real = pl_consumer::workflow_dryrun::doc_state(&mut conn, id).await.unwrap().unwrap();
        assert_eq!(dry[i].after.as_ref(), Some(&real), "dry run and real run differ for document {id}");
    }
    drop(conn);

    // --- History
    let (s, runs) = call(&r, "GET", &format!("/workflows/{wf_id}/runs"), ALICE, None).await;
    assert_eq!(s, StatusCode::OK, "{runs}");
    let items = runs["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["branch"], "else");
    assert_eq!(items[0]["changed_fields"], json!(["tags"]));
    assert_eq!(items[1]["branch"], "main");
    assert_eq!(runs["counts"]["this_week"], 2);
    let (_, page) = call(&r, "GET", &format!("/workflows/{wf_id}/runs?limit=1"), ADMIN, None).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let cur = page["next_cursor"].as_str().unwrap().to_string();
    let (_, page2) = call(&r, "GET", &format!("/workflows/{wf_id}/runs?limit=1&cursor={cur}"), ADMIN, None).await;
    assert_eq!(page2["items"][0]["branch"], "main");
    assert!(page2["next_cursor"].is_null());
    assert_eq!(call(&r, "GET", "/workflows/999999/runs", ADMIN, None).await.0, StatusCode::NOT_FOUND);
    let (_, b) = call(&r, "GET", "/workflows", ADMIN, None).await;
    let item = b["items"].as_array().unwrap().iter().find(|i| i["id"] == wf_id).cloned().unwrap();
    assert_eq!(item["runs_this_week"], 2);
    assert_eq!(item["last_run"]["result"], "ok");

    // --- AI condition (without AI integration: saved, ignored and flagged)
    let dt = scalar_i64(&pool, "SELECT id::bigint FROM documents_documenttype ORDER BY id LIMIT 1").await;
    let (s, b) = call(&r, "PATCH", &format!("/workflows/{wf_id}"), ADMIN, Some(json!({"ai_condition": {"field": "document_type", "object_id": dt, "min_confidence": 0.8}}))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["ai_condition"]["min_confidence"], 0.8);
    assert_eq!(b["summary"]["ai_condition"]["active"], false);
    assert_eq!(b["summary"]["ai_condition"]["inactive_reason"], "ai_unavailable");
    assert_eq!(b["else_actions"][0]["id"], else_action, "PATCH without else_actions keeps them");
    let (s, b) = call(&r, "PATCH", &format!("/workflows/{wf_id}"), ADMIN, Some(json!({"ai_condition": {"field": "title", "object_id": 1}}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    assert!(b["error"]["fields"]["ai_condition"].is_array());
    let (s, b) = call(&r, "PATCH", &format!("/workflows/{wf_id}"), ADMIN, Some(json!({"ai_condition": {"field": "tags", "object_id": 999999}}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    let (s, dr) = call(&r, "POST", "/dry-run", ADMIN, Some(json!({"workflow_id": wf_id, "document_ids": [main_id, else_id]}))).await;
    assert_eq!(s, StatusCode::OK, "{dr}");
    assert!(dr["notes"].as_array().unwrap().contains(&json!("ai_condition_ignored")), "{dr}");

    // --- prune (saving via pl-workflows) leaves else actions in place
    let mut put_body = workflow_body("Auto test", &f);
    put_body["else_actions"][0]["id"] = json!(else_action);
    let (s, b) = call(&r, "PUT", &format!("/workflows/{wf_id}"), ADMIN, Some(put_body)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["else_actions"][0]["id"], else_action, "same else action id kept on PUT");
    // foreign action id (main action) is not hijacked
    let main_action = b["workflow"]["actions"][0]["id"].as_i64().unwrap();
    let mut hijack = workflow_body("Auto test", &f);
    hijack["actions"][0]["id"] = json!(main_action);
    hijack["else_actions"] = json!([{"id": else_action, "type": 1, "assign_tags": [f.tag_y]}, {"id": main_action, "type": 1, "assign_tags": [f.tag_y]}]);
    let (s, b) = call(&r, "PUT", &format!("/workflows/{wf_id}"), ADMIN, Some(hijack)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["workflow"]["actions"][0]["id"], main_action);
    assert_ne!(b["else_actions"][1]["id"], main_action);
    // back to one else action: the second one is deleted
    let mut back = workflow_body("Auto test", &f);
    back["actions"][0]["id"] = json!(main_action);
    back["else_actions"][0]["id"] = json!(else_action);
    let second = b["else_actions"][1]["id"].as_i64().unwrap();
    let (s, b) = call(&r, "PUT", &format!("/workflows/{wf_id}"), ADMIN, Some(back)).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["else_actions"].as_array().unwrap().len(), 1);
    assert_eq!(scalar_i64(&pool, &format!("SELECT count(*) FROM documents_workflowaction WHERE id = {second}")).await, 0);
    assert!(b["ai_condition"].is_null(), "PUT without ai_condition clears it");
    {
        let mut c = pool.acquire().await.unwrap();
        pl_workflows::write::prune(&mut c).await.unwrap();
    }
    assert_eq!(scalar_i64(&pool, &format!("SELECT count(*) FROM documents_workflowaction WHERE id = {else_action}")).await, 1);

    // --- Test draft (unsaved), permissions
    let draft = json!({"workflow": workflow_body("Draft", &f), "limit": 20});
    assert_eq!(call(&r, "POST", "/dry-run", ALICE, Some(draft.clone())).await.0, StatusCode::FORBIDDEN);
    let wf_count = scalar_i64(&pool, "SELECT count(*) FROM documents_workflow").await;
    let (s, dr) = call(&r, "POST", "/dry-run", ADMIN, Some(draft)).await;
    assert_eq!(s, StatusCode::OK, "{dr}");
    assert!(dr["workflow_id"].is_null());
    assert_eq!(scalar_i64(&pool, "SELECT count(*) FROM documents_workflow").await, wf_count, "draft not persisted");
    let (s, _) = call(&r, "POST", "/dry-run", ADMIN, Some(json!({"workflow_id": wf_id, "trigger_type": 4}))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    // --- apply retroactively: without queue 503, without permissions 403
    assert_eq!(call(&r, "POST", &format!("/workflows/{wf_id}/apply"), ADMIN, Some(json!({"document_ids": [main_id]}))).await.0, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(call(&r, "POST", &format!("/workflows/{wf_id}/apply"), CAROL, Some(json!({"document_ids": [main_id]}))).await.0, StatusCode::FORBIDDEN);

    // --- Way back: else branch as inverted workflow
    let (s, ex) = call(&r, "GET", "/export-ngx", ADMIN, None).await;
    assert_eq!(s, StatusCode::OK, "{ex}");
    let w = ex["workflows"].as_array().unwrap().iter().find(|w| w["source_workflow_id"] == wf_id).cloned().unwrap();
    assert_eq!(w["workflow"]["triggers"][0]["filter_has_not_tags"], json!([f.tag_x]));
    assert_eq!(w["workflow"]["actions"][0]["assign_tags"], json!([f.tag_y]));

    // --- Suggestions from the archive + draft (create pattern safely: correspondent → always type dt)
    sqlx::query("UPDATE documents_document SET document_type_id = $1 WHERE correspondent_id = $2 AND root_document_id IS NULL")
        .bind(dt as i32)
        .bind(f.corr as i32)
        .execute(&pool)
        .await
        .unwrap();
    let (s, sg) = call(&r, "GET", "/suggestions?min_documents=3&min_share=0.5", ADMIN, None).await;
    assert_eq!(s, StatusCode::OK, "{sg}");
    assert_eq!(sg["cached"], false);
    let want = format!("correspondent_document_type:{}:{dt}", f.corr);
    let hit = sg["items"].as_array().unwrap().iter().find(|i| i["key"] == want.as_str()).cloned().unwrap_or_else(|| panic!("{want} missing in {sg}"));
    assert_eq!(hit["matches"], hit["total"]);
    assert_eq!(hit["share"], 1.0);
    assert!(hit["to"]["name"].is_string());
    let (_, sg2) = call(&r, "GET", "/suggestions?min_documents=3&min_share=0.5", ADMIN, None).await;
    assert_eq!(sg2["cached"], true);
    if let Some(first) = sg["items"].as_array().unwrap().first() {
        assert!(first["matches"].as_i64().unwrap() <= first["total"].as_i64().unwrap());
        let (s, d) = call(&r, "POST", "/suggestions/draft", ADMIN, Some(json!({"key": first["key"]}))).await;
        assert_eq!(s, StatusCode::OK, "{d}");
        assert_eq!(d["workflow"]["enabled"], false);
        // the draft is a valid ngx workflow
        let mut wfd = d["workflow"].clone();
        wfd["name"] = json!("from suggestion");
        let (s, b) = call(&r, "POST", "/workflows", ADMIN, Some(wfd)).await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
    }
    assert_eq!(call(&r, "POST", "/suggestions/draft", ADMIN, Some(json!({"key": "correspondent_tag:999999:1"}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&r, "GET", "/suggestions", CAROL, None).await.0, StatusCode::FORBIDDEN);

    // --- Mail as trigger (rule + workflow together)
    let rule_id = scalar_i64(&pool, "SELECT id::bigint FROM paperless_mail_mailrule ORDER BY id LIMIT 1").await;
    let (s, mf) = call(&r, "GET", "/mail-flows", ADMIN, None).await;
    assert_eq!(s, StatusCode::OK, "{mf}");
    let (s, b) = call(
        &r,
        "PATCH",
        &format!("/mail-flows/{rule_id}"),
        ADMIN,
        Some(json!({"mail_rule": {"name": "Mail flow rule"}, "workflows": [{"name": "From mail", "order": 0, "enabled": true,
            "triggers": [{"type": 2}], "actions": [{"type": 1, "assign_tags": [f.tag_y]}]}]})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["mail_rule"]["name"], "Mail flow rule");
    let flow_wf = b["workflows"].as_array().unwrap().iter().find(|w| w["workflow"]["name"] == "From mail").cloned().unwrap();
    let cons = flow_wf["workflow"]["triggers"].as_array().unwrap().iter().find(|t| t["type"] == 1).cloned().unwrap();
    assert_eq!(cons["filter_mailrule"], rule_id);
    // Error in the workflow → nothing saved (not the rule either)
    let (s, b) = call(
        &r,
        "PATCH",
        &format!("/mail-flows/{rule_id}"),
        ADMIN,
        Some(json!({"mail_rule": {"name": "should not stick"}, "workflows": [{"name": "Broken", "order": 0, "enabled": true, "triggers": [{"type": 99}], "actions": []}]})),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    assert!(b["error"]["fields"]["workflows"]["0"].is_object(), "{b}");
    let name: String = sqlx::query_scalar("SELECT name FROM paperless_mail_mailrule WHERE id = $1").bind(rule_id as i32).fetch_one(&pool).await.unwrap();
    assert_eq!(name, "Mail flow rule");
    assert_eq!(call(&r, "GET", &format!("/mail-flows/{rule_id}"), CAROL, None).await.0, StatusCode::FORBIDDEN);

    // --- Cleanup of the history
    assert_eq!(call(&r, "POST", "/runs/cleanup", ALICE, Some(json!({}))).await.0, StatusCode::FORBIDDEN);
    let (s, b) = call(&r, "POST", "/runs/cleanup", ADMIN, Some(json!({"older_than_days": 1}))).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["deleted"], 0);
    sqlx::query("UPDATE paperrust_workflow_run SET run_at = now() - interval '10 days'").execute(&pool).await.unwrap();
    let (_, b) = call(&r, "POST", "/runs/cleanup", ADMIN, Some(json!({"older_than_days": 5}))).await;
    assert_eq!(b["deleted"], 2);

    // --- Deleting via pl-workflows takes else actions along
    {
        let mut c = pool.acquire().await.unwrap();
        pl_workflows::write::delete_workflow(&mut c, wf_id).await.unwrap();
    }
    assert_eq!(scalar_i64(&pool, &format!("SELECT count(*) FROM documents_workflowaction WHERE id = {else_action}")).await, 0);
    assert_eq!(scalar_i64(&pool, &format!("SELECT count(*) FROM paperrust_workflow_else_action WHERE workflow_id = {wf_id}")).await, 0);
    assert_eq!(call(&r, "GET", &format!("/workflows/{wf_id}"), ADMIN, None).await.0, StatusCode::NOT_FOUND);

    drop(r);
    drop_db(pool).await;
}

async fn load_doc(c: &mut sqlx::PgConnection, id: i64) -> pl_consumer::docops::DocInstance {
    pl_consumer::docops::DocInstance::from_db(pl_consumer::db::load_document(c, id).await.unwrap().unwrap())
}

/// Fake AI for the decision in the real core (caught up / immediate).
struct FakeJudge(bool);

impl pl_consumer::workflow_ext::AiJudge for FakeJudge {
    fn enabled(&self) -> futures::future::BoxFuture<'_, bool> {
        Box::pin(async { true })
    }
    fn judge<'a>(&'a self, _d: i64, _c: &'a pl_consumer::workflow_ext::AiConditionDef) -> futures::future::BoxFuture<'a, Result<pl_consumer::workflow_ext::AiJudgement, String>> {
        let m = self.0;
        Box::pin(async move { Ok(pl_consumer::workflow_ext::AiJudgement { matched: m, value: Some("X".into()), confidence: Some(0.95) }) })
    }
}

#[tokio::test]
#[ignore = "braucht die Referenz-PostgreSQL (PAPERRUST_AUTOMATION_TEST_SERVER), siehe Kopfkommentar"]
async fn ai_condition_in_real_core() {
    let pool = fresh_db().await;
    let r = app(&pool);
    let f = fixture(&pool).await;
    let dt = scalar_i64(&pool, "SELECT id::bigint FROM documents_documenttype ORDER BY id LIMIT 1").await;
    let mut body = workflow_body("AI wf", &f);
    body["ai_condition"] = json!({"field": "document_type", "object_id": dt});
    let (s, b) = call(&r, "POST", "/workflows", ADMIN, Some(body)).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let wf_id = b["workflow"]["id"].as_i64().unwrap();
    let doc = scalar_i64(&pool, &format!("SELECT document_id::bigint FROM documents_document_tags WHERE tag_id = {} ORDER BY document_id LIMIT 1", f.tag_x)).await;
    let mut conn = pool.acquire().await.unwrap();
    let wf = pl_consumer::workflows::workflow_by_id(&mut conn, wf_id).await.unwrap().unwrap();
    let audit = pl_consumer::audit::AuditLog { enabled: false, actor_id: None };
    let effects = pl_consumer::native::LoggingEffects;
    let run = |mode| pl_consumer::workflows::RunEnv {
        audit: &audit,
        effects: &effects,
        time_zone: chrono_tz::UTC,
        original_file: None,
        settings: None,
        deleter: &pl_consumer::workflows::NativeSoftDelete,
        ext: pl_consumer::workflow_ext::ExtOptions { ai_mode: mode, log_skips: true },
    };
    // Defer: nothing executed, one run to be caught up
    pl_consumer::workflow_ext::install_ai_judge(Some(Arc::new(FakeJudge(true))));
    let mut d = load_doc(&mut conn, doc).await;
    let out = pl_consumer::workflows::run_workflow_as(&mut conn, &run(pl_consumer::workflow_ext::AiMode::Defer), &mut d, &wf, false, 3).await.unwrap();
    assert_eq!(out.deferred.len(), 1);
    assert!(out.runs.is_empty());
    let kw = pl_consumer::workflows::deferred_kwargs(&out.deferred[0]);
    assert_eq!(kw["match_trigger"], 3);
    // Inline, AI says no → else branch
    pl_consumer::workflow_ext::install_ai_judge(Some(Arc::new(FakeJudge(false))));
    let mut d = load_doc(&mut conn, doc).await;
    let out = pl_consumer::workflows::run_workflow_as(&mut conn, &run(pl_consumer::workflow_ext::AiMode::Inline), &mut d, &wf, false, 3).await.unwrap();
    assert_eq!(out.runs[0].branch, Some(pl_consumer::workflow_ext::Branch::Else));
    assert_eq!(out.runs[0].code, "ai_not_met_else");
    // Inline, AI says yes → main branch
    pl_consumer::workflow_ext::install_ai_judge(Some(Arc::new(FakeJudge(true))));
    let mut d = load_doc(&mut conn, doc).await;
    let out = pl_consumer::workflows::run_workflow_as(&mut conn, &run(pl_consumer::workflow_ext::AiMode::Inline), &mut d, &wf, false, 3).await.unwrap();
    assert_eq!(out.runs[0].branch, Some(pl_consumer::workflow_ext::Branch::Main));
    assert_eq!(out.runs[0].code, "ai_met");
    pl_consumer::workflow_ext::install_ai_judge(None);
    drop(conn);
    drop(r);
    drop_db(pool).await;
}

/// API path "document updated" (pl-documents-write `run_updated`): same decision, else branch,
/// history, AI condition is caught up (Defer). In a transaction that is rolled back.
#[tokio::test]
#[ignore = "braucht die Referenz-PostgreSQL (PAPERRUST_AUTOMATION_TEST_SERVER), siehe Kopfkommentar"]
async fn api_update_path_uses_same_decision() {
    let pool = fresh_db().await;
    let r = app(&pool);
    let f = fixture(&pool).await;
    pl_consumer::workflow_ext::install_ai_judge(None);
    let (s, b) = call(&r, "POST", "/workflows", ADMIN, Some(workflow_body("API path", &f))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let wf_id = b["workflow"]["id"].as_i64().unwrap();
    let with_x = scalar_i64(&pool, &format!("SELECT document_id::bigint FROM documents_document_tags WHERE tag_id = {} ORDER BY document_id LIMIT 1", f.tag_x)).await;
    let without_x = scalar_i64(
        &pool,
        &format!(
            "SELECT id::bigint FROM documents_document d WHERE root_document_id IS NULL AND deleted_at IS NULL              AND NOT EXISTS (SELECT 1 FROM documents_document_tags t WHERE t.document_id = d.id AND t.tag_id = {}) ORDER BY id LIMIT 1",
            f.tag_x
        ),
    )
    .await;
    let cfg = Config::from_env_map(Env::from_pairs(vec![
        ("PAPERLESS_SECRET_KEY", "pl-automation-tests-only-0123456789abcdefghijklmnop"),
        ("PAPERLESS_DBHOST", "127.0.0.1"),
        ("PAPERLESS_MEDIA_ROOT", std::env::temp_dir().join("pl_automation_media_unused").to_str().unwrap()),
    ]))
    .unwrap();
    let media = pl_documents_write::files::MediaCfg::from_config(&cfg);
    let ctx = pl_documents_write::auditlog::LogCtx { enabled: false, actor_id: None, actor_email: None, remote_addr: None, remote_port: None, cid: None };
    let mut conn = pool.acquire().await.unwrap();
    let mut tx = sqlx::Connection::begin(&mut *conn).await.unwrap();
    let o = pl_documents_write::workflows::run_updated(&mut tx, &ctx, &media, chrono_tz::UTC, with_x).await.unwrap();
    let mine: Vec<_> = o.runs.iter().filter(|r| r.workflow_id == wf_id).collect();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].branch, Some(pl_consumer::workflow_ext::Branch::Main));
    assert!(mine[0].changed_fields.contains(&"correspondent".to_string()), "{:?}", mine[0]);
    let o = pl_documents_write::workflows::run_updated(&mut tx, &ctx, &media, chrono_tz::UTC, without_x).await.unwrap();
    let mine: Vec<_> = o.runs.iter().filter(|r| r.workflow_id == wf_id).collect();
    assert_eq!(mine[0].branch, Some(pl_consumer::workflow_ext::Branch::Else));
    let logged: i64 = sqlx::query_scalar("SELECT count(*) FROM paperrust_workflow_run WHERE workflow_id = $1").bind(wf_id as i32).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(logged, 2);
    // AI condition + AI switched on → catch up in the API path instead of asking the model in the request
    let dt = scalar_i64(&pool, "SELECT id::bigint FROM documents_documenttype ORDER BY id LIMIT 1").await;
    sqlx::query("INSERT INTO paperrust_workflow_ext (workflow_id, ai_condition) VALUES ($1, $2) ON CONFLICT (workflow_id) DO UPDATE SET ai_condition = EXCLUDED.ai_condition")
        .bind(wf_id as i32)
        .bind(json!({"field": "document_type", "object_id": dt, "min_confidence": 0.9}))
        .execute(&mut *tx)
        .await
        .unwrap();
    pl_consumer::workflow_ext::install_ai_judge(Some(Arc::new(FakeJudge(true))));
    let o = pl_documents_write::workflows::run_updated(&mut tx, &ctx, &media, chrono_tz::UTC, with_x).await.unwrap();
    pl_consumer::workflow_ext::install_ai_judge(None);
    assert_eq!(o.deferred.len(), 1, "{o:?}");
    assert_eq!(o.deferred[0].trigger_type, 3);
    tx.rollback().await.unwrap();
    drop(conn);
    drop(r);
    drop_db(pool).await;
}
