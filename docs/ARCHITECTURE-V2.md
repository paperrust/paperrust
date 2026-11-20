# PaperRust – Architektur v2 (nativer Kern + Kompatibilitätsschicht)

Stand 2026-11-09. Entscheidungsvorlage für die nächste Phase. Ergänzt docs/ARCHITECTURE.md (Phase 1:
Drop-in-Ersatz, gilt weiter) und docs/STANDALONE.md. Zugehörig: docs/SYNC.md, docs/FRONTEND-LIGHT.md,
docs/REMOTE-APP.md, docs/ROADMAP-NEXT.md.

## 0. Bindende Vorgaben (Eigentümer)

1. paperless-ngx-Kompatibilität bleibt die **Basis** (Umzug, REST-API v1 inkl. Filterlogik, DB-Schema),
   darf die Zukunft aber **nicht einschränken**. Später werden Altlasten (Datenmodell, API-Formen,
   Rechte, Python-Eigenheiten wie Dateinamen-Vorlagen/`pystr`, Whoosh-artige Suche, sklearn-artiger
   Klassifikator) nativ neu gebaut; paperless-Kompatibilität läuft dann als **Schicht** (Importer,
   API-v1-Adapter).
2. Desktop: (1) lokal mit eingebettetem PostgreSQL (vorhanden), (2) Fern-App gegen Server-URL,
   (3) Auswahl-Sync mit Offline-Lesen/-Suche, (3b) später Offline-Erfassen/-Bearbeiten.
3. Neue **leichte Oberfläche** zusätzlich zur Angular-UI, gleiches Aussehen (docs/FRONTEND-LIGHT.md).

## 1. Zielschichten

```
 Frontends:   Angular-UI (upstream, /)      Light-UI (/rs/app/, Tauri, später mobil)     Fremd-Apps
                    │  API v1                      │  API v2                               │ v1
 ───────────────────┼──────────────────────────────┼──────────────────────────────────────┼────────
 API:        Compat-API v1  /api/…             Native API v2  /rs/api/v2/…              MCP /mcp
             (DRF-Formen, Parität)             (eigene Formen, UUID, rev, Sync)
                    │  ruft                        │ ruft
 ───────────────────┴──────────────┬───────────────┴────────────────────────────────────────────
 Kern:       Domäne (Typen, Regeln, Rechte-Entscheidung, Abfrage-AST)     Dienste (Einlesen, OCR,
             pl-domain (neu)                                              Index, Klassifikator, Mail)
                                   │
 Speicher:   Repository-Schicht pl-store (neu) ── heute: Django-Tabellen + paperrust_*-Tabellen
                                   │              später: native Tabellen, Django-Schema nur noch
                                   │              über Importer/Sichten
 ──────────────────────────────────┴───────────────────────────────────────────────────────────
             PostgreSQL ≥ 14 (Server, Desktop-Modus 1)       SQLite (nur Client-Replikat, SYNC.md)
```

Regeln der Schichtung:

- **API v2 hängt nie an v1-Code.** `pl-api-v2` darf `pl-documents::serialize_documents` (DRF-JSON) und
  `pl_core::pagination` (DRF-Paginierung) **nicht** benutzen. Es serialisiert aus Domänentypen.
- **v1 darf schrittweise auf den Kern umziehen**, aber nur, wenn der Paritäts-Diff unverändert grün
  bleibt (Abschnitt 4). Bis dahin bleiben die v1-Crates, wie sie sind.
- **Übergangsweise darf v2 vorhandene, paritätsgeprüfte Logik wiederverwenden** (z. B.
  `pl_docfilter::apply`, Schreibfunktionen aus `pl-documents-write`), aber nur **hinter** einer
  Kern-Schnittstelle (`pl-domain`-Trait), nie im v2-Vertrag sichtbar. Wird die Logik später nativ
  ersetzt, ändert sich der v2-Vertrag nicht.
