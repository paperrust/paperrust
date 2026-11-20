//! PaperRust extensions of the LLM client (no upstream counterpart): free JSON response by
//! own schema (custom fields, title), free text (prompt preview), model list and
//! connection test for the "AI" settings.
//!
//! * Ollama: `POST {endpoint}/api/chat` with `format: <schema>` or without; models `GET {endpoint}/api/tags`
//!   (`models[].name`).
//! * OpenAI-compatible: `POST {endpoint}/chat/completions` with `response_format: {type: json_schema}`;
//!   models `GET {endpoint}/models` (`data[].id`).
//!
//! Same security rules as [`LlmClient::query_raw`]: address check + pinning
//! (`PAPERLESS_AI_LLM_ALLOW_INTERNAL_ENDPOINTS`), no redirects, response size limited, the
//! API key never appears in errors or logs.

use serde_json::{json, Map, Value};

use crate::config::LlmBackend;
use crate::llm::{map_reqwest, pinned_client, read_body_limited, validate_and_pin, BodyError, LlmClient, LlmError, LLM_SYSTEM_PROMPT, MAX_RESPONSE_BYTES};

/// Result of the connection test.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConnectionCheck {
    pub ok: bool,
    /// Models reported by the provider (sorted, at most 500).
    pub models: Vec<String>,
    /// Is the configured model among them? (`None` if the provider returns no list)
    pub model_available: Option<bool>,
    /// Understandable error reason (without internals/keys).
    pub error: Option<String>,
}

impl LlmClient {
    pub fn backend(&self) -> LlmBackend {
        self.backend
    }
    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    /// Different model (e.g. image model for AI text recognition).
    pub fn with_model(mut self, model: &str) -> Self {
        if !model.trim().is_empty() {
            self.model = model.trim().to_string();
        }
        self
    }

    /// Own system text for chat/synthesis (template `chat_system`).
    pub fn with_system_text(mut self, text: Option<&str>) -> Self {
        self.system_text = text.map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
        self
    }

    /// HTTP client for the endpoint (address check + pinning like `query_raw`).
    pub(crate) async fn http_client(&self) -> Result<reqwest::Client, LlmError> {
        if self.backend == LlmBackend::Ollama || self.endpoint != "https://api.openai.com/v1" {
            let (_, pin) = validate_and_pin(&self.endpoint, self.allow_internal).await?;
            if let Some((host, addr)) = pin {
                return pinned_client(self.timeout, &host, addr);
            }
        }
        Ok(self.http.clone())
    }

    pub(crate) async fn send_json(&self, url: &str, body: &Value) -> Result<Value, LlmError> {
        let http = self.http_client().await?;
        let mut req = http.post(url).json(body);
        if self.backend == LlmBackend::OpenAiLike {
            req = req.bearer_auth(&self.api_key);
        }
        let resp = req.send().await.map_err(map_reqwest)?;
        read_json(resp).await
    }

    fn base_messages(&self, system: Option<&str>, user: &str) -> Value {
        let sys = system.unwrap_or(LLM_SYSTEM_PROMPT);
        json!([{"role": "system", "content": sys}, {"role": "user", "content": user}])
    }

    /// Request body for [`Self::complete_json`] (visible for tests).
    pub fn json_request_body(&self, prompt: &str, schema_name: &str, schema: &Value) -> (String, Value) {
        match self.backend {
            LlmBackend::Ollama => {
                let mut options = Map::new();
                options.insert("temperature".into(), json!(0));
                options.insert("num_ctx".into(), json!(self.context_size));
                for (k, v) in &self.extra {
                    options.insert(k.clone(), v.clone());
                }
                (
                    format!("{}/api/chat", self.endpoint),
                    json!({"model": self.model, "stream": false, "options": Value::Object(options), "format": schema,
                           "messages": self.base_messages(None, prompt)}),
                )
            }
            LlmBackend::OpenAiLike => {
                let mut body = Map::new();
                body.insert("messages".into(), self.base_messages(None, prompt));
                body.insert("model".into(), json!(self.model));
                for (k, v) in &self.extra {
                    body.insert(k.clone(), v.clone());
                }
                body.insert("stream".into(), json!(false));
                body.insert("temperature".into(), json!(0));
                body.insert(
                    "response_format".into(),
                    json!({"type": "json_schema", "json_schema": {"name": schema_name, "schema": schema, "strict": false}}),
                );
                (format!("{}/chat/completions", self.endpoint), Value::Object(body))
            }
        }
    }

