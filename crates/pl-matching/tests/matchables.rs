//! Port of upstream src/documents/tests/test_matchables.py@v3.4.2 (TestMatching,
//! TestCaseSensitiveMatching). The DB-backed tests (latest version content, consumption
//! signal) are expressed via `MatchDocument::effective_content` / `match_*` helpers.

use pl_matching::matching::{
    Classifier, MATCH_ALL, MATCH_ANY, MATCH_AUTO, MATCH_FUZZY, MATCH_LITERAL, MATCH_NONE, MATCH_REGEX, MatchDocument,
    Matchable, match_correspondents, match_tags, matches, split_match_python,
};

const CLASSES: [&str; 3] = ["Tag", "Correspondent", "DocumentType"];

fn assert_matches(match_text: &str, algo: i64, should: &[&str], no: &[&str], case_sensitive: bool) {
    for class in CLASSES {
        let mut m = Matchable::new(match_text, algo, !case_sensitive);
        m.class_name = class.into();
        for c in should {
            assert!(matches(&m, &MatchDocument::from_content(c)).unwrap(), "{class}: \"{match_text}\" should match \"{c}\"");
        }
        for c in no {
            assert!(!matches(&m, &MatchDocument::from_content(c)).unwrap(), "{class}: \"{match_text}\" should not match \"{c}\"");
        }
    }
}

#[test]
fn test_root_uses_latest_version_content() {
    // effective content = latest version's content (resolved by the caller)
    let doc = MatchDocument { effective_content: Some("latest version contains keyword".into()), display: "root".into() };
    assert!(matches(&Matchable::new("keyword", MATCH_ANY, true), &doc).unwrap());
    let doc = MatchDocument { effective_content: Some("latest version without token".into()), display: "root".into() };
    assert!(!matches(&Matchable::new("keyword", MATCH_ANY, true), &doc).unwrap());
}

#[test]
fn test_match_none() {
    assert_matches("", MATCH_NONE, &[], &["no", "match"], false);
}

#[test]
fn test_match_all() {
    assert_matches(
        "alpha charlie gamma",
        MATCH_ALL,
        &["I have alpha, charlie, and gamma in me"],
        &[
            "I have alpha in me",
            "I have charlie in me",
            "I have gamma in me",
            "I have alpha and charlie in me",
            "I have alphas, charlie, and gamma in me",
            "I have alphas in me",
            "I have bravo in me",
        ],
        false,
    );
    assert_matches(
        "12 34 56",
        MATCH_ALL,
        &["I have 12 34, and 56 in me"],
        &[
            "I have 12 in me",
            "I have 34 in me",
            "I have 56 in me",
            "I have 12 and 34 in me",
            "I have 120, 34, and 56 in me",
            "I have 123456 in me",
            "I have 01234567 in me",
        ],
        false,
    );
    assert_matches(
        r#"brown fox "lazy dogs""#,
        MATCH_ALL,
        &["the quick brown fox jumped over the lazy dogs", "the quick brown fox jumped over the lazy  dogs"],
        &[
            "the quick fox jumped over the lazy dogs",
            "the quick brown wolf jumped over the lazy dogs",
            "the quick brown fox jumped over the fat dogs",
            "the quick brown fox jumped over the lazy... dogs",
        ],
        false,
    );
}

#[test]
fn test_match_any() {
    assert_matches(
        "alpha charlie gamma",
        MATCH_ANY,
        &[
            "I have alpha in me",
            "I have charlie in me",
            "I have gamma in me",
            "I have alpha, charlie, and gamma in me",
            "I have alpha and charlie in me",
        ],
        &["I have alphas in me", "I have bravo in me"],
        false,
    );
    assert_matches(
        "12 34 56",
        MATCH_ANY,
        &[
            "I have 12 in me",
            "I have 34 in me",
            "I have 56 in me",
            "I have 12 and 34 in me",
            "I have 12, 34, and 56 in me",
            "I have 120, 34, and 56 in me",
        ],
        &["I have 123456 in me", "I have 01234567 in me"],
        false,
    );
    assert_matches(
        r#""brown fox" " lazy  dogs ""#,
        MATCH_ANY,
        &["the quick brown fox", "jumped over the lazy  dogs."],
        &["the lazy fox jumped over the brown dogs"],
        false,
    );
}

#[test]
fn test_match_literal() {
    assert_matches(
        "alpha charlie gamma",
        MATCH_LITERAL,
        &["I have 'alpha charlie gamma' in me"],
        &[
            "I have alpha in me",
            "I have charlie in me",
            "I have gamma in me",
            "I have alpha and charlie in me",
            "I have alpha, charlie, and gamma in me",
            "I have alphas, charlie, and gamma in me",
            "I have alphas in me",
            "I have bravo in me",
        ],
        false,
    );
    assert_matches(
        "12 34 56",
        MATCH_LITERAL,
        &["I have 12 34 56 in me"],
        &[
            "I have 12 in me",
            "I have 34 in me",
            "I have 56 in me",
            "I have 12 and 34 in me",
            "I have 12 34, and 56 in me",
            "I have 120, 34, and 560 in me",
            "I have 120, 340, and 560 in me",
            "I have 123456 in me",
            "I have 01234567 in me",
        ],
        false,
    );
}

