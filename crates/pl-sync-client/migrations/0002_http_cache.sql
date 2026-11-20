-- pl-sync-client 0002: response cache for v1 framework endpoints (ui_settings, master data lists,
-- statistics …), filled online via the gateway, delivered offline (docs/contract/pl-sync-client.md).
CREATE TABLE http_cache (
    key          TEXT PRIMARY KEY,      -- path + query (without `source`)
    status       INTEGER NOT NULL,
    content_type TEXT,
    body         BLOB NOT NULL,
    stored       TEXT NOT NULL
) WITHOUT ROWID;
