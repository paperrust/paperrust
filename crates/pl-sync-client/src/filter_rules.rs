//! Saved views → v1 query parameters for paperless-ngx (docs/SYNC.md §7, P6).
//!
//! Port of `queryParamsFromFilterRules` (upstream `src-ui/src/app/utils/query-params.ts`,
//! rule table `src-ui/src/app/data/filter-rule-type.ts`, v3.4.2) – this is exactly how the Angular UI builds
//! the parameters of the document list (`DocumentService.listFiltered`). Order of the parameters as
//! the insertion order of the JS object.

/// A rule type.
#[derive(Debug, Clone, Copy)]
pub struct RuleType {
    pub id: i64,
    pub filtervar: &'static str,
    pub isnull_filtervar: Option<&'static str>,
    pub boolean: bool,
    pub multi: bool,
}

const fn rt(id: i64, filtervar: &'static str, isnull: Option<&'static str>, boolean: bool, multi: bool) -> RuleType {
    RuleType { id, filtervar, isnull_filtervar: isnull, boolean, multi }
}

pub const SIMPLE_TEXT_PARAMETER: &str = "text";
pub const SIMPLE_TITLE_PARAMETER: &str = "title_search";
pub const NEGATIVE_NULL_FILTER_VALUE: &str = "-1";

/// `FILTER_RULE_TYPES` (filter-rule-type.ts, v3.4.2).
pub const RULE_TYPES: &[RuleType] = &[
    rt(0, "title__icontains", None, false, false),
    rt(48, SIMPLE_TITLE_PARAMETER, None, false, false),
    rt(1, "content__icontains", None, false, false),
    rt(2, "archive_serial_number", None, false, false),
    rt(3, "correspondent__id", Some("correspondent__isnull"), false, false),
    rt(26, "correspondent__id__in", None, false, true),
    rt(27, "correspondent__id__none", None, false, true),
    rt(25, "storage_path__id", Some("storage_path__isnull"), false, false),
    rt(30, "storage_path__id__in", None, false, true),
    rt(31, "storage_path__id__none", None, false, true),
    rt(4, "document_type__id", Some("document_type__isnull"), false, false),
    rt(28, "document_type__id__in", None, false, true),
    rt(29, "document_type__id__none", None, false, true),
    rt(5, "is_in_inbox", None, true, false),
    rt(6, "tags__id__all", None, false, true),
    rt(22, "tags__id__in", None, false, true),
    rt(17, "tags__id__none", None, false, true),
    rt(7, "is_tagged", None, true, false),
    rt(8, "created__date__lt", None, false, false),
    rt(9, "created__date__gt", None, false, false),
    rt(43, "created__date__lte", None, false, false),
    rt(44, "created__date__gte", None, false, false),
    rt(10, "created__year", None, false, false),
    rt(11, "created__month", None, false, false),
    rt(12, "created__day", None, false, false),
    rt(13, "added__date__lt", None, false, false),
    rt(14, "added__date__gt", None, false, false),
    rt(45, "added__date__lte", None, false, false),
    rt(46, "added__date__gte", None, false, false),
    rt(15, "modified__date__lt", None, false, false),
    rt(16, "modified__date__gt", None, false, false),
    rt(18, "archive_serial_number__isnull", None, true, false),
    rt(23, "archive_serial_number__gt", None, false, false),
    rt(24, "archive_serial_number__lt", None, false, false),
    rt(19, "title_content", None, false, false),
    rt(49, SIMPLE_TEXT_PARAMETER, None, false, false),
    rt(20, "query", None, false, false),
    rt(21, "more_like_id", None, false, false),
    rt(32, "owner__id", None, false, false),
    rt(33, "owner__id__in", None, false, true),
    rt(34, "owner__isnull", None, true, false),
    rt(35, "owner__id__none", None, false, true),
    rt(37, "shared_by__id", None, false, true),
    rt(36, "custom_fields__icontains", None, false, false),
    rt(38, "custom_fields__id__all", None, false, true),
    rt(39, "custom_fields__id__in", None, false, true),
    rt(40, "custom_fields__id__none", None, false, true),
    rt(41, "has_custom_fields", None, true, false),
    rt(42, "custom_field_query", None, false, false),
    rt(47, "mime_type", None, false, false),
    rt(50, "has_duplicates", None, true, false),
];

