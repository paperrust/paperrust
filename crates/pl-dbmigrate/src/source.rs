//! Source (read-only): SQLite file, MariaDB/MySQL, PostgreSQL.
//!
//! All reads run in **one** read-only transaction (consistent snapshot):
//! SQLite `BEGIN` + `PRAGMA query_only`, MySQL `START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY`,
//! PostgreSQL `BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY`. The transaction is never
//! committed, but rolled back at the end. Nothing is written.

use crate::schema::Column;
use crate::value::{Kind, Raw};
use anyhow::{Context, Result, anyhow, bail};
use futures::stream::{BoxStream, StreamExt};
use sqlx::mysql::{MySqlConnectOptions, MySqlConnection, MySqlRow};
use sqlx::postgres::{PgConnectOptions, PgConnection, PgRow};
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection, SqliteRow};
use sqlx::{Connection, Row, TypeInfo, ValueRef};
use std::collections::BTreeSet;
use std::str::FromStr;

pub enum Source {
    Sqlite(SqliteConnection),
    MySql(MySqlConnection),
    Pg(PgConnection),
}

/// Source URL without password (for report/log).
pub fn redact(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut u) if u.password().is_some() => {
            let _ = u.set_password(Some("***"));
            u.to_string()
        }
        _ => url.to_string(),
    }
}

pub fn sqlite_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("sqlite:")?;
    let rest = rest.strip_prefix("//").unwrap_or(rest);
    // sqlite:///C:/x → "/C:/x" → "C:/x"
    let rest = if rest.len() > 2 && rest.starts_with('/') && rest.as_bytes()[2] == b':' { &rest[1..] } else { rest };
    Some(rest.split('?').next().unwrap_or(rest).to_string())
}

