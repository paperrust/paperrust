-- PaperRust L6, first step (docs/DATAMODEL-V2.md §1.9, docs/contract/pl-objects.md "Tag-Baum"):
-- tag hierarchy only as an adjacency list. Idea like upstream #14612 (paperless-ngx, after v3.4.2,
-- commit 8aa99a381 "replace django-treenode with adjacency-list tags"), own implementation.
--
-- django-treenode kept 13 derived columns in documents_tag (ancestor/child/descendant
-- lists as text, counters, level, order, priority) and rewrote them for ALL tags on every tree
-- change. The only source of truth is tn_parent_id (foreign key to documents_tag, index
-- documents_tag_tn_parent_id_59df9f7f, ON DELETE CASCADE in the application) – it stays unchanged
-- (same name as upstream after #14612, so that newer paperless-ngx exports fit without detour).
-- Ancestors/descendants are computed by pl_objects::tagtree or by a recursive SQL query per operation.
--
-- * No longer expand-only (allowed since 2026-11-10: paperless-ngx no longer runs on the same
--   database, docs/DATAMODEL-V2.md §0). Django v3.4.2 (mixed operation) can no longer read tags
--   afterwards – deliberate, see Known differences in docs/contract/pl-objects.md.
-- * Databases of paperless-ngx 3.0–3.4.2 (migration assistant, start with an existing DB): first
--   `manage migrate` (schema snapshot or catching up to v3.4.2), then this migration; the
--   derived values are lost without harm because they follow from tn_parent_id.
--   tn_priority is the only non-derivable one: in paperless-ngx it can only be set via the Django admin page,
--   and only affects the treenode ordering (tn_order), which no API response uses.
-- * django_migrations gets NO entry documents.0028_…: the migration state v3.4.2 remains the
--   detection for the switch-over (pl_cli::migrate::detect_state).
-- * idempotent (DROP COLUMN IF EXISTS; pure catalog change, no table rewrite).
-- * Way back: only via the export in paperless-ngx format (`document_exporter`); it computes the
--   treenode fields for v3.4.2 from tn_parent_id (pl_objects::treenode, docs/DATAMODEL-V2.md §3.2).
--   By hand: recreate the columns with the v3.4.2 types (crates/pl-dbmigrate/schema/v3.4.2-postgres-pre.sql)
--   and fill them with the values from a fresh export.

ALTER TABLE documents_tag
    DROP COLUMN IF EXISTS tn_ancestors_pks,
    DROP COLUMN IF EXISTS tn_ancestors_count,
    DROP COLUMN IF EXISTS tn_children_pks,
    DROP COLUMN IF EXISTS tn_children_count,
    DROP COLUMN IF EXISTS tn_depth,
    DROP COLUMN IF EXISTS tn_descendants_pks,
    DROP COLUMN IF EXISTS tn_descendants_count,
    DROP COLUMN IF EXISTS tn_index,
    DROP COLUMN IF EXISTS tn_level,
    DROP COLUMN IF EXISTS tn_priority,
    DROP COLUMN IF EXISTS tn_order,
    DROP COLUMN IF EXISTS tn_siblings_pks,
    DROP COLUMN IF EXISTS tn_siblings_count;
