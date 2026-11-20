//! Validation of `filter_custom_field_query` (check only, no SQL).
//! upstream: src/documents/filters.py@v3.4.2 (`CustomFieldQueryParser.parse` and helpers,
//! `MonetaryAmountField`, `DateQueryField`, `SelectField`, `handle_validation_prefix`),
//! DRF 3.18.3 (`ListField`, `IntegerField`, `FloatField`, `CharField`, `BooleanField`, `DateField`).
//! Note: belongs functionally to pl-docfilter (CORE-REQUEST: shared validator); local until then.

use crate::drf::{self, VErr, msg};
use crate::pyfmt::{self, PyInt};
use crate::schema::T_CF;
use sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use pl_core::sqlbind::SqlxBinder;
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, Row};

const MAX_DEPTH: usize = 10;
const MAX_ATOMS: usize = 20;
const DATE_COMPONENTS: &[&str] = &["year", "iso_year", "month", "day", "week", "week_day", "iso_week_day", "quarter"];

fn categories(data_type: &str) -> Option<&'static [&'static str]> {
    Some(match data_type {
        "string" | "url" | "longtext" => &["basic", "string"],
        "date" | "integer" | "float" => &["basic", "arithmetic"],
        "boolean" | "select" => &["basic"],
        "monetary" => &["basic", "string", "arithmetic"],
        "documentlink" => &["basic", "containment"],
        _ => return None,
    })
}

fn exprs(category: &str) -> &'static [&'static str] {
    match category {
        "basic" => &["exact", "in", "isnull", "exists"],
        "string" => &["icontains", "istartswith", "iendswith"],
        "arithmetic" => &["gt", "gte", "lt", "lte", "range"],
        _ => &["contains"],
    }
}

const ARITH: &[&str] = &["gt", "gte", "lt", "lte", "range"];

struct Cf {
    data_type: String,
    extra_data: Value,
}

/// Error detail with prefix (`handle_validation_prefix`).
fn prefixed(prefix: &str, e: VErr) -> VErr {
    match e {
        VErr::Detail(d) => {
            let mut m = Map::new();
            m.insert(prefix.to_string(), d);
            VErr::Detail(Value::Object(m))
        }
        other => other,
    }
}

struct P<'a> {
    db: &'a mut PgConnection,
    depth: usize,
    atoms: usize,
}

/// `CustomFieldQueryParser(prefix).parse(query)` – validation only.
pub async fn validate(db: &mut PgConnection, prefix: &str, query: &str) -> Result<(), VErr> {
    let expr: Value = match serde_json::from_str(query) {
        Ok(v) => v,
        Err(_) => {
            return Err(VErr::Detail(json!({ prefix: ["Value must be valid JSON."] })));
        }
    };
    let mut p = P { db, depth: 0, atoms: 0 };
    p.parse_expr(&expr).await.map_err(|e| prefixed(prefix, e))
}

impl<'a> P<'a> {
    fn parse_expr<'b>(&'b mut self, expr: &'b Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), VErr>> + Send + 'b>>
    where
        'a: 'b,
    {
        Box::pin(async move {
            self.depth += 1;
            let r = async {
                if self.depth > MAX_DEPTH {
                    return Err(VErr::Detail(msg("Maximum nesting depth exceeded.")));
                }
                if let Value::Array(a) = expr {
                    if a.len() == 2 {
                        return self.parse_logical(&a[0], &a[1]).await;
                    } else if a.len() == 3 {
                        return self.parse_atom(&a[0], &a[1], &a[2]).await;
                    }
                }
                Err(VErr::Detail(msg("Invalid custom field query expression")))
            }
            .await;
            self.depth -= 1;
            r
        })
    }

