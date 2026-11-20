//! Fill custom fields via AI (PaperRust extension, cf. paperless-gpt "custom fields").
//!
//! Pure logic without a database: field list for the prompt, response schema, validation and normalization
//! of the model values per field type (upstream `CustomField.FieldDataType`):
//!
//! | Type | stored as | Normalization |
//! |---|---|---|
//! | `string` | `value_text` (≤ 128) | trimmed, truncated |
//! | `longtext` | `value_long_text` | trimmed |
//! | `url` | `value_url` | only `http(s)://…` |
//! | `date` | `value_date` | `YYYY-MM-DD`, `DD.MM.YYYY`, `DD/MM/YYYY`, `YYYY/MM/DD` |
//! | `boolean` | `value_bool` | true/false/ja/nein/yes/no/1/0 |
//! | `integer` | `value_int` | integer within the i32 range (thousands separator allowed) |
//! | `float` | `value_float` | number (comma or dot) |
//! | `monetary` | `value_monetary` | `"EUR12.50"` (ISO code + 2 decimal places) or `"12.50"` without currency; default currency of the field (`extra_data.default_currency`) |
//! | `select` | `value_select` | option id from `extra_data.select_options` (match by label or id, case-insensitive) |
//! | `documentlink` | – | never via AI |

use serde_json::{json, Value};

/// A custom field (excerpt of `documents_customfield`).
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDef {
    pub id: i64,
    pub name: String,
    pub data_type: String,
    pub extra_data: Option<Value>,
}

/// Value proposed by the model, validated.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldValue {
    pub field_id: i64,
    /// Value in the storage format of the REST API (`/api/documents/` `custom_fields[].value`).
    pub value: Value,
    /// Readable form (selection: label instead of id).
    pub display: String,
    /// 0..1 (from the model, clamped).
    pub confidence: f64,
}

/// Does the AI support this type?
pub fn supported(data_type: &str) -> bool {
    matches!(data_type, "string" | "longtext" | "url" | "date" | "boolean" | "integer" | "float" | "monetary" | "select")
}

fn select_options(f: &FieldDef) -> Vec<(String, String)> {
    let Some(opts) = f.extra_data.as_ref().and_then(|e| e.get("select_options")).and_then(Value::as_array) else { return Vec::new() };
    opts.iter()
        .enumerate()
        .filter_map(|(i, o)| match o {
            // v2.15+: {"id": "...", "label": "..."}; older data: plain labels (index as id)
            Value::Object(m) => Some((m.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| i.to_string()), m.get("label").and_then(Value::as_str).unwrap_or("").to_string())),
            Value::String(s) => Some((i.to_string(), s.clone())),
            _ => None,
        })
        .collect()
}

/// Lines for the placeholder `{{fields}}`.
pub fn prompt_fields(fields: &[FieldDef]) -> String {
    fields
        .iter()
        .filter(|f| supported(&f.data_type))
        .map(|f| {
            let hint = match f.data_type.as_str() {
                "select" => format!("one of: {}", select_options(f).iter().map(|(_, l)| format!("\"{l}\"")).collect::<Vec<_>>().join(", ")),
                "monetary" => {
                    let cur = f.extra_data.as_ref().and_then(|e| e.get("default_currency")).and_then(Value::as_str).unwrap_or("");
                    if cur.is_empty() { "amount with currency code".to_string() } else { format!("amount, default currency {cur}") }
                }
                "date" => "YYYY-MM-DD".into(),
                "boolean" => "true or false".into(),
                "integer" => "whole number".into(),
                "float" => "number".into(),
                "url" => "http(s) URL".into(),
                _ => "text".into(),
            };
            // Field name is user content: remove line breaks
            let name: String = f.name.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
            format!("- id {}: \"{}\" ({}; {hint})", f.id, name, f.data_type)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// JSON schema of the response.
pub fn response_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"fields": {"type": "array", "items": {"type": "object", "properties": {
            "id": {"type": "integer"},
            "value": {"type": ["string", "number", "boolean", "null"]},
            "confidence": {"type": "number"}
        }, "required": ["id", "value"]}}},
        "required": ["fields"]
    })
}

