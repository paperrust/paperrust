//! Migration helper and export/import under `/rs/api/v2/settings` (docs/SETTINGS.md §2.7, §5, WP8).
//!
//! * `GET  /settings/origin` – values from environment/file/desktop app with classification
//!   (adoptable / adopted / must stay / ineffective / obsolete / unknown).
//! * `POST /settings/adopt` – `{"keys":[…]}` (values of the running environment) or
//!   `{"values":{NAME:value}}` (from "check file") adopt into the database.
//! * `GET  /settings/slim-env` – generated slim `docker-compose.env` (secrets as a note).
//! * `POST /settings/check-file` – check `paperless.conf`/`docker-compose.env`/Compose YAML
//!   (multipart `file` or text body), without changing the server.
//! * `POST /settings/export` – JSON/`.env`; with secrets only with password confirmation and
//!   passphrase (encrypted, `pl_settings::transfer`).
//! * `POST /settings/import` – `{"data":…, "passphrase":…, "dry_run":true}`.
//!
//! Superuser only.

use axum::body::Bytes;
use axum::extract::{FromRequest, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use pl_core::AppState;
use pl_domain::Principal;
use pl_settings::migration::{self, Verdict};
use pl_settings::store::{self, Action, ApplyOptions, Input};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::http::{Caller, V2Json};
use crate::settings::{Access, R, SettingsError, actor, confirm_password, handle, snapshot};

fn superuser(p: &Principal) -> R<()> {
    if Access::of(p).superuser {
        Ok(())
    } else {
        Err(SettingsError::forbidden())
    }
}

/// `GET /settings/origin`
pub async fn origin(State(state): State<AppState>, Caller(p): Caller) -> R<Response> {
    superuser(&p)?;
    let s = snapshot(&state, &handle(&state)).await?;
    let items = migration::origin(&s);
    let count = |v: Verdict| items.iter().filter(|i| i.verdict == v).count();
    let summary = json!({
        "adoptable": count(Verdict::Adoptable), "adopted": count(Verdict::Adopted),
        "must_stay": count(Verdict::MustStay), "no_effect": count(Verdict::NoEffect),
        "obsolete": count(Verdict::Obsolete), "unknown": count(Verdict::Unknown),
    });
    Ok(crate::http::json(
        &json!({"items": items, "summary": summary, "rev": s.rev}),
    ))
}

#[derive(Debug, Deserialize)]
pub struct AdoptBody {
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default)]
    pub values: Map<String, Value>,
}

/// `POST /settings/adopt`
pub async fn adopt(
    State(state): State<AppState>,
    Caller(p): Caller,
    V2Json(body): V2Json<AdoptBody>,
) -> R<Response> {
    superuser(&p)?;
    let h = handle(&state);
    let s = snapshot(&state, &h).await?;
    let mut changes: Vec<(String, Input)> = Vec::new();
    let mut skipped = Vec::new();
    let origin = migration::origin(&s);
    for k in &body.keys {
        let it = origin
            .iter()
            .find(|i| i.name == *k || i.key == pl_settings::schema::find(k).map(|d| d.key));
        match it {
            Some(i) if matches!(i.verdict, Verdict::Adoptable | Verdict::Adopted) => {
                changes.push((
                    i.key.unwrap_or_default().to_string(),
                    Input::Raw(i.raw.clone()),
                ));
            }
            Some(i) => skipped.push(json!({"name": k, "reason": i.reason, "verdict": i.verdict})),
            None => skipped.push(json!({"name": k, "reason": "not set in the environment"})),
        }
    }
    for (k, v) in &body.values {
        let raw = match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let i = migration::classify(k, &raw, "upload", Some(&s));
        if matches!(i.verdict, Verdict::Adoptable | Verdict::Adopted) {
            changes.push((i.key.unwrap_or_default().to_string(), Input::Raw(raw)));
        } else {
            skipped.push(json!({"name": k, "reason": i.reason, "verdict": i.verdict}));
        }
    }
    if changes.is_empty() {
        return Ok(crate::http::json(
            &json!({"adopted": [], "skipped": skipped, "rev": s.rev}),
        ));
    }
    // The migration may also adopt locked keys (they stay locked as long as the environment sets them).
    let opts = ApplyOptions {
        expected_rev: None,
        env: None,
        if_unset: false,
    };
    let rev = store::apply(
        state.db(),
        h.secret_box(),
        &changes,
        &opts,
        &actor(&p),
        Action::Adopt,
    )
    .await?;
    if let Err(e) = h.reload(state.db()).await {
        tracing::warn!(error = %e, "settings reload after adopt failed");
    }
    let adopted: Vec<&str> = changes.iter().map(|(k, _)| k.as_str()).collect();
    Ok(crate::http::json(
        &json!({"adopted": adopted, "skipped": skipped, "rev": rev}),
    ))
}

