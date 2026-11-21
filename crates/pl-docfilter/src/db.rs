//! Database accesses that upstream performs during filtering (custom field lookups,
//! back references for document links). As a trait so that the logic is testable without a DB.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};

use pl_core::sqlbind::SqlxBinder;
use sea_query::{Alias, Expr, ExprTrait, Order, PostgresQueryBuilder, Query};
use sqlx::Row;

use crate::error::FilterError;
use crate::sql::{CF, CFI, TextLookup, col, text_lookup};
use crate::textfold::{CALIBRATE_CHECKS_SQL, CALIBRATE_TABLE_SQL, UpperFold};

/// Row from `documents_customfield`.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomFieldDef {
    pub id: i64,
    pub name: String,
    /// `CustomField.FieldDataType` value, e.g. "string", "date", "documentlink".
    pub data_type: String,
    pub extra_data: Option<serde_json::Value>,
}

/// Read queries that the filters need.
pub trait FilterDb: Sync {
    /// `CustomField.objects.get(pk=id)`
    fn custom_field_by_id(&self, id: i64) -> impl Future<Output = Result<Option<CustomFieldDef>, FilterError>> + Send;

    /// `CustomField.objects.get(name=name)`
    fn custom_field_by_name(&self, name: &str) -> impl Future<Output = Result<Option<CustomFieldDef>, FilterError>> + Send;

    /// Several fields at once: `id IN ids OR name IN names` (preloading for the DSL).
    fn custom_fields_lookup(&self, ids: &[i64], names: &[String]) -> impl Future<Output = Result<Vec<CustomFieldDef>, FilterError>> + Send;

    /// `CustomField.objects.filter(extra_data__icontains=needle)` (order `created`)
    fn custom_fields_extra_data_icontains(&self, needle: &str) -> impl Future<Output = Result<Vec<CustomFieldDef>, FilterError>> + Send;

    /// `CustomFieldInstance.objects.filter(document_id__in=ids, field=field_id,
    /// value_document_ids__isnull=False)` → (document_id, value_document_ids)
    fn doc_link_instances(
        &self,
        field_id: i64,
        document_ids: &[i64],
    ) -> impl Future<Output = Result<Vec<(i64, serde_json::Value)>, FilterError>> + Send;

    /// Calibrated `UPPER` folding of the database for the fast text search (see
    /// [`crate::textfold`]); `None` = use the Django form. Default: `None`.
    fn upper_fold(&self) -> impl Future<Output = Option<Arc<UpperFold>>> + Send {
        async { None }
    }
}

/// Calibration per database (host:port/name), once per process.
fn fold_cache() -> &'static Mutex<HashMap<String, Option<Arc<UpperFold>>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<UpperFold>>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

async fn calibrate_fold(pool: &sqlx::PgPool) -> Result<Option<UpperFold>, sqlx::Error> {
    let ok: Option<bool> = sqlx::query_scalar(CALIBRATE_CHECKS_SQL).fetch_optional(pool).await?.flatten();
    if ok != Some(true) {
        return Ok(None);
    }
    let rows: Vec<(i32, String)> = sqlx::query_as(CALIBRATE_TABLE_SQL).fetch_all(pool).await?;
    Ok(UpperFold::from_db_rows(&rows))
}

fn cf_select() -> sea_query::SelectStatement {
    Query::select()
        .columns([
            (Alias::new(CF), Alias::new("id")),
            (Alias::new(CF), Alias::new("name")),
            (Alias::new(CF), Alias::new("data_type")),
            (Alias::new(CF), Alias::new("extra_data")),
        ])
        .from(Alias::new(CF))
        .to_owned()
}

fn row_to_cf(row: &sqlx::postgres::PgRow) -> Result<CustomFieldDef, FilterError> {
    Ok(CustomFieldDef {
        id: row.try_get::<i32, _>("id")? as i64,
        name: row.try_get("name")?,
        data_type: row.try_get("data_type")?,
        extra_data: row.try_get("extra_data")?,
    })
}

impl FilterDb for sqlx::PgPool {
    async fn upper_fold(&self) -> Option<Arc<UpperFold>> {
        let o = self.connect_options();
        let key = format!("{}:{}/{}", o.get_host(), o.get_port(), o.get_database().unwrap_or(""));
        if let Some(v) = fold_cache().lock().ok().and_then(|m| m.get(&key).cloned()) {
            return v;
        }
        // Calibration error → Django form (do not cache, next attempt later)
        let v = calibrate_fold(self).await.ok()?.map(Arc::new);
        if let Ok(mut m) = fold_cache().lock() {
            m.insert(key, v.clone());
        }
        v
    }

    async fn custom_field_by_id(&self, id: i64) -> Result<Option<CustomFieldDef>, FilterError> {
        cf_by_id(self, id).await
    }

    async fn custom_field_by_name(&self, name: &str) -> Result<Option<CustomFieldDef>, FilterError> {
        cf_by_name(self, name).await
    }

    async fn custom_fields_lookup(&self, ids: &[i64], names: &[String]) -> Result<Vec<CustomFieldDef>, FilterError> {
        cf_lookup(self, ids, names).await
    }

    async fn custom_fields_extra_data_icontains(&self, needle: &str) -> Result<Vec<CustomFieldDef>, FilterError> {
        cf_extra_data_icontains(self, needle).await
    }

