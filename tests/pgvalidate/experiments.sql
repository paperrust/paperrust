-- Phase 6: before/after measurements on pl_pgvalidate (100k). Run as superuser (only
-- then can auto_explain be switched off in the session); changes no data.
--   psql -X -U postgres -d pl_pgvalidate -f tests/pgvalidate/experiments.sql
-- Each measurement: EXPLAIN (ANALYZE, BUFFERS) – query A = shape that PaperRust sends today
-- (from the capture), B/C = reformulation with identical result (check query included).
\set ON_ERROR_STOP 1
SET auto_explain.log_min_duration = -1;
SET statement_timeout = '600s';
\pset pager off
\timing on

-- Visible document IDs of admin (superuser) as text, as PaperRust binds them
SELECT string_agg(id::text, ',') AS admin_ids FROM documents_document
 WHERE root_document_id IS NULL AND deleted_at IS NULL \gset

\echo '=== E1 selection_data (admin, 97 695 IDs) ==='
\echo '--- E1A heute: korrelierte Unterabfrage je Korrespondent + string_to_array'
EXPLAIN (ANALYZE, BUFFERS)
SELECT o.id, (SELECT COUNT(DISTINCT sd.id) FROM documents_document sd
               WHERE sd.correspondent_id = o.id AND sd.id = ANY(string_to_array(:'admin_ids', ',')::bigint[])) AS document_count
FROM documents_correspondent o ORDER BY o.id;

\echo '--- E1B Vorschlag: ein GROUP BY über unnest(bigint[]) je Beziehung'
EXPLAIN (ANALYZE, BUFFERS)
SELECT o.id, COALESCE(c.n, 0) AS document_count
FROM documents_correspondent o
LEFT JOIN (SELECT d.correspondent_id AS k, COUNT(DISTINCT d.id) AS n
           FROM documents_document d JOIN unnest(string_to_array(:'admin_ids', ',')::bigint[]) s(id) ON d.id = s.id
           GROUP BY 1) c ON c.k = o.id
ORDER BY o.id;

\echo '--- E1A tags heute'
EXPLAIN (ANALYZE, BUFFERS)
SELECT o.id, (SELECT COUNT(DISTINCT sd.document_id) FROM documents_document_tags sd
               WHERE sd.tag_id = o.id AND sd.document_id = ANY(string_to_array(:'admin_ids', ',')::bigint[])) AS document_count
FROM documents_tag o ORDER BY o.name;

\echo '--- E1B tags Vorschlag'
EXPLAIN (ANALYZE, BUFFERS)
SELECT o.id, COALESCE(c.n, 0) AS document_count
FROM documents_tag o
LEFT JOIN (SELECT dt.tag_id AS k, COUNT(DISTINCT dt.document_id) AS n
           FROM documents_document_tags dt JOIN unnest(string_to_array(:'admin_ids', ',')::bigint[]) s(id) ON dt.document_id = s.id
           GROUP BY 1) c ON c.k = o.id
ORDER BY o.name;

\echo '--- E1C alle fünf Beziehungen in EINER Abfrage (ein Roundtrip, IDs einmal entpackt)'
EXPLAIN (ANALYZE, BUFFERS)
WITH s AS MATERIALIZED (SELECT unnest(string_to_array(:'admin_ids', ',')::bigint[]) AS id)
SELECT 'c' AS rel, d.correspondent_id AS k, COUNT(*) FROM documents_document d JOIN s ON d.id = s.id GROUP BY 2
UNION ALL SELECT 'dt', d.document_type_id, COUNT(*) FROM documents_document d JOIN s ON d.id = s.id GROUP BY 2
UNION ALL SELECT 'sp', d.storage_path_id, COUNT(*) FROM documents_document d JOIN s ON d.id = s.id GROUP BY 2
UNION ALL SELECT 't', dt.tag_id, COUNT(DISTINCT dt.document_id) FROM documents_document_tags dt JOIN s ON dt.document_id = s.id GROUP BY 2
UNION ALL SELECT 'cf', ci.field_id, COUNT(DISTINCT ci.document_id) FROM documents_customfieldinstance ci JOIN s ON ci.document_id = s.id WHERE ci.deleted_at IS NULL GROUP BY 2;

\echo '--- Ergebnisprüfung E1A = E1B (Korrespondenten), muss 0 Zeilen liefern'
SELECT * FROM (
  SELECT o.id, (SELECT COUNT(DISTINCT sd.id) FROM documents_document sd WHERE sd.correspondent_id = o.id AND sd.root_document_id IS NULL AND sd.deleted_at IS NULL) AS n FROM documents_correspondent o
  EXCEPT
  SELECT o.id, COALESCE(c.n, 0) FROM documents_correspondent o
  LEFT JOIN (SELECT d.correspondent_id AS k, COUNT(DISTINCT d.id) AS n FROM documents_document d
             JOIN unnest(string_to_array(:'admin_ids', ',')::bigint[]) s(id) ON d.id = s.id GROUP BY 1) c ON c.k = o.id) x;
