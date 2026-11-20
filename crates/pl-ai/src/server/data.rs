//! Database access of the AI routes (PostgreSQL, schema of upstream v3.4.2).
//!
//! Permissions exclusively via `pl_authz` (`permitted_object_ids`, `permitted_document_ids`,
//! `restrict_to_owner_visible`); all values bound.

use std::collections::BTreeMap;

use pl_authz::models::{ObjectModel, CORRESPONDENT, DOCUMENT_TYPE, STORAGE_PATH, TAG};
use pl_authz::objects::{permitted_document_ids, permitted_object_ids, user_is_active_superuser, PermOpts};
use pl_authz::User;
use pl_core::ApiError;
use sea_query::{Alias, Expr, Order, PostgresQueryBuilder, Query, SelectStatement};
use pl_core::sqlbind::SqlxBinder;
use serde_json::Value;
use sqlx::{PgPool, Row};

use crate::classifier::NeighbourTaxonomy;
use crate::config::{AiConfig, AppConfigRow};

pub type DResult<T> = Result<T, ApiError>;

fn a(s: &str) -> Alias {
    Alias::new(s)
}

fn db_err(e: sqlx::Error) -> ApiError {
    ApiError::internal(e)
}

async fn fetch(db: &PgPool, q: &SelectStatement) -> DResult<Vec<sqlx::postgres::PgRow>> {
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).fetch_all(db).await.map_err(db_err)
}

/// Category (model + permission) for matching/candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cat {
    Tags,
    Correspondents,
    DocumentTypes,
    StoragePaths,
}

impl Cat {
    pub const ALL: [Cat; 4] = [Cat::Tags, Cat::Correspondents, Cat::DocumentTypes, Cat::StoragePaths];
    pub fn model(self) -> &'static ObjectModel {
        match self {
            Cat::Tags => &TAG,
            Cat::Correspondents => &CORRESPONDENT,
            Cat::DocumentTypes => &DOCUMENT_TYPE,
            Cat::StoragePaths => &STORAGE_PATH,
        }
    }
    pub fn perm(self) -> &'static str {
        match self {
            Cat::Tags => "view_tag",
            Cat::Correspondents => "view_correspondent",
            Cat::DocumentTypes => "view_documenttype",
            Cat::StoragePaths => "view_storagepath",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Cat::Tags => "tags",
            Cat::Correspondents => "correspondents",
            Cat::DocumentTypes => "document_types",
            Cat::StoragePaths => "storage_paths",
        }
    }
}

/// Effective `AIConfig` (environment + `paperless_applicationconfiguration`, first row).
pub async fn load_ai_config(db: &PgPool, env: &AiConfig) -> DResult<AiConfig> {
    let row = sqlx::query(
        "SELECT ai_enabled, llm_embedding_backend, llm_embedding_model, llm_embedding_api_key, llm_embedding_endpoint, \
         llm_embedding_chunk_size, llm_context_size, llm_backend, llm_model, llm_api_key, llm_endpoint, \
         llm_output_language, llm_request_timeout FROM paperless_applicationconfiguration ORDER BY id LIMIT 1",
    )
    .fetch_optional(db)
    .await
    .map_err(db_err)?;
    let Some(r) = row else { return Ok(env.clone()) };
    let cfg = AppConfigRow {
        ai_enabled: r.try_get("ai_enabled").ok().flatten(),
        llm_embedding_backend: r.try_get("llm_embedding_backend").ok().flatten(),
        llm_embedding_model: r.try_get("llm_embedding_model").ok().flatten(),
        llm_embedding_api_key: r.try_get("llm_embedding_api_key").ok().flatten(),
        llm_embedding_endpoint: r.try_get("llm_embedding_endpoint").ok().flatten(),
        llm_embedding_chunk_size: r.try_get::<Option<i16>, _>("llm_embedding_chunk_size").ok().flatten().map(i32::from),
        llm_context_size: r.try_get::<Option<i32>, _>("llm_context_size").ok().flatten().map(i64::from),
        llm_backend: r.try_get("llm_backend").ok().flatten(),
        llm_model: r.try_get("llm_model").ok().flatten(),
        llm_api_key: r.try_get("llm_api_key").ok().flatten(),
        llm_endpoint: r.try_get("llm_endpoint").ok().flatten(),
        llm_output_language: r.try_get("llm_output_language").ok().flatten(),
        llm_request_timeout: r.try_get::<Option<i16>, _>("llm_request_timeout").ok().flatten().map(i32::from),
    };
    Ok(env.with_app_config(&cfg))
}

/// `user.ui_settings.settings` (None without a row).
pub async fn ui_settings(db: &PgPool, user_id: i64) -> DResult<Option<Value>> {
    let r = sqlx::query("SELECT settings FROM documents_uisettings WHERE user_id = $1")
        .bind(user_id as i32)
        .fetch_optional(db)
        .await
        .map_err(db_err)?;
    Ok(r.and_then(|r| r.try_get::<Option<Value>, _>("settings").ok().flatten()))
}

