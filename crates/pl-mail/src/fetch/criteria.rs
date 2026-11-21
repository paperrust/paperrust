//! Search criteria and mail actions per rule.
//!
//! upstream: src/paperless_mail/mail.py@v3.4.2 (BaseMailAction and subclasses, get_rule_action,
//! make_criterias, APPLE_MAIL_TAG_COLORS), imap_tools 1.15.1 `query.py` (AND/NOT, ParamConverter:
//! keys **sorted alphabetically**, `format_date`, `quote`), `consts.py` (SHORT_MONTH_NAMES).

use chrono::NaiveDate;

use super::client::quote;

pub const ACTION_DELETE: i16 = 1;
pub const ACTION_MOVE: i16 = 2;
pub const ACTION_MARK_READ: i16 = 3;
pub const ACTION_FLAG: i16 = 4;
pub const ACTION_TAG: i16 = 5;

/// `APPLE_MAIL_TAG_COLORS`.
pub const APPLE_MAIL_TAG_COLORS: &[(&str, &[&str])] = &[
    ("red", &[]),
    ("orange", &["$MailFlagBit0"]),
    ("yellow", &["$MailFlagBit1"]),
    ("blue", &["$MailFlagBit2"]),
    ("green", &["$MailFlagBit0", "$MailFlagBit1"]),
    ("violet", &["$MailFlagBit0", "$MailFlagBit2"]),
    ("grey", &["$MailFlagBit1", "$MailFlagBit2"]),
];

