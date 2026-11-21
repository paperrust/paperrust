//! `custom_field_query` DSL.
//!
//! upstream: src/documents/filters.py@v3.4.2 (CustomFieldQueryParser, CustomFieldQueryFilter,
//! DateQueryField, MonetaryAmountField, SelectField, handle_validation_prefix)
//!
//! Flow: (1) collect field keys up front and load them in one go, (2) parse and validate
//! synchronously like upstream (same order, same error paths), (3) resolve document link
//! `contains` via the back references and build the SQL condition from that.
//!
//! Upstream evaluates the atoms via `Count(..., filter=...) > 0` on an isolated
//! document query and then filters with `pk__in`. Here every atom is formulated directly as a
//! correlated `EXISTS` subquery on `documents_customfieldinstance` –
//! same result set, without GROUP BY and without row multiplication. Like upstream (JOIN via
//! `custom_fields`), soft-deleted instances are also taken into account.

use std::collections::HashMap;

use chrono::NaiveDate;
use sea_query::{Expr, ExprTrait, SimpleExpr, Value};
use serde_json::{Map, Value as Json};

use crate::FilterContext;
use crate::dates::{drf_parse_date, now_local, parse_relative_date, year_bounds};
use crate::db::{CustomFieldDef, FilterDb};
use crate::error::FilterError;
use crate::pyval::{PyDecimal, PyInt, PyJson, RangeCheck, py_float, py_int, py_strip};
use crate::sql::{CFI, Cond, TextLookup, col, doc_col, exists, select_one_from, text_lookup};
use pl_domain::filter::{CfColumn, CfCond, CfTest, CfValue, CmpOp, DatePart, NumCol, PartTest, Pred};

pub const CUSTOM_FIELD_QUERY_MAX_DEPTH: usize = 10;
pub const CUSTOM_FIELD_QUERY_MAX_ATOMS: usize = 20;

const DATE_COMPONENTS: &[&str] = &["year", "iso_year", "month", "day", "week", "week_day", "iso_week_day", "quarter"];

const INT_MIN: i64 = -2_147_483_648;
const INT_MAX: i64 = 2_147_483_647;

fn category_ops(category: &str) -> &'static [&'static str] {
    match category {
        "basic" => &["exact", "in", "isnull", "exists"],
        "string" => &["icontains", "istartswith", "iendswith"],
        "arithmetic" => &["gt", "gte", "lt", "lte", "range"],
        "containment" => &["contains"],
        _ => &[],
    }
}

const ARITHMETIC: &[&str] = &["gt", "gte", "lt", "lte", "range"];

fn supported_categories(data_type: &str) -> Option<&'static [&'static str]> {
    Some(match data_type {
        "string" | "url" | "longtext" => &["basic", "string"],
        "date" | "integer" | "float" => &["basic", "arithmetic"],
        "boolean" | "select" => &["basic"],
        "monetary" => &["basic", "string", "arithmetic"],
        "documentlink" => &["basic", "containment"],
        _ => return None,
    })
}

/// `CustomFieldInstance.TYPE_TO_DATA_STORE_NAME_MAP`
pub fn value_field_name(data_type: &str) -> Option<&'static str> {
    Some(match data_type {
        "string" => "value_text",
        "url" => "value_url",
        "date" => "value_date",
        "boolean" => "value_bool",
        "integer" => "value_int",
        "float" => "value_float",
        "monetary" => "value_monetary",
        "documentlink" => "value_document_ids",
        "select" => "value_select",
        "longtext" => "value_long_text",
        _ => return None,
    })
}

fn split_op(full: &str) -> (Option<&str>, &str) {
    match full.rfind("__") {
        Some(i) => (Some(&full[..i]), &full[i + 2..]),
        None => (None, full),
    }
}

fn msg_list(msgs: Vec<String>) -> Json {
    Json::Array(msgs.into_iter().map(Json::String).collect())
}

/// Year outside 1..9999 in `…__year`/`…__iso_year`. upstream `ValueError` → 500; PaperRust 400
/// (B-305/D-039, Known differences).
fn year_out_of_range() -> FilterError {
    // the caller sets the prefix `custom_field_query`
    FilterError::list("Ensure this value is between 1 and 9999.")
}

fn validation_list(msg: impl Into<String>) -> FilterError {
    FilterError::list(msg)
}

// ---------------------------------------------------------------------------------------
// DRF field validation (subset, as upstream uses it for values)
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum FieldSpec {
    Bool,
    Int {
        min: Option<i64>,
        max: Option<i64>,
    },
    Float,
    Char {
        max_length: Option<usize>,
        allow_blank: bool,
    },
    /// SelectField: CharField(max_length=16) with label→ID mapping
    Select(Json),
    Date,
    Monetary,
    List {
        child: Box<FieldSpec>,
        allow_empty: bool,
        min_length: Option<usize>,
        max_length: Option<usize>,
    },
}

#[derive(Debug, Clone, PartialEq)]
enum Validated {
    Bool(bool),
    Int(PyInt),
    Float(f64),
    Str(String),
    Date(NaiveDate),
    Decimal(PyDecimal),
    List(Vec<Validated>),
}

/// Field validation error: `Ok(detail)` is a DRF ValidationError detail,
/// `Err` an unhandled exception in upstream.
type VResult = Result<Validated, Result<Json, FilterError>>;

fn fail(msg: impl Into<String>) -> Result<Json, FilterError> {
    Ok(msg_list(vec![msg.into()]))
}

/// Python `x == y` between a JSON value from the DB and a query value.
fn py_eq(db: &Json, v: &PyJson) -> bool {
    match (db, v) {
        (Json::Null, PyJson::Null) => true,
        (Json::String(a), PyJson::Str(b)) => a == b,
        (Json::Bool(a), PyJson::Bool(b)) => a == b,
        (Json::Number(_) | Json::Bool(_), PyJson::Int { .. } | PyJson::Float(_) | PyJson::Bool(_)) => {
            let a = match db {
                Json::Bool(b) => *b as i64 as f64,
                Json::Number(n) => n.as_f64().unwrap_or(f64::NAN),
                _ => f64::NAN,
            };
            v.as_f64().map(|b| a == b).unwrap_or(false)
        }
        (Json::Array(a), PyJson::List(b)) => a.len() == b.len() && a.iter().zip(b).all(|(x, y)| py_eq(x, y)),
        _ => false,
    }
}

