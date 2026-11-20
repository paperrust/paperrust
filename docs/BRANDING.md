# Name und Kennzeichen: PaperRust

Festgelegt am 2026-11-07. Arbeitsname vorher: paperless-rs (verworfen: auf crates.io von einem
API-Client belegt; "paperless-ngx-rs" verworfen: suggeriert Zugehörigkeit zu upstream).

| Wo | Schreibweise |
|---|---|
| Texte, UI, Doku | **PaperRust** |
| Repo, Binary, Docker-Image, Crate, GitHub-Organisation | `paperrust` |
| Server-Binary | `paperrust` |
| Umzugswerkzeug | `paperrust-migrate-db` |
| Eigene Umgebungsvariablen | `PAPERRUST_*` (z. B. `PAPERRUST_BIND`, `PAPERRUST_CACHE`, `PAPERRUST_AI_*`) |
| Upstream-kompatible Variablen | bleiben `PAPERLESS_*` (Drop-in-Kompatibilität!) |
| Eigene DB-Tabellen | Präfix `paperrust_` (nur expand, nie upstream-Tabellen ändern) |
| Eigene API-Erweiterungen | `/api/x-ai/…`, `/mcp` (MCP-Servername `paperrust`) |
| Unterzeile | "PaperRust – schneller, schlanker, kompatibler Ersatz für paperless-ngx" |

Hinweise:
- Kein "ngx", kein Logo/Branding von paperless-ngx verwenden. Upstream klar nennen und würdigen
  ("based on paperless-ngx, GPL-3.0").
- "Rust" ist eine Marke der Rust Foundation: vor Veröffentlichung deren Markenrichtlinie prüfen,
  keine offizielle Billigung suggerieren.
- Vor Veröffentlichung: GitHub-Organisation `paperrust`, Domain(s) und crates.io-Name sichern (Nutzer).

## Umbenennung im Code (umgesetzt 2026-11-09, R1)

Zentral in `pl_core::brand` (`PRODUCT_NAME`, `BIN_NAME`, `PAPERRUST_VERSION`, `UPSTREAM_VERSION`, `REPO_URL`
`https://github.com/paperrust/paperrust`, `RELEASES_API_URL`, `USER_AGENT`, `ATTRIBUTION`) und
`pl_core::config` (Namensregeln, Tabelle `OWN_SETTINGS`).

| Alt | Neu | Hinweis |
|---|---|---|
| Binary `paperless-rs` | `paperrust` | altes Binary entfernt (`crates/pl-server/src/main.rs` gelöscht) |
| Binary `paperless-rs-migrate-db` | `paperrust-migrate-db` | |
| `PAPERLESS_RS_<X>` | `PAPERRUST_<X>` | alter Name gilt weiter als Alias, beim Start eine Warnung je Name |
| `PAPERLESS_MIGRATE_SOURCE/TARGET` | `PAPERRUST_MIGRATE_SOURCE/TARGET` | alter Name gilt weiter |
| Log-Target `paperless_rs::access` | `paperrust::access` | Filter z. B. `PAPERRUST_LOG="info,paperrust::access=info"` |
| DB-`application_name` `paperless-rs` | `paperrust` | überschreibbar per `PAPERLESS_DB_OPTIONS` |
| MCP `serverInfo.name` `paperless-rs`, realm `paperless-rs-mcp` | `paperrust`, `paperrust-mcp` | |
| Fehlertext „Not implemented in paperless-rs“ | „Not implemented in PaperRust“ | |
| User-Agent (keiner) | `PaperRust/<version> (+https://github.com/paperrust/paperrust)` | Webhook, Mail-OAuth, OIDC, LLM/Embedding; Versionsprüfung: R4 (pl-system) |
| Health `/__rs/health` `{status, rust_routes}` | zusätzlich `service: "paperrust"`, `version` | |
| Startmeldung „paperless-rs listening“ | „PaperRust listening“ + Banner mit Version und Würdigung | |
| Konto-Seiten/Mails „Paperless-ngx sign in“, „[Paperless-ngx] …“ | „PaperRust sign in“, „[PaperRust] …“ | nur bei `PAPERRUST_BRANDING=on`; bei `off` upstream-Texte (Parität) |
| Tabellen `plrs_*` | `paperrust_*` | schon vorher umgestellt; geprüft: in keiner der 30 lokalen DBs gibt es `plrs_*`, nie veröffentlicht → keine Umbenennungs-Migration |

