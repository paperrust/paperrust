//! Answer synthesis for the chat – llama_index 0.14.25 `get_response_synthesizer(...,
//! streaming=True)` = `CompactAndRefine` (`response_synthesizers/compact_and_refine.py`,
//! `refine.py::_run_refine_loop`), as upstream `paperless_ai/chat.py` uses it.
//!
//! Flow (byte-exact against the capture in `testdata/golden_w8.json` → `chat_flow`):
//! 1. `text_chunks` = `node.get_content(MetadataMode.LLM)` of the retrieved nodes.
//! 2. Compact: `repack(biggest(qa(query), refine(query)), chunks, padding=500)`.
//! 3. Refine loop: each chunk once more `repack(biggest(qa, refine), [chunk], padding=500)`
//!    (templates **without** `query_str`); first chunk with `qa`, each further one with `refine`
//!    (`existing_answer` = completely collected previous answer) after another `repack(…, padding=5)`;
//!    if it does not fit, the parts come to the front **reversed** (`deque.extendleft`).
//! 4. Every step is a streaming call; only the **last** one is passed through to the client.
//!
//! Token counting of the prompt: `get_empty_prompt_txt` (missing variables empty) + system prompt of the
//! LLM. Template substitution like `SafeFormatter` (`{{`/`}}` are **not** unescaped).

use std::collections::VecDeque;

use crate::llm::{LLM_SYSTEM_PROMPT, LlmClient, LlmError};
use crate::tokenizer::{PromptHelper, SplitError, count_tokens};

/// llama_index `DEFAULT_RESPONSE_PADDING_SIZE`.
pub const RESPONSE_PADDING_SIZE: usize = 500;
/// llama_index `DEFAULT_PADDING`.
pub const DEFAULT_PADDING: usize = 5;

