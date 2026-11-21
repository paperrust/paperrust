//! Filter rules of a saved view → v1 query parameters for `GET /api/documents/`.
//! Port of `queryParamsFromFilterRules` (upstream src-ui/src/app/utils/query-params.ts@v3.4.2,
//! rule table from src-ui/src/app/data/filter-rule-type.ts). Permissions are filtered by the server; the
//! rules themselves come from a view visible to the user.

/// (rule_type, filtervar, isnull_filtervar, multi, boolean)
const RULES: &[(i64, &str, Option<&str>, bool, bool)] = &[
    (0, "title__icontains", None, false, false),
    (48, "title_search", None, false, false),
    (1, "content__icontains", None, false, false),
    (2, "archive_serial_number", None, false, false),
    (3, "correspondent__id", Some("correspondent__isnull"), false, false),
    (26, "correspondent__id__in", None, true, false),
    (27, "correspondent__id__none", None, true, false),
    (25, "storage_path__id", Some("storage_path__isnull"), false, false),
    (30, "storage_path__id__in", None, true, false),
    (31, "storage_path__id__none", None, true, false),
    (4, "document_type__id", Some("document_type__isnull"), false, false),
    (28, "document_type__id__in", None, true, false),
    (29, "document_type__id__none", None, true, false),
    (5, "is_in_inbox", None, false, true),
    (6, "tags__id__all", None, true, false),
    (22, "tags__id__in", None, true, false),
    (17, "tags__id__none", None, true, false),
    (7, "is_tagged", None, false, true),
    (8, "created__date__lt", None, false, false),
    (9, "created__date__gt", None, false, false),
    (43, "created__date__lte", None, false, false),
    (44, "created__date__gte", None, false, false),
    (10, "created__year", None, false, false),
    (11, "created__month", None, false, false),
    (12, "created__day", None, false, false),
    (13, "added__date__lt", None, false, false),
    (14, "added__date__gt", None, false, false),
    (45, "added__date__lte", None, false, false),
    (46, "added__date__gte", None, false, false),
    (15, "modified__date__lt", None, false, false),
    (16, "modified__date__gt", None, false, false),
    (18, "archive_serial_number__isnull", None, false, true),
    (23, "archive_serial_number__gt", None, false, false),
    (24, "archive_serial_number__lt", None, false, false),
    (19, "title_content", None, false, false),
    (49, "text", None, false, false),
    (20, "query", None, false, false),
    (21, "more_like_id", None, false, false),
    (32, "owner__id", None, false, false),
    (33, "owner__id__in", None, true, false),
    (34, "owner__isnull", None, false, true),
    (35, "owner__id__none", None, true, false),
    (37, "shared_by__id", None, true, false),
    (36, "custom_fields__icontains", None, false, false),
    (38, "custom_fields__id__all", None, true, false),
    (39, "custom_fields__id__in", None, true, false),
    (40, "custom_fields__id__none", None, true, false),
    (41, "has_custom_fields", None, false, true),
    (42, "custom_field_query", None, false, false),
    (47, "mime_type", None, false, false),
    (50, "has_duplicates", None, false, true),
];

/// `FILTER_TITLE_CONTENT` / `FILTER_SIMPLE_TEXT` → `text`, `FILTER_TITLE` / `FILTER_SIMPLE_TITLE` → `title_search`.
const SIMPLE_TEXT: [i64; 2] = [19, 49];
const SIMPLE_TITLE: [i64; 2] = [0, 48];

/// Query parameters (order of first mention). Unknown rule types → `Err(type)`.
pub fn params_from_rules(rules: &[(i64, Option<String>)]) -> Result<Vec<(String, String)>, i64> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut set = |k: &str, v: String, append: bool| {
        if let Some(e) = out.iter_mut().find(|(x, _)| x == k) {
            if append {
                e.1 = format!("{},{v}", e.1);
            } else {
                e.1 = v;
            }
        } else {
            out.push((k.to_string(), v));
        }
    };
    for (t, value) in rules {
        let Some(&(_, var, isnull, multi, boolean)) = RULES.iter().find(|r| r.0 == *t) else { return Err(*t) };
        let v = value.clone();
        if SIMPLE_TEXT.contains(t) {
            set("text", v.unwrap_or_default(), false);
        } else if SIMPLE_TITLE.contains(t) {
            set("title_search", v.unwrap_or_default(), false);
        } else if let (Some(n), None) = (isnull, &v) {
            set(n, "1".into(), false);
        } else if let (Some(n), Some("-1")) = (isnull, v.as_deref()) {
            set(n, "0".into(), false);
        } else if multi {
            set(var, v.unwrap_or_default(), true);
        } else if boolean {
            let b = matches!(v.as_deref(), Some("true") | Some("1"));
            set(var, if b { "1" } else { "0" }.into(), false);
        } else {
            set(var, v.unwrap_or_else(|| "null".into()), false);
        }
    }
    Ok(out)
}

/// `ordering` from `sort_field`/`sort_reverse` (default like the UI: `-created`).
pub fn ordering(sort_field: Option<&str>, reverse: bool) -> String {
    let f = sort_field.filter(|f| !f.is_empty() && f.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).unwrap_or("created");
    if reverse { format!("-{f}") } else { f.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(t: i64, v: Option<&str>) -> (i64, Option<String>) {
        (t, v.map(str::to_string))
    }

    #[test]
    fn like_upstream_frontend() {
        let p = params_from_rules(&[
            r(6, Some("1")),
            r(6, Some("2")),
            r(3, None),
            r(19, Some("rechnung")),
            r(5, Some("true")),
            r(4, Some("-1")),
        ])
        .unwrap();
        assert_eq!(
            p,
            vec![
                ("tags__id__all".to_string(), "1,2".to_string()),
                ("correspondent__isnull".into(), "1".into()),
                ("text".into(), "rechnung".into()),
                ("is_in_inbox".into(), "1".into()),
                ("document_type__isnull".into(), "0".into()),
            ]
        );
        assert_eq!(params_from_rules(&[r(999, None)]), Err(999));
        assert_eq!(ordering(Some("added"), true), "-added");
        assert_eq!(ordering(Some("x;drop"), false), "created");
    }
}
