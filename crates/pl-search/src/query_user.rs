// upstream: src/documents/search/_query.py@v3.4.2 (query-mode glue, lines
//           ~200-672), src/documents/search/_registry.py, _fields.py
//
// parse_user_query: whoosh-compat parse -> diagnostics -> CJK/fuzzy leaf
// widening via emit()'s rewrite_leaf hook -> (fuzzy) const-score blend.

use std::collections::HashSet;
use std::sync::Arc;

use tantivy::query::{BooleanQuery, BoostQuery, ConstScoreQuery, Occur, Query};
use tantivy::tokenizer::{SimpleTokenizer, TextAnalyzer};

use crate::error::{py_repr, py_repr_opt, SearchError};
use crate::query_simple::{cjk_runs, has_cjk};
use crate::tokenizer::{analyze, ascii_fold, bigram_analyzer, paperless_text_analyzer, py_lower, stem_pattern_text};
use crate::wc;
use crate::wc::ast::Node;
use crate::wc::errors::{Cause, Diagnostic, DiagnosticKind, QueryError};
use crate::wc::fields::{Analyzer, FieldKind, FieldRef, FieldRegistry, FieldSpec, Multitoken, PatternNormalizer};

pub struct UserQueryConfig<'a> {
    /// settings.SEARCH_LANGUAGE
    pub language: Option<&'a str>,
    /// settings.ADVANCED_FUZZY_SEARCH_THRESHOLD is not None
    pub fuzzy: bool,
    /// django get_current_timezone()
    pub tz: chrono_tz::Tz,
    /// "now" for relative dates
    pub now: chrono::DateTime<chrono::Utc>,
}

/// `_DEFAULT_SEARCH_FIELDS`
pub const DEFAULT_SEARCH_FIELDS: [&str; 5] = ["title", "content", "correspondent", "document_type", "tag"];
/// `_FIELD_BOOSTS`
const FIELD_BOOSTS: [(&str, f64); 1] = [("title", 2.0)];
/// `_FUZZY_TIEBREAK`
const FUZZY_TIEBREAK: f32 = 0.01;

/// `_CJK_BIGRAM_FIELDS`
fn bigram_field(name: &str) -> Option<String> {
    DEFAULT_SEARCH_FIELDS.contains(&name).then(|| format!("bigram_{name}"))
}

/// _fields.py::PUBLIC_FIELDS
fn public_fields() -> Vec<FieldSpec> {
    use FieldKind::*;
    let mut v: Vec<FieldSpec> = [
        ("title", Text),
        ("content", Text),
        ("correspondent", Text),
        ("document_type", Text),
        ("storage_path", Text),
        ("original_filename", Text),
        ("tag", Text),
        ("checksum", Keyword),
        ("asn", U64),
        ("page_count", U64),
        ("num_notes", U64),
        ("created", Date),
        ("modified", Datetime),
        ("added", Datetime),
        ("notes", Json),
        ("custom_fields", Json),
        ("barcodes", Json),
    ]
    .into_iter()
    .map(|(n, k)| FieldSpec::new(n, k))
    .collect();
    for s in v.iter_mut() {
        match s.name.as_str() {
            "document_type" => s.aliases = vec!["type".into()],
            "storage_path" => s.aliases = vec!["path".into()],
            "tag" => s.comma_values = true,
            "asn" | "page_count" | "num_notes" | "modified" | "added" => s.fast = true,
            "created" => {
                s.fast = true;
                s.date_only = true;
            }
            "notes" => s.subpaths = vec![("user".into(), false), ("note".into(), true)],
            "custom_fields" => s.subpaths = vec![("name".into(), false), ("value".into(), true)],
            "barcodes" => s.subpaths = vec![("value".into(), true), ("format".into(), false)],
            _ => {}
        }
    }
    v
}

