//! Pure parts of upstream `paperless_ai/ai_classifier.py` and `paperless_ai/taxonomy.py`
//! (v3.4.2): candidate weighting from similar documents, prompt block, candidate
//! allowlist, output language. Everything that needs the DB, vector index or tokenizer is pulled out via
//! parameters/traits.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

use crate::prompts;
use crate::pyjson;
use crate::schema::AllowedIds;

/// upstream `taxonomy.MAX_TAG_CANDIDATES`.
pub const MAX_TAG_CANDIDATES: usize = 10;
/// upstream `taxonomy.MAX_SINGLE_VALUE_CANDIDATES`.
pub const MAX_SINGLE_VALUE_CANDIDATES: usize = 5;
/// upstream `ai_classifier.TAXONOMY_CANDIDATE_TOP_K`.
pub const TAXONOMY_CANDIDATE_TOP_K: usize = 15;
/// upstream `get_taxonomy_context(max_docs=5)`.
pub const CONTEXT_MAX_DOCS: usize = 5;
/// `build_prompt_without_rag`: `content[:4000]` before truncating.
pub const PROMPT_CONTENT_CHARS: usize = 4000;
/// `get_taxonomy_context`: `text[:1000]` per similar document.
pub const CONTEXT_DOC_CHARS: usize = 1000;

/// upstream `TaxonomyCandidate`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaxonomyCandidate {
    pub id: i64,
    pub name: String,
    pub weight: f64,
}

/// upstream `TaxonomyCandidates` (key order tags, document_types, correspondents,
/// storage_paths – relevant for the prompt JSON).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TaxonomyCandidates {
    pub tags: Vec<TaxonomyCandidate>,
    pub document_types: Vec<TaxonomyCandidate>,
    pub correspondents: Vec<TaxonomyCandidate>,
    pub storage_paths: Vec<TaxonomyCandidate>,
}

impl TaxonomyCandidates {
    fn categories(&self) -> [(&'static str, &Vec<TaxonomyCandidate>); 4] {
        [
            ("tags", &self.tags),
            ("document_types", &self.document_types),
            ("correspondents", &self.correspondents),
            ("storage_paths", &self.storage_paths),
        ]
    }

    /// `any(candidates.values())`.
    pub fn any(&self) -> bool {
        self.categories().iter().any(|(_, v)| !v.is_empty())
    }
}

/// upstream `SimilarDocument`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimilarDocument {
    pub document_id: i64,
    pub weight: f64,
}

/// A similar document with its *current* assignments (from the DB, inbox tags
/// already removed – upstream `Prefetch("tags", Tag.objects.filter(is_inbox_tag=False))`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NeighbourTaxonomy {
    pub document_id: i64,
    pub tag_ids: Vec<i64>,
    pub document_type_id: Option<i64>,
    pub correspondent_id: Option<i64>,
    pub storage_path_id: Option<i64>,
}

/// upstream `_node_document_weights(nodes)`: sum scores per `document_id`, sort descending
/// (stable sort like Python `sorted`, ties in insertion order).
pub fn node_document_weights(nodes: &[(Option<i64>, Option<f64>)]) -> Vec<SimilarDocument> {
    let mut order: Vec<i64> = Vec::new();
    let mut w: BTreeMap<i64, f64> = BTreeMap::new();
    for (doc, score) in nodes {
        let Some(d) = doc else { continue };
        if !w.contains_key(d) {
            order.push(*d);
        }
        *w.entry(*d).or_insert(0.0) += score.unwrap_or(0.0);
    }
    let mut out: Vec<SimilarDocument> =
        order.into_iter().map(|d| SimilarDocument { document_id: d, weight: w[&d] }).collect();
    out.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    out
}

/// upstream `_fulltext_similar_documents`: weight from the rank (`top_k - rank`).
pub fn fulltext_weights(similar_ids: &[i64], top_k: usize) -> Vec<SimilarDocument> {
    similar_ids
        .iter()
        .enumerate()
        .map(|(rank, id)| SimilarDocument { document_id: *id, weight: (top_k as f64) - rank as f64 })
        .collect()
}

