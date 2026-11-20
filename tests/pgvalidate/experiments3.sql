-- Phase 6, part 3: index candidate pgv_doc_live_created on the REAL Rust list queries
-- (capture, parameters substituted) before/after. Index only in pl_pgvalidate, removed afterwards.
\set ON_ERROR_STOP 1
SET auto_explain.log_min_duration = -1;
\pset pager off
\timing on
\echo '##### VORHER'
\echo '--- admin doclist.default'
\set q `cat results/pgvalidate/capture/replay_admin_doclist.default.sql`
EXPLAIN (ANALYZE, BUFFERS) :q;
\echo '--- admin doclist.page200'
\set q `cat results/pgvalidate/capture/replay_admin_doclist.page200.sql`
EXPLAIN (ANALYZE, BUFFERS) :q;
\echo '--- alice doclist.default'
\set q `cat results/pgvalidate/capture/replay_alice_doclist.default.sql`
EXPLAIN (ANALYZE, BUFFERS) :q;
\echo '--- alice doclist.page200'
\set q `cat results/pgvalidate/capture/replay_alice_doclist.page200.sql`
EXPLAIN (ANALYZE, BUFFERS) :q;
CREATE INDEX pgv_doc_live_created ON documents_document (created, id) WHERE root_document_id IS NULL AND deleted_at IS NULL;
VACUUM (ANALYZE) documents_document;
\echo '##### NACHHER (pgv_doc_live_created)'
\echo '--- admin doclist.default'
\set q `cat results/pgvalidate/capture/replay_admin_doclist.default.sql`
EXPLAIN (ANALYZE, BUFFERS) :q;
\echo '--- admin doclist.page200'
\set q `cat results/pgvalidate/capture/replay_admin_doclist.page200.sql`
EXPLAIN (ANALYZE, BUFFERS) :q;
\echo '--- alice doclist.default'
\set q `cat results/pgvalidate/capture/replay_alice_doclist.default.sql`
EXPLAIN (ANALYZE, BUFFERS) :q;
\echo '--- alice doclist.page200'
\set q `cat results/pgvalidate/capture/replay_alice_doclist.page200.sql`
EXPLAIN (ANALYZE, BUFFERS) :q;
DROP INDEX pgv_doc_live_created;