### Umgebungsvariablen: Namensregeln

1. **Primärname** jeder Einstellung ist `PAPERRUST_<NAME>`.
2. **Kompatibilität mit paperless-ngx:** jede upstream-Variable `PAPERLESS_<NAME>` gilt unverändert (gleiche
   Semantik). Sind beide gesetzt, gewinnt `PAPERRUST_<NAME>` (Warnung, wenn die Werte verschieden sind).
3. **Übergang:** `PAPERLESS_RS_<NAME>` → `PAPERRUST_<NAME>` (Alias, je Name einmal eine Warnung im Log).
4. Umsetzung: `Env::get` (alles über `pl_core::Config`, auch `paperrust.conf`/`paperless.conf`),
   `pl_core::config::var` (Prozess-Umgebung) und `apply_env_aliases()` als erste Anweisung von `paperrust`:
   spiegelt `PAPERLESS_RS_X` → `PAPERRUST_X` und `PAPERRUST_X` → `PAPERLESS_X` (außer eigenen Einstellungen),
   damit auch Crates mit direktem `std::env::var` und Django-Kindprozesse dieselben Werte sehen.
5. Konfigurationsdatei: `PAPERRUST_CONFIGURATION_PATH` bzw. `PAPERLESS_CONFIGURATION_PATH`, sonst die erste
   vorhandene von `../paperrust.conf`, `../paperless.conf`, `/etc/paperrust.conf`, `/etc/paperless.conf`,
   `/usr/local/etc/paperrust.conf`, `/usr/local/etc/paperless.conf`.
6. (Geschichte) Im früheren Mischbetrieb las Django nur `PAPERLESS_*` aus **seiner** Umgebung. Seit 2026-11-14
   gibt es keinen Django-Prozess mehr neben PaperRust.

### Eigene Einstellungen (Quelle: `pl_core::config::OWN_SETTINGS`)

