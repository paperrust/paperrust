-- Phase 6, part 2: sorting/filter/counting (admin view, 100k). As superuser, changes no
-- data; index tests create indexes ONLY in pl_pgvalidate and remove them again.
\set ON_ERROR_STOP 1
SET auto_explain.log_min_duration = -1;
SET statement_timeout = '600s';
\pset pager off
\timing on

\echo '=== E3 ordering=correspondent__name (Seite 1, 50)'
\echo '--- E3A heute: korrelierte Skalar-Unterabfrage im ORDER BY'
EXPLAIN (ANALYZE, BUFFERS)
SELECT d.id FROM documents_document d
WHERE d.root_document_id IS NULL AND d.deleted_at IS NULL
ORDER BY (SELECT oc.name FROM documents_correspondent oc WHERE oc.id = d.correspondent_id) ASC LIMIT 50;
\echo '--- E3B Django-Form: LEFT JOIN + ORDER BY name'
EXPLAIN (ANALYZE, BUFFERS)
SELECT d.id FROM documents_document d LEFT JOIN documents_correspondent oc ON oc.id = d.correspondent_id
WHERE d.root_document_id IS NULL AND d.deleted_at IS NULL
ORDER BY oc.name ASC LIMIT 50;

\echo '=== E4 ordering=-num_notes'
\echo '--- E4A heute: korrelierte COUNT je Zeile'
EXPLAIN (ANALYZE, BUFFERS)
SELECT d.id FROM documents_document d
WHERE d.root_document_id IS NULL AND d.deleted_at IS NULL
ORDER BY COALESCE((SELECT COUNT(*) FROM documents_note nn WHERE nn.document_id = d.id AND nn.deleted_at IS NULL), 0) DESC LIMIT 50;
\echo '--- E4B LEFT JOIN auf vorab gruppierte Notizen'
EXPLAIN (ANALYZE, BUFFERS)
SELECT d.id FROM documents_document d
LEFT JOIN (SELECT document_id, COUNT(*) AS n FROM documents_note WHERE deleted_at IS NULL GROUP BY 1) nn ON nn.document_id = d.id
WHERE d.root_document_id IS NULL AND d.deleted_at IS NULL
ORDER BY COALESCE(nn.n, 0) DESC LIMIT 50;

\echo '=== E5 has_duplicates'
\echo '--- E5A heute (Rust): EXISTS mit vier ODER-Bedingungen'
EXPLAIN (ANALYZE, BUFFERS)
SELECT COUNT(*) FROM documents_document d
WHERE d.root_document_id IS NULL AND d.deleted_at IS NULL AND EXISTS (
  SELECT 1 FROM documents_document dup WHERE dup.root_document_id IS NULL AND NOT dup.id = d.id
  AND (dup.checksum = d.checksum OR dup.checksum = NULLIF(d.archive_checksum, '')
       OR dup.archive_checksum = d.checksum OR dup.archive_checksum = NULLIF(d.archive_checksum, '')));
\echo '--- E5B Django-Form (mit permitted-Unterabfrage + LIMIT 1)'
EXPLAIN (ANALYZE, BUFFERS)
SELECT COUNT(*) FROM documents_document d
WHERE d.root_document_id IS NULL AND d.deleted_at IS NULL AND EXISTS (
  SELECT 1 FROM documents_document v0 WHERE v0.id IN (SELECT u0.id FROM documents_document u0 LEFT JOIN documents_document u1 ON u0.root_document_id = u1.id)
  AND v0.root_document_id IS NULL AND NOT v0.id = d.id
  AND (v0.checksum = d.checksum OR v0.checksum = NULLIF(d.archive_checksum, '')
       OR v0.archive_checksum = d.checksum OR v0.archive_checksum = NULLIF(d.archive_checksum, '')) LIMIT 1);
\echo '--- E5C Vorschlag: vier EXISTS (je ein Index-Lookup) statt ODER'
EXPLAIN (ANALYZE, BUFFERS)
SELECT COUNT(*) FROM documents_document d
WHERE d.root_document_id IS NULL AND d.deleted_at IS NULL AND (
  EXISTS (SELECT 1 FROM documents_document dup WHERE dup.root_document_id IS NULL AND dup.id <> d.id AND dup.checksum = d.checksum)
  OR EXISTS (SELECT 1 FROM documents_document dup WHERE dup.root_document_id IS NULL AND dup.id <> d.id AND dup.checksum = NULLIF(d.archive_checksum, ''))
  OR EXISTS (SELECT 1 FROM documents_document dup WHERE dup.root_document_id IS NULL AND dup.id <> d.id AND dup.archive_checksum = d.checksum)
  OR EXISTS (SELECT 1 FROM documents_document dup WHERE dup.root_document_id IS NULL AND dup.id <> d.id AND dup.archive_checksum = NULLIF(d.archive_checksum, '')));

\echo '=== E6 COUNT(*) der Standardliste – Index-Kandidat (partieller Index für Index-Only-Scan)'
\echo '--- E6A vorher'
EXPLAIN (ANALYZE, BUFFERS)
SELECT COUNT(*) FROM documents_document WHERE root_document_id IS NULL AND deleted_at IS NULL;
EXPLAIN (ANALYZE, BUFFERS)
SELECT id FROM documents_document WHERE root_document_id IS NULL AND deleted_at IS NULL ORDER BY created DESC LIMIT 50 OFFSET 9950;
CREATE INDEX pgv_doc_live_created ON documents_document (created, id) WHERE root_document_id IS NULL AND deleted_at IS NULL;
VACUUM (ANALYZE) documents_document;
\echo '--- E6B nachher (Index pgv_doc_live_created)'
EXPLAIN (ANALYZE, BUFFERS)
SELECT COUNT(*) FROM documents_document WHERE root_document_id IS NULL AND deleted_at IS NULL;
EXPLAIN (ANALYZE, BUFFERS)
SELECT id FROM documents_document WHERE root_document_id IS NULL AND deleted_at IS NULL ORDER BY created DESC LIMIT 50 OFFSET 9950;
SELECT pg_size_pretty(pg_relation_size('pgv_doc_live_created')) AS index_size;
DROP INDEX pgv_doc_live_created;
