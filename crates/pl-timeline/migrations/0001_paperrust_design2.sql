-- PaperRust UI (design 2): life areas, document data, timeline state, /ask history.
-- Expand-only (ARCHITECTURE-V2 §4): only new paperrust_* tables, no change to
-- Django tables, no foreign keys to them. Idempotent.
-- Rollback: DROP TABLE IF EXISTS paperrust_ask_message, paperrust_ask_conversation,
--          paperrust_timeline_state, paperrust_doc_date_state, paperrust_doc_date, paperrust_life_area;

CREATE TABLE IF NOT EXISTS paperrust_life_area (
    id        bigserial PRIMARY KEY,
    uuid      uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
    user_id   integer,                 -- NULL = instance default
    name      varchar(64) NOT NULL,
    color     varchar(7) NOT NULL,
    icon      varchar(32) NOT NULL,
    sort      bigint NOT NULL DEFAULT 0,
    rules     jsonb NOT NULL DEFAULT '{}'::jsonb,
    created   timestamptz NOT NULL DEFAULT now(),
    modified  timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS paperrust_life_area_user ON paperrust_life_area (user_id);

CREATE TABLE IF NOT EXISTS paperrust_doc_date (
    id           bigserial PRIMARY KEY,
    document_id  integer NOT NULL,
    kind         varchar(32) NOT NULL,
    date         date NOT NULL,
    source       varchar(16) NOT NULL,   -- content | user
    confidence   real NOT NULL DEFAULT 1,
    phrase       text,
    page         integer,
    note         text,
    created_by   integer,                -- source = user: creator (visible only to them)
    created      timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS paperrust_doc_date_doc ON paperrust_doc_date (document_id);
CREATE UNIQUE INDEX IF NOT EXISTS paperrust_doc_date_uniq
    ON paperrust_doc_date (document_id, kind, date, source, COALESCE(created_by, 0));

CREATE TABLE IF NOT EXISTS paperrust_doc_date_state (
    document_id  integer PRIMARY KEY,
    modified     timestamptz NOT NULL,   -- documents_document.modified at detection time
    version      integer NOT NULL,
    found        integer NOT NULL DEFAULT 0,
    extracted    timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS paperrust_timeline_state (
    user_id        integer NOT NULL,
    item           varchar(80) NOT NULL,
    document_id    integer NOT NULL,
    done_at        timestamptz,
    snoozed_until  date,
    updated        timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, item)
);

CREATE TABLE IF NOT EXISTS paperrust_ask_conversation (
    id        bigserial PRIMARY KEY,
    uuid      uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
    user_id   integer NOT NULL,
    title     varchar(80) NOT NULL,
    created   timestamptz NOT NULL DEFAULT now(),
    updated   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS paperrust_ask_conversation_user ON paperrust_ask_conversation (user_id, updated DESC, id DESC);

CREATE TABLE IF NOT EXISTS paperrust_ask_message (
    id               bigserial PRIMARY KEY,
    uuid             uuid NOT NULL UNIQUE DEFAULT gen_random_uuid(),
    conversation_id  bigint NOT NULL REFERENCES paperrust_ask_conversation (id) ON DELETE CASCADE,
    role             varchar(16) NOT NULL,   -- user | assistant
    content          text NOT NULL,
    citations        jsonb NOT NULL DEFAULT '[]'::jsonb,
    actions          jsonb NOT NULL DEFAULT '[]'::jsonb,
    meta             jsonb NOT NULL DEFAULT '{}'::jsonb,
    created          timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS paperrust_ask_message_conv ON paperrust_ask_message (conversation_id, id);
