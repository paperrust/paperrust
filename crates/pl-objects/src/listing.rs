//! Querysets of the four ViewSets: columns, annotations, FilterSets, ordering.
//!
//! upstream: src/documents/filters.py@v3.4.2 (CorrespondentFilterSet, TagFilterSet, DocumentTypeFilterSet,
//!           StoragePathFilterSet, PermittedObjectsFilter), src/documents/views.py@v3.4.2 (ViewSet querysets,
//!           ordering_fields), django-filter 26.2 (NumberFilter/BaseInFilter/CharFilter/BooleanWidget),
//!           rest_framework 3.18.3 filters.OrderingFilter, Django 5.2 postgresql lookup SQL.

use std::collections::{HashMap, HashSet};

use sea_query::{Alias, Expr, JoinType, Order, PostgresQueryBuilder, Query, SelectStatement, SimpleExpr};
use pl_core::sqlbind::SqlxBinder;
use serde_json::{Map, Value, json};
use sqlx::{PgPool, Row};

use pl_core::ApiError;
use crate::model::{Kind, ObjectRow, TagExtra};
use crate::perms;
use crate::text::is_py_space;
use pl_authz::User;

pub const ALIAS: &str = "o";

fn col(name: &str) -> Expr {
    Expr::col((Alias::new(ALIAS), Alias::new(name)))
}

/// `SELECT <columns>, document_count[, last_correspondence] FROM <table> o
/// [LEFT JOIN (<grouped counts>) pl_dc ON pl_dc.k = o.id]`.
/// `counters`: read `document_count` from the label counters (data model stage 2,
/// [`crate::counters::available`]).
pub fn select(kind: Kind, user: &User, with_count: bool, with_last_corr: bool, counters: bool) -> SelectStatement {
    select_counted(kind, user, with_count, with_last_corr, None, counters)
}

/// [`select`] with the count aggregation restricted to `count_ids` (rows outside get 0).
pub fn select_counted(
    kind: Kind,
    user: &User,
    with_count: bool,
    with_last_corr: bool,
    count_ids: Option<&[i64]>,
    counters: bool,
) -> SelectStatement {
    let mut q = Query::select();
    q.from_as(Alias::new(kind.table()), Alias::new(ALIAS));
    for c in ["id", "name", "match", "matching_algorithm", "is_insensitive", "owner_id"] {
        q.column((Alias::new(ALIAS), Alias::new(c)));
    }
    if kind == Kind::StoragePath {
        q.column((Alias::new(ALIAS), Alias::new("path")));
    }
    if kind == Kind::Tag {
        for c in ["color", "is_inbox_tag", "tn_parent_id"] {
            q.column((Alias::new(ALIAS), Alias::new(c)));
        }
    }
    if with_count || with_last_corr {
        let dc = perms::COUNT_ALIAS;
        q.join_subquery(
            JoinType::LeftJoin,
            perms::document_count_source_ext(user, kind, count_ids, with_last_corr, counters),
            Alias::new(dc),
            Expr::col((Alias::new(dc), Alias::new("k"))).equals((Alias::new(ALIAS), Alias::new("id"))),
        );
        if with_count {
            q.expr_as(Expr::cust(format!("COALESCE(\"{dc}\".\"n\", 0)")), Alias::new("document_count"));
        }
        if with_last_corr {
            q.expr_as(Expr::col((Alias::new(dc), Alias::new("lc"))), Alias::new("last_correspondence"));
        }
    }
    q
}

/// Restrict to objects visible with `codename` (`PermittedObjectsFilter`).
pub fn restrict_visible(q: &mut SelectStatement, kind: Kind, user: &User, codename: &'static str) {
    q.cond_where(perms::visible_cond(user, kind, codename, ALIAS));
}

