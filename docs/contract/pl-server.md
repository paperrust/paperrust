# Vertrag `pl-server` (Binary `paperrust`, alt `paperless-rs`)

Besitzer: Server-Agent (Welle 0). Quelle: paperless-ngx v3.4.2 (`upstream/`), Django 5.2.18,
django-cors-headers 4.9.0, django-compression-middleware 0.5.0, django-allauth 65.20.1
(Versionen aus `upstream/uv.lock`).

## Aufgabe

```
Client ──► PaperRust (127.0.0.1:8080)
             frontline (Health, CORS-Preflight, Host-Prüfung, Zugriffslog)
               ├─ montierte Route ──► Body-Gate ──► Handler ──► Django-Middleware-Nachbau
               └─ alles andere ──► Router-Fallback (Index der Oberfläche bzw. 404)
```

Seit 2026-11-14 gibt es **keine Weiterleitung an Django** mehr (`src/proxy.rs` gelöscht): jede Route
wird von PaperRust bedient. Pfade ohne Route und Fälle, die ein Handler als „nicht unterstützt“
markiert (früher „an Django delegieren“), antworten 404 (HTML-Seite, unter `/api/` DRF-JSON
`{"detail":"Not found."}`) – dasselbe Verhalten wie vorher mit `PAPERRUST_DJANGO_URL=off`.

## Konfiguration (Umgebung)

| Variable | Standard | Bedeutung |
|---|---|---|
| `PAPERLESS_RS_BIND` | `127.0.0.1:8080` | `IP:PORT`. Ungültig → Startabbruch (nie stilles `0.0.0.0`). Nicht-Loopback nur explizit, mit Warnung im Log. |
| `PAPERRUST_DJANGO_URL` (alt `PAPERLESS_RS_DJANGO_URL`) | – | **entfernt (2026-11-14)**: wird ignoriert; gesetzt → Warnung beim Start, Start läuft normal weiter |
| `PAPERRUST_STOP_ON_STDIN_EOF` | `no` | `yes`/`1`: geordneter Stopp, sobald stdin EOF liefert (Supervisor schließt die Pipe oder stirbt). Ohne Pipe (kein stdin) = sofortiger Stopp! |
| `PAPERRUST_ROUTES` | `routes.toml` (cwd) | optional. Explizit gesetzt und fehlend → Startabbruch; Standardpfad fehlend → alle Routen an |
| `PAPERRUST_PROXY_CONNECT_TIMEOUT_SECS`, `PAPERRUST_PROXY_RESPONSE_TIMEOUT_SECS`, `PAPERRUST_PROXY_X_FORWARDED_FOR` | – | **entfernt (2026-11-14)** mit dem Proxy; gesetzt → Hinweis im Log, sonst wirkungslos |
| `PAPERLESS_RS_LOG` | `info` | `tracing`-Filter (z. B. `info,paperless_rs::access=warn`) |
| `PAPERLESS_RS_DATABASE_URL` / `PAPERLESS_DB*` | – | Pool (lazy). |

Wie upstream gelesen (für den Middleware-Nachbau): `PAPERLESS_DEBUG`, `PAPERLESS_ALLOWED_HOSTS`,
`PAPERLESS_URL`, `PAPERLESS_CORS_ALLOWED_HOSTS`, `PAPERLESS_USE_X_FORWARD_HOST`,
`PAPERLESS_COOKIE_PREFIX`, `PAPERLESS_ENABLE_COMPRESSION` – Semantik wie
`settings/__init__.py` + `settings/parsers.py` (`str_to_bool`: ungültig → Startfehler wie
upstream `ValueError`).

## routes.toml

```toml
[routes]
"documents.email" = "off"   # Route abschalten (wird nicht montiert → 404)
"tags.list" = "rust"        # = "on" = an (Standard für alles Nicht-Gelistete)
```

Seit 2026-11-14 schaltet die Datei Routen nur noch **ab**: `"off"` → nicht montiert; `"rust"`/`"on"`
→ an; nicht genannte ids → an (vorher: Django). Der alte Wert `"django"` wird mit Warnung als „an“
gelesen. Die Datei ist optional.

Startfehler mit klarer Meldung bei: unbekannter id, unbekanntem Schlüssel/Wert, Syntaxfehler,
doppelter Routen-id, doppeltem `(Methode, Pfad)` (Parameternamen normalisiert: `{id}` = `{pk}`),
gleichem Pfadmuster mit unterschiedlichen Parameternamen, Pfad ohne führendes `/` oder mit
`? # %`, Pfad unter `/__rs/`.

Montage: aktive Routen werden pro Pfad zu einem `MethodRouter` zusammengeführt (auch über
Crates hinweg). Nicht aktive Methoden eines Pfades → unter `/api/` DRFs 401/403/405, sonst die
404-Seite (docs/STANDALONE-GAPS.md §5). `HEAD` folgt `GET` (axum, wie Django). Unbekannte Pfade →
Router-Fallback (GET/HEAD: Index der Oberfläche wie upstreams Catch-all, sonst 404).

## Vor jeder View (`frontline::before_views`)

| Fall | Antwort |
|---|---|
| `OPTIONS` mit `Access-Control-Request-Method` | Preflight-Antwort wie `CorsMiddleware.check_preflight` |
| Host nicht erlaubt / kein Host | Djangos 400-Seite (`DisallowedHost`) |

`%`-kodierte Pfade werden in Rust geroutet. Die frühere Schicht `param_guard` (Punkt bzw.
nicht-numerischer Wert in `{id}` → Django) gibt es seit 2026-11-14 nicht mehr; solche Pfade passen
auf keine Route bzw. scheitern an der Typprüfung des Handlers (404).

## Rust-Routen: Body-Gate und „nicht unterstützt“ (`src/body.rs`)

Jeder montierte Handler liegt hinter `body::gate` (Markierungen aus `pl_core::RouteFlags`):

0. Request mit abgelehntem DRF-Format-Suffix (`frontend::urlconf::RejectFormat`, siehe „Format-Suffixe“)
   → **404** `{"detail":"Not found."}` mit dem `Allow` der View und `Vary: Accept`, vor jeder
   Authentifizierung (DRFs Content-Negotiation scheitert in `APIView.initial` zuerst; daher kein
   `X-Api-Version`).
1. `POST`/`PUT`/`PATCH` mit `application/x-www-form-urlencoded` bzw. `multipart/form-data` auf
   einer Route ohne `accepts_form` bzw. `accepts_multipart` → unter `/api/` DRFs **415**
   `{"detail":"Unsupported media type \"<Content-Type>\" in request."}` mit `Allow`/`Vary: Accept` der View
   (seit 2026-11-20; vorher 404), sonst 404 wie ein Pfad ohne Route. Für Views mit eigenen
   `parser_classes` (z. B. bulk_edit nur JSON, selection_data Multipart+JSON) ist das genau upstream; für
   Views mit DRF-Standardparsern, deren Rust-Handler nur JSON liest, ist es eine Known difference
   (upstream verarbeitet den Formular-Körper; Beispiel `storage_paths/test/`). Markiert:
   pl-auth-api (alle, JSON/Form/Multipart), pl-system `tasks.acknowledge`/`tasks.run`
   (alle drei), pl-fields-views (alle, nur Form), pl-documents-write `documents.update`,
   `documents.partial_update`, `documents.notes.create`, `trash.action`,
   `documents.versions.partial_update` (alle drei), pl-documents `documents.selection_data` (Multipart),
   pl-ai `documents.chat` (alle drei).
2. Body wird bis **2,5 MiB** (`BODY_BUFFER_LIMIT` = `pl_core::body::DATA_UPLOAD_MAX_MEMORY_SIZE`,
   Djangos Standard, von paperless nicht überschrieben) gepuffert. Größer → Djangos 400-Seite
   (`RequestDataTooBig`); gestreamte Routen (`documents.post_document`) lesen selbst. So kann ein anonymer
   Client vor jeder Rechteprüfung höchstens 2,5 MiB pro Request belegen (Review 2, H1).
   `DefaultBodyLimit` von axum steht auf demselben Wert; Handler, die selbst lesen, nutzen
   `pl_core::body::read_body` (zu groß → Djangos 400-Seite).
3. Antwortet der Handler mit `pl_core::DelegateToDjango` (`pl_core::delegate_to_django()`,
   `ApiError::delegate_to_django`; Name historisch, heißt jetzt „Fall nicht unterstützt“), kommt die
   404 eines Pfades ohne Route (Debug-Log). Bis 2026-11-14 ging hier die Original-Anfrage an Django.
   Eine gestreamte Route darf so nicht antworten (sonst 500 + Fehler im Log).
4. Lesefehler des Bodys → Djangos `Bad Request (400)`-Seite.

### OPTIONS auf DRF-Views (`src/drf_options.rs`, seit 2026-11-20, Paritätslauf F4)

Ein `OPTIONS` auf einen `/api/`-Pfad mit Rust-Routen (außer `NO_AUTH_API_PATHS`), den keine Route selbst
bedient, beantwortet der Methoden-Fallback (`routing::method_not_allowed`) wie DRFs `APIView.options` →
`SimpleMetadata.determine_metadata`: erst Authentifizierung (401/403 wie beim 405), dann **200**
`{"name", "description", "renders", "parses"}` mit `Allow` (alle Methoden des Pfads, DRF-Reihenfolge)
und `Vary: Accept`; Einrückung per `Accept: …; indent=N`. Herkunft der Werte: `frontend::urlconf::UPSTREAM`
(View-Klasse/Aktion je Muster) + Tabellen aus dem upstream-Code:
`name` = `get_view_name` (Klassenname ohne `View`/`ViewSet`, `camelcase_to_spaces` inkl. `str.title()`,
Router-Suffix „List“/„Instance“; Aktionen `pretty_name(funktion)` bzw. `@action(name=…)`: „Audit Trail“,
„Get Next ASN“); `description` = Klassen-Docstring bzw. Aktions-Docstring (dedent); `renders` immer
`["application/json", "text/html"]`; `parses` Standard JSON/Form/Multipart, abweichend nur JSON
(`DocumentOperationPermissionMixin`-Views, `BulkDownloadView`, `BulkEditObjectsView`), Multipart
(`PostDocumentView`, `update_version`), Multipart+JSON (`SelectionDataView`). `/api/token/` beantwortet
`OPTIONS` selbst (pl-auth-api, inkl. `actions`). Known differences siehe unten (11).

### Format-Suffixe und unbekannte API-Pfade (seit 2026-11-20, Paritätslauf F5)

* DRFs `DefaultRouter` hängt an jedes Router-Muster die Variante `\.(?P<format>[a-z0-9]+)/?$` an
  (`/api/tags/5.json/`, `/api/tags.json`). `frontend::urlconf::rewrite_format_suffix` (vor dem Routing):
  Format `json`/`api` → Pfad ohne Suffix (Query bleibt); jedes andere Format (z. B. `/api/tags/1.5/` = pk 1,
  Format „5“) → derselbe Pfad mit Marker `RejectFormat` → Body-Gate 0. (JSON-404). `.api` (Browsable API)
  liefert in PaperRust JSON (Known difference 12).
* Ein GET/HEAD auf einen `/api/`-Pfad ohne Route (`/api/foo/`, `/api/users/2` ohne Schrägstrich, `/api`)
  bekommt DRFs JSON-404 `{"detail":"Not found."}` statt der Weiterleitung in die Weboberfläche
  (`frontend::index_handles`). upstream rendert dort über den Catch-all die Angular-Seite bzw. leitet
  anonym zur Anmeldung um (Known difference 13). Pfade außerhalb `/api/` leiten weiter wie bisher nach
  `/rs/app/…`.

