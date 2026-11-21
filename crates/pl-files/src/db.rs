// upstream: src/documents/models.py@v3.4.2 (Document, ShareLink, ShareLinkBundle)
//           src/documents/versioning.py@v3.4.2 (get_latest_version_for_root, versions_newest_first)
//           src/paperless/models.py@v3.4.2 (ApplicationConfiguration.app_logo / remote_ocr_*)
//
// Read-only data access for the file routes. SQL is built with sea-query.

use chrono::{DateTime, NaiveDate, Utc};
use pl_core::sqlbind::SqlxBinder;
use sea_query::{Alias, Expr, JoinType, NullOrdering, Order, PostgresQueryBuilder, Query, SelectStatement};
use sqlx::{PgPool, Row};

fn a(s: &str) -> Alias {
    Alias::new(s)
}

/// The columns of `documents_document` the file routes need, plus the correspondent name.
#[derive(Debug, Clone)]
pub struct DocRow {
    pub id: i64,
    pub title: String,
    pub created: NaiveDate,
    pub modified: DateTime<Utc>,
    pub checksum: String,
    pub archive_checksum: Option<String>,
    pub mime_type: String,
    pub filename: Option<String>,
    pub archive_filename: Option<String>,
    pub owner_id: Option<i64>,
    pub root_document_id: Option<i64>,
    pub correspondent: Option<String>,
}

impl DocRow {
    pub fn has_archive_version(&self) -> bool {
        self.archive_filename.is_some()
    }
}

fn doc_select() -> SelectStatement {
    let mut q = Query::select();
    q.from_as(a("documents_document"), a("d"))
        .expr_as(Expr::col((a("d"), a("id"))).cast_as(a("BIGINT")), a("id"))
        .expr_as(Expr::col((a("d"), a("title"))), a("title"))
        .expr_as(Expr::col((a("d"), a("created"))), a("created"))
        .expr_as(Expr::col((a("d"), a("modified"))), a("modified"))
        .expr_as(Expr::col((a("d"), a("checksum"))), a("checksum"))
        .expr_as(Expr::col((a("d"), a("archive_checksum"))), a("archive_checksum"))
        .expr_as(Expr::col((a("d"), a("mime_type"))), a("mime_type"))
        .expr_as(Expr::col((a("d"), a("filename"))), a("filename"))
        .expr_as(Expr::col((a("d"), a("archive_filename"))), a("archive_filename"))
        .expr_as(Expr::col((a("d"), a("owner_id"))).cast_as(a("BIGINT")), a("owner_id"))
        .expr_as(Expr::col((a("d"), a("root_document_id"))).cast_as(a("BIGINT")), a("root_document_id"))
        .expr_as(Expr::col((a("c"), a("name"))), a("correspondent"))
        .join_as(
            JoinType::LeftJoin,
            a("documents_correspondent"),
            a("c"),
            Expr::col((a("c"), a("id"))).equals((a("d"), a("correspondent_id"))),
        );
    q
}

fn row_to_doc(r: &sqlx::postgres::PgRow) -> Result<DocRow, sqlx::Error> {
    Ok(DocRow {
        id: r.try_get("id")?,
        title: r.try_get("title")?,
        created: r.try_get("created")?,
        modified: r.try_get("modified")?,
        checksum: r.try_get("checksum")?,
        archive_checksum: r.try_get("archive_checksum")?,
        mime_type: r.try_get("mime_type")?,
        filename: r.try_get("filename")?,
        archive_filename: r.try_get("archive_filename")?,
        owner_id: r.try_get("owner_id")?,
        root_document_id: r.try_get("root_document_id")?,
        correspondent: r.try_get("correspondent")?,
    })
}

/// `Document.objects` / `Document.global_objects` `.filter(pk=id).first()`.
pub async fn load_doc(db: &PgPool, id: i64, include_deleted: bool) -> Result<Option<DocRow>, sqlx::Error> {
    let mut q = doc_select();
    q.and_where(Expr::col((a("d"), a("id"))).eq(id));
    if !include_deleted {
        q.and_where(Expr::col((a("d"), a("deleted_at"))).is_null());
    }
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_optional(db).await?;
    row.as_ref().map(row_to_doc).transpose()
}

/// `get_latest_version_for_root(root, include_deleted)` – newest version or the root itself.
pub async fn latest_version(db: &PgPool, root: &DocRow, include_deleted: bool) -> Result<DocRow, sqlx::Error> {
    let mut q = doc_select();
    q.and_where(Expr::col((a("d"), a("root_document_id"))).eq(root.id));
    if !include_deleted {
        q.and_where(Expr::col((a("d"), a("deleted_at"))).is_null());
    }
    q.order_by_with_nulls((a("d"), a("version_index")), Order::Desc, NullOrdering::Last).order_by((a("d"), a("id")), Order::Desc).limit(1);
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_optional(db).await?;
    match row.as_ref().map(row_to_doc).transpose()? {
        Some(d) => Ok(d),
        None => Ok(root.clone()),
    }
}

