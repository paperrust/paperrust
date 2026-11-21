//! Building blocks for the workflow rework (PaperRust extension, docs/WORKFLOWS-NEU.md):
//!
//! 1. **AI condition** – [`AiVerdict`] (what the AI thinks about a document, per field with confidence)
//!    and [`AiCondition::matches`] ("Only if the AI thinks: invoice (certain)"). The verdict is supplied by
//!    `server::assist::ai_verdict` (classification + matching, without changing anything).
//! 2. **Workflow from description** – [`workflow_prompt`] + [`draft_schema`] for the model,
//!    [`sanitize_draft`] turns it into a draft in the paperless-ngx format (`/api/workflows/`:
//!    `name`, `order`, `enabled`, `triggers[]`, `actions[]`). Only known ids, only a safe
//!    subset of triggers/actions; nothing is stored. `server::assist::workflow_draft`
//!    calls the model.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::auto::{Field, Plan};

/// Threshold "certain" (equal to the default of `PAPERRUST_AI_AUTO_MIN_CONFIDENCE`).
pub const SURE: f64 = 0.9;

/// An assessment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Opinion {
    /// Existing object (or `None` for a new name / title / date).
    pub id: Option<i64>,
    /// Name, title or date.
    pub value: Option<String>,
    pub confidence: f64,
    /// `confidence >= SURE`.
    pub sure: bool,
}

/// What the AI thinks about a document.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AiVerdict {
    pub document: i64,
    pub title: Option<Opinion>,
    pub created: Option<Opinion>,
    pub document_type: Option<Opinion>,
    pub correspondent: Option<Opinion>,
    pub storage_path: Option<Opinion>,
    pub tags: Vec<Opinion>,
}

impl AiVerdict {
    /// From a plan in "suggest" mode (confidences from [`crate::auto::plan`]).
    pub fn from_plan(document: i64, plan: &Plan) -> Self {
        let mut v = AiVerdict { document, ..Default::default() };
        for c in &plan.changes {
            let o = Opinion {
                id: c.existing_id,
                value: c.value.clone().or_else(|| c.new_name.clone()),
                confidence: c.confidence,
                sure: c.confidence >= SURE,
            };
            match c.field {
                Field::Title => v.title = v.title.take().or(Some(o)),
                Field::Created => v.created = v.created.take().or(Some(o)),
                Field::DocumentType => v.document_type = v.document_type.take().or(Some(o)),
                Field::Correspondent => v.correspondent = v.correspondent.take().or(Some(o)),
                Field::StoragePath => v.storage_path = v.storage_path.take().or(Some(o)),
                Field::Tags => v.tags.push(o),
                Field::CustomFields => {}
            }
        }
        v
    }
}

/// Condition of a workflow: "the AI thinks `field` = `object_id` with at least `min_confidence`".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiCondition {
    /// `document_type` | `correspondent` | `storage_path` | `tags`.
    pub field: Field,
    pub object_id: i64,
    /// Default [`SURE`] ("certain"); 0.5 = "probably".
    #[serde(default = "default_min")]
    pub min_confidence: f64,
}

fn default_min() -> f64 {
    SURE
}

impl AiCondition {
    pub fn matches(&self, v: &AiVerdict) -> bool {
        let ok = |o: &Opinion| o.id == Some(self.object_id) && o.confidence >= self.min_confidence;
        match self.field {
            Field::DocumentType => v.document_type.as_ref().is_some_and(ok),
            Field::Correspondent => v.correspondent.as_ref().is_some_and(ok),
            Field::StoragePath => v.storage_path.as_ref().is_some_and(ok),
            Field::Tags => v.tags.iter().any(ok),
            _ => false,
        }
    }
}

/// Existing objects for the draft (id, name).
#[derive(Debug, Clone, Default)]
pub struct DraftTaxonomy {
    pub tags: Vec<(i64, String)>,
    pub correspondents: Vec<(i64, String)>,
    pub document_types: Vec<(i64, String)>,
    pub storage_paths: Vec<(i64, String)>,
}

