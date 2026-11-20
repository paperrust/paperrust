//! Filter rules of saved views → v1 query parameters (port of
//! `queryParamsFromFilterRules`, upstream src-ui/src/app/utils/query-params.ts@v3.4.2, rule types
//! from src-ui/src/app/data/filter-rule-type.ts). Only for `sync/resolve` (docs/SYNC.md §3.5).
//!
//! Deviation: full-text rules (`query`, `more_like_id`, `title_search`, `text`) run in upstream
//! via the search index; here they cannot (yet) be evaluated → `not_implemented`. Rule 19
//! (`title_content`, deprecated) is evaluated as a substring filter `title_content`.

use pl_domain::model::FilterRule;
use pl_domain::{DomainError, DomainResult};

/// (id, filtervar, isnull_filtervar, multi, boolean)
const RULES: &[(i64, &str, Option<&str>, bool, bool)] = &[
    (0, "title__icontains", None, false, false),
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

/// Full-text rules (search index): 20 query, 21 more_like_id, 48 title_search, 49 text.
const FULLTEXT: &[i64] = &[20, 21, 48, 49];

/// `NEGATIVE_NULL_FILTER_VALUE`
const NEGATIVE_NULL: &str = "-1";

pub(crate) fn to_params(rules: &[FilterRule]) -> DomainResult<Vec<(String, String)>> {
    let mut params: Vec<(String, String)> = Vec::new();
    let mut set = |k: &str, v: String| {
        if let Some(e) = params.iter_mut().find(|(key, _)| key == k) {
            e.1 = v;
        } else {
            params.push((k.to_string(), v));
        }
    };
    let mut multi: Vec<(String, String)> = Vec::new();
    for r in rules {
        if FULLTEXT.contains(&r.rule_type) {
            return Err(DomainError::NotImplemented(format!(
                "saved view uses full-text rule {} which cannot be resolved yet",
                r.rule_type
            )));
        }
        let Some(&(_, var, isnull, is_multi, boolean)) = RULES.iter().find(|x| x.0 == r.rule_type) else {
            return Err(DomainError::NotImplemented(format!("unknown filter rule type {}", r.rule_type)));
        };
        match (isnull, r.value.as_deref()) {
            (Some(n), None) => set(n, "1".into()),
            (Some(n), Some(NEGATIVE_NULL)) => set(n, "0".into()),
            _ if is_multi => {
                let v = r.value.clone().unwrap_or_default();
                match multi.iter_mut().find(|(k, _)| k == var) {
                    Some(e) => {
                        e.1.push(',');
                        e.1.push_str(&v);
                    }
                    None => multi.push((var.to_string(), v)),
                }
            }
            _ if boolean => {
                let v = r.value.as_deref().unwrap_or_default();
                set(var, if v == "true" || v == "1" { "1".into() } else { "0".into() });
            }
            _ => set(var, r.value.clone().unwrap_or_default()),
        }
    }
    for (k, v) in multi {
        set(&k, v);
    }
    Ok(params)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(t: i64, v: Option<&str>) -> FilterRule {
        FilterRule { rule_type: t, value: v.map(str::to_string) }
    }

    #[test]
    fn like_angular() {
        let p = to_params(&[r(6, Some("1")), r(6, Some("2")), r(3, None), r(7, Some("false")), r(4, Some("-1"))]).unwrap();
        assert_eq!(
            p,
            vec![
                ("correspondent__isnull".into(), "1".into()),
                ("is_tagged".into(), "0".into()),
                ("document_type__isnull".into(), "0".into()),
                ("tags__id__all".into(), "1,2".into()),
            ]
        );
        assert!(matches!(to_params(&[r(20, Some("x"))]), Err(DomainError::NotImplemented(_))));
    }
}
