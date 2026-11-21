//! Tokenizer and text splitter like llama_index 0.14.25 (upstream `paperless_ai/indexing.py`
//! `truncate_content`, `truncate_embedding_query`, `get_rag_prompt_helper`, `build_document_node`).
//!
//! * Tokenizer: llama_index `get_tokenizer()` = `tiktoken.encoding_for_model("gpt-3.5-turbo")`
//!   (= `cl100k_base`) with `allowed_special="all"` → [`count_tokens`] (tiktoken-rs, vocabulary embedded).
//! * [`TokenTextSplitter`]: `llama_index/core/node_parser/text/token.py` (`_split`, `_merge`).
//! * [`PromptHelper`]: `llama_index/core/indices/prompt_helper.py` (`truncate`, `repack`,
//!   `_get_available_chunk_size`; text templates only, no ChatPromptTemplate).
//! * [`SentenceSplitter`] (= `SimpleNodeParser`): `node_parser/text/sentence.py` with the
//!   untrained nltk `PunktSentenceTokenizer` ([`crate::punkt`]).
//!
//! All lengths/indexes are Python code points; `strip()` is Python `str.strip()`.

use crate::compat::py_isspace;

/// `len(tokenizer(text))` (cl100k_base, special tokens allowed).
pub fn count_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    tiktoken_rs::cl100k_base_singleton().encode_with_special_tokens(text).len()
}

/// Python `str.strip()`.
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(py_isspace)
}

/// llama_index `split_text_keep_separator(text, sep)` (separator at the **start** of the following chunks).
pub fn split_keep_sep(text: &str, sep: &str) -> Vec<String> {
    let parts: Vec<&str> = text.split(sep).collect();
    parts.iter().enumerate().map(|(i, p)| if i > 0 { format!("{sep}{p}") } else { (*p).to_string() }).filter(|s| !s.is_empty()).collect()
}

/// `list(text)`.
pub fn split_chars(text: &str) -> Vec<String> {
    text.chars().map(|c| c.to_string()).collect()
}

// ------------------------------------------------------------------ TokenTextSplitter

/// llama_index `TokenTextSplitter(separator, chunk_size, chunk_overlap, backup_separators=["\n"])`.
#[derive(Debug, Clone)]
pub struct TokenTextSplitter {
    pub chunk_size: usize,
    pub chunk_overlap: usize,
    pub separator: String,
    pub backup_separators: Vec<String>,
}

impl TokenTextSplitter {
    pub fn new(separator: &str, chunk_size: usize, chunk_overlap: usize) -> Self {
        Self { chunk_size, chunk_overlap, separator: separator.to_string(), backup_separators: vec!["\n".to_string()] }
    }

    fn split_fns(&self, text: &str) -> Vec<String> {
        let mut seps = vec![self.separator.clone()];
        seps.extend(self.backup_separators.iter().cloned());
        let mut splits: Vec<String>;
        for sep in &seps {
            splits = split_keep_sep(text, sep);
            if splits.len() > 1 {
                return splits;
            }
        }
        splits = Vec::new();
        // Python: without a match `splits` stays the result of the last function (split_by_char)
        let _ = splits;
        split_chars(text)
    }

    fn split(&self, text: &str, chunk_size: usize) -> Vec<String> {
        if count_tokens(text) <= chunk_size {
            return vec![text.to_string()];
        }
        let splits = self.split_fns(text);
        let mut out = Vec::new();
        let n = splits.len();
        for s in splits {
            let l = count_tokens(&s);
            if l <= chunk_size || n == 1 {
                out.push(s);
            } else {
                out.extend(self.split(&s, chunk_size));
            }
        }
        out
    }

    fn merge(&self, splits: &[String], chunk_size: usize) -> Vec<String> {
        let mut chunks: Vec<String> = Vec::new();
        let mut cur: std::collections::VecDeque<(String, usize)> = std::collections::VecDeque::new();
        let mut cur_len = 0usize;
        for s in splits {
            let l = count_tokens(s);
            if cur_len + l > chunk_size {
                let joined: String = cur.iter().map(|(t, _)| t.as_str()).collect();
                let c = py_strip(&joined);
                if !c.is_empty() {
                    chunks.push(c.to_string());
                }
                while !cur.is_empty() && (cur_len > self.chunk_overlap || cur_len + l > chunk_size) {
                    if let Some((_, fl)) = cur.pop_front() {
                        cur_len -= fl;
                    }
                }
            }
            cur.push_back((s.clone(), l));
            cur_len += l;
        }
        let joined: String = cur.iter().map(|(t, _)| t.as_str()).collect();
        let c = py_strip(&joined);
        if !c.is_empty() {
            chunks.push(c.to_string());
        }
        chunks
    }

