//! Taxonomy matching (PaperRust extension): finds, for a name, the existing
//! identical or similar correspondents, tags, document types, storage paths and custom
//! fields – with score and explanation – so that AI paths never blindly create duplicates.
//!
//! Two modes ([`MatchMode`]):
//! * [`MatchMode::Upstream`]: exactly `paperless_ai/matching.py@v3.4.2` (`_normalize` +
//!   `difflib.get_close_matches(cutoff=0.8)`), see [`crate::compat`].
//! * [`MatchMode::Extended`] (default for MCP and automation): strong normalization
//!   ([`crate::normalize`]), Jaro-Winkler/Levenshtein/token set, aliases, number incompatibility.
//!
//! Permissions: matching sees only objects that the caller may see. The Postgres integration
//! filters via `pl_authz::objects::permitted_object_ids` (like upstream `permitted_object_ids`
//! in `_match_by_name`); for in-memory operation (tests) [`Principal::can_view`] reproduces the same
//! logic.

use serde::{Deserialize, Serialize};

use crate::compat;
use crate::difflib;
use crate::normalize::{NameKind, Normalized, normalize_with};
use crate::similarity::{jaro_winkler, levenshtein_similarity, token_containment, token_set_ratio};

/// Kind of a taxonomy object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaxonomyKind {
    Correspondent,
    #[default]
    Tag,
    DocumentType,
    StoragePath,
    CustomField,
}

impl TaxonomyKind {
    pub const ALL: [TaxonomyKind; 5] =
        [TaxonomyKind::Correspondent, TaxonomyKind::Tag, TaxonomyKind::DocumentType, TaxonomyKind::StoragePath, TaxonomyKind::CustomField];

    /// Value in the API (`correspondent`, `tag`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            TaxonomyKind::Correspondent => "correspondent",
            TaxonomyKind::Tag => "tag",
            TaxonomyKind::DocumentType => "document_type",
            TaxonomyKind::StoragePath => "storage_path",
            TaxonomyKind::CustomField => "custom_field",
        }
    }

    /// Parses `correspondent`/`correspondents`/`tag`/`tags`/… .
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        let s = s.strip_suffix('s').unwrap_or(&s).to_string();
        Some(match s.as_str() {
            "correspondent" => TaxonomyKind::Correspondent,
            "tag" => TaxonomyKind::Tag,
            "document_type" | "documenttype" | "type" => TaxonomyKind::DocumentType,
            "storage_path" | "storagepath" | "path" => TaxonomyKind::StoragePath,
            "custom_field" | "customfield" | "field" => TaxonomyKind::CustomField,
            _ => return None,
        })
    }

    /// Django-Tabelle.
    pub fn table(self) -> &'static str {
        match self {
            TaxonomyKind::Correspondent => "documents_correspondent",
            TaxonomyKind::Tag => "documents_tag",
            TaxonomyKind::DocumentType => "documents_documenttype",
            TaxonomyKind::StoragePath => "documents_storagepath",
            TaxonomyKind::CustomField => "documents_customfield",
        }
    }

    /// `_meta.model_name` (Rechte-Codename `view_<model>`).
    pub fn model_name(self) -> &'static str {
        match self {
            TaxonomyKind::Correspondent => "correspondent",
            TaxonomyKind::Tag => "tag",
            TaxonomyKind::DocumentType => "documenttype",
            TaxonomyKind::StoragePath => "storagepath",
            TaxonomyKind::CustomField => "customfield",
        }
    }

    /// Which word lists the normalization uses.
    pub fn name_kind(self) -> NameKind {
        match self {
            TaxonomyKind::Correspondent => NameKind::Organisation,
            TaxonomyKind::StoragePath => NameKind::Path,
            _ => NameKind::Label,
        }
    }
}

