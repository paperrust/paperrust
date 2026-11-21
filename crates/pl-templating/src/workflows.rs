//! Templates for workflow titles, e-mail subject/body and webhook fields.
//!
//! upstream: src/documents/templating/workflows.py@v3.4.2 (_known_placeholder_names,
//! validate_workflow_template, parse_w_workflow_placeholders); callers:
//! documents/workflows/mutations.py, documents/workflows/actions.py, documents/consumer.py.

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime};
use minijinja::value::Value;

use crate::context::{CustomFieldInstance, custom_fields_context};
use crate::error::{TemplateError, WorkflowTemplateError};
use crate::pystr::path_stem;
use crate::pyvalue::{PyDateTime, date_iso};
use crate::strftime::{TzInfo, strftime};

/// `_known_placeholder_names`.
pub const KNOWN_PLACEHOLDER_NAMES: &[&str] = &[
    "correspondent",
    "document_type",
    "added",
    "added_year",
    "added_year_short",
    "added_month",
    "added_month_name",
    "added_month_name_short",
    "added_day",
    "added_time",
    "owner_username",
    "original_filename",
    "filename",
    "created",
    "created_year",
    "created_year_short",
    "created_month",
    "created_month_name",
    "created_month_name_short",
    "created_day",
    "created_time",
    "doc_title",
    "doc_url",
    "doc_id",
    "custom_fields",
];

/// `validate_workflow_template(text)` – for `WorkflowActionSerializer.validate` (`assign_title`).
pub fn validate_workflow_template(text: &str) -> Result<(), WorkflowTemplateError> {
    match crate::env::check_syntax(text) {
        Ok(()) => {}
        Err(TemplateError::Syntax(m)) | Err(TemplateError::Other(m)) | Err(TemplateError::Security(m)) => {
            return Err(WorkflowTemplateError::ValueError(format!("Template syntax error: {m}")));
        }
        Err(e) => return Err(WorkflowTemplateError::ValueError(format!("Template syntax error: {e}"))),
    }
    if let Some(msg) = crate::env::check_filter_and_test_names(text) {
        return Err(WorkflowTemplateError::ValueError(format!("Template assertion error: {msg}")));
    }
    let undeclared =
        crate::env::undeclared_variables(text).map_err(|e| WorkflowTemplateError::ValueError(format!("Template syntax error: {e}")))?;
    let unknown: Vec<String> = undeclared.into_iter().filter(|v| !KNOWN_PLACEHOLDER_NAMES.contains(&v.as_str())).collect();
    if !unknown.is_empty() {
        return Err(WorkflowTemplateError::KeyError(format!("Template references unknown placeholders: {}", unknown.join(", "))));
    }
    Ok(())
}

/// Arguments of `parse_w_workflow_placeholders`.
#[derive(Debug, Clone)]
pub struct WorkflowPlaceholders {
    /// `None` renders as "None" like upstream (consumer without correspondent).
    pub correspondent_name: Option<String>,
    pub doc_type_name: Option<String>,
    pub owner_username: Option<String>,
    /// `timezone.localtime(document.added)` (or `localtime(now())` in the consumer).
    pub local_added: DateTime<FixedOffset>,
    pub original_filename: String,
    pub filename: String,
    pub created: Option<NaiveDate>,
    pub doc_title: Option<String>,
    pub doc_url: Option<String>,
    /// `doc_id` as a number or not set. For `""` (ingestion context of
    /// `build_workflow_action_context`) see [`WorkflowDocId`] /
    /// [`parse_w_workflow_placeholders_with_id`].
    pub doc_id: Option<i64>,
    pub custom_fields: Vec<CustomFieldInstance>,
}

/// Python value of the argument `doc_id` of `parse_w_workflow_placeholders`.
///
/// upstream: `if doc_id is not None: formatting["doc_id"] = str(doc_id)` – `None` leaves the
/// placeholder undefined (StrictUndefined → error), `""` (e-mail/webhook action on ingestion,
/// `build_workflow_action_context(..., overrides)`) sets it to the empty text, a number to
/// `str(pk)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum WorkflowDocId {
    /// `None`
    #[default]
    Unset,
    /// `""`
    Empty,
    /// `document.pk`
    Id(i64),
}

impl From<Option<i64>> for WorkflowDocId {
    fn from(v: Option<i64>) -> Self {
        v.map_or(WorkflowDocId::Unset, WorkflowDocId::Id)
    }
}

impl WorkflowDocId {
    /// `str(doc_id)` or `None`.
    pub fn as_text(&self) -> Option<String> {
        match self {
            WorkflowDocId::Unset => None,
            WorkflowDocId::Empty => Some(String::new()),
            WorkflowDocId::Id(i) => Some(i.to_string()),
        }
    }
}