#[test]
fn test_match_regex() {
    assert_matches(
        r"alpha\w+gamma",
        MATCH_REGEX,
        &["I have alpha_and_gamma in me", "I have alphas_and_gamma in me"],
        &[
            "I have alpha in me",
            "I have gamma in me",
            "I have alpha and charlie in me",
            "I have alpha,and,gamma in me",
            "I have alpha and gamma in me",
            "I have alpha, charlie, and gamma in me",
            "I have alphas, charlie, and gamma in me",
            "I have alphas in me",
        ],
        false,
    );
}

#[test]
fn test_invalid_regex() {
    assert_matches("[", MATCH_REGEX, &[], &["Don't match this"], false);
}

#[test]
fn test_match_regex_timeout_returns_false() {
    // upstream: times out after 0.1 s -> False. Rust: linear-time engine, no match -> False.
    let m = Matchable::new(r"(a+)+$", MATCH_REGEX, true);
    let content = format!("{}X", "a".repeat(5000));
    assert!(!matches(&m, &MatchDocument::from_content(&content)).unwrap());
    // a pattern that needs backtracking hits the backtrack limit -> False as well
    let m = Matchable::new(r"(a+)+(?=X)b", MATCH_REGEX, true);
    assert!(!matches(&m, &MatchDocument::from_content(&content)).unwrap());
}

#[test]
fn test_match_fuzzy() {
    assert_matches(
        "Springfield, Miss.",
        MATCH_FUZZY,
        &[
            "1220 Main Street, Springf eld, Miss.",
            "1220 Main Street, Spring field, Miss.",
            "1220 Main Street, Springfeld, Miss.",
            "1220 Main Street Springfield Miss",
        ],
        &["1220 Main Street, Springfield, Mich."],
        false,
    );
}

// --- TestCaseSensitiveMatching -------------------------------------------------

#[test]
fn test_case_sensitive_match_all() {
    assert_matches(
        "alpha charlie gamma",
        MATCH_ALL,
        &["I have alpha, charlie, and gamma in me", "I have gamma, charlie, and alpha in me"],
        &[
            "I have Alpha, charlie, and gamma in me",
            "I have gamma, Charlie, and alpha in me",
            "I have alpha, charlie, and Gamma in me",
            "I have gamma, charlie, and ALPHA in me",
        ],
        true,
    );
    assert_matches(
        "Alpha charlie Gamma",
        MATCH_ALL,
        &["I have Alpha, charlie, and Gamma in me", "I have Gamma, charlie, and Alpha in me"],
        &[
            "I have Alpha, charlie, and gamma in me",
            "I have gamma, charlie, and alpha in me",
            "I have alpha, charlie, and Gamma in me",
            "I have Gamma, Charlie, and ALPHA in me",
        ],
        true,
    );
    assert_matches(
        r#"brown fox "lazy dogs""#,
        MATCH_ALL,
        &["the quick brown fox jumped over the lazy dogs", "the quick brown fox jumped over the lazy  dogs"],
        &[
            "the quick Brown fox jumped over the lazy dogs",
            "the quick brown Fox jumped over the lazy  dogs",
            "the quick brown fox jumped over the Lazy dogs",
            "the quick brown fox jumped over the lazy  Dogs",
        ],
        true,
    );
}

#[test]
fn test_case_sensitive_match_any() {
    assert_matches(
        "alpha charlie gamma",
        MATCH_ANY,
        &[
            "I have alpha in me",
            "I have charlie in me",
            "I have gamma in me",
            "I have alpha, charlie, and gamma in me",
            "I have alpha and charlie in me",
        ],
        &[
            "I have Alpha in me",
            "I have chaRLie in me",
            "I have gamMA in me",
            "I have aLPha, cHArlie, and gAMma in me",
            "I have AlphA and CharlIe in me",
        ],
        true,
    );
    assert_matches(
        "Alpha Charlie Gamma",
        MATCH_ANY,
        &[
            "I have Alpha in me",
            "I have Charlie in me",
            "I have Gamma in me",
            "I have Alpha, Charlie, and Gamma in me",
            "I have Alpha and Charlie in me",
        ],
        &[
            "I have alpha in me",
            "I have ChaRLie in me",
            "I have GamMA in me",
            "I have ALPha, CHArlie, and GAMma in me",
            "I have AlphA and CharlIe in me",
        ],
        true,
    );
    assert_matches(
        r#""brown fox" " lazy  dogs ""#,
        MATCH_ANY,
        &["the quick brown fox", "jumped over the lazy  dogs."],
        &["the quick Brown fox", "jumped over the lazy  Dogs."],
        true,
    );
}

