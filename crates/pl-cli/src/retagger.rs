//! `document_retagger`.
//!
//! upstream: src/documents/management/commands/document_retagger.py,
//! src/documents/signals/handlers.py (`set_correspondent`, `set_document_type`, `set_tags`,
//! `set_storage_path`; `document.save(update_fields=…)` → audit log + post_save
//! `update_filename_and_move_files`), src/documents/matching.py (candidates permission-filtered via
//! the owner) – v3.4.2. Matching: pl-matching; classifier: pl-classifier (PaperRust model).

use std::collections::BTreeSet;

use sqlx::PgConnection;

use crate::args::{self, flag, value};
use crate::ctx::Ctx;
use crate::{CliError, CliResult};

const OPTS: &[args::Opt] = &[
    args::PROGRESS,
    flag(&["-c", "--correspondent"], "correspondent"),
    flag(&["-T", "--tags"], "tags"),
    flag(&["-t", "--document_type"], "document_type"),
    flag(&["-s", "--storage_path"], "storage_path"),
    flag(&["-i", "--inbox-only"], "inbox_only"),
    flag(&["--use-first"], "use_first"),
    flag(&["-f", "--overwrite"], "overwrite"),
    flag(&["--suggest"], "suggest"),
    value(&["--base-url"], "base_url"),
    args::Opt { names: &["--id-range"], dest: "id_range", nargs: 2 },
];

