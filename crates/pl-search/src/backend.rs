// upstream: src/documents/search/_backend.py@v3.4.2
//   TantivyBackend (open, _parse_query, _apply_permission_filter, highlight_hits,
//   search_ids, autocomplete, more_like_this_ids), build_permission_filter,
//   SearchHit, SearchMode, get_backend
// and the tantivy-py 0.26.0 bindings it calls (src/searcher.rs: search,
// terms_with_prefix, fast_field_values; src/query.rs; src/index.rs).
//
// READ ONLY. The index is written by the Python worker (WriteBatch/rebuild).
// No IndexWriter is ever created here.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tantivy::collector::{Collector, Count, SegmentCollector, TopDocs};
use tantivy::query::{
    AllQuery, BooleanQuery, ConstScoreQuery, DisjunctionMaxQuery, ExistsQuery, MoreLikeThisQuery, Occur, Query, TermQuery,
};
use tantivy::schema::{Field, FieldType, IndexRecordOption, Value};
use tantivy::snippet::SnippetGenerator;
use tantivy::{
    DocAddress, DocId, Index, IndexReader, Order, ReloadPolicy, Score, Searcher, SegmentOrdinal, SegmentReader, TantivyDocument, Term,
};

use crate::error::SearchError;
use crate::query_simple::{
    normalize_search_text, parse_simple_text_highlight_query, parse_simple_text_query, parse_simple_title_query, tantivy_parse_query,
};
use crate::schema::{IndexState, build_schema, check_index_dir, schema_matches};
use crate::tokenizer::{ascii_fold, py_lower, register_tokenizers};

/// Search configuration (subset of paperless settings).
#[derive(Debug, Clone)]
pub struct SearchSettings {
    /// `settings.INDEX_DIR` = `DATA_DIR / "index"`.
    pub index_dir: PathBuf,
    /// `settings.SEARCH_LANGUAGE`.
    pub language: Option<String>,
    /// `settings.ADVANCED_FUZZY_SEARCH_THRESHOLD`.
    pub fuzzy_threshold: Option<f64>,
    /// `settings.TIME_ZONE` (django `get_current_timezone()`).
    pub time_zone: chrono_tz::Tz,
}

const OCR_TO_SEARCH: &[(&str, &str)] = &[
    ("ara", "ar"),
    ("dan", "da"),
    ("nld", "nl"),
    ("eng", "en"),
    ("fin", "fi"),
    ("fra", "fr"),
    ("deu", "de"),
    ("ell", "el"),
    ("hun", "hu"),
    ("ita", "it"),
    ("nor", "no"),
    ("por", "pt"),
    ("ron", "ro"),
    ("rus", "ru"),
    ("spa", "es"),
    ("swe", "sv"),
    ("tam", "ta"),
    ("tur", "tr"),
];

impl SearchSettings {
    /// Read the settings from `PAPERLESS_*` environment variables like
    /// `paperless/settings/__init__.py` does. Only for the engine without the
    /// `routes` feature (no pl-core); the server uses `routes::settings_from_core`.
    pub fn from_env() -> Self {
        let get = |k: &str| std::env::var(k).ok();
        let data_dir = get("PAPERLESS_DATA_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("../data"));
        let fuzzy_threshold = get("PAPERLESS_ADVANCED_FUZZY_SEARCH_THRESHOLD").and_then(|v| v.trim().parse::<f64>().ok());
        let time_zone = get("PAPERLESS_TIME_ZONE").and_then(|tz| tz.parse::<chrono_tz::Tz>().ok()).unwrap_or(chrono_tz::UTC);
        Self {
            index_dir: data_dir.join("index"),
            language: search_language_setting(
                get("PAPERLESS_SEARCH_LANGUAGE").as_deref(),
                &get("PAPERLESS_OCR_LANGUAGE").unwrap_or_else(|| "eng".to_string()),
            ),
            fuzzy_threshold,
            time_zone,
        }
    }
}

/// `_get_search_language_setting(ocr_lang)`.
pub fn search_language_setting(explicit: Option<&str>, ocr_lang: &str) -> Option<String> {
    if let Some(explicit) = explicit {
        // get_choice_from_env validates against SUPPORTED_LANGUAGES (upstream
        // refuses to start on an invalid value).
        if crate::tokenizer::language_for(explicit).is_some() {
            return Some(explicit.to_string());
        }
        tracing::error!("PAPERRUST_SEARCH_LANGUAGE {explicit:?} is not supported");
        return None;
    }
    let primary = ocr_lang.split('+').next().unwrap_or_default().to_lowercase();
    OCR_TO_SEARCH.iter().find(|(k, _)| *k == primary).map(|(_, v)| v.to_string())
}

/// `SearchMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    Query,
    Text,
    Title,
}