/// An existing object (only fields that matching needs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaxonomyEntry {
    pub kind: TaxonomyKind,
    pub id: i64,
    pub name: String,
    /// `owner_id` (None = without owner, visible to everyone).
    pub owner_id: Option<i64>,
    /// Storage path: template `path`; custom field: `data_type`; otherwise None.
    pub extra: Option<String>,
    /// Number of documents (if known; display only, no logic).
    pub document_count: Option<i64>,
}

/// Confirmed alias (`paperrust_taxonomy_alias`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alias {
    pub kind: TaxonomyKind,
    pub target_id: i64,
    pub alias: String,
    /// Normalized key (extended mode, accent folding).
    pub alias_key: String,
    pub owner_id: Option<i64>,
}

/// Who is querying – for visibility (like `permitted_object_ids`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Principal {
    /// None = no user (workflow for a document without owner): only objects without owner.
    pub user_id: Option<i64>,
    pub is_active: bool,
    pub is_superuser: bool,
    pub group_ids: Vec<i64>,
}

/// Object permission (guardian) for in-memory operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub kind: TaxonomyKind,
    pub object_id: i64,
    pub user_id: Option<i64>,
    pub group_id: Option<i64>,
    /// bare codename, z. B. `view_tag`, `change_tag`.
    pub codename: String,
}

impl Principal {
    /// upstream `permitted_object_ids(user, model, perm)` for a single object.
    pub fn can(&self, entry: &TaxonomyEntry, action: &str, grants: &[Grant]) -> bool {
        let Some(uid) = self.user_id else { return entry.owner_id.is_none() };
        if !self.is_active {
            return false;
        }
        if self.is_superuser {
            return true;
        }
        if entry.owner_id.is_none() || entry.owner_id == Some(uid) {
            return true;
        }
        let codename = format!("{action}_{}", entry.kind.model_name());
        grants.iter().any(|g| {
            g.kind == entry.kind
                && g.object_id == entry.id
                && g.codename == codename
                && (g.user_id == Some(uid) || g.group_id.is_some_and(|gid| self.group_ids.contains(&gid)))
        })
    }

    pub fn can_view(&self, entry: &TaxonomyEntry, grants: &[Grant]) -> bool {
        self.can(entry, "view", grants)
    }
}

/// Matching mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// exactly like upstream `matching.py`.
    Upstream,
    /// PaperRust extension.
    #[default]
    Extended,
}

/// Options.
#[derive(Debug, Clone, Copy)]
pub struct MatchOptions {
    pub mode: MatchMode,
    /// From this score a candidate counts as "identical or similar" (creation is refused).
    pub threshold: f64,
    /// Candidates below this score are not reported.
    pub report_floor: f64,
    pub limit: usize,
}

impl Default for MatchOptions {
    fn default() -> Self {
        Self { mode: MatchMode::Extended, threshold: compat::MATCH_THRESHOLD, report_floor: 0.6, limit: 10 }
    }
}

/// A reason for a score (machine-readable + text).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reason {
    pub code: String,
    pub detail: String,
}

fn reason(code: &str, detail: impl Into<String>) -> Reason {
    Reason { code: code.into(), detail: detail.into() }
}

/// A candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub kind: TaxonomyKind,
    pub id: i64,
    pub name: String,
    /// 0..1, rounded to 4 places.
    pub score: f64,
    /// Above the threshold → counts as existing ("do not create anew").
    pub above_threshold: bool,
    /// Would upstream `matching.py` also assign this name to this object?
    pub upstream_match: bool,
    pub reasons: Vec<Reason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extra: Option<String>,
}

struct Variants {
    accent: Normalized,
    german: Normalized,
    py: String,
}

fn variants(s: &str, kind: NameKind) -> Variants {
    Variants { accent: normalize_with(s, kind, false), german: normalize_with(s, kind, true), py: compat::py_normalize(s) }
}

fn round4(x: f64) -> f64 {
    (x * 10000.0).round() / 10000.0
}

/// Compares two names in extended mode. Returns score and reasons.
pub fn score_names(query: &str, candidate: &str, kind: NameKind) -> (f64, Vec<Reason>) {
    let q = variants(query, kind);
    let c = variants(candidate, kind);
    score_variants(&q, &c, query, candidate)
}

