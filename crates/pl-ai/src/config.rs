//! AI configuration.
//!
//! * upstream `paperless/settings/__init__.py@v3.4.2` ("AI Settings") + `paperless/config.py::AIConfig`:
//!   `PAPERLESS_AI_*`, overridable by `ApplicationConfiguration` (`paperless_applicationconfiguration`,
//!   singleton, fields `ai_enabled`, `llm_*`; "value from DB `or` value from environment").
//! * PaperRust extensions: `PAPERRUST_AI_*` (MCP, matching, automation) – see contract.

use std::collections::HashMap;

/// `LLMBackend`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmBackend {
    Ollama,
    OpenAiLike,
}

impl LlmBackend {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ollama" => Some(Self::Ollama),
            "openai-like" => Some(Self::OpenAiLike),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::OpenAiLike => "openai-like",
        }
    }
}

/// `LLMEmbeddingBackend`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingBackend {
    OpenAiLike,
    HuggingFace,
    Ollama,
}

impl EmbeddingBackend {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "openai-like" => Some(Self::OpenAiLike),
            "huggingface" => Some(Self::HuggingFace),
            "ollama" => Some(Self::Ollama),
            _ => None,
        }
    }
    /// `embedding._DEFAULT_MODEL_NAMES`.
    pub fn default_model(self) -> &'static str {
        match self {
            Self::OpenAiLike => "text-embedding-3-small",
            Self::HuggingFace => "sentence-transformers/all-MiniLM-L6-v2",
            Self::Ollama => "embeddinggemma",
        }
    }
}

/// Error when reading the environment (upstream: `ImproperlyConfigured` / `ValueError` at startup).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ConfigError(pub String);

/// upstream `str_to_bool`.
pub fn str_to_bool(v: &str) -> Result<bool, ConfigError> {
    match v.trim().to_lowercase().as_str() {
        "true" | "1" | "t" | "y" | "yes" => Ok(true),
        "false" | "0" | "f" | "n" | "no" => Ok(false),
        _ => Err(ConfigError(format!("Cannot convert '{v}' to a boolean."))),
    }
}

/// `AIConfig` (effective values) + PaperRust extensions.
#[derive(Debug, Clone, PartialEq)]
pub struct AiConfig {
    pub ai_enabled: bool,
    pub llm_embedding_backend: Option<EmbeddingBackend>,
    pub llm_embedding_model: Option<String>,
    pub llm_embedding_api_key: Option<String>,
    pub llm_embedding_endpoint: Option<String>,
    pub llm_embedding_chunk_size: u32,
    pub llm_context_size: u32,
    pub llm_request_timeout: u32,
    pub llm_backend: Option<LlmBackend>,
    pub llm_model: Option<String>,
    pub llm_api_key: Option<String>,
    pub llm_endpoint: Option<String>,
    pub llm_output_language: Option<String>,
    pub llm_allow_internal_endpoints: bool,
    pub llm_extra_params: serde_json::Map<String, serde_json::Value>,
    /// Extensions (`PAPERRUST_AI_*`).
    pub rs: RsAiConfig,
}

