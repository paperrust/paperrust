-- PaperRust: own tables of pl-ai (extension, not upstream schema).
-- Expand-only: only creates new tables/indexes, does not change any Django table.
-- Idempotent (IF NOT EXISTS). Applied at startup (feature ai) and by `paperrust manage migrate` – release by the coordinator
-- (phase 1 does not provide for own migrations, see docs/ARCHITECTURE.md).
-- Rollback: DROP TABLE paperrust_ai_review, paperrust_ai_audit, paperrust_taxonomy_alias;  (no dependencies on upstream)

BEGIN;

-- Confirmed mappings "other name → existing object", e.g.
-- "Swisscom (Schweiz) AG" ← "Swisscom", "Blue Entertainment".
-- Visibility: an alias is usable only if the user may see the target object
-- (checked via the target object, not via owner_id).
CREATE TABLE IF NOT EXISTS paperrust_taxonomy_alias (
    id            bigserial    PRIMARY KEY,
    kind          varchar(32)  NOT NULL
                  CHECK (kind IN ('correspondent', 'tag', 'document_type', 'storage_path', 'custom_field')),
    target_id     integer      NOT NULL,          -- id in documents_<kind>; no FK (polymorphic)
    alias         varchar(256) NOT NULL,          -- original spelling
    alias_key     varchar(256) NOT NULL,          -- pl_ai::taxonomy::alias_key(kind, alias)
    source        varchar(16)  NOT NULL DEFAULT 'manual'
                  CHECK (source IN ('manual', 'mcp', 'auto', 'merge')),
    created_by_id integer      NULL REFERENCES auth_user (id) ON DELETE SET NULL DEFERRABLE INITIALLY DEFERRED,
    created       timestamptz  NOT NULL DEFAULT now(),
    note          text         NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS paperrust_taxonomy_alias_uniq
    ON paperrust_taxonomy_alias (kind, alias_key, target_id);
CREATE INDEX IF NOT EXISTS paperrust_taxonomy_alias_lookup
    ON paperrust_taxonomy_alias (kind, alias_key);
CREATE INDEX IF NOT EXISTS paperrust_taxonomy_alias_target
    ON paperrust_taxonomy_alias (kind, target_id);

-- AI audit log: every change through MCP or the automation (in addition to the upstream
-- auditlog_logentry, if PAPERLESS_AUDIT_LOG_ENABLED). `before`/`after` allow
-- undoing; `undone_by_id` points to the undo entry.
CREATE TABLE IF NOT EXISTS paperrust_ai_audit (
    id            bigserial    PRIMARY KEY,
    created       timestamptz  NOT NULL DEFAULT now(),
    actor_id      integer      NULL REFERENCES auth_user (id) ON DELETE SET NULL DEFERRABLE INITIALLY DEFERRED,
    channel       varchar(16)  NOT NULL CHECK (channel IN ('mcp', 'auto', 'workflow', 'ui')),
    token_hint    varchar(16)  NULL,              -- first 4 characters of the token + "…", never the token
    run_id        uuid         NULL,              -- automation run / MCP session
    action        varchar(32)  NOT NULL,          -- create_taxonomy | update_document | add_alias | undo | suggest
    object_kind   varchar(32)  NOT NULL,          -- document | correspondent | tag | ...
    object_id     integer      NULL,
    forced        boolean      NOT NULL DEFAULT false,
    reason        text         NULL,              -- required when forced = true
    candidates    jsonb        NULL,              -- candidates at the time of the decision
    before        jsonb        NULL,
    after         jsonb        NULL,
    confidence    double precision NULL,
    undone_by_id  bigint       NULL REFERENCES paperrust_ai_audit (id) ON DELETE SET NULL,
    CHECK (NOT forced OR (reason IS NOT NULL AND length(btrim(reason)) > 0))
);
CREATE INDEX IF NOT EXISTS paperrust_ai_audit_object ON paperrust_ai_audit (object_kind, object_id, created DESC);
CREATE INDEX IF NOT EXISTS paperrust_ai_audit_run    ON paperrust_ai_audit (run_id);
CREATE INDEX IF NOT EXISTS paperrust_ai_audit_actor  ON paperrust_ai_audit (actor_id, created DESC);

-- Review list of the automatic processing (/api/x-ai/review/): one entry per document and
-- run, with the plan (planned changes incl. confidence) and the result.
-- status: pending (only suggested) | partial (partly applied automatically) |
--         applied | rejected | undone
CREATE TABLE IF NOT EXISTS paperrust_ai_review (
    id            bigserial    PRIMARY KEY,
    created       timestamptz  NOT NULL DEFAULT now(),
    document_id   integer      NOT NULL REFERENCES documents_document (id) ON DELETE CASCADE DEFERRABLE INITIALLY DEFERRED,
    run_id        uuid         NOT NULL,
    trigger       varchar(16)  NOT NULL CHECK (trigger IN ('consumed', 'tag', 'workflow', 'manual')),
    mode          varchar(8)   NOT NULL CHECK (mode IN ('suggest', 'apply')),
    status        varchar(8)   NOT NULL DEFAULT 'pending'
                  CHECK (status IN ('pending', 'partial', 'applied', 'rejected', 'undone')),
    suggestions   jsonb        NOT NULL,          -- raw LLM suggestions (ClassificationSuggestions)
    plan          jsonb        NOT NULL,          -- [{field, existing_id, new_name, value, confidence, apply, why, applied}]
    audit_id      bigint       NULL REFERENCES paperrust_ai_audit (id) ON DELETE SET NULL,
    decided_by_id integer      NULL REFERENCES auth_user (id) ON DELETE SET NULL DEFERRABLE INITIALLY DEFERRED,
    decided_at    timestamptz  NULL
);
CREATE INDEX IF NOT EXISTS paperrust_ai_review_doc    ON paperrust_ai_review (document_id, created DESC);
CREATE INDEX IF NOT EXISTS paperrust_ai_review_status ON paperrust_ai_review (status, created DESC);

COMMIT;