| Name | Alias (alt) | Bedeutung |
|---|---|---|
| `PAPERRUST_BIND` | `PAPERLESS_RS_BIND` | Adresse:Port des Servers (Standard 127.0.0.1:8080) |
| `PAPERRUST_DJANGO_URL` | `PAPERLESS_RS_DJANGO_URL` | **entfernt (2026-11-14), wirkungslos** – früher Django-Backend für nicht portierte Pfade; gesetzt → Warnung beim Start |
| `PAPERRUST_ROUTES` | `PAPERLESS_RS_ROUTES` | Pfad der routes.toml (Standard ./routes.toml, optional; schaltet Routen nur ab: `"off"`) |
| `PAPERRUST_LOG` | `PAPERLESS_RS_LOG` | Log-Filter (tracing EnvFilter, Standard `info`) |
| `PAPERRUST_BASE_DIR` | `PAPERLESS_RS_BASE_DIR` | Installationswurzel = Django BASE_DIR.parent |
| `PAPERRUST_DB_MAX_CONNECTIONS` | `PAPERLESS_RS_DB_MAX_CONNECTIONS` | Poolgröße, wenn `PAPERLESS_DB_POOLSIZE` fehlt (10) |
| `PAPERRUST_DB_REQUIRE_AUTH` | – | erlaubte DB-Anmeldeverfahren (`scram-sha-256`, Liste wie libpq `require_auth`); Probe beim Start ohne Passwort, nur ohne TLS-Pflicht (sqlx 0.8 kennt die Option nicht, `PGREQUIREAUTH` wirkt nicht) |
| `PAPERRUST_INSTANCE_ID` | – | Kennung der Instanz, Feld `instance` in `/__rs/health` |
| `PAPERRUST_PROXY_CONNECT_TIMEOUT_SECS`, `PAPERRUST_PROXY_RESPONSE_TIMEOUT_SECS`, `PAPERRUST_PROXY_X_FORWARDED_FOR` | `PAPERLESS_RS_…` | **entfernt (2026-11-14)** mit dem Reverse-Proxy; gesetzt → Warnung beim Start |
| `PAPERRUST_STOP_ON_STDIN_EOF` | – | Beenden bei stdin-EOF (Desktop-Supervisor) |
| `PAPERRUST_BRANDING`, `PAPERRUST_ACCENT` | – | Branding an/aus, Akzentfarbe |
| `PAPERRUST_CACHE` | – | Cache: `local` (Standard, prozesslokal) oder `postgres` (gemeinsam in `paperrust_cache`, Mehrprozessbetrieb) |
| `PAPERRUST_REDIS_CONNECT_TIMEOUT`, `PAPERRUST_REDIS_RETRY_AFTER` | – | **entfernt (2026-11-14)**, kein Redis mehr |
| `PAPERRUST_QUEUE_*`, `PAPERRUST_WORKER_NAME` | – | eigene Warteschlange/Worker (`PAPERRUST_QUEUE` = `PAPERRUST_QUEUE_BACKEND` ist wirkungslos: einzige Warteschlange PostgreSQL) |
| `PAPERRUST_INDEX_WRITER`, `PAPERRUST_CLASSIFIER_MODEL_FILE`, `PAPERRUST_IMPORT_MAX_UNZIPPED_BYTES` | – | Index, Klassifikator, Import-Grenze |
| `PAPERRUST_TESSERACT_PATH`, `PAPERRUST_TESSDATA_DIR`, `PAPERRUST_PDFIUM_PATH` | – | OCR-/PDF-Bausteine |
| `PAPERRUST_REMOTE_VERSION_URL` | – | Quelle der Versionsprüfung (pl-system) |
| `PAPERRUST_AI_API_BASE`, `PAPERRUST_AI_MCP_*`, `PAPERRUST_AI_MATCH_THRESHOLD`, `PAPERRUST_AI_AUTO_*` | `PAPERLESS_RS_AI_…` | KI-Erweiterungen (MCP, Abgleich, Automatik) |
| `PAPERRUST_AI_AUTO_CREATE_MISSING`, `PAPERRUST_AI_RUN_MAX_DOCUMENTS_PER_HOUR`, `PAPERRUST_AI_TRIGGER_TAG_CRON` | – | KI-Läufe |
| `PAPERRUST_INTEGRITY_INCREMENTAL`, `PAPERRUST_INTEGRITY_FULL_HASH_DAYS`, `PAPERRUST_INTEGRITY_THREADS`, `PAPERRUST_INTEGRITY_REPORT_KEEP` | – | Integritätsprüfung (inkrementell, Bericht), siehe docs/user/integrity.md |
| `PAPERRUST_MIGRATE_SOURCE`, `PAPERRUST_MIGRATE_TARGET` | `PAPERLESS_MIGRATE_…` | Quelle/Ziel von `paperrust-migrate-db` |

Alle übrigen Einstellungen sind die upstream-Variablen (`PAPERLESS_DBHOST`, `PAPERLESS_SECRET_KEY`, …) und
heißen zusätzlich `PAPERRUST_DBHOST`, `PAPERRUST_SECRET_KEY`, …

### Was bewusst bleibt (und warum)

- **GPL-3.0-Würdigung:** „PaperRust basiert auf paperless-ngx (GPL-3.0), © die paperless-ngx-Autoren“
  (`pl_core::brand::ATTRIBUTION`, Startbanner), „based on paperless-ngx“ unter dem Logo der Konto-Seiten
  (bei Branding `off` upstreams „by Paperless-ngx“), Hinweise im Frontend (Patches 05/08), Quellverweise `upstream: src/…@v3.4.2` in Kommentaren.
