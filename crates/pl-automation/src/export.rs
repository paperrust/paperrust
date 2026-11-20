//! Way back to paperless-ngx: express PaperRust extensions as ngx workflows (contract "Export").
//!
//! * "Else" branch → own workflow with **inverted condition**, as far as ngx can express it:
//!   per trigger type exactly one trigger with exactly one invertible condition (tags any/none,
//!   "all tags" with one tag, correspondent/type/storage path is/any/none, sources on
//!   ingestion). Otherwise: warning, no workflow.
//! * AI condition → not expressible in ngx: warning (the main workflow runs in ngx without it).
//!
//! ngx combines conditions of a trigger with AND; ¬(A ∧ B) cannot be represented without OR, hence
//! the restriction to one condition.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use pl_consumer::workflow_ext;
use pl_core::AppState;
use serde_json::{Map, Value, json};

use crate::common::{VIEW_WORKFLOW, Who};
use crate::error::{AErr, json as json_resp};

const SOURCES_ALL: [i64; 4] = [1, 2, 3, 4];

/// Result of converting a workflow.
#[derive(Debug, Default, PartialEq)]
pub struct Converted {
    pub else_workflow: Option<Value>,
    pub warnings: Vec<(&'static str, String)>,
}

fn is_empty(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        _ => false,
    }
}

fn ids(v: Option<&Value>) -> Vec<Value> {
    match v {
        Some(Value::Array(a)) => a.clone(),
        Some(v @ Value::Number(_)) => vec![v.clone()],
        _ => Vec::new(),
    }
}

/// Invertible condition of a trigger: (field, value) of the inverted condition, or reason why not.
pub fn invert_trigger(t: &Map<String, Value>) -> Result<Option<(String, Value)>, String> {
    let ty = t.get("type").and_then(Value::as_i64).unwrap_or(0);
    let mut active: Vec<(String, Value)> = Vec::new();
    let mut blocked: Vec<&str> = Vec::new();
    if ty == 1 {
        let src: Vec<i64> = ids(t.get("sources")).iter().filter_map(Value::as_i64).collect();
        if !src.is_empty() && src.len() < 4 {
            let rest: Vec<i64> = SOURCES_ALL.iter().copied().filter(|s| !src.contains(s)).collect();
            active.push(("sources".into(), json!(rest)));
        }
        for f in ["filter_filename", "filter_path", "filter_mailrule"] {
            if !is_empty(t.get(f)) {
                blocked.push(f);
            }
        }
    } else {
        for f in ["filter_filename", "filter_path", "filter_custom_field_query"] {
            if !is_empty(t.get(f)) {
                blocked.push(f);
            }
        }
        if t.get("matching_algorithm").and_then(Value::as_i64).unwrap_or(0) != 0 {
            blocked.push("match");
        }
        let pairs: [(&str, &str); 12] = [
            ("filter_has_tags", "filter_has_not_tags"),
            ("filter_has_not_tags", "filter_has_tags"),
            ("filter_has_all_tags", "filter_has_not_tags"),
            ("filter_has_correspondent", "filter_has_not_correspondents"),
            ("filter_has_any_correspondents", "filter_has_not_correspondents"),
            ("filter_has_not_correspondents", "filter_has_any_correspondents"),
            ("filter_has_document_type", "filter_has_not_document_types"),
            ("filter_has_any_document_types", "filter_has_not_document_types"),
            ("filter_has_not_document_types", "filter_has_any_document_types"),
            ("filter_has_storage_path", "filter_has_not_storage_paths"),
            ("filter_has_any_storage_paths", "filter_has_not_storage_paths"),
            ("filter_has_not_storage_paths", "filter_has_any_storage_paths"),
        ];
        for (f, inv) in pairs {
            let v = ids(t.get(f));
            if v.is_empty() {
                continue;
            }
            if f == "filter_has_all_tags" && v.len() > 1 {
                blocked.push(f);
                continue;
            }
            active.push((inv.into(), Value::Array(v)));
        }
    }
    if !blocked.is_empty() {
        return Err(format!("condition(s) {} cannot be inverted in paperless-ngx", blocked.join(", ")));
    }
    match active.len() {
        0 => Ok(None),
        1 => Ok(active.pop()),
        _ => Err("more than one condition; paperless-ngx cannot express \"not (A and B)\"".into()),
    }
}

