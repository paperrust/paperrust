# PaperRust – Synchronisation (Desktop-Modus 3/3b)

Stand 2026-11-09. Entscheidungsvorlage. Rahmen: docs/ARCHITECTURE-V2.md. Sync **über die API**, keine
DB-Replikation. Gegenstellen: PaperRust-Server (voll) und unveränderter paperless-ngx-Server
(eingeschränkt, §7).

## 1. Entscheidungen auf einen Blick

| Frage | Entscheidung | Kurzbegründung |
|---|---|---|
| Änderungserfassung | **DB-Trigger** (statement-level, Transition-Tables) → Warteliste → **Sequenzer** vergibt lückenlose `seq` | erfasst jeden Schreiber (damals auch Django im Mischbetrieb; Rust-v1, v2, Worker, psql); app-seitig würde Django und rohes SQL in > 10 Crates fehlen |
| Objekt-Identität | Seitentabelle `paperrust_sync_object` (UUID, `rev`, Tombstone) | keine Spalte an Django-Tabellen (expand-only, Rückweg) |
| Cursor | `seq` aus dem Sequenzer, nicht aus einer Sequenz im Trigger | Sequenzwerte im Trigger sind nicht in Commit-Reihenfolge → Lücken/Verluste |
| Tombstones | Zeile in `paperrust_sync_object` mit `deleted_at`, Feed-Eintrag `op=delete`, Aufbewahrung 180 Tage | — |
| Lokale DB im Client | **SQLite** (sqlx-sqlite, FTS5) | Client ist kein Server; mobil-tauglich; ein File; kein Prozess |
| Offline-Suche | SQLite **FTS5** über Titel/Inhalt/Notizen/Namen | transaktional mit den Metadaten, auch mobil |
| Auswahl | Dokumente, Tags, gespeicherte Ansichten; Auflösung serverseitig (`/sync/resolve`) | Ansichten nutzen v1-Filterregeln – nur der Server wertet sie sicher gleich aus |
| Konflikte (3b) | **Server gewinnt** + sichtbarer Hinweis mit lokaler Fassung | einfach, nachvollziehbar |
| Geheimnisse | Token im OS-Schlüsselbund, nie Passwörter | §8 |

## 2. Datenbank (Server, eigene Migration `pl_sync.0001_paperrust_sync`)

Expand-only, idempotent, eingespielt über `paperrust_migrations()` (`crates/pl-cli/src/migrate.rs`).
Voraussetzung PostgreSQL ≥ 14 (gilt bereits; `xid8`, `pg_current_xact_id()`, `gen_random_uuid()`
sind ab 13 im Kern).

```sql
-- Arten (stabil, nie umnummerieren)
-- 1 document, 2 tag, 3 correspondent, 4 document_type, 5 storage_path, 6 custom_field,
-- 7 saved_view, 8 user, 9 group, 10 share_link (später), 11 workflow (später)

CREATE TABLE IF NOT EXISTS paperrust_sync_object (
    kind        smallint    NOT NULL,
    local_id    bigint      NOT NULL,             -- id der Django-Zeile
    uuid        uuid        NOT NULL DEFAULT gen_random_uuid(),
    rev         bigint      NOT NULL DEFAULT 1,   -- +1 bei jeder erfassten Änderung (Optimistic Locking)
    seq         bigint,                           -- letzte Feed-Position (vom Sequenzer)
    deleted_at  timestamptz,                      -- Tombstone
    PRIMARY KEY (kind, local_id),
    UNIQUE (uuid)
);

-- Warteliste: von Triggern befüllt, vom Sequenzer geleert
CREATE TABLE IF NOT EXISTS paperrust_sync_pending (
    id          bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind        smallint NOT NULL,
    local_id    bigint   NOT NULL,
    perm_change boolean  NOT NULL DEFAULT false,  -- Ursache Eigentümer/guardian → Sichtbarkeit evtl. neu
    xid         xid8     NOT NULL DEFAULT pg_current_xact_id()
);

-- Feed: je Objekt genau ein Eintrag (der neueste) → Größe ~ Objekte + Tombstones
CREATE TABLE IF NOT EXISTS paperrust_sync_log (
    seq         bigint   PRIMARY KEY,
    kind        smallint NOT NULL,
    local_id    bigint   NOT NULL,
    uuid        uuid     NOT NULL,
    op          char(1)  NOT NULL CHECK (op IN ('u','d')),   -- upsert | delete
    rev         bigint   NOT NULL,
    perm_seq    bigint,                           -- seq des letzten Rechte-Wechsels (umgesetzt, §2a)
    at          timestamptz NOT NULL DEFAULT now(),
    UNIQUE (kind, local_id)
);
CREATE SEQUENCE IF NOT EXISTS paperrust_sync_seq;

CREATE TABLE IF NOT EXISTS paperrust_sync_meta (
    key   text PRIMARY KEY,   -- 'server_uuid', 'perm_epoch', 'min_cursor'
    value jsonb NOT NULL
);
```

### Trigger (welche Tabellen, welches Objekt)

