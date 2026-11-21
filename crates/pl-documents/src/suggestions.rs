// upstream: src/documents/views.py@v3.4.2 (DocumentViewSet.suggestions),
//           src/documents/conditionals.py@v3.4.2 (suggestions_etag, suggestions_last_modified),
//           src/documents/matching.py@v3.4.2 (match_correspondents, match_tags, match_document_types,
//           match_storage_paths with user=request.user), src/documents/classifier.py@v3.4.2
//           (load_classifier), src/documents/plugins/date_parsing/__init__.py@v3.4.2 (get_date_parser),
//           src/paperless/settings/__init__.py@v3.4.2 (NUMBER_OF_SUGGESTED_DATES),
//           src/documents/caching.py@v3.4.2 (get/set/refresh_suggestions_cache, CACHE_50_MINUTES)
//
//! `GET /api/documents/{id}/suggestions/`.
//!
//! Classifier: PaperRust model (pl-classifier, `PAPERRUST_CLASSIFIER_MODEL_FILE` or
//! `DATA_DIR/paperrust_classifier.model`, threshold `PAPERLESS_CLASSIFIER_MATCH_THRESHOLD`) – like
//! the consumer (`pl_consumer::native::FileClassifier`). Matching: pl-matching with the candidates
//! `pl_consumer::db::matchables(…, request.user)`. Date suggestions: `RegexDateParser` with the
//! configuration of `get_date_parser()`.
//!
//! ETag/Last-Modified (K2): upstream takes `classifier_hash`/`classifier_modified` from the
//! Django cache (set by the classifier). PaperRust takes the same values directly from the
//! loaded model (`last_auto_type_hash`, `last_doc_change_time`): ETag
//! `"<hash>:<NUMBER_OF_SUGGESTED_DATES>"`, Last-Modified = last document change at training.
//! Without a model file or with a different format version: neither of the two (as upstream).
//!
//! Suggestion cache (K2): like `get/set/refresh_suggestions_cache` 50 minutes, valid only with
//! the same classifier version and hash; additionally bound to `modified` of the document, the user
//! and the date count (upstream deletes the entry on document changes via
//! `clear_document_caches`; in the memory of this process the `modified` comparison achieves that here).
//! The model stays in memory and is reloaded when the file changes (modification time/size);
//! compiled matching patterns are kept by `pl_matching::cache` (PERF-SCALE Q8).

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, FixedOffset, Utc};
use pl_authz::models::{CORRESPONDENT, DOCUMENT, DOCUMENT_TYPE, STORAGE_PATH, TAG};
use pl_authz::perms::{ObjRef, has_perms_owner_aware};
use pl_authz::{AuthUser, User};
use pl_core::{ApiError, AppState, Drf, RequestContext};
use pl_matching::{Classifier, MatchDocument, Matchable};
use serde_json::{Value, json};

use crate::metadata::{conditional, lookup_pk};

type ApiResult<T> = Result<T, ApiError>;

fn model_file(state: &AppState) -> PathBuf {
    let c = state.config();
    pl_core::config::classifier_model_file(|k| c.env.get(k).map(str::to_string), &c.data_dir)
}

fn match_threshold(state: &AppState) -> f64 {
    state.config().env.get("PAPERLESS_CLASSIFIER_MATCH_THRESHOLD").and_then(|v| v.trim().parse().ok()).unwrap_or(0.3)
}

/// Loaded model in memory: re-read only if path, modification time or size of the
/// file change (PERF-SCALE Q8; previously loaded from disk and decrypted on every call).
struct CachedModel {
    path: PathBuf,
    stamp: (std::time::SystemTime, u64),
    model: Arc<pl_classifier::DocumentClassifier>,
}

static MODEL_CACHE: Mutex<Option<CachedModel>> = Mutex::new(None);

/// Modification time + size; `None` = no file (then no classifier, as upstream).
fn file_stamp(path: &std::path::Path) -> Option<(std::time::SystemTime, u64)> {
    let m = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    Some((m.modified().ok()?, m.len()))
}

/// Model with threshold `PAPERLESS_CLASSIFIER_MATCH_THRESHOLD` (like
/// `pl_consumer::native::ThresholdClassifier`, but on the shared model).
struct SharedClassifier {
    model: Arc<pl_classifier::DocumentClassifier>,
    threshold: f64,
}

impl Classifier for SharedClassifier {
    fn predict_correspondent(&self, content: &str) -> Option<i64> {
        self.model.predict_correspondent(content, self.threshold)
    }
    fn predict_document_type(&self, content: &str) -> Option<i64> {
        self.model.predict_document_type(content, self.threshold)
    }
    fn predict_tags(&self, content: &str) -> Vec<i64> {
        self.model.predict_tags(content)
    }
    fn predict_storage_path(&self, content: &str) -> Option<i64> {
        self.model.predict_storage_path(content, self.threshold)
    }
}