pub fn decode(kind: Kind, r: &sqlx::postgres::PgRow, with_count: bool, with_last_corr: bool) -> Result<ObjectRow, sqlx::Error> {
    let tag = if kind == Kind::Tag {
        Some(TagExtra {
            color: r.try_get("color")?,
            is_inbox_tag: r.try_get("is_inbox_tag")?,
            tn_parent_id: r.try_get::<Option<i32>, _>("tn_parent_id")?.map(i64::from),
        })
    } else {
        None
    };
    Ok(ObjectRow {
        id: r.try_get::<i32, _>("id")? as i64,
        name: r.try_get("name")?,
        r#match: r.try_get("match")?,
        matching_algorithm: r.try_get("matching_algorithm")?,
        is_insensitive: r.try_get("is_insensitive")?,
        owner_id: r.try_get::<Option<i32>, _>("owner_id")?.map(i64::from),
        document_count: if with_count { Some(r.try_get("document_count")?) } else { None },
        last_correspondence: if with_last_corr { Some(r.try_get("last_correspondence")?) } else { None },
        path: if kind == Kind::StoragePath { Some(r.try_get("path")?) } else { None },
        tag,
    })
}

pub async fn fetch(
    db: &PgPool,
    kind: Kind,
    q: &SelectStatement,
    with_count: bool,
    with_last_corr: bool,
) -> Result<Vec<ObjectRow>, ApiError> {
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(db).await?;
    rows.iter()
        .map(|r| decode(kind, r, with_count, with_last_corr).map_err(ApiError::from))
        .collect()
}

/// Up to this many ids the count aggregation filters by id in SQL; above it counts all objects
/// (see [`fetch_counts_ext`]).
pub const COUNT_IDS_IN_SQL_MAX: usize = 200;

/// `document_count` of the objects `ids` (objects without counted documents are absent → 0).
pub async fn fetch_counts(db: &PgPool, kind: Kind, user: &User, ids: &[i64]) -> Result<HashMap<i64, i64>, ApiError> {
    Ok(fetch_counts_ext(db, kind, user, ids, false).await?.into_iter().map(|(k, (n, _))| (k, n)).collect())
}

/// [`fetch_counts`] plus `last_correspondence` (`MAX(created)`, same filter) when `with_last`.
pub async fn fetch_counts_ext(
    db: &PgPool,
    kind: Kind,
    user: &User,
    ids: &[i64],
    with_last: bool,
) -> Result<HashMap<i64, (i64, Option<chrono::NaiveDate>)>, ApiError> {
    let mut out = HashMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    // Many ids (e.g. `page_size=100000`): one bind parameter per id made the aggregation ~12×
    // slower than counting every object (1 Mio. documents, 5000 correspondents: 9.4 s vs 0.8 s).
    // Count all groups instead and keep the requested ones (same values per object).
    let filter_in_sql = ids.len() <= COUNT_IDS_IN_SQL_MAX;
    let wanted: HashSet<i64> = if filter_in_sql { HashSet::new() } else { ids.iter().copied().collect() };
    let counters = crate::counters::available(db).await;
    let q = perms::document_count_source_ext(user, kind, filter_in_sql.then_some(ids), with_last, counters);
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(db).await?;
    for r in &rows {
        let k: i64 = match r.try_get::<i32, _>("k") {
            Ok(v) => v.into(),
            Err(_) => r.try_get::<i64, _>("k")?,
        };
        if !filter_in_sql && !wanted.contains(&k) {
            continue;
        }
        let lc = if with_last { r.try_get::<Option<chrono::NaiveDate>, _>("lc")? } else { None };
        out.insert(k, (r.try_get::<i64, _>("n")?, lc));
    }
    Ok(out)
}

/// Fill `document_count` (and `last_correspondence` when `with_last`) of `rows` from counts
/// fetched for exactly these rows (PERF-SCALE #3b: count only what is shown).
pub async fn fill_counts(db: &PgPool, kind: Kind, user: &User, rows: &mut [ObjectRow], with_last: bool) -> Result<(), ApiError> {
    let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let counts = fetch_counts_ext(db, kind, user, &ids, with_last).await?;
    for r in rows {
        let (n, lc) = counts.get(&r.id).copied().unwrap_or((0, None));
        r.document_count = Some(n);
        if with_last {
            r.last_correspondence = Some(lc);
        }
    }
    Ok(())
}

