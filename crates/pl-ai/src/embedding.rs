//! Embeddings and index text – upstream `paperless_ai/embedding.py` (v3.4.2) and the
//! llama_index classes `OllamaEmbedding` (0.10) / `OpenAILikeEmbedding` (0.4, based on
//! `OpenAIEmbedding` 0.6) on the wire level (captured in `testdata/golden_w8.json` → `wire`):
//!
//! * Ollama `POST {endpoint}/api/embed` `{"model", "input": [<text.strip()>…] | "<query.strip()>",
//!   "options": {"num_ctx": <context_size>}}`, batches of 10, response `embeddings`.
//! * OpenAI-compatible `POST {endpoint|https://api.openai.com/v1}/embeddings`
//!   `{"input": [<text with "\n" → " ">…], "model", "encoding_format": "base64"}`, batches of 10,
//!   `Authorization: Bearer <embedding_api_key|api_key|"fake">`, Antwort `data[i].embedding`
//!   (base64 float32 LE or list).
//! * HuggingFace (`huggingface`, gap C4): upstream computes sentence-transformers in the
//!   Python process (`HuggingFaceEmbedding`, `normalize=True`, batches of 10). PaperRust does not
//!   compute them itself, but queries a service with the same interface as HuggingFace:
//!   `text-embeddings-inference` (TEI, local, e.g. Docker) or the HuggingFace Inference API.
//!   `POST <endpoint>/embed` or `…/pipeline/feature-extraction`
//!   `{"inputs": [<text>…], "normalize": true, "truncate": true}` → `[[f32…]…]`. Without
//!   `AI_LLM_EMBEDDING_ENDPOINT` there is a clear configuration error – documents never go
//!   to a foreign service unasked. Same model (default
//!   `sentence-transformers/all-MiniLM-L6-v2`) → practically the same vectors as upstream.

use std::time::Duration;

use serde_json::{json, Value};

use crate::config::{AiConfig, EmbeddingBackend};
use crate::llm::{pinned_client, validate_and_pin, LlmError, PLACEHOLDER_API_KEY};

/// llama_index `DEFAULT_EMBED_BATCH_SIZE` / `OpenAILikeEmbedding(embed_batch_size=10)`.
pub const EMBED_BATCH_SIZE: usize = 10;

/// Configured embedding client.
#[derive(Clone)]
pub struct Embedder {
    backend: EmbeddingBackend,
    pub model: String,
    endpoint: String,
    api_key: String,
    context_size: u32,
    allow_internal: bool,
    timeout: Duration,
}

impl Embedder {
    /// upstream `get_embedding_model(config)`.
    pub fn from_config(c: &AiConfig) -> Result<Self, LlmError> {
        let b = c
            .llm_embedding_backend
            .ok_or_else(|| LlmError::InvalidConfig("Unsupported embedding backend: None".into()))?;
        let nz = |o: &Option<String>| o.clone().filter(|s| !s.is_empty());
        let (endpoint, model) = match b {
            EmbeddingBackend::OpenAiLike => (
                nz(&c.llm_embedding_endpoint).or(nz(&c.llm_endpoint)).unwrap_or_else(|| "https://api.openai.com/v1".into()),
                nz(&c.llm_embedding_model).unwrap_or_else(|| "text-embedding-3-small".into()),
            ),
            EmbeddingBackend::Ollama => (
                nz(&c.llm_embedding_endpoint).or(nz(&c.llm_endpoint)).unwrap_or_else(|| "http://localhost:11434".into()),
                nz(&c.llm_embedding_model).unwrap_or_else(|| "embeddinggemma".into()),
            ),
            EmbeddingBackend::HuggingFace => {
                let Some(endpoint) = nz(&c.llm_embedding_endpoint) else {
                    return Err(LlmError::InvalidConfig(
                        "embedding backend huggingface needs AI_LLM_EMBEDDING_ENDPOINT: the URL of a text-embeddings-inference server (local, e.g. http://localhost:8080) or of the HuggingFace Inference API running the model. PaperRust does not run embedding models in its own process; without an extra service, AI_LLM_EMBEDDING_BACKEND=ollama with model all-minilm (same model family as the default sentence-transformers/all-MiniLM-L6-v2) also works (rebuild the AI index after switching)".into(),
                    ));
                };
                let model = nz(&c.llm_embedding_model).unwrap_or_else(|| b.default_model().into());
                (huggingface_url(&endpoint, &model), model)
            }
        };
        Ok(Self {
            backend: b,
            model,
            endpoint: endpoint.trim_end_matches('/').to_string(),
            api_key: nz(&c.llm_embedding_api_key).or(nz(&c.llm_api_key)).unwrap_or_else(|| PLACEHOLDER_API_KEY.into()),
            context_size: c.llm_context_size,
            allow_internal: c.llm_allow_internal_endpoints,
            timeout: Duration::from_secs(u64::from(c.llm_request_timeout)),
        })
    }

