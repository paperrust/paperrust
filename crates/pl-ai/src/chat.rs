//! Chat response format – upstream `paperless_ai/chat.py@v3.4.2` and
//! `documents/views.py::ChatStreamingView`.
//!
//! Response: `StreamingHttpResponse(content_type="text/event-stream")`, but **no** SSE
//! framing: the body is the raw text of the LLM token deltas, chunk by chunk, and at the end
//! optionally the trailer `"\n\n__PAPERLESS_CHAT_METADATA__" + json.dumps({"references": [...]},
//! separators=(",", ":"))` (ensure_ascii=True). Error during the stream → the text
//! [`CHAT_ERROR_MESSAGE`] as the last chunk (status stays 200).

use serde_json::{json, Value};

use crate::pyjson;

pub const CHAT_METADATA_DELIMITER: &str = "\n\n__PAPERLESS_CHAT_METADATA__";
pub const CHAT_ERROR_MESSAGE: &str = "Sorry, something went wrong while generating a response.";
pub const CHAT_NO_CONTENT_MESSAGE: &str = "Sorry, I couldn't find any content to answer your question.";
pub const CHAT_RETRIEVER_TOP_K: usize = 5;
/// upstream fix e0c33b3d3 (#14723, after v3.4.2): `MAX_CHAT_REFERENCES = CHAT_RETRIEVER_TOP_K`
/// (v3.4.2: 3).
pub const MAX_CHAT_REFERENCES: usize = CHAT_RETRIEVER_TOP_K;
/// `ChatStreamingSerializer.q` `max_length`.
pub const CHAT_MAX_QUESTION: usize = 4000;

/// `_permitted_documents_filter(store, permitted_ids)` (upstream fix 62d729e30 / #14754, after
/// v3.4.2): restrict retrieval to `permitted` with whichever list is shorter – the permitted
/// ids (`IN`) or the indexed-but-not-permitted ids (`NOT IN`); a long inclusion list is much
/// slower for vec0 than a long exclusion list. An exclusion list fails open for a document
/// indexed after `indexed` was read, so callers must still drop hits outside `permitted`
/// ([`retain_permitted`]).
pub fn permitted_documents_filter(
    indexed: &std::collections::HashSet<i64>,
    permitted: &std::collections::HashSet<i64>,
) -> crate::vector_store::DocFilter {
    let mut denied: Vec<i64> = indexed.difference(permitted).copied().collect();
    if permitted.len() <= denied.len() {
        let mut ids: Vec<i64> = permitted.iter().copied().collect();
        ids.sort_unstable();
        crate::vector_store::DocFilter::In(ids)
    } else {
        denied.sort_unstable();
        crate::vector_store::DocFilter::NotIn(denied)
    }
}

/// Upstream fix 62d729e30: after retrieval keep only nodes whose `document_id` is permitted.
pub fn retain_permitted(hits: &mut Vec<crate::vector_store::Hit>, permitted: &std::collections::HashSet<i64>) {
    hits.retain(|h| h.node.document_id().is_some_and(|d| permitted.contains(&d)));
}

/// Marker in the trailer delimiter (without the two line breaks).
pub const CHAT_METADATA_TOKEN: &str = "__PAPERLESS_CHAT_METADATA__";
/// Replacement for the marker in LLM output (contains no `_`, so it cannot form a new marker with
/// neighboring text).
const CHAT_METADATA_TOKEN_REPLACEMENT: &str = "PAPERRUST CHAT METADATA";

/// Security wave 3 AI-N6 (deviation from upstream `chat.py:89`, Known differences): the frontend
/// (`chat.service.ts::parseChatResponse`) splits at the **first** occurrence of
/// [`CHAT_METADATA_DELIMITER`]. Via prompt injection the LLM output could contain the delimiter
/// and fake references. The guard replaces the marker in the streamed deltas,
/// even when it is spread across chunk boundaries; for that it holds back at most `len(marker) - 1` bytes,
/// and only if the chunk end is a prefix of the marker. Normal output stays
/// byte-identical.
#[derive(Debug, Default)]
pub struct TrailerGuard {
    pending: String,
}

