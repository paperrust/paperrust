# Vertrag: Client-Kern `pl-sync-client` (P5, Desktop Modus 2b/3)

Stand 2026-11-09. Rahmen: docs/SYNC.md §4/§5/§8/§9, docs/REMOTE-APP.md §1/§4/§5, docs/ROADMAP-NEXT.md P5.
Gegenstellen: PaperRust über API v2 (docs/contract/pl-api-v2.md, docs/api/v2.yaml) und – eingeschränkt
(P6) – unverändertes paperless-ngx über v1 (SYNC.md §7, Abschnitt „paperless-ngx“ unten).

## Bausteine

| Modul | Inhalt |
|---|---|
| `store` | SQLite-Replikat je Profil (`profiles/<id>/replica.sqlite`), **sqlx-sqlite** (gebündeltes libsqlite3 3.46, FTS5 aktiv – Spike-Nachweis `store::tests::fts5_available_in_bundled_sqlite`), Migrationen über `PRAGMA user_version` (`migrations/0001_replica.sql`), WAL |
| `remote::paperrust` | v2-Client: `server`, `me`, `ui_settings`, `auth/devices` (Ausgabe/Widerruf), `sync/changes` (Cursor, `kinds`, Long-Poll `wait`), `sync/resolve`, `objects/batch`, `sync/ids`, Dateien mit `If-None-Match`/`Range`/`If-Range`, Rohweiterleitung fürs Gateway. Keine Weiterleitungen, keine Cookies, Token nur als `Authorization: Bearer prt_…` |
| `engine` | `SyncClient`: Anmelden/Abmelden, Auswahl prüfen/übernehmen, `sync_once`, Dauerschleife `run` (Long-Poll 25 s, Backoff 2 s…2 min), Status (`watch`) |
| `selection` | Auswahl (Dokument/Tag/gespeicherte Ansicht + Dateimodus `preferred|original|both|none`), Grenzen `max_documents` (5 000) / `max_bytes` (2 GiB), Beitrag je Eintrag |
| `files` | Dateicache `profiles/<id>/files/<uuid[0..2]>/<uuid>.<original|archive|thumb>`, `*.part` + Umbenennen, SHA-256-Prüfung, Wiederaufnahme, LRU-Verdrängung nur nicht ausgewählter Dateien |
| `query` | Offline-Liste mit `DocQuery` (gleiche JSON-Form wie `POST /documents/query`), FTS5-Suche mit Ausschnitt, Metadaten-Listen mit Keyset-Cursor (`pl_domain::cursor`) |
| `gateway` | Logik des `prapi://`-Schemas (online/offline, `/local/…`), von der Hülle nur noch umgesetzt |
| `v1local` | Offline-v1 für die Light-UI (die heute v1 spricht): Dokumentliste mit v1-Filtern/Sortierung/`all`/`selection_data`, Dokument, Notizen, Metadaten, Vorschaubild/PDF/Download, Stammdaten, `ui_settings` – aus dem Replikat; dazu Antwort-Cache (`http_cache`, Migration 0002) für Rahmen-Endpunkte |
| `app` | `/rs/app/` (gepackter Light-UI-Build, SPA-Fallback) und eigene Anmeldeseite `/accounts/login/` (Modus 2b) |
| `remote::ngx`, `engine_ngx`, `filter_rules` | P6: paperless-ngx über v1 (Token, `all`, `modified__gt`, Fingerabdruck), Abbildung v1 → v2-Form, gespeicherte Ansichten → Query-Parameter (Port von `queryParamsFromFilterRules`) |
| `secret` | Token (+ Geräte-UUID, Zeile 2) im Windows-Anmeldeinformationsspeicher (`PaperRust/<profile-id>`, CredWriteW/CredReadW über `windows-sys`), sonst nur im Speicher |
| `tls` | rustls; ohne Pin Systemvertrauen (`rustls-native-certs`), mit Pin eigener `ServerCertVerifier` (SPKI-SHA-256 = `tls_pin` der Hülle) **im Handshake** |

## Ablauf (SYNC.md §4.5)

1. `GET /server` (mit Token). Fremde `server_uuid` → Replikat leeren, neu beginnen.
2. Ohne Cursor: Feed ohne Cursor nur für Metadaten-Arten (`tag, correspondent, document_type,
   storage_path, custom_field, saved_view, user, group`) → Cursor merken.
   Mit Cursor: Feed ab Cursor bis `has_more=false`. `410` → Cursor verwerfen, Metadaten neu +
   Vollabgleich. `perm_epoch` geändert oder letzter Vollabgleich > 24 h → Vollabgleich.