    /// Response as a JSON object according to `schema` (model output is parsed; code fences are removed).
    pub async fn complete_json(&self, prompt: &str, schema_name: &str, schema: &Value) -> Result<Value, LlmError> {
        let (url, body) = self.json_request_body(prompt, schema_name, schema);
        let v = self.send_json(&url, &body).await?;
        let text = response_text(self.backend, &v).ok_or_else(|| LlmError::InvalidConfig("empty answer from backend".into()))?;
        parse_json_answer(&text).ok_or_else(|| LlmError::InvalidConfig("answer from backend is not valid JSON".into()))
    }

    /// Free text response (not streamed), optionally with own system text.
    pub async fn complete_text(&self, system: Option<&str>, prompt: &str) -> Result<String, LlmError> {
        let (url, body) = match self.backend {
            LlmBackend::Ollama => (
                format!("{}/api/chat", self.endpoint),
                json!({"model": self.model, "stream": false, "options": {"num_ctx": self.context_size}, "messages": self.base_messages(system, prompt)}),
            ),
            LlmBackend::OpenAiLike => (
                format!("{}/chat/completions", self.endpoint),
                json!({"model": self.model, "stream": false, "temperature": 0.1, "messages": self.base_messages(system, prompt)}),
            ),
        };
        let v = self.send_json(&url, &body).await?;
        response_text(self.backend, &v).map(|t| strip_think(&t)).ok_or_else(|| LlmError::InvalidConfig("empty answer from backend".into()))
    }

    /// Models of the provider (Ollama `/api/tags`, OpenAI `/models`).
    pub async fn list_models(&self) -> Result<Vec<String>, LlmError> {
        let http = self.http_client().await?;
        let url = match self.backend {
            LlmBackend::Ollama => format!("{}/api/tags", self.endpoint),
            LlmBackend::OpenAiLike => format!("{}/models", self.endpoint),
        };
        let mut req = http.get(&url);
        if self.backend == LlmBackend::OpenAiLike {
            req = req.bearer_auth(&self.api_key);
        }
        let resp = req.send().await.map_err(map_reqwest)?;
        let v = read_json(resp).await?;
        let mut out: Vec<String> = match self.backend {
            LlmBackend::Ollama => v.get("models").and_then(Value::as_array).map(|a| a.iter().filter_map(|m| m.get("name").or_else(|| m.get("model")).and_then(Value::as_str).map(str::to_string)).collect()).unwrap_or_default(),
            LlmBackend::OpenAiLike => v.get("data").and_then(Value::as_array).map(|a| a.iter().filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string)).collect()).unwrap_or_default(),
        };
        out.sort();
        out.dedup();
        out.truncate(500);
        Ok(out)
    }

    /// Connection test: fetch the model list and check whether the configured model is in it.
    pub async fn check_connection(&self) -> ConnectionCheck {
        match self.list_models().await {
            Ok(models) => {
                let avail = if models.is_empty() { None } else { Some(model_in_list(&self.model, &models)) };
                ConnectionCheck { ok: true, models, model_available: avail, error: None }
            }
            Err(e) => ConnectionCheck { ok: false, models: Vec::new(), model_available: None, error: Some(friendly_error(&e)) },
        }
    }
}