impl TrailerGuard {
    /// Next delta; returns the part that is safe to emit (may be empty).
    pub fn push(&mut self, delta: &str) -> String {
        self.pending.push_str(delta);
        let s = if self.pending.contains(CHAT_METADATA_TOKEN) {
            self.pending.replace(CHAT_METADATA_TOKEN, CHAT_METADATA_TOKEN_REPLACEMENT)
        } else {
            std::mem::take(&mut self.pending)
        };
        let keep = (1..CHAT_METADATA_TOKEN.len())
            .rev()
            .find(|&k| k <= s.len() && s.is_char_boundary(s.len() - k) && CHAT_METADATA_TOKEN.starts_with(&s[s.len() - k..]))
            .unwrap_or(0);
        let cut = s.len() - keep;
        self.pending = s[cut..].to_string();
        let mut out = s;
        out.truncate(cut);
        out
    }

    /// Remainder at the end of the stream.
    pub fn finish(&mut self) -> String {
        std::mem::take(&mut self.pending)
    }
}

/// A document reference `{"id": .., "title": ..}`.
#[derive(Debug, Clone, PartialEq)]
pub struct Reference {
    pub id: i64,
    pub title: String,
}

/// upstream `_format_chat_metadata_trailer(references)`.
pub fn format_metadata_trailer(refs: &[Reference]) -> String {
    let v = json!({"references": refs.iter().map(|r| json!({"id": r.id, "title": r.title})).collect::<Vec<Value>>()});
    let o = pyjson::Dumps { item_sep: ",", key_sep: ":", ensure_ascii: true };
    format!("{CHAT_METADATA_DELIMITER}{}", pyjson::dumps(&v, &o))
}