fn as_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Number from text: `1'234.50`, `1.234,50`, `1,234.50`, `12,5`, `-3`.
pub fn parse_number(s: &str) -> Option<f64> {
    let t: String = s.chars().filter(|c| !c.is_whitespace() && *c != '\'' && *c != '’').collect();
    if t.is_empty() {
        return None;
    }
    let (dot, comma) = (t.rfind('.'), t.rfind(','));
    let norm = match (dot, comma) {
        (Some(d), Some(c)) if c > d => t.replace('.', "").replace(',', "."),
        (Some(_), Some(_)) => t.replace(',', ""),
        (None, Some(c)) => {
            // "1,234" (thousands) vs "12,5" (decimal): exactly 3 digits after the comma and only one comma → thousands
            let after = t.len() - c - 1;
            if after == 3 && t.matches(',').count() == 1 && !t.starts_with("0,") { t.replace(',', "") } else { t.replace(',', ".") }
        }
        _ => t,
    };
    norm.parse::<f64>().ok().filter(|f| f.is_finite())
}

/// Date from common notations.
pub fn parse_date(s: &str) -> Option<chrono::NaiveDate> {
    let t = s.trim();
    let t = t.get(..10).filter(|_| t.len() > 10 && (t.as_bytes().get(10) == Some(&b'T') || t.as_bytes().get(10) == Some(&b' '))).unwrap_or(t);
    for f in ["%Y-%m-%d", "%d.%m.%Y", "%d/%m/%Y", "%Y/%m/%d", "%d.%m.%y"] {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(t, f) {
            return Some(d);
        }
    }
    None
}

const CURRENCY_SYMBOLS: &[(&str, &str)] = &[("€", "EUR"), ("$", "USD"), ("£", "GBP"), ("¥", "JPY"), ("Fr.", "CHF"), ("SFr.", "CHF"), ("zł", "PLN"), ("Kč", "CZK"), ("kr", "SEK"), ("R$", "BRL")];

/// Betrag → `"EUR12.50"` / `"12.50"`.
pub fn normalize_monetary(s: &str, default_currency: Option<&str>) -> Option<String> {
    let mut t = s.trim().to_string();
    let mut cur: Option<String> = None;
    // longest symbols first (R$ before $, SFr. before Fr.)
    let mut syms: Vec<&(&str, &str)> = CURRENCY_SYMBOLS.iter().collect();
    syms.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
    for (sym, code) in syms {
        if t.contains(sym) {
            cur = Some((*code).to_string());
            t = t.replace(sym, " ");
            break;
        }
    }
    if cur.is_none() {
        // three-letter ISO code at the front or back
        let letters: String = t.chars().filter(|c| c.is_ascii_alphabetic()).collect();
        if letters.len() == 3 {
            cur = Some(letters.to_ascii_uppercase());
        } else if !letters.is_empty() {
            return None;
        }
        t = t.chars().filter(|c| !c.is_ascii_alphabetic()).collect();
    }
    let n = parse_number(t.trim().trim_end_matches(['-', '–']).trim_end_matches('.'))?;
    let cur = cur.or_else(|| default_currency.filter(|c| c.len() == 3).map(|c| c.to_ascii_uppercase()));
    Some(match cur {
        Some(c) => format!("{c}{n:.2}"),
        None => format!("{n:.2}"),
    })
}