Eine generische Funktion `paperrust_sync_capture()` als `AFTER INSERT/UPDATE/DELETE … REFERENCING
NEW TABLE AS n OLD TABLE AS o FOR EACH STATEMENT` (je Operation ein Trigger, weil Transition-Tables
nur für einzelne Ereignisse gehen). Argumente `TG_ARGV[0]` = Art, `TG_ARGV[1]` = Spalte mit der
Objekt-ID. Sie macht genau zwei Dinge, sortiert nach `local_id` (Deadlock-Vermeidung):
`INSERT … ON CONFLICT (kind,local_id) DO UPDATE SET rev = rev + 1, deleted_at = NULL` in
`paperrust_sync_object` und `INSERT` in `paperrust_sync_pending`.

| Tabelle (upstream v3.4.2) | → Art / ID-Spalte | Bemerkung |
|---|---|---|
| `documents_document` | 1 / `id` | inkl. Papierkorb (`deleted_at`, django-softdelete) |
| `documents_document_tags` | 1 / `document_id` | **wichtig:** `bulk_edit.add_tag` legt Zeilen per `bulk_create` an, ohne `modified` zu ändern (`upstream/src/documents/bulk_edit.py: add_tag`) |
| `documents_note` | 1 / `document_id` | Soft-Delete |
| `documents_customfieldinstance` | 1 / `document_id` | Soft-Delete |
| `documents_tag` | 2 / `id` | Baum (django-treenode schreibt bei Umhängen viele Zeilen – unkritisch) |
| `documents_correspondent`, `documents_documenttype`, `documents_storagepath`, `documents_customfield` | 3,4,5,6 / `id` | |
| `documents_savedview` / `documents_savedviewfilterrule` | 7 / `id` bzw. `saved_view_id` | |
| `auth_user`, `auth_group` | 8, 9 / `id` | nur Anzeige (Name), keine Geheimnisse im Feed |
| `guardian_userobjectpermission`, `guardian_groupobjectpermission` | Art aus `content_type_id` (Join `django_content_type`), ID = `object_pk::bigint` | `perm_change = true` |
| `auth_user_groups`, `auth_group_permissions`, `auth_user_user_permissions` | – | erhöhen nur `paperrust_sync_meta.perm_epoch` (Client gleicht ab, §4.5) |

`UPDATE` von `owner_id` an Objekttabellen setzt ebenfalls `perm_change` (Vergleich `o.owner_id IS
DISTINCT FROM n.owner_id` im Statement-Trigger). Die Trigger verändern keine Zeile der Django-Tabelle
und lösen keinen Fehler für den Schreiber aus, außer bei echten DB-Fehlern (dann rollt die Transaktion
zurück – gewollt, sonst ginge eine Änderung verloren).

### Sequenzer (warum zweistufig)

Eine Sequenz im Trigger vergibt Nummern beim **Schreiben**, sichtbar werden sie beim **Commit**: Tx A
erhält 50, Tx B 51, B committet zuerst, ein Client liest 51 und setzt den Cursor – A (50) ginge
verloren. Lösung: der Sequenzer nummeriert nur Einträge, deren Transaktion sicher abgeschlossen ist:

```sql
-- eine Instanz zur Zeit: pg_try_advisory_lock(hashtext('paperrust_sync_sequencer'))
WITH ready AS (
  DELETE FROM paperrust_sync_pending
  WHERE xid < pg_snapshot_xmin(pg_current_snapshot())
  RETURNING kind, local_id, perm_change
), agg AS (
  SELECT kind, local_id, bool_or(perm_change) AS perm_change FROM ready GROUP BY 1,2 ORDER BY 1,2
)
-- (Pseudocode des Entwurfs; umgesetzt in paperrust_sync_sequence(), Abweichungen §2a)
INSERT INTO paperrust_sync_log (seq, kind, local_id, uuid, op, rev, perm_change)
SELECT nextval('paperrust_sync_seq'), a.kind, a.local_id, o.uuid,
       CASE WHEN <Zeile existiert in Quelltabelle> THEN 'u' ELSE 'd' END, o.rev, a.perm_change
FROM agg a JOIN paperrust_sync_object o USING (kind, local_id)
ON CONFLICT (kind, local_id) DO UPDATE SET seq = EXCLUDED.seq, op = EXCLUDED.op, rev = EXCLUDED.rev,
       perm_change = EXCLUDED.perm_change, at = now();
-- danach: paperrust_sync_object.seq/deleted_at nachziehen; NOTIFY paperrust_sync
```

Da genau ein Sequenzer nummeriert und nur abgeschlossene Transaktionen sieht, ist `seq` in
Sichtbarkeitsreihenfolge lückenlos-sicher. Abgebrochene Transaktionen hinterlassen keine Pending-Zeilen
(sie rollen mit zurück). Ausführung: Schleife im Worker (`pl-server` `serve --worker`, wie der
Beat in `pl-queue`), geweckt per `NOTIFY` aus dem Trigger + 1 s Intervall; ohne Worker sammelt sich
`pending` nur an. Latenz typisch < 1 s; eine lange laufende Transaktion (z. B. großer Import) hält
`xmin` und damit den Feed bis zu ihrem Ende an – gewollt, nie Verlust.

