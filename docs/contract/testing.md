# Vertrag: Test-Infrastruktur (Paritäts-Diff, Sicherheits-Suite, Upstream-Adapter, Gates)

Besitz: Coder 12. Dateien: `tests/parity/harness/`, `tests/parity/accepted.json`,
`tests/parity/<crate>/cases.base.json` (generiert), `tests/security/`, `tests/upstream-adapter/`,
`scripts/run-gates.sh`, `scripts/upstream-diff.sh`, `deny.toml`, `UPSTREAM_MAP.md` (Struktur;
Zeilen pflegen die Fach-Coder), dieses Dokument.

Alle Werkzeuge: **Python ≥ 3.11, nur Standardbibliothek** (außer Upstream-Adapter, s. u.).
Alle HTTP-Clients **verweigern Nicht-Loopback-Ziele** (`NonLoopbackTarget`, auch bei Redirects;
`localhost` muss ausschließlich auf Loopback auflösen).

---

## 1. Paritäts-Harness (`tests/parity/harness/`)

```
python tests/parity/harness/run_parity.py                       # alle Kataloge, A gegen B
python tests/parity/harness/run_parity.py --crate pl-objects --case 'tags.*'
python tests/parity/harness/run_parity.py --targets a           # nur Django: Katalog + Leck-Prüfung
python tests/parity/harness/run_parity.py --list                # expandierte Fälle
python tests/parity/harness/run_parity.py --no-write            # Schreibfälle überspringen
python -m unittest tests/parity/harness/test_harness.py         # Selbsttest (Stub-Server), 10/10 grün
```

Exit: 0 = keine Abweichung/kein Fehler/kein Leck in B; 1 = Abweichung, Fehler, Leck in B
(mit `--strict` auch Katalog-/Regel-Fehler); 2 = Ziel nicht erreichbar, Nicht-Loopback, keine Nutzer.

### Konfiguration (Umgebung)

| Variable | Standard | Zweck |
|---|---|---|
| `PARITY_A_URL` | `http://127.0.0.1:8000` | Django-Referenz (A) |
| `PARITY_B_URL` | `http://127.0.0.1:8080` | paperless-rs (B) |
| `PARITY_HOST_HEADER` | host:port von A | **gleicher** `Host` an beide Ziele → absolute URLs (`next`, `previous`, `Location`) vergleichbar, ohne sie umzuschreiben |
| `PARITY_USERS_FILE` | – | Nutzerkatalog (JSON) |
| `PARITY_RESET_CMD` | `bash reference/reset.sh` (falls vorhanden) | Snapshot-Reset für Schreibfälle |
| `PARITY_CUTOFF` | Laufstart | Stichtag für flüchtige Zeitstempel |
| `PARITY_TIMEOUT` | 60 | Sekunden pro Anfrage |
| `PARITY_SENSITIVE_PATHS` | – | zusätzliche Pfade (`;`) für die Leck-Prüfung; Repo, `reference/`, `PAPERLESS_MEDIA_ROOT`/`DATA_DIR`/`CONSUMPTION_DIR` sind immer dabei |

Es wird **kein** `Accept-Encoding` gesendet (Django hat Kompressions-Middleware); Redirects werden
nicht gefolgt (Status + `Location` werden verglichen), außer der Fall setzt `follow`.

### Nutzerkatalog

