# Vertrag: API v2 Kern (`pl-domain`, `pl-store`, `pl-api-v2`) – P3/P3b

Stand 2026-11-09. Rahmen: docs/ARCHITECTURE-V2.md, docs/SYNC.md §3/§8, docs/FRONTEND-LIGHT.md §6.
OpenAPI: docs/api/v2.yaml. Geräte-Token in pl-authz: docs/contract/pl-authz.md („Geräte-Token“).

## Schichten

```
pl-server (Montage /rs/api/v2, PAPERRUST_API_V2, v1-Schreibrouter)
   └─ pl-api-v2  (axum-Router, v2-Fehlerformat, Cursor, Dateien mit ETag/Range, Geräte-Endpunkte)
        └─ pl-domain  (Typen, DocQuery-AST, Traits Repo + ChangeFeed; ohne axum/sqlx)
             ↑
        pl-store   (Repo auf Django-Tabellen + paperrust_sync_object; Rechte über pl-authz,
                    Filter übergangsweise pl_docfilter, Dateipfade pl-files, Schreiben über v1-Handler)
```

- `cargo tree -p pl-api-v2 -e normal | grep "pl-documents "` ist leer (geprüft 2026-11-09): weder
  `pl-documents` (DRF-Serialisierer) noch `pl_core::pagination` werden benutzt. `pl-documents-write`
  wird nicht verlinkt; v2-Schreibzugriffe laufen über einen **internen Router** mit den v1-Handlern,
  den `pl-server` baut (`pl_server::api_v2::LEGACY_WRITE_ROUTES`) und `pl-store` als
  `LegacyRouter` erhält. Dadurch gelten Validierung, Rechte, Audit-Log, Workflows, Index und
  Dateinamen exakt wie in v1; der v1-JSON-Körper existiert nur in `pl-store/src/legacy.rs`.
- Rechte entscheidet immer `pl-authz` (`permitted_objects_filter`, Modellrechte, guardian). v2
  bildet sie auf `access {can_view, can_change, can_delete}` und optional `sharing` ab
  (`include=sharing`, nur Eigentümer/Superuser).

## Montage (pl-server)

- `crates/pl-server/src/api_v2.rs` (neu), `lib.rs::build_app` nestet den Router unter `/rs/api/v2`
  **vor** dem Django-Fallback; nicht in `routes.toml`. `PAPERRUST_API_V2=off|false|0|no` schaltet ab
  (`ServerConfig::api_v2`).
- `frontline::must_go_to_django`: Pfade unter `/rs/api/v2` gehen nie an Django (auch %-kodiert).
- Unbekannte Pfade/Methoden unter dem Präfix: v2-Fehler 404/405 (nie Djangos Seite).

## Endpunkte (alle unter `/rs/api/v2`)

| Methode | Pfad | Anmerkung |
|---|---|---|
| GET | `/server` | ohne Anmeldung nur `product`, `api`, `features`; angemeldet + `version`, `server_uuid`, `min_cursor`, `perm_epoch` (ohne Feed `null`) |
| GET | `/me` | Nutzer, Gruppen, Modellrechte, `auth` = Anmeldeweg |
| GET/PUT | `/ui_settings` | `{"settings":{…}}`; PUT über v1 `ui_settings.post` |
| GET | `/documents` | `cursor`, `limit` (1–1000, Std. 100), `sort`, `include`, `count` |
| POST | `/documents/query` | `DocQuery` (pl-domain `query.rs`) |
| GET/PATCH | `/documents/{uuid}` | `include=content,notes,sharing`; PATCH mit `If-Match: "<rev>"` |
| GET/POST | `/documents/{uuid}/notes` | POST `{"note":…}` über v1 |
| DELETE | `/documents/{uuid}/notes/{legacy_id}` | über v1 |
| GET/HEAD | `/documents/{uuid}/files/{original\|archive\|preview\|thumbnail}` | ETag, 304, Range/If-Range, 206/416 |
| GET | `/{tags,correspondents,document_types,storage_paths,custom_fields,saved_views,users,groups}` | `cursor`, `limit`, `sort=name\|-name\|id\|-id`, `name_contains`, `count`, `document_count`, `include=sharing` |
| GET | `/{…}/{uuid}` | ETag `"<rev>"` |
| POST/PATCH/DELETE | `/{tags,correspondents,document_types}[/{uuid}]` | über v1, `If-Match` |
| POST | `/objects/batch` | `{"kind","uuids"≤500,"include"}` → `{"items","missing"}` |
| GET | `/sync/changes` | **501 bis P2 eingehängt** (Logik inkl. Rechtefilter/Entzug fertig, s. u.) |
| GET | `/sync/ids?kind=` | `{"items":[[uuid,rev],…],"as_of_seq"}` |
| POST | `/sync/resolve` | Dokumente, Tags (any/all), gespeicherte Ansichten (v1-Regeln) |
| POST/GET | `/auth/devices` | Ausgabe (Benutzer/Passwort/`code`/`device_name`) bzw. eigene Liste |
| DELETE | `/auth/devices/{uuid}` | eigenes Gerät widerrufen, sofort wirksam |
| GET/POST/DELETE | `/asn/{presets,config,preview,labels,printed[/{id}],separator}` | ASN-Etiketten-Generator (`documents.change_document`), Vertrag [pl-labels.md](pl-labels.md) |
| PUT | `/asn/config` | Body `{ranges_enabled?, location_field_name?}` (Superuser) → wie GET. GET hängt am Ende an: `ranges_enabled`, `location_field_name`, `can_manage_ranges` (übrige Felder/Reihenfolge unverändert) |
| GET/POST | `/asn/ranges` | Nummernkreise: GET → `{enabled, items:[{id,name,code,location,first_asn,last_asn,note,created,modified,used,next_asn,remaining}]}` (Recht wie `/asn`); POST `{name,code,location?,first_asn,last_asn,note?}` → 201 (Superuser) |
| GET/PUT/PATCH/DELETE | `/asn/ranges/{id}` | ein Nummernkreis; PUT voll, PATCH teilweise, DELETE → 204 (ändern: Superuser); Überschneidung/doppeltes Kürzel → 400 `fields.*`; unbekannt → 404. `POST /asn/preview` liefert am Ende zusätzlich `range` (oder `null`) |