#[derive(Debug, Clone, Default)]
pub struct RetagOptions {
    pub correspondent: bool,
    pub document_type: bool,
    pub tags: bool,
    pub storage_path: bool,
    pub inbox_only: bool,
    pub use_first: bool,
    pub overwrite: bool,
    pub suggest: bool,
    pub base_url: Option<String>,
    pub id_range: Option<(i64, i64)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub correspondents: usize,
    pub document_types: usize,
    pub tags_added: usize,
    pub tags_removed: usize,
    pub storage_paths: usize,
    pub documents_processed: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Suggestion {
    pub doc_id: i64,
    pub display: String,
    pub correspondent: Option<String>,
    pub document_type: Option<String>,
    pub tags_add: Vec<String>,
    pub tags_remove: Vec<String>,
    pub storage_path: Option<String>,
}

impl Suggestion {
    fn has_any(&self) -> bool {
        self.correspondent.is_some()
            || self.document_type.is_some()
            || !self.tags_add.is_empty()
            || !self.tags_remove.is_empty()
            || self.storage_path.is_some()
    }
}

pub async fn command(argv: Vec<String>) -> CliResult {
    let a = args::parse(&argv, OPTS, 0)?;
    let id_range = match a.values("id_range") {
        Some([lo, hi]) => Some((
            lo.parse().map_err(|_| CliError::Usage(format!("argument --id-range: invalid int value: '{lo}'")))?,
            hi.parse().map_err(|_| CliError::Usage(format!("argument --id-range: invalid int value: '{hi}'")))?,
        )),
        _ => None,
    };
    let o = RetagOptions {
        correspondent: a.flag("correspondent"),
        document_type: a.flag("document_type"),
        tags: a.flag("tags"),
        storage_path: a.flag("storage_path"),
        inbox_only: a.flag("inbox_only"),
        use_first: a.flag("use_first"),
        overwrite: a.flag("overwrite"),
        suggest: a.flag("suggest"),
        base_url: a.value("base_url").map(str::to_string),
        id_range,
    };
    if !(o.correspondent || o.document_type || o.tags || o.storage_path) {
        println!("No classifier targets specified. Use -c, -T, -t, or -s to select what to retag.");
        return Ok(());
    }
    let c = Ctx::connect().await?;
    let (stats, suggestions) = retag(&c, &o).await?;
    if o.suggest {
        let visible: Vec<&Suggestion> = suggestions.iter().filter(|s| s.has_any()).collect();
        if visible.is_empty() {
            println!("No changes suggested.");
        } else {
            println!("Suggested Changes");
            println!("Document | Correspondent | Doc Type | Tags | Storage Path");
            for s in visible {
                let doc = match &o.base_url {
                    Some(b) => format!("{} ({b}/documents/{})", s.display, s.doc_id),
                    None => format!("{} [{}]", s.display, s.doc_id),
                };
                let mut tags: Vec<String> = s.tags_add.iter().map(|t| format!("+{t}")).collect();
                tags.extend(s.tags_remove.iter().map(|t| format!("-{t}")));
                let tags = if tags.is_empty() { "-".to_string() } else { tags.join(", ") };
                println!(
                    "{doc} | {} | {} | {tags} | {}",
                    s.correspondent.as_deref().unwrap_or("-"),
                    s.document_type.as_deref().unwrap_or("-"),
                    s.storage_path.as_deref().unwrap_or("-")
                );
            }
        }
    } else {
        println!("Retagger Summary");
        println!("Documents processed  {}", stats.documents_processed);
        println!("Correspondents set   {}", stats.correspondents);
        println!("Document types set   {}", stats.document_types);
        println!("Tags added           {}", stats.tags_added);
        println!("Tags removed         {}", stats.tags_removed);
        println!("Storage paths set    {}", stats.storage_paths);
    }
    Ok(())
}

type Cls = pl_consumer::native::ThresholdClassifier;

/// Single target (correspondent/type/storage path): returns the chosen (id, name).
#[allow(clippy::too_many_arguments)]
async fn set_single(
    c: &Ctx,
    conn: &mut PgConnection,
    doc: &mut pl_consumer::docops::DocInstance,
    md: &pl_matching::MatchDocument,
    owner: Option<&pl_authz::User>,
    classifier: Option<&Cls>,
    kind: &str,
    o: &RetagOptions,
) -> CliResult<Option<(Option<i64>, String)>> {
    let (current, model, class_name, field, plural) = match kind {
        "correspondent" => (doc.row.correspondent_id, &pl_authz::models::CORRESPONDENT, "Correspondent", "correspondent", "correspondents"),
        "document_type" => (doc.row.document_type_id, &pl_authz::models::DOCUMENT_TYPE, "DocumentType", "document_type", "document types"),
        _ => (doc.row.storage_path_id, &pl_authz::models::STORAGE_PATH, "StoragePath", "storage_path", "storage paths"),
    };
    if current.is_some() && !o.overwrite {
        return Ok(None);
    }
    let cands = pl_consumer::db::matchables(conn, model, class_name, owner).await?;
    let cls = classifier.map(|x| x as &dyn pl_matching::Classifier);
    let found = match kind {
        "correspondent" => pl_matching::matching::match_correspondents(md, &cands, cls),
        "document_type" => pl_matching::matching::match_document_types(md, &cands, cls),
        _ => pl_matching::matching::match_storage_paths(md, &cands, cls),
    }
    .map_err(|e| CliError::Other(e.to_string()))?;
    let selected = found.first().map(|m| (m.id, m.name.clone()));
    if found.len() > 1 {
        if o.use_first {
            tracing::debug!(target: "paperless.handlers", "Detected {} potential {plural}, so we've opted for {}", found.len(), found[0].name);
        } else {
            tracing::debug!(target: "paperless.handlers", "Detected {} potential {plural}, not assigning any {}", found.len(), field.replace('_', " "));
            return Ok(None);
        }
    }
    if (selected.is_some() || o.overwrite) && !o.suggest {
        let name = selected.as_ref().map(|s| s.1.clone()).unwrap_or_else(|| "None".into());
        tracing::info!(target: "paperless.handlers", "Assigning {} {name} to {}", field.replace('_', " "), md.display);
        let id = selected.as_ref().map(|s| s.0);
        match kind {
            "correspondent" => doc.row.correspondent_id = id,
            "document_type" => doc.row.document_type_id = id,
            _ => doc.row.storage_path_id = id,
        }
        let audit = pl_consumer::audit::AuditLog { enabled: c.cfg.audit_log_enabled, actor_id: None };
        pl_consumer::docops::save(conn, &audit, doc, Some(&[field])).await?;
        crate::filemove::update_filename_and_move_files(c, conn, doc.row.id).await?;
    }
    Ok(selected.map(|(id, n)| (Some(id), n)))
}

pub async fn retag(c: &Ctx, o: &RetagOptions) -> CliResult<(Stats, Vec<Suggestion>)> {
    let classifier = crate::classifier::load_for_matching(c);
    let mut sql = String::from("SELECT DISTINCT d.id, d.created FROM documents_document d WHERE d.deleted_at IS NULL");
    if o.inbox_only {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM documents_document_tags dt JOIN documents_tag t ON t.id = dt.tag_id WHERE dt.document_id = d.id AND t.is_inbox_tag)",
        );
    }
    if let Some((lo, hi)) = o.id_range {
        sql.push_str(&format!(" AND d.id BETWEEN {lo} AND {hi}"));
    }
    sql.push_str(" ORDER BY d.created DESC");
    let ids: Vec<(i32, chrono::NaiveDate)> = sqlx::query_as(&sql).fetch_all(&c.pool).await?;
    let mut conn = c.pool.acquire().await?;
    let mut stats = Stats::default();
    let mut suggestions = Vec::new();
    let audit = pl_consumer::audit::AuditLog { enabled: c.cfg.audit_log_enabled, actor_id: None };
    for (id, _) in ids {
        let Some(row) = pl_consumer::db::load_document(&mut conn, i64::from(id)).await? else { continue };
        let mut doc = pl_consumer::docops::DocInstance::from_db(row);
        let owner = match doc.row.owner_id {
            Some(oid) => pl_authz::user::load_user_by_id(&c.pool, oid).await?,
            None => None,
        };
        let content = pl_consumer::workflows::effective_content(&mut conn, &doc.row).await?;
        let md = pl_matching::MatchDocument { effective_content: Some(content), display: doc.repr(&mut conn).await? };
        let mut s = Suggestion { doc_id: doc.row.id, display: md.display.clone(), ..Default::default() };
        if o.correspondent
            && let Some((_, n)) = set_single(c, &mut conn, &mut doc, &md, owner.as_ref(), classifier.as_ref(), "correspondent", o).await?
        {
            stats.correspondents += 1;
            s.correspondent = Some(n);
        }
        if o.document_type
            && let Some((_, n)) = set_single(c, &mut conn, &mut doc, &md, owner.as_ref(), classifier.as_ref(), "document_type", o).await?
        {
            stats.document_types += 1;
            s.document_type = Some(n);
        }
        if o.tags {
            // set_tags
            let current: Vec<(i64, String)> = pl_consumer::db::document_tags(&mut conn, doc.row.id).await?;
            let mut removed: Vec<String> = Vec::new();
            if o.overwrite {
                let rows: Vec<(i32, String)> = sqlx::query_as(
                    "SELECT t.id, t.name FROM documents_document_tags dt JOIN documents_tag t ON t.id = dt.tag_id
                     WHERE dt.document_id = $1 AND NOT t.is_inbox_tag AND NOT (t.match = '' AND t.matching_algorithm <> 6) ORDER BY t.name",
                )
                .bind(id)
                .fetch_all(&mut *conn)
                .await?;
                removed = rows.iter().map(|r| r.1.clone()).collect();
                if !o.suggest && !rows.is_empty() {
                    let rm: Vec<i32> = rows.iter().map(|r| r.0).collect();
                    sqlx::query("DELETE FROM documents_document_tags WHERE document_id = $1 AND tag_id = ANY($2)")
                        .bind(id)
                        .bind(&rm)
                        .execute(&mut *conn)
                        .await?;
                }
            }
            let current_ids: BTreeSet<i64> = if o.overwrite && !o.suggest {
                pl_consumer::db::document_tags(&mut conn, doc.row.id).await?.into_iter().map(|t| t.0).collect()
            } else {
                current.iter().map(|t| t.0).collect()
            };
            let cands = pl_consumer::db::matchables(&mut conn, &pl_authz::models::TAG, "Tag", owner.as_ref()).await?;
            let found = pl_matching::matching::match_tags(&md, &cands, classifier.as_ref().map(|x| x as &dyn pl_matching::Classifier))
                .map_err(|e| CliError::Other(e.to_string()))?;
            let add: Vec<&pl_matching::Matchable> = found.into_iter().filter(|m| !current_ids.contains(&m.id)).collect();
            if !add.is_empty() && !o.suggest {
                let names: Vec<&str> = add.iter().map(|m| m.name.as_str()).collect();
                tracing::info!(target: "paperless.handlers", "Tagging \"{}\" with \"{}\"", md.display, names.join(", "));
                let ids: Vec<i64> = add.iter().map(|m| m.id).collect();
                pl_consumer::docops::add_nested_tags(&mut conn, &audit, &doc, &ids).await?;
                crate::filemove::update_filename_and_move_files(c, &mut conn, doc.row.id).await?;
            }
            stats.tags_added += add.len();
            stats.tags_removed += removed.len();
            let mut add_names: Vec<String> = add.iter().map(|m| m.name.clone()).collect();
            add_names.sort();
            s.tags_add = add_names;
            s.tags_remove = removed;
        }
        if o.storage_path
            && let Some((_, n)) = set_single(c, &mut conn, &mut doc, &md, owner.as_ref(), classifier.as_ref(), "storage_path", o).await?
        {
            stats.storage_paths += 1;
            s.storage_path = Some(n);
        }
        stats.documents_processed += 1;
        if o.suggest {
            suggestions.push(s);
        }
    }
    Ok((stats, suggestions))
}
