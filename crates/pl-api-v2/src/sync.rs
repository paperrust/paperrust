//! Sync endpoints (docs/SYNC.md §3): `GET /sync/changes`, `GET /sync/ids`, `POST /sync/resolve`.
//!
//! The feed itself comes via the trait [`pl_domain::ChangeFeed`] (implementation: `pl-sync`, P2).
//! If none is attached, `/sync/changes` answers `501 not_implemented`; `ids`/`resolve`
//! also work without a feed (then `as_of_seq: null`).

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use axum::extract::Query;
use axum::response::Response;
use pl_domain::model::{Object, Selection};
use pl_domain::repo::{FeedOp, format_feed_cursor, parse_feed_cursor};
use pl_domain::{DomainError, Kind};
use serde_json::{Value, json};

use crate::V2;
use crate::error::{V2Error, V2Result};
use crate::http::{Caller, V2Json, json, truthy};

/// Longest wait for the long poll (docs/SYNC.md §8).
pub const MAX_WAIT_SECS: u64 = 30;

fn kinds_param(s: Option<&String>) -> V2Result<Option<Vec<Kind>>> {
    let Some(s) = s.filter(|s| !s.trim().is_empty()) else { return Ok(None) };
    s.split(',')
        .map(|k| Kind::parse(k.trim()).ok_or_else(|| V2Error(DomainError::field("kinds", format!("Unknown kind \"{}\".", k.trim())))))
        .collect::<V2Result<Vec<_>>>()
        .map(Some)
}

async fn as_of_seq(v2: &V2) -> V2Result<Value> {
    Ok(match &v2.feed {
        Some(f) => json!(f.meta().await?.last_seq),
        None => Value::Null,
    })
}

pub async fn changes(v2: V2, Caller(p): Caller, Query(q): Query<HashMap<String, String>>) -> V2Result<Response> {
    let Some(feed) = v2.feed.clone() else {
        return Err(V2Error(DomainError::NotImplemented("change feed not available on this server (sync capture P2 not wired)".into())));
    };
    let after = match q.get("cursor").filter(|s| !s.is_empty()) {
        None => None,
        Some(c) => Some(parse_feed_cursor(c).ok_or_else(|| V2Error(DomainError::field("cursor", "Invalid cursor.")))?),
    };
    let limit = match q.get("limit") {
        None => 500,
        Some(l) => match l.parse::<i64>() {
            Ok(n) if (1..=1000).contains(&n) => n,
            _ => return Err(V2Error(DomainError::field("limit", "Must be between 1 and 1000."))),
        },
    };
    let kinds = kinds_param(q.get("kinds"))?;
    let embed = q.get("embed").is_none_or(|e| truthy(Some(e)));
    let wait = match q.get("wait") {
        None => 0,
        Some(w) => w.parse::<u64>().map_err(|_| V2Error(DomainError::field("wait", "Seconds (0–30).")))?.min(MAX_WAIT_SECS),
    };
    if wait > 0
        && let Some(a) = after
    {
        feed.wait(a, Duration::from_secs(wait)).await?;
    }
    let page = feed.changes(after, limit, kinds.as_deref()).await?;
    let cursor = after.unwrap_or(0);

    // Visibility per kind in one fetch
    let mut by_kind: BTreeMap<i16, Vec<i64>> = BTreeMap::new();
    for e in page.entries.iter().filter(|e| e.op == FeedOp::Upsert) {
        by_kind.entry(e.kind.code()).or_default().push(e.local_id);
    }
    let mut visible: HashMap<(i16, i64), Option<Object>> = HashMap::new();
    for (code, ids) in by_kind {
        let Some(kind) = Kind::from_code(code) else { continue };
        for (id, o) in v2.repo.by_legacy_ids(&p, kind, &ids, embed).await? {
            visible.insert((code, id), o);
        }
    }
    let mut out = Vec::with_capacity(page.entries.len());
    for e in &page.entries {
        let del = json!({"seq": e.seq, "kind": e.kind.as_str(), "uuid": e.uuid, "op": "delete"});
        match e.op {
            // Tombstone: to everyone (row gone, permissions can no longer be checked; reveals only a UUID)
            FeedOp::Delete => out.push(del),
            FeedOp::Upsert => match visible.get(&(e.kind.code(), e.local_id)) {
                Some(Some(o)) => {
                    let mut c = json!({"seq": e.seq, "kind": e.kind.as_str(), "uuid": e.uuid, "op": "upsert", "rev": e.rev});
                    if embed {
                        c["object"] = serde_json::to_value(o).unwrap_or(Value::Null);
                    }
                    out.push(c);
                }
                // invisible: revoked after the cursor → delete, otherwise omit
                _ if e.perm_changed_after(cursor) => out.push(del),
                _ => {}
            },
        }
    }
    Ok(json(&json!({
        "changes": out,
        "next_cursor": format_feed_cursor(page.next_seq),
        "has_more": page.has_more,
        "perm_epoch": page.meta.perm_epoch,
    })))
}

pub async fn ids(v2: V2, Caller(p): Caller, Query(q): Query<HashMap<String, String>>) -> V2Result<Response> {
    let kind =
        q.get("kind").and_then(|k| Kind::parse(k)).ok_or_else(|| V2Error(DomainError::field("kind", "Required: document, tag, …")))?;
    let as_of = as_of_seq(&v2).await?;
    let items: Vec<Value> = v2.repo.ids(&p, kind).await?.into_iter().map(|(u, r)| json!([u, r])).collect();
    Ok(json(&json!({ "items": items, "as_of_seq": as_of })))
}

pub async fn resolve(v2: V2, Caller(p): Caller, V2Json(sel): V2Json<Selection>) -> V2Result<Response> {
    let as_of = as_of_seq(&v2).await?;
    let r = v2.repo.resolve(&p, &sel).await?;
    // field order: documents, as_of_seq, skipped_saved_views (appended last, docs/contract/pl-api-v2.md)
    Ok(json(&json!({ "documents": r.documents, "as_of_seq": as_of, "skipped_saved_views": r.skipped_saved_views })))
}