Formen: Fehler `{"error":{"code","message","fields"?,"current_rev"?}}`; Listen
`{"items":[…],"next_cursor":…|null,"count"?}`; Zeitpunkte RFC 3339 UTC, `created` als `YYYY-MM-DD`;
Referenzen als UUID, `legacy_id` zur Verknüpfung mit v1. Statuscodes: 400 invalid, 401
not_authenticated (`WWW-Authenticate: Bearer`), 403 permission_denied (auch CSRF), 404 not_found
(auch unsichtbar – vor jeder Revisionsprüfung), 409 conflict, 410 cursor_expired, 412
precondition_failed (`ETag` = aktueller rev), 429 throttled, 501 not_implemented, 503 not_installed.

## Anmeldung

Gemeinsam mit v1 über `pl-authz` (gleiche Reihenfolge wie DRF): Session-Cookie (+ CSRF bei
unsicheren Methoden), `Token`, `Basic`, Remote-User, neu `Bearer prt_…` (Geräte-Token). v2-Schreib-
zugriffe verwenden den **bereits geprüften** Kontext für den v1-Handler (kein zweites Login, die
HTTP-Methode des Kontexts wird angepasst).

## Identität und `rev`

`uuid`/`rev` aus `paperrust_sync_object` (P2-Migration `pl_sync.0001_paperrust_sync`). Fehlende
Zeilen legt `pl-store` mit `INSERT … ON CONFLICT DO NOTHING` an (Erfassung abgeschaltet, Objekte
aus der Zeit davor). `rev` steigt nur durch die P2-Trigger – ohne installierte/aktive Erfassung ist
`If-Match` wirkungslos (jeder Wert ≠ aktueller rev → 412, gleicher rev → Schreiben). Prüfung und
Schreiben sind nicht atomar (kleines Fenster zwischen `rev`-Vergleich und v1-Schreibzugriff;
Sperre über die Trigger-Zeile würde mit dem Trigger verklemmen).

## Dokumentabfrage (`DocQuery`)

Blätter: `text {query, field: title_content|title|content|original_filename}`, `tags`,
`correspondent`, `document_type`, `storage_path`, `owner`, `custom_fields` (je `{any, all, none,
is_null}`), `custom_field_query` (v1-DSL, Felder als UUID oder Name), `created|added|modified
{from,to}` (einschließlich), `asn {eq,gt,lt,is_null}`, `uuids`, `mime_type`, `shared_by_me`.
Verknüpfung `all`, `any` (ODER) und `not` (Mengenkomplement: auch Dokumente, für die die
Bedingung mangels Wert unbestimmt ist, z. B. `{"not":{"asn":{"gt":5}}}` trifft Dokumente ohne ASN),
beliebig verschachtelt (L1, seit 2026-11-10; `any: []` trifft nichts, `all: []` alles). Auswertung
über den Filter-AST `pl_domain::filter` und den SQL-Bauer `pl_store::docsql` (docs/contract/pl-docfilter.md
„L1“); jedes Blatt wird mit der v1-Prüfung von `pl_docfilter` geparst (identische Semantik wie v1).
Ohne `any`/`not` folgt der Weg dem Umschalter `PAPERRUST_QUERY_ENGINE` (`legacy` = bisherige *(seit Datenmodell-Stufe 7 entfernt: es gibt nur noch den AST-Weg, docs/DATAMODEL-V2.md §6)*
Übersetzung in v1-Parameter). Unbekannte UUIDs treffen nichts. Sortierung `created|added|modified|title|id` (± ), Keyset-Cursor
mit `legacy_id` als Tie-Breaker, an die Sortierung gebunden.

## Feed (`/sync/changes`) und P2

`pl_domain::ChangeFeed` (meta/changes/wait). `pl-api-v2::sync::changes` ist fertig: Cursor `c1.<seq>`,
`limit` 1–1000, `kinds`, `embed` (Std. true), `wait` ≤ 30 s; sichtbar → `upsert` (+`object`),
unsichtbar mit `perm_seq > cursor` → `delete` (Entzug), Tombstones an alle; `next_cursor` = letzte
gelesene seq. **Nicht eingehängt**, weil `pl-sync` (P2) noch nicht committet ist: `pl-server`
übergibt `feed: None` → 501 `not_implemented`, `/server` meldet kein `sync`-Feature, `as_of_seq` ist
`null`. Einhängen nach P2: Adapter `impl ChangeFeed` über `pl_sync::{read_meta, changes,
wait_for_changes, status().last_seq}` (Kind-Codes identisch) in `pl-server/src/api_v2.rs`.
Begrenzung „≤ 4 offene Long-Polls je Token“ (SYNC.md §8) ist noch nicht umgesetzt.

## Tests

