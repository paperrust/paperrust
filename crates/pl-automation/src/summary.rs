//! Structured summary of a workflow (contract "Kurzform"): triggers, conditions, AI condition,
//! actions and else actions with resolved names. The UI builds the sentence from it in the
//! language of the user (i18n stays in the client). The source is the ngx representation of
//! `/api/workflows/{id}/` (pl-workflows `load::render_workflows`) so that every option is covered.
//! Passwords (action "remove password"), email texts and webhook headers/bodies never appear.

use pl_authz::User;
use serde_json::{Map, Value, json};
use sqlx::PgConnection;

use crate::common::{NameKind, Names};
use crate::error::AResult;

pub fn trigger_key(t: i64) -> &'static str {
    match t {
        1 => "consumption",
        2 => "document_added",
        3 => "document_updated",
        4 => "scheduled",
        5 => "manual",
        _ => "unknown",
    }
}

pub fn action_key(t: i64) -> &'static str {
    match t {
        1 => "assignment",
        2 => "removal",
        3 => "email",
        4 => "webhook",
        5 => "password_removal",
        6 => "move_to_trash",
        7 => "remote_ocr",
        8 => "ai_suggestions",
        _ => "unknown",
    }
}

/// (ngx field, condition kind, object kind, list?)
const TRIGGER_REFS: &[(&str, &str, NameKind, bool)] = &[
    ("filter_has_tags", "tags_any", NameKind::Tag, true),
    ("filter_has_all_tags", "tags_all", NameKind::Tag, true),
    ("filter_has_not_tags", "tags_none", NameKind::Tag, true),
    ("filter_has_correspondent", "correspondent_is", NameKind::Correspondent, false),
    ("filter_has_any_correspondents", "correspondent_any", NameKind::Correspondent, true),
    ("filter_has_not_correspondents", "correspondent_none", NameKind::Correspondent, true),
    ("filter_has_document_type", "document_type_is", NameKind::DocumentType, false),
    ("filter_has_any_document_types", "document_type_any", NameKind::DocumentType, true),
    ("filter_has_not_document_types", "document_type_none", NameKind::DocumentType, true),
    ("filter_has_storage_path", "storage_path_is", NameKind::StoragePath, false),
    ("filter_has_any_storage_paths", "storage_path_any", NameKind::StoragePath, true),
    ("filter_has_not_storage_paths", "storage_path_none", NameKind::StoragePath, true),
    ("filter_mailrule", "mail_rule", NameKind::MailRule, false),
];

/// (ngx field, object kind, list?) of the actions.
const ACTION_REFS: &[(&str, NameKind, bool)] = &[
    ("assign_tags", NameKind::Tag, true),
    ("assign_correspondent", NameKind::Correspondent, false),
    ("assign_document_type", NameKind::DocumentType, false),
    ("assign_storage_path", NameKind::StoragePath, false),
    ("assign_owner", NameKind::User, false),
    ("assign_view_users", NameKind::User, true),
    ("assign_view_groups", NameKind::Group, true),
    ("assign_change_users", NameKind::User, true),
    ("assign_change_groups", NameKind::Group, true),
    ("assign_custom_fields", NameKind::CustomField, true),
    ("remove_tags", NameKind::Tag, true),
    ("remove_correspondents", NameKind::Correspondent, true),
    ("remove_document_types", NameKind::DocumentType, true),
    ("remove_storage_paths", NameKind::StoragePath, true),
    ("remove_owners", NameKind::User, true),
    ("remove_view_users", NameKind::User, true),
    ("remove_view_groups", NameKind::Group, true),
    ("remove_change_users", NameKind::User, true),
    ("remove_change_groups", NameKind::Group, true),
    ("remove_custom_fields", NameKind::CustomField, true),
];

const REMOVE_ALL: &[&str] = &[
    "remove_all_tags",
    "remove_all_correspondents",
    "remove_all_document_types",
    "remove_all_storage_paths",
    "remove_all_owners",
    "remove_all_permissions",
    "remove_all_custom_fields",
];

fn ids_of(v: Option<&Value>, list: bool) -> Vec<i64> {
    match (v, list) {
        (Some(Value::Array(a)), true) => a.iter().filter_map(Value::as_i64).collect(),
        (Some(v), false) => v.as_i64().into_iter().collect(),
        _ => Vec::new(),
    }
}