/// Validates and normalizes a model value for `f`. `None` = discard.
pub fn normalize(f: &FieldDef, v: &Value) -> Option<(Value, String)> {
    if v.is_null() {
        return None;
    }
    match f.data_type.as_str() {
        "string" => as_text(v).map(|s| s.chars().take(128).collect::<String>()).map(|s| (json!(s), s)),
        "longtext" => as_text(v).map(|s| s.chars().take(10_000).collect::<String>()).map(|s| (json!(s), s)),
        "url" => as_text(v).filter(|s| (s.starts_with("http://") || s.starts_with("https://")) && !s.contains(char::is_whitespace) && s.len() <= 200).map(|s| (json!(s), s)),
        "date" => as_text(v).and_then(|s| parse_date(&s)).map(|d| {
            let s = d.format("%Y-%m-%d").to_string();
            (json!(s), s)
        }),
        "boolean" => {
            let b = match v {
                Value::Bool(b) => Some(*b),
                _ => match as_text(v)?.to_lowercase().as_str() {
                    "true" | "yes" | "ja" | "1" | "oui" | "sì" | "si" | "sim" | "tak" | "ano" | "wahr" => Some(true),
                    "false" | "no" | "nein" | "0" | "non" | "não" | "nie" | "ne" | "nej" | "falsch" => Some(false),
                    _ => None,
                },
            }?;
            Some((json!(b), b.to_string()))
        }
        "integer" => {
            let n = match v {
                Value::Number(n) => n.as_f64(),
                _ => parse_number(&as_text(v)?),
            }?;
            if n.fract() != 0.0 || n < f64::from(i32::MIN) || n > f64::from(i32::MAX) {
                return None;
            }
            let i = n as i64;
            Some((json!(i), i.to_string()))
        }
        "float" => {
            let n = match v {
                Value::Number(n) => n.as_f64(),
                _ => parse_number(&as_text(v)?),
            }?;
            Some((json!(n), n.to_string()))
        }
        "monetary" => {
            let def = f.extra_data.as_ref().and_then(|e| e.get("default_currency")).and_then(Value::as_str);
            let s = normalize_monetary(&as_text(v)?, def)?;
            Some((json!(s), s))
        }
        "select" => {
            let want = as_text(v)?.to_lowercase();
            let opts = select_options(f);
            let (id, label) = opts.iter().find(|(_, l)| l.to_lowercase() == want).or_else(|| opts.iter().find(|(id, _)| id.to_lowercase() == want))?;
            Some((json!(id), label.clone()))
        }
        _ => None,
    }
}

