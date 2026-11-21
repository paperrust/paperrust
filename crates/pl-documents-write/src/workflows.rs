// upstream: src/documents/signals/handlers.py@v3.4.2 (run_workflows_updated → run_workflows without
//           overrides), src/documents/workflows/mutations.py@v3.4.2 (apply_assignment_to_document,
//           apply_removal_to_document), src/documents/workflows/actions.py@v3.4.2
//           (execute_move_to_trash_action, e-mail/webhook → hooks), src/documents/workflows/utils.py@v3.4.2
//           (get_workflows_for_trigger), src/documents/matching.py@v3.4.2 (document_matches_workflow →
//           pl-matching), guardian.shortcuts.remove_perm
//
//! Workflows with the "document updated" trigger (signal `document_updated`).

use serde_json::Value;
use sqlx::PgConnection;

use pl_consumer::db::DocRow;
use pl_consumer::stages::ActionInput;
use pl_consumer::workflow_ext::{self, AiMode, Decision, DeferredRun, ExtOptions, MatchState};
/// Result of a workflow run (also for `versions.rs`/`update.rs` via `crate::workflows`).
pub use pl_consumer::workflows::RunOutcome;
use pl_consumer::workflows::{
    ACTION_APPLY_AI_SUGGESTIONS, ACTION_ASSIGNMENT, ACTION_EMAIL, ACTION_MOVE_TO_TRASH, ACTION_PASSWORD_REMOVAL, ACTION_REMOTE_OCR,
    ACTION_REMOVAL, ACTION_WEBHOOK, ActionDef, record_run, workflows_for_trigger,
};
use pl_matching::workflow::{ExistingDocument, ObjRef, TRIGGER_DOCUMENT_UPDATED, WorkflowDocument, document_matches_workflow};

use crate::auditlog::LogCtx;
use crate::cfi;
use crate::docs;
use crate::error::ApiError;
use crate::files::MediaCfg;
use crate::hooks::{self, ActionContext};
use crate::softdelete;

fn has_assign_perms(a: &ActionDef) -> bool {
    !(a.assign_view_users.is_empty()
        && a.assign_view_groups.is_empty()
        && a.assign_change_users.is_empty()
        && a.assign_change_groups.is_empty())
}

fn has_remove_perms(a: &ActionDef) -> bool {
    !(a.remove_view_users.is_empty()
        && a.remove_view_groups.is_empty()
        && a.remove_change_users.is_empty()
        && a.remove_change_groups.is_empty())
}

fn cf_value(a: &ActionDef, field: i64) -> Value {
    a.assign_custom_fields_values.as_ref().and_then(|v| v.get(field.to_string())).cloned().unwrap_or(Value::Null)
}

async fn name_of(conn: &mut PgConnection, table: &str, id: Option<i64>) -> Result<Option<ObjRef>, sqlx::Error> {
    let Some(id) = id else { return Ok(None) };
    let sql = format!("SELECT name FROM {table} WHERE id = $1");
    let n: Option<String> = sqlx::query_scalar(&sql).bind(id as i32).fetch_optional(&mut *conn).await?;
    Ok(n.map(|name| ObjRef { id, name }))
}

/// `get_effective_content()`: content of the latest version, otherwise own content.
async fn effective_content(conn: &mut PgConnection, d: &DocRow) -> Result<String, sqlx::Error> {
    let v: Option<String> = sqlx::query_scalar(
        "SELECT content FROM documents_document WHERE root_document_id = $1 AND deleted_at IS NULL \
         ORDER BY version_index DESC NULLS LAST, id DESC LIMIT 1",
    )
    .bind(d.id as i32)
    .fetch_optional(&mut *conn)
    .await?;
    Ok(v.unwrap_or_else(|| d.content.clone()))
}