fn objects(names: &Names, k: NameKind, ids: &[i64]) -> Value {
    Value::Array(ids.iter().map(|id| json!({"id": id, "name": names.get(k, *id)})).collect())
}

/// Reserve all object ids of an ngx representation.
pub fn collect(names: &mut Names, workflow: &Value, else_actions: &[Value]) {
    if let Some(ts) = workflow.get("triggers").and_then(Value::as_array) {
        for t in ts {
            for (field, _, k, list) in TRIGGER_REFS {
                for id in ids_of(t.get(*field), *list) {
                    names.want(*k, id);
                }
            }
            if let Some(cf) = t.get("schedule_date_custom_field").and_then(Value::as_i64) {
                names.want(NameKind::CustomField, cf);
            }
        }
    }
    let acts = workflow.get("actions").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    for a in acts.iter().chain(else_actions) {
        for (field, k, list) in ACTION_REFS {
            for id in ids_of(a.get(*field), *list) {
                names.want(*k, id);
            }
        }
    }
}

fn s_nonempty(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// Summary of a trigger.
pub fn trigger(names: &Names, t: &Value) -> Value {
    let ty = t.get("type").and_then(Value::as_i64).unwrap_or(0);
    let mut conds: Vec<Value> = Vec::new();
    if ty == 1 {
        let sources: Vec<i64> = ids_of(t.get("sources"), true);
        if !sources.is_empty() && sources.len() < 4 {
            conds.push(json!({"kind": "sources", "values": sources}));
        }
    }
    if let Some(v) = s_nonempty(t.get("filter_filename")) {
        conds.push(json!({"kind": "filename", "value": v}));
    }
    if let Some(v) = s_nonempty(t.get("filter_path")) {
        conds.push(json!({"kind": "path", "value": v}));
    }
    let algo = t.get("matching_algorithm").and_then(Value::as_i64).unwrap_or(0);
    if algo != 0 && ty != 1 {
        conds.push(json!({
            "kind": "content_match",
            "algorithm": algo,
            "value": t.get("match").cloned().unwrap_or(Value::Null),
            "insensitive": t.get("is_insensitive").cloned().unwrap_or(Value::Bool(true)),
        }));
    }
    for (field, kind, k, list) in TRIGGER_REFS {
        let ids = ids_of(t.get(*field), *list);
        if ids.is_empty() {
            continue;
        }
        if *list {
            conds.push(json!({"kind": kind, "objects": objects(names, *k, &ids)}));
        } else {
            conds.push(json!({"kind": kind, "object": {"id": ids[0], "name": names.get(*k, ids[0])}}));
        }
    }
    if let Some(q) = s_nonempty(t.get("filter_custom_field_query")) {
        conds.push(json!({"kind": "custom_field_query", "value": q}));
    }
    let schedule = if ty == 4 {
        let cf = t.get("schedule_date_custom_field").and_then(Value::as_i64);
        json!({
            "offset_days": t.get("schedule_offset_days").cloned().unwrap_or(Value::Null),
            "is_recurring": t.get("schedule_is_recurring").cloned().unwrap_or(Value::Null),
            "recurring_interval_days": t.get("schedule_recurring_interval_days").cloned().unwrap_or(Value::Null),
            "date_field": t.get("schedule_date_field").cloned().unwrap_or(Value::Null),
            "custom_field": cf.map(|id| json!({"id": id, "name": names.get(NameKind::CustomField, id)})),
        })
    } else {
        Value::Null
    };
    json!({
        "id": t.get("id").cloned().unwrap_or(Value::Null),
        "type": ty,
        "type_key": trigger_key(ty),
        "conditions": conds,
        "schedule": schedule,
    })
}

/// Summary of an action.
pub fn action(names: &Names, a: &Value) -> Value {
    let ty = a.get("type").and_then(Value::as_i64).unwrap_or(0);
    let mut items: Vec<Value> = Vec::new();
    match ty {
        1 => {
            if let Some(t) = s_nonempty(a.get("assign_title")) {
                items.push(json!({"kind": "assign_title", "value": t}));
            }
        }
        2 => {
            for f in REMOVE_ALL {
                if a.get(*f).and_then(Value::as_bool) == Some(true) {
                    items.push(json!({"kind": f}));
                }
            }
        }
        3 => {
            let e = a.get("email").cloned().unwrap_or(Value::Null);
            items.push(json!({
                "kind": "email",
                "to": e.get("to").cloned().unwrap_or(Value::Null),
                "subject": e.get("subject").cloned().unwrap_or(Value::Null),
                "include_document": e.get("include_document").cloned().unwrap_or(Value::Null),
            }));
        }
        4 => {
            let w = a.get("webhook").cloned().unwrap_or(Value::Null);
            items.push(json!({
                "kind": "webhook",
                "url": w.get("url").cloned().unwrap_or(Value::Null),
                "as_json": w.get("as_json").cloned().unwrap_or(Value::Null),
                "include_document": w.get("include_document").cloned().unwrap_or(Value::Null),
            }));
        }
        8 => {
            items.push(json!({
                "kind": "ai_suggestions",
                "fields": a.get("ai_suggestion_fields").cloned().unwrap_or(Value::Null),
                "create_missing": a.get("ai_create_missing").cloned().unwrap_or(Value::Null),
                "overwrite_existing": a.get("ai_overwrite_existing").cloned().unwrap_or(Value::Null),
            }));
        }
        _ => {}
    }
    if ty == 1 || ty == 2 {
        for (field, k, list) in ACTION_REFS {
            if (ty == 1) != field.starts_with("assign_") {
                continue;
            }
            let ids = ids_of(a.get(*field), *list);
            if ids.is_empty() {
                continue;
            }
            let mut item = Map::new();
            item.insert("kind".into(), json!(field));
            if *list {
                item.insert("objects".into(), objects(names, *k, &ids));
            } else {
                item.insert("object".into(), json!({"id": ids[0], "name": names.get(*k, ids[0])}));
            }
            if *field == "assign_custom_fields" {
                item.insert("values".into(), a.get("assign_custom_fields_values").cloned().unwrap_or(Value::Null));
            }
            items.push(Value::Object(item));
        }
    }
    json!({
        "id": a.get("id").cloned().unwrap_or(Value::Null),
        "type": ty,
        "type_key": action_key(ty),
        "items": items,
    })
}

/// Status of the AI condition for display.
pub async fn ai_status() -> (bool, bool) {
    match pl_consumer::workflow_ext::ai_judge() {
        Some(j) => (true, j.enabled().await),
        None => (false, false),
    }
}

/// Summary of a workflow (names already resolved).
pub fn build(
    names: &Names,
    workflow: &Value,
    else_actions: &[Value],
    ai: Option<&pl_consumer::workflow_ext::AiConditionDef>,
    ai_state: (bool, bool),
) -> Value {
    let triggers: Vec<Value> =
        workflow.get("triggers").and_then(Value::as_array).map(|ts| ts.iter().map(|t| trigger(names, t)).collect()).unwrap_or_default();
    let mut types: Vec<i64> = triggers.iter().filter_map(|t| t.get("type").and_then(Value::as_i64)).collect();
    types.sort_unstable();
    types.dedup();
    let actions: Vec<Value> =
        workflow.get("actions").and_then(Value::as_array).map(|xs| xs.iter().map(|a| action(names, a)).collect()).unwrap_or_default();
    let else_v: Vec<Value> = else_actions.iter().map(|a| action(names, a)).collect();
    let ai_v = ai.map(|c| {
        let kind = match c.field.as_str() {
            "document_type" => NameKind::DocumentType,
            "correspondent" => NameKind::Correspondent,
            "storage_path" => NameKind::StoragePath,
            _ => NameKind::Tag,
        };
        let only_consumption = !types.is_empty() && types.iter().all(|t| *t == 1);
        let inactive_reason = if !ai_state.0 {
            Some("ai_unavailable")
        } else if !ai_state.1 {
            Some("ai_disabled")
        } else if only_consumption {
            Some("consumption_only")
        } else {
            None
        };
        json!({
            "field": c.field,
            "object": {"id": c.object_id, "name": names.get(kind, c.object_id)},
            "min_confidence": c.min_confidence,
            "active": inactive_reason.is_none(),
            "inactive_reason": inactive_reason,
        })
    });
    json!({
        "trigger_types": types,
        "triggers": triggers,
        "ai_condition": ai_v,
        "actions": actions,
        "else_actions": else_v,
    })
}

/// Reserve names for the AI condition.
pub fn collect_ai(names: &mut Names, ai: Option<&pl_consumer::workflow_ext::AiConditionDef>) {
    if let Some(c) = ai {
        let kind = match c.field.as_str() {
            "document_type" => NameKind::DocumentType,
            "correspondent" => NameKind::Correspondent,
            "storage_path" => NameKind::StoragePath,
            _ => NameKind::Tag,
        };
        names.want(kind, c.object_id);
    }
}

/// Summary with name resolution (one query per object kind).
pub async fn summarize(
    conn: &mut PgConnection,
    user: &User,
    workflow: &Value,
    else_actions: &[Value],
    ai: Option<&pl_consumer::workflow_ext::AiConditionDef>,
) -> AResult<Value> {
    let mut names = Names::default();
    collect(&mut names, workflow, else_actions);
    collect_ai(&mut names, ai);
    names.resolve(conn, user).await?;
    Ok(build(&names, workflow, else_actions, ai, ai_status().await))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_and_action_shapes() {
        let names = Names::default();
        let t = json!({"id": 1, "type": 2, "sources": [1, 2, 3], "filter_filename": "*.pdf", "filter_path": null,
            "matching_algorithm": 1, "match": "invoice", "is_insensitive": true, "filter_has_tags": [4, 5],
            "filter_has_correspondent": 7, "filter_custom_field_query": null});
        let s = trigger(&names, &t);
        assert_eq!(s["type_key"], "document_added");
        let kinds: Vec<&str> = s["conditions"].as_array().unwrap().iter().map(|c| c["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, vec!["filename", "content_match", "tags_any", "correspondent_is"]);
        assert_eq!(s["conditions"][2]["objects"][1]["id"], 5);
        // Ingestion: sources as condition, no content matching
        let c = trigger(&names, &json!({"type": 1, "sources": [3], "matching_algorithm": 1, "match": "x"}));
        assert_eq!(c["conditions"][0]["kind"], "sources");
        assert_eq!(c["conditions"].as_array().unwrap().len(), 1);

        let a = action(
            &names,
            &json!({"id": 9, "type": 1, "assign_title": "T {{ title }}", "assign_tags": [1],
            "assign_custom_fields": [3], "assign_custom_fields_values": {"3": "x"}, "remove_tags": [2]}),
        );
        let kinds: Vec<&str> = a["items"].as_array().unwrap().iter().map(|c| c["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, vec!["assign_title", "assign_tags", "assign_custom_fields"]);
        assert_eq!(a["items"][2]["values"]["3"], "x");
        // Passwords never appear
        let p = action(&names, &json!({"type": 5, "passwords": ["secret"]}));
        assert!(!p.to_string().contains("secret"));
        let e = action(
            &names,
            &json!({"type": 3, "email": {"to": "a@b.invalid", "subject": "S", "body": "secret body", "include_document": true}}),
        );
        assert_eq!(e["items"][0]["to"], "a@b.invalid");
        assert!(!e.to_string().contains("secret body"));
    }

    #[test]
    fn ai_condition_marks_inactive() {
        let names = Names::default();
        let c = pl_consumer::workflow_ext::AiConditionDef { field: "document_type".into(), object_id: 2, min_confidence: 0.9 };
        let wf = json!({"triggers": [{"type": 2}], "actions": []});
        let s = build(&names, &wf, &[], Some(&c), (false, false));
        assert_eq!(s["ai_condition"]["active"], false);
        assert_eq!(s["ai_condition"]["inactive_reason"], "ai_unavailable");
        let s = build(&names, &wf, &[], Some(&c), (true, true));
        assert_eq!(s["ai_condition"]["active"], true);
        let wf1 = json!({"triggers": [{"type": 1}], "actions": []});
        let s = build(&names, &wf1, &[], Some(&c), (true, true));
        assert_eq!(s["ai_condition"]["inactive_reason"], "consumption_only");
    }
}