/// SQL for `get_effective_content()` of a document (alias `d`).
pub const EFFECTIVE_CONTENT: &str = "CASE WHEN d.root_document_id IS NOT NULL THEN d.content ELSE COALESCE((SELECT v.content FROM documents_document v WHERE v.root_document_id = d.id AND v.deleted_at IS NULL ORDER BY v.version_index DESC NULLS LAST, v.id DESC LIMIT 1), d.content) END";

/// Document for AI routes (`Document.objects`, i.e. without trash).
#[derive(Debug, Clone)]
pub struct AiDoc {
    pub id: i64,
    pub title: String,
    pub filename: Option<String>,
    pub content: String,
    pub owner_id: Option<i64>,
    /// Root document for the permission check (`get_root_document`).
    pub root_id: i64,
    pub root_owner_id: Option<i64>,
    pub root_document_id: Option<i64>,
}

pub async fn load_doc(db: &PgPool, id: i64) -> DResult<Option<AiDoc>> {
    let sql = format!(
        "SELECT d.id, d.title, d.filename, d.owner_id, d.root_document_id, r.id AS r_id, r.owner_id AS r_owner, ({EFFECTIVE_CONTENT}) AS eff \
         FROM documents_document d LEFT JOIN documents_document r ON r.id = d.root_document_id \
         WHERE d.id = $1 AND d.deleted_at IS NULL"
    );
    let Some(r) = sqlx::query(&sql).bind(i32::try_from(id).unwrap_or(-1)).fetch_optional(db).await.map_err(db_err)? else {
        return Ok(None);
    };
    let g = |c: &str| r.try_get::<Option<i32>, _>(c).ok().flatten().map(i64::from);
    let id = g("id").unwrap_or(id);
    let root_document_id = g("root_document_id");
    let (root_id, root_owner_id) = match (root_document_id, g("r_id")) {
        (Some(_), Some(rid)) => (rid, g("r_owner")),
        _ => (id, g("owner_id")),
    };
    Ok(Some(AiDoc {
        id,
        title: r.try_get("title").unwrap_or_default(),
        filename: r.try_get("filename").ok().flatten(),
        content: r.try_get::<Option<String>, _>("eff").ok().flatten().unwrap_or_default(),
        owner_id: g("owner_id"),
        root_id,
        root_owner_id,
        root_document_id,
    }))
}

/// `list(permitted_document_ids(user))`.
pub async fn permitted_doc_ids(db: &PgPool, user: Option<&User>) -> DResult<Vec<i64>> {
    let q = permitted_document_ids(user, "view_document", false);
    let rows = fetch(db, &q).await?;
    Ok(rows.iter().filter_map(|r| r.try_get::<i32, _>(0).ok().map(i64::from)).collect())
}

/// Non-deleted documents from `ids` that `user` may see (`None` = no check).
pub async fn filter_permitted_docs(db: &PgPool, user: Option<&User>, ids: &[i64], check: bool) -> DResult<Vec<i64>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut q = Query::select();
    q.column((a("x"), a("id")))
        .from_as(a("documents_document"), a("x"))
        .and_where(Expr::col((a("x"), a("id"))).is_in(ids.iter().map(|i| *i as i32).collect::<Vec<_>>()))
        .and_where(Expr::col((a("x"), a("deleted_at"))).is_null());
    if check {
        q.and_where(Expr::col((a("x"), a("id"))).in_subquery(permitted_document_ids(user, "view_document", false)));
    }
    let rows = fetch(db, &q).await?;
    Ok(rows.iter().filter_map(|r| r.try_get::<i32, _>(0).ok().map(i64::from)).collect())
}

/// IDs of all documents in the trash (`Document.deleted_objects`).
pub async fn trashed_ids(db: &PgPool) -> DResult<Vec<i64>> {
    let rows = sqlx::query("SELECT id FROM documents_document WHERE deleted_at IS NOT NULL")
        .fetch_all(db)
        .await
        .map_err(db_err)?;
    Ok(rows.iter().filter_map(|r| r.try_get::<i32, _>(0).ok().map(i64::from)).collect())
}

/// `Document.objects.filter(pk__in=ids).prefetch_related(Prefetch("tags", Tag.objects.filter(is_inbox_tag=False)))`
/// in default order (`-created`), tags by name.
pub async fn neighbours(db: &PgPool, ids: &[i64]) -> DResult<Vec<NeighbourTaxonomy>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let ids32: Vec<i32> = ids.iter().map(|i| *i as i32).collect();
    let rows = sqlx::query(
        "SELECT d.id, d.document_type_id, d.correspondent_id, d.storage_path_id, \
         COALESCE((SELECT array_agg(t.id ORDER BY t.name, t.id) FROM documents_document_tags dt JOIN documents_tag t ON t.id = dt.tag_id \
                   WHERE dt.document_id = d.id AND NOT t.is_inbox_tag), '{}') AS tags \
         FROM documents_document d WHERE d.id = ANY($1) AND d.deleted_at IS NULL ORDER BY d.created DESC",
    )
    .bind(&ids32)
    .fetch_all(db)
    .await
    .map_err(db_err)?;
    Ok(rows
        .iter()
        .map(|r| {
            let g = |c: &str| r.try_get::<Option<i32>, _>(c).ok().flatten().map(i64::from);
            NeighbourTaxonomy {
                document_id: g("id").unwrap_or_default(),
                tag_ids: r.try_get::<Vec<i32>, _>("tags").unwrap_or_default().into_iter().map(i64::from).collect(),
                document_type_id: g("document_type_id"),
                correspondent_id: g("correspondent_id"),
                storage_path_id: g("storage_path_id"),
            }
        })
        .collect())
}

