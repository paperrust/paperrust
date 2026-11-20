//! [`KnowledgeIndex`] over the **existing** LLM index (upstream `paperless_ai` llmindex):
//! same directory (`LLM_INDEX_DIR`), same nodes, same embedding model, same
//! permission filtering as the document chat (`server::chat::run`): unrestricted users → all
//! except trash, otherwise `permitted_document_ids(user)`. No own index, no own
//! embedding pipeline. New documents get into the index via the existing path
//! (`update_document_in_llm_index` after import, `llmindex_index` periodically).

use futures::future::BoxFuture;
use pl_authz::objects::user_is_unrestricted;
use pl_core::AppState;

use super::{data, effective_config, index};
use crate::embedding::Embedder;
use crate::mcp::knowledge::{IndexState, KnowledgeIndex, Passage};
use crate::mcp::Actor;
use crate::vector_store::{DocFilter, VecStore};

/// Production implementation (needs `AppState`: DB + `DATA_DIR`).
pub struct LlmIndexKnowledge(pub AppState);

impl KnowledgeIndex for LlmIndexKnowledge {
    fn state(&self) -> BoxFuture<'_, IndexState> {
        Box::pin(async move {
            let Ok(cfg) = effective_config(&self.0).await else { return IndexState::Disabled };
            if !cfg.llm_index_enabled() {
                return IndexState::Disabled;
            }
            match index::llm_index_exists(&self.0).await {
                Ok(true) => IndexState::Ready,
                _ => {
                    // like upstream retrieve_similar_nodes: missing index → trigger a build
                    index::queue_llm_index_update_if_needed(&self.0, false, "LLM index not found for MCP ask_documents.").await;
                    IndexState::NotBuilt
                }
            }
        })
    }

    fn semantic<'a>(&'a self, actor: &'a Actor, query: &'a str, top_k: usize, restrict: Option<&'a [i64]>) -> BoxFuture<'a, Result<Vec<Passage>, String>> {
        Box::pin(async move {
            let state = &self.0;
            let db = state.db();
            let cfg = effective_config(state).await.map_err(|e| e.status().to_string())?;
            let uid = actor.user_id().ok_or("anonymous")?;
            let user = pl_authz::user::load_user_by_id(db, uid).await.map_err(|e| e.to_string())?.ok_or("user not found")?;
            if !user.is_active {
                return Ok(Vec::new());
            }
            // same filters as the chat
            // upstream fix 62d729e30 (#14754): restricted users get the shorter of IN / NOT IN
            // (chosen under the read lock) plus a post-filter on the permitted set.
            let (mut filters, permitted) = if user_is_unrestricted(Some(&user)) {
                (vec![DocFilter::NotIn(data::trashed_ids(db).await.map_err(|e| e.status().to_string())?)], None)
            } else {
                let p: std::collections::HashSet<i64> =
                    data::permitted_doc_ids(db, Some(&user)).await.map_err(|e| e.status().to_string())?.into_iter().collect();
                (Vec::new(), Some(p))
            };
            if let Some(r) = restrict {
                filters.push(DocFilter::In(r.to_vec()));
            }
            let emb = Embedder::from_config(&cfg).map_err(|e| e.to_string())?;
            let qv = emb.embed_query(query).await.map_err(|e| e.to_string())?;
            let dir = index::index_dir(state);
            let d2 = dir.clone();
            let _r = tokio::task::spawn_blocking(move || crate::locks::RwGuard::read(&d2)).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
            let hits = tokio::task::spawn_blocking(move || -> Result<Vec<crate::vector_store::Hit>, String> {
                let st = VecStore::open(&dir, None).map_err(|e| e.to_string())?;
                let mut filters = filters;
                if let Some(p) = &permitted {
                    let indexed = st.indexed_document_ids().map_err(|e| e.to_string())?;
                    filters.insert(0, crate::chat::permitted_documents_filter(&indexed, p));
                }
                let mut hits = st.query(&qv, top_k, &filters).map_err(|e| e.to_string())?;
                if let Some(p) = &permitted {
                    crate::chat::retain_permitted(&mut hits, p);
                }
                Ok(hits)
            })
            .await
            .map_err(|e| e.to_string())??;
            Ok(hits
                .into_iter()
                .filter_map(|h| {
                    Some(Passage { document_id: h.node.document_id()?, text: h.node.text.clone(), llm_text: h.node.llm_content(), score: h.score })
                })
                .collect())
        })
    }

    fn output_language<'a>(&'a self, actor: &'a Actor) -> BoxFuture<'a, Option<String>> {
        Box::pin(async move {
            let cfg = effective_config(&self.0).await.ok()?;
            let ui = match actor.user_id() {
                Some(u) => data::ui_settings(self.0.db(), u).await.ok().flatten(),
                None => None,
            };
            crate::classifier::get_llm_output_language(cfg.llm_output_language.as_deref(), ui.as_ref())
        })
    }
}
