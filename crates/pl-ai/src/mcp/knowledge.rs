//! Knowledge access of the MCP server: `ask_documents` (RAG with source references) and helpers for
//! `get_document_content` (pages/sections).
//!
//! **No second index:** semantic search runs via [`KnowledgeIndex`]; the production
//! implementation (`crate::server::mcp_index`) uses paperless's existing LLM index
//! (upstream `paperless_ai` llmindex: same nodes/chunks, same embedding model, same
//! similarity search and same permission filtering as the document chat `/api/documents/chat/`).
//! The answer is produced by the configured LLM with the chat templates (`chat_qa`/`chat_refine`,
//! `CompactAndRefine`) – only extended by numbered sources.
//!
//! Behavior:
//! * Index ready → hybrid: full text (search index via `search_documents`) + index hits,
//!   merged via Reciprocal Rank Fusion.
//! * AI on, index not built yet → notice + full text only (build is triggered like upstream).
//! * AI off / no embedding backend → full-text passages only.
//! * Without LLM (AI off) → only the weighted passages, no answer.

use std::collections::{BTreeMap, HashMap};

use futures::future::BoxFuture;
use serde::Serialize;

use super::McpService;
use super::backend::{Actor, Backend, DocumentDetail, SearchParams};

/// State of the LLM index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexState {
    /// AI off or no embedding backend configured.
    Disabled,
    /// AI on, index not (yet) built.
    NotBuilt,
    Ready,
}

/// A hit from the LLM index (one node).
#[derive(Debug, Clone, PartialEq)]
pub struct Passage {
    pub document_id: i64,
    /// Node text (chunk of the document content).
    pub text: String,
    /// Text for the LLM (`node.get_content(MetadataMode.LLM)`, like the chat).
    pub llm_text: String,
    pub score: f64,
}

/// Semantic search over the existing LLM index.
pub trait KnowledgeIndex: Send + Sync {
    fn state(&self) -> BoxFuture<'_, IndexState>;
    /// Top-k nodes only from documents that `actor` may see (like the chat), optionally restricted to
    /// `restrict`.
    fn semantic<'a>(
        &'a self,
        actor: &'a Actor,
        query: &'a str,
        top_k: usize,
        restrict: Option<&'a [i64]>,
    ) -> BoxFuture<'a, Result<Vec<Passage>, String>>;
    /// Output language for the user (`get_llm_output_language`), if known.
    fn output_language<'a>(&'a self, _actor: &'a Actor) -> BoxFuture<'a, Option<String>> {
        Box::pin(async { None })
    }
}

/// Page break in extracted PDF text (pdftotext/OCR sidecar).
pub const PAGE_BREAK: char = '\x0c';

/// Page number (1-based) at character position `offset`, only if the content has page breaks.
pub fn page_at(content: &str, offset: usize) -> Option<usize> {
    if !content.contains(PAGE_BREAK) {
        return None;
    }
    Some(content.chars().take(offset).filter(|c| *c == PAGE_BREAK).count() + 1)
}

/// Character position of a chunk in the content (via its beginning, whitespace-normalized roughly).
pub fn locate(content: &str, chunk: &str) -> Option<usize> {
    let probe: String = chunk.trim().chars().take(60).collect();
    if probe.chars().count() < 8 {
        return None;
    }
    content.find(&probe).map(|b| content[..b].chars().count())
}

fn terms(q: &str) -> Vec<String> {
    q.split(|c: char| !c.is_alphanumeric()).filter(|t| t.chars().count() >= 3).map(str::to_lowercase).collect()
}

/// Excerpt (at most `max` characters) around the first hit of a search term; returns
/// (excerpt, character position).
pub fn snippet(content: &str, query: &str, max: usize) -> (String, usize) {
    let lower = content.to_lowercase();
    // to_lowercase can change lengths → position via characters of the lowercase text
    let hit = terms(query).iter().filter_map(|t| lower.find(t.as_str()).map(|b| lower[..b].chars().count())).min();
    let chars: Vec<char> = content.chars().collect();
    let start = hit.map(|h| h.saturating_sub(max / 3)).unwrap_or(0).min(chars.len());
    let end = (start + max).min(chars.len());
    let s: String = chars[start..end].iter().collect();
    (s.split_whitespace().collect::<Vec<_>>().join(" "), start)
}

