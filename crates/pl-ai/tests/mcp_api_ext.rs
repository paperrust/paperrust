//! Production backend (`ApiBackend`, REST with the agent's token) against a local
//! REST mock: checks the requests of the new tools (multipart upload to
//! `/api/documents/post_document/`, task status, notes, saved views,
//! document lists) and that the token is passed through. No network except 127.0.0.1.

use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::{Multipart, Path, RawQuery};
use axum::http::HeaderMap;
use axum::routing::{get, post};
use base64::Engine as _;
use pl_ai::classifier::IdentityTruncator;
use pl_ai::mcp::api::{ApiBackend, MemorySideStore};
use pl_ai::mcp::http::{router, testclient::Client};
use pl_ai::mcp::{McpService, McpSettings};
use serde_json::{Value, json};

const TOKEN: &str = "apitoken000000000000000000000000000000000";

#[derive(Default)]
struct Seen {
    uploads: Vec<Vec<(String, String)>>,
    queries: Vec<String>,
    notes: Vec<String>,
}

fn authed(h: &HeaderMap) -> bool {
    h.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Token {TOKEN}"))
}

async fn rest_mock(seen: Arc<Mutex<Seen>>) -> String {
    let s1 = seen.clone();
    let s2 = seen.clone();
    let s3 = seen.clone();
    let app = axum::Router::new()
        .route(
            "/api/ui_settings/",
            get(|h: HeaderMap| async move {
                if !authed(&h) {
                    return (axum::http::StatusCode::UNAUTHORIZED, Json(json!({"detail": "Invalid token."})));
                }
                (
                    axum::http::StatusCode::OK,
                    Json(json!({"user": {"id": 5, "username": "agent", "is_superuser": false, "groups": []},
                        "permissions": ["add_document", "view_document", "change_document", "view_tag", "view_correspondent",
                                        "view_note", "add_note", "view_savedview", "view_documenttype", "view_customfield"]})),
                )
            }),
        )
        .route(
            "/api/correspondents/",
            get(|| async { Json(json!({"count": 1, "next": null, "results": [{"id": 3, "name": "Swisscom (Schweiz) AG", "owner": null}]})) }),
        )
        .route("/api/tags/", get(|| async { Json(json!({"count": 1, "next": null, "results": [{"id": 4, "name": "Rechnung", "owner": null}]})) }))
        .route(
            "/api/documents/post_document/",
            post(move |h: HeaderMap, mut mp: Multipart| {
                let seen = s1.clone();
                async move {
                    assert!(authed(&h));
                    let mut fields = Vec::new();
                    while let Ok(Some(f)) = mp.next_field().await {
                        let name = f.name().unwrap_or("").to_string();
                        let fname = f.file_name().map(str::to_string);
                        let ct = f.content_type().map(str::to_string);
                        let data = f.bytes().await.unwrap();
                        match fname {
                            Some(n) => fields.push((name, format!("{n}|{}|{}", ct.unwrap_or_default(), data.len()))),
                            None => fields.push((name, String::from_utf8_lossy(&data).to_string())),
                        }
                    }
                    seen.lock().unwrap().uploads.push(fields);
                    Json(json!("0b5e0d1c-1111-2222-3333-444455556666"))
                }
            }),
        )
        .route(
            "/api/tasks/",
            get(|RawQuery(q): RawQuery| async move {
                assert_eq!(q.as_deref(), Some("task_id=0b5e0d1c-1111-2222-3333-444455556666"));
                Json(json!([{"task_id": "0b5e0d1c-1111-2222-3333-444455556666", "status": "SUCCESS", "task_file_name": "scan.pdf",
                             "result": "Success. New document id 77 created", "related_document": "77"}]))
            }),
        )
        .route(
            "/api/documents/{id}/notes/",
            get(|Path(id): Path<i64>| async move {
                Json(json!([{"id": 1, "note": format!("Notiz zu {id}"), "created": "2024-01-01T00:00:00Z", "user": {"id": 5, "username": "agent"}}]))
            })
            .post(move |Path(_id): Path<i64>, Json(b): Json<Value>| {
                let seen = s2.clone();
                async move {
                    seen.lock().unwrap().notes.push(b["note"].as_str().unwrap_or("").to_string());
                    Json(json!([{"id": 2, "note": b["note"], "user": {"id": 5, "username": "agent"}}, {"id": 1, "note": "alt"}]))
                }
            }),
        )
        .route(
            "/api/documents/{id}/",
            get(|Path(id): Path<i64>| async move {
                Json(json!({"id": id, "title": "Rechnung", "content": "Text", "tags": [], "user_can_change": true, "notes": [], "custom_fields": []}))
            }),
        )
        .route(
            "/api/saved_views/",
            get(|| async {
                Json(json!({"count": 1, "next": null, "results": [{"id": 7, "name": "Inbox", "sort_field": "added", "sort_reverse": true,
                    "filter_rules": [{"rule_type": 6, "value": "4"}, {"rule_type": 19, "value": "mai rechnung"}, {"rule_type": 3, "value": null}]}]}))
            }),
        )
        .route(
            "/api/documents/",
            get(move |RawQuery(q): RawQuery| {
                let seen = s3.clone();
                async move {
                    seen.lock().unwrap().queries.push(q.unwrap_or_default());
                    Json(json!({"count": 1, "results": [{"id": 77, "title": "Neu", "created": "2024-06-01", "added": "2024-06-02T10:00:00Z", "tags": [4]}]}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

async fn start() -> (String, Arc<Mutex<Seen>>, Arc<MemorySideStore>) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let api = rest_mock(seen.clone()).await;
    let store = Arc::new(MemorySideStore::default());
    let backend = Arc::new(ApiBackend::new(&api, store.clone()));
    let settings = McpSettings { tool_groups: vec!["read".into(), "write".into(), "ingest".into()], ..Default::default() };
    let svc = Arc::new(McpService::new(backend, settings, None, Arc::new(IdentityTruncator)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router(svc)).await;
    });
    (format!("http://{addr}"), seen, store)
}

fn sc(v: &Value) -> &Value {
    &v["structuredContent"]
}

#[tokio::test]
async fn rest_requests_of_new_tools() {
    let (base, seen, store) = start().await;
    let mut c = Client::new(&base, Some(TOKEN));
    c.initialize().await.unwrap();

    // add_document → multipart with matched ids, file as "document"
    let pdf = base64::engine::general_purpose::STANDARD.encode(b"%PDF-1.4\n%%EOF\n");
    let r = c
        .call("add_document", json!({"file_base64": pdf, "filename": "scan.pdf", "title": "Rechnung", "correspondent": "Swisscom", "tags": ["rechnung"], "created": "2024-06-01"}))
        .await
        .unwrap();
    assert_eq!(sc(&r)["task_id"], "0b5e0d1c-1111-2222-3333-444455556666", "{r}");
    let up = seen.lock().unwrap().uploads[0].clone();
    let get = |k: &str| up.iter().filter(|(n, _)| n == k).map(|(_, v)| v.clone()).collect::<Vec<_>>();
    assert_eq!(get("title"), vec!["Rechnung"]);
    assert_eq!(get("correspondent"), vec!["3"]);
    assert_eq!(get("tags"), vec!["4"]);
    assert_eq!(get("created"), vec!["2024-06-01"]);
    assert_eq!(get("document"), vec!["scan.pdf|application/pdf|15"]);
    assert!(store.audit.lock().unwrap().iter().any(|e| e.action == "add_document"));

    // task status (old field names, API v10)
    let r = c.call("get_task_status", json!({"task_id": "0b5e0d1c-1111-2222-3333-444455556666"})).await.unwrap();
    assert_eq!(sc(&r)["status"], "SUCCESS");
    assert_eq!(sc(&r)["related_document"], "77");
    assert_eq!(c.call("get_task_status", json!({"task_id": "../x"})).await.unwrap()["isError"], true);

    // notes
    let r = c.call("list_notes", json!({"document_id": 9})).await.unwrap();
    assert_eq!(sc(&r)["notes"][0]["user"], "agent");
    let r = c.call("add_note", json!({"document_id": 9, "note": "Erledigt"})).await.unwrap();
    assert_eq!(sc(&r)["note_id"], 2, "{r}");
    assert_eq!(seen.lock().unwrap().notes, vec!["Erledigt"]);

    // saved view → filter rules as query (like the upstream frontend)
    let r = c.call("run_saved_view", json!({"id": 7, "limit": 10})).await.unwrap();
    assert_eq!(sc(&r)["count"], 1, "{r}");
    let q = seen.lock().unwrap().queries.last().cloned().unwrap();
    for part in ["tags__id__all=4", "text=mai%20rechnung", "correspondent__isnull=1", "ordering=-added", "page_size=10", "page=1"] {
        assert!(q.contains(part), "{part} fehlt in {q}");
    }
    // neueste Dokumente
    let r = c.call("list_recent_documents", json!({"limit": 3})).await.unwrap();
    assert_eq!(sc(&r)["results"][0]["added"], "2024-06-02T10:00:00Z");
    let q = seen.lock().unwrap().queries.last().cloned().unwrap();
    assert!(q.contains("ordering=-added") && q.contains("page_size=3"), "{q}");
}
