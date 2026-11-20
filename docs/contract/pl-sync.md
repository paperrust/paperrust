# Vertrag `pl-sync` – Änderungserfassung für den Sync

Paket P2 (docs/ROADMAP-NEXT.md). Entwurf und Begründung: docs/SYNC.md §2, Umsetzung und
Abweichungen: docs/SYNC.md §2a. Rahmen: docs/ARCHITECTURE-V2.md §4 (expand-only, v1-Parität).

Crate `crates/pl-sync` (ohne axum, ohne v1-Crates), Migration
`crates/pl-sync/migrations/0001_paperrust_sync.sql`, DB-Tests `crates/pl-sync/tests/db.rs`,
Django-Schreiber-Prüfung `tests/sync/django_capture.py`, Lastmessung `tests/sync/load_bench.py`
(Ergebnisse `results/sync/`).

## 1. Schema (alle Objekte mit Präfix `paperrust_sync`)

| Objekt | Zweck |
|---|---|
| `paperrust_sync_object (kind, local_id) → uuid, rev, seq, deleted_at` | stabile Identität (UUID) und Revision je Objekt |
| `paperrust_sync_pending (id, kind, local_id, perm_change, xid)` | Warteliste der Trigger (`xid` = `pg_current_xact_id()`) |
| `paperrust_sync_log (seq PK, kind, local_id UNIQUE, uuid, op u/d, rev, perm_seq, at)` | Feed, je Objekt genau ein (neuester) Eintrag |
| `paperrust_sync_seq` | Feed-Nummern (nur der Sequenzer zieht sie) |
| `paperrust_sync_meta (key, value jsonb)` | `server_uuid`, `perm_epoch`, `min_cursor`, `enabled` |

Arten: 1 document, 2 tag, 3 correspondent, 4 document_type, 5 storage_path, 6 custom_field,
7 saved_view, 8 user, 9 group (stabil, `pl_sync::Kind`).

SQL-Objekte: `paperrust_sync_tables()` (erfasste Tabellen: Art, ID-Ausdruck, `'owner'`),
`paperrust_sync_capture_body(src)`/`paperrust_sync_epoch_body(cond)` (Vorlagen),
`paperrust_sync_install_triggers()` (erzeugt je Tabelle/Operation `paperrust_sync_cap_<tabelle>_<op>()`
mit statischem SQL und den Trigger), `paperrust_sync_drop_triggers()` (entfernt Trigger und erzeugte
Funktionen), Sicht `paperrust_sync_live (kind, local_id)` (lebende Objekte), `paperrust_sync_sequence(limit)`,
`paperrust_sync_rebuild()`, `paperrust_sync_retention(days)`.

### Trigger (55 auf v3.4.2)

`AFTER INSERT/UPDATE/DELETE … REFERENCING NEW TABLE AS paperrust_n / OLD TABLE AS paperrust_o FOR
EACH STATEMENT` (Namen `paperrust_sync_ins|upd|del`):

| Tabelle | Art / ID | perm |
|---|---|---|
| `documents_document` | 1 / `coalesce(root_document_id, id)` | `owner_id`-Wechsel |
| `documents_document_tags`, `documents_note`, `documents_customfieldinstance` | 1 / `document_id` | – |
| `documents_tag`, `documents_correspondent`, `documents_documenttype`, `documents_storagepath` | 2–5 / `id` | `owner_id`-Wechsel |
| `documents_customfield` | 6 / `id` | – |
| `documents_savedview` / `documents_savedviewfilterrule` | 7 / `id` bzw. `saved_view_id` | `owner_id` (savedview) |
| `auth_user`, `auth_group` | 8, 9 / `id` | – |
| `guardian_userobjectpermission`, `guardian_groupobjectpermission` | Art aus `django_content_type` (`documents.*`), ID = `object_pk` (nur numerisch) | immer |
| `auth_user_groups`, `auth_group_permissions`, `auth_user_user_permissions` | – | nur `perm_epoch + 1` |
| `auth_user` (`paperrust_sync_perm`, UPDATE) | – | `perm_epoch + 1` bei `is_superuser`/`is_active`-Wechsel |

Jeder Trigger: `INSERT … ON CONFLICT DO UPDATE SET rev = rev + 1, deleted_at = NULL` in
`paperrust_sync_object` (nach `local_id` sortiert) und eine Zeile je Objekt in
`paperrust_sync_pending`, dann `pg_notify('paperrust_sync_pending', '')`. Kein Schreibzugriff auf
Django-Tabellen, kein Fehler für den Schreiber außer echten DB-Fehlern. Statements ohne Zeilen
schreiben nichts.

### Sequenzer

`paperrust_sync_sequence(limit)`: Advisory-Sperre (try, je Transaktion) → `xmin` des eigenen
Snapshots merken → in **einer** Anweisung: Warteliste mit `xid < xmin` löschen (höchstens `limit`),
je Objekt zusammenfassen, Log-Upsert mit `nextval` in `(kind, local_id)`-Reihenfolge, `op` per Join auf
`paperrust_sync_live` (Papierkorb/Version/fehlend → `d`), `perm_seq` = neue seq bei Rechte-Wechsel
sonst bisheriger Wert, `paperrust_sync_object.seq/deleted_at` mit `SKIP LOCKED` nachziehen →
`pg_notify('paperrust_sync', '')`. Rückgabe: verarbeitete Zeilen, `-1` = anderer Sequenzer aktiv.