- Unit: `cargo test -j 2 -p pl-domain -p pl-store -p pl-api-v2 -p pl-authz` (8 + 4 + 5 + 48).
- DB (Kopie `pl_p3_v2_test` aus `paperless_snapshot`, Medien in Temp-Verzeichnis, wird gelöscht):
  `PL_P3_TEST_DB=1 cargo test -p pl-server -j 2 --test api_v2_db -- --ignored --test-threads 1`
  – Anmeldung (Token, Session+CSRF, ungültig/inaktiv), Rechte-Matrix v1-Liste ↔ v2-Liste für
  admin/alice/bob/carol über 7 Arten, `can_change` ↔ v1 `user_can_change`, Objektebene (404 statt
  412), Cursor (7 Sortierungen, Seitengröße unabhängig), `documents/query` ↔ v1-Filter, Dateien
  (ETag/304/Range/416/If-Range/HEAD/Vorschaubild), Schreiben mit `If-Match`, Notizen,
  Geräte-Token (Ausgabe, v1+v2, nur Hash, Liste, Widerruf sofort, MFA, Drossel), Sync ids/resolve,
  501-Stub, Schalter `PAPERRUST_API_V2=off`.

## Known differences / offen

1. Schreiben nur für Tags, Korrespondenten, Dokumenttypen, Dokument-PATCH, Notizen,
   UI-Einstellungen; Speicherpfade (v1-Schreibrouten noch Django), Custom Fields, gespeicherte
   Ansichten, Nutzer, Gruppen nur lesend (405).
2. Notizen (`include=notes`) nur mit Modellrecht `view_note`, sonst Feld fehlt.
3. `sync/resolve` mit Volltextregeln gespeicherter Ansichten (query, more_like_id, title_search,
   text) → 501; Regel 19 (`title_content`) als Teilstringfilter.
4. Dateigrößen werden je Abruf per `stat` ermittelt (kein Cache).
5. Audit-Log der v2-Schreibzugriffe wie v1 (Token/Geräte-Token: Akteur NULL; Session: Nutzer).

## CORE-REQUESTs

1. `pl-cli/src/migrate.rs`: `v.extend_from_slice(pl_authz::device::migrations::ALL);` (P3b-Tabelle).
2. Nach P2-Commit: Feed-Adapter in `pl-server/src/api_v2.rs` (s. o.).

---

# PaperRust-Oberfläche (Design 2): Lebensbereiche, Zeitstrahl, Fragen

Stand 2026-11-10. Entscheidung: docs/UI-ZWEITES-DESIGN.md (Zeitstrahl + KI-Chat). Verbraucher:
`frontend-light/src/design2`. Logik und Seitentabellen: Crate **`pl-timeline`** (ohne axum);
HTTP: `pl-api-v2/src/{life_areas,timeline,ask}.rs`. OpenAPI: docs/api/v2.yaml (Pfade
`/life-areas…`, `/timeline…`, `/ask…`). Alle Formen folgen den v2-Regeln oben (Fehler
`{"error":{…}}`, UUID-Referenzen + `legacy_id`, reine Daten `YYYY-MM-DD`, Zeitpunkte RFC 3339 UTC).

## Seitentabellen (expand-only, Migration `pl_timeline.0001_paperrust_design2`)

| Tabelle | Inhalt |
|---|---|
| `paperrust_life_area` | Lebensbereich: `uuid`, `user_id` (NULL = Instanz-Vorgabe), `name`, `color`, `icon`, `sort`, `rules` (jsonb), `created`, `modified` |
| `paperrust_doc_date` | Datum eines Dokuments aus **Inhalt** (`source='content'`) oder **Erinnerung** (`source='user'`, `created_by`): `document_id`, `kind`, `date`, `confidence`, `phrase`, `page`, `note` |
| `paperrust_doc_date_state` | Stand der Inhaltserkennung je Dokument (`modified` des Dokuments bei der Erkennung, `version` des Erkenners) |
| `paperrust_timeline_state` | je Nutzer und Eintrag: `done_at`, `snoozed_until` |
| `paperrust_ask_conversation`, `paperrust_ask_message` | Gesprächsverlauf `/ask` je Nutzer |

Kein Fremdschlüssel auf Django-Tabellen; gelöschte/unsichtbare Dokumente fallen über den Join weg.
Die Tabellen werden beim ersten Zugriff angelegt (idempotent, wie `/asn`) bzw. über
`paperrust manage migrate`. Rückbau: `DROP TABLE paperrust_life_area, paperrust_doc_date,
paperrust_doc_date_state, paperrust_timeline_state, paperrust_ask_message, paperrust_ask_conversation`.

## Einstellungen (docs/SETTINGS.md §1.10, Klasse A, live)

| Schlüssel | Standard | Wirkung |
|---|---|---|
| `PAPERRUST_TIMELINE_EXTRACT_DATES` | `no` | Fristen aus dem Dokumentinhalt erkennen (nach dem Einlesen/Neu-Verarbeiten, beim Lesen veralteter Dokumente, Rückfüllung). Aus → Inhaltsdaten werden **nicht angezeigt** (bleiben gespeichert) |
| `PAPERRUST_TIMELINE_DATE_FIELDS` | leer | JSON-Objekt Art → Custom Fields (Name oder id), z. B. `{"due":["Fällig am"],"valid_until":["Gültig bis",7],"cancel_by":["Kündigungsfrist"],"warranty_until":["Garantie bis"],"valid_from":["Vertragsbeginn"],"amount":["Betrag"],"ignore":["Geburtstag"]}`. Felder ohne Zuordnung → Erkennung am Feldnamen (s. u.); `ignore` schließt Felder aus; ungültige Teile → `setting_warnings` in `/timeline/config` |

## Datumsarten (`kind`)

| kind | Bedeutung | Frist („Demnächst“) | Spanne |
|---|---|---|---|
| `created` | Dokumentdatum (`documents_document.created`) | nein | – |
| `due` | fällig / zahlbar bis | ja | – |
| `cancel_by` | Kündigung bis / Kündigungsfrist | ja | – |
| `valid_from` | gültig ab / Vertragsbeginn | nein | – |
| `valid_until` | gültig bis / Laufzeitende | ja | `valid_from` des Dokuments (frühestes), sonst `created` → Datum |
| `warranty_until` | Garantie bis | ja | `created` → Datum |

