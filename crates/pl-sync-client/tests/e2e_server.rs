//! E2E against a **local** PaperRust test server (SYNC.md §9 "E2E PaperRust", ROADMAP P5).
//! Only with explicit environment variables, otherwise skipped (`#[ignore]`):
//!
//! * `PL_P5_E2E_URL`  – e.g. `http://127.0.0.1:8195/` (own instance on a DB copy)
//! * `PL_P5_E2E_DB`   – Postgres URL of the copy; name must start with `pl_p5_` (never `paperless`)
//! * `PL_P5_E2E_DIR`  – profile folder (scratch); token in the real Windows keychain under
//!   `PaperRust/p5-e2e-*`, deleted at the end of `e2e_4_cleanup`.
//!
//! Flow (stop the server between 1 and 2, start it again before 3):
//! `cargo test -p pl-sync-client --test e2e_server -- --ignored --exact e2e_1_online` etc.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use pl_sync_client::secret::{SecretStore, os_store};
use pl_sync_client::selection::Limits;
use pl_sync_client::store::SelectionItem;
use pl_sync_client::tls::TlsOptions;
use pl_sync_client::{Gateway, GwRequest, ProfileConfig, SyncClient};
use serde_json::{Value, json};
use zeroize::Zeroizing;

struct E2e {
    url: String,
    dir: PathBuf,
    db: String,
}

fn e2e() -> E2e {
    let url = std::env::var("PL_P5_E2E_URL").expect("PL_P5_E2E_URL");
    assert!(url.starts_with("http://127.0.0.1:") || url.starts_with("https://127.0.0.1:"), "local servers only");
    let db = std::env::var("PL_P5_E2E_DB").expect("PL_P5_E2E_DB");
    let name = db.rsplit('/').next().unwrap_or("");
    assert!(name.starts_with("pl_p5_"), "only a pl_p5_* copy, never the reference DB");
    E2e { url, dir: PathBuf::from(std::env::var("PL_P5_E2E_DIR").expect("PL_P5_E2E_DIR")), db }
}

async fn client(e: &E2e, profile: &str) -> SyncClient {
    let secrets: Arc<dyn SecretStore> = Arc::from(os_store());
    let cfg = ProfileConfig {
        profile_id: profile.into(),
        url: e.url.clone(),
        tls: TlsOptions::default(),
        dir: e.dir.join("profiles").join(profile),
        device_name: format!("P5-E2E {profile}"),
        flavor: "paperrust".into(),
    };
    let c = SyncClient::open(cfg, secrets).await.unwrap();
    c.set_resolve_throttle(Duration::ZERO);
    c
}

fn get(path: &str, query: Option<&str>) -> GwRequest {
    GwRequest { method: "GET".into(), path: path.into(), query: query.map(str::to_string), headers: vec![], body: vec![] }
}

fn post(path: &str, body: Value) -> GwRequest {
    GwRequest { method: "POST".into(), path: path.into(), query: None, headers: vec![], body: serde_json::to_vec(&body).unwrap() }
}

fn body(r: &pl_sync_client::GwResponse) -> Value {
    serde_json::from_slice(&r.body).unwrap_or(Value::Null)
}

fn report(e: &E2e, name: &str, v: Value) {
    println!("P5-E2E {name}: {v}");
    let p = e.dir.join(format!("{name}.json"));
    let _ = std::fs::write(p, serde_json::to_vec_pretty(&v).unwrap());
}

