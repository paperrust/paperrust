//! Ordering of the document list.
//!
//! upstream: src/documents/filters.py@v3.4.2 (DocumentsOrderingFilter),
//! src/documents/views.py@v3.4.2 (DocumentViewSet.ordering_fields, get_queryset:
//! `.order_by("-created", "-id")`, `num_notes`), rest_framework/filters.py (OrderingFilter)

use sea_query::{Alias, Expr, ExprTrait, JoinType, Order, Query, SelectStatement, SimpleExpr};
use serde_json::Value as Json;

use crate::db::{CustomFieldDef, FilterDb};
use crate::error::FilterError;
use crate::pyval::{py_int, py_strip};
use crate::sql::{CFI, CORRESPONDENT, DOCTYPE, NOTE, STORAGEPATH, col, doc_col, exists, scalar, select_one_from};

/// `DocumentViewSet.ordering_fields`
pub const ORDERING_FIELDS: &[&str] = &[
    "id",
    "title",
    "correspondent__name",
    "document_type__name",
    "storage_path__name",
    "created",
    "modified",
    "added",
    "archive_serial_number",
    "num_notes",
    "owner",
    "page_count",
    "custom_field_",
];

const PREFIX: &str = "custom_field_";

/// `ordering=custom_field_<not an int>` (upstream `int()` → ValueError → 500) and
/// `custom_field_<id>` together with other terms (upstream FieldError → 500). PaperRust: 400
/// (B-305/B-313/D-039, Known differences).
pub const ORDERING_CF_INVALID: &str = "Invalid custom field ordering. Use custom_field_<id> or -custom_field_<id> on its own.";

/// Sort expression plus a LEFT JOIN where needed.
///
/// Phase 6 (docs/contract/pgvalidate.md "Corrections"): foreign key names are sorted like in
/// Django via `LEFT OUTER JOIN … ORDER BY "<table>"."name"` instead of a correlated
/// scalar subquery per row; `num_notes` via a pre-grouped note count.
/// Result and NULL ordering are the same (PostgreSQL: ASC = NULLS LAST, DESC = NULLS
/// FIRST, like Django without `nulls_first/last`); no additional tiebreaker (like upstream).
enum Join {
    /// `LEFT JOIN <table> AS <alias> ON <alias>.id = documents_document.<fk>`
    Related { table: &'static str, fk: &'static str, alias: &'static str },
    /// `LEFT JOIN (SELECT document_id, COUNT(*) AS n FROM documents_note WHERE deleted_at IS NULL
    /// GROUP BY document_id) AS onn ON onn.document_id = documents_document.id`
    Notes,
}

impl Join {
    fn alias(&self) -> &'static str {
        match self {
            Join::Related { alias, .. } => alias,
            Join::Notes => NOTES_ALIAS,
        }
    }

    fn apply(&self, q: &mut SelectStatement) {
        match self {
            Join::Related { table, fk, alias } => {
                q.join_as(JoinType::LeftJoin, Alias::new(*table), Alias::new(*alias), col(alias, "id").eq(doc_col(fk)));
            }
            Join::Notes => {
                let counts = Query::select()
                    .column((Alias::new("nn"), Alias::new("document_id")))
                    .expr_as(Expr::cust("COUNT(*)"), Alias::new("n"))
                    .from_as(Alias::new(NOTE), Alias::new("nn"))
                    .and_where(col("nn", "deleted_at").is_null())
                    .group_by_col((Alias::new("nn"), Alias::new("document_id")))
                    .to_owned();
                q.join_subquery(JoinType::LeftJoin, counts, Alias::new(NOTES_ALIAS), col(NOTES_ALIAS, "document_id").eq(doc_col("id")));
            }
        }
    }
}

const NOTES_ALIAS: &str = "onn";

/// `num_notes = Coalesce(Subquery(Note.objects.filter(document=OuterRef("pk"))…Count), 0)`
/// as the value of the grouped count (requires [`Join::Notes`]).
fn num_notes_expr() -> SimpleExpr {
    Expr::cust_with_exprs("COALESCE($1, 0)", [col(NOTES_ALIAS, "n")])
}

