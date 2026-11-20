//! Database access shared by the handlers (Django tables, schema owned by upstream).
//!
//! upstream: rest_framework/authtoken/models.py (Token.generate_key, get_or_create),
//! allauth 65.20.1 mfa/models.py (Authenticator), mfa/base/internal/flows.py
//! (delete_and_cleanup, delete_dangling_recovery_codes), mfa/totp/internal/auth.py
//! (TOTP.activate), mfa/recovery_codes/internal/auth.py (RecoveryCodes.activate).

use rand::RngCore;
use sea_query::{Alias, Expr, Order, PostgresQueryBuilder, Query};
use pl_core::sqlbind::SqlxBinder as _;
use serde_json::Value;
use sqlx::{PgPool, Row};

pub fn a(name: &str) -> Alias {
    Alias::new(name)
}

/// sea-query builder error (only on column/value arity mismatch) as a sqlx error.
pub fn qerr(e: sea_query::error::Error) -> sqlx::Error {
    sqlx::Error::Protocol(format!("query build: {e}"))
}

/// `Token.generate_key()`: `binascii.hexlify(os.urandom(20)).decode()`.
pub fn new_token_key() -> String {
    let mut b = [0u8; 20];
    rand::rngs::OsRng.fill_bytes(&mut b);
    crate::totp::hex(&b)
}

/// `user.auth_token.key` or `None`.
pub async fn token_for_user(db: &PgPool, user_id: i64) -> Result<Option<String>, sqlx::Error> {
    let (sql, values) = Query::select()
        .column(a("key"))
        .from(a("authtoken_token"))
        .and_where(Expr::col(a("user_id")).eq(user_id))
        .build_sqlx(PostgresQueryBuilder);
    let row = sqlx::query_with(&sql, values).fetch_optional(db).await?;
    row.map(|r| r.try_get::<String, _>("key")).transpose()
}

async fn insert_token(db: &PgPool, user_id: i64) -> Result<(), sqlx::Error> {
    // Token.save(): key = generate_key(), created = now(). OneToOne on user_id.
    let (sql, values) = Query::insert()
        .into_table(a("authtoken_token"))
        .columns([a("key"), a("created"), a("user_id")])
        .values([new_token_key().into(), Expr::current_timestamp().into(), user_id.into()])
        .map_err(qerr)?
        .on_conflict(sea_query::OnConflict::column(a("user_id")).do_nothing().to_owned())
        .build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).execute(db).await?;
    Ok(())
}

/// `Token.objects.get_or_create(user=user)`.
pub async fn get_or_create_token(db: &PgPool, user_id: i64) -> Result<String, sqlx::Error> {
    if let Some(k) = token_for_user(db, user_id).await? {
        return Ok(k);
    }
    insert_token(db, user_id).await?;
    token_for_user(db, user_id).await?.ok_or(sqlx::Error::RowNotFound)
}

/// `GenerateAuthTokenView.post`: delete the existing token, create a new one.
pub async fn regenerate_token(db: &PgPool, user_id: i64) -> Result<String, sqlx::Error> {
    let (sql, values) = Query::delete()
        .from_table(a("authtoken_token"))
        .and_where(Expr::col(a("user_id")).eq(user_id))
        .build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).execute(db).await?;
    insert_token(db, user_id).await?;
    token_for_user(db, user_id).await?.ok_or(sqlx::Error::RowNotFound)
}

/// One `mfa_authenticator` row.
#[derive(Debug, Clone)]
pub struct Authenticator {
    pub id: i64,
    pub kind: String,
    pub data: Value,
}

/// `Authenticator.objects.filter(user=user[, type=kind])` ordered by pk (`.first()`).
pub async fn authenticators(db: &PgPool, user_id: i64, kind: Option<&str>) -> Result<Vec<Authenticator>, sqlx::Error> {
    let mut q = Query::select();
    q.expr_as(Expr::col(a("id")).cast_as(a("int8")), a("id"))
        .column(a("type"))
        .column(a("data"))
        .from(a("mfa_authenticator"))
        .and_where(Expr::col(a("user_id")).eq(user_id))
        .order_by(a("id"), Order::Asc);
    if let Some(k) = kind {
        q.and_where(Expr::col(a("type")).eq(k));
    }
    let (sql, values) = q.build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(db).await?;
    rows.iter()
        .map(|r| {
            Ok(Authenticator {
                id: r.try_get("id")?,
                kind: r.try_get("type")?,
                data: r.try_get::<sqlx::types::Json<Value>, _>("data")?.0,
            })
        })
        .collect()
}

/// `delete_and_cleanup(request, authenticator)`: delete it; if the user has no
/// non-recovery-code authenticator left, delete all remaining ones (dangling recovery codes).
pub async fn delete_and_cleanup(db: &PgPool, auth_id: i64, user_id: i64) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    let (sql, values) = Query::delete()
        .from_table(a("mfa_authenticator"))
        .and_where(Expr::col(a("id")).eq(auth_id))
        .build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).execute(&mut *tx).await?;
    let (sql, values) = Query::select()
        .expr(Expr::col(a("id")).count())
        .from(a("mfa_authenticator"))
        .and_where(Expr::col(a("user_id")).eq(user_id))
        .and_where(Expr::col(a("type")).ne("recovery_codes"))
        .build_sqlx(PostgresQueryBuilder);
    let others: i64 = sqlx::query_with(&sql, values).fetch_one(&mut *tx).await?.try_get(0)?;
    if others == 0 {
        let (sql, values) = Query::delete()
            .from_table(a("mfa_authenticator"))
            .and_where(Expr::col(a("user_id")).eq(user_id))
            .build_sqlx(PostgresQueryBuilder);
        sqlx::query_with(&sql, values).execute(&mut *tx).await?;
    }
    tx.commit().await
}

/// `Authenticator(user, type, data).save()` (created_at = now, last_used_at = NULL).
pub async fn insert_authenticator(db: &PgPool, user_id: i64, kind: &str, data: Value) -> Result<(), sqlx::Error> {
    let (sql, values) = Query::insert()
        .into_table(a("mfa_authenticator"))
        .columns([a("user_id"), a("type"), a("data"), a("created_at"), a("last_used_at")])
        .values([
            user_id.into(),
            kind.into(),
            Expr::val(data.to_string()).cast_as(a("jsonb")),
            Expr::current_timestamp().into(),
            Expr::val(Option::<String>::None).cast_as(a("timestamptz")),
        ])
        .map_err(qerr)?
        .build_sqlx(PostgresQueryBuilder);
    sqlx::query_with(&sql, values).execute(db).await?;
    Ok(())
}

/// `is_mfa_enabled` for many users at once.
pub async fn mfa_enabled_users(db: &PgPool, ids: &[i64]) -> Result<std::collections::HashSet<i64>, sqlx::Error> {
    if ids.is_empty() {
        return Ok(Default::default());
    }
    let (sql, values) = Query::select()
        .distinct()
        .expr_as(Expr::col(a("user_id")).cast_as(a("int8")), a("user_id"))
        .from(a("mfa_authenticator"))
        .and_where(Expr::col(a("user_id")).is_in(ids.iter().copied()))
        .build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(db).await?;
    rows.iter().map(|r| r.try_get::<i64, _>("user_id")).collect()
}

/// Is this a unique-violation (`IntegrityError`) from PostgreSQL?
pub fn is_unique_violation(e: &sqlx::Error) -> bool {
    matches!(e, sqlx::Error::Database(d) if d.code().as_deref() == Some("23505"))
}
