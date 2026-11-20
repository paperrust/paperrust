//! Type-safe binding of dynamically built sea-query statements (FINDINGS B3/N4, `tests/stress/BEFUNDE.md`).
//!
//! sqlx remembers prepared statements per connection **only by SQL text** (sqlx-postgres
//! `get_or_prepare`, even with `persistent(false)`). If two paths build the same text with
//! different Rust types (`i32` → `int4`, `i64` → `int8`, …), the second fails on a
//! connection that the first has already used: `incorrect binary data format in bind parameter 1`
//! or `insufficient data left in message`. Typical for short sea-query queries such as
//! `SELECT "id", "name" FROM "documents_correspondent" WHERE "id" IN ($1)`, which many crates build.
//!
//! [`SqlxBinder::build_sqlx`] replaces `sea_query_binder::SqlxBinder::build_sqlx` and appends a
//! **type signature of the bound values** as an SQL comment (`/*t:…*/`, hash over the
//! kinds of values, no values). The same text with different binding types thus yields a different
//! cache key; the same types continue to share one prepared statement (no
//! loss of performance, no need to disable the statement cache).
//!
//! Rule: in domain crates `use pl_core::sqlbind::SqlxBinder` instead of `sea_query_binder::SqlxBinder`
//! (guard: `crates/pl-core/tests/sqlbind_guard.rs`).

use std::hash::{DefaultHasher, Hash, Hasher};

use sea_query::{QueryBuilder, Value};
use sea_query_binder::SqlxValues;

/// Like `sea_query_binder::SqlxBinder`, but with a type signature in the SQL text (module docs).
pub trait SqlxBinder {
    fn build_sqlx<T: QueryBuilder>(&self, query_builder: T) -> (String, SqlxValues);
}

impl<S: sea_query_binder::SqlxBinder> SqlxBinder for S {
    fn build_sqlx<T: QueryBuilder>(&self, query_builder: T) -> (String, SqlxValues) {
        let (mut sql, values) = sea_query_binder::SqlxBinder::build_sqlx(self, query_builder);
        tag_types(&mut sql, &values.0 .0);
        (sql, values)
    }
}

/// Appends ` /*t:<hash>*/` if values are bound (hash over the kinds of values, without contents).
pub fn tag_types(sql: &mut String, values: &[Value]) {
    if values.is_empty() {
        return;
    }
    // DefaultHasher::new() has fixed keys: deterministic within the same program (the per-connection cache
    // needs nothing more).
    let mut h = DefaultHasher::new();
    values.len().hash(&mut h);
    for v in values {
        std::mem::discriminant(v).hash(&mut h);
    }
    use std::fmt::Write as _;
    let _ = write!(sql, " /*t:{:016x}*/", h.finish());
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_query::{Alias, Expr, PostgresQueryBuilder, Query};

    fn q(v: Value) -> String {
        Query::select()
            .column(Alias::new("id"))
            .from(Alias::new("t"))
            .and_where(Expr::col(Alias::new("id")).is_in([v]))
            .build_sqlx(PostgresQueryBuilder)
            .0
    }

    #[test]
    fn types_change_text_values_do_not() {
        let a = q(Value::Int(Some(1)));
        let b = q(Value::Int(Some(2)));
        let c = q(Value::BigInt(Some(1)));
        let d = q(Value::Int(None));
        assert_eq!(a, b);
        assert_eq!(a, d);
        assert_ne!(a, c);
        assert!(a.starts_with(r#"SELECT "id" FROM "t" WHERE "id" IN ($1) /*t:"#), "{a}");
    }

    #[test]
    fn no_values_no_tag() {
        let (sql, _) = Query::select().column(Alias::new("id")).from(Alias::new("t")).build_sqlx(PostgresQueryBuilder);
        assert_eq!(sql, r#"SELECT "id" FROM "t""#);
    }
}
