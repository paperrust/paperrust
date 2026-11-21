//! Querysets of `TasksViewSet`: base queryset, `PaperlessTaskFilterSet`, ordering.
//!
//! upstream: src/documents/views.py@v3.4.2 (`TasksViewSet.get_queryset`,
//! `get_status_count_queryset`, `ordering_fields`), src/documents/filters.py@v3.4.2
//! (`PaperlessTaskFilterSet`), django-filter 26.2 (`MultipleChoiceFilter`, `BooleanFilter` +
//! `BooleanWidget`, `ModelChoiceFilter`, `DateTimeFilter`, `translate_validation`),
//! rest_framework/filters.py@3.18.3 (`OrderingFilter`).

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use pl_authz::User;
use pl_core::{QueryParams, ValidationErrors};
use sea_query::{Alias, Cond, Condition, Expr, ExprTrait as _, Order, SelectStatement, SimpleExpr};

use crate::labels::{COMPLETE_STATUSES, STATUS, TASK_TYPE, TRIGGER_SOURCE, translate};
use crate::py;

/// Table alias used for `documents_paperlesstask` in all task queries.
pub const T: &str = "t";

pub fn a(s: &str) -> Alias {
    Alias::new(s)
}

pub fn col(c: &str) -> SimpleExpr {
    Expr::col((a(T), a(c))).into()
}

/// `TasksViewSet._V9_TASK_NAME_TO_TYPE`
fn v9_task_name_to_type(name: &str) -> &str {
    match name {
        "check_sanity" => "sanity_check",
        "llmindex_update" => "llm_index",
        other => other,
    }
}

/// `TasksViewSet._V9_TYPE_TO_TRIGGER_SOURCES`
fn v9_type_to_sources(t: &str) -> Option<&'static [&'static str]> {
    match t {
        "auto_task" => Some(&["system", "email_consume", "folder_consume"]),
        "scheduled_task" => Some(&["scheduled"]),
        "manual_task" => Some(&["manual", "web_ui", "api_upload"]),
        _ => None,
    }
}

/// `TasksViewSet.get_queryset()` as a condition on alias `t`.
pub fn base_condition(user: &User, is_v9: bool, q: &QueryParams) -> Condition {
    let mut c = Cond::all();
    if !user.is_staff {
        c = c.add(Cond::any().add(col("owner_id").eq(user.id)).add(col("owner_id").is_null()));
    }
    if is_v9 {
        if let Some(name) = q.get("task_name") {
            c = c.add(col("task_type").eq(v9_task_name_to_type(name)));
        }
        if let Some(sources) = q.get("type").and_then(v9_type_to_sources) {
            c = c.add(col("trigger_source").is_in(sources.iter().copied()));
        }
    }
    if let Some(task_id) = q.get("task_id") {
        c = c.add(col("task_id").eq(task_id));
    }
    c
}

/// Validated `PaperlessTaskFilterSet` values (`None` = filter not applied).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct TaskFilter {
    pub task_type: Vec<String>,
    pub trigger_source: Vec<String>,
    pub status: Vec<String>,
    pub acknowledged: Option<bool>,
    pub owner: Option<String>,
    pub name: Option<String>,
    pub result: Option<String>,
    pub is_complete: Option<bool>,
    pub date_created_after: Option<DateTime<Utc>>,
    pub date_created_before: Option<DateTime<Utc>>,
}

/// django-filter `BooleanWidget.value_from_datadict`.
fn boolean_widget(v: Option<&str>) -> Option<bool> {
    match v.map(str::to_lowercase).as_deref() {
        Some("1" | "true") => Some(true),
        Some("0" | "false") => Some(false),
        _ => None,
    }
}

fn multiple_choice(errors: &mut ValidationErrors, field: &str, values: Vec<&str>, choices: &[(&str, &str)]) -> Vec<String> {
    for v in &values {
        if !choices.iter().any(|(c, _)| c == v) {
            errors.add(field, format!("Select a valid choice. {v} is not one of the available choices."));
            return Vec::new();
        }
    }
    values.into_iter().map(str::to_string).collect()
}