- **Rechte** entscheidet immer der Kern (heute `pl-authz`: Modellrechte, guardian, `owner`). v2 bildet
  sie auf ein eigenes, einfaches Format ab (`{"can_view","can_change","can_delete"}` + Freigabeliste)
  statt `set_permissions`/`full_perms` (DRF-Form).

## 2. Was wohin gehört (vorhandene und neue Crates)

| Crate | Rolle in v2 | Änderung |
|---|---|---|
| `pl-core` | AppState, Konfiguration, DB-Pool, Cache, Datum | unverändert; v2-Fehlerformat **nicht** hier (DRF-Format bleibt v1) |
| `pl-authz` | Authentifizierung (Session, Token, Basic, Remote-User), Rechte | + Geräte-Token (`Authorization: Bearer prt_…`, SYNC.md §8) – kleiner CORE-REQUEST |
| `pl-objects`, `pl-documents`, `pl-documents-write`, `pl-fields-views`, `pl-files`, `pl-search`, `pl-system`, `pl-mail`, `pl-workflows`, `pl-auth-api` | **Compat-API v1** (DRF-Formen) | unverändert; später Umzug auf `pl-store` |
| `pl-docfilter` | v1-Filter (`DocumentFilterSet`, `custom_field_query`) | bleibt v1-Parser; v2 übersetzt seinen JSON-Abfrage-AST übergangsweise in `QueryParams` (Abschnitt 5, L1) |
| `pl-templating` | Dateinamen-Vorlagen (Jinja2/pystr-Emulation) | Legacy-Kandidat L2 |
| `pl-search`, `pl-indexer` | tantivy im upstream-Format | Legacy-Kandidat L3 |
| `pl-classifier`, `pl-matching` | Zuordnung | Legacy-Kandidat L4 |
| `pl-dbmigrate`, `pl-cli` (export/import) | Umzug von paperless-ngx | **Compat-Schicht „Migration“** – bleibt dauerhaft |
| **`pl-domain`** (neu) | Domänentypen (`Document`, `Tag`, …, mit `uuid`, `rev`), Abfrage-AST `DocQuery`, Traits `DocumentRepo`, `ObjectRepo`, `FileStore`, `Permissions` | neu, ohne axum/DRF-Abhängigkeit |
| **`pl-store`** (neu) | Umsetzung der Repos auf Django-Tabellen + `paperrust_sync_object` (UUID/rev) | neu; einziger Ort, der Django-Tabellennamen für v2 kennt |
| **`pl-sync`** (neu) | Änderungserfassung: Migration, Sequenzer, Feed-Abfrage, Aufbewahrung (SYNC.md) | neu |
| **`pl-api-v2`** (neu) | Router `/rs/api/v2/…`, v2-Fehlerformat, Paginierung mit Cursor | neu |
| **`pl-sync-client`** (neu) | Client-Kern: SQLite-Replikat, Remotes (PaperRust v2 / paperless-ngx v1), Dateicache, Offline-Suche, später Outbox | neu, ohne axum; läuft in Tauri-Hülle (Desktop/Mobil) |
| `pl-server` | Montage: v1 über `routes.toml`, **v2 und Light-UI fest montiert** | 2 kleine CORE-REQUESTs (Mount v2, Mount `/rs/app/`) |
| `apps/desktop`, `apps/desktop-shell` | Modus 1 (Supervisor) + Modus 2/3 (Fern/Sync) | docs/REMOTE-APP.md |
| `frontend/` | Angular-UI (Patch-Serie) | unverändert |
| **`frontend-light/`** (neu) | Light-UI (Svelte 5) | docs/FRONTEND-LIGHT.md |

Abhängigkeitsrichtung: `pl-api-v2 → pl-domain ← pl-store`, `pl-api-v2 → pl-sync`, `pl-store → pl-authz,
pl-core` (und übergangsweise `pl-docfilter`, `pl-documents-write`), `pl-sync-client → pl-domain` (nur
Typen, Feature `client` ohne sqlx-postgres). **Keine** v1-Crate hängt an `pl-api-v2`.

## 3. v1 und v2 auf einem Server

### Pfade und Versionen