/// The requesting user as far as the index permission filter is concerned
/// (`build_permission_filter`). `None` at call sites means "superuser / no
/// filtering", exactly like upstream passes `user=None`.
#[derive(Debug, Clone)]
pub struct SearchUser {
    pub id: i64,
    /// `user.groups.values_list("pk")` (current memberships).
    pub group_ids: Vec<i64>,
}

/// `SearchHit`.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub id: i64,
    /// Python float of the tantivy f32 score.
    pub score: f64,
    pub rank: usize,
    /// `highlights["content"]`.
    pub content_highlight: Option<String>,
    /// `highlights["notes"]`.
    pub notes_highlight: Option<String>,
}

impl SearchHit {
    pub fn stub(id: i64, rank: usize) -> Self {
        Self { id, score: 0.0, rank, content_highlight: None, notes_highlight: None }
    }

    /// `SearchResultSerializer.to_representation`: the `__search_hit__` object.
    pub fn to_json(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        m.insert("score".into(), serde_json::json!(self.score));
        m.insert("highlights".into(), serde_json::Value::String(self.content_highlight.clone().unwrap_or_default()));
        m.insert(
            "note_highlights".into(),
            match self.notes_highlight.as_deref() {
                Some(s) if !s.is_empty() => serde_json::Value::String(s.to_string()),
                _ => serde_json::Value::Null,
            },
        );
        m.insert("rank".into(), serde_json::json!(self.rank));
        serde_json::Value::Object(m)
    }
}

/// `TantivyBackend.SORT_FIELD_MAP`.
pub fn sort_field_map(name: &str) -> Option<&'static str> {
    Some(match name {
        "title" => "title_sort",
        "correspondent__name" => "correspondent_sort",
        "document_type__name" => "type_sort",
        "created" => "created",
        "added" => "added",
        "modified" => "modified",
        "archive_serial_number" => "asn",
        "page_count" => "page_count",
        "num_notes" => "num_notes",
        _ => return None,
    })
}

/// `TantivyBackend.SORTABLE_FIELDS`.
pub fn is_sortable_field(name: &str) -> bool {
    matches!(name, "created" | "added" | "modified" | "archive_serial_number" | "page_count" | "num_notes")
}

struct Opened {
    index: Index,
    reader: IndexReader,
    /// false for the in-RAM empty stand-in (index missing/mismatched).
    on_disk: bool,
}

/// Read-only tantivy backend with a reloading reader.
///
/// The reader uses `ReloadPolicy::OnCommitWithDelay` (tantivy-py default), so
/// commits by the Python worker become visible automatically. If the index
/// directory is missing or its `.index_settings.json` does not match (upstream
/// would wipe/rebuild), an empty in-RAM index with the same schema is used —
/// exactly what a fresh upstream process would search — and opening the real
/// index is retried at most every `RETRY_INTERVAL`.
pub struct SearchBackend {
    settings: SearchSettings,
    opened: RwLock<Option<Arc<Opened>>>,
    last_attempt: Mutex<Option<Instant>>,
}

const RETRY_INTERVAL: Duration = Duration::from_secs(5);

impl SearchBackend {
    pub fn new(settings: SearchSettings) -> Self {
        Self { settings, opened: RwLock::new(None), last_attempt: Mutex::new(None) }
    }

    pub fn settings(&self) -> &SearchSettings {
        &self.settings
    }

    fn language(&self) -> Option<&str> {
        self.settings.language.as_deref()
    }