/// serde JSON value as PyJson (for values from the database).
pub(crate) fn json_to_py(v: &Json) -> PyJson {
    match v {
        Json::Null => PyJson::Null,
        Json::Bool(b) => PyJson::Bool(*b),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                PyJson::int(i)
            } else if let Some(u) = n.as_u64() {
                PyJson::Int { negative: false, digits: u.to_string() }
            } else {
                PyJson::Float(n.as_f64().unwrap_or(f64::NAN))
            }
        }
        Json::String(s) => PyJson::Str(s.clone()),
        Json::Array(a) => PyJson::List(a.iter().map(json_to_py).collect()),
        Json::Object(o) => PyJson::Dict(o.iter().map(|(k, v)| (k.clone(), json_to_py(v))).collect()),
    }
}

/// DRF `IntegerField.re_decimal.sub('', s)` with `re_decimal = r'\.0*\s*$'`.
fn strip_decimal_zeros(s: &str) -> &str {
    for (i, c) in s.char_indices() {
        if c == '.' {
            let rest = &s[i + 1..];
            let after_zeros = rest.trim_start_matches('0');
            if after_zeros.chars().all(char::is_whitespace) {
                return &s[..i];
            }
        }
    }
    s
}

fn validate(spec: &FieldSpec, data: &PyJson, rel: &dyn Fn(&str) -> Option<NaiveDate>) -> VResult {
    // CharField.run_validation: empty string before anything else
    if matches!(spec, FieldSpec::Char { .. } | FieldSpec::Select(_)) {
        let allow_blank = matches!(spec, FieldSpec::Char { allow_blank: true, .. });
        if matches!(data, PyJson::Str(s) if py_strip(s).is_empty()) {
            if !allow_blank {
                return Err(fail("This field may not be blank."));
            }
            return Ok(Validated::Str(String::new()));
        }
    }
    // Field.validate_empty_values
    if *data == PyJson::Null {
        return Err(fail("This field may not be null."));
    }
    match spec {
        FieldSpec::Bool => {
            let r = match data {
                PyJson::Bool(b) => Some(*b),
                // DRF 3.18.3 `BooleanField._lower_if_str`: lowercased before the set lookup
                PyJson::Str(s) => match s.to_lowercase().as_str() {
                    "t" | "T" | "y" | "Y" | "yes" | "Yes" | "YES" | "true" | "True" | "TRUE" | "on" | "On" | "ON" | "1" => Some(true),
                    "f" | "F" | "n" | "N" | "no" | "No" | "NO" | "false" | "False" | "FALSE" | "off" | "Off" | "OFF" | "0" => Some(false),
                    _ => None,
                },
                PyJson::Int { .. } | PyJson::Float(_) => data.as_f64().and_then(|f| {
                    if f == 1.0 {
                        Some(true)
                    } else if f == 0.0 {
                        Some(false)
                    } else {
                        None
                    }
                }),
                _ => None,
            };
            r.map(Validated::Bool).ok_or_else(|| fail("Must be a valid boolean."))
        }
        FieldSpec::Int { min, max } => {
            let s = data.py_str();
            if matches!(data, PyJson::Str(_)) && s.chars().count() > 1000 {
                return Err(fail("String value too large."));
            }
            let v = match data {
                PyJson::Int { .. } => data.as_pyint(),
                PyJson::Dict(_) | PyJson::List(_) => None,
                _ => py_int(strip_decimal_zeros(&s)),
            };
            let Some(v) = v else {
                return Err(fail("A valid integer is required."));
            };
            if let Some(mx) = max
                && matches!(v.range_check(i64::MIN, *mx), RangeCheck::Overflow)
            {
                return Err(fail(format!("Ensure this value is less than or equal to {mx}.")));
            }
            if let Some(mn) = min
                && matches!(v.range_check(*mn, i64::MAX), RangeCheck::Underflow)
            {
                return Err(fail(format!("Ensure this value is greater than or equal to {mn}.")));
            }
            Ok(Validated::Int(v))
        }
        FieldSpec::Float => match data {
            PyJson::Str(s) => {
                if s.chars().count() > 1000 {
                    return Err(fail("String value too large."));
                }
                py_float(s).map(Validated::Float).ok_or_else(|| fail("A valid number is required."))
            }
            PyJson::Float(f) => Ok(Validated::Float(*f)),
            PyJson::Bool(b) => Ok(Validated::Float(*b as i64 as f64)),
            PyJson::Int { .. } => match data.as_f64() {
                Some(f) if f.is_finite() => Ok(Validated::Float(f)),
                // OverflowError: int too large to convert to float
                _ => Err(Err(FilterError::internal("OverflowError in float()"))),
            },
            _ => Err(fail("A valid number is required.")),
        },
        FieldSpec::Char { max_length, .. } => validate_char(data, *max_length),
        FieldSpec::Select(options) => {
            let mut mapped: Option<Json> = None;
            if let Json::Array(opts) = options {
                for o in opts {
                    let Json::Object(o) = o else {
                        return Err(Err(FilterError::internal("select option is not a dict")));
                    };
                    let label = o.get("label").cloned().unwrap_or(Json::Null);
                    if py_eq(&label, data) {
                        mapped = Some(o.get("id").cloned().unwrap_or(Json::Null));
                        break;
                    }
                }
            } else if !matches!(options, Json::Null) {
                // Iteration over non-lists: strings/dicts yield no dicts → AttributeError
                return Err(Err(FilterError::internal("select_options is not a list")));
            } else {
                return Err(Err(FilterError::internal("select_options is None")));
            }
            let value = match mapped {
                Some(j) => json_to_py(&j),
                None => data.clone(),
            };
            validate_char(&value, Some(16))
        }
        FieldSpec::Date => {
            if let PyJson::Str(s) = data {
                if let Some(d) = drf_parse_date(s) {
                    return Ok(Validated::Date(d));
                }
                // DateQueryField: fallback to relative expressions (dateparser)
                return rel(s)
                    .map(Validated::Date)
                    .ok_or_else(|| fail("Expected a date or a relative date such as \"today\" or \"in 30 days\"."));
            }
            Err(fail("Date has wrong format. Use one of these formats instead: YYYY-MM-DD."))
        }
        FieldSpec::Monetary => {
            let s = match data {
                PyJson::Str(s) => s.clone(),
                PyJson::Int { .. } | PyJson::Float(_) | PyJson::Bool(_) => data.py_str(),
                _ => return Err(fail("A valid number is required.")),
            };
            let mut value: &str = py_strip(&s);
            if let Some(first) = value.chars().next()
                && !first.is_ascii_digit()
                && first != '-'
            {
                value = match value.char_indices().nth(3) {
                    Some((i, _)) => &value[i..],
                    None => "",
                };
            }
            PyDecimal::parse(value).map(Validated::Decimal).ok_or_else(|| fail("A valid number is required."))
        }
        FieldSpec::List { child, allow_empty, min_length, max_length } => {
            let items = match data {
                PyJson::List(items) => items,
                other => {
                    return Err(fail(format!("Expected a list of items but got type \"{}\".", other.type_name())));
                }
            };
            if !allow_empty && items.is_empty() {
                return Err(fail("This list may not be empty."));
            }
            let mut out = Vec::with_capacity(items.len());
            let mut errors = Map::new();
            for (idx, item) in items.iter().enumerate() {
                match validate(child, item, rel) {
                    Ok(v) => out.push(v),
                    Err(Ok(detail)) => {
                        errors.insert(idx.to_string(), detail);
                    }
                    Err(Err(e)) => return Err(Err(e)),
                }
            }
            if !errors.is_empty() {
                return Err(Ok(Json::Object(errors)));
            }
            if let Some(mx) = max_length
                && out.len() > *mx
            {
                return Err(fail(format!("Ensure this field has no more than {mx} elements.")));
            }
            if let Some(mn) = min_length
                && out.len() < *mn
            {
                return Err(fail(format!("Ensure this field has at least {mn} elements.")));
            }
            Ok(Validated::List(out))
        }
    }
}

