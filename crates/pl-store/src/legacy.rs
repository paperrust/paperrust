//! Write paths via the v1 handlers (transition, ARCHITECTURE-V2 §1: reuse of parity-checked
//! logic only **behind** the core interface).
//!
//! `pl-server` passes a [`LegacyRouter`] with the v1 routes (`tags.create`,
//! `documents.partial_update`, …). A v2 write access is translated here into an internal v1 request with
//! the **same login context** (no second login, CSRF has already been checked);
//! validation, permissions, audit log, workflows, index and file names thus remain identical to v1.
//! The v1 response is evaluated only for success/error, the result is re-read from the DB.

use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt as _;
use pl_domain::model::*;
use pl_domain::{DomainError, DomainResult, Kind, Principal};
use serde_json::{Map, Value, json};
use sqlx::PgPool;
use tower::ServiceExt as _;
use uuid::Uuid;

use crate::{Store, auth, db_err, documents, ident, objects};

/// Router with the v1 write handlers (state and `Authz` extension already set).
#[derive(Clone)]
pub struct LegacyRouter(axum::Router);

impl LegacyRouter {
    pub fn new(router: axum::Router) -> Self {
        LegacyRouter(router)
    }

    /// The internal v1 router (for in-process read accesses, e.g. `/ask` in pl-api-v2).
    pub fn router(&self) -> axum::Router {
        self.0.clone()
    }
}

/// v1 path of the list of a kind.
fn v1_base(kind: Kind) -> DomainResult<&'static str> {
    match kind {
        Kind::Tag => Ok("/api/tags/"),
        Kind::Correspondent => Ok("/api/correspondents/"),
        Kind::DocumentType => Ok("/api/document_types/"),
        other => Err(DomainError::NotImplemented(format!("writing {} is not available in API v2 yet", other.plural()))),
    }
}

/// One internal v1 request; returns the JSON body on 2xx.
async fn call(store: &Store, p: &Principal, method: Method, path: &str, body: Option<Value>) -> DomainResult<Value> {
    let Some(router) = store.legacy.clone() else {
        return Err(DomainError::NotImplemented("write path not mounted".into()));
    };
    let ext = auth::ext(p)?;
    let mut ctx = (*ext.ctx).clone();
    ctx.http_method = method.clone();
    let bytes = body.map(|b| serde_json::to_vec(&b).unwrap_or_default()).unwrap_or_default();
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "localhost")
        .header(header::ACCEPT, "application/json; version=10")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(bytes))
        .map_err(DomainError::internal)?;
    req.extensions_mut().insert(Arc::new(ctx));
    if let Some(peer) = ext.peer {
        req.extensions_mut().insert(axum::extract::ConnectInfo(peer));
    }
    let resp = match router.0.oneshot(req).await {
        Ok(r) => r,
        Err(e) => match e {},
    };
    if pl_core::markers::is_delegation(&resp) {
        return Err(DomainError::NotImplemented("this change is only supported by the classic API".into()));
    }
    let status = resp.status();
    let raw = resp.into_body().collect().await.map_err(DomainError::internal)?.to_bytes();
    let v: Value = if raw.is_empty() { Value::Null } else { serde_json::from_slice(&raw).unwrap_or(Value::Null) };
    if status.is_success() {
        return Ok(v);
    }
    Err(map_error(status, v))
}