    async fn parse_logical(&mut self, op: &Value, args: &Value) -> Result<(), VErr> {
        let Value::String(op_s) = op else {
            // upstream: AttributeError (`op.lower()`) → 500
            return Err(VErr::Server("logical operator is not a string".into()));
        };
        let low = op_s.to_lowercase();
        if low == "not" {
            return self.parse_expr(args).await.map_err(|e| prefixed("1", e));
        }
        if low != "and" && low != "or" {
            return Err(VErr::Detail(json!({"0": [format!("Invalid logical operator {}", pyfmt::repr(op))]})));
        }
        // _parse_expr_list(args, validation_prefix="1")
        let r = async {
            let items = match args {
                Value::Array(a) if !a.is_empty() => a,
                _ => return Err(VErr::Detail(msg("Invalid expression list. Must be nonempty."))),
            };
            for (i, e) in items.iter().enumerate() {
                self.parse_expr(e).await.map_err(|err| prefixed(&i.to_string(), err))?;
            }
            Ok(())
        }
        .await;
        r.map_err(|e| prefixed("1", e))
    }

    async fn parse_atom(&mut self, id_or_name: &Value, op: &Value, value: &Value) -> Result<(), VErr> {
        self.atoms += 1;
        if self.atoms > MAX_ATOMS {
            return Err(VErr::Detail(msg("Maximum number of query conditions exceeded.")));
        }
        let cf = self.get_custom_field(id_or_name).await.map_err(|e| prefixed("0", e))?;
        validate_op(&cf, op).map_err(|e| prefixed("1", e))?;
        validate_value(&cf, op, value).map_err(|e| prefixed("2", e))?;
        Ok(())
    }

    async fn get_custom_field(&mut self, id_or_name: &Value) -> Result<Cf, VErr> {
        if id_or_name.is_array() || id_or_name.is_object() {
            // upstream: TypeError (unhashable) → 500
            return Err(VErr::Server("unhashable custom field reference".into()));
        }
        let mut q = Query::select();
        q.expr(Expr::col(Alias::new("data_type")))
            .expr(Expr::cust("COALESCE(\"extra_data\", 'null'::jsonb)"))
            .from(Alias::new(T_CF))
            .limit(1);
        let by_id = match id_or_name {
            Value::Bool(b) => Some(Some(*b as i64)),
            Value::Number(n) if !n.is_f64() => Some(n.as_i64().filter(|i| (-2_147_483_648..=2_147_483_647).contains(i))),
            _ => None,
        };
        let found = match by_id {
            Some(None) => None,
            Some(Some(id)) => {
                q.and_where(Expr::col(Alias::new("id")).eq(id));
                self.fetch_cf(&q).await?
            }
            None => match id_or_name {
                Value::Null => None,
                other => {
                    q.and_where(Expr::col(Alias::new("name")).eq(pyfmt::py_str(other)));
                    self.fetch_cf(&q).await?
                }
            },
        };
        found.ok_or_else(|| {
            VErr::Detail(msg(format!("{} is not a valid custom field.", pyfmt::repr(id_or_name))))
        })
    }

    async fn fetch_cf(&mut self, q: &sea_query::SelectStatement) -> Result<Option<Cf>, VErr> {
        let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
        let row = sqlx::query_with(&sql, values).fetch_optional(&mut *self.db).await?;
        match row {
            None => Ok(None),
            Some(r) => {
                let data_type: String = r.try_get(0)?;
                let extra: sqlx::types::Json<Value> = r.try_get(1)?;
                Ok(Some(Cf { data_type, extra_data: extra.0 }))
            }
        }
    }
}

fn split_op(full_op: &Value) -> (Option<String>, String) {
    let s = pyfmt::py_str(full_op);
    match s.rsplit_once("__") {
        Some((p, o)) => (Some(p.to_string()), o.to_string()),
        None => (None, s),
    }
}

fn validate_op(cf: &Cf, raw_op: &Value) -> Result<(), VErr> {
    let (prefix, op) = split_op(raw_op);
    let Some(cats) = categories(&cf.data_type) else {
        return Err(VErr::Server(format!("unknown data type {}", cf.data_type)));
    };
    let mut supported = cats.iter().any(|c| exprs(c).contains(&op.as_str()));
    if let Some(p) = &prefix
        && !(DATE_COMPONENTS.contains(&p.as_str()) && cf.data_type == "date") {
            supported = false;
        }
    if !supported {
        return Err(VErr::Detail(msg(format!(
            "{} does not support query expr {}.",
            cf.data_type,
            pyfmt::repr(raw_op)
        ))));
    }
    Ok(())
}

