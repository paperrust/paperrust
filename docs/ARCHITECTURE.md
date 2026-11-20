# PaperRust – Architektur

> Projektname: **PaperRust** (technisch `paperrust`). Siehe docs/BRANDING.md.

Ziel: 1:1 dieselbe Funktionalität wie paperless-ngx **v3.4.2** (`upstream/`, Submodul, unverändert),
nur schneller und ressourcenschonender. Vorgehen nach dem Skill `multi-tenant-whitelabel`
(Port mit Vertrag + Paritäts-Diff, danach Review, Audit, Sicherheit, E2E, Benchmark, Optimierung).
Mandantenfähigkeit ist **kein** Ziel.

## Zielbild Standalone

Ohne äußere Abhängigkeiten (kein Redis/Celery/Python/Tika/Gotenberg – seit 2026-11-14 erreicht: kein
Django-, Celery- oder Redis-Code mehr) und als native Desktop-App mit eingebettetem PostgreSQL. **Leitplanken dafür gelten ab sofort**, siehe docs/STANDALONE.md.

## Grundsatz

**Verhalten zuerst festnageln, dann ändern.** Jede Rust-Route ist ein Drop-in-Ersatz der
Django-Route: gleicher Pfad, gleiche Methode, gleiche Statuscodes, gleiches JSON (Schlüssel und
Reihenfolge), gleiche Fehlertexte, gleiche Rechteprüfung. Das Angular-Frontend aus `upstream/src-ui`
bleibt unverändert. Abweichungen nur begründet in `docs/contract/<crate>.md` → "Known differences".

## Betrieb heute (seit 2026-11-14)

```
Browser / Apps ──► paperrust (axum)
                     ├─ Route vorhanden (routes.toml schaltet höchstens ab) → Rust-Handler
                     └─ sonst → 404 (HTML; unter /api/ DRF-JSON {"detail":"Not found."})
                                  │
     PostgreSQL (Schema = Django-Migrationen von upstream + paperrust_*-Tabellen)
       ├─ Aufgaben-Warteschlange paperrust_task_queue (Rust-Worker im selben oder eigenen Prozess)
       ├─ Statusereignisse zwischen Prozessen per LISTEN/NOTIFY
       └─ optional gemeinsamer Cache paperrust_cache (PAPERRUST_CACHE=postgres, Mehrprozessbetrieb)
```

Kein Reverse-Proxy, kein Celery, kein Redis. `PAPERRUST_DJANGO_URL`, `PAPERRUST_QUEUE` und
`PAPERLESS_REDIS` sind wirkungslos (Warnung bzw. Hinweis beim Start). Weil die PaperRust-Migrationen
(z. B. `pl_objects.0001_paperrust_tag_adjacency`) das Schema ändern, kann paperless-ngx dieselbe
Datenbank danach nicht mehr nutzen: vor dem Umstieg sichern und paperless-ngx stoppen.

## Übergangsbetrieb (Strangler) – Geschichte bis 2026-11-13

```
Browser / Apps ──► paperrust (axum)
                     ├─ Route in Rust umgesetzt und in routes.toml aktiv → Rust-Handler
                     └─ sonst → Reverse Proxy an Django (upstream, unverändert)
                                  │
     PostgreSQL (Schema = Django-Migrationen von upstream)  ◄── beide lesen/schreiben
     Redis (Celery-Broker, Channels)  ◄── Celery-Worker von upstream (OCR, Klassifikator, Mail, KI)
```

- **Schema gehört upstream.** Rust liest/schreibt die Django-Tabellen direkt
  (`documents_document`, `documents_tag`, `auth_user`, `guardian_userobjectpermission`, ...).
  Keine eigenen Migrationen in Phase 1.
- **Datenbank Phase 1: PostgreSQL.** SQL wird mit `sea-query` gebaut (nicht als String-Konkatenation),
  damit SQLite später als zweites Backend möglich ist. Rohes SQL nur, wenn es keine sea-query-Form gibt;
  dann kommentiert.
- **Keine sqlx-Compile-Zeit-Makros** (`query!`) – kein DATABASE_URL beim Bauen nötig.
- **Aufgaben (Celery, entfernt):** Rust legte Aufgaben mit einfachen Argumenten (IDs, Strings) als
  Celery-Protokoll-v2-Nachricht im JSON-Format in Redis ab (`pl_core::celery`). Aufgaben, die
  Python-Objekte als Argument brauchen (z. B. `consume_file` mit `ConsumableDocument`), bleiben
  vorerst auf Django (Route nicht aktivieren, im Vertrag vermerken).
- **Suchindex (tantivy):** Rust **liest nur**, Schreiber bleibt der Python-Worker.
- **WebSocket `/ws/status/`:** wurde an Django durchgereicht (heute nativ in Rust).
- **Sitzungen:** `SESSION_ENGINE = cached_db` → Rust liest `django_session` (DB ist maßgeblich),
  prüft die Signatur mit `SECRET_KEY`. Schreibt Rust Sitzungen (Login/Logout), muss es den
  Django-Cache-Eintrag invalidieren.