fn score_variants(q: &Variants, c: &Variants, query: &str, candidate: &str) -> (f64, Vec<Reason>) {
    if query == candidate {
        return (1.0, vec![reason("exact", "identischer Name")]);
    }
    if query.to_lowercase() == candidate.to_lowercase() {
        return (1.0, vec![reason("case_insensitive", "gleich bis auf Groß-/Kleinschreibung")]);
    }
    if !q.py.is_empty() && q.py == c.py {
        return (0.99, vec![reason("upstream_normalized_equal", "gleich nach upstream-Normalisierung (Satzzeichen)")]);
    }
    let mut best = 0.0f64;
    let mut reasons: Vec<Reason> = Vec::new();
    for (qa, ca, label) in [(&q.accent, &c.accent, "akzentfrei"), (&q.german, &c.german, "Umlaut-Umschrift")] {
        if qa.key.is_empty() || ca.key.is_empty() {
            continue;
        }
        let mut r: Vec<Reason> = Vec::new();
        let mut s;
        if qa.key == ca.key {
            s = 0.97;
            let mut detail = format!("gleicher Kern \"{}\" ({label})", qa.key);
            let mut removed: Vec<String> = qa.removed_legal.iter().chain(ca.removed_legal.iter()).cloned().collect();
            removed.extend(qa.removed_region.iter().chain(ca.removed_region.iter()).cloned());
            removed.sort();
            removed.dedup();
            if !removed.is_empty() {
                detail.push_str(&format!("; ignoriert: {}", removed.join(", ")));
            }
            r.push(reason("normalized_equal", detail));
        } else if qa.compact == ca.compact {
            s = 0.95;
            r.push(reason("compact_equal", format!("gleich ohne Leerzeichen \"{}\"", qa.compact)));
        } else if qa.sorted_key == ca.sorted_key {
            s = 0.94;
            r.push(reason("token_order", "gleiche Wörter, andere Reihenfolge"));
        } else {
            let jw = jaro_winkler(&qa.key, &ca.key);
            let lev = levenshtein_similarity(&qa.key, &ca.key);
            let dl = difflib::ratio(&ca.key, &qa.key);
            let ts = token_set_ratio(&qa.tokens, &ca.tokens);
            let cont = token_containment(&qa.tokens, &ca.tokens);
            let edit = 0.5 * jw + 0.5 * lev;
            s = edit.max(dl).max(ts * 0.95);
            r.push(reason("fuzzy", format!("Jaro-Winkler {jw:.2}, Levenshtein {lev:.2}, difflib {dl:.2}, Token-Set {ts:.2} ({label})")));
            let short_tokens = qa.tokens.len().min(ca.tokens.len());
            if cont >= 1.0 && short_tokens >= 1 {
                let longest_short = if qa.tokens.len() <= ca.tokens.len() { &qa.tokens } else { &ca.tokens };
                if longest_short.iter().any(|t| t.chars().count() >= 4) && s < 0.9 {
                    s = 0.9;
                    r.push(reason("contains", "alle Wörter des kürzeren Namens sind im längeren enthalten"));
                }
            }
        }
        // Number incompatibility: "Steuern 2024" ≠ "Steuern 2025", "Konto 1" ≠ "Konto 2".
        if qa.numbers != ca.numbers && !(qa.numbers.is_empty() && ca.numbers.is_empty()) && s > 0.75 {
            s = 0.75;
            r.push(reason("number_mismatch", format!("unterschiedliche Zahlen ({:?} / {:?})", qa.numbers, ca.numbers)));
        }
        if s > best {
            best = s;
            reasons = r;
        }
    }
    (best, reasons)
}

/// Would upstream `_match_names_to_queryset([query], [candidate])` match?
pub fn upstream_pair_match(query: &str, candidate: &str) -> bool {
    let q = compat::py_normalize(query);
    let c = compat::py_normalize(candidate);
    if query.is_empty() {
        return false;
    }
    q == c || !difflib::get_close_matches(&q, &[c.as_str()], 1, compat::MATCH_THRESHOLD).is_empty()
}