/// Short form for sources (at most 300 characters, whitespace collapsed).
pub fn short(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max { flat } else { format!("{}…", flat.chars().take(max).collect::<String>()) }
}

/// Part `index` of a content: by pages (if page breaks) or fixed sections.
/// Returns (mode, number of parts, text) or `None` if `index` is out of range.
pub fn content_part(content: &str, index: usize, chunk_chars: usize, by_page: bool) -> Option<(&'static str, usize, String)> {
    if by_page && content.contains(PAGE_BREAK) {
        let pages: Vec<&str> = content.split(PAGE_BREAK).collect();
        return pages.get(index).map(|p| ("pages", pages.len(), (*p).to_string()));
    }
    let chars: Vec<char> = content.chars().collect();
    let n = chars.len().div_ceil(chunk_chars.max(1)).max(1);
    if index >= n {
        return None;
    }
    let s = index * chunk_chars;
    Some(("chunks", n, chars[s..(s + chunk_chars).min(chars.len())].iter().collect()))
}

/// A source in the answer.
#[derive(Debug, Clone, Serialize)]
pub struct Citation {
    /// Number that `[n]` in the answer refers to.
    pub n: usize,
    pub document_id: i64,
    pub title: String,
    /// Page (1-based) if the text contains page breaks, otherwise `null`.
    pub page: Option<usize>,
    pub snippet: String,
    /// `semantic` (LLM index), `fulltext` (search index) or `both`.
    pub source: &'static str,
    pub score: f64,
    #[serde(skip)]
    pub llm_text: String,
}

const RRF_K: f64 = 60.0;

/// Result of [`ask`].
#[derive(Debug, Serialize)]
pub struct AskResult {
    pub question: String,
    pub index_state: IndexState,
    pub retrieval: &'static str,
    pub answer: Option<String>,
    pub answer_error: Option<String>,
    pub citations: Vec<Citation>,
    pub notes: Vec<String>,
}

/// Result of retrieval (steps 1–3 of [`ask`]).
#[derive(Debug, Clone)]
pub struct Retrieved {
    pub index_state: IndexState,
    pub retrieval: &'static str,
    /// Numbered sources (`n` from 1), by weight.
    pub citations: Vec<Citation>,
    pub notes: Vec<String>,
}