    /// `split_text(text)`.
    pub fn split_text(&self, text: &str) -> Vec<String> {
        if text.is_empty() {
            return vec![String::new()];
        }
        let splits = self.split(text, self.chunk_size);
        self.merge(&splits, self.chunk_size)
    }
}

// ------------------------------------------------------------------ PromptHelper

/// Errors like upstream (`ValueError` → "Invalid AI configuration." in the views).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SplitError(pub String);

/// llama_index `PromptHelper` (text templates).
#[derive(Debug, Clone)]
pub struct PromptHelper {
    pub context_window: usize,
    pub num_output: usize,
    pub chunk_overlap_ratio: f64,
    pub chunk_size_limit: Option<usize>,
    pub separator: String,
}

/// `indexing.RAG_NUM_OUTPUT`.
pub const RAG_NUM_OUTPUT: usize = 512;
/// `indexing.RAG_CHUNK_OVERLAP`.
pub const RAG_CHUNK_OVERLAP: usize = 200;
/// llama_index `DEFAULT_PADDING`.
pub const DEFAULT_PADDING: usize = 5;

/// upstream `get_rag_chunk_overlap(chunk_size)` = `min(200, chunk_size - 1)`.
pub fn rag_chunk_overlap(chunk_size: usize) -> usize {
    RAG_CHUNK_OVERLAP.min(chunk_size.saturating_sub(1))
}

impl PromptHelper {
    /// upstream `get_rag_prompt_helper(chunk_size, context_size)`.
    pub fn rag(chunk_size: usize, context_size: usize) -> Self {
        Self {
            context_window: context_size,
            num_output: RAG_NUM_OUTPUT,
            chunk_overlap_ratio: 0.1,
            chunk_size_limit: Some(chunk_size),
            separator: " ".into(),
        }
    }

    /// `_get_available_chunk_size(prompt, num_chunks, padding, llm)`; `prompt_tokens` = Tokens
    /// of the empty prompt (`get_empty_prompt_txt`) + system prompt of the LLM.
    pub fn available_chunk_size(&self, prompt_tokens: usize, num_chunks: usize, padding: usize) -> Result<i64, SplitError> {
        let avail = self.context_window as i64 - prompt_tokens as i64 - self.num_output as i64;
        if avail < 0 {
            return Err(SplitError(format!("Calculated available context size {avail} was not non-negative.")));
        }
        // Python `//` (floor) – avail ≥ 0, num_chunks ≥ 1
        let mut r = avail / num_chunks.max(1) as i64 - padding as i64;
        if let Some(l) = self.chunk_size_limit {
            r = r.min(l as i64);
        }
        Ok(r)
    }

    /// `get_text_splitter_given_prompt`.
    pub fn splitter(&self, prompt_tokens: usize, num_chunks: usize, padding: usize) -> Result<TokenTextSplitter, SplitError> {
        let cs = self.available_chunk_size(prompt_tokens, num_chunks, padding)?;
        if cs <= 0 {
            return Err(SplitError(format!("Chunk size {cs} is not positive.")));
        }
        let cs = cs as usize;
        let overlap = (self.chunk_overlap_ratio * cs as f64) as usize;
        let mut s = TokenTextSplitter::new(&self.separator, cs, overlap);
        // PromptHelper creates TokenTextSplitter without a backup_separators argument → default ["\n"]
        s.backup_separators = vec!["\n".into()];
        Ok(s)
    }

    /// `truncate(prompt, text_chunks, padding)`.
    pub fn truncate(&self, prompt_tokens: usize, chunks: &[String], padding: usize) -> Result<Vec<String>, SplitError> {
        if chunks.is_empty() {
            return Ok(Vec::new());
        }
        let sp = self.splitter(prompt_tokens, chunks.len(), padding)?;
        Ok(chunks.iter().map(|c| sp.split_text(c).into_iter().next().unwrap_or_default()).collect())
    }

    /// `repack(prompt, text_chunks, padding)`.
    pub fn repack(&self, prompt_tokens: usize, chunks: &[String], padding: usize) -> Result<Vec<String>, SplitError> {
        let sp = self.splitter(prompt_tokens, 1, padding)?;
        let combined = chunks.iter().map(|c| py_strip(c)).filter(|c| !c.is_empty()).collect::<Vec<_>>().join("\n\n");
        Ok(sp.split_text(&combined))
    }
}

