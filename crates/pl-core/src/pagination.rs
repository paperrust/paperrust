//! Pagination like `StandardPagination`.
//!
//! upstream: src/paperless/views.py@v3.4.2 (`StandardPagination`: `page_size=25`,
//! `page_size_query_param="page_size"`, `max_page_size=100000`, v9 with `all`),
//! rest_framework/pagination.py@3.18.3 (`PageNumberPagination`), django/core/paginator.py@5.2
//! (`Paginator.validate_number`, `num_pages`, `allow_empty_first_page=True`).
//!
//! Behavior:
//! * `page` = **last** value of `?page=` (empty → 1, `last` → last page). Non-integer,
//!   `< 1` or `> num_pages` → 404 `{"detail": "Invalid page."}`.
//! * `page_size` = `_positive_int(strict=True, cutoff=100000)`, on error/0/negative → 25.
//! * `num_pages = max(1, ceil(count / page_size))` (an empty list has page 1).
//! * `next`/`previous` = `build_absolute_uri()` with `replace_query_param`/`remove_query_param`
//!   (all parameters sorted and re-encoded with `quote_plus`; `previous` on page 1 without `page`).
//! * Response: `count`, `next`, `previous`, [`all` only v9], `results`.

use crate::sqlbind::SqlxBinder;
use sea_query::{Alias, Asterisk, Expr, PostgresQueryBuilder, Query, SelectStatement, SimpleExpr};
use serde_json::{Map, Value};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};

use crate::{ApiError, QueryParams, RequestContext, urlenc};

/// `StandardPagination.page_size`.
pub const DEFAULT_PAGE_SIZE: u64 = 25;
/// `StandardPagination.max_page_size`.
pub const MAX_PAGE_SIZE: u64 = 100_000;
/// Column alias for the bundled total count in [`fetch_page`].
pub const TOTAL_COUNT_ALIAS: &str = "__pl_total_count";

/// Python `int(str)` (base 10): outer whitespace, sign, `_` between digits.
/// ASCII digits only (see Known differences).
pub fn python_int(s: &str) -> Option<i128> {
    let t = s.trim();
    let (neg, body) = match t.as_bytes().first()? {
        b'-' => (true, &t[1..]),
        b'+' => (false, &t[1..]),
        _ => (false, t),
    };
    if body.is_empty() || body.starts_with('_') || body.ends_with('_') || body.contains("__") {
        return None;
    }
    let mut v: i128 = 0;
    for b in body.bytes() {
        match b {
            b'_' => continue,
            b'0'..=b'9' => {
                v = v.checked_mul(10)?.checked_add((b - b'0') as i128)?;
            }
            _ => return None,
        }
    }
    Some(if neg { -v } else { v })
}

/// Requested page (before checking against the total count).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageParam {
    /// Integer (can be ≤ 0 → 404 later).
    Number(i128),
    /// `page=last`.
    Last,
    /// Not an integer → 404.
    Invalid,
}

/// Pagination parameters of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageRequest {
    pub page: PageParam,
    pub page_size: u64,
}

impl PageRequest {
    /// Reads `page`/`page_size` like `StandardPagination`.
    pub fn from_query(q: &QueryParams) -> Self {
        let page_size = q
            .get("page_size")
            .and_then(python_int)
            .filter(|v| *v > 0)
            .map(|v| (v.min(MAX_PAGE_SIZE as i128)) as u64)
            .unwrap_or(DEFAULT_PAGE_SIZE);
        let page = match q.get("page") {
            None | Some("") => PageParam::Number(1),
            Some("last") => PageParam::Last,
            Some(p) => match python_int(p) {
                Some(n) => PageParam::Number(n),
                None => PageParam::Invalid,
            },
        };
        Self { page, page_size }
    }

    /// `Paginator.validate_number` against a known total count.
    pub fn resolve(&self, count: i64) -> Result<PageInfo, ApiError> {
        let num_pages = num_pages(count, self.page_size);
        let page = match self.page {
            PageParam::Last => num_pages,
            PageParam::Invalid => return Err(ApiError::invalid_page()),
            PageParam::Number(n) if n < 1 || n > num_pages as i128 => return Err(ApiError::invalid_page()),
            PageParam::Number(n) => n as u64,
        };
        Ok(PageInfo { count, page, page_size: self.page_size })
    }