- **Kompatibilität:** `PAPERLESS_*`-Variablen, Django-Tabellen/-Schema, API-Pfade, JSON-Felder, Cookies,
  `paperless.conf`, Download-Namen wie `paperless-share-*.zip`, IMAP-Schlüsselwort `paperless`,
  Export-Manifest-Version `3.4.2` – alles, was Clients und Daten sehen (früher auch Django im Mischbetrieb).
- **`X-Version: 3.4.2`** (`pl_core::brand::UPSTREAM_VERSION`): die Angular-Oberfläche liest den Header nicht
  (Version ist einkompiliert, Updates über `/api/remote_version/`), aber Apps/Skripte von Drittanbietern
  erkennen daran den API-Stand. Eine PaperRust-Version dort würde sie brechen. Die eigene Version steht in
  `/__rs/health` und im Startbanner.
- **Log-Targets `paperless.*`** (`paperless.auth`, `paperless.handlers`, …): das sind die upstream-
  Loggernamen; Log-Auswertung (fail2ban, Filter, UI-Logansicht) bleibt kompatibel. Eigene Targets heißen
  `paperrust::access` bzw. nach Crate (`pl_ai`, `pl_mail`).
- **Temp-Präfixe** `paperless-mail-`, `paperless-reprocess-` usw.: wie upstream, nicht sichtbar.
- **Testwert** `paperless-rs-reference-secret-key-local-only-…`: SECRET_KEY der Referenzumgebung
  (`reference/env.sh`, nicht änderbar); Pfade `…/Projekte/paperless-rs/…` bis zur Umbenennung des Ordners.
- **Crate-Namen `pl-*`:** intern, nirgends sichtbar (Binaries heißen `paperrust*`); Umbenennen würde jede
  Datei, jeden Vertrag und `Cargo.lock` berühren, ohne Nutzen für Nutzer.

`grep -ri "paperless-rs\|PAPERLESS_RS\|plrs_"` in `crates/` zeigt danach nur noch die Alias-Stellen in
pl-core/pl-server (Absicht), den Referenz-Testwert und die Repo-Pfade.

## Umsetzung ohne Fork

> **Abgelöst am 2026-11-13:** PaperRust liefert die Angular-Oberfläche nicht mehr aus; `frontend/`
> (Patches, `rebrand.mjs`, `build.sh`) ist gelöscht. Die eigene Oberfläche `frontend-light/` trägt Name,
> Logo und Farbe selbst; `/` und alte Angular-Adressen leiten auf `/rs/app/` (docs/FRONTEND-PARITY.md).
> Der folgende Absatz beschreibt den früheren Stand.

Entscheidung des Nutzers: Das Angular-Frontend (`upstream/src-ui`) wird **nicht** geforkt. Stand
2026-11-09 (R2) wird es beim Build kopiert und mit einer **Patch-Serie** (`frontend/patches/01…08`) und
dem Ersetzungsskript `frontend/rebrand.mjs` auf PaperRust umgestellt (Name in Quelltexten und allen 51
Übersetzungen, Doku-/Repo-Links, Logos, Akzentfarbe, Versions-/Lizenzhinweis). Ergebnis `frontend/dist/`,
Betrieb mit `PAPERLESS_STATICDIR=<repo>/frontend/dist`. Für ein Upstream-Update: Submodul anheben,
`frontend/build.sh` laufen lassen; passt ein Patch nicht mehr, bricht der Build mit dessen Namen ab.
Einzelheiten: **frontend/README.md**.

Serverseitig (für das unveränderte upstream-Frontend **und** für die von Rust ausgelieferten Seiten)
ergänzt `crates/pl-server/src/branding.rs` (Vertrag: docs/contract/pl-server.md, „Branding“):