Ob die Zeile existiert, prüft der Sequenzer je Art über die Quelltabelle (`documents_document` mit
`deleted_at IS NULL` → `u`, Papierkorb oder fehlend → `d`). Papierkorb ist für Clients „gelöscht“;
Wiederherstellen erzeugt wieder `u`.

### Erstbefüllung und Sonderfälle

- Migration legt für **alle bestehenden Zeilen** (Django-Ära) `paperrust_sync_object` an und schreibt
  einen Log-Eintrag je Objekt (Startzustand) – in Blöcken à 10 000, idempotent.
- `paperrust_sync_meta.server_uuid` wird einmal erzeugt. DB-Kopie Server ↔ Desktop (docs/DESKTOP.md
  „Umzug per Kopie“) kopiert sie mit → `paperrust manage sync reset-identity` (neue Server-UUID +
  `min_cursor` = aktuelle `seq`; alle Clients synchronisieren neu).
- `pl-dbmigrate`/`document_importer`: laufen vor der Sync-Migration (leere Ziel-DB) bzw. lösen die
  Trigger normal aus; großer Import → danach `paperrust manage sync rebuild` (Warteliste verwerfen,
  Log neu aufbauen) ist schneller.
- Aufbewahrung: Tombstones (`op='d'`) älter als `PAPERRUST_SYNC_TOMBSTONE_DAYS` (180) werden gelöscht,
  `min_cursor` = größte gelöschte `seq`. Cursor < `min_cursor` → `410` (§3).
- Rückweg/Abschalten: `paperrust manage sync disable` (Trigger weg, Tabellen bleiben),
  `… sync enable` (Trigger + `rebuild`).

### 2a. Umsetzung (P2, Stand 2026-11-09) und Abweichungen vom Entwurf

Crate `crates/pl-sync`, Migration `crates/pl-sync/migrations/0001_paperrust_sync.sql`
(`pl_sync.0001_paperrust_sync`), Vertrag `docs/contract/pl-sync.md`. Die gesamte Logik
(Trigger, Sequenzer, Neuaufbau, Aufbewahrung) liegt als SQL-Funktionen in der Migration; Rust ruft
sie nur auf. Abweichungen/Präzisierungen gegenüber §2:

1. **`perm_seq` statt `perm_change` im Log.** Ein boolesches Feld würde beim nächsten gewöhnlichen
   Update überschrieben; ein Client, dessen Cursor vor dem Rechte-Wechsel liegt, bekäme dann kein
   `delete` (Entzug ginge verloren). `perm_seq` = seq des letzten Rechte-Wechsels, bleibt bei
   späteren Änderungen erhalten; Feed-Regel: unsichtbar und `perm_seq > cursor` → `delete`.
   In `paperrust_sync_pending` bleibt `perm_change boolean`.
2. **Zwei NOTIFY-Kanäle:** Trigger → `paperrust_sync_pending` (weckt den Sequenzer), Sequenzer →
   `paperrust_sync` (weckt Long-Poll). Ein gemeinsamer Kanal hätte Long-Poller vor dem Nummerieren
   geweckt.
3. **Dokument-Versionen** (`documents_document.root_document_id`, v3.4.2): ID-Ausdruck
   `coalesce(root_document_id, id)` – eine Version ändert das Wurzel-Dokument; Versionen selbst sind
   keine Feed-Objekte (Existenzprüfung `root_document_id IS NULL AND deleted_at IS NULL`).
4. **`auth_user`:** zusätzlich Trigger `paperrust_sync_perm` – Änderung von `is_superuser`/`is_active`
   erhöht `perm_epoch` (verändert Sichtbarkeit); `last_login` u. a. nicht.
5. **Sequenzer sperrt nie gegen Schreiber:** `paperrust_sync_object.seq/deleted_at` werden mit
   `FOR UPDATE SKIP LOCKED` nachgezogen (eine gerade gesperrte Zeile hat selbst einen
   Wartelisten-Eintrag und folgt im nächsten Lauf). Damit keine Verklemmung Django ↔ Sequenzer.
   Advisory-Sperre `pg_try_advisory_xact_lock(hashtext('paperrust_sync_sequencer'))` je Transaktion;
   Neuaufbau, Aufbewahrung und `reset-identity` nehmen dieselbe Sperre.
6. **`rev`** steigt je erfasster *Anweisung* (nicht je Feldänderung); ein Django-`save()` mit
   M2M-Neusetzen erhöht `rev` daher um mehrere Schritte – monoton, für `If-Match` ausreichend.
