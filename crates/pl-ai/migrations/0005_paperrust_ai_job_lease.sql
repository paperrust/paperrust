-- PaperRust: AI job lease/budget and review audit list (review findings 2, 3, 6; extension, not upstream schema).
-- Expand-only, idempotent (IF NOT EXISTS).
-- Rollback:
--   DROP INDEX paperrust_ai_job_one_active;
--   ALTER TABLE paperrust_ai_job DROP COLUMN lease_owner, DROP COLUMN new_objects_used;
--   ALTER TABLE paperrust_ai_review DROP COLUMN audit_ids;

BEGIN;

-- Lease of the worker that runs the job (random id per run); heartbeat is the lease timestamp.
-- A job counts as interrupted only after its lease expired; a worker that lost its lease stops.
ALTER TABLE paperrust_ai_job ADD COLUMN IF NOT EXISTS lease_owner uuid NULL;
-- New objects (tags, correspondents, …) created by the job so far; survives "resume".
ALTER TABLE paperrust_ai_job ADD COLUMN IF NOT EXISTS new_objects_used integer NOT NULL DEFAULT 0;

-- At most one queued/running job (atomic guard for create/resume). Older duplicates from before
-- this guard are settled first, otherwise the index could not be built.
UPDATE paperrust_ai_job SET status = 'interrupted', lease_owner = NULL, message = 'duplicate active job'
 WHERE status IN ('queued', 'running')
   AND id <> (SELECT max(id) FROM paperrust_ai_job WHERE status IN ('queued', 'running'));
CREATE UNIQUE INDEX IF NOT EXISTS paperrust_ai_job_one_active ON paperrust_ai_job ((true))
 WHERE status IN ('queued', 'running');

-- All audit entries of a review entry (every partial acceptance), undone in reverse order.
-- `audit_id` stays as the latest entry for API compatibility.
ALTER TABLE paperrust_ai_review ADD COLUMN IF NOT EXISTS audit_ids bigint[] NOT NULL DEFAULT '{}';
UPDATE paperrust_ai_review SET audit_ids = ARRAY[audit_id]
 WHERE audit_id IS NOT NULL AND cardinality(audit_ids) = 0;

COMMIT;