Am Router liegen außerdem `Extension(pl_authz::Authz)` (Pool + `AuthConfig::from_core`) und
pro Request `pl_core::RequestNotes`; `ConnectInfo<SocketAddr>` setzt `serve.rs` pro Verbindung.

## Reverse Proxy (entfernt 2026-11-14)

Bis 2026-11-14 reichte `src/proxy.rs` alles ohne aktive Rust-Route byte-gleich an Django weiter
(Header-Schreibweise erhalten, Hop-by-hop entfernt, Bodies/SSE/WebSocket gestreamt, 502/504 bei
nicht erreichbarem Django, `X-Forwarded-For` angehängt, Zeitlimits
`PAPERRUST_PROXY_*`). Modul, Tests (`tests/proxy.rs`) und Variablen sind entfernt; die Beschreibung
steht in der Git-Historie dieser Datei. Die Client-IP für Login-Drossel/Audit liest PaperRust
selbst (`PAPERLESS_TRUSTED_PROXIES`/`PAPERLESS_ALLAUTH_TRUSTED_PROXY_COUNT` wie upstream, wenn ein
eigener Reverse Proxy davor steht).

## Middleware-Nachbau für Rust-Antworten (`src/django_mw.rs`, `src/compress.rs`)

Reihenfolge wie Djangos Antwortweg (umgekehrte `MIDDLEWARE`-Liste):

| upstream-Middleware | Wirkung in Rust |
|---|---|
| `XFrameOptionsMiddleware` | `X-Frame-Options: SAMEORIGIN` (setdefault; Ausnahme per Antwort-Extension `XFrameOptionsExempt`) |
| `ApiVersionMiddleware` | `X-Api-Version: 10`, `X-Version: 3.4.2` wenn authentifiziert: pl-authz notiert das in `pl_core::RequestNotes` (oder Handler setzt `pl_core::Authenticated`) |
| `CsrfViewMiddleware`, `SessionMiddleware` (Cookies) | CSRF: pl-authz. Löschung eines leeren/ungültigen Session-Cookies: pl-authz notiert `SessionState::delete_cookie_header` in `RequestNotes`, pl-server hängt das `Set-Cookie` an |
| `LocaleMiddleware` | `Vary += Accept-Language`, `Content-Language` (setdefault) = ausgehandelte Sprache (Port von `get_language_from_request`, Cookie `{prefix}django_language`, `src/locale.rs`) |
| `CorsMiddleware` | `Vary += origin`; bei erlaubter Origin (Schema+netloc in `CORS_ALLOWED_ORIGINS`): `access-control-allow-origin`, `-allow-credentials: true`, `-expose-headers: Content-Disposition` |
| `SessionMiddleware` (`accessed`) | `Vary += Cookie` immer (Referenz geprüft: auch bei Token-/Basic-Auth, anonym, 401) |
| `SecurityMiddleware` | setdefault `X-Content-Type-Options: nosniff`, `Referrer-Policy: same-origin`, `Cross-Origin-Opener-Policy: same-origin` (Django-Standard, upstream setzt nichts davon um; kein HSTS) |
| `StreamAwareCompressionMiddleware` (Standard an) | Port von compression_middleware 0.5.0: `text/event-stream` nie; vorhandenes `Content-Encoding` nie; nicht-streamend < 500 Byte nie; sonst `Vary += Accept-Encoding`, Wahl zstd > br > gzip (q ignoriert außer q=0, `*` = zstd), Stufen zstd 7 / br 4 / gzip 6; nicht-streamend nur wenn ≥ 100 Byte gespart, dann `Content-Length` neu; streamend ohne `Content-Length`; starkes `ETag` → `W/` |

`CommonMiddleware` (Content-Length) erledigt hyper. `WhiteNoise` betrifft nur `/static/` (Django).

## Health

`GET|HEAD /__rs/health` nur von Loopback-Peers → `200 {"status":"ok","rust_routes":N}`.
Andere Peers bzw. andere `/__rs/…`-Pfade → Router-Fallback (404 wie ein unbekannter Pfad).

## Zugriffslog

`target=paperless_rs::access`, Level info: Methode, Pfad **ohne Query** (Tokens/Suchbegriffe),
Status, Dauer ms, `via=rust|internal` (`internal` = Health/Branding-Dateien; `django` gibt es seit
2026-11-14 nicht mehr), Peer-IP. `/share/<slug>` wird als
`/share/<redacted>` geloggt (Capability-URL).

Seit W18 (E2E-Befund 2026-11-08: ein hängender, an Django weitergereichter `POST
/api/document_types/` hinterließ keine Zeile) zusätzlich, **nach** den bisherigen Feldern (Parser wie
`tests/e2e/compare.mjs` bleiben gültig):

* `gate_ms` – Zeit im Body-Gate (Puffern des Request-Bodys einer Rust-Route), sonst `-`;
* `outcome="ok"` bzw. `outcome="client closed"`: Wird der Request-Future verworfen, bevor eine
  Antwort existiert (Client schließt die Verbindung, z. B. Navigation während der Handler noch
  arbeitet), schreibt ein Drop-Guard trotzdem die Zeile mit `status=499` (nginx-Konvention „client
  closed request“) und der Dauer bis zum Abbruch.

Das Feld `upstream_ms` (Wartezeit auf Django) ist seit 2026-11-14 entfernt, ebenso die Warnung
`django answered slowly`. Test: `tests/access_log.rs`.

## Branding (`src/branding.rs`, 2026-11-08)

Nur Logo, Name, Akzentfarbe; kein Fork von `src-ui` (docs/BRANDING.md „Umsetzung ohne Fork“).

| Variable | Standard | Bedeutung |
|---|---|---|
| `PAPERRUST_BRANDING` | `on` | `on/yes/true/1` bzw. `off/no/false/0`, sonst Startfehler. `off` = byte-transparent wie ohne Branding. |
| `PAPERRUST_ACCENT` | `#B7410E` | `#RRGGBB`/`RRGGBB`/`#RGB`, sonst Startfehler. H/S/L ganzzahlig gerundet, L so weit gesenkt, bis Kontrast zu `#f8f9fa` (Button-Schrift, damit auch weiß) ≥ 4,5:1. |

- **Name/Logo:** `main.rs` füllt bei Branding an *nicht gesetzte* `PAPERLESS_APP_TITLE`/`_LOGO` in
  `pl_core::Config` mit `PaperRust` / `/__rs/brand/logo.svg` → wirkt auf `ui_settings.get` (pl-auth-api
  unverändert). Leer gesetzt bleibt leer (upstream: kein Branding). Django: docs/OPERATIONS.md.
- **Kandidat** (vor dem Routing entschieden): `GET`, kein `Range`/`Upgrade`, `Sec-Fetch-Dest: document`
  bzw. ohne `Sec-Fetch-Dest` `Accept` enthält `text/html`; Pfad nicht `/api`, `/ws`, `/share`, `/static`,
  `/assets`, `/logo`, `/fetch`, `/admin`, `/media`, `/__rs` (je exakt oder mit `/…`) und nicht `/favicon.ico`.
  Für Kandidaten wird `Accept-Encoding` **vor** Django/Handler entfernt.
- **Injektion** nur bei Status 200, `Content-Type: text/html…`, ohne `Content-Encoding` (außer
  identity), ohne `Content-Disposition: attachment`, bekannte Größe ≤ 4 MiB, vorhandenem `</head>` und
  ohne vorhandenes `id="paperrust-brand"` (also genau einmal): vor `</head>` ein
  `<style id="paperrust-brand">` (Angular-Index, erkannt an `<pngx-root`: `:root{--pngx-primary:H,S%;
  --pngx-primary-lightness:L%}` + Dunkelmodus-Selektoren wie theme.scss; sonst Konto-Seiten:
  `body{--pngx-primary:#hex;…}` wie static/base.css), danach `<link rel="icon" type="image/svg+xml"
  href="/__rs/brand/favicon.svg">`; jedes `<link>` mit rel-Token `icon` bekommt
  `href="/__rs/brand/favicon.ico"` (gab es keins, wird eins ergänzt; `apple-touch-icon` bleibt).
  `Content-Length` neu, `ETag` `"x"` → `"x-prb<fnv32 des CSS>"` (schwach bleibt schwach), `Content-MD5` entfernt.
- **Kompression danach** mit dem Original-`Accept-Encoding` nach Django-Regeln (`compress.rs`), auch für
  Kandidaten-Antworten ohne Injektion (Redirects, Nicht-HTML) – so sehen sie aus wie von Django mit
  Kompression (dekomprimiert gleich, Bytes siehe Known difference 1). Rust-HTML-Antworten: Injektion
  nach `finish_headers`, vor der Kompression. Alle anderen Antworten (API, Dateien, SSE, WebSocket)
  laufen unverändert gestreamt durch.
- **Assets** `GET|HEAD /__rs/brand/{logo,logo-inverse,logo-mono,app-icon,favicon}.svg`, `favicon.ico`
  (jeder Peer, nur bei Branding an; sonst und für unbekannte Namen → Django 404). `Cache-Control: public,
  max-age=86400`, `X-Content-Type-Options: nosniff`, `Content-Security-Policy: default-src 'none';
  style-src 'unsafe-inline'`, `ETag`. Keine Django-Middleware-Header (wie Health).

Tests: `src/branding.rs` (Kontrast, Farbparser, Kandidaten, Injektion genau einmal, Favicon-Links,
ICO-Raster gegen favicon.svg-Geometrie), `tests/branding.rs` (Injektion mit/ohne Kompression,
Rust-HTML, API/Nicht-Navigation/POST unverändert, `off` byte-transparent, Assets).

## Known differences

1. **Kompression, Bytes:** gleiche Entscheidung und gleiches `Content-Encoding`, aber andere
   Kompressor-Implementierung → komprimierte Bytes und damit (selten, an der 100-Byte-Grenze)
   die Entscheidung „lohnt sich“ können abweichen. Dekomprimierter Inhalt gleich.
2. **„Streaming“** wird als „Body ohne exakte Größe“ oder Extension `compress::Streaming`
   erkannt (Django: `StreamingHttpResponse`/`FileResponse`). Datei-Downloads aus Rust sollten
   `Streaming` setzen, damit auch kleine Dateien wie in Django komprimiert werden.
3. **Header-Reihenfolge/-Schreibweise bei Rust-Antworten:** Rust-Header werden klein geschrieben
   gesendet, Reihenfolge kann abweichen (HTTP-semantisch gleich).
4. **`X-Api-Version`/`X-Version`** und Session-Cookie-Löschung bei Rust-Antworten nur, wenn der
   Handler über pl-authz authentifiziert (`AuthUser`/`MaybeUser`) oder `pl_core::Authenticated`
   setzt. Django prüft `request.user` in der Middleware immer (Session); ein Rust-Handler ohne
   Authentifizierung (z. B. `/share/…`, Logo) sendet die Header daher auch bei gültiger Session
   nicht. Scheitert die Authentifizierung (401/403), wird ebenfalls nichts notiert.
5. **`check_for_language`** wird für alle `LANGUAGES` als wahr angenommen (Django prüft die
   Existenz eines Katalogs; betrifft evtl. `no-no`).