| | v1 (Compat) | v2 (nativ) |
|---|---|---|
| Pfad | `/api/…` (upstream-Pfade, unverändert) | **`/rs/api/v2/…`** |
| Versionierung | DRF `Accept: application/json; version=9|10` (`paperless/settings/__init__.py`: `ALLOWED_VERSIONS ["9","10"]`, Standard 10) | Pfadversion; inkompatible Änderung ⇒ `/rs/api/v3/` (v2 läuft ≥ 12 Monate parallel) |
| Schalter | `routes.toml` (je Route `rust`/`django`) | fest montiert, abschaltbar mit `PAPERRUST_API_V2=off`; **nicht** in `routes.toml` (keine Django-Entsprechung) |
| Fehler | DRF (`{"detail": …}`) | `{"error":{"code":"not_found","message":"…","fields":{…}}}` (RFC-9457-ähnlich, `application/json`) |
| Paginierung | `count/next/previous/results` (+`all` in v9) | Cursor: `{"items":[…],"next_cursor":"…"|null}` |
| IDs | Ganzzahl | `uuid` (Schlüssel) + `legacy_id` (Ganzzahl, nur zur Verknüpfung mit v1/Angular) |
| Datum | DRF-Format in `PAPERLESS_TIME_ZONE` | RFC 3339 UTC (`…Z`), reine Daten `YYYY-MM-DD` |

**Warum `/rs/api/v2/` und nicht `/api/v2/`:** `/api/` gehört upstream – unbekannte `/api/`-Pfade gehen
im Mischbetrieb an Django (`pl-server/src/routing.rs`, Fallback-Proxy) und künftige upstream-Versionen
könnten dort Pfade anlegen. „v2“ kollidiert dort zudem begrifflich mit DRF-`version=10`. Präfix `/rs/`
ist (wie `/__rs/`, `pl-server/src/frontline.rs: INTERNAL_PREFIX`) in upstream nicht belegt. Die
Montage muss vor dem Django-Fallback greifen; `/rs/`-Pfade werden **nie** an Django weitergereicht
(auch nicht bei `%`-kodierten Pfaden, vgl. `must_go_to_django`).

Light-UI: **`/rs/app/`** (SPA-Fallback `/rs/app/*` → `index.html`). Angular bleibt unter `/`.

### Gemeinsame Anmeldung

- **Session** (Cookie wie Django, `django_session`, `pl-authz`) gilt für v1 und v2 → Angular und
  Light-UI im selben Browser teilen die Anmeldung. Für unsichere Methoden mit Session-Anmeldung prüft
  v2 CSRF wie v1 (`X-CSRFToken` + `csrftoken`-Cookie, `pl-authz/src/csrf.rs`).
- **Token**: v1-Token (`Authorization: Token …`, `authtoken_token`) gilt auch für v2.
- **Geräte-Token** (neu, SYNC.md §8): `Bearer prt_…`, je Gerät widerrufbar; gilt für v2 **und** v1
  (pl-authz-Erweiterung). Django kennt es nicht → im Mischbetrieb nur auf Rust-Routen gültig;
  dokumentiert als Known difference in `docs/contract/pl-authz.md`.
- Login-Seite: die vorhandene (`accounts.login.*`, gleiche Optik); Light-UI leitet mit
  `?next=/rs/app/` dorthin. Kein zweiter Login-Mechanismus im Browser.

## 4. Regeln, die die v1-Parität unberührt lassen

1. **Schema expand-only:** nur neue Tabellen/Funktionen/Trigger mit Präfix `paperrust_`, keine neue
   Spalte, kein geänderter Default, keine geänderte Constraint an Django-Tabellen. Trigger sind
   reine `AFTER`-Trigger, verändern `NEW` nicht und schreiben nur in `paperrust_*` (SYNC.md §2).
2. Neue Migrationen nur über `pl_cli::migrate::paperrust_migrations()` (`crates/pl-cli/src/migrate.rs`),
   idempotent, mit Rückbau-Kommentar (Muster: `crates/pl-queue/migrations/0001_paperrust_task_queue.sql`).