    fn open_on_disk(&self) -> Result<Option<Opened>, SearchError> {
        let dir = &self.settings.index_dir;
        match check_index_dir(dir, self.language()) {
            // Upstream opens a partially rebuilt index too (marker ignored).
            IndexState::Ok | IndexState::Rebuilding => {}
            IndexState::Missing => return Ok(None),
            IndexState::SettingsMismatch(why) => {
                tracing::warn!("search index settings mismatch ({why}); waiting for reindex");
                return Ok(None);
            }
        }
        let index = match Index::open_in_dir(dir) {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!("search index could not be opened: {e}");
                return Ok(None);
            }
        };
        if !schema_matches(&index.schema()) {
            tracing::warn!("search index schema does not match; waiting for reindex");
            return Ok(None);
        }
        register_tokenizers(&index, self.language());
        let reader = index.reader_builder().reload_policy(ReloadPolicy::OnCommitWithDelay).try_into()?;
        Ok(Some(Opened { index, reader, on_disk: true }))
    }

    fn open_empty(&self) -> Result<Opened, SearchError> {
        let index = Index::create_in_ram(build_schema());
        register_tokenizers(&index, self.language());
        let reader = index.reader_builder().reload_policy(ReloadPolicy::Manual).try_into()?;
        Ok(Opened { index, reader, on_disk: false })
    }

    fn current(&self) -> Result<Arc<Opened>, SearchError> {
        if let Ok(guard) = self.opened.read()
            && let Some(o) = guard.as_ref()
        {
            if o.on_disk {
                return Ok(o.clone());
            }
            // in-RAM stand-in: retry the real index now and then
            let due = self.last_attempt.lock().map(|t| t.is_none_or(|t| t.elapsed() >= RETRY_INTERVAL)).unwrap_or(true);
            if !due {
                return Ok(o.clone());
            }
        }
        let mut guard = self.opened.write().map_err(|_| SearchError::Internal("search backend lock poisoned".into()))?;
        if let Some(o) = guard.as_ref()
            && o.on_disk
        {
            return Ok(o.clone());
        }
        if let Ok(mut t) = self.last_attempt.lock() {
            *t = Some(Instant::now());
        }
        let opened = match self.open_on_disk()? {
            Some(o) => o,
            None => match guard.as_ref() {
                Some(existing) => return Ok(existing.clone()),
                None => self.open_empty()?,
            },
        };
        let opened = Arc::new(opened);
        *guard = Some(opened.clone());
        Ok(opened)
    }

    /// Force a reader reload (normally automatic). Useful in tests.
    pub fn reload(&self) -> Result<(), SearchError> {
        let o = self.current()?;
        o.reader.reload()?;
        Ok(())
    }

    /// `_parse_query`.
    fn parse_query(&self, index: &Index, query: &str, mode: SearchMode, now: DateTime<Utc>) -> Result<Box<dyn Query>, SearchError> {
        let query = normalize_search_text(query);
        match mode {
            SearchMode::Text => parse_simple_text_query(index, &query),
            SearchMode::Title => parse_simple_title_query(index, &query),
            SearchMode::Query => {
                let cfg = crate::query_user::UserQueryConfig {
                    language: self.language(),
                    fuzzy: self.settings.fuzzy_threshold.is_some(),
                    tz: self.settings.time_zone,
                    now,
                };
                crate::query_user::parse_user_query(index, &query, &cfg)
            }
        }
    }

    /// `_apply_permission_filter`.
    fn apply_permission_filter(index: &Index, query: Box<dyn Query>, user: Option<&SearchUser>) -> Result<Box<dyn Query>, SearchError> {
        match user {
            None => Ok(query),
            Some(user) => Ok(Box::new(BooleanQuery::new(vec![(Occur::Must, query), (Occur::Must, build_permission_filter(index, user)?)]))),
        }
    }

    /// User query (+ filter query as `const_score(…, 0.0)`) + permission filter, like `search_ids`.
    fn final_query(
        &self,
        index: &Index,
        query: &str,
        user: Option<&SearchUser>,
        mode: SearchMode,
        filter_query: Option<&str>,
    ) -> Result<Box<dyn Query>, SearchError> {
        let now = Utc::now();
        let mut user_query = self.parse_query(index, query, mode, now)?;
        if let Some(fq) = filter_query {
            let filter = self.parse_query(index, fq, SearchMode::Query, now)?;
            // Query.and_must_match(const_score_query(filter, 0.0))
            user_query =
                Box::new(BooleanQuery::new(vec![(Occur::Must, user_query), (Occur::Must, Box::new(ConstScoreQuery::new(filter, 0.0)))]));
        }
        Self::apply_permission_filter(index, user_query, user)
    }

    /// `search_ids`.
    #[allow(clippy::too_many_arguments)]
    pub fn search_ids(
        &self,
        query: &str,
        user: Option<&SearchUser>,
        sort_field: Option<&str>,
        sort_reverse: bool,
        mode: SearchMode,
        filter_query: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<i64>, SearchError> {
        let o = self.current()?;
        let final_query = self.final_query(&o.index, query, user, mode, filter_query)?;

        let searcher = o.reader.searcher();
        let effective_limit = match limit {
            Some(l) => l,
            None => searcher.num_docs() as usize,
        };
        if effective_limit == 0 {
            return Ok(Vec::new());
        }

        let mapped = sort_field.and_then(sort_field_map);
        let addrs: Vec<DocAddress> = if let Some(mapped) = mapped {
            let order = if sort_reverse { Order::Desc } else { Order::Asc };
            sorted_search(&searcher, &*final_query, mapped, order, effective_limit)?
        } else {
            let hits = searcher.search(&*final_query, &TopDocs::with_limit(effective_limit).order_by_score())?;
            let mut all: Vec<(DocAddress, f64)> = hits.into_iter().map(|(s, a)| (a, f64::from(s))).collect();
            if !all.is_empty() {
                let mut max_score = all.iter().map(|h| h.1).fold(f64::NEG_INFINITY, f64::max);
                if max_score == 0.0 {
                    max_score = 1.0;
                }
                for h in &mut all {
                    h.1 /= max_score;
                }
            }
            if let Some(threshold) = self.settings.fuzzy_threshold {
                all.retain(|h| h.1 >= threshold);
            }
            all.into_iter().map(|h| h.0).collect()
        };
        fast_field_ids(&searcher, &addrs)
    }

    /// L3 (`PAPERRUST_SEARCH_ENGINE=native`): hit count and the first `window` hits in
    /// the same order as [`Self::search_ids`] with `limit = None` – without
    /// ranking all hits. `window = None` returns the complete list.
    ///
    /// Equality with `search_ids`: `TopNComputer` orders totally (key, then `DocAddress`
    /// ascending), so the first `k` of a complete ranking are exactly the top `k`.
    /// The hit count comes from the `Count` collector over the same query (same set, without
    /// scoring). For the complete list `limit = hit count` suffices: the buffer per segment
    /// (`2·limit`) then never fills up, no threshold ever arises – exactly like
    /// `limit = num_docs` at upstream, but without `num_docs`-sized buffers.
    /// Exceptions (always a complete ranking with `limit = hit count`): relevance sorting
    /// without a permission filter (superuser), because the top-level query can then be a pure term union
    /// that tantivy evaluates via block-WAND (summation order depends on the
    /// threshold), and a set fuzziness threshold (normalisation over the maximum).
    #[allow(clippy::too_many_arguments)]
    pub fn search_window(
        &self,
        query: &str,
        user: Option<&SearchUser>,
        sort_field: Option<&str>,
        sort_reverse: bool,
        mode: SearchMode,
        filter_query: Option<&str>,
        window: Option<usize>,
    ) -> Result<RankedWindow, SearchError> {
        if self.settings.fuzzy_threshold.is_some() {
            let ids = self.search_ids(query, user, sort_field, sort_reverse, mode, filter_query, None)?;
            return Ok(RankedWindow { total: ids.len(), ids });
        }
        let o = self.current()?;
        let final_query = self.final_query(&o.index, query, user, mode, filter_query)?;
        let searcher = o.reader.searcher();
        let total = searcher.search(&*final_query, &Count)?;
        if total == 0 {
            return Ok(RankedWindow { total: 0, ids: Vec::new() });
        }
        let mapped = sort_field.and_then(sort_field_map);
        let mut limit = window.map_or(total, |w| w.min(total));
        if mapped.is_none() && user.is_none() && !window_exact_for_superuser() {
            limit = total;
        }
        if limit == 0 {
            return Ok(RankedWindow { total, ids: Vec::new() });
        }
        let addrs: Vec<DocAddress> = if let Some(mapped) = mapped {
            let order = if sort_reverse { Order::Desc } else { Order::Asc };
            sorted_search(&searcher, &*final_query, mapped, order, limit)?
        } else {
            searcher.search(&*final_query, &TopDocs::with_limit(limit).order_by_score())?.into_iter().map(|(_, a)| a).collect()
        };
        Ok(RankedWindow { total, ids: fast_field_ids(&searcher, &addrs)? })
    }

    /// L3: like [`Self::more_like_this_ids`] (`limit = None`), but only the hit count + the first
    /// `window` hits (without the source document). Superuser: full ranking (see
    /// [`Self::search_window`]).
    pub fn more_like_this_window(
        &self,
        doc_id: i64,
        user: Option<&SearchUser>,
        window: Option<usize>,
    ) -> Result<RankedWindow, SearchError> {
        let o = self.current()?;
        let searcher = o.reader.searcher();
        let schema = o.index.schema();
        let id_field = field(&schema, "id")?;
        let id_term = Term::from_field_u64(id_field, doc_id as u64);
        let id_query = TermQuery::new(id_term.clone(), IndexRecordOption::WithFreqsAndPositions);
        let results = searcher.search(&id_query, &TopDocs::with_limit(1).order_by_score())?;
        let Some((_, doc_address)) = results.first().copied() else {
            return Ok(RankedWindow { total: 0, ids: Vec::new() });
        };
        let mlt: Box<dyn Query> = Box::new(
            MoreLikeThisQuery::builder()
                .with_min_doc_frequency(1)
                .with_min_term_frequency(1)
                .with_max_query_terms(12)
                .with_max_doc_frequency(mlt_max_doc_frequency(&searcher))
                .with_stop_words(Vec::new())
                .with_document(doc_address),
        );
        let final_query = Self::apply_permission_filter(&o.index, mlt, user)?;
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> tantivy::Result<(usize, Vec<DocAddress>)> {
            let raw_total = count_scored(&searcher, &*final_query)?;
            if raw_total == 0 {
                return Ok((0, Vec::new()));
            }
            let self_q = BooleanQuery::new(vec![
                (Occur::Must, final_query.box_clone()),
                (Occur::Must, Box::new(TermQuery::new(id_term.clone(), IndexRecordOption::Basic))),
            ]);
            let has_self = count_scored(&searcher, &self_q)? > 0;
            let total = raw_total - usize::from(has_self);
            let mut limit = window.map_or(raw_total, |w| w.saturating_add(1).min(raw_total));
            if user.is_none() && !window_exact_for_superuser() {
                limit = raw_total;
            }
            let hits = searcher.search(&*final_query, &TopDocs::with_limit(limit).order_by_score())?;
            Ok((total, hits.into_iter().map(|(_, a)| a).collect()))
        }));
        let (total, addrs) = match run {
            Ok(r) => r?,
            Err(_) => {
                tracing::warn!(
                    "More Like This scoring panicked (likely stale tantivy segment stats after deletions); returning no results. A search index reindex will rebuild consistent statistics."
                );
                return Ok(RankedWindow { total: 0, ids: Vec::new() });
            }
        };
        let mut ids: Vec<i64> = fast_field_ids(&searcher, &addrs)?.into_iter().filter(|id| *id != doc_id).collect();
        if let Some(w) = window {
            ids.truncate(w);
        }
        Ok(RankedWindow { total, ids })
    }

    /// `highlight_hits`.
    pub fn highlight_hits(&self, query: &str, doc_ids: &[i64], mode: SearchMode, rank_start: usize) -> Result<Vec<SearchHit>, SearchError> {
        if doc_ids.is_empty() {
            return Ok(Vec::new());
        }
        let o = self.current()?;
        let index = &o.index;
        let schema = index.schema();
        let user_query = self.parse_query(index, query, mode, Utc::now())?;
        let query = normalize_search_text(query);

        let mut highlight_query: Option<Box<dyn Query>> = None;
        if mode == SearchMode::Text {
            match parse_simple_text_highlight_query(index, &query) {
                Ok(q) => highlight_query = Some(q),
                Err(_) => {
                    tracing::debug!("Skipping simple text highlight query: token string is not valid tantivy query syntax: {query:?}")
                }
            }
        }

        // re.sub(r"\w[\w.]*:", "", query).strip()
        let bare_query =
            bare_query_re().map(|re| re.replace_all(&query, "").trim().to_string()).unwrap_or_else(|| query.trim().to_string());
        let notes_text_query: Option<Box<dyn Query>> =
            if bare_query.is_empty() { None } else { tantivy_parse_query(index, &bare_query, &["notes_text"]).ok() };

        let searcher = o.reader.searcher();
        let id_field = field(&schema, "id")?;
        let id_filter = BooleanQuery::new(
            doc_ids
                .iter()
                .map(|did| {
                    let q: Box<dyn Query> =
                        Box::new(TermQuery::new(Term::from_field_u64(id_field, *did as u64), IndexRecordOption::WithFreqsAndPositions));
                    (Occur::Should, q)
                })
                .collect(),
        );
        let batch_query = BooleanQuery::new(vec![(Occur::Must, user_query.box_clone()), (Occur::Must, Box::new(id_filter))]);
        let batch_results = searcher.search(&batch_query, &TopDocs::with_limit(doc_ids.len()).order_by_score())?;
        let addrs: Vec<DocAddress> = batch_results.iter().map(|(_, a)| *a).collect();
        let ids = fast_field_ids(&searcher, &addrs)?;
        let mut addr_by_id: HashMap<i64, (f64, DocAddress)> = HashMap::new();
        for ((score, addr), id) in batch_results.iter().zip(ids) {
            addr_by_id.insert(id, (f64::from(*score), *addr));
        }

        let content_field = field(&schema, "content")?;
        let notes_text_field = field(&schema, "notes_text")?;
        let hl_query: &dyn Query = highlight_query.as_deref().unwrap_or(&*user_query);
        let notes_query: &dyn Query = notes_text_query.as_deref().unwrap_or(&*user_query);

        let mut snippet_generator: Option<SnippetGenerator> = None;
        let mut notes_snippet_generator: Option<SnippetGenerator> = None;
        let mut hits = Vec::new();
        for (i, doc_id) in doc_ids.iter().enumerate() {
            let rank = rank_start + i;
            let Some((score, addr)) = addr_by_id.get(doc_id) else {
                continue;
            };
            let mut hit = SearchHit::stub(*doc_id, rank);
            hit.score = *score;
            let doc: TantivyDocument = searcher.doc(*addr)?;
            // try: ... except Exception: pass (partial highlights are kept)
            let _ = (|| -> Result<(), tantivy::TantivyError> {
                if snippet_generator.is_none() {
                    snippet_generator = Some(SnippetGenerator::create(&searcher, hl_query, content_field)?);
                }
                if let Some(g) = &snippet_generator {
                    let html = g.snippet_from_doc(&doc).to_html();
                    if !html.is_empty() {
                        hit.content_highlight = Some(html);
                    }
                }
                if mode == SearchMode::Query && doc.get_first(notes_text_field).is_some() {
                    if notes_snippet_generator.is_none() {
                        notes_snippet_generator = Some(SnippetGenerator::create(&searcher, notes_query, notes_text_field)?);
                    }
                    if let Some(g) = &notes_snippet_generator {
                        let html = g.snippet_from_doc(&doc).to_html();
                        if !html.is_empty() {
                            hit.notes_highlight = Some(html);
                        }
                    }
                }
                Ok(())
            })()
            .map_err(|e| tracing::debug!("Failed to generate highlights for doc {doc_id}: {e}"));
            hits.push(hit);
        }
        Ok(hits)
    }

    /// `autocomplete(term, limit, user)`; `user` is `None` for superusers.
    pub fn autocomplete(&self, term: &str, limit: usize, user: Option<&SearchUser>) -> Result<Vec<String>, SearchError> {
        let normalized = ascii_fold(&py_lower(term));
        if normalized.is_empty() {
            return Ok(Vec::new());
        }
        let o = self.current()?;
        let searcher = o.reader.searcher();
        let permission_query = match user {
            Some(u) => Some(build_permission_filter(&o.index, u)?),
            None => None,
        };
        terms_with_prefix(&searcher, "autocomplete_word", &normalized, permission_query.as_deref(), Some(limit))
    }

    /// `more_like_this_ids`. `doc_id` must already be the root document id
    /// (`root_document_ids([doc_id])`, resolved by the caller from the DB).
    pub fn more_like_this_ids(&self, doc_id: i64, user: Option<&SearchUser>, limit: Option<usize>) -> Result<Vec<i64>, SearchError> {
        let o = self.current()?;
        let searcher = o.reader.searcher();
        let schema = o.index.schema();
        let id_field = field(&schema, "id")?;
        let id_query = TermQuery::new(Term::from_field_u64(id_field, doc_id as u64), IndexRecordOption::WithFreqsAndPositions);
        let results = searcher.search(&id_query, &TopDocs::with_limit(1).order_by_score())?;
        let Some((_, doc_address)) = results.first().copied() else {
            return Ok(Vec::new());
        };
        let mlt: Box<dyn Query> = Box::new(
            MoreLikeThisQuery::builder()
                .with_min_doc_frequency(1)
                .with_min_term_frequency(1)
                .with_max_query_terms(12)
                .with_max_doc_frequency(mlt_max_doc_frequency(&searcher))
                .with_stop_words(Vec::new())
                .with_document(doc_address),
        );
        let final_query = Self::apply_permission_filter(&o.index, mlt, user)?;
        let effective_limit = match limit {
            Some(l) => l,
            None => searcher.num_docs() as usize,
        };
        // Tantivy 0.26 can panic in BM25 idf scoring with soft-deleted docs;
        // upstream catches the PanicException and returns [].
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            searcher.search(&*final_query, &TopDocs::with_limit(effective_limit + 1).order_by_score())
        }));
        let hits = match run {
            Ok(r) => r?,
            Err(_) => {
                tracing::warn!(
                    "More Like This scoring panicked (likely stale tantivy segment stats after deletions); returning no results. A search index reindex will rebuild consistent statistics."
                );
                return Ok(Vec::new());
            }
        };
        let addrs: Vec<DocAddress> = hits.iter().map(|(_, a)| *a).collect();
        let mut ids: Vec<i64> = fast_field_ids(&searcher, &addrs)?.into_iter().filter(|id| *id != doc_id).collect();
        if let Some(l) = limit {
            ids.truncate(l);
        }
        Ok(ids)
    }
}