/// Weights per object id in insertion order (Python `defaultdict` keeps the order).
#[derive(Debug, Default, Clone)]
pub struct Weights {
    order: Vec<i64>,
    w: BTreeMap<i64, f64>,
}

impl Weights {
    fn add(&mut self, id: i64, weight: f64) {
        if !self.w.contains_key(&id) {
            self.order.push(id);
        }
        *self.w.entry(id).or_insert(0.0) += weight;
    }
    pub fn ids(&self) -> Vec<i64> {
        self.order.clone()
    }
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }
}

/// Weighted ids per category (before the permission check).
#[derive(Debug, Default, Clone)]
pub struct WeightedTaxonomy {
    pub tags: Weights,
    pub document_types: Weights,
    pub correspondents: Weights,
    pub storage_paths: Weights,
}

/// First part of upstream `build_taxonomy_candidates`: sum up the weights.
/// `neighbours` in the order in which the DB returns them (upstream: `Document.objects.filter(pk__in=…)`,
/// default ordering `-created`).
pub fn weigh_neighbours(similar: &[SimilarDocument], neighbours: &[NeighbourTaxonomy]) -> WeightedTaxonomy {
    let weights: BTreeMap<i64, f64> = similar.iter().map(|s| (s.document_id, s.weight)).collect();
    let mut out = WeightedTaxonomy::default();
    for n in neighbours {
        let Some(w) = weights.get(&n.document_id).copied() else { continue };
        for t in &n.tag_ids {
            out.tags.add(*t, w);
        }
        if let Some(id) = n.document_type_id.filter(|v| *v != 0) {
            out.document_types.add(id, w);
        }
        if let Some(id) = n.correspondent_id.filter(|v| *v != 0) {
            out.correspondents.add(id, w);
        }
        if let Some(id) = n.storage_path_id.filter(|v| *v != 0) {
            out.storage_paths.add(id, w);
        }
    }
    out
}

/// upstream `_visible_ranked_candidates`: only visible ones (`visible_names` = id → name after
/// `restrict_queryset_to_owner_visible`), descending by weight (stable), capped.
pub fn visible_ranked(weights: &Weights, visible_names: &BTreeMap<i64, String>, limit: usize) -> Vec<TaxonomyCandidate> {
    let mut c: Vec<TaxonomyCandidate> = weights
        .order
        .iter()
        .filter_map(|id| {
            visible_names.get(id).map(|n| TaxonomyCandidate { id: *id, name: n.clone(), weight: weights.w[id] })
        })
        .collect();
    c.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    c.truncate(limit);
    c
}

/// upstream `format_taxonomy_for_prompt(candidates)`.
pub fn format_taxonomy_for_prompt(c: &TaxonomyCandidates) -> String {
    let mut payload = Map::new();
    for (k, v) in c.categories() {
        if v.is_empty() {
            continue;
        }
        payload.insert(k.into(), Value::Array(v.iter().map(|x| json!({"id": x.id, "name": x.name})).collect()));
    }
    let s = if payload.is_empty() {
        String::new()
    } else {
        pyjson::dumps(&Value::Object(payload), &pyjson::Dumps { ensure_ascii: false, ..pyjson::DEFAULT })
    };
    prompts::taxonomy_block(&s)
}

/// upstream `_candidate_id_allowlist(candidates)`.
pub fn candidate_id_allowlist(c: &TaxonomyCandidates) -> AllowedIds {
    c.categories().iter().map(|(k, v)| (k.to_string(), v.iter().map(|x| x.id).collect())).collect()
}

/// Text truncation like upstream `indexing.truncate_content` (llama_index `TokenTextSplitter` +
/// `PromptHelper.truncate`, tokenizer tiktoken `cl100k_base`). Not yet ported to Rust –
/// see contract "Entscheidungen". [`IdentityTruncator`] passes the text through unchanged.
pub trait Truncator: Send + Sync {
    fn truncate_content(&self, content: &str, chunk_size: usize, context_size: usize) -> String;
}