#[derive(Clone)]
enum VField {
    Bool,
    Int { min: Option<i64>, max: Option<i64> },
    Float,
    Date,
    Char { max_len: Option<usize>, allow_blank: bool },
    Select(Value),
    Monetary,
    List { child: Box<VField>, allow_empty: bool, min_len: Option<usize>, max_len: Option<usize> },
}

fn serializer_field(cf: &Cf, full_op: &Value) -> Result<VField, VErr> {
    let (prefix, op) = split_op(full_op);
    let dt = cf.data_type.as_str();
    let field = if op == "isnull" || op == "exists" {
        VField::Bool
    } else if dt == "date" && prefix.as_deref().is_some_and(|p| DATE_COMPONENTS.contains(&p)) {
        VField::Int { min: None, max: None }
    } else if dt == "date" {
        VField::Date
    } else if dt == "documentlink" {
        VField::List {
            child: Box::new(VField::Int { min: None, max: None }),
            allow_empty: true,
            min_len: None,
            max_len: None,
        }
    } else if dt == "select" {
        let opts = match &cf.extra_data {
            Value::Object(o) => match o.get("select_options") {
                Some(v) => v.clone(),
                None => return Err(VErr::Server("select_options missing".into())),
            },
            _ => return Err(VErr::Server("extra_data missing".into())),
        };
        VField::Select(opts)
    } else if dt == "url" {
        VField::Char { max_len: None, allow_blank: false }
    } else if dt == "monetary" && (ARITH.contains(&op.as_str()) || op == "exact" || op == "in") {
        VField::Monetary
    } else {
        match dt {
            "string" => VField::Char { max_len: Some(128), allow_blank: true },
            "monetary" => VField::Char { max_len: Some(128), allow_blank: true },
            "longtext" => VField::Char { max_len: None, allow_blank: true },
            "boolean" => VField::Bool,
            "integer" => VField::Int { min: Some(-2_147_483_648), max: Some(2_147_483_647) },
            "float" => VField::Float,
            _ => return Err(VErr::Server(format!("unknown data type {dt}"))),
        }
    };
    Ok(if op == "in" {
        VField::List { child: Box::new(field), allow_empty: false, min_len: None, max_len: None }
    } else if op == "range" {
        VField::List { child: Box::new(field), allow_empty: true, min_len: Some(2), max_len: Some(2) }
    } else {
        field
    })
}

fn validate_value(cf: &Cf, op: &Value, value: &Value) -> Result<(), VErr> {
    let f = serializer_field(cf, op)?;
    run_validation(&f, value)
}

