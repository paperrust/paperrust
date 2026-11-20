//! Placeholder context for e-mail/webhook actions.
//!
//! upstream: src/documents/workflows/actions.py@v3.4.2 (build_workflow_action_context),
//! src/documents/templating/workflows.py@v3.4.2 (parse_w_workflow_placeholders).

use std::path::Path;

use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use chrono_tz::Tz;
use sqlx::PgConnection;

use pl_consumer::DocumentMetadataOverrides;
use pl_consumer::db::DocRow;
use pl_templating::WorkflowDocId;

/// `context` dict of `build_workflow_action_context`.
#[derive(Debug, Clone)]
pub struct ActionContext {
    pub title: String,
    pub doc_url: String,
    pub correspondent: String,
    pub document_type: String,
    pub owner_username: String,
    pub filename: String,
    pub current_filename: String,
    pub added: DateTime<FixedOffset>,
    pub created: Option<NaiveDate>,
    /// `document.pk` or `""` (consumption).
    pub id: WorkflowDocId,
    pub custom_fields: Vec<pl_templating::CustomFieldInstance>,
    /// `document.mime_type`
    pub mime_type: String,
    /// `document.source_path` (stored document only).
    pub source_path: Option<std::path::PathBuf>,
}

/// `Path(x).stem` in pl-templating treats only `/` as separator (POSIX like Docker); on Windows
/// `Path` also splits at `\` – rewrite consumption paths (`str(original_file)`) accordingly.
fn native_path(p: &str) -> String {
    if cfg!(windows) { p.replace('\\', "/") } else { p.to_string() }
}

impl ActionContext {
    /// Arguments for `parse_w_workflow_placeholders(text, correspondent, document_type,
    /// owner_username, added, filename, current_filename, created, title, doc_url, id, custom_fields)`
    /// (`id` as [`WorkflowDocId`], see [`Self::render`]).
    pub fn placeholders(&self) -> pl_templating::WorkflowPlaceholders {
        pl_templating::WorkflowPlaceholders {
            correspondent_name: Some(self.correspondent.clone()),
            doc_type_name: Some(self.document_type.clone()),
            owner_username: Some(self.owner_username.clone()),
            local_added: self.added,
            original_filename: native_path(&self.filename),
            filename: native_path(&self.current_filename),
            created: self.created,
            doc_title: Some(self.title.clone()),
            doc_url: Some(self.doc_url.clone()),
            doc_id: match self.id {
                WorkflowDocId::Id(i) => Some(i),
                _ => None,
            },
            custom_fields: self.custom_fields.clone(),
        }
    }

    /// Render text; `Ok(None)` = syntax error (upstream `None`), `Err` = exception.
    pub fn render(&self, text: &str) -> Result<Option<String>, String> {
        pl_templating::parse_w_workflow_placeholders_with_id(text, &self.placeholders(), &self.id).map_err(|e| e.to_string())
    }
}

/// Context for a stored document (`use_overrides = False`), loaded from the DB.
pub async fn for_document(
    conn: &mut PgConnection,
    doc_id: i64,
    tz: Tz,
    paperless_url: &str,
    base_url: &str,
    originals_dir: &Path,
) -> Result<Option<ActionContext>, sqlx::Error> {
    let Some(d) = pl_consumer::db::load_document(conn, doc_id).await? else { return Ok(None) };
    Ok(Some(for_row(conn, &d, tz, paperless_url, base_url, originals_dir).await?))
}

async fn name_of(conn: &mut PgConnection, table: &str, id: Option<i64>) -> Result<String, sqlx::Error> {
    match id {
        Some(id) => Ok(pl_consumer::db::object_name(conn, table, id).await?.unwrap_or_default()),
        None => Ok(String::new()),
    }
}

async fn username_of(conn: &mut PgConnection, id: Option<i64>) -> Result<String, sqlx::Error> {
    match id {
        Some(id) => Ok(pl_consumer::db::username(conn, id).await?.unwrap_or_default()),
        None => Ok(String::new()),
    }
}

/// Context for a document instance (`use_overrides = False`): fields from `d` (including not yet
/// saved assignments), names and custom fields via `conn` (possibly inside the running
/// transaction).
pub async fn for_row(
    conn: &mut PgConnection,
    d: &DocRow,
    tz: Tz,
    paperless_url: &str,
    base_url: &str,
    originals_dir: &Path,
) -> Result<ActionContext, sqlx::Error> {
    let correspondent = name_of(conn, "documents_correspondent", d.correspondent_id).await?;
    let document_type = name_of(conn, "documents_documenttype", d.document_type_id).await?;
    let owner_username = username_of(conn, d.owner_id).await?;
    let custom_fields = pl_consumer::templating::custom_field_instances(conn, d.id).await?;
    Ok(ActionContext {
        title: d.title.clone(),
        doc_url: format!("{paperless_url}{base_url}documents/{}/", d.id),
        correspondent,
        document_type,
        owner_username,
        filename: d.original_filename.clone().unwrap_or_default(),
        current_filename: d.filename.clone().unwrap_or_default(),
        added: d.added.with_timezone(&tz).fixed_offset(),
        created: Some(d.created),
        id: WorkflowDocId::Id(d.id),
        custom_fields,
        mime_type: d.mime_type.clone(),
        source_path: stored_source_path(originals_dir, d.filename.as_deref()),
    })
}

