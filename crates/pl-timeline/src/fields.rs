//! Which custom fields mean which kind of date (or the amount).
//!
//! Precedence: setting [`crate::SETTING_FIELDS`] (JSON object kind → list of field names or
//! ids; additionally `"amount"` and `"ignore"`), otherwise detection by field name ([`kind_for_name`]).
//! Date kinds apply only to fields of type `date`, `amount` only to `monetary`.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::DateKind;

/// A custom field (`documents_customfield`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInfo {
    pub id: i64,
    pub name: String,
    pub data_type: String,
}

/// Where the assignment comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MapSource {
    Setting,
    Heuristic,
}

/// Effective assignment.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FieldKinds {
    /// Date field id → (kind, source).
    pub dates: BTreeMap<i64, (DateKind, MapSource)>,
    /// Betragsfelder in Vorrangreihenfolge.
    pub amount: Vec<i64>,
}

fn fold(s: &str) -> String {
    s.to_lowercase().replace('ä', "ae").replace('ö', "oe").replace('ü', "ue").replace('ß', "ss")
}

/// Detection by field name (first matching rule wins).
pub fn kind_for_name(name: &str) -> Option<DateKind> {
    let n = fold(name);
    let words: Vec<&str> = n.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let has = |f: &dyn Fn(&str) -> bool| words.iter().any(|w| f(w));
    let phrase = |p: &str| n.contains(p);
    if has(&|w| w.contains("kuendig") || w.starts_with("cancel") || w == "notice") {
        return Some(DateKind::CancelBy);
    }
    if has(&|w| w.contains("garantie") || w.contains("gewaehrleist") || w.starts_with("warrant")) {
        return Some(DateKind::WarrantyUntil);
    }
    if phrase("gueltig ab")
        || phrase("gueltig von")
        || phrase("valid from")
        || has(&|w| w.contains("beginn") || w == "start" || w.starts_with("startdat"))
    {
        return Some(DateKind::ValidFrom);
    }
    if phrase("gueltig bis")
        || phrase("valid until")
        || phrase("valid to")
        || phrase("valid thru")
        || has(&|w| {
            w.starts_with("ablauf")
                || w.starts_with("laufzeit")
                || w == "ende"
                || w.ends_with("ende")
                || w.starts_with("expir")
                || w == "end"
        })
    {
        return Some(DateKind::ValidUntil);
    }
    if has(&|w| {
        w.starts_with("faellig")
            || w.starts_with("zahlbar")
            || w.ends_with("frist")
            || w == "due"
            || w == "deadline"
            || w.starts_with("payment")
    }) {
        return Some(DateKind::Due);
    }
    None
}

fn matches(v: &Value, f: &FieldInfo) -> bool {
    match v {
        Value::String(s) => fold(s.trim()) == fold(f.name.trim()) || s.trim().parse::<i64>().ok() == Some(f.id),
        Value::Number(n) => n.as_i64() == Some(f.id),
        _ => false,
    }
}

fn list(v: &Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a.clone(),
        Value::Null => vec![],
        other => vec![other.clone()],
    }
}

/// Assignment from fields and setting; `warnings` names invalid parts of the setting.
pub fn resolve(fields: &[FieldInfo], setting: Option<&str>) -> (FieldKinds, Vec<String>) {
    let mut warnings = Vec::new();
    let mut map: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    if let Some(raw) = setting.map(str::trim).filter(|s| !s.is_empty()) {
        match serde_json::from_str::<Value>(raw) {
            Ok(Value::Object(o)) => {
                for (k, v) in o {
                    if k == "amount" || k == "ignore" || DateKind::parse(&k).is_some_and(|k| k != DateKind::Created) {
                        map.insert(k, list(&v));
                    } else {
                        warnings.push(format!("unknown kind {k:?}"));
                    }
                }
            }
            _ => warnings.push(format!("{} must be a JSON object", crate::SETTING_FIELDS)),
        }
    }
    let ignore = map.get("ignore").cloned().unwrap_or_default();
    let mut out = FieldKinds::default();
    let mut sorted: Vec<&FieldInfo> = fields.iter().collect();
    sorted.sort_by_key(|f| f.id);
    for f in &sorted {
        if ignore.iter().any(|v| matches(v, f)) {
            continue;
        }
        match f.data_type.as_str() {
            "date" => {
                let set = DateKind::ALL
                    .into_iter()
                    .filter(|k| *k != DateKind::Created)
                    .find(|k| map.get(k.as_str()).is_some_and(|l| l.iter().any(|v| matches(v, f))));
                if let Some(k) = set {
                    out.dates.insert(f.id, (k, MapSource::Setting));
                } else if let Some(k) = kind_for_name(&f.name) {
                    out.dates.insert(f.id, (k, MapSource::Heuristic));
                }
            }
            "monetary" => {}
            _ => {}
        }
    }
    // Amount: assigned ones first (in the order of the setting), then all other monetary
    if let Some(l) = map.get("amount") {
        for v in l {
            if let Some(f) = sorted.iter().find(|f| f.data_type == "monetary" && matches(v, f))
                && !out.amount.contains(&f.id)
            {
                out.amount.push(f.id);
            }
        }
    }
    for f in &sorted {
        if f.data_type == "monetary" && !out.amount.contains(&f.id) && !ignore.iter().any(|v| matches(v, f)) {
            out.amount.push(f.id);
        }
    }
    (out, warnings)
}