/// v1 error → domain error (field names mapped to v2).
fn map_error(status: StatusCode, v: Value) -> DomainError {
    let detail = v.get("detail").and_then(Value::as_str).map(str::to_string);
    match status {
        StatusCode::BAD_REQUEST => {
            let mut fields = Map::new();
            let mut message = "Invalid input.".to_string();
            match v {
                Value::Object(m) => {
                    for (k, val) in m {
                        if k == "non_field_errors"
                            && let Some(first) = val.as_array().and_then(|a| a.first()).and_then(Value::as_str)
                        {
                            message = first.to_string();
                        }
                        let key = match k.as_str() {
                            "archive_serial_number" => "asn",
                            "match" => "matching.pattern",
                            "matching_algorithm" => "matching.algorithm",
                            "is_insensitive" => "matching.is_insensitive",
                            other => other,
                        };
                        fields.insert(key.to_string(), val);
                    }
                }
                Value::Array(a) => {
                    if let Some(first) = a.first().and_then(Value::as_str) {
                        message = first.to_string();
                    }
                }
                _ => {}
            }
            DomainError::Invalid { message, fields }
        }
        StatusCode::UNAUTHORIZED => DomainError::Unauthenticated,
        StatusCode::FORBIDDEN => {
            DomainError::Forbidden(detail.unwrap_or_else(|| "You do not have permission to perform this action.".into()))
        }
        StatusCode::NOT_FOUND => DomainError::NotFound,
        StatusCode::CONFLICT => DomainError::Conflict(detail.unwrap_or_default()),
        s => DomainError::Internal(format!("v1 write path answered {s}: {v}")),
    }
}