    async fn client(&self) -> Result<reqwest::Client, LlmError> {
        let check = self.backend != EmbeddingBackend::OpenAiLike || self.endpoint != "https://api.openai.com/v1";
        if check {
            let (_, pin) = validate_and_pin(&self.endpoint, self.allow_internal).await?;
            if let Some((host, addr)) = pin {
                return pinned_client(self.timeout, &host, addr);
            }
        }
        reqwest::Client::builder()
            .user_agent(crate::USER_AGENT)
            .timeout(self.timeout)
            .redirect(reqwest::redirect::Policy::none())
            // Security review 2026-11-19 F11: no proxy from HTTP(S)_PROXY.
            .no_proxy()
            .build()
            .map_err(|e| LlmError::InvalidConfig(e.to_string()))
    }

    /// Request (URL, body) for a batch or a query (visible for tests).
    pub fn request(&self, texts: &[String], query: bool) -> (String, Value) {
        match self.backend {
            EmbeddingBackend::HuggingFace => {
                // sentence-transformers: text unchanged; TEI/Inference API truncate texts that are too long
                // like `encode` (truncate) and normalize like `normalize_embeddings=True`.
                let input: Vec<&String> = if query { texts.iter().take(1).collect() } else { texts.iter().collect() };
                (self.endpoint.clone(), json!({"inputs": input, "normalize": true, "truncate": true}))
            }
            EmbeddingBackend::Ollama => {
                let strip = |s: &String| crate::tokenizer::py_strip(s).to_string();
                let input = if query { json!(strip(&texts[0])) } else { json!(texts.iter().map(strip).collect::<Vec<_>>()) };
                (
                    format!("{}/api/embed", self.endpoint),
                    json!({"model": self.model, "input": input, "options": {"num_ctx": self.context_size}}),
                )
            }
            _ => {
                let input: Vec<String> = texts.iter().map(|t| t.replace('\n', " ")).collect();
                (
                    format!("{}/embeddings", self.endpoint),
                    json!({"input": input, "model": self.model, "encoding_format": "base64"}),
                )
            }
        }
    }

    async fn call(&self, http: &reqwest::Client, texts: &[String], query: bool) -> Result<Vec<Vec<f32>>, LlmError> {
        let (url, body) = self.request(texts, query);
        let mut req = http.post(&url).json(&body);
        match self.backend {
            EmbeddingBackend::Ollama => {}
            // TEI needs no key; the Inference API needs a HuggingFace token
            EmbeddingBackend::HuggingFace => {
                if self.api_key != PLACEHOLDER_API_KEY {
                    req = req.bearer_auth(&self.api_key);
                }
            }
            EmbeddingBackend::OpenAiLike => req = req.bearer_auth(&self.api_key),
        }
        let resp = req.send().await.map_err(map_err)?;
        if !resp.status().is_success() {
            return Err(LlmError::Provider(resp.status().as_u16()));
        }
        let raw = crate::llm::read_body_limited(resp, crate::llm::MAX_RESPONSE_BYTES).await.map_err(|e| match e {
            crate::llm::BodyError::Http(e) => map_err(e),
            crate::llm::BodyError::TooLarge => LlmError::InvalidConfig("response from embedding backend too large".into()),
        })?;
        let v: Value = serde_json::from_slice(&raw).map_err(|_| LlmError::Transport("error decoding response body".into()))?;
        let to_vec = |e: &Value| -> Option<Vec<f32>> {
            match e {
                Value::Array(a) => a.iter().map(|x| x.as_f64().map(|f| f as f32)).collect(),
                Value::String(s) => {
                    use base64::Engine as _;
                    let b = base64::engine::general_purpose::STANDARD.decode(s).ok()?;
                    Some(crate::vector_store::unpack(&b))
                }
                _ => None,
            }
        };
        let out: Option<Vec<Vec<f32>>> = match self.backend {
            EmbeddingBackend::Ollama => v.get("embeddings").and_then(Value::as_array).map(|a| a.iter().filter_map(to_vec).collect()),
            EmbeddingBackend::HuggingFace => v.as_array().map(|a| a.iter().filter_map(to_vec).collect()),
            _ => v.get("data").and_then(Value::as_array).map(|a| a.iter().filter_map(|d| d.get("embedding").and_then(to_vec)).collect()),
        };
        let out = out.ok_or_else(|| LlmError::InvalidConfig("invalid embedding response".into()))?;
        let expect = if query { 1 } else { texts.len() };
        if out.len() != expect {
            return Err(LlmError::InvalidConfig("embedding count mismatch".into()));
        }
        Ok(out)
    }