/// Retrieval of `ask_documents` (also `/rs/api/v2/ask`): full text via the backend (with the
/// user's permissions), index hits additionally checked against the user's `get_document`,
/// merged via Reciprocal Rank Fusion.
pub async fn retrieve<B: Backend>(
    svc: &McpService<B>,
    actor: &Actor,
    question: &str,
    max_passages: usize,
    restrict: Option<&[i64]>,
) -> Retrieved {
    let mut notes = Vec::new();
    let state = match &svc.knowledge {
        Some(k) => k.state().await,
        None => IndexState::Disabled,
    };
    // 1. Full text (search index of the server)
    let mut full: Vec<i64> = Vec::new();
    let p = SearchParams { query: Some(question.to_string()), limit: Some((max_passages * 2).min(50) as u32), ..Default::default() };
    match svc.backend.search_documents(actor, &p).await {
        Ok((_, docs)) => full = docs.into_iter().map(|d| d.id).filter(|id| restrict.is_none_or(|r| r.contains(id))).collect(),
        Err(e) => notes.push(format!("full-text search failed: {e}")),
    }
    // 2. LLM index (only if built)
    let mut sem: Vec<Passage> = Vec::new();
    match (state, &svc.knowledge) {
        (IndexState::Ready, Some(k)) => {
            match k.semantic(actor, question, max_passages.max(crate::chat::CHAT_RETRIEVER_TOP_K), restrict).await {
                Ok(v) => sem = v,
                Err(e) => {
                    tracing::warn!(target: "pl_ai::mcp", error = %e, "LLM index query failed");
                    notes.push("The LLM index could not be queried; using full-text search only.".into());
                }
            }
        }
        (IndexState::NotBuilt, _) => notes.push(
            "AI is enabled but the LLM index has not been built yet (its build has been queued); answering from full-text search only."
                .into(),
        ),
        _ => notes.push("AI/LLM index is not enabled on this server; returning full-text passages only.".into()),
    }
    // 3. Load documents (visibility!) and build passages
    let mut docs: HashMap<i64, Option<DocumentDetail>> = HashMap::new();
    let mut ids: Vec<i64> = full.clone();
    ids.extend(sem.iter().map(|s| s.document_id));
    for id in ids {
        if let std::collections::hash_map::Entry::Vacant(e) = docs.entry(id) {
            e.insert(svc.backend.get_document(actor, id).await.ok().flatten());
        }
    }
    let full_rank: BTreeMap<i64, usize> = full.iter().enumerate().map(|(i, d)| (*d, i)).collect();
    let mut sem_best: BTreeMap<i64, usize> = BTreeMap::new();
    for (i, s) in sem.iter().enumerate() {
        sem_best.entry(s.document_id).or_insert(i);
    }
    let mut cands: Vec<Citation> = Vec::new();
    for (i, s) in sem.iter().enumerate() {
        let Some(Some(d)) = docs.get(&s.document_id) else { continue }; // not visible → discard
        let score = 1.0 / (RRF_K + i as f64 + 1.0) + full_rank.get(&s.document_id).map(|r| 1.0 / (RRF_K + *r as f64 + 1.0)).unwrap_or(0.0);
        cands.push(Citation {
            n: 0,
            document_id: s.document_id,
            title: d.summary.title.clone(),
            page: locate(&d.content, &s.text).and_then(|o| page_at(&d.content, o)),
            snippet: short(&s.text, 300),
            source: if full_rank.contains_key(&s.document_id) { "both" } else { "semantic" },
            score,
            llm_text: s.llm_text.clone(),
        });
    }
    let chunk = svc.settings.llm_chunk_size.clamp(200, 4000) * 3; // Zeichen ≈ Tokens × 3
    for (r, id) in full.iter().enumerate() {
        if sem_best.contains_key(id) {
            continue; // already represented by an index passage
        }
        let Some(Some(d)) = docs.get(id) else { continue };
        let (text, off) = snippet(&d.content, question, chunk);
        if text.is_empty() {
            continue;
        }
        cands.push(Citation {
            n: 0,
            document_id: *id,
            title: d.summary.title.clone(),
            page: page_at(&d.content, off),
            snippet: short(&text, 300),
            source: "fulltext",
            score: 1.0 / (RRF_K + r as f64 + 1.0),
            llm_text: format!("title: {}\n\n{}", d.summary.title, text),
        });
    }
    cands.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    cands.truncate(max_passages);
    for (i, c) in cands.iter_mut().enumerate() {
        c.n = i + 1;
        c.score = (c.score * 1e6).round() / 1e6;
    }
    let retrieval = match (state, sem.is_empty()) {
        (IndexState::Ready, false) => "hybrid",
        _ => "fulltext",
    };
    Retrieved { index_state: state, retrieval, citations: cands, notes }
}