/// `load_classifier()`: missing file → `None`, error → warning + `None`. Via
/// [`MODEL_CACHE`] (same file = same model; result like a fresh load).
fn load_classifier(state: &AppState) -> Option<SharedClassifier> {
    let path = model_file(state);
    let threshold = match_threshold(state);
    let Some(stamp) = file_stamp(&path) else {
        if let Ok(mut g) = MODEL_CACHE.lock() {
            *g = None;
        }
        return None;
    };
    if let Ok(g) = MODEL_CACHE.lock()
        && let Some(c) = g.as_ref().filter(|c| c.path == path && c.stamp == stamp)
    {
        return Some(SharedClassifier { model: Arc::clone(&c.model), threshold });
    }
    let key = state.config().secret_key.as_bytes().to_vec();
    match pl_classifier::load_classifier(&path, &key, false) {
        Ok(Some(model)) => {
            let model = Arc::new(model);
            if let Ok(mut g) = MODEL_CACHE.lock() {
                *g = Some(CachedModel { path, stamp, model: Arc::clone(&model) });
            }
            Some(SharedClassifier { model, threshold })
        }
        Ok(None) => {
            if let Ok(mut g) = MODEL_CACHE.lock() {
                *g = None;
            }
            None
        }
        Err(e) => {
            tracing::warn!(target: "paperless.classifier", "Unknown error while loading document classification model: {e}");
            None
        }
    }
}

/// `settings.NUMBER_OF_SUGGESTED_DATES` (`PAPERLESS_NUMBER_OF_SUGGESTED_DATES`, Standard 3).
fn number_of_suggested_dates(state: &AppState) -> ApiResult<i64> {
    state
        .config()
        .env
        .int("PAPERLESS_NUMBER_OF_SUGGESTED_DATES")
        .map(|v| v.unwrap_or(3))
        .map_err(|e| ApiError::internal(format!("PAPERRUST_NUMBER_OF_SUGGESTED_DATES: {e}")))
}

/// `OcrConfig().language`: `ApplicationConfiguration.language or settings.OCR_LANGUAGE`.
async fn ocr_language(state: &AppState) -> ApiResult<String> {
    let app: Option<Option<String>> = sqlx::query_scalar("SELECT language FROM paperless_applicationconfiguration ORDER BY id LIMIT 1")
        .fetch_optional(state.db())
        .await?;
    Ok(match app.flatten().filter(|l| !l.is_empty()) {
        Some(l) => l,
        None => state.config().env.get("PAPERLESS_OCR_LANGUAGE").unwrap_or("eng").to_string(),
    })
}

/// `get_date_parser()` + `sorted({*islice(parse(filename, content), n)})` as `%Y-%m-%d`.
async fn suggested_dates(state: &AppState, filename: &str, content: &str, n: i64) -> ApiResult<Vec<String>> {
    if n <= 0 {
        return Ok(Vec::new());
    }
    let settings = pl_consumer::settings::ConsumerSettings::from_env().map_err(|e| ApiError::internal(e.0))?;
    let lang = ocr_language(state).await?;
    let now = Utc::now();
    let cfg = pl_matching::DateParserConfig {
        languages: pl_matching::dates::resolve_languages(settings.date_parser_languages.as_deref(), &lang),
        timezone_str: settings.time_zone.name().to_string(),
        ignore_dates: match &settings.ignore_dates_raw {
            Some(raw) => pl_matching::dates::parse_ignore_dates(raw, &settings.date_order, now),
            None => Default::default(),
        },
        reference_time: now.fixed_offset(),
        filename_date_order: settings.filename_date_order.clone(),
        content_date_order: settings.date_order.clone(),
    };
    let (filename, content) = (filename.to_string(), content.to_string());
    tokio::task::spawn_blocking(move || {
        let p = pl_matching::RegexDateParser::new(cfg);
        let set: BTreeSet<DateTime<FixedOffset>> = p.parse(&filename, &content).take(n as usize).collect();
        set.into_iter().map(|d| d.format("%Y-%m-%d").to_string()).collect()
    })
    .await
    .map_err(|e| ApiError::internal(format!("date parser task: {e}")))
}

fn ids(v: Vec<&Matchable>) -> Value {
    Value::Array(v.into_iter().map(|m| Value::from(m.id)).collect())
}

