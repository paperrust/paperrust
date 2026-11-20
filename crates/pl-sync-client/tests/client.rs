//! SYNC.md §9 "Client" (+ parts of "Sicherheit"): reconciliation logic against a mock remote following the
//! v2 contract, FTS5 search, limits, file cache resume, gateway online/offline.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use pl_sync_client::secret::{MemoryStore, SecretStore, target_for};
use pl_sync_client::selection::Limits;
use pl_sync_client::store::SelectionItem;
use pl_sync_client::tls::TlsOptions;
use pl_sync_client::{Error, Gateway, GwRequest, ProfileConfig, SyncClient};
use serde_json::{Value, json};
use zeroize::Zeroizing;

struct Env {
    mock: M,
    srv: Running,
    client: SyncClient,
    secrets: Arc<MemoryStore>,
    dir: tempfile::TempDir,
}

async fn env(feed: bool, n: u64) -> Env {
    let mock = Mock::new(feed);
    seed(&mock, n);
    let srv = serve(router(mock.clone()), None).await;
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemoryStore::default());
    let client = open_client(&srv.url(false), dir.path(), secrets.clone()).await;
    Env { mock, srv, client, secrets, dir }
}

async fn open_client(url: &str, dir: &std::path::Path, secrets: Arc<MemoryStore>) -> SyncClient {
    let cfg = ProfileConfig {
        profile_id: "p1".into(),
        url: url.into(),
        tls: TlsOptions::default(),
        dir: dir.join("profiles").join("p1"),
        device_name: "Test-Laptop".into(),
        flavor: "paperrust".into(),
    };
    let c = SyncClient::open(cfg, secrets).await.unwrap();
    c.set_resolve_throttle(Duration::ZERO);
    c
}

fn tag_sel(n: u64, files: &str) -> SelectionItem {
    SelectionItem { kind: "tag".into(), key: uid(2, n), files: files.into() }
}

async fn login(c: &SyncClient) {
    c.login(USER, Zeroizing::new(PASS.to_string()), None).await.unwrap();
}

fn get(path: &str, query: Option<&str>) -> GwRequest {
    GwRequest {
        method: "GET".into(),
        path: path.into(),
        query: query.map(str::to_string),
        headers: vec![("Origin".into(), "http://tauri.localhost".into())],
        body: vec![],
    }
}

fn post(path: &str, body: Value) -> GwRequest {
    GwRequest {
        method: "POST".into(),
        path: path.into(),
        query: None,
        headers: vec![("content-type".into(), "application/json".into())],
        body: serde_json::to_vec(&body).unwrap(),
    }
}

fn body(r: &pl_sync_client::GwResponse) -> Value {
    serde_json::from_slice(&r.body).unwrap()
}

#[tokio::test]
async fn login_stores_token_only_in_keychain() {
    let e = env(true, 2).await;
    // falsches Passwort
    let bad = e.client.login(USER, Zeroizing::new("nope".into()), None).await.unwrap_err();
    assert!(matches!(bad, Error::Api { status: 400, .. }), "{bad:?}");
    assert!(e.secrets.get(&target_for("p1")).unwrap().is_none());
    login(&e.client).await;
    let token = e.secrets.get(&target_for("p1")).unwrap().unwrap();
    assert!(token.starts_with("prt_"));
    // Keychain holds token + device UUID; replica gone → revocation still works
    let (_, dev) = token.split_once('\n').unwrap();
    assert!(e.mock.st.lock().unwrap().devices.contains_key(dev));
    // Token never in a URL
    assert!(e.mock.requests().iter().all(|(_, pq, _)| !pq.contains("prt_")));
    // Reopening loads the token from the keychain
    let c2 = open_client(&e.srv.url(false), e.dir.path(), e.secrets.clone()).await;
    assert!(c2.status().logged_in);
    assert!(c2.remote().server().await.unwrap().server_uuid.is_some());
    // Profile with deleted replica: revocation via the UUID from the keychain
    let fresh = tempfile::tempdir().unwrap();
    let c3 = open_client(&e.srv.url(false), fresh.path(), e.secrets.clone()).await;
    assert!(c3.logout(true).await.unwrap(), "revoked without the old replica");
    assert!(e.mock.st.lock().unwrap().devices.is_empty());
}