#[test]
fn test_case_sensitive_match_literal() {
    assert_matches(
        "alpha charlie gamma",
        MATCH_LITERAL,
        &["I have 'alpha charlie gamma' in me"],
        &[
            "I have 'Alpha charlie gamma' in me",
            "I have 'alpha Charlie gamma' in me",
            "I have 'alpha charlie Gamma' in me",
            "I have 'Alpha Charlie Gamma' in me",
        ],
        true,
    );
    assert_matches(
        "Alpha Charlie Gamma",
        MATCH_LITERAL,
        &["I have 'Alpha Charlie Gamma' in me"],
        &[
            "I have 'Alpha charlie gamma' in me",
            "I have 'alpha Charlie gamma' in me",
            "I have 'alpha charlie Gamma' in me",
            "I have 'alpha charlie gamma' in me",
        ],
        true,
    );
}

#[test]
fn test_case_sensitive_match_regex() {
    assert_matches(
        r"alpha\w+gamma",
        MATCH_REGEX,
        &["I have alpha_and_gamma in me", "I have alphas_and_gamma in me"],
        &["I have Alpha_and_Gamma in me", "I have alpHAs_and_gaMMa in me"],
        true,
    );
    assert_matches(
        r"Alpha\w+gamma",
        MATCH_REGEX,
        &["I have Alpha_and_gamma in me", "I have Alphas_and_gamma in me"],
        &["I have Alpha_and_Gamma in me", "I have alphas_and_gamma in me"],
        true,
    );
}

// --- consumption signal equivalents (match_tags / match_correspondents) --------------

fn doc_with_keyword() -> MatchDocument {
    MatchDocument::from_content("I contain the keyword.")
}

fn named(id: i64, class: &str, text: &str, algo: i64) -> Matchable {
    let mut m = Matchable::new(text, algo, true);
    m.id = id;
    m.class_name = class.into();
    m.name = format!("{class}{id}");
    m
}

#[test]
fn test_tag_applied_any() {
    let tags = vec![named(1, "Tag", "keyword", MATCH_ANY)];
    let got = match_tags(&doc_with_keyword(), &tags, None).unwrap();
    assert_eq!(got.iter().map(|t| t.id).collect::<Vec<_>>(), vec![1]);
}

#[test]
fn test_tag_not_applied() {
    let tags = vec![named(1, "Tag", "no-match", MATCH_ANY)];
    assert!(match_tags(&doc_with_keyword(), &tags, None).unwrap().is_empty());
}

#[test]
fn test_correspondent_applied() {
    let cs = vec![named(1, "Correspondent", "keyword", MATCH_ANY)];
    assert_eq!(match_correspondents(&doc_with_keyword(), &cs, None).unwrap().len(), 1);
}

#[test]
fn test_correspondent_not_applied() {
    let cs = vec![named(1, "Correspondent", "no-match", MATCH_ANY)];
    assert!(match_correspondents(&doc_with_keyword(), &cs, None).unwrap().is_empty());
}

struct FakeClassifier;

impl Classifier for FakeClassifier {
    fn predict_correspondent(&self, _: &str) -> Option<i64> {
        Some(2)
    }
    fn predict_document_type(&self, _: &str) -> Option<i64> {
        None
    }
    fn predict_tags(&self, _: &str) -> Vec<i64> {
        vec![3]
    }
    fn predict_storage_path(&self, _: &str) -> Option<i64> {
        None
    }
}

#[test]
fn auto_matching_uses_classifier_only_for_auto_objects() {
    let cs = vec![named(1, "Correspondent", "x", MATCH_AUTO), named(2, "Correspondent", "x", MATCH_AUTO), named(3, "Correspondent", "x", MATCH_NONE)];
    let got = match_correspondents(&doc_with_keyword(), &cs, Some(&FakeClassifier)).unwrap();
    assert_eq!(got.iter().map(|t| t.id).collect::<Vec<_>>(), vec![2]);
    let tags = vec![named(3, "Tag", "x", MATCH_ANY), named(3, "Tag", "x", MATCH_AUTO)];
    assert_eq!(match_tags(&doc_with_keyword(), &tags, Some(&FakeClassifier)).unwrap().len(), 1);
}

#[test]
fn unsupported_algorithm_errors() {
    assert!(matches(&Matchable::new("x", 99, true), &doc_with_keyword()).is_err());
}

#[test]
fn split_match_docstring_example() {
    assert_eq!(
        split_match_python(r#"  some random  words "with   quotes  " and   spaces"#),
        vec!["some", "random", "words", r"with\s+quotes", "and", "spaces"]
    );
}

#[test]
fn fuzzy_edge_cases() {
    // only punctuation: pattern becomes "" -> partial_ratio("", "") == 100 for empty content
    let m = Matchable::new("?!", MATCH_FUZZY, true);
    assert!(matches(&m, &MatchDocument::from_content("")).unwrap());
    assert!(matches(&m, &MatchDocument::from_content("...")).unwrap());
    assert!(!matches(&m, &MatchDocument::from_content("abc")).unwrap());
    // whitespace-only match is rejected before any algorithm
    assert!(!matches(&Matchable::new("   ", MATCH_FUZZY, true), &MatchDocument::from_content("")).unwrap());
}