## Crates und Besitz

Ein Crate pro Fachbereich, **keine Querabhängigkeiten zwischen Fach-Crates** außer den unten
genannten Schnittstellen. Geteiltes gehört in `pl-core` / `pl-authz`.

| Crate | Inhalt | Upstream-Quelle (v3.4.2) |
|---|---|---|
| `pl-core` | AppState, Konfiguration (`PAPERLESS_*`), DB-Pool, Fehler im DRF-Format, JSON/Datum wie DRF, API-Version (`Accept`), Paginierung (`StandardPagination`), generische Filter/Sortierung (django-filter-Lookups), Aufgaben-Schnittstelle (`tasks`, Dispatcher), Cache (lokal/PostgreSQL), Routen-Typ | `paperless/settings`, `paperless/views.py::StandardPagination`, DRF-Verhalten |
| `pl-authz` | Authentifizierung (Session, Token, Basic, Remote-User), CSRF wie Django/DRF, Passwort-Hasher (PBKDF2), Nutzer laden, Modellrechte, guardian-Objektrechte, `get_objects_for_user_owner_aware`, Serialisierung von `owner`/`permissions`/`user_can_change`, `set_permissions` | `paperless/auth.py`, `documents/permissions.py`, `paperless/serialisers.py` |
| `pl-server` | Binary `paperrust`, Routen-Montage, `routes.toml` (nur Abschalten), 404 für nicht bediente Pfade, WebSocket, Zugriffslog, Sicherheitsheader wie Django | `paperless/urls.py`, `paperless/middleware.py` |
| `pl-auth-api` | `/api/token/`, `/api/profile/*`, `/api/users/`, `/api/groups/`, `/api/ui_settings/`, TOTP | `paperless/views.py`, `documents/views.py::UiSettingsView` |
| `pl-objects` | `/api/tags/`, `/api/correspondents/`, `/api/document_types/`, `/api/storage_paths/` (+ `test`), `/api/bulk_edit_objects/` | `documents/views.py`, `documents/serialisers.py`, `documents/filters.py` |
| `pl-documents` | Dokument-Serializer, `/api/documents/` Liste + Detail (GET), `metadata`, `notes`, `history`, `suggestions`*, `selection_data`, `next_asn`, Versionen (GET), `share_links`-Aktion | `documents/views.py::DocumentViewSet/UnifiedSearchViewSet` |
| `pl-docfilter` | Vollständiges `DocumentFilterSet`, `custom_field_query`-DSL, Sortierung inkl. Custom-Field-Sortierung | `documents/filters.py` |
| `pl-files` | `download`, `preview`, `thumb`, `/share/<slug>`, `/api/documents/bulk_download/`, Logo | `documents/views.py` |
| `pl-documents-write` | PUT/PATCH/DELETE Dokument, `/api/trash/`, `/api/documents/bulk_edit/`, `delete/`, `reprocess/`, `rotate/`, `merge/`, `edit_pdf/`, `remove_password/`, `email` (Aufgaben über die PaperRust-Warteschlange) | `documents/views.py`, `documents/bulk_edit.py` |
| `pl-fields-views` | `/api/custom_fields/`, `/api/saved_views/`, `/api/share_links/`, `/api/share_link_bundles/` | `documents/views.py`, `documents/serialisers.py` |
| `pl-workflows` | `/api/workflows/`, `/api/workflow_triggers/`, `/api/workflow_actions/` | `documents/views.py`, `documents/serialisers.py` |
| `pl-search` | `/api/documents/?query=`/`more_like_id`, `/api/search/`, `/api/search/autocomplete/` (tantivy, nur lesen) | `documents/search/`, `documents/views.py` |
| `pl-system` | `/api/tasks/`, `/api/statistics/`, `/api/status/`, `/api/config/`, `/api/logs/`, `/api/remote_version/` | `documents/views.py`, `paperless/views.py` |
| `pl-mail` | `/api/mail_accounts/`, `/api/mail_rules/`, `/api/processed_mail/`, `/api/oauth/callback/` | `paperless_mail/` |
| `pl-ai` | 1:1 `paperless_ai` (KI-Vorschläge, Chat/RAG, Vektorindex) + Erweiterung: Taxonomie-Abgleich gegen Duplikate, MCP-Server `/mcp`, automatische Verarbeitung (siehe docs/AI.md) | `paperless_ai/`, `documents/views.py` (suggestions, chat) |
| `pl-templating` | Dateinamen-/Speicherpfad-Vorlagen (Jinja2-Sandbox mit eigenen Filtern) exakt wie upstream, als Bibliothek für pl-objects (storage_paths test), pl-documents-write (Umbenennen), Workflows | `documents/templating/*`, `documents/file_handling.py` |
| `pl-matching` | Zuordnungsalgorithmen (none/any/all/literal/regex/fuzzy; auto=Klassifikator später), Workflow-Trigger-Abgleich, Datumserkennung aus Text (Ersatz für dateparser) – Bibliothek für den späteren Rust-Worker | `documents/matching.py`, `documents/plugins/date_parsing/*`, `documents/parsers.py` |
| `pl-parsers` | Worker-Baustein: Dateityp-Erkennung, Textextraktion ohne Tika (Text, HTML, DOCX/XLSX/PPTX, ODT/ODS/ODP, RTF, EML/MSG), Metadaten, Vorschaubilder – Bibliothek | `paperless/parsers/*`, `paperless_tika/`, `paperless_mail/parsers.py` |
| `pl-classifier` | Worker-Baustein: Klassifikator (Ersatz für scikit-learn) für automatische Zuordnung von Tags/Korrespondent/Typ/Pfad – Bibliothek | `documents/classifier.py` |
| `pl-queue` | Worker-Baustein: Aufgaben-Warteschlange ohne Redis/Celery (PostgreSQL, SKIP LOCKED), Zeitpläne wie CELERY_BEAT_SCHEDULE, PaperlessTask-Status, Trait `TaskQueue` (einzige Umsetzung: PostgreSQL) | `paperless/celery.py`, `documents/tasks.py`, Settings CELERY_* |
| `pl-ocr` | Worker-Baustein: OCR wie OCRmyPDF (Modi skip/redo/force, Sprachen, Deskew/Rotate, durchsuchbares PDF/PDF-A mit Textebene), Tesseract/PDFium-Anbindung | `paperless/parsers/tesseract.py`, OCRmyPDF-Verhalten |
| `pl-indexer` | Worker-Baustein: tantivy-**Schreiber** (Dokument hinzufügen/aktualisieren/löschen, Neuaufbau) im selben Index-Format wie upstream | `documents/search/*` (Writer-Teil), `documents/tasks.py` (index_*) |
| `pl-dbmigrate` | Werkzeug `paperrust-migrate-db`: Umzug SQLite/MariaDB/MySQL → PostgreSQL (Schema v3.4.2, alle Zeilen, Sequenzen, Prüfung) | `documents/migrations`, `paperless/migrations`, `paperless_mail/migrations`, Django-Feldtypen je Backend |

