//! Mock counterpart following the v2 contract (docs/api/v2.yaml, docs/SYNC.md §3) for the client tests
//! (SYNC.md §9 "Client": reconciliation logic against a mock remote). Only 127.0.0.1, synthetic data.
#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

pub const USER: &str = "alice";
pub const PASS: &str = "alice-pass";
/// Second account on the same server (account switch in one profile).
pub const USER2: &str = "bob";
pub const PASS2: &str = "bob-pass";

pub fn sha(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// UUID from kind + number (deterministic).
pub fn uid(prefix: u8, n: u64) -> String {
    format!("{prefix:08x}-0000-4000-8000-{n:012x}")
}

#[derive(Debug, Clone)]
pub struct Obj {
    pub kind: String,
    pub uuid: String,
    pub legacy_id: i64,
    pub rev: i64,
    pub json: Value,
    pub deleted: bool,
    pub visible: bool,
    pub perm_seq: Option<i64>,
    pub seq: i64,
}

#[derive(Default)]
pub struct MockState {
    pub objs: BTreeMap<(String, String), Obj>,
    pub next_seq: i64,
    pub min_cursor: i64,
    pub server_uuid: String,
    pub perm_epoch: i64,
    pub feed: bool,
    pub content: HashMap<String, String>,
    pub notes: HashMap<String, Value>,
    pub files: HashMap<(String, String), Vec<u8>>,
    pub tokens: HashSet<String>,
    pub devices: HashMap<String, String>,
    pub views: HashMap<String, Vec<String>>,
    /// Saved views the server cannot evaluate (full-text rule → 501 when resolved alone).
    pub unsupported_views: HashSet<String>,
    /// Files without `Content-Length` (chunked transfer).
    pub chunked: bool,
    /// Protokoll: (Methode, Pfad+Query, Authorization vorhanden)
    pub log: Vec<(String, String, bool)>,
    pub auth_headers: Vec<String>,
    /// Range-Anfragen an Dateien
    pub ranges: Vec<String>,
    pub next_legacy: i64,
}

pub struct Mock {
    pub st: Mutex<MockState>,
    pub notify: Notify,
}

pub type M = Arc<Mock>;

impl Mock {
    pub fn new(feed: bool) -> M {
        Arc::new(Mock {
            st: Mutex::new(MockState {
                next_seq: 1,
                server_uuid: "11111111-2222-4333-8444-555555555555".into(),
                perm_epoch: 1,
                feed,
                next_legacy: 1,
                ..Default::default()
            }),
            notify: Notify::new(),
        })
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut MockState) -> R) -> R {
        let mut g = self.st.lock().unwrap();
        let r = f(&mut g);
        drop(g);
        self.notify.notify_waiters();
        r
    }

    /// Create/change an object (new seq, rev + 1).
    pub fn put(&self, kind: &str, uuid: &str, mut json: Value) {
        self.with(|s| {
            let seq = s.next_seq;
            s.next_seq += 1;
            let (legacy, rev, visible, perm) = match s.objs.get(&(kind.to_string(), uuid.to_string())) {
                Some(o) => (o.legacy_id, o.rev + 1, o.visible, o.perm_seq),
                None => {
                    let l = s.next_legacy;
                    s.next_legacy += 1;
                    (l, 1, true, None)
                }
            };
            json["uuid"] = json!(uuid);
            json["legacy_id"] = json!(legacy);
            json["rev"] = json!(rev);
            s.objs.insert(
                (kind.into(), uuid.into()),
                Obj { kind: kind.into(), uuid: uuid.into(), legacy_id: legacy, rev, json, deleted: false, visible, perm_seq: perm, seq },
            );
        });
    }

    pub fn remove(&self, kind: &str, uuid: &str) {
        self.with(|s| {
            let seq = s.next_seq;
            s.next_seq += 1;
            if let Some(o) = s.objs.get_mut(&(kind.into(), uuid.into())) {
                o.deleted = true;
                o.seq = seq;
            }
        });
    }

    /// Revoke/grant a permission (perm_seq).
    pub fn set_visible(&self, kind: &str, uuid: &str, visible: bool) {
        self.with(|s| {
            let seq = s.next_seq;
            s.next_seq += 1;
            if let Some(o) = s.objs.get_mut(&(kind.into(), uuid.into())) {
                o.visible = visible;
                o.perm_seq = Some(seq);
                o.seq = seq;
            }
        });
    }

    pub fn get(&self, kind: &str, uuid: &str) -> Option<Obj> {
        self.st.lock().unwrap().objs.get(&(kind.to_string(), uuid.to_string())).cloned()
    }

    /// Document with files (original always, archive with `archive`).
    pub fn put_doc(&self, uuid: &str, title: &str, tags: &[&str], corr: Option<&str>, content: &str, archive: bool) {
        let orig = format!("%PDF-1.4\n% original {uuid} {title}\n{}", "x".repeat(2048)).into_bytes();
        let arch = format!("%PDF-1.7\n% archive {uuid} {title}\n{}", "y".repeat(3072)).into_bytes();
        let thumb = format!("RIFF....WEBPVP8 {uuid}").into_bytes();
        let files = json!({
            "original": {"checksum": sha(&orig), "size": orig.len()},
            "archive": if archive { json!({"checksum": sha(&arch), "size": arch.len()}) } else { Value::Null },
            "thumbnail": {"etag": format!("W/\"{}\"", thumb.len())},
        });
        let doc = json!({"title": title, "created": "2024-03-01", "added": "2024-03-02T08:00:00Z",
            "modified": "2024-03-02T08:00:00Z", "correspondent": corr, "document_type": null, "storage_path": null,
            "tags": tags, "custom_fields": [], "asn": null, "owner": {"uuid": uid(8, 1), "username": USER},
            "page_count": 1, "mime_type": "application/pdf", "original_filename": format!("{title}.pdf"),
            "notes_count": 1, "files": files, "access": {"can_view": true, "can_change": true, "can_delete": true}});
        self.with(|s| {
            s.content.insert(uuid.into(), content.into());
            s.notes.insert(
                uuid.into(),
                json!([{"legacy_id": 1, "note": format!("Notiz zu {title}"), "created": "2024-03-02T08:00:00Z", "user": null}]),
            );
            s.files.insert((uuid.into(), "original".into()), orig);
            if archive {
                s.files.insert((uuid.into(), "archive".into()), arch);
            } else {
                s.files.remove(&(uuid.into(), "archive".into()));
            }
            s.files.insert((uuid.into(), "thumbnail".into()), thumb);
        });
        self.put("document", uuid, doc);
    }

    pub fn requests(&self) -> Vec<(String, String, bool)> {
        self.st.lock().unwrap().log.clone()
    }

    pub fn clear_log(&self) {
        self.st.lock().unwrap().log.clear();
    }
}