impl Source {
    pub async fn connect(url: &str) -> Result<Source> {
        if let Some(path) = sqlite_path(url) {
            if !std::path::Path::new(&path).is_file() {
                bail!("SQLite-Datei nicht gefunden: {path}");
            }
            let opts = SqliteConnectOptions::new().filename(&path).read_only(true).create_if_missing(false).pragma("query_only", "1");
            let mut c = SqliteConnection::connect_with(&opts)
                .await
                .with_context(|| format!("SQLite-Datei {path} lässt sich nicht (nur lesend) öffnen"))?;
            sqlx::raw_sql("BEGIN").execute(&mut c).await?;
            // capture the snapshot immediately
            sqlx::raw_sql("SELECT count(*) FROM sqlite_master").execute(&mut c).await?;
            return Ok(Source::Sqlite(c));
        }
        if url.starts_with("mysql://") || url.starts_with("mariadb://") {
            let u = url.replacen("mariadb://", "mysql://", 1);
            let opts = MySqlConnectOptions::from_str(&u).context("MySQL-URL ungültig")?.charset("utf8mb4");
            let mut c = MySqlConnection::connect_with(&opts).await.context("Verbindung zu MySQL/MariaDB fehlgeschlagen")?;
            sqlx::raw_sql("SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ").execute(&mut c).await?;
            sqlx::raw_sql("SET time_zone = '+00:00'").execute(&mut c).await?;
            sqlx::raw_sql("START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY").execute(&mut c).await?;
            return Ok(Source::MySql(c));
        }
        if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            let opts = PgConnectOptions::from_str(url).context("PostgreSQL-URL ungültig")?;
            let mut c = PgConnection::connect_with(&opts).await.context("Verbindung zur PostgreSQL-Quelle fehlgeschlagen")?;
            sqlx::raw_sql(
                "SET TIME ZONE 'UTC'; SET bytea_output = 'hex'; SET extra_float_digits = 1; \
                 BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY",
            )
            .execute(&mut c)
            .await?;
            return Ok(Source::Pg(c));
        }
        bail!("unbekannte Quell-URL (erwartet sqlite:/pfad, mysql://, mariadb://, postgres://)")
    }

    pub fn dialect(&self) -> &'static str {
        match self {
            Source::Sqlite(_) => "sqlite",
            Source::MySql(_) => "mysql",
            Source::Pg(_) => "postgresql",
        }
    }

    pub async fn close(self) {
        match self {
            Source::Sqlite(mut c) => {
                let _ = sqlx::raw_sql("ROLLBACK").execute(&mut c).await;
                let _ = c.close().await;
            }
            Source::MySql(mut c) => {
                let _ = sqlx::raw_sql("ROLLBACK").execute(&mut c).await;
                let _ = c.close().await;
            }
            Source::Pg(mut c) => {
                let _ = sqlx::raw_sql("ROLLBACK").execute(&mut c).await;
                let _ = c.close().await;
            }
        }
    }

    fn q(&self, ident: &str) -> String {
        match self {
            Source::MySql(_) => format!("`{}`", ident.replace('`', "``")),
            _ => format!("\"{}\"", ident.replace('"', "\"\"")),
        }
    }

    /// Server/library version (for the report)
    pub async fn version(&mut self) -> Result<String> {
        Ok(match self {
            Source::Sqlite(c) => format!("SQLite {}", sqlx::query_scalar::<_, String>("SELECT sqlite_version()").fetch_one(c).await?),
            Source::MySql(c) => sqlx::query_scalar::<_, String>("SELECT VERSION()").fetch_one(c).await?,
            Source::Pg(c) => sqlx::query_scalar::<_, String>("SELECT version()").fetch_one(c).await?,
        })
    }

    pub async fn tables(&mut self) -> Result<BTreeSet<String>> {
        let v: Vec<String> =
            match self {
                Source::Sqlite(c) => {
                    sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'")
                        .fetch_all(c)
                        .await?
                }
                Source::MySql(c) => {
                    let rows: Vec<Vec<u8>> = sqlx::query_scalar(
                        "SELECT CAST(table_name AS CHAR) FROM information_schema.tables \
                     WHERE table_schema = DATABASE() AND table_type = 'BASE TABLE'",
                    )
                    .fetch_all(c)
                    .await?;
                    rows.into_iter().map(|b| String::from_utf8_lossy(&b).into_owned()).collect()
                }
                Source::Pg(c) => sqlx::query_scalar(
                    "SELECT table_name::text FROM information_schema.tables WHERE table_schema = 'public' AND table_type = 'BASE TABLE'",
                )
                .fetch_all(c)
                .await?,
            };
        Ok(v.into_iter().collect())
    }

    /// (column name, computed?)
    pub async fn columns(&mut self, table: &str) -> Result<Vec<(String, bool)>> {
        Ok(match self {
            Source::Sqlite(c) => {
                let sql = format!("PRAGMA table_xinfo(\"{}\")", table.replace('"', "\"\""));
                let rows = sqlx::query(&sql).fetch_all(c).await?;
                rows.iter()
                    .map(|r| {
                        let name: String = r.try_get("name").unwrap_or_default();
                        let hidden: i64 = r.try_get("hidden").unwrap_or(0);
                        (name, hidden == 2 || hidden == 3)
                    })
                    .collect()
            }
            Source::MySql(c) => {
                let rows = sqlx::query(
                    "SELECT CAST(column_name AS CHAR) AS n, CAST(extra AS CHAR) AS e FROM information_schema.columns \
                     WHERE table_schema = DATABASE() AND table_name = ? ORDER BY ordinal_position",
                )
                .bind(table)
                .fetch_all(c)
                .await?;
                rows.iter()
                    .map(|r| {
                        let n: Vec<u8> = r.try_get_unchecked(0).unwrap_or_default();
                        let e: Option<Vec<u8>> = r.try_get_unchecked(1).unwrap_or_default();
                        let e = String::from_utf8_lossy(&e.unwrap_or_default()).to_ascii_uppercase();
                        (String::from_utf8_lossy(&n).into_owned(), e.contains("GENERATED"))
                    })
                    .collect()
            }
            Source::Pg(c) => {
                let rows = sqlx::query(
                    "SELECT column_name::text, is_generated::text FROM information_schema.columns \
                     WHERE table_schema = 'public' AND table_name = $1 ORDER BY ordinal_position",
                )
                .bind(table)
                .fetch_all(c)
                .await?;
                rows.iter()
                    .map(|r| {
                        let n: String = r.try_get(0).unwrap_or_default();
                        let g: String = r.try_get(1).unwrap_or_default();
                        (n, g == "ALWAYS")
                    })
                    .collect()
            }
        })
    }

    pub async fn migrations(&mut self) -> Result<BTreeSet<(String, String)>> {
        let rows: Vec<(String, String)> = match self {
            Source::Sqlite(c) => sqlx::query_as("SELECT app, name FROM django_migrations").fetch_all(c).await?,
            Source::MySql(c) => {
                let r = sqlx::query("SELECT CAST(app AS CHAR), CAST(name AS CHAR) FROM django_migrations").fetch_all(c).await?;
                r.iter()
                    .map(|r| {
                        let a: Vec<u8> = r.try_get_unchecked(0).unwrap_or_default();
                        let n: Vec<u8> = r.try_get_unchecked(1).unwrap_or_default();
                        (String::from_utf8_lossy(&a).into_owned(), String::from_utf8_lossy(&n).into_owned())
                    })
                    .collect()
            }
            Source::Pg(c) => sqlx::query_as("SELECT app::text, name::text FROM django_migrations").fetch_all(c).await?,
        };
        Ok(rows.into_iter().collect())
    }

    pub async fn count(&mut self, table: &str) -> Result<i64> {
        let sql = format!("SELECT count(*) FROM {}", self.q(table));
        Ok(match self {
            Source::Sqlite(c) => sqlx::query_scalar::<_, i64>(&sql).fetch_one(c).await?,
            Source::MySql(c) => sqlx::query_scalar::<_, i64>(&sql).fetch_one(c).await?,
            Source::Pg(c) => sqlx::query_scalar::<_, i64>(&sql).fetch_one(c).await?,
        })
    }

    /// Approximate data size of the table in bytes (estimate only, for --dry-run)
    pub async fn approx_bytes(&mut self, table: &str) -> Option<i64> {
        match self {
            Source::Sqlite(c) => sqlx::query_scalar::<_, i64>("SELECT sum(pgsize) FROM dbstat WHERE name = ?")
                .bind(table)
                .fetch_one(c)
                .await
                .ok(),
            Source::MySql(c) => sqlx::query_scalar::<_, i64>(
                "SELECT CAST(data_length + index_length AS SIGNED) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name = ?",
            )
            .bind(table)
            .fetch_one(c)
            .await
            .ok(),
            Source::Pg(c) => sqlx::query_scalar::<_, i64>("SELECT pg_total_relation_size(('public.' || quote_ident($1))::regclass)")
                .bind(table)
                .fetch_one(c)
                .await
                .ok(),
        }
    }

    /// SQLite: `PRAGMA foreign_key_check` (read-only) – finds orphaned foreign keys up front.
    pub async fn foreign_key_violations(&mut self) -> Result<Vec<String>> {
        match self {
            Source::Sqlite(c) => {
                let rows = sqlx::query("PRAGMA foreign_key_check").fetch_all(c).await?;
                Ok(rows
                    .iter()
                    .take(200)
                    .map(|r| {
                        let t: String = r.try_get(0).unwrap_or_default();
                        let id: Option<i64> = r.try_get(1).unwrap_or(None);
                        let p: String = r.try_get(2).unwrap_or_default();
                        format!("{t} rowid={} → {p}", id.map(|i| i.to_string()).unwrap_or("?".into()))
                    })
                    .collect())
            }
            _ => Ok(vec![]),
        }
    }

    /// SELECT for the copy: delivers values so that `value::convert` can interpret them unambiguously.
    pub fn select_sql(&self, table: &str, cols: &[&Column]) -> String {
        let exprs: Vec<String> = cols
            .iter()
            .map(|c| {
                let q = self.q(&c.name);
                match self {
                    Source::Sqlite(_) => q,
                    Source::MySql(_) => match c.kind {
                        Kind::Float => q,
                        Kind::Bytea => format!("HEX({q})"),
                        Kind::Ts | Kind::TsNaive => format!("DATE_FORMAT({q}, '%Y-%m-%d %H:%i:%s.%f')"),
                        Kind::Date => format!("DATE_FORMAT({q}, '%Y-%m-%d')"),
                        Kind::Time => format!("TIME_FORMAT({q}, '%H:%i:%s.%f')"),
                        Kind::Int | Kind::Bool | Kind::Numeric => format!("CAST({q} AS CHAR)"),
                        _ => q,
                    },
                    Source::Pg(_) => format!("{q}::text"),
                }
            })
            .collect();
        format!("SELECT {} FROM {}", exprs.join(", "), self.q(table))
    }

    pub fn stream<'a>(&'a mut self, sql: &'a str, cols: &'a [&'a Column]) -> BoxStream<'a, Result<Vec<Raw>>> {
        match self {
            Source::Sqlite(c) => sqlx::query(sql).fetch(c).map(|r| r.map_err(anyhow::Error::from).and_then(sqlite_row)).boxed(),
            Source::MySql(c) => {
                sqlx::query(sql).fetch(c).map(move |r| r.map_err(anyhow::Error::from).and_then(|r| mysql_row(&r, cols))).boxed()
            }
            Source::Pg(c) => sqlx::query(sql).fetch(c).map(|r| r.map_err(anyhow::Error::from).and_then(|r| pg_row(&r))).boxed(),
        }
    }

    /// Expression as text (verification report queries, same representation in all dialects for
    /// integers/strings).
    pub fn text(&self, expr: &str) -> String {
        match self {
            Source::Sqlite(_) => format!("CAST({expr} AS TEXT)"),
            Source::MySql(_) => format!("CAST({expr} AS CHAR)"),
            Source::Pg(_) => format!("({expr})::text"),
        }
    }

    /// `SELECT count(*) FROM <table> [WHERE …]`
    pub async fn count_where(&mut self, table: &str, cond: Option<&str>) -> Result<i64> {
        let sql = format!("SELECT count(*) FROM {}{}", self.q(table), cond.map(|c| format!(" WHERE {c}")).unwrap_or_default());
        Ok(match self {
            Source::Sqlite(c) => sqlx::query_scalar::<_, i64>(&sql).fetch_one(c).await?,
            Source::MySql(c) => sqlx::query_scalar::<_, i64>(&sql).fetch_one(c).await?,
            Source::Pg(c) => sqlx::query_scalar::<_, i64>(&sql).fetch_one(c).await?,
        })
    }

    /// Every result row as text cells (NULL = `None`), streamed (select columns with [`Self::text`]
    /// or as strings).
    pub async fn for_each_text_row(&mut self, sql: &str, mut f: impl FnMut(Vec<Option<String>>)) -> Result<()> {
        fn text(v: Option<Vec<u8>>) -> Option<String> {
            v.map(|b| String::from_utf8(b).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
        }
        match self {
            Source::Sqlite(c) => {
                let mut s = sqlx::query(sql).fetch(c);
                while let Some(r) = s.next().await {
                    let r = r?;
                    f((0..r.len()).map(|i| r.try_get_unchecked::<Option<Vec<u8>>, _>(i).map(text)).collect::<Result<_, _>>()?);
                }
            }
            Source::MySql(c) => {
                let mut s = sqlx::query(sql).fetch(c);
                while let Some(r) = s.next().await {
                    let r = r?;
                    f((0..r.len()).map(|i| r.try_get_unchecked::<Option<Vec<u8>>, _>(i).map(text)).collect::<Result<_, _>>()?);
                }
            }
            Source::Pg(c) => {
                let mut s = sqlx::query(sql).fetch(c);
                while let Some(r) = s.next().await {
                    let r = r?;
                    f((0..r.len()).map(|i| r.try_get::<Option<String>, _>(i)).collect::<Result<_, _>>()?);
                }
            }
        }
        Ok(())
    }
}

fn sqlite_row(row: SqliteRow) -> Result<Vec<Raw>> {
    let n = row.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let v = row.try_get_raw(i)?;
        if v.is_null() {
            out.push(Raw::Null);
            continue;
        }
        let ty = v.type_info().name().to_string();
        out.push(match ty.as_str() {
            "INTEGER" | "BOOLEAN" => Raw::Int(row.try_get_unchecked::<i64, _>(i)?),
            "REAL" => Raw::Real(row.try_get_unchecked::<f64, _>(i)?),
            "BLOB" => Raw::Blob(row.try_get_unchecked::<Vec<u8>, _>(i)?),
            "TEXT" | "DATE" | "TIME" | "DATETIME" | "NUMERIC" => Raw::Text(row.try_get_unchecked::<Vec<u8>, _>(i)?),
            other => return Err(anyhow!("unerwarteter SQLite-Speichertyp {other}")),
        });
    }
    Ok(out)
}