3. Dokument-Änderungen im Feed: lokal vorhandene werden aus dem eingebetteten Objekt aktualisiert
   (`has_detail=0` bei neuem `rev` → Inhalt/Notizen werden per `batch` nachgeladen, Dateien bei
   neuer Prüfsumme); `delete` (Löschen, Papierkorb, Rechteentzug) entfernt Zeile + Dateien.
   Enthält die Auswahl Tags/Ansichten, löst jede Dokument-/Tag-/Ansichtsänderung ein neues
   `resolve` aus (gedrosselt ≤ 1×/30 s; der Long-Poll wartet dann nur bis zum Ablauf der Drossel).
4. `resolve` je Auswahleintrag (Einzeldokumente gemeinsam in einer Anfrage) → Plan (Vereinigung,
   Dateiwünsche je Dokument) → über der Grenze: Fehler `limit_exceeded`, nichts geändert.
   Abgewählte Dokumente: `in_selection=0` (Metadaten 7 Tage), Dateien sofort weg.
5. `objects/batch` mit `include=content,notes` in Blöcken à 500 für neue/geänderte Dokumente.
6. Dateien (4 parallel): Varianten nach Plan + Vorschaubild; vorhandene mit gleicher Prüfsumme bzw.
   gleichem Vorschau-ETag ohne Anfrage; `files.thumbnail.etag = null` → keine Anfrage.
7. Vollabgleich: `sync/ids` je Art → lokal Überzähliges löschen, abweichende `rev` per `batch`.

**Abfragemodus:** Meldet der Server kein `sync`-Feature (Feed nicht eingehängt, `/sync/changes`
→ 501), arbeitet der Kern ohne Feed: Metadaten über `ids`+`batch`, Dokumente über `resolve`+`batch`,
alle 60 s (`Status.mode = "polling"`).

## Schema (Abweichungen von SYNC.md §4.2)

- `document.id INTEGER PRIMARY KEY` + `key UNIQUE` (stabile rowid für FTS5);
- `fts` mit **externem Inhalt** (`content='document'`, Trigger) statt `content=''` → `snippet()`
  geht; dafür `document.notes` (Text) und `document.names` (Korrespondent, Typ, Speicherpfad, Tags)
  als Spalten, `names` wird bei Umbenennung der Objekte neu berechnet;
- zusätzlich `remote.device_uuid/last_contact/me_json/ui_settings`, `setting` (Grenzen),
  `file.state/checksum/pinned`, `document.added/storage_path/owner/mime_type/original_filename/
  notes_json/has_detail/left_selection`; `outbox` (3b) schon angelegt.

## `prapi://` für die Light-UI (S9 und 2b) – was die UI nutzen kann

Basis: Windows `http://prapi.localhost`, sonst `prapi://localhost`. Nur aus App-Origins
(`tauri://localhost`, `http(s)://tauri.localhost`), sonst 403 vor jeder Verarbeitung; CORS nur
für diese. Jede Antwort hat `X-PaperRust-Source: remote|local`.

| Pfad | online | offline / `?source=local` |
|---|---|---|
| `/rs/api/v2/…` | an den Server (Token vom Kern) | lesend aus dem Replikat: `server`, `me`, `ui_settings`, `documents` (GET mit `cursor,limit,sort,include,count`), `documents/query` (POST, `DocQuery`; `text` = FTS5), `documents/{uuid}` (`include=content,notes`), `…/notes`, `…/files/{original,archive,preview,thumbnail}` (ETag/304/Range/206/416; `404 not_cached`, wenn nicht lokal), `objects/batch`, `{tags,correspondents,document_types,storage_paths,custom_fields,saved_views,users,groups}[/{uuid}]`; Schreiben → `503 offline` |
| `/api/…` (v1, übergangsweise – die Light-UI spricht heute v1) | an den Server; GET-Antworten der Rahmen-Endpunkte (`ui_settings`, `profile`, `statistics`, `tasks/status_counts`, Stammdatenlisten …) werden zwischengespeichert | lesend aus dem Replikat (`v1local`): `documents/` (v1-Filter, `ordering`, `page`/`page_size`, `all`, `selection_data`, `truncate_content`, `fields`), `documents/<id>/` + `notes/`, `metadata/`, `suggestions/` (leer), `thumb/`, `preview/`, `download/` (`?original=true`), Stammdaten (Cache, sonst synthetisiert), `ui_settings` (Cache, sonst aus `/me`); anderes aus dem Cache oder `503 offline`; Schreiben `503` |
| `/rs/app/…` | gepackte Light-UI (SPA-Fallback, CSP wie pl-server) | dito |
| `/accounts/login/`, `/accounts/logout/` | eigene Anmeldeseite (→ `/local/login`, Rücksprung nur nach `/rs/app/…`), Abmelden widerruft das Gerät | dito |
| `/rs/api/v2/auth/…`, `/api/token/`, `/api/profile/generate_auth_token/`, `/api/profile/totp/`, `/api/auth/`, `/api/oauth/` | **403** (Token nur im Kern) | 403 |