    /// Offset, if the page can be determined without the total count (number ≥ 1).
    pub fn offset_hint(&self) -> Option<u64> {
        match self.page {
            PageParam::Number(n) if n >= 1 => (n as u64).checked_sub(1)?.checked_mul(self.page_size),
            _ => None,
        }
    }
}

/// `Paginator.num_pages` with `allow_empty_first_page=True`, `orphans=0`.
pub fn num_pages(count: i64, page_size: u64) -> u64 {
    let hits = count.max(1) as u64;
    hits.div_ceil(page_size.max(1))
}

/// A valid page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageInfo {
    pub count: i64,
    /// 1-based.
    pub page: u64,
    pub page_size: u64,
}

impl PageInfo {
    pub fn num_pages(&self) -> u64 {
        num_pages(self.count, self.page_size)
    }
    pub fn has_next(&self) -> bool {
        self.page < self.num_pages()
    }
    pub fn has_previous(&self) -> bool {
        self.page > 1
    }
    pub fn offset(&self) -> u64 {
        (self.page - 1) * self.page_size
    }

    /// `get_next_link()` based on the absolute request URL.
    pub fn next_link(&self, absolute_uri: &str) -> Option<String> {
        self.has_next().then(|| urlenc::replace_query_param(absolute_uri, "page", &(self.page + 1).to_string()))
    }

    /// `get_previous_link()`.
    pub fn previous_link(&self, absolute_uri: &str) -> Option<String> {
        if !self.has_previous() {
            return None;
        }
        let p = self.page - 1;
        Some(if p == 1 {
            urlenc::remove_query_param(absolute_uri, "page")
        } else {
            urlenc::replace_query_param(absolute_uri, "page", &p.to_string())
        })
    }
}

/// `get_paginated_response(data)`: `count`, `next`, `previous`, `all` (only if `Some` –
/// the caller passes it when `ctx.version().includes_all()`), `results`.
pub fn paginated_body(ctx: &RequestContext, info: &PageInfo, results: Vec<Value>, all: Option<Vec<Value>>) -> Value {
    let uri = ctx.build_absolute_uri();
    paginated_body_for_uri(&uri, info, results, all)
}

/// Like [`paginated_body`], with an explicit absolute URL.
pub fn paginated_body_for_uri(absolute_uri: &str, info: &PageInfo, results: Vec<Value>, all: Option<Vec<Value>>) -> Value {
    let mut m = Map::new();
    m.insert("count".into(), Value::from(info.count));
    m.insert("next".into(), info.next_link(absolute_uri).map(Value::String).unwrap_or(Value::Null));
    m.insert("previous".into(), info.previous_link(absolute_uri).map(Value::String).unwrap_or(Value::Null));
    if let Some(all) = all {
        m.insert("all".into(), Value::Array(all));
    }
    m.insert("results".into(), Value::Array(results));
    Value::Object(m)
}

/// `SELECT COUNT(*) FROM (<select without ORDER BY/LIMIT/OFFSET>) AS __pl_count`.
pub fn count_statement(select: &SelectStatement) -> SelectStatement {
    let mut inner = select.clone();
    inner.clear_order_by();
    inner.reset_limit();
    inner.reset_offset();
    Query::select().expr(Expr::col(Asterisk).count()).from_subquery(inner, Alias::new("__pl_count")).to_owned()
}

/// Runs `COUNT(*)` over the (filtered) query.
pub async fn count(db: &PgPool, select: &SelectStatement) -> Result<i64, ApiError> {
    let (sql, values) = count_statement(select).build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_one(db).await?;
    Ok(row.try_get::<i64, _>(0)?)
}