/// _registry.py::get_field_registry
pub fn get_field_registry(language: Option<&str>) -> FieldRegistry {
    let text_a = paperless_text_analyzer(language);
    let text_analyzer: Analyzer = Arc::new(move |t: &str| analyze(&text_a, t));
    let identity: Analyzer = Arc::new(|t: &str| vec![t.to_string()]);
    let lang = language.map(str::to_string);
    // _make_pattern_normalizer
    let pattern: PatternNormalizer = Arc::new(move |t: &str| {
        let folded = ascii_fold(&py_lower(t));
        let stemmed = stem_pattern_text(&folded, lang.as_deref());
        vec![folded, stemmed]
    });
    // _fold_normalizer
    let fold: PatternNormalizer = Arc::new(|t: &str| vec![ascii_fold(&py_lower(t))]);
    let specs = public_fields()
        .into_iter()
        .map(|mut s| {
            if s.kind == FieldKind::Keyword {
                s.analyzer = Some(identity.clone());
                s.pattern_normalizer = Some(fold.clone());
            } else {
                s.analyzer = Some(text_analyzer.clone());
                s.pattern_normalizer = Some(pattern.clone());
            }
            s
        })
        .collect();
    FieldRegistry::new(specs)
}

/// `_get_emit_field_registry`
fn get_emit_field_registry(language: Option<&str>) -> FieldRegistry {
    let mut specs = get_field_registry(language).specs().to_vec();
    let b = bigram_analyzer();
    let bigram: Analyzer = Arc::new(move |t: &str| analyze(&b, t));
    for f in DEFAULT_SEARCH_FIELDS {
        let mut s = FieldSpec::new(&format!("bigram_{f}"), FieldKind::Text);
        s.analyzer = Some(bigram.clone());
        s.multitoken = Multitoken::And;
        specs.push(s);
    }
    FieldRegistry::new(specs)
}

/// `_collapse`
fn collapse(and: bool, mut children: Vec<Node>) -> Node {
    if children.len() == 1 {
        return children.remove(0);
    }
    if and {
        Node::And(children)
    } else {
        Node::Or(children)
    }
}

/// (field, str(leaf.text), is Phrase)
fn leaf_parts(leaf: &Node) -> Option<(&FieldRef, String, bool)> {
    match leaf {
        Node::Term { field: Some(f), text, .. } => Some((f, text.py_str(), false)),
        Node::Phrase { field: Some(f), text, .. } => Some((f, text.clone(), true)),
        _ => None,
    }
}

/// `_cjk_alternative`
fn cjk_alternative(leaf: &Node) -> Option<Node> {
    let (field, text, is_phrase) = leaf_parts(leaf)?;
    let bf = bigram_field(&field.name)?;
    if !has_cjk(&text) {
        return None;
    }
    // `_TOKEN_SPLITTER`: the simple tokenizer with no filters.
    let splitter = TextAnalyzer::builder(SimpleTokenizer::default()).build();
    let bref = FieldRef::plain(bf);
    let mut cjk_terms = Vec::new();
    let mut latin_terms = Vec::new();
    for token in analyze(&splitter, &text) {
        let runs = cjk_runs(&token);
        if !runs.is_empty() {
            cjk_terms.extend(runs.into_iter().map(|r| Node::term(Some(bref.clone()), r, 0)));
        } else {
            latin_terms.push(Node::term(Some(field.clone()), &token, 0));
        }
    }
    let mut pieces = Vec::new();
    if !cjk_terms.is_empty() {
        pieces.push(collapse(is_phrase, cjk_terms));
    }
    pieces.extend(latin_terms);
    if pieces.is_empty() {
        return None;
    }
    Some(collapse(true, pieces))
}

/// `_fuzzy_alternative`
fn fuzzy_alternative(leaf: &Node, splitter: &TextAnalyzer) -> Option<Node> {
    let (field, text, is_phrase) = leaf_parts(leaf)?;
    let words: Vec<String> = analyze(splitter, &text)
        .into_iter()
        .filter(|w| w.chars().count() > 1 && !has_cjk(w))
        .collect();
    if words.is_empty() {
        return None;
    }
    let leaves = words
        .into_iter()
        .map(|w| Node::Fuzzy { field: field.clone(), text: w, distance: 1, prefix: true })
        .collect();
    Some(collapse(is_phrase, leaves))
}

/// `_widen_leaf`; `None` = the leaf itself.
fn widen_leaf(leaf: &Node, fuzzy: bool, negated: &HashSet<u32>, splitter: &TextAnalyzer) -> Option<Node> {
    let field = leaf.field()?;
    if !DEFAULT_SEARCH_FIELDS.contains(&field.name.as_str()) {
        return None;
    }
    let mut alternatives = Vec::new();
    if let Some(c) = cjk_alternative(leaf) {
        alternatives.push(c);
    }
    if fuzzy
        && !negated.contains(&leaf.uid())
        && let Some(near) = fuzzy_alternative(leaf, splitter)
    {
        alternatives.push(Node::Boosted(Box::new(near), 0.1));
    }
    if alternatives.is_empty() {
        return None;
    }
    let mut children = vec![leaf.clone()];
    children.extend(alternatives);
    Some(Node::Or(children))
}

