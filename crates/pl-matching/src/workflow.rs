// upstream: src/documents/matching.py@v3.4.2 (consumable_document_matches_workflow,
//           existing_document_matches_workflow, prefilter_documents_by_workflowtrigger,
//           document_matches_workflow)
//           src/documents/data_models.py@v3.4.2 (DocumentSource, ConsumableDocument)
//           src/documents/models.py@v3.4.2 (WorkflowTrigger, WorkflowTriggerType)
//
// Pure functions over own data types. The caller loads triggers/documents from the DB.
// `prefilter_matches` reproduces the queryset filter of
// `prefilter_documents_by_workflowtrigger` as an in-memory predicate (SQL NULL semantics
// of Django `filter`/`exclude`); `filename_prefilter_regex` returns the regex text that
// upstream passes to `original_filename__iregex` for a DB-side implementation.

use crate::fnmatch::{fnmatch, iregex_search, translate};
use crate::matching::{MATCH_NONE, MatchDocument, Matchable, UnsupportedMatchingAlgorithm, matches};

/// `documents.data_models.DocumentSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DocumentSource {
    ConsumeFolder = 1,
    ApiUpload = 2,
    MailFetch = 3,
    WebUI = 4,
}

impl DocumentSource {
    pub fn from_i64(v: i64) -> Option<Self> {
        match v {
            1 => Some(Self::ConsumeFolder),
            2 => Some(Self::ApiUpload),
            3 => Some(Self::MailFetch),
            4 => Some(Self::WebUI),
            _ => None,
        }
    }
    /// Python enum member name.
    pub fn name(self) -> &'static str {
        match self {
            Self::ConsumeFolder => "ConsumeFolder",
            Self::ApiUpload => "ApiUpload",
            Self::MailFetch => "MailFetch",
            Self::WebUI => "WebUI",
        }
    }
}

/// `WorkflowTrigger.WorkflowTriggerType`.
pub const TRIGGER_CONSUMPTION: i64 = 1;
pub const TRIGGER_DOCUMENT_ADDED: i64 = 2;
pub const TRIGGER_DOCUMENT_UPDATED: i64 = 3;
pub const TRIGGER_SCHEDULED: i64 = 4;
/// PaperRust (after paperless-ngx dev #14704): runs only when a user applies the workflow by hand
/// to selected documents (`bulk_edit` method `run_workflow`).
pub const TRIGGER_MANUAL: i64 = 5;

/// A related object (Tag, Correspondent, DocumentType, StoragePath) with its `str()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjRef {
    pub id: i64,
    pub name: String,
}

impl ObjRef {
    pub fn new(id: i64, name: &str) -> Self {
        Self { id, name: name.into() }
    }
}

/// `ConsumableDocument` (only the fields used for matching).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumableDocument {
    pub source: DocumentSource,
    /// `Path(original_file).resolve()` as string (absolute path).
    pub original_file: String,
    pub original_path: Option<String>,
    pub mailrule_id: Option<i64>,
}

impl ConsumableDocument {
    /// `original_file.name`
    pub fn file_name(&self) -> &str {
        self.original_file.rsplit('/').next().unwrap_or(&self.original_file)
    }
}

/// An existing `Document` (only the fields used for matching).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExistingDocument {
    pub id: i64,
    /// `get_effective_content()`
    pub effective_content: Option<String>,
    /// `str(document)`
    pub display: String,
    /// `document.tags.all()` in model ordering (only used in reason texts besides ids)
    pub tags: Vec<ObjRef>,
    pub correspondent: Option<ObjRef>,
    pub document_type: Option<ObjRef>,
    pub storage_path: Option<ObjRef>,
    pub original_filename: Option<String>,
}

