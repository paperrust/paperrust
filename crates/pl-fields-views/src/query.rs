//! Filter and ordering helpers like django-filter 26 / DRF `OrderingFilter`.
//! Local until `pl-core` offers generic filters (CORE-REQUEST).
//!
//! upstream: src/documents/filters.py@v3.4.2 (ID_KWARGS, CHAR_KWARGS, DATETIME_KWARGS),
//! django_filters/{filters,fields,rest_framework/filterset}.py, rest_framework/filters.py.

use sea_query::{Expr, SelectStatement, SimpleExpr};
use serde_json::{Map, Value, json};

use crate::shim::{ApiError, ApiResult, Ctx, tz};

/// Collected filter errors (`filterset.errors` → 400).
#[derive(Default)]
pub struct FilterErrors(Map<String, Value>);

impl FilterErrors {
    pub fn add(&mut self, name: &str, msg: &str) {
        self.0.insert(name.to_string(), json!([msg]));
    }
    pub fn into_result(self) -> ApiResult<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(ApiError::bad_request(Value::Object(self.0)))
        }
    }
}

/// `forms.DecimalField` → `int(Decimal)` (truncation), like Django integer lookups.
pub fn parse_number(s: &str) -> Option<i64> {
    let s = s.trim();
    static RE: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    let re = RE
        .get_or_init(|| regex::Regex::new(r"^[+-]?(\d+(\.\d*)?|\.\d+)([eE][+-]?\d+)?$").ok())
        .as_ref()?;
    if !re.is_match(s) {
        return None;
    }
    let f: f64 = s.parse().ok()?;
    if !f.is_finite() {
        return None;
    }
    Some(f.trunc() as i64)
}

/// Django `LIKE`-Escaping (`\`, `%`, `_`).
pub fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// `id` (exact) and `id__in` (NumberFilter / BaseInFilter).
pub fn id_filters(q: &mut SelectStatement, ctx: &Ctx, col: &str, errs: &mut FilterErrors) {
    if let Some(v) = ctx.param("id").filter(|v| !v.is_empty()) {
        match parse_number(v) {
            Some(n) => {
                q.and_where(Expr::cust_with_values(format!("{col} = $1"), [n]));
            }
            None => errs.add("id", "Enter a number."),
        }
    }
    if let Some(v) = ctx.param("id__in").filter(|v| !v.is_empty()) {
        let mut ids = Vec::new();
        let mut bad = false;
        for part in v.split(',') {
            if part.trim().is_empty() {
                continue;
            }
            match parse_number(part) {
                Some(n) => ids.push(n),
                None => bad = true,
            }
        }
        if bad {
            errs.add("id__in", "Enter a number.");
        } else {
            // i64 literals (no user text), since sea-query is built here without `postgres-array`
            let list = ids
                .iter()
                .map(i64::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            q.and_where(Expr::cust(if list.is_empty() {
                "FALSE".to_string()
            } else {
                format!("{col} IN ({list})")
            }));
        }
    }
}

/// CHAR_KWARGS: `istartswith`, `iendswith`, `icontains`, `iexact`.
pub fn char_filters(q: &mut SelectStatement, ctx: &Ctx, field: &str, col: &str) {
    for lookup in ["istartswith", "iendswith", "icontains", "iexact"] {
        let name = format!("{field}__{lookup}");
        let Some(v) = ctx.param(&name).filter(|v| !v.is_empty()) else {
            continue;
        };
        let (op, pat) = match lookup {
            "istartswith" => ("LIKE", format!("{}%", like_escape(v))),
            "iendswith" => ("LIKE", format!("%{}", like_escape(v))),
            "icontains" => ("LIKE", format!("%{}%", like_escape(v))),
            _ => ("=", v.to_string()),
        };
        q.and_where(Expr::cust_with_values(
            format!("UPPER({col}::text) {op} UPPER($1)"),
            [pat],
        ));
    }
}


/// DATETIME_KWARGS for a DateTimeField field.
pub fn datetime_filters(
    q: &mut SelectStatement,
    ctx: &Ctx,
    field: &str,
    col: &str,
    errs: &mut FilterErrors,
) {
    let zone = tz();
    let tz = zone.name();
    let local = format!("({col} AT TIME ZONE '{tz}')");
    for lookup in [
        "year", "month", "day", "date__gt", "date__gte", "gt", "gte", "date__lt", "date__lte",
        "lt", "lte",
    ] {
        let name = format!("{field}__{lookup}");
        let Some(v) = ctx.param(&name).filter(|v| !v.is_empty()) else {
            continue;
        };
        match lookup {
            "year" | "month" | "day" => match parse_number(v) {
                Some(n) => {
                    q.and_where(Expr::cust_with_values(
                        format!("EXTRACT({} FROM {local}) = $1", lookup.to_uppercase()),
                        [n],
                    ));
                }
                None => errs.add(&name, "Enter a number."),
            },
            l if l.starts_with("date__") => match pl_core::datetime::parse_date_input(v, ctx.rc.language) {
                Ok(Some(d)) => {
                    let op = op_for(&l[6..]);
                    q.and_where(Expr::cust_with_values(
                        format!("({local})::date {op} $1"),
                        [d],
                    ));
                }
                Ok(None) => {}
                Err(m) => errs.add(&name, &m),
            },
            l => match pl_core::datetime::parse_iso_datetime_input(v, &zone) {
                Ok(Some(dt)) => {
                    let op = op_for(l);
                    q.and_where(Expr::cust_with_values(format!("{col} {op} $1"), [dt]));
                }
                Ok(None) => {}
                Err(m) => errs.add(&name, &m),
            },
        }
    }
}

fn op_for(l: &str) -> &'static str {
    match l {
        "gt" => ">",
        "gte" => ">=",
        "lt" => "<",
        _ => "<=",
    }
}

/// DRF `OrderingFilter.get_ordering`: valid terms or `None` (→ default).
pub fn ordering_terms(ctx: &Ctx, valid: &[&str]) -> Option<Vec<(String, bool)>> {
    let raw = ctx.param("ordering")?;
    let terms: Vec<(String, bool)> = raw
        .split(',')
        .map(str::trim)
        .filter_map(|t| {
            let (name, desc) = match t.strip_prefix('-') {
                Some(n) => (n, true),
                None => (t, false),
            };
            valid.contains(&name).then(|| (name.to_string(), desc))
        })
        .collect();
    if terms.is_empty() { None } else { Some(terms) }
}

/// ORDER BY from terms; `col_for` returns the SQL expression per field name.
pub fn apply_ordering(
    q: &mut SelectStatement,
    terms: &[(String, bool)],
    col_for: impl Fn(&str) -> String,
) {
    for (name, desc) in terms {
        let e: SimpleExpr = Expr::cust(col_for(name));
        q.order_by_expr(
            e,
            if *desc {
                sea_query::Order::Desc
            } else {
                sea_query::Order::Asc
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(parse_number("5"), Some(5));
        assert_eq!(parse_number("1.9"), Some(1));
        assert_eq!(parse_number("1e3"), Some(1000));
        assert_eq!(parse_number("abc"), None);
        assert_eq!(parse_number("nan"), None);
    }

    #[test]
    fn like() {
        assert_eq!(like_escape("50%_a\\"), "50\\%\\_a\\\\");
    }

}
