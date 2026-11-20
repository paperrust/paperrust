// upstream: src/documents/views.py@v3.4.2 (DocumentViewSet.update/partial_update/destroy, get_object,
//           _resolve_file_doc), src/documents/serialisers.py@v3.4.2 (DocumentSerializer.update,
//           OwnedObjectSerializer.update, DocumentUpdateFieldsModelSerializer.update,
//           CustomFieldInstanceSerializer.create), drf-writable-nested 0.7.2 (NestedUpdateMixin.update,
//           update_or_create_reverse_relations, delete_reverse_relations_if_need),
//           src/documents/versioning.py@v3.4.2 (get_latest_version_for_root, resolve_requested_version_for_root)
//
//! PUT/PATCH/DELETE `/api/documents/{id}/` with all side effects.

use std::collections::BTreeSet;

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sea_query::Expr;
use serde_json::{Map, Value, json};
use sqlx::PgConnection;

use pl_authz::AuthUser;
use pl_authz::models::DOCUMENT;
use pl_authz::perms::ObjRef;
use pl_core::{AppState, RequestContext};
use pl_documents::{SerializeAction, SerializeOpts};

use crate::auditlog::LogCtx;
use crate::cfi;
use crate::docs;
use crate::error::{ApiError, ApiResult};
use crate::files::MediaCfg;
use crate::serializer::{self, Outcome, Validated};
use crate::user::CurrentUser;
use crate::views::Peer;

/// Request context for auditlog (middleware data + settings).
pub fn log_ctx(state: &AppState, ctx: &RequestContext, auth: &AuthUser, peer: Option<std::net::SocketAddr>) -> LogCtx {
    let meta = crate::audit::RequestMeta::from_request(&ctx.headers, peer);
    let user = auth.user();
    let (actor_id, actor_email) =
        pl_core::client::audit_actor(user.id, &user.email, crate::user::request_user_source(auth.context().method));
    LogCtx::middleware(
        state.config().audit_log_enabled,
        actor_id.as_ref().map(|id| (id, actor_email.as_deref())),
        meta.remote_addr,
        meta.remote_port,
        meta.cid,
    )
}

/// `get_object_or_404`: Python `int()` of the URL pk (otherwise 404 without text).
pub fn parse_pk(pk: &str) -> Option<i64> {
    crate::drf::py_int_from_str(pk)
}

/// `self.get_object()` (filtered list + pk) and `check_object_permissions`.
async fn get_object(state: &AppState, ctx: &RequestContext, auth: &AuthUser, pk: &str) -> ApiResult<pl_consumer::db::DocRow> {
    let user = auth.user();
    let db = state.db();
    let mut q = pl_documents::listing::filtered_documents(db, *state.tz(), user, &ctx.query).await?;
    let id = parse_pk(pk).ok_or_else(|| ApiError::Core(pl_core::ApiError::not_found()))?;
    q.and_where(Expr::col((sea_query::Alias::new(pl_documents::listing::DOC), sea_query::Alias::new("id"))).eq(id));
    q.clear_order_by();
    if pl_documents::db::fetch_optional(db, &q).await?.is_none() {
        return Err(ApiError::Core(pl_core::ApiError::not_found_model("Document")));
    }
    let mut conn = db.acquire().await?;
    let Some(doc) = docs::load_live(&mut conn, id).await? else {
        return Err(ApiError::Core(pl_core::ApiError::not_found_model("Document")));
    };
    // FINDINGS B2: return the connection before the permission check fetches one from the pool itself
    drop(conn);
    auth.context()
        .check_object_perms(db, &DOCUMENT, ObjRef { id: doc.id, owner_id: doc.owner_id })
        .await
        .map_err(|e| ApiError::Core(e.into()))?;
    Ok(doc)
}

fn drf_ok(ctx: &RequestContext, v: Value) -> Response {
    pl_core::Drf::ok(v).indent(ctx.negotiation.indent).into_response()
}