/// Result of [`SearchBackend::search_window`]: hit count + start of the ranking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedWindow {
    /// Number of all hits (`len(search_ids(..., limit=None))`).
    pub total: usize,
    /// The first hits in index order (all of them if `ids.len() == total`).
    pub ids: Vec<i64>,
}

impl RankedWindow {
    pub fn is_complete(&self) -> bool {
        self.ids.len() == self.total
    }
}

/// Top-k by relevance even without a permission filter (superuser)? Default no (block-WAND, see
/// [`SearchBackend::search_window`]); `PL_L3_WINDOW_SUPERUSER=1` only for measurements.
fn window_exact_for_superuser() -> bool {
    std::env::var("PL_L3_WINDOW_SUPERUSER").is_ok_and(|v| v == "1")
}

fn bare_query_re() -> Option<&'static regex::Regex> {
    static RE: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\w[\w.]*:").ok()).as_ref()
}

fn field(schema: &tantivy::schema::Schema, name: &str) -> Result<Field, SearchError> {
    schema.get_field(name).map_err(|e| SearchError::Internal(e.to_string()))
}

/// `build_permission_filter`.
pub fn build_permission_filter(index: &Index, user: &SearchUser) -> Result<Box<dyn Query>, SearchError> {
    let schema = index.schema();
    let term = |name: &str, v: i64| -> Result<Box<dyn Query>, SearchError> {
        Ok(Box::new(TermQuery::new(Term::from_field_u64(field(&schema, name)?, v as u64), IndexRecordOption::WithFreqsAndPositions)))
    };
    let owner_any: Box<dyn Query> = Box::new(ExistsQuery::new("owner_id".to_string(), false));
    let no_owner: Box<dyn Query> = Box::new(BooleanQuery::new(vec![(Occur::Must, Box::new(AllQuery)), (Occur::MustNot, owner_any)]));
    let mut disjuncts = vec![no_owner, term("owner_id", user.id)?, term("viewer_id", user.id)?];
    for g in &user.group_ids {
        disjuncts.push(term("viewer_group_id", *g)?);
    }
    Ok(Box::new(DisjunctionMaxQuery::new(disjuncts)))
}