/// `restrict_queryset_to_owner_visible(model.objects.filter(pk__in=ids), user, perm)` → (id, name)
/// in model default order (`name`).
pub async fn visible_objects(db: &PgPool, user: Option<&User>, cat: Cat, ids: &[i64]) -> DResult<Vec<(i64, String)>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let m = cat.model();
    let mut q = Query::select();
    q.column((a("o"), a("id")))
        .column((a("o"), a("name")))
        .from_as(a(m.table), a("o"))
        .and_where(Expr::col((a("o"), a("id"))).is_in(ids.iter().map(|i| *i as i32).collect::<Vec<_>>()))
        .order_by((a("o"), a("name")), Order::Asc);
    if !user_is_active_superuser(user) {
        q.and_where(Expr::col((a("o"), a("id"))).in_subquery(permitted_object_ids(user, m, cat.perm(), &PermOpts::default())));
    }
    rows_id_name(fetch(db, &q).await?)
}

/// `model.objects.filter(pk__in=permitted_object_ids(user, model, perm))` → (id, name) by name.
pub async fn permitted_pool(db: &PgPool, user: Option<&User>, cat: Cat) -> DResult<Vec<(i64, String)>> {
    let m = cat.model();
    let mut q = Query::select();
    q.column((a("o"), a("id")))
        .column((a("o"), a("name")))
        .from_as(a(m.table), a("o"))
        .and_where(Expr::col((a("o"), a("id"))).in_subquery(permitted_object_ids(user, m, cat.perm(), &PermOpts::default())))
        .order_by((a("o"), a("name")), Order::Asc);
    rows_id_name(fetch(db, &q).await?)
}

fn rows_id_name(rows: Vec<sqlx::postgres::PgRow>) -> DResult<Vec<(i64, String)>> {
    Ok(rows
        .iter()
        .filter_map(|r| Some((i64::from(r.try_get::<i32, _>(0).ok()?), r.try_get::<String, _>(1).ok()?)))
        .collect())
}

/// Similar documents for the RAG context: id → (title, filename, effective content);
/// `Document.objects` + `permitted_document_ids` for non-superusers.
pub async fn context_docs(
    db: &PgPool,
    user: Option<&User>,
    ids: &[i64],
) -> DResult<BTreeMap<i64, (Option<String>, Option<String>, Option<String>)>> {
    if ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let ids32: Vec<i32> = ids.iter().map(|i| *i as i32).collect();
    let allowed: Option<Vec<i64>> = if user_is_active_superuser(user) { None } else { Some(filter_permitted_docs(db, user, ids, true).await?) };
    let sql = format!(
        "SELECT d.id, d.title, d.filename, ({EFFECTIVE_CONTENT}) AS eff FROM documents_document d WHERE d.id = ANY($1) AND d.deleted_at IS NULL"
    );
    let rows = sqlx::query(&sql).bind(&ids32).fetch_all(db).await.map_err(db_err)?;
    let mut out = BTreeMap::new();
    for r in rows {
        let Some(id) = r.try_get::<i32, _>("id").ok().map(i64::from) else { continue };
        if allowed.as_ref().is_some_and(|a| !a.contains(&id)) {
            continue;
        }
        out.insert(id, (r.try_get("title").ok(), r.try_get("filename").ok().flatten(), r.try_get("eff").ok().flatten()));
    }
    Ok(out)
}

/// (title, file name) of allowed documents (chat references).
pub async fn doc_titles(db: &PgPool, ids: &[i64]) -> DResult<BTreeMap<i64, (Option<String>, Option<String>)>> {
    if ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let ids32: Vec<i32> = ids.iter().map(|i| *i as i32).collect();
    let rows = sqlx::query("SELECT id, title, filename FROM documents_document WHERE id = ANY($1) AND deleted_at IS NULL")
        .bind(&ids32)
        .fetch_all(db)
        .await
        .map_err(db_err)?;
    Ok(rows
        .iter()
        .filter_map(|r| Some((i64::from(r.try_get::<i32, _>("id").ok()?), (r.try_get("title").ok(), r.try_get("filename").ok().flatten()))))
        .collect())
}

/// `root_document_ids([id])` (first root, otherwise the document itself).
pub async fn root_id(db: &PgPool, id: i64) -> DResult<i64> {
    let r = sqlx::query("SELECT COALESCE(root_document_id, id) AS r FROM documents_document WHERE id = $1 AND deleted_at IS NULL")
        .bind(id as i32)
        .fetch_optional(db)
        .await
        .map_err(db_err)?;
    Ok(r.and_then(|r| r.try_get::<i32, _>("r").ok()).map(i64::from).unwrap_or(id))
}