Quellen (`source`): `created`, `custom_field` (Custom Field vom Typ `date`, **live** gelesen, nicht
gespeichert), `content` (Erkennung im Inhalt, gespeichert, `confidence` 0,5–0,95), `user`
(Erinnerung des Nutzers, nur für ihn sichtbar). Gleiches Dokument + Art + Datum aus mehreren Quellen
→ **ein** Eintrag, Vorrang `custom_field` > `user` > `content`; alle Quellen in `sources`.

Feldnamen-Erkennung für Felder vom Typ `date` ohne Zuordnung in der Einstellung (Groß/klein und
Umlaute egal; erste passende Regel gewinnt): `cancel_by` kündig/cancel/notice · `warranty_until`
garantie/gewährleistung/warranty · `valid_from` gültig ab/beginn/valid from/start · `valid_until`
gültig bis/ablauf/laufzeit/ende/expir/valid until/end · `due` fällig/zahlbar/frist/due/deadline/
payment. Andere Datumsfelder erscheinen nicht. `amount`: zugeordnete Felder, sonst erstes Feld vom
Typ `monetary` (nach id) mit Wert.

Inhaltserkennung (`pl_timeline::extract`, Version 1): Schlüsselphrase (de+en, z. B. „fällig am“,
„zahlbar bis“, „Zahlungsziel“, „due date“, „payable by“, „gültig bis“, „valid until“,
„Ablaufdatum“, „Kündigung bis/zum“, „kündbar bis“, „Kündigungstermin“, „cancel by“, „Garantie bis“,
„Gewährleistung bis“, „warranty until“, „gültig ab“, „Vertragsbeginn“), danach das erste Datum der
upstream-Datumserkennung (`DATE_REGEX` + dateparser aus `pl-matching`, `PAPERLESS_DATE_ORDER`,
Sprachen aus `PAPERLESS_DATE_PARSER_LANGUAGES`/`PAPERLESS_OCR_LANGUAGE` ∪ de, en) innerhalb von
48 Zeichen ohne Leerzeile. Steht „ab/seit/vom/from/since“ dazwischen (außer bei `valid_from`),
zählt es nicht. Künftige Daten sind erlaubt (anders als beim Dokumentdatum). Jahr 1900–2200;
`confidence = 0,95 − Abstand/100` (≥ 0,5), −0,15 bei allgemeinen Phrasen („Garantie“, „expiry“),
−0,2 wenn vor dem Dokumentdatum. Höchstens 20 je Dokument, erste 200 000 Zeichen.

Aktualisierung (nur bei eingeschalteter Einstellung): (1) im Worker nach dem Einlesen und nach dem
Neu-Verarbeiten (neben `ai_after_consumption`), (2) beim Lesen von `/timeline*` für bis zu 200
veraltete Dokumente (`documents_document.modified` neuer als die Erkennung, z. B. nach einer
Inhaltsänderung über v1), (3) `paperrust manage timeline_backfill [--force]`, Worker-Aufgabe
`paperrust.timeline.backfill` (`force`), `POST /timeline/backfill` (Superuser, läuft im
Hintergrund des Servers).

## Rechte

- Zeitstrahl, Demnächst, Erinnerungen, Erledigt/Zurückstellen, `/ask`: Modellrecht
  `documents.view_document` (sonst 403), Dokumente **genau wie die Dokumentliste**
  (`pl_store::visible_ids_sql` = `base_query` + `permitted_objects_filter`: Wurzeldokumente außerhalb
  des Papierkorbs; Eigentümer/ohne Eigentümer/guardian-Objektrecht direkt oder über Gruppen;
  Superuser alles). Unsichtbare Dokumente → 404 bei Einzelzugriffen.
- Korrespondent/Dokumenttyp im Eintrag: `uuid` immer, `name` nur wenn der Nutzer das Objekt sehen
  darf (sonst `null`, wie v1 „privat“).
- Lebensbereiche lesen: jeder angemeldete Nutzer (eigene + Instanz-Vorgabe). Eigene schreiben:
  jeder aktive Nutzer. Instanz-Vorgabe schreiben: nur Superuser. Regeln dürfen nur Objekte
  enthalten, die der Schreibende sehen darf (sonst 400 – unbekannt und unsichtbar sind nicht
  unterscheidbar).
- `/ask`: Retrieval **identisch** zum MCP-Werkzeug `ask_documents`
  (`pl_ai::mcp::knowledge::retrieve`: Volltext über v1 `GET /api/documents/?query=` + LLM-Index mit
  dem Rechtefilter des Chats, Index-Treffer gegen `GET /api/documents/{id}/` des Nutzers geprüft,
  Reciprocal Rank Fusion), nur läuft der v1-Aufruf **im Prozess** mit dem bereits geprüften
  Anmeldekontext statt per HTTP mit Token (`ApiBackend` mit `Transport`). Antwort mit dem
  konfigurierten LLM und den Chat-Vorlagen wie MCP. Gespeicherte Quellen werden beim Lesen des
  Verlaufs erneut gegen die aktuelle Sichtbarkeit gefiltert. Je Nutzer höchstens 20 Fragen je
  Minute (429 `throttled`).

## Lebensbereiche `/life-areas`

> **Nicht mehr von der Oberfläche genutzt (2026-11-16):** Das PaperRust-Design hat die Lebensbereiche entfernt
> (docs/DESIGN2-ENTWIRREN.md §4: Einstellungskarte, Begriff und API-Nutzung weg; die Jahresleiste gruppiert nach
> Dokumenttyp oder Tag). Die Routen `/rs/api/v2/life-areas*` (`crates/pl-api-v2/src/life_areas.rs`) und die
> Tabelle `paperrust_life_area` bleiben vorerst unverändert erreichbar (API-Kunden, bestehende Daten); Entfernen
> ist eine eigene Entscheidung.

