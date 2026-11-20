//! pl-ai – AI functions: 1:1 port of upstream paperless_ai plus own extensions
//! (taxonomy matching, MCP server, automatic processing). See docs/AI.md and
//! docs/contract/pl-ai.md.
//!
//! | Module | Content | upstream (v3.4.2) |
//! |---|---|---|
//! | [`compat`] | `_normalize`, `_match_names_to_queryset`, `extract_unmatched_names` (exact) | `paperless_ai/matching.py` |
//! | [`difflib`] | `SequenceMatcher`, `get_close_matches` (exact) | CPython `difflib` |
//! | [`normalize`], [`similarity`], [`taxonomy`], [`alias`] | extended matching, aliases | – (extension) |
//! | [`prompts`] | Jinja templates, byte-exact | `paperless_ai/prompts/*.j2` |
//! | [`schema`] | LLM response schema, validation | `paperless_ai/base_model.py` |
//! | [`classifier`] | candidate weighting, prompt construction | `paperless_ai/ai_classifier.py`, `taxonomy.py` |
//! | [`chat`] | chat stream format (trailer) | `paperless_ai/chat.py` |
//! | [`llm`] | HTTP client Ollama / OpenAI-compatible | `paperless_ai/client.py` (llama_index) |
//! | [`config`] | `PAPERLESS_AI_*`, `PAPERRUST_AI_*` | `paperless/settings`, `paperless/config.py::AIConfig` |
//! | [`mcp`] | MCP server (Streamable HTTP) | – (extension) |
//! | [`auto`] | planning of automatic processing (confidence, limits, duplicate lock) | `documents/workflows/ai.py` (basis) |
//! | [`tokenizer`], [`punkt`] | tiktoken `cl100k_base`, TokenTextSplitter, PromptHelper, SentenceSplitter, nltk-Punkt | llama_index 0.14.25, nltk |
//! | [`node`] | index nodes (`TextNode`, `node_to_metadata_dict`) | llama_index |
//! | [`embedding`] | embedding client, `build_llm_index_text` | `paperless_ai/embedding.py` |
//! | [`vector_store`], [`locks`] | sqlite-vec index in the upstream format, filelock-compatible locks | `paperless_ai/vector_store.py`, `indexing.py` |
//! | [`synth`] | chat synthesis compact/refine (streaming) | llama_index `CompactAndRefine` |
//! | [`pickle`] | Django-Cache-Pickle `SuggestionCacheData` | `documents/caching.py` |
//! | [`vision`] | AI text recognition: page images → text (Ollama/OpenAI-compatible) | – (extension, cf. paperless-gpt) |
//! | [`prompt_store`] | own prompt templates per task | – (extension) |
//! | [`custom_fields`] | custom fields via AI: prompt, validation, normalization | – (extension) |
//! | [`options`] | AI options outside the settings registry | – (extension) |
//! | [`workflow_assist`] | AI condition, workflow draft from description | – (extension) |
//! | [`llm_ext`] | JSON/text responses, model list, connection test | – (extension) |
//! | `server` | routes `ai_suggestions`, `chat`, `/api/x-ai/*`, index tasks, workflow action | `documents/views.py`, `documents/tasks.py` |

pub mod alias;
pub mod auto;
pub mod chat;
pub mod classifier;
pub mod compat;
pub mod config;
pub mod difflib;
pub mod embedding;
pub mod locks;
pub mod node;
pub mod pickle;
pub mod punkt;
pub mod synth;
pub mod tokenizer;
pub mod vector_store;
pub mod llm;
pub mod mcp;
pub mod normalize;
pub mod prompts;
pub mod pyjson;
pub mod schema;
pub mod similarity;
pub mod taxonomy;
pub mod custom_fields;
pub mod llm_ext;
pub mod options;
pub mod prompt_store;
pub mod vision;
pub mod workflow_assist;
#[cfg(test)]
mod testutil;

#[cfg(feature = "server")]
pub mod pg;
#[cfg(feature = "server")]
mod routes;
#[cfg(feature = "server")]
pub mod server;

#[cfg(feature = "server")]
pub use routes::routes;

/// `User-Agent` of the LLM/embedding clients – equal to `pl_core::brand::USER_AGENT` (without
/// the feature `server`, pl-ai also builds without pl-core; equality checked by a test).
pub const USER_AGENT: &str = concat!("PaperRust/", env!("CARGO_PKG_VERSION"), " (+https://github.com/paperrust/paperrust)");

/// MCP server name (`serverInfo.name`) – equal to `pl_core::brand::BIN_NAME`.
pub const MCP_SERVER_NAME: &str = "paperrust";

#[cfg(all(test, feature = "server"))]
mod brand_tests {
    #[test]
    fn same_as_pl_core() {
        assert_eq!(super::USER_AGENT, pl_core::brand::USER_AGENT);
        assert_eq!(super::MCP_SERVER_NAME, pl_core::brand::BIN_NAME);
    }
}
