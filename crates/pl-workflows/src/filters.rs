//! Filter of the workflow list (`GET /api/workflows/?trigger_type=5&enabled=true`).
//!
//! Own implementation following the idea from paperless-ngx dev `3dbeca603` (#14704,
//! `src/documents/filters.py::WorkflowFilterSet`): `id`/`id__in`, `enabled` (exact) and
//! `trigger_type` (workflow has at least one trigger of this type). In v3.4.2
//! `WorkflowViewSet` had no filters; unknown parameters are still ignored.
//!
//! Behaviour like django-filter (via `pl_core::filters`): order of the errors
//! `id__in`, `id`, `enabled`, `trigger_type`; invalid number → 400 `{"param": ["Enter a number."]}`;
//! `enabled` with an unknown value → filter is dropped.

use pl_core::filters::{FieldKind, FilterContext, FilterDef, FilterSet, ID_KWARGS, Lookup, build_condition};
use pl_core::{ApiError, QueryParams, ValidationErrors};
use sea_query::{Alias, Expr, Query, SimpleExpr};

use crate::schema::{T_TRIGGER, T_WF_TRIGGERS, T_WORKFLOW};

fn a(s: &str) -> Alias {
    Alias::new(s)
}

/// Name of the parameter for the trigger type.
pub const TRIGGER_TYPE_PARAM: &str = "trigger_type";

fn workflow_filterset() -> FilterSet {
    FilterSet::new()
        .field("id", Expr::col((a(T_WORKFLOW), a("id"))), FieldKind::INT, ID_KWARGS)
        .field("enabled", Expr::col((a(T_WORKFLOW), a("enabled"))), FieldKind::Boolean, &[Lookup::Exact])
}

/// `trigger_type = NumberFilter(field_name="triggers__type", distinct=True)`: condition on
/// `documents_workflowtrigger.type` (PositiveSmallIntegerField), as a subquery over the
/// join table – hence no duplicate rows (corresponds to `distinct=True`).
fn trigger_type_condition(raw: Option<&str>, ctx: &FilterContext<'_>) -> Result<Option<SimpleExpr>, String> {
    let def = FilterDef {
        param: TRIGGER_TYPE_PARAM.into(),
        expr: Expr::col((a(T_TRIGGER), a("type"))).into(),
        kind: FieldKind::POSITIVE_SMALLINT,
        lookup: Lookup::Exact,
    };
    let Some(cond) = build_condition(&def, raw, ctx)? else { return Ok(None) };
    let sub = Query::select()
        .column((a(T_WF_TRIGGERS), a("workflow_id")))
        .from(a(T_WF_TRIGGERS))
        .inner_join(
            a(T_TRIGGER),
            Expr::col((a(T_TRIGGER), a("id"))).equals((a(T_WF_TRIGGERS), a("workflowtrigger_id"))),
        )
        .and_where(cond)
        .to_owned();
    Ok(Some(Expr::col((a(T_WORKFLOW), a("id"))).in_subquery(sub)))
}

/// All conditions of the workflow list (AND-combined). Errors collected → 400.
pub fn workflow_conditions(q: &QueryParams, ctx: &FilterContext<'_>) -> Result<Vec<SimpleExpr>, ApiError> {
    let fs = workflow_filterset();
    let mut errors = ValidationErrors::new();
    let mut out = Vec::new();
    for d in fs.defs() {
        match build_condition(d, q.get(&d.param), ctx) {
            Ok(Some(c)) => out.push(c),
            Ok(None) => {}
            Err(m) => errors.add(&d.param, m),
        }
    }
    match trigger_type_condition(q.get(TRIGGER_TYPE_PARAM), ctx) {
        Ok(Some(c)) => out.push(c),
        Ok(None) => {}
        Err(m) => errors.add(TRIGGER_TYPE_PARAM, m),
    }
    errors.into_result()?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_query::PostgresQueryBuilder;

    fn tz() -> pl_core::chrono_tz::Tz {
        pl_core::chrono_tz::UTC
    }

    fn sql(conds: &[SimpleExpr]) -> String {
        let mut q = Query::select();
        q.column((a(T_WORKFLOW), a("id"))).from(a(T_WORKFLOW));
        for c in conds {
            q.and_where(c.clone());
        }
        q.to_string(PostgresQueryBuilder)
    }

    fn qp(pairs: &[(&str, &str)]) -> QueryParams {
        QueryParams::from_pairs(pairs.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())))
    }

    #[test]
    fn param_order() {
        assert_eq!(workflow_filterset().params(), vec!["id__in", "id", "enabled"]);
    }

    #[test]
    fn trigger_type_and_enabled() {
        let tz = tz();
        let ctx = FilterContext { tz: &tz, language: "en-us" };
        let c = workflow_conditions(&qp(&[("trigger_type", "5"), ("enabled", "true")]), &ctx).unwrap();
        assert_eq!(c.len(), 2);
        let s = sql(&c);
        assert!(s.contains("\"documents_workflow\".\"enabled\" = TRUE"), "{s}");
        assert!(s.contains("IN (SELECT \"documents_workflow_triggers\".\"workflow_id\""), "{s}");
        assert!(s.contains("\"documents_workflowtrigger\".\"type\" = 5"), "{s}");
    }

    #[test]
    fn empty_and_unknown_values_are_ignored() {
        let tz = tz();
        let ctx = FilterContext { tz: &tz, language: "en-us" };
        let c = workflow_conditions(&qp(&[("trigger_type", ""), ("enabled", "maybe"), ("foo", "1")]), &ctx).unwrap();
        assert!(c.is_empty());
    }

    #[test]
    fn invalid_numbers_collect_errors_in_order() {
        let tz = tz();
        let ctx = FilterContext { tz: &tz, language: "en-us" };
        let err = workflow_conditions(&qp(&[("trigger_type", "x"), ("id", "y")]), &ctx).unwrap_err();
        assert_eq!(err.status(), axum::http::StatusCode::BAD_REQUEST);
        let body = err.body_json().cloned().unwrap();
        let keys: Vec<&String> = body.as_object().unwrap().keys().collect();
        assert_eq!(keys, vec!["id", "trigger_type"]);
        assert_eq!(body["trigger_type"], serde_json::json!(["Enter a number."]));
    }
}
