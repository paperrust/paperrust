//! E2E P6: restricted sync against an **unmodified paperless-ngx 3.4.2** (own, writable
//! copy of the reference via `tests/parity/w2-*.sh` with `W2_NAME`, never the reference DB `paperless`).
//!
//! * `PL_P6_NGX_URL` – e.g. `http://127.0.0.1:8120/` (Django of the W2 copy)
//! * `PL_P6_NGX_DB`  – Postgres URL of the copy, name starts with `pl_w2` (permission revocation directly in the DB)
//! * `PL_P6_NGX_STOP` – command that stops the server (offline part, e.g. `bash tests/parity/w2-stop.sh dj`)
//!
//! `cargo test -p pl-sync-client --test e2e_ngx -- --ignored --nocapture`

use std::sync::Arc;
use std::time::{Duration, Instant};

use pl_sync_client::secret::MemoryStore;
use pl_sync_client::selection::Limits;
use pl_sync_client::store::SelectionItem;
use pl_sync_client::tls::TlsOptions;
use pl_sync_client::{Gateway, GwRequest, ProfileConfig, SyncClient};
use serde_json::{Value, json};
use zeroize::Zeroizing;

const TOKEN_ALICE: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002";

fn get(path: &str, query: Option<&str>) -> GwRequest {
    GwRequest { method: "GET".into(), path: path.into(), query: query.map(str::to_string), headers: vec![], body: vec![] }
}

fn body(r: &pl_sync_client::GwResponse) -> Value {
    serde_json::from_slice(&r.body).unwrap_or(Value::Null)
}

/// Directly to Django (check/change calls of the test, as alice).
async fn api(url: &str, method: &str, path: &str, b: Option<Value>) -> (u16, Value) {
    let c = reqwest::Client::new();
    let mut rb = c
        .request(reqwest::Method::from_bytes(method.as_bytes()).unwrap(), format!("{url}{path}"))
        .header("Authorization", format!("Token {TOKEN_ALICE}"))
        .header("Accept", "application/json; version=9");
    if let Some(b) = b {
        rb = rb.json(&b);
    }
    let r = rb.send().await.unwrap();
    let s = r.status().as_u16();
    (s, r.json().await.unwrap_or(Value::Null))
}