fn validate_char(data: &PyJson, max_length: Option<usize>) -> VResult {
    let s = match data {
        PyJson::Str(s) => s.clone(),
        PyJson::Int { .. } | PyJson::Float(_) => data.py_str(),
        _ => return Err(fail("Not a valid string.")),
    };
    let value = py_strip(&s).to_string();
    let mut errs = Vec::new();
    if let Some(mx) = max_length
        && value.chars().count() > mx
    {
        errs.push(format!("Ensure this field has no more than {mx} characters."));
    }
    if value.contains('\0') {
        errs.push("Null characters are not allowed.".to_string());
    }
    if !errs.is_empty() {
        return Err(Ok(msg_list(errs)));
    }
    Ok(Validated::Str(value))
}

// ---------------------------------------------------------------------------------------
// Expression tree
// ---------------------------------------------------------------------------------------

#[derive(Debug)]
enum Node {
    Leaf(Cond),
    /// Atom as AST node (L1, `Parser::ast`)
    Pred(Pred),
    And(Vec<Node>),
    Or(Vec<Node>),
    Not(Box<Node>),
    /// Document link `contains` with a non-empty ID list (needs DB back references)
    DocLinkContains {
        field_id: i64,
        ids: Vec<PyInt>,
    },
}

#[derive(Debug, Clone, PartialEq)]
enum CacheKey {
    Int(PyInt),
    Str(String),
}

struct Parser<'a> {
    by_id: &'a HashMap<i64, CustomFieldDef>,
    by_name: &'a HashMap<String, CustomFieldDef>,
    cache: Vec<(CacheKey, CustomFieldDef)>,
    depth: usize,
    atoms: usize,
    max_depth: usize,
    max_atoms: usize,
    ctx: &'a FilterContext,
    /// L1: build atoms as [`Pred`] instead of SQL (same checks, same errors).
    ast: bool,
}