Quelle (erste Fundstelle): `PARITY_USERS_FILE` → `reference/users.json` → Block
```` ```json parity-users ```` in `reference/README.md` → Markdown-Tabelle in `reference/README.md`
mit 40-Hex-Token je Zeile (erste Spalte = Name, Spalten „Passwort“/„Rolle“ optional).

```json
{"users": [
  {"name": "admin",    "username": "admin", "password": "…", "token": "<40 hex>", "role": "superuser"},
  {"name": "user1",    "username": "…", "password": "…", "token": "…", "role": "user"},
  {"name": "user2",    "username": "…", "password": "…", "token": "…", "role": "user"},
  {"name": "noperm",   "username": "…", "password": "…", "token": "…", "role": "no model perms"},
  {"name": "inactive", "username": "…", "password": "…", "token": "…", "role": "is_active=false"}
]}
```

Logische Rollen, die die Kataloge verwenden (fehlende → Fall „übersprungen“ mit Grund):
`admin` (Superuser), `user1` (Modellrechte, besitzt Objekte), `user2` (besitzt Objekte, die mit
`user1` **nicht** geteilt sind; idealerweise einige per Objektrecht/Gruppe geteilt), `noperm`
(keine Modellrechte), `inactive`. Referenzen in Fällen: `anon`, `badtoken` (gültige Form,
unbekannt), `<name>` (Token), `<name>@session` (allauth-Formular-Login `/accounts/login/` mit
CSRF-Cookie + `csrfmiddlewaretoken`, danach `X-CSRFToken` bei unsicheren Methoden),
`<name>@basic`. Sitzungs-Logins werden je Ziel einmal gemacht und nach jedem Reset erneuert.
Cookie-Namen berücksichtigen `PAPERLESS_COOKIE_PREFIX` (Suffix `csrftoken`/`sessionid`).

### Fallkatalog (`tests/parity/<crate>/cases*.json`, YAML wenn PyYAML vorhanden)

- `cases.base.json` – **generiert** von `gen_base_catalog.py` (nicht von Hand ändern).
- `cases.json` (o. ä. `cases-*.json`) – gehört dem Fach-Coder der Crate, wird nie überschrieben.
- Format und alle Schlüssel: Kopfkommentar `plharness/cases.py`. Kurz: `id`, `route` (Routen-id),
  `method`, `path`, `query`, `json`/`form`/`data`, `headers`, `users` × `api_versions` (Matrix;
  `"10"`, `"9"`, `"none"` = `Accept: application/json`, sonst ungültige Version), `write`,
  `compare` (`full` | `status` | `status+headers` | `json-only`), `compare_headers`,
  `expect_status` (nur Plausibilitätsprüfung gegen A → „Katalog-Hinweis“), `leak_allow`, `follow`,
  `steps` (Abläufe, `capture` von Werten pro Ziel), `skip`.
- `vars`: einmal gegen **A** aufgelöst (z. B. erste Dokument-id), dann für A und B gleich verwendet:
  `{"get": "/api/tags/", "query": {...}, "user": "admin", "json": "results.0.id", "where": {...},
  "where_not": {...}, "field": "...", "default": ...}`; Abhängigkeiten zwischen Variablen erlaubt.
  `${name}` in Strings; ein String, der nur `${name}` ist, wird typtreu ersetzt.
- Fall-id im Report: `<id>@<user>/v<version>`.

### Vergleich

Status, Header-Whitelist und Körper. JSON wird **tief inkl. Schlüsselreihenfolge** verglichen;
Typen sind streng (`1` ≠ `1.0` ≠ `true`). Diff-Arten: `status`, `header`, `body-kind`, `body`
(Nicht-JSON: Länge + SHA-256), `type`, `value`, `missing`, `extra`, `key-order`, `length`.
Verglichene Header: `content-type`, `content-disposition`, `content-language`, `location`, `allow`,
`vary` (Token-Reihenfolge egal), `www-authenticate`, `x-api-version`, `x-version`,
`cache-control`, `etag`, `last-modified`, `x-frame-options`, `x-content-type-options`,
`referrer-policy`, `cross-origin-opener-policy`, sowie die **Namen** gesetzter Cookies.
Nie verglichen: `Server`, `Date`, Cookie-Werte, `Content-Length`, Hop-by-Hop.

### Normalisierung – abschließende Liste (beidseitig gleich angewandt)

1. ISO-8601-Datum-Zeit-Strings (ganzer Wert) **≥ Stichtag** → `<ts>` (im Lauf entstandene Werte:
   `last_login`, `modified`, `added` neuer Objekte). Ältere Zeitstempel werden exakt verglichen.
2. Nur in Schreibfällen: Werte unter `task_id` und ein UUID-String als ganzer Körper → `<task-id>`.
3. CSRF: JSON-Schlüssel `csrf_token`/`csrfmiddlewaretoken`/`csrftoken` und das versteckte
   Formularfeld in HTML → `<csrf>`.
4. Header: siehe Whitelist (Server-Header etc. werden nicht verglichen).
5. Reihenfolge ohne Verhaltensbedeutung (`plharness/normalize.py`): Listen aus Python-sets
   (`inherited_permissions`, `permissions`) sortiert; `selected_*` der POST-Antwort von
   `selection_data` nach `id` (`selection-data-post-by-id`); Label-Listen mit
   `ordering=[-]document_count` **nur innerhalb gleicher `document_count`** nach `id`
   (`document-count-ties-by-id`, seit 2026-11-11: upstream `order_by("document_count")` ohne
   Gleichstandsschlüssel → Planreihenfolge von PostgreSQL; Zahlen und Reihenfolge verschiedener
   Zahlen bleiben exakt geprüft. Grenze: ein Gleichstand über eine Seitengrenze hinweg wird nicht
   abgedeckt).

Alles andere muss identisch sein oder durch eine **benannte Regel** akzeptiert werden. Die Anzahl
angewandter Normalisierungen steht pro Fall im JSON-Report (`normalized`).

### Akzeptierte Abweichungen

`tests/parity/accepted.json` (global) und `tests/parity/<crate>/accepted.json` (Fach-Coder), Format
in `plharness/rules.py`. Pflicht: `name`, `reason`, `ref` (Abschnitt „Known differences“ im
Vertrag). Filter: `case` (fnmatch), `kind`, `path` (Regex auf Diff-Pfad), `a`/`b` (exakte Werte).
Ein Fall, dessen Diffs alle akzeptiert sind, zählt als „nur akzeptiert“; Treffer werden je Regel
gezählt, unbenutzte Regeln markiert.

### Schreibfälle

`write: true` → Ablauf `reset → A → reset → B → reset` (`reference/reset.sh` des Referenz-Agenten,
Snapshot-Restore). Ohne Reset-Skript werden Schreibfälle **übersprungen** (nie ungeschützt
ausgeführt). Lesefälle laufen zuerst, Schreibfälle am Ende.

**DB-Zustand prüfen (W2, 2026-11-08):** ein Schritt `{"sql": "select …"}` führt die Abfrage
schreibgeschützt (`default_transaction_read_only`) gegen `PARITY_SQL_DB` aus – direkt nach den
HTTP-Schritten der jeweiligen Seite, vor dem nächsten Reset – und vergleicht die Zeilen als
JSON (`json_agg`, gleiche Normalisierung wie Antworten, Zeitstempel ≥ Stichtag → `<ts>`).
Ohne `PARITY_SQL_DB` wird der Fall übersprungen (nie gegen die gemeinsame DB `paperless`).
`--write-only` beschränkt auf Schreibfälle.

**Private Kopie für Schreibläufe** (`tests/parity/w2-*.sh`, Daten in `tests/parity/.w2/`,
git-ignoriert): DB `pl_w2_ref` (aus eigener Vorlage `pl_w2_snapshot` ← `paperless_snapshot`),
Daten aus `reference/snapshot/data.tar`, Redis-Präfix `w2`, Django :8100, PaperRust :8101
(Binary-Kopie, eigene `routes.toml`, `PAPERRUST_BRANDING=off`). `w2_reset.py` setzt nur diese
Kopie zurück (~7–10 s; Dateien nur bei Änderung neu entpackt) und leert danach veraltete
Pool-Verbindungen von :8101. Ablauf: `w2-start.sh`, dann
`tests/parity/w2-run.sh --crate <crate>` (Bericht nach `results/parity/w2/`), `w2-stop.sh`.
Fälle mit SQL-Prüfungen: `tests/parity/<crate>/cases-w2.json` (erzeugt von `gen_cases_w2.py`).

### Leck-Prüfung (jede Antwort von A und B)

Kategorien: `password-hash` (Django-Hasher-Formate), `token`/`password` (alle bekannten
Nutzer-Geheimnisse), `secret` (`PAPERLESS_SECRET_KEY`, private Schlüssel), `path` (Repo-,
Referenz-, Medienpfade, Windows-Nutzerpfade, typische Installationspfade, `site-packages`,
`.cargo/registry`), `stacktrace` (Python-Traceback, `File "…py", line`, Rust-`panicked at`,
Backtrace, DB-Treiber-Fehler), `sql`. Binäre Körper werden nicht durchsucht (Bytes werden ohnehin
verglichen). Funde in B lassen den Lauf rot werden; Funde in A werden als upstream-Befund
gelistet. Fundstellen im Report sind maskiert (`<token:40 chars>`). Erlaubte Kategorien je Fall per
`leak_allow` (z. B. `/api/profile/` liefert das eigene `auth_token`, `/api/status/` Pfade).

### Report

`results/parity/<YYYY-MM-DD_HHMMSS>.json` (Rohdaten inkl. Diffs, Variablen, Regel-Nutzung) und
`.md` (verglichen / identisch / verschieden / nur akzeptiert / übersprungen / Fehler / Lecks; je
Crate; Routen ohne Abweichung = Kandidaten für `rust` in `routes.toml`; Abweichungen;
Lecks; Regeln; Katalog-Hinweise; Skip-Gründe; **Historie** der letzten 20 Läufe aus
`results/parity/history.jsonl`), dazu `latest.md` / `latest.json`.

### Aktivierungskriterium einer Route

Eine Route-id darf in `routes.toml` auf `rust`, wenn alle Fälle mit dieser `route` im letzten
vollständigen Lauf `identical` oder `accepted` sind, keine `error`, kein Leck in B und mindestens
die Nutzer `admin`, `user1`, `anon` und beide API-Versionen abgedeckt sind.

## 2. Basis-Fallkatalog

`python tests/parity/harness/gen_base_catalog.py` erzeugt 12 Dateien, **756 Fälle, 1686 expandiert**:

| Crate | Fälle | expandiert | Inhalt |
|---|---:|---:|---|
| pl-objects | 176 | 366 | tags/correspondents/document_types/storage_paths: Listen (page, page_size, alle `ordering_fields` ±, `name__icontains`/`iexact`/`istartswith`/`iendswith`, `full_perms`, getFew `id__in&ordering=-id&page_size=1000`, listAll `page_size=100000`, `is_root`, `last_correspondence`), Detail, 405-Proben, Fehler (Seite zu groß/ungültig/0, page_size 0/x/riesig, unbekannte Sortierung, ids ungültig), Auth-Varianten, API-Versionen |
| pl-documents | 42 | 110 | Frontend-Liste (`page=1&page_size=50&ordering=-created&truncate_content=true&include_selection_data=true`), `fields`, `full_perms`, v9 `all`, Detail inkl. `fields=id,versions`, `metadata`, `notes`, `history`, `share_links`, `root`, `suggestions`, `ai_suggestions`, `next_asn`, `selection_data` (POST, lesend) |
| pl-docfilter | 104 | 188 | jede Filtervariable aus `src-ui/src/app/data/filter-rule-type.ts` (Kommalisten wie `utils/query-params.ts`, isnull 1/0), ungültige Werte, `custom_field_query` (exists/isnull/icontains/AND/OR/NOT/unbekannt/zu tief/leer), alle Sortierfelder ±, `custom_field_<id>`, Kombination wie das Frontend |
| pl-files | 27 | 46 | download/preview/thumb (+`original`, `version`, `follow_formatting`, Session), `/share/<slug>`, `/logo/`, `/fetch/*`, bulk_download (Status+Header), Pfad-Traversal-Proben |
| pl-documents-write | 25 | 49 | `/api/trash/` Listen, Validierungsfehler aller Aktions-Routen (ohne Wirkung), 3 Schreibabläufe (PATCH Titel, bulk add_tag, Löschen → Papierkorb; nur mit Reset) |
| pl-fields-views | 107 | 258 | custom_fields, saved_views, share_links (Datumsfilter), share_link_bundles (`documents`, `status`) |
| pl-workflows | 54 | 144 | workflows, workflow_triggers, workflow_actions |
| pl-search | 18 | 33 | `/api/search/` (db_only, lang, fehlend), autocomplete (limit), `query`, `more_like_id`, Syntaxfehler, offene Datumsbereiche |
| pl-system | 59 | 146 | tasks (Filter inkl. v9 `task_name`/`type`, `status_counts`, `summary?days`, `active`), statistics, status, config, logs (+Traversal), remote_version |
| pl-mail | 63 | 162 | mail_accounts, mail_rules, processed_mail (`rule`, `status`), oauth callback |
| pl-auth-api | 65 | 162 | users, groups, profile (+totp, social providers), ui_settings, token (GET/POST falsch/leer) |
| pl-server | 16 | 22 | `/api/`, schema, favicon, assets, Index/SPA, `/accounts/login/`, unbekannte Pfade, OPTIONS/HEAD, admin, headless config |

Nutzer-Matrix: Listen `admin,user1,noperm,anon`, Details `admin,user1,user2,noperm`, Auth-Varianten
`anon,badtoken,inactive,admin@session,admin@basic`, Versionen `10,9` bzw. `none,99,abc`.
Nicht im Basiskatalog: echte Schreibaufrufe der POST-/PUT-/DELETE-Routen (Fach-Coder, `write: true`),
`post_document`/Uploads, WebSocket, `documents/chat/` (Streaming).

## 3. Sicherheits-Suite (`tests/security/`)

```
TARGET=django python tests/security/run_security.py      # Referenz
TARGET=rust   python tests/security/run_security.py      # paperless-rs
  [-k muster] [--no-ratelimit] [-v]     (SECURITY_URL überschreibt die URL)
```
Report: `results/security/<stamp>-<target>.json|.md`, `history.jsonl`. Exit 0/1, 2 = Ziel/Nutzer fehlt.

| Datei | Prüft |
|---|---|
| `test_auth.py` | anonym 401/403 auf allen API-Listen, gültiges/unbekanntes/fehlgeformtes Token (DRF-Texte), andere Schemata, Token nicht per Query, Basic gut/falsch/kaputt, inaktiver Nutzer (Token/Basic), Session-Login, falsches Passwort, gefälschtes/fehlendes Session-Cookie |
| `test_csrf.py` | Session-POST ohne/mit falschem `X-CSRFToken` und ohne CSRF-Cookie → 403 „CSRF Failed…“, mit Token → 200; Token/Basic ohne CSRF → 200; Login-Formular ohne CSRF → 403 |
| `test_permissions.py` | Sichtbarkeits-Invariante (aus Superuser-Sicht berechnet) für documents/tags/correspondents/document_types/storage_paths für `user1`/`user2`; fremde Objekte: Detail und alle Dokument-Unterrouten 403/404 ohne Titel-Leck; Superuser; `noperm` → 403; Admin-Routen (`logs`, `status`); `full_perms` erweitert nicht; Filter (`id__in`, `owner__id…`) umgehen keine Rechte; `bulk_download` fremder Dokumente; PATCH/DELETE ohne change-Recht → 403 (nur mit `reference/reset.sh` oder `SECURITY_WRITE=1`, danach Reset) |
| `test_traversal.py` | 21 Traversal-Proben auf `/logo/`, `/share/`, `/api/logs/`, `/static/`, `/assets/`, `/fetch/`, Dokument-Dateirouten (anonym + admin), Datei-Parameter, `Content-Disposition` ohne Pfadanteile |
| `test_leaks.py` | 31 feindliche Parameter, 5 feindliche Körper: kein 5xx, keine Stacktraces/Pfade/SQL; Passwortfelder maskiert (users, profile, mail_accounts); keine fremden Tokens; Fehlerseiten ohne Interna; kein Open Redirect über `next` |
| `test_headers.py` | `X-Frame-Options: SAMEORIGIN`, `nosniff`, `Referrer-Policy`/`COOP: same-origin`, kein `X-Powered-By` |
| `test_zz_ratelimit.py` | Token-Login `POST /api/token/` (Scope `login`, Standard 5/min, `SECURITY_TOKEN_RATE`) → 429 spätestens nach N+1 Versuchen, Text „Request was throttled.“, `Retry-After`; läuft zuletzt (sperrt Token-Logins von 127.0.0.1 bis zu 1 min) |

Die Suite ist datenunabhängig (Erwartungen aus der Superuser-Sicht abgeleitet), braucht aber die
Rollen aus dem Nutzerkatalog; fehlende Rollen → Test übersprungen (im Report mit Grund).

## 4. Upstream-Test-Adapter (`tests/upstream-adapter/`)

**Ergebnis der Machbarkeitsprüfung: machbar, ohne upstream-Code zu ändern; Mechanik verifiziert,
Lauf gegen paperless noch nicht möglich (Abhängigkeiten/Test-DB fehlen, s. u.).**

Umsetzung: pytest-Plugin `paperless_http_adapter.py` (`-p paperless_http_adapter`):

1. **Sichtbare Testdaten:** `django.test.TestCase._databases_support_transactions → False` – Djangos
   eigener Pfad für nicht-transaktionale Backends: `setUpTestData` pro Test, Daten werden
   **committet**, nach jedem Test `flush` (TransactionTestCase). Gilt auch für pytest-djangos
   `db`-Fixture (nutzt dieselbe Klasse).
2. **Anfragen über HTTP:** `django.test.Client.request` (auch DRF-`APIClient`) baut aus dem
   WSGI-Environ eine echte HTTP-Anfrage an `UPSTREAM_ADAPTER_TARGET` für Pfade unter
   `UPSTREAM_ADAPTER_PATHS` (Standard `/api/,/share/,/logo/,/fetch/`), mit `Host: testserver`
   (absolute URLs wie in den upstream-Assertions).
3. **Auth:** `force_authenticate(user)` → `Authorization: Token <key>` (Token-Zeile bei Bedarf
   angelegt); `force_authenticate(token=t)` → `t.key`; `force_login`/`login` → echtes
   Session-Cookie; für Session-POSTs wird ein CSRF-Cookie/Header-Paar ergänzt, außer der Client
   wurde mit `enforce_csrf_checks=True` gebaut (wie Djangos Test-Client).
4. **Antwort:** `HttpResponse` mit Status, Headern, Cookies, `.data` (geparstes JSON), `.json()`,
   `.url`, `.streaming_content`, `.client`, `.request`.

Selbsttest (`run.sh selftest`, Toy-Django-App auf SQLite-Datei + wsgiref-Thread als „Live-Server“,
`tests/.venv` mit django 5.2.18, djangorestframework 3.18.3, pytest 9.1.1 aus
`reference/downloads/wheels`): **9/9 grün** – committete Daten sichtbar, flush zwischen Tests,
force_authenticate, Reset, JSON/Multipart-POST, anonym, Session+CSRF, erzwungenes CSRF bleibt 403,
Redirect + `follow`, Nicht-API-Pfade bleiben in-process.

Ablauf gegen paperless:
```
tests/upstream-adapter/run.sh baseline        # in-process (Django), JUnit -> results/upstream-adapter/
tests/upstream-adapter/run.sh http            # über HTTP an $UPSTREAM_ADAPTER_TARGET (Std. :8080)
tests/upstream-adapter/run.sh compare         # ok / regression / fixed / both-fail / skipped
```
`regression` = in-process grün, über HTTP rot → Rust weicht ab **oder** Adapter-Grenze (Liste unten).

Voraussetzungen (nicht erfüllt, für Referenz-Agent/Koordinator):
- Python mit upstream-Laufzeit- **und** Testabhängigkeiten: `pytest-django`, `pytest-env`,
  `pytest-mock`, `pytest-httpx`, `factory-boy`, `faker`, `imagehash`, `daphne` (fehlen in
  `reference/requirements-windows.txt`; `run.sh` meldet Exit 2).
- Test-DB: pytest-django legt `test_<PAPERLESS_DBNAME>` an (`--reuse-db`). Der Live-Server
  (paperless-rs **und** der Django-Proxy-Rückfall) muss auf **diese** DB zeigen, mit
  `SECRET_KEY=test-secret-key-do-not-use-in-production` und `ALLOWED_HOSTS` inkl. `testserver`.
- Ein Prozess (`-p no:xdist`, erzwungen), feste Reihenfolge (`-p no:randomly`).

Grenzen (erklären `regression`-Einträge, die keine Rust-Abweichung sind):
- `override_settings`/`DirectoriesMixin` (temporäres `MEDIA_ROOT`, `CONSUMPTION_DIR`, Index-Pfad)
  und `mock.patch` (Celery-Aufgaben, Klassifikator, Mail, HTTP) wirken nur im Testprozess, nicht im
  Server (67 `override_settings`, 130 `mock.patch` in den `test_api_*.py`).
- Upstream-Tests setzen den MD5-Hasher (`src/conftest.py::_fast_password_hasher`); Passwort-Logins
  gegen Rust funktionieren nur, wenn Rust `md5$` prüft.
- `on_commit`-Callbacks laufen jetzt wirklich (vorher nie) → Celery-`delay` braucht Redis.
- Token-Zeilen entstehen zusätzlich (Tests, die Tokens zählen, `test_api_profile`), `force_authenticate`
  für inaktive/ungespeicherte Nutzer scheitert über HTTP.
- `.data` ist geparstes JSON statt `ReturnDict` mit Python-Objekten (`datetime`, `Decimal`,
  `ErrorDetail.code` gehen verloren); `response.context`/`templates`/`wsgi_request` leer.
- Flush statt Rollback: deutlich langsamer (jede Testmethode leert alle Tabellen).

## 5. Release-Gates (`scripts/run-gates.sh`)

`scripts/run-gates.sh [--only a,b] [--skip c]`, `GATES_OFFLINE=1` (kein Advisory-/Registry-Abruf),
`GATES_PACKAGES="-p …"` (Standard `--workspace`), `GATES_JOBS` (Standard 2). Nicht interaktiv.
Exit 0 grün, **1 mind. ein Gate rot (Vorrang)**, **2 kein Gate rot, aber Werkzeug fehlt**.
Logs: `results/gates/<stamp>/<gate>.log`, `summary.md`, `results/gates/latest.md`, `history.jsonl`.

| Gate | Befehl | Werkzeug auf diesem Rechner |
|---|---|---|
| gitleaks | `gitleaks dir . --config tests/security/gitleaks.toml --redact` | **fehlt** |
| fmt | `cargo fmt --all -- --check` | vorhanden |
| clippy | `cargo clippy --workspace --all-targets -j 2 -- -D warnings` | vorhanden |
| audit | `cargo audit --deny warnings` (`--no-fetch` offline) | vorhanden (0.22.2) |
| deny | `cargo deny check` mit `deny.toml` (Lizenzen GPL-3.0-kompatibel, nur crates.io) | vorhanden (0.20.2) |
| machete | `cargo machete crates` | vorhanden (0.9.2) |
| semgrep | `semgrep scan --config p/rust --config p/secrets --config p/owasp-top-ten --error` | **fehlt** |
| (upstream-diff) | `oasdiff` optional | **fehlt** (eingebauter Struktur-Diff als Ersatz) |

Installation (nicht ausgeführt, nur Hinweis): gitleaks-Release-Binary (github.com/gitleaks/gitleaks)
auf PATH; `pipx install semgrep` (bzw. eigenes venv); `go install github.com/oasdiff/oasdiff@latest`.

`deny.toml`: erlaubt GPL-3.0-only/-or-later, GPL-2.0-or-later, LGPL-2.1+/3.0, MIT, MIT-0,
Apache-2.0 (+LLVM-exception), BSD-2/3, ISC, Zlib, BSL-1.0, 0BSD, CC0-1.0, Unlicense, MPL-2.0,
Unicode-3.0/DFS-2016, CDLA-Permissive-2.0; bewusst nicht: GPL-2.0-only, AGPL, OpenSSL, SSPL, BUSL.
Quellen: nur crates.io, keine Git-Quellen; `wildcards = "deny"` (Pfad-Abhängigkeiten erlaubt, sobald
die Crates `publish = false` haben, s. CORE-REQUEST).

Erster Lauf (2026-11-07, ohne clippy, offline): fmt **rot** (unformatierter Code in Arbeit),
audit **rot** (RUSTSEC-2023-0071 `rsa`, RUSTSEC-2026-0253 `lru` unsound), deny **rot** (nur
`wildcard`: Workspace-Crates ohne `publish = false`; Lizenzen + Quellen grün), machete **rot**
(ungenutzte Abhängigkeiten in mehreren Crates, z. B. pl-workflows), gitleaks/semgrep fehlen.

## 6. upstream-diff (`scripts/upstream-diff.sh vALT vNEU`)

Liest nur das Git-Repo `upstream/` (`git diff`/`git show`/`git archive`), Arbeitsbaum bleibt
unverändert. Report `results/upstream-diff/<ALT>..<NEU>.md|.json`:
OpenAPI-Diff (Schemas aus `reference/schema/<tag>.json|yml` oder mit `UPSTREAM_DIFF_SCHEMA=1` per
`manage.py spectacular` aus einem `git archive`-Export, `oasdiff` falls installiert, sonst
eingebauter Diff von Operationen/Parametern/Schema-Feldern inkl. Reihenfolge), Migrationen,
geänderte Python-Dateien → Crates über `UPSTREAM_MAP.md` (+ nicht zugeordnete), geänderte
src-ui-Services/Modelle/Query-Abbildung/Interceptors/Environments.
**Rotes Gate (Exit 1):** Celery-Settings (`CELERY_*`, `paperless/celery.py`), tantivy-/whoosh-compat-Version
in `pyproject.toml`, `documents/search/_schema.py`, `documents/search/_tokenizer.py`,
`ALLOWED_VERSIONS`/`DEFAULT_VERSION`. Exit 2 bei unbekanntem Tag/fehlendem Werkzeug.
Probe: `v3.4.1 v3.4.2` → rot (Celery-Settings geändert); `v3.4.0 v3.4.2` → 412 Dateien, 13 Crates betroffen.

## 7. Browser-E2E (`tests/e2e/`, Playwright)

Fährt dieselben Klickwege im echten Browser (Chromium headless shell, Playwright `1.64.1`, fest in
`tests/e2e/package.json`) gegen **A** = unverändertes Django v3.4.2 und **B** = PaperRust und
vergleicht je Schritt. Einstieg: `tests/e2e/run.sh` (Git-Bash). Einmalig: `cd tests/e2e && npm install
&& PLAYWRIGHT_BROWSERS_PATH=.browsers npx playwright install chromium --only-shell` (nur Chromium;
`node_modules/`, `.browsers/`, `.work/` in `tests/e2e/.gitignore`).

**Umgebung (eigene, neben der Referenz):**

| | Wert |
|---|---|
| DB | `pl_e2e` = `CREATE DATABASE … TEMPLATE paperless_snapshot` (127.0.0.1:54329, Rolle `paperless`); bleibt nach dem Lauf stehen |
| Daten | `tests/e2e/.work/data/{data,media,consume,scratch}` aus `reference/snapshot/data.tar`; Logs `tests/e2e/.work/logs/` |
| Redis | Garnet der Referenz, aber eigene DB `/5` (Cache, Celery-Queue; Reset per `FLUSHDB`) |
| A | granian wie `reference/django-start.sh` auf **:8170** (`PAPERLESS_URL` :8170, CSRF-Origins :8170 + :8171), Static aus `reference/data/static` (nur gelesen) |
| B | `results/bin/paperrust-e2e.exe serve --worker` auf **:8171**, Django-Fallback :8170, `PAPERRUST_QUEUE=postgres`, `PAPERRUST_BRANDING=off` |
| B-Routen | `tests/e2e/.work/routes.toml`, beim Start generiert: alle Einträge `= "rust"` aus `routes.toml` + `documents.post_document` (abschaltbar: `E2E_UPLOAD_RUST=0`) |

Skripte: `env.sh` (gemeinsame Umgebung, sourct `reference/env.sh` und biegt DB/Ordner/Ports um),
`reset.sh` (vor **jedem** Lauf: DB neu aus Template, Datenordner neu, Garnet-DB 5 leeren),
`start-django.sh` (Logs je Start `.work/logs/django-<hhmmss>.{out,err}.log`), `start-rust.sh [off|on]`,
`stop.sh [rust|django|all]` (nur per PID aus `.work/run/*.pid`), `watchdog.sh` (startet Django während
der Klickwege neu, falls :8170 nicht mehr antwortet; Vorfälle in `.work/logs/watchdog.log` und im Bericht),
`e2e.mjs` (Klickwege; wartet vor jedem Schritt bis 180 s auf den Server und vermerkt `serverDownBefore`),
`compare.mjs` (Vergleich + Bericht), `run.sh` (alles).

**Ablauf `run.sh`:** reset → A starten → Klickwege A → reset → B (Branding aus) → Klickwege B →
B stoppen → Vergleich → reset → B mit Branding (Standard) → Branding-Screenshots → alle eigenen
Prozesse per PID beenden. A und B teilen sich die DB, laufen daher nacheinander, jeweils ab Snapshot.
Django wird vor jedem Reset gestoppt und danach neu gestartet (ein `DROP DATABASE … WITH (FORCE)` unter
laufendem granian ließ den Django-Prozess im Versuch ohne Logeintrag verschwinden).

**Browser:** 1440×900, deviceScaleFactor 1, `en-US`, Zeitzone Europe/Zurich, `reducedMotion`, helles
Farbschema, **keine** feste Browser-Uhr (`page.clock.setFixedTime` friert `Date.now` und damit RxJS-`debounceTime`
ein: Dirty-Check und Textfilter reagieren nie; A und B laufen am selben Tag), Nutzer `admin` aus
`reference/users.json`.

**Klickwege (Schritte, gleiche Liste für A und B):** Login-Seite, falsches Passwort, richtiges Passwort
(Dashboard mit Widgets), Logout, Login; Dokumentliste (Tabellenansicht, Sortieren nach Titel, umkehren,
Seite 2, Filter Tag, Filter Korrespondent, Filter zurücksetzen, Textfilter, Auswahl Seite/Alle/Keine);
gespeicherte Ansicht „Save as…“ + Liste; Dokument 1 (Details, Vorschau, Inhalt, Metadaten, Notizen,
Notiz anlegen/löschen, Titel ändern + Speichern, erneut öffnen); Tags/Korrespondenten/Dokumenttypen je
Liste/Anlegen/Umbenennen/Löschen; Einstellungen (Einträge pro Seite → Speichern); Papierkorb +
Wiederherstellen; Profil, API-Token neu, schließen; Dashboard; Upload eines kleinen PDFs
(`fixtures/e2e-upload.pdf`, erzeugt von `fixtures/make-upload-pdf.py`) **zuletzt** und nur, wo ein
Worker läuft (A: kein Celery-Worker → übersprungen und vermerkt; B: nur wenn `post_document` = rust).
Ein fehlschlagender Schritt wird vermerkt, die Folgeschritte laufen weiter.

**Erfasst je Schritt:** Screenshot (Viewport), URL, sichtbarer Text (`innerText`, zeilenweise),
sichtbare Tabellen (Zellen), Toast-Texte, API-Aufrufe, Netzfehler (HTTP ≥ 400, fehlgeschlagene Requests
außer `ERR_ABORTED`), Konsolenfehler + `pageerror`.

**Masken / Normalisierung (beidseitig gleich):** Toasts, Popover, Tooltips im Screenshot ausgeblendet
(Text separat verglichen); magenta überdeckt: Notiz-Zeitstempel, API-Token-Feld; Text: Uhrzeiten →
`<zeit>`, 40-stellige Hex-Tokens → `<token>`; Tag-Farbe beim Anlegen fest `#336699`.

**Vergleich (`compare.mjs`):** pixelmatch (Farbschwelle 0.1, ohne Antialiasing); Schritt gilt als
Pixel-abweichend ab **0,2 %** abweichender Pixel (`--threshold`). Dazu Textzeilen nur in A/nur in B,
Tabellen, Toasts, URL, Netz- und Konsolenfehler (Mengenvergleich, IDs im Pfad → `{id}`), Schrittfehler.
Netzfehler und API-Aufrufe von B werden über das Zugriffslog von B (`via="rust|django"`) und die
Startzeilen `rust route active id=…` einer Route-ID zugeordnet.

**Ausgabe:** `results/e2e/<datum>/report.md` (Tabelle aller Schritte, Details mit eingebetteten
Bildern A | B | Diff nur bei Abweichungen), `A/`, `B/`, `diff/`, `summary.json`, `routes-B.toml`,
`rust-access-B.log`, `branding/` (B mit Branding an: Login, Dashboard, Dokumentliste, Dokument,
Einstellungen).

## Known differences

- Keine Abweichung von upstream-Verhalten (Test-Infrastruktur).
- Normalisierung `vary` (Token-Reihenfolge) ist semantisch neutral, aber keine reine
  Flüchtigkeits-Normalisierung – bewusst, da HTTP die Reihenfolge nicht festlegt.

**Branding und Parität (2026-11-08):** PaperRust ist standardmäßig gebrandet (`PAPERRUST_BRANDING`
Standard `on`: Style-Injektion in HTML-Navigationen, `app_title`/`app_logo` = PaperRust in der Rust-Route
`ui_settings.get`, Assets unter `/__rs/brand/`). Für **jeden Paritätslauf** muss B mit
`PAPERRUST_BRANDING=off` laufen (dann ist der Server byte-transparent wie vorher; per Test
`tests/branding.rs::off_is_byte_transparent` belegt) und weder A noch B dürfen `PAPERLESS_APP_TITLE`/
`PAPERLESS_APP_LOGO` gesetzt haben – nur so bleibt der Harness bei 0 Unterschieden. Das bisherige
Start-Skript `results/bin/start-rs.sh` setzt die Variable noch nicht (alte Binaries kennen kein Branding);
vor dem nächsten Neubau von B dort `export PAPERRUST_BRANDING=off` ergänzen.
