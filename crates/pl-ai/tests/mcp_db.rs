#![cfg(feature = "server")]
//! MCP extension against a **copy** of the reference DB (never `paperless` itself):
//! migration 0002 (scopes), scope management `/api/x-ai/mcp-scopes/` (in-process with
//! `pl_authz::Authz`), scope resolution token → user → default, audit rows, and the
//! semantic search over the **existing** LLM index (built via `update_llm_index`, local
//! embedding mock) with the chat's permission filtering for every user of the copy.
//!
//! ```sh
//! psql -c "CREATE DATABASE pl_mcp_test OWNER paperless TEMPLATE paperless_snapshot"
//! PL_AI_MCP_TEST_DB=pl_mcp_test PL_AI_MCP_TEST_DATA_DIR=<empty directory> \
//!   cargo test -p pl-ai -j 2 --test mcp_db -- --ignored --nocapture
//! psql -c "DROP DATABASE pl_mcp_test"
//! ```

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use axum::routing::post;
use axum::Json;
use pl_ai::mcp::api::{ApiBackend, SideStore};
use pl_ai::mcp::backend::{Actor, AuditEntry, Backend, Credential};
use pl_ai::mcp::knowledge::{IndexState, KnowledgeIndex};
use pl_ai::mcp::{scopes, McpService, McpSettings};
use pl_ai::pg::PgSideStore;
use pl_ai::taxonomy::Principal;
use pl_authz::{AuthConfig, Authz};
use serde_json::{json, Value};
use tower::ServiceExt;

const ADMIN: &str = "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a001";
const ALICE: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002";
const BOB: &str = "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b003";

/// Deterministic "embeddings": word hashes into 16 dimensions.
fn fake_embedding(t: &str) -> Vec<f32> {
    let mut v = vec![0.001f32; 16];
    for w in t.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2) {
        let h = w.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(u32::from(b)));
        v[(h % 16) as usize] += 1.0;
    }
    v
}

