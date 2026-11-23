//! `DocumentFilterSet` (django-filter) of the document list.
//!
//! upstream: src/documents/filters.py@v3.4.2 (DocumentFilterSet, ObjectFilter, InboxFilter,
//! TitleContentFilter, EffectiveContentFilter, SharedByUser, CustomFieldsFilter,
//! MimeTypeFilter, CustomFieldQueryFilter), django_filters (FilterSet, NumberFilter,
//! BaseInFilter, CharFilter, BooleanFilter/BooleanWidget, DateFilter, IsoDateTimeFilter)
//!
//! Flow like django-filter: first **all** form fields are validated (errors are collected in
//! field order → HTTP 400 `{"field": ["…"]}`), then the filters are applied in the same
//! order. Multi-valued relations (tags, custom field instances) are formulated as
//! correlated `EXISTS` subqueries – no duplicate rows, no `DISTINCT`.

use chrono::{DateTime, NaiveDate, Utc};
use sea_query::{Alias, Expr, ExprTrait, Query, SelectStatement, SimpleExpr};
use serde_json::{Map, Value as Json};

use crate::cfquery::custom_field_query_cond;
use crate::dates::{
    DateTimeInputError, django_form_date, iso_datetime_field, py_naive_datetime_str, start_of_local_day, year_bounds, year_bounds_datetime,
};
use crate::db::FilterDb;
use crate::error::FilterError;
use crate::perms::{document_is_shared, visible_root_documents};
use crate::pyval::{NUMBER_FILTER_MAX, PyDecimal, PyInt, RangeCheck, py_int, py_strip};
use crate::sql::{
    CF, CFI, CORRESPONDENT, Cond, DOC, DOC_TAGS, DOCTYPE, STORAGEPATH, TAG, TextLookup, col, doc_col, effective_content, exists,
    folder_exists, select_one_from, text_lookup,
};
use crate::textfold::{UpperFold, folded_lookup};
use crate::{FilterContext, User};