Die WebView darf `Authorization`, `Cookie`, `X-CSRFToken` nicht setzen (werden verworfen);
weitergereicht werden nur `accept, accept-language, content-type, if-match, if-none-match, range,
if-range, idempotency-key, x-requested-with`.

Kern-Funktionen (`/local/…`, JSON):

| Methode | Pfad | Körper → Antwort |
|---|---|---|
| GET | `/local/status` | → `{status:{state,online,mode,logged_in,last_sync,last_error,documents,phase,done,total,token_persistent}, source, force_offline, profile:{id,url,user_name,server_uuid,last_full_sync}, limits, selection, cache_bytes, documents, objects}` |
| POST | `/local/login` | `{username,password,code?}` → `{ok}`; Passwort nur im Speicher (`zeroize`) |
| POST | `/local/logout` | `{wipe?}` → `{ok,revoked}` (Gerät am Server widerrufen, Schlüsselbund leer, mit `wipe` Replikat + Dateien weg) |
| GET | `/local/selection` | → `{items:[{type,key,files}],limits}` |
| POST | `/local/selection/preview` | `{items}` → Plan `{entries:[{type,key,files,documents,bytes}],total_documents,total_bytes,limits,within_limits,message,as_of_seq}` |
| PUT | `/local/selection` | `{items}` → Plan; über der Grenze `422 limit_exceeded` (Text nennt die Beiträge) |
| PUT | `/local/limits` | `{max_documents,max_bytes}` |
| POST | `/local/sync` | sofort abgleichen |
| POST | `/local/mode` | `{force_offline}` (Offline erzwingen, z. B. Test/Flugmodus) |
| GET | `/local/search?q=&limit=` | → `{items:[{document,snippet}]}` (bm25, Ausschnitt mit `[[…]]`) |

Fehler im v2-Format `{"error":{"code","message"}}`; zusätzliche Codes: `offline` (503),
`not_cached` (404), `limit_exceeded` (422), `pin_mismatch`/`tls` (495), `not_configured` (503,
Hülle ohne aktives Sync-Profil), `origin_not_allowed` (403).

## Hülle (apps/desktop-shell, Phase B)

- `src/prapi.rs`: Schema-Registrierung (`register_asynchronous_uri_scheme_protocol`), Herkunfts-
  prüfung, CORS/Preflight, Umsetzung `http::Request` ↔ `GwRequest`.
- `src/sync.rs`: Kern je aktivem PaperRust-Profil mit `sync = true` oder `ui = "app"` starten
  (`profiles/<id>/`), Gateway in `PrapiState`; Befehle `sync_profile_state`, `sync_profile_set`
  (schaltet `sync` im `app.toml`, Neustart); `forget` beim Abmelden/Entfernen eines Profils
  (Schlüsselbund-Eintrag).
- `main.rs` (nur Ergänzungen): `mod prapi; mod sync;`, `.manage(…)`,
  `.register_asynchronous_uri_scheme_protocol`, Start nach `remote_loop`, `forget` in
  `profile_logout`/`profile_remove`, zwei Befehle im `generate_handler!`.
- `tauri.conf.json`: CSP `connect-src`/`img-src` um `prapi: http://prapi.localhost`.
- `ui/settings.html` + `ui/sync.js`: Karte „Offline-Bestand (Sync)“ (Ein/Aus, Anmelden, Auswahl
  Tags/Ansichten + Dateimodus, Grenzen, Prüfen/Übernehmen, Jetzt abgleichen, Abmelden).