async fn existing(conn: &mut PgConnection, d: &DocRow) -> Result<ExistingDocument, sqlx::Error> {
    let tags: Vec<(i32, String)> = sqlx::query_as(
        "SELECT t.id, t.name FROM documents_tag t JOIN documents_document_tags dt ON dt.tag_id = t.id \
         WHERE dt.document_id = $1 ORDER BY t.name, t.id",
    )
    .bind(d.id as i32)
    .fetch_all(&mut *conn)
    .await?;
    Ok(ExistingDocument {
        id: d.id,
        effective_content: Some(effective_content(conn, d).await?),
        display: docs::repr(conn, d).await?,
        tags: tags.into_iter().map(|(id, name)| ObjRef { id: i64::from(id), name }).collect(),
        correspondent: name_of(conn, "documents_correspondent", d.correspondent_id).await?,
        document_type: name_of(conn, "documents_documenttype", d.document_type_id).await?,
        storage_path: name_of(conn, "documents_storagepath", d.storage_path_id).await?,
        original_filename: d.original_filename.clone(),
    })
}

fn action_input(ctx: ActionContext, doc: &DocRow) -> ActionInput {
    ActionInput { ctx, overrides: None, document: Some(doc.clone()), mime_type: doc.mime_type.clone() }
}

/// After [`run_updated`]: enqueue `apply_ai_suggestions` (W4).
pub async fn enqueue_outcome(state: &pl_core::AppState, outcome: &RunOutcome) {
    if outcome.ai_suggestions.is_empty() && outcome.deferred.is_empty() {
        return;
    }
    let q = crate::pdf_edit::pg_queue(state);
    pl_consumer::workflows::enqueue_ai_suggestions(q.as_deref(), outcome).await;
}

/// `run_workflows(DOCUMENT_UPDATED, document)`. Returns: AI tasks to enqueue.
pub async fn run_updated(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: &MediaCfg,
    tz: chrono_tz::Tz,
    doc_id: i64,
) -> Result<RunOutcome, ApiError> {
    let mut outcome = RunOutcome::default();
    let Some(first) = docs::load_global(conn, doc_id).await? else { return Ok(outcome) };
    if first.root_document_id.is_some() {
        tracing::debug!(target: "paperless.handlers", "Skipping workflow execution for version document {doc_id}");
        return Ok(outcome);
    }
    // Review H1: attachment only from ORIGINALS_DIR (otherwise invalid path → no attachment)
    let original_file = media.checked_source_path(&first).unwrap_or_else(|e| {
        tracing::warn!(target: "paperless.handlers", "{e}");
        media.originals_dir.join(crate::files::UNSAFE_MARKER)
    });
    for wf in workflows_for_trigger(conn, TRIGGER_DOCUMENT_UPDATED).await? {
        // document.refresh_from_db()
        let Some(mut doc) = docs::load_global(conn, doc_id).await? else {
            tracing::info!(target: "paperless.handlers", "Document no longer exists, skipping remaining workflows");
            break;
        };
        if doc.deleted_at.is_some() {
            tracing::info!(target: "paperless.handlers", "Document was moved to trash, skipping remaining workflows");
            break;
        }
        let ex = existing(conn, &doc).await?;
        // Final security review K-N1: evaluate filter_custom_field_query over the same connection
        // (transaction) instead of never matching
        let selected: Vec<&pl_matching::workflow::WorkflowTrigger> =
            wf.triggers.iter().filter(|t| t.trigger_type == TRIGGER_DOCUMENT_UPDATED).collect();
        let cfq = pl_consumer::workflows::eval_custom_field_queries(conn, &selected, &[doc.id], tz).await?;
        let state = match document_matches_workflow(
            WorkflowDocument::Existing(&ex),
            &wf.name,
            &wf.triggers,
            TRIGGER_DOCUMENT_UPDATED,
            Some(&cfq),
        ) {
            Ok(true) => MatchState::Matched,
            Ok(false) => MatchState::NotMatched { reason: None },
            Err(e) => {
                tracing::error!(target: "paperless.handlers", "Error matching workflow {}: {e:?}", wf.name);
                MatchState::Error(format!("{e:?}"))
            }
        };
        // PaperRust: else branch, AI condition (caught up after the commit in the worker), history –
        // the same decision as in the worker (pl_consumer::workflow_ext::decide).
        let decision = workflow_ext::decide(&wf, TRIGGER_DOCUMENT_UPDATED, state, Some(doc.id), EXT).await;
        let branch = match &decision {
            Decision::Run { branch, .. } => *branch,
            Decision::Defer => {
                outcome.deferred.push(DeferredRun { workflow_id: wf.id, document_id: doc.id, trigger_type: TRIGGER_DOCUMENT_UPDATED });
                continue;
            }
            Decision::Skip { .. } => {
                if let Some(r) = workflow_ext::record_for(&wf, TRIGGER_DOCUMENT_UPDATED, Some(doc.id), &decision) {
                    workflow_ext::insert_run(conn, &r).await?;
                    outcome.runs.push(r);
                }
                continue;
            }
        };
        let doc_id = doc.id;
        let before = doc.clone();
        match run_branch(conn, ctx, media, tz, &wf, branch, &decision, before, &mut doc, &original_file, &mut outcome).await {
            Ok(()) => {}
            Err(e) => {
                if let ApiError::Database(db) = &e {
                    workflow_ext::insert_error(conn, wf.id, TRIGGER_DOCUMENT_UPDATED, Some(doc_id), db).await;
                }
                return Err(e);
            }
        }
    }
    Ok(outcome)
}