/// The ordering needs the count annotation inside the list query (`document_count`,
/// `last_correspondence`); otherwise counts are fetched for the shown rows only.
pub fn ordering_needs_counts(terms: &Option<Vec<OrderTerm>>) -> bool {
    terms.as_ref().is_some_and(|ts| ts.iter().any(|t| t.field == "document_count" || t.field == "last_correspondence"))
}

// ----------------------------------------------------------------------------------------------
// FilterSets
// ----------------------------------------------------------------------------------------------

/// Where filter values come from: the query string (`QueryDict`, strings) or a JSON dict
/// (`bulk_edit_objects.filters`).
pub enum FilterData<'a> {
    Query(&'a [(String, String)]),
    Json(&'a Map<String, Value>),
}

impl FilterData<'_> {
    fn get(&self, name: &str) -> Option<Value> {
        match self {
            FilterData::Query(q) => q.iter().rev().find(|(k, _)| k == name).map(|(_, v)| Value::String(v.clone())),
            FilterData::Json(m) => m.get(name).cloned(),
        }
    }
    /// Strict mode (DjangoFilterBackend raises 400) vs. bound FilterSet `.qs` (invalid fields ignored).
    fn strict(&self) -> bool {
        matches!(self, FilterData::Query(_))
    }
}

/// Result of Django `forms.DecimalField.clean` for an id filter.
#[derive(Debug, PartialEq)]
pub enum Num {
    Empty,
    /// Valid number, as `int(Decimal)` (truncated toward zero; fits i64).
    Int(i64),
    /// Valid number outside the integer range (`IntegerFieldOverflow` → no match).
    NoMatch,
    Invalid,
}

/// `forms.DecimalField.to_python` + `validate` (Python `Decimal` syntax, finite only).
pub fn parse_decimal(raw: &str) -> Num {
    let s = raw.trim_matches(is_py_space);
    if s.is_empty() {
        return Num::Empty;
    }
    let b = s.as_bytes();
    let mut i = 0;
    let mut neg = false;
    if b[0] == b'+' || b[0] == b'-' {
        neg = b[0] == b'-';
        i = 1;
    }
    let mut int_digits = String::new();
    let mut frac_digits = String::new();
    let take_digits = |i: &mut usize, out: &mut String| -> bool {
        let start = *i;
        let mut last_us = false;
        while *i < b.len() && (b[*i].is_ascii_digit() || b[*i] == b'_') {
            if b[*i] == b'_' {
                if *i == start || last_us {
                    return false;
                }
                last_us = true;
            } else {
                out.push(b[*i] as char);
                last_us = false;
            }
            *i += 1;
        }
        !last_us
    };
    if !take_digits(&mut i, &mut int_digits) {
        return Num::Invalid;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        if !take_digits(&mut i, &mut frac_digits) {
            return Num::Invalid;
        }
    }
    if int_digits.is_empty() && frac_digits.is_empty() {
        return Num::Invalid; // also covers Infinity/NaN (non-finite → "Enter a number.")
    }
    let mut exp: i64 = 0;
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        let mut eneg = false;
        if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
            eneg = b[i] == b'-';
            i += 1;
        }
        let mut ed = String::new();
        if !take_digits(&mut i, &mut ed) || ed.is_empty() {
            return Num::Invalid;
        }
        exp = ed.parse::<i64>().unwrap_or(i64::MAX / 4);
        if eneg {
            exp = -exp;
        }
    }
    if i != b.len() {
        return Num::Invalid;
    }
    // value = digits * 10^(exp - len(frac))
    let mut digits = format!("{int_digits}{frac_digits}");
    let mut e = exp - frac_digits.len() as i64;
    while e < 0 && digits.ends_with('0') {
        digits.pop();
        e += 1;
    }
    let mut digits = digits.trim_start_matches('0').to_string();
    if e < 0 {
        // `IntegerField.get_prep_value` → `int(Decimal)`: truncate toward zero ("1.5" → 1),
        // parity run 2026-11-08 `tags.list.id-decimal`
        let cut = e.unsigned_abs() as usize;
        if cut >= digits.len() {
            return Num::Int(0);
        }
        digits.truncate(digits.len() - cut); // ASCII digits only
        e = 0;
    }
    if digits.is_empty() {
        return Num::Int(0);
    }
    if digits.len() as i64 + e > 18 {
        return Num::NoMatch;
    }
    let full = format!("{digits}{}", "0".repeat(e as usize));
    match full.parse::<i64>() {
        Ok(v) => Num::Int(if neg { -v } else { v }),
        Err(_) => Num::NoMatch,
    }
}

