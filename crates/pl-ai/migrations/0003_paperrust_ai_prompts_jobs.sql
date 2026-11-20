-- PaperRust: own prompts, AI options and post-processing jobs (extension, not upstream schema).
-- Expand-only, idempotent (IF NOT EXISTS). Rollback:
--   DROP TABLE paperrust_ai_job, paperrust_ai_option, paperrust_ai_prompt;  (no dependencies on upstream)

BEGIN;

-- Own prompt templates per task. Without a row the built-in template applies (pl_ai::prompt_store).
-- kind: classification | title | custom_fields | vision_ocr | chat_system
CREATE TABLE IF NOT EXISTS paperrust_ai_prompt (
    kind          varchar(32)  PRIMARY KEY
                  CHECK (kind IN ('classification', 'title', 'custom_fields', 'vision_ocr', 'chat_system')),
    template      text         NOT NULL CHECK (length(template) BETWEEN 1 AND 20000),
    updated       timestamptz  NOT NULL DEFAULT now(),
    updated_by_id integer      NULL REFERENCES auth_user (id) ON DELETE SET NULL DEFERRABLE INITIALLY DEFERRED
);

-- AI options that are not (yet) in the settings registry (pl_ai::options): custom fields via AI,
-- AI text recognition, behavior of the trigger tag, post-processing. One entry per key.
CREATE TABLE IF NOT EXISTS paperrust_ai_option (
    key           varchar(64)  PRIMARY KEY,
    value         jsonb        NOT NULL,
    updated       timestamptz  NOT NULL DEFAULT now(),
    updated_by_id integer      NULL REFERENCES auth_user (id) ON DELETE SET NULL DEFERRABLE INITIALLY DEFERRED
);

-- "Post-process existing archive": one job per start, progress and cancellation.
-- status: queued | running | done | cancelled | failed | interrupted
CREATE TABLE IF NOT EXISTS paperrust_ai_job (
    id            bigserial    PRIMARY KEY,
    created       timestamptz  NOT NULL DEFAULT now(),
    created_by_id integer      NULL REFERENCES auth_user (id) ON DELETE SET NULL DEFERRABLE INITIALLY DEFERRED,
    run_id        uuid         NOT NULL,
    mode          varchar(8)   NOT NULL CHECK (mode IN ('suggest', 'apply')),
    filter        jsonb        NOT NULL,          -- pl_ai::jobs::JobFilter
    status        varchar(12)  NOT NULL DEFAULT 'queued'
                  CHECK (status IN ('queued', 'running', 'done', 'cancelled', 'failed', 'interrupted')),
    total         integer      NOT NULL DEFAULT 0,
    processed     integer      NOT NULL DEFAULT 0,
    applied       integer      NOT NULL DEFAULT 0,
    suggested     integer      NOT NULL DEFAULT 0,
    errors        integer      NOT NULL DEFAULT 0,
    last_document_id integer   NULL,              -- resume point (ascending id)
    cancel_requested boolean   NOT NULL DEFAULT false,
    heartbeat     timestamptz  NULL,
    finished      timestamptz  NULL,
    message       text         NULL
);
CREATE INDEX IF NOT EXISTS paperrust_ai_job_status ON paperrust_ai_job (status, created DESC);

COMMIT;