/// Parse and validate the filter parameters in form-field order. `owner` is checked for
/// syntax here; existence (`User.objects.get(pk=...)`) is checked by the caller via
/// [`TaskFilter::owner`] and [`owner_error`].
pub fn parse_filter(q: &QueryParams, lang: &str, tz: &Tz, owner_exists: impl Fn(i64) -> bool) -> Result<TaskFilter, ValidationErrors> {
    let mut e = ValidationErrors::new();
    let mut f = TaskFilter {
        task_type: multiple_choice(&mut e, "task_type", q.get_list("task_type"), TASK_TYPE),
        trigger_source: multiple_choice(&mut e, "trigger_source", q.get_list("trigger_source"), TRIGGER_SOURCE),
        status: multiple_choice(&mut e, "status", q.get_list("status"), STATUS),
        acknowledged: boolean_widget(q.get("acknowledged")),
        ..Default::default()
    };
    match q.get("owner") {
        None | Some("") => {}
        Some(raw) => match py::int(raw) {
            Some(id) if i64::try_from(id).is_ok_and(&owner_exists) => f.owner = Some(id.to_string()),
            _ => e.add("owner", OWNER_INVALID),
        },
    }
    f.name = q.get("name").filter(|v| !v.is_empty()).map(str::to_string);
    f.result = q.get("result").filter(|v| !v.is_empty()).map(str::to_string);
    f.is_complete = boolean_widget(q.get("is_complete"));
    for (field, slot) in [("date_created_after", 0), ("date_created_before", 1)] {
        if let Some(raw) = q.get(field) {
            match pl_core::datetime::parse_datetime_input(raw, lang, tz) {
                Ok(v) => {
                    if slot == 0 {
                        f.date_created_after = v;
                    } else {
                        f.date_created_before = v;
                    }
                }
                Err(msg) => e.add(field, msg),
            }
        }
    }
    if e.is_empty() { Ok(f) } else { Err(e) }
}

/// `ModelChoiceField.invalid_choice`.
pub const OWNER_INVALID: &str = "Select a valid choice. That choice is not one of the available choices.";

/// Owner ids referenced by `?owner=` that must exist (for the existence pre-check).
pub fn owner_candidate(q: &QueryParams) -> Option<i64> {
    q.get("owner").filter(|v| !v.is_empty()).and_then(py::int).and_then(|v| i64::try_from(v).ok())
}

/// Django `icontains` on PostgreSQL: `UPPER(x::text) LIKE UPPER('%<escaped>%')`.
fn icontains(expr: &str, value: &str) -> SimpleExpr {
    let escaped = value.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    // custom SQL: sea-query has no ILIKE-with-UPPER form matching Django's SQL exactly
    Expr::cust_with_values(format!("UPPER(({expr})::text) LIKE UPPER($1)"), [format!("%{escaped}%")])
}

/// The filter as SQL condition on alias `t`.
pub fn filter_condition(f: &TaskFilter, lang: &str) -> Condition {
    let mut c = Cond::all();
    if !f.task_type.is_empty() {
        c = c.add(col("task_type").is_in(f.task_type.clone()));
    }
    if !f.trigger_source.is_empty() {
        c = c.add(col("trigger_source").is_in(f.trigger_source.clone()));
    }
    if !f.status.is_empty() {
        c = c.add(col("status").is_in(f.status.clone()));
    }
    if let Some(b) = f.acknowledged {
        c = c.add(col("acknowledged").eq(b));
    }
    if let Some(id) = f.owner.as_deref().and_then(|o| o.parse::<i64>().ok()) {
        c = c.add(col("owner_id").eq(id));
    }
    if let Some(name) = &f.name {
        let v = name.to_lowercase();
        let types: Vec<&str> = TASK_TYPE.iter().filter(|(_, l)| translate(lang, l).to_lowercase().contains(&v)).map(|(t, _)| *t).collect();
        let sources: Vec<&str> =
            TRIGGER_SOURCE.iter().filter(|(_, l)| translate(lang, l).to_lowercase().contains(&v)).map(|(t, _)| *t).collect();
        c = c.add(
            Cond::any()
                .add(icontains("\"t\".\"input_data\" ->> 'filename'", name))
                .add(col("task_type").is_in(types))
                .add(col("trigger_source").is_in(sources)),
        );
    }
    if let Some(r) = &f.result {
        let mut any = Cond::any()
            .add(icontains("\"t\".\"result_data\" ->> 'reason'", r))
            .add(icontains("\"t\".\"result_data\" ->> 'error_message'", r));
        if let Some(n) = py::int(r) {
            let json = n.to_string();
            // Django KeyTransformExact: (result_data -> 'key') = '<json>'::jsonb
            any = any
                .add(Expr::cust_with_values("(\"t\".\"result_data\" -> 'document_id') = ($1)::jsonb", [json.clone()]))
                .add(Expr::cust_with_values("(\"t\".\"result_data\" -> 'duplicate_of') = ($1)::jsonb", [json]));
        }
        if r.to_lowercase().contains("duplicate") {
            // Django: result_data__duplicate_of__isnull=False → HasKey
            any = any.add(Expr::cust("\"t\".\"result_data\" ? 'duplicate_of'"));
        }
        c = c.add(any);
    }
    if let Some(b) = f.is_complete {
        if b {
            c = c.add(col("status").is_in(COMPLETE_STATUSES.iter().copied()));
        } else {
            c = c.add(col("status").is_not_in(COMPLETE_STATUSES.iter().copied()));
        }
    }
    if let Some(d) = f.date_created_after {
        c = c.add(col("date_created").gte(d));
    }
    if let Some(d) = f.date_created_before {
        c = c.add(col("date_created").lte(d));
    }
    c
}

