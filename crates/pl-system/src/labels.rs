//! `PaperlessTask` choice values, their English labels and translations.
//!
//! upstream: src/documents/models.py@v3.4.2 (`PaperlessTask.Status`, `.TaskType`,
//! `.TriggerSource`, `COMPLETE_STATUSES`), translations from upstream/src/locale/*/django.po
//! (see `labels_generated.rs`, produced by tests/parity/pl-system/gen_labels.py).

#[path = "labels_generated.rs"]
mod generated;

/// `PaperlessTask.Status` choices (value, label) in declaration order.
pub const STATUS: &[(&str, &str)] =
    &[("pending", "Pending"), ("started", "Started"), ("success", "Success"), ("failure", "Failure"), ("revoked", "Revoked")];

/// `PaperlessTask.TaskType` choices in declaration order.
pub const TASK_TYPE: &[(&str, &str)] = &[
    ("consume_file", "Consume File"),
    ("train_classifier", "Train Classifier"),
    ("sanity_check", "Sanity Check"),
    ("index_optimize", "Index Optimize"),
    ("mail_fetch", "Mail Fetch"),
    ("llm_index", "LLM Index"),
    ("empty_trash", "Empty Trash"),
    ("check_workflows", "Check Workflows"),
    ("bulk_update", "Bulk Update"),
    ("reprocess_document", "Reprocess Document"),
    ("build_share_link", "Build Share Link"),
    ("bulk_delete", "Bulk Delete"),
    ("apply_ai_suggestions", "Apply AI Suggestions"),
];

/// `PaperlessTask.TriggerSource` choices in declaration order.
pub const TRIGGER_SOURCE: &[(&str, &str)] = &[
    ("scheduled", "Scheduled"),
    ("web_ui", "Web UI"),
    ("api_upload", "API Upload"),
    ("folder_consume", "Folder Consume"),
    ("email_consume", "Email Consume"),
    ("system", "System"),
    ("manual", "Manual"),
];

/// `PaperlessTask.COMPLETE_STATUSES`.
pub const COMPLETE_STATUSES: &[&str] = &["success", "failure", "revoked"];

/// English label of a choice value (`None` if the value is not a declared choice).
pub fn label_of(choices: &[(&str, &'static str)], value: &str) -> Option<&'static str> {
    choices.iter().find(|(v, _)| *v == value).map(|(_, l)| *l)
}

/// `gettext(msgid)` for the active language (`settings.LANGUAGES` code, e.g. `de-de`).
pub fn translate(language: &str, msgid: &'static str) -> &'static str {
    let lang = language.to_ascii_lowercase();
    generated::TRANSLATIONS.iter().find(|(l, m, _)| *l == lang && *m == msgid).map(|(_, _, t)| *t).unwrap_or(msgid)
}

/// `get_<field>_display()`: translated label, or the raw value for unknown values.
pub fn display(choices: &[(&str, &'static str)], value: &str, language: &str) -> String {
    match label_of(choices, value) {
        Some(l) => translate(language, l).to_string(),
        None => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translates() {
        assert_eq!(display(STATUS, "pending", "de-de"), "Ausstehend");
        assert_eq!(display(STATUS, "pending", "en-us"), "Pending");
        assert_eq!(display(TASK_TYPE, "unknown_x", "de-de"), "unknown_x");
        assert_eq!(display(TRIGGER_SOURCE, "manual", "DE-DE"), "Manuell");
    }
}
