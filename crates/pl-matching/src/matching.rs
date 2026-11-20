// upstream: src/documents/matching.py@v3.4.2 (matches, _split_match, log_reason,
//           match_correspondents, match_document_types, match_tags, match_storage_paths)
//           src/documents/models.py@v3.4.2 (MatchingModel constants, Document.suggestion_content)
//
// Pure functions over own data types. Database access (permitted_object_ids,
// get_effective_content with versions) is the caller's job: pass the already
// permission-filtered candidate objects and the resolved effective content.

use crate::fuzz::partial_ratio;
use crate::pyre::{self, BoundedSearch, PY_SPACE_CLASS};
use crate::regex_compat::safe_regex_search;

pub const MATCH_NONE: i64 = 0;
pub const MATCH_ANY: i64 = 1;
pub const MATCH_ALL: i64 = 2;
pub const MATCH_LITERAL: i64 = 3;
pub const MATCH_REGEX: i64 = 4;
pub const MATCH_FUZZY: i64 = 5;
pub const MATCH_AUTO: i64 = 6;

/// Upstream raises `NotImplementedError("Unsupported matching algorithm")`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedMatchingAlgorithm(pub i64);

impl std::fmt::Display for UnsupportedMatchingAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Unsupported matching algorithm")
    }
}

impl std::error::Error for UnsupportedMatchingAlgorithm {}

/// Anything with `match`, `matching_algorithm`, `is_insensitive`
/// (Tag, Correspondent, DocumentType, StoragePath, WorkflowTrigger).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Matchable {
    pub id: i64,
    /// Python class name used in log messages ("Tag", "Correspondent", "WorkflowTrigger", ...).
    pub class_name: String,
    /// `name` attribute, or `str(obj)` for objects without a name (WorkflowTrigger).
    pub name: String,
    pub match_text: String,
    pub matching_algorithm: i64,
    pub is_insensitive: bool,
}

impl Matchable {
    pub fn new(match_text: &str, matching_algorithm: i64, is_insensitive: bool) -> Self {
        Self {
            id: 0,
            class_name: "MatchingModel".into(),
            name: String::new(),
            match_text: match_text.into(),
            matching_algorithm,
            is_insensitive,
        }
    }
}

/// The document as seen by the matcher.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MatchDocument {
    /// `document.get_effective_content()` (latest version's content for root documents).
    pub effective_content: Option<String>,
    /// `str(document)` for log messages.
    pub display: String,
}

impl MatchDocument {
    pub fn from_content(content: &str) -> Self {
        Self { effective_content: Some(content.into()), display: String::new() }
    }

    /// `Document.suggestion_content`: crops very long content (>1_200_000 chars) to
    /// the first 800_000 and last 200_000 chars joined by a space.
    pub fn suggestion_content(&self) -> Option<String> {
        let c = self.effective_content.as_deref()?;
        let n = c.chars().count();
        if c.is_empty() || n <= 1_200_000 {
            return Some(c.to_string());
        }
        let head: String = c.chars().take(800_000).collect();
        let tail: String = c.chars().skip(n - 200_000).collect();
        Some(format!("{head} {tail}"))
    }
}

fn log_reason(m: &Matchable, doc: &MatchDocument, reason: &str) {
    tracing::debug!(
        target: "paperless.matching",
        "{} {} matched on document {} because {}",
        m.class_name,
        m.name,
        doc.display,
        reason
    );
}

/// `_split_match`: keywords as regex fragments (escaped, inner whitespace -> `\s+`).
pub fn split_match(match_text: &str) -> Vec<String> {
    pyre::findterms(match_text)
        .into_iter()
        .map(|(quoted, plain)| {
            let t = if quoted.is_empty() { plain } else { quoted };
            let norm = pyre::normspace(pyre::py_strip(&t));
            norm.split(' ').map(pyre::escape).collect::<Vec<_>>().join(&format!("{PY_SPACE_CLASS}+"))
        })
        .collect()
}

/// `_split_match` exactly as upstream returns it (Python `re.escape` output with
/// `\ ` replaced by `\s+`); for display/tests only.
pub fn split_match_python(match_text: &str) -> Vec<String> {
    pyre::findterms(match_text)
        .into_iter()
        .map(|(quoted, plain)| {
            let t = if quoted.is_empty() { plain } else { quoted };
            let norm = pyre::normspace(pyre::py_strip(&t));
            python_re_escape(&norm).replace(r"\ ", r"\s+")
        })
        .collect()
}