impl Parser<'_> {
    fn parse_expr(&mut self, expr: &PyJson, prefix: &str) -> Result<Node, FilterError> {
        self.depth += 1;
        let r = self.parse_expr_inner(expr);
        self.depth -= 1;
        r.map_err(|e| e.prefixed(prefix))
    }

    fn parse_expr_inner(&mut self, expr: &PyJson) -> Result<Node, FilterError> {
        if self.depth > self.max_depth {
            return Err(validation_list("Maximum nesting depth exceeded."));
        }
        if let PyJson::List(items) = expr {
            if items.len() == 2 {
                return self.parse_logical(&items[0], &items[1]);
            } else if items.len() == 3 {
                return self.parse_atom(&items[0], &items[1], &items[2]);
            }
        }
        Err(validation_list("Invalid custom field query expression"))
    }

    fn parse_expr_list(&mut self, exprs: &PyJson, prefix: &str) -> Result<Vec<Node>, FilterError> {
        let r = (|| {
            let items = match exprs {
                PyJson::List(items) if !items.is_empty() => items,
                _ => return Err(validation_list("Invalid expression list. Must be nonempty.")),
            };
            let mut out = Vec::with_capacity(items.len());
            for (i, e) in items.iter().enumerate() {
                out.push(self.parse_expr(e, &i.to_string())?);
            }
            Ok(out)
        })();
        r.map_err(|e| e.prefixed(prefix))
    }

    fn parse_logical(&mut self, op: &PyJson, args: &PyJson) -> Result<Node, FilterError> {
        let PyJson::Str(op_s) = op else {
            return Err(FilterError::internal("AttributeError: op has no attribute 'lower'"));
        };
        let lower = op_s.to_lowercase();
        if lower == "not" {
            return Ok(Node::Not(Box::new(self.parse_expr(args, "1")?)));
        }
        let is_and = match lower.as_str() {
            "and" => true,
            "or" => false,
            _ => {
                return Err(FilterError::Validation(crate::error::field_errors(
                    "0",
                    vec![format!("Invalid logical operator {}", op.py_repr())],
                )));
            }
        };
        let nodes = self.parse_expr_list(args, "1")?;
        Ok(if is_and { Node::And(nodes) } else { Node::Or(nodes) })
    }

    fn get_custom_field(&mut self, key: &PyJson) -> Result<CustomFieldDef, FilterError> {
        if !key.is_hashable() {
            return Err(FilterError::internal("TypeError: unhashable type"));
        }
        // Cache hit like Python's dict (1 == 1.0 == True)
        let probe: Option<CacheKey> = match key {
            PyJson::Bool(_) | PyJson::Int { .. } => key.as_pyint().map(CacheKey::Int),
            PyJson::Float(f) if f.fract() == 0.0 && f.abs() < 1e18 => Some(CacheKey::Int(PyInt::Small(*f as i128))),
            PyJson::Str(s) => Some(CacheKey::Str(s.clone())),
            _ => None,
        };
        if let Some(p) = &probe
            && let Some((_, f)) = self.cache.iter().find(|(k, _)| k == p)
        {
            return Ok(f.clone());
        }
        let found = match key {
            PyJson::Bool(_) | PyJson::Int { .. } => key.as_pyint().and_then(|i| i.as_i64()).and_then(|i| self.by_id.get(&i).cloned()),
            PyJson::Null => None,
            other => self.by_name.get(&other.py_str()).cloned(),
        };
        let Some(field) = found else {
            return Err(validation_list(format!("{} is not a valid custom field.", key.py_repr())));
        };
        self.cache.push((CacheKey::Int(PyInt::Small(field.id as i128)), field.clone()));
        self.cache.push((CacheKey::Str(field.name.clone()), field.clone()));
        Ok(field)
    }

    fn validate_atom_op(&self, field: &CustomFieldDef, raw_op: &PyJson) -> Result<String, FilterError> {
        let full = raw_op.py_str();
        let (prefix, op) = split_op(&full);
        let Some(cats) = supported_categories(&field.data_type) else {
            return Err(FilterError::internal(format!("KeyError: unsupported data type {}", field.data_type)));
        };
        let mut supported = cats.iter().any(|c| category_ops(c).contains(&op));
        if let Some(p) = prefix
            && !(DATE_COMPONENTS.contains(&p) && field.data_type == "date")
        {
            supported = false;
        }
        if !supported {
            return Err(validation_list(format!("{} does not support query expr {}.", field.data_type, raw_op.py_repr())));
        }
        Ok(full)
    }

    fn serializer_field(&self, field: &CustomFieldDef, full_op: &str) -> Result<FieldSpec, FilterError> {
        let (prefix, op) = split_op(full_op);
        let dt = field.data_type.as_str();
        let base = if op == "isnull" || op == "exists" {
            FieldSpec::Bool
        } else if dt == "date" && prefix.is_some_and(|p| DATE_COMPONENTS.contains(&p)) {
            FieldSpec::Int { min: None, max: None }
        } else if dt == "date" {
            FieldSpec::Date
        } else if dt == "documentlink" {
            FieldSpec::List {
                child: Box::new(FieldSpec::Int { min: None, max: None }),
                allow_empty: true,
                min_length: None,
                max_length: None,
            }
        } else if dt == "select" {
            // custom_field.extra_data["select_options"]
            match &field.extra_data {
                Some(Json::Object(m)) => match m.get("select_options") {
                    Some(opts) => FieldSpec::Select(opts.clone()),
                    None => return Err(FilterError::internal("KeyError: 'select_options'")),
                },
                _ => return Err(FilterError::internal("TypeError: extra_data is not subscriptable")),
            }
        } else if dt == "url" {
            FieldSpec::Char { max_length: None, allow_blank: false }
        } else if dt == "monetary" && (ARITHMETIC.contains(&op) || op == "exact" || op == "in") {
            FieldSpec::Monetary
        } else {
            match dt {
                "string" | "monetary" => FieldSpec::Char { max_length: Some(128), allow_blank: true },
                "longtext" => FieldSpec::Char { max_length: None, allow_blank: true },
                "integer" => FieldSpec::Int { min: Some(INT_MIN), max: Some(INT_MAX) },
                "float" => FieldSpec::Float,
                "boolean" => FieldSpec::Bool,
                other => {
                    return Err(FilterError::internal(format!("NotImplementedError: {other}")));
                }
            }
        };
        Ok(match op {
            "in" => FieldSpec::List { child: Box::new(base), allow_empty: false, min_length: None, max_length: None },
            "range" => FieldSpec::List { child: Box::new(base), allow_empty: true, min_length: Some(2), max_length: Some(2) },
            _ => base,
        })
    }

    fn validate_atom_value(&self, field: &CustomFieldDef, op: &str, value: &PyJson) -> Result<Validated, FilterError> {
        let spec = self.serializer_field(field, op)?;
        let now = now_local(self.ctx.now, self.ctx.tz);
        let langs = self.ctx.date_parser_languages.as_deref();
        let rel = move |s: &str| parse_relative_date(s, now, langs);
        match validate(&spec, value, &rel) {
            Ok(v) => Ok(v),
            Err(Ok(detail)) => Err(FilterError::Validation(detail)),
            Err(Err(e)) => Err(e),
        }
    }

    fn parse_atom(&mut self, key: &PyJson, op: &PyJson, value: &PyJson) -> Result<Node, FilterError> {
        self.atoms += 1;
        if self.atoms > self.max_atoms {
            return Err(validation_list("Maximum number of query conditions exceeded."));
        }
        let field = self.get_custom_field(key).map_err(|e| e.prefixed("0"))?;
        let op = self.validate_atom_op(&field, op).map_err(|e| e.prefixed("1"))?;
        let value = self.validate_atom_value(&field, &op, value).map_err(|e| e.prefixed("2"))?;
        if self.ast {
            return build_atom_pred(&field, &op, value);
        }
        build_atom(&field, &op, value)
    }
}

// ---------------------------------------------------------------------------------------
// SQL for atoms
// ---------------------------------------------------------------------------------------

fn sql_value(v: &Validated) -> Result<SimpleExpr, FilterError> {
    Ok(match v {
        Validated::Bool(b) => Expr::val(*b).into(),
        Validated::Int(i) => match i.as_i64() {
            Some(x) => Expr::val(x).into(),
            None => {
                // Only possible for comparisons without range check; bind as numeric.
                let s = if i.is_negative() { "-1e400" } else { "1e400" };
                Expr::cust_with_exprs("CAST($1 AS numeric)", [Expr::val(s).into()])
            }
        },
        Validated::Float(f) => Expr::val(*f).into(),
        Validated::Str(s) => Expr::val(s.as_str()).into(),
        Validated::Date(d) => Expr::val(*d).into(),
        Validated::Decimal(d) => {
            if !d.is_finite() {
                // Django DecimalField.to_python rejects NaN/Inf with a
                // django.core.exceptions.ValidationError → unhandled → 500
                return Err(FilterError::internal("non-finite decimal in lookup"));
            }
            Expr::cust_with_exprs("CAST($1 AS numeric)", [Expr::val(d.to_sql_numeric()).into()])
        }
        Validated::List(items) => {
            let json = Json::Array(items.iter().map(validated_to_json).collect());
            Expr::val(Value::Json(Some(Box::new(json)))).into()
        }
    })
}

fn validated_to_json(v: &Validated) -> Json {
    match v {
        Validated::Bool(b) => Json::Bool(*b),
        Validated::Int(i) => match i.as_i64() {
            Some(x) => Json::from(x),
            None => Json::Null,
        },
        Validated::Float(f) => serde_json::Number::from_f64(*f).map(Json::Number).unwrap_or(Json::Null),
        Validated::Str(s) => Json::String(s.clone()),
        Validated::Date(d) => Json::String(d.format("%Y-%m-%d").to_string()),
        Validated::Decimal(d) => Json::String(d.to_sql_numeric()),
        Validated::List(items) => Json::Array(items.iter().map(validated_to_json).collect()),
    }
}

