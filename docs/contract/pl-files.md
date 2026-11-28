# Vertrag `pl-files` (Dateiauslieferung)

Upstream: paperless-ngx v3.4.2 (`upstream/`). Quellen, falls nicht anders angegeben:
`src/documents/views.py`, `src/documents/conditionals.py`, `src/documents/versioning.py`,
`src/documents/bulk_download.py`, `src/documents/models.py`, `src/documents/permissions.py`,
`src/documents/serialisers.py::BulkDownloadSerializer`, `src/paperless/views.py::FaviconView`,
Django 5.2.18 (`django/http/response.py::FileResponse`, `django/utils/cache.py`,
`django/views/decorators/http.py::condition`, `django/utils/http.py`), DRF 3.18.3,
pathvalidate 3.3.1.

## Routen

| id | Methode | Pfad (axum) | Upstream |
|---|---|---|---|
| `documents.download` | GET (+HEAD) | `/api/documents/{id}/download/` | `DocumentViewSet.download` → `file_response` → `serve_file` |
| `documents.preview` | GET (+HEAD) | `/api/documents/{id}/preview/` | `DocumentViewSet.preview` |
| `documents.thumb` | GET (+HEAD) | `/api/documents/{id}/thumb/` | `DocumentViewSet.thumb` |
| `documents.bulk_download` | POST | `/api/documents/bulk_download/` | `BulkDownloadView.post` |
| `share.get` | GET (+HEAD) | `/share/{slug}` und `/share/{slug}/` | `SharedLinkView.get` (`^share/(?P<slug>\w+)/?$`) |
| `logo.get` | GET (+HEAD) | `/logo`, `/logo/`, `/logo/{*filename}` | `serve_logo` (`^logo(?:/(?P<filename>.+))?/?$`) |
| `favicon.get` | GET (+HEAD) | `/favicon.ico` | `paperless/views.py::FaviconView` |

Nicht in diesem Crate: `metadata` (braucht Parser → `pl-documents`), `/fetch/doc|thumb|preview/<pk>`
(reine `RedirectView`, in pl-server `ui.fetch_*`), `share_link_bundles`-Aufbau (Worker-Aufgabe, pl-fields-views).

GPG: v3.4.2 hat `storage_type` entfernt (Migration `0003_remove_document_storage_type`); es gibt
keine GPG-verschlüsselten Dokumente mehr. Nichts weiterzureichen.

## Gemeinsame Bausteine

### Dateipfade (`Document`-Properties)

- `MEDIA_ROOT` = `PAPERLESS_MEDIA_ROOT` (resolved). `ORIGINALS_DIR = MEDIA_ROOT/documents/originals`,
  `ARCHIVE_DIR = …/archive`, `THUMBNAIL_DIR = …/thumbnails`,
  `SHARE_LINK_BUNDLE_DIR = …/share_link_bundles`.
- `source_path = ORIGINALS_DIR / (filename if filename else f"{pk:07}{file_type}")`
- `has_archive_version = archive_filename is not None` (auch `""` zählt als vorhanden)
- `archive_path = ARCHIVE_DIR / archive_filename`
- `thumbnail_path = THUMBNAIL_DIR / f"{pk:07}.webp"` (immer pk des *effektiven* Dokuments)
- `file_type = get_default_file_extension(mime_type)`: Tabelle der Parser
  (`paperless/parsers/{text,tesseract,mail,tika,remote}.py::_SUPPORTED_MIME_TYPES`), sonst
  `mimetypes.guess_extension`, sonst `""`. `image/tiff` → `.tiff`, wenn der Remote-OCR-Parser
  gültig konfiguriert ist (`engine == "azureai"` und `api_key` und `endpoint`, aus
  `paperless_applicationconfiguration.remote_ocr_*` oder `PAPERLESS_REMOTE_OCR_*`), sonst `.tif`.

### Öffentlicher Dateiname (`Document.get_public_filename(archive, counter, max_bytes=255)`)

1. Kontext = Root-Dokument, falls `root_document_id` gesetzt (Titel/Korrespondent/Datum der Root).
2. `str(doc)` = `created.isoformat()` (`YYYY-MM-DD`) + `" " + correspondent.name` (falls gesetzt)
   + `" " + title` (falls nicht leer).
3. `extension = ".pdf" if archive else file_type` (der Version!); `tail = f"_{counter:02}{ext}"`
   falls `counter` sonst `ext`.
