//! Small shared helpers of the mail views.

use serde_json::{Map, Value};
use sqlx::PgPool;
use sqlx::postgres::{PgArguments, Postgres};
use sqlx::query::QueryAs;

use crate::ctx::MailUser;
use crate::drf::{self, Errors, Got};
use crate::error::{ApiError, ApiResult};
use crate::perms::{self, Model, ValidSetPermissions};
use crate::pyval;

/// URL PK like `get_object_or_404(queryset, pk=<str>)`:
/// `Err(404 "Not found.")` on ValueError, `Ok(None)` for a number outside the range
/// (→ "No X matches the given query."), otherwise `Ok(Some(pk))`.
pub fn url_pk(raw: &str) -> ApiResult<Option<i32>> {
    match pyval::parse_py_int(raw) {
        None => Err(ApiError::not_found()),
        Some(i) => Ok(i32::try_from(i).ok()),
    }
}

/// `$first..$first+2` = user_id, codename, content type.
pub fn visible_where(user: &MailUser, alias: &str, model: Model, first: usize) -> (String, bool) {
    match perms::visible_sql(user, alias, first) {
        None => ("TRUE".into(), false),
        Some(s) if s == "FALSE" => (s, false),
        Some(s) => (s.replace("{table}", model.table), true),
    }
}

pub fn bind_vis<'q, O>(
    q: QueryAs<'q, Postgres, O, PgArguments>,
    user: &MailUser,
    needs: bool,
    model: Model,
    model_ct: i32,
) -> QueryAs<'q, Postgres, O, PgArguments> {
    if !needs {
        return q;
    }
    q.bind(user.id).bind(format!("view_{}", model.name)).bind(model_ct)
}

/// Field `set_permissions` (parser + `validate_set_permissions`).
pub async fn set_permissions_field(
    db: &PgPool,
    e: &mut Errors,
    data: &Map<String, Value>,
    partial: bool,
) -> Got<ValidSetPermissions> {
    match drf::empty_values::<()>(drf::raw(data, "set_permissions"), drf::OPTIONAL, partial, None) {
        Err(fe) => {
            e.add("set_permissions", fe);
            Got::Skip
        }
        Ok(Err(_)) => Got::Skip,
        Ok(Ok(v)) => match perms::parse_set_permissions(v) {
            Err(fe) => {
                e.add("set_permissions", fe);
                Got::Skip
            }
            Ok(sp) => match perms::validate_set_permissions(db, &sp).await {
                Ok(valid) => Got::Set(valid),
                Err(fe) => {
                    e.add("set_permissions", fe);
                    Got::Skip
                }
            },
        },
    }
}

/// Additional root certificates for tests only (`PL_MAIL_TEST_EXTRA_ROOT_DER_HEX`).
pub fn test_extra_roots() -> Vec<Vec<u8>> {
    #[cfg(test)]
    {
        if let Some(v) = TEST_ROOTS.get() {
            return v.clone();
        }
    }
    Vec::new()
}

#[cfg(test)]
pub static TEST_ROOTS: std::sync::OnceLock<Vec<Vec<u8>>> = std::sync::OnceLock::new();

/// Shorten log text without possible secrets (only the kind of error, no payload).
pub fn redact(msg: &str) -> String {
    msg.chars().take(200).collect()
}