- **Modus 2b (`ui = "app"`)**: `begin()` startet für solche Profile keine `remote_loop` (keine
  Server-Seite im Fenster), erlaubt die Origin `http://prapi.localhost` und setzt
  `ready_url = http://prapi.localhost/rs/app/`; die Light-UI kommt aus `light\` neben dem Programm
  (`PAPERRUST_LIGHT_UI_DIR`, Debug: `frontend-light/dist`). Herkunftsprüfung: App-Seiten
  (`/rs/app/…`, `/accounts/login…`) dürfen als Dokument ohne Origin/Referer geladen werden, Daten nur
  mit Origin/Referer einer App-Origin (inkl. prapi selbst); Server-HTML läuft mit
  `CSP: sandbox` (nie mit der prapi-Origin). Befehl `sync_profile_set_ui` (+ Karte „Oberfläche“
  in sync.js freigeschaltet, wenn PaperRust/paperless-ngx und Light-UI-Build vorhanden).
- E2E der **gebauten App** über WebView2-CDP: `apps/desktop-shell/e2e/app-2b-e2e.mjs`
  (Ergebnis `results/desktop/app-2b/`).
- **Links in die klassische Oberfläche (2b, Restposten 2026-11-10):** Seitenpfade außerhalb von
  App/API/Kern (`/documents/12/notes`, `/settings`, `/documents?query=…`; `app::is_classic_path`)
  werden **nicht** durchgereicht – die Angular-Oberfläche braucht Sitzungs-Cookie und Skripte, über
  prapi liefe Server-HTML nur mit `CSP: sandbox` (leer) und ohne Cookies. Online fängt die Hülle die
  Navigation ab (`Gateway::classic_url` → `<server>/<pfad>?<query>` im System-Browser, Fenster bleibt
  in der App); ohne Abfangen/beim Neuladen zeigt das Gateway eine Hinweisseite mit Link zum Server
  (Server-Origin ist im Fenster nicht erlaubt → System-Browser). Offline: Seite „Nur online
  verfügbar“ (503, CSP `default-src 'none'`, kein Skript, Link zurück zur App). `prapi`-Vorprüfung
  lässt diese Seite als Dokument ohne Origin zu (nur GET).
- **`/ws/status/` in 2b (`wsrelay`):** eigene Schemata können keine WebSockets. Der Kern lauscht auf
  `127.0.0.1:<zufällig>`; `GET /local/ws` → `{url, online}` (`url` = `ws://127.0.0.1:<port>/ws/status/?k=<244 Bit>`,
  `null` ohne Vermittler oder bei paperless-ngx, dessen Socket nur Sitzungs-Cookies kennt). App-Seiten
  bekommen `connect-src 'self' ws://127.0.0.1:<port>`. Je Verbindung: Pfad, Schlüssel (zeitkonstant),
  `Origin` ∈ App-Origins, `Upgrade` → sonst 403; offline/abgemeldet/Server weg → 503; online neuer
  Handshake zum Server (`<basis>ws/status/`, TLS/Pinning wie der Kern, `Authorization: Bearer prt_…`,
  ohne Cookie/Origin/Schlüssel), bei 101 nur noch Bytes kopiert. Die Light-UI holt die Adresse über
  `/local/ws` und versucht es nach Abbruch still erneut (5 s … 60 s). Server-Seite: `pl-server`
  `ws.rs` nimmt ohne Sitzung zusätzlich das Geräte-Token an (Known difference dort).
- Tests: `app::tests::classic_paths`, `classic_page_online_links_to_server_offline_explain`,
  `wsrelay::tests` (Prüfung, Handshake ohne Cookie/Origin), `tests/client.rs::
  classic_links_online_point_to_the_server_offline_explain`, `ws_status_relay_online_with_device_token_offline_503`
  (echter Upgrade gegen Mock mit Bearer-Pflicht: 101, `Sec-WebSocket-Accept`, Meldung + Echo, 403/503);
  E2E: `results/desktop/app-2b/leftovers/` (siehe DESKTOP.md §24).

## paperless-ngx (P6, SYNC.md §7)

Profil `flavor = ngx`, gleiche Replikat-/Dateicache-/Gateway-Logik:

- **Anmeldung** `POST /api/token/` (Benutzer/Passwort/`code`), Token im Schlüsselbund, Header
  `Token <key>`, `Accept: application/json; version=9`. Abmelden kann am Server nichts widerrufen
  (ein Token je Nutzer) – Hinweis in der UI.