    async fn doc_link_instances(&self, field_id: i64, document_ids: &[i64]) -> Result<Vec<(i64, serde_json::Value)>, FilterError> {
        link_instances(self, field_id, document_ids).await
    }
}

/// [`FilterDb`] over **one** connection (e.g. an open transaction: workflow triggers thus see
/// the not yet committed field values, like Django in `atomic()`). No text folding
/// (Django form), the calibration needs a pool.
pub struct ConnFilterDb<'c>(tokio::sync::Mutex<&'c mut sqlx::PgConnection>);

impl<'c> ConnFilterDb<'c> {
    pub fn new(conn: &'c mut sqlx::PgConnection) -> Self {
        ConnFilterDb(tokio::sync::Mutex::new(conn))
    }

    /// Return the connection.
    pub fn into_inner(self) -> &'c mut sqlx::PgConnection {
        self.0.into_inner()
    }
}

impl FilterDb for ConnFilterDb<'_> {
    async fn custom_field_by_id(&self, id: i64) -> Result<Option<CustomFieldDef>, FilterError> {
        let mut g = self.0.lock().await;
        cf_by_id(&mut **g, id).await
    }

    async fn custom_field_by_name(&self, name: &str) -> Result<Option<CustomFieldDef>, FilterError> {
        let mut g = self.0.lock().await;
        cf_by_name(&mut **g, name).await
    }

    async fn custom_fields_lookup(&self, ids: &[i64], names: &[String]) -> Result<Vec<CustomFieldDef>, FilterError> {
        let mut g = self.0.lock().await;
        cf_lookup(&mut **g, ids, names).await
    }

    async fn custom_fields_extra_data_icontains(&self, needle: &str) -> Result<Vec<CustomFieldDef>, FilterError> {
        let mut g = self.0.lock().await;
        cf_extra_data_icontains(&mut **g, needle).await
    }

    async fn doc_link_instances(&self, field_id: i64, document_ids: &[i64]) -> Result<Vec<(i64, serde_json::Value)>, FilterError> {
        let mut g = self.0.lock().await;
        link_instances(&mut **g, field_id, document_ids).await
    }
}

async fn cf_by_id<'e, E: sqlx::PgExecutor<'e>>(e: E, id: i64) -> Result<Option<CustomFieldDef>, FilterError> {
    // AutoField (int4): out of range → DoesNotExist (Django IntegerFieldOverflow)
    let Ok(id32) = i32::try_from(id) else {
        return Ok(None);
    };
    let (sql, values) = cf_select().and_where(col(CF, "id").eq(id32)).build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_optional(e).await?;
    row.as_ref().map(row_to_cf).transpose()
}

async fn cf_by_name<'e, E: sqlx::PgExecutor<'e>>(e: E, name: &str) -> Result<Option<CustomFieldDef>, FilterError> {
    let (sql, values) = cf_select().and_where(col(CF, "name").eq(name)).build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_optional(e).await?;
    row.as_ref().map(row_to_cf).transpose()
}

async fn cf_lookup<'e, E: sqlx::PgExecutor<'e>>(e: E, ids: &[i64], names: &[String]) -> Result<Vec<CustomFieldDef>, FilterError> {
    let ids: Vec<i32> = ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
    if ids.is_empty() && names.is_empty() {
        return Ok(Vec::new());
    }
    let mut cond = sea_query::Cond::any();
    if !ids.is_empty() {
        cond = cond.add(col(CF, "id").is_in(ids));
    }
    if !names.is_empty() {
        cond = cond.add(col(CF, "name").is_in(names.iter().map(|n| n.as_str())));
    }
    let (sql, values) = cf_select().cond_where(cond).build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(e).await?;
    rows.iter().map(row_to_cf).collect()
}

async fn cf_extra_data_icontains<'e, E: sqlx::PgExecutor<'e>>(e: E, needle: &str) -> Result<Vec<CustomFieldDef>, FilterError> {
    let (sql, values) = cf_select()
        .and_where(text_lookup(col(CF, "extra_data"), TextLookup::IContains, needle))
        .order_by((Alias::new(CF), Alias::new("created")), Order::Asc)
        .build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(e).await?;
    rows.iter().map(row_to_cf).collect()
}

async fn link_instances<'e, E: sqlx::PgExecutor<'e>>(
    e: E,
    field_id: i64,
    document_ids: &[i64],
) -> Result<Vec<(i64, serde_json::Value)>, FilterError> {
    let ids: Vec<i32> = document_ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
    let Ok(field32) = i32::try_from(field_id) else {
        return Ok(Vec::new());
    };
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let (sql, values) = Query::select()
        .columns([(Alias::new("dl"), Alias::new("document_id")), (Alias::new("dl"), Alias::new("value_document_ids"))])
        .from_as(Alias::new(CFI), Alias::new("dl"))
        .and_where(col("dl", "document_id").is_in(ids))
        .and_where(col("dl", "field_id").eq(field32))
        .and_where(col("dl", "value_document_ids").is_null().not())
        .and_where(Expr::col((Alias::new("dl"), Alias::new("deleted_at"))).is_null())
        .build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(e).await?;
    rows.iter()
        .map(|r| Ok((r.try_get::<i32, _>("document_id")? as i64, r.try_get::<serde_json::Value, _>("value_document_ids")?)))
        .collect()
}