/// "Document updated" options via the API: catch up the AI condition after the commit.
const EXT: ExtOptions = ExtOptions { ai_mode: AiMode::Defer, log_skips: false };

/// Actions of the chosen branch, saving, `WorkflowRun`, history, trash.
#[allow(clippy::too_many_arguments)]
async fn run_branch(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: &MediaCfg,
    tz: chrono_tz::Tz,
    wf: &pl_consumer::workflows::WorkflowDef,
    branch: workflow_ext::Branch,
    decision: &Decision,
    before: DocRow,
    doc: &mut DocRow,
    original_file: &std::path::Path,
    outcome: &mut RunOutcome,
) -> Result<(), ApiError> {
    let effects = hooks::workflow_effects();
    let actions = workflow_ext::branch_actions(wf, branch);
    {
        let mut trash = false;
        for action in actions {
            tracing::info!(target: "paperless.handlers", "Applying {} from Workflow: {}", action.display(), wf.name);
            let actx = ActionContext {
                workflow_action_id: action.id,
                action_type: action.action_type,
                document_id: Some(doc.id),
                title: doc.title.clone(),
                original_file: original_file.to_path_buf(),
                trigger_type: TRIGGER_DOCUMENT_UPDATED,
            };
            match action.action_type {
                ACTION_ASSIGNMENT => assignment(conn, ctx, media, tz, action, doc).await?,
                ACTION_REMOVAL => removal(conn, ctx, media, action, doc).await?,
                // W6: as upstream synchronously in the signal, with the state inside this connection
                // (document in memory incl. the previous assignments of this run).
                ACTION_EMAIL => effects.email_in(conn, &action_input(actx, doc)).await,
                ACTION_WEBHOOK => effects.webhook_in(conn, &action_input(actx, doc)).await,
                // W1: runs in the background and waits for the saved document.
                ACTION_PASSWORD_REMOVAL => effects.password_removal(&actx),
                ACTION_MOVE_TO_TRASH => trash = true,
                ACTION_REMOTE_OCR => tracing::debug!(
                    target: "paperless.handlers",
                    "Remote OCR action only applies to consumption triggers, ignoring"
                ),
                // W4: apply_ai_suggestions.delay_on_commit(action_id=, document_id=) – the caller
                // enqueues after the run (connection without transaction = committed immediately).
                ACTION_APPLY_AI_SUGGESTIONS => outcome.ai_suggestions.push((action.id, doc.id)),
                _ => {}
            }
        }
        doc.title = doc.title.chars().take(128).collect();
        docs::save(conn, ctx, Some(media), doc, Some(&["title", "correspondent", "document_type", "storage_path", "owner", "modified"]))
            .await?;
        record_run(conn, wf.id, TRIGGER_DOCUMENT_UPDATED, Some(doc.id)).await?;
        if let Some(mut r) = workflow_ext::record_for(wf, TRIGGER_DOCUMENT_UPDATED, Some(doc.id), decision) {
            r.changed_fields = workflow_ext::changed_fields(&before, Some(doc), actions);
            workflow_ext::insert_run(conn, &r).await?;
            outcome.runs.push(r);
        }
        if trash {
            softdelete::soft_delete_document(conn, ctx, media, doc.id, None).await?;
            tracing::debug!(target: "paperless.workflows.actions", "Moved document {} to trash", doc.id);
        }
    }
    Ok(())
}