/// PaperRust's own settings.
#[derive(Debug, Clone, PartialEq)]
pub struct RsAiConfig {
    /// `PAPERRUST_AI_MCP_ENABLED` (default: no) – serve `/mcp`.
    pub mcp_enabled: bool,
    /// `PAPERRUST_AI_MCP_RATE_LIMIT` – requests per minute and token (default 120).
    pub mcp_rate_limit_per_minute: u32,
    /// `PAPERRUST_AI_MCP_MAX_CREATES_PER_HOUR` – new creations per token and hour (default 30).
    pub mcp_max_creates_per_hour: u32,
    /// `PAPERRUST_AI_MCP_ALLOWED_ORIGINS` – comma-separated; empty = only requests without `Origin`
    /// or with the same host (protection against DNS rebinding per the MCP specification).
    pub mcp_allowed_origins: Vec<String>,
    /// `PAPERRUST_AI_MCP_TOOL_GROUPS` – enabled tool groups (`read`, `write`, `ingest`),
    /// comma-separated (default `read,write`; `ingest` is opt-in). Admin switch per group.
    pub mcp_tool_groups: Vec<String>,
    /// `PAPERRUST_AI_MCP_ALLOW_URL_FETCH` – `add_document` may fetch an http(s) URL
    /// (default no; public addresses only, SSRF protection).
    pub mcp_allow_url_fetch: bool,
    /// `PAPERRUST_AI_MCP_MAX_UPLOAD_BYTES` – largest file for `add_document` (decoded, default
    /// 1 572 864 = 1.5 MiB, fits below pl-server's 2.5 MiB limit when base64-encoded).
    pub mcp_max_upload_bytes: u32,
    /// `PAPERRUST_AI_MCP_MAX_INGESTS_PER_HOUR` – `add_document` per token and hour (default 30).
    pub mcp_max_ingests_per_hour: u32,
    /// `PAPERRUST_AI_MATCH_THRESHOLD` – threshold of the extended matching (default 0.8).
    pub match_threshold: f64,
    /// `PAPERRUST_AI_AUTO_MODE` – `off` | `suggest` | `apply` (Standard off).
    pub auto_mode: AutoMode,
    /// `PAPERRUST_AI_AUTO_MIN_CONFIDENCE` – apply automatically from here (default 0.9).
    pub auto_min_confidence: f64,
    /// `PAPERRUST_AI_AUTO_TRIGGER_TAG` – tag name that triggers processing (default empty).
    pub auto_trigger_tag: Option<String>,
    /// `PAPERRUST_AI_AUTO_MAX_NEW_OBJECTS` – new objects per run (default 3).
    pub auto_max_new_objects: u32,
    /// `PAPERRUST_AI_AUTO_MAX_DOCUMENTS_PER_RUN` (Standard 50).
    pub auto_max_documents_per_run: u32,
}

/// Automation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoMode {
    Off,
    Suggest,
    Apply,
}

impl Default for RsAiConfig {
    fn default() -> Self {
        Self {
            mcp_enabled: false,
            mcp_rate_limit_per_minute: 120,
            mcp_max_creates_per_hour: 30,
            mcp_allowed_origins: Vec::new(),
            mcp_tool_groups: vec!["read".into(), "write".into()],
            mcp_allow_url_fetch: false,
            mcp_max_upload_bytes: 1_572_864,
            mcp_max_ingests_per_hour: 30,
            match_threshold: crate::compat::MATCH_THRESHOLD,
            auto_mode: AutoMode::Off,
            auto_min_confidence: 0.9,
            auto_trigger_tag: None,
            auto_max_new_objects: 3,
            auto_max_documents_per_run: 50,
        }
    }
}

/// Values from `paperless_applicationconfiguration` (all nullable).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AppConfigRow {
    pub ai_enabled: Option<bool>,
    pub llm_embedding_backend: Option<String>,
    pub llm_embedding_model: Option<String>,
    pub llm_embedding_api_key: Option<String>,
    pub llm_embedding_endpoint: Option<String>,
    pub llm_embedding_chunk_size: Option<i32>,
    pub llm_context_size: Option<i64>,
    pub llm_backend: Option<String>,
    pub llm_model: Option<String>,
    pub llm_api_key: Option<String>,
    pub llm_endpoint: Option<String>,
    pub llm_output_language: Option<String>,
    pub llm_request_timeout: Option<i32>,
}

fn non_empty(v: Option<&String>) -> Option<String> {
    v.filter(|s| !s.is_empty()).cloned()
}

fn int_env(env: &HashMap<String, String>, key: &str, default: u32) -> Result<u32, ConfigError> {
    match env.get(key) {
        None => Ok(default),
        Some(v) => {
            let n: i64 = v.trim().parse().map_err(|_| ConfigError(format!("invalid literal for int() with base 10: '{v}'")))?;
            if n < 1 {
                return Err(ConfigError(format!("{key} must be >= 1")));
            }
            u32::try_from(n).map_err(|_| ConfigError(format!("{key} out of range")))
        }
    }
}