fn list(items: &[(i64, String)]) -> String {
    let v: Vec<Value> = items.iter().take(200).map(|(i, n)| json!({"id": i, "name": n})).collect();
    Value::Array(v).to_string()
}

/// Prompt for "workflow from description".
pub fn workflow_prompt(description: &str, t: &DraftTaxonomy) -> String {
    let desc: String = description.chars().take(2000).collect();
    format!(
        "You help to set up an automation rule (workflow) for a document management system.\n\
Turn the user's description into one workflow. Use only the IDs listed below. If the description needs a tag, \
correspondent or document type that does not exist, put its name into \"missing\" instead of inventing an ID.\n\n\
Trigger types: 1 = when a document is being consumed (filters: filename pattern, source), 2 = after a document was added, \
3 = when a document was updated. Sources: 1 = consume folder, 2 = API upload, 3 = mail, 4 = web upload.\n\
Matching (for \"match\" on the content): 0 none, 1 any word, 2 all words, 3 exact text, 4 regular expression, 5 fuzzy.\n\
Action types: 1 = assign (title, tags, correspondent, document type, storage path), 2 = remove tags, \
8 = apply AI suggestions.\n\n\
Tags: {}\nCorrespondents: {}\nDocument types: {}\nStorage paths: {}\n\n\
Description (from the user):\n{desc}",
        list(&t.tags),
        list(&t.correspondents),
        list(&t.document_types),
        list(&t.storage_paths)
    )
}

/// Response schema of the model.
pub fn draft_schema() -> Value {
    let ids = json!({"type": "array", "items": {"type": "integer"}});
    json!({
        "type": "object",
        "properties": {
            "name": {"type": "string"},
            "triggers": {"type": "array", "items": {"type": "object", "properties": {
                "type": {"type": "integer"}, "sources": ids, "filter_filename": {"type": ["string", "null"]},
                "match": {"type": "string"}, "matching_algorithm": {"type": "integer"},
                "filter_has_tags": ids, "filter_has_correspondent": {"type": ["integer", "null"]},
                "filter_has_document_type": {"type": ["integer", "null"]}
            }, "required": ["type"]}},
            "actions": {"type": "array", "items": {"type": "object", "properties": {
                "type": {"type": "integer"}, "assign_title": {"type": ["string", "null"]}, "assign_tags": ids,
                "assign_correspondent": {"type": ["integer", "null"]}, "assign_document_type": {"type": ["integer", "null"]},
                "assign_storage_path": {"type": ["integer", "null"]}, "remove_tags": ids
            }, "required": ["type"]}},
            "missing": {"type": "array", "items": {"type": "string"}},
            "explanation": {"type": "string"}
        },
        "required": ["name", "triggers", "actions"]
    })
}

/// Draft (ngx format) + notes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkflowDraft {
    /// `{"name", "order", "enabled", "triggers", "actions"}` like `POST /api/workflows/`.
    pub workflow: Value,
    /// Names that do not exist yet (not created).
    pub missing: Vec<String>,
    /// Short explanation of the model (untrusted, display only).
    pub explanation: String,
    /// Removed, unknown or unsafe parts (codes).
    pub dropped: Vec<String>,
}

fn known(ids: &[(i64, String)], v: Option<&Value>) -> Option<i64> {
    let id = v?.as_i64()?;
    ids.iter().any(|(i, _)| *i == id).then_some(id)
}

fn known_list(ids: &[(i64, String)], v: Option<&Value>, dropped: &mut Vec<String>, what: &str) -> Vec<i64> {
    let mut out = Vec::new();
    for x in v.and_then(Value::as_array).into_iter().flatten() {
        match x.as_i64().filter(|id| ids.iter().any(|(i, _)| i == id)) {
            Some(id) if !out.contains(&id) => out.push(id),
            Some(_) => {}
            None => dropped.push(format!("unknown_{what}")),
        }
    }
    out
}

fn text(v: Option<&Value>, max: usize) -> String {
    v.and_then(Value::as_str).map(|s| s.chars().filter(|c| !c.is_control()).take(max).collect()).unwrap_or_default()
}