fn check_rev(if_match: Option<i64>, current: i64) -> DomainResult<()> {
    match if_match {
        Some(r) if r != current => Err(DomainError::RevMismatch { current }),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------------------------
// Atomic `If-Match` check and v1 write (contract pl-api-v2, "Identität und rev").
//
// A row lock (`SELECT … FOR UPDATE`) on the object is not possible: the v1 handler writes the
// same row on another pool connection, so it would wait for our lock while we wait for its
// response. Instead every v2 write takes a transaction-scoped advisory lock per object, re-reads
// `rev` while holding it and calls v1 inside the lock. Two v2 writers of one object are thus
// serialized: the second one sees the rev bumped by the first and gets 412.
// ---------------------------------------------------------------------------------------------

/// A held per-object write lock.
trait ObjectLock {
    /// Releases the lock (awaited, so the next writer is not left waiting for a lazy rollback).
    async fn release(self);
}

/// Advisory lock held in a transaction on a connection of the lock side pool.
struct PgObjectLock(sqlx::Transaction<'static, sqlx::Postgres>);

impl ObjectLock for PgObjectLock {
    async fn release(self) {
        if let Err(e) = self.0.rollback().await {
            // A failed rollback closes the connection, which also ends the lock.
            tracing::warn!(error = %e, "releasing the v2 write lock failed");
        }
    }
}

/// Connections of the lock side pool per database. The lock connection idles while the v1
/// handler works on the general pool; taking it from that pool could starve the v1 handler
/// (small pools) – hence an own small pool (`pl_core::dbconn::side_pool`, FINDINGS B2).
static LOCK_POOLS: OnceLock<Mutex<Vec<(String, PgPool)>>> = OnceLock::new();
/// Concurrent lock holders/waiters per process; more writers queue on the pool acquire.
const LOCK_POOL_MAX: u32 = 4;

fn lock_pool(db: &PgPool) -> PgPool {
    let o = db.connect_options();
    let key = format!("{}|{}|{}|{}", o.get_host(), o.get_port(), o.get_database().unwrap_or(""), o.get_username());
    let pools = LOCK_POOLS.get_or_init(|| Mutex::new(Vec::new()));
    let mut g = match pools.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some((_, p)) = g.iter().find(|(k, _)| *k == key) {
        return p.clone();
    }
    let p = pl_core::dbconn::side_pool(db, LOCK_POOL_MAX);
    g.push((key, p.clone()));
    p
}

/// Advisory lock key of an object (same scheme as the consumer locks: `hashtextextended`).
fn lock_key(kind: Kind, id: i64) -> String {
    format!("paperrust:v2write:{}:{id}", kind.code())
}

async fn lock_object(store: &Store, kind: Kind, id: i64) -> DomainResult<PgObjectLock> {
    let mut tx = lock_pool(&store.db).begin().await.map_err(db_err)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(lock_key(kind, id))
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
    Ok(PgObjectLock(tx))
}

/// Current `rev` of a live object (read after the lock was granted).
async fn current_rev(store: &Store, kind: Kind, uuid: Uuid) -> DomainResult<i64> {
    ident::resolve_one(&store.db, kind, uuid).await.map(|(_, rev)| rev)
}

/// Check-and-write under a held lock: `current_rev` is awaited only now (futures are lazy), the
/// revision compared, then `write` runs; the lock is released afterwards in every case.
async fn locked_write<L, R, W, WF, T>(lock: L, current_rev: R, if_match: Option<i64>, write: W) -> DomainResult<T>
where
    L: ObjectLock,
    R: Future<Output = DomainResult<i64>>,
    W: FnOnce() -> WF,
    WF: Future<Output = DomainResult<T>>,
{
    let result = async {
        check_rev(if_match, current_rev.await?)?;
        write().await
    }
    .await;
    lock.release().await;
    result
}

async fn object_body(store: &Store, kind: Kind, w: &ObjectWrite) -> DomainResult<Value> {
    let mut m = Map::new();
    if let Some(n) = &w.name {
        m.insert("name".into(), json!(n));
    }
    if let Some(mw) = &w.matching {
        if let Some(alg) = &mw.algorithm {
            let code = matching_code(alg).ok_or_else(|| {
                DomainError::field("matching.algorithm", format!("Unknown algorithm \"{alg}\" ({}).", MATCHING_ALGORITHMS.join(", ")))
            })?;
            m.insert("matching_algorithm".into(), json!(code));
        }
        if let Some(pat) = &mw.pattern {
            m.insert("match".into(), json!(pat));
        }
        if let Some(i) = mw.is_insensitive {
            m.insert("is_insensitive".into(), json!(i));
        }
    }
    let only = |field: &str, allowed: bool| -> DomainResult<()> {
        if allowed { Ok(()) } else { Err(DomainError::field(field, format!("Not a field of {}.", kind.as_str()))) }
    };
    if let Some(c) = &w.color {
        only("color", kind == Kind::Tag)?;
        m.insert("color".into(), json!(c));
    }
    if let Some(b) = w.is_inbox_tag {
        only("is_inbox_tag", kind == Kind::Tag)?;
        m.insert("is_inbox_tag".into(), json!(b));
    }
    if let Some(parent) = &w.parent {
        only("parent", kind == Kind::Tag)?;
        let v = match parent {
            Some(u) => json!(ident::ref_ids(&store.db, Kind::Tag, "parent", &[*u]).await?[0]),
            None => Value::Null,
        };
        m.insert("parent".into(), v);
    }
    if let Some(path) = &w.path {
        only("path", kind == Kind::StoragePath)?;
        m.insert("path".into(), json!(path));
    }
    if let Some(owner) = &w.owner {
        let v = match owner {
            Some(u) => json!(ident::ref_ids(&store.db, Kind::User, "owner", &[*u]).await?[0]),
            None => Value::Null,
        };
        m.insert("owner".into(), v);
    }
    Ok(Value::Object(m))
}

async fn reload(store: &Store, p: &Principal, kind: Kind, id: i64) -> DomainResult<Object> {
    let mut v = objects::by_legacy_ids(store, p, kind, &[id]).await?;
    v.pop().and_then(|(_, o)| o).ok_or(DomainError::NotFound)
}

pub(crate) async fn create(store: &Store, p: &Principal, kind: Kind, w: &ObjectWrite) -> DomainResult<Object> {
    let base = v1_base(kind)?;
    let body = object_body(store, kind, w).await?;
    let v = call(store, p, Method::POST, base, Some(body)).await?;
    let id = v.get("id").and_then(Value::as_i64).ok_or_else(|| DomainError::Internal("v1 create without id".into()))?;
    reload(store, p, kind, id).await
}

/// Visible? (404 before any revision check, so that `412` reveals nothing.)
async fn visible(store: &Store, p: &Principal, kind: Kind, uuid: Uuid) -> DomainResult<(i64, i64)> {
    let (id, rev) = ident::resolve_one(&store.db, kind, uuid).await?;
    let user = auth::user(p)?;
    crate::perm::require(user, crate::perm::spec(kind).view_perm)?;
    match objects::by_legacy_ids(store, p, kind, &[id]).await?.pop() {
        Some((_, Some(_))) => Ok((id, rev)),
        _ => Err(DomainError::NotFound),
    }
}

pub(crate) async fn update(
    store: &Store,
    p: &Principal,
    kind: Kind,
    uuid: Uuid,
    if_match: Option<i64>,
    w: &ObjectWrite,
) -> DomainResult<Object> {
    let base = v1_base(kind)?;
    let (id, rev) = visible(store, p, kind, uuid).await?;
    // Early check keeps the error order (412 before body validation); the binding check
    // follows under the lock.
    check_rev(if_match, rev)?;
    let body = object_body(store, kind, w).await?;
    let lock = lock_object(store, kind, id).await?;
    let path = format!("{base}{id}/");
    locked_write(lock, current_rev(store, kind, uuid), if_match, || call(store, p, Method::PATCH, &path, Some(body))).await?;
    reload(store, p, kind, id).await
}

pub(crate) async fn delete(store: &Store, p: &Principal, kind: Kind, uuid: Uuid, if_match: Option<i64>) -> DomainResult<()> {
    let base = v1_base(kind)?;
    let (id, rev) = visible(store, p, kind, uuid).await?;
    check_rev(if_match, rev)?;
    let lock = lock_object(store, kind, id).await?;
    let path = format!("{base}{id}/");
    locked_write(lock, current_rev(store, kind, uuid), if_match, || call(store, p, Method::DELETE, &path, None)).await?;
    Ok(())
}

pub(crate) async fn patch_document(
    store: &Store,
    p: &Principal,
    uuid: Uuid,
    if_match: Option<i64>,
    patch: &DocumentPatch,
) -> DomainResult<Document> {
    let user = auth::user(p)?;
    let (id, rev) = documents::visible_one(store, user, uuid).await?;
    check_rev(if_match, rev)?;
    let db = &store.db;
    let mut m = Map::new();
    if let Some(t) = &patch.title {
        m.insert("title".into(), json!(t));
    }
    if let Some(c) = &patch.created {
        m.insert("created".into(), json!(c.to_string()));
    }
    for (field, kind, val) in [
        ("correspondent", Kind::Correspondent, &patch.correspondent),
        ("document_type", Kind::DocumentType, &patch.document_type),
        ("storage_path", Kind::StoragePath, &patch.storage_path),
        ("owner", Kind::User, &patch.owner),
    ] {
        if let Some(v) = val {
            let j = match v {
                Some(u) => json!(ident::ref_ids(db, kind, field, &[*u]).await?[0]),
                None => Value::Null,
            };
            m.insert(field.into(), j);
        }
    }
    if let Some(tags) = &patch.tags {
        m.insert("tags".into(), json!(ident::ref_ids(db, Kind::Tag, "tags", tags).await?));
    }
    if let Some(asn) = &patch.asn {
        m.insert("archive_serial_number".into(), json!(asn));
    }
    if let Some(cfs) = &patch.custom_fields {
        let fields: Vec<Uuid> = cfs.iter().map(|c| c.field).collect();
        let ids = ident::ref_ids(db, Kind::CustomField, "custom_fields", &fields).await?;
        let mut out = Vec::new();
        for (c, fid) in cfs.iter().zip(ids) {
            // document links: UUIDs → IDs
            let value = match &c.value {
                Value::Array(a) if a.iter().all(|x| x.as_str().is_some_and(|s| Uuid::parse_str(s).is_ok())) && !a.is_empty() => {
                    let us: Vec<Uuid> = a.iter().filter_map(|x| x.as_str().and_then(|s| Uuid::parse_str(s).ok())).collect();
                    json!(ident::ref_ids(db, Kind::Document, "custom_fields", &us).await?)
                }
                other => other.clone(),
            };
            out.push(json!({"field": fid, "value": value}));
        }
        m.insert("custom_fields".into(), Value::Array(out));
    }
    let lock = lock_object(store, Kind::Document, id).await?;
    let path = format!("/api/documents/{id}/");
    locked_write(lock, current_rev(store, Kind::Document, uuid), if_match, || call(store, p, Method::PATCH, &path, Some(Value::Object(m))))
        .await?;
    documents::get_by_id(store, user, id).await
}

pub(crate) async fn add_note(store: &Store, p: &Principal, doc: Uuid, text: &str, format: &str) -> DomainResult<Vec<Note>> {
    let user = auth::user(p)?;
    let (id, _) = documents::visible_one(store, user, doc).await?;
    call(store, p, Method::POST, &format!("/api/documents/{id}/notes/"), Some(json!({"note": text, "format": format}))).await?;
    documents::notes_by_id(store, id).await
}

pub(crate) async fn delete_note(store: &Store, p: &Principal, doc: Uuid, note: i64) -> DomainResult<Vec<Note>> {
    let user = auth::user(p)?;
    let (id, _) = documents::visible_one(store, user, doc).await?;
    call(store, p, Method::DELETE, &format!("/api/documents/{id}/notes/?id={note}"), None).await?;
    documents::notes_by_id(store, id).await
}

pub(crate) async fn set_ui_settings(store: &Store, p: &Principal, settings: &Value) -> DomainResult<Value> {
    if !settings.is_object() {
        return Err(DomainError::field("settings", "Must be an object."));
    }
    call(store, p, Method::POST, "/api/ui_settings/", Some(json!({"settings": settings}))).await?;
    objects::ui_settings(store, p).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};

    /// In-memory stand-in for the advisory lock.
    struct MemLock(#[allow(dead_code)] tokio::sync::OwnedMutexGuard<()>);

    impl ObjectLock for MemLock {
        async fn release(self) {}
    }

    /// One v2 writer: lock, re-read rev, compare, "write" (bump rev after a yield, like the
    /// v1 round trip).
    async fn writer(lock: Arc<tokio::sync::Mutex<()>>, rev: Arc<AtomicI64>, if_match: i64) -> DomainResult<()> {
        let guard = MemLock(lock.lock_owned().await);
        let read = {
            let rev = rev.clone();
            async move { Ok(rev.load(Ordering::SeqCst)) }
        };
        locked_write(guard, read, Some(if_match), || async {
            tokio::task::yield_now().await;
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            rev.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
    }

    /// Regression (TOCTOU between If-Match check and v1 write): two concurrent writers with
    /// the same If-Match – exactly one succeeds, the other gets 412 with the new rev.
    #[test]
    fn concurrent_if_match_writes_are_serialized() {
        let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_time().build() {
            Ok(rt) => rt,
            Err(e) => panic!("runtime: {e}"),
        };
        rt.block_on(async {
            for _ in 0..20 {
                let lock = Arc::new(tokio::sync::Mutex::new(()));
                let rev = Arc::new(AtomicI64::new(1));
                let a = tokio::spawn(writer(lock.clone(), rev.clone(), 1));
                let b = tokio::spawn(writer(lock.clone(), rev.clone(), 1));
                let (ra, rb) = match (a.await, b.await) {
                    (Ok(ra), Ok(rb)) => (ra, rb),
                    _ => panic!("writer task failed"),
                };
                let oks = [&ra, &rb].iter().filter(|r| r.is_ok()).count();
                assert_eq!(oks, 1, "exactly one writer may win: {ra:?} / {rb:?}");
                let err = if ra.is_err() { ra } else { rb };
                assert!(matches!(err, Err(DomainError::RevMismatch { current: 2 })), "{err:?}");
                assert_eq!(rev.load(Ordering::SeqCst), 2);
            }
        });
    }

    #[test]
    fn mismatch_skips_the_write_and_releases() {
        let rt = match tokio::runtime::Builder::new_current_thread().build() {
            Ok(rt) => rt,
            Err(e) => panic!("runtime: {e}"),
        };
        rt.block_on(async {
            let lock = Arc::new(tokio::sync::Mutex::new(()));
            let wrote = std::cell::Cell::new(false);
            let r: DomainResult<()> = locked_write(MemLock(lock.clone().lock_owned().await), async { Ok(7) }, Some(6), || async {
                wrote.set(true);
                Ok(())
            })
            .await;
            assert!(matches!(r, Err(DomainError::RevMismatch { current: 7 })));
            assert!(!wrote.get());
            // lock released
            assert!(lock.try_lock().is_ok());
            // no If-Match: write runs
            let r: DomainResult<()> =
                locked_write(MemLock(lock.clone().lock_owned().await), async { Ok(7) }, None, || async { Ok(()) }).await;
            assert!(r.is_ok());
        });
    }

    /// Same TOCTOU regression as above, but with the real PostgreSQL advisory lock
    /// (`lock_object` on the side pool). Only takes advisory locks (no table is read or written),
    /// still only against a local test database:
    /// ```text
    /// PAPERRUST_STORE_TEST_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/pl_store_lock_test \
    ///   cargo test -p pl-store -j 2 --lib legacy::tests::advisory_lock -- --ignored
    /// ```
    #[test]
    #[ignore = "needs PAPERRUST_STORE_TEST_DATABASE_URL (local test database, advisory locks only)"]
    fn advisory_lock_serializes_concurrent_if_match_writes() {
        let Ok(url) = std::env::var("PAPERRUST_STORE_TEST_DATABASE_URL") else { return };
        let db = url.trim_end_matches('/').rsplit('/').next().unwrap_or("").split('?').next().unwrap_or("").to_string();
        assert!(db.starts_with("pl_") && db.contains("test"), "refusing to run against a non-test database ({db})");
        let rt = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() {
            Ok(rt) => rt,
            Err(e) => panic!("runtime: {e}"),
        };
        rt.block_on(async {
            let pool = PgPool::connect(&url).await.expect("connect");
            let env = pl_core::config::Env::from_pairs([("PAPERRUST_SECRET_KEY", "pl-store-lock-test-secret-key-local-only-0123456789")]);
            let cfg = Arc::new(pl_core::Config::from_env_map(env).expect("config"));
            let store = Arc::new(Store::new(pool, cfg, None));
            // Object id unique per run so parallel runs do not share the key.
            let id = i64::from(std::process::id());
            for _ in 0..10 {
                let rev = Arc::new(AtomicI64::new(1));
                let run = |store: Arc<Store>, rev: Arc<AtomicI64>| async move {
                    let lock = lock_object(&store, Kind::Tag, id).await?;
                    let read = {
                        let rev = rev.clone();
                        async move { Ok(rev.load(Ordering::SeqCst)) }
                    };
                    locked_write(lock, read, Some(1), || async {
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        rev.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                    .await
                };
                let a = tokio::spawn(run(store.clone(), rev.clone()));
                let b = tokio::spawn(run(store.clone(), rev.clone()));
                let (ra, rb) = match (a.await, b.await) {
                    (Ok(ra), Ok(rb)) => (ra, rb),
                    _ => panic!("writer task failed"),
                };
                assert_eq!([&ra, &rb].iter().filter(|r| r.is_ok()).count(), 1, "exactly one writer may win: {ra:?} / {rb:?}");
                let err = if ra.is_err() { ra } else { rb };
                assert!(matches!(err, Err(DomainError::RevMismatch { current: 2 })), "{err:?}");
            }
            // Released after every write: the lock is free again for another session.
            let mut other = store.db.acquire().await.expect("conn");
            let free: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock(hashtextextended($1, 0))")
                .bind(lock_key(Kind::Tag, id))
                .fetch_one(&mut *other)
                .await
                .expect("try lock");
            assert!(free, "lock released after the writes");
            let _ =
                sqlx::query("SELECT pg_advisory_unlock(hashtextextended($1, 0))").bind(lock_key(Kind::Tag, id)).execute(&mut *other).await;
        });
    }

    #[test]
    fn lock_keys_are_per_kind_and_object() {
        assert_ne!(lock_key(Kind::Tag, 1), lock_key(Kind::Correspondent, 1));
        assert_ne!(lock_key(Kind::Tag, 1), lock_key(Kind::Tag, 2));
        assert!(lock_key(Kind::Document, 5).starts_with("paperrust:v2write:"));
    }
}