/// tantivy-py `Searcher.fast_field_values("id", addrs)`.
fn fast_field_ids(searcher: &Searcher, addrs: &[DocAddress]) -> Result<Vec<i64>, SearchError> {
    let mut columns = HashMap::new();
    let mut out = Vec::with_capacity(addrs.len());
    for addr in addrs {
        let col = match columns.entry(addr.segment_ord) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let reader = searcher.segment_reader(addr.segment_ord);
                e.insert(reader.fast_fields().u64("id")?)
            }
        };
        match col.first(addr.doc_id) {
            Some(v) => out.push(v as i64),
            None => {
                return Err(SearchError::Index(format!("document without id at {addr:?}")));
            }
        }
    }
    Ok(out)
}

/// tantivy-py `Searcher.search(..., order_by_field=..., order=...)`.
fn sorted_search(
    searcher: &Searcher,
    query: &dyn Query,
    field_name: &str,
    order: Order,
    limit: usize,
) -> Result<Vec<DocAddress>, SearchError> {
    let schema = searcher.schema();
    let f = field(schema, field_name)?;
    let top = TopDocs::with_limit(limit);
    let addrs = match schema.get_field_entry(f).field_type() {
        FieldType::U64(_) => {
            searcher.search(query, &top.order_by_fast_field::<u64>(field_name, order))?.into_iter().map(|(_, a)| a).collect()
        }
        FieldType::Date(_) => {
            searcher.search(query, &top.order_by_fast_field::<tantivy::DateTime>(field_name, order))?.into_iter().map(|(_, a)| a).collect()
        }
        FieldType::Str(_) => {
            searcher.search(query, &top.order_by_string_fast_field(field_name, order))?.into_iter().map(|(_, a)| a).collect()
        }
        _ => {
            return Err(SearchError::Internal(format!("cannot order by field {field_name}")));
        }
    };
    Ok(addrs)
}

