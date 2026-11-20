//! SQL building blocks (sea-query) for the document filters.
//!
//! The outer query must read `FROM "documents_document"` **without an alias**; all
//! correlated subqueries refer to `"documents_document"."<column>"` and
//! use their own aliases.

use sea_query::{
    Alias, Expr, ExprTrait, NullOrdering, Order, Query, SelectStatement, SimpleExpr,
};

/// Table name of the documents (outer query, without alias).
pub const DOC: &str = "documents_document";
pub const DOC_TAGS: &str = "documents_document_tags";
pub const TAG: &str = "documents_tag";
pub const CFI: &str = "documents_customfieldinstance";
pub const CF: &str = "documents_customfield";
pub const NOTE: &str = "documents_note";
pub const CORRESPONDENT: &str = "documents_correspondent";
pub const DOCTYPE: &str = "documents_documenttype";
pub const STORAGEPATH: &str = "documents_storagepath";

/// `"table"."column"`
pub fn col(table: &str, column: &str) -> SimpleExpr {
    Expr::col((Alias::new(table), Alias::new(column))).into()
}

/// `"documents_document"."column"`
pub fn doc_col(column: &str) -> SimpleExpr {
    col(DOC, column)
}

/// `SELECT 1 FROM <table> AS <alias>`
pub fn select_one_from(table: &str, alias: &str) -> SelectStatement {
    Query::select()
        .expr(Expr::val(1))
        .from_as(Alias::new(table), Alias::new(alias))
        .to_owned()
}

pub fn exists(sel: SelectStatement) -> SimpleExpr {
    Expr::exists(sel)
}

/// Condition with Django's special cases `EmptyResultSet` (FALSE) and `FullResultSet` (TRUE).
#[derive(Debug, Clone)]
pub enum Cond {
    True,
    False,
    Expr(SimpleExpr),
}

impl Cond {
    pub fn and(self, other: Cond) -> Cond {
        match (self, other) {
            (Cond::False, _) | (_, Cond::False) => Cond::False,
            (Cond::True, o) | (o, Cond::True) => o,
            (Cond::Expr(a), Cond::Expr(b)) => Cond::Expr(a.and(b)),
        }
    }

    pub fn or(self, other: Cond) -> Cond {
        match (self, other) {
            (Cond::True, _) | (_, Cond::True) => Cond::True,
            (Cond::False, o) | (o, Cond::False) => o,
            (Cond::Expr(a), Cond::Expr(b)) => Cond::Expr(a.or(b)),
        }
    }

    pub fn negate(self) -> Cond {
        match self {
            Cond::True => Cond::False,
            Cond::False => Cond::True,
            Cond::Expr(e) => Cond::Expr(e.not()),
        }
    }

    /// Attach to the query as a `WHERE` condition.
    pub fn apply_to(self, q: &mut SelectStatement) {
        match self {
            Cond::True => {}
            Cond::False => {
                q.and_where(Expr::cust("FALSE"));
            }
            Cond::Expr(e) => {
                q.and_where(e);
            }
        }
    }

    pub fn into_expr(self) -> SimpleExpr {
        match self {
            Cond::True => Expr::cust("TRUE"),
            Cond::False => Expr::cust("FALSE"),
            Cond::Expr(e) => e,
        }
    }
}

impl From<SimpleExpr> for Cond {
    fn from(e: SimpleExpr) -> Self {
        Cond::Expr(e)
    }
}

/// Django's `prep_for_like_query`.
pub fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextLookup {
    IContains,
    IStartsWith,
    IEndsWith,
    IExact,
}

impl TextLookup {
    pub fn from_name(s: &str) -> Option<TextLookup> {
        Some(match s {
            "icontains" => TextLookup::IContains,
            "istartswith" => TextLookup::IStartsWith,
            "iendswith" => TextLookup::IEndsWith,
            "iexact" => TextLookup::IExact,
            _ => return None,
        })
    }
}

/// Django (PostgreSQL) `__icontains` etc.: `UPPER(col::text) LIKE UPPER(%s)`.
pub fn text_lookup(lhs: SimpleExpr, lookup: TextLookup, value: &str) -> SimpleExpr {
    let (op, pattern) = match lookup {
        TextLookup::IContains => ("LIKE", format!("%{}%", like_escape(value))),
        TextLookup::IStartsWith => ("LIKE", format!("{}%", like_escape(value))),
        TextLookup::IEndsWith => ("LIKE", format!("%{}", like_escape(value))),
        TextLookup::IExact => ("=", value.to_string()),
    };
    Expr::cust_with_exprs(
        format!("UPPER(($1)::text) {op} UPPER($2)"),
        [lhs, Expr::val(pattern).into()],
    )
}

/// `effective_content` like `documents.versioning.annotate_effective_content`:
/// content of the newest (non-deleted) version, otherwise the document's own content.
pub fn effective_content() -> SimpleExpr {
    let sub = Query::select()
        .column((Alias::new("ec_v"), Alias::new("content")))
        .from_as(Alias::new(DOC), Alias::new("ec_v"))
        .and_where(col("ec_v", "root_document_id").eq(doc_col("id")))
        .and_where(col("ec_v", "deleted_at").is_null())
        .order_by_with_nulls(
            (Alias::new("ec_v"), Alias::new("version_index")),
            Order::Desc,
            NullOrdering::Last,
        )
        .order_by((Alias::new("ec_v"), Alias::new("id")), Order::Desc)
        .limit(1)
        .to_owned();
    Expr::cust_with_exprs(
        "COALESCE(($1), $2)",
        [SimpleExpr::SubQuery(None, Box::new(sub.into_sub_query_statement())), doc_col("content")],
    )
}

/// Scalar subquery as expression.
pub fn scalar(sel: SelectStatement) -> SimpleExpr {
    SimpleExpr::SubQuery(None, Box::new(sel.into_sub_query_statement()))
}