/// `WorkflowTrigger` (matching-relevant fields).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkflowTrigger {
    pub id: i64,
    pub trigger_type: i64,
    /// `sources` (MultiSelectField, values as ints)
    pub sources: Vec<i64>,
    pub filter_filename: Option<String>,
    pub filter_path: Option<String>,
    pub filter_mailrule_id: Option<i64>,
    pub matching_algorithm: i64,
    pub match_text: String,
    pub is_insensitive: bool,
    pub filter_has_tags: Vec<ObjRef>,
    pub filter_has_all_tags: Vec<ObjRef>,
    pub filter_has_not_tags: Vec<ObjRef>,
    pub filter_has_any_correspondents: Vec<ObjRef>,
    pub filter_has_correspondent: Option<ObjRef>,
    pub filter_has_not_correspondents: Vec<ObjRef>,
    pub filter_has_any_document_types: Vec<ObjRef>,
    pub filter_has_document_type: Option<ObjRef>,
    pub filter_has_not_document_types: Vec<ObjRef>,
    pub filter_has_any_storage_paths: Vec<ObjRef>,
    pub filter_has_storage_path: Option<ObjRef>,
    pub filter_has_not_storage_paths: Vec<ObjRef>,
    pub filter_custom_field_query: Option<String>,
}

impl WorkflowTrigger {
    /// `str(trigger)`
    pub fn display(&self) -> String {
        format!("WorkflowTrigger {}", self.id)
    }

    fn as_matchable(&self) -> Matchable {
        Matchable {
            id: self.id,
            class_name: "WorkflowTrigger".into(),
            // hasattr(trigger, "name") is False -> str(trigger)
            name: self.display(),
            match_text: self.match_text.clone(),
            matching_algorithm: self.matching_algorithm,
            is_insensitive: self.is_insensitive,
        }
    }
}

/// Invalid `filter_custom_field_query` (upstream: serializers.ValidationError).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidCustomFieldQuery;

/// Evaluates `CustomFieldQueryParser` queries (lives in pl-docfilter / the DB layer).
pub trait CustomFieldQueryEvaluator {
    /// Does the document with `document_id` match `query`?
    fn matches(&self, query: &str, document_id: i64) -> Result<bool, InvalidCustomFieldQuery>;
}

fn py_list_repr_names(objs: &[ObjRef], model: &str) -> String {
    // repr(list of model instances) -> "[<Tag: a>, <Tag: b>]"
    let inner: Vec<String> = objs.iter().map(|o| format!("<{model}: {}>", o.name)).collect();
    format!("[{}]", inner.join(", "))
}

fn py_str_list(items: &[&str]) -> String {
    let inner: Vec<String> = items.iter().map(|s| format!("'{s}'")).collect();
    format!("[{}]", inner.join(", "))
}

fn opt_name(o: &Option<ObjRef>) -> String {
    o.as_ref().map_or_else(|| "None".to_string(), |o| o.name.clone())
}

/// `consumable_document_matches_workflow(document, trigger)` -> (matched, reason)
pub fn consumable_document_matches_workflow(doc: &ConsumableDocument, trigger: &WorkflowTrigger) -> (bool, String) {
    let mut trigger_matched = true;
    let mut reason = String::new();

    if !trigger.sources.is_empty() && !trigger.sources.contains(&(doc.source as i64)) {
        let names: Vec<&str> = trigger
            .sources
            .iter()
            .map(|s| DocumentSource::from_i64(*s).map_or("?", DocumentSource::name))
            .collect();
        reason = format!("Document source {} not in {}", doc.source.name(), py_str_list(&names));
        trigger_matched = false;
    }

    if let Some(rule) = trigger.filter_mailrule_id {
        if doc.mailrule_id != Some(rule) {
            let doc_rule = doc.mailrule_id.map_or_else(|| "None".to_string(), |r| r.to_string());
            reason = format!("Document mail rule {doc_rule} != {rule}");
            trigger_matched = false;
        }
    }

    if let Some(ff) = trigger.filter_filename.as_deref().filter(|f| !f.is_empty()) {
        if !fnmatch(&doc.file_name().to_lowercase(), &ff.to_lowercase()) {
            reason = format!("Document filename {} does not match {}", doc.file_name(), ff.to_lowercase());
            trigger_matched = false;
        }
    }

    let match_against = doc.original_path.as_deref().unwrap_or(&doc.original_file);
    if let Some(fp) = trigger.filter_path.as_deref().filter(|f| !f.is_empty()) {
        if !fnmatch(match_against, fp) {
            reason = format!("Document path {} does not match {}", doc.original_file, fp);
            trigger_matched = false;
        }
    }

    (trigger_matched, reason)
}