/// `TasksViewSet.ordering_fields`
pub const ORDERING_FIELDS: &[&str] = &["date_created", "date_done", "status", "task_type", "duration_seconds", "wait_time_seconds"];

/// `OrderingFilter.get_ordering`: valid terms of `?ordering=`, else `["-date_created"]`.
pub fn ordering(q: &QueryParams) -> Vec<String> {
    if let Some(p) = q.get("ordering").filter(|p| !p.is_empty()) {
        let terms: Vec<String> =
            p.split(',').map(|t| t.trim().to_string()).filter(|t| ORDERING_FIELDS.contains(&t.strip_prefix('-').unwrap_or(t))).collect();
        if !terms.is_empty() {
            return terms;
        }
    }
    vec!["-date_created".to_string()]
}

pub fn apply_ordering(sel: &mut SelectStatement, terms: &[String]) {
    for t in terms {
        match t.strip_prefix('-') {
            Some(f) => sel.order_by((a(T), a(f)), Order::Desc),
            None => sel.order_by((a(T), a(t)), Order::Asc),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qp(s: &str) -> QueryParams {
        QueryParams::parse(s)
    }

    #[test]
    fn ordering_like_drf() {
        assert_eq!(ordering(&qp("")), vec!["-date_created"]);
        assert_eq!(ordering(&qp("ordering=")), vec!["-date_created"]);
        assert_eq!(ordering(&qp("ordering=bogus")), vec!["-date_created"]);
        assert_eq!(ordering(&qp("ordering=status,%20-date_done,bogus")), vec!["status", "-date_done"]);
        assert_eq!(ordering(&qp("ordering=--status")), vec!["-date_created"]);
    }

    #[test]
    fn filter_errors_in_field_order() {
        let e = parse_filter(&qp("status=bogus&task_type=x&owner=abc"), "en-us", &chrono_tz::UTC, |_| true).unwrap_err().to_value();
        assert_eq!(
            e.to_string(),
            concat!(
                r#"{"task_type":["Select a valid choice. x is not one of the available choices."],"#,
                r#""status":["Select a valid choice. bogus is not one of the available choices."],"#,
                r#""owner":["Select a valid choice. That choice is not one of the available choices."]}"#
            )
        );
        let e = parse_filter(&qp("status="), "en-us", &chrono_tz::UTC, |_| true).unwrap_err().to_value();
        assert_eq!(e["status"][0], "Select a valid choice.  is not one of the available choices.");
    }

    #[test]
    fn filter_values() {
        let f = parse_filter(
            &qp("status=success&status=failure&acknowledged=TRUE&is_complete=maybe&name=&owner=3"),
            "en-us",
            &chrono_tz::UTC,
            |id| id == 3,
        )
        .unwrap();
        assert_eq!(f.status, vec!["success", "failure"]);
        assert_eq!(f.acknowledged, Some(true));
        assert_eq!(f.is_complete, None);
        assert_eq!(f.name, None);
        assert_eq!(f.owner.as_deref(), Some("3"));
        assert!(parse_filter(&qp("owner=4"), "en-us", &chrono_tz::UTC, |id| id == 3).is_err());
        assert!(parse_filter(&qp("date_created_after=nope"), "en-us", &chrono_tz::UTC, |_| true).is_err());
        let f = parse_filter(&qp("date_created_after=2024-01-02"), "en-us", &chrono_tz::UTC, |_| true).unwrap();
        assert_eq!(f.date_created_after.unwrap().to_rfc3339(), "2024-01-02T00:00:00+00:00");
    }
}