/// Page + total count in **one** round trip.
///
/// Appends to `select` the column `(SELECT COUNT(*) FROM (<select without ORDER/LIMIT>) …) AS
/// __pl_total_count` (uncorrelated → PostgreSQL computes it once as an InitPlan) and
/// `LIMIT page_size OFFSET …`. `select` must already contain the desired sorting.
///
/// * Empty page at `OFFSET 0` → `count = 0` without a further query.
/// * Empty later page (or `page=last`/invalid) → separate `COUNT`, then check like Django
///   (page 1 with 0 hits is valid, otherwise 404 `Invalid page.`).
/// * Rows contain all columns of `select` plus [`TOTAL_COUNT_ALIAS`].
pub async fn fetch_page(db: &PgPool, select: &SelectStatement, req: &PageRequest) -> Result<(Vec<PgRow>, PageInfo), ApiError> {
    if let Some(offset) = req.offset_hint() {
        let mut q = select.clone();
        q.expr_as(SimpleExpr::SubQuery(None, Box::new(count_statement(select).into_sub_query_statement())), Alias::new(TOTAL_COUNT_ALIAS));
        q.limit(req.page_size).offset(offset);
        let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
        let rows = sqlx::query_with(&sql, values).fetch_all(db).await?;
        if let Some(first) = rows.first() {
            let total: i64 = first.try_get(TOTAL_COUNT_ALIAS)?;
            let info = req.resolve(total)?;
            return Ok((rows, info));
        }
        // Empty first page (LIMIT ≥ 1, OFFSET 0) ⇒ 0 hits: no second COUNT over the same
        // (possibly expensive) condition (phase 6, docs/contract/pgvalidate.md "Korrekturen").
        let total = if offset == 0 && req.page_size > 0 { 0 } else { count(db, select).await? };
        let info = req.resolve(total)?;
        return Ok((Vec::new(), info));
    }
    let total = count(db, select).await?;
    let info = req.resolve(total)?;
    let mut q = select.clone();
    q.limit(info.page_size).offset(info.offset());
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(db).await?;
    Ok((rows, info))
}

/// Like [`fetch_page`], but the total count as `COUNT(*) OVER ()` of the same query instead of an
/// independent subquery: **one** pass over the hits instead of two. For conditions that must
/// check every row anyway (e.g. substring search in the content); the page's plan stays
/// the same (full scan + top-N sort), the window function sits between scan and
/// sort. Empty/invalid pages like [`fetch_page`].
///
/// Precondition: `COUNT(*) OVER ()` counts the rows **before** `DISTINCT` and after `GROUP BY` per
/// group – only for queries without top-level `DISTINCT`/`GROUP BY`/`HAVING`/`UNION` is it
/// the hit count. If the query has one of them, it falls back to [`fetch_page`] (separate COUNT)
/// (security review final B-N3: maintenance risk covered instead of merely documented).
pub async fn fetch_page_windowed(db: &PgPool, select: &SelectStatement, req: &PageRequest) -> Result<(Vec<PgRow>, PageInfo), ApiError> {
    let Some(offset) = req.offset_hint() else {
        return fetch_page(db, select, req).await;
    };
    // Placeholders instead of values: user text cannot disturb the bracket/quote detection
    if !window_count_exact(&select.build(PostgresQueryBuilder).0) {
        return fetch_page(db, select, req).await;
    }
    let mut q = select.clone();
    q.expr_as(Expr::cust("COUNT(*) OVER ()"), Alias::new(TOTAL_COUNT_ALIAS));
    q.limit(req.page_size).offset(offset);
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(db).await?;
    if let Some(first) = rows.first() {
        let total: i64 = first.try_get(TOTAL_COUNT_ALIAS)?;
        let info = req.resolve(total)?;
        return Ok((rows, info));
    }
    let total = if offset == 0 && req.page_size > 0 { 0 } else { count(db, select).await? };
    let info = req.resolve(total)?;
    Ok((Vec::new(), info))
}

/// `COUNT(*) OVER ()` yields the hit count for `sql`: no `DISTINCT` directly after the outer
/// `SELECT`, no `GROUP BY`/`HAVING`/`UNION`/`INTERSECT`/`EXCEPT` outside of parentheses
/// (subqueries) and quoted strings/identifiers.
fn window_count_exact(sql: &str) -> bool {
    let head = sql.trim_start();
    if head.len() >= 15 && head[..15].eq_ignore_ascii_case("SELECT DISTINCT") {
        return false;
    }
    let mut top = String::with_capacity(sql.len());
    let (mut depth, mut quote) = (0usize, None::<char>);
    for c in sql.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None => match c {
                '\'' | '"' => quote = Some(c),
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ if depth == 0 => top.push(c.to_ascii_uppercase()),
                _ => {}
            },
        }
    }
    ![" GROUP BY ", " HAVING ", " UNION ", " INTERSECT ", " EXCEPT "].iter().any(|k| top.contains(k))
}