fn mysql_row(row: &MySqlRow, cols: &[&Column]) -> Result<Vec<Raw>> {
    let mut out = Vec::with_capacity(cols.len());
    for (i, c) in cols.iter().enumerate() {
        let v = row.try_get_raw(i)?;
        if v.is_null() {
            out.push(Raw::Null);
            continue;
        }
        out.push(match c.kind {
            Kind::Float => Raw::Real(row.try_get_unchecked::<f64, _>(i)?),
            _ => Raw::Text(row.try_get_unchecked::<Vec<u8>, _>(i)?),
        });
    }
    Ok(out)
}

fn pg_row(row: &PgRow) -> Result<Vec<Raw>> {
    let n = row.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let v: Option<String> = row.try_get(i)?;
        out.push(match v {
            None => Raw::Null,
            Some(s) => Raw::Text(s.into_bytes()),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_urls() {
        assert_eq!(sqlite_path("sqlite:///C:/data/db.sqlite3").unwrap(), "C:/data/db.sqlite3");
        assert_eq!(sqlite_path("sqlite:///usr/src/paperless/data/db.sqlite3").unwrap(), "/usr/src/paperless/data/db.sqlite3");
        assert_eq!(sqlite_path("sqlite:data/db.sqlite3").unwrap(), "data/db.sqlite3");
        assert_eq!(sqlite_path("sqlite://C:/x.db?mode=ro").unwrap(), "C:/x.db");
        assert!(sqlite_path("postgres://x").is_none());
    }

    #[test]
    fn redacts_password() {
        assert_eq!(redact("postgres://u:geheim@h:5432/db"), "postgres://u:***@h:5432/db");
        assert_eq!(redact("sqlite:///x"), "sqlite:///x");
    }
}
