//! LLM suggestion cache like upstream `documents/caching.py` (v3.4.2) in the **shared**
//! Django `default` cache (Redis, `KEY_PREFIX:1:<key>`, pickle protocol 5), so that Django and
//! PaperRust read/write the same entries. Without a Redis cache (DEBUG/LocMem/other backend)
//! PaperRust uses the shared process cache `pl_core::cache` (`AppState::cache()`) – as
//! Django does in this case; then a Redis connection is never attempted.
//!
//! * `_llm_generation_key(doc)` = `doc_<id>_suggest_llm_generation`: `get_or_set(uuid4().hex,
//!   timeout=6000)` + `touch(6000)`.
//! * `_llm_variant_key(doc, backend)` = `doc_<id>_suggest_llm_<generation>_<sha256(backend)[:16]>`.
//! * `get`: valid only if `classifier_version == 1002` and `classifier_hash == backend`.
//! * `set(..., timeout=3000)`, `refresh` = `touch(3000)`.
//!
//! Redis errors are logged and treated like a cache miss (upstream: 500) –
//! see contract, Known differences.

use pl_core::AppState;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::pickle;

/// `LLM_CACHE_CLASSIFIER_VERSION`.
pub const LLM_CACHE_CLASSIFIER_VERSION: i64 = 1002;
/// `CACHE_50_MINUTES`.
pub const CACHE_50_MINUTES: u64 = 3000;
/// `LLM_CACHE_GENERATION_TIMEOUT`.
pub const LLM_CACHE_GENERATION_TIMEOUT: u64 = 6000;

async fn get_raw(state: &AppState, key: &str) -> Option<Vec<u8>> {
    match state.cache().get(key).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(target: "pl_ai", error = %e.status(), "cache get failed (redis unavailable?)");
            None
        }
    }
}

async fn set_raw(state: &AppState, key: &str, v: Vec<u8>, ttl: u64, nx: bool) {
    let c = state.cache();
    let r = if nx { c.add(key, &v, Some(ttl.max(1))).await.map(|_| ()) } else { c.set(key, &v, Some(ttl.max(1))).await };
    if let Err(e) = r {
        tracing::warn!(target: "pl_ai", error = %e.status(), "cache set failed");
    }
}

async fn touch(state: &AppState, key: &str, ttl: u64) {
    if let Err(e) = state.cache().touch(key, Some(ttl)).await {
        tracing::warn!(target: "pl_ai", error = %e.status(), "cache touch failed");
    }
}

/// `get_suggestion_cache_key(doc)`.
pub fn suggestion_key(doc: i64) -> String {
    format!("doc_{doc}_suggest")
}

fn generation_key(doc: i64) -> String {
    format!("{}_llm_generation", suggestion_key(doc))
}

fn decode_str(b: &[u8]) -> Option<String> {
    // RedisSerializer.loads: first int(data), otherwise pickle.loads
    if let Ok(s) = std::str::from_utf8(b)
        && s.trim().parse::<i64>().is_ok()
    {
        return Some(s.trim().to_string());
    }
    match pickle::loads(b) {
        Ok(pickle::Loaded::Str(s)) => Some(s),
        _ => None,
    }
}

/// `_llm_variant_key(doc, backend)`.
pub async fn variant_key(state: &AppState, doc: i64, backend: &str) -> String {
    let gk = generation_key(doc);
    // get_or_set: get → (fehlt) add(NX) → get
    let mut generation = get_raw(state, &gk).await.and_then(|b| decode_str(&b));
    if generation.is_none() {
        let fresh = uuid::Uuid::new_v4().simple().to_string();
        set_raw(state, &gk, pickle::dumps_str(&fresh), LLM_CACHE_GENERATION_TIMEOUT, true).await;
        generation = Some(get_raw(state, &gk).await.and_then(|b| decode_str(&b)).unwrap_or(fresh));
    }
    touch(state, &gk, LLM_CACHE_GENERATION_TIMEOUT).await;
    let h = Sha256::digest(backend.as_bytes());
    let hex: String = h.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("{}_llm_{}_{hex}", suggestion_key(doc), generation.unwrap_or_default())
}

/// `get_llm_suggestion_cache(doc, backend)` → the raw `suggestions`.
pub async fn get_llm_suggestions(state: &AppState, doc: i64, backend: &str) -> Option<(String, Value)> {
    let key = variant_key(state, doc, backend).await;
    let raw = get_raw(state, &key).await?;
    match pickle::loads(&raw) {
        Ok(pickle::Loaded::Suggestion { version, hash, suggestions })
            if version == LLM_CACHE_CLASSIFIER_VERSION && hash == backend && truthy(&suggestions) =>
        {
            Some((key, suggestions))
        }
        Ok(_) => None,
        Err(e) => {
            tracing::warn!(target: "pl_ai", error = %e, "cache entry not readable");
            None
        }
    }
}

/// Python truth value of `cached_llm_suggestions` (a dataclass instance is always truthy).
fn truthy(_v: &Value) -> bool {
    true
}

/// `refresh_llm_suggestions_cache(doc, backend)`.
pub async fn refresh(state: &AppState, doc: i64, backend: &str) {
    let key = variant_key(state, doc, backend).await;
    touch(state, &key, CACHE_50_MINUTES).await;
}

/// `set_llm_suggestions_cache(doc, suggestions, backend)`.
pub async fn set_llm_suggestions(state: &AppState, doc: i64, suggestions: &Value, backend: &str) {
    let key = variant_key(state, doc, backend).await;
    let v = pickle::dumps_suggestion_cache(LLM_CACHE_CLASSIFIER_VERSION, backend, suggestions);
    set_raw(state, &key, v, CACHE_50_MINUTES, false).await;
}

/// `invalidate_llm_suggestions_cache(doc)`: new generation token, if one exists.
pub async fn invalidate(state: &AppState, doc: i64) {
    let gk = generation_key(doc);
    if get_raw(state, &gk).await.is_some() {
        let fresh = uuid::Uuid::new_v4().simple().to_string();
        set_raw(state, &gk, pickle::dumps_str(&fresh), LLM_CACHE_GENERATION_TIMEOUT, false).await;
    }
}