7. **Erstbefüllung/Neuaufbau** mengenbasiert in einer Anweisung je Art (statt Blöcken à 10 000; die
   Trigger-Anlage sperrt die Tabellen ohnehin bis zum Commit der Migration, daher keine Lücke).
   Neuaufbau setzt `perm_seq` aller neu geschriebenen Einträge und erhöht `perm_epoch` (Rechte-
   Wechsel ohne Trigger sind nicht einzeln bekannt); Tombstones weiterhin fehlender Objekte bleiben
   unverändert; Dokumente im Papierkorb bekommen beim Neuaufbau keine neue Identität.
8. **Aufbewahrung** löscht mit dem Tombstone auch die Identität (`paperrust_sync_object`), sonst
   wüchse die Tabelle unbegrenzt. `min_cursor` = größte gelöschte seq.
9. **`manage sync`** zusätzlich: `sequence` (ein Durchlauf von Hand), `retention [--days N]`,
   `disable --purge` (vollständiger Rückbau inkl. Zeile in `paperrust_migrations`). `disable` setzt
   `paperrust_sync_meta.enabled = false`; ein erneutes `manage migrate` legt dann keine Trigger an.
10. **Sequenzer-Ort:** im Worker (`--worker`, `--worker-only`; `pl-server/src/worker.rs`
    `start_background`). Bis 2026-11-13 lief er im Mischbetrieb mit Celery-Brücke auch im
    HTTP-Prozess; die Brücke ist seit 2026-11-14 entfernt. Ohne Migration wartet er untätig. `PAPERRUST_SYNC_SEQUENCER=off` schaltet ab.
11. **Trigger-Funktionen erzeugt statt eine generische:** `paperrust_sync_install_triggers()` erzeugt aus
    *einer* Vorlage je (Tabelle, Operation) eine Funktion `paperrust_sync_cap_<tabelle>_<ins|upd|del>`
    mit statischem SQL; Art und ID-Ausdruck kommen weiter nur aus `paperrust_sync_tables()`
    (ARCHITECTURE-V2 §4.7 bleibt erfüllt). Grund: die generische Funktion mit `EXECUTE format(…)`
    plante jede Anweisung neu (~0,8 ms je schreibender Anweisung); statisch hält plpgsql den Plan je
    Verbindung (Folgeaufrufe ~0,2 ms, erster Aufruf je Sitzung ~5 ms). Existenzprüfung über die
    Sicht `paperrust_sync_live` (Join statt Funktion je Zeile: Erstbefüllung 100 000 Dokumente
    71 s → 4 s). `paperrust_sync_object` mit `fillfactor = 70` (HOT-Updates von `rev`).
12. Nicht erfasst: `TRUNCATE` (Django nutzt es nur in `flush`), Tabellen außerhalb der Liste (z. B.
    `documents_sharelink`, Workflows – Arten 10/11 später).

## 3. Sync-API v2 (Server)

Alle Pfade unter `/rs/api/v2/`. Anmeldung wie ARCHITECTURE-V2 §3 (Session+CSRF, Token, Geräte-Token).
Rechte immer über den Kern (`pl-authz`: `get_objects_for_user_owner_aware`); kein Parameter erweitert
Sichtbarkeit.

### 3.1 Server-Info

`GET /rs/api/v2/server` →
```json
{"server_uuid":"…","product":"paperrust","version":"0.9.0","api":2,
 "features":["sync","device_tokens","files_range"],"min_cursor":"c1.0","perm_epoch":17}
```
Ohne Anmeldung nur `product`, `api`, `features` (für die Erkennung im Client).

### 3.2 Änderungs-Feed

`GET /rs/api/v2/sync/changes?cursor=<c>&limit=500&kinds=document,tag,…&embed=true`

```json
{
  "changes": [
    {"seq":1042,"kind":"document","uuid":"…","op":"upsert","rev":7,"object":{…v2-Dokument…}},
    {"seq":1043,"kind":"tag","uuid":"…","op":"delete"}
  ],
  "next_cursor": "c1.1043",
  "has_more": false,
  "perm_epoch": 17
}
```

- Cursor opak (`c<format>.<seq>`), Start ohne `cursor` = vollständiger Stand ab `min_cursor`.
- `limit` 1–1000 (Standard 500). Der Server liest bis zu `limit` Log-Zeilen `seq > cursor ORDER BY seq`
  und filtert nach Sichtbarkeit: sichtbar → `upsert` (mit `object` bei `embed=true`); unsichtbar und
  `perm_seq > cursor` (Rechte-Wechsel nach dem Cursor, `pl_sync::Change::perm_changed_after`) →
  `{"op":"delete"}` (Entzug, für den Client ununterscheidbar von Löschen); sonst unsichtbar →
  ausgelassen. `op=d` wird **an alle** gemeldet (Zeile weg → Rechte nicht mehr
  prüfbar; verrät nur eine zufällige UUID). `next_cursor` = letzte **gelesene** `seq`, auch wenn alles
  gefiltert wurde.
- `410 {"error":{"code":"cursor_expired"}}` bei Cursor < `min_cursor` oder fremder `server_uuid` → Client
  macht Vollabgleich.
- Langes Warten: `&wait=25` (Sekunden) hält die Anfrage offen, bis `NOTIFY paperrust_sync` kommt
  (Long-Poll; kein WebSocket nötig, geht durch jeden Proxy).