/// Model response → safe draft. Unknown ids, trigger/action types and fields are dropped
/// (noted in `dropped`); the AI never suggests e-mail/webhook/permission actions.
pub fn sanitize_draft(raw: &Value, t: &DraftTaxonomy) -> WorkflowDraft {
    let mut dropped = Vec::new();
    let mut triggers = Vec::new();
    for tr in raw.get("triggers").and_then(Value::as_array).into_iter().flatten().take(3) {
        let ty = tr.get("type").and_then(Value::as_i64).unwrap_or(0);
        if !(1..=3).contains(&ty) {
            dropped.push("trigger_type".into());
            continue;
        }
        let mut m = Map::new();
        m.insert("type".into(), json!(ty));
        let sources: Vec<i64> = tr
            .get("sources")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_i64).filter(|s| (1..=4).contains(s)).collect())
            .unwrap_or_default();
        m.insert("sources".into(), json!(if sources.is_empty() { vec![1, 2, 3, 4] } else { sources }));
        let ff = text(tr.get("filter_filename"), 256);
        m.insert("filter_filename".into(), if ff.is_empty() { Value::Null } else { json!(ff) });
        m.insert("filter_path".into(), Value::Null);
        let alg = tr.get("matching_algorithm").and_then(Value::as_i64).filter(|a| (0..=5).contains(a)).unwrap_or(0);
        let mt = text(tr.get("match"), 256);
        m.insert("matching_algorithm".into(), json!(if mt.is_empty() { 0 } else { alg }));
        m.insert("match".into(), json!(if alg == 0 { String::new() } else { mt }));
        m.insert("is_insensitive".into(), json!(true));
        m.insert("filter_has_tags".into(), json!(known_list(&t.tags, tr.get("filter_has_tags"), &mut dropped, "tag")));
        m.insert("filter_has_correspondent".into(), json!(known(&t.correspondents, tr.get("filter_has_correspondent"))));
        m.insert("filter_has_document_type".into(), json!(known(&t.document_types, tr.get("filter_has_document_type"))));
        triggers.push(Value::Object(m));
    }
    let mut actions = Vec::new();
    for (i, a) in raw.get("actions").and_then(Value::as_array).into_iter().flatten().take(5).enumerate() {
        let ty = a.get("type").and_then(Value::as_i64).unwrap_or(0);
        let mut m = Map::new();
        m.insert("type".into(), json!(ty));
        m.insert("order".into(), json!(i));
        match ty {
            1 => {
                let title = text(a.get("assign_title"), 128);
                m.insert("assign_title".into(), if title.is_empty() { Value::Null } else { json!(title) });
                m.insert("assign_tags".into(), json!(known_list(&t.tags, a.get("assign_tags"), &mut dropped, "tag")));
                m.insert("assign_correspondent".into(), json!(known(&t.correspondents, a.get("assign_correspondent"))));
                m.insert("assign_document_type".into(), json!(known(&t.document_types, a.get("assign_document_type"))));
                m.insert("assign_storage_path".into(), json!(known(&t.storage_paths, a.get("assign_storage_path"))));
            }
            2 => {
                m.insert("remove_tags".into(), json!(known_list(&t.tags, a.get("remove_tags"), &mut dropped, "tag")));
            }
            8 => {
                m.insert(
                    "ai_suggestion_fields".into(),
                    json!(["title", "tags", "correspondent", "document_type", "storage_path", "created"]),
                );
                m.insert("ai_create_missing".into(), json!(false));
                m.insert("ai_overwrite_existing".into(), json!(false));
            }
            _ => {
                dropped.push("action_type".into());
                continue;
            }
        }
        actions.push(Value::Object(m));
    }
    let mut name = text(raw.get("name"), 256);
    if name.is_empty() {
        name = "AI draft".into();
    }
    let missing: Vec<String> = raw
        .get("missing")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str())
                .map(|s| s.chars().take(128).collect::<String>())
                .filter(|s| !s.trim().is_empty())
                .take(20)
                .collect()
        })
        .unwrap_or_default();
    dropped.sort();
    dropped.dedup();
    WorkflowDraft {
        workflow: json!({"name": name, "order": 0, "enabled": false, "triggers": triggers, "actions": actions}),
        missing,
        explanation: text(raw.get("explanation"), 1000),
        dropped,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto::PlannedChange;

    fn ch(field: Field, id: Option<i64>, c: f64) -> PlannedChange {
        PlannedChange { field, existing_id: id, new_name: None, value: None, confidence: c, apply: false, why: String::new(), data: None }
    }

    #[test]
    fn condition() {
        let plan = Plan {
            changes: vec![ch(Field::DocumentType, Some(4), 0.95), ch(Field::Tags, Some(7), 0.82), ch(Field::Tags, Some(8), 0.95)],
            new_objects: 0,
        };
        let v = AiVerdict::from_plan(1, &plan);
        assert!(v.document_type.as_ref().unwrap().sure);
        assert!(AiCondition { field: Field::DocumentType, object_id: 4, min_confidence: SURE }.matches(&v));
        assert!(!AiCondition { field: Field::DocumentType, object_id: 5, min_confidence: 0.1 }.matches(&v));
        assert!(!AiCondition { field: Field::Tags, object_id: 7, min_confidence: SURE }.matches(&v));
        assert!(AiCondition { field: Field::Tags, object_id: 7, min_confidence: 0.5 }.matches(&v));
        assert!(!AiCondition { field: Field::Correspondent, object_id: 1, min_confidence: 0.0 }.matches(&v));
        let c: AiCondition = serde_json::from_value(json!({"field": "document_type", "object_id": 4})).unwrap();
        assert_eq!(c.min_confidence, SURE);
    }

    #[test]
    fn draft_is_sanitized() {
        let t = DraftTaxonomy {
            tags: vec![(1, "Rechnung".into()), (2, "Inbox".into())],
            correspondents: vec![(3, "Swisscom".into())],
            document_types: vec![(4, "Invoice".into())],
            storage_paths: vec![],
        };
        let raw = json!({
            "name": "Swisscom-Rechnungen",
            "triggers": [{"type": 2, "match": "Swisscom", "matching_algorithm": 1, "filter_has_tags": [2, 99]}, {"type": 4}],
            "actions": [
                {"type": 1, "assign_tags": [1, 1, 55], "assign_correspondent": 3, "assign_document_type": 77, "assign_storage_path": 1},
                {"type": 2, "remove_tags": [2]},
                {"type": 3, "email": {"to": "x@example.com"}},
                {"type": 8}
            ],
            "missing": ["Telefon"],
            "explanation": "Ordnet Rechnungen zu."
        });
        let d = sanitize_draft(&raw, &t);
        let w = &d.workflow;
        assert_eq!(w["enabled"], false);
        assert_eq!(w["triggers"].as_array().unwrap().len(), 1);
        assert_eq!(w["triggers"][0]["filter_has_tags"], json!([2]));
        assert_eq!(w["triggers"][0]["sources"], json!([1, 2, 3, 4]));
        assert_eq!(w["actions"].as_array().unwrap().len(), 3);
        assert_eq!(w["actions"][0]["assign_tags"], json!([1]));
        assert_eq!(w["actions"][0]["assign_correspondent"], 3);
        assert_eq!(w["actions"][0]["assign_document_type"], Value::Null);
        assert_eq!(w["actions"][0]["assign_storage_path"], Value::Null);
        assert_eq!(w["actions"][2]["type"], 8);
        assert_eq!(d.missing, vec!["Telefon".to_string()]);
        assert!(
            d.dropped.contains(&"action_type".to_string())
                && d.dropped.contains(&"trigger_type".to_string())
                && d.dropped.contains(&"unknown_tag".to_string())
        );
        let p = workflow_prompt("Rechnungen von Swisscom taggen", &t);
        assert!(p.contains("\"Swisscom\"") && p.ends_with("Rechnungen von Swisscom taggen"));
        assert_eq!(sanitize_draft(&json!({}), &t).workflow["name"], "AI draft");
    }
}