fn ids(objs: &[ObjRef]) -> Vec<i64> {
    objs.iter().map(|o| o.id).collect()
}

/// `existing_document_matches_workflow(document, trigger)` -> (matched, reason)
pub fn existing_document_matches_workflow(
    doc: &ExistingDocument,
    trigger: &WorkflowTrigger,
    custom_fields: Option<&dyn CustomFieldQueryEvaluator>,
) -> Result<(bool, Option<String>), UnsupportedMatchingAlgorithm> {
    if trigger.matching_algorithm > MATCH_NONE {
        let md = MatchDocument { effective_content: doc.effective_content.clone(), display: doc.display.clone() };
        if !matches(&trigger.as_matchable(), &md)? {
            return Ok((
                false,
                Some(format!(
                    "Document content matching settings for algorithm '{}' did not match",
                    trigger.matching_algorithm
                )),
            ));
        }
    }

    let doc_tags = ids(&doc.tags);
    let doc_tags_repr = py_list_repr_names(&doc.tags, "Tag");

    if !trigger.filter_has_tags.is_empty() && !trigger.filter_has_tags.iter().any(|t| doc_tags.contains(&t.id)) {
        return Ok((
            false,
            Some(format!(
                "Document tags {doc_tags_repr} do not include {}",
                py_list_repr_names(&trigger.filter_has_tags, "Tag")
            )),
        ));
    }
    if !trigger.filter_has_all_tags.is_empty()
        && !trigger.filter_has_all_tags.iter().all(|t| doc_tags.contains(&t.id))
    {
        return Ok((
            false,
            Some(format!(
                "Document tags {doc_tags_repr} do not contain all of {}",
                py_list_repr_names(&trigger.filter_has_all_tags, "Tag")
            )),
        ));
    }
    if !trigger.filter_has_not_tags.is_empty() && trigger.filter_has_not_tags.iter().any(|t| doc_tags.contains(&t.id))
    {
        return Ok((
            false,
            Some(format!(
                "Document tags {doc_tags_repr} include excluded tags {}",
                py_list_repr_names(&trigger.filter_has_not_tags, "Tag")
            )),
        ));
    }

    // correspondent
    let cid = doc.correspondent.as_ref().map(|c| c.id);
    if !trigger.filter_has_any_correspondents.is_empty()
        && !cid.is_some_and(|c| ids(&trigger.filter_has_any_correspondents).contains(&c))
    {
        return Ok((
            false,
            Some(format!(
                "Document correspondent {} is not one of {}",
                opt_name(&doc.correspondent),
                py_list_repr_names(&trigger.filter_has_any_correspondents, "Correspondent")
            )),
        ));
    }
    if let Some(fc) = &trigger.filter_has_correspondent {
        if cid != Some(fc.id) {
            return Ok((
                false,
                Some(format!(
                    "Document correspondent {} does not match {}",
                    opt_name(&doc.correspondent),
                    fc.name
                )),
            ));
        }
    }
    if let Some(c) = cid.filter(|c| *c != 0) {
        if ids(&trigger.filter_has_not_correspondents).contains(&c) {
            return Ok((
                false,
                Some(format!(
                    "Document correspondent {} is excluded by {}",
                    opt_name(&doc.correspondent),
                    py_list_repr_names(&trigger.filter_has_not_correspondents, "Correspondent")
                )),
            ));
        }
    }

    // document type
    let did = doc.document_type.as_ref().map(|c| c.id);
    if !trigger.filter_has_any_document_types.is_empty()
        && !did.is_some_and(|c| ids(&trigger.filter_has_any_document_types).contains(&c))
    {
        return Ok((
            false,
            Some(format!(
                "Document doc type {} is not one of {}",
                opt_name(&doc.document_type),
                py_list_repr_names(&trigger.filter_has_any_document_types, "DocumentType")
            )),
        ));
    }
    if let Some(ft) = &trigger.filter_has_document_type {
        if did != Some(ft.id) {
            return Ok((
                false,
                Some(format!("Document doc type {} does not match {}", opt_name(&doc.document_type), ft.name)),
            ));
        }
    }
    if let Some(d) = did.filter(|d| *d != 0) {
        if ids(&trigger.filter_has_not_document_types).contains(&d) {
            return Ok((
                false,
                Some(format!(
                    "Document doc type {} is excluded by {}",
                    opt_name(&doc.document_type),
                    py_list_repr_names(&trigger.filter_has_not_document_types, "DocumentType")
                )),
            ));
        }
    }

    // storage path
    let sid = doc.storage_path.as_ref().map(|c| c.id);
    if !trigger.filter_has_any_storage_paths.is_empty()
        && !sid.is_some_and(|c| ids(&trigger.filter_has_any_storage_paths).contains(&c))
    {
        return Ok((
            false,
            Some(format!(
                "Document storage path {} is not one of {}",
                opt_name(&doc.storage_path),
                py_list_repr_names(&trigger.filter_has_any_storage_paths, "StoragePath")
            )),
        ));
    }
    if let Some(fs) = &trigger.filter_has_storage_path {
        if sid != Some(fs.id) {
            return Ok((
                false,
                Some(format!("Document storage path {} does not match {}", opt_name(&doc.storage_path), fs.name)),
            ));
        }
    }
    if let Some(s) = sid.filter(|s| *s != 0) {
        if ids(&trigger.filter_has_not_storage_paths).contains(&s) {
            return Ok((
                false,
                Some(format!(
                    "Document storage path {} is excluded by {}",
                    opt_name(&doc.storage_path),
                    py_list_repr_names(&trigger.filter_has_not_storage_paths, "StoragePath")
                )),
            ));
        }
    }

    // custom field query
    if let Some(q) = trigger.filter_custom_field_query.as_deref().filter(|q| !q.is_empty()) {
        let result = match custom_fields {
            Some(ev) => ev.matches(q, doc.id),
            None => Err(InvalidCustomFieldQuery),
        };
        match result {
            Err(_) => return Ok((false, Some("Invalid custom field query configuration".into()))),
            Ok(false) => {
                return Ok((
                    false,
                    Some("Document custom fields do not match the configured custom field query".into()),
                ));
            }
            Ok(true) => {}
        }
    }

    if let (Some(ff), Some(of)) = (trigger.filter_filename.as_deref().filter(|f| !f.is_empty()), &doc.original_filename) {
        if !fnmatch(&of.to_lowercase(), &ff.to_lowercase()) {
            return Ok((false, Some(format!("Document filename {of} does not match {}", ff.to_lowercase()))));
        }
    }

    Ok((true, None))
}