/// Per-segment set of matching docs (tantivy-py `PerSegmentBitSetCollector`).
/// Collected through `Searcher::search`, so deleted docs are excluded.
struct BitSetCollector;

struct SegmentBits {
    ord: SegmentOrdinal,
    words: Vec<u64>,
    any: bool,
}

impl SegmentBits {
    fn contains(&self, doc: DocId) -> bool {
        let i = (doc / 64) as usize;
        self.words.get(i).is_some_and(|w| w & (1u64 << (doc % 64)) != 0)
    }
}

impl Collector for BitSetCollector {
    type Fruit = Vec<SegmentBits>;
    type Child = SegmentBits;

    fn for_segment(&self, ord: SegmentOrdinal, segment: &SegmentReader) -> tantivy::Result<Self::Child> {
        Ok(SegmentBits { ord, words: vec![0; (segment.max_doc() as usize).div_ceil(64)], any: false })
    }

    fn requires_scoring(&self) -> bool {
        false
    }

    fn merge_fruits(&self, fruits: Vec<SegmentBits>) -> tantivy::Result<Self::Fruit> {
        Ok(fruits)
    }
}

impl SegmentCollector for SegmentBits {
    type Fruit = SegmentBits;

    fn collect(&mut self, doc: DocId, _score: Score) {
        let i = (doc / 64) as usize;
        if let Some(w) = self.words.get_mut(i) {
            *w |= 1u64 << (doc % 64);
            self.any = true;
        }
    }

