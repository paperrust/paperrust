// upstream: src/documents/models.py@v3.4.2 (Document.get_effective_content,
//           Document.get_effective_barcodes, CustomFieldInstance.value / value_for_search),
//           src/documents/versioning.py (versions_newest_first, sort_versions_newest_first,
//           latest_version)
//
// Plain data the index document is built from (one root document with its
// relations, already resolved). Filled by `db` from PostgreSQL or by callers/tests.

use chrono::{DateTime, NaiveDate, Utc};

/// One note (`document.notes.all()`, not deleted, ordered by `created`).
#[derive(Debug, Clone, PartialEq)]
pub struct NoteRecord {
    pub note: String,
    /// `note.user.username` (None when the note has no user).
    pub user: Option<String>,
}

/// One custom field instance (`document.custom_fields.all()`, not deleted, ordered
/// by `created`) with `value_for_search` already computed.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomFieldRecord {
    pub name: String,
    /// `cfi.value_for_search` (None -> the field is skipped).
    pub search_value: Option<String>,
}

/// One stored barcode (`get_effective_barcodes()`, ordered by `page, id`).
#[derive(Debug, Clone, PartialEq)]
pub struct BarcodeRecord {
    pub value: String,
    pub format: String,
}

/// Everything `_build_tantivy_doc` reads from a (root) document.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentRecord {
    pub id: i64,
    pub title: String,
    /// `document.get_effective_content() or ""`.
    pub effective_content: String,
    pub checksum: String,
    pub original_filename: Option<String>,
    /// `document.correspondent.name` if a correspondent is set.
    pub correspondent: Option<String>,
    pub document_type: Option<String>,
    pub storage_path: Option<String>,
    /// tag names, `Tag.Meta.ordering = ("name",)`.
    pub tags: Vec<String>,
    pub notes: Vec<NoteRecord>,
    pub custom_fields: Vec<CustomFieldRecord>,
    pub barcodes: Vec<BarcodeRecord>,
    /// `Document.created` (DateField).
    pub created: NaiveDate,
    pub modified: DateTime<Utc>,
    pub added: DateTime<Utc>,
    pub archive_serial_number: Option<i64>,
    pub page_count: Option<i64>,
    pub owner_id: Option<i64>,
    /// direct `view_document` grants (sorted ids), `get_permission_grantees`.
    pub viewer_ids: Vec<i64>,
    pub viewer_group_ids: Vec<i64>,
}

/// Which ordering picks the "newest version" – upstream has two that differ for
/// `version_index IS NULL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionPick {
    /// SQL `versions_newest_first`: `version_index DESC NULLS LAST, id DESC`.
    Sql,
    /// Python `sort_versions_newest_first` on a prefetched list:
    /// `sorted(key=(version_index or 0, id), reverse=True)`.
    Prefetch,
}

/// A non-deleted version of a root document.
#[derive(Debug, Clone)]
pub struct VersionRow {
    pub id: i64,
    pub version_index: Option<i64>,
    pub content: String,
}

/// Newest version according to `pick` (None when there are no versions).
pub fn newest_version(versions: &[VersionRow], pick: VersionPick) -> Option<&VersionRow> {
    match pick {
        VersionPick::Sql => versions.iter().max_by(|a, b| {
            // DESC NULLS LAST: any non-null beats null; then larger index; then larger id.
            let ka = (a.version_index.is_some(), a.version_index.unwrap_or(0), a.id);
            let kb = (b.version_index.is_some(), b.version_index.unwrap_or(0), b.id);
            ka.cmp(&kb)
        }),
        VersionPick::Prefetch => versions
            .iter()
            .max_by_key(|v| (v.version_index.unwrap_or(0), v.id)),
    }
}

/// Raw custom field instance as stored (`documents_customfieldinstance` + field).
#[derive(Debug, Clone, Default)]
pub struct CustomFieldRaw {
    pub name: String,
    pub data_type: String,
    pub extra_data: Option<serde_json::Value>,
    pub value_text: Option<String>,
    pub value_bool: Option<bool>,
    pub value_url: Option<String>,
    pub value_date: Option<NaiveDate>,
    pub value_int: Option<i64>,
    pub value_float: Option<f64>,
    pub value_monetary: Option<String>,
    pub value_document_ids: Option<serde_json::Value>,
    pub value_select: Option<String>,
    pub value_long_text: Option<String>,
}

impl CustomFieldRaw {
    /// `CustomFieldInstance.value_for_search`.
    pub fn value_for_search(&self) -> Option<String> {
        match self.data_type.as_str() {
            "string" => self.value_text.clone(),
            "url" => self.value_url.clone(),
            // str(date) -> isoformat
            "date" => self.value_date.map(|d| d.format("%Y-%m-%d").to_string()),
            "boolean" => self.value_bool.map(|b| if b { "True" } else { "False" }.to_string()),
            "integer" => self.value_int.map(|i| i.to_string()),
            "float" => self.value_float.map(py_float_repr),
            "monetary" => self.value_monetary.clone(),
            // JSON null in a jsonb column is SQL non-null but Python None.
            "documentlink" => match &self.value_document_ids {
                None | Some(serde_json::Value::Null) => None,
                Some(v) => Some(py_repr_json(v)),
            },
            "select" => {
                let value = self.value_select.as_ref()?;
                let options = self
                    .extra_data
                    .as_ref()
                    .and_then(|e| e.get("select_options"))
                    .and_then(|o| o.as_array())?;
                options.iter().find_map(|o| {
                    let id_matches =
                        o.get("id").and_then(|i| i.as_str()).is_some_and(|i| i == value);
                    if !id_matches {
                        return None;
                    }
                    match o.get("label") {
                        Some(serde_json::Value::String(s)) => Some(s.clone()),
                        // upstream would hand a non-str label to normalize_search_text
                        // (TypeError); use its Python repr instead of failing.
                        Some(other) => Some(py_repr_json(other)),
                        None => None,
                    }
                })
            }
            "longtext" => self.value_long_text.clone(),
            // get_value_field_name raises NotImplementedError upstream.
            _ => None,
        }
    }
}