### 3.3 Objekte abrufen

- `POST /rs/api/v2/objects/batch` `{"kind":"document","uuids":[…≤500],"include":["content","notes"]}` →
  `{"items":[…],"missing":[uuid…]}` (`missing` = gelöscht **oder** nicht sichtbar).
- Listen je Art (für die Light-UI, nicht für den Sync): `GET /rs/api/v2/tags?cursor=…&limit=…`.
- ID-Abgleich: `GET /rs/api/v2/sync/ids?kind=document` → `{"items":[["<uuid>",7],…],"as_of_seq":1043}`
  (alle sichtbaren UUIDs mit `rev`, gestreamt; ~45 Byte/Dokument → 100 000 Dokumente ≈ 4,5 MB, gzip
  ≈ 2 MB).

v2-Dokument (Auszug; Referenzen als UUID):
```json
{"uuid":"…","legacy_id":123,"rev":7,"title":"…","created":"2024-01-31","added":"…Z","modified":"…Z",
 "correspondent":"<uuid>|null","document_type":"<uuid>|null","storage_path":"<uuid>|null",
 "tags":["<uuid>"],"custom_fields":[{"field":"<uuid>","value":…}],"asn":42|null,
 "owner":{"uuid":"…","username":"…"}|null,"page_count":3,"mime_type":"application/pdf",
 "original_filename":"…","notes_count":1,
 "files":{"original":{"checksum":"…","size":12345},"archive":{"checksum":"…","size":23456}|null,
          "thumbnail":{"etag":"…"}},
 "access":{"can_change":true,"can_delete":true}}
```
`content` und `notes` nur mit `include`. Größen stammen aus dem Dateisystem (Cache im Server, nicht in
der DB).

### 3.4 Dateien

`GET /rs/api/v2/documents/{uuid}/files/{original|archive|thumbnail}`

- `ETag`: `"<checksum>"` für original/archive (Spalten `checksum`/`archive_checksum`), für thumbnail
  `W/"<mtime>-<size>"`. `If-None-Match` → `304`.
- `Range`/`If-Range` → `206` (Muster: `static.files` in `pl-server` kann das schon, vgl.
  docs/STANDALONE.md „WhiteNoise-Verhalten“). `Accept-Ranges: bytes`, `Content-Length` immer.
- `Content-Disposition: attachment; filename*=UTF-8''…`, `Cache-Control: private, max-age=0`.
- Rechte wie v1 `download` (`pl-files`).

### 3.5 Auswahl auflösen

`POST /rs/api/v2/sync/resolve`
```json
{"documents":["<uuid>"],"tags":["<uuid>"],"saved_views":["<uuid>"],"tag_mode":"any"}
```
→ `{"documents":[{"uuid":"…","rev":7,"size":{"original":…,"archive":…}}],"as_of_seq":1043,"skipped_saved_views":[]}`.
`skipped_saved_views`: angefragte Ansichten, deren Regeln der Server nicht auswerten kann (Volltextregeln,
unbekannte Regelarten), als UUIDs in Anfragereihenfolge; der Rest der Auswahl wird trotzdem aufgelöst.
Besteht die Anfrage nur aus solchen Ansichten → `501`.
Gespeicherte Ansichten werden mit **ihren v1-Filterregeln** über `pl-docfilter` ausgewertet (übergangsweise,
ARCHITECTURE-V2 §1); Tags inkl. Unter-Tags wie upstream (`tags__id__all`/`__in` mit Nachfahren).

### 3.6 Pagination allgemein

Cursor-basiert (`next_cursor`), stabil bei gleichzeitigen Änderungen (Sortierschlüssel + `uuid` als
Tie-Breaker). Kein `count` ohne `?count=true` (teuer bei 100 000 Dokumenten, vgl. docs/contract/pgvalidate.md).

## 4. Client: lokales Replikat (`crates/pl-sync-client`)

### 4.1 Warum SQLite und nicht das eingebettete PostgreSQL

| | eingebettetes PG (Modus 1) | **SQLite** |
|---|---|---|
| Mobil (Tauri iOS/Android) | nicht möglich | ja |
| Prozesse/Ports | Cluster-Prozess, Port, Rolle, Passwort (docs/DESKTOP.md §2–5) | keine (Datei) |
| Start/RAM | Sekunden, ~30–60 MB | ms, wenige MB |
| Schema | Django-Schema nötig, um die Server-Logik wiederzuverwenden | eigenes, schlankes v2-Schema |
| Paketgröße Modus 2/3 | PG-Binaries Pflicht | nichts extra (libsqlite3 gebündelt, sqlx-Feature `sqlite`) |

Das Replikat ist **kein Server**: es braucht weder Rechteauswertung (der Server liefert nur Sichtbares)
noch v1-Filter-DSL. Ein lokaler PaperRust-Server als Replikat würde die Django-Schema-Bindung
in den Client tragen – genau das, was ARCHITECTURE-V2 vermeiden will. Modus 1 bleibt beim eingebetteten
PG (ein Codepfad mit dem Server, Entscheidung 2026-11-07, docs/STANDALONE.md). Workspace hat `sqlx 0.8`
bereits; benötigt wird nur das Feature `sqlite` (prüfen: FTS5 im gebündelten libsqlite3 aktiv – im Spike
P5 nachweisen).