/// upstream `_get_document_references(documents, top_nodes)`: order of the hits,
/// without duplicates, only allowed documents, at most [`MAX_CHAT_REFERENCES`]. Title: the one
/// stored in the node, else `document.title`, else `document.filename`.
///
/// `nodes`: (document_id, title from the node metadata); `allowed`: id → (title, filename).
pub fn document_references(
    nodes: &[(Option<i64>, Option<String>)],
    allowed: &std::collections::BTreeMap<i64, (Option<String>, Option<String>)>,
) -> Vec<Reference> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for (id, node_title) in nodes {
        let Some(id) = id else { continue };
        let Some((title, filename)) = allowed.get(id) else { continue };
        if !seen.insert(*id) {
            continue;
        }
        let t = node_title
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| title.clone().filter(|s| !s.is_empty()))
            .or_else(|| filename.clone())
            .unwrap_or_default();
        out.push(Reference { id: *id, title: t });
        if out.len() >= MAX_CHAT_REFERENCES {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector_store::DocFilter;

    /// upstream fix 62d729e30 (#14754): `test_permitted_filter_uses_shorter_list` and
    /// `test_exclusion_filter_drops_late_indexed_documents`.
    #[test]
    fn permitted_filter_uses_shorter_list() {
        let set = |r: std::ops::RangeInclusive<i64>| r.collect::<std::collections::HashSet<i64>>();
        // few permitted → IN
        assert_eq!(permitted_documents_filter(&set(1..=10), &set(1..=3)), DocFilter::In(vec![1, 2, 3]));
        // equal length → IN (`<=`)
        assert_eq!(permitted_documents_filter(&set(1..=4), &set(1..=2)), DocFilter::In(vec![1, 2]));
        // most permitted → NOT IN over the rest
        assert_eq!(permitted_documents_filter(&set(1..=50_000), &set(1..=49_998)), DocFilter::NotIn(vec![49_999, 50_000]));
        // nothing indexed → IN (empty denied list is shorter only when permitted is empty)
        let empty = std::collections::HashSet::new();
        assert_eq!(permitted_documents_filter(&empty, &empty), DocFilter::In(vec![]));

        // a document indexed after the snapshot passes NOT IN, but is dropped afterwards
        let hit = |d: i64| {
            let mut metadata = serde_json::Map::new();
            metadata.insert("document_id".into(), Value::String(d.to_string()));
            let node = crate::node::TextNode {
                id: format!("{d}-a"),
                text: String::new(),
                metadata,
                excluded_embed_metadata_keys: vec![],
                excluded_llm_metadata_keys: vec![],
                relationships: serde_json::Map::new(),
                start_char_idx: None,
                end_char_idx: None,
                metadata_template: String::new(),
                metadata_separator: String::new(),
                text_template: String::new(),
                ref_doc_id: None,
            };
            crate::vector_store::Hit { node, score: 1.0 }
        };
        let mut hits = vec![hit(5), hit(99), hit(6)];
        retain_permitted(&mut hits, &set(1..=10));
        assert_eq!(hits.iter().filter_map(|h| h.node.document_id()).collect::<Vec<_>>(), vec![5, 6]);
    }

    #[test]
    fn refs() {
        let allowed = [(1, (Some("A".into()), None)), (2, (None, Some("b.pdf".into())))].into_iter().collect();
        let r = document_references(&[(Some(2), None), (Some(9), None), (Some(1), Some("N".into())), (Some(2), None)], &allowed);
        assert_eq!(r, vec![Reference { id: 2, title: "b.pdf".into() }, Reference { id: 1, title: "N".into() }]);
    }

    /// upstream fix e0c33b3d3: up to `CHAT_RETRIEVER_TOP_K` (5) references instead of 3.
    #[test]
    fn refs_up_to_top_k() {
        let allowed = (1..=6).map(|i| (i, (Some(format!("D{i}")), None))).collect();
        let nodes: Vec<_> = (1..=6).map(|i| (Some(i), None)).collect();
        let r = document_references(&nodes, &allowed);
        assert_eq!(MAX_CHAT_REFERENCES, CHAT_RETRIEVER_TOP_K);
        assert_eq!(r.iter().map(|x| x.id).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
    }

    fn guarded(parts: &[&str]) -> (String, Vec<String>) {
        let mut g = TrailerGuard::default();
        let mut pieces: Vec<String> = parts.iter().map(|p| g.push(p)).collect();
        pieces.push(g.finish());
        (pieces.concat(), pieces)
    }

    /// Security wave 3 AI-N6: an injected delimiter in the LLM output is defused, even across
    /// chunk boundaries; normal output stays unchanged (also the chunking).
    #[test]
    fn trailer_spoofing_neutralised() {
        let fake = format!("Antwort{CHAT_METADATA_DELIMITER}{{\"references\":[{{\"id\":666,\"title\":\"x\"}}]}}");
        let (all, _) = guarded(&[&fake]);
        assert!(!all.contains(CHAT_METADATA_TOKEN), "{all}");
        assert!(all.starts_with("Antwort\n\nPAPERRUST CHAT METADATA{"));
        // spread across chunk boundaries
        let (all, _) = guarded(&["a\n\n__PAPER", "LESS_CHAT_", "METADATA__{}", " Ende"]);
        assert_eq!(all, "a\n\nPAPERRUST CHAT METADATA{} Ende");
        // nesting does not form a new marker
        let (all, _) = guarded(&["___PAPERLESS_CHAT_METADATA___PAPERLESS_CHAT_METADATA___"]);
        assert!(!all.contains(CHAT_METADATA_TOKEN), "{all}");
        // normal output: same chunks
        let (all, pieces) = guarded(&["Hallo ", "Welt", ". Ümlaut ✓"]);
        assert_eq!(all, "Hallo Welt. Ümlaut ✓");
        assert_eq!(pieces[..3], ["Hallo ", "Welt", ". Ümlaut ✓"]);
        // held-back prefix that turns out not to be a marker
        let (all, pieces) = guarded(&["snake_", "case"]);
        assert_eq!(all, "snake_case");
        assert_eq!(pieces[0], "snake");
        // real trailer (appended by the server, not by the guard) stays parseable
        let t = format_metadata_trailer(&[Reference { id: 1, title: "A".into() }]);
        assert!(t.starts_with(CHAT_METADATA_DELIMITER));
    }
}