async fn assignment(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: &MediaCfg,
    tz: chrono_tz::Tz,
    action: &ActionDef,
    doc: &mut DocRow,
) -> Result<(), ApiError> {
    if !action.assign_tags.is_empty() {
        // Document.objects.get(pk=…).add_nested_tags(action.assign_tags.all())
        let mut ids: Vec<i64> = Vec::new();
        for t in &action.assign_tags {
            if let Some((anc, _)) = docs::tag_tree(conn, *t).await? {
                ids.push(*t);
                ids.extend(anc);
            }
        }
        ids.sort_unstable();
        ids.dedup();
        if docs::load_live(conn, doc.id).await?.is_none() {
            return Err(ApiError::Uncaught("Document matching query does not exist.".into()));
        }
        docs::tags_add(conn, ctx, Some(media), doc.id, &ids).await?;
    }
    if let Some(c) = action.assign_correspondent {
        doc.correspondent_id = Some(c);
    }
    if let Some(t) = action.assign_document_type {
        doc.document_type_id = Some(t);
    }
    if let Some(s) = action.assign_storage_path {
        doc.storage_path_id = Some(s);
    }
    if let Some(u) = action.assign_owner {
        doc.owner_id = Some(u);
    }
    if !action.assign_custom_fields.is_empty() {
        let mut fields = Vec::new();
        for f in &action.assign_custom_fields {
            if let Some(d) = cfi::field_def(conn, *f).await? {
                fields.push(d);
            }
        }
        for f in fields {
            let value = cf_value(action, f.id);
            let existing = cfi::live_for(conn, doc.id, f.id).await?;
            let empty = value.is_null() || value.as_str() == Some("");
            match existing {
                Some(mut inst) if !empty => {
                    cfi::set_value(&mut inst, &f.data_type, &value).map_err(ApiError::Uncaught)?;
                    cfi::save_full(conn, ctx, media, &inst, &f.data_type).await?;
                }
                Some(_) => {}
                None => {
                    cfi::create(conn, ctx, media, doc.id, &f, &value).await?;
                }
            }
        }
    }
    if let Some(t) = action.assign_title.as_ref().filter(|t| !t.is_empty()) {
        let corr = name_of(conn, "documents_correspondent", doc.correspondent_id).await?.map(|o| o.name);
        let dtype = name_of(conn, "documents_documenttype", doc.document_type_id).await?.map(|o| o.name);
        let owner: Option<String> = match doc.owner_id {
            // same SQL text as pl-accounts (binds int8) → likewise i64 (FINDINGS B3)
            Some(o) => sqlx::query_scalar("SELECT username FROM auth_user WHERE id = $1").bind(o).fetch_optional(&mut *conn).await?,
            None => None,
        };
        let p = pl_templating::WorkflowPlaceholders {
            correspondent_name: Some(corr.unwrap_or_default()),
            doc_type_name: Some(dtype.unwrap_or_default()),
            owner_username: Some(owner.unwrap_or_default()),
            local_added: doc.added.with_timezone(&tz).fixed_offset(),
            original_filename: doc.original_filename.clone().unwrap_or_default(),
            filename: doc.filename.clone().unwrap_or_default(),
            created: Some(doc.created),
            doc_title: Some(String::new()),
            doc_url: Some(String::new()),
            doc_id: Some(doc.id),
            custom_fields: pl_consumer::templating::custom_field_instances(conn, doc.id).await?,
        };
        if let Some(title) = pl_templating::render_workflow_title(t, &p) {
            doc.title = title;
        }
    }
    if has_assign_perms(action) {
        let spec = pl_authz::owned::PermissionsSpec {
            view: Some(pl_authz::owned::GranteesUpdate {
                users: Some(action.assign_view_users.clone()),
                groups: Some(action.assign_view_groups.clone()),
            }),
            change: Some(pl_authz::owned::GranteesUpdate {
                users: Some(action.assign_change_users.clone()),
                groups: Some(action.assign_change_groups.clone()),
            }),
        };
        pl_authz::owned::set_permissions_for_objects(conn, &spec, &pl_authz::models::DOCUMENT, &[doc.id], true)
            .await
            .map_err(|e| ApiError::Uncaught(format!("{e:?}")))?;
    }
    Ok(())
}