6. **`HEAD` auf Rust-GET-Routen:** axum entfernt den Body erst nach dem Handler; die
   Kompressionsentscheidung sieht daher den leeren Body (kein `Content-Encoding` bei HEAD).
7. **`Date`-Header:** hyper setzt `Date`, falls die Antwort keinen hat; granian sendet ihn ohnehin.
   Rust-Antworten haben keinen `server`-Header (upstream: `server: granian`).
8. **Panik im Handler:** wird abgefangen → Djangos generische 500-Seite (`ApiError::internal`),
   Middleware-Header wie bei Django; Verbindung bleibt nutzbar.
9. **Request-Trailer** werden beim Puffern verworfen. Upload-Routen mit Spooling auf Platte tragen die
   Markierung *streaming* (`upload::STREAMING_ROUTES`, derzeit nur `documents.post_document`); alle anderen
   Bodies > 2,5 MiB → Djangos 400-Seite (bis 2026-11-14: an Django weitergereicht).

10. **Branding (Standard an):** HTML-Navigationen enthalten das Brand-Style/Favicon-Links,
   `ui_settings.get` liefert `app_title`/`app_logo` = PaperRust, wenn `PAPERLESS_APP_*` nicht gesetzt sind;
   `HEAD` auf diese Seiten wird nicht angepasst (`Content-Length` des unveränderten Django-HEAD).
   Für Paritätsläufe `PAPERRUST_BRANDING=off`.

11. **OPTIONS-Metadaten** (`drf_options`): kein `actions`-Teil (Feld-Metadaten des Serializers für
    Views mit POST/PUT – upstream z. B. `actions.POST` bei `/api/tags/`); view-spezifische Rechte vor den
    Metadaten (z. B. Admin-Views, Modellrechte) werden nicht geprüft (nur Authentifizierung, wie beim 405);
    PaperRust-eigene `/api/`-Routen ohne upstream-View bekommen als `name` das letzte Pfadsegment
    (`/api/x-ai/review/` → „Review“), `description` leer, Standard-Renderer/-Parser.
12. **Format-Suffix `.api`:** wie `.json` (JSON), upstream die HTML-Seite der Browsable API.
13. **Unbekannte `/api/`-Pfade (GET/HEAD):** JSON-404 statt Angular-Index/Login-Weiterleitung (Catch-all
    `re_path(r".*")`); für API-Clients eindeutiger. Ebenso `version_id` ohne `\d+` in pl-documents-write.
14. **415 aus dem Body-Gate** kommt vor der Authentifizierung (DRF parst erst beim Zugriff auf
    `request.data`, also nach Rechteprüfung): ein anonymer Formular-POST bekommt 415 statt 401/403, ohne
    `X-Api-Version`.

## CORE-REQUEST

Alle Punkte erledigt (Integration, 2026-11-08): `pl_core::Authenticated`/`RequestNotes`,
`pl_core::locale` (diese Implementierung, `pl_server::locale` ist ein Re-Export),
Workspace-Abhängigkeiten, `Config::bind` = `127.0.0.1:8080`. Historie:

1. **Authentifiziert-Markierung für `ApiVersionMiddleware`** (in `pl-core`):
   eine Antwort-Extension `pub struct Authenticated;` in `pl-core`, die `pl-authz` (bzw. der
   Handler nach erfolgreicher Authentifizierung) setzt; `pl-server` setzt dann
   `X-Api-Version: 10`/`X-Version: 3.4.2` wie `ApiVersionMiddleware`. (Bis dahin wertet
   `pl-server` seinen eigenen Typ `django_mw::Authenticated` aus.)
2. **Sprachaushandlung doppelt:** `pl_core::request::language_from_request` und
   `pl-server/src/locale.rs` (gegen Django 5.2.18 portiert und gegen die Referenz geprüft:
   `de-CH`→`de-de`, Cookie `fr`→`fr-fr`). Bitte eine gemeinsame Version (im Kern) behalten;
   `pl-server` stellt dann um.
3. **Workspace-Abhängigkeiten** (bis dahin fest in `crates/pl-server/Cargo.toml`):
   `hyper-util 0.1.21` (client-legacy, server-auto, server-graceful, http1, http2, tokio, service),
   `http-body-util 0.1.5`, `bytes 1.12.1`, `toml 1.2.0`, `tokio-util 0.7.18` (io),
   `async-compression 0.4.42` (tokio, gzip, brotli, zstd); dev: `tokio-tungstenite 0.29.0`
   (gleiche Version wie axum-ws). `hyper` braucht Features `http1, http2, client, server`.
4. ~~`AppState::from_env`~~ erledigt (`AppState::new`/`with_pool` genutzt). PaperRust braucht
   PostgreSQL (andere Engine → Startabbruch mit Meldung).
5. **`pl_core::Config::bind`** (Standard `0.0.0.0:8001`) widerspricht der Server-Vorgabe
   (`PAPERLESS_RS_BIND`, Standard `127.0.0.1:8080`, nie still `0.0.0.0`). `pl-server` liest
   `PAPERLESS_RS_BIND` selbst (`config.rs`); bitte das Feld im Kern
   entfernen oder auf `127.0.0.1:8080` angleichen. Die Django-Settings (Hosts, CORS,
   Cookie-Präfix, Kompression) übernimmt `pl-server` in `main.rs` aus `pl_core::Config`
   (`DjangoSettings::from_core`; Test `agrees_with_pl_core` sichert Gleichheit mit dem eigenen Parser).
6. ~~pl-documents-write `cfi` without file move~~ done (2026-11-28): pl-documents-write offers
   `cfi::update_or_create_deferred(conn, ctx, doc_id, field, value)` and
   `cfi::soft_delete_deferred(conn, ctx, c, transaction_id)` (same DB/auditlog part as
   `update_or_create` / `soft_delete(then_full_save=false)`, without `after_custom_field_saved`); the local
   mirror `src/ai_cfi_tx.rs` is gone.

## Frontend und Betrieb ohne Django (W22, 2026-11-08, `src/frontend/`, `src/shutdown.rs`)

> **Known difference seit 2026-11-13 (Ablösung Angular, docs/FRONTEND-PARITY.md):** Ist die Light-UI
> eingehängt (`light_ui::Mounted`), rendert `ui.index` die Angular-Index-Seite nicht mehr, sondern antwortet
> auf `/` und jede SPA-Adresse mit `302` auf denselben Pfad unter `/rs/app/` (Query bleibt,
> `light_ui::legacy_redirect`); die Anmeldung erledigt dann `/rs/app/`. Ohne Light-UI-Build (oder mit
> `PAPERRUST_LIGHT_UI=off`) bleibt das upstream-Verhalten unten. Außerdem setzt `pl-server` die
> `paperrust_share_mode`-Metatags (`share_meta`) in `index.html`/`m.html` der Light-UI ein
> (`LightUi::inject_head`).

Ziel: PaperRust läuft komplett ohne Django – API **und** Oberfläche. Quelle: `src/paperless/urls.py`,
`src/documents/views.py::IndexView`, `src/documents/templates/index.html`, whitenoise 6
(`base.py`, `responders.py`, `middleware.py`, `media_types.py`), Django 5.2 (`login_required`,
`redirect_to_login`, `ensure_csrf_cookie`, `RedirectView`).

### Routen (nur über `routes.toml` aktiv)

| id | Methode | Pfad | upstream | Verhalten |
|---|---|---|---|---|
| `ui.index` | GET/HEAD | `/` **und Catch-all** | `re_path(r".*", login_required(ensure_csrf_cookie(IndexView)))` | jeder GET/HEAD ohne Rust-Route, der in Django auf **keine** spezielle View fällt (`frontend::urlconf::UPSTREAM`), rendert den Index (Router-Fallback). Anonym → `302 /accounts/login/?next=<get_full_path, quote(safe="/")>`. Angemeldet (Session, `AutoLoginMiddleware`, Remote-User wie pl-accounts) → `index.html`, `csrftoken` neu gesetzt (+`Vary: Cookie`), Django-Messages (`window.DJANGO_MESSAGES`, `escapejs`) verbraucht (Cookie gelöscht). Sprache der Bundles: `ui_settings.settings.language` (truthy), sonst `get_language()`; `en-us` → `en-US`. Texte des Ladebildschirms: gettext der aktiven Sprache (`frontend/index_i18n.rs`). Token-/Basic-Auth zählen nicht (nur Django-Session wie upstream). |
| `static.files` | alle | `/static/{*path}` | `WhiteNoiseMiddleware` (`STATIC_ROOT` = `PAPERLESS_STATICDIR`, Präfix `/static/`) | Datei-Liste einmal beim Start (`StaticFiles::scan`, Referenz: 7440 Dateien in ~1,5 s); `Content-Type` aus whitenoise-Tabelle (`text/*` mit `; charset="utf-8"`), `Cache-Control: max-age=60, public` (DEBUG: 0), `Access-Control-Allow-Origin: *`, `Vary: Accept-Encoding` bei Vorkomprimaten, `Last-Modified`, `ETag "<mktime(Last-Modified) in TIME_ZONE>-<size>"` (whitenoise-Eigenheit, gegen Referenz geprüft), kleinste passende Alternative (`\bbr\b`/`\bgzip\b`, `*` = keine), `304` (If-None-Match exakt / If-Modified-Since), ein Byte-Range (`start >= end` → 416 wie whitenoise), andere Methoden `405 Allow: GET, HEAD`. Nur SecurityMiddleware-Header + Kompression (Marker `django_mw::SecurityOnly`, `compress::Streaming` = FileResponse). Datei fehlt → wie Django weiter zum Index. Aktiv → `static.swagger_ui_dist` (pl-cli) wird nicht montiert (WhiteNoise liefert auch die Sidecar-Dateien). |
| `ui.api_root` | GET | `/api/` | `RedirectView(url="schema/view/")` | 302 |
| `ui.assets` | GET | `/assets/{*path}` | `RedirectView(STATIC_URL + "frontend/en-US/assets/%(path)s")` | 302, Pfad dekodiert, `iri_to_uri` |
| `ui.fetch_doc` / `ui.fetch_thumb` / `ui.fetch_preview` | GET | `/fetch/{doc,thumb,preview}/{num}` | `RedirectView(BASE_URL + "api/documents/%(pk)s/{download,thumb,preview}/")` | `\d+`, sonst Index |

`/favicon.ico` (`favicon.get`) und `/logo…` (`logo.*`) liegen in pl-files. Branding (`branding.rs`)
greift auf den Rust-Index wie vorher auf Djangos Index (Rust-Antwort → `inject_response`).
Service-Worker: upstream hat keinen (kein `ngsw`).

### Zusammengefasste Static-Ablage (F1, 2026-11-09, `statics.rs`, `frontend/tools/static-dedupe.mjs`)

Problem: der Angular-Build legt je Sprache (37) einen vollständigen Ordner ab; PDF.js-Daten (`assets/`),
`styles.css`, Icons usw. sind in allen Sprachen bytegleich, `main.js` unterscheidet sich nur in den
eingebetteten Übersetzungen. `frontend/build.sh` legt deshalb (Standard; `FRONTEND_DEDUPE=0` schaltet ab)
jede sprachübergreifend gleiche Datei nur einmal ab (kanonisch `en-US`, sonst die alphabetisch erste
Sprache; gleicher relativer Pfad **und** gleicher Inhalt) und jedes andere `main.js` als Copy/Insert-Delta
gegen `frontend/en-US/main.js` (≈ 80 KB statt 3,1 MB). Manifest `<STATIC_ROOT>/.paperrust/static.json`:

