-- pl-sync-client 0004: the response cache is keyed by the owning account
-- (`<user_name>\x1f<path?query>`) and never stores credential fields (e.g. `auth_token` of
-- `/api/profile/`). Older entries have no owner and may contain such fields → discard them all;
-- they are filled again on the next online request (docs/contract/pl-sync-client.md).
DELETE FROM http_cache;