/// Django's IntegerField lookups with overflow handling (`IntegerFieldOverflow`).
fn int_compare(lhs: SimpleExpr, op: &str, v: PyInt) -> Cond {
    let rc = v.range_check(INT_MIN, INT_MAX);
    match (op, rc) {
        (_, RangeCheck::InRange(x)) => Cond::Expr(match op {
            "exact" => lhs.eq(x),
            "gt" => lhs.gt(x),
            "gte" => lhs.gte(x),
            "lt" => lhs.lt(x),
            _ => lhs.lte(x),
        }),
        ("exact", _) => Cond::False,
        ("gt" | "gte", RangeCheck::Underflow) => Cond::True,
        ("gt" | "gte", _) => Cond::False,
        ("lt" | "lte", RangeCheck::Overflow) => Cond::True,
        _ => Cond::False,
    }
}

fn extract_expr(component: &str, column: SimpleExpr) -> SimpleExpr {
    let tpl = match component {
        "week_day" => "EXTRACT(DOW FROM $1) + 1",
        "iso_week_day" => "EXTRACT(ISODOW FROM $1)",
        "iso_year" => "EXTRACT(ISOYEAR FROM $1)",
        "year" => "EXTRACT(YEAR FROM $1)",
        "month" => "EXTRACT(MONTH FROM $1)",
        "day" => "EXTRACT(DAY FROM $1)",
        "week" => "EXTRACT(WEEK FROM $1)",
        _ => "EXTRACT(QUARTER FROM $1)",
    };
    Expr::cust_with_exprs(tpl, [column])
}

fn clamp_i64(v: &PyInt) -> i64 {
    match v {
        PyInt::Small(x) => (*x).clamp(i64::MIN as i128, i64::MAX as i128) as i64,
        PyInt::Big { negative: true } => i64::MIN,
        PyInt::Big { negative: false } => i64::MAX,
    }
}

/// Condition for `value_<type>__<op>` (without field binding).
fn value_lookup(column: SimpleExpr, full_op: &str, value: &Validated) -> Result<Cond, FilterError> {
    let (prefix, op) = split_op(full_op);
    if let Some(component) = prefix {
        // date fields only (checked in validate_atom_op)
        if op == "isnull" {
            let Validated::Bool(b) = value else { return Err(FilterError::internal("isnull value")) };
            let e = extract_expr(component, column);
            return Ok(Cond::Expr(if *b { e.is_null() } else { e.is_not_null() }));
        }
        if op == "exists" {
            return Err(FilterError::internal("FieldError: Unsupported lookup 'exists'"));
        }
        if (component == "year" || component == "iso_year") && matches!(op, "exact" | "gt" | "gte" | "lt" | "lte") {
            let Validated::Int(y) = value else { return Err(FilterError::internal("year value")) };
            let Some(year) = y.as_i64() else {
                return Err(year_out_of_range());
            };
            let Some((start, end)) = year_bounds(year, component == "iso_year") else {
                return Err(year_out_of_range());
            };
            return Ok(Cond::Expr(match op {
                "exact" => column.between(Expr::val(start), Expr::val(end)),
                "gt" => column.gt(end),
                "gte" => column.gte(start),
                "lt" => column.lt(start),
                _ => column.lte(end),
            }));
        }
        let e = extract_expr(component, column);
        return Ok(match (op, value) {
            ("exact" | "gt" | "gte" | "lt" | "lte", Validated::Int(v)) => int_compare(e, op, *v),
            ("in", Validated::List(items)) => {
                let vals: Vec<i64> = items
                    .iter()
                    .filter_map(|i| match i {
                        Validated::Int(v) => v.as_i64(),
                        _ => None,
                    })
                    .collect();
                if vals.is_empty() { Cond::False } else { Cond::Expr(e.is_in(vals)) }
            }
            ("range", Validated::List(items)) if items.len() == 2 => {
                let (Validated::Int(a), Validated::Int(b)) = (&items[0], &items[1]) else {
                    return Err(FilterError::internal("range values"));
                };
                Cond::Expr(e.between(Expr::val(clamp_i64(a)), Expr::val(clamp_i64(b))))
            }
            _ => return Err(FilterError::internal("unsupported date component lookup")),
        });
    }
    Ok(match op {
        "exact" => Cond::Expr(column.eq(sql_value(value)?)),
        "in" => {
            let Validated::List(items) = value else { return Err(FilterError::internal("in value")) };
            let vals = items.iter().map(sql_value).collect::<Result<Vec<_>, _>>()?;
            Cond::Expr(column.is_in(vals))
        }
        "isnull" => {
            let Validated::Bool(b) = value else { return Err(FilterError::internal("isnull value")) };
            Cond::Expr(if *b { column.is_null() } else { column.is_not_null() })
        }
        "gt" => Cond::Expr(column.gt(sql_value(value)?)),
        "gte" => Cond::Expr(column.gte(sql_value(value)?)),
        "lt" => Cond::Expr(column.lt(sql_value(value)?)),
        "lte" => Cond::Expr(column.lte(sql_value(value)?)),
        "range" => {
            let Validated::List(items) = value else { return Err(FilterError::internal("range value")) };
            if items.len() != 2 {
                return Err(FilterError::internal("range length"));
            }
            Cond::Expr(column.between(sql_value(&items[0])?, sql_value(&items[1])?))
        }
        "icontains" | "istartswith" | "iendswith" => {
            let Validated::Str(s) = value else { return Err(FilterError::internal("text value")) };
            let lookup = TextLookup::from_name(op).ok_or_else(|| FilterError::internal("lookup"))?;
            Cond::Expr(text_lookup(column, lookup, s))
        }
        _ => return Err(FilterError::internal(format!("unsupported lookup {op}"))),
    })
}

/// `EXISTS (SELECT 1 FROM documents_customfieldinstance cq WHERE cq.document_id = doc.id
///          AND cq.field_id = <id> [AND cond])`
fn cfi_exists(field_id: i64, cond: Cond) -> Cond {
    let mut sel = select_one_from(CFI, "cq");
    sel.and_where(col("cq", "document_id").eq(doc_col("id"))).and_where(col("cq", "field_id").eq(field_id));
    match cond {
        Cond::False => return Cond::False,
        Cond::True => {}
        Cond::Expr(e) => {
            sel.and_where(e);
        }
    }
    Cond::Expr(exists(sel))
}