fn strip_ids(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut o = Map::new();
            for (k, x) in m {
                if k == "id" {
                    continue;
                }
                o.insert(k.clone(), strip_ids(x));
            }
            Value::Object(o)
        }
        other => other.clone(),
    }
}

/// Skeleton of a trigger (type, schedule) without conditions.
fn bare_trigger(t: &Map<String, Value>) -> Map<String, Value> {
    let mut o = Map::new();
    for k in ["type", "schedule_offset_days", "schedule_is_recurring", "schedule_recurring_interval_days", "schedule_date_field", "schedule_date_custom_field"] {
        if let Some(v) = t.get(k) {
            o.insert(k.into(), v.clone());
        }
    }
    o.insert("matching_algorithm".into(), json!(0));
    o.insert("match".into(), json!(""));
    o.insert("is_insensitive".into(), json!(true));
    if t.get("type").and_then(Value::as_i64) == Some(1) {
        o.insert("sources".into(), json!(SOURCES_ALL));
    }
    o
}

/// Convert a workflow (ngx representation) with extensions for ngx.
pub fn convert(workflow: &Value, else_actions: &[Value], has_ai: bool) -> Converted {
    let mut out = Converted::default();
    let name = workflow.get("name").and_then(Value::as_str).unwrap_or("");
    if has_ai {
        out.warnings.push(("ai_condition_dropped", "The AI condition cannot be expressed in paperless-ngx; the workflow will run without it.".into()));
    }
    if else_actions.is_empty() {
        return out;
    }
    let triggers: Vec<&Map<String, Value>> = workflow.get("triggers").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_object).collect()).unwrap_or_default();
    let mut types: Vec<i64> = triggers.iter().filter_map(|t| t.get("type").and_then(Value::as_i64)).collect();
    types.sort_unstable();
    types.dedup();
    let mut new_triggers = Vec::new();
    for ty in types {
        let of_type: Vec<&&Map<String, Value>> = triggers.iter().filter(|t| t.get("type").and_then(Value::as_i64) == Some(ty)).collect();
        if of_type.len() != 1 {
            out.warnings.push(("else_not_expressible", format!("Else branch: several triggers of type {ty}; paperless-ngx cannot express the inverse.")));
            return out;
        }
        match invert_trigger(of_type[0]) {
            Ok(Some((field, value))) => {
                let mut t = bare_trigger(of_type[0]);
                t.insert(field, value);
                new_triggers.push(Value::Object(t));
            }
            Ok(None) => {
                // without conditions the main branch always matches → else never runs for this type
            }
            Err(why) => {
                out.warnings.push(("else_not_expressible", format!("Else branch: {why}.")));
                return out;
            }
        }
    }
    if new_triggers.is_empty() {
        out.warnings.push(("else_never_runs", "Else branch never runs (the workflow has no conditions); not exported.".into()));
        return out;
    }
    if has_ai {
        out.warnings.push(("else_ai_part_dropped", "Else branch: the part \"AI says no\" cannot be expressed in paperless-ngx.".into()));
    }
    let mut w = Map::new();
    w.insert("name".into(), json!(format!("{name} (else)").chars().take(256).collect::<String>()));
    w.insert("order".into(), workflow.get("order").cloned().unwrap_or(json!(0)));
    w.insert("enabled".into(), workflow.get("enabled").cloned().unwrap_or(json!(true)));
    w.insert("triggers".into(), Value::Array(new_triggers));
    w.insert("actions".into(), Value::Array(else_actions.iter().map(strip_ids).collect()));
    out.else_workflow = Some(Value::Object(w));
    out
}