4. `head` = `str(doc)` UTF-8 auf `max(0, 255 - len(tail))` Bytes gekürzt (halbe Zeichen weg).
5. `pathvalidate.sanitize_filename(head+tail, replacement_text="-", max_len=255)`, Plattform
   `universal`: ersetzt `\x00-\x08 \x0e-\x1f \x7f / : * ? " < > | \t \n \r \x0b \x0c \\` durch `-`,
   kürzt auf 255 Bytes, dann *eine* Validierung: reservierter Name (`CON`, `PRN`, `AUX`,
   `CLOCK$`, `NUL`, `COM0-9`, `LPT0-9`, `COM¹²³`, `LPT¹²³`, `:`; Wurzelname = Teil vor erstem
   `.`) → alle Vorkommen durch `NAME_` ersetzen; sonst führendes Leerzeichen / endendes
   Leerzeichen oder Punkt → `lstrip(" ")`, `rstrip(" ")`, `rstrip(" .")`; Leer → `""`.

### `serve_file(doc, use_archive, disposition, follow_formatting)`

- Archiv: Datei `archive_path`, Name `archive_filename` (follow_formatting) sonst
  `get_public_filename(archive=True)`, `Content-Type: application/pdf`.
- Original: Datei `source_path`, Name `filename` bzw. `get_public_filename()`,
  `Content-Type = mime_type`; bei `disposition == "inline"` wird `application/csv`/`text/csv` zu
  `text/plain`; `text/plain`, `text/csv`, `application/csv` bekommen `; charset=utf-8`.
- `Content-Disposition: {disposition}; filename="{ascii}"; filename*=utf-8''{quote(name)}` mit
  `ascii = NFKD(name.replace(",", "_"))` → ASCII (nicht-ASCII verworfen) → `\` und `"` → `_`;
  `quote` = `urllib.parse.quote` (sicher: `A-Za-z0-9_.-~/`, UTF-8-Prozentkodierung, Großbuchstaben).
- `Content-Length` = Dateigröße. Body gestreamt.
- Datei fehlt → `FileNotFoundError` (je nach Aufrufer 404 bzw. Redirect).

### Versionsauflösung (`versioning.py`)

- `latest(root, include_deleted)` = neueste Version `ORDER BY version_index DESC NULLS LAST, id DESC`
  unter `root_document_id = root.id` (ohne gelöschte, außer `include_deleted`), sonst Root.
- View (`_get_effective_file_doc`): Anfrage-Dokument aus `global_objects` (auch Papierkorb).
  Ist es selbst eine Version und kein `version`-Parameter da → dieses. Sonst
  `resolve_requested_version_for_root(root, include_deleted = "version" in query)`:
  Parameter fehlt/leer → `latest`; nicht `int()`-bar → **404 `{"detail":"Invalid version parameter"}`**;
  Kandidat (auch gelöscht, falls Parameter da) fehlt oder gehört nicht zur Root → **404 `{"detail":"Not found."}`**.
- ETag-/Last-Modified-Funktionen (`resolve_effective_document_by_pk`) nutzen dieselbe Logik mit
  `include_deleted=False` (Papierkorb-Dokumente → kein ETag/Last-Modified).

### Rechte (Datei-Routen)

1. DRF: `IsAuthenticated` + `PaperlessObjectPermissions.has_permission` → globale Modellrechte
   `documents.view_document` (Superuser aktiv → ja; inaktiv → nein; Nutzer- oder Gruppenrecht).
   Nicht angemeldet → **401** `{"detail":"Authentication credentials were not provided."}`
   (`WWW-Authenticate: Basic realm="api"`), ohne Recht → **403**
   `{"detail":"You do not have permission to perform this action."}`.
2. Bedingungsprüfung (nur preview/thumb, s. u.) – **vor** dem Objektrecht.
3. Dokument per `global_objects.get(id=pk)` → fehlt: **404 `{"detail":"Not found."}`**.
4. `has_perms_owner_aware(user, "view_document", root)`: Root ohne Owner, Owner == Nutzer,
   aktiver Superuser, oder guardian-Objektrecht `view_document` (Nutzer oder Gruppe) auf die
   Root → sonst **403** `text/html; charset=utf-8` Body `Insufficient permissions`.

## `documents.download`

