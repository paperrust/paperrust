//! Automatic processing – pure decision logic (planning); execution, storage and
//! triggers live in `server::review` (review list, undo), `server::jobs` (post-processing) and in the
//! worker (pl-server: after ingestion, trigger tag, workflow action).
//! Contract: `docs/contract/pl-ai.md`, section "Automatische Verarbeitung".
//!
//! Based on upstream `documents/workflows/ai.py@v3.4.2::apply_ai_suggestions_to_document`
//! (workflow action `APPLY_AI_SUGGESTIONS = 8`, fields `ai_suggestion_fields`,
//! `ai_create_missing`, `ai_overwrite_existing`, Celery task `documents.tasks.apply_ai_suggestions`).
//! PaperRust adds: confidence, suggest/apply modes, limits per run, audit +
//! undo, duplicate protection via [`crate::taxonomy::check_duplicate`] instead of upstream
//! `get_or_create(name=…)`.
//!
//! Custom fields (`Field::CustomFields`) are planned by `server::review` from [`crate::custom_fields`]
//! ([`custom_field_changes`]).

use serde::{Deserialize, Serialize};

use crate::config::AutoMode;
use crate::schema::ClassificationSuggestions;
use crate::taxonomy::{Alias, MatchOptions, TaxonomyEntry, TaxonomyKind, check_duplicate};

/// What triggers a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum Trigger {
    /// After ingestion (`document_consumption_finished`).
    Consumed,
    /// Tag was set (e.g. `ai-process`); the tag is removed after the run.
    Tag { tag_id: i64 },
    /// Workflow action (upstream `APPLY_AI_SUGGESTIONS`) or own action.
    Workflow { workflow_id: i64, action_id: i64 },
    /// Manual (UI/MCP) for a selection.
    Manual,
}

/// Limits of a run (against costs and against prompt-injection damage).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RunLimits {
    pub max_documents: u32,
    pub max_new_objects: u32,
    pub max_changes_per_document: u32,
    pub min_confidence_apply: f64,
}

impl Default for RunLimits {
    fn default() -> Self {
        Self { max_documents: 50, max_new_objects: 3, max_changes_per_document: 8, min_confidence_apply: 0.9 }
    }
}

/// Which fields may be edited (upstream `AISuggestionField`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    Title,
    Tags,
    Correspondent,
    DocumentType,
    StoragePath,
    Created,
    /// Extension: custom fields (prompt `custom_fields`).
    CustomFields,
}

/// Custom field values → planned changes. Fields with an existing value (`filled`) are not
/// overwritten; `apply` only in mode `Apply` from the threshold.
pub fn custom_field_changes(
    values: &[crate::custom_fields::FieldValue],
    filled: &[i64],
    mode: AutoMode,
    limits: &RunLimits,
) -> Vec<PlannedChange> {
    if mode == AutoMode::Off {
        return Vec::new();
    }
    values
        .iter()
        .filter(|v| !filled.contains(&v.field_id))
        .map(|v| PlannedChange {
            field: Field::CustomFields,
            existing_id: Some(v.field_id),
            new_name: None,
            value: Some(v.display.clone()),
            confidence: v.confidence,
            apply: mode == AutoMode::Apply && v.confidence >= limits.min_confidence_apply,
            why: "value found in document".into(),
            data: Some(v.value.clone()),
        })
        .collect()
}

/// A planned change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedChange {
    pub field: Field,
    /// Existing object (matching) …
    pub existing_id: Option<i64>,
    /// … or name to be created (only if allowed and no candidate).
    pub new_name: Option<String>,
    /// Title/date.
    pub value: Option<String>,
    /// 0..1 – matching score or model confidence.
    pub confidence: f64,
    /// Apply (true) or only suggest (false).
    pub apply: bool,
    pub why: String,
    /// Storage value (custom fields: value like `custom_fields[].value` of the REST API, `existing_id` =
    /// field id, `value` = readable form). PaperRust extension.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Result of planning for a document.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub changes: Vec<PlannedChange>,
    pub new_objects: u32,
}

/// Visible objects per kind (for planning).
pub struct Visible<'a> {
    pub entries: &'a [TaxonomyEntry],
    pub aliases: &'a [Alias],
}