/// GET /export-ngx – conversion plan for all workflows with extensions.
pub async fn plan(State(st): State<AppState>, who: Who) -> Result<Response, AErr> {
    who.require(&[VIEW_WORKFLOW])?;
    workflow_ext::ensure_pool(st.db()).await?;
    let mut conn = st.db().acquire().await?;
    let ids: Vec<i32> = sqlx::query_scalar(
        "SELECT w.id FROM documents_workflow w WHERE EXISTS (SELECT 1 FROM paperrust_workflow_else_action e WHERE e.workflow_id = w.id) \
         OR EXISTS (SELECT 1 FROM paperrust_workflow_ext x WHERE x.workflow_id = w.id AND x.ai_condition IS NOT NULL) ORDER BY w.\"order\", w.id",
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut workflows = Vec::new();
    let mut warnings = Vec::new();
    for id in ids {
        let Some(l) = crate::workflows::load(&mut conn, i64::from(id)).await? else { continue };
        let c = convert(&l.workflow, &l.else_actions, l.ai.is_some());
        for (code, message) in c.warnings {
            warnings.push(json!({"workflow_id": id, "code": code, "message": message}));
        }
        if let Some(w) = c.else_workflow {
            workflows.push(json!({"source_workflow_id": id, "workflow": w}));
        }
    }
    Ok(json_resp(StatusCode::OK, &json!({"workflows": workflows, "warnings": warnings})))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn inversions() {
        assert_eq!(invert_trigger(&obj(json!({"type": 2, "filter_has_tags": [1, 2]}))).unwrap(), Some(("filter_has_not_tags".into(), json!([1, 2]))));
        assert_eq!(invert_trigger(&obj(json!({"type": 3, "filter_has_correspondent": 5}))).unwrap(), Some(("filter_has_not_correspondents".into(), json!([5]))));
        assert_eq!(invert_trigger(&obj(json!({"type": 3, "filter_has_not_document_types": [4]}))).unwrap(), Some(("filter_has_any_document_types".into(), json!([4]))));
        assert_eq!(invert_trigger(&obj(json!({"type": 2, "filter_has_all_tags": [1]}))).unwrap(), Some(("filter_has_not_tags".into(), json!([1]))));
        assert!(invert_trigger(&obj(json!({"type": 2, "filter_has_all_tags": [1, 2]}))).is_err());
        assert!(invert_trigger(&obj(json!({"type": 2, "filter_has_tags": [1], "filter_has_correspondent": 2}))).is_err());
        assert!(invert_trigger(&obj(json!({"type": 2, "filter_filename": "*.pdf"}))).is_err());
        assert!(invert_trigger(&obj(json!({"type": 2, "matching_algorithm": 1, "match": "x"}))).is_err());
        assert_eq!(invert_trigger(&obj(json!({"type": 1, "sources": [3]}))).unwrap(), Some(("sources".into(), json!([1, 2, 4]))));
        assert!(invert_trigger(&obj(json!({"type": 1, "sources": [3], "filter_mailrule": 2}))).is_err());
        assert_eq!(invert_trigger(&obj(json!({"type": 2}))).unwrap(), None);
    }

    #[test]
    fn convert_else_workflow() {
        let wf = json!({"name": "Invoices", "order": 3, "enabled": true,
            "triggers": [{"id": 1, "type": 2, "filter_has_tags": [7], "matching_algorithm": 0}], "actions": []});
        let els = vec![json!({"id": 9, "type": 1, "assign_tags": [8], "email": {"id": 4, "to": "x"}})];
        let c = convert(&wf, &els, false);
        let w = c.else_workflow.unwrap();
        assert_eq!(w["name"], "Invoices (else)");
        assert_eq!(w["triggers"][0]["filter_has_not_tags"], json!([7]));
        assert!(w["triggers"][0].get("id").is_none());
        assert!(w["actions"][0].get("id").is_none());
        assert!(w["actions"][0]["email"].get("id").is_none());
        assert!(c.warnings.is_empty());
        // not expressible → warning, no workflow
        let wf2 = json!({"name": "x", "triggers": [{"type": 2, "filter_filename": "a*"}]});
        let c = convert(&wf2, &els, true);
        assert!(c.else_workflow.is_none());
        let codes: Vec<&str> = c.warnings.iter().map(|w| w.0).collect();
        assert_eq!(codes, vec!["ai_condition_dropped", "else_not_expressible"]);
        // without conditions else never runs
        let wf3 = json!({"name": "x", "triggers": [{"type": 3}]});
        assert_eq!(convert(&wf3, &els, false).warnings[0].0, "else_never_runs");
    }
}