Query: `original` (`== "true"` → Original), `version`, `follow_formatting` (jeder nicht leere
Wert ist wahr – auch `false`/`0`; Python-Truthiness von `query_params.get(...)`).
Antwort 200: `serve_file(disposition="attachment", use_archive = !original && has_archive)`.
Keine Cache-/ETag-Header. Datei fehlt → 404 `{"detail":"Not found."}`.

## `documents.preview`

Wie download mit `disposition="inline"`, `follow_formatting` immer falsch.
Dekoratoren: `Cache-Control: no-cache` (auf jede zurückgegebene Antwort inkl. 403/304/412, nicht
auf ausgelöste 404), `condition(etag=preview_etag, last_modified=preview_last_modified)`:
- ETag = `checksum` falls `original=true` sonst `archive_checksum` (None → kein ETag), gequotet.
- Last-Modified = `modified` (Sekunden, abgeschnitten).
- Django `get_conditional_response`: If-Match (fehlschlägt → 412), If-Unmodified-Since → 412,
  If-None-Match trifft → 304, If-Modified-Since (nur ohne If-None-Match) → 304. Das geschieht
  **vor** Objektrecht und Existenzprüfung. ETag/Last-Modified werden an jede GET/HEAD-Antwort
  (auch 403) gehängt, falls berechnet.

## `documents.thumb`

`Content-Type: image/webp`, `Content-Disposition: inline; filename="{pk:07}.webp"` (Django
`FileResponse` aus Dateiname), `Content-Length`. Dekoratoren wie preview mit
ETag = `checksum` des effektiven Dokuments, Last-Modified = mtime der Thumbnail-Datei (UTC).
Datei fehlt → 404 `{"detail":"Not found."}`.

## `share.get` (`SharedLinkView`, ohne Authentifizierung)

1. `ShareLink.objects.filter(slug=slug).first()` (ohne soft-gelöschte Links).
   - `expiration < now` → **302** `Location: {LOGIN_URL}?sharelink_expired=1`.
   - sonst `serve_file(doc=share_link.document (auch im Papierkorb, keine Versionsauflösung),
     use_archive = file_version == "archive" && has_archive, disposition="inline")`.
     Datei fehlt → **302** `{LOGIN_URL}?sharelink_notfound=1`.
2. Sonst `ShareLinkBundle` mit Slug: fehlt → 302 `?sharelink_notfound=1`; abgelaufen → 302
   `?sharelink_expired=1`; Status `pending`/`processing` → **202** `text/html; charset=utf-8`
   „The share link bundle is still being prepared. Please try again later.“; Status `failed`,
   `file_path` leer/absolut/außerhalb `SHARE_LINK_BUNDLE_DIR` (nach `resolve()`) oder Datei fehlt →
   **503** „The share link bundle is unavailable.“; sonst 200 `application/zip`,
   `Content-Disposition: attachment; filename='paperless-share-{slug[:12]}.zip'; filename*=utf-8''…`
   (einfache Anführungszeichen wie upstream).
`LOGIN_URL = (PAPERLESS_FORCE_SCRIPT_NAME or "") + "/accounts/login/"`.

## `documents.bulk_download`

Rechte: `IsAuthenticated`, `ViewDocumentsPermissions` (POST → `documents.view_document`).
Parser: nur JSON (`application/json`, sonst 415
`{"detail":"Unsupported media type \"…\" in request."}`); leerer Body → `{}`.
Request (`BulkDownloadSerializer`, Fehlerreihenfolge der Felder):
`documents` (Liste von Ganzzahlen; Feldvalidierung: alle existieren (nicht gelöscht), keine
Duplikate → sonst `{"documents":["Some documents in documents don't exist or were specified twice."]}`),
`all` (bool, Standard false), `filters` (dict), `excluded_documents` (Liste, Standard `[]`),
`content` (`archive`*|`originals`|`both`), `compression` (`none`*|`deflated`|`bzip2`|`lzma`),
`follow_formatting` (bool, Standard false). Objektvalidierung (`non_field_errors`):
`excluded_documents is only supported when all is true.`,
`documents is required unless all is true.`.
Danach: `Document.objects.filter(pk__in=ids)` (Ordnung `-created`); gibt es eines ohne
`view_document` (`permitted_document_ids`, Versionen über ihre Root) → **403** `text/html`
`Insufficient permissions`. Jedes Dokument → `latest(root(doc))`. ZIP in `SCRATCH_DIR`
(Temp-Datei, danach gestreamt), Antwort 200 `application/zip`,
`Content-Disposition: attachment; filename="documents.zip"`, `Content-Length`.
Einträge (`bulk_download.py`):
- `archive`: Archiv falls vorhanden (Name mit `.pdf`), sonst Original.
- `originals`: Original.
- `both`: `archive/<name>` (falls Archiv) und `originals/<name>`.
- Name ohne follow_formatting: `folder + get_public_filename(archive, counter)` mit
  steigendem `counter`, bis der Name noch nicht im ZIP ist; mit follow_formatting:
  `folder / archive_filename` bzw. `folder / filename` (normpath, Duplikate möglich).