fn field_expr(name: &str) -> Option<(SimpleExpr, Option<Join>)> {
    let related = |table, fk, alias| (col(alias, "name"), Some(Join::Related { table, fk, alias }));
    Some(match name {
        "id" | "title" | "created" | "modified" | "added" | "archive_serial_number" | "page_count" => (doc_col(name), None),
        // Order by the FK: User has no Meta.ordering → owner_id
        "owner" => (doc_col("owner_id"), None),
        "correspondent__name" => related(CORRESPONDENT, "correspondent_id", "oc"),
        "document_type__name" => related(DOCTYPE, "document_type_id", "odt"),
        "storage_path__name" => related(STORAGEPATH, "storage_path_id", "osp"),
        "num_notes" => (num_notes_expr(), Some(Join::Notes)),
        _ => return None,
    })
}

/// Default from `DocumentViewSet.get_queryset`: `-created, -id`.
pub fn apply_default_ordering(q: &mut SelectStatement) {
    q.order_by_expr(doc_col("created"), Order::Desc).order_by_expr(doc_col("id"), Order::Desc);
}

/// DRF `OrderingFilter.get_ordering` → valid terms (or empty).
pub fn valid_terms(param: &str) -> Vec<(String, bool)> {
    param
        .split(',')
        .map(py_strip)
        .filter_map(|t| {
            let (desc, name) = match t.strip_prefix('-') {
                Some(n) => (true, n),
                None => (false, t),
            };
            ORDERING_FIELDS.contains(&name).then(|| (name.to_string(), desc))
        })
        .collect()
}

/// Value subquery `CustomFieldInstance.objects.filter(document_id=OuterRef("id"),
/// field_id=…).values(<column>)[:1]` (default ordering `created`).
fn cf_value_subquery(field_id: i64, column: &str) -> SimpleExpr {
    let sub = Query::select()
        .column((Alias::new("cfo"), Alias::new(column)))
        .from_as(Alias::new(CFI), Alias::new("cfo"))
        .and_where(col("cfo", "document_id").eq(doc_col("id")))
        .and_where(col("cfo", "field_id").eq(field_id))
        .and_where(col("cfo", "deleted_at").is_null())
        .order_by((Alias::new("cfo"), Alias::new("created")), Order::Asc)
        .limit(1)
        .to_owned();
    scalar(sub)
}

/// Python `str(x)` for values that Django compares via `CharField.get_prep_value`.
fn json_py_str(v: &Json) -> String {
    crate::cfquery::json_to_py(v).py_str()
}

/// SELECT field: index of the option in the list sorted by label (sum over all
/// instances of the document like upstream `Sum(Case(When(...)))`).
fn select_order_expr(field: &CustomFieldDef) -> Result<SimpleExpr, FilterError> {
    let (opts_sorted, n) = select_options_sorted(field)?;
    let field_match = || col("cfs", "field_id").eq(field.id);
    let mut case: Option<sea_query::CaseStatement> = None;
    for (pos, id) in opts_sorted.iter().enumerate() {
        let cond = match id {
            None => field_match().and(col("cfs", "value_select").is_null()),
            Some(s) => field_match().and(col("cfs", "value_select").eq(s.clone())),
        };
        case = Some(match case {
            None => Expr::case(cond, Expr::val(pos as i64)),
            Some(c) => c.case(cond, Expr::val(pos as i64)),
        });
    }
    let null_cond = field_match().and(col("cfs", "value_select").is_null());
    let case = match case {
        None => Expr::case(null_cond, Expr::val(n)),
        Some(c) => c.case(null_cond, Expr::val(n)),
    }
    .finally(Expr::val(0i64));
    let sum = Query::select()
        .expr(Expr::cust_with_exprs("SUM($1)", [case.into()]))
        .from_as(Alias::new(CFI), Alias::new("cfs"))
        .and_where(col("cfs", "document_id").eq(doc_col("id")))
        .to_owned();
    Ok(Expr::cust_with_exprs("COALESCE($1, 0)", [scalar(sum)]))
}