/// PUT /api/documents/{id}/
pub async fn update_handler(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Peer(peer): Peer,
    Path(pk): Path<String>,
    body: Bytes,
) -> Response {
    match update(&state, &ctx, &auth, peer, &pk, &body, false).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

/// PATCH /api/documents/{id}/
pub async fn partial_update_handler(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Peer(peer): Peer,
    Path(pk): Path<String>,
    body: Bytes,
) -> Response {
    match update(&state, &ctx, &auth, peer, &pk, &body, true).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

/// `get_latest_version_for_root(root)` or `_resolve_file_doc` with `?version=`.
async fn content_doc_id(conn: &mut PgConnection, ctx: &RequestContext, root_id: i64) -> ApiResult<i64> {
    let latest = |include_deleted: bool| {
        format!(
            "SELECT id FROM documents_document WHERE root_document_id = $1 {} \
             ORDER BY version_index DESC NULLS LAST, id DESC LIMIT 1",
            if include_deleted { "" } else { "AND deleted_at IS NULL" }
        )
    };
    match ctx.query.get("version") {
        None => {
            let v: Option<i32> = sqlx::query_scalar(&latest(false)).bind(root_id as i32).fetch_optional(&mut *conn).await?;
            Ok(v.map(i64::from).unwrap_or(root_id))
        }
        Some("") => {
            let v: Option<i32> = sqlx::query_scalar(&latest(true)).bind(root_id as i32).fetch_optional(&mut *conn).await?;
            Ok(v.map(i64::from).unwrap_or(root_id))
        }
        Some(p) => {
            let Some(vid) = crate::drf::py_int_from_str(p) else {
                return Err(ApiError::Core(pl_core::ApiError::with_detail(StatusCode::NOT_FOUND, "Invalid version parameter")));
            };
            let row: Option<(i32, Option<i32>)> =
                sqlx::query_as("SELECT id, root_document_id FROM documents_document WHERE id = $1")
                    .bind(vid as i32)
                    .fetch_optional(&mut *conn)
                    .await?;
            match row {
                Some((id, root)) if i64::from(id) == root_id || root.map(i64::from) == Some(root_id) => Ok(i64::from(id)),
                _ => Err(ApiError::Core(pl_core::ApiError::not_found())),
            }
        }
    }
}

/// Scalar fields of `DocumentSerializer` with `(allow_null, allow_blank, required)` as DRF builds
/// them (model fields: `title`/`content` `blank=True`; `created` has a default; the FK fields are
/// declared with `allow_null=True`; `archive_serial_number` `null=True`; `owner`
/// `allow_null=True, required=False`; `created_date` `required=False`).
const DOC_SCALARS: &[(&str, bool, bool, bool)] = &[
    ("title", false, true, false),
    ("content", false, true, false),
    ("created", false, false, false),
    ("created_date", false, false, false),
    ("correspondent", true, false, true),
    ("document_type", true, false, true),
    ("storage_path", true, false, true),
    ("archive_serial_number", true, false, false),
    ("owner", true, false, false),
];

/// `DocumentSerializer` input from a form body, field by field like DRF's `get_value` for HTML
/// input (rest_framework 3.18.3 fields.py `Field.get_value`, relations.py
/// `ManyRelatedField.get_value`, serializers.py `Serializer.get_value`/`ListSerializer.get_value`,
/// utils/html.py):
/// * scalars: last value; `""` → `null` (allow_null) / kept (allow_blank) / skipped (not required);
/// * `tags`: `getlist`; missing on PUT → `[]` (clears the tags, like upstream);
/// * `remove_inbox_tags`: missing on PUT → `false` (`default_empty_html`), `""` → skipped;
/// * `custom_fields`: `parse_html_list` (`custom_fields[0]field=…`); plain `custom_fields=…` is
///   ignored like upstream;
/// * `set_permissions`: `parse_html_dict` – keys after `set_permissions.` (e.g. `view.users`) are
///   passed on unchanged, so validation reports them as unknown actions exactly as upstream.
pub(crate) fn document_form_data(form: &crate::form::FormData, partial: bool) -> Map<String, Value> {
    let mut m = Map::new();
    for &(name, allow_null, allow_blank, required) in DOC_SCALARS {
        let Some(v) = form.get(name) else { continue };
        if v.is_empty() && allow_null {
            m.insert(name.into(), if allow_blank { Value::String(String::new()) } else { Value::Null });
        } else if v.is_empty() && !required {
            if allow_blank {
                m.insert(name.into(), Value::String(String::new()));
            }
        } else {
            m.insert(name.into(), Value::String(v.to_owned()));
        }
    }
    let tags = form.get_list("tags");
    if !tags.is_empty() || !partial {
        m.insert("tags".into(), Value::Array(tags.into_iter().map(|t| Value::String(t.to_owned())).collect()));
    }
    match form.get("remove_inbox_tags") {
        None if !partial => {
            m.insert("remove_inbox_tags".into(), Value::Bool(false));
        }
        None => {}
        Some("") => {}
        Some(v) => {
            m.insert("remove_inbox_tags".into(), Value::String(v.to_owned()));
        }
    }
    if let Some(cf) = crate::form::parse_html_list(form, "custom_fields") {
        m.insert("custom_fields".into(), cf);
    }
    let mut perms = Map::new();
    for (k, _) in &form.0 {
        if let Some(rest) = k.strip_prefix("set_permissions.").filter(|r| !r.is_empty())
            && !perms.contains_key(rest)
        {
            let vals = form.get_list(k).into_iter().map(|v| Value::String(v.to_owned())).collect();
            perms.insert(rest.to_owned(), Value::Array(vals));
        }
    }
    if !perms.is_empty() {
        m.insert("set_permissions".into(), Value::Object(perms));
    }
    m
}

#[allow(clippy::too_many_arguments)]
async fn update(
    state: &AppState,
    ctx: &RequestContext,
    auth: &AuthUser,
    peer: Option<std::net::SocketAddr>,
    pk: &str,
    body: &Bytes,
    partial: bool,
) -> ApiResult<Response> {
    let user = auth.context().require_model_perms(&DOCUMENT).map_err(|e| ApiError::Core(e.into()))?.clone();
    // FormParser/MultiPartParser: `request.data` is a QueryDict; reduced to what the fields'
    // `get_value` yields for HTML input (see `document_form_data`).
    let root = get_object(state, ctx, auth, pk).await?;
    let form = crate::form::parse(&ctx.headers, body).await?;
    let data = match form {
        Some(f) => Value::Object(document_form_data(&f, partial)),
        None => crate::views::parse_json_body(&ctx.headers, body)?,
    };
    let data = match data {
        Value::Object(m) => m,
        Value::Null => return Err(ApiError::Uncaught("argument of type 'NoneType' is not iterable".into())),
        other => {
            return Err(ApiError::Validation(json!({"non_field_errors": [crate::views::not_a_dict_message(&other)]})));
        }
    };
    let media = MediaCfg::from_config(&state.live_config());
    let tz = *state.tz();
    let base_ctx = log_ctx(state, ctx, auth, peer);
    // FINDINGS B2: never fetch a second pool connection while this one is held. Step 1
    // (check, save) holds a connection and returns it before serializing
    // (`serialize_documents` fetches connections itself via `state`); step 2 (workflows,
    // WebSocket) then fetches one again and returns it before queue/index.
    let (content_doc, after) = {
        let mut pc = state.db().acquire().await?;
        let conn = &mut *pc;

        let content_doc = content_doc_id(conn, ctx, root.id).await?;
        let content_updated = data.contains_key("content");
        let updated_content = data.get("content").cloned();
        let mut sdata = data.clone();
        let mut spartial = partial;
        if content_updated && content_doc != root.id {
            if updated_content.as_ref().is_none_or(Value::is_null) {
                return Err(ApiError::Validation(json!({"content": ["This field may not be null."]})));
            }
            sdata.remove("content");
            spartial = true;
        }
        serializer::convert_old_created(&mut sdata, tz);
        let fields: Option<Vec<String>> =
            ctx.query.get("fields").filter(|f| !f.is_empty()).map(|f| f.split(',').map(str::to_string).collect());
        let vd = match serializer::validate(conn, &user, root.id, &sdata, spartial, fields.as_deref()).await? {
            Outcome::Invalid(e) => return Err(ApiError::Validation(e)),
            Outcome::Ok(v) => *v,
        };

        // PaperRust storage-location hook: new ASN and fields that the request sets itself (remember before the move)
        let new_asn = vd.archive_serial_number.flatten().filter(|a| Some(*a) != root.archive_serial_number);
        let own_fields: Vec<i64> = vd.custom_fields.iter().flatten().map(|i| i.field.id).collect();

        // perform_update → DocumentSerializer.update
        // s3 follow-up to FINDINGS B1/B4: lock the root document first, then write tags/fields/row –
        // in one transaction, fully retried on 40P01/40001 (see `in_root_tx`).
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let r: ApiResult<()> = async {
                let mut tx = begin_root_tx(conn, root.id).await?;
                apply_update(&mut tx, &base_ctx, &media, &user, &root, vd.clone(), &sdata, spartial).await?;
                tx.commit().await?;
                Ok(())
            }
            .await;
            if !retry_conflict(&r, attempt, root.id).await {
                break r?;
            }
        }

        if let Some(asn) = new_asn {
            let actx = if base_ctx.enabled { base_ctx.set_actor(user.id, &user.email) } else { base_ctx.clone() };
            set_asn_location(conn, &actx, &media, root.id, asn, &own_fields).await;
        }

        let after = base_ctx.after_set_actor();
        if content_updated && content_doc != root.id {
            let content = match updated_content.as_ref() {
                Some(Value::String(s)) => s.clone(),
                Some(v) => crate::drf::py_str(v),
                None => String::new(),
            };
            save_version_content(conn, &after, &media, root.id, content_doc, &content).await?;
        }
        (content_doc, after)
    };

    // refreshed_doc = get_queryset().get(pk=root.pk) → serialize (full_perms on PUT/PATCH)
    let mut opts = SerializeOpts::from_query(&ctx.query, SerializeAction::Other);
    opts.full_perms = true;
    let mut out = pl_documents::serialize_documents(state, &user, &[root.id], &opts).await?;
    let Some(mut response) = out.pop() else {
        return Err(ApiError::Uncaught("Document matching query does not exist.".into()));
    };

    let (outcome, live) = {
        let mut pc = state.db().acquire().await?;
        let conn = &mut *pc;
        if ctx.query.get("version").is_some()
            && let Some(obj) = response.as_object_mut()
            && obj.contains_key("content")
        {
            let c: Option<String> = sqlx::query_scalar("SELECT content FROM documents_document WHERE id = $1")
                .bind(content_doc as i32)
                .fetch_optional(&mut *conn)
                .await?;
            obj.insert("content".into(), c.map(Value::String).unwrap_or(Value::Null));
        }

        // document_updated → run_workflows_updated, send_websocket_document_updated (W3),
        // add_or_update_document_in_llm_index (via hooks, pl-ai)
        let mut attempt = 0u32;
        let outcome = loop {
            attempt += 1;
            let r = async {
                let mut tx = begin_root_tx(conn, root.id).await?;
                let o = crate::workflows::run_updated(&mut tx, &after, &media, tz, root.id).await?;
                tx.commit().await?;
                Ok(o)
            }
            .await;
            if !retry_conflict(&r, attempt, root.id).await {
                break r?;
            }
        };
        crate::hooks::send_websocket_document_updated(state, conn, root.id).await;
        let live = docs::load_live(conn, root.id).await?.is_some();
        (outcome, live)
    };
    crate::workflows::enqueue_outcome(state, &outcome).await;
    crate::hooks::document_updated(state, root.id).await;

    // Index: final state incl. deletion
    if live {
        crate::index::add_or_update(state, &[root.id]).await;
    } else {
        crate::index::remove(state, &[root.id]).await;
    }
    Ok(drf_ok(ctx, response))
}

/// Transaction for a write step on the document `root_id` that **locks the document row first**
/// (`FOR UPDATE`); the caller retries fully up to `MAX_ATTEMPTS` times on deadlock/serialization failure (40P01/40001)
/// ([`retry_conflict`]).
///
/// Why (s3 follow-up to FINDINGS B1/B4): in autocommit PATCH inserted a tag assignment (entry in the
/// uniqueness index) and only afterwards locked the document row via the sync trigger; ingesting
/// a version (or a workflow) held the document row and wanted to insert the same assignment
/// → deadlock, PATCH 500. With the row lock up front both only wait for each other.
/// Known difference: upstream writes without a surrounding transaction; if a step aborts (e.g.
/// validation error in `custom_fields` after saving), nothing remains half-saved here.
async fn begin_root_tx(conn: &mut PgConnection, root_id: i64) -> ApiResult<sqlx::Transaction<'_, sqlx::Postgres>> {
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
    pl_consumer::medialock::lock_document_rows(&mut tx, &[root_id]).await?;
    Ok(tx)
}