fn build_atom(field: &CustomFieldDef, op: &str, value: Validated) -> Result<Node, FilterError> {
    if field.data_type == "documentlink" && op == "contains" {
        let Validated::List(items) = value else {
            return Err(FilterError::internal("contains value"));
        };
        if items.is_empty() {
            return Ok(Node::Leaf(cfi_exists(field.id, Cond::Expr(col("cq", "value_document_ids").is_not_null()))));
        }
        let ids = items
            .into_iter()
            .map(|v| match v {
                Validated::Int(i) => Ok(i),
                _ => Err(FilterError::internal("contains item")),
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Node::DocLinkContains { field_id: field.id, ids });
    }
    let mut value_field = value_field_name(&field.data_type).ok_or_else(|| FilterError::internal("NotImplementedError: data type"))?;
    let (_, base_op) = split_op(op);
    if field.data_type == "monetary" && (ARITHMETIC.contains(&op) || op == "exact" || op == "in") {
        value_field = "value_monetary_amount";
    }
    if op == "exists" {
        let Validated::Bool(b) = value else {
            return Err(FilterError::internal("exists value"));
        };
        let has = cfi_exists(field.id, Cond::True);
        return Ok(Node::Leaf(if b { has } else { has.negate() }));
    }
    let _ = base_op;
    let cond = value_lookup(col("cq", value_field), op, &value)?;
    Ok(Node::Leaf(cfi_exists(field.id, cond)))
}

// ---------------------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------------------

/// Collects (in parser order) the field keys that upstream would look up.
fn collect_keys(expr: &PyJson, depth: usize, atoms: &mut usize, ids: &mut Vec<i64>, names: &mut Vec<String>) {
    if depth > CUSTOM_FIELD_QUERY_MAX_DEPTH || *atoms > CUSTOM_FIELD_QUERY_MAX_ATOMS {
        return;
    }
    let PyJson::List(items) = expr else { return };
    if items.len() == 2 {
        if let (PyJson::Str(op), args) = (&items[0], &items[1]) {
            match op.to_lowercase().as_str() {
                "not" => collect_keys(args, depth + 1, atoms, ids, names),
                "and" | "or" => {
                    if let PyJson::List(list) = args {
                        for e in list {
                            collect_keys(e, depth + 1, atoms, ids, names);
                        }
                    }
                }
                _ => {}
            }
        }
    } else if items.len() == 3 {
        *atoms += 1;
        match &items[0] {
            k @ (PyJson::Bool(_) | PyJson::Int { .. }) => {
                if let Some(i) = k.as_pyint().and_then(|i| i.as_i64()) {
                    ids.push(i);
                }
            }
            PyJson::Null | PyJson::List(_) | PyJson::Dict(_) => {}
            other => names.push(other.py_str()),
        }
    }
}

/// Converts a `value_document_ids` entry into a document ID (like `id__in=set(...)`).
fn link_id(v: &Json) -> Result<Option<i64>, FilterError> {
    match v {
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Some(i))
            } else if let Some(f) = n.as_f64() {
                if f.fract() == 0.0 && f.abs() < 9.2e18 {
                    Ok(Some(f as i64))
                } else {
                    Err(FilterError::internal("invalid document id in link"))
                }
            } else {
                Ok(None)
            }
        }
        Json::Bool(b) => Ok(Some(*b as i64)),
        Json::String(s) => match py_int(s) {
            Some(i) => Ok(i.as_i64()),
            None => Err(FilterError::internal("ValueError: invalid document id in link")),
        },
        Json::Null => Ok(None),
        _ => Err(FilterError::internal("TypeError: unhashable link entry")),
    }
}

async fn resolve<D: FilterDb>(node: Node, db: &D) -> Result<Cond, FilterError> {
    // iterative via Box::pin, since recursive async
    fn inner<'a, D: FilterDb>(
        node: Node,
        db: &'a D,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Cond, FilterError>> + Send + 'a>> {
        Box::pin(async move {
            Ok(match node {
                Node::Leaf(c) => c,
                Node::Pred(_) => return Err(FilterError::internal("AST node in SQL resolve")),
                Node::Not(n) => inner(*n, db).await?.negate(),
                Node::And(nodes) => {
                    let mut acc = Cond::True;
                    for n in nodes {
                        acc = acc.and(inner(n, db).await?);
                    }
                    acc
                }
                Node::Or(nodes) => {
                    let mut acc = Cond::False;
                    for n in nodes {
                        acc = acc.or(inner(n, db).await?);
                    }
                    acc
                }
                Node::DocLinkContains { field_id, ids } => {
                    // Values outside i64 cannot be document IDs → missing
                    let mut wanted: Vec<i64> = Vec::new();
                    for i in &ids {
                        match i.as_i64() {
                            Some(x) => {
                                if !wanted.contains(&x) {
                                    wanted.push(x)
                                }
                            }
                            None => return Ok(Cond::False),
                        }
                    }
                    let links = db.doc_link_instances(field_id, &wanted).await?;
                    let found: Vec<i64> = links.iter().map(|(d, _)| *d).collect();
                    if wanted.iter().any(|w| !found.contains(w)) {
                        return Ok(Cond::False);
                    }
                    let mut acc: Option<Vec<i64>> = None;
                    for (_, value) in &links {
                        let set: Vec<i64> = match value {
                            Json::Array(a) => {
                                let mut s = Vec::new();
                                for v in a {
                                    if let Some(id) = link_id(v)?
                                        && !s.contains(&id)
                                    {
                                        s.push(id);
                                    }
                                }
                                s
                            }
                            _ => return Err(FilterError::internal("TypeError: value_document_ids not iterable")),
                        };
                        acc = Some(match acc {
                            None => set,
                            Some(prev) => prev.into_iter().filter(|x| set.contains(x)).collect(),
                        });
                    }
                    let ids = acc.unwrap_or_default();
                    if ids.is_empty() {
                        Cond::False
                    } else {
                        let mut ids = ids;
                        ids.sort_unstable();
                        Cond::Expr(doc_col("id").is_in(ids))
                    }
                }
            })
        })
    }
    inner(node, db).await
}

/// `CustomFieldQueryFilter.filter`: parses `value` and returns the condition for the
/// outer document query. Empty value → `Cond::True` (no filter).
pub async fn custom_field_query_cond<D: FilterDb>(
    value: &str,
    validation_prefix: &str,
    ctx: &FilterContext,
    db: &D,
) -> Result<Cond, FilterError> {
    if value.is_empty() {
        return Ok(Cond::True);
    }
    let Some(expr) = PyJson::loads(value) else {
        return Err(FilterError::field(validation_prefix, "Value must be valid JSON."));
    };
    let mut ids = Vec::new();
    let mut names = Vec::new();
    let mut atoms = 0;
    collect_keys(&expr, 1, &mut atoms, &mut ids, &mut names);
    let fields = db.custom_fields_lookup(&ids, &names).await?;
    let mut by_id = HashMap::new();
    let mut by_name = HashMap::new();
    for f in fields {
        by_name.insert(f.name.clone(), f.clone());
        by_id.insert(f.id, f);
    }
    let node = {
        let mut p = Parser {
            by_id: &by_id,
            by_name: &by_name,
            cache: Vec::new(),
            depth: 0,
            atoms: 0,
            max_depth: CUSTOM_FIELD_QUERY_MAX_DEPTH,
            max_atoms: CUSTOM_FIELD_QUERY_MAX_ATOMS,
            ctx,
            ast: false,
        };
        p.parse_expr(&expr, validation_prefix)?
    };
    resolve(node, db).await
}