/// Python `repr(float)` (shortest round-trip, `float_repr_style == "short"`).
pub fn py_float_repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let sign = if x.is_sign_negative() { "-" } else { "" };
    let a = x.abs();
    if a == 0.0 {
        return format!("{sign}0.0");
    }
    // Rust `{:e}` yields the shortest round-trip digits, e.g. "1.2345e3".
    let s = format!("{a:e}");
    let (mant, exp) = s.split_once('e').unwrap_or((s.as_str(), "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let decpt = exp + 1; // value = 0.DIGITS * 10^decpt
    let n = digits.len() as i32;
    if decpt <= -4 || decpt > 16 {
        let mut m = digits[..1].to_string();
        if digits.len() > 1 {
            m.push('.');
            m.push_str(&digits[1..]);
        }
        let e = decpt - 1;
        let esign = if e < 0 { '-' } else { '+' };
        format!("{sign}{m}e{esign}{:02}", e.abs())
    } else if decpt <= 0 {
        format!("{sign}0.{}{}", "0".repeat((-decpt) as usize), digits)
    } else if decpt >= n {
        format!("{sign}{}{}.0", digits, "0".repeat((decpt - n) as usize))
    } else {
        let (i, f) = digits.split_at(decpt as usize);
        format!("{sign}{i}.{f}")
    }
}

/// Python `repr(str)` (quote choice and common escapes).
fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `str()` of the value psycopg's json loader yields for a jsonb value
/// (list -> "[1, 2]", None, True/False, floats via repr, dicts with ': ').
pub fn py_repr_json(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else {
                py_float_repr(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        Value::String(s) => py_repr_str(s),
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(py_repr_json).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::Object(map) => {
            let inner: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("{}: {}", py_repr_str(k), py_repr_json(v)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_repr_like_python() {
        let cases = [
            (0.0, "0.0"),
            (-0.0, "-0.0"),
            (1.0, "1.0"),
            (100.0, "100.0"),
            (3.25, "3.25"),
            (0.1, "0.1"),
            (1e16, "1e+16"),
            (1e15, "1000000000000000.0"),
            (1.5e300, "1.5e+300"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (1.2345e-7, "1.2345e-07"),
            (123456789.125, "123456789.125"),
            (-2.5, "-2.5"),
            (1.2345678901234568e17, "1.2345678901234568e+17"),
            (f64::INFINITY, "inf"),
        ];
        for (x, want) in cases {
            assert_eq!(py_float_repr(x), want, "{x}");
        }
    }

    #[test]
    fn json_repr_like_python() {
        let v: serde_json::Value = serde_json::json!([1, 2, 3]);
        assert_eq!(py_repr_json(&v), "[1, 2, 3]");
        assert_eq!(py_repr_json(&serde_json::json!([])), "[]");
        assert_eq!(py_repr_json(&serde_json::json!(["a", null, true, 1.5])), "['a', None, True, 1.5]");
        assert_eq!(py_repr_json(&serde_json::json!("it's")), "\"it's\"");
    }

    #[test]
    fn value_for_search_types() {
        let mut raw = CustomFieldRaw {
            data_type: "boolean".into(),
            value_bool: Some(false),
            ..Default::default()
        };
        assert_eq!(raw.value_for_search().as_deref(), Some("False"));
        raw.data_type = "date".into();
        raw.value_date = NaiveDate::from_ymd_opt(2025, 1, 6);
        assert_eq!(raw.value_for_search().as_deref(), Some("2025-01-06"));
        raw.data_type = "select".into();
        raw.value_select = Some("opt2".into());
        raw.extra_data = Some(serde_json::json!({"select_options": [
            {"id": "opt1", "label": "One"}, {"id": "opt2", "label": "Two"}]}));
        assert_eq!(raw.value_for_search().as_deref(), Some("Two"));
        raw.value_select = Some("missing".into());
        assert_eq!(raw.value_for_search(), None);
        raw.data_type = "documentlink".into();
        raw.value_document_ids = Some(serde_json::json!([4, 7]));
        assert_eq!(raw.value_for_search().as_deref(), Some("[4, 7]"));
        raw.data_type = "integer".into();
        assert_eq!(raw.value_for_search(), None);
    }

    #[test]
    fn version_orderings_differ_on_null_index() {
        let v = vec![
            VersionRow { id: 10, version_index: None, content: "null".into() },
            VersionRow { id: 5, version_index: Some(0), content: "zero".into() },
        ];
        // SQL: NULLS LAST -> index 0 wins; Python: (0,10) > (0,5) -> id 10 wins.
        assert_eq!(newest_version(&v, VersionPick::Sql).unwrap().id, 5);
        assert_eq!(newest_version(&v, VersionPick::Prefetch).unwrap().id, 10);
        assert!(newest_version(&[], VersionPick::Sql).is_none());
    }
}