`LifeArea`:
```json
{"uuid":"…","scope":"user","name":"Wohnen","color":"#2F6B4F","icon":"home","order":0,
 "rules":{"tags":["<uuid>"],"document_types":[],"storage_paths":[],"correspondents":[]},
 "created":"2026-11-10T09:00:00Z","modified":"2026-11-10T09:00:00Z"}
```
Regeln: ein Dokument gehört zum Bereich, wenn **eine** Regel greift (Tag ∈ `tags` ODER Typ ∈
`document_types` ODER Speicherpfad ∈ `storage_paths` ODER Korrespondent ∈ `correspondents`).
Ein Dokument kann in mehreren Bereichen liegen; ohne Treffer → Bahn `other`. `name` 1–64 Zeichen
(getrimmt), `color` `#RRGGBB`, `icon` `^[a-z0-9-]{1,32}$` (Vorschläge nutzen `home car health money
work insurance tax family education travel pet phone shopping legal tag other`), `order` Ganzzahl,
höchstens 50 Bereiche je Geltungsbereich, je Regelliste ≤ 200 UUIDs.

| Methode | Pfad | Antwort |
|---|---|---|
| GET | `/life-areas[?scope=user\|instance]` | ohne `scope`: wirksame Liste `{"items":[LifeArea],"source":"user"\|"instance"\|"none"}` (eigene, falls vorhanden, sonst Instanz-Vorgabe); mit `scope`: nur diese (`source` = scope) |
| PUT | `/life-areas?scope=user\|instance` | `{"items":[LifeAreaWrite]}` ersetzt den ganzen Satz (Einträge mit `uuid` aus diesem Satz behalten sie); → wie GET mit `scope` |
| DELETE | `/life-areas?scope=user\|instance` | alle löschen (eigene → zurück zur Vorgabe); 204 |
| POST | `/life-areas?scope=user\|instance` | ein Bereich (`LifeAreaWrite`), 201 `LifeArea` |
| GET/PATCH/DELETE | `/life-areas/{uuid}` | eigener Bereich oder Vorgabe (lesen alle, ändern Superuser); PATCH teilweise; DELETE 204 |
| GET | `/life-areas/suggestions[?lang=de\|en]` | `{"items":[LifeAreaWrite + "reason","document_count"]}` aus vorhandenen Daten des Nutzers – **wird nie gespeichert**; Übernahme durch PUT |

`LifeAreaWrite` = `{uuid?, name, color?, icon?, order?, rules?}` (Standard: Farbe aus der Palette
nach Position, `icon` `tag`, `order` = Position). `scope` Standard `user`.

Vorschläge: Schlüsselwörter in Namen sichtbarer Tags, Dokumenttypen und Speicherpfade (je mit
≥ 1 sichtbarem Dokument) → Bereiche Wohnen, Finanzen, Versicherungen, Steuern, Gesundheit,
Fahrzeug, Arbeit, Verträge & Abos, Familie, Bildung; danach, falls weniger als 4, die häufigsten
übrigen Tags (≥ 3 Dokumente, keine Posteingangs-Tags) bis höchstens 8 Bereiche. Sortiert nach
Dokumentanzahl.

## Zeitstrahl `/timeline`

`GET /timeline?from=YYYY-MM-DD&to=YYYY-MM-DD&areas=<uuid>,…,other&kinds=due,…&limit=2000`

Standard `from` = heute − 365 Tage, `to` = heute + 365 Tage (`to` ≥ `from`, höchstens 3660 Tage,
sonst 400), `kinds` alle, `areas` alle + `other`, `limit` 1–10000 (Einträge gesamt, Std. 2000;
mehr → `truncated: true`, frühe Daten zuerst). Ein Eintrag erscheint, wenn sein Datum in
[from, to] liegt oder seine Spanne den Bereich überschneidet. „heute“ in `PAPERLESS_TIME_ZONE`.

```json
{"from":"2026-01-01","to":"2026-12-31","today":"2026-11-10","areas_source":"user",
 "extract_dates":false,"truncated":false,
 "lanes":[{"key":"<area uuid>","area":{LifeArea},"count":3,"items":[TimelineItem]},
          {"key":"other","area":null,"count":1,"items":[…]}]}
```
Bahnen in Bereichsreihenfolge (`order`, dann Name), jede angefragte Bahn auch leer; `other`
zuletzt. Einträge je Bahn nach `date`, dann Titel.

`TimelineItem`:
```json
{"id":"<doc uuid>.due.20261130","kind":"due","date":"2026-11-30",
 "span":{"start":"2026-01-01","end":"2026-11-30"},
 "source":"custom_field","sources":["custom_field","content"],"confidence":1.0,
 "field":{"uuid":"…","name":"Fällig am"},"phrase":"zahlbar bis 30.11.2026","page":2,
 "reminder":{"id":17,"note":"…"},
 "document":{"uuid":"…","legacy_id":12,"title":"…","created":"2026-10-02",
   "correspondent":{"uuid":"…","name":"Swisscom"},"document_type":{"uuid":"…","name":"Rechnung"}},
 "amount":{"value":"89.90","currency":"CHF","field":"<uuid>"},
 "state":{"done":false,"done_at":null,"snoozed_until":null}}
```
`span`, `field`, `phrase`, `page`, `reminder`, `correspondent`, `document_type`, `amount` können
`null` sein. `confidence` 1.0 für `created`/`custom_field`/`user`. `id` ist stabil (Dokument-UUID,
Art, Datum) und Schlüssel für Erledigt/Zurückstellen.