// ---------------------------------------------------------------------------------------
// L1: atoms as AST (pl_domain::filter), same checks and errors as the SQL path
// ---------------------------------------------------------------------------------------

/// `value_<type>` column name → [`CfColumn`].
pub fn cf_column(name: &str) -> Option<CfColumn> {
    Some(match name {
        "value_text" => CfColumn::Text,
        "value_long_text" => CfColumn::LongText,
        "value_bool" => CfColumn::Bool,
        "value_int" => CfColumn::Int,
        "value_float" => CfColumn::Float,
        "value_date" => CfColumn::Date,
        "value_url" => CfColumn::Url,
        "value_monetary" => CfColumn::Monetary,
        "value_monetary_amount" => CfColumn::MonetaryAmount,
        "value_document_ids" => CfColumn::DocumentIds,
        "value_select" => CfColumn::Select,
        _ => return None,
    })
}

fn date_part(component: &str) -> DatePart {
    match component {
        "year" => DatePart::Year,
        "iso_year" => DatePart::IsoYear,
        "month" => DatePart::Month,
        "day" => DatePart::Day,
        "week" => DatePart::Week,
        "week_day" => DatePart::WeekDay,
        "iso_week_day" => DatePart::IsoWeekDay,
        _ => DatePart::Quarter,
    }
}

fn cmp_op(op: &str) -> CmpOp {
    match op {
        "exact" => CmpOp::Eq,
        "gt" => CmpOp::Gt,
        "gte" => CmpOp::Gte,
        "lt" => CmpOp::Lt,
        _ => CmpOp::Lte,
    }
}

/// Like [`sql_value`] (same errors), as AST value.
fn cf_value(v: &Validated) -> Result<CfValue, FilterError> {
    Ok(match v {
        Validated::Bool(b) => CfValue::Bool(*b),
        Validated::Int(i) => match i.as_i64() {
            Some(x) => CfValue::Int(x),
            None => CfValue::HugeInt { negative: i.is_negative() },
        },
        Validated::Float(f) => CfValue::Float(*f),
        Validated::Str(s) => CfValue::Text(s.clone()),
        Validated::Date(d) => CfValue::Date(*d),
        Validated::Decimal(d) => {
            if !d.is_finite() {
                return Err(FilterError::internal("non-finite decimal in lookup"));
            }
            CfValue::Numeric(d.to_sql_numeric())
        }
        Validated::List(items) => CfValue::Json(Json::Array(items.iter().map(validated_to_json).collect())),
    })
}

/// Result of a value lookup: constant (Django's Empty-/FullResultSet) or a test.
enum Lookup {
    Const(bool),
    Test(CfTest),
}

/// Like [`value_lookup`] (same errors, same overflow handling), as AST.
fn value_lookup_ast(full_op: &str, value: &Validated) -> Result<Lookup, FilterError> {
    let (prefix, op) = split_op(full_op);
    if let Some(component) = prefix {
        let part = date_part(component);
        if op == "isnull" {
            let Validated::Bool(b) = value else { return Err(FilterError::internal("isnull value")) };
            return Ok(Lookup::Test(CfTest::Part(part, PartTest::IsNull(*b))));
        }
        if op == "exists" {
            return Err(FilterError::internal("FieldError: Unsupported lookup 'exists'"));
        }
        if (component == "year" || component == "iso_year") && matches!(op, "exact" | "gt" | "gte" | "lt" | "lte") {
            let Validated::Int(y) = value else { return Err(FilterError::internal("year value")) };
            let Some(year) = y.as_i64() else {
                return Err(year_out_of_range());
            };
            let Some((start, end)) = year_bounds(year, component == "iso_year") else {
                return Err(year_out_of_range());
            };
            return Ok(Lookup::Test(CfTest::YearBounds(cmp_op(op), start, end)));
        }
        return Ok(match (op, value) {
            ("exact" | "gt" | "gte" | "lt" | "lte", Validated::Int(v)) => match (op, v.range_check(INT_MIN, INT_MAX)) {
                (_, RangeCheck::InRange(x)) => Lookup::Test(CfTest::Part(part, PartTest::Cmp(cmp_op(op), x))),
                ("exact", _) => Lookup::Const(false),
                ("gt" | "gte", RangeCheck::Underflow) => Lookup::Const(true),
                ("gt" | "gte", _) => Lookup::Const(false),
                ("lt" | "lte", RangeCheck::Overflow) => Lookup::Const(true),
                _ => Lookup::Const(false),
            },
            ("in", Validated::List(items)) => {
                let vals: Vec<i64> = items
                    .iter()
                    .filter_map(|i| match i {
                        Validated::Int(v) => v.as_i64(),
                        _ => None,
                    })
                    .collect();
                if vals.is_empty() { Lookup::Const(false) } else { Lookup::Test(CfTest::Part(part, PartTest::In(vals))) }
            }
            ("range", Validated::List(items)) if items.len() == 2 => {
                let (Validated::Int(a), Validated::Int(b)) = (&items[0], &items[1]) else {
                    return Err(FilterError::internal("range values"));
                };
                Lookup::Test(CfTest::Part(part, PartTest::Between(clamp_i64(a), clamp_i64(b))))
            }
            _ => return Err(FilterError::internal("unsupported date component lookup")),
        });
    }
    Ok(Lookup::Test(match op {
        "exact" => CfTest::Cmp(CmpOp::Eq, cf_value(value)?),
        "in" => {
            let Validated::List(items) = value else { return Err(FilterError::internal("in value")) };
            CfTest::In(items.iter().map(cf_value).collect::<Result<Vec<_>, _>>()?)
        }
        "isnull" => {
            let Validated::Bool(b) = value else { return Err(FilterError::internal("isnull value")) };
            CfTest::IsNull(*b)
        }
        "gt" | "gte" | "lt" | "lte" => CfTest::Cmp(cmp_op(op), cf_value(value)?),
        "range" => {
            let Validated::List(items) = value else { return Err(FilterError::internal("range value")) };
            if items.len() != 2 {
                return Err(FilterError::internal("range length"));
            }
            CfTest::Between(cf_value(&items[0])?, cf_value(&items[1])?)
        }
        "icontains" | "istartswith" | "iendswith" => {
            let Validated::Str(s) = value else { return Err(FilterError::internal("text value")) };
            let lookup = TextLookup::from_name(op).ok_or_else(|| FilterError::internal("lookup"))?;
            CfTest::Text(crate::ast::text_lookup_ast(lookup), s.clone())
        }
        _ => return Err(FilterError::internal(format!("unsupported lookup {op}"))),
    }))
}

