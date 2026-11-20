-- PaperRust extensions of the workflows (docs/WORKFLOWS-NEU.md, contract docs/contract/automation.md).
-- Expand-only and idempotent. The ngx storage format (documents_workflow*) stays unchanged; these
-- tables are additions that paperless-ngx simply does not know after a way back (all foreign keys
-- with ON DELETE CASCADE, so that deletions in ngx never fail on them).

-- History per workflow (runs of the real execution: ingestion, updated, schedule, manual, mail).
CREATE TABLE IF NOT EXISTS paperrust_workflow_run (
    id bigserial PRIMARY KEY,
    workflow_id integer NOT NULL REFERENCES documents_workflow (id) ON DELETE CASCADE,
    trigger_type smallint NOT NULL,
    document_id integer NULL,
    run_at timestamptz NOT NULL DEFAULT now(),
    result varchar(16) NOT NULL,
    branch varchar(8) NOT NULL DEFAULT 'main',
    code varchar(48) NOT NULL DEFAULT '',
    message text NOT NULL DEFAULT '',
    changed_fields jsonb NOT NULL DEFAULT '[]'::jsonb,
    source smallint NULL
);
CREATE INDEX IF NOT EXISTS paperrust_workflow_run_wf_idx ON paperrust_workflow_run (workflow_id, id DESC);
CREATE INDEX IF NOT EXISTS paperrust_workflow_run_at_idx ON paperrust_workflow_run (run_at);

-- "Else" branch: actions (normal documents_workflowaction rows) that run when the trigger
-- has fired but the conditions do not match.
CREATE TABLE IF NOT EXISTS paperrust_workflow_else_action (
    workflow_id integer NOT NULL REFERENCES documents_workflow (id) ON DELETE CASCADE,
    workflowaction_id integer NOT NULL REFERENCES documents_workflowaction (id) ON DELETE CASCADE,
    PRIMARY KEY (workflow_id, workflowaction_id)
);
CREATE INDEX IF NOT EXISTS paperrust_workflow_else_action_action_idx ON paperrust_workflow_else_action (workflowaction_id);

-- Further settings per workflow (AI condition).
CREATE TABLE IF NOT EXISTS paperrust_workflow_ext (
    workflow_id integer PRIMARY KEY REFERENCES documents_workflow (id) ON DELETE CASCADE,
    ai_condition jsonb NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);
