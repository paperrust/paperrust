-- PaperRust: system settings from the UI (docs/SETTINGS.md §2.2). Crate: pl-settings.
--
-- * expand-only (ARCHITECTURE-V2 §4): only new tables/sequence with prefix paperrust_, no
--   change to Django tables. ApplicationConfiguration remains the only source of its fields.
-- * idempotent (IF NOT EXISTS), may run multiple times.
-- * Rollback: DROP TABLE paperrust_setting_history, paperrust_setting; DROP SEQUENCE paperrust_setting_rev;
--   (no upstream data affected; afterwards only environment/paperless.conf apply again).

CREATE SEQUENCE IF NOT EXISTS paperrust_setting_rev;

CREATE TABLE IF NOT EXISTS paperrust_setting (
    -- canonical name, e.g. 'PAPERLESS_EMAIL_HOST'
    key             text PRIMARY KEY,
    -- canonical env string (NULL for secrets)
    value           text,
    -- Nonce (24 bytes) || XChaCha20-Poly1305 ciphertext (secrets only)
    secret_ct       bytea,
    -- Identifier of the key derived from SECRET_KEY
    secret_kid      text,
    -- from paperrust_setting_rev
    rev             bigint NOT NULL,
    updated_at      timestamptz NOT NULL DEFAULT now(),
    -- auth_user.id, deliberately without FK
    updated_by      integer,
    updated_by_name text,
    CONSTRAINT paperrust_setting_value_or_secret CHECK ((value IS NULL) <> (secret_ct IS NULL))
);

CREATE TABLE IF NOT EXISTS paperrust_setting_history (
    id          bigserial PRIMARY KEY,
    key         text NOT NULL,
    -- secrets: '***'
    old_value   text,
    new_value   text,
    -- set | reset | adopt | import | cli | rekey
    action      text NOT NULL,
    at          timestamptz NOT NULL DEFAULT now(),
    user_id     integer,
    user_name   text,
    remote_addr inet
);

CREATE INDEX IF NOT EXISTS paperrust_setting_history_key_idx ON paperrust_setting_history (key, id DESC);
