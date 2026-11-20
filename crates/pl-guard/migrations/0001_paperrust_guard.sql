-- Protection against ransomware (pl-guard, docs/contract/pl-guard.md): guard state, events,
-- findings, backup runs and the object/hash cache of the backup.
-- Expand-only: only new paperrust_* tables, no foreign keys to Django tables. Idempotent.
-- Rollback: DROP TABLE IF EXISTS paperrust_guard_state, paperrust_guard_event, paperrust_guard_finding,
--           paperrust_backup_run, paperrust_backup_object, paperrust_backup_filecache;

CREATE TABLE IF NOT EXISTS paperrust_guard_state (
    id          integer PRIMARY KEY CHECK (id = 1),
    read_only   boolean NOT NULL DEFAULT false,
    since       timestamptz,
    reason      text NOT NULL DEFAULT '',
    event_id    bigint,
    lifted_at   timestamptz,
    lifted_by   integer
);
INSERT INTO paperrust_guard_state (id) VALUES (1) ON CONFLICT (id) DO NOTHING;

CREATE TABLE IF NOT EXISTS paperrust_guard_event (
    id           bigserial PRIMARY KEY,
    created      timestamptz NOT NULL DEFAULT now(),
    status       varchar(16) NOT NULL DEFAULT 'open',   -- open | resolved
    files        integer NOT NULL DEFAULT 0,
    summary      text NOT NULL DEFAULT '',
    resolved_at  timestamptz,
    resolved_by  integer,
    note         text NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS paperrust_guard_finding (
    id        bigserial PRIMARY KEY,
    created   timestamptz NOT NULL DEFAULT now(),
    event_id  bigint,
    path      text NOT NULL,
    reason    varchar(32) NOT NULL,
    detail    text NOT NULL DEFAULT '',
    weight    integer NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS paperrust_guard_finding_event ON paperrust_guard_finding (event_id);
CREATE INDEX IF NOT EXISTS paperrust_guard_finding_created ON paperrust_guard_finding (created);

CREATE TABLE IF NOT EXISTS paperrust_backup_run (
    id            bigserial PRIMARY KEY,
    started       timestamptz NOT NULL DEFAULT now(),
    finished      timestamptz,
    status        varchar(16) NOT NULL DEFAULT 'running',   -- running | ok | error
    trig          varchar(16) NOT NULL DEFAULT 'manual',    -- scheduled | manual | manage
    target        text NOT NULL DEFAULT '',
    snapshot      varchar(64),
    files_total   bigint NOT NULL DEFAULT 0,
    files_new     bigint NOT NULL DEFAULT 0,
    bytes_total   bigint NOT NULL DEFAULT 0,
    bytes_new     bigint NOT NULL DEFAULT 0,
    db_bytes      bigint NOT NULL DEFAULT 0,
    pruned        integer NOT NULL DEFAULT 0,
    warnings      text NOT NULL DEFAULT '',
    error         text NOT NULL DEFAULT ''
);

-- Objects present at the target (name = SHA-256 of the plaintext), with retention period (S3 Object Lock).
CREATE TABLE IF NOT EXISTS paperrust_backup_object (
    target        text NOT NULL,
    id            char(64) NOT NULL,
    size          bigint NOT NULL,
    stored_at     timestamptz NOT NULL DEFAULT now(),
    retain_until  timestamptz,
    PRIMARY KEY (target, id)
);

-- Checksum cache: re-hash a file only if size or modification time differ.
CREATE TABLE IF NOT EXISTS paperrust_backup_filecache (
    path      text PRIMARY KEY,
    size      bigint NOT NULL,
    mtime_ns  bigint NOT NULL,
    sha256    char(64) NOT NULL
);