`GET /timeline/upcoming?days=60&overdue_days=30&areas=…&include_done=false` – „Demnächst“: nur
Fristen (`due`, `cancel_by`, `valid_until`, `warranty_until`) mit Datum in
[heute − overdue_days, heute + days] (`days` 1–366, `overdue_days` 0–366), ohne erledigte (außer
`include_done=true`) und ohne zurückgestellte (`snoozed_until` > heute):
```json
{"today":"2026-11-10","days":60,"overdue_days":30,
 "items":[TimelineItem + {"overdue":false,"days_left":20,"areas":["<uuid>","other"]}]}
```
sortiert nach Datum; jeder Eintrag nur einmal.

| Methode | Pfad | Antwort |
|---|---|---|
| PUT | `/timeline/items/{id}/state` | `{"done":bool}` und/oder `{"snoozed_until":"YYYY-MM-DD"\|null}` bzw. `{"snooze_days":1..366}` → `{"id","done","done_at","snoozed_until"}`; Dokument muss sichtbar sein, sonst 404 |
| DELETE | `/timeline/items/{id}/state` | zurücksetzen, 204 |
| GET | `/timeline/reminders` | eigene Erinnerungen `{"items":[{id,item_id,document,legacy_id,title,kind,date,note,created}]}` (nur sichtbare Dokumente) |
| POST | `/timeline/reminders` | `{"document":"<uuid>","kind":"due","date":"YYYY-MM-DD","note"?:≤500}` → 201 Erinnerung; gleiche (Dokument, Art, Datum) schon vorhanden → 200 dieselbe |
| DELETE | `/timeline/reminders/{id}` | eigene, 204 (fremde/unbekannte 404) |
| GET | `/timeline/config` | `{"extract_dates","kinds":[…],"deadline_kinds":[…],"fields":[{uuid,legacy_id,name,data_type,kind,source:"setting"\|"heuristic"\|null}],"amount_fields":[uuid],"setting_warnings":[…],"extraction":{"documents","extracted","stale"},"backfill_running"}` |
| POST | `/timeline/backfill` | Superuser; `{"force"?:bool}` → 202 `{"started":true}` (409 `conflict` wenn schon läuft, 400 wenn Erkennung aus) |

## Fragen ans Archiv `/ask`

`POST /ask`:
```json
{"question":"Wann ist die Swisscom-Rechnung fällig?","conversation":"<uuid>",
 "documents":["<uuid>"],"max_passages":8,"answer":true,"stream":true}
```
`lang` (`de` Std., `en`) = Sprache der Aktions-Beschriftungen. `question` 1–2000 Zeichen (getrimmt), `documents` ≤ 500 (Einschränkung; unsichtbare/unbekannte
treffen nichts), `max_passages` 1–20 (Std. 8), `answer` Std. true (false → nur Passagen, kein
LLM-Aufruf). Ohne `conversation` → neues Gespräch (Titel = Frage, 80 Zeichen). Fremdes/unbekanntes
Gespräch → 404.

**Streaming** (`"stream":true` oder `Accept: text/event-stream`): `200 text/event-stream`, je
Ereignis `event: <name>\ndata: <JSON einzeilig>\n\n`, Reihenfolge:

| event | data |
|---|---|
| `meta` | `{"conversation","message","index_state":"disabled"\|"not_built"\|"ready","retrieval":"hybrid"\|"fulltext","notes":[…]}` |
| `citations` | `{"citations":[Citation]}` (vor der Antwort) |
| `delta` | `{"text":"…"}` (0…n, nur mit LLM) |
| `actions` | `{"actions":[Action]}` |
| `done` | vollständige `AskMessage` (gespeichert, `notes` inkl. LLM-Hinweisen) |
| `error` | `{"error":{"code","message"}}` (statt `done`, danach Ende) |

Eingabefehler (400/401/403/404/429) kommen vor dem Stream als normale v2-Fehler. Ohne Streaming:
`200 {"conversation":"<uuid>","message":AskMessage}`.

`AskMessage`:
```json
{"uuid":"…","conversation":"…","role":"assistant","question":"…",
 "answer":"… [1] …","answer_error":null,
 "index_state":"disabled","retrieval":"fulltext","notes":["…"],
 "citations":[Citation],"actions":[Action],"created":"…"}
```
`Citation` = `{"n":1,"document":"<uuid>","legacy_id":12,"title":"…","page":2,"snippet":"…",
"source":"semantic"|"fulltext"|"both","score":0.0328}` (`page` `null` ohne Seitenumbrüche) – `[n]`
in `answer` verweist auf `n`.

KI aus / kein LLM → `answer: null`, nur gewichtete Passagen (`citations`), Hinweise in `notes`
(„No LLM is configured; returning ranked passages only.“ bzw. Index-Hinweise wie MCP). LLM-Fehler
→ `answer_error` („AI backend not reachable.“ …), Quellen bleiben.

`Action` (Vorschläge, nichts wird automatisch ausgeführt):
- `{"type":"open_documents","label":"…","documents":[uuid],"legacy_ids":[int],
  "query":{"uuids":[…]}}` – Bedingung für `POST /documents/query`; v1-Link `/documents?id__in=…`.
- `{"type":"create_reminder","label":"…","document":uuid,"legacy_id":12,"kind":"due",
  "date":"2026-11-30","phrase":"…","request":{"method":"POST","path":"/rs/api/v2/timeline/reminders",
  "body":{"document","kind","date","note"}}}` – je zitiertem Dokument mit erkannter künftiger Frist
  (Inhaltserkennung auf Abruf, unabhängig von der Einstellung), die noch nicht auf dem Zeitstrahl
  des Nutzers steht; höchstens 3.