/// `document.source_path` for a stored name: only via `safe_join` (security review
/// 2026-11-19 F9). Unsafe DB name → path that never exists (`pl_consumer::files::join`,
/// warning in the log); the attachment then fails like a missing file.
fn stored_source_path(originals_dir: &Path, filename: Option<&str>) -> Option<std::path::PathBuf> {
    filename.map(|f| pl_consumer::files::join(originals_dir, f))
}

/// Context on consumption (`use_overrides = True`) from the overrides at the time of the action:
/// correspondent/type/owner via `filter(pk=…).first()` (only if the ID is "truthy"; if the
/// object is missing → `""`), `created` of the overrides, custom fields from `overrides.custom_fields`
/// (`CustomField.objects.filter(pk__in=…)`), `id = ""`.
pub async fn for_overrides(
    conn: &mut PgConnection,
    o: &DocumentMetadataOverrides,
    original_file: &Path,
    mime_type: &str,
    tz: Tz,
) -> Result<ActionContext, sqlx::Error> {
    let truthy = |v: Option<i64>| v.filter(|i| *i != 0);
    let correspondent = name_of(conn, "documents_correspondent", truthy(o.correspondent_id)).await?;
    let document_type = name_of(conn, "documents_documenttype", truthy(o.document_type_id)).await?;
    let owner_username = username_of(conn, truthy(o.owner_id)).await?;
    let custom_fields = match &o.custom_fields {
        Some(cf) => pl_consumer::templating::override_custom_fields(conn, cf).await?,
        None => Vec::new(),
    };
    let mut c = for_consumption(o.title.as_deref().unwrap_or(""), original_file, mime_type, tz);
    c.correspondent = correspondent;
    c.document_type = document_type;
    c.owner_username = owner_username;
    c.created = o.created;
    c.custom_fields = custom_fields;
    Ok(c)
}

/// Context on consumption without DB access (empty overrides: only title and file).
pub fn for_consumption(title: &str, original_file: &Path, mime_type: &str, tz: Tz) -> ActionContext {
    let file = original_file.to_string_lossy().into_owned();
    ActionContext {
        title: if title.is_empty() { file.clone() } else { title.to_string() },
        doc_url: String::new(),
        correspondent: String::new(),
        document_type: String::new(),
        owner_username: String::new(),
        filename: file.clone(),
        current_filename: file,
        added: Utc::now().with_timezone(&tz).fixed_offset(),
        created: None,
        id: WorkflowDocId::Empty,
        custom_fields: Vec::new(),
        mime_type: mime_type.to_string(),
        source_path: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Security review 2026-11-19 F9: DB name with `..`/absolute path never points outside originals.
    #[test]
    fn stored_source_path_stays_in_originals() {
        let o = Path::new("/m/originals");
        assert_eq!(stored_source_path(o, None), None);
        assert_eq!(stored_source_path(o, Some("2024/a.pdf")), Some(o.join("2024").join("a.pdf")));
        for bad in ["../../etc/passwd", "/etc/passwd", "a/../../x"] {
            let p = stored_source_path(o, Some(bad)).unwrap();
            assert!(p.ends_with(pl_consumer::files::UNSAFE_MARKER), "{bad:?} -> {}", p.display());
            assert!(!p.exists());
        }
        let ads = stored_source_path(o, Some("a.pdf:strom")).unwrap();
        assert_eq!(ads.ends_with(pl_consumer::files::UNSAFE_MARKER), cfg!(windows));
    }

    #[test]
    fn consumption_context_renders_empty_doc_id() {
        let c = for_consumption("", Path::new("/in/scan.pdf"), "application/pdf", chrono_tz::UTC);
        assert_eq!(c.title, "/in/scan.pdf");
        assert_eq!(c.render("[{{ doc_id }}|{{ doc_url }}|{{ original_filename }}]").unwrap().unwrap(), "[||scan]");
        let mut d = c.clone();
        d.id = WorkflowDocId::Id(5);
        assert_eq!(d.render("{{ doc_id }}").unwrap().unwrap(), "5");
    }
}