#[tokio::test]
async fn initial_sync_then_incremental_feed() {
    let e = env(true, 30).await;
    login(&e.client).await;
    // Selection: tag "Steuern" (even documents) with preferred files
    let plan = e.client.apply_selection(&[tag_sel(1, "preferred")]).await.unwrap();
    assert_eq!(plan.total_documents, 15);
    let rep = e.client.sync_once().await.unwrap();
    assert!(rep.full);
    assert_eq!(rep.documents_fetched, 15);
    let r = e.client.replica();
    assert_eq!(r.count_documents(true).await.unwrap(), 15);
    assert_eq!(r.count_objects("tag").await.unwrap(), 2);
    assert_eq!(r.count_objects("correspondent").await.unwrap(), 1);
    // Content + notes + files (archive for every third, otherwise original) + thumbnail
    let d6 = r.document_full(&uid(1, 6), true, true).await.unwrap().unwrap();
    assert!(d6["content"].as_str().unwrap().contains("Steuerbescheid Nummer 6"));
    assert_eq!(d6["notes"][0]["note"], "Notiz zu Rechnung 00006");
    assert!(r.file(&uid(1, 6), "archive").await.unwrap().is_some());
    assert!(r.file(&uid(1, 6), "original").await.unwrap().is_none());
    assert!(r.file(&uid(1, 2), "original").await.unwrap().is_some());
    assert!(r.file(&uid(1, 2), "thumb").await.unwrap().is_some());
    assert_eq!(rep.files_downloaded, 30);
    let st = e.client.status();
    assert_eq!(st.state, "idle");
    assert_eq!(st.mode, "feed");
    assert_eq!(st.documents, 15);

    // second run: nothing to do (no files again, no contents)
    e.mock.clear_log();
    let rep = e.client.sync_once().await.unwrap();
    assert_eq!(rep.documents_fetched, 0);
    assert_eq!(rep.files_downloaded, 0);
    assert!(!e.mock.requests().iter().any(|(_, p, _)| p.contains("/files/original") || p.contains("/files/archive")));

    // Server changes: title/content of a selected document, deletes one, revokes a permission,
    // renames the tag, a previously unselected document gets the tag
    e.mock.put_doc(&uid(1, 2), "Rechnung geändert", &[&uid(2, 1)], None, "Neuer Inhalt Mahnung", false);
    e.mock.remove("document", &uid(1, 4));
    e.mock.set_visible("document", &uid(1, 8), false);
    let mut tag = e.mock.get("tag", &uid(2, 1)).unwrap().json;
    tag["name"] = json!("Abgaben");
    e.mock.put("tag", &uid(2, 1), tag);
    e.mock.put_doc(&uid(1, 3), "Rechnung 00003", &[&uid(2, 2), &uid(2, 1)], None, "jetzt ausgewählt", true);

    let rep = e.client.sync_once().await.unwrap();
    assert!(rep.feed_changes >= 5, "{rep:?}");
    assert_eq!(r.count_documents(true).await.unwrap(), 14, "15 - gelöscht - entzogen + neu");
    assert!(r.document_state(&uid(1, 4)).await.unwrap().is_none());
    assert!(r.document_state(&uid(1, 8)).await.unwrap().is_none());
    assert!(!e.client.files().path_for(&uid(1, 4), "original").exists(), "files of deleted doc removed");
    let d2 = r.document_full(&uid(1, 2), true, false).await.unwrap().unwrap();
    assert_eq!(d2["title"], "Rechnung geändert");
    assert_eq!(d2["content"], "Neuer Inhalt Mahnung");
    // new checksum → file reloaded and matching
    let f = r.file(&uid(1, 2), "original").await.unwrap().unwrap();
    assert_eq!(f.checksum.as_deref(), d2["files"]["original"]["checksum"].as_str());
    assert!(r.document_state(&uid(1, 3)).await.unwrap().is_some());
    // Search: rename and new content in the index
    let gw = Gateway::new(e.client.clone());
    let s = body(&gw.handle(get("/local/search", Some("q=mahnung"))).await);
    assert_eq!(s["items"].as_array().unwrap().len(), 1);
    let q = gw
        .handle(GwRequest { query: Some("source=local".into()), ..post("/rs/api/v2/documents/query", json!({"filter": {"text": {"query": "abgaben"}}, "count": true})) })
        .await;
    assert_eq!(body(&q)["count"], 14);
}

#[tokio::test]
async fn deselect_keeps_metadata_but_drops_files() {
    let e = env(true, 10).await;
    login(&e.client).await;
    e.client.apply_selection(&[tag_sel(1, "both"), SelectionItem { kind: "document".into(), key: uid(1, 1), files: "none".into() }]).await.unwrap();
    e.client.sync_once().await.unwrap();
    let r = e.client.replica();
    assert_eq!(r.count_documents(true).await.unwrap(), 6);
    assert!(r.file(&uid(1, 6), "original").await.unwrap().is_some());
    assert!(r.file(&uid(1, 6), "archive").await.unwrap().is_some());
    assert!(r.file(&uid(1, 1), "original").await.unwrap().is_none(), "files=none");
    assert!(r.file(&uid(1, 1), "thumb").await.unwrap().is_some(), "thumbnails always");
    // only document 1 left
    e.client.apply_selection(&[SelectionItem { kind: "document".into(), key: uid(1, 1), files: "none".into() }]).await.unwrap();
    e.client.sync_once().await.unwrap();
    assert_eq!(r.count_documents(true).await.unwrap(), 1);
    assert_eq!(r.count_documents(false).await.unwrap(), 6, "metadata kept for 7 days");
    assert!(r.file(&uid(1, 6), "original").await.unwrap().is_none());
    assert!(!e.client.files().path_for(&uid(1, 6), "archive").exists());
    // gone after 7 days
    sqlx::query("UPDATE document SET left_selection = '2000-01-01T00:00:00Z' WHERE in_selection = 0").execute(r.pool()).await.unwrap();
    e.client.sync_once().await.unwrap();
    assert_eq!(r.count_documents(false).await.unwrap(), 1);
}

