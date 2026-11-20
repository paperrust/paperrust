-- Scanner integration (pl-scan, docs/contract/pl-scan.md): known eSCL scanners and devices for
-- SMTP intake. Expand-only: only new paperrust_* tables, no foreign keys to
-- Django tables. Idempotent.
-- Rollback: DROP TABLE IF EXISTS paperrust_scan_device, paperrust_scanner;

CREATE TABLE IF NOT EXISTS paperrust_scanner (
    id          bigserial PRIMARY KEY,
    name        varchar(128) NOT NULL,
    base_url    varchar(512) NOT NULL UNIQUE,   -- http(s)://host:port/eSCL/
    uuid        varchar(64),
    model       varchar(128),
    created     timestamptz NOT NULL DEFAULT now(),
    created_by  integer
);

CREATE TABLE IF NOT EXISTS paperrust_scan_device (
    id                  bigserial PRIMARY KEY,
    name                varchar(128) NOT NULL,
    token_hash          char(64) NOT NULL UNIQUE,   -- sha256(token), plaintext never stored
    token_hint          varchar(16) NOT NULL,
    owner_id            integer NOT NULL,           -- auth_user.id (owner of the documents)
    allowed_ips         text NOT NULL DEFAULT '',
    created             timestamptz NOT NULL DEFAULT now(),
    created_by          integer,
    revoked_at          timestamptz,
    last_used           timestamptz,
    last_ip             varchar(64),
    documents_received  bigint NOT NULL DEFAULT 0
);