const AUTO_MIN: i64 = -2_147_483_648;
const AUTO_MAX: i64 = 2_147_483_647;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumTarget {
    Id,
    Asn,
    CorrespondentId,
    TagsId,
    DocumentTypeId,
    StoragePathId,
    OwnerId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cmp {
    Exact,
    Gt,
    Gte,
    Lt,
    Lte,
}

#[derive(Debug, Clone, Copy)]
enum CharTarget {
    Doc(&'static str),
    Fk(&'static str, &'static str),
    TagsName,
}

#[derive(Debug, Clone, Copy)]
enum Part {
    Year,
    Month,
    Day,
}

#[derive(Debug, Clone, Copy)]
enum ObjRel {
    Tags,
    Fk(&'static str),
    CustomFields,
}

#[derive(Debug, Clone, Copy)]
enum Spec {
    Num(NumTarget, Cmp),
    NumIn(NumTarget),
    Char(CharTarget, TextLookup),
    IsNull(&'static str),
    /// (column, is DateTimeField, part)
    DatePart(&'static str, bool, Part),
    /// DateFilter on DateField (`created__gt`, `created__date__gt`)
    DateCmp(&'static str, Cmp),
    /// DateFilter on `__date` of a DateTimeField (`added__date__gt`)
    DtDateCmp(&'static str, Cmp),
    /// IsoDateTimeFilter (`added__gt`)
    DtCmp(&'static str, Cmp),
    HasDuplicates,
    IsTagged,
    ObjAll(ObjRel),
    ObjNone(ObjRel),
    ObjIn(ObjRel),
    Inbox,
    TitleContent,
    Content(TextLookup),
    CustomFieldsIContains,
    HasCustomFields,
    CustomFieldQuery,
    SharedBy,
    MimeType,
    /// PaperRust folder view: `folder__id` (NumberFilter-like, one id)
    FolderId,
    /// PaperRust folder view: `folder__id__in` (`false`) / `folder__id__tree` (`true`, incl. subfolders)
    FolderIn(bool),
    /// PaperRust folder view: `folder__isnull` (`true` = "Unsorted")
    FolderIsNull,
}

use CharTarget as CT;
use Cmp::*;
use NumTarget as NT;
use Spec::*;
use TextLookup::{IContains, IEndsWith, IExact, IStartsWith};

macro_rules! chars {
    ($p:literal, $t:expr) => {
        [
            (concat!($p, "__istartswith"), Char($t, IStartsWith)),
            (concat!($p, "__iendswith"), Char($t, IEndsWith)),
            (concat!($p, "__icontains"), Char($t, IContains)),
            (concat!($p, "__iexact"), Char($t, IExact)),
        ]
    };
}

/// All filters in django-filter order: first `Meta.fields` (declared filters with the same
/// name take their place there), then the remaining declared filters in declaration
/// order.
fn filters() -> Vec<(&'static str, Spec)> {
    let mut v: Vec<(&'static str, Spec)> = Vec::new();
    v.push(("id__in", NumIn(NT::Id)));
    v.push(("id", Num(NT::Id, Exact)));
    v.extend(chars!("title", CT::Doc("title")));
    v.extend([
        ("archive_serial_number", Num(NT::Asn, Exact)),
        ("archive_serial_number__gt", Num(NT::Asn, Gt)),
        ("archive_serial_number__gte", Num(NT::Asn, Gte)),
        ("archive_serial_number__lt", Num(NT::Asn, Lt)),
        ("archive_serial_number__lte", Num(NT::Asn, Lte)),
        ("archive_serial_number__isnull", IsNull("archive_serial_number")),
        ("created__year", DatePart("created", false, Part::Year)),
        ("created__month", DatePart("created", false, Part::Month)),
        ("created__day", DatePart("created", false, Part::Day)),
        ("created__gt", DateCmp("created", Gt)),
        ("created__gte", DateCmp("created", Gte)),
        ("created__lt", DateCmp("created", Lt)),
        ("created__lte", DateCmp("created", Lte)),
    ]);
    for (y, m, d, dgt, dgte, gt, gte, dlt, dlte, lt, lte, c) in [
        (
            "added__year",
            "added__month",
            "added__day",
            "added__date__gt",
            "added__date__gte",
            "added__gt",
            "added__gte",
            "added__date__lt",
            "added__date__lte",
            "added__lt",
            "added__lte",
            "added",
        ),
        (
            "modified__year",
            "modified__month",
            "modified__day",
            "modified__date__gt",
            "modified__date__gte",
            "modified__gt",
            "modified__gte",
            "modified__date__lt",
            "modified__date__lte",
            "modified__lt",
            "modified__lte",
            "modified",
        ),
    ] {
        v.extend([
            (y, DatePart(c, true, Part::Year)),
            (m, DatePart(c, true, Part::Month)),
            (d, DatePart(c, true, Part::Day)),
            (dgt, DtDateCmp(c, Gt)),
            (dgte, DtDateCmp(c, Gte)),
            (gt, DtCmp(c, Gt)),
            (gte, DtCmp(c, Gte)),
            (dlt, DtDateCmp(c, Lt)),
            (dlte, DtDateCmp(c, Lte)),
            (lt, DtCmp(c, Lt)),
            (lte, DtCmp(c, Lte)),
        ]);
    }
    v.extend(chars!("original_filename", CT::Doc("original_filename")));
    v.extend(chars!("checksum", CT::Doc("checksum")));
    v.push(("correspondent__isnull", IsNull("correspondent_id")));
    v.push(("correspondent__id__in", NumIn(NT::CorrespondentId)));
    v.push(("correspondent__id", Num(NT::CorrespondentId, Exact)));
    v.extend(chars!("correspondent__name", CT::Fk(CORRESPONDENT, "correspondent_id")));
    // tags__id__in: declared ObjectFilter at the Meta position
    v.push(("tags__id__in", ObjIn(ObjRel::Tags)));
    v.push(("tags__id", Num(NT::TagsId, Exact)));
    v.extend(chars!("tags__name", CT::TagsName));
    v.push(("document_type__isnull", IsNull("document_type_id")));
    v.push(("document_type__id__in", NumIn(NT::DocumentTypeId)));
    v.push(("document_type__id", Num(NT::DocumentTypeId, Exact)));
    v.extend(chars!("document_type__name", CT::Fk(DOCTYPE, "document_type_id")));
    v.push(("storage_path__isnull", IsNull("storage_path_id")));
    v.push(("storage_path__id__in", NumIn(NT::StoragePathId)));
    v.push(("storage_path__id", Num(NT::StoragePathId, Exact)));
    v.extend(chars!("storage_path__name", CT::Fk(STORAGEPATH, "storage_path_id")));
    v.push(("owner__isnull", IsNull("owner_id")));
    v.push(("owner__id__in", NumIn(NT::OwnerId)));
    v.push(("owner__id", Num(NT::OwnerId, Exact)));
    // custom_fields__icontains: declared (CustomFieldsFilter) at the Meta position
    v.push(("custom_fields__icontains", CustomFieldsIContains));
    // remaining declared filters
    v.extend([
        ("has_duplicates", HasDuplicates),
        ("is_tagged", IsTagged),
        ("tags__id__all", ObjAll(ObjRel::Tags)),
        ("tags__id__none", ObjNone(ObjRel::Tags)),
        ("correspondent__id__none", ObjNone(ObjRel::Fk("correspondent_id"))),
        ("document_type__id__none", ObjNone(ObjRel::Fk("document_type_id"))),
        ("storage_path__id__none", ObjNone(ObjRel::Fk("storage_path_id"))),
        ("is_in_inbox", Inbox),
        ("title_content", TitleContent),
        ("content__istartswith", Content(IStartsWith)),
        ("content__iendswith", Content(IEndsWith)),
        ("content__icontains", Content(IContains)),
        ("content__iexact", Content(IExact)),
        ("owner__id__none", ObjNone(ObjRel::Fk("owner_id"))),
        ("custom_fields__id__all", ObjAll(ObjRel::CustomFields)),
        ("custom_fields__id__none", ObjNone(ObjRel::CustomFields)),
        ("custom_fields__id__in", ObjIn(ObjRel::CustomFields)),
        ("has_custom_fields", HasCustomFields),
        ("custom_field_query", CustomFieldQuery),
        ("shared_by__id", SharedBy),
        ("mime_type", MimeType),
        ("created__date__gt", DateCmp("created", Gt)),
        ("created__date__gte", DateCmp("created", Gte)),
        ("created__date__lt", DateCmp("created", Lt)),
        ("created__date__lte", DateCmp("created", Lte)),
    ]);
    // PaperRust folder view (not in paperless-ngx, there unknown parameters are ignored;
    // docs/contract/pl-docfilter.md "Known differences"). Last, so the form error order of the upstream
    // filters stays unchanged.
    v.extend([
        ("folder__id", FolderId),
        ("folder__id__in", FolderIn(false)),
        ("folder__id__tree", FolderIn(true)),
        ("folder__isnull", FolderIsNull),
    ]);
    v
}

/// Names of all filter parameters in form order (for docs/tests).
pub fn filter_names() -> Vec<&'static str> {
    filters().into_iter().map(|(n, _)| n).collect()
}

#[derive(Debug, Clone)]
enum Cleaned {
    Skip,
    Int(PyInt),
    IntList(Vec<Option<PyInt>>),
    Str(String),
    Bool(bool),
    Date(NaiveDate),
    Dt(DateTime<Utc>),
    Raw(String),
}

const ENTER_A_NUMBER: &str = "Enter a number.";

/// `forms.DecimalField` + `MaxValueValidator(1e50)` of `NumberFilter`.
fn clean_decimal(v: &str) -> Result<Option<PyInt>, String> {
    if v.is_empty() {
        return Ok(None);
    }
    let Some(d) = PyDecimal::parse(v) else {
        return Err(ENTER_A_NUMBER.into());
    };
    if !d.is_finite() {
        return Err(ENTER_A_NUMBER.into());
    }
    if d.gt_integer(NUMBER_FILTER_MAX) {
        return Err("Ensure this value is less than or equal to 1e+50.".into());
    }
    // Django's IntegerField.get_prep_value: int(Decimal) (truncate)
    Ok(d.trunc())
}

/// `BooleanWidget.value_from_datadict`
fn clean_bool(v: Option<&str>) -> Cleaned {
    match v.map(|s| s.to_lowercase()).as_deref() {
        Some("1") | Some("true") => Cleaned::Bool(true),
        Some("0") | Some("false") => Cleaned::Bool(false),
        _ => Cleaned::Skip,
    }
}

fn clean(spec: Spec, raw: Option<&str>, ctx: &FilterContext) -> Result<Cleaned, String> {
    Ok(match spec {
        Num(..) | DatePart(..) | FolderId => match raw {
            None => Cleaned::Skip,
            Some(v) => match clean_decimal(v)? {
                None => Cleaned::Skip,
                Some(i) => Cleaned::Int(i),
            },
        },
        NumIn(_) | FolderIn(_) => match raw {
            None | Some("") => Cleaned::Skip,
            Some(v) => {
                let mut out = Vec::new();
                for item in v.split(',') {
                    out.push(clean_decimal(item)?);
                }
                Cleaned::IntList(out)
            }
        },
        Char(..) => match raw {
            None => Cleaned::Skip,
            Some(v) => {
                let s = py_strip(v);
                if s.is_empty() {
                    Cleaned::Skip
                } else if s.contains('\0') {
                    return Err("Null characters are not allowed.".into());
                } else {
                    Cleaned::Str(s.to_string())
                }
            }
        },
        IsNull(_) | HasDuplicates | IsTagged | HasCustomFields | FolderIsNull => clean_bool(raw),
        DateCmp(..) | DtDateCmp(..) => match raw {
            None => Cleaned::Skip,
            Some(v) => match django_form_date(v) {
                Ok(None) => Cleaned::Skip,
                Ok(Some(d)) => Cleaned::Date(d),
                Err(()) => return Err("Enter a valid date.".into()),
            },
        },
        DtCmp(..) => match raw {
            None => Cleaned::Skip,
            Some(v) => match iso_datetime_field(v, ctx.tz) {
                Ok(None) => Cleaned::Skip,
                Ok(Some(dt)) => Cleaned::Dt(dt),
                Err(DateTimeInputError::Invalid) => return Err("Enter a valid date/time.".into()),
                Err(DateTimeInputError::Ambiguous { naive }) => {
                    return Err(format!(
                        "{} couldn\u{2019}t be interpreted in time zone {}; it may be ambiguous or it may not exist.",
                        py_naive_datetime_str(&naive),
                        ctx.tz.name()
                    ));
                }
            },
        },
        ObjAll(_)
        | ObjNone(_)
        | ObjIn(_)
        | Inbox
        | TitleContent
        | Content(_)
        | CustomFieldsIContains
        | CustomFieldQuery
        | SharedBy
        | MimeType => match raw {
            None => Cleaned::Skip,
            Some(v) => Cleaned::Raw(v.to_string()),
        },
    })
}

fn range_of(t: NumTarget) -> (i64, i64) {
    match t {
        // PositiveIntegerField on PostgreSQL
        NT::Asn => (0, AUTO_MAX),
        _ => (AUTO_MIN, AUTO_MAX),
    }
}

fn num_column(t: NumTarget) -> SimpleExpr {
    doc_col(match t {
        NT::Id => "id",
        NT::Asn => "archive_serial_number",
        NT::CorrespondentId => "correspondent_id",
        NT::DocumentTypeId => "document_type_id",
        NT::StoragePathId => "storage_path_id",
        NT::OwnerId => "owner_id",
        NT::TagsId => "id",
    })
}

/// `EXISTS (SELECT 1 FROM documents_document_tags dt WHERE dt.document_id = doc.id AND …)`
fn tag_exists(cond: Option<SimpleExpr>) -> SimpleExpr {
    let mut sel = select_one_from(DOC_TAGS, "dt");
    sel.and_where(col("dt", "document_id").eq(doc_col("id")));
    if let Some(c) = cond {
        sel.and_where(c);
    }
    exists(sel)
}

/// `EXISTS (SELECT 1 FROM documents_customfieldinstance ci WHERE ci.document_id = doc.id AND …)`
/// (incl. soft-deleted instances, like the JOIN in upstream)
fn cfi_exists(cond: Option<SimpleExpr>) -> SimpleExpr {
    let mut sel = select_one_from(CFI, "ci");
    sel.and_where(col("ci", "document_id").eq(doc_col("id")));
    if let Some(c) = cond {
        sel.and_where(c);
    }
    exists(sel)
}

fn cmp(lhs: SimpleExpr, op: Cmp, rhs: impl Into<SimpleExpr>) -> SimpleExpr {
    let rhs = rhs.into();
    match op {
        Exact => lhs.eq(rhs),
        Gt => lhs.gt(rhs),
        Gte => lhs.gte(rhs),
        Lt => lhs.lt(rhs),
        Lte => lhs.lte(rhs),
    }
}

/// Django's IntegerField lookup incl. `IntegerFieldOverflow`.
fn int_lookup(lhs: SimpleExpr, op: Cmp, v: PyInt, (min, max): (i64, i64)) -> Cond {
    match (op, v.range_check(min, max)) {
        (_, RangeCheck::InRange(x)) => Cond::Expr(cmp(lhs, op, x)),
        (Exact, _) => Cond::False,
        (Gt | Gte, RangeCheck::Underflow) => Cond::True,
        (Gt | Gte, _) => Cond::False,
        (Lt | Lte, RangeCheck::Overflow) => Cond::True,
        (Lt | Lte, _) => Cond::False,
    }
}

/// Values of an in-list that fit into the range (others cannot match).
fn in_values(list: &[Option<PyInt>], range: (i64, i64)) -> Vec<i64> {
    list.iter()
        .flatten()
        .filter_map(|v| match v.range_check(range.0, range.1) {
            RangeCheck::InRange(x) => Some(x),
            _ => None,
        })
        .collect()
}

/// `ObjectFilter`: comma-separated IDs with Python `int()`; an invalid value → no filter.
fn object_ids(raw: &str) -> Option<Vec<PyInt>> {
    if raw.is_empty() {
        return None;
    }
    raw.split(',').map(py_int).collect()
}

fn obj_exact(rel: ObjRel, id: i64) -> SimpleExpr {
    match rel {
        ObjRel::Tags => tag_exists(Some(col("dt", "tag_id").eq(id))),
        ObjRel::CustomFields => cfi_exists(Some(col("ci", "field_id").eq(id))),
        ObjRel::Fk(c) => doc_col(c).eq(id),
    }
}

fn obj_exclude(rel: ObjRel, id: i64) -> SimpleExpr {
    match rel {
        // exclude(fk__id=x) on nullable FK: NOT (fk = x AND fk IS NOT NULL)
        ObjRel::Fk(c) => doc_col(c).eq(id).and(doc_col(c).is_not_null()).not(),
        other => obj_exact(other, id).not(),
    }
}

fn duplicates_exists(user: &User) -> SimpleExpr {
    let mut sel = select_one_from(DOC, "dup");
    sel.and_where(col("dup", "root_document_id").is_null()).and_where(col("dup", "id").eq(doc_col("id")).not());
    // active superusers see everything (also in the trash) → no permission subquery needed
    if !pl_authz::objects::user_is_unrestricted(Some(user)) {
        sel.and_where(col("dup", "id").in_subquery(visible_root_documents(user)));
    }
    let nonempty_archive = || Expr::cust_with_exprs("NULLIF($1, '')", [doc_col("archive_checksum")]);
    sel.and_where(
        col("dup", "checksum")
            .eq(doc_col("checksum"))
            .or(col("dup", "checksum").eq(nonempty_archive()))
            .or(col("dup", "archive_checksum").eq(doc_col("checksum")))
            .or(col("dup", "archive_checksum").eq(nonempty_archive())),
    );
    exists(sel)
}

/// Candidates for `has_duplicates=true` (performance, phase 6, docs/contract/pgvalidate.md
/// section 8): documents whose `checksum` or `archive_checksum` occurs at least twice as a
/// checksum in the **whole** collection (`checksum` twice, `archive_checksum` twice or
/// `checksum` = `archive_checksum` of any document). Every document with a duplicate in the
/// sense of [`duplicates_exists`] belongs to it (the shared key occurs in both
/// documents) – the set is a **superset**; the exact condition (visibility,
/// `id <>`, `NULLIF`) is then still checked by [`duplicates_exists`], only for the candidates.
///
/// The three subsets run as index-only scans over the checksum indexes (sorted, without
/// hash/sort in memory). `ARRAY(...)` turns that into an InitPlan, the outer query reads the
/// candidates via `id = ANY(...)` over the primary key, instead of (as before) checking 97,000 rows in
/// `created` order, each with a BitmapOr subquery.
pub fn duplicate_candidates() -> SimpleExpr {
    Expr::cust(
        r#""documents_document"."id" = ANY(ARRAY(SELECT "dupc"."id" FROM "documents_document" AS "dupc" INNER JOIN (SELECT "dupk1"."checksum" AS "k" FROM "documents_document" AS "dupk1" GROUP BY "dupk1"."checksum" HAVING COUNT(*) > 1 UNION ALL SELECT "dupk2"."archive_checksum" FROM "documents_document" AS "dupk2" WHERE "dupk2"."archive_checksum" IS NOT NULL GROUP BY "dupk2"."archive_checksum" HAVING COUNT(*) > 1 UNION ALL SELECT "dupk3"."checksum" FROM "documents_document" AS "dupk3" WHERE EXISTS(SELECT 1 FROM "documents_document" AS "dupk4" WHERE "dupk4"."archive_checksum" = "dupk3"."checksum")) AS "dupk" ON "dupc"."checksum" = "dupk"."k" OR "dupc"."archive_checksum" = "dupk"."k"))"#,
    )
}

fn json_py_str(v: &Json) -> String {
    crate::cfquery::json_to_py(v).py_str()
}

/// IDs of the select options whose label contains `value` (`CustomFieldsFilter`).
async fn icontains_option_ids<D: FilterDb>(value: &str, db: &D) -> Result<Vec<String>, FilterError> {
    let fields = db.custom_fields_extra_data_icontains(value).await?;
    let needle = value.to_lowercase();
    let mut option_ids: Vec<String> = Vec::new();
    for f in &fields {
        let options = match &f.extra_data {
            Some(Json::Object(m)) => m.get("select_options").cloned().unwrap_or(Json::Array(vec![])),
            _ => return Err(FilterError::internal("AttributeError: extra_data.get")),
        };
        let items: Vec<Json> = match options {
            Json::Array(a) => a,
            // Iteration over dict keys or string characters → option.get() fails
            Json::Object(m) if m.is_empty() => vec![],
            Json::String(s) if s.is_empty() => vec![],
            _ => return Err(FilterError::internal("TypeError: select_options")),
        };
        for o in items {
            let Json::Object(o) = o else {
                return Err(FilterError::internal("AttributeError: option.get"));
            };
            let Some(Json::String(label)) = o.get("label") else {
                return Err(FilterError::internal("AttributeError: label.lower"));
            };
            if label.to_lowercase().contains(&needle) {
                match o.get("id") {
                    None | Some(Json::Null) => {}
                    Some(id) => option_ids.push(json_py_str(id)),
                }
            }
        }
    }
    Ok(option_ids)
}

/// `CustomFieldsFilter` (custom_fields__icontains)
async fn custom_fields_icontains<D: FilterDb>(value: &str, db: &D) -> Result<Cond, FilterError> {
    let option_ids = icontains_option_ids(value, db).await?;
    let lk = |c: SimpleExpr| text_lookup(c, IContains, value);
    let field_names = Query::select()
        .column((Alias::new("cfn"), Alias::new("id")))
        .from_as(Alias::new(CF), Alias::new("cfn"))
        .and_where(lk(col("cfn", "name")))
        .to_owned();
    let mut any = col("ci", "field_id").in_subquery(field_names);
    for c in ["value_text", "value_bool", "value_int", "value_float", "value_date", "value_url", "value_monetary", "value_document_ids"] {
        any = any.or(lk(col("ci", c)));
    }
    if !option_ids.is_empty() {
        any = any.or(col("ci", "value_select").is_in(option_ids));
    }
    any = any.or(lk(col("ci", "value_long_text")));
    Ok(Cond::Expr(cfi_exists(Some(any))))
}

async fn filter_cond<D: FilterDb>(
    spec: Spec,
    value: Cleaned,
    user: &User,
    ctx: &FilterContext,
    db: &D,
    fold: Option<&UpperFold>,
) -> Result<Cond, FilterError> {
    use Cleaned as C;
    Ok(match (spec, value) {
        (_, C::Skip) => Cond::True,
        (Num(NT::TagsId, _), C::Int(v)) => match v.range_check(AUTO_MIN, AUTO_MAX) {
            RangeCheck::InRange(x) => Cond::Expr(tag_exists(Some(col("dt", "tag_id").eq(x)))),
            _ => Cond::False,
        },
        (Num(t, op), C::Int(v)) => int_lookup(num_column(t), op, v, range_of(t)),
        (NumIn(t), C::IntList(list)) => {
            let vals = in_values(&list, range_of(t));
            if vals.is_empty() { Cond::False } else { Cond::Expr(num_column(t).is_in(vals)) }
        }
        (Char(target, lookup), C::Str(s)) => Cond::Expr(match target {
            CT::Doc(c) => text_lookup(doc_col(c), lookup, &s),
            CT::Fk(table, fk) => {
                let sub = Query::select()
                    .column((Alias::new("fkn"), Alias::new("id")))
                    .from_as(Alias::new(table), Alias::new("fkn"))
                    .and_where(text_lookup(col("fkn", "name"), lookup, &s))
                    .to_owned();
                doc_col(fk).in_subquery(sub)
            }
            CT::TagsName => {
                let sub = Query::select()
                    .column((Alias::new("tn"), Alias::new("id")))
                    .from_as(Alias::new(TAG), Alias::new("tn"))
                    .and_where(text_lookup(col("tn", "name"), lookup, &s))
                    .to_owned();
                tag_exists(Some(col("dt", "tag_id").in_subquery(sub)))
            }
        }),
        (IsNull(c), C::Bool(b)) => Cond::Expr(if b { doc_col(c).is_null() } else { doc_col(c).is_not_null() }),
        (DatePart(c, is_dt, part), C::Int(v)) => match part {
            Part::Year => {
                let Some(year) = v.as_i64() else {
                    return Err(year_out_of_range(c));
                };
                if is_dt {
                    let Some((start, end)) = year_bounds_datetime(year, ctx.tz) else {
                        return Err(year_out_of_range(c));
                    };
                    Cond::Expr(doc_col(c).gte(start).and(doc_col(c).lt(end)))
                } else {
                    let Some((start, end)) = year_bounds(year, false) else {
                        return Err(year_out_of_range(c));
                    };
                    Cond::Expr(doc_col(c).between(Expr::val(start), Expr::val(end)))
                }
            }
            Part::Month | Part::Day => {
                let unit = if matches!(part, Part::Month) { "MONTH" } else { "DAY" };
                let e = if is_dt {
                    Expr::cust_with_exprs(format!("EXTRACT({unit} FROM $1 AT TIME ZONE $2)"), [doc_col(c), Expr::val(ctx.tz.name()).into()])
                } else {
                    Expr::cust_with_exprs(format!("EXTRACT({unit} FROM $1)"), [doc_col(c)])
                };
                int_lookup(e, Exact, v, (AUTO_MIN, AUTO_MAX))
            }
        },
        (DateCmp(c, op), C::Date(d)) => Cond::Expr(cmp(doc_col(c), op, Expr::val(d))),
        (DtDateCmp(c, op), C::Date(d)) => {
            // (col AT TIME ZONE tz)::date <op> d  ⇔  comparison with local start of day
            // (index-friendly, same result set)
            let next = d.succ_opt();
            let col_e = doc_col(c);
            match op {
                Gt => match next {
                    Some(n) => Cond::Expr(col_e.gte(start_of_local_day(n, ctx.tz))),
                    None => Cond::False,
                },
                Gte => Cond::Expr(col_e.gte(start_of_local_day(d, ctx.tz))),
                Lt => Cond::Expr(col_e.lt(start_of_local_day(d, ctx.tz))),
                Lte => match next {
                    Some(n) => Cond::Expr(col_e.lt(start_of_local_day(n, ctx.tz))),
                    None => Cond::Expr(col_e.is_not_null()),
                },
                Exact => return Err(FilterError::internal("unused lookup")),
            }
        }
        (DtCmp(c, op), C::Dt(dt)) => Cond::Expr(cmp(doc_col(c), op, Expr::val(dt))),
        (FolderId, C::Int(v)) => match v.range_check(i64::MIN, i64::MAX) {
            RangeCheck::InRange(x) => Cond::Expr(folder_exists(&f::FolderCond::In(vec![x]))),
            _ => Cond::False,
        },
        (FolderIn(tree), C::IntList(list)) => {
            let vals = in_values(&list, (i64::MIN, i64::MAX));
            if vals.is_empty() {
                Cond::False
            } else {
                Cond::Expr(folder_exists(&if tree { f::FolderCond::Tree(vals) } else { f::FolderCond::In(vals) }))
            }
        }
        (FolderIsNull, C::Bool(b)) => {
            let e = folder_exists(&f::FolderCond::Any);
            Cond::Expr(if b { e.not() } else { e })
        }
        (HasDuplicates, C::Bool(b)) => {
            let e = duplicates_exists(user);
            // true: first candidates (superset, see duplicate_candidates), then the exact
            // condition. false stays the Django form `NOT EXISTS(...)`: negating the
            // candidate list would have to check the whole array per row (expensive with many duplicates).
            Cond::Expr(if b { duplicate_candidates().and(e) } else { e.not() })
        }
        (IsTagged, C::Bool(b)) => {
            let e = tag_exists(None);
            Cond::Expr(if b { e } else { e.not() })
        }
        (HasCustomFields, C::Bool(b)) => {
            let e = cfi_exists(None);
            Cond::Expr(if b { e } else { e.not() })
        }
        (ObjAll(rel), C::Raw(raw)) => match object_ids(&raw) {
            None => Cond::True,
            Some(ids) => {
                let mut acc = Cond::True;
                for id in ids {
                    acc = acc.and(match id.range_check(AUTO_MIN, AUTO_MAX) {
                        RangeCheck::InRange(x) => Cond::Expr(obj_exact(rel, x)),
                        _ => Cond::False,
                    });
                }
                acc
            }
        },
        (ObjNone(rel), C::Raw(raw)) => match object_ids(&raw) {
            None => Cond::True,
            Some(ids) => {
                let mut acc = Cond::True;
                for id in ids {
                    if let RangeCheck::InRange(x) = id.range_check(AUTO_MIN, AUTO_MAX) {
                        acc = acc.and(Cond::Expr(obj_exclude(rel, x)));
                    }
                }
                acc
            }
        },
        (ObjIn(rel), C::Raw(raw)) => match object_ids(&raw) {
            None => Cond::True,
            Some(ids) => {
                let vals: Vec<i64> = ids
                    .into_iter()
                    .filter_map(|i| match i.range_check(AUTO_MIN, AUTO_MAX) {
                        RangeCheck::InRange(x) => Some(x),
                        _ => None,
                    })
                    .collect();
                if vals.is_empty() {
                    Cond::False
                } else {
                    Cond::Expr(match rel {
                        ObjRel::Tags => tag_exists(Some(col("dt", "tag_id").is_in(vals))),
                        ObjRel::CustomFields => cfi_exists(Some(col("ci", "field_id").is_in(vals))),
                        ObjRel::Fk(c) => doc_col(c).is_in(vals),
                    })
                }
            }
        },
        (Inbox, C::Raw(raw)) => {
            let inbox_tags = Query::select()
                .column((Alias::new("it"), Alias::new("id")))
                .from_as(Alias::new(TAG), Alias::new("it"))
                .and_where(col("it", "is_inbox_tag").eq(true))
                .to_owned();
            let e = tag_exists(Some(col("dt", "tag_id").in_subquery(inbox_tags)));
            // PaperRust: also "1"/"0" (the UI sends booleans as 1/0; upstream
            // silently ignores them and then returns all documents – Known difference).
            match raw.as_str() {
                "true" | "1" => Cond::Expr(e),
                "false" | "0" => Cond::Expr(e.not()),
                _ => Cond::True,
            }
        }
        (TitleContent, C::Raw(raw)) => {
            let v = py_strip(&raw);
            if v.is_empty() {
                Cond::True
            } else {
                tracing_warn("Deprecated document filter parameter 'title_content' used; use `text` instead.");
                // fast, result-identical form with calibrated DB (crate::textfold)
                Cond::Expr(folded_lookup(fold, doc_col("title"), IContains, v).or(folded_lookup(fold, effective_content(), IContains, v)))
            }
        }
        (Content(lookup), C::Raw(raw)) => {
            let v = py_strip(&raw);
            if v.is_empty() { Cond::True } else { Cond::Expr(folded_lookup(fold, effective_content(), lookup, v)) }
        }
        (CustomFieldsIContains, C::Raw(raw)) => {
            let v = py_strip(&raw);
            if v.is_empty() {
                Cond::True
            } else {
                tracing_warn(
                    "Deprecated document filter parameter 'custom_fields__icontains' used; use `custom_field_query` or advanced Tantivy field syntax instead.",
                );
                custom_fields_icontains(v, db).await?
            }
        }
        (CustomFieldQuery, C::Raw(raw)) => custom_field_query_cond(&raw, "custom_field_query", ctx, db).await?,
        (SharedBy, C::Raw(raw)) => {
            // owner_id=value: Django's int(value); ValueError → 500
            let Some(v) = py_int(&raw) else {
                return Err(FilterError::internal("ValueError: Field 'id' expected a number"));
            };
            match v.range_check(AUTO_MIN, AUTO_MAX) {
                RangeCheck::InRange(x) => Cond::Expr(doc_col("owner_id").eq(x).and(document_is_shared())),
                _ => Cond::False,
            }
        }
        (MimeType, C::Raw(raw)) => {
            let v = py_strip(&raw);
            if v.is_empty() { Cond::True } else { Cond::Expr(text_lookup(doc_col("mime_type"), IContains, v)) }
        }
        (spec, value) => {
            return Err(FilterError::internal(format!("filter/value mismatch: {spec:?} / {value:?}")));
        }
    })
}

/// Upstream logs deprecation warnings; here, without a tracing dependency, a stderr-free
/// no-op so that no user input ends up in logs.
fn tracing_warn(_msg: &str) {}

/// `<col>__year` outside 1..9999. upstream: `ValueError: year out of range` → 500; PaperRust:
/// 400 with field message (B-305/D-039, Known differences).
fn year_out_of_range(c: &str) -> FilterError {
    FilterError::field(&format!("{c}__year"), "Ensure this value is between 1 and 9999.")
}

/// Applies the complete `DocumentFilterSet` to `q`.
/// `DocumentFilterSet(data=<dict>).qs` (selection filters of bulk operations,
/// `DocumentSelectionMixin._resolve_document_ids`): unlike the DRF backend an invalid field is
/// not a 400 – django-filter drops it from `cleaned_data` and the remaining filters apply.
/// Errors raised while filtering (e.g. `custom_field_query`) stay errors.
pub async fn apply_filterset_lenient<D: FilterDb>(
    q: &mut SelectStatement,
    params: &crate::QueryParams,
    user: &User,
    ctx: &FilterContext,
    db: &D,
) -> Result<(), FilterError> {
    let defs = filters();
    let cleaned: Vec<_> = defs.iter().filter_map(|(name, spec)| clean(*spec, params.get(name), ctx).ok().map(|c| (*spec, c))).collect();
    let needs_fold = cleaned.iter().any(|(spec, value)| matches!(spec, TitleContent | Content(_)) && !matches!(value, Cleaned::Skip));
    let fold = if needs_fold { db.upper_fold().await } else { None };
    for (spec, value) in cleaned {
        if matches!(value, Cleaned::Skip) {
            continue;
        }
        filter_cond(spec, value, user, ctx, db, fold.as_deref()).await?.apply_to(q);
    }
    Ok(())
}

pub async fn apply_filterset<D: FilterDb>(
    q: &mut SelectStatement,
    params: &crate::QueryParams,
    user: &User,
    ctx: &FilterContext,
    db: &D,
) -> Result<(), FilterError> {
    let defs = filters();
    let mut errors = Map::new();
    let mut cleaned = Vec::with_capacity(defs.len());
    for (name, spec) in &defs {
        match clean(*spec, params.get(name), ctx) {
            Ok(c) => cleaned.push((*spec, c)),
            Err(msg) => {
                errors.insert((*name).to_string(), Json::Array(vec![Json::String(msg)]));
            }
        }
    }
    if !errors.is_empty() {
        return Err(FilterError::Validation(Json::Object(errors)));
    }
    // only calibrate UPPER folding if a content filter is active
    let needs_fold = cleaned.iter().any(|(spec, value)| matches!(spec, TitleContent | Content(_)) && !matches!(value, Cleaned::Skip));
    let fold = if needs_fold { db.upper_fold().await } else { None };
    for (spec, value) in cleaned {
        if matches!(value, Cleaned::Skip) {
            continue;
        }
        filter_cond(spec, value, user, ctx, db, fold.as_deref()).await?.apply_to(q);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------
// L1: DocumentFilterSet → AST (pl_domain::filter). Same form validation (`clean`), same
// runtime errors in the same order as `filter_cond`; only the result is a node.
// ---------------------------------------------------------------------------------------

use pl_domain::filter as f;

fn ast_num_col(t: NumTarget) -> f::NumCol {
    match t {
        NT::Id | NT::TagsId => f::NumCol::Id,
        NT::Asn => f::NumCol::Asn,
        NT::CorrespondentId => f::NumCol::Correspondent,
        NT::DocumentTypeId => f::NumCol::DocumentType,
        NT::StoragePathId => f::NumCol::StoragePath,
        NT::OwnerId => f::NumCol::Owner,
    }
}

fn ast_fk_col(c: &str) -> Result<f::NumCol, FilterError> {
    Ok(match c {
        "archive_serial_number" => f::NumCol::Asn,
        "correspondent_id" => f::NumCol::Correspondent,
        "document_type_id" => f::NumCol::DocumentType,
        "storage_path_id" => f::NumCol::StoragePath,
        "owner_id" => f::NumCol::Owner,
        "id" => f::NumCol::Id,
        other => return Err(FilterError::internal(format!("unknown column {other}"))),
    })
}

fn ast_cmp(op: Cmp) -> f::CmpOp {
    match op {
        Exact => f::CmpOp::Eq,
        Gt => f::CmpOp::Gt,
        Gte => f::CmpOp::Gte,
        Lt => f::CmpOp::Lt,
        Lte => f::CmpOp::Lte,
    }
}

fn ast_ts_col(c: &str) -> Result<f::TsCol, FilterError> {
    match c {
        "added" => Ok(f::TsCol::Added),
        "modified" => Ok(f::TsCol::Modified),
        other => Err(FilterError::internal(format!("unknown timestamp column {other}"))),
    }
}

fn ast_text_col(c: &str) -> Result<f::TextCol, FilterError> {
    Ok(match c {
        "title" => f::TextCol::Title,
        "original_filename" => f::TextCol::OriginalFilename,
        "checksum" => f::TextCol::Checksum,
        "mime_type" => f::TextCol::MimeType,
        other => return Err(FilterError::internal(format!("unknown text column {other}"))),
    })
}

fn ast_rel(table: &str) -> Result<f::NamedRel, FilterError> {
    Ok(match table {
        CORRESPONDENT => f::NamedRel::Correspondent,
        DOCTYPE => f::NamedRel::DocumentType,
        STORAGEPATH => f::NamedRel::StoragePath,
        other => return Err(FilterError::internal(format!("unknown relation {other}"))),
    })
}

/// [`int_lookup`] as AST (Django's `IntegerFieldOverflow`).
fn int_lookup_pred(col: f::NumCol, op: Cmp, v: PyInt, (min, max): (i64, i64)) -> f::Pred {
    match (op, v.range_check(min, max)) {
        (_, RangeCheck::InRange(x)) => f::Pred::Num { col, op: ast_cmp(op), value: x },
        (Exact, _) => f::Pred::False,
        (Gt | Gte, RangeCheck::Underflow) => f::Pred::True,
        (Gt | Gte, _) => f::Pred::False,
        (Lt | Lte, RangeCheck::Overflow) => f::Pred::True,
        (Lt | Lte, _) => f::Pred::False,
    }
}

fn obj_exact_pred(rel: ObjRel, id: i64) -> Result<f::Pred, FilterError> {
    Ok(match rel {
        ObjRel::Tags => f::Pred::Tag(f::TagCond::Is(id)),
        ObjRel::CustomFields => f::Pred::Instance(f::InstanceCond::Field(id)),
        ObjRel::Fk(c) => f::Pred::Num { col: ast_fk_col(c)?, op: f::CmpOp::Eq, value: id },
    })
}

fn obj_exclude_pred(rel: ObjRel, id: i64) -> Result<f::Pred, FilterError> {
    Ok(match rel {
        ObjRel::Fk(c) => f::Pred::FkNot { col: ast_fk_col(c)?, id },
        other => obj_exact_pred(other, id)?.negate(),
    })
}

fn ast_text(l: TextLookup) -> f::TextLookup {
    crate::ast::text_lookup_ast(l)
}

async fn filter_pred<D: FilterDb>(spec: Spec, value: Cleaned, ctx: &FilterContext, db: &D) -> Result<f::Pred, FilterError> {
    use Cleaned as C;
    use f::Pred as P;
    Ok(match (spec, value) {
        (_, C::Skip) => P::True,
        (Num(NT::TagsId, _), C::Int(v)) => match v.range_check(AUTO_MIN, AUTO_MAX) {
            RangeCheck::InRange(x) => P::Tag(f::TagCond::Is(x)),
            _ => P::False,
        },
        (Num(t, op), C::Int(v)) => int_lookup_pred(ast_num_col(t), op, v, range_of(t)),
        (NumIn(t), C::IntList(list)) => {
            let vals = in_values(&list, range_of(t));
            if vals.is_empty() { P::False } else { P::NumIn { col: ast_num_col(t), values: vals } }
        }
        (Char(target, lookup), C::Str(s)) => match target {
            CT::Doc(c) => P::Text { col: ast_text_col(c)?, lookup: ast_text(lookup), value: s },
            CT::Fk(table, _) => P::RelName { rel: ast_rel(table)?, lookup: ast_text(lookup), value: s },
            CT::TagsName => P::Tag(f::TagCond::Name(ast_text(lookup), s)),
        },
        (IsNull(c), C::Bool(b)) => P::IsNull { col: ast_fk_col(c)?, is_null: b },
        (DatePart(c, is_dt, part), C::Int(v)) => match part {
            Part::Year => {
                let Some(year) = v.as_i64() else {
                    return Err(year_out_of_range(c));
                };
                if is_dt {
                    let Some((start, end)) = year_bounds_datetime(year, ctx.tz) else {
                        return Err(year_out_of_range(c));
                    };
                    let col = ast_ts_col(c)?;
                    P::Ts { col, op: f::CmpOp::Gte, at: start }.and(P::Ts { col, op: f::CmpOp::Lt, at: end })
                } else {
                    let Some((start, end)) = year_bounds(year, false) else {
                        return Err(year_out_of_range(c));
                    };
                    P::CreatedBetween { from: start, to: end }
                }
            }
            Part::Month | Part::Day => {
                let dp = if matches!(part, Part::Month) { f::DatePart::Month } else { f::DatePart::Day };
                match v.range_check(AUTO_MIN, AUTO_MAX) {
                    RangeCheck::InRange(x) => {
                        if is_dt {
                            P::TsPart { col: ast_ts_col(c)?, part: dp, tz: ctx.tz.name().to_string(), op: f::CmpOp::Eq, value: x }
                        } else {
                            P::CreatedPart { part: dp, op: f::CmpOp::Eq, value: x }
                        }
                    }
                    _ => P::False,
                }
            }
        },
        (DateCmp(_, op), C::Date(d)) => P::Created { op: ast_cmp(op), date: d },
        (DtDateCmp(c, op), C::Date(d)) => {
            let col = ast_ts_col(c)?;
            let next = d.succ_opt();
            match op {
                Gt => match next {
                    Some(n) => P::Ts { col, op: f::CmpOp::Gte, at: start_of_local_day(n, ctx.tz) },
                    None => P::False,
                },
                Gte => P::Ts { col, op: f::CmpOp::Gte, at: start_of_local_day(d, ctx.tz) },
                Lt => P::Ts { col, op: f::CmpOp::Lt, at: start_of_local_day(d, ctx.tz) },
                Lte => match next {
                    Some(n) => P::Ts { col, op: f::CmpOp::Lt, at: start_of_local_day(n, ctx.tz) },
                    None => P::TsNotNull { col },
                },
                Exact => return Err(FilterError::internal("unused lookup")),
            }
        }
        (DtCmp(c, op), C::Dt(dt)) => P::Ts { col: ast_ts_col(c)?, op: ast_cmp(op), at: dt },
        (HasDuplicates, C::Bool(b)) => P::HasDuplicates(b),
        (FolderId, C::Int(v)) => match v.range_check(i64::MIN, i64::MAX) {
            RangeCheck::InRange(x) => P::Folder(f::FolderCond::In(vec![x])),
            _ => P::False,
        },
        (FolderIn(tree), C::IntList(list)) => {
            let vals = in_values(&list, (i64::MIN, i64::MAX));
            if vals.is_empty() {
                P::False
            } else if tree {
                P::Folder(f::FolderCond::Tree(vals))
            } else {
                P::Folder(f::FolderCond::In(vals))
            }
        }
        (FolderIsNull, C::Bool(b)) => {
            let e = P::Folder(f::FolderCond::Any);
            if b { e.negate() } else { e }
        }
        (IsTagged, C::Bool(b)) => {
            let e = P::Tag(f::TagCond::Any);
            if b { e } else { e.negate() }
        }
        (HasCustomFields, C::Bool(b)) => {
            let e = P::Instance(f::InstanceCond::Any);
            if b { e } else { e.negate() }
        }
        (ObjAll(rel), C::Raw(raw)) => match object_ids(&raw) {
            None => P::True,
            Some(ids) => {
                let mut acc = P::True;
                for id in ids {
                    acc = acc.and(match id.range_check(AUTO_MIN, AUTO_MAX) {
                        RangeCheck::InRange(x) => obj_exact_pred(rel, x)?,
                        _ => P::False,
                    });
                }
                acc
            }
        },
        (ObjNone(rel), C::Raw(raw)) => match object_ids(&raw) {
            None => P::True,
            Some(ids) => {
                let mut acc = P::True;
                for id in ids {
                    if let RangeCheck::InRange(x) = id.range_check(AUTO_MIN, AUTO_MAX) {
                        acc = acc.and(obj_exclude_pred(rel, x)?);
                    }
                }
                acc
            }
        },
        (ObjIn(rel), C::Raw(raw)) => match object_ids(&raw) {
            None => P::True,
            Some(ids) => {
                let vals: Vec<i64> = ids
                    .into_iter()
                    .filter_map(|i| match i.range_check(AUTO_MIN, AUTO_MAX) {
                        RangeCheck::InRange(x) => Some(x),
                        _ => None,
                    })
                    .collect();
                if vals.is_empty() {
                    P::False
                } else {
                    match rel {
                        ObjRel::Tags => P::Tag(f::TagCond::In(vals)),
                        ObjRel::CustomFields => P::Instance(f::InstanceCond::FieldIn(vals)),
                        ObjRel::Fk(c) => P::NumIn { col: ast_fk_col(c)?, values: vals },
                    }
                }
            }
        },
        (Inbox, C::Raw(raw)) => match raw.as_str() {
            "true" | "1" => P::Tag(f::TagCond::Inbox),
            "false" | "0" => P::Tag(f::TagCond::Inbox).negate(),
            _ => P::True,
        },
        (TitleContent, C::Raw(raw)) => {
            let v = py_strip(&raw);
            if v.is_empty() {
                P::True
            } else {
                P::Text { col: f::TextCol::Title, lookup: f::TextLookup::IContains, value: v.to_string() }.or(P::Text {
                    col: f::TextCol::Content,
                    lookup: f::TextLookup::IContains,
                    value: v.to_string(),
                })
            }
        }
        (Content(lookup), C::Raw(raw)) => {
            let v = py_strip(&raw);
            if v.is_empty() { P::True } else { P::Text { col: f::TextCol::Content, lookup: ast_text(lookup), value: v.to_string() } }
        }
        (CustomFieldsIContains, C::Raw(raw)) => {
            let v = py_strip(&raw);
            if v.is_empty() {
                P::True
            } else {
                let option_ids = icontains_option_ids(v, db).await?;
                P::Instance(f::InstanceCond::IContains { value: v.to_string(), select_option_ids: option_ids })
            }
        }
        (CustomFieldQuery, C::Raw(raw)) => crate::cfquery::custom_field_query_pred(&raw, "custom_field_query", ctx, db).await?,
        (SharedBy, C::Raw(raw)) => {
            let Some(v) = py_int(&raw) else {
                return Err(FilterError::internal("ValueError: Field 'id' expected a number"));
            };
            match v.range_check(AUTO_MIN, AUTO_MAX) {
                RangeCheck::InRange(x) => P::SharedBy(x),
                _ => P::False,
            }
        }
        (MimeType, C::Raw(raw)) => {
            let v = py_strip(&raw);
            if v.is_empty() {
                P::True
            } else {
                P::Text { col: f::TextCol::MimeType, lookup: f::TextLookup::IContains, value: v.to_string() }
            }
        }
        (spec, value) => {
            return Err(FilterError::internal(format!("filter/value mismatch: {spec:?} / {value:?}")));
        }
    })
}

/// `DocumentFilterSet` → [`f::Pred`] (L1). `lenient` = `FilterSet(data=dict).qs` of the
/// selection filters (invalid fields are dropped instead of 400), otherwise DRF backend (all
/// form errors → 400 in form order).
pub async fn parse_filterset<D: FilterDb>(
    params: &crate::QueryParams,
    ctx: &FilterContext,
    db: &D,
    lenient: bool,
) -> Result<f::Pred, FilterError> {
    let defs = filters();
    let mut errors = Map::new();
    let mut cleaned = Vec::with_capacity(defs.len());
    for (name, spec) in &defs {
        match clean(*spec, params.get(name), ctx) {
            Ok(c) => cleaned.push((*spec, c)),
            Err(msg) => {
                if !lenient {
                    errors.insert((*name).to_string(), Json::Array(vec![Json::String(msg)]));
                }
            }
        }
    }
    if !errors.is_empty() {
        return Err(FilterError::Validation(Json::Object(errors)));
    }
    let mut acc = f::Pred::True;
    for (spec, value) in cleaned {
        if matches!(value, Cleaned::Skip) {
            continue;
        }
        // like filter_cond: keep evaluating even after a constant FALSE (errors of later
        // filters, e.g. 500 from custom_field_query, stay visible)
        let p = filter_pred(spec, value, ctx, db).await?;
        acc = acc.and(p);
    }
    Ok(acc)
}
