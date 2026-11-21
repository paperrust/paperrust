//! Validating and converting values (docs/SETTINGS.md §2.4, E7).
//!
//! What is stored is the **canonical env string**; validation uses **the same parsers** as at
//! startup: the value is put into a single-key [`Env`] and read via `Env::bool/int/float/json/list`,
//! crontabs via `pl_queue::cron::Crontab::from_paperless`, time zones via `chrono_tz`.
//! The API takes/returns typed JSON ([`to_json`], [`from_json`]).

use pl_core::config::Env;
use serde_json::Value;

use crate::schema::{JsonShape, SettingDef, Ty};

/// Validation error (for 422 `fields` or CLI message).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{key}: {message}")]
pub struct ValueError {
    pub key: String,
    pub message: String,
}

fn err(def: &SettingDef, message: impl Into<String>) -> ValueError {
    ValueError { key: def.key.to_string(), message: message.into() }
}

/// Typed value after parsing (for parity tests and consumers).
#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<String>),
    Json(Value),
}

/// Parses a canonical env string for `def` with the parsers of `pl_core::config::Env`
/// (identical to the startup path) and checks bounds/choices.
pub fn parse(def: &SettingDef, raw: &str) -> Result<Parsed, ValueError> {
    let env = Env::from_pairs([(def.key, raw)]);
    parse_from_env(def, &env)?.ok_or_else(|| err(def, "not set"))
}

/// Like [`parse`], but from an arbitrary [`Env`] (startup path: process environment).
/// `Ok(None)` if the key is not set there.
pub fn parse_from_env(def: &SettingDef, env: &Env) -> Result<Option<Parsed>, ValueError> {
    let Some(raw) = env.get(def.key) else {
        return Ok(None);
    };
    let ce = |e: pl_core::config::ConfigError| err(def, e.to_string());
    let p = match def.ty {
        Ty::Bool => Parsed::Bool(env.bool(def.key, "").map_err(ce)?),
        Ty::OnOff => Parsed::Bool(match raw.trim().to_ascii_lowercase().as_str() {
            "on" => true,
            "off" => false,
            v => pl_core::config::str_to_bool(v).ok_or_else(|| err(def, format!("expected on or off, got '{raw}'")))?,
        }),
        Ty::Int { min, max } => {
            let v = env.int(def.key).map_err(ce)?.unwrap_or_default();
            if let Some(m) = min.filter(|m| v < *m) {
                return Err(err(def, format!("must be >= {m}")));
            }
            if let Some(m) = max.filter(|m| v > *m) {
                return Err(err(def, format!("must be <= {m}")));
            }
            Parsed::Int(v)
        }
        Ty::Float { min } => {
            let v = env.float(def.key).map_err(ce)?.unwrap_or_default();
            if !v.is_finite() {
                return Err(err(def, "must be a finite number"));
            }
            if let Some(m) = min.filter(|m| v < *m) {
                return Err(err(def, format!("must be >= {m}")));
            }
            Parsed::Float(v)
        }
        Ty::Json { shape } => {
            let v = env.json(def.key).map_err(ce)?.unwrap_or(Value::Null);
            match (shape, &v) {
                (JsonShape::Any, _) | (JsonShape::Object, Value::Object(_)) | (JsonShape::Array, Value::Array(_)) => {}
                (JsonShape::Object, _) => return Err(err(def, "must be a JSON object")),
                (JsonShape::Array, _) => return Err(err(def, "must be a JSON list")),
            }
            Parsed::Json(v)
        }
        Ty::List => Parsed::List(env.list(def.key, &[])),
        Ty::Choice { choices } => {
            if !choices.contains(&raw) {
                return Err(err(def, format!("invalid value '{raw}'. Valid choices are: {}", choices.join(", "))));
            }
            Parsed::Str(raw.to_string())
        }
        Ty::Cron => {
            if raw != "disable" {
                pl_queue::cron::Crontab::from_paperless(raw).map_err(|e| err(def, e.to_string()))?;
            }
            Parsed::Str(raw.to_string())
        }
        Ty::TimeZone => {
            raw.parse::<chrono_tz::Tz>().map_err(|_| err(def, format!("Incorrect timezone setting: {raw}")))?;
            Parsed::Str(raw.to_string())
        }
        Ty::Url => {
            if !raw.is_empty() {
                url::Url::parse(raw).map_err(|e| err(def, format!("invalid URL: {e}")))?;
            }
            Parsed::Str(raw.to_string())
        }
        Ty::Path | Ty::Str => Parsed::Str(raw.to_string()),
    };
    extra_checks(def, &p)?;
    Ok(Some(p))
}