/// Ollama names models `name:tag`; `llava` matches `llava:latest`.
pub fn model_in_list(model: &str, models: &[String]) -> bool {
    models.iter().any(|m| m == model || m.strip_suffix(":latest") == Some(model) || model.strip_suffix(":latest") == Some(m.as_str()))
}

/// Short, understandable error reason (keys and addresses of internal services are never returned).
pub fn friendly_error(e: &LlmError) -> String {
    match e {
        LlmError::Timeout => "timeout".into(),
        LlmError::Provider(401) | LlmError::Provider(403) => "unauthorized".into(),
        LlmError::Provider(404) => "not_found".into(),
        LlmError::Provider(s) => format!("provider_status_{s}"),
        LlmError::InvalidConfig(m) if m.contains("non-public") => "blocked_address".into(),
        LlmError::InvalidConfig(m) if m.contains("resolve") => "unresolvable".into(),
        LlmError::InvalidConfig(_) => "invalid_configuration".into(),
        LlmError::Transport(_) => "unreachable".into(),
    }
}

pub(crate) async fn read_json(resp: reqwest::Response) -> Result<Value, LlmError> {
    let status = resp.status();
    if !status.is_success() {
        return Err(LlmError::Provider(status.as_u16()));
    }
    let raw = read_body_limited(resp, MAX_RESPONSE_BYTES).await.map_err(|e| match e {
        BodyError::Http(e) if e.is_timeout() => LlmError::Timeout,
        BodyError::Http(_) => LlmError::InvalidConfig("invalid JSON from backend".into()),
        BodyError::TooLarge => LlmError::InvalidConfig("response from backend too large".into()),
    })?;
    serde_json::from_slice(&raw).map_err(|_| LlmError::InvalidConfig("invalid JSON from backend".into()))
}

/// Response text (Ollama `message.content`, OpenAI `choices[0].message.content`).
pub(crate) fn response_text(backend: LlmBackend, v: &Value) -> Option<String> {
    let p = match backend {
        LlmBackend::Ollama => "/message/content",
        LlmBackend::OpenAiLike => "/choices/0/message/content",
    };
    v.pointer(p).and_then(Value::as_str).map(str::to_string).filter(|s| !s.trim().is_empty())
}

/// Removes `<think>…</think>` (thinking models like qwen3/deepseek-r1).
pub fn strip_think(s: &str) -> String {
    let mut out = s.to_string();
    while let (Some(a), Some(b)) = (out.find("<think>"), out.find("</think>")) {
        if b < a {
            break;
        }
        out.replace_range(a..b + "</think>".len(), "");
    }
    out.trim().to_string()
}

/// Removes an enclosing Markdown code fence (```lang … ```).
pub fn strip_fence(s: &str) -> String {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```") {
        let body = rest.split_once('\n').map(|(_, b)| b).unwrap_or("");
        let body = body.trim_end();
        return body.strip_suffix("```").unwrap_or(body).trim().to_string();
    }
    t.to_string()
}

