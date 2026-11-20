-- PaperRust: MCP token scopes (extension, not upstream schema).
-- Expand-only, idempotent. Rollback: DROP TABLE paperrust_ai_mcp_scope;
--
-- One entry per token (token_key = SHA-256 hex of the token; applies to DRF tokens and
-- device tokens prt_…, the plaintext is never stored) OR per user (default for all
-- tokens of the user without their own entry). Without an entry, all groups enabled by the
-- administrator apply (PAPERRUST_AI_MCP_TOOL_GROUPS). Scopes only restrict; the
-- user's permissions always apply in addition.

BEGIN;

CREATE TABLE IF NOT EXISTS paperrust_ai_mcp_scope (
    id            bigserial    PRIMARY KEY,
    token_key     varchar(64)  NULL,              -- sha256(token) hex
    user_id       integer      NULL REFERENCES auth_user (id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    scopes        varchar(64)  NOT NULL,          -- comma-separated: read,write,ingest (empty = nothing)
    label         varchar(128) NULL,
    created       timestamptz  NOT NULL DEFAULT now(),
    created_by_id integer      NULL REFERENCES auth_user (id) ON DELETE SET NULL DEFERRABLE INITIALLY DEFERRED,
    CHECK ((token_key IS NULL) <> (user_id IS NULL)),
    CHECK (token_key IS NULL OR token_key ~ '^[0-9a-f]{64}$')
);
CREATE UNIQUE INDEX IF NOT EXISTS paperrust_ai_mcp_scope_token ON paperrust_ai_mcp_scope (token_key) WHERE token_key IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS paperrust_ai_mcp_scope_user ON paperrust_ai_mcp_scope (user_id) WHERE token_key IS NULL;

COMMIT;