    /// `get_text_embedding_batch(texts)` (batches of [`EMBED_BATCH_SIZE`]).
    pub async fn embed_texts(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError> {
        let http = self.client().await?;
        let mut out = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(EMBED_BATCH_SIZE) {
            out.extend(self.call(&http, chunk, false).await?);
        }
        Ok(out)
    }

    /// `get_query_embedding(query)`.
    pub async fn embed_query(&self, q: &str) -> Result<Vec<f32>, LlmError> {
        let http = self.client().await?;
        let mut v = self.call(&http, &[q.to_string()], true).await?;
        v.pop().ok_or_else(|| LlmError::InvalidConfig("empty embedding".into()))
    }
}

/// Target URL for `huggingface`: complete URLs (`…/embed`, `…/models/…`,
/// `…/feature-extraction`) stay; the HuggingFace Inference API (`*.huggingface.co` without
/// model path) gets `/models/<model>/pipeline/feature-extraction`; everything else counts as a
/// text-embeddings-inference server and gets `/embed`.
pub fn huggingface_url(endpoint: &str, model: &str) -> String {
    let e = endpoint.trim_end_matches('/');
    if e.ends_with("/embed") || e.contains("/models/") || e.ends_with("/feature-extraction") {
        return e.to_string();
    }
    let host = e.split_once("://").map(|(_, r)| r).unwrap_or(e).split(['/', ':']).next().unwrap_or("").to_ascii_lowercase();
    if host == "huggingface.co" || host.ends_with(".huggingface.co") {
        return format!("{e}/models/{model}/pipeline/feature-extraction");
    }
    format!("{e}/embed")
}

fn map_err(e: reqwest::Error) -> LlmError {
    if e.is_timeout() {
        LlmError::Timeout
    } else if let Some(s) = e.status() {
        LlmError::Provider(s.as_u16())
    } else {
        LlmError::Transport(e.without_url().to_string())
    }
}

/// `OCR_LEADER_REGEX` + `HORIZONTAL_WHITESPACE_REGEX` (`_normalize_llm_index_text`).
pub fn normalize_llm_index_text(text: &str) -> String {
    static R1: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    static R2: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    let r1 = R1.get_or_init(|| regex::Regex::new(r"[._\-\x{00b7}]{4,}").ok());
    let r2 = R2.get_or_init(|| regex::Regex::new(r"[ \t\x{00a0}]+").ok());
    let a = match r1 {
        Some(r) => r.replace_all(text, " ").into_owned(),
        None => text.to_string(),
    };
    match r2 {
        Some(r) => r.replace_all(&a, " ").into_owned(),
        None => a,
    }
}

/// upstream `build_llm_index_text(doc)`: `notes` = `str(note.note)` in order,
/// `custom_fields` = (field name, `str(instance)`), `content` = `get_effective_content() or ""`.
pub fn build_llm_index_text(notes: &[String], custom_fields: &[(String, String)], content: &str) -> String {
    let mut lines = vec![format!("Notes: {}", notes.join(","))];
    for (name, inst) in custom_fields {
        lines.push(format!("Custom Field - {name}: {inst}"));
    }
    lines.push("\nContent:\n".into());
    lines.push(content.to_string());
    normalize_llm_index_text(&lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C4: HuggingFace via TEI / Inference API (wire format, URL rules, mandatory endpoint).
    #[test]
    fn huggingface_requests() {
        assert_eq!(huggingface_url("http://tei:8080/", "m"), "http://tei:8080/embed");
        assert_eq!(huggingface_url("http://tei:8080/embed", "m"), "http://tei:8080/embed");
        assert_eq!(
            huggingface_url("https://router.huggingface.co/hf-inference", "sentence-transformers/all-MiniLM-L6-v2"),
            "https://router.huggingface.co/hf-inference/models/sentence-transformers/all-MiniLM-L6-v2/pipeline/feature-extraction"
        );
        assert_eq!(huggingface_url("https://x.endpoints.huggingface.cloud", "m"), "https://x.endpoints.huggingface.cloud/embed");
        let mut env = std::collections::HashMap::new();
        env.insert("PAPERLESS_AI_ENABLED".to_string(), "true".to_string());
        env.insert("PAPERLESS_AI_LLM_EMBEDDING_BACKEND".to_string(), "huggingface".to_string());
        let c = AiConfig::from_env_map(&env).expect("config");
        match Embedder::from_config(&c) {
            Err(LlmError::InvalidConfig(m)) => assert!(m.contains("AI_LLM_EMBEDDING_ENDPOINT") && m.contains("ollama")),
            _ => panic!("endpoint required"),
        }
        env.insert("PAPERLESS_AI_LLM_EMBEDDING_ENDPOINT".to_string(), "http://127.0.0.1:8080".to_string());
        let c = AiConfig::from_env_map(&env).expect("config");
        let e = Embedder::from_config(&c).expect("huggingface embedder");
        assert_eq!(e.model, "sentence-transformers/all-MiniLM-L6-v2");
        let (url, body) = e.request(&["a\nb".into(), "c".into()], false);
        assert_eq!(url, "http://127.0.0.1:8080/embed");
        assert_eq!(body, json!({"inputs": ["a\nb", "c"], "normalize": true, "truncate": true}));
        let (_, q) = e.request(&["frage".into()], true);
        assert_eq!(q, json!({"inputs": ["frage"], "normalize": true, "truncate": true}));
    }

    /// C4: response of a TEI-like service (local test server) is read, batches of 10.
    #[tokio::test]
    async fn huggingface_roundtrip_against_local_tei() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c2 = calls.clone();
        let app = axum::Router::new().route(
            "/embed",
            axum::routing::post(move |axum::Json(b): axum::Json<Value>| {
                let c2 = c2.clone();
                async move {
                    c2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let n = b["inputs"].as_array().map(Vec::len).unwrap_or(0);
                    axum::Json(json!((0..n).map(|i| vec![i as f64, 0.5]).collect::<Vec<_>>()))
                }
            }),
        );
        let Ok(listener) = tokio::net::TcpListener::bind("127.0.0.1:0").await else { return };
        let Ok(addr) = listener.local_addr() else { return };
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let mut env = std::collections::HashMap::new();
        env.insert("PAPERLESS_AI_ENABLED".to_string(), "true".to_string());
        env.insert("PAPERLESS_AI_LLM_EMBEDDING_BACKEND".to_string(), "huggingface".to_string());
        env.insert("PAPERLESS_AI_LLM_EMBEDDING_ENDPOINT".to_string(), format!("http://{addr}"));
        let e = Embedder::from_config(&AiConfig::from_env_map(&env).expect("config")).expect("embedder");
        let texts: Vec<String> = (0..12).map(|i| format!("t{i}")).collect();
        let v = e.embed_texts(&texts).await.expect("embed");
        assert_eq!(v.len(), 12);
        assert_eq!(v[11], vec![1.0, 0.5]);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(e.embed_query("q").await.expect("query"), vec![0.0, 0.5]);
    }

    #[test]
    fn index_text() {
        assert_eq!(build_llm_index_text(&[], &[], "a....b\t\tc"), "Notes: \n\nContent:\n\na b c");
        assert_eq!(
            build_llm_index_text(&["x".into(), "y".into()], &[("F".into(), "F : 1".into())], ""),
            "Notes: x,y\nCustom Field - F: F : 1\n\nContent:\n\n"
        );
    }
}
