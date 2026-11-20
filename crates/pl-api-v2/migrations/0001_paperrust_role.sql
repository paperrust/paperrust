-- pl_api_v2.0001_paperrust_role – roles (docs/DESIGN2-ENTWIRREN.md §1, contract
-- docs/contract/pl-api-v2.md "Rollen"). Expand-only and idempotent: runs via
-- `manage migrate` (pl_cli::migrate::paperrust_migrations) and on the first call of the roles API
-- (pl_api_v2::roles::ensure). The source of truth remains the Django permissions (auth_user_user_permissions,
-- auth_group_permissions, auth_user.is_superuser/is_staff); these tables are only the layer
-- on top. The permission lists of the fixed roles must match pl_api_v2::roles::{READ,EDIT,MANAGE}
-- (unit test `seed_matches_constants`).
SELECT pg_advisory_xact_lock(hashtext('paperrust_role'));

CREATE TABLE IF NOT EXISTS paperrust_role (
    id bigserial PRIMARY KEY,
    key varchar(16) UNIQUE,
    name varchar(128) NOT NULL,
    description text NOT NULL DEFAULT '',
    builtin boolean NOT NULL DEFAULT false,
    is_admin boolean NOT NULL DEFAULT false,
    permissions text[] NOT NULL DEFAULT '{}',
    created timestamptz NOT NULL DEFAULT now(),
    modified timestamptz NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX IF NOT EXISTS paperrust_role_name_uniq ON paperrust_role (lower(name));

CREATE TABLE IF NOT EXISTS paperrust_role_assignment (
    subject varchar(5) NOT NULL CHECK (subject IN ('user', 'group')),
    subject_id bigint NOT NULL,
    role_id bigint NOT NULL REFERENCES paperrust_role (id) ON DELETE CASCADE,
    assigned timestamptz NOT NULL DEFAULT now(),
    assigned_by bigint,
    PRIMARY KEY (subject, subject_id)
);
CREATE INDEX IF NOT EXISTS paperrust_role_assignment_role ON paperrust_role_assignment (role_id);

CREATE TABLE IF NOT EXISTS paperrust_role_config (
    id smallint PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    mode varchar(16) NOT NULL DEFAULT 'classic' CHECK (mode IN ('classic', 'custom')),
    modified timestamptz NOT NULL DEFAULT now()
);
INSERT INTO paperrust_role_config (id, mode) VALUES (1, 'classic') ON CONFLICT (id) DO NOTHING;

-- Fixed roles (one row per role; format is read by the unit test).
INSERT INTO paperrust_role (key, name, description, builtin, is_admin, permissions) VALUES
('read', 'Nur lesen', 'Dokumente, Notizen, Tags, Korrespondenten, Dokumenttypen, Speicherpfade, Felder, gespeicherte Ansichten und Freigabelinks ansehen.', true, false, '{documents.view_correspondent,documents.view_customfield,documents.view_document,documents.view_documenttype,documents.view_note,documents.view_savedview,documents.view_sharelink,documents.view_sharelinkbundle,documents.view_storagepath,documents.view_tag}'),
('edit', 'Bearbeiten', 'Wie Nur lesen, dazu Dokumente und Notizen hinzufügen und ändern, eigene Ansichten anlegen und ändern, Freigabelinks anlegen.', true, false, '{documents.add_document,documents.add_note,documents.add_savedview,documents.add_sharelink,documents.add_sharelinkbundle,documents.change_document,documents.change_note,documents.change_savedview,documents.change_sharelinkbundle,documents.view_correspondent,documents.view_customfield,documents.view_document,documents.view_documenttype,documents.view_note,documents.view_savedview,documents.view_sharelink,documents.view_sharelinkbundle,documents.view_storagepath,documents.view_tag}'),
('manage', 'Verwalten', 'Wie Bearbeiten, dazu löschen; Tags, Korrespondenten, Dokumenttypen, Speicherpfade, Felder, Workflows, Mail-Konten und -Regeln verwalten; Aufgaben und Verlauf ansehen.', true, false, '{auditlog.view_logentry,documents.add_correspondent,documents.add_customfield,documents.add_document,documents.add_documenttype,documents.add_note,documents.add_savedview,documents.add_sharelink,documents.add_sharelinkbundle,documents.add_storagepath,documents.add_tag,documents.add_workflow,documents.change_correspondent,documents.change_customfield,documents.change_document,documents.change_documenttype,documents.change_note,documents.change_savedview,documents.change_sharelinkbundle,documents.change_storagepath,documents.change_tag,documents.change_workflow,documents.delete_correspondent,documents.delete_customfield,documents.delete_document,documents.delete_documenttype,documents.delete_note,documents.delete_savedview,documents.delete_sharelink,documents.delete_sharelinkbundle,documents.delete_storagepath,documents.delete_tag,documents.delete_workflow,documents.view_correspondent,documents.view_customfield,documents.view_document,documents.view_documenttype,documents.view_note,documents.view_paperlesstask,documents.view_savedview,documents.view_sharelink,documents.view_sharelinkbundle,documents.view_storagepath,documents.view_tag,documents.view_workflow,paperless_mail.add_mailaccount,paperless_mail.add_mailrule,paperless_mail.add_processedmail,paperless_mail.change_mailaccount,paperless_mail.change_mailrule,paperless_mail.change_processedmail,paperless_mail.delete_mailaccount,paperless_mail.delete_mailrule,paperless_mail.delete_processedmail,paperless_mail.view_mailaccount,paperless_mail.view_mailrule,paperless_mail.view_processedmail}'),
('admin', 'Administrator', 'Sieht und darf alles, auch Benutzer, Gruppen und System (is_superuser und is_staff).', true, true, '{}')
ON CONFLICT (key) DO UPDATE SET
    name = EXCLUDED.name,
    description = EXCLUDED.description,
    builtin = true,
    is_admin = EXCLUDED.is_admin,
    permissions = EXCLUDED.permissions;