impl AiConfig {
    /// From the process environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        // process environment plus values saved in the web UI (docs/SETTINGS.md WP4d)
        Self::from_env_map(&pl_core::config::vars_live())
    }

    /// From a variable map (testable).
    pub fn from_env_map(env: &HashMap<String, String>) -> Result<Self, ConfigError> {
        // PAPERRUST_AI_* primary, PAPERLESS_AI_* (paperless-ngx) and PAPERLESS_RS_AI_* (old) as aliases
        // (without feature `server` there is no pl-core: then only the names as given).
        #[cfg(feature = "server")]
        let env = &pl_core::config::with_aliases(env);
        let s = |k: &str| env.get(k).cloned();
        let ai_enabled = str_to_bool(env.get("PAPERLESS_AI_ENABLED").map(String::as_str).unwrap_or("NO"))?;
        let emb = match env.get("PAPERLESS_AI_LLM_EMBEDDING_BACKEND") {
            None => None,
            Some(v) => Some(EmbeddingBackend::parse(v).ok_or_else(|| {
                ConfigError(format!("Invalid value '{v}' for PAPERRUST_AI_LLM_EMBEDDING_BACKEND"))
            })?),
        };
        let backend = match env.get("PAPERLESS_AI_LLM_BACKEND") {
            None => None,
            Some(v) => Some(
                LlmBackend::parse(v)
                    .ok_or_else(|| ConfigError(format!("Invalid value '{v}' for PAPERRUST_AI_LLM_BACKEND")))?,
            ),
        };
        let extra = match env.get("PAPERLESS_AI_LLM_EXTRA_PARAMS") {
            None => serde_json::Map::new(),
            Some(v) => match serde_json::from_str::<serde_json::Value>(v) {
                Ok(serde_json::Value::Object(m)) => m,
                _ => return Err(ConfigError("PAPERRUST_AI_LLM_EXTRA_PARAMS must be a JSON object".into())),
            },
        };
        let allow_internal =
            str_to_bool(env.get("PAPERLESS_AI_LLM_ALLOW_INTERNAL_ENDPOINTS").map(String::as_str).unwrap_or("true"))?;

        let mut rs = RsAiConfig::default();
        if let Some(v) = env.get("PAPERRUST_AI_MCP_ENABLED") {
            rs.mcp_enabled = str_to_bool(v)?;
        }
        rs.mcp_rate_limit_per_minute = int_env(env, "PAPERRUST_AI_MCP_RATE_LIMIT", rs.mcp_rate_limit_per_minute)?;
        rs.mcp_max_creates_per_hour =
            int_env(env, "PAPERRUST_AI_MCP_MAX_CREATES_PER_HOUR", rs.mcp_max_creates_per_hour)?;
        if let Some(v) = env.get("PAPERRUST_AI_MCP_ALLOWED_ORIGINS") {
            rs.mcp_allowed_origins = v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect();
        }
        if let Some(v) = env.get("PAPERRUST_AI_MCP_TOOL_GROUPS") {
            let groups: Vec<String> = v.split(',').map(|x| x.trim().to_lowercase()).filter(|x| !x.is_empty()).collect();
            if let Some(bad) = groups.iter().find(|g| !crate::mcp::scopes::GROUPS.contains(&g.as_str())) {
                return Err(ConfigError(format!("PAPERRUST_AI_MCP_TOOL_GROUPS: unknown group '{bad}' (read, write, ingest)")));
            }
            rs.mcp_tool_groups = groups;
        }
        if let Some(v) = env.get("PAPERRUST_AI_MCP_ALLOW_URL_FETCH") {
            rs.mcp_allow_url_fetch = str_to_bool(v)?;
        }
        rs.mcp_max_upload_bytes = int_env(env, "PAPERRUST_AI_MCP_MAX_UPLOAD_BYTES", rs.mcp_max_upload_bytes)?;
        rs.mcp_max_ingests_per_hour = int_env(env, "PAPERRUST_AI_MCP_MAX_INGESTS_PER_HOUR", rs.mcp_max_ingests_per_hour)?;
        if let Some(v) = env.get("PAPERRUST_AI_MATCH_THRESHOLD") {
            let t: f64 = v.trim().parse().map_err(|_| ConfigError("PAPERRUST_AI_MATCH_THRESHOLD must be a number".into()))?;
            if !(0.0..=1.0).contains(&t) {
                return Err(ConfigError("PAPERRUST_AI_MATCH_THRESHOLD must be between 0 and 1".into()));
            }
            rs.match_threshold = t;
        }
        if let Some(v) = env.get("PAPERRUST_AI_AUTO_MODE") {
            rs.auto_mode = match v.trim().to_lowercase().as_str() {
                "off" | "" => AutoMode::Off,
                "suggest" => AutoMode::Suggest,
                "apply" => AutoMode::Apply,
                _ => return Err(ConfigError("PAPERRUST_AI_AUTO_MODE must be off, suggest or apply".into())),
            };
        }
        if let Some(v) = env.get("PAPERRUST_AI_AUTO_MIN_CONFIDENCE") {
            rs.auto_min_confidence =
                v.trim().parse().map_err(|_| ConfigError("PAPERRUST_AI_AUTO_MIN_CONFIDENCE must be a number".into()))?;
        }
        rs.auto_trigger_tag = s("PAPERRUST_AI_AUTO_TRIGGER_TAG").filter(|x| !x.is_empty());
        rs.auto_max_new_objects = int_env(env, "PAPERRUST_AI_AUTO_MAX_NEW_OBJECTS", rs.auto_max_new_objects)?;
        rs.auto_max_documents_per_run =
            int_env(env, "PAPERRUST_AI_AUTO_MAX_DOCUMENTS_PER_RUN", rs.auto_max_documents_per_run)?;

        Ok(Self {
            ai_enabled,
            llm_embedding_backend: emb,
            llm_embedding_model: s("PAPERLESS_AI_LLM_EMBEDDING_MODEL"),
            llm_embedding_api_key: s("PAPERLESS_AI_LLM_EMBEDDING_API_KEY"),
            llm_embedding_endpoint: s("PAPERLESS_AI_LLM_EMBEDDING_ENDPOINT"),
            llm_embedding_chunk_size: int_env(env, "PAPERLESS_AI_LLM_EMBEDDING_CHUNK_SIZE", 1024)?,
            llm_context_size: int_env(env, "PAPERLESS_AI_LLM_CONTEXT_SIZE", 8192)?,
            llm_request_timeout: int_env(env, "PAPERLESS_AI_LLM_REQUEST_TIMEOUT", 120)?,
            llm_backend: backend,
            llm_model: s("PAPERLESS_AI_LLM_MODEL"),
            llm_api_key: s("PAPERLESS_AI_LLM_API_KEY"),
            llm_endpoint: s("PAPERLESS_AI_LLM_ENDPOINT"),
            llm_output_language: s("PAPERLESS_AI_LLM_OUTPUT_LANGUAGE"),
            llm_allow_internal_endpoints: allow_internal,
            llm_extra_params: extra,
            rs,
        })
    }

    /// `AIConfig.__post_init__`: DB value `or` environment value (empty strings/0 count as "not set",
    /// `ai_enabled` only for `NULL`). `llm_allow_internal_endpoints`/`llm_extra_params` from the environment only.
    pub fn with_app_config(&self, row: &AppConfigRow) -> Self {
        let mut c = self.clone();
        if let Some(b) = row.ai_enabled {
            c.ai_enabled = b;
        }
        if let Some(b) = non_empty(row.llm_embedding_backend.as_ref()).and_then(|v| EmbeddingBackend::parse(&v)) {
            c.llm_embedding_backend = Some(b);
        }
        c.llm_embedding_model = non_empty(row.llm_embedding_model.as_ref()).or(c.llm_embedding_model);
        c.llm_embedding_api_key = non_empty(row.llm_embedding_api_key.as_ref()).or(c.llm_embedding_api_key);
        c.llm_embedding_endpoint = non_empty(row.llm_embedding_endpoint.as_ref()).or(c.llm_embedding_endpoint);
        if let Some(v) = row.llm_embedding_chunk_size.filter(|v| *v > 0) {
            c.llm_embedding_chunk_size = v as u32;
        }
        if let Some(v) = row.llm_context_size.filter(|v| *v > 0) {
            c.llm_context_size = u32::try_from(v).unwrap_or(c.llm_context_size);
        }
        if let Some(v) = row.llm_request_timeout.filter(|v| *v > 0) {
            c.llm_request_timeout = v as u32;
        }
        if let Some(b) = non_empty(row.llm_backend.as_ref()).and_then(|v| LlmBackend::parse(&v)) {
            c.llm_backend = Some(b);
        }
        c.llm_model = non_empty(row.llm_model.as_ref()).or(c.llm_model);
        c.llm_api_key = non_empty(row.llm_api_key.as_ref()).or(c.llm_api_key);
        c.llm_endpoint = non_empty(row.llm_endpoint.as_ref()).or(c.llm_endpoint);
        c.llm_output_language = non_empty(row.llm_output_language.as_ref()).or(c.llm_output_language);
        c
    }

    /// `AIConfig.llm_index_enabled`.
    pub fn llm_index_enabled(&self) -> bool {
        self.ai_enabled && self.llm_embedding_backend.is_some()
    }

    /// `embedding.get_configured_model_name`.
    pub fn configured_embedding_model_name(&self) -> String {
        if let Some(m) = self.llm_embedding_model.as_ref().filter(|m| !m.is_empty()) {
            return m.clone();
        }
        self.llm_embedding_backend.map(EmbeddingBackend::default_model).unwrap_or("sentence-transformers/all-MiniLM-L6-v2").to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn defaults() {
        let c = AiConfig::from_env_map(&env(&[])).unwrap();
        assert!(!c.ai_enabled);
        assert_eq!(c.llm_embedding_chunk_size, 1024);
        assert_eq!(c.llm_context_size, 8192);
        assert_eq!(c.llm_request_timeout, 120);
        assert!(c.llm_allow_internal_endpoints);
        assert!(!c.llm_index_enabled());
        assert!(!c.rs.mcp_enabled);
        assert_eq!(c.rs.mcp_tool_groups, vec!["read".to_string(), "write".to_string()]);
        assert!(!c.rs.mcp_allow_url_fetch);
        assert_eq!(c.rs.mcp_max_upload_bytes, 1_572_864);
    }

    #[test]
    fn mcp_groups() {
        let c = AiConfig::from_env_map(&env(&[("PAPERRUST_AI_MCP_TOOL_GROUPS", " Read, ingest ,"), ("PAPERRUST_AI_MCP_ALLOW_URL_FETCH", "yes")])).unwrap();
        assert_eq!(c.rs.mcp_tool_groups, vec!["read".to_string(), "ingest".to_string()]);
        assert!(c.rs.mcp_allow_url_fetch);
        assert!(AiConfig::from_env_map(&env(&[("PAPERRUST_AI_MCP_TOOL_GROUPS", "read,admin")])).is_err());
        assert!(AiConfig::from_env_map(&env(&[("PAPERRUST_AI_MCP_MAX_UPLOAD_BYTES", "0")])).is_err());
    }

    #[test]
    fn invalid() {
        assert!(AiConfig::from_env_map(&env(&[("PAPERLESS_AI_LLM_BACKEND", "foo")])).is_err());
        assert!(AiConfig::from_env_map(&env(&[("PAPERLESS_AI_LLM_CONTEXT_SIZE", "0")])).is_err());
        assert!(AiConfig::from_env_map(&env(&[("PAPERLESS_AI_LLM_EXTRA_PARAMS", "[1]")])).is_err());
        assert!(AiConfig::from_env_map(&env(&[("PAPERLESS_AI_ENABLED", "maybe")])).is_err());
    }

    #[test]
    fn db_overrides() {
        let c = AiConfig::from_env_map(&env(&[
            ("PAPERLESS_AI_ENABLED", "yes"),
            ("PAPERLESS_AI_LLM_MODEL", "env-model"),
            ("PAPERLESS_AI_LLM_EMBEDDING_BACKEND", "ollama"),
        ]))
        .unwrap();
        let row = AppConfigRow { ai_enabled: Some(false), llm_model: Some(String::new()), llm_context_size: Some(4096), ..Default::default() };
        let e = c.with_app_config(&row);
        assert!(!e.ai_enabled);
        assert_eq!(e.llm_model.as_deref(), Some("env-model"));
        assert_eq!(e.llm_context_size, 4096);
        assert_eq!(c.configured_embedding_model_name(), "embeddinggemma");
    }
}