    fn harvest(self) -> Self::Fruit {
        self
    }
}

/// tantivy-py `next_prefix_bound`.
fn next_prefix_bound(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut bound = prefix.to_vec();
    while let Some(last) = bound.last().copied() {
        if last < 0xFF {
            let n = bound.len();
            bound[n - 1] = last + 1;
            return Some(bound);
        }
        bound.pop();
    }
    None
}

/// tantivy-py `Searcher.terms_with_prefix(field, prefix, filter_query, limit)`:
/// per term the number of (filtered) documents containing it, summed over
/// segments, sorted by count desc then term asc, top `limit`.
/// Unfiltered counts are the term dictionary `doc_freq` (includes deleted docs,
/// like upstream).
pub fn terms_with_prefix(
    searcher: &Searcher,
    field_name: &str,
    prefix: &str,
    filter: Option<&dyn Query>,
    limit: Option<usize>,
) -> Result<Vec<String>, SearchError> {
    let schema = searcher.schema();
    let f = field(schema, field_name)?;
    if !matches!(schema.get_field_entry(f).field_type(), FieldType::Str(_)) {
        return Err(SearchError::Internal(format!("{field_name} is not a text field")));
    }
    if limit == Some(0) {
        return Ok(Vec::new());
    }
    let filter_sets: Option<Vec<SegmentBits>> = match filter {
        Some(q) => {
            let sets = searcher.search(q, &BitSetCollector)?;
            if sets.iter().all(|s| !s.any) {
                return Ok(Vec::new());
            }
            Some(sets)
        }
        None => None,
    };
    let prefix_bytes = prefix.as_bytes();
    let upper = next_prefix_bound(prefix_bytes);
    let open_ended = upper.is_none() && !prefix_bytes.is_empty();
    let mut counts: HashMap<String, u64> = HashMap::new();
    for (seg_ord, segment) in searcher.segment_readers().iter().enumerate() {
        let filter_set = match &filter_sets {
            Some(sets) => match sets.iter().find(|s| s.ord as usize == seg_ord) {
                Some(s) if s.any => Some(s),
                _ => continue,
            },
            None => None,
        };
        let inv = segment.inverted_index(f)?;
        let mut builder = inv.terms().range().ge(prefix_bytes);
        if let Some(u) = &upper {
            builder = builder.lt(u);
        }
        let mut stream = builder.into_stream()?;
        while stream.advance() {
            let key = stream.key();
            if open_ended && !key.starts_with(prefix_bytes) {
                break;
            }
            let Ok(term) = std::str::from_utf8(key) else {
                continue;
            };
            let count: u64 = match filter_set {
                None => u64::from(stream.value().doc_freq),
                Some(set) => {
                    let mut postings = inv.read_postings_from_terminfo(stream.value(), IndexRecordOption::Basic)?;
                    let mut c = 0u64;
                    use tantivy::DocSet;
                    let mut doc = postings.doc();
                    while doc != tantivy::TERMINATED {
                        if set.contains(doc) {
                            c += 1;
                        }
                        doc = postings.advance();
                    }
                    c
                }
            };
            if count > 0 {
                *counts.entry(term.to_string()).or_insert(0) += count;
            }
        }
    }
    let mut all: Vec<(String, u64)> = counts.into_iter().collect();
    all.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    if let Some(l) = limit {
        all.truncate(l);
    }
    Ok(all.into_iter().map(|(t, _)| t).collect())
}