fn err(status: StatusCode, code: &str) -> Response {
    (status, axum::Json(json!({"error": {"code": code, "message": code}}))).into_response()
}

fn authed(m: &M, h: &HeaderMap) -> bool {
    let Some(a) = h.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()) else { return false };
    let Some(t) = a.strip_prefix("Bearer ") else { return false };
    m.st.lock().unwrap().tokens.contains(t)
}

macro_rules! need_auth {
    ($m:expr, $h:expr) => {
        if !authed(&$m, &$h) {
            return err(StatusCode::UNAUTHORIZED, "not_authenticated");
        }
    };
}

async fn log_mw(State(m): State<M>, req: Request<Body>, next: axum::middleware::Next) -> Response {
    let auth = req.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).map(str::to_string);
    {
        let mut s = m.st.lock().unwrap();
        let pq = req.uri().path_and_query().map(|p| p.as_str().to_string()).unwrap_or_default();
        s.log.push((req.method().to_string(), pq, auth.is_some()));
        if let Some(a) = auth {
            s.auth_headers.push(a);
        }
    }
    next.run(req).await
}

async fn server(State(m): State<M>, h: HeaderMap) -> Response {
    let s = m.st.lock().unwrap();
    let mut features = vec!["device_tokens", "files_range", "documents_query"];
    if s.feed {
        features.push("sync");
    }
    let mut body = json!({"product": "paperrust", "api": 2, "features": features});
    drop(s);
    if authed(&m, &h) {
        let s = m.st.lock().unwrap();
        body["version"] = json!("0.9.0-mock");
        if s.feed {
            body["server_uuid"] = json!(s.server_uuid);
            body["min_cursor"] = json!(format!("c1.{}", s.min_cursor));
            body["perm_epoch"] = json!(s.perm_epoch);
        } else {
            body["server_uuid"] = Value::Null;
            body["min_cursor"] = Value::Null;
            body["perm_epoch"] = Value::Null;
        }
    }
    axum::Json(body).into_response()
}