Verlauf:

| Methode | Pfad | Antwort |
|---|---|---|
| GET | `/ask/conversations?limit=50&cursor=` | eigene, zuletzt aktive zuerst `{"items":[{uuid,title,created,updated,message_count}],"next_cursor"}` (`limit` 1–200) |
| GET | `/ask/conversations/{uuid}` | `{uuid,title,created,updated,"messages":[{uuid,role:"user"\|"assistant",content,citations,actions,meta,created}]}` |
| DELETE | `/ask/conversations/{uuid}` | 204 |
| DELETE | `/ask/conversations` | alle eigenen, 204 |

`meta` einer Antwort = `{index_state, retrieval, notes, answer_error}`. Retrieval berücksichtigt nur
die aktuelle Frage (wie `ask_documents`); der Verlauf dient der Anzeige.

## Tests (Design 2)

- Unit: `cargo test -j 4 -p pl-timeline` (Erkennungsmuster de/en, Feldnamen, Regeln, Spannen,
  Zusammenführen, Demnächst, Vorschläge), `-p pl-api-v2` (SSE-Format, Aktionen, Eingaben),
  `-p pl-ai` (MCP `ask_documents` unverändert über `retrieve` + `start_answer`).
- DB (Kopie `pl_design2_test` aus `paperless_snapshot`, wird gelöscht):
  `PL_DESIGN2_TEST_DB=1 cargo test -p pl-server -j 4 --test design2_db -- --ignored --test-threads 1`.

## CORE-REQUESTs (Design 2)

1. Wurzel-`Cargo.toml` `[workspace.dependencies]`: `pl-timeline = { path = "crates/pl-timeline" }`
   (heute Pfadabhängigkeit in pl-api-v2, pl-cli, pl-server – wie pl-labels).
2. `pl-server/src/api_v2.rs`: `LEGACY_WRITE_ROUTES` enthält jetzt auch die Lese-Routen
   `documents.list`, `documents.retrieve` (für `/ask` im Prozess); `pl-store::LegacyRouter::router()`.
3. `pl-server`: Feature `ai` schaltet `pl-api-v2/ai` mit (ohne → `/ask` 501); pl-api-v2 dort ohne
   Standard-Features eingebunden.

## Rollen `/roles` (docs/DESIGN2-ENTWIRREN.md §1)

Neu in PaperRust (kein upstream-Gegenstück). Quelle: `crates/pl-api-v2/src/roles.rs`, Migration
`pl_api_v2.0001_paperrust_role` (`crates/pl-api-v2/migrations/0001_paperrust_role.sql`, eingetragen in
`pl_cli::migrate::paperrust_migrations()`; zusätzlich beim ersten Aufruf der API idempotent
ausgeführt, `roles::ensure`). Rollen sind eine **Schicht über den Django-Rechten**: Wahrheit bleiben
`auth_user_user_permissions`, `auth_group_permissions`, `auth_user.is_superuser/is_staff`. Ein Umstieg
zurück zu paperless-ngx verliert nur die Rollennamen.

**Tabellen:** `paperrust_role(id, key UNIQUE NULL, name, description, builtin, is_admin, permissions
text[], created, modified)` (Name eindeutig ohne Groß/Klein), `paperrust_role_assignment(subject
'user'|'group', subject_id, role_id → ON DELETE CASCADE, assigned, assigned_by)` (nur Merkhilfe),
`paperrust_role_config(id=1, mode 'classic'|'custom', modified)` – der Rollenmodus liegt hier, nicht im
Einstellungsregister (pl-settings).

| Methode | Pfad | Rechte | Antwort |
|---|---|---|---|
| GET | `/roles` | aktiv und (Superuser, `is_staff` oder `auth.view_user`) | 200 Übersicht |
| PUT | `/roles/mode` `{"mode":"classic"\|"custom"}` | Superuser | 200 Übersicht |
| POST | `/roles` `{"name","description"?,"permissions"?}` | Superuser, nur Modus `custom` | 201 Rolle |
| PATCH | `/roles/{id}` (Felder wie POST, alle optional) | Superuser, nur `custom`, nicht fest | 200 Rolle |
| DELETE | `/roles/{id}` | Superuser, nur `custom`, nicht fest | 204 |
| POST | `/roles/{id}/assign` `{"users":[ids],"groups":[ids]}` | s. u. | 200 `{"users":[{"id","role_id"}],"groups":[…]}` |
| GET | `/roles/resolve?users=1,2&groups=3` | wie GET `/roles`; für `groups` zusätzlich Superuser/Staff/`auth.view_group` | 200 `{"users":{"1":id\|"individual"},"groups":{…}}` |

Übersicht: `{"mode","roles":[Rolle…],"topics":[{"key","models":[…]}],"always":[…]}`; Rolle:
`{"id","key":"read"|"edit"|"manage"|"admin"|null,"name","description","builtin","admin","permissions":[…]}`
(Rechte sortiert, ohne `always`; Administrator: `[]`). Reihenfolge der Rollen: feste zuerst, dann nach
id. **Modus `classic`:** nur die vier festen Rollen in `roles`, Erkennung nur gegen sie, eigene
Rollen bleiben gespeichert (erscheinen wieder in `custom`), Zuweisen einer eigenen Rolle → 400.

**Feste Rollen** (Rechte kommen aus dem Code `roles::{READ,EDIT_EXTRA,MANAGE_EXTRA}`; die Zeilen in der
Tabelle werden bei jedem `ensure` abgeglichen):

- `read` „Nur lesen“: `documents.view_` + `correspondent, customfield, document, documenttype, note,
  savedview, sharelink, sharelinkbundle, storagepath, tag`.