/// `Field.run_validation` for the parser fields (always with value, never partial).
fn run_validation(f: &VField, data: &Value) -> Result<(), VErr> {
    // CharField empty-value check before everything else
    if let VField::Char { .. } | VField::Select(_) = f {
        let allow_blank = matches!(f, VField::Char { allow_blank: true, .. });
        let blank = match data {
            Value::String(s) => pyfmt::py_strip(s).is_empty(),
            Value::Null => false,
            other => pyfmt::py_strip(&pyfmt::py_str(other)).is_empty(),
        };
        if blank {
            return if allow_blank { Ok(()) } else { Err(VErr::Detail(msg("This field may not be blank."))) };
        }
    }
    if data.is_null() {
        return Err(VErr::Detail(msg("This field may not be null.")));
    }
    match f {
        VField::Bool => drf::bool_field(data).map(|_| ()),
        VField::Int { min, max } => drf::int_field(data, *min, *max).map(|_| ()),
        VField::Float => float_field(data),
        VField::Date => date_field(data),
        VField::Char { max_len, .. } => drf::char_field(data, *max_len).map(|_| ()),
        VField::Select(opts) => {
            let mut d = data.clone();
            if let Value::Array(options) = opts
                && let Some(o) = options.iter().find(|o| o.get("label") == Some(data)) {
                    d = o.get("id").cloned().unwrap_or(Value::Null);
                }
            drf::char_field(&d, Some(16)).map(|_| ())
        }
        VField::Monetary => monetary_field(data),
        VField::List { child, allow_empty, min_len, max_len } => {
            let items = match data {
                Value::Array(a) => a,
                other => {
                    return Err(VErr::Detail(msg(format!(
                        "Expected a list of items but got type \"{}\".",
                        pyfmt::type_name(other)
                    ))));
                }
            };
            if !allow_empty && items.is_empty() {
                return Err(VErr::Detail(msg("This list may not be empty.")));
            }
            let mut errors = Map::new();
            for (i, it) in items.iter().enumerate() {
                match run_validation(child, it) {
                    Ok(()) => {}
                    Err(VErr::Detail(d)) => {
                        errors.insert(i.to_string(), d);
                    }
                    Err(e) => return Err(e),
                }
            }
            if !errors.is_empty() {
                return Err(VErr::Detail(Value::Object(errors)));
            }
            let mut errs = Vec::new();
            if let Some(mx) = max_len
                && items.len() > *mx {
                    errs.push(json!(format!("Ensure this field has no more than {mx} elements.")));
                }
            if let Some(mn) = min_len
                && items.len() < *mn {
                    errs.push(json!(format!("Ensure this field has at least {mn} elements.")));
                }
            if !errs.is_empty() {
                return Err(VErr::Detail(Value::Array(errs)));
            }
            Ok(())
        }
    }
}

/// Python `float(data)` as DRF `FloatField`.
fn float_field(data: &Value) -> Result<(), VErr> {
    let ok = match data {
        Value::Number(_) | Value::Bool(_) => true,
        Value::String(s) => {
            if s.chars().count() > 1000 {
                return Err(VErr::Detail(msg("String value too large.")));
            }
            py_float_str(s)
        }
        _ => false,
    };
    if ok { Ok(()) } else { Err(VErr::Detail(msg("A valid number is required."))) }
}

fn strip_underscores(t: &str) -> Option<String> {
    // Underscores only between digits
    let c: Vec<char> = t.chars().collect();
    for (i, ch) in c.iter().enumerate() {
        if *ch == '_' {
            let prev = i.checked_sub(1).and_then(|j| c.get(j)).is_some_and(|p| p.is_ascii_digit());
            let next = c.get(i + 1).is_some_and(|p| p.is_ascii_digit());
            if !prev || !next {
                return None;
            }
        }
    }
    Some(t.replace('_', ""))
}

fn py_float_str(s: &str) -> bool {
    let t = pyfmt::py_strip(s);
    let body = t.strip_prefix(['+', '-']).unwrap_or(t);
    let low = body.to_ascii_lowercase();
    if matches!(low.as_str(), "inf" | "infinity" | "nan") {
        return true;
    }
    let Some(clean) = strip_underscores(body) else { return false };
    decimal_syntax(&clean)
}

