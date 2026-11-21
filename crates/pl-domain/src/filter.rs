//! Typed filter AST of the document list (ARCHITECTURE-V2 §5 **L1**).
//!
//! The AST is the shared, **resolved** form of all document queries:
//!
//! * v1 (`GET /api/documents/?…`, `all=true` selections): `pl_docfilter::ast` translates the
//!   django-filter parameters, `?search=`, `ordering` and the `custom_field_query` DSL into it
//!   (validation, error texts and Python/Django quirks stay in the parser).
//! * v2 (`POST /rs/api/v2/documents/query`): `pl-store` translates [`crate::query::Cond`]
//!   (UUIDs → `legacy_id`) into it; only with this do `any`/`not` over arbitrary conditions exist.
//! * SQL: `pl_store::docsql` builds **one** query (base, conditions,
//!   visibility, sorting) from [`DocFilter`]. This crate knows no table names.
//!
//! All values are already checked and resolved (IDs as `i64`, dates as `NaiveDate`, timestamps
//! in UTC, custom field definitions looked up). Constant partial results (Django's
//! `EmptyResultSet`/`FullResultSet`, e.g. on integer overflow) are entered as [`Pred::False`] /
//! [`Pred::True`]; [`Pred::and`]/[`Pred::or`] simplify them like Django.
//!
//! Contract and mapping table v1 parameter → node → SQL: docs/contract/pl-docfilter.md
//! (section "L1: DocQuery-AST").

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;

/// Comparison operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Gt,
    Gte,
    Lt,
    Lte,
}

/// Case-insensitive text comparison (Django `i*` lookups:
/// `UPPER(x::text) LIKE UPPER(pattern)` or `=` for `iexact`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextLookup {
    IContains,
    IStartsWith,
    IEndsWith,
    IExact,
}

/// Integer document columns (incl. foreign keys).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumCol {
    Id,
    Asn,
    Correspondent,
    DocumentType,
    StoragePath,
    Owner,
}

/// Text columns of the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextCol {
    Title,
    /// `effective_content`: content of the newest non-deleted version, otherwise its own.
    Content,
    OriginalFilename,
    Checksum,
    MimeType,
}

/// Foreign tables with `name` (name filter `<fk>__name__<lookup>` and `?search=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedRel {
    Correspondent,
    DocumentType,
    StoragePath,
}

/// Timestamp columns (`timestamptz`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsCol {
    Added,
    Modified,
}

/// Date part for `EXTRACT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatePart {
    Year,
    IsoYear,
    Quarter,
    Month,
    Week,
    Day,
    /// Django `week_day`: Sunday = 1 … Saturday = 7 (`DOW + 1`).
    WeekDay,
    /// ISO: Monday = 1 … Sunday = 7.
    IsoWeekDay,
}

/// Tag condition: `EXISTS (document-tag assignment that … satisfies)`. Nested tags are **not**
/// resolved, like upstream v3.4.2 (the assignment already contains the
/// parents when saving; filters only check the assignment table).
#[derive(Debug, Clone, PartialEq)]
pub enum TagCond {
    /// any tag (`is_tagged`)
    Any,
    Is(i64),
    In(Vec<i64>),
    /// Tag name (`tags__name__<lookup>`)
    Name(TextLookup, String),
    /// an inbox tag (`is_in_inbox`)
    Inbox,
}

/// Condition over any custom field instance of the document (incl. soft-deleted, like the
/// JOIN in upstream).
#[derive(Debug, Clone, PartialEq)]
pub enum InstanceCond {
    Any,
    Field(i64),
    FieldIn(Vec<i64>),
    /// legacy `custom_fields__icontains`: field name, values or matching select options.
    IContains {
        value: String,
        select_option_ids: Vec<String>,
    },
}

/// Value column of a custom field instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfColumn {
    Text,
    LongText,
    Bool,
    Int,
    Float,
    Date,
    Url,
    /// Raw text `"EUR12.50"`
    Monetary,
    /// numeric amount (generated column)
    MonetaryAmount,
    DocumentIds,
    Select,
}

/// Value in a custom field comparison (DB type follows the column).
#[derive(Debug, Clone, PartialEq)]
pub enum CfValue {
    Bool(bool),
    Int(i64),
    /// Integer outside `i64` (compared as `numeric` ±1e400, like Python `int`).
    HugeInt {
        negative: bool,
    },
    Float(f64),
    Text(String),
    Date(NaiveDate),
    /// Decimal number as SQL `numeric` literal (monetary amount).
    Numeric(String),
    /// JSON (document link list for `exact`/`in`).
    Json(Value),
}