/// Amount `"CHF89.90"` / `"89.90"` → (value, currency).
pub fn parse_monetary(v: &str) -> Option<(String, Option<String>)> {
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    let (cur, num) = if v.len() > 3 && v.as_bytes()[..3].iter().all(u8::is_ascii_alphabetic) {
        (Some(v[..3].to_ascii_uppercase()), &v[3..])
    } else {
        (None, v)
    };
    let n: f64 = num.trim().parse().ok()?;
    Some((format!("{n:.2}"), cur))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(id: i64, name: &str, t: &str) -> FieldInfo {
        FieldInfo { id, name: name.into(), data_type: t.into() }
    }

    #[test]
    fn names() {
        let cases = [
            ("Fällig am", Some(DateKind::Due)),
            ("Zahlbar bis", Some(DateKind::Due)),
            ("Zahlungsfrist", Some(DateKind::Due)),
            ("Due date", Some(DateKind::Due)),
            ("Kündigungsfrist", Some(DateKind::CancelBy)),
            ("Kuendigung bis", Some(DateKind::CancelBy)),
            ("Garantie bis", Some(DateKind::WarrantyUntil)),
            ("Warranty", Some(DateKind::WarrantyUntil)),
            ("Gültig ab", Some(DateKind::ValidFrom)),
            ("Vertragsbeginn", Some(DateKind::ValidFrom)),
            ("Gültig bis", Some(DateKind::ValidUntil)),
            ("Vertragsende", Some(DateKind::ValidUntil)),
            ("Ablaufdatum", Some(DateKind::ValidUntil)),
            ("Expiry", Some(DateKind::ValidUntil)),
            ("Rechnungsdatum", None),
            ("Kalenderdatum", None),
            ("Geburtstag", None),
        ];
        for (n, k) in cases {
            assert_eq!(kind_for_name(n), k, "{n}");
        }
    }

    #[test]
    fn setting_wins_and_types() {
        let fields = [
            f(1, "Fällig am", "date"),
            f(2, "Termin", "date"),
            f(3, "Betrag", "monetary"),
            f(4, "Gebühr", "monetary"),
            f(5, "Gültig bis", "string"),
            f(6, "Garantie bis", "date"),
        ];
        let (k, w) = resolve(&fields, None);
        assert!(w.is_empty());
        assert_eq!(k.dates.get(&1), Some(&(DateKind::Due, MapSource::Heuristic)));
        assert!(!k.dates.contains_key(&2), "unbekannter Name");
        assert!(!k.dates.contains_key(&5), "kein Datumsfeld");
        assert_eq!(k.amount, vec![3, 4]);
        let (k, w) = resolve(&fields, Some(r#"{"cancel_by":["termin"],"due":[6],"amount":["Gebühr"],"ignore":["Fällig am"],"nope":[]}"#));
        assert_eq!(w.len(), 1, "{w:?}");
        assert_eq!(k.dates.get(&2), Some(&(DateKind::CancelBy, MapSource::Setting)));
        assert_eq!(k.dates.get(&6), Some(&(DateKind::Due, MapSource::Setting)));
        assert!(!k.dates.contains_key(&1), "ignoriert");
        assert_eq!(k.amount, vec![4, 3]);
        let (_, w) = resolve(&fields, Some("[1]"));
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn monetary() {
        assert_eq!(parse_monetary("CHF89.9"), Some(("89.90".into(), Some("CHF".into()))));
        assert_eq!(parse_monetary("eur1234.50"), Some(("1234.50".into(), Some("EUR".into()))));
        assert_eq!(parse_monetary("12"), Some(("12.00".into(), None)));
        assert_eq!(parse_monetary(""), None);
        assert_eq!(parse_monetary("abc"), None);
    }
}