/// upstream `indexing.truncate_content(content, chunk_size, context_size)`.
pub fn truncate_content(content: &str, chunk_size: usize, context_size: usize) -> Result<String, SplitError> {
    let ph = PromptHelper::rag(chunk_size, context_size);
    let splitter = TokenTextSplitter::new(" ", chunk_size, rag_chunk_overlap(chunk_size));
    let chunks = splitter.split_text(content);
    // PromptTemplate("{content}") → empty prompt "" → 0 tokens, no LLM
    let t = ph.truncate(0, &chunks, DEFAULT_PADDING)?;
    Ok(t.join(" "))
}

/// upstream `indexing.truncate_embedding_query(content, chunk_size)`.
pub fn truncate_embedding_query(content: &str, chunk_size: usize) -> String {
    let splitter = TokenTextSplitter::new(" ", chunk_size, 0);
    splitter.split_text(content).into_iter().next().unwrap_or_default()
}

/// [`crate::classifier::Truncator`] with tiktoken (upstream-identical).
pub struct TiktokenTruncator;

impl crate::classifier::Truncator for TiktokenTruncator {
    fn truncate_content(&self, content: &str, chunk_size: usize, context_size: usize) -> String {
        match truncate_content(content, chunk_size, context_size) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(target: "pl_ai", error = %e, "truncate_content failed; content passed unchanged");
                content.to_string()
            }
        }
    }
}

// ------------------------------------------------------------------ SentenceSplitter

/// llama_index `CHUNKING_REGEX`.
pub const CHUNKING_REGEX: &str = "[^,.;。？！]+[,.;。？！]?|[,.;。？！]";
/// llama_index `DEFAULT_PARAGRAPH_SEP`.
pub const DEFAULT_PARAGRAPH_SEP: &str = "\n\n\n";

#[derive(Debug, Clone)]
struct Split {
    text: String,
    is_sentence: bool,
    token_size: usize,
}

/// llama_index `SentenceSplitter(chunk_size, chunk_overlap)` (= `SimpleNodeParser`).
#[derive(Debug, Clone)]
pub struct SentenceSplitter {
    pub chunk_size: usize,
    pub chunk_overlap: usize,
    pub separator: String,
    pub paragraph_separator: String,
    chunk_re: regex::Regex,
}

impl SentenceSplitter {
    pub fn new(chunk_size: usize, chunk_overlap: usize) -> Result<Self, SplitError> {
        if chunk_overlap > chunk_size {
            return Err(SplitError(format!(
                "Got a larger chunk overlap ({chunk_overlap}) than chunk size ({chunk_size}), should be smaller."
            )));
        }
        let chunk_re = regex::Regex::new(CHUNKING_REGEX).map_err(|e| SplitError(e.to_string()))?;
        Ok(Self { chunk_size, chunk_overlap, separator: " ".into(), paragraph_separator: DEFAULT_PARAGRAPH_SEP.into(), chunk_re })
    }

    /// upstream `build_document_node`: `SimpleNodeParser(chunk_size, chunk_overlap=min(200, chunk_size-1))`.
    pub fn for_index(chunk_size: usize) -> Result<Self, SplitError> {
        Self::new(chunk_size, rag_chunk_overlap(chunk_size))
    }

    fn splits_by_fns(&self, text: &str) -> (Vec<String>, bool) {
        let s = split_keep_sep(text, &self.paragraph_separator);
        if s.len() > 1 {
            return (s, true);
        }
        let s = crate::punkt::split_sentences(text);
        if s.len() > 1 {
            return (s, true);
        }
        let mut splits: Vec<String> = self.chunk_re.find_iter(text).map(|m| m.as_str().to_string()).collect();
        if splits.len() > 1 {
            return (splits, false);
        }
        splits = split_keep_sep(text, &self.separator);
        if splits.len() > 1 {
            return (splits, false);
        }
        (split_chars(text), false)
    }

    fn split(&self, text: &str, chunk_size: usize) -> Vec<Split> {
        let ts = count_tokens(text);
        if ts <= chunk_size {
            return vec![Split { text: text.to_string(), is_sentence: true, token_size: ts }];
        }
        let (parts, is_sentence) = self.splits_by_fns(text);
        let n = parts.len();
        let mut out = Vec::new();
        for p in parts {
            let ts = count_tokens(&p);
            if ts <= chunk_size || n == 1 {
                out.push(Split { text: p, is_sentence, token_size: ts });
            } else {
                out.extend(self.split(&p, chunk_size));
            }
        }
        out
    }