async fn create_device(State(m): State<M>, axum::Json(b): axum::Json<Value>) -> Response {
    let known = (b["username"] == USER && b["password"] == PASS) || (b["username"] == USER2 && b["password"] == PASS2);
    if !known {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": {"code": "invalid", "message": "Unable to log in with provided credentials."}})),
        )
            .into_response();
    }
    let token = format!("prt_{}", uuid::Uuid::new_v4().simple());
    let dev = uuid::Uuid::new_v4().to_string();
    let mut s = m.st.lock().unwrap();
    s.tokens.insert(token.clone());
    s.devices.insert(dev.clone(), token.clone());
    (StatusCode::CREATED, axum::Json(json!({"token": token, "device": {"uuid": dev, "name": b["device_name"], "created": "2024-01-01T00:00:00Z", "last_used": null}})))
        .into_response()
}

async fn revoke_device(State(m): State<M>, h: HeaderMap, Path(u): Path<String>) -> Response {
    need_auth!(m, h);
    let mut s = m.st.lock().unwrap();
    match s.devices.remove(&u) {
        Some(t) => {
            s.tokens.remove(&t);
            StatusCode::NO_CONTENT.into_response()
        }
        None => err(StatusCode::NOT_FOUND, "not_found"),
    }
}

fn doc_out(s: &MockState, o: &Obj, content: bool, notes: bool) -> Value {
    let mut v = o.json.clone();
    if o.kind == "document" {
        if content {
            v["content"] = json!(s.content.get(&o.uuid).cloned().unwrap_or_default());
        }
        if notes {
            v["notes"] = s.notes.get(&o.uuid).cloned().unwrap_or(json!([]));
        }
    }
    v
}

async fn changes(State(m): State<M>, h: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    need_auth!(m, h);
    if !m.st.lock().unwrap().feed {
        return err(StatusCode::NOT_IMPLEMENTED, "not_implemented");
    }
    let cursor: Option<i64> = match q.get("cursor") {
        None => None,
        Some(c) => match c.strip_prefix("c1.").and_then(|n| n.parse().ok()) {
            Some(n) => Some(n),
            None => return err(StatusCode::BAD_REQUEST, "invalid"),
        },
    };
    if let Some(c) = cursor
        && c < m.st.lock().unwrap().min_cursor
    {
        return err(StatusCode::GONE, "cursor_expired");
    }
    let limit: usize = q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(500);
    let kinds: Option<Vec<String>> = q.get("kinds").map(|k| k.split(',').map(str::to_string).collect());
    let wait: u64 = q.get("wait").and_then(|w| w.parse().ok()).unwrap_or(0).min(30);
    let after = cursor.unwrap_or(0);
    if wait > 0 {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(wait);
        loop {
            let notified = m.notify.notified();
            if m.st.lock().unwrap().objs.values().any(|o| o.seq > after) {
                break;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                break;
            }
        }
    }
    let s = m.st.lock().unwrap();
    let mut entries: Vec<&Obj> =
        s.objs.values().filter(|o| o.seq > after).filter(|o| kinds.as_ref().is_none_or(|k| k.contains(&o.kind))).collect();
    entries.sort_by_key(|o| o.seq);
    let has_more = entries.len() > limit;
    entries.truncate(limit);
    let next = entries.last().map(|o| o.seq).unwrap_or(after);
    let mut out = Vec::new();
    for o in entries {
        let del = json!({"seq": o.seq, "kind": o.kind, "uuid": o.uuid, "op": "delete"});
        if o.deleted {
            out.push(del);
        } else if o.visible {
            out.push(
                json!({"seq": o.seq, "kind": o.kind, "uuid": o.uuid, "op": "upsert", "rev": o.rev, "object": doc_out(&s, o, false, false)}),
            );
        } else if o.perm_seq.is_some_and(|p| p > after) {
            out.push(del);
        }
    }
    axum::Json(json!({"changes": out, "next_cursor": format!("c1.{next}"), "has_more": has_more, "perm_epoch": s.perm_epoch}))
        .into_response()
}