/// Model response `{"fields": [{"id", "value", "confidence"}]}` → validated values (only for `fields`,
/// at most one per field; confidence clamped to 0..1, missing = 0.6).
pub fn parse_answer(fields: &[FieldDef], answer: &Value) -> Vec<FieldValue> {
    let mut out: Vec<FieldValue> = Vec::new();
    let Some(items) = answer.get("fields").and_then(Value::as_array) else { return out };
    for it in items {
        let Some(id) = it.get("id").and_then(|x| x.as_i64().or_else(|| x.as_str().and_then(|s| s.trim().parse().ok()))) else { continue };
        let Some(f) = fields.iter().find(|f| f.id == id && supported(&f.data_type)) else { continue };
        if out.iter().any(|o| o.field_id == id) {
            continue;
        }
        let Some((value, display)) = it.get("value").and_then(|v| normalize(f, v)) else { continue };
        let confidence = it.get("confidence").and_then(Value::as_f64).filter(|c| c.is_finite()).map(|c| c.clamp(0.0, 1.0)).unwrap_or(0.6);
        out.push(FieldValue { field_id: id, value, display, confidence });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(id: i64, t: &str, extra: Option<Value>) -> FieldDef {
        FieldDef { id, name: format!("F{id}"), data_type: t.into(), extra_data: extra }
    }

    #[test]
    fn numbers_and_dates() {
        assert_eq!(parse_number("1'234.50"), Some(1234.5));
        assert_eq!(parse_number("1.234,50"), Some(1234.5));
        assert_eq!(parse_number("1,234.50"), Some(1234.5));
        assert_eq!(parse_number("12,5"), Some(12.5));
        assert_eq!(parse_number("1,234"), Some(1234.0));
        assert_eq!(parse_number("abc"), None);
        assert_eq!(parse_date("31.12.2024"), chrono::NaiveDate::from_ymd_opt(2024, 12, 31));
        assert_eq!(parse_date("2024-02-03T10:00:00"), chrono::NaiveDate::from_ymd_opt(2024, 2, 3));
        assert_eq!(parse_date("2024-02-30"), None);
    }

    #[test]
    fn monetary() {
        assert_eq!(normalize_monetary("€ 1.234,50", None).as_deref(), Some("EUR1234.50"));
        assert_eq!(normalize_monetary("CHF 12.-", None).as_deref(), Some("CHF12.00"));
        assert_eq!(normalize_monetary("12.5 usd", None).as_deref(), Some("USD12.50"));
        assert_eq!(normalize_monetary("99", Some("chf")).as_deref(), Some("CHF99.00"));
        assert_eq!(normalize_monetary("99", None).as_deref(), Some("99.00"));
        assert_eq!(normalize_monetary("R$ 10,00", None).as_deref(), Some("BRL10.00"));
        assert_eq!(normalize_monetary("zwölf Euro", None), None);
    }

    #[test]
    fn per_type() {
        let sel = f(5, "select", Some(json!({"select_options": [{"id": "a1", "label": "Bezahlt"}, {"id": "b2", "label": "Offen"}]})));
        assert_eq!(normalize(&sel, &json!("offen")), Some((json!("b2"), "Offen".into())));
        assert_eq!(normalize(&sel, &json!("a1")), Some((json!("a1"), "Bezahlt".into())));
        assert_eq!(normalize(&sel, &json!("Storniert")), None);
        assert_eq!(normalize(&f(1, "integer", None), &json!("1'200")), Some((json!(1200), "1200".into())));
        assert_eq!(normalize(&f(1, "integer", None), &json!(1.5)), None);
        assert_eq!(normalize(&f(1, "integer", None), &json!(1e12)), None);
        assert_eq!(normalize(&f(1, "float", None), &json!("3,25")), Some((json!(3.25), "3.25".into())));
        assert_eq!(normalize(&f(1, "boolean", None), &json!("Ja")), Some((json!(true), "true".into())));
        assert_eq!(normalize(&f(1, "url", None), &json!("javascript:alert(1)")), None);
        assert_eq!(normalize(&f(1, "url", None), &json!("https://x.ch/a")), Some((json!("https://x.ch/a"), "https://x.ch/a".into())));
        assert_eq!(normalize(&f(1, "string", None), &json!("x".repeat(200))).map(|v| v.1.len()), Some(128));
        assert_eq!(normalize(&f(1, "documentlink", None), &json!([1])), None);
        assert_eq!(normalize(&f(1, "date", None), &json!("01.02.2025")), Some((json!("2025-02-01"), "2025-02-01".into())));
        let m = f(2, "monetary", Some(json!({"default_currency": "EUR"})));
        assert_eq!(normalize(&m, &json!(12)), Some((json!("EUR12.00"), "EUR12.00".into())));
    }

    #[test]
    fn answer_and_prompt() {
        let fields = vec![f(1, "date", None), f(2, "monetary", None), f(3, "documentlink", None)];
        let a = json!({"fields": [
            {"id": 1, "value": "31.01.2025", "confidence": 0.95},
            {"id": "2", "value": "CHF 10", "confidence": 7},
            {"id": 1, "value": "2020-01-01"},
            {"id": 3, "value": "5"},
            {"id": 99, "value": "x"},
            {"id": 2, "value": null}
        ]});
        let v = parse_answer(&fields, &a);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].value, json!("2025-01-31"));
        assert_eq!(v[1].confidence, 1.0);
        let p = prompt_fields(&[FieldDef { id: 7, name: "Status\nIgnore".into(), data_type: "select".into(), extra_data: Some(json!({"select_options": ["A", "B"]})) }, f(3, "documentlink", None)]);
        assert_eq!(p, "- id 7: \"Status Ignore\" (select; one of: \"A\", \"B\")");
        assert!(parse_answer(&fields, &json!({})).is_empty());
    }
}