/// JSON from a model response: directly, without fence/thinking part, else the first `{…}`.
pub fn parse_json_answer(text: &str) -> Option<Value> {
    let t = strip_fence(&strip_think(text));
    if let Ok(v) = serde_json::from_str::<Value>(&t) {
        return v.is_object().then_some(v);
    }
    let (a, b) = (t.find('{')?, t.rfind('}')?);
    if b <= a {
        return None;
    }
    serde_json::from_str::<Value>(&t[a..=b]).ok().filter(Value::is_object)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AiConfig;
    use crate::testutil::mock_server;

    fn client(backend: &str, endpoint: &str) -> LlmClient {
        let mut env = std::collections::HashMap::new();
        env.insert("PAPERLESS_AI_ENABLED".to_string(), "yes".to_string());
        env.insert("PAPERLESS_AI_LLM_BACKEND".to_string(), backend.to_string());
        env.insert("PAPERLESS_AI_LLM_ENDPOINT".to_string(), endpoint.to_string());
        env.insert("PAPERLESS_AI_LLM_MODEL".to_string(), "m1".to_string());
        env.insert("PAPERLESS_AI_LLM_API_KEY".to_string(), "sk-secret".to_string());
        LlmClient::from_config(&AiConfig::from_env_map(&env).unwrap()).unwrap()
    }

    #[test]
    fn json_answer_parsing() {
        assert_eq!(parse_json_answer("{\"a\":1}"), Some(json!({"a": 1})));
        assert_eq!(parse_json_answer("```json\n{\"a\":2}\n```"), Some(json!({"a": 2})));
        assert_eq!(parse_json_answer("<think>hm {x}</think>Here: {\"a\":3} done"), Some(json!({"a": 3})));
        assert_eq!(parse_json_answer("no json"), None);
        assert_eq!(parse_json_answer("[1,2]"), None);
        assert_eq!(strip_think("<think>a</think> b <think>c</think>"), "b");
        assert!(model_in_list("llava", &["llava:latest".into()]));
        assert!(model_in_list("llava:latest", &["llava".into()]));
        assert!(!model_in_list("llava", &["llava:13b".into()]));
    }

    #[tokio::test]
    async fn ollama_models_and_json() {
        let (url, log) = mock_server(|r| match r.path.as_str() {
            "/api/tags" => (200, r#"{"models":[{"name":"qwen2.5vl:7b"},{"name":"m1:latest"}]}"#.into()),
            "/api/chat" => (200, r#"{"message":{"role":"assistant","content":"```json\n{\"fields\":[]}\n```"},"done":true}"#.into()),
            _ => (404, "{}".into()),
        })
        .await;
        let c = client("ollama", &url);
        let chk = c.check_connection().await;
        assert!(chk.ok);
        assert_eq!(chk.models, vec!["m1:latest".to_string(), "qwen2.5vl:7b".to_string()]);
        assert_eq!(chk.model_available, Some(true));
        let v = c.complete_json("p", "S", &json!({"type": "object"})).await.unwrap();
        assert_eq!(v, json!({"fields": []}));
        let reqs = log.lock().unwrap().clone();
        let chat = reqs.iter().find(|r| r.path == "/api/chat").unwrap();
        let body = chat.json();
        assert_eq!(body["format"], json!({"type": "object"}));
        assert_eq!(body["messages"][0]["role"], "system");
        assert!(chat.header("authorization").is_none(), "Ollama ohne Schlüssel");
    }

    #[tokio::test]
    async fn openai_models_and_errors() {
        let (url, log) = mock_server(|r| match r.path.as_str() {
            "/v1/models" => (200, r#"{"data":[{"id":"gpt-4o-mini"},{"id":"gpt-4o"}]}"#.into()),
            "/v1/chat/completions" => (200, r#"{"choices":[{"message":{"content":"Hallo"}}]}"#.into()),
            _ => (404, "{}".into()),
        })
        .await;
        let c = client("openai-like", &format!("{url}/v1"));
        let chk = c.check_connection().await;
        assert_eq!(chk.model_available, Some(false));
        assert_eq!(c.complete_text(Some("sys"), "hi").await.unwrap(), "Hallo");
        let reqs = log.lock().unwrap().clone();
        assert_eq!(reqs[0].header("authorization"), Some("Bearer sk-secret"));
        let (_, body) = c.json_request_body("p", "Schema", &json!({"type": "object"}));
        assert_eq!(body["response_format"]["type"], "json_schema");
        // Error: understandable reason without key
        let (bad, _) = mock_server(|_| (401, "{\"error\":\"sk-secret invalid\"}".into())).await;
        let chk = client("openai-like", &bad).check_connection().await;
        assert!(!chk.ok);
        assert_eq!(chk.error.as_deref(), Some("unauthorized"));
        assert!(!format!("{chk:?}").contains("sk-secret"));
        let chk = client("ollama", "http://127.0.0.1:9").check_connection().await;
        assert_eq!(chk.error.as_deref(), Some("unreachable"));
    }
}