| Was | Wie |
|---|---|
| Schalter | `PAPERRUST_BRANDING=on|off` (Standard **on**). `off` → Server byte-transparent wie upstream (keine Injektion, upstream-Texte auf Index- und Konto-Seiten, `/__rs/brand/` antwortet 404). |
| Name/Logo | upstream-Haken `PAPERLESS_APP_TITLE`/`PAPERLESS_APP_LOGO`. Sind sie **nicht gesetzt** (leer gilt als gesetzt = „kein Branding“ wie upstream), bekommt die Rust-Route `ui_settings.get` „PaperRust“ und `/__rs/brand/logo.svg` (über `pl_core::Config`). Ein im UI hochgeladenes Logo/Titel (`ApplicationConfiguration`) hat wie upstream Vorrang. Django liest seine Umgebung selbst → docs/OPERATIONS.md. |
| Produktname | `pl_core::Config::product_name = "PaperRust"` bei `on`: Ladebildschirm des Index (Vorlage `crates/pl-server/src/frontend/index_paperrust.html` = `frontend/server/index.html`: Titel, Autor, Logo, Doku-Link; Text „… wird geladen“ in allen Sprachen), Seitentitel der Konto-Seiten („PaperRust sign in“ …), Mail-Betreff/-Gruß („[PaperRust] …“). |
| GPL-Hinweis | Konto-Seiten (Login, Registrierung, Passwort, MFA, Social): unter dem PaperRust-Logo klein „based on paperless-ngx“ (de/fr/it übersetzt, sonst Englisch) statt „BY PAPERLESS-NGX“; `meta author` „PaperRust (based on paperless-ngx, GPL-3.0)“. Im Frontend: Seitenleiste und Willkommens-Widget (Patches 05/08). |
| Akzentfarbe | `PAPERRUST_ACCENT` (Hex, Standard Rost `#B7410E`). Vor `</head>` jeder HTML-**GET-Navigation** (Angular-Index, Login-/Konto-Seiten; nie `/api/`, `/share/`, `/static/`, `/logo`, `/ws/`, `/admin/`, Dateien) genau ein `<style id="paperrust-brand">`, das `--pngx-primary` (H, S) und `--pngx-primary-lightness` setzt (Konto-Seiten: die dortigen Hex-Variablen aus `static/base.css`). Das gepatchte Frontend hat Rost schon als Standard (Patch 07). |
| Favicon | `<link rel="icon">` → `/__rs/brand/favicon.ico` (16/32/48, in Rust aus der Geometrie von `favicon.svg` gerastert), zusätzlich `<link rel="icon" type="image/svg+xml" href="/__rs/brand/favicon.svg">`. |
| Assets | `/__rs/brand/{logo,logo-inverse,logo-mono,app-icon,favicon}.svg`, `favicon.ico` – per `include_bytes!` aus `docs/design/assets`, `Cache-Control: public, max-age=86400`, `nosniff`, CSP ohne Skripte. PNG-Icons des Frontends: `frontend/assets` (`tools/render-icons.mjs`). |

**Kontrast (geprüft im Test `branding::tests`):** `#B7410E` = hsl(18, 86 %, 39 %) → gerendert
`#b9410e`. Button-Schrift im hellen Modus ist `--bs-light` `#f8f9fa`: **5,21:1** (weiß: 5,49:1),
Hover (−5 %) 6,41:1 – also ≥ 4,5:1 ohne Anpassung. Für eine eigene Akzentfarbe senkt PaperRust die
Helligkeit in 1-%-Schritten, bis ≥ 4,5:1 gegen `#f8f9fa` erreicht ist (z. B. `#E07A3F` wird dunkler).
Dunkelmodus: upstream setzt die Button-Schrift auf `#abb2bf`; Rost erreicht dort 2,58:1 (upstream-Grün
mit seinen 31 % nur 2,47:1). Mehr ginge nur über eine andere Schriftfarbe – das wäre mehr als „nur
Akzentfarbe“, daher bewusst nicht.