#[tokio::test]
async fn limits_refuse_selection() {
    let e = env(true, 20).await;
    login(&e.client).await;
    e.client.set_limits(Limits { max_bytes: 10_000, max_documents: 100 }).await.unwrap();
    // Tag A: 10 documents, preferred ≈ 10 × 2–3 KB > 10 000 bytes
    let p = e.client.preview_selection(&[tag_sel(1, "preferred")]).await.unwrap();
    assert!(!p.within_limits);
    assert!(p.total_bytes > 10_000);
    let err = e.client.apply_selection(&[tag_sel(1, "preferred")]).await.unwrap_err();
    assert!(matches!(err, Error::LimitExceeded(_)));
    assert!(e.client.replica().selection().await.unwrap().is_empty(), "nothing applied");
    // without files it fits (counts only documents)
    e.client.apply_selection(&[tag_sel(1, "none")]).await.unwrap();
    e.client.set_limits(Limits { max_bytes: 10_000, max_documents: 5 }).await.unwrap();
    assert!(matches!(e.client.apply_selection(&[tag_sel(1, "none")]).await, Err(Error::LimitExceeded(_))));
    // Gateway: 422 limit_exceeded with hint
    let gw = Gateway::new(e.client.clone());
    let mut req = post("/local/selection", json!({"items": [{"type": "tag", "key": uid(1, 1), "files": "none"}, {"type": "tag", "key": uid(2, 1), "files": "none"}]}));
    req.method = "PUT".into();
    let r = gw.handle(req).await;
    assert_eq!(r.status, 422);
    assert_eq!(body(&r)["error"]["code"], "limit_exceeded");
}

#[tokio::test]
async fn cursor_expired_and_foreign_server_trigger_full_resync() {
    let e = env(true, 6).await;
    login(&e.client).await;
    e.client.apply_selection(&[tag_sel(1, "none")]).await.unwrap();
    e.client.sync_once().await.unwrap();
    assert_eq!(e.client.replica().count_documents(true).await.unwrap(), 3);
    // Retention on the server: cursor too old → 410 → full sync, same result
    e.mock.remove("document", &uid(1, 2));
    e.mock.with(|s| s.min_cursor = s.next_seq + 10);
    let rep = e.client.sync_once().await.unwrap();
    assert!(rep.full);
    assert_eq!(e.client.replica().count_documents(true).await.unwrap(), 2);
    // foreign server_uuid (DB copy) → replica anew
    e.mock.with(|s| {
        s.server_uuid = "99999999-2222-4333-8444-555555555555".into();
        s.min_cursor = 0;
    });
    let rep = e.client.sync_once().await.unwrap();
    assert!(rep.full);
    assert_eq!(e.client.replica().count_documents(true).await.unwrap(), 2);
    assert_eq!(e.client.replica().remote().await.unwrap().unwrap().server_uuid.as_deref(), Some("99999999-2222-4333-8444-555555555555"));
}

#[tokio::test]
async fn polling_mode_without_feed() {
    // Server without feed (/sync/changes → 501, as it is today until P2 is hooked in)
    let e = env(false, 8).await;
    login(&e.client).await;
    e.client.apply_selection(&[tag_sel(2, "none")]).await.unwrap();
    e.client.sync_once().await.unwrap();
    assert_eq!(e.client.status().mode, "polling");
    assert_eq!(e.client.replica().count_documents(true).await.unwrap(), 4);
    assert_eq!(e.client.replica().count_objects("tag").await.unwrap(), 2);
    assert!(!e.mock.requests().iter().any(|(_, p, _)| p.contains("sync/changes")));
    // Changes detected via ids/resolve
    e.mock.remove("document", &uid(1, 1));
    e.mock.put_doc(&uid(1, 3), "Neu 3", &[&uid(2, 2)], None, "abc", false);
    e.mock.remove("tag", &uid(2, 1));
    e.client.sync_once().await.unwrap();
    let r = e.client.replica();
    assert_eq!(r.count_documents(true).await.unwrap(), 3);
    assert_eq!(r.document_json(&uid(1, 3)).await.unwrap().unwrap()["title"], "Neu 3");
    assert_eq!(r.count_objects("tag").await.unwrap(), 1);
}