/// Documents for bulk download: `Document.objects.filter(pk__in=ids)` with Meta ordering `-created`.
pub async fn load_docs_ordered(db: &PgPool, ids: &[i64]) -> Result<Vec<DocRow>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut q = doc_select();
    q.and_where(Expr::col((a("d"), a("id"))).is_in(ids.iter().copied()))
        .and_where(Expr::col((a("d"), a("deleted_at"))).is_null())
        .order_by((a("d"), a("created")), Order::Desc);
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(db).await?;
    rows.iter().map(row_to_doc).collect()
}

/// `Document.objects.filter(id__in=documents).count()`.
pub async fn count_existing(db: &PgPool, ids: &[i64]) -> Result<i64, sqlx::Error> {
    if ids.is_empty() {
        return Ok(0);
    }
    let mut q = Query::select();
    q.expr_as(Expr::col(a("id")).count().cast_as(a("BIGINT")), a("n"))
        .from(a("documents_document"))
        .and_where(Expr::col(a("id")).is_in(ids.iter().copied()))
        .and_where(Expr::col(a("deleted_at")).is_null());
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_one(db).await?;
    row.try_get("n")
}

#[derive(Debug, Clone)]
pub struct ShareLinkRow {
    pub expiration: Option<DateTime<Utc>>,
    pub document_id: i64,
    pub file_version: String,
}

/// `ShareLink.objects.filter(slug=slug).first()` (soft-deleted links excluded).
pub async fn share_link(db: &PgPool, slug: &str) -> Result<Option<ShareLinkRow>, sqlx::Error> {
    let mut q = Query::select();
    q.column(a("expiration"))
        .expr_as(Expr::col(a("document_id")).cast_as(a("BIGINT")), a("document_id"))
        .column(a("file_version"))
        .from(a("documents_sharelink"))
        .and_where(Expr::col(a("slug")).eq(slug))
        .and_where(Expr::col(a("deleted_at")).is_null())
        .order_by(a("created"), Order::Asc)
        .limit(1);
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_optional(db).await?;
    match row {
        None => Ok(None),
        Some(r) => Ok(Some(ShareLinkRow {
            expiration: r.try_get("expiration")?,
            document_id: r.try_get("document_id")?,
            file_version: r.try_get("file_version")?,
        })),
    }
}

#[derive(Debug, Clone)]
pub struct BundleRow {
    pub slug: String,
    pub expiration: Option<DateTime<Utc>>,
    pub status: String,
    pub file_path: String,
}

/// `ShareLinkBundle.objects.filter(slug=slug).first()` (Meta ordering `-created`).
pub async fn share_link_bundle(db: &PgPool, slug: &str) -> Result<Option<BundleRow>, sqlx::Error> {
    let mut q = Query::select();
    q.columns([a("slug"), a("expiration"), a("status"), a("file_path")])
        .from(a("documents_sharelinkbundle"))
        .and_where(Expr::col(a("slug")).eq(slug))
        .order_by(a("created"), Order::Desc)
        .limit(1);
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_optional(db).await?;
    match row {
        None => Ok(None),
        Some(r) => Ok(Some(BundleRow {
            slug: r.try_get("slug")?,
            expiration: r.try_get("expiration")?,
            status: r.try_get("status")?,
            file_path: r.try_get("file_path")?,
        })),
    }
}

#[derive(Debug, Clone, Default)]
pub struct AppConfigRow {
    pub app_logo: Option<String>,
    pub remote_ocr_engine: Option<String>,
    pub remote_ocr_api_key: Option<String>,
    pub remote_ocr_endpoint: Option<String>,
}

/// `ApplicationConfiguration.objects.first()` (singleton, ordered by pk).
pub async fn app_config(db: &PgPool) -> Result<Option<AppConfigRow>, sqlx::Error> {
    let mut q = Query::select();
    q.columns([a("app_logo"), a("remote_ocr_engine"), a("remote_ocr_api_key"), a("remote_ocr_endpoint")])
        .from(a("paperless_applicationconfiguration"))
        .order_by(a("id"), Order::Asc)
        .limit(1);
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_optional(db).await?;
    match row {
        None => Ok(None),
        Some(r) => Ok(Some(AppConfigRow {
            app_logo: r.try_get("app_logo")?,
            remote_ocr_engine: r.try_get("remote_ocr_engine")?,
            remote_ocr_api_key: r.try_get("remote_ocr_api_key")?,
            remote_ocr_endpoint: r.try_get("remote_ocr_endpoint")?,
        })),
    }
}

/// `RemoteEngineConfig.from_app_config().engine_is_valid()` – DB value or env fallback
/// (Python `or`: empty strings fall back too).
pub fn remote_ocr_active(row: Option<&AppConfigRow>, cfg: &pl_core::Config) -> bool {
    fn pick(db: Option<&Option<String>>, env: &Option<String>) -> Option<String> {
        match db.and_then(|v| v.as_ref()).filter(|s| !s.is_empty()) {
            Some(s) => Some(s.clone()),
            None => env.clone(),
        }
    }
    let (env_engine, env_key, env_endpoint) = crate::config::remote_ocr_env(cfg);
    let engine = pick(row.map(|r| &r.remote_ocr_engine), &env_engine);
    let key = pick(row.map(|r| &r.remote_ocr_api_key), &env_key);
    let endpoint = pick(row.map(|r| &r.remote_ocr_endpoint), &env_endpoint);
    engine.as_deref() == Some("azureai") && key.is_some() && endpoint.is_some()
}
