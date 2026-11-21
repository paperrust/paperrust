//! Fast text search (`pl_docfilter::textfold`) against the real database collation: for many
//! texts × search texts × lookups the folded form must yield exactly the same as Django's
//! `UPPER(x::text) LIKE UPPER(p)`. Read-only (expressions via `VALUES`, no table).
//!
//!   cargo test -p pl-docfilter -j 1 --test textfold_db -- --ignored --nocapture
//!
//! Environment: PL_REF_DB (default postgres://paperless:paperless@127.0.0.1:54329/paperless).

use pl_core::sqlbind::SqlxBinder;
use pl_docfilter::FilterDb;
use pl_docfilter::sql::{TextLookup, text_lookup};
use sea_query::{Alias, Expr, PostgresQueryBuilder, Query};
use sqlx::Row;

fn texts() -> Vec<String> {
    let specials = ["ß", "ı", "ŉ", "ſ", "ǰ", "ẖ", "ẗ", "ẘ", "ẙ", "ẚ", "ﬀ", "ﬁ", "ﬂ", "ﬃ", "ﬄ", "ﬅ", "ﬆ"];
    let mut v: Vec<String> = [
        "Rechnung 2024",
        "STRASSE",
        "Straße",
        "straſse",
        "ﬁle",
        "Rechnungß",
        "Aı",
        "ǰo",
        "ẖa",
        "Ärger über Gebühren",
        "xﬀy",
        "ﬅop",
        "ŉo",
        "i\u{0307}x",
        "ΐ",
        "ﬃx",
        "Fİle",
        "",
        "a_b%c",
        "back\\slash",
        "Mieter Miete rent current",
        "ǅ ǈ ǋ",
        "ꭰ",
        "Ω Ωmega",
        "ﬆ-ﬅ",
        "SSS",
        "ßß",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    for s in specials {
        v.push(format!("a{s}b"));
        v.push(s.to_string());
        v.push(format!("x {s}{s} y"));
    }
    v
}

fn patterns() -> Vec<&'static str> {
    vec![
        "rechnung", "strasse", "SS", "s", "file", "fi", "f", "ai", "i", "jo", "j", "ha", "h", "t", "w", "y", "a", "ab", "ffy", "stop",
        "st", "n", "no", "ix", "ffi", "rger", "_b%", "\\s", "rent", "mega", "", " ", "x s", "x ss", "sb", "ib", "nb", "lb", "tb",
    ]
}

#[tokio::test]
#[ignore = "braucht die Referenz-DB (reference/README.md)"]
async fn folded_equals_django_form() {
    let db = std::env::var("PL_REF_DB").unwrap_or_else(|_| "postgres://paperless:paperless@127.0.0.1:54329/paperless".into());
    let pool = sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(&db).await.expect("db");
    let fold = pool.upper_fold().await.expect("Kalibrierung aktiv (UTF-8, Standardkollation)");
    println!("Sonderzeichen der DB: {:?}", fold.specials());
    let lookups = [TextLookup::IContains, TextLookup::IStartsWith, TextLookup::IEndsWith, TextLookup::IExact];
    let mut checked = 0;
    for p in patterns() {
        for lk in lookups {
            // SELECT t, old, new FROM (VALUES ...) v(t)
            let mut q = Query::select();
            let t = || Expr::col((Alias::new("v"), Alias::new("t"))).into();
            q.expr_as(Expr::col((Alias::new("v"), Alias::new("t"))), Alias::new("t"))
                .expr_as(text_lookup(t(), lk, p), Alias::new("old"))
                .expr_as(fold.lookup_limited(t(), lk, p, usize::MAX), Alias::new("new"))
                .from_values(texts().into_iter().map(|s| sea_query::ValueTuple::One(s.into())), Alias::new("v"));
            let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
            let sql = sql.replace(r#"AS "v""#, r#"AS "v" ("t")"#);
            let rows = sqlx::query_with(&sql, values).fetch_all(&pool).await.expect(&sql);
            for r in rows {
                let text: String = r.get("t");
                let old: Option<bool> = r.get("old");
                let new: Option<bool> = r.get("new");
                assert_eq!(old, new, "{text:?} {lk:?} {p:?}\n{sql}");
                checked += 1;
            }
        }
    }
    println!("{checked} Vergleiche gleich");
}