### 4.2 Schema (lokal, eine Datei je Server-Profil `sync/<profile-id>/replica.sqlite`)

```sql
CREATE TABLE remote   (id INTEGER PRIMARY KEY CHECK (id = 1), url TEXT NOT NULL, flavor TEXT NOT NULL
                       CHECK (flavor IN ('paperrust','ngx')), server_uuid TEXT, user_name TEXT,
                       cursor TEXT, perm_epoch INTEGER, ngx_watermark TEXT, last_full_sync TEXT);
CREATE TABLE object   (kind TEXT NOT NULL, key TEXT NOT NULL, rev INTEGER, json TEXT NOT NULL,
                       name TEXT, PRIMARY KEY (kind, key));          -- Tags, Korrespondenten, …
CREATE TABLE document (key TEXT PRIMARY KEY, legacy_id INTEGER, rev INTEGER, title TEXT, created TEXT,
                       modified TEXT, correspondent TEXT, document_type TEXT, asn INTEGER,
                       json TEXT NOT NULL, content TEXT, in_selection INTEGER NOT NULL DEFAULT 1,
                       fingerprint TEXT);                            -- fingerprint: §7
CREATE TABLE document_tag (doc TEXT, tag TEXT, PRIMARY KEY (doc, tag));
CREATE TABLE selection (type TEXT CHECK (type IN ('document','tag','saved_view')), key TEXT,
                        files TEXT NOT NULL DEFAULT 'preferred'      -- preferred|original|both|none
                        , PRIMARY KEY (type, key));
CREATE TABLE file     (doc TEXT, variant TEXT, path TEXT, etag TEXT, size INTEGER, fetched TEXT,
                       last_access TEXT, PRIMARY KEY (doc, variant));
CREATE VIRTUAL TABLE fts USING fts5(title, content, notes, names, content='', tokenize='unicode61 remove_diacritics 2');
CREATE TABLE outbox   (...);  -- 3b, §6
```

`key` = UUID (PaperRust) bzw. `ngx:<id>` (paperless-ngx). Metadaten-Arten (Tags, Korrespondenten,
Typen, Speicherpfade, Custom Fields, gespeicherte Ansichten, Nutzer-Namen) werden **immer vollständig**
gespiegelt (klein), Dokumente nur gemäß Auswahl.

### 4.3 Dateicache

`sync/<profile-id>/files/<uuid[0..2]>/<uuid>.<variant>` (`variant` = original|archive|thumb). Schreiben
in `*.part` + Umbenennen; Prüfung gegen `checksum` bei original/archive. `preferred` = Archiv-PDF, falls
vorhanden, sonst Original; Vorschaubilder immer für alle ausgewählten Dokumente. Rechte des Ordners wie
docs/DESKTOP.md §18 (M4: nur aktueller Benutzer). Keine eigene Verschlüsselung im MVP; Empfehlung
Datenträgerverschlüsselung (BitLocker/FileVault), im Einstellungsdialog angezeigt.

### 4.4 Auswahl und Grenzen

- Wählbar: einzelne Dokumente, Tags (inkl. Unter-Tags), gespeicherte Ansichten. Ergebnis = Vereinigung.
- Grenzen je Profil (Einstellungen): `max_bytes` Standard **2 GiB**, `max_documents` Standard **5 000**.
  Vor dem Übernehmen zeigt der Client Anzahl und Größe (aus `/sync/resolve`) und verweigert über der Grenze
  (Hinweis, welche Auswahl wie viel beiträgt). Ohne Dateien (`files=none`) zählen nur Metadaten (+ Text).
- Fällt ein Dokument aus der Auswahl: Metadaten bleiben 7 Tage (`in_selection=0`, für „zuletzt
  gesehen“), Dateien werden sofort gelöscht.

### 4.5 Ablauf

1. **Erstabgleich:** Metadaten über `/sync/changes` ohne Cursor (nur Metadaten-Arten), dann `/sync/resolve`,
   dann `objects/batch` (mit `content`, `notes`), dann Dateien (parallel 4, Fortsetzbar über Range).
2. **Laufend:** Long-Poll `/sync/changes?cursor=…&wait=25`; Dokumente außerhalb der Auswahl werden
   ignoriert, aber auf Auswahl-Relevanz geprüft (Tag-Änderung kann ein Dokument in die Auswahl bringen) →
   bei Änderungen an Dokumenten/Tags/Ansichten `resolve` gedrosselt (≤ 1× / 30 s) erneut.
3. **perm_epoch geändert, `410`, oder alle 24 h:** Vollabgleich `/sync/ids` → lokal fehlende holen, lokal
   überzählige löschen.
4. Dateien bei `files.*.checksum`-Änderung neu laden; Vorschaubild per `If-None-Match`.