/// Silence "unused" for the Value trait import used by `get_first().is_some()`.
#[allow(dead_code)]
fn _value_trait_used<'a, V: Value<'a>>(_v: V) {}

/// Hit count with scoring switched on (L3, `more_like_this_window`). `MoreLikeThisQuery`
/// can only be evaluated with scoring; `Count` alone switches it off ("MoreLikeThisQuery requires
/// to enable scoring", e.g. for a superuser without a surrounding permission filter). The tuple with a
/// top-1 collector by score forces scoring, the counted set stays the same.
fn count_scored(searcher: &tantivy::Searcher, query: &dyn Query) -> tantivy::Result<usize> {
    let (n, _) = searcher.search(query, &(Count, TopDocs::with_limit(1).order_by_score()))?;
    Ok(n)
}

/// Upper bound for the document frequency of More-Like-This terms: number of live documents.
/// In `MoreLikeThis`, tantivy 0.26 computes the idf with `num_docs` (without deleted), but `doc_freq`
/// counts deleted ones too; a term with `doc_freq > num_docs` triggers the assertion in
/// `bm25::idf` (panic → previously an empty list). Such terms appear in practically every
/// document (idf ≈ 0) and contribute nothing to similarity; we leave them out.
fn mlt_max_doc_frequency(searcher: &tantivy::Searcher) -> u64 {
    searcher.segment_readers().iter().map(|r| u64::from(r.num_docs())).sum()
}
