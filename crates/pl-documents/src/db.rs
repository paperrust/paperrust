//! Small DB helpers: sea-query → sqlx, robust column access. Errors are
//! `pl_core::ApiError` (500 with Django's generic page, cause only in the log).

use pl_core::ApiError;
use sea_query::{Alias, PostgresQueryBuilder, SelectStatement};
use pl_core::sqlbind::SqlxBinder;
use sqlx::Row;
use sqlx::postgres::PgRow;

pub type ApiResult<T> = Result<T, ApiError>;

pub fn a(name: &str) -> Alias {
    Alias::new(name)
}

pub async fn fetch_all(db: &sqlx::PgPool, q: &SelectStatement) -> ApiResult<Vec<PgRow>> {
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    Ok(sqlx::query_with(&sql, values).fetch_all(db).await?)
}

pub async fn fetch_optional(db: &sqlx::PgPool, q: &SelectStatement) -> ApiResult<Option<PgRow>> {
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    Ok(sqlx::query_with(&sql, values).fetch_optional(db).await?)
}

/// `col = ANY(string_to_array($1, ',')::bigint[])` – a single bind variable instead of one
/// per id (PostgreSQL allows at most 65535). Raw SQL, because sea-query without the
/// `postgres-array` feature does not support array binding here; the value is bound, not interpolated.
pub fn id_in_list(col: sea_query::SimpleExpr, ids: &[i64]) -> sea_query::SimpleExpr {
    let list: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
    sea_query::Expr::cust_with_exprs(
        "$1 = ANY(string_to_array($2, ',')::bigint[])",
        [col, sea_query::Expr::val(list.join(",")).into()],
    )
}

fn col_err(col: &str, e: sqlx::Error) -> ApiError {
    ApiError::internal(format!("column {col}: {e}"))
}

/// Integer column regardless of int2/int4/int8.
pub fn opt_i64(row: &PgRow, col: &str) -> ApiResult<Option<i64>> {
    if let Ok(v) = row.try_get::<Option<i64>, _>(col) {
        return Ok(v);
    }
    if let Ok(v) = row.try_get::<Option<i32>, _>(col) {
        return Ok(v.map(i64::from));
    }
    row.try_get::<Option<i16>, _>(col)
        .map(|v| v.map(i64::from))
        .map_err(|e| col_err(col, e))
}

pub fn i64_col(row: &PgRow, col: &str) -> ApiResult<i64> {
    opt_i64(row, col)?.ok_or_else(|| ApiError::internal(format!("column {col}: NULL")))
}

pub fn get<'r, T>(row: &'r PgRow, col: &str) -> ApiResult<T>
where
    T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get::<T, _>(col).map_err(|e| col_err(col, e))
}

/// ids (column `id`) of all rows of a query, in query order.
pub async fn id_list(db: &sqlx::PgPool, q: &SelectStatement) -> ApiResult<Vec<i64>> {
    fetch_all(db, q)
        .await?
        .iter()
        .map(|r| i64_col(r, "id"))
        .collect()
}