```json
{"format": "paperrust-static/1",
 "alias": [{"dir": "frontend/de-DE/", "to": "frontend/en-US/", "files": ["assets/js/pdf.min.mjs", "…"]}],
 "delta": {"frontend/de-DE/main.js": {"base": "frontend/en-US/main.js", "base_size": 3107324,
           "file": ".paperrust/delta/de-DE/main.js.prd", "size": 3119296, "fnv1a32": "…", "sha256": "…"}}}
```

Delta `PRD1`: `"PRD1"`, dann Befehle `0x01 off:u32le len:u32le` (aus der Basis kopieren) bzw.
`0x02 len:u32le <bytes>` (einfügen). `index.html` und alles außerhalb von `frontend/` bleiben echte Dateien.

`StaticFiles::scan` nimmt die Einträge als **virtuelle Dateien** in dieselbe Tabelle wie echte Dateien auf;
danach gilt unverändert die whitenoise-Logik (Alternativen `.gz`/`.br` – auch virtuelle –, Content-Type,
Cache-Control, Vary, Last-Modified, ETag `mtime-size`, 304, Range, 405/416):

* **alias**: Bytes, Größe und mtime der gespeicherten Datei → ETag/Last-Modified gleich wie bei der
  kanonischen Kopie (vorher: Kopierzeit je Sprache, also ebenfalls „irgendein Build-Zeitpunkt“).
* **delta**: Größe aus dem Manifest, mtime der `.prd`-Datei. GET ohne passendes Vorkomprimat baut den
  Inhalt im Blocking-Pool neu (Basis lesen, deren Größe prüfen, Delta anwenden, Größe + FNV-1a-32 gegen
  das Manifest) und hält die letzten **3** Ergebnisse im Speicher (≈ 3 MB je Sprache); HEAD baut nichts.
  Passt etwas nicht (Basis ersetzt, Delta beschädigt): `500` wie bei einer verschwundenen Datei +
  `ERROR … static delta file cannot be rebuilt` – nie falsche Bytes. `main.js.gz`/`.br` der Sprache
  bleiben echte Dateien und werden direkt ausgeliefert.
* `<STATIC_ROOT>/.paperrust/` wird nie ausgeliefert (`/static/.paperrust/…` → wie fehlende Datei → Index).
* Ohne Manifest: Verhalten wie bisher. Manifest unlesbar/unbekanntes Format: `ERROR`, nur echte Dateien.
  Einträge, deren gespeicherte Datei fehlt oder die eine echte Datei überdecken würden: übersprungen
  (`WARN … skipped=N`); fehlende `.gz`/`.br`-Ziele (Desktop-Paket ohne Vorkomprimate) still.
* Pfade im Manifest: relativ, `/`-getrennt, ohne `.`/`..`/Laufwerk/Backslash, sonst übersprungen.

Django/WhiteNoise oder nginx kennen das Manifest nicht: für sie mit `FRONTEND_DEDUPE=0` bauen oder
`node frontend/tools/static-dedupe.mjs expand <dir>` (stellt alle Dateien bytegleich wieder her, geprüft
mit `diff -r` gegen den unzusammengefassten Build). Die Referenzumgebung nutzt eigene upstream-Statics
(`reference/data/static`) und ist nicht betroffen.