/// Finds candidates for `query` among the (already permission-filtered!) `entries` of a kind.
/// `aliases` are the visible aliases of this kind.
pub fn find_similar(kind: TaxonomyKind, query: &str, entries: &[TaxonomyEntry], aliases: &[Alias], opts: &MatchOptions) -> Vec<Candidate> {
    let nk = kind.name_kind();
    let mut out: Vec<Candidate> = Vec::new();
    match opts.mode {
        MatchMode::Upstream => {
            let pool: Vec<(i64, String)> = entries.iter().filter(|e| e.kind == kind).map(|e| (e.id, e.name.clone())).collect();
            for (id, name) in compat::match_names_to_pool(&[Some(query)], &pool) {
                let extra = entries.iter().find(|e| e.id == id && e.kind == kind).and_then(|e| e.extra.clone());
                let score = round4(difflib::ratio(&compat::py_normalize(&name), &compat::py_normalize(query)));
                out.push(Candidate {
                    kind,
                    id,
                    name,
                    score,
                    above_threshold: true,
                    upstream_match: true,
                    reasons: vec![reason("upstream", "upstream matching.py (_normalize + difflib, Schwelle 0.8)")],
                    extra,
                });
            }
            return out;
        }
        MatchMode::Extended => {}
    }
    let q = variants(query, nk);
    let q_alias_key = normalize_with(query, nk, false).key;
    for e in entries.iter().filter(|e| e.kind == kind) {
        let c = variants(&e.name, nk);
        let (mut score, mut reasons) = score_variants(&q, &c, query, &e.name);
        // Storage path: also compare against the path template.
        if kind == TaxonomyKind::StoragePath
            && let Some(path) = &e.extra
        {
            let (ps, pr) = score_names(query, path, NameKind::Path);
            if ps > score {
                score = ps;
                reasons = pr;
                reasons.push(reason("path", format!("Vergleich mit Pfadvorlage \"{path}\"")));
            }
        }
        for a in aliases.iter().filter(|a| a.kind == kind && a.target_id == e.id) {
            if !q_alias_key.is_empty() && a.alias_key == q_alias_key && score < 1.0 {
                score = 1.0;
                reasons = vec![reason("alias", format!("bestätigter Alias \"{}\"", a.alias))];
            }
        }
        if score >= opts.report_floor {
            out.push(Candidate {
                kind,
                id: e.id,
                name: e.name.clone(),
                score: round4(score),
                above_threshold: score >= opts.threshold,
                upstream_match: upstream_pair_match(query, &e.name),
                reasons,
                extra: e.extra.clone(),
            });
        }
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.name.cmp(&b.name)).then(a.id.cmp(&b.id)));
    out.truncate(opts.limit);
    out
}

/// Result of the duplicate check before creation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DuplicateCheck {
    /// Is there a candidate above the threshold?
    pub blocked: bool,
    pub candidates: Vec<Candidate>,
}

/// Checks whether `name` may be newly created (no candidates above the threshold).
pub fn check_duplicate(
    kind: TaxonomyKind,
    name: &str,
    entries: &[TaxonomyEntry],
    aliases: &[Alias],
    opts: &MatchOptions,
) -> DuplicateCheck {
    let ext = MatchOptions { mode: MatchMode::Extended, ..*opts };
    let mut candidates = find_similar(kind, name, entries, aliases, &ext);
    // Also upstream hits block (otherwise the upstream UI would immediately assign the new object
    // to the old one).
    for c in candidates.iter_mut() {
        if c.upstream_match && !c.above_threshold {
            c.above_threshold = true;
            c.reasons.push(reason("upstream_match", "upstream matching.py würde diesen Namen zuordnen"));
        }
    }
    let blocked = candidates.iter().any(|c| c.above_threshold);
    DuplicateCheck { blocked, candidates }
}