/// `prep_for_like_query`.
fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

fn value_to_form_str(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        other => Some(crate::text::py_str(other)),
    }
}

/// Apply the FilterSet of `kind`. In strict mode invalid values give the django-filter 400.
pub fn apply_filters(q: &mut SelectStatement, kind: Kind, data: &FilterData) -> Result<(), ApiError> {
    let mut errors = Map::new();
    // id (exact) – NumberFilter
    if let Some(v) = data.get("id")
        && let Some(s) = value_to_form_str(&v) {
            match parse_decimal(&s) {
                Num::Empty => {}
                Num::Int(i) => {
                    q.and_where(col("id").eq(i));
                }
                Num::NoMatch => {
                    q.and_where(Expr::cust("FALSE"));
                }
                Num::Invalid => {
                    errors.insert("id".into(), json!(["Enter a number."]));
                }
            }
        }
    // id__in – BaseInFilter(NumberFilter): BaseCSVWidget + BaseCSVField(DecimalField)
    if let Some(v) = data.get("id__in") {
        let items: Option<Vec<String>> = match &v {
            Value::Null => None,
            Value::String(s) if s.is_empty() => Some(vec![]),
            Value::String(s) => Some(s.split(',').map(|x| x.to_string()).collect()),
            Value::Array(a) => Some(a.iter().filter_map(value_to_form_str).collect()),
            // `value.split(",")` on a non-string → AttributeError upstream (500)
            _ => return Err(ApiError::internal("id__in filter value is not a string")),
        };
        if let Some(items) = items
            && !items.is_empty() {
                let mut ids: Vec<i64> = vec![];
                let mut bad = false;
                for it in &items {
                    match parse_decimal(it) {
                        Num::Empty | Num::NoMatch => {}
                        Num::Int(i) => ids.push(i),
                        Num::Invalid => bad = true,
                    }
                }
                if bad {
                    errors.insert("id__in".into(), json!(["Enter a number."]));
                } else if ids.is_empty() {
                    q.and_where(Expr::cust("FALSE"));
                } else {
                    q.and_where(col("id").is_in(ids));
                }
            }
    }
    for field in kind.char_filter_fields() {
        for lookup in ["istartswith", "iendswith", "icontains", "iexact"] {
            let name = format!("{field}__{lookup}");
            let Some(v) = data.get(&name) else { continue };
            let Some(s) = value_to_form_str(&v) else { continue };
            let s = s.trim_matches(is_py_space).to_string();
            if s.is_empty() {
                continue;
            }
            // Django: UPPER("col"::text) LIKE UPPER(%s) / = UPPER(%s)
            let lhs = format!("UPPER({ALIAS}.\"{field}\"::text)");
            let (op, pat) = match lookup {
                "istartswith" => ("LIKE", format!("{}%", like_escape(&s))),
                "iendswith" => ("LIKE", format!("%{}", like_escape(&s))),
                "icontains" => ("LIKE", format!("%{}%", like_escape(&s))),
                _ => ("=", s),
            };
            q.and_where(Expr::cust_with_values(format!("{lhs} {op} UPPER($1)"), [pat]));
        }
    }
    if kind == Kind::Tag
        && let Some(v) = data.get("is_root") {
            let b = match &v {
                Value::String(s) => match s.to_lowercase().as_str() {
                    "1" | "true" => Some(true),
                    "0" | "false" => Some(false),
                    _ => None,
                },
                Value::Bool(b) => Some(*b),
                Value::Number(n) if n.as_f64() == Some(1.0) => Some(true),
                Value::Number(n) if n.as_f64() == Some(0.0) => Some(false),
                _ => None,
            };
            match b {
                Some(true) => {
                    q.and_where(col("tn_parent_id").is_null());
                }
                Some(false) => {
                    q.and_where(col("tn_parent_id").is_not_null());
                }
                None => {}
            }
        }
    if !errors.is_empty() && data.strict() {
        return Err(ApiError::validation(Value::Object(errors)));
    }
    Ok(())
}

