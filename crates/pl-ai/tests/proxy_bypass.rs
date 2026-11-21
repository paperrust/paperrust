//! Security review 2026-11-19 F11: AI/MCP clients must not use a proxy set via `HTTP(S)_PROXY`/`ALL_PROXY`.
//! Otherwise the proxy establishes the connection itself (own
//! DNS resolution) and the address check including the pinned IP does not take effect.
//!
//! Own test file with exactly one test, because the proxy variables are set process-wide.
//! Local hosts only (127.0.0.1), no third-party services.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Local server that counts every connection and answers with `body`.
async fn counting_server(body: &'static str) -> (std::net::SocketAddr, Arc<AtomicUsize>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let h2 = hits.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            h2.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut b = [0u8; 4096];
                let _ = s.read(&mut b).await;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
            });
        }
    });
    (addr, hits)
}

#[tokio::test]
async fn ai_clients_ignore_environment_proxy() {
    let (proxy, proxy_hits) = counting_server("{}").await;
    let (target, target_hits) = counting_server(r#"{"ok":true}"#).await;
    let p = format!("http://{proxy}");
    // SAFETY: only test in this file (own process), no other threads read the environment.
    unsafe {
        for k in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"] {
            std::env::set_var(k, &p);
        }
        std::env::set_var("NO_PROXY", "");
        std::env::set_var("no_proxy", "");
    }

    // 1. pinned_client (LLM/embedding/MCP ingest with DNS names)
    let c = pl_ai::llm::pinned_client(Duration::from_secs(5), "llm.invalid", target).unwrap();
    let r = c.get(format!("http://llm.invalid:{}/", target.port())).send().await.unwrap();
    assert_eq!(r.text().await.unwrap(), r#"{"ok":true}"#);

    // 2. MCP ingest without pinning (IP literal)
    let f = pl_ai::mcp::ingest::fetch_url(&format!("http://{target}/doc.pdf"), 1000, true).await.unwrap();
    assert_eq!(f.bytes, br#"{"ok":true}"#);

    // 3. LLM client from the configuration (default client without pinning)
    let mut env = HashMap::new();
    env.insert("PAPERLESS_AI_ENABLED".to_string(), "true".to_string());
    env.insert("PAPERLESS_AI_LLM_BACKEND".to_string(), "ollama".to_string());
    env.insert("PAPERLESS_AI_LLM_ENDPOINT".to_string(), format!("http://{target}"));
    env.insert("PAPERLESS_AI_LLM_ALLOW_INTERNAL_ENDPOINTS".to_string(), "true".to_string());
    let cfg = pl_ai::config::AiConfig::from_env_map(&env).unwrap();
    let llm = pl_ai::llm::LlmClient::from_config(&cfg).unwrap();
    // Response does not match the schema – all that matters is where the connection went.
    let _ = llm.query_raw("x").await;

    // 4. Embedding client (default client without pinning)
    let mut env2 = env.clone();
    env2.insert("PAPERLESS_AI_LLM_EMBEDDING_BACKEND".to_string(), "openai-like".to_string());
    env2.insert("PAPERLESS_AI_LLM_EMBEDDING_ENDPOINT".to_string(), format!("http://{target}"));
    let cfg2 = pl_ai::config::AiConfig::from_env_map(&env2).unwrap();
    let emb = pl_ai::embedding::Embedder::from_config(&cfg2).unwrap();
    let _ = emb.embed_query("x").await;

    assert_eq!(proxy_hits.load(Ordering::SeqCst), 0, "proxy must not be used");
    assert!(target_hits.load(Ordering::SeqCst) >= 4, "target hits: {}", target_hits.load(Ordering::SeqCst));
}
