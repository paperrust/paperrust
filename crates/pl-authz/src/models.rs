//! Descriptors of the Django models that carry an `owner` and guardian object permissions.
//!
//! upstream: src/documents/models.py@v3.4.2 (ModelWithOwner and subclasses, ShareLink,
//!           ShareLinkBundle, Document.root_document, SoftDeleteModel),
//!           src/paperless_mail/models.py@v3.4.2 (MailAccount, MailRule, ProcessedMail).

/// One Django model as seen by the permission system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectModel {
    /// `_meta.app_label` (`django_content_type.app_label`).
    pub app_label: &'static str,
    /// `_meta.model_name` (`django_content_type.model`, lower-cased class name).
    pub model_name: &'static str,
    /// `_meta.object_name` (class name, used in "No X matches the given query.").
    pub object_name: &'static str,
    /// Database table.
    pub table: &'static str,
    /// Has `global_objects` (django-soft-delete): the default manager hides `deleted_at IS NOT NULL`.
    pub soft_delete: bool,
}

impl ObjectModel {
    /// Bare codename, e.g. `view_tag` (guardian stores bare codenames).
    pub fn codename(&self, action: &str) -> String {
        format!("{action}_{}", self.model_name)
    }

    /// Qualified permission, e.g. `documents.view_tag` (`user.has_perm`).
    pub fn perm(&self, action: &str) -> String {
        format!("{}.{action}_{}", self.app_label, self.model_name)
    }
}

macro_rules! model {
    ($name:ident, $app:literal, $model:literal, $obj:literal, $table:literal, $soft:literal) => {
        pub const $name: ObjectModel =
            ObjectModel { app_label: $app, model_name: $model, object_name: $obj, table: $table, soft_delete: $soft };
    };
}

model!(DOCUMENT, "documents", "document", "Document", "documents_document", true);
model!(TAG, "documents", "tag", "Tag", "documents_tag", false);
model!(CORRESPONDENT, "documents", "correspondent", "Correspondent", "documents_correspondent", false);
model!(DOCUMENT_TYPE, "documents", "documenttype", "DocumentType", "documents_documenttype", false);
model!(STORAGE_PATH, "documents", "storagepath", "StoragePath", "documents_storagepath", false);
model!(SAVED_VIEW, "documents", "savedview", "SavedView", "documents_savedview", false);
model!(PAPERLESS_TASK, "documents", "paperlesstask", "PaperlessTask", "documents_paperlesstask", false);
model!(SHARE_LINK, "documents", "sharelink", "ShareLink", "documents_sharelink", true);
model!(SHARE_LINK_BUNDLE, "documents", "sharelinkbundle", "ShareLinkBundle", "documents_sharelinkbundle", false);
model!(NOTE, "documents", "note", "Note", "documents_note", true);
model!(CUSTOM_FIELD, "documents", "customfield", "CustomField", "documents_customfield", false);
model!(WORKFLOW, "documents", "workflow", "Workflow", "documents_workflow", false);
model!(WORKFLOW_TRIGGER, "documents", "workflowtrigger", "WorkflowTrigger", "documents_workflowtrigger", false);
model!(WORKFLOW_ACTION, "documents", "workflowaction", "WorkflowAction", "documents_workflowaction", false);
model!(MAIL_ACCOUNT, "paperless_mail", "mailaccount", "MailAccount", "paperless_mail_mailaccount", false);
model!(MAIL_RULE, "paperless_mail", "mailrule", "MailRule", "paperless_mail_mailrule", false);
model!(PROCESSED_MAIL, "paperless_mail", "processedmail", "ProcessedMail", "paperless_mail_processedmail", false);

/// `Document.root_document` – the parent field that authorizes document versions.
pub const DOCUMENT_PARENT_FIELD: &str = "root_document_id";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(DOCUMENT_TYPE.codename("change"), "change_documenttype");
        assert_eq!(MAIL_RULE.perm("view"), "paperless_mail.view_mailrule");
    }
}