Garantie: Eine Transaktion, die vor einer anderen begonnen hat und später committet, wird nie
übersprungen – ihre Einträge erhalten eine seq, die größer ist als jede vorher sichtbare (Test
`late_committing_transaction_is_not_skipped`).

## 2. Rust-API (`pl_sync`)

| Funktion | Zweck |
|---|---|
| `migrations::{ALL, NAME_0001, M0001_SYNC, apply}` | Migration; `ALL` ist in `pl_cli::migrate::paperrust_migrations()` registriert |
| `installed`, `status`, `read_meta` | Zustand (`Status` serialisierbar) |
| `sequence_once(pool, batch)` | Warteliste abarbeiten (Schleife über `paperrust_sync_sequence`) |
| `sequencer::run(pool, SyncConfig, stop)` | Schleife: `LISTEN paperrust_sync_pending` + Intervall, stündlich Aufbewahrung |
| `retention(pool, days)`, `rebuild`, `enable`, `disable(pool, purge)`, `reset_identity` | Verwaltung |
| `changes(pool, after, limit, kinds) -> Page` | Feed-Seite ohne Rechtefilter (für `pl-api-v2`), REPEATABLE READ; `CursorExpired` bei `after < min_cursor` |
| `Change::perm_changed_after(cursor)` | Regel „unsichtbar → delete“ (SYNC.md §3.2) |
| `wait_for_changes(pool, after, timeout)` | Long-Poll-Hilfe (`LISTEN paperrust_sync`) |
| `format_cursor`/`parse_cursor` | `c1.<seq>` |
| `manage::command(pool, argv)` | `paperrust manage sync …` (eingebunden in pl-cli) |

Einstellungen (`SyncConfig::from_lookup`): `PAPERRUST_SYNC_SEQUENCER` (an), `PAPERRUST_SYNC_TOMBSTONE_DAYS`
(180), `PAPERRUST_SYNC_INTERVAL` (1 s), `PAPERRUST_SYNC_RETENTION_INTERVAL` (3600 s),
`PAPERRUST_SYNC_BATCH` (50 000).

## 3. Betrieb

- Einspielen: `paperrust manage migrate` (Teil von `paperrust_migrations()`), Erstbefüllung nur bei
  leerem Log. Der Sequenzer läuft in `paperrust serve --worker|--worker-only` (bis 2026-11-14 zusätzlich
  im Mischbetrieb mit Celery-Brücke im HTTP-Prozess); mehrere Prozesse sind sicher.
- `paperrust manage sync status [--json] | enable | disable [--purge] | rebuild | reset-identity |
  sequence | retention [--days N]`.
- **Rückweg zu paperless-ngx:** `manage sync disable` entfernt alle Trigger (Tabellen bleiben,
  Django stören sie nicht); `--purge` entfernt zusätzlich alle `paperrust_sync_*`-Objekte. Ohne
  `disable` arbeiten die Trigger unter Django weiter, die Warteliste wächst dann aber ohne
  Sequenzer – vor einem dauerhaften Wechsel deshalb `disable`.
- Großer Import: danach `manage sync rebuild` (Warteliste verwerfen, alle Objekte neu nummerieren).
- DB-Kopie Server ↔ Desktop: `manage sync reset-identity`.

## 4. Nachweise

- `cargo test -p pl-sync` (5 Unit-Tests) und mit
  `PAPERRUST_SYNC_TEST_DATABASE_URL=postgres://paperless:paperless@127.0.0.1:54329/pl_sync_test`
  `cargo test -p pl-sync -- --include-ignored` (10 DB-Tests; DB-Name muss mit `pl_sync_test` beginnen,
  leere DB bekommt das v3.4.2-Schema aus `crates/pl-dbmigrate/schema`).
- Parität: `tests/parity` mit `PARITY_SYNC_CAPTURE=on` (w2-env.sh/w2_reset.py: Reset aus
  `<Vorlage>_sync`; eigene Kopie über `W2_NAME`/`W2_DJ_PORT`/`W2_RS_PORT`), Bericht vermerkt den Schalter;
  Zusammenfassung `results/sync/parity-capture-2026-11-09.md`.
- API v2: `pl-server/src/api_v2.rs` `SyncFeed` setzt `pl_domain::ChangeFeed` um; Test
  `PL_P3_TEST_DB=1 cargo test -p pl-server --test api_v2_db -- --ignored` (Feed-Fall „sync: changes“).
- Django-Schreiber: `tests/sync/django_capture.py`; Last: `tests/sync/load_bench.py` →
  `results/sync/load-*.md`.

## 5. Offene Punkte (für P3 ff.)

- Rechtefilter des Feeds, Einbettung der Objekte, `sync/ids`, `sync/resolve`, Long-Poll-Begrenzung
  (≤ 4 je Token) gehören zu `pl-api-v2`.
- Arten 10/11 (share_link, workflow) noch nicht erfasst.