3. **Paritätsläufe** (`tests/parity/harness`, `tests/parity/accepted.json`) laufen künftig **zweimal**:
   ohne und mit installierter Änderungserfassung (`PARITY_SYNC_CAPTURE=on`). Beide müssen gleich
   grün sein (heute: lesend 1787/1797, schreibend 515/521, Rest freigegeben). Ein v2-Paket gilt erst
   als fertig, wenn beide Läufe unverändert sind.
4. v2-Code liegt in eigenen Crates; Änderungen an v1-Crates nur als CORE-REQUEST (z. B. eine Funktion
   `pub` machen), nie Verhaltensänderung.
5. `routes.toml` bleibt die Quelle für v1; v2-Routen tauchen dort nicht auf.
6. Rückweg zu paperless-ngx bleibt offen: `paperrust manage sync disable` entfernt die Trigger;
   `paperrust_*`-Tabellen stören Django nicht (README „Rückweg offen“). **Überholt seit 2026-11-13:**
   L6 Schritt 1 (`pl_objects.0001_paperrust_tag_adjacency`) ändert Django-Tabellen; Rückweg nur noch
   über Sicherung oder Export (docs/DATAMODEL-V2.md §3.2), kein Mischbetrieb mehr.
7. Upstream-Updates (docs/UPSTREAM-SYNC.md): Trigger sind generisch (nur `id`/FK-Spalte über
   `TG_ARGV`), damit neue upstream-Spalten sie nicht brechen; jede upstream-Migration, die eine
   erfasste Tabelle umbenennt, steht auf der Prüfliste des Sync-Schritts.

## 5. Abbau der Altlasten (später, gestuft)

Grundmuster für jeden Schritt: (a) native Umsetzung hinter der vorhandenen `pl-domain`-Schnittstelle,
(b) v2 nutzt sie sofort, (c) v1 wird auf die native Umsetzung + einen **Adapter** gestellt und muss
den Paritätslauf bestehen; wo das nicht geht, bleibt der alte Code als Adapter-Bibliothek in der
Compat-Schicht, (d) Altcode wandert in `compat/`-Crates (Namenspräfix `pl-compat-…`).

| # | Altlast | Ersatz (nativ) | Was die Compat-Schicht danach tut | Voraussetzung |
|---|---|---|---|---|
| L1 | Filter-Query-Parameter (`DocumentFilterSet`, django-filter-Lookups, `custom_field_query`-DSL) | typisierter `DocQuery`-AST in `pl-domain`, eigener SQL-Bauer | v1-Parser übersetzt Query-Parameter → AST (`pl-docfilter` wird zum Parser) | v2-Suche stabil, Parität mit AST-Weg |
| L2 | Dateinamen-/Speicherpfad-Vorlagen (Jinja2-Sandbox, `pystr`/`pyrepr`-Emulation, `pl-core/src/pyrepr.rs`, `pyset.rs`) | native Vorlagen (einfache Platzhalter `{created:%Y}/{correspondent}`), deterministisch | v1-`storage_paths` mit altem Syntax → `pl-compat-templating` (heutiger Code), neue Pfade nativ; Migration bietet Umwandlung an | L6 |
| L3 | Whoosh-artige Suche (tantivy im upstream-Indexformat, Query-Syntax upstream) | eigenes tantivy-Schema (UUID-Schlüssel, Sprachen/Stemming, Facetten), Abfrage über `DocQuery` | v1-`query=`/`more_like_id` → Übersetzer auf neues Schema; altes Format nur noch zum Import | L1 |
| L4 | sklearn-artiger Klassifikator (`pl-classifier`, Pickle-Formatnähe) | neues Modell/Format, ggf. KI (`pl-ai`) | v1-`suggestions` liefert aus neuem Modell im v1-Format | – |
| L5 | Rechte (Django-Modellrechte `auth_permission`, guardian-Objektrechte, `owner`) | Rollen + Freigaben (Objekt ↔ Nutzer/Gruppe ↔ Stufe view/edit), Tabelle `paperrust_grant` | v1 `permissions`/`set_permissions` ↔ Abbildung auf Freigaben; Importer übernimmt guardian-Zeilen | L6 |
| L6 | Datenmodell (Django-Tabellen als Wahrheit) | native Tabellen `paperrust_document` usw. mit UUID-PK, `rev`, sauberen Typen; Django-Tabellen werden **Sichten** (updatable views) oder entfallen | Importer (paperless-ngx-DB → nativ), v1 liest/schreibt nativ über `pl-store`; Rückweg = Exporter (paperless-ngx-Exportformat) | L1, Sync stabil, `pl-store` deckt alle v1-Crates ab |
| L7 | DRF-Eigenheiten in v1 (Feldreihenfolge, Fehlertexte, `all`) | – | bleibt nur in der Compat-Schicht | – |