Größe (37 Sprachen, Build vom 2026-11-08): `frontend/dist` 522,4 MB / 19 097 Dateien → **88,1 MB** / 954
Dateien; ohne `.gz`/`.br` (Desktop-Paket `static\`) 317,3 MB / 7 298 Dateien → **23,2 MB** / 351 Dateien.
Rest der 88 MB: je Sprache `main.js.gz` + `main.js.br` (≈ 1,5 MB, Kompressat nicht deltafähig).

Tests: `statics.rs` `fnv_and_delta_format`, `deduplicated_storage_serves_like_real_files` (Alias-Header =
Original, Delta identisch/br/Range/HEAD/Cache, `.paperrust` unsichtbar, kaputte Basis → 500);
Browser `frontend/e2e/dedupe.sh` (DB `pl_f1`, Port 8320, ohne Django): Login, Dashboard, Dokument mit
PDF.js-Vorschau in en-US und de-DE, keine 4xx/5xx unter `/static/`, Bytes virtueller Dateien je
Kodierung gleich dem Original – mit `frontend/dist` und mit einer Kopie ohne `.gz`/`.br` (wie Desktop).

### Nicht bediente Pfade (seit 2026-11-14 immer; vorher nur mit `PAPERRUST_DJANGO_URL=off`)

* Antwort = Djangos 404-Seite (`DEBUG=False`, ohne 404.html), unter `/api/` DRF-JSON
  `{"detail":"Not found."}`, mit den Django-Middleware-Headern wie eine Django-404. Gilt für:
  Router-Fallback (nicht portierter Pfad bzw. Methode außerhalb `/api/`), Body-Gate (Form/Multipart
  auf JSON-Route außerhalb `/api/`; unter `/api/` seit 2026-11-20 DRFs 415), abgeschaltete Routen (`"off"`).
  Seit 2026-11-20 meldet kein Handler mehr „nicht unterstützt“ (`delegate_to_django`); der Mechanismus
  (Gate-Punkt 3) bleibt nur als Sicherheitsnetz.
  Body > 2,5 MiB auf gepufferter Route → Djangos 400-Seite.
* Vor jeder View: `%`-Pfade werden in Rust geroutet; CORS-Preflight antwortet wie `CorsMiddleware`
  (200, `Content-Length: 0`, CORS + Security-Header, Marker `PreflightOnly`); unerlaubter Host →
  Djangos 400-Seite. Keine `param_guard`-Schicht mehr.
* Startmeldung (INFO): „upstream URL patterns without a PaperRust route answer 404 …“ mit Anzahl,
  je Muster eine Debug-Zeile (`frontend::urlconf::unported`, Methoden-unabhängig; `/ws/status/` wird
  nativ bedient); Warnung, wenn `ui.index`/`static.files` abgeschaltet sind. Gesetzte entfernte
  Variablen (`PAPERRUST_DJANGO_URL`, `PAPERRUST_PROXY_*`, `PAPERRUST_QUEUE=celery`, `PAPERLESS_REDIS`
  …) → je ein Hinweis (`pl_core::config::retired_warnings`, über `log_alias_warnings`).

Geschichte – Stand 2026-11-08 mit `results/parity/routes-all.toml` + `ui.*`/`static.files` (195 Routen)
nicht verfügbar ohne Django (inzwischen großteils in Rust, aktueller Stand: docs/STANDALONE-GAPS.md):
`documents/delete/`, `rotate/`, `merge/`, `merge_as_versions/`,
`edit_pdf/`, `remove_password/`, `documents/<pk>/{email,metadata,suggestions,update_version}/`,
`documents/<pk>/versions/<id>/`, `documents/email/`, `storage_paths/test/`,
`profile/social_account_providers/`, `oauth/callback/` (Gmail/Outlook-OAuth), `auth/headless/…`
(allauth headless), `/admin/`, Social-Provider außer `openid_connect` (`/accounts/<provider>/login/…`).

Cache: Djangos `default`-Cache über `pl_core::cache` (`AppState::cache()`), Standard prozesslokal,
mit `PAPERRUST_CACHE=postgres` gemeinsam in der Tabelle `paperrust_cache` (Migration
`pl_core.0001_paperrust_cache` beim Start, parallel zu den übrigen idempotenten Migrationen).
Startmeldung `cache: process-local (LocMemCache semantics)` bzw. `cache: shared in PostgreSQL
(paperrust_cache)`. Warnung (`cli::local_cache_warning`), wenn ein Prozess mit `--no-worker` oder
`--worker-only` den lokalen Cache nutzt (dann teilen HTTP-Prozess und Worker weder Drosseln,
TOTP-Replay-Sperre noch Invalidierungen). Nie eine Redis-Verbindung.

### Sauberer Stopp (`src/shutdown.rs`)

Erstes von: Ctrl+C; Unix `SIGTERM`; Windows `CTRL_BREAK`/`CTRL_CLOSE`/`CTRL_LOGOFF`/`CTRL_SHUTDOWN`;
stdin-EOF bei `PAPERRUST_STOP_ON_STDIN_EOF=1` (eigener Thread, blockiert das Runtime-Ende nicht).
Dann: Server nimmt nichts mehr an, offene Verbindungen bis 10 s, Worker/Beat/Consumer bekommen
das Stoppsignal und werden abgewartet, Runtime-Ende mit 5 s Frist für hängende Blocking-Tasks.
Log: `shutting down reason=…`, `stopping worker…`, `stopped`.

### Tests

`src/frontend/{mod,statics,urlconf}.rs` (Template wie Django-Ausgabe inkl. Leerzeilen/Tabs,
Login-Weiterleitung, Sprachumsetzung, whitenoise-Typen/Ranges/ETag-Eigenheit/304/405/416,
URL-Konfiguration), `tests/frontend.rs` (Catch-all, Static, Redirects, 404
HTML/JSON, Preflight, 400), Parität `tests/parity/pl-server/cases.json` (A = Django :8000,
B = PaperRust :8084 ohne Django, `tests/parity/pl-server/w22env.sh`).

### Known differences (Frontend)

1. **Ladebildschirm-Texte übersetzt** („Paperless-ngx wird geladen…“): wie im Docker-Image mit
   kompilierten Katalogen (dieselbe Quelle wie pl-accounts). Die Referenz hat keine `.mo`
   (nur `.po`) und zeigt Englisch → Paritätsfälle `ui.index.lang-de/fr` weichen genau in diesen
   drei Texten ab (`tests/parity/pl-server/accepted.json`).
2. **URL-Konfiguration als Daten** (`urlconf::UPSTREAM`): `{}` passt auf jedes Segment (Django je
   Muster `\d+`, `\w+`, `[-:\w]+` …), allauth-Provider-URLs generisch. Exotische Pfade
   (z. B. `/share/a.b`) können in Django Index, hier „spezielle View“ (→ 404) sein.
3. **WhiteNoise ohne Autorefresh:** neue Dateien in `STATIC_ROOT` erst nach Neustart (upstream
   ebenso ohne DEBUG). `WHITENOISE_USE_FINDERS` (DEBUG) nicht nachgebaut.
4. **ETag-Zeitzone:** `mktime` in `PAPERLESS_TIME_ZONE` (Django setzt `TZ` unter Unix). Unter
   Windows nutzt Django die System-Zeitzone – auf dem Referenzrechner identisch.
5. **Nicht-GET auf Index-Pfade** (`POST /dashboard`): 404 statt Djangos 403 (CSRF)
   bzw. 405; Host-400 mit `X-Frame-Options`/`Vary: Cookie` (Django: ohne, Middleware-Abbruch).
6. **`{% static %}`:** `urljoin` vereinfacht (Basis + Pfad, keine `..`-Auflösung; nur bei
   manipulierten `ui_settings.language` relevant, Ausgabe bleibt escaped).
7. **`/static/.paperrust/…`** wird nie ausgeliefert (Ablage-Manifest, siehe „Zusammengefasste
   Static-Ablage“; ein upstream-STATIC_ROOT hat diesen Ordner nicht).
8. **Index (`ui.index`, `/` und jede alte SPA-Adresse) leitet immer um** (seit 2026-11-16,
   Testprotokoll D-100): `302` auf denselben Pfad unter `/rs/app/` (Query bleibt, `no-store`), auch
   anonym – die Light-UI erledigt die Anmeldung selbst. Die Angular-Index-Seite wird nicht mehr
   ausgeliefert (vorher ohne Light-UI-Build: Angular-Seite, die ohne Statics endlos „lädt“). Fehlt
   der Light-UI-Build oder ist `PAPERRUST_LIGHT_UI=off`, antwortet `/rs/app/…` mit `503` und einer
   kurzen Seite „Weboberfläche nicht installiert“ (ohne Pfade; das Verzeichnis steht als Warnung im
   Log). upstream: Login-Weiterleitung bzw. Angular-`index.html`.

## Startprüfungen (`src/dbcheck.rs`, seit 2026-11-16)

Vor Migrationen, Worker und HTTP-Server (nach `PAPERRUST_DB_REQUIRE_AUTH`, vor dem Einlesen der
Oberflächen-Einstellungen):

1. **Datenbank erreichbar** (D-087): Verbindungsversuche alle 0,5 s bis `PAPERLESS_DB_TIMEOUT`
   (Standard 10 s), sonst Abbruch mit „Datenbank nicht erreichbar / database not reachable:
   `host:port`, DB `name` (waited N s): <Fehler>“ – ohne Benutzer/Passwort. Server erreichbar, aber
   Anmeldung/DB abgelehnt: sofortiger Abbruch mit der Meldung des Servers. Vorher: „pool timed out
   while waiting for an open connection“ nach ~33 s ohne Adresse.
2. **Schema aktuell** (A-181/B-312): wie `manage migrate --check` (`pl_cli::migrate::startup_check`):
   leere DB, fremde DB, andere paperless-ngx-Version oder offene `paperrust_*`-Migration → Abbruch
   mit „Datenbank ist nicht auf dem neuesten Stand – zuerst `paperrust manage migrate` ausführen“.
   Vorher startete der Server still und lieferte später 500 (z. B. Tag anlegen: `tn_ancestors_pks`
   NOT NULL).

**Warum Abbruch statt automatisch migrieren:** Docker-Entrypoint (`paperrust-entrypoint`) und
Desktop-Supervisor rufen vor jedem Start ohnehin `paperrust manage migrate` auf (der
`--worker-only`-Dienst der Mehrprozess-Compose-Datei startet erst nach dem gesunden `web`-Dienst),
für sie ändert sich nichts. Die Umbaustufen des Datenmodells bauen Tabellen um und brauchen vorher
eine Sicherung (D-073) – das soll kein Serverstart nebenbei tun. upstream ohne Docker ebenso:
`manage.py migrate` von Hand. Die DB-Tests von pl-server bauen die App in-process (`build_app`)
und sind nicht betroffen; Testskripte, die `serve` auf einer frischen Snapshot-Kopie starten,
müssen vorher migrieren (`frontend-light/tests/server.sh` und `crates/pl-server/tests/e2e_w*/run_server.sh`
tun das jetzt).

## Befehlszeile: `--help`, `--version` (seit 2026-11-16, D-067)

`paperrust --help` / `-h` / `help` und `paperrust --version` / `-V` / `version` (jeweils als einziges
Argument) geben Hilfe bzw. `paperrust <version> (compatible with paperless-ngx 3.4.2)` aus
(`pl_core::brand`), Exit 0. Unbekannte Argumente: Meldung + Usage, Exit 2 (wie bisher).

## Worker im Prozess (`paperrust serve --worker`, Welle 3 W1, `src/cli.rs`, `src/worker.rs`)

Binaries `paperrust` (neu) und `paperless-rs` (alt), gleiches Programm:
`paperrust [serve] [--worker | --worker-only | --no-worker]` (Standard `--no-worker`) und
`paperrust manage <befehl> …` (W10: an `pl_cli::run_async` in einer eigenen tokio-Runtime, Exit-Code von
pl-cli; `mail_fetcher` ist mit `pl_cli::mail::set_mail_fetcher` an pl-mails
`fetch::process_mail_accounts(worker, None)` gebunden – direkt wie upstream `tasks.process_mail_accounts()`,
Anhänge gehen als `consume_file` in die PaperRust-Warteschlange). Ersetzt die upstream-Dienste `celery worker`, `celery beat`, `manage.py document_consumer`.

| Modus | HTTP | Worker + Beat + Eingangsordner | Statusmeldungen |
|---|---|---|---|
| `--no-worker` | ja | nein | PgQueue: `LISTEN paperrust_status` → Bus → WebSocket |
| `--worker` | ja | ja | In-Prozess-Bus (+ LISTEN für weitere Worker-Prozesse) |
| `--worker-only` | nein | ja | Bus → `pg_notify('paperrust_status')` |

- **Warteschlange** (`worker::queue_config` → `pl_queue::QueueConfig::from_env`): immer die PostgreSQL-
  Warteschlange (`PgQueue`, `paperrust_task_queue`). Seit 2026-11-14 gibt es weder Celery-Brücke noch
  Auswahl: `PAPERRUST_QUEUE`/`PAPERRUST_QUEUE_BACKEND` sind wirkungslos (Wert `celery`/`redis` →
  Hinweis beim Start, nie Abbruch). Startmeldung `task queue: PostgreSQL (paperrust_task_queue, …)`.
- **AppState** (pl-core `Services`, Typ als Schlüssel, weil pl-queue von pl-core abhängt):
  `state.service::<Arc<dyn pl_queue::TaskQueue>>()`, `state.service::<pl_queue::EventBus>()`,
  `pl_core::tasks::Dispatcher`; mit Feature `ai` `pl_ai::server::index::AiTasks(QueueAiSink)`: pl-ai reiht
  `index_document`, `llmindex_index`, `update_document_in_llm_index` … in die PgQueue ein (W10).
- **Task-Versand der Handler-Crates** (W10, vereinheitlicht W14): pl-objects (`sidefx::send_bulk_update_documents`),
  pl-mail (`celery::send_process_mail_accounts`), pl-fields-views (`build_share_link_bundle` bei create/rebuild) und
  pl-system (`tasks/run.rs`) senden über `pl_core::tasks::send_task`. `setup_queue` hängt immer
  `pl_core::tasks::Dispatcher(QueueDispatch(queue))` ein (`worker::enqueue_options`: `TaskRequest` →
  `enqueue(name, kwargs, EnqueueOptions{args, trigger_source, headers, countdown, expires, task_id, time_limit})`,
  PgQueue legt den PaperlessTask selbst an → `records_task() = true`). pl-documents-write (bulk_edit/reprocess) nutzt ihn ebenfalls
  (`prepare_task` → `record` nur bei `records_in_caller()` → `publish` nach dem Commit, W13); die PDF-Methoden
  (Chords/`link`) gehen direkt an `TaskQueue`.
- **Handler `documents.tasks.build_share_link_bundle(bundle_id)`** (W14): `pl_fields_views::bundle_task`
  (ZIP_DEFLATED in `SCRATCH_DIR`, Verschieben nach `SHARE_LINK_BUNDLE_DIR/<slug>.zip`, Status/`last_error` wie upstream).
- **Hooks beim Start** (`worker::install_hooks`, alle Modi, W10): `pl_documents_write::hooks::set_workflow_effects`
  mit `pl_mail::effects::SharedEffects(MailWorkflowEffects)` (E-Mail/Webhook/Passwort-Aktionen der
  „Document Updated“-Workflows aus pl-documents-write); `set_pdf_ops(NativePdfOps)` (W13: PDF-Methoden von
  bulk_edit mit pl-pdfops); mit Feature `ai`
  `set_document_updated_hook(AiDocumentUpdated)` = `add_or_update_document_in_llm_index` an `document_updated`
  (PUT/PATCH; `llm_index_enabled` → `update_document_in_llm_index {"document_id"}` in die PgQueue).
- **Routen von pl-cli** (`pl_cli::routes()` in `all_routes()`): `schema.get`, `schema.swagger_ui`,
  `static.swagger_ui_dist` (Schalter in `routes.toml` wie alle anderen).
- **Migration** `crates/pl-queue/migrations/0001_paperrust_task_queue.sql` wird bei jedem Start
  eingespielt (`pl_queue::migrations::apply`, idempotent, expand-only); Fehler → Startabbruch.
- **Migration KI** `crates/pl-ai/migrations/0001_paperrust_ai.sql` (`paperrust_taxonomy_alias`,
  `paperrust_ai_audit`, `paperrust_ai_review`; `pl_ai::server::migrations::apply`, idempotent, expand-only)
  beim Start, wenn Feature `ai` aktiv ist (`worker::apply_ai_migration`); Fehler → Startabbruch. Ohne Feature `ai` nie.
- **Handler** (`worker::TaskRegistry`, upstream-Namen):

| Aufgabe | Status | Baustein |
|---|---|---|
| `documents.tasks.consume_file` | portiert | `pl_consumer::task::handler` (Consumer mit `queue` für Barcode-Teile/verschobene Index-Updates) |
| `documents.tasks.index_document` / `remove_document_from_index` | portiert | `pl_indexer::Indexer::{index_document_task, remove_document_task}`; `LockExhausted` → `SearchIndexLockError` mit Autoretry (5×, Backoff 60, Jitter) |
| `documents.tasks.index_optimize` | portiert (No-op wie upstream) | `Indexer::optimize` |
| `documents.tasks.train_classifier` | portiert | Datenabfrage hier (ohne Posteingangs-Dokumente, AUTO-Labels, Inhalt in 1000er-Blöcken), Training pl-classifier in `spawn_blocking`, Modell `DATA_DIR/paperrust_classifier.model` (wie Consumer); Ergebnistexte wie upstream |
| `documents.tasks.bulk_update_documents` | portiert (W15/W13) | `Consumer::bulk_update_signals(ids)` (je Dokument `clear_document_caches`, `document_updated` → Workflows „Document Updated“, `post_save` → Dateiname), danach Index (`add_or_update_ids`) und mit Feature `ai` `update_llm_index(document_ids)` |
| `documents.tasks.update_document_content_maybe_archive_file(document_id, *, remote_ocr=False)` | portiert (W15) | `Consumer::update_document_content_maybe_archive_file`; `DoesNotExist`/DB-Fehler → FAILURE mit Python-Klassenname; nach `Updated` mit Feature `ai` und `llm_index_enabled` `update_document_in_llm_index` einreihen (upstream synchron im Task) |
| `documents.tasks.check_scheduled_workflows()` | portiert (W15) | `Consumer::check_scheduled_workflows`; Beat „Check and run scheduled workflows“ |
| `documents.tasks.sanity_check(*, raise_on_error=True)` | portiert (W13) | `pl_cli::sanity::check_sanity` + `log_messages` (Logger `paperless.sanity_checker`, Titel über `global_objects`, Stufe je Meldung); Ergebnis `No issues detected.` bzw. `N document(s) with errors, … found.` (+ ` Check logs for details.` bei Fehlern); Fehler und `raise_on_error` → FAILURE `SanityCheckFailedException`. Z2: über `pl_cli::sanity::run` mit `Options::for_task` (`PAPERRUST_INTEGRITY_*`, ungültige Werte → Warnung + Vollprüfung): optional inkrementell, JSON-Bericht unter `DATA_DIR/integrity/`, Abschlusszeile `paperless.integrity`; Ergebnistext/Status unverändert |
| `documents.tasks.empty_trash(doc_ids=None)` | portiert (W13) | `pl_documents_write::tasks::empty_trash` (abgelaufen nach `EMPTY_TRASH_DELAY`·24 h bzw. `doc_ids`, harte Löschung mit Kaskade/Dateien/LogEntries); Beat „Empty trash“ |
| `documents.bulk_edit.delete(doc_ids)`, `documents.bulk_edit.restore_archive_serial_numbers_task(backup, *args)` | portiert (W13) | `pl_documents_write::tasks::{bulk_delete, restore_asn_task}` (Chord-Body/`link` bzw. Errback der PDF-Methoden; Errback-Argumente `(request, exc, tb)` vorne → wie upstream `AttributeError`) |
| `paperless_mail.tasks.process_mail_accounts`, `paperless_mail.mail.apply_mail_action`, `paperless_mail.mail.error_callback`, `documents.workflows.webhooks.send_webhook` | portiert (pl-mail, W6) | `worker::register_mail_handlers` → `pl_mail::fetch::register_mail_handlers(handlers, MailWorker, WebhookSettings)`; Consumer-`effects` = `pl_mail::effects::SharedEffects` (E-Mail/Webhook/Passwort-Aktionen der Workflows) |
| `documents.tasks.llmindex_index` (kwarg `rebuild`, Standard false) | portiert (Feature `ai`, W10) | `pl_ai::server::index::llmindex_index`; Beat „Rebuild LLM index“ (`PAPERLESS_LLM_INDEX_TASK_CRON`, `10 2 * * *`, expires 23 h) wird damit geplant; `rebuild` kein bool → `TypeError` |
| `documents.tasks.apply_ai_suggestions(action_id, document_id)` | portiert (Feature `ai`, W10) | `pl_ai::server::workflow::apply_ai_suggestions`; `AiTaskError{autoretry}` (`LLMTimeoutError`) → `TaskError::autoretry` mit Regel aus `pl_queue::registry` (max 3, Backoff 60…600, Jitter) |
| `documents.tasks.update_document_in_llm_index` / `remove_document_from_llm_index` | portiert (Feature `ai`, W10) | `pl_ai::server::index::{update_document_in_llm_index, remove_document_from_llm_index}`; Argument **`document_id`** (kwarg oder `args[0]`) statt gepickeltem `Document` |
| `paperrust.ai.llm_index_compact` | PaperRust (Feature `ai`) | `pl_ai::server::index::llm_index_compact` (upstream nur `manage.py document_llmindex compact`); nicht im Beat |
| `paperrust.ai.process_trigger_tag` | PaperRust (Feature `ai`) | `pl_ai::server::review::process_trigger_tag`; Beat „PaperRust: process AI trigger tag“ nur mit `PAPERLESS_RS_AI_AUTO_TRIGGER_TAG` und `PAPERLESS_RS_AI_AUTO_MODE` ≠ `off`, Zeitplan `PAPERRUST_AI_TRIGGER_TAG_CRON` (Standard `*/5 * * * *`, `disable` = aus), expires 4 min, `trigger_source=scheduled` |
| `paperrust.ai.process_document(document_id, trigger="consumed")` | PaperRust (Feature `ai`) | `pl_ai::server::review::run(state, [id], auto_mode, trigger, None)`; ohne `ai_enabled` bzw. mit `auto_mode=off` → nichts |

Ohne Feature `ai` sind die vier upstream-KI-Aufgaben als „nicht portiert“ registriert.

- **Consumer-Bausteine** (W15, gesetzt in `register_document_handlers`): `consumer.caches =
  RedisDocumentCaches` (Name historisch; `cache().delete_many` der Schlüssel aus
  `pl_consumer::signals::document_cache_keys("", id)` im lokalen bzw. PostgreSQL-Cache, mit Feature `ai`
  zusätzlich `pl_ai::server::cache::invalidate`; Fehler → Warnung),
  `consumer.deleter = DocumentsWriteSoftDelete` (`pl_documents_write::softdelete::soft_delete_document` mit
  `MediaCfg::from_config`, volle Kaskade inkl. auditlog der verbundenen Objekte).
- **Signal-Empfänger außerhalb pl-consumer** (W10, Feature `ai`): `consume_file` wird umhüllt; nach Erfolg
  (`{"document_id": id}`) läuft `worker::ai_after_consumption` = upstream `add_or_update_document_in_llm_index`
  an `document_consumption_finished` (`AIConfig().llm_index_enabled` → `update_document_in_llm_index` mit
  `{"document_id": id}` einreihen) und danach – PaperRust-Erweiterung – bei `ai_enabled` und
  `PAPERLESS_RS_AI_AUTO_MODE` ≠ off `paperrust.ai.process_document` (`trigger=consumed`). Einreihfehler → Log,
  das Einlesen bleibt erfolgreich. `bulk_update_documents` ruft nach dem Suchindex wie upstream
  `update_llm_index(rebuild=False, document_ids=…)` (Fehler → Aufgabe FAILURE). `apply_ai_suggestions` reiht
  nach einer Änderung wie upstream `update_document_in_llm_index` ein (pl-ai). Ungültige KI-Konfiguration →
  Warnung, KI-Hooks übersprungen.

- **Beat**: `pl_queue::beat::parse_beat_schedule` (Fehler → Startabbruch wie upstream), Zustand
  `paperrust_beat_state`. Einträge mit **nicht portierter** Aufgabe werden nicht geplant (Warnung beim Start),
  damit nicht stündlich FAILURE-Einträge entstehen; manuell gestartet laufen sie in den klaren Fehler.
- **Eingangsordner**: `pl_consumer::watcher::FolderWatcher` mit `QueueSink` (`trigger_source=folder_consume`),
  Einstellungen `PAPERLESS_CONSUMER_*`; Ordner fehlt → Fehler im Log, Server läuft weiter.
- **Herunterfahren** (Strg+C): Stoppsignal an Worker/Beat/Watcher; Worker gibt laufenden Aufgaben
  `PAPERRUST_QUEUE_SHUTDOWN_GRACE`.
- ~~`worker::NEEDS_PG_QUEUE`/`adjust_plan`~~ (entfernt 2026-11-14): ohne Celery-Brücke gibt es keine Routen
  mehr, die nur mit PgQueue montiert werden.

## Upload `documents.post_document` (`src/upload.rs`)

`POST /api/documents/post_document/` – upstream `PostDocumentView.post` + `PostDocumentSerializer`
(`documents/views.py`, `documents/serialisers.py` v3.4.2). Markierungen: `accepts_drf_parsers` und **streaming**
(`upload::STREAMING_ROUTES`, `body::Gate::streaming`): der Body wird nicht gepuffert;
`DefaultBodyLimit` für diese Route aus.

1. `RequestContext` (406), Authentifizierung (`IsAuthenticated`, 401/403 wie DRF), dann
   `has_perm("documents.add_document")`, sonst `403 text/html` `Insufficient permissions`.
2. Leerer Body (`Content-Length: 0`) → leere Daten (DRF `_parse`, kein 415) → `{"document":["No file was submitted."]}`.
   Sonst Content-Type ≠ `multipart/form-data` → 415 `Unsupported media type "<Content-Type>" in request.`;
   ungültige Grenze → 400 `Multipart form parse error - Invalid boundary in multipart: None`.
3. Multipart gestreamt: Dateiteile (Name aus `filename`, Djangos `sanitize_file_name`: HTML-Entities, Basisname,
   nur druckbare Zeichen; leer/`.`/`..` → Teil ignoriert) direkt nach `SCRATCH_DIR/tmpXXXXXXXX/`
   (`tempfile.mkdtemp`); der letzte Dateiteil `document` zählt. Nicht-Datei-Felder in den Speicher, Summe
   (Name + 2 + Daten) > 2,5 MiB → Djangos 400-Seite (`RequestDataTooBig`); > 100 Dateien → 400-Seite
   (`TooManyFilesSent`).
4. Validierung in Feldreihenfolge (`created, document, title, correspondent, document_type, storage_path, tags,
   archive_serial_number, custom_fields, from_webui`), alle Fehler gesammelt, Texte wie DRF/Django
   (gegen die Referenz geprüft, Unit-Test `errors_like_reference`): DateTimeField (ISO, Datum in
   `PAPERLESS_TIME_ZONE`), FileField (`No file was submitted.`, `The submitted data was not a file. …`,
   `The submitted file is empty.`), MIME per pl-parsers (`File type … not supported`, PDF-Ausnahme für
   `application/octet-stream` + `.pdf`), CharField (`strip`, `This field may not be blank.`, Nullzeichen),
   PrimaryKeyRelatedField (`Incorrect type. Expected pk value, received str.`, `Invalid pk "…" - object does not exist.`,
   ohne Rechteprüfung wie upstream; Tags: erster Fehler), IntegerField 0…4294967295, JSONField
   (`Value must be valid JSON.`) + `validate_custom_fields` (Liste/Dict/Einzelwert, Wertprüfung je Typ wie
   `CustomFieldInstanceSerializer.validate`: url/integer/monetary/string/select/documentlink/date), BooleanField.
5. Erfolg: Datei → `mkdtemp/<pathvalidate.sanitize_filename(NFC(name))>`, mtime/atime = jetzt (Sekunden),
   `ConsumableDocument(WebUI|ApiUpload)`, Overrides (`filename`, `title`, Objekte, `tag_ids`, `created`, `asn`,
   `owner_id` = Nutzer, `custom_fields` Dict bzw. `{id: null}`), `consume_file` über `TaskQueue`
   (`trigger_source` `web_ui`/`api_upload`), Antwort `200` `"<task-id>"` (JSON-String). Fehler → Temp-Ordner gelöscht.
6. PaperRust-Felder (fehlen sie, gilt das ngx-Verhalten): `ai_ocr` (Boolean, KI-Texterkennung, docs/contract/pl-ai.md)
   und `folder` (Ordner-id, Ordneransicht Stufe 2): Syntax wie PrimaryKeyRelatedField in `validate`, danach
   `pl_api_v2::folders::check_upload_target` (sichtbarer Ordner mit Änderungsrecht, `change_document`) → sonst 400
   `{"folder": [...]}`; Override `folder_id`, der Consumer legt das Dokument in diesen Ordner. Einzelheiten und
   Known differences: docs/contract/pl-api-v2.md „Stufe 2“.

## WebSocket `/ws/status/` (`src/ws.rs`)

Immer nativ (bis 2026-11-14 nur mit PgQueue, sonst Proxy an Django/Channels). Vor dem Routing in `frontline` erkannt (Channels läuft
ohne Djangos HTTP-Middleware, kein Host-Check). Authentifizierung nur per Session-Cookie (`AuthMiddlewareStack`,
`pl_authz::Authz::session_user`); anonym → `403` ohne Body (wie granian). Nachrichten = `EventBus`-Ereignisse
als `json.dumps(event)` (`WsPayload::to_ws_text`), `status_update`/`document_updated` nur bei `visible_to`
(Superuser, Besitzer, `users_can_view`, Gruppe in `groups_can_view`), `documents_deleted` an alle; Heartbeat
`{"type": "heartbeat"}` alle 30 s; Nachrichten des Clients werden ignoriert.

### Known differences (Worker/Upload/WebSocket)

11. **Upload-Speicher:** Django liest die Datei für `validate_document` komplett in den Speicher; hier wird gestreamt
    und für die MIME-Erkennung werden nur die ersten 7 MiB gelesen (libmagic liest ebenso höchstens 7 MiB).
12. **Multipart-Parserfehler** außer fehlender Grenze: Text `Multipart form parse error - Invalid multipart body`
    statt Djangos genauer Meldung. `html.unescape` nur für gängige Entitäten, `str.isprintable` angenähert,
    `filename*`-Kodierung wie multer.
13. **`csrfmiddlewaretoken` als Formularfeld** (Session-Auth ohne `X-CSRFToken`-Header) prüft pl-authz noch nicht
    (siehe INTEGRATION-TODO); die Angular-UI sendet den Header.
14. **WebSocket-Gruppen** werden beim Verbindungsaufbau gelesen (upstream: je Ereignis `user.groups…aexists()`);
    eine Gruppenänderung wirkt ab der nächsten Verbindung.
14a. **Geräte-Token am Socket** (2026-11-10): ohne Sitzungs-Cookie nimmt `/ws/status/` zusätzlich
    `Authorization: Bearer prt_…` an (gleiche Prüfung wie die API, `AuthMethod::DeviceToken`; widerrufen →
    403). Grund: Desktop-Modus 2b vermittelt den Socket aus dem Client-Kern (`pl-sync-client::wsrelay`), der
    keine Sitzung hat. Browser können am WebSocket kein `Authorization` setzen; `Token`/`Basic` bleiben wie
    upstream abgelehnt.
14b. **Origin-Prüfung am Socket** (Sicherheitsprüfung 2026-11-19): ist der Socket per Sitzungs-Cookie
    angemeldet und schickt der Browser einen `Origin`, muss er zu `<scheme>://<Host>` oder zu
    `CSRF_TRUSTED_ORIGINS` (auch aus `PAPERLESS_URL`, `*`-Muster) passen – gleiche Regeln wie Djangos
    `CsrfViewMiddleware` (pl-authz `csrf`). Sonst `403` ohne Body. Ohne `Origin` (Nicht-Browser) und mit
    Geräte-Token unverändert. upstream (`AuthMiddlewareStack` ohne `AllowedHostsOriginValidator`) nimmt
    jeden Origin an: eine fremde Webseite konnte mit dem Cookie des Nutzers den Status-Socket öffnen
    und Dateinamen/Aufgaben mitlesen (Cross-Site WebSocket Hijacking). Test: `ws::tests::origin_check`.
15. **Custom-Field-Werte** mit Nicht-ASCII-Ziffern (`integer`), Python-`Decimal`-Sonderformen und
    `NaN`/`Infinity` im JSON (Python `json.loads` erlaubt sie) weichen ab; Werttypen, an denen upstream mit
    `TypeError` (500) scheitert, ergeben hier ebenfalls 500.
16. ~~`bulk_update_documents` teilweise~~ – vollständig (W15-Signale, W13-Einbau).
17. **Beat** plant nicht portierte Aufgaben nicht ein (upstream: alle Einträge).
18. **KI-Index-Aufgaben** `update_document_in_llm_index`/`remove_document_from_llm_index` bekommen
    `document_id` statt eines gepickelten `Document` (aus Rust nicht sendbar). `remove_document_from_llm_index` wird im Worker noch von
    niemandem ausgelöst (upstream `post_delete` bei endgültigem Löschen; `empty_trash` löscht ohne diesen Empfänger).
19. ~~`document_updated` ohne LLM-Index~~ – über `hooks::DocumentUpdatedHook` (W13).
20. **Workflow-Aktion `APPLY_AI_SUGGESTIONS`** bei „Document Added“: pl-consumer reiht
    `apply_ai_suggestions` noch nicht ein (Warnung im Log; CORE-REQUEST W10) – der Task-Handler selbst ist portiert.
21. **PaperRust-Aufgaben `paperrust.ai.*`** und der Beat-Eintrag für den Auslöser-Tag sind Erweiterungen
    (kein upstream-Gegenstück); `llmindex_index` bleibt unter dem upstream-Namen.
22. **Kein Django, kein Celery, kein Redis** (2026-11-14): Proxy, Celery-Brücke, `param_guard`,
    `NEEDS_PG_QUEUE` und die Redis-Anbindung sind entfernt. Was ein Handler nicht unterstützt und was keine
    Route hat, antwortet 404; Form/Multipart auf JSON-Routen 404; Bodies > 2,5 MiB auf gepufferten Routen
    Djangos 400-Seite. Zugriffslog ohne `upstream_ms`, `via` nur `rust`/`internal`. Mehrprozessbetrieb
    braucht `PAPERRUST_CACHE=postgres` (Warnung sonst).
23. **Atomic AI writes** (2026-11-27, pl-ai Known differences 24): `worker::ConsumerDocWriter` reports
    `supports_transaction() = true`. Its `*_tx` methods write on the caller's connection and never
    begin/commit: `save_tx`/`add_nested_tags_tx`/`remove_tags_tx` via `pl_consumer::docops` (auditlog
    `LogEntry` as before), `get_or_create_tx` via `pl_ai::server::workflow::sql_get_or_create`, custom
    fields via pl-documents-write `cfi::update_or_create_deferred` / `cfi::soft_delete_deferred` (same
    auditlog entries as `cfi::update_or_create` / `cfi::soft_delete(then_full_save=false)`, **without**
    `after_custom_field_saved`). `update_filename_and_move_files` (pl-consumer `files`) runs only in
    `after_commit`, in its own transaction after the caller committed – so a rollback (e.g. the AI audit
    insert fails) never leaves files moved while the row keeps the old name. Difference to the pool path:
    after a custom-field-only change the file name check always runs (upstream only if the template uses
    `custom_fields`; same result, one extra template render). The pool methods (`save`,
    `set_custom_field`, …) are unchanged. See CORE-REQUEST 6 (done).
    Tests: `tests/ai_writer_tx.rs` (`consumer_writer_is_transactional`; DB `tx_writes_roll_back_completely`
    `#[ignore]`, needs `PAPERRUST_CONSUMER_TEST_DATABASE_URL` = copy `pl_consumer_test*`).

## Parität Proxy vs. Django (Geschichte: manuell, 2026-11-08, Referenz granian :8000, paperless-rs :8080 ohne Rust-Route; Proxy seit 2026-11-14 entfernt)

Skript: gleiche Requests direkt und über den Proxy, gleicher `Host: 127.0.0.1:8000`, Vergleich
aller Antwort-Header (ohne `Date`, **inkl. Reihenfolge und Schreibweise**) und Body byte-genau.

| Request | Status | Header | Body |
|---|---|---|---|
| `GET /accounts/login/` | 200/200 | gleich bis auf neues CSRF-Cookie + `Expires` (pro Request zufällig/zeitabhängig, auch direkt) | gleich bis auf `csrfmiddlewaretoken` (zufällig) |
| `GET /accounts/login` (APPEND_SLASH) | 302/302 | gleich | gleich |
| `GET /api/` (Token) | 302/302 (→ `schema/view/`) | gleich | gleich |
| `GET /api/documents/` anonym | 401/401 | gleich | gleich |
| `GET /api/documents/?page_size=3&ordering=-id&title__icontains=%C3%A4` (Token) | 200/200 | gleich | gleich |
| `GET /api/documents/?page_size=25` + `Accept-Encoding: gzip, br, zstd` | 200/200, `content-encoding: zstd` | gleich | gleich (komprimierte Bytes identisch) |
| `GET /api/documents/3/download/` und `?original=true` | 200/200 | gleich | gleich |
| `GET /api/documents/3/thumb/` | 200/200 | gleich | gleich |
| `GET /share/refshare00…` | 200/200 | gleich | gleich |
| WebSocket `/ws/status/` anonym | 403/403 | gleich | – |
| Login über den Proxy (GET Formular → POST mit CSRF, `Origin: http://127.0.0.1:8080`) | 302 → `/dashboard`, Session-Cookie gesetzt; danach `/api/ui_settings/` mit Session 200; WebSocket `/ws/status/` mit Session → **101** | | |

Middleware-Abgleich (Rust-Nachbau vs. Django direkt, `/api/tags/`): `vary: Accept, Accept-Language, origin, Cookie`
(+ `Accept-Encoding` bei ≥ 500 Byte), `content-language: de-de` für `de-CH,de;q=0.9`, `fr-fr` für Cookie
`django_language=fr`, CORS-Header für `Origin: http://127.0.0.1:8000` (PAPERLESS_URL), keine für fremde Origin,
`x-frame-options`, `x-content-type-options`, `referrer-policy`, `cross-origin-opener-policy` – stimmt mit
`django_mw.rs` überein (der Rust-Teil selbst ist mangels aktiver Route nur per Integrationstest geprüft).

Nicht geprüft: großer Upload über den Proxy gegen echtes Django (Celery-Worker läuft in der Referenz nicht;
Streaming ist per Integrationstest mit 8 MiB belegt), SSE `/api/documents/chat/` gegen echtes Django (KI aus).

### E2E Worker (W13)

`crates/pl-server/tests/worker_w13.rs` (`#[ignore]`, `source tests/parity/pl-documents-write/w13/w13env.sh`):
Registry enthält `sanity_check`, `check_scheduled_workflows`, `empty_trash`,
`update_document_content_maybe_archive_file`, `bulk_edit.delete`, `restore_archive_serial_numbers_task`;
echter `pl_queue::Worker` auf PgQueue gegen `pl_w13_ref` führt `sanity_check(raise_on_error=False)`,
`check_scheduled_workflows` und `empty_trash(doc_ids=[196])` aus → SUCCESS, PaperlessTask `success`,
Dokument 196 hart gelöscht.

## Start und Leistung (O1, 2026-11-08)

Messung: `tests/bench/bench.py` (A/B verschränkt, Median aus 3 Runden), Rohdaten
`results/bench/2026-11-08-o1*/`. Antworten unverändert (Paritätslauf siehe Bericht O1).

* **Startzeit** – Ursache der 7–15 s: `StaticFiles::scan` (WhiteNoise-Nachbau) rief für jede der
  ~20 000 Dateien unter `STATIC_ROOT` `std::fs::metadata(pfad)` auf; unter Windows ist das je Datei
  ein `CreateFile` (+ Virenscanner) – 6–9 s, und es lief **vor** dem `bind`.
  * `walk` nimmt die Metadaten des Verzeichniseintrags (`DirEntry::metadata`, unter Windows aus
    der Verzeichnisliste, kein Öffnen); nur Links/Junctions werden wie `os.stat` verfolgt
    (gleiche Größe/mtime → gleiche ETags/Last-Modified).
  * Der Scan läuft im Hintergrund (`statics::StaticScan`, `spawn_blocking`); der Server lauscht
    sofort, nur `/static/…`-Anfragen warten beim ersten Mal auf die Liste. Ohne Tokio-Laufzeit
    (Tests) synchron wie bisher; ein vorab gesetztes `Arc<StaticFiles>` hat Vorrang.
  * Queue-, KI- und (bei `PAPERRUST_CACHE=postgres`) Cache-Migration (idempotent, getrennte Tabellen)
    laufen gleichzeitig (`tokio::join!`).
  * Nach dem `bind` öffnet `warm_pool` im Hintergrund **eine** Pool-Verbindung und führt darauf
    einmal die Nutzerabfrage aus (`pl_authz::user::warm_up`, Schlüssel, die es nicht gibt:
    liest/schreibt nichts) – ein neues PostgreSQL-Backend kostet unter Windows zig ms. Mit 4
    vorgewärmten Verbindungen wurden die Folgeanfragen langsamer (der Pool reicht freie
    Verbindungen reihum aus, jede war für die übrigen Tabellen noch kalt) – daher nur eine.
* **Allocator**: seit 2026-11-13 ist `mimalloc` Standard (Feature `mimalloc` in `default`,
  Workspace-Dep `mimalloc =0.1.52`, `mimalloc::MiMalloc`): +4–8 % Durchsatz, dafür Private Bytes
  19 → 43 MB im Leerlauf und 30 → 87 MB unter Last (Working Set ±1–3 MB). Entscheidung des
  Projektinhabers: Tempo vor den ~25–60 MB. Für sehr knappe Geräte mit System-Allokator bauen:
  `cargo build --release -p pl-server --no-default-features --features ai`.
* **Release-Profil** geprüft, unverändert: `lto = "fat"`, `codegen-units = 1`,
  `strip = "symbols"` (keine Debuginfo im Binary). `panic = "unwind"` bleibt Pflicht: ein Panic
  in einem Handler darf nur die Anfrage beenden (Tokio fängt ihn), nicht den Server. Die 67 MB
  sind Code (tantivy, rxing, image, lopdf, pl-ai, rustls/ring) plus ~3 MB eingebettete Daten.
* **Tokio**: Standard (Worker = logische Kerne, 2 MiB Stack, nur bei Bedarf belegt). Mit
  `TOKIO_WORKER_THREADS=4` gleicher Durchsatz und ~4 MB weniger Spitzen-Speicher – kein
  Grund für eine feste Zahl; über die Umgebungsvariable einstellbar (Tokio liest sie selbst).

Messwerte O1 (Release, `pl_bench`, 8 Clients, Median 3 Runden; Laptop parallel belastet, daher
nur relativ aussagekräftig – `results/bench/2026-11-08-o1-final/`):

| Endpunkt (rps · p50 ms) | vorher (a83174d) | O1 | O1 + mimalloc |
|---|---|---|---|
| Dokumentliste (25) | 447 · 17,5 | 514 · 15,2 | 530 · 14,5 |
| Dokument öffnen | 798 · 9,9 | 936 · 8,4 | 925 · 8,4 |
| Alle Tags | 842 · 9,4 | 995 · 7,8 | 1078 · 7,3 |
| Alle Korrespondenten | 1325 · 6,0 | 1585 · 4,9 | 1717 · 4,6 |
| Statistik | 1351 · 5,8 | 1400 · 5,6 | 1405 · 5,6 |
| Gespeicherte Ansichten | 1342 · 5,9 | 1698 · 4,6 | 1720 · 4,6 |
| UI-Einstellungen | 2243 · 3,4 | 2397 · 3,2 | 2422 · 3,2 |

## Upstream-Fixes nach v3.4.2 (paperless-ngx `dev`)

- **a139bdff4 (#14722):** Worker registriert `documents.tasks.clear_expired_sessions`
  (`worker::names::CLEAR_EXPIRED_SESSIONS` → `pl_accounts::session::clear_expired_sessions`; Fehler →
  `OperationalError`). Beat-Eintrag in pl-queue.
- **00096e025 (#14735):** `config::django_settings` bildet `allowed_hosts` über
  `pl_core::config::initial_allowed_hosts` / `allowed_host_from_url_hostname` (Regeln im Vertrag von
  pl-core), Fehlertext „has no valid hostname“. Test: `config::tests::hosts_like_upstream`.

## Last-Befunde B2/B8/B10 (2026-11-18)

Quelle: tests/stress/BEFUNDE.md.

- **B10 – `src/detach.rs`:** Middleware direkt um den Router: jede Anfrage mit nicht-sicherer Methode
  (POST/PUT/PATCH/DELETE) läuft als eigene Tokio-Aufgabe; schließt der Client die Verbindung, wird nur
  das Warten abgebrochen, der Handler arbeitet bis zur Antwort weiter (Workflows, Suchindex, WebSocket,
  Einreihen nach dem Commit). GET/HEAD/OPTIONS bleiben abbrechbar. Eine Panik im Handler wird zu
  `500 Server Error` (Django-Seite) statt Verbindungsabbruch. Das Zugriffs-Log meldet weiter
  `status=499 outcome="client closed"` für den Client.
- **B2:** Sync-Sequenzer auf eigenem Nebenpool (2), Long-Poll der API v2 (`SyncFeed::wait`) auf
  eigenem Nebenpool (`PAPERRUST_DB_LONGPOLL_CONNECTIONS`, Standard 8), Worker-Steuerung (Claim,
  Heartbeat/Lease, Abschluss) auf eigenem Nebenpool (`TASK_WORKERS + 3`).
  Der Einstellungs-Listener von pl-settings (`reload.rs`) lauscht seit der Gesamtprüfung ebenfalls
  über `pl_core::dbconn::listener` (eigene Verbindung); der Long-Poll prüft über die Verbindung des
  Listeners selbst (`pl_sync::wait_for_changes`), nicht über eine zweite aus dem Nebenpool.
- **B8:** `bulk_update_documents` schreibt den Index über `Indexer::add_or_update_ids_task`
  (mit Wiederholung); vorübergehende Index-Fehler werden zu `retry` (OSError, 60 s) statt endgültig.

## Speicher-Befund 2026-11-18 (Jetson-Benchmark)

- `search_sync`: prüft die Migration `pl_indexer.0001_paperrust_search_dirty` nur noch bis zum ersten
  Erfolg (vorher jede Sekunde `to_regclass`), nach einem Fehler von `drain_once` wieder. Halbiert die
  Leerlauf-Abfragen an PostgreSQL (eine statt zwei je Sekunde); Abgleich-Verzögerung unverändert 1 s.
  Läuft über einen eigenen Ein-Verbindungs-Pool (`dbconn::side_pool(…, 1)`): über den Hauptpool (FIFO)
  hielt die Sekunden-Abfrage reihum jede nach dem Einlesen offene Verbindung am Leben.
- Pool-Lebensdauer und Leerlauf-Frist der Verbindungen: `docs/contract/pl-core.md` „Speicher-Befund
  2026-11-18“; Tesseract-Freigabe im Leerlauf: `docs/contract/pl-ocr.md` §8.

## Grundspeicher nach dem ersten Einlesen (2026-11-18)

Frage: Was hält der Prozess nach dem ersten Einlesen fest (Jetson: 103 MiB frisch, ~170–200 MiB danach)?
Messung (a) Rust-Heap je Baustein mit zählendem Allokator (Hilfsprogramm, Debug-Build, Windows; lebende
Bytes nach dem Aufruf), (b) ganzer Prozess (Debug-Build, Windows, `serve --worker`, Ablauf: frisch →
Text-PDF + Textdatei mit Datumsangaben einlesen → 40 s Leerlauf → Dokument, Metadaten, Vorschläge, Suche,
Vorschau → 40 s Leerlauf).

| Baustein (Rust-Heap, bleibt bis Prozessende) | vorher | nachher |
|---|---|---|
| Datumserkennung: eingebettete dateparser-Daten (alle 228 Datensätze, roh) | 3,2 MiB | 3,2 MiB |
| Datumserkennung: erste Sprache (en) übersetzen, **davon Zeitzonen-Vorfilter** | 58,5 MiB (**46,7**) | 13,3 MiB (**1,3**) |
| Datumserkennung: jede weitere Sprache (de) | 6,5 MiB | 6,7 MiB |
| Datumserkennung: Suchpuffer bei 4 gleichzeitig parsenden Threads | 29,8 MiB | 14,6 MiB |
| Spracherkennung `langdetect` (erste Metadaten-Ansicht): lebend / belegt (mimalloc) | 14,2 / 21,5 MiB | 5,9 / 13,2 MiB |
| Metadaten-Leser (PDF, Bild) | 0 | 0 |

| Prozess (Debug, Windows): Working Set / Private Bytes | vorher | nachher |
|---|---|---|
| frisch (20 s) | 58 / 97 MiB | 57 / 93 MiB |
| 40 s nach dem Einlesen | 156 / 230 MiB | 104 / 141 MiB |
| nach Ansicht, Metadaten, Vorschlägen, Suche, Vorschau + 40 s | 173 / 248 MiB | 121 / 155 MiB |

Geändert:

- **Zeitzonen-Vorfilter** (pl-matching `dp_tz`): ein `RegexSet` aus 773 vollen Mustern mit Unicode-`\W`/`\d`
  wurde durch einen Vorfilter nur aus den Namen ersetzt (Kandidaten danach mit dem vollen Muster, gleiches
  Ergebnis; Vertrag pl-matching). Größter Einzelposten. Nebenbei schneller: erste Übersetzung 4,7 → 0,9 s,
  400 Datumsangaben 20,8 → 1,8 s (beides Debug-Build).
- **Spracherkennung** (pl-documents `langdetect`): Tabelle mit gepackten 3-Zeichen-Schlüsseln und einem
  gemeinsamen Wertevektor statt `HashMap<String, Vec<…>>`; Aufbau ohne Allokation je n-gram. Gleiche
  Werte und Reihenfolge (Tests `profiles_load`, `detects_common_languages`, `deterministic`).
- **Freien Heap zurückgeben** (`memtrim`, ab `cli::run`): ein schlafender Thread ruft alle 30 s
  `mi_collect(true)` (fällige Rückgaben von mimalloc, auch für beendete `spawn_blocking`-Threads) und unter
  Linux/glibc `malloc_trim(0)` (PDFium/Tesseract). Unter Windows −5…9 MiB Private Bytes im Leerlauf
  (gemessen mit/ohne); unter Linux nicht gemessen. Keine Einstellung (nichts abzuwägen). Der in pl-ocr
  vorgesehene `set_release_hook` wird weiterhin nicht gesetzt – der Thread deckt ihn mit ab.

Bewusst behalten (Geschwindigkeit vor Speicher): übersetzte Sprachdaten je benutzter Sprache (sonst je
Dokument neu übersetzen, ~0,1–0,5 s), das zwischengespeicherte Klassifikator-Modell der Vorschläge, der
Suchindex-Leser, geladene PDFium-Bibliothek. Nicht gemessen: Release-Build, Linux/Jetson (PSS), Tesseract
(hier nicht installiert; wird nach `PAPERRUST_OCR_ENGINE_IDLE_SECS` freigegeben). Die Jetson-Zahl
170–200 MiB ist mit einem Lauf von `tools/bench-arm` auf dem neuen Stand nachzuprüfen.

## Ausfall-Befund C1 (tests/chaos, 2026-11-18)

`dbcheck::unavailable_middleware` (äußerste Schicht): solange `pl_core::dbconn::is_down()`, beantwortet
der Server `/api/…` und `/rs/api/…` sofort mit 503 + JSON-Meldung, Seiten (z. B. `/rs/app/`,
`/accounts/login/`) mit einer kurzen zweisprachigen 503-Seite, die sich alle 5 s neu lädt; statische
Dateien bleiben erreichbar. `cli.rs` startet `pl_core::dbconn::watch_health`. Vorher: 30 s Warten, dann 500.
Lesende API-Anfragen (GET/HEAD), die schon liefen, als die DB wegbrach, enden mit derselben 503, sobald
die DB als weg gilt und sie länger als 3 s (`RETRY_LIMIT`) laufen (Gesamtprüfung 2026-11-18: vorher
wartete so eine Anfrage bis zur Rückkehr der DB, gemessen 17,9 s); schreibende laufen weiter.