- `edit` „Bearbeiten“: read + `documents.add_document, change_document, add_note, change_note,
  add_savedview, change_savedview, add_sharelink, add_sharelinkbundle, change_sharelinkbundle`.
- `manage` „Verwalten“: edit + `documents.delete_document, delete_note, delete_sharelink,
  delete_sharelinkbundle, delete_savedview`; `add/change/delete` für `tag, correspondent,
  documenttype, storagepath, customfield`; `documents.{view,add,change,delete}_workflow`;
  `paperless_mail.{view,add,change,delete}_{mailaccount,mailrule,processedmail}`;
  `documents.view_paperlesstask`; `auditlog.view_logentry` (Verlauf, upstream
  `DocumentViewSet.history`).
- `admin` „Administrator“: `is_superuser = is_staff = true`, keine Codenames.
- `always`: `documents.view_uisettings, documents.add_uisettings, documents.change_uisettings`
  (upstream `UiSettingsView`, GET view / POST add).

**Themen** (`topics`, Modelle wie upstream `PermissionType` im Frontend): `documents` = document, note,
sharelink, sharelinkbundle, `auditlog.logentry` (Verlauf); `organize` = tag, correspondent,
documenttype, storagepath, customfield; `views` = savedview; `automation` = workflow,
`paperless_mail.mailaccount`, `mailrule`, `processedmail`, `documents.paperlesstask`; `system` =
`auth.user`, `auth.group`, `paperless.applicationconfiguration`. Die Logs (System) hängen upstream an
`is_staff` (`PaperlessAdminPermissions`), nicht an einem Modellrecht. Eigene Rollen dürfen genau
`{view,add,change,delete}_<model>` dieser Modelle enthalten (und `always`, wird entfernt); alles
andere (auch Codenames, die in der DB fehlen) → 400 `fields.permissions` „Unknown permission "x".“.

**Zuweisen** (eine Transaktion, alles oder nichts):
- Rechte: Superuser; sonst `auth.change_user` (wenn `users` nicht leer) bzw. `auth.change_group` (wenn
  `groups` nicht leer), sonst 403. Nicht-Superuser: Administrator-Rolle → 403 „Superuser status can
  only be granted by a superuser“; Superuser als Ziel → 403 „Superusers can only be modified by other
  superusers“ (wie v1 `UserViewSet`); Rolle mit Rechten, die der Zuweisende selbst nicht hat → 403
  „You cannot assign a role with permissions you do not have.“.
- Wirkung je Benutzer: direkte Rechte = Rollenmenge ∪ `always` (nur in der DB vorhandene Rechte).
  Administrator-Rolle: `is_superuser = is_staff = true`, direkte Rechte = `always`. Andere Rolle für
  einen Superuser: `is_superuser = is_staff = false` – für sich selbst 400 `fields.users` „You cannot
  remove your own administrator role“. Andere Rolle für Nicht-Superuser: `is_staff` bleibt.
  Gruppenmitgliedschaften bleiben unverändert.
- Je Gruppe: `auth_group_permissions` = Rollenmenge ∪ `always`; Administrator-Rolle → 400
  `fields.groups`.
- Unbekannte Ids → 400 `fields.users|groups` „Invalid pk "x" - object does not exist.“; höchstens 1000
  Ids je Liste. Danach `paperrust_role_assignment` (upsert) und Log `paperrust.roles` „role assigned“
  (nur Ids).

**Erkennung** (`resolve`): Superuser → id der Administrator-Rolle (Fallback `"admin"`, nur falls die
Zeile fehlt). Sonst die **direkten** Rechte (ohne Gruppenrechte) ohne `always` gegen jede aktive
Nicht-Admin-Rolle (Rollenmenge ∩ vorhandene Rechte, ohne `always`); erste exakte Übereinstimmung, sonst
`"individual"`. Gruppen genauso. Unbekannte Ids fehlen in der Antwort.

Fehlerform wie überall in v2 (`{"error":{…}}`); 404 für unbekannte Rollen-id.

### Known differences / Entscheidungen (Rollen)

- Kein Audit-Log-Eintrag: upstream registriert `User`/`Group` nicht bei auditlog (auch v1
  `/api/users/` schreibt keinen); stattdessen Log-Zeile `paperrust.roles`.
- Schutz vor Rechteausweitung (nur Rollen mit eigenen Rechten zuweisbar) ist strenger als upstream
  (`/api/users/` erlaubt `change_user`-Inhabern beliebige `user_permissions`).
- `paperless.view_global_statistics`, `paperless.view_system_monitoring` und Rechte außerhalb der
  Themen (z. B. Workflow-Trigger/-Aktionen, guardian, Token) sind in keiner Rolle; Benutzer mit
  solchen direkten Rechten werden als `"individual"` erkannt.
- Eine Rolle ohne Rechte (eigene Rolle mit `permissions: []`) passt auf Benutzer ohne direkte Rechte.
- Löschen einer eigenen Rolle ändert keine Django-Rechte; Betroffene werden „individual“.

### Tests (Rollen)

- Unit (`cargo test -p pl-api-v2 --lib roles`): Rundlauf Rolle → zugewiesene Menge → Erkennung für
  jede feste Rolle (Benutzer und Gruppe), `always` ignoriert, fehlende Rechte in älteren DBs,
  Seeds in der Migration = Konstanten, Admin-Flags, Selbst-Herabstufung, Admin für Gruppe,
  Nicht-Superuser-Regeln, Modus-Wächter, Codename-Prüfung, Themenabdeckung.
- DB (`tests/roles_api.rs`, `#[ignore]`, eigene Kopie `pl_roles_api_test` aus `paperless_snapshot`,
  Anleitung im Kopfkommentar).