async fn embedding_mock() -> String {
    let app = axum::Router::new().route(
        "/api/embed",
        post(|Json(b): Json<Value>| async move {
            let texts: Vec<String> = match &b["input"] {
                Value::String(s) => vec![s.clone()],
                Value::Array(a) => a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect(),
                _ => vec![],
            };
            Json(json!({"embeddings": texts.iter().map(|t| fake_embedding(t)).collect::<Vec<_>>()}))
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let a = l.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(l, app).await;
    });
    format!("http://{a}")
}

fn actor(id: i64, token: &str, superuser: bool) -> Actor {
    Actor {
        principal: Principal { user_id: Some(id), is_active: true, is_superuser: superuser, group_ids: vec![] },
        username: format!("u{id}"),
        perms: BTreeSet::new(),
        token_hint: "test…".into(),
        rate_key: format!("u{id}"),
        credential: Credential::new(token),
    }
}

async fn call(app: &axum::Router, method: &str, path: &str, token: &str, body: Option<Value>) -> (u16, Value) {
    let mut req = Request::builder().method(method).uri(path).header("authorization", format!("Token {token}")).header("accept", "application/json");
    let b = match body {
        Some(v) => {
            req = req.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let r = app.clone().oneshot(req.body(b).unwrap()).await.unwrap();
    let st = r.status().as_u16();
    let bytes = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
    (st, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
#[ignore = "braucht eine DB-Kopie: PL_AI_MCP_TEST_DB (aus paperless_snapshot) + PL_AI_MCP_TEST_DATA_DIR"]
async fn mcp_extension_against_db_copy() {
    let db = std::env::var("PL_AI_MCP_TEST_DB").expect("PL_AI_MCP_TEST_DB");
    assert_ne!(db, "paperless", "nie gegen die Referenz-DB");
    let data = std::env::var("PL_AI_MCP_TEST_DATA_DIR").expect("PL_AI_MCP_TEST_DATA_DIR");
    let emb = embedding_mock().await;
    // SAFETY: only test in this file; set before configuration/threads read it.
    unsafe {
        for (k, v) in [
            ("PAPERLESS_DBHOST", "127.0.0.1"),
            ("PAPERLESS_DBPORT", "54329"),
            ("PAPERLESS_DBNAME", db.as_str()),
            ("PAPERLESS_DBUSER", "paperless"),
            ("PAPERLESS_DBPASS", "paperless"),
            ("PAPERLESS_DATA_DIR", data.as_str()),
            ("PAPERLESS_SECRET_KEY", "mcp-db-test-only-not-a-real-secret-0123456789"),
            ("PAPERLESS_AI_ENABLED", "yes"),
            ("PAPERLESS_AI_LLM_EMBEDDING_BACKEND", "ollama"),
            ("PAPERLESS_AI_LLM_EMBEDDING_ENDPOINT", emb.as_str()),
            ("PAPERLESS_AI_LLM_BACKEND", "ollama"),
            ("PAPERLESS_AI_LLM_ENDPOINT", emb.as_str()),
        ] {
            std::env::set_var(k, v);
        }
    }
    let state = pl_core::AppState::from_env().await.expect("state");
    let pool = state.db().clone();
    pl_ai::server::migrations::apply(&pool).await.unwrap();
    pl_ai::server::migrations::apply(&pool).await.unwrap(); // idempotent

    // ---- scope management (superuser) ----
    let authz = Authz::new(pool.clone(), AuthConfig::from_core(state.config()));
    let mut router: axum::Router<pl_core::AppState> = axum::Router::new();
    for r in pl_ai::routes().into_iter().filter(|r| r.id.starts_with("ai.mcp_scopes")) {
        router = router.route(r.path, r.handler);
    }
    let app = router.with_state(state.clone()).layer(axum::Extension(authz));
    let (st, _) = call(&app, "POST", "/api/x-ai/mcp-scopes/", ALICE, Some(json!({"token": ALICE, "scopes": ["read", "write", "ingest"]}))).await;
    assert_eq!(st, 403, "Nicht-Superuser darf keine Scopes setzen");
    let (st, v) = call(&app, "POST", "/api/x-ai/mcp-scopes/", ADMIN, Some(json!({"token": ALICE, "scopes": ["read"], "label": "Alice Claude"}))).await;
    assert_eq!(st, 201, "{v}");
    assert_eq!(v["token_key_prefix"].as_str().unwrap().len(), 8);
    assert!(!v.to_string().contains(ALICE), "Token im Klartext zurückgegeben");
    let (st, _) = call(&app, "POST", "/api/x-ai/mcp-scopes/", ADMIN, Some(json!({"user_id": 4, "scopes": ["read", "write"]}))).await;
    assert_eq!(st, 201);
    let (st, v) = call(&app, "POST", "/api/x-ai/mcp-scopes/", ADMIN, Some(json!({"user_id": 4, "scopes": ["admin"]}))).await;
    assert_eq!(st, 400, "{v}");
    let (st, v) = call(&app, "POST", "/api/x-ai/mcp-scopes/", ADMIN, Some(json!({"token": "x", "user_id": 4, "scopes": []}))).await;
    assert_eq!(st, 400, "{v}");
    let (st, v) = call(&app, "GET", "/api/x-ai/mcp-scopes/", ADMIN, None).await;
    assert_eq!((st, v["count"].as_i64()), (200, Some(2)), "{v}");
    // plaintext token stored nowhere
    let leaked: i64 = sqlx::query_scalar("SELECT count(*) FROM paperrust_ai_mcp_scope WHERE token_key = $1").bind(ALICE).fetch_one(&pool).await.unwrap();
    assert_eq!(leaked, 0);

    // ---- resolution: token entry > user entry > none ----
    let store = Arc::new(PgSideStore::new(pool.clone()));
    assert_eq!(store.token_scopes(&scopes::token_key(ALICE), Some(3)).await.unwrap(), Some(vec!["read".to_string()]));
    assert_eq!(store.token_scopes(&scopes::token_key(BOB), Some(4)).await.unwrap(), Some(vec!["read".to_string(), "write".to_string()]));
    assert_eq!(store.token_scopes(&scopes::token_key("c0c0"), Some(5)).await.unwrap(), None);
    let backend = Arc::new(ApiBackend::new("http://127.0.0.1:9", store.clone()));
    let settings = McpSettings { tool_groups: vec!["read".into(), "write".into(), "ingest".into()], ..Default::default() };
    let svc = McpService::new(backend.clone(), settings, None, Arc::new(pl_ai::classifier::IdentityTruncator));
    let g = |v: &[&'static str]| v.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(svc.groups(&actor(3, ALICE, false)).await.unwrap(), g(&["read"]));
    assert_eq!(svc.groups(&actor(4, BOB, false)).await.unwrap(), g(&["read", "write"]));
    assert_eq!(svc.groups(&actor(5, "c0c0", false)).await.unwrap(), g(&["read", "write", "ingest"]));
    assert_eq!(backend.token_scopes(&actor(3, ALICE, false)).await.unwrap(), Some(vec!["read".to_string()]));
    // delete → default again
    let id = v["results"][0]["id"].as_i64().unwrap();
    assert_eq!(call(&app, "DELETE", &format!("/api/x-ai/mcp-scopes/{id}/"), ADMIN, None).await.0, 204);
    assert_eq!(call(&app, "DELETE", &format!("/api/x-ai/mcp-scopes/{id}/"), ADMIN, None).await.0, 404);
    assert_eq!(svc.groups(&actor(3, ALICE, false)).await.unwrap(), g(&["read", "write", "ingest"]));

    // ---- audit rows ----
    store
        .audit(AuditEntry {
            actor_id: Some(3),
            channel: "mcp",
            token_hint: Some("a1a1…".into()),
            run_id: None,
            action: "add_document",
            object_kind: "document".into(),
            object_id: None,
            forced: false,
            reason: None,
            candidates: None,
            before: None,
            after: Some(json!({"task_id": "t-1", "sha256": "00"})),
            confidence: None,
        })
        .await
        .unwrap();
    assert_eq!(store.audit_count("add_document", Some(3)).await.unwrap(), 1);
    let set: i64 = sqlx::query_scalar("SELECT count(*) FROM paperrust_ai_audit WHERE channel = 'ui' AND action IN ('mcp_scope_set', 'mcp_scope_delete')")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(set, 3, "zwei Setzungen + eine Löschung auditiert");

    // ---- existing LLM index: build via the normal path, retrieval like the chat ----
    let k = pl_ai::server::mcp_index::LlmIndexKnowledge(state.clone());
    assert_eq!(k.state().await, IndexState::NotBuilt);
    let cfg = pl_ai::server::effective_config(&state).await.unwrap();
    assert!(cfg.llm_index_enabled());
    let msg = pl_ai::server::index::update_llm_index(&state, &cfg, true, None).await.expect("index build");
    println!("index: {msg}");
    assert_eq!(k.state().await, IndexState::Ready);
    let trashed: BTreeSet<i64> = pl_ai::server::data::trashed_ids(&pool).await.unwrap().into_iter().collect();
    let mut seen_any = false;
    for (uid, tok, su) in [(2, ADMIN, true), (3, ALICE, false), (4, BOB, false), (5, "c0c0", false)] {
        let user = pl_authz::user::load_user_by_id(&pool, uid).await.unwrap().unwrap();
        let permitted: BTreeSet<i64> = pl_ai::server::data::permitted_doc_ids(&pool, Some(&user)).await.unwrap().into_iter().collect();
        let hits = k.semantic(&actor(uid, tok, su), "rechnung invoice vertrag", 50, None).await.unwrap();
        let ids: BTreeSet<i64> = hits.iter().map(|h| h.document_id).collect();
        println!("user {uid}: {} hits, {} docs, {} permitted", hits.len(), ids.len(), permitted.len());
        assert!(ids.is_disjoint(&trashed), "Papierkorb im Ergebnis für {uid}");
        if !su {
            assert!(ids.is_subset(&permitted), "user {uid} sieht fremde Dokumente: {:?}", ids.difference(&permitted).collect::<Vec<_>>());
        }
        seen_any |= !ids.is_empty();
        // restriction to specific documents
        if let Some(first) = ids.iter().next().copied() {
            let only = k.semantic(&actor(uid, tok, su), "rechnung", 10, Some(&[first])).await.unwrap();
            assert!(only.iter().all(|h| h.document_id == first));
        }
    }
    assert!(seen_any, "keine Treffer im Index");
    let lang = k.output_language(&actor(3, ALICE, false)).await;
    println!("output language alice: {lang:?}");
}