/// Start of the answer (step 4 of [`ask`]).
pub enum AnswerStart {
    /// No LLM configured (AI off) – passages only.
    NoLlm,
    /// LLM returned nothing (`Empty Response` like llama_index).
    Empty,
    /// Text deltas; an `Err` means the backend aborted.
    Stream(futures::stream::BoxStream<'static, Result<String, crate::llm::LlmError>>),
    /// Error before the first delta (message for the user).
    Failed(String),
}

/// Notice when no LLM is configured (like `ask_documents`).
pub const NOTE_NO_LLM: &str = "No LLM is configured; returning ranked passages only.";
/// Notice without hits.
pub const NOTE_NO_PASSAGES: &str = "No matching passages found in the documents you may see.";
/// Error message on abort during the stream.
pub const ANSWER_FAILED: &str = "The AI backend failed while answering.";

/// Answer with the configured LLM and the chat templates (numbered sources).
pub async fn start_answer<B: Backend>(svc: &McpService<B>, actor: &Actor, question: &str, citations: &[Citation]) -> AnswerStart {
    let Some(llm) = svc.current_llm().await else { return AnswerStart::NoLlm };
    let lang = match &svc.knowledge {
        Some(k) => k.output_language(actor).await,
        None => None,
    };
    let chunks: Vec<String> = citations.iter().map(|c| format!("[{}] {}", c.n, c.llm_text)).collect();
    let q = format!("{question}\n(Cite the sources you use as [n].)");
    let helper = crate::tokenizer::PromptHelper::rag(svc.settings.llm_chunk_size, svc.settings.llm_context_size);
    let qa = crate::prompts::chat_qa(lang.as_deref());
    let rf = crate::prompts::chat_refine(lang.as_deref());
    match crate::synth::compact_and_refine(&llm, &helper, &qa, &rf, &q, &chunks).await {
        Ok(Some(s)) => AnswerStart::Stream(s),
        Ok(None) => AnswerStart::Empty,
        Err(e) => {
            tracing::warn!(target: "pl_ai::mcp", error = %e, "ask_documents synthesis failed");
            AnswerStart::Failed(match e {
                crate::synth::SynthError::Llm(crate::llm::LlmError::Timeout) => "AI backend request timed out.".into(),
                crate::synth::SynthError::Llm(crate::llm::LlmError::Transport(_)) => "AI backend not reachable.".into(),
                _ => ANSWER_FAILED.into(),
            })
        }
    }
}

/// `ask_documents`. Permissions: full text via the backend (with the user's permissions),
/// index hits are additionally checked against the user's `get_document`.
pub async fn ask<B: Backend>(
    svc: &McpService<B>,
    actor: &Actor,
    question: &str,
    max_passages: usize,
    restrict: Option<&[i64]>,
    want_answer: bool,
) -> AskResult {
    let Retrieved { index_state: state, retrieval, citations: cands, mut notes } =
        retrieve(svc, actor, question, max_passages, restrict).await;
    // 4. Answer with the configured LLM (chat templates)
    let (mut answer, mut answer_error) = (None, None);
    if want_answer && !cands.is_empty() {
        match start_answer(svc, actor, question, &cands).await {
            AnswerStart::NoLlm => notes.push(NOTE_NO_LLM.into()),
            AnswerStart::Empty => answer = Some("Empty Response".into()),
            AnswerStart::Failed(m) => answer_error = Some(m),
            AnswerStart::Stream(mut s) => {
                use futures::StreamExt;
                let mut out = String::new();
                let mut failed = false;
                while let Some(d) = s.next().await {
                    match d {
                        Ok(t) => out.push_str(&t),
                        Err(_) => {
                            failed = true;
                            break;
                        }
                    }
                }
                if failed {
                    answer_error = Some(ANSWER_FAILED.into());
                } else {
                    answer = Some(out);
                }
            }
        }
    }
    if cands.is_empty() {
        notes.push(NOTE_NO_PASSAGES.into());
    }
    AskResult { question: question.to_string(), index_state: state, retrieval, answer, answer_error, citations: cands, notes }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_and_snippets() {
        let c = "Seite eins\x0cSeite zwei mit Rechnung 42\x0cSeite drei";
        assert_eq!(page_at(c, 0), Some(1));
        let (s, off) = snippet(c, "Wo ist die Rechnung?", 20);
        assert!(s.contains("Rechnung"), "{s}");
        assert_eq!(page_at(c, off), Some(2));
        assert_eq!(page_at("ohne Umbruch", 3), None);
        assert_eq!(locate(c, "Seite zwei mit Rechnung"), Some(11));
        let (mode, n, t) = content_part(c, 1, 5, true).unwrap();
        assert_eq!((mode, n, t.as_str()), ("pages", 3, "Seite zwei mit Rechnung 42"));
        let (mode, n, t) = content_part("abcdefghij", 1, 4, true).unwrap();
        assert_eq!((mode, n, t.as_str()), ("chunks", 3, "efgh"));
        assert!(content_part("abc", 5, 4, false).is_none());
        assert_eq!(short("a  b\n c", 300), "a b c");
        assert_eq!(short("abcdef", 3), "abc…");
    }
}