/// `CACHE_50_MINUTES`.
const SUGGESTION_TTL: std::time::Duration = std::time::Duration::from_secs(50 * 60);
/// Maximum number of stored suggestions (oldest are evicted first).
const SUGGESTION_CACHE_MAX: usize = 4096;

/// `SuggestionCacheData` + PaperRust binding to document state and user.
#[derive(Clone)]
struct CachedSuggestions {
    classifier_version: u32,
    classifier_hash: String,
    user_id: i64,
    doc_modified: DateTime<Utc>,
    number_of_dates: i64,
    suggestions: Value,
    expires: std::time::Instant,
}

static SUGGESTION_CACHE: Mutex<Option<std::collections::HashMap<i64, CachedSuggestions>>> = Mutex::new(None);

/// Version and hash of the loaded model (`CLASSIFIER_VERSION_KEY`, `CLASSIFIER_HASH_KEY`).
fn classifier_identity(c: &SharedClassifier) -> Option<(u32, String)> {
    Some((c.model.format_version, c.model.last_auto_type_hash.clone()?))
}

/// `get_suggestion_cache` + `refresh_suggestions_cache` (a hit extends by 50 minutes).
fn get_suggestion_cache(doc_id: i64, ident: &(u32, String), user_id: i64, modified: DateTime<Utc>, n: i64) -> Option<Value> {
    let mut g = SUGGESTION_CACHE.lock().ok()?;
    let map = g.as_mut()?;
    let now = std::time::Instant::now();
    let hit = map.get_mut(&doc_id).filter(|c| {
        c.expires > now
            && c.classifier_version == ident.0
            && c.classifier_hash == ident.1
            && c.user_id == user_id
            && c.doc_modified == modified
            && c.number_of_dates == n
    })?;
    hit.expires = now + SUGGESTION_TTL;
    Some(hit.suggestions.clone())
}

/// `set_suggestions_cache(document_id, suggestions, classifier)` (without classifier: nothing).
fn set_suggestions_cache(doc_id: i64, ident: Option<&(u32, String)>, user_id: i64, modified: DateTime<Utc>, n: i64, v: &Value) {
    let Some(ident) = ident else { return };
    let Ok(mut g) = SUGGESTION_CACHE.lock() else { return };
    let map = g.get_or_insert_with(std::collections::HashMap::new);
    let now = std::time::Instant::now();
    if map.len() >= SUGGESTION_CACHE_MAX {
        map.retain(|_, c| c.expires > now);
        if map.len() >= SUGGESTION_CACHE_MAX {
            map.clear();
        }
    }
    map.insert(
        doc_id,
        CachedSuggestions {
            classifier_version: ident.0,
            classifier_hash: ident.1.clone(),
            user_id,
            doc_modified: modified,
            number_of_dates: n,
            suggestions: v.clone(),
            expires: now + SUGGESTION_TTL,
        },
    );
}

/// `suggestions_etag` / `suggestions_last_modified`: from the loaded model (only with a
/// model file and matching format version).
async fn etag_and_last_modified(state: &AppState) -> ApiResult<(Option<String>, Option<i64>)> {
    let st = state.clone();
    let classifier =
        tokio::task::spawn_blocking(move || load_classifier(&st)).await.map_err(|e| ApiError::internal(format!("classifier task: {e}")))?;
    let Some(c) = classifier else { return Ok((None, None)) };
    if c.model.format_version != pl_classifier::FORMAT_VERSION {
        return Ok((None, None));
    }
    let n = number_of_suggested_dates(state)?;
    let etag = c.model.last_auto_type_hash.as_ref().map(|h| format!("{h}:{n}"));
    let last_modified = c.model.last_doc_change_us.map(|us| us.div_euclid(1_000_000));
    Ok((etag, last_modified))
}

/// GET /api/documents/{id}/suggestions/
pub async fn suggestions(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Path(pk): Path<String>,
    method: Method,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let user = auth.require_model_perms(&DOCUMENT)?.clone();
    let indent = ctx.negotiation.indent;
    let (etag, last_modified) = etag_and_last_modified(&state).await?;
    Ok(conditional(&method, &headers, etag, last_modified, || async move {
        let pk = lookup_pk(&pk)?;
        view(&state, &user, pk).await.map(|r| match r {
            Some(body) => Drf::ok(body).indent(indent).into_response(),
            None => ApiError::django_text(StatusCode::FORBIDDEN, "Insufficient permissions").into_response(),
        })
    })
    .await)
}