/// Regex text upstream uses for `original_filename__iregex`:
/// `fnmatch_translate(filter_filename).lstrip("^").rstrip("$")` (Python regex syntax).
pub fn filename_prefilter_regex(filter_filename: &str) -> String {
    let t = translate(filter_filename);
    t.trim_start_matches('^').trim_end_matches('$').to_string()
}

/// In-memory equivalent of `prefilter_documents_by_workflowtrigger` for one document.
pub fn prefilter_matches(
    doc: &ExistingDocument,
    trigger: &WorkflowTrigger,
    custom_fields: Option<&dyn CustomFieldQueryEvaluator>,
) -> bool {
    let doc_tags = ids(&doc.tags);
    if !trigger.filter_has_tags.is_empty() && !trigger.filter_has_tags.iter().any(|t| doc_tags.contains(&t.id)) {
        return false;
    }
    if !trigger.filter_has_all_tags.is_empty() && !trigger.filter_has_all_tags.iter().all(|t| doc_tags.contains(&t.id))
    {
        return false;
    }
    if trigger.filter_has_not_tags.iter().any(|t| doc_tags.contains(&t.id)) {
        return false;
    }

    fn fk_ok(value: Option<i64>, any: &[ObjRef], exact: &Option<ObjRef>, not: &[ObjRef]) -> bool {
        if !any.is_empty() && !value.is_some_and(|v| any.iter().any(|o| o.id == v)) {
            return false;
        }
        if let Some(e) = exact {
            if value != Some(e.id) {
                return false;
            }
        }
        // exclude(fk__in=...) keeps NULL rows
        if let Some(v) = value {
            if not.iter().any(|o| o.id == v) {
                return false;
            }
        }
        true
    }

    if !fk_ok(
        doc.correspondent.as_ref().map(|o| o.id),
        &trigger.filter_has_any_correspondents,
        &trigger.filter_has_correspondent,
        &trigger.filter_has_not_correspondents,
    ) || !fk_ok(
        doc.document_type.as_ref().map(|o| o.id),
        &trigger.filter_has_any_document_types,
        &trigger.filter_has_document_type,
        &trigger.filter_has_not_document_types,
    ) || !fk_ok(
        doc.storage_path.as_ref().map(|o| o.id),
        &trigger.filter_has_any_storage_paths,
        &trigger.filter_has_storage_path,
        &trigger.filter_has_not_storage_paths,
    ) {
        return false;
    }

    if let Some(q) = trigger.filter_custom_field_query.as_deref().filter(|q| !q.is_empty()) {
        match custom_fields.map(|ev| ev.matches(q, doc.id)) {
            Some(Ok(true)) => {}
            // invalid query -> documents.none(); no evaluator -> cannot decide -> none
            _ => return false,
        }
    }

    if let Some(ff) = trigger.filter_filename.as_deref().filter(|f| !f.is_empty()) {
        let Some(of) = &doc.original_filename else { return false };
        if !iregex_search(&filename_prefilter_regex(ff), of) {
            return false;
        }
    }
    true
}