/// Select options in label order (`None` = option without `id`) and their count, with the
/// Python errors of upstream (`sorted(..., key=label)`).
pub fn select_options_sorted(field: &CustomFieldDef) -> Result<(Vec<Option<String>>, i64), FilterError> {
    let options: Vec<Json> = match &field.extra_data {
        Some(Json::Object(m)) => match m.get("select_options") {
            None => Vec::new(),
            Some(Json::Array(a)) => a.clone(),
            Some(_) => return Err(FilterError::internal("select_options is not a list")),
        },
        _ => return Err(FilterError::internal("AttributeError: extra_data.get")),
    };
    let mut opts: Vec<&serde_json::Map<String, Json>> = Vec::new();
    for o in &options {
        match o {
            Json::Object(m) => opts.push(m),
            _ => return Err(FilterError::internal("AttributeError: option.get")),
        }
    }
    // sorted(..., key=lambda x: x.get("label")) – Python comparison, mixed types → TypeError
    let labels: Vec<Json> = opts.iter().map(|m| m.get("label").cloned().unwrap_or(Json::Null)).collect();
    let all_str = labels.iter().all(|l| l.is_string());
    let all_null = labels.iter().all(|l| l.is_null());
    let all_num = labels.iter().all(|l| l.is_number() || l.is_boolean());
    let mut idx: Vec<usize> = (0..opts.len()).collect();
    if opts.len() > 1 {
        if all_str {
            idx.sort_by(|a, b| labels[*a].as_str().cmp(&labels[*b].as_str()));
        } else if all_num {
            let num = |v: &Json| match v {
                Json::Bool(b) => *b as i64 as f64,
                other => other.as_f64().unwrap_or(f64::NAN),
            };
            idx.sort_by(|a, b| num(&labels[*a]).partial_cmp(&num(&labels[*b])).unwrap_or(std::cmp::Ordering::Equal));
        } else if !all_null {
            return Err(FilterError::internal("TypeError: '<' not supported between label types"));
        } else {
            return Err(FilterError::internal("TypeError: '<' not supported between NoneType"));
        }
    }
    let sorted = idx
        .iter()
        .map(|i| match opts[*i].get("id").cloned().unwrap_or(Json::Null) {
            Json::Null => None,
            other => Some(json_py_str(&other)),
        })
        .collect();
    Ok((sorted, opts.len() as i64))
}

fn custom_field_value_expr(field: &CustomFieldDef) -> Result<SimpleExpr, FilterError> {
    let column = match field.data_type.as_str() {
        // upstream also sorts LONG_TEXT via value_text
        "string" | "longtext" => "value_text",
        "integer" => "value_int",
        "float" => "value_float",
        "date" => "value_date",
        "monetary" => "value_monetary_amount",
        "documentlink" => "value_document_ids",
        "url" => "value_url",
        "boolean" => "value_bool",
        "select" => return select_order_expr(field),
        _ => return Err(FilterError::internal("ValueError: Invalid custom field data type")),
    };
    Ok(cf_value_subquery(field.id, column))
}