async fn removal(conn: &mut PgConnection, ctx: &LogCtx, media: &MediaCfg, action: &ActionDef, doc: &mut DocRow) -> Result<(), ApiError> {
    if action.remove_all_tags {
        docs::tags_clear(conn, ctx, Some(media), doc.id).await?;
    } else {
        let mut remove: Vec<i64> = Vec::new();
        for t in &action.remove_tags {
            remove.push(*t);
            if let Some((_, desc)) = docs::tag_tree(conn, *t).await? {
                remove.extend(desc);
            }
        }
        remove.sort_unstable();
        remove.dedup();
        if !remove.is_empty() {
            docs::tags_remove(conn, ctx, Some(media), doc.id, &remove).await?;
        }
    }
    if action.remove_all_correspondents || doc.correspondent_id.is_some_and(|c| action.remove_correspondents.contains(&c)) {
        doc.correspondent_id = None;
    }
    if action.remove_all_document_types || doc.document_type_id.is_some_and(|c| action.remove_document_types.contains(&c)) {
        doc.document_type_id = None;
    }
    if action.remove_all_storage_paths || doc.storage_path_id.is_some_and(|c| action.remove_storage_paths.contains(&c)) {
        doc.storage_path_id = None;
    }
    if action.remove_all_owners || doc.owner_id.is_some_and(|c| action.remove_owners.contains(&c)) {
        doc.owner_id = None;
    }
    if action.remove_all_permissions {
        let spec = pl_authz::owned::PermissionsSpec {
            view: Some(pl_authz::owned::GranteesUpdate { users: Some(vec![]), groups: Some(vec![]) }),
            change: Some(pl_authz::owned::GranteesUpdate { users: Some(vec![]), groups: Some(vec![]) }),
        };
        pl_authz::owned::set_permissions_for_objects(conn, &spec, &pl_authz::models::DOCUMENT, &[doc.id], false)
            .await
            .map_err(|e| ApiError::Uncaught(format!("{e:?}")))?;
    }
    if has_remove_perms(action) {
        for (ids, table, ident, codename) in [
            (&action.remove_view_users, "guardian_userobjectpermission", "user_id", "view_document"),
            (&action.remove_change_users, "guardian_userobjectpermission", "user_id", "change_document"),
            (&action.remove_view_groups, "guardian_groupobjectpermission", "group_id", "view_document"),
            (&action.remove_change_groups, "guardian_groupobjectpermission", "group_id", "change_document"),
        ] {
            if ids.is_empty() {
                continue;
            }
            let ids32: Vec<i32> = ids.iter().map(|i| *i as i32).collect();
            let sql = format!(
                "DELETE FROM {table} WHERE object_pk = $1 AND {ident} = ANY($2) AND permission_id IN \
                 (SELECT p.id FROM auth_permission p JOIN django_content_type c ON c.id = p.content_type_id \
                  WHERE p.codename = $3 AND c.app_label = 'documents' AND c.model = 'document') \
                 AND content_type_id = (SELECT id FROM django_content_type WHERE app_label = 'documents' AND model = 'document')"
            );
            sqlx::query(&sql).bind(doc.id.to_string()).bind(&ids32).bind(codename).execute(&mut *conn).await?;
        }
    }
    if action.remove_all_custom_fields {
        cfi::hard_delete_where(conn, ctx, &[doc.id], None).await?;
    } else if !action.remove_custom_fields.is_empty() {
        cfi::hard_delete_where(conn, ctx, &[doc.id], Some(&action.remove_custom_fields)).await?;
    }
    Ok(())
}