**Dunkelmodus und Nutzerfarbe:** upstream schaltet im Dunkelmodus nur die *Standard*farbe um
(`[data-bs-theme=dark|auto] body:not(.primary-light):not(.primary-dark)` → 31 %, Navbar bleibt
„og green“). Das Brand-Style überschreibt genau diese Selektoren (gleiche Spezifität, steht nach
`styles.css`) mit Rost in derselben Helligkeit wie hell. Die per-Nutzer-„Theme-Farbe“ setzt die
Variablen inline am `<html>` (schlägt `:root`) und die Body-Klasse `primary-light|dark` (dann greifen
die Dunkelmodus-Regeln nicht) → **Nutzerfarbe hat weiter Vorrang**, auch im Dunkelmodus. Bootstrap-
Komponenten mit primary (`.btn-primary`, `.btn-outline-primary`, `.bg-primary`/Navbar, `.text-primary`,
Links, Fokus-Schatten, Toasts, `--pngx-primary-*`-Abstufungen) leiten sich alle aus den beiden
Variablen ab.

**Was weiterhin nicht geht bzw. bewusst so bleibt:**
- Mit dem **unveränderten** upstream-Frontend (statt `frontend/dist`) gelten die alten Grenzen:
  `theme-color` Grün `#17541f`, upstream-PNG-Icons/Manifest, Texte „Paperless-ngx“ in der Oberfläche.
  Mit `frontend/dist` sind sie behoben (Patches 06/07, `rebrand.mjs`, `frontend/assets`).
- (Geschichte, bis 2026-11-13) Django-eigene Seiten (Mischbetrieb, Route auf `django`) zeigten Djangos Templates: inline-SVG-Logo, wenn
  Django ohne `PAPERLESS_APP_LOGO` läuft, „by Paperless-ngx“, upstream-Seitentitel. Abhilfe: die
  Konto-Routen in Rust bedienen (Standard in `routes.toml`) bzw. Django `PAPERLESS_APP_LOGO`/`_TITLE` setzen.
- Konto-Seite ohne Logo, nur mit eigenem Titel (`PAPERLESS_APP_LOGO=""`): links vom Titel bleibt das
  upstream-Blatt (`svg_leaf.html`); ganz ohne Titel/Logo (beide leer) das upstream-Logo – „leer“ heißt
  wie upstream „kein eigenes Branding“.
- `--pngx-primary-faded` (= Helligkeit + 76 %) wird bei Rost 39 % auf weiß gekappt – genau wie upstream
  bei jeder Nutzerfarbe mit ≥ 24 % Helligkeit.
- Die Übersetzung „based on paperless-ngx“ der Konto-Seiten gibt es nur für de/fr/it (kein upstream-
  Katalog); übrige Sprachen zeigen Englisch.
- Paritätsläufe: Branding **aus** (`PAPERRUST_BRANDING=off`), sonst unterscheiden sich `ui_settings`
  (`app_title`/`app_logo`) und HTML-Seiten von Django – siehe docs/contract/testing.md.
  Vorschlag Akzeptanzregel für Läufe **mit** Branding (nicht eingetragen): Index und Konto-Seiten gelten
  als gleich, wenn sie nach Normalisierung übereinstimmen: (1) `<style id="paperrust-brand">…</style>`
  und die beiden `<link rel="icon" …/__rs/brand/…>` entfernen; (2) auf dem Index den Block zwischen
  `<title>` … `</title>`, `meta author`, das Lade-`<svg class='logo'…>…</svg>` und den Doku-`href` des
  Warnhinweises durch Platzhalter ersetzen; (3) in übersetzten Texten `PaperRust` → `Paperless-ngx`;
  (4) auf Konto-Seiten `<span class="byline …">…</span>`, `meta author` und `<img src="/__rs/brand/logo.svg" …>`
  bzw. das upstream-Logo/Blatt durch Platzhalter ersetzen. Alles andere (Formulare, CSRF, Meldungen,
  Statuscodes, Header außer `Content-Length`) muss byte-gleich bleiben.