`*` `suggestions` braucht den Python-Klassifikator → bleibt bis auf Weiteres Django.

### Erlaubte Abhängigkeiten zwischen Fach-Crates

- `pl-documents` → `pl-docfilter` (Filter/Sortierung der Dokumentliste) und optional `pl-search`
  (Volltext in der Liste).
- `pl-documents-write`, `pl-search`, `pl-files` → `pl-documents` (Serializer `serialize_documents`).
- Alles → `pl-core`, `pl-authz`.

Schnittstellen (früh als Stub veröffentlichen, damit andere bauen können):

```rust
// pl-docfilter
pub fn apply(q: &mut sea_query::SelectStatement, params: &QueryParams, user: &pl_authz::User) -> Result<(), ApiError>;
pub fn apply_ordering(q: &mut sea_query::SelectStatement, ordering: Option<&str>) -> Result<(), ApiError>;
// pl-documents
pub async fn serialize_documents(state: &AppState, user: &User, ids: &[i64], opts: &SerializeOpts) -> Result<Vec<serde_json::Value>, ApiError>;
```

## Routen

Jedes Crate liefert `pub fn routes() -> Vec<pl_core::ApiRoute>` (id, Methode, Pfad, Upstream-Quelle,
Handler). Alle Routen sind an; `routes.toml` (optional, `PAPERRUST_ROUTES`) kann eine `id` nur mit
`"off"` abschalten (`"rust"`/`"on"` = an, alter Wert `"django"` = an mit Warnung, unbekannte ids =
Startfehler). Unbekannte Pfade antworten 404. Bis 2026-11-13 schaltete die Datei zwischen `rust` und
`django` (Standard `django`; eine Route ging erst auf `rust`, wenn ihr Paritäts-Diff 0 war).

## Konventionen

- JSON: `serde_json` mit `preserve_order`; Schlüsselreihenfolge = Feldreihenfolge des DRF-Serializers.
- Datum/Zeit wie DRF: ISO 8601 in `PAPERLESS_TIME_ZONE`, `+00:00` → `Z`, Mikrosekunden nur wenn ≠ 0
  (Helfer in `pl-core`). Datum ohne Zeit: `YYYY-MM-DD`.
- Fehler wie DRF: `{"detail": "..."}` bzw. Feldfehler `{"feld": ["..."]}`, gleiche Texte und Status.
- Paginierung wie `StandardPagination` (v9 mit `all`, v10 ohne), `next`/`previous` als absolute URL wie
  Django (`build_absolute_uri`, `replace_query_param`).
- Rechte: immer serverseitig über `pl-authz`. Nie aus Query-Parametern übernehmen.
- Keine Panics in Handlern (`unwrap` nur in Tests). `panic = "unwind"`.
- Jede Datei, die upstream-Logik portiert, nennt die Quelle im Kopfkommentar:
  `// upstream: src/documents/filters.py@v3.4.2 (DocumentFilterSet)`.
- `UPSTREAM_MAP.md` führt Rust-Modul ↔ Python-Dateien ↔ Routen (für `scripts/upstream-diff.sh`).