Fehlende Datei → 500 (upstream: Exception beim `zipf.write`).

## `logo.get` (`serve_logo`)

`ApplicationConfiguration.objects.first().app_logo`: gesetzt → `MEDIA_ROOT/<name>`; sonst
`PAPERLESS_APP_LOGO` (fehlt → 404); Pfad `MEDIA_ROOT/APP_LOGO.lstrip("/")` resolved muss unter
`MEDIA_ROOT/logo` liegen und Datei sein, sonst 404 (Django-HTML-404). Antwort 200
`Content-Type` per libmagic, `Content-Disposition: attachment; filename="<basename>"`. Der
`filename`-Teil der URL wird ignoriert. Ohne Anmeldung erreichbar.

## `favicon.get`

`STATIC_ROOT/paperless/img/favicon.ico` (`PAPERLESS_STATICDIR`), `image/x-icon`,
`Content-Disposition: inline; filename="favicon.ico"`; fehlt → 404 `text/html` `favicon.ico not found`.

## Nebenwirkungen

Keine DB-Schreibzugriffe, keine Celery-Aufgaben, kein Audit-Log. bulk_download legt eine
Temp-Datei `*-compressed-archive` in `SCRATCH_DIR` an und löscht sie.

## Known differences

1. **Pfad-Traversal:** `filename`/`archive_filename` aus der DB werden lexikalisch geprüft
   (keine absoluten Pfade, kein `..` über das Basisverzeichnis). Upstream öffnet jeden Pfad.
   Betroffene Dokumente → 404 statt Auslieferung. (Sicherheitshärtung, upstream-Daten sind
   normal nie so.)
2. **Nicht-numerische `{id}`:** 404 `{"detail":"Not found."}`; upstream 500 (`ValueError`).
3. **ZIP-Bytes:** Eintragsnamen, Inhalte, Reihenfolge und Kompressionsart wie upstream; Byte-Layout
   (Zeitstempel-Kodierung, Extra-Felder, Versionen) kann abweichen. `compression=bzip2|lzma`
   (Codecs nicht enthalten, keine der beiden Weboberflächen bietet sie an) → seit 2026-11-20 400
   `{"compression": ["\"bzip2\" is not supported by PaperRust. Use \"none\" or \"deflated\"."]}`
   (upstream liefert das ZIP; vorher 404 „nicht unterstützt“). `all=true` läuft über den von pl-server
   installierten `SelectionResolver`; fehlt er (Fehlkonfiguration), 500.
4. **Thumbnail-Last-Modified:** Rust liest die mtime direkt; upstream cached sie 50 min im
   Django-Cache (kann veraltet sein).
5. **JSON-Parse-Fehler** in bulk_download: Status 400 und Schlüssel `detail` gleich, Text
   `JSON parse error - …` enthält die serde-Meldung statt der Python-Meldung.
6. **Übersetzte Texte** (`share.get` 202/503, seit 2026-11-20): Übersetzung aus den upstream-Katalogen
   (`share_i18n.rs`, generiert aus `src/locale/*/LC_MESSAGES/django.po`, nicht-fuzzy) für die aktive Sprache
   (`Accept-Language`/`django_language`-Cookie); ohne Übersetzung Englisch wie Djangos gettext. Vorher bei
   anderer Sprache als Englisch 404 „nicht unterstützt“.