/// `true` (after a short pause) if `r` failed due to deadlock/serialization failure and one more
/// attempt is allowed; otherwise `false` (accept the result).
async fn retry_conflict<T>(r: &ApiResult<T>, attempt: u32, root_id: i64) -> bool {
    match r {
        Err(e) if e.is_retryable() && attempt < pl_consumer::db::MAX_ATTEMPTS => {
            tracing::warn!(target: "paperless.api", "document {root_id}: database conflict, retrying ({attempt})");
            tokio::time::sleep(pl_consumer::db::retry_backoff(attempt)).await;
            true
        }
        _ => false,
    }
}

/// Save the content of a version (`content_doc.save(update_fields=["content", "modified"])`).
///
/// Lock order as when ingesting a new version (s3 follow-up to FINDINGS B1): **first the root
/// document, then the version**, in its own transaction. Previously the `UPDATE` of the version
/// in autocommit first locked the version row and afterwards, via the sync trigger (version → root document),
/// the root document, while ingestion held the root document and locked the version when updating
/// the version file names → deadlock (40P01, PATCH 500 or ingestion FAILURE).
/// Deadlock/serialization failure are still retried up to `MAX_ATTEMPTS` times (the
/// transaction is complete).
pub(crate) async fn save_version_content(
    conn: &mut PgConnection,
    ctx: &LogCtx,
    media: &MediaCfg,
    root_id: i64,
    version_id: i64,
    content: &str,
) -> ApiResult<()> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let r: ApiResult<()> = async {
            let mut tx = sqlx::Connection::begin(&mut *conn).await?;
            lock_root_then_version(&mut tx, root_id, version_id).await?;
            if let Some(mut cd) = docs::load_global(&mut tx, version_id).await? {
                cd.content = content.to_string();
                docs::save(&mut tx, ctx, Some(media), &mut cd, Some(&["content", "modified"])).await?;
            }
            tx.commit().await?;
            Ok(())
        }
        .await;
        match r {
            Err(e) if e.is_retryable() && attempt < pl_consumer::db::MAX_ATTEMPTS => {
                tokio::time::sleep(pl_consumer::db::retry_backoff(attempt)).await;
            }
            r => return r,
        }
    }
}