### 4.6 Offline-Suche

FTS5 über `title`, `content`, `notes`, `names` (Korrespondent, Typ, Tags als Text). Abfrage-Syntax der
Light-UI-Suchleiste: Wörter UND-verknüpft, Präfix (`rech*`), Phrase in `"…"`; Filter (Tag, Korrespondent,
Typ, Datum) als SQL auf `document`/`document_tag`. Bewusst **nicht** die tantivy/Whoosh-Syntax des
Servers (Altlast L3); die UI zeigt offline „Suche im lokalen Bestand“.

## 5. Light-UI und Replikat (Desktop/Mobil)

Die Tauri-Hülle stellt den Client-Kern unter einem eigenen URI-Schema bereit (`prapi://`, unter Windows
`http://prapi.localhost/`), das **dieselben v2-Pfade und -Formen** spricht: online → weitergereicht an den
Server (Token aus dem Schlüsselbund, TLS-Pinning im Kern), offline oder bei `?source=local` → aus SQLite.
Antworten tragen `X-PaperRust-Source: remote|local`. Damit braucht die Light-UI nur **einen**
API-Client (Details docs/REMOTE-APP.md §5, docs/FRONTEND-LIGHT.md §5).

## 6. 3b: Outbox und Konflikte (später)

```sql
CREATE TABLE outbox (id INTEGER PRIMARY KEY, created TEXT, op TEXT CHECK (op IN
  ('patch_document','add_note','upload','delete_document','bulk_tags')), target TEXT, base_rev INTEGER,
  payload TEXT NOT NULL, idempotency_key TEXT UNIQUE, state TEXT DEFAULT 'queued'
  CHECK (state IN ('queued','sending','done','conflict','failed')), error TEXT, attempts INTEGER DEFAULT 0);
```

- Lokale Änderung wird sofort im Replikat angezeigt (markiert „nicht gesendet“) und in die Outbox gelegt.
- Senden in Reihenfolge, je Ziel seriell. PaperRust: `PATCH /rs/api/v2/documents/{uuid}` mit
  `If-Match: "<base_rev>"` + `Idempotency-Key`; `412` → **Konflikt**.
- **Server gewinnt:** Replikat übernimmt Serverstand; die lokale Fassung bleibt als Konflikteintrag
  (state `conflict`, Payload + Serverstand) und erscheint als Hinweis am Dokument („Deine Offline-Änderung
  wurde nicht übernommen – anzeigen / erneut anwenden / verwerfen“). Kein automatisches Zusammenführen.
- Hochladen: `POST /api/documents/post_document/` (v1, beide Servertypen) mit Original; Duplikat-Ablehnung
  des Servers (gleiche Prüfsumme) gilt als erledigt. Offline erfasste Dokumente erscheinen lokal als
  „wartet auf Verarbeitung“ (keine lokale OCR im MVP).
- paperless-ngx: kein `If-Match` → vor `PATCH /api/documents/<id>/` `modified` lesen; neuer als Basis →
  Konflikt (Restfenster dokumentiert).

## 7. Eingeschränkter Modus gegen paperless-ngx (unverändert)

Erkennung: `GET /rs/api/v2/server` → 404, `GET /api/` mit Token liefert `X-Api-Version`/`X-Version`
(`upstream/src/paperless/middleware.py`). Zugriff nur über v1 mit `Accept: application/json; version=9`
(wegen `all`, s. u.).

| Aufgabe | Weg | Grenze |
|---|---|---|
| Metadaten | alle 5 min `GET /api/tags/?page_size=100000` usw. vollständig, Vergleich per Hash | klein, unkritisch |
| Geänderte Dokumente | `GET /api/documents/?modified__gt=<watermark>&ordering=modified&page_size=100&fields=…` (`modified` hat `DATETIME_KWARGS`, `upstream/src/documents/filters.py`); Watermark = größtes gesehenes `modified` minus 2 s Überlappung | **verpasst** Tag-Änderungen per Massenbearbeitung und Löschen von Tags/Notizen (kein `modified`-Update, s. §2) |
| … Ausgleich | alle 15 min **Fingerabdruck-Abgleich** der Auswahl: `id__in=<≤100 ids>&fields=id,modified,tags,correspondent,document_type,storage_path,custom_fields,notes,archive_serial_number,title,created` → Hash je Dokument vergleichen | Last ∝ Auswahl, nicht Bestand |
| Gelöschte/entzogene | `GET /api/documents/?page_size=1` mit `version=9` → Feld `all` = **alle sichtbaren IDs** (`upstream/src/paperless/views.py: StandardPagination.get_all_result_ids`); lokal fehlende = gelöscht | v9 könnte in künftigen upstream-Versionen entfallen → Ersatz: `fields=id&page_size=100000` |
| Auswahl auflösen | Tags: `tags__id__in`/`__all`; gespeicherte Ansicht: ihre Filterregeln → Query-Parameter (gleiche Abbildung wie Angular `upstream/src-ui/src/app/utils/query-params.ts`, `filter-rules.ts`), in Rust nachgebaut im Client | Regeltypen ohne Abbildung → Ansicht nicht wählbar (Hinweis) |
| Dateien | `/api/documents/<id>/download/?original=true`, `/thumb/`, `/preview/`; ETag/Range nicht garantiert → Vergleich über `checksum` aus der Liste | vollständige Downloads |
| Anmeldung | `/api/token/` (Benutzer, Passwort, ggf. `code` für TOTP – `PaperlessAuthTokenSerializer`) | ein Token je Nutzer, nicht je Gerät widerrufbar |