fn opt(v: &Option<String>) -> Value {
    v.clone().map(Value::from).unwrap_or(Value::from(()))
}

/// `parse_w_workflow_placeholders(...)`.
///
/// * `Ok(Some(text))` – rendered
/// * `Ok(None)` – syntax/security error (upstream: warning, `return None`)
/// * `Err(_)` – UndefinedError or another exception (upstream: `raise`)
pub fn parse_w_workflow_placeholders(text: &str, p: &WorkflowPlaceholders) -> Result<Option<String>, TemplateError> {
    parse_w_workflow_placeholders_with_id(text, p, &WorkflowDocId::from(p.doc_id))
}

/// Like [`parse_w_workflow_placeholders`], but `doc_id` as [`WorkflowDocId`] (overrides
/// `p.doc_id`), so that `""` can also be passed as in the ingestion trigger.
pub fn parse_w_workflow_placeholders_with_id(
    text: &str,
    p: &WorkflowPlaceholders,
    doc_id: &WorkflowDocId,
) -> Result<Option<String>, TemplateError> {
    let la = p.local_added;
    let la_naive = la.naive_local();
    let la_tz = TzInfo::Fixed(*la.offset());
    let af = |f: &str| Value::from(strftime(&la_naive, la_tz, f));
    let mut ctx: Vec<(&'static str, Value)> = vec![
        ("correspondent", opt(&p.correspondent_name)),
        ("document_type", opt(&p.doc_type_name)),
        ("added", Value::from(PyDateTime { naive: la_naive, offset: Some(*la.offset()) }.isoformat("T"))),
        ("added_year", af("%Y")),
        ("added_year_short", af("%y")),
        ("added_month", af("%m")),
        ("added_month_name", af("%B")),
        ("added_month_name_short", af("%b")),
        ("added_day", af("%d")),
        ("added_time", af("%H:%M")),
        ("owner_username", opt(&p.owner_username)),
        ("original_filename", Value::from(path_stem(&p.original_filename))),
        ("filename", Value::from(path_stem(&p.filename))),
    ];
    ctx.push(("custom_fields", custom_fields_context(&p.custom_fields)?));
    if let Some(c) = p.created {
        let cn = c.and_time(NaiveTime::MIN);
        let cf = |f: &str| Value::from(strftime(&cn, TzInfo::Naive, f));
        ctx.push(("created", Value::from(date_iso(c))));
        ctx.push(("created_year", cf("%Y")));
        ctx.push(("created_year_short", cf("%y")));
        ctx.push(("created_month", cf("%m")));
        ctx.push(("created_month_name", cf("%B")));
        ctx.push(("created_month_name_short", cf("%b")));
        ctx.push(("created_day", cf("%d")));
        ctx.push(("created_time", cf("%H:%M")));
    }
    if let Some(t) = &p.doc_title {
        ctx.push(("doc_title", Value::from(t.clone())));
    }
    if let Some(u) = &p.doc_url {
        ctx.push(("doc_url", Value::from(u.clone())));
    }
    if let Some(id) = doc_id.as_text() {
        ctx.push(("doc_id", Value::from(id)));
    }

    tracing::debug!(target: "paperless.templating", "Parsing Workflow Jinja template: {text}");
    match crate::env::render(text, Value::from_iter(ctx)) {
        Ok(r) => Ok(Some(r)),
        Err(e @ TemplateError::Undefined(_)) => Err(e),
        Err(TemplateError::Syntax(m)) => {
            tracing::warn!(target: "paperless.templating", "Template syntax error in title generation: {m}");
            Ok(None)
        }
        Err(TemplateError::Security(m)) => {
            tracing::warn!(target: "paperless.templating", "Template attempted restricted operation: {m}");
            Ok(None)
        }
        Err(e) => {
            tracing::warn!(target: "paperless.templating", "Unknown error in title generation: {e}");
            tracing::warn!(target: "paperless.templating", "Invalid title format '{text}', workflow not applied: {e}");
            Err(e)
        }
    }
}

/// Short form for the title assignment (`apply_assignment_to_document`): `Some(title)` only if the
/// result is not empty (`if title: document.title = title`); on errors `None` (upstream
/// logs and keeps the old title).
pub fn render_workflow_title(text: &str, p: &WorkflowPlaceholders) -> Option<String> {
    match parse_w_workflow_placeholders(text, p) {
        Ok(Some(t)) if !t.is_empty() => Some(t),
        Ok(_) => None,
        Err(e) => {
            tracing::error!(
                target: "paperless.workflows.mutations",
                "Error occurred parsing title assignment '{text}', falling back to original: {e}"
            );
            None
        }
    }
}