#[tokio::test]
async fn file_cache_resume_and_checksum() {
    let e = env(true, 3).await;
    login(&e.client).await;
    e.client.apply_selection(&[SelectionItem { kind: "document".into(), key: uid(1, 3), files: "original".into() }]).await.unwrap();
    e.client.sync_once().await.unwrap();
    let r = e.client.replica();
    let doc = r.document_json(&uid(1, 3)).await.unwrap().unwrap();
    // Simulate abort: partial state with 100 bytes + row `partial`
    let target = e.client.files().path_for(&uid(1, 3), "original");
    let full = std::fs::read(&target).unwrap();
    std::fs::remove_file(&target).unwrap();
    let mut part = target.clone().into_os_string();
    part.push(".part");
    std::fs::write(&part, &full[..100]).unwrap();
    let mut row = r.file(&uid(1, 3), "original").await.unwrap().unwrap();
    row.state = "partial".into();
    row.etag = Some(format!("\"{}\"", sha(&full)));
    r.put_file(&row).await.unwrap();
    let got = e.client.files().fetch(r, pl_sync_client::files::Source::PaperRust(e.client.remote()), &doc, "original", true).await.unwrap();
    assert_eq!(got, pl_sync_client::files::Fetched::Resumed(100, full.len() as u64));
    assert_eq!(std::fs::read(&target).unwrap(), full);
    assert!(e.mock.st.lock().unwrap().ranges.contains(&"bytes=100-".to_string()));
    // File changed on the server, partial state outdated → If-Range does not match → 200 → completely new
    std::fs::remove_file(&target).unwrap();
    std::fs::write(&part, b"stale bytes").unwrap();
    let mut row = r.file(&uid(1, 3), "original").await.unwrap().unwrap();
    row.state = "partial".into();
    row.etag = Some("\"0000\"".into());
    r.put_file(&row).await.unwrap();
    let got = e.client.files().fetch(r, pl_sync_client::files::Source::PaperRust(e.client.remote()), &doc, "original", true).await.unwrap();
    assert_eq!(got, pl_sync_client::files::Fetched::Downloaded(full.len() as u64));
    assert_eq!(std::fs::read(&target).unwrap(), full);
    // Checksum wrong → error, no file
    e.mock.with(|s| s.files.insert((uid(1, 3), "original".into()), b"%PDF-tampered".to_vec()));
    r.delete_file(&uid(1, 3), "original").await.unwrap();
    std::fs::remove_file(&target).unwrap();
    let err = e.client.files().fetch(r, pl_sync_client::files::Source::PaperRust(e.client.remote()), &doc, "original", true).await.unwrap_err();
    assert!(matches!(err, Error::Checksum(_)));
    assert!(!target.exists());
    // Thumbnail: 304 via If-None-Match
    e.client.files().fetch(r, pl_sync_client::files::Source::PaperRust(e.client.remote()), &doc, "thumb", true).await.unwrap();
    let again = e.client.files().fetch(r, pl_sync_client::files::Source::PaperRust(e.client.remote()), &doc, "thumb", true).await.unwrap();
    assert_eq!(again, pl_sync_client::files::Fetched::Unchanged);
}