/// Lock root document, then version (`FOR UPDATE`), see [`save_version_content`].
pub(crate) async fn lock_root_then_version(conn: &mut PgConnection, root_id: i64, version_id: i64) -> Result<(), sqlx::Error> {
    pl_consumer::medialock::lock_document_rows(conn, &[root_id]).await?;
    pl_consumer::medialock::lock_document_rows(conn, &[version_id]).await
}

/// PaperRust: set the storage location from the number range of the new ASN (`pl_consumer::asn_location`,
/// only with the "multiple number ranges" switch; a different value set by hand remains). Own
/// transaction; errors are only logged, the change to the document stays valid.
async fn set_asn_location(conn: &mut PgConnection, ctx: &LogCtx, media: &MediaCfg, doc_id: i64, asn: i64, own_fields: &[i64]) {
    let r: Result<(), String> = async {
        let mut tx = sqlx::Connection::begin(&mut *conn).await.map_err(|e| e.to_string())?;
        let res: Result<(), String> = async {
            let Some(p) = pl_consumer::asn_location::plan(&mut tx, doc_id, asn, own_fields).await.map_err(|e| e.to_string())? else {
                return Ok(());
            };
            let Some(f) = cfi::field_def(&mut tx, p.field_id).await.map_err(|e| e.to_string())? else { return Ok(()) };
            cfi::update_or_create(&mut tx, ctx, media, doc_id, &f, &Value::String(p.location.clone())).await.map_err(|e| format!("{e:?}"))?;
            tracing::info!(target: "paperrust.asn_location", "document {doc_id}: ASN {asn} → location {:?}", p.location);
            Ok(())
        }
        .await;
        match res {
            Ok(()) => tx.commit().await.map_err(|e| e.to_string()),
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
    }
    .await;
    if let Err(e) = r {
        tracing::warn!(target: "paperrust.asn_location", "document {doc_id}: could not set location for ASN {asn}: {e}");
    }
}

/// Tags hierarchy like `DocumentSerializer.update`.
async fn resolve_tags(conn: &mut PgConnection, prev: &BTreeSet<i64>, requested: &[i64]) -> Result<Vec<i64>, sqlx::Error> {
    let req: BTreeSet<i64> = requested.iter().copied().collect();
    let added: BTreeSet<i64> = req.difference(prev).copied().collect();
    let mut required_by_add: BTreeSet<i64> = added.clone();
    for t in &added {
        if let Some((anc, _)) = docs::tag_tree(conn, *t).await? {
            required_by_add.extend(anc);
        }
    }
    let removed: BTreeSet<i64> = prev.difference(&req).copied().collect();
    let mut blocked: BTreeSet<i64> = removed.clone();
    for t in &removed {
        if let Some((_, desc)) = docs::tag_tree(conn, *t).await? {
            blocked.extend(desc);
        }
    }
    for t in &required_by_add {
        blocked.remove(t);
    }
    let mut fin: BTreeSet<i64> = req.clone();
    for t in &req {
        if let Some((anc, _)) = docs::tag_tree(conn, *t).await? {
            fin.extend(anc);
        }
    }
    for t in &blocked {
        fin.remove(t);
    }
    Ok(fin.into_iter().collect())
}

/// `DocumentSerializer.update` (+ OwnedObjectSerializer, NestedUpdateMixin).
#[allow(clippy::too_many_arguments)]
async fn apply_update(
    conn: &mut PgConnection,
    base_ctx: &LogCtx,
    media: &MediaCfg,
    user: &CurrentUser,
    root: &pl_consumer::db::DocRow,
    mut vd: Validated,
    initial: &Map<String, Value>,
    partial: bool,
) -> ApiResult<()> {
    let doc_id = root.id;
    if let Some(cd) = vd.created_date.take() {
        if vd.created.is_none() {
            vd.created = Some(cd);
        }
        tracing::warn!(target: "paperless.serializers", "created_date is deprecated, use created instead");
    }
    // removed documentlink fields: dissolve symmetric links
    if let Some(items) = &vd.custom_fields {
        let live = cfi::live_for_document(conn, doc_id).await?;
        if !live.is_empty() {
            let incoming: Vec<i64> = items.iter().map(|i| i.field.id).collect();
            for c in &live {
                let Some(f) = cfi::field_def(conn, c.field_id).await? else { continue };
                if f.data_type != "documentlink" || incoming.contains(&f.id) {
                    continue;
                }
                if let Some(list) = c.value_document_ids.as_ref().filter(|v| !v.is_null()) {
                    let ids: Vec<i64> = list.as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
                    for d in ids {
                        cfi::remove_doclink(conn, base_ctx, media, doc_id, &f, d).await?;
                    }
                }
            }
        }
    }
    let current: BTreeSet<i64> = docs::tag_ids(conn, doc_id).await?.into_iter().collect();
    if let Some(req) = vd.tags.clone() {
        vd.tags = Some(resolve_tags(conn, &current, &req).await?);
    }
    if vd.remove_inbox_tags == Some(Some(true)) {
        let tags: Vec<i64> = vd.tags.clone().unwrap_or_else(|| current.iter().copied().collect());
        let mut keep: BTreeSet<i64> = BTreeSet::new();
        for t in &tags {
            if !current.contains(t) {
                keep.insert(*t);
                if let Some((anc, _)) = docs::tag_tree(conn, *t).await? {
                    keep.extend(anc);
                }
            }
        }
        let inbox: Vec<(i32,)> =
            sqlx::query_as("SELECT id FROM documents_tag WHERE is_inbox_tag ORDER BY name, id").fetch_all(&mut *conn).await?;
        let inbox: Vec<i64> = inbox.into_iter().map(|(i,)| i64::from(i)).filter(|i| !keep.contains(i)).collect();
        // hierarchy.with_descendants(inbox_ids) - keep_ids (upstream #14612)
        let mut remove: BTreeSet<i64> = inbox.iter().copied().collect();
        remove.extend(pl_consumer::db::tags_descendants(conn, &inbox).await?);
        let remove: BTreeSet<i64> = remove.into_iter().filter(|i| !keep.contains(i)).collect();
        vd.tags = Some(tags.into_iter().filter(|t| !remove.contains(t)).collect());
    }

    let ctx = if base_ctx.enabled { base_ctx.set_actor(user.id, &user.email) } else { base_ctx.clone() };

    // OwnedObjectSerializer.update
    let owner_changed = vd.owner.is_some_and(|o| o != root.owner_id);
    if (owner_changed || vd.set_permissions.is_some())
        && !(user.is_superuser || root.owner_id == Some(user.id) || root.owner_id.is_none())
    {
        return Err(ApiError::Core(pl_core::ApiError::permission_denied_msg("Insufficient permissions.")));
    }
    if let Some(spec) = &vd.set_permissions {
        pl_authz::owned::set_permissions_for_objects(conn, spec, &DOCUMENT, &[doc_id], false)
            .await
            .map_err(|e| ApiError::Core(e.into()))?;
    }

    // DocumentUpdateFieldsModelSerializer.update
    let Some(mut inst) = docs::load_global(conn, doc_id).await? else {
        return Err(ApiError::Uncaught("document vanished".into()));
    };
    // The instance comes from get_object(); fields since then (doclinks) only `modified`.
    let mut update_fields: Vec<&str> = Vec::new();
    if let Some(c) = vd.correspondent {
        inst.correspondent_id = c;
        update_fields.push("correspondent");
    }
    if let Some(c) = vd.document_type {
        inst.document_type_id = c;
        update_fields.push("document_type");
    }
    if let Some(c) = vd.storage_path {
        inst.storage_path_id = c;
        update_fields.push("storage_path");
    }
    if let Some(t) = &vd.title {
        inst.title = t.clone();
        update_fields.push("title");
    }
    if let Some(c) = &vd.content {
        inst.content = c.clone();
        update_fields.push("content");
    }
    if let Some(c) = vd.created {
        inst.created = c;
        update_fields.push("created");
    }
    if let Some(a) = vd.archive_serial_number {
        inst.archive_serial_number = a;
        update_fields.push("archive_serial_number");
    }
    if let Some(o) = vd.owner {
        inst.owner_id = o;
        update_fields.push("owner");
    }
    update_fields.push("modified");
    // Fields that get_object() loaded (stale), except the saved ones – write only
    // update_fields like Django.
    docs::save(conn, &ctx, Some(media), &mut inst, Some(&update_fields)).await?;
    if let Some(t) = &vd.tags {
        docs::tags_set(conn, &ctx, Some(media), doc_id, t).await?;
    }

    // update_or_create_reverse_relations (custom_fields)
    if vd.custom_fields.is_some()
        && let Some(Value::Array(raw_items)) = initial.get("custom_fields")
    {
        let mut errors: Vec<Value> = Vec::new();
        let mut current_ids: Vec<i64> = Vec::new();
        for raw in raw_items {
            // _get_related_pk: data.get('pk') or data.get('id')
            let rel_pk = raw
                .get("pk")
                .filter(|v| crate::drf::py_truthy(v))
                .or_else(|| raw.get("id").filter(|v| crate::drf::py_truthy(v)))
                .map(crate::drf::py_str);
            if let Some(p) = rel_pk
                && let Some(n) = crate::drf::py_int_from_str(&p)
                && cfi::load(conn, n).await?.is_some_and(|c| c.deleted_at.is_none())
            {
                // ModelSerializer.update → setattr(instance, "value", …) on a property
                return Err(ApiError::Uncaught("AttributeError: property 'value' has no setter".into()));
            }
            let _ = partial;
            match serializer::validate_cfi(conn, user, raw, false).await? {
                Err(e) => errors.push(e),
                Ok(item) => {
                    let value = item.value.clone().unwrap_or(Value::Null);
                    if item.field.data_type == "documentlink" {
                        let ids: Vec<i64> = value.as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
                        cfi::reflect_doclinks(conn, &ctx, media, doc_id, &item.field, &ids).await?;
                    }
                    let id = cfi::update_or_create(conn, &ctx, media, doc_id, &item.field, &value).await?;
                    current_ids.push(id);
                    errors.push(json!({}));
                }
            }
        }
        if errors.iter().any(|e| e.as_object().is_some_and(|o| !o.is_empty())) {
            return Err(ApiError::Validation(json!({"custom_fields": errors})));
        }
        // delete_reverse_relations_if_need: soft-delete remaining instances (each with its own transaction_id)
        for c in cfi::live_for_document(conn, doc_id).await? {
            if current_ids.contains(&c.id) {
                continue;
            }
            cfi::soft_delete(conn, &ctx, media, &c, uuid::Uuid::new_v4(), false).await?;
        }
    }
    // hard delete custom field instances that were soft deleted (outside of set_actor)
    let after = base_ctx.after_set_actor();
    let deleted = cfi::deleted_for_document(conn, doc_id).await?;
    cfi::hard_delete(conn, &after, &deleted).await?;
    Ok(())
}

/// DELETE /api/documents/{id}/
pub async fn destroy_handler(
    State(state): State<AppState>,
    ctx: RequestContext,
    auth: AuthUser,
    Peer(peer): Peer,
    Path(pk): Path<String>,
) -> Response {
    let r: ApiResult<Response> = async {
        auth.context().require_model_perms(&DOCUMENT).map_err(|e| ApiError::Core(e.into()))?;
        let doc = get_object(&state, &ctx, &auth, &pk).await?;
        crate::index::remove(&state, &[doc.id]).await;
        // super().destroy → get_object() again (same result) → perform_destroy
        let media = MediaCfg::from_config(&state.live_config());
        let lctx = log_ctx(&state, &ctx, &auth, peer);
        let mut conn = state.db().acquire().await?;
        match crate::softdelete::soft_delete_document_prefetched(&mut conn, &lctx, &media, doc.id).await {
            Ok(()) => Ok(pl_core::json::drf_no_content()),
            Err(e) => {
                tracing::error!(target: "paperless.api", "Error deleting document: {e:?}");
                Err(ApiError::BadRequestText("Error deleting document, check logs for more detail.".into()))
            }
        }
    }
    .await;
    match r {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

#[cfg(test)]
mod form_tests {
    use super::*;
    use crate::form::FormData;

    fn fd(pairs: &[(&str, &str)]) -> FormData {
        FormData(pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect())
    }

    #[test]
    fn put_form_like_drf() {
        let m = document_form_data(&fd(&[("title", "a"), ("title", "b"), ("correspondent", ""), ("created", ""), ("content", "")]), false);
        assert_eq!(
            Value::Object(m),
            json!({"title": "b", "content": "", "correspondent": null, "tags": [], "remove_inbox_tags": false})
        );
    }

    #[test]
    fn patch_form_like_drf() {
        let m = document_form_data(
            &fd(&[
                ("tags", "1"),
                ("tags", "2"),
                ("archive_serial_number", ""),
                ("remove_inbox_tags", "true"),
                ("custom_fields", "9"),
                ("custom_fields[0]field", "3"),
                ("custom_fields[0]value", "x"),
                ("set_permissions.view.users", "4"),
                ("set_permissions.view.users", "5"),
            ]),
            true,
        );
        assert_eq!(
            Value::Object(m),
            json!({
                "archive_serial_number": null,
                "tags": ["1", "2"],
                "remove_inbox_tags": "true",
                "custom_fields": [{"field": "3", "value": "x"}],
                "set_permissions": {"view.users": ["4", "5"]}
            })
        );
        assert_eq!(Value::Object(document_form_data(&fd(&[]), true)), json!({}));
    }
}