Zeitpunkt: frühestens nach Light-UI-MVP und Sync (ROADMAP-NEXT P8). Jede Stufe einzeln freigeben.

**Stand L1 (2026-11-10): umgesetzt.** AST `pl_domain::filter`, Parser `pl_docfilter::ast`, SQL-Bauer
`pl_store::docsql` (Sichtbarkeit als austauschbare `Visibility` für L5), v2 `any`/`not`, Umschalter
`PAPERRUST_QUERY_ENGINE` (Standard `ast`), Teilindex `paperrust_doc_live_created`. Differenztest *(seit Datenmodell-Stufe 7 entfernt: es gibt nur noch den AST-Weg, docs/DATAMODEL-V2.md §6)*
25 655 Fälle 0 verschieden, Paritätslauf lesend 0 verschieden; Details und Messung in
docs/contract/pl-docfilter.md „L1“. Altcode (`pl_docfilter::apply`) bleibt als `legacy`-Weg, bis er
nach `pl-compat-…` wandert.

**Stand L5 (2026-11-10): umgesetzt.** Freigabe-Index `paperrust_grant` (perm_id, subject ±Nutzer/
Gruppe, object_id, ct) als von Triggern gepflegte Ableitung von guardian (Django-Tabellen bleiben
Wahrheit, Mischbetrieb und Rückweg unverändert); Eigentümer, Superuser/aktiv und Gruppen werden wie
upstream je Anfrage gelesen (Modellrechte beeinflussen die Objekt-Sichtbarkeit upstream nicht).
Alle Rechte-Lesewege (pl-authz-Bausteine, docsql-`Visibility`, pl-store, pl-search, pl-mail)
umschaltbar über `PAPERRUST_PERMISSION_ENGINE` (Standard `grants`), `paperrust manage grants *(Umschalter seit Datenmodell-Stufe 7 entfernt: mit bereitem Index immer `grants`, ohne – nur auf Datenbanken ohne PaperRust-Migrationen – guardian direkt, docs/DATAMODEL-V2.md §6)*
status|check|rebuild|enable|disable`. Differenz-/Triggertest 0 verschieden (u. a. 440 208
Checker-Vergleiche, 1 000 Zufallsrunden), Parität lesend 2 024/0 und schreibend 565/0 verschieden;
1 Mio. Dokumente: Liste `COUNT` −35 %, Seite −28 %, Grantees je Seite −99 %. Details
docs/contract/pl-authz.md „L5“.

## 6. Entscheidungen (Zusammenfassung)

- Native API unter `/rs/api/v2/`, Light-UI unter `/rs/app/`, Anmeldung geteilt (Session/Token +
  neue Geräte-Token).
- Neue Crates `pl-domain`, `pl-store`, `pl-sync`, `pl-api-v2`, `pl-sync-client`; v1-Crates unverändert.
- Expand-only-Schema, Paritätslauf mit und ohne Änderungserfassung.
- Altlasten werden erst ersetzt, wenn v2 trägt; Reihenfolge L1 → L3/L6 → L2/L5; Compat-Schicht = Importer
  + v1-Adapter, dauerhaft.