/// Does not truncate (only for tests/extension paths, **not** upstream-identical).
pub struct IdentityTruncator;

impl Truncator for IdentityTruncator {
    fn truncate_content(&self, content: &str, _chunk_size: usize, _context_size: usize) -> String {
        content.to_string()
    }
}

/// upstream `build_prompt_without_rag` (`content` = `get_effective_content() or ""`).
pub fn build_prompt_without_rag(
    filename: &str,
    content: &str,
    candidates: Option<&TaxonomyCandidates>,
    truncator: &dyn Truncator,
    chunk_size: usize,
    context_size: usize,
) -> String {
    let head: String = content.chars().take(PROMPT_CONTENT_CHARS).collect();
    let content = truncator.truncate_content(&head, chunk_size, context_size);
    let block = candidates.map(format_taxonomy_for_prompt).unwrap_or_default();
    let has = candidates.is_some_and(TaxonomyCandidates::any);
    prompts::classification(filename, &content, &block, has)
}

/// upstream `build_prompt_with_rag`.
pub fn build_prompt_with_rag(
    filename: &str,
    content: &str,
    candidates: Option<&TaxonomyCandidates>,
    context: &str,
    truncator: &dyn Truncator,
    chunk_size: usize,
    context_size: usize,
) -> String {
    let base = build_prompt_without_rag(filename, content, candidates, truncator, chunk_size, context_size);
    let ctx = truncator.truncate_content(context, chunk_size, context_size);
    prompts::rag_context(&base, &ctx)
}