- **Je Lauf** (Abfragemodus, alle 60 s): `ui_settings` → Stammdaten vollständig
  (`page_size=100000`, höchstens alle 5 min) → Auswahl (Dokument-ID, `tags__id__in`, gespeicherte
  Ansicht über `filter_rules::query_params`; Ergebnis über `all`) → **gelöscht/entzogen** über `all`
  ohne Filter → **geändert** über `modified__gt=<Wasserstand − 2 s>` und alle 15 min
  **Fingerabdruck** (`id__in` ≤ 100, `fields=id,modified,tags,correspondent,document_type,
  storage_path,custom_fields,notes,archive_serial_number,title,created`, SHA-256) → vollständig
  holen (`id__in`, Inhalt + Notizen, `metadata/` für Prüfsummen) → Dateien (`download/`,
  `download/?original=true`, `thumb/` mit ETag = Prüfsumme).
- **Abbildung**: v1 → v2-Form; Schlüssel = synthetische UUID `00000000-0000-4000-8<Art>-<ID hex>`
  (`remote::ngx::ngx_uuid`), `rev` aus dem Fingerabdruck. Online geht v1 der Light-UI direkt an
  paperless-ngx (Token vom Kern), `/rs/api/v2/…` wird immer aus dem Replikat beantwortet.
- **Grenzen**: v1 liefert keine Dateigrößen → vorab nur die Dokumentzahl geprüft, Bytes begrenzt der
  Dateicache. IDs, die `all` meldet, die `id__in` aber nicht liefert (z. B. Versionen), werden
  gemerkt und nicht in jedem Lauf neu angefragt.

## Tests

- `cargo test -p pl-sync-client -j 2` – Unit (lib, 28) + `tests/client.rs` (12, Mock-Gegenstelle
  nach dem v2-Vertrag: Erstabgleich, Feed, Löschen/Entzug, Umbenennen, Abwählen + 7 Tage, Grenzen,
  410, fremde `server_uuid`, Abfragemodus ohne Feed, Datei-Wiederaufnahme/Prüfsumme/304,
  Gateway online/offline inkl. Liste/Suche/PDF/Range/Schreibsperre, Long-Poll-Schleife,
  Token nie auf der Platte + Abmelden widerruft, Widerruf auch ohne Replikat über die Geräte-UUID im Schlüsselbund, alle Einstiegspunkte `tokio::spawn`-fähig) + `tests/tls.rs` (4: richtiger/falscher Pin
  ohne gesendete Anfrage, abgelaufen, eigene CA, http-Loopback).
- `tests/e2e_server.rs` (`#[ignore]`, braucht lokalen Testserver auf einer `pl_p5_*`-Kopie,
  Variablen `PL_P5_E2E_URL`, `PL_P5_E2E_DB`, `PL_P5_E2E_DIR`): Erstabgleich ≥ 5 000 Dokumente
  (Messung), Änderung/Papierkorb/Rechteentzug über den Feed, offline Liste/Suche/PDF, Neustart
  konsistent, Aufräumen (Widerruf + Schlüsselbund).
- `tests/client.rs::v1_offline_for_the_light_ui`: v1 online durchgereicht + Cache, offline Liste
  (Filter, `all`, `selection_data`, Seiten), Volltext, Dokument/`fields`, Vorschaubild, PDF,
  Download, Schreibsperre; `app::tests` (SPA, Pfadausbruch), `filter_rules::tests` (Fälle aus
  `query-params.spec.ts`), `remote::ngx::tests` (Abbildung, Fingerabdruck).
- `tests/e2e_ngx.rs` (`#[ignore]`, beschreibbare paperless-ngx-3.4.2-Kopie über
  `W2_NAME=w2p6 tests/parity/w2-*.sh`; `PL_P6_NGX_URL`, `PL_P6_NGX_DB` (`pl_w2*`), `PL_P6_NGX_STOP`):
  Token-Anmeldung, Stammdaten = Server, Tag-Auflösung = Serverliste, gespeicherte Ansicht,
  `bulk_edit add_tag` ändert `modified` **nicht** und wird ohne Fingerabdruck **nicht**, mit
  Fingerabdruck **doch** erkannt, Löschen und Rechteentzug über `all`, Gateway (v1 remote, v2 lokal),
  offline Liste/Suche/PDF/`ui_settings`.
- `cargo clippy -p pl-sync-client -j 2 --tests -- -D warnings` sauber.

## Messung (2026-11-09, Debug-Build, Laptop geteilt mit anderen Agenten)

