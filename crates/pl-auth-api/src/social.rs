//! Social accounts of a user as shown by `ProfileSerializer.social_accounts`.
//!
//! upstream: src/paperless/serialisers.py@v3.4.2 (SocialAccountSerializer.get_name),
//! allauth 65.20.1 socialaccount/models.py (SocialAccount.get_provider_account),
//! socialaccount/adapter.py (_build_apps_from_settings, list_apps, get_app, get_provider),
//! socialaccount/providers/base/provider.py (ProviderAccount.to_str, get_brand),
//! socialaccount/providers/openid_connect/provider.py (_pick_data, provider name = app.name).
//!
//! Known difference (contract #8): providers with their own `to_str()` (GitHub, Google, …)
//! are rendered with the generic base implementation.

use crate::db::a;
use pl_core::Config;
use sea_query::{Expr, Order, PostgresQueryBuilder, Query};
use pl_core::sqlbind::SqlxBinder as _;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};

#[derive(Debug, Clone)]
struct App {
    provider: String,
    name: String,
    hidden: bool,
}

async fn apps_for(db: &PgPool, cfg: &Config, provider: &str) -> Result<Vec<App>, sqlx::Error> {
    let (sql, values) = Query::select()
        .columns([a("provider"), a("name"), a("settings")])
        .from(a("socialaccount_socialapp"))
        .cond_where(
            sea_query::Cond::any()
                .add(Expr::col(a("provider")).eq(provider))
                .add(Expr::col(a("provider_id")).eq(provider)),
        )
        .order_by(a("id"), Order::Asc)
        .build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(db).await?;
    // provider_to_apps groups by app.provider (insertion order of first appearance)
    let mut grouped: Vec<(String, Vec<App>)> = Vec::new();
    let mut push = |app: App| match grouped.iter_mut().find(|(p, _)| *p == app.provider) {
        Some((_, v)) => v.push(app),
        None => grouped.push((app.provider.clone(), vec![app])),
    };
    for r in &rows {
        let settings: Value = r
            .try_get::<sqlx::types::Json<Value>, _>("settings")
            .map(|j| j.0)
            .unwrap_or(Value::Null);
        push(App {
            provider: r.try_get("provider")?,
            name: r.try_get("name")?,
            hidden: settings.get("hidden").is_some_and(truthy),
        });
    }
    // settings.SOCIALACCOUNT_PROVIDERS (PAPERLESS_SOCIALACCOUNT_PROVIDERS)
    if let Ok(Some(Value::Object(providers))) = cfg.env.json("PAPERLESS_SOCIALACCOUNT_PROVIDERS") {
        for (p, pcfg) in providers {
            let configs: Vec<Value> = match pcfg.get("APPS") {
                Some(Value::Array(v)) => v.clone(),
                Some(Value::Null) | None => match pcfg.get("APP") {
                    Some(Value::Null) | None => continue,
                    Some(c) => vec![c.clone()],
                },
                Some(other) => vec![other.clone()],
            };
            for c in configs {
                let provider_id = c.get("provider_id").and_then(Value::as_str).unwrap_or("");
                if provider_id != provider && p != provider {
                    continue;
                }
                push(App {
                    provider: p.clone(),
                    name: c.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    hidden: c.get("settings").and_then(|s| s.get("hidden")).is_some_and(truthy),
                });
            }
        }
    }
    Ok(grouped.into_iter().flat_map(|(_, v)| v).collect())
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `ProviderAccount.to_str()` (base implementation) on the provider's user data.
pub fn base_to_str(user_data: Option<&Value>, brand_name: &str) -> String {
    const TBL: &[(Option<&str>, &[&str])] = &[
        (None, &["username", "userName", "user_name", "login", "handle"]),
        (None, &["email", "Email", "mail", "email_address"]),
        (None, &["name", "display_name", "displayName", "displayname", "Display_Name", "nickname"]),
        (None, &["full_name", "fullName"]),
        (Some("first_name"), &["first_name", "firstname", "firstName", "First_Name", "given_name", "givenName"]),
        (
            Some("last_name"),
            &["last_name", "lastname", "lastName", "Last_Name", "family_name", "familyName", "surname"],
        ),
    ];
    if let Some(Value::Object(data)) = user_data
        && !data.is_empty() {
            let mut first = String::new();
            let mut last = String::new();
            for (store_as, variants) in TBL {
                for key in *variants {
                    if let Some(Value::String(v)) = data.get(*key) {
                        let v = v.trim();
                        match store_as {
                            None if !v.is_empty() => return v.to_string(),
                            None => {}
                            Some("first_name") => first = v.to_string(),
                            Some(_) => last = v.to_string(),
                        }
                    }
                }
            }
            if !first.is_empty() || !last.is_empty() {
                return format!("{first} {last}").trim().to_string();
            }
        }
    brand_name.to_string()
}

/// OpenID Connect `_pick_data(extra_data)`.
fn oidc_pick(extra: &Value) -> &Value {
    if let Some(u) = extra.get("userinfo").filter(|x| truthy(x)) {
        return u;
    }
    if let Some(t) = extra.get("id_token").filter(|x| truthy(x)) {
        return t;
    }
    extra
}

/// `ProfileSerializer.social_accounts` for a user: `[{id, provider, name}]`.
/// `Err` = unhandled upstream exception (MultipleObjectsReturned) → 500.
pub async fn social_accounts(db: &PgPool, cfg: &Config, user_id: i64) -> Result<Result<Vec<Value>, String>, sqlx::Error> {
    let (sql, values) = Query::select()
        .expr_as(Expr::col(a("id")).cast_as(a("int8")), a("id"))
        .columns([a("provider"), a("extra_data")])
        .from(a("socialaccount_socialaccount"))
        .and_where(Expr::col(a("user_id")).eq(user_id))
        .order_by(a("id"), Order::Asc)
        .build_sqlx(PostgresQueryBuilder);
    let rows = sqlx::query_with(&sql, values).fetch_all(db).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let id: i64 = r.try_get("id")?;
        let provider: String = r.try_get("provider")?;
        let extra: Value = r
            .try_get::<sqlx::types::Json<Value>, _>("extra_data")
            .map(|j| j.0)
            .unwrap_or(Value::Null);
        let mut apps = apps_for(db, cfg, &provider).await?;
        if apps.len() > 1 {
            apps.retain(|a| !a.hidden);
            if apps.len() != 1 {
                return Ok(Err(format!("MultipleObjectsReturned for social app {provider:?}")));
            }
        }
        let name = match apps.first() {
            None => "Unknown App".to_string(),
            Some(app) if app.provider == "openid_connect" => base_to_str(Some(oidc_pick(&extra)), &app.name),
            Some(app) => {
                let brand = if app.name.is_empty() { app.provider.clone() } else { app.name.clone() };
                base_to_str(Some(&extra), &brand)
            }
        };
        out.push(json!({ "id": id, "provider": provider, "name": name }));
    }
    Ok(Ok(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_str_like_allauth() {
        assert_eq!(base_to_str(Some(&json!({"name": " Keycloak "})), "X"), "Keycloak");
        assert_eq!(base_to_str(Some(&json!({"email": "a@b.c", "name": "n"})), "X"), "a@b.c");
        assert_eq!(base_to_str(Some(&json!({"given_name": "A", "family_name": "B"})), "X"), "A B");
        assert_eq!(base_to_str(Some(&json!({})), "Brand"), "Brand");
        assert_eq!(base_to_str(None, "Brand"), "Brand");
        let extra = json!({"userinfo": {"preferred_username": "x", "email": "u@x.de"}, "id_token": {"name": "t"}});
        assert_eq!(base_to_str(Some(oidc_pick(&extra)), "B"), "u@x.de");
    }
}