/// RAG context block from `get_taxonomy_context`: `TITLE: <title>\n<text[:1000]>`, joined with `\n\n`.
pub fn context_blocks(docs: &[(Option<String>, Option<String>, Option<String>)]) -> String {
    docs.iter()
        .take(CONTEXT_MAX_DOCS)
        .map(|(title, filename, content)| {
            let text: String = content.as_deref().unwrap_or("").chars().take(CONTEXT_DOC_CHARS).collect();
            let t = title
                .as_deref()
                .filter(|s| !s.is_empty())
                .or(filename.as_deref().filter(|s| !s.is_empty()))
                .unwrap_or("Untitled");
            format!("TITLE: {t}\n{text}")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// `settings.LANGUAGES` (codes and English names; upstream translates the names via
/// `gettext_lazy` into the active language – see contract "Known differences").
pub const LANGUAGES: &[(&str, &str)] = &[
    ("en-us", "English (US)"),
    ("ar-ar", "Arabic"),
    ("af-za", "Afrikaans"),
    ("be-by", "Belarusian"),
    ("bg-bg", "Bulgarian"),
    ("ca-es", "Catalan"),
    ("cs-cz", "Czech"),
    ("da-dk", "Danish"),
    ("de-de", "German"),
    ("el-gr", "Greek"),
    ("en-gb", "English (GB)"),
    ("es-es", "Spanish"),
    ("fa-ir", "Persian"),
    ("fi-fi", "Finnish"),
    ("fr-fr", "French"),
    ("hu-hu", "Hungarian"),
    ("id-id", "Indonesian"),
    ("it-it", "Italian"),
    ("ja-jp", "Japanese"),
    ("ko-kr", "Korean"),
    ("lb-lu", "Luxembourgish"),
    ("no-no", "Norwegian"),
    ("nl-nl", "Dutch"),
    ("pl-pl", "Polish"),
    ("pt-br", "Portuguese (Brazil)"),
    ("pt-pt", "Portuguese"),
    ("ro-ro", "Romanian"),
    ("ru-ru", "Russian"),
    ("sk-sk", "Slovak"),
    ("sl-si", "Slovenian"),
    ("sr-cs", "Serbian"),
    ("sv-se", "Swedish"),
    ("tr-tr", "Turkish"),
    ("uk-ua", "Ukrainian"),
    ("vi-vn", "Vietnamese"),
    ("zh-cn", "Chinese Simplified"),
    ("zh-tw", "Chinese Traditional"),
];

/// upstream `get_language_name(language_code)`.
pub fn get_language_name(code: &str) -> String {
    let n = code.to_lowercase();
    LANGUAGES.iter().find(|(c, _)| c.to_lowercase() == n).map(|(_, name)| name.to_string()).unwrap_or_else(|| code.to_string())
}

/// upstream `get_llm_output_language(ai_config, user)`: configured language, else
/// `ui_settings.settings["language"]` of the user (only if `settings` is an object).
pub fn get_llm_output_language(configured: Option<&str>, ui_settings: Option<&Value>) -> Option<String> {
    if let Some(c) = configured.filter(|c| !c.is_empty()) {
        return Some(c.to_string());
    }
    let lang = ui_settings.and_then(Value::as_object).and_then(|o| o.get("language"))?;
    match lang {
        Value::Null => None,
        Value::String(s) if s.is_empty() => None,
        Value::String(s) => Some(s.clone()),
        // upstream passes arbitrary JSON values through (e.g. a number) – as text
        other => Some(other.to_string()),
    }
}

/// Cache key part `llm_cache_backend` from `DocumentViewSet.ai_suggestions`.
pub fn llm_cache_backend(backend: Option<&str>, model: Option<&str>, endpoint: Option<&str>, output_language: Option<&str>, user_id: i64) -> String {
    let user = format!("user={user_id}");
    [backend, model, endpoint, output_language, Some(user.as_str())]
        .into_iter()
        .flatten()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_and_ranking() {
        let nodes = vec![(Some(1), Some(0.5)), (Some(2), Some(0.9)), (Some(1), Some(0.5)), (None, Some(9.0))];
        let s = node_document_weights(&nodes);
        assert_eq!(s.iter().map(|x| x.document_id).collect::<Vec<_>>(), vec![1, 2]);
        let nb = vec![
            NeighbourTaxonomy { document_id: 1, tag_ids: vec![10, 11], correspondent_id: Some(5), ..Default::default() },
            NeighbourTaxonomy { document_id: 2, tag_ids: vec![11], correspondent_id: Some(6), ..Default::default() },
        ];
        let w = weigh_neighbours(&s, &nb);
        let names: BTreeMap<i64, String> = [(10, "a".into()), (11, "b".into()), (5, "c".into())].into_iter().collect();
        let tags = visible_ranked(&w.tags, &names, MAX_TAG_CANDIDATES);
        assert_eq!(tags.iter().map(|t| t.id).collect::<Vec<_>>(), vec![11, 10]);
        let corr = visible_ranked(&w.correspondents, &names, MAX_SINGLE_VALUE_CANDIDATES);
        assert_eq!(corr.len(), 1, "unsichtbare (6) fallen weg");
    }

    #[test]
    fn language() {
        assert_eq!(get_language_name("DE-de"), "German");
        assert_eq!(get_language_name("xx"), "xx");
        let ui = json!({"language": "fr-fr"});
        assert_eq!(get_llm_output_language(None, Some(&ui)).as_deref(), Some("fr-fr"));
        assert_eq!(get_llm_output_language(Some("de"), Some(&ui)).as_deref(), Some("de"));
        assert_eq!(get_llm_output_language(None, Some(&json!([]))), None);
        assert_eq!(llm_cache_backend(Some("ollama"), Some("llama3.1"), None, Some("de"), 4), "ollama:llama3.1:de:user=4");
    }

    #[test]
    fn context() {
        let c = context_blocks(&[(None, Some("f.pdf".into()), Some("abc".into())), (Some("".into()), None, None)]);
        assert_eq!(c, "TITLE: f.pdf\nabc\n\nTITLE: Untitled\n");
    }
}
