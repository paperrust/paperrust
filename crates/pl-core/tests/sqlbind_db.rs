//! FINDINGS N4 (`tests/stress/BEFUNDE.md`): the same sea-query text, bound once with `i32`, once with
//! `i64`, on the **same** connection. Without a type signature the second call fails
//! (`incorrect binary data format in bind parameter 1`), with [`pl_core::sqlbind::SqlxBinder`] it does not.
//!
//! ```text
//! PAPERRUST_SQLBIND_TEST_SERVER=postgres://paperless:paperless@127.0.0.1:54329 \
//!   cargo test -p pl-core -j 2 --test sqlbind_db -- --ignored
//! ```
//! Creates the empty throwaway DB `pl_sqlbind_test` and drops it at the end (never the reference DBs).

use sea_query::{Alias, Expr, PostgresQueryBuilder, Query, SelectStatement};
use sqlx::{Connection, PgConnection};

const DB: &str = "pl_sqlbind_test";

fn stmt(ids: sea_query::Value) -> SelectStatement {
    Query::select()
        .column(Alias::new("id"))
        .column(Alias::new("name"))
        .from(Alias::new("t_corr"))
        .and_where(Expr::col(Alias::new("id")).is_in([ids]))
        .to_owned()
}

#[tokio::test]
#[ignore = "braucht PostgreSQL (PAPERRUST_SQLBIND_TEST_SERVER)"]
async fn same_text_int4_then_int8_on_one_connection() {
    let server = std::env::var("PAPERRUST_SQLBIND_TEST_SERVER").expect("PAPERRUST_SQLBIND_TEST_SERVER");
    let server = server.trim_end_matches('/');
    let mut admin = PgConnection::connect(&format!("{server}/postgres")).await.expect("connect");
    sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&mut admin).await.expect("drop");
    sqlx::query(&format!("CREATE DATABASE {DB}")).execute(&mut admin).await.expect("create");
    let mut c = PgConnection::connect(&format!("{server}/{DB}")).await.expect("connect test db");
    sqlx::query("CREATE TABLE t_corr (id integer PRIMARY KEY, name text NOT NULL)").execute(&mut c).await.expect("table");
    sqlx::query("INSERT INTO t_corr VALUES (1, 'a'), (2, 'b')").execute(&mut c).await.expect("rows");

    // 1) Raw sea_query_binder: error reproduced (evidence for the cause)
    {
        use sea_query_binder::SqlxBinder as Raw;
        let (s1, v1) = Raw::build_sqlx(&stmt(1i32.into()), PostgresQueryBuilder);
        let (s2, v2) = Raw::build_sqlx(&stmt(2i64.into()), PostgresQueryBuilder);
        assert_eq!(s1, s2);
        assert_eq!(sqlx::query_with(&s1, v1).fetch_all(&mut c).await.expect("int4").len(), 1);
        let err = sqlx::query_with(&s2, v2).fetch_all(&mut c).await.expect_err("int8 nach int4 muss scheitern");
        assert!(err.to_string().contains("incorrect binary data format"), "{err}");
    }
    // 2) pl_core::sqlbind: both orders, repeatedly
    {
        use pl_core::sqlbind::SqlxBinder;
        for _ in 0..3 {
            for v in [sea_query::Value::from(1i32), 2i64.into(), Some(2i16).into(), 1i64.into(), 2i32.into()] {
                let (s, vals) = stmt(v).build_sqlx(PostgresQueryBuilder);
                let rows = sqlx::query_with(&s, vals).fetch_all(&mut c).await.expect("getaggte Anweisung");
                assert_eq!(rows.len(), 1, "{s}");
            }
        }
    }
    c.close().await.ok();
    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {DB} WITH (FORCE)")).execute(&mut admin).await;
}