fn text(body: String, filename: &str) -> Response {
    let mut r = (StatusCode::OK, body).into_response();
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        r.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    r.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

/// `GET /settings/slim-env`
pub async fn slim_env(State(state): State<AppState>, Caller(p): Caller) -> R<Response> {
    superuser(&p)?;
    let s = snapshot(&state, &handle(&state)).await?;
    let items: Vec<_> = migration::origin(&s)
        .into_iter()
        .filter(|i| i.source != "desktop")
        .collect();
    Ok(text(
        migration::slim_env(&items, &[], true),
        "docker-compose.env",
    ))
}

/// `POST /settings/check-file` (multipart `file` or text body, ≤ 1 MiB)
pub async fn check_file(
    State(state): State<AppState>,
    Caller(p): Caller,
    req: Request,
) -> R<Response> {
    superuser(&p)?;
    let ct = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let content = if ct.starts_with("multipart/") {
        let mut mp = axum::extract::Multipart::from_request(req, &state)
            .await
            .map_err(|e| SettingsError::field("file", format!("Invalid upload: {e}")))?;
        let mut found = None;
        while let Some(f) = mp
            .next_field()
            .await
            .map_err(|e| SettingsError::field("file", format!("Invalid upload: {e}")))?
        {
            if f.name() == Some("file") {
                found =
                    Some(f.bytes().await.map_err(|e| {
                        SettingsError::field("file", format!("Invalid upload: {e}"))
                    })?);
            }
        }
        found.ok_or_else(|| SettingsError::field("file", "No file was submitted."))?
    } else {
        Bytes::from_request(req, &state)
            .await
            .map_err(|_| SettingsError::field("file", "Could not read the request body."))?
    };
    if content.len() > 1024 * 1024 {
        return Err(SettingsError::field("file", "File too large (1 MiB max)."));
    }
    let text_in = String::from_utf8_lossy(&content);
    let parsed = migration::parse_file(&text_in);
    let s = snapshot(&state, &handle(&state)).await?;
    let items = migration::check_pairs(&parsed.pairs, Some(&s));
    let others: Vec<(String, String)> = parsed
        .pairs
        .iter()
        .filter(|(k, _)| !migration::is_ours(k))
        .cloned()
        .collect();
    // The file comes from the admin themselves – the slim version may contain its values.
    let slim = migration::slim_env(&items, &others, false);
    Ok(crate::http::json(&json!({
        "kind": parsed.kind, "service": parsed.service, "env_files": parsed.env_files,
        "items": items, "slim_env": slim,
    })))
}

#[derive(Debug, Deserialize)]
pub struct ExportBody {
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub include_secrets: bool,
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub passphrase: Option<String>,
}

/// `POST /settings/export`
pub async fn export(
    State(state): State<AppState>,
    Caller(p): Caller,
    V2Json(body): V2Json<ExportBody>,
) -> R<Response> {
    superuser(&p)?;
    let h = handle(&state);
    let fmt = body.format.as_deref().unwrap_or("json");
    if body.include_secrets {
        confirm_password(&p, body.password.as_deref())?;
        if fmt != "json" {
            return Err(SettingsError::field(
                "format",
                "Secrets can only be exported as encrypted JSON.",
            ));
        }
    }
    match fmt {
        "json" => {
            let mut conn = state
                .db()
                .acquire()
                .await
                .map_err(SettingsError::internal)?;
            let doc = store::export(&mut conn, h.secret_box(), body.include_secrets)
                .await
                .map_err(SettingsError::internal)?;
            let out = if body.include_secrets {
                let pass = body
                    .passphrase
                    .as_deref()
                    .filter(|p| !p.is_empty())
                    .ok_or_else(|| {
                        SettingsError::field(
                            "passphrase",
                            "A passphrase is required to export secrets.",
                        )
                    })?;
                pl_settings::transfer::encrypt(&doc, pass)
                    .map_err(|e| SettingsError::field("passphrase", e))?
            } else {
                doc
            };
            let mut r = crate::http::json(&out);
            r.headers_mut().insert(
                header::CONTENT_DISPOSITION,
                HeaderValue::from_static("attachment; filename=\"paperrust-settings.json\""),
            );
            r.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            Ok(r)
        }
        "env" => {
            let s = snapshot(&state, &h).await?;
            Ok(text(store::export_env(&s, false), "paperrust-settings.env"))
        }
        other => Err(SettingsError::field(
            "format",
            format!("Unknown format {other} (json|env)."),
        )),
    }
}

#[derive(Debug, Deserialize)]
pub struct ImportBody {
    pub data: Value,
    #[serde(default)]
    pub passphrase: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

/// `POST /settings/import`
pub async fn import(
    State(state): State<AppState>,
    Caller(p): Caller,
    V2Json(body): V2Json<ImportBody>,
) -> R<Response> {
    superuser(&p)?;
    let h = handle(&state);
    let doc = if pl_settings::transfer::is_encrypted(&body.data) {
        let pass = body.passphrase.as_deref().ok_or_else(|| {
            SettingsError::field(
                "passphrase",
                "This file is encrypted – enter the passphrase.",
            )
        })?;
        pl_settings::transfer::decrypt(&body.data, pass)
            .map_err(|e| SettingsError::field("passphrase", e))?
    } else {
        body.data.clone()
    };
    let changes = store::import_changes(&doc).map_err(|e| SettingsError::field("data", e))?;
    let s = snapshot(&state, &h).await?;
    let env = h.env();
    let mut rows = Vec::new();
    let mut apply = Vec::new();
    for (k, input) in changes {
        let Some(def) = pl_settings::schema::find(&k) else {
            rows.push(json!({"key": k, "status": "unknown"}));
            continue;
        };
        let locked = def.ac_field.is_none()
            && def
                .env_names()
                .iter()
                .any(|n| env.origin_exact(n) == Some(pl_core::config::EnvOrigin::Process));
        let writable = def.ac_field.is_some() || pl_settings::layer::rule_n(def);
        let cur = s.get(def.key).and_then(|e| e.value.clone());
        let new = match &input {
            Input::Raw(r) => Some(r.clone()),
            Input::Json(v) => pl_settings::appconfig::to_env_string(v),
            Input::Reset => None,
        };
        let status = if !writable {
            "not_writable"
        } else if locked {
            "locked_by_environment"
        } else if cur == new {
            "unchanged"
        } else {
            "change"
        };
        let show = |v: &Option<String>| {
            if def.secret {
                v.as_ref().map(|_| "***".to_string())
            } else {
                v.clone()
            }
        };
        rows.push(
            json!({"key": def.key, "status": status, "current": show(&cur), "new": show(&new)}),
        );
        if status == "change" {
            apply.push((def.key.to_string(), input));
        }
    }
    if body.dry_run || apply.is_empty() {
        return Ok(crate::http::json(
            &json!({"dry_run": body.dry_run, "items": rows, "applied": 0, "rev": s.rev}),
        ));
    }
    let opts = ApplyOptions {
        expected_rev: None,
        env: Some(env),
        if_unset: false,
    };
    let rev = store::apply(
        state.db(),
        h.secret_box(),
        &apply,
        &opts,
        &actor(&p),
        Action::Import,
    )
    .await?;
    if let Err(e) = h.reload(state.db()).await {
        tracing::warn!(error = %e, "settings reload after import failed");
    }
    Ok(crate::http::json(
        &json!({"dry_run": false, "items": rows, "applied": apply.len(), "rev": rev}),
    ))
}