## 8. Sicherheit

- **Geräte-Token (PaperRust):** `POST /rs/api/v2/auth/devices` `{"username","password","code"?,"device_name"}`
  → `{"token":"prt_<32 Byte base62>","device":{"uuid","name"}}` (einmal angezeigt). Server speichert nur
  `sha256(token)` in `paperrust_device_token(id, uuid, user_id, name, token_hash, created, last_used,
  revoked_at)`; Liste/Widerruf `GET/DELETE /rs/api/v2/auth/devices[/uuid]`, sichtbar in der Profilseite.
  Gleiches Login-Throttling wie `/api/token/` (Scope `login`). MFA-Pflicht wie upstream (`code`).
- **Client:** Token nur im OS-Schlüsselbund (Windows Credential Manager, macOS Keychain, Linux Secret
  Service; Crate `keyring`, neue Abhängigkeit – nur in `pl-sync-client`/Hülle). Kein Secret Service → Token
  nur im Speicher, Neuanmeldung je Start. **Passwörter werden nie gespeichert**, nur für den Token-Abruf
  im Speicher gehalten und danach überschrieben (`zeroize`).
- **TLS:** rustls (Workspace: `reqwest` mit `rustls-tls`, `rustls-native-certs`), Systemvertrauen. Selbst
  signiert: einmalige Bestätigung des SHA-256-Fingerabdrucks (SPKI) → Pinning für dieses Profil;
  geänderter Fingerabdruck = harter Fehler mit Hinweis. `http://` nur für Loopback/private Netze nach
  ausdrücklicher Bestätigung (Warnbanner bleibt).
- Profil entfernen / Abmelden → Token im Server widerrufen (wenn erreichbar), Schlüsselbund-Eintrag,
  Replikat und Dateicache löschen.
- Server: v2 prüft Rechte bei **jedem** Abruf; Feed, `batch`, `ids`, `resolve`, Dateien nie ohne Rechteprüfung.
  Long-Poll begrenzt (≤ 30 s, ≤ 4 offene je Token).

## 9. Testplan (nur lokale Testserver, nie Produktion, nie echte Nutzerdaten)

| Ebene | Inhalt | Wo |
|---|---|---|
| Unit Server | Trigger je Tabelle (Insert/Update/Delete/M2M/guardian/owner), Sequenzer-Reihenfolge mit zwei parallelen Transaktionen (A vor B begonnen, B zuerst committet → beide im Feed, A nicht übersprungen), Tombstone-Aufbewahrung/`410`, Sichtbarkeitsfilter (`perm_change`), Erstbefüllung idempotent | `crates/pl-sync/tests` gegen Wegwerf-DB (Muster `tests/pgvalidate`) |
| Parität | voller v1-Lauf mit und ohne Trigger, identische Ergebnisse | `tests/parity` (`PARITY_SYNC_CAPTURE=on`) |
| Django-Schreiber | Referenz-Django (`reference/`, Port 8000) schreibt (bulk_edit add_tag, Notiz, Löschen) → Feed enthält alles | `tests/sync/django_capture` |
| Last | 100 000 Dokumente (Daten aus docs/contract/pgvalidate.md): Erstbefüllung, `bulk_edit` 10 000 Dokumente, Feed-Durchsatz, Größe `ids` | Bench, Ergebnis `results/sync/` |
| Client | SQLite-Schema-Migration, Abgleich-Logik gegen einen Mock-Remote (aufgezeichnete Antworten), FTS5-Suche, Grenzen, Dateicache-Wiederaufnahme | `crates/pl-sync-client/tests` |
| E2E PaperRust | lokaler PaperRust-Testserver (Kopie der Referenzdaten, eigener Port) + Client: Auswahl → offline (Server stoppen) → Lesen/Suchen/PDF öffnen → Server ändert/löscht/entzieht Recht → Client nach Neustart konsistent | `tests/sync/e2e.mjs` |
| E2E paperless-ngx | dieselben Fälle gegen die Referenz-Django-Instanz (unverändertes upstream, `reference/README.md`) | dito, `--flavor ngx` |
| Sicherheit | Token nie im Log/Replikat/Dateisystem (grep über Profilordner), Pinning-Wechsel, Rechteentzug, fremde `server_uuid` | `tests/security` |

Testdaten: ausschließlich `reference/` (Testnutzer und -Tokens aus `reference/README.md`) bzw. synthetisch
hochskaliert. Keine Verbindung zu Produktivservern in Tests oder CI.
