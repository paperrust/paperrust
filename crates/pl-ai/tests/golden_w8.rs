//! Golden tests part 2 (W8) against the real upstream/llama_index output
//! (`tests/parity/pl-ai/gen_golden_w8.py` → `testdata/golden_w8.json`, `testdata/llmindex_py.db`).

use std::collections::BTreeMap;
use std::sync::Mutex;

use pl_ai::llm::LlmError;
use pl_ai::node::{build_document_nodes, TextNode};
use pl_ai::synth::{compact_and_refine, StreamLlm};
use pl_ai::tokenizer::{count_tokens, truncate_content, truncate_embedding_query, PromptHelper, SentenceSplitter, TokenTextSplitter};
use pl_ai::vector_store::{DocFilter, VecStore};
use pl_ai::{pickle, prompts, punkt};
use serde_json::Value;
use sha2::Digest;

fn golden() -> Value {
    serde_json::from_str(include_str!("../testdata/golden_w8.json")).expect("golden_w8.json")
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().expect("array").iter().map(|x| s(x).to_string()).collect()
}

#[test]
fn token_counts() {
    let g = golden();
    for row in g["tokens"].as_array().unwrap() {
        assert_eq!(count_tokens(s(&row[0])), row[1].as_u64().unwrap() as usize, "{:?}", s(&row[0]).chars().take(40).collect::<String>());
    }
}

#[test]
fn token_text_splitter() {
    let g = golden();
    let mut n = 0;
    for c in g["token_splitter"].as_array().unwrap() {
        let sp = TokenTextSplitter::new(" ", c["chunk_size"].as_u64().unwrap() as usize, c["overlap"].as_u64().unwrap() as usize);
        assert_eq!(sp.split_text(s(&c["text"])), strs(&c["chunks"]), "cs={} text={:?}", c["chunk_size"], s(&c["text"]).chars().take(30).collect::<String>());
        n += 1;
    }
    assert!(n > 30);
}

#[test]
fn truncate_like_upstream() {
    let g = golden();
    for c in g["truncate_content"].as_array().unwrap() {
        let r = truncate_content(s(&c["text"]), c["chunk_size"].as_u64().unwrap() as usize, c["context_size"].as_u64().unwrap() as usize);
        match &c["result"] {
            Value::String(exp) => assert_eq!(r.as_deref().ok(), Some(exp.as_str()), "{c}"),
            Value::Object(e) => assert_eq!(r.err().map(|e| e.0), e["error"].as_str().map(str::to_string)),
            _ => unreachable!(),
        }
    }
    for c in g["truncate_embedding_query"].as_array().unwrap() {
        assert_eq!(truncate_embedding_query(s(&c["text"]), c["chunk_size"].as_u64().unwrap() as usize), s(&c["result"]));
    }
}