/// `_negated_leaf_ids` (leaf uids instead of id())
fn negated_leaf_ids(node: &Node) -> HashSet<u32> {
    let mut out = HashSet::new();
    let mut stack = vec![(node, false)];
    while let Some((cur, neg)) = stack.pop() {
        match cur {
            Node::Term { uid, .. } | Node::Phrase { uid, .. } => {
                if neg {
                    out.insert(*uid);
                }
            }
            Node::Not(c) => stack.push((c, true)),
            Node::AndNot(p, n) => {
                stack.push((p, neg));
                stack.push((n, true));
            }
            Node::AndMaybe(a, b) | Node::Require(a, b) => {
                stack.push((a, neg));
                stack.push((b, neg));
            }
            Node::Boosted(c, _) => stack.push((c, neg)),
            Node::And(c) | Node::Or(c) => stack.extend(c.iter().map(|x| (x, neg))),
            _ => {}
        }
    }
    out
}

fn field_name(d: &Diagnostic) -> Option<String> {
    d.field.as_ref().map(ToString::to_string)
}

/// `_single_diagnostic_to_error` (its message)
fn single_diagnostic_message(d: &Diagnostic) -> String {
    let fname = field_name(d);
    match d.kind {
        DiagnosticKind::BadDate => format!(
            "Invalid date value {} for field {}.",
            py_repr_opt(d.raw_value.as_deref()),
            py_repr_opt(fname.as_deref())
        ),
        DiagnosticKind::BadNumber => format!(
            "Invalid numeric value {} for field {}.",
            py_repr_opt(d.raw_value.as_deref()),
            py_repr_opt(fname.as_deref())
        ),
        DiagnosticKind::TooDeep => "The search query is nested too deeply.".to_string(),
        DiagnosticKind::PatternOnNumeric | DiagnosticKind::PatternOnBooleanExists | DiagnosticKind::PatternOnSubpath => {
            let label = d.field_kind.map(|k| format!(" ({})", k.name().to_lowercase())).unwrap_or_default();
            format!("Wildcard patterns are not supported for field {}{label}.", py_repr_opt(fname.as_deref()))
        }
        DiagnosticKind::SingleCharBracketRange => {
            let label = match fname.as_deref() {
                Some(f) if !f.is_empty() => format!(" for field {}", py_repr(f)),
                _ => String::new(),
            };
            format!(
                "{} looks like a bracket range{label}, but '[' is not a wildcard character on its own. Combine it with a wildcard, e.g. a trailing '*', or double-quote the value to search it as literal text.",
                py_repr_opt(d.raw_value.as_deref())
            )
        }
        _ => {
            tracing::warn!("Unmapped parse diagnostic {:?}: {}", d.kind, d.message);
            "The search query could not be executed.".to_string()
        }
    }
}

/// `_diagnostics_to_error` (+ search_query_error_messages: one per diagnostic)
fn diagnostics_to_error(diagnostics: &[Diagnostic]) -> SearchError {
    SearchError::Query(diagnostics.iter().map(single_diagnostic_message).collect())
}

/// `_user_facing_emit_message`
fn user_facing_emit_message(d: &Diagnostic) -> String {
    let fname = field_name(d);
    let f = py_repr_opt(fname.as_deref());
    match d.kind {
        DiagnosticKind::ExistsRequiresFast => format!("Existence searches (field:*) are not supported for field {f}."),
        DiagnosticKind::TextRange => format!("Range searches are not supported for field {f}."),
        DiagnosticKind::PatternTooComplex => format!("The wildcard pattern for field {f} is too complex."),
        _ => {
            tracing::warn!("Unmapped emit diagnostic {:?}: {}", d.kind, d.message);
            "The search query could not be executed.".to_string()
        }
    }
}

/// `_map_emit_error`
fn map_emit_error(e: QueryError) -> SearchError {
    let d = &e.diagnostic;
    if d.cause == Cause::Internal {
        return SearchError::Internal(d.message.clone());
    }
    if d.cause == Cause::Misconfigured && d.kind != DiagnosticKind::ExistsRequiresFast {
        tracing::error!(
            "Search index misconfiguration for field {} ({:?}): {}",
            field_name(d).unwrap_or_else(|| "None".into()),
            d.kind,
            d.message
        );
        return SearchError::Internal(d.message.clone());
    }
    SearchError::query(user_facing_emit_message(d))
}