Lokaler Testserver `paperrust serve` (Debug, Feed eingehängt) auf der Kopie `pl_p5_sync`
(`paperless_snapshot` mit `tests/pgvalidate/scale_100k.sql` auf 25 Klone je Seed-Dokument
= 5 204 Dokumente, 5 070 für admin sichtbar), Client im selben Rechner:

| Schritt | Ergebnis |
|---|---|
| Erstabgleich 5 070 Dokumente ohne Dateien (Auswahl prüfen + Metadaten-Feed + resolve + batch mit Inhalt/Notizen + 195 vorhandene Vorschaubilder) | **17,6 s** (Ziel < 120 s); Replikat 32,5 MB, 5 070 FTS-Zeilen |
| Folge-Abgleich ohne Änderungen | 0,13 s |
| Tag „Invoices“ mit Archiv-PDFs dazu | 5,5 s, 18 PDFs |
| Offline: Liste (25, `count`), FTS-Suche über 5 069 Dokumente | Suche 40 ms |
| Titeländerung/Papierkorb (direkt in der DB), Rechteentzug alice | über den Feed nach ≤ 1–2 Sequenzer-Takten im Replikat |

## Known differences / offen

1. Offline-Suche = FTS5 (Präfix je Wort, Phrasen), nicht die tantivy-Syntax des Servers; lokal
   umfasst `title_content` auch Notizen und Namen. `custom_field_query`, `shared_by_me`, `any`/`not`
   offline → 501.
2. `added`/`modified`-Grenzen werden offline auf dem UTC-Datum verglichen (Server: Zeitzone).
3. Tags im `DocQuery` exakt (wie `pl_docfilter`), Auflösung von Tag-Auswahlen mit Unter-Tags macht
   der Server (`resolve`).
4. Thumbnails zählen nicht zur Bytes-Grenze (Größe unbekannt); Cachegrenze = `max_bytes` + 256 MiB.
5. Schlüsselbund nur Windows; macOS/Linux: Token nur im Speicher (Neuanmeldung je Start), bis
   `keyring`/Secret Service eingebunden ist.
6. ≤ 4 Long-Polls je Token (SYNC.md §8) ist Server-Sache (noch offen in pl-api-v2).
7. In gemischten Installationen beantwortet Django v1-Pfade, die noch nicht auf Rust laufen; Django
   kennt `Bearer prt_…` nicht → solche v1-Aufrufe der Light-UI scheitern in 2b mit 401.
9. paperless-ngx: Schlüssel sind synthetische UUIDs statt `ngx:<id>` (SYNC.md §4.2), damit
   `DocQuery`/Gateway ohne Sonderfall gelten. Tag-Auswahl mit Unter-Tags so, wie paperless-ngx
   `tags__id__in` auswertet. Regelarten ohne Abbildung → Ansicht nicht wählbar (`invalid`).
   paperless-ngx kann kein `Range` → abgebrochene Downloads beginnen neu.
10. Offline-v1 ist lesend und deckt Liste/Dokument/Dateien/Stammdaten ab; Volltext wie oben (FTS5),
    Sortierungen nach Notizzahl/Eigentümer genähert; `custom_field_query`, `more_like_id`,
    `shared_by__id` offline 501; Verlauf, Vorschläge (leer), Aufgaben, Mail usw. nur aus dem
    Antwort-Cache oder 503.
8. Rust-Compilerfehler rust-lang/rust#102211 („Send is not general enough“) wird umgangen:
   geboxte Futures an wenigen Stellen, keine geliehenen Slices/Closures über `await` (Kommentare
   im Code) – sonst ist `tokio::spawn(client.run(…))` nicht möglich.
11. **`/rs/app/…` nur innerhalb des App-Ordners** (Sicherheitsprüfung 2026-11-19 F8): der Pfad wird nach
    der Prozent-Dekodierung über `app::app_file` aufgelöst (Regeln wie `pl_core::safepath::safe_join`,
    hier auf allen Plattformen streng: `/` und `\` als Trenner, kein absoluter Pfad, kein `..`, kein
    `:`, kein NUL, keine Windows-Gerätenamen, kein Punkt/Leerzeichen am Ende). Ausbruch → 404 (kein
    SPA-Fallback). Eigene Fassung statt `pl_core`, weil pl-sync-client bewusst ohne axum/sqlx-Postgres
    auskommt (siehe CORE-REQUEST im Bericht: `safepath` in ein leichtes Crate verlegen).