/// `DocumentsOrderingFilter.filter_queryset` + DRF `OrderingFilter`.
///
/// Expects a query **without** ORDER BY; sets the complete ordering including the
/// default `-created, -id` if no valid term is given. Like upstream there is no
/// additional tiebreaker for explicit ordering.
pub async fn apply_ordering<D: FilterDb>(q: &mut SelectStatement, ordering: Option<&str>, db: &D) -> Result<(), FilterError> {
    let param = ordering.unwrap_or("");
    if !param.is_empty() && param.contains(PREFIX) {
        let after = param.split(PREFIX).nth(1).unwrap_or("");
        let Some(id) = py_int(after) else {
            return Err(FilterError::field("ordering", ORDERING_CF_INVALID));
        };
        let id_str = match id {
            crate::pyval::PyInt::Small(v) => v.to_string(),
            crate::pyval::PyInt::Big { negative } => {
                // str(int(...)) normalized: sign, without underscores/leading zeros
                let digits: String = py_strip(after).trim_start_matches(['+', '-']).chars().filter(|c| *c != '_').collect();
                format!("{}{}", if negative { "-" } else { "" }, digits.trim_start_matches('0'))
            }
        };
        let field = match id.as_i64() {
            Some(i) => db.custom_field_by_id(i).await?,
            None => None,
        };
        let Some(field) = field else {
            return Err(FilterError::field(&format!("{PREFIX}{id_str}"), "Custom field not found"));
        };
        let value = custom_field_value_expr(&field)?;
        let replaced = param.replace(&format!("{PREFIX}{id_str}"), "custom_field_value");
        let order = match replaced.as_str() {
            "custom_field_value" => Order::Asc,
            "-custom_field_value" => Order::Desc,
            // upstream: Django order_by("…") with unknown name → FieldError → 500; PaperRust 400
            _ => return Err(FilterError::field("ordering", ORDERING_CF_INVALID)),
        };
        let mut has = select_one_from(CFI, "cfh");
        has.and_where(col("cfh", "document_id").eq(doc_col("id")))
            .and_where(col("cfh", "field_id").eq(field.id))
            .and_where(col("cfh", "deleted_at").is_null());
        q.order_by_expr(exists(has), Order::Desc);
        q.order_by_expr(value, order);
        return Ok(());
    }
    let terms = valid_terms(param);
    if terms.is_empty() {
        apply_default_ordering(q);
        return Ok(());
    }
    let mut joined: Vec<&'static str> = Vec::new();
    for (name, desc) in terms {
        let Some((expr, join)) = field_expr(&name) else {
            // "custom_field_" itself is only reachable via the branch above
            return Err(FilterError::internal("FieldError"));
        };
        if let Some(j) = join
            && !joined.contains(&j.alias())
        {
            j.apply(q);
            joined.push(j.alias());
        }
        q.order_by_expr(expr, if desc { Order::Desc } else { Order::Asc });
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// L1: `ordering` → pl_domain::filter::Sort (same errors as apply_ordering)
// ---------------------------------------------------------------------------------------

use pl_domain::filter::{CfSortValue, Sort, SortKey};

fn sort_key(name: &str) -> Option<SortKey> {
    Some(match name {
        "id" => SortKey::Id,
        "title" => SortKey::Title,
        "created" => SortKey::Created,
        "modified" => SortKey::Modified,
        "added" => SortKey::Added,
        "archive_serial_number" => SortKey::Asn,
        "page_count" => SortKey::PageCount,
        "owner" => SortKey::Owner,
        "correspondent__name" => SortKey::CorrespondentName,
        "document_type__name" => SortKey::DocumentTypeName,
        "storage_path__name" => SortKey::StoragePathName,
        "num_notes" => SortKey::NumNotes,
        _ => return None,
    })
}

fn custom_field_sort_value(field: &CustomFieldDef) -> Result<CfSortValue, FilterError> {
    use pl_domain::filter::CfColumn as C;
    Ok(CfSortValue::Column(match field.data_type.as_str() {
        "string" | "longtext" => C::Text,
        "integer" => C::Int,
        "float" => C::Float,
        "date" => C::Date,
        "monetary" => C::MonetaryAmount,
        "documentlink" => C::DocumentIds,
        "url" => C::Url,
        "boolean" => C::Bool,
        "select" => return Ok(CfSortValue::SelectOptions(select_options_sorted(field)?.0)),
        _ => return Err(FilterError::internal("ValueError: Invalid custom field data type")),
    }))
}

/// `DocumentsOrderingFilter` + DRF `OrderingFilter` as [`Sort`] (L1).
pub async fn parse_ordering<D: FilterDb>(ordering: Option<&str>, db: &D) -> Result<Sort, FilterError> {
    let param = ordering.unwrap_or("");
    if !param.is_empty() && param.contains(PREFIX) {
        let after = param.split(PREFIX).nth(1).unwrap_or("");
        let Some(id) = py_int(after) else {
            return Err(FilterError::field("ordering", ORDERING_CF_INVALID));
        };
        let id_str = match id {
            crate::pyval::PyInt::Small(v) => v.to_string(),
            crate::pyval::PyInt::Big { negative } => {
                let digits: String = py_strip(after).trim_start_matches(['+', '-']).chars().filter(|c| *c != '_').collect();
                format!("{}{}", if negative { "-" } else { "" }, digits.trim_start_matches('0'))
            }
        };
        let field = match id.as_i64() {
            Some(i) => db.custom_field_by_id(i).await?,
            None => None,
        };
        let Some(field) = field else {
            return Err(FilterError::field(&format!("{PREFIX}{id_str}"), "Custom field not found"));
        };
        let value = custom_field_sort_value(&field)?;
        let replaced = param.replace(&format!("{PREFIX}{id_str}"), "custom_field_value");
        let desc = match replaced.as_str() {
            "custom_field_value" => false,
            "-custom_field_value" => true,
            _ => return Err(FilterError::field("ordering", ORDERING_CF_INVALID)),
        };
        return Ok(Sort::CustomField { field_id: field.id, value, desc });
    }
    let terms = valid_terms(param);
    if terms.is_empty() {
        return Ok(Sort::Default);
    }
    let mut out = Vec::with_capacity(terms.len());
    for (name, desc) in terms {
        let Some(k) = sort_key(&name) else {
            return Err(FilterError::internal("FieldError"));
        };
        out.push((k, desc));
    }
    Ok(Sort::Terms(out))
}