#[tokio::test]
async fn gateway_online_offline_switch() {
    let e = env(true, 12).await;
    login(&e.client).await;
    e.client.apply_selection(&[tag_sel(1, "preferred")]).await.unwrap();
    e.client.sync_once().await.unwrap();
    let gw = Gateway::new(e.client.clone());

    // online: forwarded, token from the core, Authorization/cookie from the WebView discarded
    let mut req = get("/rs/api/v2/tags", None);
    req.headers.push(("Authorization".into(), "Token stolen".into()));
    req.headers.push(("Cookie".into(), "sessionid=x".into()));
    e.mock.clear_log();
    let r = gw.handle(req).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("x-paperrust-source"), Some("remote"));
    assert_eq!(body(&r)["source_mock"], true);
    let auths = e.mock.st.lock().unwrap().auth_headers.clone();
    assert!(auths.last().unwrap().starts_with("Bearer prt_"));
    assert!(!auths.iter().any(|a| a.contains("stolen")));
    // gesperrte Pfade
    assert_eq!(gw.handle(post("/rs/api/v2/auth/devices", json!({"username": USER, "password": PASS, "device_name": "x"}))).await.status, 403);
    assert_eq!(gw.handle(get("/api/token/", None)).await.status, 403);
    // Page paths outside app/API = links into the classic UI → notice page
    let r = gw.handle(get("/etc/passwd", None)).await;
    assert_eq!((r.status, r.header("content-security-policy").map(|c| c.starts_with("default-src 'none'"))), (200, Some(true)));
    assert_eq!(gw.handle(get("/rs/other", None)).await.status, 404);
    // ?source=local: from the replica
    let r = gw.handle(get("/rs/api/v2/tags", Some("source=local"))).await;
    assert_eq!(r.header("x-paperrust-source"), Some("local"));
    assert_eq!(body(&r)["items"].as_array().unwrap().len(), 2);

    // Server weg → offline
    e.srv.stop();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let err = e.client.sync_once().await.unwrap_err();
    assert!(err.is_offline(), "{err:?}");
    assert_eq!(e.client.status().state, "offline");
    // Liste
    let r = gw.handle(get("/rs/api/v2/documents", Some("sort=title&limit=50&count=true"))).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("x-paperrust-source"), Some("local"));
    let list = body(&r);
    assert_eq!(list["count"], 6);
    assert_eq!(list["items"][0]["title"], "Rechnung 00002");
    // Search (FTS5): prefix, umlaut without accent, correspondent
    let r = gw.handle(post("/rs/api/v2/documents/query", json!({"filter": {"text": {"query": "fallig"}}, "count": true}))).await;
    assert_eq!(body(&r)["count"], 6);
    let r = gw.handle(post("/rs/api/v2/documents/query", json!({"filter": {"text": {"query": "steueramt zur"}}, "count": true}))).await;
    assert_eq!(body(&r)["count"], 1, "doc 10");
    let r = gw.handle(get("/local/search", Some("q=rechnung%2000004"))).await;
    assert_eq!(body(&r)["items"][0]["document"]["title"], "Rechnung 00004");
    // Document with content
    let r = gw.handle(get(&format!("/rs/api/v2/documents/{}", uid(1, 6)), Some("include=content,notes"))).await;
    assert!(body(&r)["content"].as_str().unwrap().contains("Nummer 6"));
    // Open PDF (preview → archive if present), Range, 304
    let r = gw.handle(get(&format!("/rs/api/v2/documents/{}/files/preview", uid(1, 6)), None)).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.header("content-type"), Some("application/pdf"));
    assert!(r.body.starts_with(b"%PDF-1.7"), "archive served");
    let etag = r.header("etag").unwrap().to_string();
    let mut req = get(&format!("/rs/api/v2/documents/{}/files/preview", uid(1, 6)), None);
    req.headers.push(("If-None-Match".into(), etag.clone()));
    assert_eq!(gw.handle(req).await.status, 304);
    let mut req = get(&format!("/rs/api/v2/documents/{}/files/original", uid(1, 4)), None);
    req.headers.push(("Range".into(), "bytes=0-7".into()));
    let r = gw.handle(req).await;
    assert_eq!(r.status, 206);
    assert_eq!(r.body, b"%PDF-1.4");
    // not selected / not in the cache
    assert_eq!(gw.handle(get(&format!("/rs/api/v2/documents/{}", uid(1, 1)), None)).await.status, 404);
    let r = gw.handle(get(&format!("/rs/api/v2/documents/{}/files/archive", uid(1, 4)), None)).await;
    assert_eq!(body(&r)["error"]["code"], "not_cached");
    // Writing blocked offline, v1 not available offline
    let mut req = post(&format!("/rs/api/v2/documents/{}", uid(1, 6)), json!({"title": "x"}));
    req.method = "PATCH".into();
    let r = gw.handle(req).await;
    assert_eq!(r.status, 503);
    assert_eq!(body(&r)["error"]["code"], "offline");
    // v1 offline from the replica (light UI), see v1_offline_for_the_light_ui
    assert_eq!(gw.handle(get("/api/documents/", None)).await.header("x-paperrust-source"), Some("local"));
    assert_eq!(gw.handle(get("/api/mail_rules/", None)).await.status, 503);
    // Status
    let st = body(&gw.handle(get("/local/status", None)).await);
    assert_eq!(st["source"], "local");
    assert_eq!(st["documents"], 6);
}

#[tokio::test]
async fn long_poll_run_loop_follows_changes() {
    let e = env(true, 4).await;
    login(&e.client).await;
    e.client.apply_selection(&[tag_sel(1, "none")]).await.unwrap();
    let (tx, rx) = tokio::sync::watch::channel(false);
    let c = e.client.clone();
    let task = tokio::spawn(async move { c.run(rx).await });
    // Erstabgleich abwarten
    let r = e.client.replica().clone();
    wait_until(|| {
        let r = r.clone();
        async move { r.count_documents(true).await.unwrap() == 2 }
    })
    .await;
    // Change on the server appears without intervention (long poll)
    e.mock.put_doc(&uid(1, 2), "Live geändert", &[&uid(2, 1)], None, "x", false);
    let r2 = r.clone();
    wait_until(|| {
        let r = r2.clone();
        async move { r.document_json(&uid(1, 2)).await.unwrap().is_some_and(|d| d["title"] == "Live geändert") }
    })
    .await;
    let m = e.mock.clone();
    wait_until(|| {
        let m = m.clone();
        async move { m.requests().iter().any(|(_, p, _)| p.contains("wait=")) }
    })
    .await;
    tx.send(true).unwrap();
    e.client.wake();
    tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap();
}