#[tokio::test]
#[ignore = "needs a writable paperless-ngx copy (PL_P6_NGX_URL, PL_P6_NGX_DB, PL_P6_NGX_STOP)"]
async fn ngx_degraded_sync() {
    let url = std::env::var("PL_P6_NGX_URL").expect("PL_P6_NGX_URL");
    assert!(url.starts_with("http://127.0.0.1:"), "local servers only");
    let db = std::env::var("PL_P6_NGX_DB").expect("PL_P6_NGX_DB");
    assert!(db.rsplit('/').next().unwrap_or("").starts_with("pl_w2"), "only a W2 copy");
    let pg = sqlx::postgres::PgPool::connect(&db).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cfg = ProfileConfig {
        profile_id: "p6ngx".into(),
        url: url.clone(),
        tls: TlsOptions::default(),
        dir: dir.path().join("profiles").join("p6ngx"),
        device_name: "P6".into(),
        flavor: "ngx".into(),
    };
    let c = SyncClient::open(cfg, Arc::new(MemoryStore::default())).await.unwrap();
    c.set_limits(Limits { max_bytes: 1 << 30, max_documents: 5_000 }).await.unwrap();
    assert!(c.login("alice", Zeroizing::new("wrong".into()), None).await.is_err());
    c.login("alice", Zeroizing::new("alice".into()), None).await.unwrap();

    // Stammdaten
    let t = Instant::now();
    let rep = c.sync_once().await.unwrap();
    println!("P6 metadata: {rep:?} in {:?}", t.elapsed());
    let r = c.replica();
    let tags = r.count_objects("tag").await.unwrap();
    assert!(tags > 0);
    let (_, server_tags) = api(&url, "GET", "api/tags/?page_size=100000", None).await;
    assert_eq!(tags, server_tags["count"].as_i64().unwrap(), "all visible tags mirrored");

    // Selection: tag "Invoices" + one saved view
    let tag_key: String = sqlx::query_scalar("SELECT key FROM object WHERE kind = 'tag' AND name = 'Invoices'").fetch_one(r.pool()).await.unwrap();
    let tag_id: i64 = sqlx::query_scalar("SELECT legacy_id FROM object WHERE kind = 'tag' AND name = 'Invoices'").fetch_one(r.pool()).await.unwrap();
    let views: Vec<(String, String, i64)> = sqlx::query_as("SELECT key, name, legacy_id FROM object WHERE kind = 'saved_view' ORDER BY legacy_id").fetch_all(r.pool()).await.unwrap();
    let mut items = vec![SelectionItem { kind: "tag".into(), key: tag_key.clone(), files: "preferred".into() }];
    // first view whose rules can be mapped
    let mut view_used = None;
    for (key, name, _) in &views {
        let it = SelectionItem { kind: "saved_view".into(), key: key.clone(), files: "none".into() };
        if let Ok(p) = c.preview_selection(std::slice::from_ref(&it)).await {
            view_used = Some((name.clone(), p.total_documents));
            items.push(it);
            break;
        }
    }
    let plan = c.apply_selection(&items).await.unwrap();
    let (_, srv) = api(&url, "GET", &format!("api/documents/?tags__id__in={tag_id}&page_size=1"), None).await;
    assert_eq!(plan.entries[0].documents as i64, srv["count"].as_i64().unwrap(), "tag resolution = server list");
    let t = Instant::now();
    let rep = c.sync_once().await.unwrap();
    println!("P6 initial: {rep:?} in {:?}, view {view_used:?}", t.elapsed());
    let n_local = r.count_documents(true).await.unwrap();
    assert_eq!(n_local as u64, plan.total_documents);
    let pdfs: i64 = sqlx::query_scalar("SELECT count(*) FROM file WHERE variant IN ('original','archive') AND state = 'complete'").fetch_one(r.pool()).await.unwrap();
    assert!(pdfs > 0, "files downloaded");
    // Inhalt + Notizen da, Volltext offline
    let any: String = sqlx::query_scalar("SELECT key FROM document WHERE in_selection = 1 AND has_detail = 1 LIMIT 1").fetch_one(r.pool()).await.unwrap();
    assert!(r.document_full(&any, true, true).await.unwrap().unwrap()["content"].is_string());

    // Bulk tag (bulk_edit add_tag does not change `modified`) → only the fingerprint finds it
    let doc_id: i64 = sqlx::query_scalar("SELECT legacy_id FROM document WHERE in_selection = 1 ORDER BY legacy_id LIMIT 1").fetch_one(r.pool()).await.unwrap();
    let other_tag: i64 = sqlx::query_scalar("SELECT legacy_id FROM object WHERE kind = 'tag' AND legacy_id <> ? AND key NOT IN (SELECT tag FROM document_tag dt JOIN document d ON d.key = dt.doc WHERE d.legacy_id = ?) ORDER BY legacy_id LIMIT 1")
        .bind(tag_id)
        .bind(doc_id)
        .fetch_one(r.pool())
        .await
        .unwrap();
    let (_, before) = api(&url, "GET", &format!("api/documents/{doc_id}/"), None).await;
    let (s, _) = api(&url, "POST", "api/documents/bulk_edit/", Some(json!({"documents": [doc_id], "method": "add_tag", "parameters": {"tag": other_tag}}))).await;
    assert_eq!(s, 200);
    let (_, after) = api(&url, "GET", &format!("api/documents/{doc_id}/"), None).await;
    let modified_unchanged = before["modified"] == after["modified"];
    println!("P6 add_tag: modified unchanged = {modified_unchanged}");
    let tagged = |r: pl_sync_client::store::Replica| async move {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM document_tag dt JOIN document d ON d.key = dt.doc JOIN object o ON o.kind = 'tag' AND o.key = dt.tag WHERE d.legacy_id = ? AND o.legacy_id = ?")
            .bind(doc_id)
            .bind(other_tag)
            .fetch_one(r.pool())
            .await
            .unwrap()
    };
    c.set_fingerprint_interval(Duration::from_secs(3600));
    c.sync_once().await.unwrap();
    let seen_without_fp = tagged(r.clone()).await;
    if modified_unchanged {
        assert_eq!(seen_without_fp, 0, "modified__gt alone misses add_tag (documented gap)");
    }
    c.set_fingerprint_interval(Duration::ZERO);
    c.sync_once().await.unwrap();
    assert_eq!(tagged(r.clone()).await, 1, "fingerprint check found the bulk tag change");

    // Delete (trash) → detected via `all`
    let del_id: i64 = sqlx::query_scalar("SELECT legacy_id FROM document WHERE in_selection = 1 AND legacy_id <> ? ORDER BY legacy_id DESC LIMIT 1").bind(doc_id).fetch_one(r.pool()).await.unwrap();
    let (s, _) = api(&url, "DELETE", &format!("api/documents/{del_id}/"), None).await;
    assert_eq!(s, 204);
    c.sync_once().await.unwrap();
    let gone: i64 = sqlx::query_scalar("SELECT count(*) FROM document WHERE legacy_id = ?").bind(del_id).fetch_one(r.pool()).await.unwrap();
    assert_eq!(gone, 0, "deleted document removed");

    // Permission revocation (object permission directly in the copy) → detected via `all`
    let shared: Option<i32> = sqlx::query_scalar(
        "SELECT d.id FROM documents_document d JOIN guardian_userobjectpermission p ON p.object_pk = d.id::text AND p.user_id = 3
           JOIN auth_permission ap ON ap.id = p.permission_id AND ap.codename = 'view_document'
          WHERE d.owner_id IS NOT NULL AND d.owner_id <> 3 AND d.deleted_at IS NULL
            AND NOT EXISTS (SELECT 1 FROM guardian_groupobjectpermission g JOIN auth_user_groups ug ON ug.group_id = g.group_id
                            WHERE ug.user_id = 3 AND g.object_pk = d.id::text)
          ORDER BY d.id LIMIT 1",
    )
    .fetch_optional(&pg)
    .await
    .unwrap();
    let rid = shared.expect("seed shares documents with alice by user permission") as i64;
    // make sure it is selected
    let mut items2 = items.clone();
    items2.push(SelectionItem { kind: "document".into(), key: format!("00000000-0000-4000-8001-{rid:012x}"), files: "none".into() });
    c.apply_selection(&items2).await.unwrap();
    c.sync_once().await.unwrap();
    let have: i64 = sqlx::query_scalar("SELECT count(*) FROM document WHERE legacy_id = ?").bind(rid).fetch_one(r.pool()).await.unwrap();
    assert_eq!(have, 1);
    sqlx::query("DELETE FROM guardian_userobjectpermission WHERE object_pk = $1 AND user_id = 3").bind(rid.to_string()).execute(&pg).await.unwrap();
    c.sync_once().await.unwrap();
    let have: i64 = sqlx::query_scalar("SELECT count(*) FROM document WHERE legacy_id = ?").bind(rid).fetch_one(r.pool()).await.unwrap();
    assert_eq!(have, 0, "revoked document removed");

    // Gateway online: v1 passed through (token from the core), v2 from the replica
    let gw = Gateway::new(c.clone());
    let on = gw.handle(get("/api/documents/", Some("page=1&page_size=5"))).await;
    assert_eq!(on.header("x-paperrust-source"), Some("remote"));
    let v2 = gw.handle(get("/rs/api/v2/documents", Some("limit=5&count=true"))).await;
    assert_eq!(v2.header("x-paperrust-source"), Some("local"));
    let _ = gw.handle(get("/api/ui_settings/", None)).await;

    // Offline: stop the server → list, search, PDF from the replica (v1 like the light UI)
    if let Ok(cmd) = std::env::var("PL_P6_NGX_STOP") {
        let st = std::process::Command::new("bash").arg("-c").arg(&cmd).status().unwrap();
        assert!(st.success());
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(c.sync_once().await.unwrap_err().is_offline());
        let l = body(&gw.handle(get("/api/documents/", Some(&format!("page=1&page_size=25&ordering=-created&tags__id__all={tag_id}")))).await);
        assert!(l["count"].as_i64().unwrap() > 0, "{l}");
        let s = gw.handle(get("/api/documents/", Some("page=1&page_size=25&title_content=invoice"))).await;
        assert_eq!(s.header("x-paperrust-source"), Some("local"));
        let id = l["results"][0]["id"].as_i64().unwrap();
        let pdf = gw.handle(get(&format!("/api/documents/{id}/preview/"), None)).await;
        assert_eq!(pdf.status, 200);
        assert!(pdf.body.starts_with(b"%PDF") || pdf.header("content-type") != Some("application/pdf"));
        let ui = gw.handle(get("/api/ui_settings/", None)).await;
        assert_eq!(body(&ui)["user"]["username"], "alice");
        println!("P6 offline: list {} docs, search {}, pdf {} bytes", l["count"], body(&s)["count"], pdf.body.len());
    }
    c.logout(true).await.unwrap();
}