/// Waits until the sequencer has numbered the change and the feed delivers it.
async fn sync_until<F: Fn() -> Fut, Fut: std::future::Future<Output = bool>>(c: &SyncClient, ok: F) {
    for _ in 0..40 {
        c.sync_once().await.unwrap();
        if ok().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("change not visible after 20 s");
}

#[tokio::test]
#[ignore = "needs a local PaperRust test server (PL_P5_E2E_URL, PL_P5_E2E_DB, PL_P5_E2E_DIR)"]
async fn e2e_1_online() {
    let e = e2e();
    let pg = sqlx::postgres::PgPool::connect(&e.db).await.unwrap();
    let c = client(&e, "p5-e2e-admin").await;
    c.set_limits(Limits { max_bytes: 2 << 30, max_documents: 10_000 }).await.unwrap();
    if !c.status().logged_in {
        c.login("admin", Zeroizing::new("admin".into()), None).await.unwrap();
    }
    let info = c.remote().server().await.unwrap();
    assert!(info.has("sync"), "server must offer the feed: {info:?}");

    // --- Initial sync: all visible documents without files (measurement ROADMAP P5)
    let ids = c.remote().ids("document").await.unwrap();
    let n = ids.items.len();
    assert!(n >= 5_000, "expected ≥ 5 000 documents, got {n}");
    let sel: Vec<SelectionItem> =
        ids.items.iter().map(|(u, _)| SelectionItem { kind: "document".into(), key: u.clone(), files: "none".into() }).collect();
    let t0 = Instant::now();
    let plan = c.apply_selection(&sel).await.unwrap();
    let t_resolve = t0.elapsed();
    let rep = c.sync_once().await.unwrap();
    let t_total = t0.elapsed();
    let local = c.replica().count_documents(true).await.unwrap();
    assert_eq!(local as usize, n);
    assert_eq!(plan.total_documents as usize, n);
    let fts: i64 = sqlx::query_scalar("SELECT count(*) FROM fts").fetch_one(c.replica().pool()).await.unwrap();
    report(
        &e,
        "initial-5000",
        json!({"documents": n, "seconds_total": t_total.as_secs_f64(), "seconds_preview": t_resolve.as_secs_f64(),
               "report": rep, "fts_rows": fts, "replica_bytes": std::fs::metadata(e.dir.join("profiles/p5-e2e-admin/replica.sqlite")).map(|m| m.len()).unwrap_or(0)}),
    );
    assert!(t_total < Duration::from_secs(120), "initial sync took {t_total:?}");

    // --- second run without changes
    let t1 = Instant::now();
    let rep2 = c.sync_once().await.unwrap();
    report(&e, "noop", json!({"seconds": t1.elapsed().as_secs_f64(), "report": rep2}));
    assert_eq!(rep2.documents_fetched, 0);

    // --- Selection with files: tag "Invoices" (seed documents with real files) preferred
    let tags = c.replica().pool();
    let invoices: String =
        sqlx::query_scalar("SELECT key FROM object WHERE kind = 'tag' AND name = 'Invoices'").fetch_one(tags).await.unwrap();
    let mut sel2 = sel.clone();
    sel2.push(SelectionItem { kind: "tag".into(), key: invoices.clone(), files: "preferred".into() });
    c.apply_selection(&sel2).await.unwrap();
    let t2 = Instant::now();
    let rep3 = c.sync_once().await.unwrap();
    let cached: i64 = sqlx::query_scalar("SELECT count(*) FROM file WHERE variant IN ('original','archive') AND state = 'complete'")
        .fetch_one(c.replica().pool())
        .await
        .unwrap();
    report(&e, "files-invoices", json!({"seconds": t2.elapsed().as_secs_f64(), "report": rep3, "cached_pdfs": cached}));
    assert!(cached > 0, "no files cached: {rep3:?}");

    // --- Server changes (directly in the copy, like a foreign writer/Django): title, trash
    let (doc_a, doc_b): (i64, i64) = sqlx::query_as(
        "SELECT min(id)::int8, max(id)::int8 FROM documents_document WHERE id BETWEEN 10 AND 60 AND deleted_at IS NULL AND root_document_id IS NULL",
    )
    .fetch_one(&pg)
    .await
    .unwrap();
    sqlx::query("UPDATE documents_document SET title = 'P5 E2E geändert', modified = now() WHERE id = $1")
        .bind(doc_a)
        .execute(&pg)
        .await
        .unwrap();
    sqlx::query("UPDATE documents_document SET deleted_at = now() WHERE id = $1").bind(doc_b).execute(&pg).await.unwrap();
    let r = c.replica().clone();
    sync_until(&c, || {
        let r = r.clone();
        async move {
            let t: Option<String> =
                sqlx::query_scalar("SELECT title FROM document WHERE legacy_id = ?").bind(doc_a).fetch_optional(r.pool()).await.unwrap();
            let gone: i64 =
                sqlx::query_scalar("SELECT count(*) FROM document WHERE legacy_id = ?").bind(doc_b).fetch_one(r.pool()).await.unwrap();
            t.as_deref() == Some("P5 E2E geändert") && gone == 0
        }
    })
    .await;
    // Search finds the new title locally
    let gw = Gateway::new(c.clone());
    let s = body(&gw.handle(get("/local/search", Some("q=e2e%20ge%C3%A4ndert"))).await);
    assert_eq!(s["items"][0]["document"]["legacy_id"], doc_a);
    // Restore trash (server state for later runs)
    sqlx::query("UPDATE documents_document SET deleted_at = NULL WHERE id = $1").bind(doc_b).execute(&pg).await.unwrap();

    // --- Permission revocation (alice): document visible only via guardian object permission → permission gone → deleted
    let ca = client(&e, "p5-e2e-alice").await;
    ca.set_limits(Limits { max_bytes: 2 << 30, max_documents: 10_000 }).await.unwrap();
    if !ca.status().logged_in {
        ca.login("alice", Zeroizing::new("alice".into()), None).await.unwrap();
    }
    let aids = ca.remote().ids("document").await.unwrap();
    let asel: Vec<SelectionItem> =
        aids.items.iter().map(|(u, _)| SelectionItem { kind: "document".into(), key: u.clone(), files: "none".into() }).collect();
    ca.apply_selection(&asel).await.unwrap();
    ca.sync_once().await.unwrap();
    assert_eq!(ca.replica().count_documents(true).await.unwrap() as usize, aids.items.len());
    // Candidate: foreign document, visible to alice only via a user object permission
    let cand: Option<i64> = sqlx::query_scalar(
        "SELECT d.id::int8 FROM documents_document d
           JOIN guardian_userobjectpermission p ON p.object_pk = d.id::text AND p.user_id = 3
           JOIN auth_permission ap ON ap.id = p.permission_id AND ap.codename = 'view_document'
          WHERE d.owner_id IS NOT NULL AND d.owner_id <> 3 AND d.deleted_at IS NULL AND d.root_document_id IS NULL
            AND NOT EXISTS (SELECT 1 FROM guardian_groupobjectpermission g JOIN auth_user_groups ug ON ug.group_id = g.group_id
                            WHERE ug.user_id = 3 AND g.object_pk = d.id::text)
          ORDER BY d.id LIMIT 1",
    )
    .fetch_optional(&pg)
    .await
    .unwrap();
    let cand = cand.expect("seed has documents shared with alice by user permission");
    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM document WHERE legacy_id = ?").bind(cand).fetch_one(ca.replica().pool()).await.unwrap();
    assert_eq!(before, 1);
    let removed: Vec<(i64, i32, i32, i32)> = sqlx::query_as(
        "DELETE FROM guardian_userobjectpermission WHERE object_pk = $1 AND user_id = 3 RETURNING id::int8, content_type_id, permission_id, user_id",
    )
    .bind(cand.to_string())
    .fetch_all(&pg)
    .await
    .unwrap();
    let ra = ca.replica().clone();
    sync_until(&ca, || {
        let r = ra.clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM document WHERE legacy_id = ?").bind(cand).fetch_one(r.pool()).await.unwrap()
                == 0
        }
    })
    .await;
    // Recht wiederherstellen
    for (_, ct, perm, user) in &removed {
        sqlx::query(
            "INSERT INTO guardian_userobjectpermission (object_pk, content_type_id, permission_id, user_id) VALUES ($1, $2, $3, $4)",
        )
        .bind(cand.to_string())
        .bind(ct)
        .bind(perm)
        .bind(user)
        .execute(&pg)
        .await
        .unwrap();
    }
    report(&e, "changes", json!({"title_changed": doc_a, "trashed": doc_b, "revoked_for_alice": cand}));
}