async fn wait_until<F, Fut>(f: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    for _ in 0..200 {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("condition not reached in 10 s");
}

/// SYNC.md §9 "Sicherheit": token never in the replica/profile folder, logging out revokes on the server.
#[tokio::test]
async fn token_never_on_disk_and_logout_revokes() {
    let e = env(true, 6).await;
    login(&e.client).await;
    let token = e.secrets.get(&target_for("p1")).unwrap().unwrap().split('\n').next().unwrap().to_string();
    e.client.apply_selection(&[tag_sel(1, "preferred")]).await.unwrap();
    e.client.sync_once().await.unwrap();
    e.client.replica().pool().execute_wal_checkpoint().await;
    let profile = e.dir.path().join("profiles").join("p1");
    let hits = grep_dir(&profile, token.as_bytes());
    assert!(hits.is_empty(), "token found in {hits:?}");
    assert!(grep_dir(&profile, PASS.as_bytes()).is_empty(), "password found on disk");
    // Log out: device revoked on the server, keychain empty, replica deleted
    let gw = Gateway::new(e.client.clone());
    let r = gw.handle(post("/local/logout", json!({"wipe": true}))).await;
    assert_eq!(body(&r)["revoked"], true);
    assert!(e.secrets.get(&target_for("p1")).unwrap().is_none());
    assert!(!e.mock.st.lock().unwrap().tokens.contains(&token));
    assert_eq!(e.client.replica().count_documents(false).await.unwrap(), 0);
    assert!(!profile.join("files").exists());
    // widerrufenes Token → 401 → auth_required
    e.client.remote().set_token(Some(Zeroizing::new(token)));
    let err = e.client.sync_once().await.unwrap_err();
    assert!(matches!(err, Error::Unauthenticated));
    assert_eq!(e.client.status().state, "auth_required");
}

fn grep_dir(dir: &std::path::Path, needle: &[u8]) -> Vec<String> {
    let mut hits = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for ent in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = ent.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(b) = std::fs::read(&p)
                && b.windows(needle.len()).any(|w| w == needle)
            {
                hits.push(p.display().to_string());
            }
        }
    }
    hits
}

trait Checkpoint {
    async fn execute_wal_checkpoint(&self);
}

impl Checkpoint for sqlx::SqlitePool {
    async fn execute_wal_checkpoint(&self) {
        let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)").execute(self).await;
    }
}

/// All entry points that the shell calls from `tauri::async_runtime::spawn` must be `Send`
/// (rust-lang/rust#102211 – only a compile test; runs briefly against the mock).
#[tokio::test]
async fn entry_points_are_spawnable() {
    let mock = Mock::new(true);
    seed(&mock, 1);
    let srv = serve(router(mock.clone()), None).await;
    let dir = tempfile::tempdir().unwrap();
    let url = srv.url(false);
    let path = dir.path().to_path_buf();
    let h = tokio::spawn(async move {
        let secrets: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
        let cfg = ProfileConfig { profile_id: "s".into(), url, tls: TlsOptions::default(), dir: path, device_name: "x".into(), flavor: "paperrust".into() };
        let c = SyncClient::open(cfg, secrets).await.unwrap();
        c.login(USER, Zeroizing::new(PASS.into()), None).await.unwrap();
        let gw = Gateway::new(c.clone());
        let _ = gw.handle(get("/local/status", None)).await;
        let _ = c.apply_selection(&[tag_sel(1, "none")]).await;
        let _ = c.sync_once().await;
        let _ = c.logout(false).await;
        c
    });
    let c = h.await.unwrap();
    let (tx, rx) = tokio::sync::watch::channel(true);
    tokio::spawn(c.run(rx)).await.unwrap();
    drop(tx);
}