/// `queryParamsFromFilterRules(filterRules)` → parameter list. Unknown rule type → error (the
/// view is then not selectable, SYNC.md §7).
pub fn query_params(rules: &[(i64, Option<String>)]) -> Result<Vec<(String, String)>, String> {
    let mut params: Vec<(String, String)> = Vec::new();
    let set = |k: &str, v: String, params: &mut Vec<(String, String)>| match params.iter_mut().find(|(a, _)| a == k) {
        Some(e) => e.1 = v,
        None => params.push((k.to_string(), v)),
    };
    for (rule_type, value) in rules {
        let Some(t) = RULE_TYPES.iter().find(|t| t.id == *rule_type) else {
            return Err(format!("filter rule type {rule_type} has no mapping"));
        };
        match (rule_type, value) {
            (19 | 49, v) => {
                if let Some(v) = v {
                    set(SIMPLE_TEXT_PARAMETER, v.clone(), &mut params);
                }
            }
            (0 | 48, v) => {
                if let Some(v) = v {
                    set(SIMPLE_TITLE_PARAMETER, v.clone(), &mut params);
                }
            }
            (_, None) if t.isnull_filtervar.is_some() => set(t.isnull_filtervar.unwrap_or_default(), "1".into(), &mut params),
            (_, Some(v)) if t.isnull_filtervar.is_some() && v == NEGATIVE_NULL_FILTER_VALUE => {
                set(t.isnull_filtervar.unwrap_or_default(), "0".into(), &mut params)
            }
            (_, Some(v)) if t.multi => {
                let cur = params.iter().find(|(a, _)| a == t.filtervar).map(|(_, x)| x.clone());
                let nv = match cur {
                    Some(c) => format!("{c},{v}"),
                    None => v.clone(),
                };
                set(t.filtervar, nv, &mut params);
            }
            (_, Some(v)) => {
                let v = if t.boolean { if v == "true" || v == "1" { "1".into() } else { "0".into() } } else { v.clone() };
                set(t.filtervar, v, &mut params);
            }
            // JS: params[fv] = null → omitted on serialization
            (_, None) => {}
        }
    }
    Ok(params)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(t: i64, v: &str) -> (i64, Option<String>) {
        (t, Some(v.to_string()))
    }

    // Cases from src-ui/src/app/utils/query-params.spec.ts (v3.4.2, "should convert filter rules to query params" among others)
    #[test]
    fn converts_like_angular() {
        assert_eq!(
            query_params(&[r(6, "1"), r(6, "2"), r(22, "3")]).unwrap(),
            vec![("tags__id__all".into(), "1,2".into()), ("tags__id__in".into(), "3".into())]
        );
        assert_eq!(query_params(&[r(19, "foo")]).unwrap(), vec![("text".to_string(), "foo".to_string())]);
        assert_eq!(query_params(&[r(0, "bar")]).unwrap(), vec![("title_search".to_string(), "bar".to_string())]);
        // isnull: null → 1, -1 → 0
        assert_eq!(query_params(&[(3, None)]).unwrap(), vec![("correspondent__isnull".to_string(), "1".to_string())]);
        assert_eq!(query_params(&[r(3, "-1")]).unwrap(), vec![("correspondent__isnull".to_string(), "0".to_string())]);
        assert_eq!(query_params(&[r(3, "12")]).unwrap(), vec![("correspondent__id".to_string(), "12".to_string())]);
        // Boolean rules
        assert_eq!(query_params(&[r(7, "false")]).unwrap(), vec![("is_tagged".to_string(), "0".to_string())]);
        assert_eq!(query_params(&[r(5, "true")]).unwrap(), vec![("is_in_inbox".to_string(), "1".to_string())]);
        assert_eq!(query_params(&[r(41, "1")]).unwrap(), vec![("has_custom_fields".to_string(), "1".to_string())]);
        // date, CF query unchanged
        assert_eq!(query_params(&[r(44, "2024-01-01")]).unwrap(), vec![("created__date__gte".to_string(), "2024-01-01".to_string())]);
        assert_eq!(
            query_params(&[r(42, "[\"AND\",[[1,\"exists\",true]]]")]).unwrap(),
            vec![("custom_field_query".to_string(), "[\"AND\",[[1,\"exists\",true]]]".to_string())]
        );
        assert!(query_params(&[r(999, "x")]).is_err());
        assert!(query_params(&[]).unwrap().is_empty());
    }
}