const SHORT_MONTH_NAMES: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Error when building an action (`MailError` / `ValueError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionError {
    Mail(String),
    Value(String),
}

impl std::fmt::Display for ActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionError::Mail(m) | ActionError::Value(m) => f.write_str(m),
        }
    }
}

/// Mail action of a rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MailAction {
    Delete,
    Move,
    MarkRead,
    Flag,
    /// `TagMailAction`: Apple colour or keyword (Gmail label, if supported).
    Tag {
        color: Option<String>,
        keyword: Option<String>,
        gmail: bool,
    },
}

/// `get_rule_action(rule, supports_gmail_labels=…)`.
pub fn get_rule_action(action: i16, parameter: Option<&str>, supports_gmail_labels: bool) -> Result<MailAction, ActionError> {
    Ok(match action {
        ACTION_FLAG => MailAction::Flag,
        ACTION_DELETE => MailAction::Delete,
        ACTION_MOVE => MailAction::Move,
        ACTION_MARK_READ => MailAction::MarkRead,
        ACTION_TAG => {
            // rule.action_parameter can be None → upstream AttributeError (`.lower()`)
            let p = parameter.ok_or_else(|| ActionError::Value("'NoneType' object has no attribute 'lower'".into()))?;
            if p.to_lowercase().contains("apple:") {
                let parts: Vec<&str> = p.split(':').collect();
                if parts.len() != 2 {
                    return Err(ActionError::Value(format!("too many values to unpack (expected 2, got {})", parts.len())));
                }
                let color = pl_parsers::pystr::py_strip(parts[1]).to_lowercase();
                if !APPLE_MAIL_TAG_COLORS.iter().any(|(c, _)| *c == color) {
                    return Err(ActionError::Mail("Not a valid AppleMail tag color.".into()));
                }
                MailAction::Tag { color: Some(color), keyword: None, gmail: supports_gmail_labels }
            } else {
                MailAction::Tag { color: None, keyword: Some(p.to_string()), gmail: supports_gmail_labels }
            }
        }
        _ => return Err(ActionError::Value("Unknown action.".into())),
    })
}

/// A search parameter (imap_tools `ParamConverter`), sortable by key.
#[derive(Debug, Clone)]
enum Param {
    Seen(bool),
    Flagged(bool),
    NoKeyword(String),
    From(String),
    To(String),
    Subject(String),
    Body(String),
    DateGte(NaiveDate),
}

impl Param {
    fn key(&self) -> &'static str {
        match self {
            Param::Seen(_) => "seen",
            Param::Flagged(_) => "flagged",
            Param::NoKeyword(_) => "no_keyword",
            Param::From(_) => "from_",
            Param::To(_) => "to",
            Param::Subject(_) => "subject",
            Param::Body(_) => "body",
            Param::DateGte(_) => "date_gte",
        }
    }
    fn convert(&self) -> String {
        match self {
            Param::Seen(v) => if *v { "SEEN" } else { "UNSEEN" }.into(),
            Param::Flagged(v) => if *v { "FLAGGED" } else { "UNFLAGGED" }.into(),
            Param::NoKeyword(k) => format!("UNKEYWORD {k}"),
            Param::From(v) => format!("FROM {}", quote(v)),
            Param::To(v) => format!("TO {}", quote(v)),
            Param::Subject(v) => format!("SUBJECT {}", quote(v)),
            Param::Body(v) => format!("BODY {}", quote(v)),
            Param::DateGte(d) => {
                use chrono::Datelike;
                format!("SINCE {}-{}-{}", d.day(), SHORT_MONTH_NAMES[d.month0() as usize], d.year())
            }
        }
    }
}

/// `AND(*converted, **params)` → `(a b c)` (parameters alphabetically by key).
fn and(converted: &[String], mut params: Vec<Param>) -> String {
    params.sort_by(|a, b| a.key().cmp(b.key()));
    let mut all: Vec<String> = converted.to_vec();
    all.extend(params.iter().map(Param::convert));
    format!("({})", all.join(" "))
}

/// Fields of the rule that `make_criterias` reads.
#[derive(Debug, Clone, Default)]
pub struct RuleFilter {
    pub maximum_age: i32,
    pub filter_from: Option<String>,
    pub filter_to: Option<String>,
    pub filter_subject: Option<String>,
    pub filter_body: Option<String>,
}

fn nonempty(v: &Option<String>) -> Option<String> {
    v.as_ref().filter(|s| !s.is_empty()).cloned()
}

/// `make_criterias(rule, supports_gmail_labels=…)` as text (like `str(criteria)`).
/// `today` = `date.today()` (local date in `PAPERLESS_TIME_ZONE`).
pub fn make_criterias(rule: &RuleFilter, action: &MailAction, today: NaiveDate) -> String {
    let mut criterias: Vec<Param> = Vec::new();
    if rule.maximum_age > 0 {
        let d = today - chrono::Duration::days(i64::from(rule.maximum_age));
        criterias.push(Param::DateGte(d));
    }
    if let Some(v) = nonempty(&rule.filter_from) {
        criterias.push(Param::From(v));
    }
    if let Some(v) = nonempty(&rule.filter_to) {
        criterias.push(Param::To(v));
    }
    if let Some(v) = nonempty(&rule.filter_subject) {
        criterias.push(Param::Subject(v));
    }
    if let Some(v) = nonempty(&rule.filter_body) {
        criterias.push(Param::Body(v));
    }
    // action.get_criteria()
    match action {
        MailAction::Delete | MailAction::Move => {
            if criterias.is_empty() {
                "ALL".into()
            } else {
                and(&[], criterias)
            }
        }
        MailAction::MarkRead => {
            criterias.push(Param::Seen(false));
            and(&[], criterias)
        }
        MailAction::Flag => {
            criterias.push(Param::Flagged(false));
            and(&[], criterias)
        }
        MailAction::Tag { color: Some(_), .. } => {
            criterias.push(Param::Flagged(false));
            and(&[], criterias)
        }
        MailAction::Tag { keyword: Some(k), gmail: true, .. } => {
            // AND(NOT(gmail_label=k), no_keyword=k) → LogicOperator; then AND(rule_query, **criterias)
            let not = format!("NOT (X-GM-LABELS {})", quote(k));
            let inner = and(&[not], vec![Param::NoKeyword(k.clone())]);
            and(&[inner], criterias)
        }
        MailAction::Tag { keyword: Some(k), .. } => {
            criterias.push(Param::NoKeyword(k.clone()));
            and(&[], criterias)
        }
        MailAction::Tag { .. } => "ALL".into(),
    }
}

/// Python `fnmatch.fnmatch` (pattern and name are already lower case; `normcase` is unchanged on
/// POSIX). Translation like `fnmatch.translate`: `*`, `?`, `[seq]`, `[!seq]`.
pub fn fnmatch(name: &str, pat: &str) -> bool {
    let n: Vec<char> = name.chars().collect();
    let p: Vec<char> = pat.chars().collect();
    type Class = (usize, Vec<(char, char)>, bool);
    fn class(p: &[char], i: usize) -> Option<Class> {
        // returns end (index after `]`), ranges, negated; None → `[` literal
        let mut j = i + 1;
        let mut neg = false;
        if j < p.len() && p[j] == '!' {
            neg = true;
            j += 1;
        }
        if j < p.len() && p[j] == ']' {
            j += 1;
        }
        while j < p.len() && p[j] != ']' {
            j += 1;
        }
        if j >= p.len() {
            return None;
        }
        let mut k = i + 1 + usize::from(neg);
        let mut ranges = Vec::new();
        let end = j;
        while k < end {
            let c = p[k];
            if k + 2 < end && p[k + 1] == '-' {
                let hi = p[k + 2];
                if c <= hi {
                    ranges.push((c, hi));
                }
                k += 3;
            } else {
                ranges.push((c, c));
                k += 1;
            }
        }
        Some((end + 1, ranges, neg))
    }
    fn rec(n: &[char], p: &[char], ni: usize, pi: usize) -> bool {
        if pi == p.len() {
            return ni == n.len();
        }
        match p[pi] {
            '*' => {
                let mut pj = pi;
                while pj < p.len() && p[pj] == '*' {
                    pj += 1;
                }
                (ni..=n.len()).any(|k| rec(n, p, k, pj))
            }
            '?' => ni < n.len() && rec(n, p, ni + 1, pi + 1),
            '[' => match class(p, pi) {
                Some((next, ranges, neg)) => {
                    if ni >= n.len() {
                        return false;
                    }
                    let c = n[ni];
                    let hit = ranges.iter().any(|(lo, hi)| *lo <= c && c <= *hi);
                    // empty class: `(?!)` or, when negated, `.`
                    let ok = if ranges.is_empty() { neg } else { hit != neg };
                    ok && rec(n, p, ni + 1, next)
                }
                None => ni < n.len() && n[ni] == '[' && rec(n, p, ni + 1, pi + 1),
            },
            c => ni < n.len() && n[ni] == c && rec(n, p, ni + 1, pi + 1),
        }
    }
    rec(&n, &p, 0, 0)
}

/// `filename_inclusion_matches`.
pub fn filename_inclusion_matches(include: Option<&str>, filename: &str) -> bool {
    match include.filter(|s| !s.is_empty()) {
        None => true,
        Some(inc) => {
            let f = filename.to_lowercase();
            inc.split(',').any(|pat| !pat.is_empty() && fnmatch(&f, &pat.to_lowercase()))
        }
    }
}

/// `filename_exclusion_matches`.
pub fn filename_exclusion_matches(exclude: Option<&str>, filename: &str) -> bool {
    match exclude.filter(|s| !s.is_empty()) {
        None => false,
        Some(exc) => {
            let f = filename.to_lowercase();
            exc.split(',').any(|pat| !pat.is_empty() && fnmatch(&f, &pat.to_lowercase()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, dd: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, dd).unwrap()
    }

    #[test]
    fn criteria_like_imap_tools() {
        let today = d(2025, 3, 10);
        let r =
            RuleFilter { maximum_age: 30, filter_from: Some("a@b".into()), filter_subject: Some("Inv \"x\"".into()), ..Default::default() };
        let c = make_criterias(&r, &MailAction::MarkRead, today);
        assert_eq!(c, "(SINCE 8-Feb-2025 FROM \"a@b\" UNSEEN SUBJECT \"Inv \\\"x\\\"\")");
        let r0 = RuleFilter::default();
        assert_eq!(make_criterias(&r0, &MailAction::Delete, today), "ALL");
        assert_eq!(make_criterias(&r0, &MailAction::Flag, today), "(UNFLAGGED)");
        let tag = MailAction::Tag { color: None, keyword: Some("paperless".into()), gmail: true };
        assert_eq!(make_criterias(&r0, &tag, today), "((NOT (X-GM-LABELS \"paperless\") UNKEYWORD paperless))");
        let tag2 = MailAction::Tag { color: None, keyword: Some("paperless".into()), gmail: false };
        let r2 = RuleFilter { maximum_age: 1, filter_body: Some("b".into()), filter_to: Some("t".into()), ..Default::default() };
        assert_eq!(make_criterias(&r2, &tag2, today), "(BODY \"b\" SINCE 9-Mar-2025 UNKEYWORD paperless TO \"t\")");
    }

    #[test]
    fn tag_action_parsing() {
        assert!(matches!(get_rule_action(5, Some("apple:Green"), false), Ok(MailAction::Tag { color: Some(c), .. }) if c == "green"));
        assert_eq!(get_rule_action(5, Some("apple:pink"), false), Err(ActionError::Mail("Not a valid AppleMail tag color.".into())));
        assert!(matches!(get_rule_action(5, Some("apple:a:b"), false), Err(ActionError::Value(_))));
        assert!(matches!(get_rule_action(5, Some("kw"), true), Ok(MailAction::Tag { keyword: Some(_), gmail: true, .. })));
    }

    #[test]
    fn fnmatch_like_python() {
        assert!(fnmatch("invoice.pdf", "*.pdf"));
        assert!(fnmatch("invoice.pdf", "inv*"));
        assert!(!fnmatch("invoice.pdf", "*.png"));
        assert!(fnmatch("a1.pdf", "a?.pdf"));
        assert!(fnmatch("a1.pdf", "a[0-9].pdf"));
        assert!(!fnmatch("ab.pdf", "a[!b].pdf"));
        assert!(fnmatch("a[.pdf", "a[.pdf"));
        assert!(filename_inclusion_matches(Some("*.PDF,*.png"), "Scan.pdf"));
        assert!(!filename_inclusion_matches(Some(","), "x"));
        assert!(filename_exclusion_matches(Some("*sig*"), "signature.asc"));
        assert!(!filename_exclusion_matches(None, "x"));
    }
}