#[tokio::test]
#[ignore = "needs the P5 test server STOPPED (after e2e_1_online)"]
async fn e2e_2_offline() {
    let e = e2e();
    let c = client(&e, "p5-e2e-admin").await;
    let err = c.sync_once().await.unwrap_err();
    assert!(err.is_offline(), "{err:?}");
    let gw = Gateway::new(c.clone());
    // Liste
    let r = gw.handle(get("/rs/api/v2/documents", Some("limit=25&count=true&sort=-created"))).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("x-paperrust-source"), Some("local"));
    let list = body(&r);
    assert!(list["count"].as_i64().unwrap() >= 5_000);
    assert_eq!(list["items"].as_array().unwrap().len(), 25);
    assert!(list["next_cursor"].is_string());
    // Suche
    let t = Instant::now();
    let r =
        gw.handle(post("/rs/api/v2/documents/query", json!({"filter": {"text": {"query": "invoice"}}, "count": true, "limit": 10}))).await;
    let search_ms = t.elapsed().as_millis();
    let hits = body(&r)["count"].as_i64().unwrap();
    assert!(hits > 0);
    // Open PDF (archive/original of an Invoices document)
    let doc: String = sqlx::query_scalar("SELECT doc FROM file WHERE variant IN ('archive','original') AND state = 'complete' LIMIT 1")
        .fetch_one(c.replica().pool())
        .await
        .unwrap();
    let r = gw.handle(get(&format!("/rs/api/v2/documents/{doc}/files/preview"), None)).await;
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let is_pdf = r.body.starts_with(b"%PDF");
    let d = body(&gw.handle(get(&format!("/rs/api/v2/documents/{doc}"), Some("include=content"))).await);
    assert!(is_pdf || d["mime_type"] != "application/pdf", "preview must be a PDF");
    // Schreiben gesperrt
    let mut req = post(&format!("/rs/api/v2/documents/{doc}"), json!({"title": "x"}));
    req.method = "PATCH".into();
    assert_eq!(gw.handle(req).await.status, 503);
    report(
        &e,
        "offline",
        json!({"list_count": list["count"], "search_hits": hits, "search_ms": search_ms, "pdf_bytes": r.body.len(),
                                 "pdf_doc": doc, "status": c.status()}),
    );
}