/// `[digits][.digits][e[+-]digits]` with at least one digit in the mantissa.
fn decimal_syntax(s: &str) -> bool {
    let (mant, exp) = match s.find(['e', 'E']) {
        Some(i) => (&s[..i], Some(&s[i + 1..])),
        None => (s, None),
    };
    let (ip, fp) = match mant.split_once('.') {
        Some((a, b)) => (a, b),
        None => (mant, ""),
    };
    if !(ip.chars().all(|c| c.is_ascii_digit()) && fp.chars().all(|c| c.is_ascii_digit())) {
        return false;
    }
    if ip.is_empty() && fp.is_empty() {
        return false;
    }
    if let Some(e) = exp {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        if e.is_empty() || !e.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    true
}

/// `MonetaryAmountField.to_internal_value`
fn monetary_field(data: &Value) -> Result<(), VErr> {
    let invalid = || VErr::Detail(msg("A valid number is required."));
    let s = match data {
        Value::String(_) | Value::Number(_) | Value::Bool(_) => pyfmt::py_str(data),
        _ => return Err(invalid()),
    };
    let mut value: String = pyfmt::py_strip(&s).to_string();
    if let Some(first) = value.chars().next()
        && !first.is_numeric() && first != '-' {
            value = value.chars().skip(3).collect();
        }
    // Decimal(value)
    let t = pyfmt::py_strip(&value);
    let body = t.strip_prefix(['+', '-']).unwrap_or(t);
    let low = body.to_ascii_lowercase();
    if matches!(low.as_str(), "inf" | "infinity" | "nan" | "snan") {
        return Ok(());
    }
    match strip_underscores(body) {
        Some(c) if decimal_syntax(&c) => Ok(()),
        _ => Err(invalid()),
    }
}

/// `DateQueryField.to_internal_value` (DRF `DateField` with ISO 8601, otherwise relative expressions).
fn date_field(data: &Value) -> Result<(), VErr> {
    if let Value::String(s) = data {
        if parse_iso_date(s) {
            return Ok(());
        }
        if relative_date(s) {
            return Ok(());
        }
        return Err(VErr::Detail(msg(
            "Expected a date or a relative date such as \"today\" or \"in 30 days\".",
        )));
    }
    Err(VErr::Detail(msg("Date has wrong format. Use one of these formats instead: YYYY-MM-DD.")))
}

fn valid_ymd(y: i64, m: i64, d: i64) -> bool {
    chrono::NaiveDate::from_ymd_opt(y as i32, m as u32, d as u32).is_some() && (1..=9999).contains(&y)
}

/// Django `parse_date`: `date.fromisoformat` (Py 3.12) or `\d{4}-\d{1,2}-\d{1,2}$`.
fn parse_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| -> Option<i64> {
        let part = s.get(r)?;
        if part.chars().all(|c| c.is_ascii_digit()) { part.parse().ok() } else { None }
    };
    // YYYY-MM-DD / YYYYMMDD
    if b.len() == 10 && b[4] == b'-' && b[7] == b'-'
        && let (Some(y), Some(m), Some(d)) = (digits(0..4), digits(5..7), digits(8..10))
            && valid_ymd(y, m, d) {
                return true;
            }
    if b.len() == 8
        && let (Some(y), Some(m), Some(d)) = (digits(0..4), digits(4..6), digits(6..8))
            && valid_ymd(y, m, d) {
                return true;
            }
    // ISO-Woche YYYY-Www-D / YYYYWwwD / YYYY-Www / YYYYWww
    let iso_week = |y: i64, w: i64, d: i64| -> bool {
        (1..=7).contains(&d)
            && chrono::NaiveDate::from_isoywd_opt(y as i32, w as u32, chrono::Weekday::try_from((d - 1) as u8).unwrap_or(chrono::Weekday::Mon))
                .is_some()
    };
    let wk = |s: &str| -> Option<(i64, i64, i64)> {
        let t: String = s.chars().filter(|c| *c != '-').collect();
        if t.len() != 7 && t.len() != 8 {
            return None;
        }
        let tb = t.as_bytes();
        if tb[4] != b'W' {
            return None;
        }
        let y: i64 = t.get(0..4)?.parse().ok()?;
        let w: i64 = t.get(5..7)?.parse().ok()?;
        let d: i64 = if t.len() == 8 { t.get(7..8)?.parse().ok()? } else { 1 };
        Some((y, w, d))
    };
    if s.contains('W') {
        let dashed_ok = s.len() == 10 && b.get(4) == Some(&b'-') && b.get(8) == Some(&b'-')
            || s.len() == 8 && b.get(4) == Some(&b'-')
            || s.len() == 8 && !s.contains('-')
            || s.len() == 7 && !s.contains('-');
        if dashed_ok
            && let Some((y, w, d)) = wk(s)
                && iso_week(y, w, d) {
                    return true;
                }
    }
    // date_re: \d{4}-\d{1,2}-\d{1,2}$ (Python `$` allows a trailing \n)
    let t = s.strip_suffix('\n').unwrap_or(s);
    let parts: Vec<&str> = t.split('-').collect();
    if parts.len() == 3
        && parts[0].len() == 4
        && (1..=2).contains(&parts[1].len())
        && (1..=2).contains(&parts[2].len())
        && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()))
    {
        // datetime.date(**kw) with an invalid date raises ValueError → invalid
        let y: i64 = parts[0].parse().unwrap_or(0);
        let m: i64 = parts[1].parse().unwrap_or(0);
        let d: i64 = parts[2].parse().unwrap_or(0);
        return valid_ymd(y, m, d);
    }
    false
}