/// Mode 2b: light UI (v1) via the gateway – online forwarded + framework cached,
/// offline list/filter/search/document/thumbnail/PDF from the replica in v1 form.
#[tokio::test]
async fn v1_offline_for_the_light_ui() {
    let e = env(true, 12).await;
    login(&e.client).await;
    e.client.apply_selection(&[tag_sel(1, "preferred")]).await.unwrap();
    e.client.sync_once().await.unwrap();
    let app = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(app.path().join("assets")).unwrap();
    std::fs::write(app.path().join("index.html"), "<pngx-root></pngx-root>").unwrap();
    let gw = Gateway::new(e.client.clone()).with_app_dir(Some(app.path().to_path_buf()));
    // online: forwarded and remembered for offline
    let r = gw.handle(get("/api/ui_settings/", None)).await;
    assert_eq!(r.header("x-paperrust-source"), Some("remote"));
    assert_eq!(body(&r)["settings"]["from"], "server");
    // app pages and own login page
    assert_eq!(gw.handle(get("/rs/app/documents", None)).await.body, b"<pngx-root></pngx-root>");
    assert_eq!(gw.handle(get("/accounts/login/", Some("next=%2Frs%2Fapp%2F"))).await.status, 200);

    e.srv.stop();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let _ = e.client.sync_once().await;
    let r = gw.handle(get("/api/ui_settings/", None)).await;
    assert_eq!(r.header("x-paperrust-source"), Some("local"));
    assert_eq!(body(&r)["settings"]["from"], "server", "cached response");
    // master data synthesized (no cache for this URL)
    let tags = body(&gw.handle(get("/api/tags/", Some("page=1&page_size=100000"))).await);
    assert_eq!(tags["count"], 2);
    let steuern = tags["results"].as_array().unwrap().iter().find(|t| t["name"] == "Steuern").unwrap().clone();
    assert_eq!(steuern["document_count"], 6);
    let tag_id = steuern["id"].as_i64().unwrap();
    // List as the light UI requests it
    let q = format!("page=1&page_size=4&ordering=-created&truncate_content=true&include_selection_data=true&tags__id__all={tag_id}");
    let l = body(&gw.handle(get("/api/documents/", Some(&q))).await);
    assert_eq!(l["count"], 6);
    assert_eq!(l["results"].as_array().unwrap().len(), 4);
    assert_eq!(l["all"].as_array().unwrap().len(), 6);
    assert!(l["next"].as_str().unwrap().contains("page=2"));
    assert_eq!(l["results"][0]["tags"][0], tag_id);
    assert!(l["selection_data"]["selected_tags"].as_array().unwrap().iter().any(|t| t["id"] == tag_id && t["document_count"] == 6));
    // Full text (query=) and sort by title
    let l = body(&gw.handle(get("/api/documents/", Some("page=1&page_size=25&ordering=title&query=steuerbescheid%20nummer%2010"))).await);
    assert_eq!(l["results"][0]["title"], "Rechnung 00010");
    // Page out of range → 404 like DRF
    assert_eq!(gw.handle(get("/api/documents/", Some("page=9&page_size=25"))).await.status, 404);
    // Dokument, Notizen, Vorschaubild, PDF
    let id = l["results"][0]["id"].as_i64().unwrap();
    let d = body(&gw.handle(get(&format!("/api/documents/{id}/"), Some("full_perms=true"))).await);
    assert!(d["content"].as_str().unwrap().contains("Nummer 10"));
    assert_eq!(d["notes"][0]["note"], "Notiz zu Rechnung 00010");
    let f = body(&gw.handle(get(&format!("/api/documents/{id}/"), Some("fields=id,title"))).await);
    assert_eq!(f.as_object().unwrap().len(), 2);
    assert_eq!(gw.handle(get(&format!("/api/documents/{id}/thumb/"), None)).await.status, 200);
    let pdf = gw.handle(get(&format!("/api/documents/{id}/preview/"), None)).await;
    assert_eq!(pdf.status, 200);
    assert!(pdf.body.starts_with(b"%PDF"));
    let dl = gw.handle(get(&format!("/api/documents/{id}/download/"), Some("original=true"))).await;
    assert!(dl.header("content-disposition").unwrap().starts_with("attachment"));
    // not selected → 404, writing → 503
    assert_eq!(gw.handle(get("/api/documents/1/", None)).await.status, 404);
    let mut w = post(&format!("/api/documents/{id}/notes/"), json!({"note": "x"}));
    w.method = "POST".into();
    assert_eq!(gw.handle(w).await.status, 503);
}

// ---------------------------------------------------------------- Modus 2b: klassische Links, /ws/status/

#[tokio::test]
async fn classic_links_online_point_to_the_server_offline_explain() {
    let e = env(true, 2).await;
    login(&e.client).await;
    e.client.sync_once().await.unwrap();
    let gw = Gateway::new(e.client.clone());
    // online: target on the server (the shell opens it in the system browser), notice page with link
    let want = format!("{}documents/12/notes?x=1", e.srv.url(false));
    assert_eq!(gw.classic_url("/documents/12/notes", Some("x=1")).as_deref(), Some(want.as_str()));
    assert_eq!(gw.classic_url("/api/documents/", None), None);
    assert_eq!(gw.classic_url("/rs/app/documents", None), None);
    let r = gw.handle(get("/documents/12/notes", Some("x=1"))).await;
    assert_eq!(r.status, 200);
    assert!(String::from_utf8_lossy(&r.body).contains(&format!("href=\"{want}\"")));
    assert!(r.header("content-security-policy").unwrap().starts_with("default-src 'none'"));
    // There is no writing on such paths
    assert_eq!(gw.handle(post("/documents/12/notes", json!({}))).await.status, 404);
    // offline: only available online, no target
    gw.handle(post("/local/mode", json!({"force_offline": true}))).await;
    assert_eq!(gw.classic_url("/documents/12/notes", None), None);
    let r = gw.handle(get("/documents/12/notes", None)).await;
    assert_eq!(r.status, 503);
    assert!(String::from_utf8_lossy(&r.body).contains("Nur online verfügbar"));
}

