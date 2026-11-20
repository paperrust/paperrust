#!/usr/bin/env bash
# W8 AI parity: own reference copy (never the shared reference on :8000 / DB paperless).
#   DB        pl_w8_ref   (from own template pl_w8_snapshot <- paperless_snapshot)
#   Data      tests/parity/pl-ai/.w8/data/{data,media,consume}  (from reference/snapshot/data.tar)
#   Redis     shared Garnet, prefix "w8" (reset deletes only w8* keys)
#   Django    127.0.0.1:8180 (upstream unchanged, AI_ENABLED, LLM/embeddings = local mock)
#   Mock LLM  127.0.0.1:8181 (tests/parity/pl-ai/mock_llm.py, log .w8/mock.log)
W8_REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
source "$W8_REPO/reference/env.sh"
export W8_DIR="$W8_REPO/tests/parity/pl-ai/.w8"
export W8_DB=pl_w8_ref
export W8_TPL=pl_w8_snapshot
export W8_DJ_PORT=8180
export W8_MOCK_PORT=8181
export PAPERLESS_DBNAME="$W8_DB"
export PGDATABASE="$W8_DB"
export PAPERLESS_REDIS_PREFIX=w8
export PAPERLESS_DATA_DIR="$(cygpath -w "$W8_DIR/data/data")"
export PAPERLESS_MEDIA_ROOT="$(cygpath -w "$W8_DIR/data/media")"
export PAPERLESS_CONSUMPTION_DIR="$(cygpath -w "$W8_DIR/data/consume")"
export PAPERLESS_STATICDIR="$(cygpath -w "$REF_DIR/data/static")"
export PAPERLESS_LOGGING_DIR="$(cygpath -w "$W8_DIR/logs")"
export PAPERLESS_URL="http://127.0.0.1:$W8_DJ_PORT"
export PAPERLESS_CSRF_TRUSTED_ORIGINS="http://127.0.0.1:$W8_DJ_PORT"
# AI: only the local mock (no cloud service)
export PAPERLESS_AI_ENABLED=true
export PAPERLESS_AI_LLM_BACKEND="${W8_LLM_BACKEND:-ollama}"
export PAPERLESS_AI_LLM_MODEL=llama3.1
if [[ "$PAPERLESS_AI_LLM_BACKEND" == "openai-like" ]]; then
  export PAPERLESS_AI_LLM_ENDPOINT="http://127.0.0.1:$W8_MOCK_PORT/v1"
else
  export PAPERLESS_AI_LLM_ENDPOINT="http://127.0.0.1:$W8_MOCK_PORT"
fi
if [[ "${W8_EMBED_BACKEND:-ollama}" == "none" ]]; then
  unset PAPERLESS_AI_LLM_EMBEDDING_BACKEND PAPERLESS_AI_LLM_EMBEDDING_ENDPOINT   # Full-text fallback (Tantivy MLT)
else
  export PAPERLESS_AI_LLM_EMBEDDING_BACKEND="${W8_EMBED_BACKEND:-ollama}"
  if [[ "$PAPERLESS_AI_LLM_EMBEDDING_BACKEND" == "openai-like" ]]; then
    export PAPERLESS_AI_LLM_EMBEDDING_ENDPOINT="http://127.0.0.1:$W8_MOCK_PORT/v1"
  else
    export PAPERLESS_AI_LLM_EMBEDDING_ENDPOINT="http://127.0.0.1:$W8_MOCK_PORT"
  fi
fi
export PAPERLESS_AI_LLM_EMBEDDING_CHUNK_SIZE="${W8_CHUNK:-256}"
export PAPERLESS_AI_LLM_CONTEXT_SIZE="${W8_CTX:-4096}"
export PAPERLESS_AI_LLM_ALLOW_INTERNAL_ENDPOINTS=true
export MOCK_LLM_LOG="$(cygpath -w "$W8_DIR/mock.log")"
export NO_COLOR=1