/// Additional checks per key (§2.4).
fn extra_checks(def: &SettingDef, p: &Parsed) -> Result<(), ValueError> {
    if let ("PAPERLESS_CONSUMER_IGNORE_PATTERNS", Parsed::Json(Value::Array(items))) = (def.key, p) {
        for it in items {
            let Some(s) = it.as_str() else {
                return Err(err(def, "every pattern must be a string"));
            };
            regex::Regex::new(s).map_err(|e| err(def, format!("invalid regex '{s}': {e}")))?;
        }
    }
    if let ("PAPERLESS_CONSUMER_IGNORE_DIRS", Parsed::Json(Value::Array(items))) = (def.key, p)
        && items.iter().any(|i| !i.is_string())
    {
        return Err(err(def, "every entry must be a string"));
    }
    if def.key == "PAPERLESS_ENABLE_UPDATE_CHECK"
        && let Parsed::Str(s) = p
        && s != "default"
        && pl_core::config::str_to_bool(s).is_none()
    {
        return Err(err(def, format!("Cannot convert '{s}' to a boolean.")));
    }
    Ok(())
}

/// Canonical env string → typed JSON (for the API). Invalid values (e.g. from an
/// old environment) are returned as a string so that the display never fails.
pub fn to_json(def: &SettingDef, raw: &str) -> Value {
    match parse(def, raw) {
        Ok(Parsed::Bool(b)) => Value::Bool(b),
        Ok(Parsed::Int(i)) => Value::from(i),
        Ok(Parsed::Float(f)) => serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::String(raw.into())),
        Ok(Parsed::List(l)) => Value::Array(l.into_iter().map(Value::String).collect()),
        Ok(Parsed::Json(v)) => v,
        Ok(Parsed::Str(s)) => Value::String(s),
        Err(_) => Value::String(raw.to_string()),
    }
}

/// Typed JSON (API/import) → canonical env string, checked with [`parse`].
pub fn from_json(def: &SettingDef, v: &Value) -> Result<String, ValueError> {
    let raw = match (&def.ty, v) {
        (_, Value::Null) => return Err(err(def, "null is not a value (use reset)")),
        (Ty::Bool, Value::Bool(b)) => if *b { "true" } else { "false" }.to_string(),
        (Ty::OnOff, Value::Bool(b)) => if *b { "on" } else { "off" }.to_string(),
        (Ty::Int { .. }, Value::Number(n)) if n.is_i64() || n.is_u64() => n.to_string(),
        (Ty::Float { .. }, Value::Number(n)) => n.to_string(),
        (Ty::List, Value::Array(items)) => {
            let mut parts = Vec::new();
            for i in items {
                let s = i.as_str().ok_or_else(|| err(def, "list entries must be strings"))?;
                if s.contains(',') {
                    return Err(err(def, "list entries must not contain ','"));
                }
                parts.push(s.trim().to_string());
            }
            parts.join(",")
        }
        (Ty::Json { .. }, v) => serde_json::to_string(v).map_err(|e| err(def, e.to_string()))?,
        (_, Value::String(s)) => s.clone(),
        (_, other) => {
            return Err(err(def, format!("unexpected JSON type for this setting: {other}")));
        }
    };
    parse(def, &raw)?;
    Ok(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::find;

    #[test]
    fn parsers_like_start() {
        let d = find("PAPERLESS_EMAIL_PORT").unwrap();
        assert_eq!(parse(d, "587").unwrap(), Parsed::Int(587));
        assert!(parse(d, "x").is_err());
        assert!(parse(d, "0").is_err());
        let b = find("PAPERLESS_EMAIL_USE_TLS").unwrap();
        assert_eq!(parse(b, "YES").unwrap(), Parsed::Bool(true));
        assert!(parse(b, "maybe").is_err());
        let c = find("PAPERLESS_TRAIN_TASK_CRON").unwrap();
        assert!(parse(c, "5 */1 * * *").is_ok() && parse(c, "disable").is_ok());
        assert!(parse(c, "5 * *").is_err());
        let tz = find("PAPERLESS_TIME_ZONE").unwrap();
        assert!(parse(tz, "Europe/Zurich").is_ok() && parse(tz, "Mars/Base").is_err());
        let ip = find("PAPERLESS_CONSUMER_IGNORE_PATTERNS").unwrap();
        assert!(parse(ip, r#"["^\\.", "a"]"#).is_ok());
        assert!(parse(ip, r#"["("]"#).is_err());
        assert!(parse(ip, "{}").is_err());
        let ch = find("PAPERLESS_OCR_MODE").unwrap();
        assert!(parse(ch, "redo").is_ok() && parse(ch, "nope").is_err());
        let up = find("PAPERLESS_ENABLE_UPDATE_CHECK").unwrap();
        assert!(parse(up, "default").is_ok() && parse(up, "no").is_ok() && parse(up, "bla").is_err());
    }

    #[test]
    fn json_round_trip() {
        let l = find("PAPERLESS_ALLOWED_HOSTS").unwrap();
        let raw = from_json(l, &serde_json::json!(["a.example", "b.example"])).unwrap();
        assert_eq!(raw, "a.example,b.example");
        assert_eq!(to_json(l, &raw), serde_json::json!(["a.example", "b.example"]));
        let b = find("PAPERLESS_EMAIL_USE_SSL").unwrap();
        assert_eq!(from_json(b, &Value::Bool(true)).unwrap(), "true");
        assert!(from_json(b, &Value::from(1)).is_err());
        let i = find("PAPERLESS_EMAIL_PORT").unwrap();
        assert!(from_json(i, &serde_json::json!(1.5)).is_err());
    }
}