/// Test on a date part (`EXTRACT(part FROM value_date)`).
#[derive(Debug, Clone, PartialEq)]
pub enum PartTest {
    IsNull(bool),
    Cmp(CmpOp, i64),
    In(Vec<i64>),
    Between(i64, i64),
}

/// Condition over the value column of a custom field instance.
#[derive(Debug, Clone, PartialEq)]
pub enum CfTest {
    Cmp(CmpOp, CfValue),
    In(Vec<CfValue>),
    IsNull(bool),
    Between(CfValue, CfValue),
    Text(TextLookup, String),
    /// Year/ISO year as date bounds (index-friendly): `value BETWEEN from AND to` for `Eq`,
    /// otherwise comparison against the matching bound (`gt` → `> to`, `gte` → `>= from` …).
    YearBounds(CmpOp, NaiveDate, NaiveDate),
    Part(DatePart, PartTest),
}

/// Test over a specific value column.
#[derive(Debug, Clone, PartialEq)]
pub struct CfCond {
    pub column: CfColumn,
    pub test: CfTest,
}

/// Sorting (v1: `ordering`; v2: `sort`).
#[derive(Debug, Clone, PartialEq)]
pub enum Sort {
    /// `-created, -id` (`DocumentViewSet.get_queryset`)
    Default,
    /// DRF `OrderingFilter`: terms without tiebreaker, like upstream.
    Terms(Vec<(SortKey, bool)>),
    /// `ordering=[-]custom_field_<id>`: `has_field DESC, wert ASC|DESC`.
    CustomField { field_id: i64, value: CfSortValue, desc: bool },
    /// No sorting (counting, unordered sets).
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Id,
    Title,
    Created,
    Modified,
    Added,
    Asn,
    PageCount,
    /// by `owner_id` (User has no `Meta.ordering`)
    Owner,
    CorrespondentName,
    DocumentTypeName,
    StoragePathName,
    NumNotes,
}

/// Sort value of a custom field.
#[derive(Debug, Clone, PartialEq)]
pub enum CfSortValue {
    /// Value of the first non-deleted instance (by `created`).
    Column(CfColumn),
    /// Select: position of the option in the list sorted by label (`None` = option without
    /// `id`), sum over all instances; without value → number of options.
    SelectOptions(Vec<Option<String>>),
}

/// A predicate over a root document (outer row).
#[derive(Debug, Clone, PartialEq)]
pub enum Pred {
    True,
    False,
    And(Vec<Pred>),
    Or(Vec<Pred>),
    /// SQL `NOT` (Django `~Q`/`exclude` over NULL-free sub-conditions, e.g. `EXISTS`).
    Not(Box<Pred>),
    /// Set complement: `(p) IS NOT TRUE` – also rows for which `p` is NULL (v2 `not`).
    Complement(Box<Pred>),
    Num {
        col: NumCol,
        op: CmpOp,
        value: i64,
    },
    NumIn {
        col: NumCol,
        values: Vec<i64>,
    },
    IsNull {
        col: NumCol,
        is_null: bool,
    },
    /// `exclude(<fk>__id=x)` on a nullable FK: `NOT (fk = x AND fk IS NOT NULL)` (NULL stays).
    FkNot {
        col: NumCol,
        id: i64,
    },
    Text {
        col: TextCol,
        lookup: TextLookup,
        value: String,
    },
    /// Name of the referenced object (`fk IN (SELECT id FROM <rel> WHERE name …)`).
    RelName {
        rel: NamedRel,
        lookup: TextLookup,
        value: String,
    },
    /// `created` (DateField) against a date.
    Created {
        op: CmpOp,
        date: NaiveDate,
    },
    /// `created BETWEEN from AND to` (`created__year`).
    CreatedBetween {
        from: NaiveDate,
        to: NaiveDate,
    },
    /// `EXTRACT(part FROM created) <op> value` (integer overflow is already resolved).
    CreatedPart {
        part: DatePart,
        op: CmpOp,
        value: i64,
    },
    /// Timestamp comparison (UTC).
    Ts {
        col: TsCol,
        op: CmpOp,
        at: DateTime<Utc>,
    },
    TsNotNull {
        col: TsCol,
    },
    /// `EXTRACT(part FROM col AT TIME ZONE tz) <op> value` (Django `USE_TZ`: local time).
    TsPart {
        col: TsCol,
        part: DatePart,
        tz: String,
        op: CmpOp,
        value: i64,
    },
    Tag(TagCond),
    Instance(InstanceCond),
    /// `EXISTS (instance of field field_id [with test])`; `None` = field present.
    CustomField {
        field_id: i64,
        test: Option<CfCond>,
    },
    /// There is a duplicate visible to the user (same checksum) – visibility is applied by
    /// the SQL builder.
    HasDuplicates(bool),
    /// `owner = user_id` and any object share (`shared_by__id`).
    SharedBy(i64),
}