/// Like [`build_atom`], as AST node.
fn build_atom_pred(field: &CustomFieldDef, op: &str, value: Validated) -> Result<Node, FilterError> {
    if field.data_type == "documentlink" && op == "contains" {
        let Validated::List(items) = value else {
            return Err(FilterError::internal("contains value"));
        };
        if items.is_empty() {
            let cond = CfCond { column: CfColumn::DocumentIds, test: CfTest::IsNull(false) };
            return Ok(Node::Pred(Pred::CustomField { field_id: field.id, test: Some(cond) }));
        }
        let ids = items
            .into_iter()
            .map(|v| match v {
                Validated::Int(i) => Ok(i),
                _ => Err(FilterError::internal("contains item")),
            })
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Node::DocLinkContains { field_id: field.id, ids });
    }
    let mut value_field = value_field_name(&field.data_type).ok_or_else(|| FilterError::internal("NotImplementedError: data type"))?;
    if field.data_type == "monetary" && (ARITHMETIC.contains(&op) || op == "exact" || op == "in") {
        value_field = "value_monetary_amount";
    }
    if op == "exists" {
        let Validated::Bool(b) = value else {
            return Err(FilterError::internal("exists value"));
        };
        let has = Pred::CustomField { field_id: field.id, test: None };
        return Ok(Node::Pred(if b { has } else { has.negate() }));
    }
    let column = cf_column(value_field).ok_or_else(|| FilterError::internal("value column"))?;
    Ok(Node::Pred(match value_lookup_ast(op, &value)? {
        Lookup::Const(false) => Pred::False,
        Lookup::Const(true) => Pred::CustomField { field_id: field.id, test: None },
        Lookup::Test(test) => Pred::CustomField { field_id: field.id, test: Some(CfCond { column, test }) },
    }))
}

/// Document link `contains [ids]`: intersection of the back references (like [`resolve`]).
async fn doc_link_contains_ids<D: FilterDb>(field_id: i64, ids: &[PyInt], db: &D) -> Result<Option<Vec<i64>>, FilterError> {
    // Values outside i64 cannot be document IDs → missing
    let mut wanted: Vec<i64> = Vec::new();
    for i in ids {
        match i.as_i64() {
            Some(x) => {
                if !wanted.contains(&x) {
                    wanted.push(x)
                }
            }
            None => return Ok(None),
        }
    }
    let links = db.doc_link_instances(field_id, &wanted).await?;
    let found: Vec<i64> = links.iter().map(|(d, _)| *d).collect();
    if wanted.iter().any(|w| !found.contains(w)) {
        return Ok(None);
    }
    let mut acc: Option<Vec<i64>> = None;
    for (_, value) in &links {
        let set: Vec<i64> = match value {
            Json::Array(a) => {
                let mut s = Vec::new();
                for v in a {
                    if let Some(id) = link_id(v)?
                        && !s.contains(&id)
                    {
                        s.push(id);
                    }
                }
                s
            }
            _ => return Err(FilterError::internal("TypeError: value_document_ids not iterable")),
        };
        acc = Some(match acc {
            None => set,
            Some(prev) => prev.into_iter().filter(|x| set.contains(x)).collect(),
        });
    }
    let mut ids = acc.unwrap_or_default();
    if ids.is_empty() {
        return Ok(None);
    }
    ids.sort_unstable();
    Ok(Some(ids))
}

async fn resolve_pred<D: FilterDb>(node: Node, db: &D) -> Result<Pred, FilterError> {
    fn inner<'a, D: FilterDb>(
        node: Node,
        db: &'a D,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Pred, FilterError>> + Send + 'a>> {
        Box::pin(async move {
            Ok(match node {
                Node::Pred(p) => p,
                Node::Leaf(_) => return Err(FilterError::internal("SQL node in AST resolve")),
                Node::Not(n) => inner(*n, db).await?.negate(),
                Node::And(nodes) => {
                    let mut acc = Pred::True;
                    for n in nodes {
                        acc = acc.and(inner(n, db).await?);
                    }
                    acc
                }
                Node::Or(nodes) => {
                    let mut acc = Pred::False;
                    for n in nodes {
                        acc = acc.or(inner(n, db).await?);
                    }
                    acc
                }
                Node::DocLinkContains { field_id, ids } => match doc_link_contains_ids(field_id, &ids, db).await? {
                    None => Pred::False,
                    Some(ids) => Pred::NumIn { col: NumCol::Id, values: ids },
                },
            })
        })
    }
    inner(node, db).await
}

/// Like [`custom_field_query_cond`], as AST (L1). Same checks, errors and DB accesses.
pub async fn custom_field_query_pred<D: FilterDb>(
    value: &str,
    validation_prefix: &str,
    ctx: &FilterContext,
    db: &D,
) -> Result<Pred, FilterError> {
    if value.is_empty() {
        return Ok(Pred::True);
    }
    let Some(expr) = PyJson::loads(value) else {
        return Err(FilterError::field(validation_prefix, "Value must be valid JSON."));
    };
    let mut ids = Vec::new();
    let mut names = Vec::new();
    let mut atoms = 0;
    collect_keys(&expr, 1, &mut atoms, &mut ids, &mut names);
    let fields = db.custom_fields_lookup(&ids, &names).await?;
    let mut by_id = HashMap::new();
    let mut by_name = HashMap::new();
    for f in fields {
        by_name.insert(f.name.clone(), f.clone());
        by_id.insert(f.id, f);
    }
    let node = {
        let mut p = Parser {
            by_id: &by_id,
            by_name: &by_name,
            cache: Vec::new(),
            depth: 0,
            atoms: 0,
            max_depth: CUSTOM_FIELD_QUERY_MAX_DEPTH,
            max_atoms: CUSTOM_FIELD_QUERY_MAX_ATOMS,
            ctx,
            ast: true,
        };
        p.parse_expr(&expr, validation_prefix)?
    };
    resolve_pred(node, db).await
}
