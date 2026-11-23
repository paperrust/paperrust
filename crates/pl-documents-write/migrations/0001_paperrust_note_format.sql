-- PaperRust: note format (plain/markdown), same column as paperless-ngx 3.5.0 migration
-- documents.0030_note_format (upstream #14626, src/documents/migrations/0030_note_format.py@v3.5.1:
-- AddField Note.format CharField(max_length=16, choices plain/markdown, default "plain")).
--
-- * Fresh PaperRust installs and databases taken over from paperless-ngx 2.20–3.4.2: the column is
--   added here (same name, type and NOT NULL as upstream). Existing notes become 'plain'.
-- * Databases taken over from paperless-ngx 3.5.x already have the column, but WITHOUT a column
--   default (Django adds it with DEFAULT 'plain' and drops the default again). The default is set
--   here so writers that do not name the column (older code paths, Django 3.4.2 tooling) still work.
-- * Deliberately NOT a CHECK constraint: upstream has none either (choices are validated by the API).
-- * django_migrations gets NO documents.0030 row: the takeover detection stays unchanged
--   (pl_cli::migrate::detect_state / pl_dbmigrate::schema::check_migrations accept the row when
--   paperless-ngx wrote it).
-- * idempotent; adding a column with a constant default is a catalog-only change (no table rewrite).

ALTER TABLE documents_note ADD COLUMN IF NOT EXISTS format varchar(16) NOT NULL DEFAULT 'plain';
ALTER TABLE documents_note ALTER COLUMN format SET DEFAULT 'plain';