/// CPython 3.7+ `re.escape`.
pub fn python_re_escape(s: &str) -> String {
    const SPECIAL: &str = "()[]{}?*+-|^$\\.&~# \t\n\r\x0b\x0c";
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if SPECIAL.contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Compiled keyword searches, keyed by (pattern, ignore_case) (PERF-SCALE Q8, [`crate::cache`]).
static BOUNDED: crate::cache::PatternCache<(String, bool), BoundedSearch> = crate::cache::PatternCache::new();

fn word_search(fragment: &str, content: &str, insensitive: bool) -> bool {
    match BOUNDED.get_or_try((fragment.to_string(), insensitive), || BoundedSearch::new(fragment, insensitive)) {
        Ok(s) => s.is_match(content),
        Err(e) => {
            // cannot happen for escaped fragments; upstream would raise re.error
            tracing::error!(target: "paperless.matching", "invalid keyword pattern {fragment}: {e}");
            false
        }
    }
}

/// `re.sub(r"[^\w\s]", "", s)` with Python `re` classes.
fn strip_non_word_space(s: &str) -> String {
    s.chars().filter(|&c| pyre::is_py_word(c) || pyre::is_py_space(c)).collect()
}

/// `matches(matching_model, document)`.
pub fn matches(m: &Matchable, doc: &MatchDocument) -> Result<bool, UnsupportedMatchingAlgorithm> {
    let content = doc.effective_content.as_deref().unwrap_or("");

    if pyre::py_strip(&m.match_text).is_empty() {
        return Ok(false);
    }
    let insensitive = m.is_insensitive;

    match m.matching_algorithm {
        MATCH_NONE => Ok(false),
        MATCH_ALL => {
            for word in split_match(&m.match_text) {
                if !word_search(&word, content, insensitive) {
                    return Ok(false);
                }
            }
            log_reason(m, doc, &format!("it contains all of these words: {}", m.match_text));
            Ok(true)
        }
        MATCH_ANY => {
            for (word, py_word) in split_match(&m.match_text).into_iter().zip(split_match_python(&m.match_text)) {
                if word_search(&word, content, insensitive) {
                    log_reason(m, doc, &format!("it contains this word: {py_word}"));
                    return Ok(true);
                }
            }
            Ok(false)
        }
        MATCH_LITERAL => {
            let result = word_search(&pyre::escape(&m.match_text), content, insensitive);
            if result {
                log_reason(m, doc, &format!("it contains this string: \"{}\"", m.match_text));
            }
            Ok(result)
        }
        MATCH_REGEX => {
            let found = safe_regex_search(&m.match_text, content, insensitive);
            if let Some(f) = &found {
                log_reason(
                    m,
                    doc,
                    &format!(
                        "the string {} matches the regular expression {}",
                        f.text, m.match_text
                    ),
                );
            }
            Ok(found.is_some())
        }
        MATCH_FUZZY => {
            let mut pattern = strip_non_word_space(&m.match_text);
            let mut text = strip_non_word_space(content);
            if insensitive {
                pattern = pattern.to_lowercase();
                text = text.to_lowercase();
            }
            if partial_ratio(&pattern, &text, 90.0) != 0.0 {
                log_reason(
                    m,
                    doc,
                    &format!("parts of the document content somehow match the string {}", m.match_text),
                );
                Ok(true)
            } else {
                Ok(false)
            }
        }
        // this is done elsewhere (classifier)
        MATCH_AUTO => Ok(false),
        other => Err(UnsupportedMatchingAlgorithm(other)),
    }
}

/// Interface to the (later) Rust port of `documents.classifier.DocumentClassifier`.
/// Input is `document.suggestion_content`.
pub trait Classifier {
    fn predict_correspondent(&self, content: &str) -> Option<i64>;
    fn predict_document_type(&self, content: &str) -> Option<i64>;
    fn predict_tags(&self, content: &str) -> Vec<i64>;
    fn predict_storage_path(&self, content: &str) -> Option<i64>;
}

fn filter_single<'a>(
    doc: &MatchDocument,
    candidates: &'a [Matchable],
    pred_id: Option<i64>,
) -> Result<Vec<&'a Matchable>, UnsupportedMatchingAlgorithm> {
    let mut out = Vec::new();
    for o in candidates {
        if matches(o, doc)? || (Some(o.id) == pred_id && o.matching_algorithm == MATCH_AUTO) {
            out.push(o);
        }
    }
    Ok(out)
}

fn suggestion(doc: &MatchDocument) -> String {
    doc.suggestion_content().unwrap_or_default()
}

/// `match_correspondents`. `candidates` = correspondents visible to the user
/// (`permitted_object_ids(user, Correspondent, "view_correspondent")`, or all if
/// there is neither a user nor a document owner), in queryset order.
pub fn match_correspondents<'a>(
    doc: &MatchDocument,
    candidates: &'a [Matchable],
    classifier: Option<&dyn Classifier>,
) -> Result<Vec<&'a Matchable>, UnsupportedMatchingAlgorithm> {
    let pred = classifier.and_then(|c| c.predict_correspondent(&suggestion(doc)));
    filter_single(doc, candidates, pred)
}

/// `match_document_types`.
pub fn match_document_types<'a>(
    doc: &MatchDocument,
    candidates: &'a [Matchable],
    classifier: Option<&dyn Classifier>,
) -> Result<Vec<&'a Matchable>, UnsupportedMatchingAlgorithm> {
    let pred = classifier.and_then(|c| c.predict_document_type(&suggestion(doc)));
    filter_single(doc, candidates, pred)
}

/// `match_storage_paths`.
pub fn match_storage_paths<'a>(
    doc: &MatchDocument,
    candidates: &'a [Matchable],
    classifier: Option<&dyn Classifier>,
) -> Result<Vec<&'a Matchable>, UnsupportedMatchingAlgorithm> {
    let pred = classifier.and_then(|c| c.predict_storage_path(&suggestion(doc)));
    filter_single(doc, candidates, pred)
}

/// `match_tags`.
pub fn match_tags<'a>(
    doc: &MatchDocument,
    candidates: &'a [Matchable],
    classifier: Option<&dyn Classifier>,
) -> Result<Vec<&'a Matchable>, UnsupportedMatchingAlgorithm> {
    let predicted = classifier.map(|c| c.predict_tags(&suggestion(doc))).unwrap_or_default();
    let mut out = Vec::new();
    for o in candidates {
        if matches(o, doc)? || (o.matching_algorithm == MATCH_AUTO && predicted.contains(&o.id)) {
            out.push(o);
        }
    }
    Ok(out)
}