/// `None` = `HttpResponseForbidden("Insufficient permissions")`.
async fn view(state: &AppState, user: &User, pk: Option<i64>) -> ApiResult<Option<Value>> {
    let db = state.db();
    // get_object_or_404(Document.objects…, pk=pk)
    let Some(pk) = pk else { return Err(ApiError::not_found_model("Document")) };
    let Some(doc) = pl_files::db::load_doc(db, pk, false).await? else {
        return Err(ApiError::not_found_model("Document"));
    };
    if !has_perms_owner_aware(db, Some(user), "change_document", &DOCUMENT, ObjRef { id: doc.id, owner_id: doc.owner_id }).await? {
        return Ok(None);
    }
    let mut conn = db.acquire().await?;
    let Some(row) = pl_consumer::db::load_document(&mut conn, pk).await? else {
        return Err(ApiError::not_found_model("Document"));
    };
    // FINDINGS B2: do not hold the connection while the classifier is loaded and data is searched
    // (`suggested_dates` → `ocr_language` fetches its own connection from the pool).
    drop(conn);
    let classifier = {
        let st = state.clone();
        tokio::task::spawn_blocking(move || load_classifier(&st)).await.map_err(|e| ApiError::internal(format!("classifier task: {e}")))?
    };
    let n = number_of_suggested_dates(state)?;
    let ident = classifier.as_ref().and_then(classifier_identity);
    let (doc_id, modified) = (row.id, row.modified);
    if let Some(ident) = &ident
        && let Some(cached) = get_suggestion_cache(doc_id, ident, user.id, modified, n)
    {
        return Ok(Some(cached));
    }
    let dates = suggested_dates(state, row.filename.as_deref().unwrap_or(""), &row.content, n).await?;
    let mut conn = db.acquire().await?;
    let effective = pl_consumer::workflows::effective_content(&mut conn, &row).await?;
    let filename_for_repr = row.title.clone();
    let inst = pl_consumer::docops::DocInstance::from_db(row);
    let display = inst.repr(&mut conn).await.unwrap_or(filename_for_repr);
    let md = MatchDocument { effective_content: Some(effective), display };
    let c_cands = pl_consumer::db::matchables(&mut conn, &CORRESPONDENT, "Correspondent", Some(user)).await?;
    let t_cands = pl_consumer::db::matchables(&mut conn, &TAG, "Tag", Some(user)).await?;
    let d_cands = pl_consumer::db::matchables(&mut conn, &DOCUMENT_TYPE, "DocumentType", Some(user)).await?;
    let s_cands = pl_consumer::db::matchables(&mut conn, &STORAGE_PATH, "StoragePath", Some(user)).await?;
    drop(conn);
    // without await (the classifier is not Send/Sync across await points)
    let un = |e: pl_matching::matching::UnsupportedMatchingAlgorithm| ApiError::internal(e.to_string());
    let (correspondents, tags, document_types, storage_paths) = {
        let cls = classifier.as_ref().map(|c| c as &dyn Classifier);
        (
            ids(pl_matching::matching::match_correspondents(&md, &c_cands, cls).map_err(un)?),
            ids(pl_matching::matching::match_tags(&md, &t_cands, cls).map_err(un)?),
            ids(pl_matching::matching::match_document_types(&md, &d_cands, cls).map_err(un)?),
            ids(pl_matching::matching::match_storage_paths(&md, &s_cands, cls).map_err(un)?),
        )
    };

    let resp = json!({
        "correspondents": correspondents,
        "tags": tags,
        "document_types": document_types,
        "storage_paths": storage_paths,
        "dates": dates,
    });
    // Cache the suggestions and the classifier hash for later
    set_suggestions_cache(doc_id, ident.as_ref(), user.id, modified, n, &resp);
    Ok(Some(resp))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestion_cache_rules() {
        let ident = (1u32, "abc".to_string());
        let t = Utc::now();
        let v = json!({"tags": [1]});
        set_suggestions_cache(-7, Some(&ident), 2, t, 3, &v);
        assert_eq!(get_suggestion_cache(-7, &ident, 2, t, 3), Some(v.clone()));
        // different hash / user / document state / date count → no hit
        assert_eq!(get_suggestion_cache(-7, &(1, "x".into()), 2, t, 3), None);
        assert_eq!(get_suggestion_cache(-7, &ident, 3, t, 3), None);
        assert_eq!(get_suggestion_cache(-7, &ident, 2, t + chrono::Duration::seconds(1), 3), None);
        assert_eq!(get_suggestion_cache(-7, &ident, 2, t, 4), None);
        // without a classifier nothing is stored
        set_suggestions_cache(-8, None, 2, t, 3, &v);
        assert_eq!(get_suggestion_cache(-8, &ident, 2, t, 3), None);
    }
}