/// Plans the changes for a document from LLM suggestions – pure, without side effects.
///
/// * Names are always matched against existing ones first (extended mode).
/// * New object only if `create_missing`, no candidate above the threshold and the run budget
///   `remaining_new_objects` suffices; storage paths are never newly created (as upstream).
/// * `apply` only in mode `Apply` and from `min_confidence_apply`; objects to be created are
///   never "safe" (confidence 0.5) and are therefore only suggested in mode `Apply`, unless
///   `min_confidence_apply <= 0.5`.
#[allow(clippy::too_many_arguments)]
pub fn plan<'v>(
    s: &ClassificationSuggestions,
    fields: &[Field],
    mode: AutoMode,
    limits: &RunLimits,
    create_missing: bool,
    remaining_new_objects: u32,
    visible: &dyn Fn(TaxonomyKind) -> Visible<'v>,
    opts: &MatchOptions,
) -> Plan {
    let mut p = Plan::default();
    if mode == AutoMode::Off {
        return p;
    }
    let apply_ok = |c: f64| mode == AutoMode::Apply && c >= limits.min_confidence_apply;
    if fields.contains(&Field::Title) && !s.title.trim().is_empty() {
        let t: String = s.title.trim().chars().take(128).collect();
        p.changes.push(PlannedChange {
            field: Field::Title,
            existing_id: None,
            new_name: None,
            value: Some(t),
            confidence: 0.6,
            apply: apply_ok(0.6),
            why: "LLM title".into(),
            data: None,
        });
    }
    if fields.contains(&Field::Created)
        && let Some(d) = s.dates.iter().find(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok())
    {
        p.changes.push(PlannedChange {
            field: Field::Created,
            existing_id: None,
            new_name: None,
            value: Some(d.clone()),
            confidence: 0.6,
            apply: apply_ok(0.6),
            why: "first valid suggested date".into(),
            data: None,
        });
    }
    let singles = [
        (Field::Correspondent, TaxonomyKind::Correspondent, &s.correspondents),
        (Field::DocumentType, TaxonomyKind::DocumentType, &s.document_types),
        (Field::StoragePath, TaxonomyKind::StoragePath, &s.storage_paths),
    ];
    for (field, kind, choice) in singles {
        if !fields.contains(&field) {
            continue;
        }
        if let Some(id) = choice.existing_ids.first() {
            p.changes.push(PlannedChange {
                field,
                existing_id: Some(*id),
                new_name: None,
                value: None,
                confidence: 0.95,
                apply: apply_ok(0.95),
                why: "model picked a shown candidate".into(),
                data: None,
            });
            continue;
        }
        let v = visible(kind);
        let Some(name) = choice.new_names.first() else { continue };
        let d = check_duplicate(kind, name, v.entries, v.aliases, opts);
        if let Some(c) = d.candidates.iter().find(|c| c.above_threshold) {
            p.changes.push(PlannedChange {
                field,
                existing_id: Some(c.id),
                new_name: None,
                value: None,
                confidence: c.score,
                apply: apply_ok(c.score),
                why: format!("matched '{}' to existing '{}'", name, c.name),
                data: None,
            });
        } else if create_missing && kind != TaxonomyKind::StoragePath && p.new_objects < remaining_new_objects {
            p.new_objects += 1;
            p.changes.push(PlannedChange {
                field,
                existing_id: None,
                new_name: Some(name.clone()),
                value: None,
                confidence: 0.5,
                apply: apply_ok(0.5),
                why: "no equivalent object exists".into(),
                data: None,
            });
        }
    }
    if fields.contains(&Field::Tags) {
        let v = visible(TaxonomyKind::Tag);
        for id in &s.tags.existing_ids {
            p.changes.push(PlannedChange {
                field: Field::Tags,
                existing_id: Some(*id),
                new_name: None,
                value: None,
                confidence: 0.95,
                apply: apply_ok(0.95),
                why: "model picked a shown candidate".into(),
                data: None,
            });
        }
        for name in &s.tags.new_names {
            let d = check_duplicate(TaxonomyKind::Tag, name, v.entries, v.aliases, opts);
            if let Some(c) = d.candidates.iter().find(|c| c.above_threshold) {
                if p.changes.iter().any(|x| x.field == Field::Tags && x.existing_id == Some(c.id)) {
                    continue;
                }
                p.changes.push(PlannedChange {
                    field: Field::Tags,
                    existing_id: Some(c.id),
                    new_name: None,
                    value: None,
                    confidence: c.score,
                    apply: apply_ok(c.score),
                    why: format!("matched '{}' to existing '{}'", name, c.name),
                    data: None,
                });
            } else if create_missing && p.new_objects < remaining_new_objects {
                p.new_objects += 1;
                p.changes.push(PlannedChange {
                    field: Field::Tags,
                    existing_id: None,
                    new_name: Some(name.clone()),
                    value: None,
                    confidence: 0.5,
                    apply: apply_ok(0.5),
                    why: "no equivalent tag exists".into(),
                    data: None,
                });
            }
        }
    }
    p.changes.truncate(limits.max_changes_per_document as usize);
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::TaxonomyChoice;

    #[test]
    fn plan_matches_existing_and_respects_budget() {
        static ENTRIES: std::sync::OnceLock<Vec<TaxonomyEntry>> = std::sync::OnceLock::new();
        let entries = ENTRIES.get_or_init(|| {
            vec![
                TaxonomyEntry {
                    kind: TaxonomyKind::Correspondent,
                    id: 1,
                    name: "Swisscom (Schweiz) AG".into(),
                    owner_id: None,
                    extra: None,
                    document_count: None,
                },
                TaxonomyEntry {
                    kind: TaxonomyKind::Tag,
                    id: 5,
                    name: "Rechnung".into(),
                    owner_id: None,
                    extra: None,
                    document_count: None,
                },
            ]
        });
        let vis = |_k: TaxonomyKind| Visible { entries: entries.as_slice(), aliases: &[] };
        let s = ClassificationSuggestions {
            title: "Rechnung Mai".into(),
            correspondents: TaxonomyChoice { existing_ids: vec![], new_names: vec!["Swisscom AG".into()] },
            tags: TaxonomyChoice { existing_ids: vec![], new_names: vec!["Rechnungen".into(), "Telefon".into(), "Internet".into()] },
            ..Default::default()
        };
        let fields = [Field::Title, Field::Correspondent, Field::Tags];
        let p = plan(&s, &fields, AutoMode::Apply, &RunLimits::default(), true, 1, &vis, &MatchOptions::default());
        let corr = p.changes.iter().find(|c| c.field == Field::Correspondent).unwrap();
        assert_eq!(corr.existing_id, Some(1));
        assert!(corr.apply);
        assert!(p.changes.iter().any(|c| c.field == Field::Tags && c.existing_id == Some(5)));
        assert_eq!(p.new_objects, 1, "Budget 1");
        assert!(p.changes.iter().filter(|c| c.new_name.is_some()).all(|c| !c.apply), "neue Objekte nie automatisch");
        let off = plan(&s, &fields, AutoMode::Off, &RunLimits::default(), true, 1, &vis, &MatchOptions::default());
        assert!(off.changes.is_empty());
        let sug = plan(&s, &fields, AutoMode::Suggest, &RunLimits::default(), false, 1, &vis, &MatchOptions::default());
        assert!(sug.changes.iter().all(|c| !c.apply));
        assert_eq!(sug.new_objects, 0);
    }

    #[test]
    fn custom_fields_planned_without_overwrite() {
        use crate::custom_fields::FieldValue;
        let vals = vec![
            FieldValue { field_id: 1, value: serde_json::json!("EUR12.00"), display: "EUR12.00".into(), confidence: 0.95 },
            FieldValue { field_id: 2, value: serde_json::json!("2025-01-01"), display: "2025-01-01".into(), confidence: 0.7 },
            FieldValue { field_id: 3, value: serde_json::json!("x"), display: "x".into(), confidence: 0.99 },
        ];
        let ch = custom_field_changes(&vals, &[3], AutoMode::Apply, &RunLimits::default());
        assert_eq!(ch.len(), 2);
        assert!(ch[0].apply && !ch[1].apply);
        assert_eq!(ch[0].data, Some(serde_json::json!("EUR12.00")));
        assert!(custom_field_changes(&vals, &[], AutoMode::Suggest, &RunLimits::default()).iter().all(|c| !c.apply));
        assert!(custom_field_changes(&vals, &[], AutoMode::Off, &RunLimits::default()).is_empty());
    }
}