#[test]
fn punkt_like_nltk() {
    let g = golden();
    let mut bad = Vec::new();
    for row in g["punkt"].as_array().unwrap() {
        let got = punkt::split_sentences(s(&row[0]));
        if got != strs(&row[1]) {
            bad.push(format!("{:?}\n  got {:?}\n  exp {:?}", s(&row[0]), got, strs(&row[1])));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn sentence_splitter_like_llama_index() {
    let g = golden();
    for c in g["sentence_splitter"].as_array().unwrap() {
        let sp = SentenceSplitter::new(c["chunk_size"].as_u64().unwrap() as usize, c["overlap"].as_u64().unwrap() as usize).unwrap();
        let got = sp.split_text(s(&c["text"])).unwrap();
        assert_eq!(got, strs(&c["chunks"]), "cs={} ov={}", c["chunk_size"], c["overlap"]);
    }
}

#[test]
fn index_nodes_like_llama_index() {
    let g = golden();
    for c in g["nodes"].as_array().unwrap() {
        let meta = c["metadata"].as_object().unwrap().clone();
        let nodes = build_document_nodes("7", s(&c["text"]), &meta, c["chunk_size"].as_u64().unwrap() as usize, |i| format!("7-node-{i}")).unwrap();
        let exp = c["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), exp.len(), "node count");
        for (n, e) in nodes.iter().zip(exp) {
            assert_eq!(n.text, s(&e["text"]));
            assert_eq!(n.llm_content(), s(&e["llm_content"]));
            assert_eq!(n.embed_content(), s(&e["embed_content"]));
            assert_eq!(n.node_content(), s(&e["node_content"]));
            let back = TextNode::from_node_content(s(&e["node_content"])).unwrap();
            assert_eq!(&back, n);
        }
    }
}

#[test]
fn pickle_like_cpython() {
    let g = golden();
    for k in ["pickle", "pickle_empty"] {
        let p = &g[k];
        let exp: Vec<u8> = (0..s(&p["hex"]).len() / 2).map(|i| u8::from_str_radix(&s(&p["hex"])[i * 2..i * 2 + 2], 16).unwrap()).collect();
        let got = pickle::dumps_suggestion_cache(1002, s(&p["backend"]), &p["suggestions"]);
        assert_eq!(got, exp, "{k}");
        match pickle::loads(&exp).unwrap() {
            pickle::Loaded::Suggestion { version, hash, suggestions } => {
                assert_eq!(version, 1002);
                assert_eq!(hash, s(&p["backend"]));
                assert_eq!(suggestions, p["suggestions"]);
            }
            other => panic!("{other:?}"),
        }
    }
}

/// tests/parity/pl-ai/mock_llm.py::embed
fn mock_embed(text: &str) -> Vec<f32> {
    let re = regex::Regex::new(r"\w+").unwrap();
    let mut v = [0f64; 8];
    for w in re.find_iter(&text.to_lowercase()) {
        let h = sha2::Sha256::digest(w.as_str().as_bytes());
        v[(h[0] % 8) as usize] += 1.0 + f64::from(h[1] % 4) / 4.0;
    }
    if v.iter().all(|x| *x == 0.0) {
        v[0] = 1.0;
    }
    v.iter().map(|x| *x as f32).collect()
}

fn copy_db() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("plai-w8-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/llmindex_py.db"), d.join("llmindex.db")).unwrap();
    d
}

#[test]
fn reads_python_index_and_retrieves_like_upstream() {
    let g = golden();
    let d = copy_db();
    let st = VecStore::open(&d, None).unwrap();
    assert!(st.table_exists().unwrap());
    assert_eq!(st.vector_dim().unwrap(), Some(8));
    assert_eq!(st.stored_model_name().unwrap().as_deref(), Some("embeddinggemma"));
    let mt = st.get_modified_times().unwrap();
    assert_eq!(mt.get("3").map(String::as_str), Some("2025-01-03T08:00:00+00:00"));
    for q in g["retrieval"].as_array().unwrap() {
        let filters: Vec<DocFilter> = match q["filter"].as_array() {
            None => vec![],
            Some(a) => vec![DocFilter::In(a.iter().map(|x| s(x).parse().unwrap()).collect())],
        };
        let hits = st.query(&mock_embed(s(&q["query"])), 5, &filters).unwrap();
        let exp = q["hits"].as_array().unwrap();
        assert_eq!(hits.len(), exp.len(), "{}", q["query"]);
        for (h, e) in hits.iter().zip(exp) {
            assert_eq!(h.node.id, s(&e[0]));
            assert!((h.score - e[1].as_f64().unwrap()).abs() < 1e-12, "{} {}", h.score, e[1]);
        }
    }
    drop(st);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn writes_index_python_can_share() {
    // Rust writes nodes into the same file and reads them back; compaction keeps everything.
    let d = copy_db();
    let mut st = VecStore::open(&d, Some("embeddinggemma")).unwrap();
    let meta: serde_json::Map<String, Value> = serde_json::from_str(
        r#"{"document_id":"9","title":"Neu","tags":[],"correspondent":null,"document_type":null,"filename":"n.pdf","storage_path":null,"archive_serial_number":null,"created":"2025-01-01","added":"2025-01-01T00:00:00+00:00","modified":"2025-01-09T00:00:00+00:00"}"#,
    )
    .unwrap();
    let nodes = build_document_nodes("9", "Notes: \n\nContent:\n\nNeues Dokument über Steuern.", &meta, 1024, |i| format!("9-{i}")).unwrap();
    let rows: Vec<_> = nodes.into_iter().map(|n| pl_ai::vector_store::Row { embedding: mock_embed(&n.embed_content()), node: n }).collect();
    st.upsert_document(9, &rows).unwrap();
    st.upsert_document(9, &rows).unwrap();
    let hits = st.query(&mock_embed("Notes: Content: Neues Dokument über Steuern."), 1, &[DocFilter::In(vec![9])]).unwrap();
    assert_eq!(hits[0].node.id, "9-0");
    assert!((hits[0].score - 1.0).abs() < 1e-6);
    let live = st.live_rows().unwrap();
    assert!(st.compact(true).unwrap());
    assert_eq!(st.live_rows().unwrap(), live);
    st.delete(9).unwrap();
    assert!(st.query(&mock_embed("x"), 3, &[DocFilter::Eq(9)]).unwrap().is_empty());
    drop(st);
    let _ = std::fs::remove_dir_all(&d);
}

struct FakeLlm {
    sys: String,
    prompts: Mutex<Vec<String>>,
}

impl StreamLlm for FakeLlm {
    fn system_prompt(&self) -> &str {
        &self.sys
    }
    fn stream<'a>(
        &'a self,
        prompt: &'a str,
    ) -> futures::future::BoxFuture<'a, Result<futures::stream::BoxStream<'static, Result<String, LlmError>>, LlmError>> {
        Box::pin(async move {
            self.prompts.lock().unwrap().push(prompt.to_string());
            // mock_llm.answer_text via "\n".join(contents)
            let joined = format!("{}\n{}", self.sys, prompt);
            let h = sha2::Sha256::digest(joined.as_bytes());
            let hex: String = h.iter().map(|b| format!("{b:02x}")).collect();
            let text = format!("Antwort {}: Die Dokumente nennen {} Zeichen. Ende.", &hex[..6], joined.chars().count());
            let re = regex::Regex::new(r"\S+\s*").unwrap();
            let parts: Vec<Result<String, LlmError>> = re.find_iter(&text).map(|m| Ok(m.as_str().to_string())).collect();
            Ok(futures::StreamExt::boxed(futures::stream::iter(parts)))
        })
    }
}

#[tokio::test]
async fn chat_synthesis_like_llama_index() {
    use futures::StreamExt;
    let g = golden();
    let d = copy_db();
    let st = VecStore::open(&d, None).unwrap();
    let all = st.query(&mock_embed("x"), 1000, &[]).unwrap();
    let by_id: BTreeMap<String, TextNode> = all.into_iter().map(|h| (h.node.id.clone(), h.node)).collect();
    for c in g["chat_flow"].as_array().unwrap() {
        let q = s(&c["query"]);
        let lang = c["lang"].as_str();
        // Retrieval like upstream (NIN [] = everything)
        let hits = st.query(&mock_embed(q), 5, &[DocFilter::NotIn(vec![])]).unwrap();
        assert_eq!(hits.iter().map(|h| h.node.id.clone()).collect::<Vec<_>>(), strs(&c["top"]));
        let chunks: Vec<String> = hits.iter().map(|h| by_id[&h.node.id].llm_content()).collect();
        let llm = FakeLlm { sys: "SYS".into(), prompts: Mutex::new(Vec::new()) };
        let helper = PromptHelper::rag(c["chunk_size"].as_u64().unwrap() as usize, c["context_size"].as_u64().unwrap() as usize);
        let stream = compact_and_refine(&llm, &helper, &prompts::chat_qa(lang), &prompts::chat_refine(lang), q, &chunks)
            .await
            .unwrap()
            .unwrap();
        let out: Vec<String> = stream.map(|r| r.unwrap()).collect().await;
        let mut exp_stream = strs(&c["stream"]);
        exp_stream.retain(|x| !x.is_empty());
        assert_eq!(out, exp_stream);
        let exp_prompts: Vec<String> = c["requests"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["body"].get("messages").is_some())
            .map(|r| s(&r["body"]["messages"][1]["content"]).to_string())
            .collect();
        assert_eq!(*llm.prompts.lock().unwrap(), exp_prompts, "{} {}", c["backend"], q);
    }
    drop(st);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn wire_formats_like_llama_index() {
    let g = golden();
    let w = &g["wire"];
    let mut env = std::collections::HashMap::new();
    env.insert("PAPERLESS_AI_ENABLED".to_string(), "true".to_string());
    env.insert("PAPERLESS_AI_LLM_BACKEND".to_string(), "ollama".to_string());
    env.insert("PAPERLESS_AI_LLM_ENDPOINT".to_string(), "http://127.0.0.1:PORT".to_string());
    env.insert("PAPERLESS_AI_LLM_EXTRA_PARAMS".to_string(), r#"{"seed": 1}"#.to_string());
    env.insert("PAPERLESS_AI_LLM_EMBEDDING_BACKEND".to_string(), "ollama".to_string());
    let cfg = pl_ai::config::AiConfig::from_env_map(&env).unwrap();
    let e = pl_ai::embedding::Embedder::from_config(&cfg).unwrap();
    let reqs = w["ollama_embed"]["requests"].as_array().unwrap();
    assert_eq!(e.request(&["eins zwei".into(), "drei".into()], false).1, reqs[0]["body"]);
    assert_eq!(e.request(&["frage eins".into()], true).1, reqs[1]["body"]);
    let llm = pl_ai::llm::LlmClient::from_config(&cfg).unwrap();
    let (url, mut body) = llm.stream_request_body("Q: hallo");
    assert_eq!(url, "http://127.0.0.1:PORT/api/chat");
    let exp = &w["ollama_stream"]["requests"][0]["body"];
    body["messages"][0]["content"] = Value::from("SYSTEM PROMPT");
    assert_eq!(&body, exp);

    env.insert("PAPERLESS_AI_LLM_BACKEND".to_string(), "openai-like".to_string());
    env.insert("PAPERLESS_AI_LLM_ENDPOINT".to_string(), "http://127.0.0.1:PORT/v1".to_string());
    env.insert("PAPERLESS_AI_LLM_EMBEDDING_BACKEND".to_string(), "openai-like".to_string());
    env.insert("PAPERLESS_AI_LLM_EMBEDDING_API_KEY".to_string(), "k".to_string());
    let cfg = pl_ai::config::AiConfig::from_env_map(&env).unwrap();
    let e = pl_ai::embedding::Embedder::from_config(&cfg).unwrap();
    let reqs = w["openai_embed"]["requests"].as_array().unwrap();
    assert_eq!(e.request(&["eins zwei".into(), "drei".into()], false).1, reqs[0]["body"]);
    assert_eq!(e.request(&["frage eins".into()], true).1, reqs[1]["body"]);
    let llm = pl_ai::llm::LlmClient::from_config(&cfg).unwrap();
    let (_, mut body) = llm.stream_request_body("Q: hallo");
    body["messages"][0]["content"] = Value::from("SYSTEM PROMPT");
    assert_eq!(&body, &w["openai_stream"]["requests"][0]["body"]);
}
