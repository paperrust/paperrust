//! Server part of pl-ai (feature `server`): upstream routes `ai_suggestions` and `chat`,
//! LLM index tasks, workflow action `APPLY_AI_SUGGESTIONS`, review list `/api/x-ai/review/`.

pub mod assist;
pub mod cache;
pub mod chat;
pub mod data;
pub mod index;
pub mod jobs;
pub mod mcp_admin;
pub mod mcp_index;
pub mod review;
pub mod store;
pub mod suggest;
pub mod workflow;


use pl_core::{ApiError, AppState};

use crate::config::AiConfig;

/// Own tables of pl-ai (`migrations/0001_paperrust_ai.sql`: `paperrust_taxonomy_alias`,
/// `paperrust_ai_audit`, `paperrust_ai_review`; `0002_paperrust_ai_mcp_scope.sql`: `paperrust_ai_mcp_scope`; expand-only, idempotent). pl-server applies them at
/// startup (W10), otherwise call only deliberately.
pub mod migrations {
    /// `migrations/0001_paperrust_ai.sql`.
    pub const M0001_PAPERRUST_AI: &str = include_str!("../../migrations/0001_paperrust_ai.sql");
    /// `migrations/0002_paperrust_ai_mcp_scope.sql` (MCP-Token-Scopes).
    pub const M0002_PAPERRUST_AI_MCP_SCOPE: &str = include_str!("../../migrations/0002_paperrust_ai_mcp_scope.sql");
    /// `migrations/0003_paperrust_ai_prompts_jobs.sql` (own prompts, AI options, post-processing).
    pub const M0003_PAPERRUST_AI_PROMPTS_JOBS: &str = include_str!("../../migrations/0003_paperrust_ai_prompts_jobs.sql");

    /// All migrations in order `(name, sql)`.
    pub const ALL: &[(&str, &str)] =
        &[("0001_paperrust_ai", M0001_PAPERRUST_AI), ("0002_paperrust_ai_mcp_scope", M0002_PAPERRUST_AI_MCP_SCOPE),
        ("0003_paperrust_ai_prompts_jobs", M0003_PAPERRUST_AI_PROMPTS_JOBS),
    ];

    /// Applies all migrations (idempotent, `CREATE … IF NOT EXISTS`).
    pub async fn apply(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
        for (_, sql) in ALL {
            sqlx::raw_sql(sql).execute(pool).await?;
        }
        Ok(())
    }
}

type EnvConfigCache = Option<(Option<std::sync::Arc<std::collections::BTreeMap<String, String>>>, Result<AiConfig, String>)>;
static ENV_CONFIG: std::sync::Mutex<EnvConfigCache> = std::sync::Mutex::new(None);

/// `PAPERLESS_AI_*`/`PAPERRUST_AI_*` from the environment (like Django settings at startup), re-read
/// as soon as the values change in the UI (docs/SETTINGS.md WP4d).
pub fn env_config() -> Result<AiConfig, ApiError> {
    let ov = pl_core::config::live_overlay();
    let mut g = match ENV_CONFIG.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let stale = match &*g {
        Some((o, _)) => match (o, &ov) {
            (None, None) => false,
            (Some(a), Some(b)) => !std::sync::Arc::ptr_eq(a, b),
            _ => true,
        },
        None => true,
    };
    if stale {
        *g = Some((ov, AiConfig::from_env().map_err(|e| e.0)));
    }
    g.as_ref()
        .map(|(_, r)| r.clone())
        .unwrap_or_else(|| Err("unavailable".into()))
        .map_err(|e| ApiError::internal(format!("invalid AI configuration: {e}")))
}

/// `AIConfig()` (environment + `ApplicationConfiguration`).
pub async fn effective_config(state: &AppState) -> Result<AiConfig, ApiError> {
    let env = env_config()?;
    data::load_ai_config(state.db(), &env).await
}

/// Python `int(str)` for decimal numbers (whitespace, sign, single `_` between digits).
pub fn py_int(s: &str) -> Option<i64> {
    let t = s.trim_matches(crate::compat::py_isspace);
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') || digits.contains("__") {
        return None;
    }
    let clean: String = digits.chars().filter(|c| *c != '_').collect();
    if !clean.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let v: i64 = clean.parse().ok()?;
    Some(if neg { -v } else { v })
}

/// Python `str(number)` of a JSON value.
pub fn py_number_str(n: &serde_json::Number) -> String {
    if n.is_i64() || n.is_u64() {
        n.to_string()
    } else {
        pl_core::json::python_float_repr(n.as_f64().unwrap_or(0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_like_python() {
        assert_eq!(py_int(" 12 "), Some(12));
        assert_eq!(py_int("-1_0"), Some(-10));
        assert_eq!(py_int("1__0"), None);
        assert_eq!(py_int("1.5"), None);
        assert_eq!(py_int(""), None);
    }
}