/// Small subset of dateparser `relative-time` (English). See Known differences.
fn relative_date(s: &str) -> bool {
    let t = pyfmt::py_strip(s).to_lowercase();
    if matches!(t.as_str(), "today" | "yesterday" | "tomorrow" | "now") {
        return true;
    }
    let words: Vec<&str> = t.split_whitespace().collect();
    let (core, ok_frame) = match words.as_slice() {
        ["in", rest @ ..] => (rest.to_vec(), true),
        [rest @ .., "ago"] => (rest.to_vec(), true),
        _ => (words.clone(), false),
    };
    if !ok_frame || core.len() != 2 {
        return false;
    }
    let num_ok = matches!(pyfmt::parse_int(core[0]), PyInt::Ok(_)) || matches!(core[0], "a" | "an" | "one");
    let unit = core[1].trim_end_matches('s');
    num_ok && matches!(unit, "second" | "minute" | "hour" | "day" | "week" | "fortnight" | "month" | "year")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cf(dt: &str) -> Cf {
        Cf { data_type: dt.into(), extra_data: json!({"select_options": [{"id": "abc", "label": "A"}]}) }
    }

    fn detail(r: Result<(), VErr>) -> Value {
        match r {
            Err(VErr::Detail(d)) => d,
            Ok(()) => Value::Null,
            Err(VErr::Server(s)) => panic!("{s}"),
        }
    }

    #[test]
    fn ops() {
        assert!(validate_op(&cf("string"), &json!("icontains")).is_ok());
        assert_eq!(
            detail(validate_op(&cf("boolean"), &json!("gt"))),
            json!(["boolean does not support query expr 'gt'."])
        );
        assert!(validate_op(&cf("date"), &json!("year__gte")).is_ok());
        assert!(validate_op(&cf("integer"), &json!("year__gte")).is_err());
    }

    #[test]
    fn values() {
        assert!(validate_value(&cf("integer"), &json!("exact"), &json!(5)).is_ok());
        assert_eq!(
            detail(validate_value(&cf("integer"), &json!("in"), &json!([1, "x"]))),
            json!({"1": ["A valid integer is required."]})
        );
        assert_eq!(
            detail(validate_value(&cf("integer"), &json!("range"), &json!([1]))),
            json!(["Ensure this field has at least 2 elements."])
        );
        assert!(validate_value(&cf("date"), &json!("gt"), &json!("2024-01-31")).is_ok());
        assert!(validate_value(&cf("date"), &json!("gt"), &json!("in 30 days")).is_ok());
        assert_eq!(
            detail(validate_value(&cf("date"), &json!("gt"), &json!(5))),
            json!(["Date has wrong format. Use one of these formats instead: YYYY-MM-DD."])
        );
        assert!(validate_value(&cf("monetary"), &json!("gt"), &json!("EUR10.50")).is_ok());
        assert!(validate_value(&cf("monetary"), &json!("gt"), &json!("abc")).is_err());
        assert!(validate_value(&cf("select"), &json!("exact"), &json!("A")).is_ok());
        assert!(validate_value(&cf("string"), &json!("exact"), &json!("")).is_ok());
        assert_eq!(
            detail(validate_value(&cf("url"), &json!("exact"), &json!(""))),
            json!(["This field may not be blank."])
        );
        assert!(validate_value(&cf("boolean"), &json!("exists"), &json!(true)).is_ok());
    }
}