/// Raw WebSocket handshake to the relay; response head + rest.
async fn ws_handshake(port: u16, target: &str, origin: &str) -> (tokio::net::TcpStream, String, Vec<u8>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let req = format!(
        "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nOrigin: {origin}\r\n\
         Cookie: sessionid=stolen\r\nAuthorization: Token stolen\r\n\r\n"
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let rest = buf.split_off(i + 4);
            return (s, String::from_utf8_lossy(&buf).into_owned(), rest);
        }
        let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut chunk)).await.unwrap().unwrap();
        if n == 0 {
            return (s, String::from_utf8_lossy(&buf).into_owned(), vec![]);
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Einen unmaskierten Textrahmen (Server → Client) lesen.
async fn ws_read_text(s: &mut tokio::net::TcpStream, mut have: Vec<u8>) -> String {
    use tokio::io::AsyncReadExt;
    let mut chunk = [0u8; 1024];
    loop {
        if have.len() >= 4 {
            assert_eq!(have[0], 0x81, "text frame");
            let (len, off) = match have[1] & 0x7f {
                126 => (u16::from_be_bytes([have[2], have[3]]) as usize, 4),
                n => (n as usize, 2),
            };
            if have.len() >= off + len {
                return String::from_utf8(have[off..off + len].to_vec()).unwrap();
            }
        }
        let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut chunk)).await.unwrap().unwrap();
        assert!(n > 0, "closed");
        have.extend_from_slice(&chunk[..n]);
    }
}

/// Maskierten Textrahmen (Client → Server, < 126 Bytes) schreiben.
async fn ws_write_text(s: &mut tokio::net::TcpStream, text: &str) {
    use tokio::io::AsyncWriteExt;
    let mask = [1u8, 2, 3, 4];
    let mut f = vec![0x81, 0x80 | text.len() as u8];
    f.extend_from_slice(&mask);
    f.extend(text.bytes().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    s.write_all(&f).await.unwrap();
}

#[tokio::test]
async fn ws_status_relay_online_with_device_token_offline_503() {
    use pl_sync_client::wsrelay;
    let mock = Mock::new(true);
    seed(&mock, 2);
    let (addr, server) = serve_with_ws(mock.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let client = open_client(&format!("http://127.0.0.1:{}/", addr.port()), dir.path(), Arc::new(MemoryStore::default())).await;
    login(&client).await;
    client.sync_once().await.unwrap();
    let app = tempfile::tempdir().unwrap();
    std::fs::write(app.path().join("index.html"), "<html>app</html>").unwrap();
    let gw = Arc::new(Gateway::new(client.clone()).with_app_dir(Some(app.path().to_path_buf())));
    // without relay: no address, CSP without connect-src
    assert_eq!(body(&gw.handle(get("/local/ws", None)).await)["url"], Value::Null);
    assert!(!gw.handle(get("/rs/app/", None)).await.header("content-security-policy").unwrap().contains("connect-src"));
    let (listener, info) = wsrelay::bind().await.unwrap();
    gw.set_ws_relay(info.clone());
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
    let origins = vec!["http://prapi.localhost".to_string()];
    let relay = tokio::spawn(wsrelay::serve(listener, info.clone(), gw.clone(), origins, stop_rx));

    let st = body(&gw.handle(get("/local/ws", None)).await);
    let url = st["url"].as_str().unwrap().to_string();
    assert_eq!(url, info.url());
    assert_eq!(st["online"], true);
    // App pages may reach the relay
    let page = gw.handle(get("/rs/app/", None)).await;
    assert!(page.header("content-security-policy").unwrap().ends_with(&format!("connect-src 'self' ws://127.0.0.1:{}", info.port)));
    let target = url.trim_start_matches(&format!("ws://127.0.0.1:{}", info.port)).to_string();

    // foreign origin / wrong key → 403, without asking the server
    let (_, head, _) = ws_handshake(info.port, &target, "https://evil.example").await;
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");
    let (_, head, _) = ws_handshake(info.port, "/ws/status/?k=nope", "http://prapi.localhost").await;
    assert!(head.starts_with("HTTP/1.1 403"), "{head}");

    // online: 101, message of the server arrives, messages pass through; the server saw Bearer
    // instead of cookie/token/origin of the window (otherwise 403 in the mock)
    let (mut s, head, rest) = ws_handshake(info.port, &target, "http://prapi.localhost").await;
    assert!(head.starts_with("HTTP/1.1 101"), "{head}");
    assert!(head.to_ascii_lowercase().contains("sec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo="), "{head}");
    let msg = ws_read_text(&mut s, rest).await;
    assert!(msg.contains("\"status_update\"") && msg.contains("a.pdf"), "{msg}");
    ws_write_text(&mut s, "ping").await;
    assert_eq!(ws_read_text(&mut s, vec![]).await, "echo:ping");

    // offline (forced): 503, the light UI tries again later
    gw.handle(post("/local/mode", json!({"force_offline": true}))).await;
    let (_, head, _) = ws_handshake(info.port, &target, "http://prapi.localhost").await;
    assert!(head.starts_with("HTTP/1.1 503"), "{head}");
    assert_eq!(body(&gw.handle(get("/local/ws", None)).await)["online"], false);
    // Server weg → ebenfalls 503
    gw.handle(post("/local/mode", json!({"force_offline": false}))).await;
    server.abort();
    let _ = server.await;
    drop(s);
    let (_, head, _) = ws_handshake(info.port, &target, "http://prapi.localhost").await;
    assert!(head.starts_with("HTTP/1.1 503"), "{head}");

    stop_tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), relay).await.unwrap().unwrap();
}