/// llama_index `SafeFormatter.format`: `re.sub(r"\{([^{}]+)\}", …)`, unknown keys stay.
pub fn safe_format(template: &str, vars: &[(&str, &str)]) -> String {
    let c: Vec<char> = template.chars().collect();
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    while i < c.len() {
        if c[i] == '{' {
            // longest run without { } up to the next }
            let mut j = i + 1;
            while j < c.len() && c[j] != '{' && c[j] != '}' {
                j += 1;
            }
            if j < c.len() && c[j] == '}' && j > i + 1 {
                let key: String = c[i + 1..j].iter().collect();
                match vars.iter().find(|(k, _)| *k == key) {
                    Some((_, v)) => out.push_str(v),
                    None => out.extend(&c[i..=j]),
                }
                i = j + 1;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// `get_template_vars` (`\{([a-zA-Z_][a-zA-Z0-9_]*(?:\.[a-zA-Z_][a-zA-Z0-9_]*)*)\}`).
pub fn template_vars(template: &str) -> Vec<String> {
    static R: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    let Some(r) = R.get_or_init(|| regex::Regex::new(r"\{([a-zA-Z_][a-zA-Z0-9_]*(?:\.[a-zA-Z_][a-zA-Z0-9_]*)*)\}").ok()) else {
        return Vec::new();
    };
    r.captures_iter(template).filter_map(|m| m.get(1).map(|x| x.as_str().to_string())).collect()
}

/// Template with partial binding (`PromptTemplate.partial_format`).
#[derive(Debug, Clone)]
pub struct Prompt {
    pub template: String,
    pub kwargs: Vec<(String, String)>,
}

impl Prompt {
    pub fn new(t: &str) -> Self {
        Self { template: t.to_string(), kwargs: Vec::new() }
    }
    pub fn partial(&self, kv: &[(&str, &str)]) -> Self {
        let mut p = self.clone();
        for (k, v) in kv {
            p.kwargs.retain(|(a, _)| a != k);
            p.kwargs.push((k.to_string(), v.to_string()));
        }
        p
    }
    pub fn format(&self, kv: &[(&str, &str)]) -> String {
        let mut all: Vec<(&str, &str)> = self.kwargs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        for (k, v) in kv {
            all.retain(|(a, _)| a != k);
            all.push((k, v));
        }
        safe_format(&self.template, &all)
    }
    /// `get_empty_prompt_txt`.
    pub fn empty_text(&self) -> String {
        let vars = template_vars(&self.template);
        let mut all: Vec<(String, String)> = self.kwargs.clone();
        for v in vars {
            if !all.iter().any(|(k, _)| *k == v) {
                all.push((v, String::new()));
            }
        }
        let refs: Vec<(&str, &str)> = all.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        safe_format(&self.template, &refs)
    }
}

/// `get_biggest_prompt` (by character count of the empty prompt, on a tie the first).
fn biggest<'a>(a: &'a Prompt, b: &'a Prompt) -> &'a Prompt {
    if b.empty_text().chars().count() > a.empty_text().chars().count() { b } else { a }
}

/// Synthesis error.
#[derive(Debug, thiserror::Error)]
pub enum SynthError {
    #[error(transparent)]
    Llm(#[from] LlmError),
    #[error(transparent)]
    Split(#[from] SplitError),
}

/// Sends a prompt (streaming call); `final_step` → the stream is returned.
pub trait StreamLlm: Send + Sync {
    fn system_prompt(&self) -> &str;
    fn stream<'a>(
        &'a self,
        prompt: &'a str,
    ) -> futures::future::BoxFuture<'a, Result<futures::stream::BoxStream<'static, Result<String, LlmError>>, LlmError>>;
}

impl StreamLlm for LlmClient {
    fn system_prompt(&self) -> &str {
        self.system_text.as_deref().unwrap_or(LLM_SYSTEM_PROMPT)
    }
    fn stream<'a>(
        &'a self,
        prompt: &'a str,
    ) -> futures::future::BoxFuture<'a, Result<futures::stream::BoxStream<'static, Result<String, LlmError>>, LlmError>> {
        Box::pin(self.stream_chat(prompt))
    }
}

async fn collect(s: futures::stream::BoxStream<'static, Result<String, LlmError>>) -> Result<String, LlmError> {
    use futures::StreamExt;
    let mut s = s;
    let mut out = String::new();
    while let Some(d) = s.next().await {
        out.push_str(&d?);
    }
    Ok(out)
}

/// `CompactAndRefine.get_response(query, text_chunks)` with `streaming=True`: returns the
/// stream of the last LLM call. No chunks → `None` (llama_index: "Empty Response").
pub async fn compact_and_refine(
    llm: &dyn StreamLlm,
    helper: &PromptHelper,
    qa_template: &str,
    refine_template: &str,
    query: &str,
    text_chunks: &[String],
) -> Result<Option<futures::stream::BoxStream<'static, Result<String, LlmError>>>, SynthError> {
    let sys_tokens = count_tokens(llm.system_prompt());
    let qa = Prompt::new(qa_template);
    let refine = Prompt::new(refine_template);
    // CompactAndRefine._make_compact_text_chunks
    let qa_q = qa.partial(&[("query_str", query)]);
    let refine_q = refine.partial(&[("query_str", query)]);
    let max_q = biggest(&qa_q, &refine_q);
    let new_texts = helper.repack(count_tokens(&max_q.empty_text()) + sys_tokens, text_chunks, RESPONSE_PADDING_SIZE)?;
    // Refine._run_refine_loop
    let max_plain = biggest(&qa, &refine);
    let max_tokens = count_tokens(&max_plain.empty_text()) + sys_tokens;
    let mut dq: VecDeque<String> = VecDeque::new();
    for c in &new_texts {
        for t in helper.repack(max_tokens, std::slice::from_ref(c), RESPONSE_PADDING_SIZE)? {
            dq.push_back(t);
        }
    }
    let mut response: Option<String> = None;
    let mut pending: Option<futures::stream::BoxStream<'static, Result<String, LlmError>>> = None;
    while let Some(chunk) = dq.pop_front() {
        if let Some(p) = pending.take() {
            response = Some(collect(p).await?);
        }
        let prompt = match &response {
            None => qa_q.format(&[("context_str", &chunk)]),
            Some(prev) => {
                let rp = refine_q.partial(&[("existing_answer", prev)]);
                let repacked = helper.repack(count_tokens(&rp.empty_text()) + sys_tokens, std::slice::from_ref(&chunk), DEFAULT_PADDING)?;
                if repacked.len() > 1 {
                    for r in repacked {
                        dq.push_front(r); // extendleft: reversed order
                    }
                    // `response` stays (generator was already collected)
                    continue;
                }
                let c = repacked.into_iter().next().unwrap_or_default();
                rp.format(&[("context_msg", &c)])
            }
        };
        pending = Some(llm.stream(&prompt).await?);
    }
    Ok(pending)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_formatter() {
        assert_eq!(safe_format("a {x} {{y}} {z}", &[("x", "{1}"), ("y", "Y")]), "a {1} {Y} {z}");
        assert_eq!(template_vars("Q: {query_str} {{lang}} {a.b} {1x}"), vec!["query_str", "lang", "a.b"]);
        let p = Prompt::new("C:{context_str} Q:{query_str}").partial(&[("query_str", "q")]);
        assert_eq!(p.empty_text(), "C: Q:q");
        assert_eq!(p.format(&[("context_str", "c")]), "C:c Q:q");
    }
}