// ----------------------------------------------------------------------------------------------
// Ordering
// ----------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct OrderTerm {
    pub field: String,
    pub desc: bool,
}

/// `OrderingFilter.get_ordering` (`None` = fall back to `Lower("name")`).
/// `last_corr_valid`: `last_correspondence` is in `ordering_fields` (correspondents).
pub fn ordering_terms(kind: Kind, param: Option<&str>, last_corr_valid: bool) -> Option<Vec<OrderTerm>> {
    let p = param?;
    if p.is_empty() {
        return None;
    }
    let terms: Vec<OrderTerm> = p
        .split(',')
        .map(|t| t.trim())
        .filter(|t| {
            let f = t.strip_prefix('-').unwrap_or(t);
            kind.ordering_fields().contains(&f) || (last_corr_valid && f == "last_correspondence")
        })
        .map(|t| OrderTerm { field: t.trim_start_matches('-').to_string(), desc: t.starts_with('-') })
        .collect();
    if terms.is_empty() { None } else { Some(terms) }
}

/// Apply ordering. `annotated_last`: whether `last_correspondence` exists in the query.
pub fn apply_ordering(q: &mut SelectStatement, terms: &Option<Vec<OrderTerm>>, annotated_last: bool) -> Result<(), ApiError> {
    match terms {
        None => {
            q.order_by_expr(Expr::cust(format!("LOWER({ALIAS}.name)")), Order::Asc);
        }
        Some(ts) => {
            for t in ts {
                let ord = if t.desc { Order::Desc } else { Order::Asc };
                let e: SimpleExpr = match t.field.as_str() {
                    "document_count" => Expr::cust("document_count"),
                    "last_correspondence" => {
                        if !annotated_last {
                            // FieldError: Cannot resolve keyword 'last_correspondence' (upstream 500)
                            return Err(ApiError::internal("ordering by unannotated last_correspondence"));
                        }
                        Expr::cust("last_correspondence")
                    }
                    f => col(f).into(),
                };
                q.order_by_expr(e, ord);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimal_parsing() {
        assert_eq!(parse_decimal("5"), Num::Int(5));
        assert_eq!(parse_decimal(" 5.0 "), Num::Int(5));
        assert_eq!(parse_decimal("1e3"), Num::Int(1000));
        // parity run 2026-11-08 `tags.list.id-decimal`: int(Decimal("1.5")) == 1
        assert_eq!(parse_decimal("1.5"), Num::Int(1));
        assert_eq!(parse_decimal("-1.9"), Num::Int(-1));
        assert_eq!(parse_decimal("0.5"), Num::Int(0));
        assert_eq!(parse_decimal("12.345e1"), Num::Int(123));
        assert_eq!(parse_decimal("1e30"), Num::NoMatch);
        assert_eq!(parse_decimal("1_0"), Num::Int(10));
        assert_eq!(parse_decimal(""), Num::Empty);
        assert_eq!(parse_decimal("abc"), Num::Invalid);
        assert_eq!(parse_decimal("NaN"), Num::Invalid);
        assert_eq!(parse_decimal("1e"), Num::Invalid);
        assert_eq!(parse_decimal(".5e1"), Num::Int(5));
        assert_eq!(parse_decimal("-0"), Num::Int(0));
    }

    #[test]
    fn ordering() {
        assert_eq!(ordering_terms(Kind::Tag, Some("-name,bogus"), false), Some(vec![OrderTerm { field: "name".into(), desc: true }]));
        assert_eq!(ordering_terms(Kind::Tag, Some("path"), false), None);
        assert_eq!(ordering_terms(Kind::StoragePath, Some("path"), false).map(|v| v.len()), Some(1));
        assert_eq!(ordering_terms(Kind::Correspondent, Some("last_correspondence"), true).map(|v| v.len()), Some(1));
    }

    #[test]
    fn like_escaping() {
        assert_eq!(like_escape("a%b_c\\"), "a\\%b\\_c\\\\");
    }
}