    fn merge(&self, splits: &[Split], chunk_size: usize) -> Result<Vec<String>, SplitError> {
        let mut chunks: Vec<String> = Vec::new();
        let mut cur: Vec<(String, usize)> = Vec::new();
        let mut cur_len = 0usize;
        let mut new_chunk = true;
        let close = |chunks: &mut Vec<String>, cur: &mut Vec<(String, usize)>, cur_len: &mut usize, new_chunk: &mut bool| {
            chunks.push(cur.iter().map(|(t, _)| t.as_str()).collect());
            let last = std::mem::take(cur);
            *cur_len = 0;
            *new_chunk = true;
            let mut i = last.len();
            while i > 0 && *cur_len + last[i - 1].1 <= self.chunk_overlap {
                *cur_len += last[i - 1].1;
                cur.insert(0, last[i - 1].clone());
                i -= 1;
            }
        };
        let mut idx = 0;
        while idx < splits.len() {
            let s = &splits[idx];
            if s.token_size > chunk_size {
                return Err(SplitError("Single token exceeded chunk size".into()));
            }
            if cur_len + s.token_size > chunk_size && !new_chunk {
                close(&mut chunks, &mut cur, &mut cur_len, &mut new_chunk);
            } else {
                if new_chunk && cur_len + s.token_size > chunk_size {
                    while !cur.is_empty() && cur_len + s.token_size > chunk_size {
                        let (_, l) = cur.remove(0);
                        cur_len -= l;
                    }
                }
                if s.is_sentence || cur_len + s.token_size <= chunk_size || new_chunk {
                    cur_len += s.token_size;
                    cur.push((s.text.clone(), s.token_size));
                    idx += 1;
                    new_chunk = false;
                } else {
                    close(&mut chunks, &mut cur, &mut cur_len, &mut new_chunk);
                }
            }
        }
        if !new_chunk {
            chunks.push(cur.iter().map(|(t, _)| t.as_str()).collect());
        }
        Ok(chunks.iter().map(|c| py_strip(c)).filter(|c| !c.is_empty()).map(str::to_string).collect())
    }

    /// `split_text(text)`.
    pub fn split_text(&self, text: &str) -> Result<Vec<String>, SplitError> {
        self.split_text_with_size(text, self.chunk_size)
    }

    /// `split_text_metadata_aware(text, metadata_str)`: chunk size minus metadata tokens.
    pub fn split_text_metadata_aware(&self, text: &str, metadata_str: &str) -> Result<Vec<String>, SplitError> {
        let ml = count_tokens(metadata_str);
        if ml >= self.chunk_size {
            return Err(SplitError(format!(
                "Metadata length ({ml}) is longer than chunk size ({}). Consider increasing the chunk size or decreasing the size of your metadata to avoid this.",
                self.chunk_size
            )));
        }
        self.split_text_with_size(text, self.chunk_size - ml)
    }

    fn split_text_with_size(&self, text: &str, chunk_size: usize) -> Result<Vec<String>, SplitError> {
        if text.is_empty() {
            return Ok(vec![String::new()]);
        }
        let splits = self.split(text, chunk_size);
        self.merge(&splits, chunk_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        assert_eq!(count_tokens(""), 0);
        assert_eq!(count_tokens("hello world"), 2);
        assert_eq!(count_tokens("<|endoftext|>"), 1, "allowed_special=all");
    }

    #[test]
    fn keep_sep() {
        assert_eq!(split_keep_sep("a b  c", " "), vec!["a", " b", " ", " c"]);
        assert_eq!(split_keep_sep(" a", " "), vec![" a"]);
    }

    #[test]
    fn token_splitter_small() {
        let s = TokenTextSplitter::new(" ", 3, 1);
        assert_eq!(s.split_text(""), vec![""]);
        assert_eq!(s.split_text("one two"), vec!["one two"]);
        assert_eq!(s.split_text("one two three four five"), vec!["one two three", "three four five"]);
    }

    #[test]
    fn helper_errors() {
        let ph = PromptHelper::rag(1024, 400);
        assert!(ph.available_chunk_size(0, 1, 5).is_err());
    }
}