7. **Objektrecht vor Conditional** (preview/thumb, seit 2026-11-27, bewusste Sicherheitsverbesserung
   gegenüber upstream): upstream (`@method_decorator(condition(etag_func=…, last_modified_func=…))`
   auf `DocumentViewSet.preview`/`thumb`) berechnet ETag/Last-Modified über
   `resolve_effective_document_by_pk` **vor** der Objektrechtsprüfung; ein Nutzer mit globalem
   `view_document`, aber ohne Objektrecht, erhielt so bei passendem `If-None-Match` 304 bzw. bei
   `If-Match` 412 und an der 403-Antwort die Prüfsumme als `ETag` und `Last-Modified`
   (Orakel für Existenz/Inhalt). PaperRust prüft zuerst Dokument + Objektrecht
   (`_resolve_request_and_root_doc`, 404/403), erst danach ETag/Last-Modified und die
   Vorbedingungen (304/412), dann die Auslieferung (`files.rs::guarded_conditional_view`).
   Außerdem hängt `conditional::finish` `ETag`/`Last-Modified` nur an 2xx- und 304-Antworten (Django:
   an jede GET/HEAD-Antwort, auch 403/404/412); `Cache-Control: no-cache` bleibt an zurückgegebenen
   Antworten wie upstream. Folgen: nicht existierendes Dokument mit `If-Match` → 404 statt 412;
   403/404/412 ohne Validatoren. Regressionstests `files::tests::refused_request_never_sees_validators_or_304`,
   `conditional::tests::finish_skips_validators_on_errors`.
8. **libmagic** (Logo): eigener Sniffer für PNG/JPEG/GIF/WebP/SVG/ICO/BMP, sonst
   `application/octet-stream`; libmagic kann für exotische Dateien andere Typen liefern.
9. **bulk_download unter Windows (nur Referenz):** upstream öffnet die temporäre ZIP-Datei und
   löscht sie sofort (`f = temp_path.open("rb"); temp_path.unlink()`, POSIX-Idiom). Unter Windows
   schlägt `unlink` einer offenen Datei mit `PermissionError [WinError 32]` fehl, das `except`
   versucht erneut `unlink(missing_ok=True)` und wirft weiter → die Windows-Referenz antwortet bei
   **jedem** Download, der bis zum ZIP kommt, 500 (`reference/logs/django.err.log`). Auf Linux
   (Produktivbetrieb, Docker-Image) liefert upstream 200 mit ZIP – das bildet Rust nach.
   Reines Referenz-/Windows-Artefakt; Akzeptanzregel `bulk-download-windows-unlink` vorgeschlagen.
10. **Kompression/ETag-Schwächung durch GZipMiddleware** (`W/"…"`, kein `Content-Length` bei gzip)
   ist Sache von `pl-server`.

## Umsetzung (Rust)

| Modul | Inhalt |
|---|---|
| `files.rs` | download / preview / thumb (inkl. `condition()` + `cache_control`) |
| `share.rs` | `SharedLinkView` (Links + Bundles) |
| `bulk.rs` | `BulkDownloadSerializer`-Validierung, Strategien, ZIP in `SCRATCH_DIR`, Strom |
| `logo.rs` | `serve_logo`, `FaviconView` |
| `zipw.rs` | ZIP-Schreiber (stored/deflated, ZIP64), konstanter Speicher |
| `naming.rs` | `get_public_filename`, pathvalidate-Port, Content-Disposition, `quote` |
| `conditional.rs` | Django `condition()`/`get_conditional_response`, HTTP-Datum |
| `resolve.rs`, `db.rs`, `docfile.rs`, `serve.rs` | Versionen, DB-Zugriff (sea-query), Pfade, Streaming, Traversal-Schutz |

Auth/Rechte: `pl_authz::AuthUser` + `require_model_perms(DOCUMENT)` bzw.
`require_perms_by_method(VIEW_DOCUMENTS_PERMISSIONS)`, `has_perms_owner_aware`,
`permitted_document_ids`. Pfade/Einstellungen: `pl_core::Config`. DRF-Header `Allow`/`Vary: Accept`:
`pl_core::route::drf_view` (nur API-Routen; share/logo/favicon sind reine Django-Views).
`HEAD` beantwortet axum über die GET-Route (Body entfernt, Header gleich).

## Parität

Skript: `tests/parity/pl-files/compare.py` (stdlib-Python, vergleicht Status, relevante Header und
Body-Hash Django vs. Rust; ZIPs über Eintragsnamen/Kompressionsart/Inhalts-Hash). Ergebnis: **noch
nicht gelaufen** (Referenz-DB/Django und Server-Montage waren während der Umsetzung nicht
verfügbar). Unit-Tests: 21 grün (u. a. upstream-Testfall
`test_download_filename_normalization_does_not_inject_parameters` exakt). Erzeugte ZIPs mit
CPython `zipfile.testzip()` gegengeprüft.