/// A document passed to `document_matches_workflow`.
pub enum WorkflowDocument<'a> {
    Consumable(&'a ConsumableDocument),
    Existing(&'a ExistingDocument),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkflowMatchError {
    /// `Exception(f"Trigger type {trigger_type} not yet supported")`
    UnsupportedTriggerType(i64),
    /// trigger type / document kind mismatch (upstream would fail with AttributeError)
    WrongDocumentKind,
    Matching(UnsupportedMatchingAlgorithm),
}

/// `document_matches_workflow(document, workflow, trigger_type)`.
/// `triggers` = all triggers of the workflow (queryset order); `workflow_name` = `Workflow.name`.
pub fn document_matches_workflow(
    document: WorkflowDocument<'_>,
    workflow_name: &str,
    triggers: &[WorkflowTrigger],
    trigger_type: i64,
    custom_fields: Option<&dyn CustomFieldQueryEvaluator>,
) -> Result<bool, WorkflowMatchError> {
    let wf = format!("Workflow: {workflow_name}");
    let selected: Vec<&WorkflowTrigger> = triggers.iter().filter(|t| t.trigger_type == trigger_type).collect();
    let mut trigger_matched = true;
    if selected.is_empty() {
        trigger_matched = false;
        tracing::info!(target: "paperless.matching", "Document did not match {wf}");
        tracing::debug!(target: "paperless.matching", "No matching triggers with type {trigger_type} found");
    } else {
        for trigger in selected {
            let (matched, reason) = match trigger_type {
                TRIGGER_CONSUMPTION => match &document {
                    WorkflowDocument::Consumable(d) => {
                        let (m, r) = consumable_document_matches_workflow(d, trigger);
                        (m, Some(r))
                    }
                    WorkflowDocument::Existing(_) => return Err(WorkflowMatchError::WrongDocumentKind),
                },
                TRIGGER_DOCUMENT_ADDED | TRIGGER_DOCUMENT_UPDATED | TRIGGER_SCHEDULED | TRIGGER_MANUAL => match &document {
                    WorkflowDocument::Existing(d) => existing_document_matches_workflow(d, trigger, custom_fields)
                        .map_err(WorkflowMatchError::Matching)?,
                    WorkflowDocument::Consumable(_) => return Err(WorkflowMatchError::WrongDocumentKind),
                },
                other => return Err(WorkflowMatchError::UnsupportedTriggerType(other)),
            };
            trigger_matched = matched;
            if matched {
                tracing::info!(target: "paperless.matching", "Document matched {} from {wf}", trigger.display());
                return Ok(true);
            }
            tracing::info!(target: "paperless.matching", "Document did not match {wf}");
            tracing::debug!(target: "paperless.matching", "{}", reason.unwrap_or_else(|| "None".into()));
        }
    }
    Ok(trigger_matched)
}