/// `parse_user_query`
pub fn parse_user_query(
    index: &tantivy::Index,
    raw_query: &str,
    cfg: &UserQueryConfig,
) -> Result<Box<dyn Query>, SearchError> {
    let registry = get_field_registry(cfg.language);
    let result = wc::parse(raw_query, &registry, &DEFAULT_SEARCH_FIELDS, &FIELD_BOOSTS, cfg.tz, cfg.now)
        .map_err(SearchError::Internal)?;
    if !result.diagnostics.is_empty() {
        return Err(diagnostics_to_error(&result.diagnostics));
    }
    let fuzzy_on = cfg.fuzzy;
    let cjk = has_cjk(raw_query);
    let emit_registry_owned;
    let emit_registry = if cjk || fuzzy_on {
        emit_registry_owned = get_emit_field_registry(cfg.language);
        &emit_registry_owned
    } else {
        &registry
    };
    let negated = if fuzzy_on { negated_leaf_ids(&result.ast) } else { HashSet::new() };
    // `_FUZZY_WORD_SPLITTER`: the index analyzer minus stemming.
    let splitter = paperless_text_analyzer(None);

    let emit_widened = |fuzzy: bool| -> Result<Box<dyn Query>, QueryError> {
        let hook = |leaf: &Node| widen_leaf(leaf, fuzzy, &negated, &splitter);
        let hook_ref: Option<wc::ast::RewriteLeaf<'_>> = if cjk || fuzzy { Some(&hook) } else { None };
        wc::emit::emit(&result.ast, index, emit_registry, hook_ref)
    };

    if !fuzzy_on {
        return emit_widened(false).map_err(map_emit_error);
    }
    let widened = emit_widened(true).map_err(map_emit_error)?;
    let scored = emit_widened(false).map_err(map_emit_error)?;
    let tiebreak = widened.box_clone();
    Ok(Box::new(BooleanQuery::from(vec![
        (Occur::Must, Box::new(ConstScoreQuery::new(widened, 0.1)) as Box<dyn Query>),
        (Occur::Should, scored),
        (Occur::Should, Box::new(BoostQuery::new(tiebreak, FUZZY_TIEBREAK))),
    ])))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use chrono::{DateTime, TimeZone, Utc};
    use tantivy::collector::DocSetCollector;
    use tantivy::schema::Value;
    use tantivy::{Index, TantivyDocument};

    use super::*;
    use crate::schema::build_schema;
    use crate::testutil::{build_doc, TestDoc};
    use crate::tokenizer::register_tokenizers;

    const CHECKSUM: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

    fn frozen_now() -> DateTime<Utc> {
        // test_documented_syntax.py::FROZEN_NOW (a Monday)
        Utc.with_ymd_and_hms(2026, 6, 15, 12, 0, 0).unwrap()
    }

    fn cfg(fuzzy: bool) -> UserQueryConfig<'static> {
        UserQueryConfig { language: None, fuzzy, tz: chrono_tz::UTC, now: frozen_now() }
    }

    fn docs() -> Vec<TestDoc> {
        let mut d1 = TestDoc::new(1, "Invoice one", "invoice secret contents");
        d1.tags = vec!["bills".into(), "unpaid".into()];
        d1.created = Some((2005, 3, 4));
        d1.asn = Some(100);
        d1.page_count = Some(12);
        d1.checksum = CHECKSUM.into();
        d1.notes = vec![("a note".into(), Some("admin".into()))];
        d1.document_type = Some("Bill".into());
        let mut d2 = TestDoc::new(2, "Invoice two", "invoice ordinary contents");
        d2.tags = vec!["bills".into(), "archived".into()];
        d2.created = Some((2026, 6, 14));
        let mut d3 = TestDoc::new(3, "Phrase", "the quick brown fox jumps");
        d3.created = Some((2026, 6, 15));
        let mut d4 = TestDoc::new(4, "東京都の公共文書", "東京都 report");
        d4.created = Some((2026, 1, 10));
        let mut d5 = TestDoc::new(5, "Kyoto", "京都の観光案内");
        d5.created = Some((2020, 1, 1));
        vec![d1, d2, d3, d4, d5]
    }

    fn ram_index(docs: &[TestDoc], extra: &[(u64, DateTime<Utc>)]) -> Index {
        let index = Index::create_in_ram(build_schema());
        register_tokenizers(&index, None);
        let mut w: tantivy::IndexWriter = index.writer_with_num_threads(1, 15_000_000).unwrap();
        for d in docs {
            w.add_document(build_doc(&index, d)).unwrap();
        }
        let schema = index.schema();
        for (id, added) in extra {
            let mut doc = TantivyDocument::default();
            doc.add_u64(schema.get_field("id").unwrap(), *id);
            doc.add_text(schema.get_field("content").unwrap(), "dated body");
            doc.add_date(
                schema.get_field("added").unwrap(),
                tantivy::DateTime::from_timestamp_secs(added.timestamp()),
            );
            w.add_document(doc).unwrap();
        }
        w.commit().unwrap();
        index
    }

    fn run(index: &Index, q: &str, fuzzy: bool) -> Result<BTreeSet<u64>, SearchError> {
        let query = parse_user_query(index, q, &cfg(fuzzy))?;
        let reader = index.reader().unwrap();
        let searcher = reader.searcher();
        let id = index.schema().get_field("id").unwrap();
        let hits = searcher.search(&*query, &DocSetCollector).unwrap();
        Ok(hits
            .into_iter()
            .map(|a| {
                let d: TantivyDocument = searcher.doc(a).unwrap();
                d.get_first(id).and_then(|v| v.as_u64()).unwrap()
            })
            .collect())
    }

    fn ids(index: &Index, q: &str) -> BTreeSet<u64> {
        match run(index, q, false) {
            Ok(s) => s,
            Err(e) => panic!("{q}: {e:?}"),
        }
    }

    fn set(v: &[u64]) -> BTreeSet<u64> {
        v.iter().copied().collect()
    }

    fn errs(index: &Index, q: &str, fuzzy: bool) -> Vec<String> {
        match run(index, q, fuzzy) {
            Err(SearchError::Query(m)) => m,
            other => panic!("{q}: expected a query error, got {other:?}"),
        }
    }

    fn msg(s: &str) -> Vec<String> {
        vec![s.to_string()]
    }

    #[test]
    fn logical_expressions() {
        // test_documented_syntax.py::TestLogicalExpressions
        let idx = ram_index(&docs(), &[]);
        assert_eq!(ids(&idx, "invoice NOT secret"), set(&[2]));
        assert_eq!(ids(&idx, "invoice -secret"), set(&[1]));
        assert_eq!(ids(&idx, "invoice AND (secret OR ordinary)"), set(&[1, 2]));
        assert_eq!(ids(&idx, "invoice OR fox"), set(&[1, 2, 3]));
        assert_eq!(ids(&idx, "invoice ANDNOT secret"), set(&[2]));
        assert_eq!(ids(&idx, "invoice ANDMAYBE secret"), set(&[1, 2]));
        assert_eq!(ids(&idx, "invoice REQUIRE ordinary"), set(&[2]));
        assert_eq!(ids(&idx, "NOT invoice"), set(&[3, 4, 5]));
        assert_eq!(ids(&idx, "*:*").len(), 5);
        assert_eq!(ids(&idx, "title:invoice^3 fox"), set(&[]));
        assert_eq!(ids(&idx, "type:bill"), set(&[1]));
    }

    #[test]
    fn phrases_and_patterns() {
        let idx = ram_index(&docs(), &[]);
        assert_eq!(ids(&idx, "\"quick brown fox\""), set(&[3]));
        assert_eq!(ids(&idx, "\"brown quick fox\""), set(&[]));
        assert_eq!(ids(&idx, "\"quick fox\"~2"), set(&[3]));
        assert_eq!(ids(&idx, "invoic*"), set(&[1, 2]));
        assert_eq!(ids(&idx, "content:con?ents"), set(&[1, 2]));
        assert_eq!(ids(&idx, "title:INV*"), set(&[1, 2]));
        assert_eq!(ids(&idx, "content:*"), set(&[1, 2, 3, 4, 5]));
    }

    #[test]
    fn tag_comma_list() {
        // test_documented_syntax.py::TestTagCommaList
        let idx = ram_index(&docs(), &[]);
        assert_eq!(ids(&idx, "tag:bills,unpaid"), set(&[1]));
        assert_eq!(ids(&idx, "tag:bills"), set(&[1, 2]));
    }

    #[test]
    fn metadata_fields() {
        // test_documented_syntax.py::TestArchiveMetadataFields
        let idx = ram_index(&docs(), &[]);
        let full = format!("checksum:{CHECKSUM}");
        for q in [
            "asn:100",
            "asn:[50 to 150]",
            "page_count:12",
            "page_count:[10 to 20]",
            "num_notes:1",
            "num_notes:[1 to 5]",
            full.as_str(),
            "checksum:9f86d081*",
            "checksum:9F86D081*",
            "notes:note",
            "notes.user:admin",
            "asn:*",
        ] {
            assert_eq!(ids(&idx, q), set(&[1]), "{q}");
        }
        assert_eq!(ids(&idx, "checksum:9f86d081"), set(&[]));
        assert_eq!(ids(&idx, &format!("checksum:{}", CHECKSUM.to_uppercase())), set(&[]));
    }

    #[test]
    fn date_forms() {
        // test_documented_syntax.py::TestDocumentedDateForms (on `created`)
        let idx = ram_index(&docs(), &[]);
        assert_eq!(ids(&idx, "created:2005-03-04"), set(&[1]));
        assert_eq!(ids(&idx, "created:2005-03"), set(&[1]));
        assert_eq!(ids(&idx, "created:[2005-01-01 to 2005-12-31]"), set(&[1]));
        assert_eq!(ids(&idx, "created:[2005 to 2009]"), set(&[1]));
        assert_eq!(ids(&idx, "created:today"), set(&[3]));
        assert_eq!(ids(&idx, "created:yesterday"), set(&[2]));
        assert_eq!(ids(&idx, "created:january"), set(&[4]));
        assert_eq!(ids(&idx, "created:\"this month\""), set(&[2, 3]));
        assert_eq!(ids(&idx, "created:this month"), set(&[2, 3]));
        assert_eq!(ids(&idx, "created:['2005-03-04' to 2005-03-05]"), set(&[1]));
        assert_eq!(ids(&idx, "created:[2005-03-04T09:00:00Z to 2005-03-04T17:00:00Z]"), set(&[1]));
        assert_eq!(ids(&idx, "created:[-1 week to now]"), set(&[2, 3]));
        assert_eq!(ids(&idx, "created:\"previous year\""), set(&[]));
        assert_eq!(ids(&idx, "created:[2020 TO]"), set(&[2, 3, 4, 5]));
    }

    #[test]
    fn datetime_forms() {
        let stamps = [
            (10, Utc.with_ymd_and_hms(2026, 6, 15, 9, 0, 0).unwrap()),
            (11, Utc.with_ymd_and_hms(2026, 6, 14, 9, 0, 0).unwrap()),
            (12, Utc.with_ymd_and_hms(2026, 6, 22, 10, 0, 0).unwrap()),
            (13, Utc.with_ymd_and_hms(2026, 6, 8, 10, 0, 0).unwrap()),
            (14, Utc.with_ymd_and_hms(2005, 3, 4, 15, 30, 0).unwrap()),
        ];
        let idx = ram_index(&[], &stamps);
        assert_eq!(ids(&idx, "added:today"), set(&[10]));
        assert_eq!(ids(&idx, "added:\"next monday\""), set(&[12]));
        assert_eq!(ids(&idx, "added:\"last monday\""), set(&[13]));
        assert_eq!(ids(&idx, "added:\"2005-03-04T15:30:00Z\""), set(&[14]));
        assert_eq!(ids(&idx, "added:[2005-03-04T09:00:00Z to 2005-03-04T17:00:00Z]"), set(&[14]));
        for q in ["added:now", "added:noon", "added:midnight", "added:\"now\"", "added:\"-1 week\""] {
            assert_eq!(ids(&idx, q), set(&[]), "{q}");
        }
        assert_eq!(ids(&idx, "added:['-1 week' to now]"), set(&[10, 11]));
        assert_eq!(
            errs(&idx, "added:2005-03-04T15:30:00Z", false),
            msg("Invalid date value '2005-03-04T15:30:00Z' for field 'added'.")
        );
    }

    #[test]
    fn cjk_widening() {
        let idx = ram_index(&docs(), &[]);
        // 東京都 is one unspaced token; the bigram field finds it, and
        // multitoken=AND keeps the 京都-only doc 5 out.
        assert_eq!(ids(&idx, "東京都"), set(&[4]));
        assert_eq!(ids(&idx, "京都"), set(&[4, 5]));
        assert_eq!(ids(&idx, "観光"), set(&[5]));
        assert_eq!(ids(&idx, "京都 NOT 東京"), set(&[5]));
        assert_eq!(ids(&idx, "content:東京都 report"), set(&[4]));
    }

    #[test]
    fn fuzzy_blend() {
        let idx = ram_index(&docs(), &[]);
        assert_eq!(run(&idx, "invoce", true).unwrap(), set(&[1, 2]));
        assert_eq!(run(&idx, "invoce", false).unwrap(), set(&[]));
        assert_eq!(run(&idx, "secrt", true).unwrap(), set(&[1]));
        // A negated leaf gets no fuzzy side (NOT tax must not exclude taxi).
        assert_eq!(run(&idx, "invoice NOT secrt", true).unwrap(), set(&[1, 2]));
        for q in ["invoice", "created:today", "created:[2005 to 2009]", "added:\"previous month\"", "title:202[0-1]*"] {
            assert!(run(&idx, q, true).is_ok(), "{q}");
        }
        assert_eq!(
            errs(&idx, "invoice NOT title:[a to b]", true),
            msg("Range searches are not supported for field 'title'.")
        );
    }

    #[test]
    fn user_errors() {
        // test_query.py / test_error_routing.py expectations
        let idx = ram_index(&docs(), &[]);
        assert_eq!(errs(&idx, "created:202023", false), msg("Invalid date value '202023' for field 'created'."));
        assert_eq!(errs(&idx, "asn:notanumber", false), msg("Invalid numeric value 'notanumber' for field 'asn'."));
        assert_eq!(
            errs(&idx, "created:notadate AND asn:notanumber", false),
            vec![
                "Invalid date value 'notadate' for field 'created'.".to_string(),
                "Invalid numeric value 'notanumber' for field 'asn'.".to_string()
            ]
        );
        assert_eq!(
            errs(&idx, "notes.user:*", false),
            msg("Existence searches (field:*) are not supported for field 'notes.user'.")
        );
        assert_eq!(errs(&idx, "title:[a to b]", false), msg("Range searches are not supported for field 'title'."));
        assert_eq!(errs(&idx, "asn:1*", false), msg("Wildcard patterns are not supported for field 'asn' (u64)."));
        assert_eq!(
            errs(&idx, "notes.user:adm*", false),
            msg("Wildcard patterns are not supported for field 'notes.user' (json).")
        );
        assert_eq!(
            errs(&idx, "title:200[1-9]", false),
            msg("'200[1-9]' looks like a bracket range for field 'title', but '[' is not a wildcard character on its own. Combine it with a wildcard, e.g. a trailing '*', or double-quote the value to search it as literal text.")
        );
        assert_eq!(
            errs(&idx, "created:december 2019", false),
            msg("Invalid date value 'december 2019' for field 'created'.")
        );
        assert_eq!(errs(&idx, "type:x NOT asn:-5", false), msg("Invalid numeric value '-5' for field 'asn'."));
        let deep = format!("{}a{}", "(".repeat(300), ")".repeat(300));
        assert_eq!(errs(&idx, &deep, false), msg("The search query is nested too deeply."));
        // An unfielded bracket range fans out over every default field.
        assert_eq!(errs(&idx, "200[1-9]", false).len(), 5);
    }

    #[test]
    fn natural_language_queries_do_not_raise() {
        // test_query.py::test_spaced_dash_queries_do_not_raise and friends
        let idx = ram_index(&docs(), &[]);
        for q in [
            "h52.1 - kurzsichtigkeit",
            "H52.1 - asd",
            "h52.1 -",
            ". -",
            "h52. -",
            ".12 -",
            "h52.1 - ku",
            "",
            "AND",
            "NOT NOT alpha",
            "(((",
            ")))",
            "'unterminated",
            "\"unterminated",
            "title: foo",
            "foo^",
            "foo ^2",
            "[a TO",
            "a:b:c",
            "*",
            "?",
            "x\u{55e}",
            "ä ö ü ß",
        ] {
            assert!(run(&idx, q, false).is_ok(), "{q}");
            assert!(run(&idx, q, true).is_ok(), "{q} (fuzzy)");
        }
        assert_eq!(ids(&idx, ""), set(&[]));
    }
}
