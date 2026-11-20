//! Index nodes in the format of llama_index 0.14.25 (`TextNode`, `node_to_metadata_dict`,
//! `metadata_dict_to_node`), so that the vector index is interchangeable between upstream (Python) and
//! PaperRust.
//!
//! upstream: `paperless_ai/indexing.py::build_document_node` (LlamaDocument with
//! `excluded_embed_metadata_keys = all`, `excluded_llm_metadata_keys = ["document_id"]`,
//! `SimpleNodeParser(chunk_size, chunk_overlap=min(200, chunk_size-1))`), llama_index
//! `node_parser/interface.py` (`MetadataAwareTextSplitter._parse_nodes`, `_postprocess_parsed_nodes`),
//! `node_parser/node_utils.py::build_nodes_from_splits`, `schema.py` (`TextNode.hash`,
//! `Node.hash`, `MediaResource.hash`, `get_content`, `get_metadata_str`),
//! `vector_stores/utils.py::node_to_metadata_dict`.

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::pyjson;
use crate::tokenizer::{SentenceSplitter, SplitError};

/// Python `repr(str)` (quote choice, escapes, non-printable characters).
pub fn py_repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(quote);
    for c in s.chars() {
        let u = c as u32;
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            _ if u < 0x20 || u == 0x7f || (0x80..0xa1).contains(&u) || u == 0xad => out.push_str(&format!("\\x{u:02x}")),
            // Zs (except space), Zl, Zp and common Cf are not printable in Python
            _ if matches!(u, 0x1680 | 0x2000..=0x200f | 0x2028..=0x202f | 0x205f..=0x2064 | 0x2066..=0x206f | 0x3000 | 0xfeff | 0xfff9..=0xfffb | 0x061c | 0x180e) => {
                out.push_str(&format!("\\u{u:04x}"))
            }
            _ if (0xe000..=0xf8ff).contains(&u) || u >= 0xf0000 => {
                if u > 0xffff { out.push_str(&format!("\\U{u:08x}")) } else { out.push_str(&format!("\\u{u:04x}")) }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `str(value)` of a JSON value (str → unchanged, otherwise `repr`).
pub fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

/// Python `repr(value)`.
pub fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => py_repr_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter().map(|(k, v)| format!("{}: {}", py_repr_str(k), py_repr(v))).collect::<Vec<_>>().join(", ")
        ),
    }
}

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let d = h.finalize();
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// `metadata_template.format(key=, value=str(v))` joined with `metadata_separator`.
pub fn metadata_str(meta: &Map<String, Value>, excluded: &[String]) -> String {
    meta.iter()
        .filter(|(k, _)| !excluded.iter().any(|e| e == *k))
        .map(|(k, v)| format!("{k}: {}", py_str(v)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A node (`TextNode`) of the index.
#[derive(Debug, Clone, PartialEq)]
pub struct TextNode {
    pub id: String,
    pub text: String,
    pub metadata: Map<String, Value>,
    pub excluded_embed_metadata_keys: Vec<String>,
    pub excluded_llm_metadata_keys: Vec<String>,
    /// `relationships` as a JSON object (keys "1"/"2"/"3").
    pub relationships: Map<String, Value>,
    pub start_char_idx: Option<usize>,
    pub end_char_idx: Option<usize>,
    pub metadata_template: String,
    pub metadata_separator: String,
    pub text_template: String,
    /// `ref_doc_id` (= `relationships["1"].node_id`).
    pub ref_doc_id: Option<String>,
}

impl TextNode {
    /// `TextNode.hash` = `sha256(str(text) + str(metadata))`.
    pub fn hash(&self) -> String {
        sha256_hex(&format!("{}{}", self.text, py_repr(&Value::Object(self.metadata.clone()))))
    }

    fn related_info(&self) -> Value {
        json!({"node_id": self.id, "node_type": "1", "metadata": Value::Object(self.metadata.clone()),
               "hash": self.hash(), "class_name": "RelatedNodeInfo"})
    }

    fn meta_str_mode(&self, excluded: &[String]) -> String {
        self.metadata
            .iter()
            .filter(|(k, _)| !excluded.iter().any(|e| e == *k))
            .map(|(k, v)| py_format_kv(&self.metadata_template, k, &py_str(v)))
            .collect::<Vec<_>>()
            .join(&self.metadata_separator)
    }

    /// `get_content(MetadataMode.LLM)`.
    pub fn llm_content(&self) -> String {
        let ms = self.meta_str_mode(&self.excluded_llm_metadata_keys);
        let ms = crate::tokenizer::py_strip(&ms).to_string();
        if ms.is_empty() {
            return self.text.clone();
        }
        let t = self.text_template.replace("{metadata_str}", "\u{0}MS\u{0}").replace("{content}", "\u{0}CT\u{0}");
        let t = t.replace("\u{0}MS\u{0}", &ms).replace("\u{0}CT\u{0}", &self.text);
        crate::tokenizer::py_strip(&t).to_string()
    }

    /// `get_content(MetadataMode.EMBED)`.
    pub fn embed_content(&self) -> String {
        let ms = self.meta_str_mode(&self.excluded_embed_metadata_keys);
        let ms = crate::tokenizer::py_strip(&ms).to_string();
        if ms.is_empty() {
            return self.text.clone();
        }
        let t = self.text_template.replace("{metadata_str}", &ms).replace("{content}", &self.text);
        crate::tokenizer::py_strip(&t).to_string()
    }

    /// `node.model_dump(mode="json")` with `embedding = None` (key order like pydantic).
    fn node_dict(&self) -> Value {
        let mut m = Map::new();
        m.insert("id_".into(), json!(self.id));
        m.insert("embedding".into(), Value::Null);
        m.insert("metadata".into(), Value::Object(self.metadata.clone()));
        m.insert("excluded_embed_metadata_keys".into(), json!(self.excluded_embed_metadata_keys));
        m.insert("excluded_llm_metadata_keys".into(), json!(self.excluded_llm_metadata_keys));
        m.insert("relationships".into(), Value::Object(self.relationships.clone()));
        m.insert("metadata_template".into(), json!(self.metadata_template));
        m.insert("metadata_separator".into(), json!(self.metadata_separator));
        m.insert("text".into(), json!(self.text));
        m.insert("mimetype".into(), json!("text/plain"));
        m.insert("start_char_idx".into(), json!(self.start_char_idx));
        m.insert("end_char_idx".into(), json!(self.end_char_idx));
        m.insert("text_template".into(), json!(self.text_template));
        m.insert("class_name".into(), json!("TextNode"));
        Value::Object(m)
    }

    /// `json.dumps(node_to_metadata_dict(node, remove_text=False, flat_metadata=False))` –
    /// the column `node_content` of the vec0 index.
    pub fn node_content(&self) -> String {
        let mut meta = self.metadata.clone();
        let nd = pyjson::dumps(&self.node_dict(), &pyjson::Dumps { ensure_ascii: false, ..pyjson::DEFAULT });
        meta.insert("_node_content".into(), json!(nd));
        meta.insert("_node_type".into(), json!("TextNode"));
        let r = self.ref_doc_id.clone().unwrap_or_else(|| "None".into());
        meta.insert("document_id".into(), json!(r));
        meta.insert("doc_id".into(), json!(r));
        meta.insert("ref_doc_id".into(), json!(r));
        pyjson::dumps(&Value::Object(meta), &pyjson::DEFAULT)
    }

    /// `metadata_dict_to_node(json.loads(node_content))`.
    pub fn from_node_content(s: &str) -> Result<Self, String> {
        let outer: Value = serde_json::from_str(s).map_err(|e| e.to_string())?;
        let inner_s = outer.get("_node_content").and_then(Value::as_str).ok_or("Node content not found in metadata dict.")?;
        let n: Value = serde_json::from_str(inner_s).map_err(|e| e.to_string())?;
        let strs = |k: &str| -> Vec<String> {
            n.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
        };
        let st = |k: &str, d: &str| n.get(k).and_then(Value::as_str).unwrap_or(d).to_string();
        let relationships = n.get("relationships").and_then(Value::as_object).cloned().unwrap_or_default();
        let ref_doc_id = relationships.get("1").and_then(|r| r.get("node_id")).and_then(Value::as_str).map(str::to_string);
        Ok(Self {
            id: st("id_", ""),
            text: st("text", ""),
            metadata: n.get("metadata").and_then(Value::as_object).cloned().unwrap_or_default(),
            excluded_embed_metadata_keys: strs("excluded_embed_metadata_keys"),
            excluded_llm_metadata_keys: strs("excluded_llm_metadata_keys"),
            relationships,
            start_char_idx: n.get("start_char_idx").and_then(Value::as_u64).map(|v| v as usize),
            end_char_idx: n.get("end_char_idx").and_then(Value::as_u64).map(|v| v as usize),
            metadata_template: st("metadata_template", "{key}: {value}"),
            metadata_separator: st("metadata_separator", "\n"),
            text_template: st("text_template", "{metadata_str}\n\n{content}"),
            ref_doc_id,
        })
    }

    /// `metadata["document_id"]` as a number.
    pub fn document_id(&self) -> Option<i64> {
        match self.metadata.get("document_id") {
            Some(Value::String(s)) => s.trim().parse().ok(),
            Some(Value::Number(n)) => n.as_i64(),
            _ => None,
        }
    }
}

fn py_format_kv(template: &str, k: &str, v: &str) -> String {
    template.replace("{key}", "\u{0}K\u{0}").replace("{value}", "\u{0}V\u{0}").replace("\u{0}K\u{0}", k).replace("\u{0}V\u{0}", v)
}

/// Python `str.find(sub, start)` in Codepoints.
fn py_find(hay: &[char], needle: &[char], start: usize) -> Option<usize> {
    if needle.is_empty() {
        return if start <= hay.len() { Some(start) } else { None };
    }
    if needle.len() > hay.len() {
        return None;
    }
    (start..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == *needle)
}

/// upstream `build_document_node`: splits `text` (from `build_llm_index_text`) into nodes.
/// `metadata` in upstream order (document_id, title, tags, …, modified).
/// `id_gen(i)` returns the node id (upstream: `uuid4`).
pub fn build_document_nodes(
    doc_id: &str,
    text: &str,
    metadata: &Map<String, Value>,
    chunk_size: usize,
    mut id_gen: impl FnMut(usize) -> String,
) -> Result<Vec<TextNode>, SplitError> {
    let all_keys: Vec<String> = metadata.keys().cloned().collect();
    let llm_excl = vec!["document_id".to_string()];
    let embed_ms = metadata_str(metadata, &all_keys);
    let llm_ms = metadata_str(metadata, &llm_excl);
    let ms = if embed_ms.chars().count() > llm_ms.chars().count() { embed_ms } else { llm_ms };
    let splitter = SentenceSplitter::for_index(chunk_size)?;
    let splits = splitter.split_text_metadata_aware(text, &ms)?;
    // Document.hash = sha256("-".join([metadata_str(ALL)?, sha256(text|"<empty_string>")]))
    let all_ms = metadata_str(metadata, &[]);
    let text_hash = sha256_hex(if text.is_empty() { "<empty_string>" } else { text });
    let doc_identity = if all_ms.is_empty() { text_hash } else { format!("{all_ms}-{text_hash}") };
    let doc_hash = sha256_hex(&doc_identity);
    let source = json!({"node_id": doc_id, "node_type": "4", "metadata": Value::Object(metadata.clone()),
                        "hash": doc_hash, "class_name": "RelatedNodeInfo"});
    let mut nodes: Vec<TextNode> = splits
        .into_iter()
        .enumerate()
        .map(|(i, t)| {
            let mut rel = Map::new();
            rel.insert("1".into(), source.clone());
            TextNode {
                id: id_gen(i),
                text: t,
                metadata: Map::new(),
                excluded_embed_metadata_keys: all_keys.clone(),
                excluded_llm_metadata_keys: llm_excl.clone(),
                relationships: rel,
                start_char_idx: None,
                end_char_idx: None,
                metadata_template: "{key}: {value}".into(),
                metadata_separator: "\n".into(),
                text_template: "{metadata_str}\n\n{content}".into(),
                ref_doc_id: Some(doc_id.to_string()),
            }
        })
        .collect();
    // _postprocess_parsed_nodes
    let hay: Vec<char> = text.chars().collect();
    let mut search = 0usize;
    for i in 0..nodes.len() {
        let needle: Vec<char> = nodes[i].text.chars().collect();
        if let Some(s) = py_find(&hay, &needle, search) {
            nodes[i].start_char_idx = Some(s);
            nodes[i].end_char_idx = Some(s + needle.len());
            search = s + 1;
        }
        let mut merged = metadata.clone();
        for (k, v) in &nodes[i].metadata {
            merged.insert(k.clone(), v.clone());
        }
        nodes[i].metadata = merged;
        if i > 0 {
            let prev = nodes[i - 1].related_info();
            nodes[i].relationships.insert("2".into(), prev);
        }
        if i + 1 < nodes.len() {
            let next = nodes[i + 1].related_info();
            nodes[i].relationships.insert("3".into(), next);
        }
    }
    Ok(nodes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repr() {
        assert_eq!(py_repr(&json!(["a", "b'c", null, 3])), "['a', \"b'c\", None, 3]");
        assert_eq!(py_repr_str("x\u{a0}y"), "'x\\xa0y'");
    }
}