impl Pred {
    /// AND with Django simplification (`False` wins, `True` is dropped).
    pub fn and(self, other: Pred) -> Pred {
        match (self, other) {
            (Pred::False, _) | (_, Pred::False) => Pred::False,
            (Pred::True, o) | (o, Pred::True) => o,
            (Pred::And(mut a), Pred::And(b)) => {
                a.extend(b);
                Pred::And(a)
            }
            (Pred::And(mut a), o) => {
                a.push(o);
                Pred::And(a)
            }
            (a, b) => Pred::And(vec![a, b]),
        }
    }

    /// OR with simplification (`True` wins, `False` is dropped).
    pub fn or(self, other: Pred) -> Pred {
        match (self, other) {
            (Pred::True, _) | (_, Pred::True) => Pred::True,
            (Pred::False, o) | (o, Pred::False) => o,
            (Pred::Or(mut a), Pred::Or(b)) => {
                a.extend(b);
                Pred::Or(a)
            }
            (Pred::Or(mut a), o) => {
                a.push(o);
                Pred::Or(a)
            }
            (a, b) => Pred::Or(vec![a, b]),
        }
    }

    /// SQL `NOT` with simplification.
    pub fn negate(self) -> Pred {
        match self {
            Pred::True => Pred::False,
            Pred::False => Pred::True,
            // NOT NOT p stays: in SQL that is not p (NULL stays NULL)
            p => Pred::Not(Box::new(p)),
        }
    }

    /// Set complement with simplification.
    pub fn complement(self) -> Pred {
        match self {
            Pred::True => Pred::False,
            Pred::False => Pred::True,
            p => Pred::Complement(Box::new(p)),
        }
    }

    /// All sub-conditions AND-combined (empty list = `True`).
    pub fn all(items: impl IntoIterator<Item = Pred>) -> Pred {
        items.into_iter().fold(Pred::True, Pred::and)
    }

    /// All sub-conditions OR-combined (empty list = `False`).
    pub fn any(items: impl IntoIterator<Item = Pred>) -> Pred {
        items.into_iter().fold(Pred::False, Pred::or)
    }

    /// `true` if a substring search over the content occurs anywhere (full scan; then
    /// pl-documents fetches page and count in one pass).
    pub fn has_content_text(&self) -> bool {
        match self {
            Pred::Text { col: TextCol::Content, .. } => true,
            Pred::And(v) | Pred::Or(v) => v.iter().any(Pred::has_content_text),
            Pred::Not(p) | Pred::Complement(p) => p.has_content_text(),
            _ => false,
        }
    }
}

/// A complete document query (without visibility and pagination).
#[derive(Debug, Clone, PartialEq)]
pub struct DocFilter {
    pub pred: Pred,
    pub sort: Sort,
}

impl Default for DocFilter {
    fn default() -> Self {
        DocFilter { pred: Pred::True, sort: Sort::Default }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(v: i64) -> Pred {
        Pred::Num { col: NumCol::Id, op: CmpOp::Eq, value: v }
    }

    #[test]
    fn django_constant_folding() {
        assert_eq!(Pred::True.and(n(1)), n(1));
        assert_eq!(n(1).and(Pred::False), Pred::False);
        assert_eq!(Pred::False.or(n(1)), n(1));
        assert_eq!(n(1).or(Pred::True), Pred::True);
        assert_eq!(Pred::all([n(1), n(2), n(3)]), Pred::And(vec![n(1), n(2), n(3)]));
        assert_eq!(Pred::any(Vec::<Pred>::new()), Pred::False);
        assert_eq!(Pred::all(Vec::<Pred>::new()), Pred::True);
        assert_eq!(Pred::True.negate(), Pred::False);
        assert_eq!(n(1).negate(), Pred::Not(Box::new(n(1))));
        assert_eq!(n(1).complement(), Pred::Complement(Box::new(n(1))));
    }

    #[test]
    fn content_text_detection() {
        let t = Pred::Text { col: TextCol::Content, lookup: TextLookup::IContains, value: "x".into() };
        assert!(Pred::Or(vec![n(1), t.clone()]).has_content_text());
        assert!(t.complement().has_content_text());
        assert!(!n(1).has_content_text());
    }
}