/// Normalized alias key (for `paperrust_taxonomy_alias.alias_key`).
pub fn alias_key(kind: TaxonomyKind, alias: &str) -> String {
    normalize_with(alias, kind.name_kind(), false).key
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(kind: TaxonomyKind, names: &[&str]) -> Vec<TaxonomyEntry> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| TaxonomyEntry { kind, id: i as i64 + 1, name: n.to_string(), owner_id: None, extra: None, document_count: None })
            .collect()
    }

    fn best(kind: TaxonomyKind, q: &str, names: &[&str]) -> Option<Candidate> {
        find_similar(kind, q, &entries(kind, names), &[], &MatchOptions::default()).into_iter().next()
    }

    #[test]
    fn correspondents_legal_forms() {
        let names = ["Swisscom (Schweiz) AG", "Sunrise GmbH", "Die Schweizerische Post AG"];
        let c = best(TaxonomyKind::Correspondent, "Swisscom", &names).unwrap();
        assert_eq!(c.id, 1);
        assert!(c.above_threshold, "{c:?}");
        assert_eq!(c.reasons[0].code, "normalized_equal");
        // upstream would not have assigned "Swisscom" to "Swisscom (Schweiz) AG"
        assert!(!c.upstream_match);
        let c = best(TaxonomyKind::Correspondent, "SWISSCOM AG", &names).unwrap();
        assert_eq!(c.id, 1);
        let c = best(TaxonomyKind::Correspondent, "Sunrise UPC GmbH", &names).unwrap();
        assert_eq!(c.id, 2);
        assert!(c.above_threshold, "{c:?}");
    }

    #[test]
    fn umlauts_both_ways() {
        let n = ["Müller & Söhne GmbH"];
        for q in ["Mueller und Soehne", "Muller Sohne", "MÜLLER & SÖHNE", "mueller & soehne gmbh"] {
            let c = best(TaxonomyKind::Correspondent, q, &n).unwrap();
            assert!(c.above_threshold, "{q}: {c:?}");
        }
    }

    #[test]
    fn french_italian() {
        let n = ["Crédit Agricole (Suisse) SA", "Banca dello Stato del Cantone Ticino"];
        assert!(best(TaxonomyKind::Correspondent, "Credit Agricole", &n).unwrap().above_threshold);
        let c = best(TaxonomyKind::Correspondent, "Banca Stato Cantone Ticino", &n).unwrap();
        assert_eq!(c.id, 2);
        assert!(c.above_threshold);
    }

    #[test]
    fn number_mismatch_blocks_false_positive() {
        // upstream wrongly assigns "Steuern 2025" to "Steuern 2024" (golden match_pool #8)
        assert!(upstream_pair_match("Steuern 2025", "Steuern 2024"));
        let c = best(TaxonomyKind::Tag, "Steuern 2025", &["Steuern 2024"]).unwrap();
        assert!(!c.above_threshold);
        assert!(c.reasons.iter().any(|r| r.code == "number_mismatch"));
        // check_duplicate blocks anyway, because upstream would assign
        let d = check_duplicate(
            TaxonomyKind::Tag,
            "Steuern 2025",
            &entries(TaxonomyKind::Tag, &["Steuern 2024"]),
            &[],
            &MatchOptions::default(),
        );
        assert!(d.blocked);
    }

    #[test]
    fn typos_and_plural() {
        let n = ["Rechnung", "Versicherung", "Krankenkasse"];
        assert_eq!(best(TaxonomyKind::DocumentType, "Rechnungen", &n).unwrap().id, 1);
        assert!(best(TaxonomyKind::DocumentType, "Rechnungen", &n).unwrap().above_threshold);
        assert!(best(TaxonomyKind::Tag, "Versicherungen", &n).unwrap().above_threshold);
        assert!(best(TaxonomyKind::Tag, "Krankenkassse", &n).unwrap().above_threshold);
        assert!(best(TaxonomyKind::Tag, "Kranken Kasse", &n).unwrap().above_threshold);
    }

    #[test]
    fn unrelated_not_reported_or_below() {
        let n = ["Rechnung", "Vertrag"];
        let r = find_similar(TaxonomyKind::Tag, "Urlaub", &entries(TaxonomyKind::Tag, &n), &[], &MatchOptions::default());
        assert!(r.iter().all(|c| !c.above_threshold), "{r:?}");
        let d = check_duplicate(TaxonomyKind::Tag, "Urlaub", &entries(TaxonomyKind::Tag, &n), &[], &MatchOptions::default());
        assert!(!d.blocked);
    }

    #[test]
    fn alias_hits() {
        let e = entries(TaxonomyKind::Correspondent, &["Swisscom (Schweiz) AG"]);
        let a = vec![Alias {
            kind: TaxonomyKind::Correspondent,
            target_id: 1,
            alias: "Blue Entertainment".into(),
            alias_key: alias_key(TaxonomyKind::Correspondent, "Blue Entertainment"),
            owner_id: None,
        }];
        let r = find_similar(TaxonomyKind::Correspondent, "blue entertainment AG", &e, &a, &MatchOptions::default());
        assert_eq!(r[0].score, 1.0);
        assert_eq!(r[0].reasons[0].code, "alias");
    }

    #[test]
    fn storage_path_compares_path() {
        let mut e = entries(TaxonomyKind::StoragePath, &["Finanzen"]);
        e[0].extra = Some("Finanzen/Rechnungen/{{ created_year }}".into());
        let r = find_similar(TaxonomyKind::StoragePath, "Finanzen/Rechnungen", &e, &[], &MatchOptions::default());
        assert!(r[0].above_threshold, "{r:?}");
    }

    #[test]
    fn upstream_mode_is_compat() {
        let e = entries(TaxonomyKind::Tag, &["Test Tag 1", "Test Tag 2"]);
        let o = MatchOptions { mode: MatchMode::Upstream, ..Default::default() };
        let r = find_similar(TaxonomyKind::Tag, "Teest Tag 2", &e, &[], &o);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].id, 2);
        assert!(find_similar(TaxonomyKind::Tag, "Swisscom", &e, &[], &o).is_empty());
    }

    #[test]
    fn visibility_like_permitted_object_ids() {
        let mut e = entries(TaxonomyKind::Tag, &["a", "b", "c", "d"]);
        e[1].owner_id = Some(7); // foreign
        e[2].owner_id = Some(5); // own
        e[3].owner_id = Some(7); // foreign, but released via group
        let grants = vec![Grant { kind: TaxonomyKind::Tag, object_id: 4, user_id: None, group_id: Some(3), codename: "view_tag".into() }];
        let p = Principal { user_id: Some(5), is_active: true, is_superuser: false, group_ids: vec![3] };
        let vis: Vec<i64> = e.iter().filter(|x| p.can_view(x, &grants)).map(|x| x.id).collect();
        assert_eq!(vis, vec![1, 3, 4]);
        let none = Principal::default();
        assert_eq!(e.iter().filter(|x| none.can_view(x, &grants)).count(), 1);
        let inactive = Principal { user_id: Some(5), is_active: false, is_superuser: true, group_ids: vec![] };
        assert_eq!(e.iter().filter(|x| inactive.can_view(x, &grants)).count(), 0);
        let su = Principal { user_id: Some(1), is_active: true, is_superuser: true, group_ids: vec![] };
        assert_eq!(e.iter().filter(|x| su.can_view(x, &grants)).count(), 4);
    }

    #[test]
    fn kind_parse() {
        assert_eq!(TaxonomyKind::parse("tags"), Some(TaxonomyKind::Tag));
        assert_eq!(TaxonomyKind::parse("document_types"), Some(TaxonomyKind::DocumentType));
        assert_eq!(TaxonomyKind::parse("custom_fields"), Some(TaxonomyKind::CustomField));
        assert_eq!(TaxonomyKind::parse("x"), None);
    }
}
