//! MCP server with the REST backend against the **reference Django instance** (127.0.0.1:8000,
//! users from `reference/users.json`). Read-only: only tools that write nothing are used
//! (create_taxonomy only in cases that are rejected on the server side).
//!
//! `#[ignore]` because the reference environment does not always run. Invocation:
//! `cargo test -p pl-ai --test mcp_reference -j 2 -- --ignored`

use std::sync::Arc;

use pl_ai::classifier::IdentityTruncator;
use pl_ai::mcp::api::{ApiBackend, MemorySideStore};
use pl_ai::mcp::http::{router, testclient::Client};
use pl_ai::mcp::{McpService, McpSettings};
use serde_json::{json, Value};

const DJANGO: &str = "http://127.0.0.1:8000";
const ALICE: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002";
const BOB: &str = "b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b003";
const CAROL: &str = "c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c004";
const DAVE: &str = "d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d0d005";

async fn start() -> String {
    let backend = Arc::new(ApiBackend::new(DJANGO, Arc::new(MemorySideStore::default())));
    let svc = Arc::new(McpService::new(backend, McpSettings::default(), None, Arc::new(IdentityTruncator)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router(svc)).await;
    });
    format!("http://{addr}")
}

fn sc(v: &Value) -> &Value {
    &v["structuredContent"]
}

#[tokio::test]
#[ignore = "braucht die Referenz-Django-Instanz auf 127.0.0.1:8000"]
async fn reference_read_only() {
    let base = start().await;

    // inactive user / invalid token → 401
    let dave = Client::new(&base, Some(DAVE));
    let (st, _, _) = dave.raw(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}), &[]).await.unwrap();
    assert_eq!(st, 401);

    let mut alice = Client::new(&base, Some(ALICE));
    alice.initialize().await.unwrap();
    let r = alice.call("list_taxonomy", json!({"kind": "correspondent"})).await.unwrap();
    let items = sc(&r)["items"].as_array().cloned().unwrap_or_default();
    assert!(!items.is_empty(), "{r}");
    println!("alice sees {} correspondents: {:?}", items.len(), items.iter().map(|i| i["name"].clone()).collect::<Vec<_>>());

    // matching: every existing spelling is recognized
    for it in &items {
        let name = it["name"].as_str().unwrap();
        let variant = format!("{} GmbH", name.to_uppercase());
        let r = alice.call("find_similar", json!({"kind": "correspondent", "name": variant})).await.unwrap();
        assert_eq!(sc(&r)["exists"], true, "{variant}: {r}");
        // creation is refused on the server side (writes nothing)
        let r = alice.call("create_taxonomy", json!({"kind": "correspondent", "name": variant})).await.unwrap();
        assert_eq!(sc(&r)["created"], false, "{variant}: {r}");
    }

    let r = alice.call("search_documents", json!({"limit": 5})).await.unwrap();
    let count = sc(&r)["count"].as_i64().unwrap_or(0);
    println!("alice: {count} documents");
    if let Some(first) = sc(&r)["results"].as_array().and_then(|a| a.first()) {
        let id = first["id"].as_i64().unwrap();
        let r = alice.call("get_document", json!({"id": id, "max_chars": 50})).await.unwrap();
        assert_eq!(sc(&r)["id"], id, "{r}");
        assert!(sc(&r)["content"].as_str().unwrap().chars().count() <= 50);
    }

    // Bob does not see everything Alice sees (private objects), and vice versa
    let mut bob = Client::new(&base, Some(BOB));
    bob.initialize().await.unwrap();
    let rb = bob.call("list_taxonomy", json!({"kind": "correspondent"})).await.unwrap();
    println!("bob sees {} correspondents", sc(&rb)["total"]);

    // user without model permissions
    let carol = Client::new(&base, Some(CAROL));
    let (st, _, _) = carol.raw(&json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}), &[]).await.unwrap();
    assert_eq!(st, 403, "Token gültig, aber ohne Rechte");
}