fn live<'a>(s: &'a MockState, kind: &'a str) -> impl Iterator<Item = &'a Obj> + 'a {
    s.objs.values().filter(move |o| o.kind == kind && !o.deleted && o.visible)
}

async fn ids(State(m): State<M>, h: HeaderMap, Query(q): Query<HashMap<String, String>>) -> Response {
    need_auth!(m, h);
    let s = m.st.lock().unwrap();
    let kind = q.get("kind").cloned().unwrap_or_default();
    let items: Vec<Value> = live(&s, &kind).map(|o| json!([o.uuid, o.rev])).collect();
    let as_of = if s.feed { json!(s.next_seq - 1) } else { Value::Null };
    axum::Json(json!({"items": items, "as_of_seq": as_of})).into_response()
}

async fn resolve(State(m): State<M>, h: HeaderMap, axum::Json(b): axum::Json<Value>) -> Response {
    need_auth!(m, h);
    let s = m.st.lock().unwrap();
    let strs = |k: &str| -> Vec<String> {
        b[k].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
    };
    let (docs, tags, views) = (strs("documents"), strs("tags"), strs("saved_views"));
    // like pl-store: unsupported views are skipped, only a request of nothing but those → 501
    let (skipped, views): (Vec<String>, Vec<String>) = views.into_iter().partition(|v| s.unsupported_views.contains(v));
    if views.is_empty() && docs.is_empty() && tags.is_empty() && !strs("saved_views").is_empty() {
        return (
            StatusCode::NOT_IMPLEMENTED,
            axum::Json(
                json!({"error": {"code": "not_implemented", "message": "saved view uses full-text rule 20 which cannot be resolved yet"}}),
            ),
        )
            .into_response();
    }
    let mut out = Vec::new();
    for o in live(&s, "document") {
        let otags: Vec<&str> = o.json["tags"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let hit = docs.contains(&o.uuid)
            || tags.iter().any(|t| otags.contains(&t.as_str()))
            || views.iter().any(|v| s.views.get(v).is_some_and(|l| l.contains(&o.uuid)));
        if hit {
            let size = |v: &str| s.files.get(&(o.uuid.clone(), v.to_string())).map(|b| b.len());
            out.push(json!({"uuid": o.uuid, "rev": o.rev, "size": {"original": size("original"), "archive": size("archive")}}));
        }
    }
    let as_of = if s.feed { json!(s.next_seq - 1) } else { Value::Null };
    axum::Json(json!({"documents": out, "as_of_seq": as_of, "skipped_saved_views": skipped})).into_response()
}

async fn batch(State(m): State<M>, h: HeaderMap, axum::Json(b): axum::Json<Value>) -> Response {
    need_auth!(m, h);
    let s = m.st.lock().unwrap();
    let kind = b["kind"].as_str().unwrap_or("").to_string();
    let inc: Vec<&str> = b["include"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let mut items = Vec::new();
    let mut missing = Vec::new();
    for u in b["uuids"].as_array().cloned().unwrap_or_default() {
        let u = u.as_str().unwrap_or("").to_string();
        match s.objs.get(&(kind.clone(), u.clone())).filter(|o| !o.deleted && o.visible) {
            Some(o) => items.push(doc_out(&s, o, inc.contains(&"content"), inc.contains(&"notes"))),
            None => missing.push(u),
        }
    }
    axum::Json(json!({"items": items, "missing": missing})).into_response()
}

async fn me(State(m): State<M>, h: HeaderMap) -> Response {
    need_auth!(m, h);
    axum::Json(json!({"uuid": uid(8, 1), "legacy_id": 3, "username": USER, "first_name": "", "last_name": "", "email": "",
        "is_superuser": false, "is_staff": false, "groups": [], "permissions": [], "auth": "device_token"}))
    .into_response()
}

async fn ui_settings(State(m): State<M>, h: HeaderMap) -> Response {
    need_auth!(m, h);
    axum::Json(json!({"settings": {"language": "de-de"}})).into_response()
}

async fn list(State(m): State<M>, h: HeaderMap, Path(plural): Path<String>) -> Response {
    need_auth!(m, h);
    let kind = plural.trim_end_matches('s').to_string();
    let s = m.st.lock().unwrap();
    let items: Vec<Value> = live(&s, &kind).map(|o| o.json.clone()).collect();
    axum::Json(json!({"items": items, "next_cursor": null, "source_mock": true})).into_response()
}

async fn file(State(m): State<M>, h: HeaderMap, Path((u, variant)): Path<(String, String)>) -> Response {
    need_auth!(m, h);
    let s = m.st.lock().unwrap();
    let Some(o) = s.objs.get(&("document".to_string(), u.clone())).filter(|o| !o.deleted && o.visible) else {
        return err(StatusCode::NOT_FOUND, "not_found");
    };
    let Some(data) = s.files.get(&(u.clone(), variant.clone())).cloned() else { return err(StatusCode::NOT_FOUND, "not_found") };
    let etag = if variant == "thumbnail" { format!("W/\"{}\"", data.len()) } else { format!("\"{}\"", sha(&data)) };
    let _ = o;
    if h.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) == Some(etag.as_str()) {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    let range = h.get(header::RANGE).and_then(|v| v.to_str().ok()).map(str::to_string);
    let if_range_ok = h.get(header::IF_RANGE).and_then(|v| v.to_str().ok()).is_none_or(|v| v == etag);
    drop(s);
    if let Some(r) = range {
        m.st.lock().unwrap().ranges.push(r.clone());
        if if_range_ok
            && let Some(start) = r.strip_prefix("bytes=").and_then(|x| x.strip_suffix('-')).and_then(|x| x.parse::<usize>().ok())
            && start < data.len()
        {
            let body = data[start..].to_vec();
            return (
                StatusCode::PARTIAL_CONTENT,
                [
                    (header::ETAG, etag),
                    (header::CONTENT_RANGE, format!("bytes {start}-{}/{}", data.len() - 1, data.len())),
                    (header::ACCEPT_RANGES, "bytes".into()),
                ],
                body,
            )
                .into_response();
        }
    }
    if m.st.lock().unwrap().chunked {
        let chunks: Vec<Result<bytes::Bytes, std::io::Error>> = data.chunks(512).map(|c| Ok(bytes::Bytes::copy_from_slice(c))).collect();
        return (StatusCode::OK, [(header::ETAG, etag)], Body::from_stream(futures::stream::iter(chunks))).into_response();
    }
    (StatusCode::OK, [(header::ETAG, etag), (header::ACCEPT_RANGES, "bytes".into())], data).into_response()
}

async fn v1_ui_settings(State(m): State<M>, h: HeaderMap) -> Response {
    need_auth!(m, h);
    axum::Json(json!({"user": {"id": 3, "username": USER}, "settings": {"from": "server"}, "permissions": ["view_document"]}))
        .into_response()
}

pub fn router(m: M) -> Router {
    Router::new()
        .route("/api/ui_settings/", get(v1_ui_settings))
        .route("/rs/api/v2/server", get(server))
        .route("/rs/api/v2/me", get(me))
        .route("/rs/api/v2/ui_settings", get(ui_settings))
        .route("/rs/api/v2/auth/devices", post(create_device))
        .route("/rs/api/v2/auth/devices/{uuid}", delete(revoke_device))
        .route("/rs/api/v2/sync/changes", get(changes))
        .route("/rs/api/v2/sync/ids", get(ids))
        .route("/rs/api/v2/sync/resolve", post(resolve))
        .route("/rs/api/v2/objects/batch", post(batch))
        .route("/rs/api/v2/documents/{uuid}/files/{variant}", get(file))
        .route("/rs/api/v2/{plural}", get(list))
        .layer(axum::middleware::from_fn_with_state(m.clone(), log_mw))
        .with_state(m)
}

/// Running server; `stop()` closes the listener **and** all open connections (offline).
pub struct Running {
    pub addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Running {
    pub fn url(&self, https: bool) -> String {
        format!("{}://127.0.0.1:{}/", if https { "https" } else { "http" }, self.addr.port())
    }

    pub fn stop(&self) {
        self.task.abort();
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn serve(router: Router, tls: Option<tokio_rustls::TlsAcceptor>) -> Running {
    serve_on(router, tls, "127.0.0.1:0").await
}

pub async fn serve_on(router: Router, tls: Option<tokio_rustls::TlsAcceptor>, bind: &str) -> Running {
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use hyper_util::service::TowerToHyperService;
    let listener = tokio::net::TcpListener::bind(bind).await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut conns = tokio::task::JoinSet::new();
        loop {
            let Ok((stream, _)) = listener.accept().await else { continue };
            let svc = TowerToHyperService::new(router.clone());
            let tls = tls.clone();
            conns.spawn(async move {
                let b = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());
                match tls {
                    Some(acc) => {
                        if let Ok(s) = acc.accept(stream).await {
                            let _ = b.serve_connection(TokioIo::new(s), svc).await;
                        }
                    }
                    None => {
                        let _ = b.serve_connection(TokioIo::new(stream), svc).await;
                    }
                }
            });
            while conns.try_join_next().is_some() {}
        }
    });
    Running { addr, task }
}

/// Test data: 2 tags, 1 correspondent, `n` documents (even → tag A, every third with archive).
pub fn seed(m: &M, n: u64) {
    m.put(
        "tag",
        &uid(2, 1),
        json!({"name": "Steuern", "color": "#a6cee3", "text_color": "#000000", "is_inbox_tag": false,
        "matching": {"algorithm": "none", "pattern": "", "is_insensitive": true}, "parent": null, "owner": null,
        "access": {"can_view": true, "can_change": true, "can_delete": true}}),
    );
    m.put(
        "tag",
        &uid(2, 2),
        json!({"name": "Auto", "color": "#b2df8a", "text_color": "#000000", "is_inbox_tag": false,
        "matching": {"algorithm": "none", "pattern": "", "is_insensitive": true}, "parent": null, "owner": null,
        "access": {"can_view": true, "can_change": true, "can_delete": true}}),
    );
    m.put(
        "correspondent",
        &uid(3, 1),
        json!({"name": "Steueramt Zürich", "matching": {"algorithm": "none", "pattern": "", "is_insensitive": true},
        "owner": null, "access": {"can_view": true, "can_change": true, "can_delete": true}}),
    );
    m.put("user", &uid(8, 1), json!({"username": USER, "first_name": "", "last_name": ""}));
    for i in 1..=n {
        let tags: Vec<String> = if i % 2 == 0 { vec![uid(2, 1)] } else { vec![uid(2, 2)] };
        let tags: Vec<&str> = tags.iter().map(String::as_str).collect();
        let corr = if i % 5 == 0 { Some(uid(3, 1)) } else { None };
        m.put_doc(
            &uid(1, i),
            &format!("Rechnung {i:05}"),
            &tags,
            corr.as_deref(),
            &format!("Steuerbescheid Nummer {i} fällig am Monatsende"),
            i % 3 == 0,
        );
    }
}

/// `/ws/status/` like pl-server (device token `Bearer prt_…`, otherwise 403); immediately sends a
/// `status_update` message and afterwards the echo of every text message (path window → server).
async fn ws_status(State(m): State<M>, h: HeaderMap, ws: axum::extract::ws::WebSocketUpgrade) -> Response {
    use axum::extract::ws::Message;
    if !authed(&m, &h) || h.contains_key(header::COOKIE) || h.contains_key(header::ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.on_upgrade(|mut s| async move {
        let hello = json!({"type": "status_update", "data": {"filename": "a.pdf", "task_id": "t1", "current_progress": 1,
            "max_progress": 100, "status": "STARTED", "message": "new_file", "document_id": null}});
        if s.send(Message::Text(hello.to_string().into())).await.is_err() {
            return;
        }
        while let Some(Ok(msg)) = s.recv().await {
            if let Message::Text(t) = msg
                && s.send(Message::Text(format!("echo:{}", t.as_str()).into())).await.is_err()
            {
                break;
            }
        }
    })
}

/// Mock with `/ws/status/` via `axum::serve` (upgrades; without TLS).
pub async fn serve_with_ws(m: M) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let app = router(m.clone()).merge(Router::new().route("/ws/status/", get(ws_status)).with_state(m));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (addr, task)
}