/// IDs of all hits in sort order (`get_all_result_ids`, v9 `all`).
/// `id_expr` e.g. `Expr::col((Document::Table, Document::Id))`.
pub async fn fetch_all_ids(db: &PgPool, select: &SelectStatement, id_expr: SimpleExpr) -> Result<Vec<i64>, ApiError> {
    let mut q = select.clone();
    q.clear_selects();
    q.expr(Expr::expr(id_expr).cast_as(Alias::new("bigint")));
    q.reset_limit();
    q.reset_offset();
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(db).await?;
    rows.iter().map(|r| r.try_get::<i64, _>(0).map_err(ApiError::from)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(q: &str) -> PageRequest {
        PageRequest::from_query(&QueryParams::parse(q))
    }

    /// B-N3: window count only without top-level DISTINCT/GROUP BY/UNION.
    #[test]
    fn window_count_precondition() {
        assert!(window_count_exact(r#"SELECT "id" FROM "t" WHERE "id" IN (SELECT "x" FROM "u" GROUP BY "x") ORDER BY "id""#));
        assert!(window_count_exact(r#"SELECT "id" FROM "t" WHERE "a" = 'x GROUP BY y'"#));
        assert!(window_count_exact(r#"SELECT "group by" FROM "t""#));
        assert!(!window_count_exact(r#"SELECT DISTINCT "id" FROM "t""#));
        assert!(!window_count_exact(r#"SELECT "a", COUNT(*) FROM "t" GROUP BY "a""#));
        assert!(!window_count_exact(r#"SELECT "a" FROM "t" UNION SELECT "b" FROM "u""#));
        let mut q = Query::select();
        q.column(Alias::new("id")).from(Alias::new("t")).distinct();
        assert!(!window_count_exact(&q.to_string(PostgresQueryBuilder)));
        let mut q = Query::select();
        q.column(Alias::new("id")).from(Alias::new("t")).group_by_col(Alias::new("id"));
        assert!(!window_count_exact(&q.to_string(PostgresQueryBuilder)));
    }

    #[test]
    fn params() {
        assert_eq!(req(""), PageRequest { page: PageParam::Number(1), page_size: 25 });
        assert_eq!(req("page=&page_size=0").page_size, 25);
        assert_eq!(req("page_size=-5").page_size, 25);
        assert_eq!(req("page_size=abc").page_size, 25);
        assert_eq!(req("page_size=999999999").page_size, 100_000);
        assert_eq!(req("page_size= 7 ").page_size, 7);
        assert_eq!(req("page=2&page=3").page, PageParam::Number(3));
        assert_eq!(req("page=last").page, PageParam::Last);
        assert_eq!(req("page=1.0").page, PageParam::Invalid);
        assert_eq!(req("page=1_0").page, PageParam::Number(10));
    }

    #[test]
    fn resolve_like_django() {
        assert_eq!(req("").resolve(0).unwrap().page, 1);
        assert_eq!(req("page=2").resolve(0).unwrap_err().body_json().unwrap()["detail"], "Invalid page.");
        assert!(req("page=0").resolve(10).is_err());
        assert!(req("page=x").resolve(10).is_err());
        assert_eq!(req("page=last&page_size=10").resolve(25).unwrap().page, 3);
        assert_eq!(req("page=last").resolve(0).unwrap().page, 1);
        assert!(req("page=4&page_size=10").resolve(30).is_err());
    }

    #[test]
    fn links() {
        let uri = "http://h/api/tags/?page_size=10&page=2&name__icontains=a%20b";
        let info = req("page_size=10&page=2").resolve(35).unwrap();
        assert_eq!(info.next_link(uri).unwrap(), "http://h/api/tags/?name__icontains=a+b&page=3&page_size=10");
        assert_eq!(info.previous_link(uri).unwrap(), "http://h/api/tags/?name__icontains=a+b&page_size=10");
        let info = req("page=1").resolve(10).unwrap();
        assert!(info.next_link(uri).is_none() && info.previous_link(uri).is_none());
        let v = paginated_body_for_uri("http://h/x/", &req("").resolve(30).unwrap(), vec![], Some(vec![Value::from(1)]));
        assert_eq!(v.to_string(), r#"{"count":30,"next":"http://h/x/?page=2","previous":null,"all":[1],"results":[]}"#);
    }

    #[test]
    fn count_sql() {
        let mut s = Query::select();
        s.column(Alias::new("id")).from(Alias::new("documents_tag")).order_by(Alias::new("name"), sea_query::Order::Asc).limit(5);
        let sql = count_statement(&s).to_string(PostgresQueryBuilder);
        assert_eq!(sql, r#"SELECT COUNT(*) FROM (SELECT "id" FROM "documents_tag") AS "__pl_count""#);
    }
}