#[tokio::test]
#[ignore = "needs the P5 test server RUNNING again (after e2e_2_offline)"]
async fn e2e_3_reconnect() {
    let e = e2e();
    let c = client(&e, "p5-e2e-admin").await;
    // new process = restart of the client; server back → consistent with the server state
    let rep = c.sync_once().await.unwrap();
    let ids = c.remote().ids("document").await.unwrap();
    let local = c.replica().document_states().await.unwrap();
    let selected: std::collections::HashSet<String> = local.iter().filter(|d| d.2).map(|d| d.0.clone()).collect();
    let remote: std::collections::HashSet<String> = ids.items.iter().map(|(u, _)| u.clone()).collect();
    // everything selected exists on the server with the same rev
    let revs: std::collections::HashMap<&String, i64> = ids.items.iter().map(|(u, r)| (u, *r)).collect();
    for (k, rev, sel, _) in &local {
        if *sel {
            assert_eq!(revs.get(k), Some(rev), "doc {k} rev differs");
        }
    }
    assert!(selected.is_subset(&remote));
    assert_eq!(c.status().state, "idle");
    report(&e, "reconnect", json!({"report": rep, "selected": selected.len(), "remote_visible": remote.len()}));
}

#[tokio::test]
#[ignore = "cleanup of the P5 E2E profiles (keychain entries, replica)"]
async fn e2e_4_cleanup() {
    let e = e2e();
    for p in ["p5-e2e-admin", "p5-e2e-alice"] {
        let c = client(&e, p).await;
        let revoked = c.logout(true).await.unwrap();
        println!("P5-E2E cleanup {p}: revoked on server = {revoked}");
        let s = os_store();
        assert!(s.get(&pl_sync_client::secret::target_for(p)).unwrap().is_none());
        c.replica().close().await;
    }
}
