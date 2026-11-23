# PaperRust – Alle Einstellungen in der Oberfläche

Stand 2026-11-09. Entscheidungsvorlage. Gehört zu docs/ARCHITECTURE-V2.md (v1-Compat / v2-nativ),
docs/FRONTEND-LIGHT.md (Light-UI), docs/DESKTOP.md (Supervisor, `desktop.toml`), docs/REMOTE-APP.md
(`app.toml`), docs/user/configuration.md (Referenz für Nutzer, bleibt die Detailbeschreibung jeder
Variablen).

**Ziel:** Alles, was sich sinnvoll zur Laufzeit einstellen lässt, wird in der Oberfläche einstellbar.
Bestehende Umgebungsvariablen, `docker-compose.env` und `paperless.conf` funktionieren unverändert weiter
(Umstieg von paperless-ngx), die v1-API (`/api/config/`, `/api/ui_settings/`) bleibt paritätsgleich.

---

## 0. Entscheidungen auf einen Blick

| # | Entscheidung | Kurzbegründung |
|---|---|---|
| E1 | **Vier Klassen** A (live), B (mit Neustart), C (Bootstrap), D (bewusst nur Umgebung) + X (wirkungslos in PaperRust, nur Anzeige). Jeder Schlüssel ist genau einer Klasse zugeordnet (Abschnitt 1). | Klare Erwartung, was der Knopf „Speichern“ bewirkt. |
| E2 | **Vorrang für neue GUI-Schlüssel:** Umgebung > Datenbank (`paperrust_setting`) > Konfigurationsdatei > Standard. Ein über die Umgebung gesetzter Wert ist in der GUI **gesperrt** („gesetzt über Umgebung – hier nicht änderbar“). | Wer heute per Compose/Env konfiguriert, behält die Kontrolle; Rettungsweg bei Fehleinstellung: Variable setzen + Neustart. |
| E3 | **ApplicationConfiguration bleibt die einzige Quelle** für ihre 45 Felder, mit **upstream-Vorrang** (DB-Wert > Umgebung, Python-`or`-Semantik). Keine Kopie in `paperrust_setting`, keine Sperre durch Umgebung. v1 `/api/config/`, Angular-Seite „Konfiguration“ und v2 lesen/schreiben **dieselbe Zeile**. | v1-Parität, keine Divergenz, Umsteiger behalten ihre DB-Werte. |
| E4 | Bootstrap (C) ist nur dort in der GUI schreibbar, wo ein Aufseher die Bootstrap-Datei schreibt: **Desktop** (`desktop.toml` über den Supervisor, Anfrage-Datei `run/settings.request.json`, nur von Loopback). Server/Docker: Anzeige mit Hinweis „über Umgebung setzen“. | DB-Zugang, SECRET_KEY, Pfade, Bind sind vor der DB nötig. |
| E5 | Klasse D (nur Umgebung): **Vertrauensgrenze zum Host** – Proxy-/Header-Vertrauen, Remote-User, Auto-Login, SSRF-Schalter (`*_ALLOW_INTERNAL_*`, Webhook-Ports/Schemata), Vor-/Nach-Verarbeitungsskripte, DEBUG. | Ein übernommenes Admin-Konto darf weder Code auf dem Host ausführen noch ins interne Netz greifen noch Anmeldung umgehen. Host-Admin ≠ App-Admin. |
| E6 | **Geheimnisse** in `paperrust_setting` werden **verschlüsselt** (XChaCha20-Poly1305, Schlüssel per HKDF-SHA256 aus `SECRET_KEY`, AAD = Schlüsselname). API ist **write-only** (nur `is_set`). AC-Geheimnisse (`llm_api_key`, `llm_embedding_api_key`, `remote_ocr_api_key`) und Mail-Konto-Passwörter bleiben **Klartext wie upstream** (v1/Django lesen sie). | Neue Tabelle hat keine Compat-Pflicht; DB-Dumps sind oft schwächer geschützt als die Umgebung. |
| E7 | Werte werden als **kanonischer Env-String** gespeichert und mit **denselben Parsern** wie `pl_core::config` geprüft (ein Schema-Register `pl-settings`). API nimmt/liefert typisiertes JSON. | Gleiche Semantik Env ↔ GUI, Export als `.env` trivial. |
| E8 | **Live-Übernahme:** Schnappschuss `ArcSwap<Settings>` je Prozess, Änderung → `pg_notify('paperrust_settings', rev)`, alle Prozesse laden neu (Abfrage-Rückfall 30 s). Laden unter geteilter Advisory-Sperre (gegen die exklusive beim Schreiben), weil der Stand aus einer nicht transaktionalen Sequenz kommt – sonst Schnappschuss mit neuem Stand und alten Werten (Gesamtprüfung 2026-11-18). Teilsysteme der Klasse B-W (Eingangsordner-Überwachung, Zeitplaner, Worker-Pool) startet der Prozess selbst neu; Klasse B-S setzt `restart_pending`. | Kein Polling je Anfrage, mehrere Prozesse konsistent. |
| E9 | **Neustart aus der GUI:** `POST /rs/api/v2/settings/restart` beendet geordnet mit **Exit-Code 75**; Docker-Entrypoint, Desktop-Supervisor und systemd-Vorlage (`RestartForceExitStatus=75`) starten neu. Ohne Aufseher nur Hinweis. | Ein Mechanismus für alle Betriebsarten. |
| E10 | **API:** `/rs/api/v2/settings` (Liste mit Schema/Herkunft, Einzelwert, PATCH atomar mit `expected_rev`, DELETE = Standard, Verlauf, Export/Import, Umzugshilfe). Nur Superuser; AC-Schlüssel zusätzlich mit derselben Berechtigung wie v1 `/api/config/`. ui_settings bleiben pro Nutzer. | Schmal, versioniert, kein Einfluss auf v1. |
| E11 | **GUI:** Light-UI, Reiter **„System“** in `admin/settings` (Kategorien, Suche, Abzeichen „Neustart nötig“, „aus Umgebung“, „überschreibt Umgebung“, „Geheimnis“, „Desktop“). Angular bekommt per Patch nur einen Link/Hinweis; seine Seite „Konfiguration“ bleibt. | Kein Umbau der Angular-UI. |
| E12 | **Umzug:** Ansicht „Herkunft“ + „in Datenbank übernehmen“ (pro Schlüssel / alle GUI-fähigen) + erzeugte **schlanke `docker-compose.env`** (nur C/D). `paperless.conf`/Compose-Dateien können hochgeladen und geprüft werden. | Umsteiger können ihre Umgebung schrittweise entschlacken. |

Zählung (Abschnitt 1, Systemeinstellungen ohne pro-Nutzer-Werte): **A 150 · B 34 · C 83 · D 23 · X 37**
(gesamt 327; maschinell geprüft vom Test `inventory_complete` in `crates/pl-settings`). Dazu 44 pro-Nutzer-ui_settings-Schlüssel (Abschnitt 1.17) und 10 Arten von DB-Objekten,
die schon in der Oberfläche gepflegt werden (Abschnitt 1.18).

---

## 1. Inventar

### Legende

- **Schlüssel:** upstream-Variablen ohne Präfix geschrieben, also `DBHOST` = `PAPERLESS_DBHOST`
  (gleichwertig `PAPERRUST_DBHOST`, Übergangsname `PAPERLESS_RS_*` nur für eigene). Eigene Variablen
  mit vollem Namen `PAPERRUST_…`. Neu vorgeschlagene Schlüssel sind mit *(neu)* markiert.
- **Quelle (heute):** `E` Umgebung/`paperrust.conf` (bzw. `paperless.conf`) · `AC:<feld>` ApplicationConfiguration
  (`paperless_applicationconfiguration`, überschreibt `E`) · `DT` `desktop.toml` (Supervisor) ·
  `DE` vom Desktop-Supervisor als Umgebung gesetzt (`apps/desktop/src/server.rs: server_env`) ·
  `AT` `app.toml` (Desktop-Hülle) · `DI` Docker-Entrypoint · `K` fest im Code.
- **Wirksam:** `L` live (nächste Anfrage/Aufgabe) · `W` Neustart eines Teilsystems im Prozess
  (Worker-Pool, Zeitplaner, Ordnerüberwachung – macht der Prozess selbst) · `S` Neustart des
  Server-Prozesses · `1` nur beim ersten Start · `+I` zusätzlich Neuaufbau (Index/Dateinamen) nötig.
- **Geh.:** Geheimnis (write-only, verschlüsselt bzw. maskiert).
- **Kl.:** A GUI live · B GUI mit Neustart · C Bootstrap · D nur Umgebung · X wirkungslos in
  PaperRust (paperless-ngx-/Docker-upstream-Reste), nur angezeigt.
- Kategorie = Unterabschnitt. Standardwerte aus upstream v3.4.2 (`src/paperless/settings/*.py`) bzw.
  dem Rust-Code. Bedeutung ausführlich: docs/user/configuration.md.

### 1.1 Datenbank, Geheimschlüssel, Start

| Schlüssel | Bedeutung | Typ / Werte | Standard | Quelle | Wirksam | Geh. | Kl. |
|---|---|---|---|---|---|---|---|
| `CONFIGURATION_PATH` | Pfad der Konfigurationsdatei | Pfad | Suchliste `../paperrust.conf` … | E | S | nein | C |
| `DBENGINE` | DB-Art | `postgresql` (nur das; sqlite/mariadb → Umzug) | postgresql wenn `DBHOST` | E, DE | S | nein | C |
| `DBHOST` | DB-Host | Host/Socket | – | E, DE | S | nein | C |
| `DBPORT` | DB-Port | int | 5432 | E, DE | S | nein | C |
| `DBNAME` | DB-Name | str | paperless | E, DT | S | nein | C |
| `DBUSER` | DB-Nutzer | str | paperless | E, DT | S | nein | C |
| `DBPASS` | DB-Passwort | str | paperless | E, DT | S | ja | C |
| `DBSSLMODE` | TLS-Modus | disable…verify-full | prefer | E, DE | S | nein | C |
| `DBSSLROOTCERT` | CA-Datei | Pfad | – | E | S | nein | C |
| `DBSSLCERT` | Client-Zertifikat | Pfad | – | E | S | nein | C |
| `DBSSLKEY` | Client-Schlüssel | Pfad | – | E | S | nein | C |
| `DB_OPTIONS` | zusätzliche Verbindungsoptionen | `k=v,k=v` | – | E | S | nein | C |
| `DB_POOLSIZE` | Poolgröße | int | – (→ `PAPERRUST_DB_MAX_CONNECTIONS`) | E | S | nein | C |
| `DB_TIMEOUT` | Verbindungs-Timeout | int s | – | E | S | nein | C |
| `PAPERRUST_DB_MAX_CONNECTIONS` | Poolgröße ohne `DB_POOLSIZE` | int | 10 | E | S | nein | C |
| `PAPERRUST_DB_ACQUIRE_TIMEOUT` | Wartezeit auf eine freie DB-Verbindung (s), Vorrang vor `DB_TIMEOUT` (tests/stress/BEFUNDE.md B2) | int ≥ 1 | 30 (sqlx) | E | S | nein | C |
| `PAPERRUST_DB_LONGPOLL_CONNECTIONS` | eigene DB-Verbindungen (Nebenpool) für wartende Sync-Clients (Long-Poll, B2) | int ≥ 1 | 8 | E | S | nein | C |
| `PAPERRUST_DB_REQUIRE_AUTH` | erlaubte DB-Anmeldeverfahren | z. B. `scram-sha-256` | keine Prüfung | E, DE | S | nein | D |
| `SECRET_KEY` | Django-/Sitzungs-Schlüssel, Basis der Settings-Verschlüsselung | str ≠ `change-me` | – (Pflicht) | E, DT | S | ja | C |
| `REDIS` | Redis-URL von paperless-ngx – wirkungslos, PaperRust braucht kein Redis (gesetzt → Hinweis im Log) | URL | – | E | – | ja¹ | X |
| `REDIS_PREFIX` | Schlüsselpräfix Redis (wirkungslos) | str | – | E | – | nein | X |
| `PAPERRUST_CACHE` | Cache: lokal oder gemeinsam in PostgreSQL (Mehrprozessbetrieb, Tabelle `paperrust_cache`) | `local`/`postgres` | local | E | S | nein | B |
| `PAPERRUST_INSTANCE_ID` | Instanzkennung in `/__rs/health` | str | – | E | S | nein | B |
| `PAPERRUST_INIT` | Entrypoint-Init (Migration, Superuser) ausführen | bool | yes | DI | 1 | nein | C |
| `PAPERRUST_INIT_DB_TIMEOUT` | Warten auf DB im Entrypoint | s | 300 | DI | 1 | nein | C |
| `ADMIN_USER` | Superuser beim ersten Start anlegen | str | – | E, DI | 1 | nein | C |
| `ADMIN_PASSWORD` | dessen Passwort | str | – | E, DI | 1 | ja | C |
| `ADMIN_MAIL` | dessen E-Mail | str | root@localhost | E, DI | 1 | nein | C |
| `DEBUG` | Debug-Modus (Django) | bool | no | E | S | nein | D |
| `DB_READ_CACHE_ENABLED` | Django-Lesecache (cachalot) | bool | no | E | – | nein | X |
| `READ_CACHE_TTL` | dessen TTL | int s | 3600 | E | – | nein | X |
| `READ_CACHE_REDIS_URL` | dessen Redis | URL | = `REDIS` | E | – | nein | X |
| `CACHE_BACKEND` | Django-Cache-Klasse; `…RedisCache`/`…DatabaseCache` → wie `PAPERRUST_CACHE=postgres` (Redis wird nie verbunden) | Python-Pfad | – | E, DE | – | nein | X |
| `CHANNELS_BACKEND` | Django-Channels-Klasse | Python-Pfad | RedisPubSub | E | – | nein | X |
| `APPS` | zusätzliche Django-Apps | Liste | – | E | – | nein | X |

¹ Redis-URL kann ein Passwort enthalten → wie Geheimnis maskiert.

### 1.2 Pfade und Installationslayout

| Schlüssel | Bedeutung | Typ | Standard | Quelle | Wirksam | Geh. | Kl. |
|---|---|---|---|---|---|---|---|
| `PAPERRUST_BASE_DIR` | Installationswurzel | Pfad | Arbeitsverzeichnis | E | S | nein | C |
| `DATA_DIR` | Daten (Index, Modell, Logs) | Pfad | BASE/data | E, DE | S | nein | C |
| `MEDIA_ROOT` | Originale/Archiv/Vorschauen | Pfad | BASE/media | E, DE | S | nein | C |
| `CONSUMPTION_DIR` | Eingangsordner | Pfad | BASE/consume | E, DE | S | nein | C |
| `SCRATCH_DIR` | Arbeitsordner | Pfad | /tmp/paperless | E, DE | S | nein | C |
| `STATICDIR` | statische Dateien | Pfad | BASE/static | E, DE | S | nein | C |
| `LOGGING_DIR` | Logordner | Pfad | DATA/log | E, DE | S | nein | C |
| `TRASH_DIR` (alt: `EMPTY_TRASH_DIR`) | Ordner für gelöschte Dateien | Pfad | – (löschen) | E | S | nein | C |
| `MODEL_FILE` | Pfad Klassifikator (upstream) | Pfad | DATA/classification_model.pickle | E | S | nein | C |
| `PAPERRUST_CLASSIFIER_MODEL_FILE` | Pfad Rust-Klassifikator | Pfad | DATA/paperrust_classifier.model | E | S | nein | C |
| `PAPERRUST_ROUTES` | `routes.toml` | Pfad | ./routes.toml | E, DE | S | nein | C |
| `PAPERRUST_TESSERACT_PATH` | tesseract-Programm | Pfad | PATH | E, DE | S | nein | C |
| `PAPERRUST_TESSERACT_LIB` | libtesseract (Datei/Ordner, `off`) | Pfad/off | Suche | E | S | nein | C |
| `PAPERRUST_TESSDATA_DIR` | tessdata | Pfad | – | E, DE | S | nein | C |
| `PAPERRUST_PDFIUM_PATH` | PDFium-Bibliothek | Pfad | Suche | E, DE | S | nein | C |
| `PAPERRUST_LIGHT_UI_DIR` | Dateien der Light-UI | Pfad | STATICDIR/../light | E | S | nein | C |
| `THUMBNAIL_FONT_NAME` | Schrift für Text-Vorschaubilder | Pfad .ttf | /usr/share/fonts/liberation/LiberationSerif-Regular.ttf | E | L | nein | B |
| `EMAIL_CERTIFICATE_LOCATION` | CA-Datei für IMAP | Pfad | – | E | W | nein | B |

### 1.3 Netz, Webserver, Proxy, CORS/CSRF

| Schlüssel | Bedeutung | Typ | Standard | Quelle | Wirksam | Geh. | Kl. |
|---|---|---|---|---|---|---|---|
| `PAPERRUST_BIND` | Adresse:Port | `ip:port` | 127.0.0.1:8080 (Docker 0.0.0.0:8000) | E, DE, DI | S | nein | C |
| `BIND_ADDR` / `PORT` | upstream-granian; Entrypoint baut daraus `PAPERRUST_BIND` | ip / int | `::` / 8000 | DI | S | nein | C |
| `WEBSERVER_WORKERS` | granian-Prozesse | int | 1 | DI | – | nein | X |
| `PAPERRUST_DJANGO_URL` | entfernt: Weiterleitung an Django gibt es nicht mehr (gesetzt → Warnung beim Start) | – | – | E | – | nein | X |
| `PAPERRUST_STOP_ON_STDIN_EOF` | Ende bei stdin-EOF (Supervisor) | bool | no | E (Supervisor) | S | nein | C |
| `FORCE_SCRIPT_NAME` | Unterpfad hinter Proxy | `/pfad` | – | E | S | nein | C |
| `URL` | öffentliche Adresse (CSRF, Links, OAuth) | URL | – | E, DE | S | nein | B |
| `ALLOWED_HOSTS` | erlaubte Host-Köpfe | Liste | `*` | E, DE | L | nein | B |
| `CSRF_TRUSTED_ORIGINS` | vertrauenswürdige Origins | Liste URL | – | E | L | nein | B |
| `CORS_ALLOWED_HOSTS` | CORS-Origins | Liste URL | http://localhost:8000 | E | L | nein | B |
| `COOKIE_PREFIX` | Präfix Sitzungs-/CSRF-Cookie | str | "" | E | S | nein | B |
| `SESSION_COOKIE_AGE` | Sitzungsdauer | s | 1 814 400 (3 Wochen) | E | L | nein | A |
| `LOGOUT_REDIRECT_URL` | Ziel nach Abmeldung | URL | /accounts/login/?loggedout=1 | E | L | nein | A |
| `STATIC_URL` | URL statischer Dateien | URL | BASE_URL+static/ | E | – | nein | X |
| `ENABLE_COMPRESSION` | Antwortkompression | bool | yes | E | L | nein | A |
| `TOKEN_THROTTLE_RATE` | Login-/Token-Drosselung | `n/min` | 5/min | E | L | nein | A |
| `TRUSTED_PROXIES` | vertrauenswürdige Proxys (Remote-User, IP) | Liste IP/CIDR | – | E | S | nein | D |
| `ALLAUTH_TRUSTED_PROXY_COUNT` | Proxy-Anzahl für Client-IP | int ≥ 0 | len(TRUSTED_PROXIES) | E | S | nein | D |
| `ALLAUTH_TRUSTED_CLIENT_IP_HEADER` | Kopf mit Client-IP | Header | – | E | S | nein | D |
| `USE_X_FORWARD_HOST` | X-Forwarded-Host vertrauen | bool | false | E | S | nein | D |
| `USE_X_FORWARD_PORT` | X-Forwarded-Port vertrauen | bool | false | E | S | nein | D |
| `PROXY_SSL_HEADER` | Kopf für „ist HTTPS“ | JSON-Liste | – | E | S | nein | D |
| `PAPERRUST_DOCS_URL` | Basis der Hilfe-Links (statt `/rs/docs/`) | URL mit `/` | eingebaut | E | L | nein | A |
| `PAPERRUST_SHARE_MODE` | Freigabe-Hinweis im Desktop | `local`/`lan` | – | DE (aus `lan_sharing`) | S | nein | C |
| `PAPERRUST_SHARE_URL` | Adresse in Freigabelinks (Desktop-LAN) | URL | = `URL` | E | S | nein | B |
| `PAPERRUST_API_V2` *(in ARCHITECTURE-V2 vorgesehen)* | v2-API abschalten | `on`/`off` | on | E | S | nein | B |
| `PAPERRUST_LIGHT_UI` | Light-UI `/rs/app/` abschalten | `on`/`off` | on | E | S | nein | B |
| `PAPERRUST_SMTP_INTAKE` | SMTP-Empfang nur für Geräte (Scan-to-Email, pl-scan, docs/user/scanner.md); aus = es lauscht nichts | `on`/`off` | off | E | S | nein | B |
| `PAPERRUST_SMTP_INTAKE_LISTEN` | SMTP-Empfang: Adresse und Port | `ip:port` | 0.0.0.0:2525 | E | S | nein | B |
| `PAPERRUST_SMTP_INTAKE_MAX_MB` | SMTP-Empfang: größte Nachricht | MB 1–1024 | 50 | E | S | nein | B |
| `PAPERRUST_SMTP_INTAKE_ALLOWED_IPS` | SMTP-Empfang: erlaubte Absender (IP/CIDR, Komma/Leerzeichen); leer = alle | Liste IP/CIDR | – | E | S | nein | B |
| `PAPERRUST_SMTP_INTAKE_RATE_PER_MINUTE` | SMTP-Empfang: Nachrichten je Minute und Gerät | int 1–10000 | 30 | E | S | nein | B |
| `PAPERRUST_SMTP_INTAKE_REQUIRE_TLS` | SMTP-Empfang: Anmeldung erst nach STARTTLS (braucht Zertifikat) | `on`/`off` | off | E | S | nein | B |
| `PAPERRUST_SMTP_INTAKE_IMPLICIT_TLS` | SMTP-Empfang: direktes TLS ab dem ersten Byte (SMTPS, üblich Port 465) statt STARTTLS (braucht Zertifikat) | `on`/`off` | off | E | S | nein | B |
| `PAPERRUST_SMTP_INTAKE_TLS_CERT` / `_TLS_KEY` | SMTP-Empfang: Zertifikat und Schlüssel (PEM) für STARTTLS; ohne = nur im vertrauenswürdigen LAN | Pfad | – | E | S | nein | D |

### 1.4 Anmeldung, Konten, SSO/OIDC, Remote-User

| Schlüssel | Bedeutung | Typ | Standard | Quelle | Wirksam | Geh. | Kl. |
|---|---|---|---|---|---|---|---|
| `ACCOUNT_ALLOW_SIGNUPS` | Selbstregistrierung | bool | no | E | L | nein | A |
| `ACCOUNT_DEFAULT_GROUPS` | Gruppen neuer Nutzer | Liste Gruppen | – | E | L | nein | A |
| `ACCOUNT_DEFAULT_HTTP_PROTOCOL` | Protokoll in Mail-Links | http/https | https | E | L | nein | A |
| `ACCOUNT_EMAIL_VERIFICATION` | E-Mail-Bestätigung | none/optional/mandatory | optional | E | L | nein | A |
| `ACCOUNT_EMAIL_UNKNOWN_ACCOUNTS` | Mails an unbekannte Adressen (Passwort-Reset) | bool | true | E | L | nein | A |
| `ACCOUNT_SESSION_REMEMBER` | „angemeldet bleiben“ | bool | true | E | L | nein | A |
| `SOCIALACCOUNT_PROVIDERS` | OIDC/OAuth-Anbieter inkl. Client-Secrets | JSON | {} | E | S | ja | B |
| `SOCIALACCOUNT_ALLOW_SIGNUPS` | Registrierung über SSO | bool | yes | E | L | nein | A |
| `SOCIAL_AUTO_SIGNUP` | SSO-Konto ohne Formular anlegen | bool | no | E | L | nein | A |
| `SOCIAL_ACCOUNT_DEFAULT_GROUPS` | Gruppen neuer SSO-Nutzer | Liste | – | E | L | nein | A |
| `SOCIAL_ACCOUNT_SYNC_GROUPS` | Gruppen aus Claim abgleichen | bool | no | E | L | nein | A |
| `SOCIAL_ACCOUNT_SYNC_GROUPS_CLAIM` | Name des Claims | str | groups | E | L | nein | A |
| `SOCIAL_ACCOUNT_SYNC_SUPERUSER_GROUP` | Gruppe → Superuser | str | – | E | L | nein | A |
| `SOCIAL_ACCOUNT_SYNC_STAFF_GROUP` | Gruppe → Staff | str | – | E | L | nein | A |
| `SOCIAL_ACCOUNT_MFA_TRUSTED_AMR` | AMR-Werte, die als MFA gelten | Liste | mfa | E | L | nein | A |
| `DISABLE_REGULAR_LOGIN` | Passwort-Login abschalten | bool | no | E | L | nein | A² |
| `REDIRECT_LOGIN_TO_SSO` | Login-Seite direkt zum SSO | bool | no | E | L | nein | A² |
| `AUTO_LOGIN_USERNAME` | ohne Anmeldung als Nutzer X | str | – | E | S | nein | D |
| `ENABLE_HTTP_REMOTE_USER` | Remote-User-Kopf für die UI | bool | no | E | S | nein | D |
| `ENABLE_HTTP_REMOTE_USER_API` | Remote-User-Kopf für die API | bool | no | E | S | nein | D |
| `HTTP_REMOTE_USER_HEADER_NAME` | Name des Kopfs | str | HTTP_REMOTE_USER | E | S | nein | D |
| *Passwortregeln* (`AUTH_PASSWORD_VALIDATORS`) | Mindestlänge, Ähnlichkeit, häufige, nur-Ziffern | – | Django-Standard (4 Validatoren) | K | L | nein | A *(neu: `PAPERRUST_PASSWORD_MIN_LENGTH`, Standard 8)* |
| *MFA* (`MFA_TOTP_ISSUER`) | Aussteller im TOTP | str | Paperless-ngx (PaperRust: „PaperRust“) | K | L | nein | A *(neu: `PAPERRUST_MFA_TOTP_ISSUER`)* |
| *API-Token / Geräte-Token* | Token je Nutzer bzw. Gerät | – | – | DB-Objekt (Abschnitt 1.18) | – | – | – |

² Schutz vor Aussperren: Speichern wird abgelehnt, solange `SOCIALACCOUNT_PROVIDERS` keinen Anbieter
enthält bzw. der speichernde Superuser kein verknüpftes SSO-Konto hat.

### 1.5 E-Mail-Versand (SMTP) und Benachrichtigungen

| Schlüssel | Bedeutung | Typ | Standard | Quelle | Wirksam | Geh. | Kl. |
|---|---|---|---|---|---|---|---|
| `EMAIL_HOST` | SMTP-Server | Host | localhost | E | L | nein | A |
| `EMAIL_PORT` | SMTP-Port | int | 25 | E | L | nein | A |
| `EMAIL_HOST_USER` | SMTP-Nutzer | str | "" | E | L | nein | A |
| `EMAIL_HOST_PASSWORD` | SMTP-Passwort | str | "" | E | L | ja | A |
| `EMAIL_FROM` | Absender | Adresse | = `EMAIL_HOST_USER` | E | L | nein | A |
| `EMAIL_USE_TLS` | STARTTLS | bool | no | E | L | nein | A |
| `EMAIL_USE_SSL` | implizites TLS | bool | no | E | L | nein | A |
| *Betreffpräfix* (`EMAIL_SUBJECT_PREFIX`) | Präfix | str | „[Paperless-ngx] “ | K | L | nein | A *(neu: `PAPERRUST_EMAIL_SUBJECT_PREFIX`)* |
| *Test-Mail* | Knopf „Testnachricht senden“ | Aktion | – | – | – | – | – |

Benachrichtigungen im Browser (Einlesen erfolgreich/fehlgeschlagen) sind pro Nutzer (1.17).

### 1.6 Mail-Abruf (paperless_mail)

| Schlüssel | Bedeutung | Typ | Standard | Quelle | Wirksam | Geh. | Kl. |
|---|---|---|---|---|---|---|---|
| `EMAIL_TASK_CRON` | Abrufzeitplan | Crontab/`disable` | `*/10`, Versatz aus SECRET_KEY | E | W | nein | A |
| `EMAIL_ALLOW_INTERNAL_HOSTS` | IMAP zu internen Adressen | bool | true | E | L | nein | D |
| `EMAIL_PARSE_DEFAULT_LAYOUT` | Standard-Layout Mail→PDF (Regeln ohne eigenes Layout) | 1–4 | 1 | E | L | nein | A |
| `ENABLE_GPG_DECRYPTOR` | GPG-Mails entschlüsseln (pl-mail `fetch::gpg`, braucht `gpg`) | bool | no | E | S | nein | B |
| `EMAIL_GNUPG_HOME` | GnuPG-Ordner mit den privaten Schlüsseln | Pfad | – | E | S | nein | D |
| `OAUTH_CALLBACK_BASE_URL` | Rückruf-Basis Gmail/Outlook | URL | = `URL` | E | L | nein | A |
| `GMAIL_OAUTH_CLIENT_ID` | Gmail-OAuth-Client | str | – | E | L | nein | A |
| `GMAIL_OAUTH_CLIENT_SECRET` | dessen Secret | str | – | E | L | ja | A |
| `OUTLOOK_OAUTH_CLIENT_ID` | Outlook-OAuth-Client | str | – | E | L | nein | A |
| `OUTLOOK_OAUTH_CLIENT_SECRET` | dessen Secret | str | – | E | L | ja | A |

Konten und Regeln selbst: DB-Objekte (1.18).

### 1.7 Einlesen (Consumer)

| Schlüssel | Bedeutung | Typ | Standard | Quelle | Wirksam | Geh. | Kl. |
|---|---|---|---|---|---|---|---|
| `CONSUMER_POLLING_INTERVAL` | Abfrageintervall Eingangsordner (0 = Ereignisse) | float s | 0 | E | W | nein | A |
| `CONSUMER_STABILITY_DELAY` | Wartezeit, bis Datei stabil („inotify delay“) | float s | 5 | E | L | nein | A |
| `CONSUMER_RECURSIVE` | Unterordner einlesen | bool | no | E | W | nein | A |
| `CONSUMER_SUBDIRS_AS_TAGS` | Unterordner als Tags | bool | no | E | L | nein | A |
| `CONSUMER_IGNORE_PATTERNS` | Dateinamen-Muster ignorieren | JSON-Liste Regex | [] (+eingebaute) | E | L | nein | A |
| `CONSUMER_IGNORE_DIRS` | Ordner ignorieren | JSON-Liste | [] | E | L | nein | A |
| `CONSUMER_DELETE_DUPLICATES` | Duplikate löschen | bool | no | E | L | nein | A |
| `CONSUMER_ENABLE_COLLATE_DOUBLE_SIDED` | Doppelseiten zusammenführen | bool | no | E | L | nein | A |
| `CONSUMER_COLLATE_DOUBLE_SIDED_SUBDIR_NAME` | Unterordnername dafür | str | double-sided | E | L | nein | A |
| `CONSUMER_COLLATE_DOUBLE_SIDED_TIFF_SUPPORT` | TIFF dabei zulassen | bool | no | E | L | nein | A |
| `CONSUMER_DISABLE` | Eingangsordner-Überwachung aus (jeder nicht leere Wert, wie upstream) | defined | – | E | W | nein | B |
| `PRE_CONSUME_SCRIPT` | Skript vor dem Einlesen | Pfad | – | E | L | nein | D |
| `POST_CONSUME_SCRIPT` | Skript nach dem Einlesen | Pfad | – | E | L | nein | D |
| `MAX_IMAGE_PIXELS` | Pixelgrenze Bilder (Dekompressionsbombe) | int | Pillow-Standard | E | L | nein | A |
| `PAPERRUST_SHRINK_ON_INGEST_MB` *(neu)* | Scans (kaum Text) über so vielen MB beim Einlesen verkleinern; gespeichert wird die kleinere Datei (pl-pdftools) | int ≥ 0 | 0 (aus) | E | L | nein | A |
| `PAPERRUST_SHRINK_PRESET` *(neu)* | Stufe beim Verkleinern: `lossless`, `standard` (150 dpi, JPEG 60), `strong` (96 dpi) | Auswahl | standard | E | L | nein | A |
| `PAPERRUST_PDFTOOL_TIMEOUT` *(neu)* | Zeitgrenze je Lauf des PDF-Werkzeugs (Schwärzen, Verkleinern, Reparieren) | int s ≥ 1 | 120 | E | L | nein | A |
| `PAPERRUST_PDFTOOL_MEMORY_MB` *(neu)* | Speichergrenze des PDF-Werkzeugs (Kindprozess) | int MB ≥ 0 | 1536 | E | L | nein | A |
| `PAPERRUST_OFFICE_TO_PDF` *(neu)* | Office-Dateien (docx/doc, odt, rtf, xlsx/xls/ods, pptx/ppt/odp/odg) und Mails beim Einlesen mit WordCraft (Kindprozess `paperrust-pdftool`) in ein Archiv-PDF umwandeln; Vorschaubild dann von Seite 1 | bool | true | E | L | nein | A |
| `PAPERRUST_OFFICE_TO_PDF_TIMEOUT` *(neu)* | Zeitgrenze je Office-Umwandlung (0 = wie `PAPERRUST_PDFTOOL_TIMEOUT`) | int s ≥ 0 | 0 | E | L | nein | A |
| `PAPERRUST_PDFTOOL_BIN` *(neu)* | Pfad zu `paperrust-pdftool` | Pfad | neben dem Server-Programm | E | S | nein | C |

### 1.8 Barcodes und Aufteilen (ApplicationConfiguration-Felder)

Alle: Quelle `E` + `AC`, Vorrang AC (E3), wirksam L, kein Geheimnis, Klasse A.

| Schlüssel | AC-Feld | Bedeutung | Typ | Standard | Kl. |
|---|---|---|---|---|---|
| `CONSUMER_ENABLE_BARCODES` | barcodes_enabled | Trennblatt-Barcodes | bool | no | A |
| `CONSUMER_BARCODE_TIFF_SUPPORT` | barcode_enable_tiff_support | auch TIFF | bool | no | A |
| `CONSUMER_BARCODE_STRING` | barcode_string | Text des Trennblatts | str | PATCHT | A |
| `CONSUMER_BARCODE_RETAIN_SPLIT_PAGES` | barcode_retain_split_pages | Trennseite behalten | bool | no | A |
| `CONSUMER_ENABLE_ASN_BARCODE` | barcode_enable_asn | ASN aus Barcode | bool | no | A |
| `CONSUMER_ASN_BARCODE_PREFIX` | barcode_asn_prefix | ASN-Präfix | str | ASN | A |
| `CONSUMER_BARCODE_UPSCALE` | barcode_upscale | Hochskalieren | float | 0.0 | A |
| `CONSUMER_BARCODE_DPI` | barcode_dpi | Render-DPI | int | 300 | A |
| `CONSUMER_BARCODE_MAX_PAGES` | barcode_max_pages | max. gescannte Seiten (0 = alle) | int | 0 | A |
| `CONSUMER_ENABLE_TAG_BARCODE` | barcode_enable_tag | Tags aus Barcode | bool | no | A |
| `CONSUMER_TAG_BARCODE_MAPPING` | barcode_tag_mapping | Regex → Tag | JSON-Objekt | `{"TAG:(.*)": "\\g<1>"}` | A |
| `CONSUMER_TAG_BARCODE_SPLIT` | barcode_tag_split | an Tag-Barcodes trennen | bool | no | A |
| `CONSUMER_STORE_BARCODE_VALUES` | barcode_store_values | Barcodewerte speichern | bool | no | A |

### 1.9 OCR, Archiv-PDF, Konvertierung, Remote-OCR

OCR-Felder mit AC-Feld: Vorrang AC (E3), wirksam L (nächstes Dokument).

| Schlüssel | Bedeutung | Typ | Standard | Quelle | Wirksam | Geh. | Kl. |
|---|---|---|---|---|---|---|---|
| `OCR_LANGUAGE` | Tesseract-Sprachen | `deu+eng` | eng | E, AC:language, DT→DE | L | nein | A |
| `OCR_MODE` | auto/force/redo/off | Auswahl | auto | E, AC:mode | L | nein | A |
| `ARCHIVE_FILE_GENERATION` | Archiv-PDF | auto/always/never | auto | E, AC:archive_file_generation | L | nein | A |
| `OCR_OUTPUT_TYPE` | pdf/pdfa/pdfa-1/2/3 (PDF/A ohne Ghostscript: Bereinigung + Selbstprüfung, sonst ohne PDF/A-Kennzeichen; pl-ocr §9) | Auswahl | pdfa | E, AC:output_type | L | nein | A |
| `OCR_PAGES` | nur erste n Seiten (0 = alle) | int | 0 | E, AC:pages | L | nein | A |
| `OCR_IMAGE_DPI` | DPI für Bilder ohne Angabe | int | – | E, AC:image_dpi | L | nein | A |
| `OCR_CLEAN` | clean/clean-final/none (eigener unpaper-Ersatz, pl-ocr `preprocess`) | Auswahl | clean | E, AC:unpaper_clean | L | nein | A |
| `OCR_DESKEW` | Geraderichten (pl-ocr `preprocess`, ±15°, ab 0,1°) | bool | true | E, AC:deskew | L | nein | A |
| `OCR_ROTATE_PAGES` | Seiten drehen | bool | true | E, AC:rotate_pages | L | nein | A |
| `OCR_ROTATE_PAGES_THRESHOLD` | Drehschwelle | float | 12.0 | E, AC:rotate_pages_threshold | L | nein | A |
| `OCR_MAX_IMAGE_PIXELS` | Pixelgrenze OCR | float | – | E, AC:max_image_pixels | L | nein | A |
| `OCR_COLOR_CONVERSION_STRATEGY` | Farbumwandlung PDF/A (Bilder + Vektorfarben; CMYK bei PDF/A → RGB) | LeaveColorUnchanged/RGB/UseDeviceIndependentColor/Gray/CMYK | RGB | E, AC:color_conversion_strategy | L | nein | A |
| `OCR_USER_ARGS` | weitere OCRmyPDF-Optionen (u. a. optimize 0–3, jpeg/png_quality, title…, tesseract_*; Rest mit Warnung je Schlüssel) | JSON | – | E, AC:user_args | L | nein | A |
| `OCR_SKIP_ARCHIVE_FILE` | alt, ohne Wirkung (upstream-Prüfung warnt) | – | – | E | – | nein | X |
| `OCR_LANGUAGES` | Sprachpakete im upstream-Image installieren | Liste | – | DI | – | nein | X |
| `PAPERRUST_OCR_LANGUAGE_AUTO` | OCR-Sprache automatisch: Textsprache bestimmen, bei fremder installierter Sprache noch einmal erkennen (pl-consumer, 2026-11-21) | bool | false | E | L | nein | A |
| `PAPERRUST_OCR_ENGINE` | OCR-Engine | tesseract/windows/macos/auto | tesseract | E | L | nein | A |
| `PAPERRUST_OCR_ENGINE_IDLE_SECS` | geladene Tesseract-Engines (Sprachmodelle) nach so vielen Sekunden Leerlauf entladen und Speicher zurückgeben (pl-ocr `tesslib`); 0 = nie (schnellste erste Erkennung, mehr RAM) | int ≥ 0 | 180 | E | L | nein | A |
| `PAPERRUST_PDFA_FONT_DIR` | Ordner mit Ersatzschriften (Liberation) für Schriften, die im Original nicht eingebettet sind (PDF/A-Archiv, pl-ocr `pdfa_fonts`); sonst `/usr/share/fonts/liberation` u. a. | Pfad | Suche | E | L | nein | C |
| `REMOTE_OCR_ENGINE` | Remote-OCR (Azure, REST in `pl-ocr::remote`) | azureai/– | – | E, AC:remote_ocr_engine | L | nein | A |
| `REMOTE_OCR_API_KEY` | API-Schlüssel | str | – | E, AC:remote_ocr_api_key | L | ja (Klartext, E6) | A |
| `REMOTE_OCR_ENDPOINT` | Endpunkt | URL | – | E, AC:remote_ocr_endpoint | L | nein | A |
| `REMOTE_OCR_MODE` | always/workflow_only | Auswahl | always | E, AC:remote_ocr_mode | L | nein | A |
| `REMOTE_OCR_ALLOW_INTERNAL_ENDPOINTS` | interne Adressen erlaubt | bool | true | E | L | nein | D |
| `TIKA_ENABLED` | Tika/Gotenberg nutzen | bool | no | E | – | nein | X |
| `TIKA_ENDPOINT` | Tika-URL | URL | http://localhost:9998 | E | – | nein | X |
| `TIKA_GOTENBERG_ENDPOINT` | Gotenberg-URL | URL | http://localhost:3000 | E | – | nein | X |
| `CONVERT_BINARY`, `CONVERT_TMPDIR`, `CONVERT_MEMORY_LIMIT`, `GS_BINARY` | ImageMagick/Ghostscript (upstream-Doku, nicht mehr im Code) | – | – | E | – | nein | X |
| `ENABLE_NLTK`, `NLTK_DIR` | entfernt | – | – | E | – | nein | X |

(`CONVERT_*`/`GS_BINARY` zählen als 4, `ENABLE_NLTK`/`NLTK_DIR` als 2 Schlüssel.)

### 1.10 Dokumente: Datum, Dateinamen, Suche, Zuordnung, Papierkorb, Audit

| Schlüssel | Bedeutung | Typ | Standard | Quelle | Wirksam | Geh. | Kl. |
|---|---|---|---|---|---|---|---|
| `DATE_ORDER` | Datumsreihenfolge im Inhalt | DMY/MDY/YMD… | DMY | E | L | nein | A |
| `FILENAME_DATE_ORDER` | Datum aus Dateiname | wie oben/leer | – | E | L | nein | A |
| `DATE_PARSER_LANGUAGES` | Sprachen der Datumserkennung | `de+en` | aus OCR-Sprache | E | L | nein | A |
| `IGNORE_DATES` | ignorierte Daten | Liste Daten | – | E | L | nein | A |
| `NUMBER_OF_SUGGESTED_DATES` | Anzahl Datumsvorschläge | int | 3 | E | L | nein | A |
| `FILENAME_FORMAT` | Speicherpfad-Vorlage (global) | Vorlage | – | E | L+I | nein | A |
| `FILENAME_FORMAT_REMOVE_NONE` | leere Platzhalter entfernen | bool | no | E | L+I | nein | A |
| `SEARCH_LANGUAGE` | Sprache des Suchindex | ISO 639-1 | aus OCR-Sprache | E | S+I | nein | B |
| `ADVANCED_FUZZY_SEARCH_THRESHOLD` | Schwelle unscharfe Suche | float | – | E | L | nein | A |
| `CLASSIFIER_MATCH_THRESHOLD` | Mindestsicherheit Klassifikator | float | 0.3 | E | L | nein | A |
| `MATCH_REGEX_TIMEOUT_SECONDS` | Regex-Zeitlimit für Zuordnungsmuster (pl-matching `regex_compat`) | float | 0.1 | E | L | nein | A |
| `TIME_ZONE` | Zeitzone | IANA | UTC | E, DT→DE | S | nein | B |
| `EMPTY_TRASH_DELAY` | Tage im Papierkorb | int ≥ 1 | 30 | E | L | nein | A |
| `AUDIT_LOG_ENABLED` | Änderungsprotokoll | bool | true | E | S | nein | B |
| `ENABLE_UPDATE_CHECK` | Versionsprüfung (veraltet → ui_settings `update-checking`) | bool/default | default | E | L | nein | A |
| `PAPERRUST_REMOTE_VERSION_URL` | Quelle der Versionsprüfung | URL | GitHub-Releases | E | L | nein | B |
| `APP_TITLE` | Anwendungstitel | str | – | E, AC:app_title | L | nein | A |
| `APP_LOGO` | Logo | Datei (Upload `/logo/…`) | – | E, AC:app_logo | L | nein | A |
| `PAPERRUST_TIMELINE_EXTRACT_DATES` | Zeitstrahl (PaperRust-Oberfläche): Fristen/Laufzeiten im Inhalt erkennen (fällig am, gültig bis, Kündigung bis, Garantie bis …; docs/contract/pl-api-v2.md) | bool | no | E | L | nein | A |
| `PAPERRUST_TIMELINE_DATE_FIELDS` | Zeitstrahl: Custom Fields je Datumsart, z. B. `{"due":["Fällig am"],"amount":["Betrag"],"ignore":[…]}`; leer = Erkennung am Feldnamen | JSON-Objekt | – | E | L | nein | A |
| `PAPERRUST_TEXT_HIT_CACHE` | Trefferspeicher für die Titel/Inhalt-Suche ohne weitere Filter (v1-Liste, Auswahl „alle“): Treffer-ids je Suchtext einmal ermitteln, danach nur neue/geänderte Zeilen am Text prüfen; Ergebnis identisch (`pl_store::textcache`, docs/PERF-SCALE.md §12) | on/off | on | E | L | nein | A |
| `PAPERRUST_TEXT_HIT_CACHE_WORKERS` | Verbindungen für den parallelen Aufbau des Trefferspeichers, je ein Blockbereich mit gemeinsamem Schnappschuss (`pl_store::textcache`, docs/contract/pl-docfilter.md) | Zahl 1–64 | Prozessorkerne, höchstens 8 | E | L | nein | A |
| `PAPERRUST_SEARCH_ENGINE` | Volltextsuche der Dokumentliste (`query=`, `text=`, `title_search=`, `more_like_id=`; ARCHITECTURE-V2 §5 L3): `native` = Rechtefilter im Index, nur die Treffer bis zum Seitenende ranken, Trefferzahl aus tantivy, SQL nur für die Seite – gilt nur ohne weitere Filter, für aktive Nutzer und solange `paperrust_search_dirty` leer ist (der Server führt den Index für Rechte/Papierkorb selbst nach), sonst exakt wie `legacy` = upstream-Ablauf (alle Treffer ranken, mit allen sichtbaren IDs schneiden); Ergebnis identisch, Indexformat unverändert (docs/contract/pl-search.md „L3“) | legacy/native | native | E | L | nein | A |

### 1.11 Aufgaben, Worker, Zeitpläne, Warteschlange

| Schlüssel | Bedeutung | Typ | Standard | Quelle | Wirksam | Geh. | Kl. |
|---|---|---|---|---|---|---|---|
| `TASK_WORKERS` | parallele Aufgaben (Einlesen läuft so viele Dokumente gleichzeitig) | int oder `auto` | `auto` (seit 2026-11-23; upstream 1): `ceil(Kerne ÷ 2)` (mit gesetztem `THREADS_PER_WORKER`: `ceil(Kerne ÷ THREADS_PER_WORKER)`), höchstens 4, nach freiem Speicher begrenzt; die Dokumente teilen sich die Kerne über den Seiten-Pool; `1` = ein Dokument nach dem anderen (upstream) | E | W | nein | B |
| `THREADS_PER_WORKER` | höchstens so viele Seiten eines Dokuments gleichzeitig (OCR); alle Dokumente teilen sich einen Seiten-Pool (pl-ocr Known difference 23) | int | automatisch: Pool = `min(Kerne, Speichergrenze)` (Pool × 100 MB ≤ halber freier Speicher, Container-Grenzen zählen), je Dokument bis zum ganzen Pool; gesetzt: Obergrenze je Dokument, Pool = Kerne; upstream `max(Kerne ÷ TASK_WORKERS, 1)` | E | W | nein | B |
| `WORKER_TIMEOUT` | Zeitlimit je Aufgabe | s | 1800 | E | L | nein | A |
| `TRAIN_TASK_CRON` | Klassifikator trainieren | Crontab/disable | `5 */1 * * *` | E | W | nein | A |
| `INDEX_TASK_CRON` | Index optimieren | Crontab | `0 0 * * *` | E | W | nein | A |
| `SANITY_TASK_CRON` | Prüfung | Crontab | `30 0 * * sun` | E | W | nein | A |
| `EMPTY_TRASH_TASK_CRON` | Papierkorb leeren | Crontab | `0 1 * * *` | E | W | nein | A |
| `WORKFLOW_SCHEDULED_TASK_CRON` | geplante Workflows | Crontab | `5 */1 * * *` | E | W | nein | A |
| `PAPERRUST_WORKFLOW_RUN_RETENTION_DAYS` | Verlauf der Workflows aufbewahren (Automatik, `paperrust_workflow_run`; `0` = unbegrenzt) | int Tage | 90 | E | L | nein | A |
| `LLM_INDEX_TASK_CRON` | KI-Index | Crontab | `10 2 * * *` | E | W | nein | A |
| `SHARE_LINK_BUNDLE_CLEANUP_CRON` | abgelaufene Freigabe-Bündel | Crontab | `0 2 * * *` | E | W | nein | A |
| `SESSION_CLEANUP_TASK_CRON` | abgelaufene Sitzungen löschen (upstream fix a139bdff4) | Crontab | `0 3 * * *` | E | W | nein | A |
| `PAPERRUST_AI_TRIGGER_TAG_CRON` | KI-Auslöser-Tag | Crontab | – | E | W | nein | A |
| `PAPERRUST_QUEUE` (alt `PAPERRUST_QUEUE_BACKEND`) | wirkungslos: Warteschlange ist immer PostgreSQL (`celery`/`redis` → Warnung) | – | – | E | – | nein | X |
| `PAPERRUST_QUEUE_NAMES` | abgearbeitete Warteschlangen | Liste | celery | E | W | nein | B |
| `PAPERRUST_QUEUE_VISIBILITY_TIMEOUT` | Lease-Dauer | s | 60 | E | W | nein | A |
| `PAPERRUST_QUEUE_MAX_DELIVERIES` | Zustellungen je Versuch | int | 3 | E | L | nein | A |
| `PAPERRUST_QUEUE_POLL_INTERVAL` | Abfrage ohne NOTIFY | s | 1 | E | W | nein | A |
| `PAPERRUST_QUEUE_SHUTDOWN_GRACE` | Wartezeit beim Beenden | s | 30 | E | L | nein | A |
| `PAPERRUST_QUEUE_RESULT_EXPIRES` | Aufbewahrung erledigter Aufgaben | s | 3600 | E | L | nein | A |
| `PAPERRUST_WORKER_NAME` | Worker-Name | str | paperrust@host:pid | E, DE | S | nein | B |
| `PAPERRUST_INDEX_WRITER` | Suchindex über die Warteschlange, direkt oder aus (`celery` = alter Name für `queue`) | `queue`/`rust`/`off` | queue | E | S | nein | B |
| `ENABLE_FLOWER` | Flower im upstream-Image | defined | – | DI | – | nein | X |
| `SUPERVISORD_WORKING_DIR` | veraltet | – | – | DI | – | nein | X |

(`PAPERRUST_QUEUE`/`_QUEUE_BACKEND` zählen als 2.)

### 1.12 Integrität, Sync, Import

| Schlüssel | Bedeutung | Typ | Standard | Quelle | Wirksam | Geh. | Kl. |
|---|---|---|---|---|---|---|---|
| `PAPERRUST_INTEGRITY_INCREMENTAL` | unveränderte Dateien nicht neu hashen | bool | no | E | L | nein | A |
| `PAPERRUST_INTEGRITY_FULL_HASH_DAYS` | Höchstalter einer Bestätigung | int Tage | 28 | E | L | nein | A |
| `PAPERRUST_INTEGRITY_THREADS` | parallele Leser | int (0 = auto ≤ 4) | 0 | E | L | nein | A |
| `PAPERRUST_INTEGRITY_REPORT_KEEP` | aufbewahrte Berichte | int | 10 | E | L | nein | A |
| `PAPERRUST_GUARD` | Ransomware-Wächter (docs/user/ransomware.md) | on/off | on | E | W | nein | A |
| `PAPERRUST_GUARD_THRESHOLD` | so viele verdächtige Dateien → Nur-Lese-Modus | int ≥ 1 | 20 | E | W | nein | A |
| `PAPERRUST_GUARD_WINDOW_MINUTES` | Zeitfenster der Schwelle | int Minuten | 10 | E | W | nein | A |
| `PAPERRUST_GUARD_SCAN_INTERVAL` | Ordnerdurchsicht (Größe/Zeit) | s (0 = aus) | 900 | E | W | nein | A |
| `PAPERRUST_GUARD_SAMPLE_FILES` | je Durchsicht gehashte Dokumente | int | 20 | E | W | nein | A |
| `PAPERRUST_GUARD_WATCH` | Dateisystem-Ereignisse | on/off | on | E | W | nein | A |
| `PAPERRUST_GUARD_EXTRA_EXTENSIONS` | zusätzliche verdächtige Endungen | Liste | – | E | W | nein | A |
| `PAPERRUST_GUARD_MAIL` | Empfänger der Alarm-Mail | Liste | alle Superuser | E | W | nein | A |
| `PAPERRUST_BACKUP_TARGET` | Sicherungsziel (Ordner oder `s3://bucket/präfix`) | str | – | E | L | nein | A |
| `PAPERRUST_BACKUP_CRON` | Zeitplan der Sicherung | Crontab | 15 2 * * * | E | W | nein | A |
| `PAPERRUST_BACKUP_DATABASE` | Datenbank mitsichern | on/off | on | E | L | nein | A |
| `PAPERRUST_BACKUP_KEEP_DAILY` / `_WEEKLY` / `_MONTHLY` | Aufbewahrung | int | 7 / 4 / 12 | E | L | nein | A |
| `PAPERRUST_BACKUP_S3_ENDPOINT` / `_S3_REGION` / `_S3_ACCESS_KEY` | S3-Zugang | URL/str | – / us-east-1 / – | E | L | nein | A |
| `PAPERRUST_BACKUP_S3_SECRET_KEY` | S3-Geheimschlüssel | str | – | E | L | ja | A |
| `PAPERRUST_BACKUP_S3_OBJECT_LOCK` | Object Lock | off/governance/compliance | off | E | L | nein | A |
| `PAPERRUST_BACKUP_S3_LOCK_DAYS` | Sperrfrist | int Tage | 30 | E | L | nein | A |
| `PAPERRUST_BACKUP_PASSPHRASE` | Passphrase für `backup init`/`restore` | str | – | E | – | ja | D |
| `PAPERRUST_SYNC_SEQUENCER` | Sync-Sequenzer im Worker | bool | on | E | W | nein | B |
| `PAPERRUST_SYNC_TOMBSTONE_DAYS` | Aufbewahrung Löschmarken | int Tage | 180 | E | L | nein | A |
| `PAPERRUST_SYNC_INTERVAL` | Sequenzer-Abfrage | s | 1 | E | W | nein | A |
| `PAPERRUST_SYNC_RETENTION_INTERVAL` | Aufräumlauf | s | 3600 | E | W | nein | A |
| `PAPERRUST_SYNC_BATCH` | Zeilen je Transaktion | int > 0 | 50 000 | E | L | nein | A |
| `PAPERRUST_IMPORT_MAX_UNZIPPED_BYTES` | Entpackgrenze beim Import (CLI) | int Bytes | 1 GiB + 100 × Archiv | E | – | nein | D |
| `PAPERRUST_MIGRATE_SOURCE` / `_TARGET` / `_API_TOKEN` | Umzugswerkzeug `pl-dbmigrate` (Kommandozeile) | URL/Token | – | E | 1 | Token ja | C |

(`PAPERRUST_MIGRATE_*` zählen als 3, `PAPERRUST_BACKUP_KEEP_*` als 3, `PAPERRUST_BACKUP_S3_ENDPOINT`/`_REGION`/`_ACCESS_KEY` als 3.)

### 1.13 Protokolle

| Schlüssel | Bedeutung | Typ | Standard | Quelle | Wirksam | Geh. | Kl. |
|---|---|---|---|---|---|---|---|
| `PAPERRUST_LOG` | Log-Filter | EnvFilter | info | E | L³ | nein | A |
| `PAPERRUST_LOG_FILES` | Logdateien schreiben | bool | yes | E | S | nein | B |
| `LOGROTATE_MAX_SIZE` | Größe je Logdatei | Bytes | 1 MiB | E | S | nein | B |
| `LOGROTATE_MAX_BACKUPS` | Anzahl Logdateien | int | 20 | E | S | nein | B |

³ über `tracing_subscriber::reload`-Handle.

### 1.14 Branding, Oberfläche (systemweit)

| Schlüssel | Bedeutung | Typ | Standard | Quelle | Wirksam | Geh. | Kl. |
|---|---|---|---|---|---|---|---|
| `PAPERRUST_BRANDING` | PaperRust-Logo/Name/Farbe | bool | on | E | S | nein | B |
| `PAPERRUST_ACCENT` | Akzentfarbe | Hex | #B7410E | E | S | nein | B |
| *Standardsprache* *(neu `PAPERRUST_DEFAULT_LANGUAGE`)* | Sprache für Nutzer ohne Wahl / Login-Seite | Locale | Browser | K | L | nein | A |
| *Standard-Dunkelmodus, -Datumsformat, -Listengröße* *(neu `PAPERRUST_UI_DEFAULTS`)* | Vorgaben für ui_settings neuer Nutzer | JSON (Teilmenge 1.17) | – | K | L | nein | A |

### 1.15 KI / LLM / MCP

AI_*-Felder mit AC-Feld: Vorrang AC (E3), wirksam L.

| Schlüssel | Bedeutung | Typ | Standard | Quelle | Wirksam | Geh. | Kl. |
|---|---|---|---|---|---|---|---|
| `AI_ENABLED` | KI an | bool | no | E, AC:ai_enabled | L | nein | A |
| `AI_LLM_BACKEND` | ollama/openai-like | Auswahl | – | E, AC:llm_backend | L | nein | A |
| `AI_LLM_MODEL` | Modell | str | – | E, AC:llm_model | L | nein | A |
| `AI_LLM_API_KEY` | API-Schlüssel | str | – | E, AC:llm_api_key | L | ja (Klartext, E6) | A |
| `AI_LLM_ENDPOINT` | Endpunkt | URL | – | E, AC:llm_endpoint | L | nein | A |
| `AI_LLM_OUTPUT_LANGUAGE` | Ausgabesprache | str | – | E, AC:llm_output_language | L | nein | A |
| `AI_LLM_CONTEXT_SIZE` | Kontextgröße | int ≥ 1 | 8192 | E, AC:llm_context_size | L | nein | A |
| `AI_LLM_REQUEST_TIMEOUT` | Zeitlimit | int s ≥ 1 | 120 | E, AC:llm_request_timeout | L | nein | A |
| `AI_LLM_EMBEDDING_BACKEND` | huggingface/openai-like/ollama | Auswahl | – | E, AC:llm_embedding_backend | L+I | nein | A |
| `AI_LLM_EMBEDDING_MODEL` | Embedding-Modell | str | – | E, AC:llm_embedding_model | L+I | nein | A |
| `AI_LLM_EMBEDDING_API_KEY` | API-Schlüssel | str | – | E, AC:llm_embedding_api_key | L | ja (Klartext, E6) | A |
| `AI_LLM_EMBEDDING_ENDPOINT` | Endpunkt | URL | – | E, AC:llm_embedding_endpoint | L | nein | A |
| `AI_LLM_EMBEDDING_CHUNK_SIZE` | Chunk-Größe | int ≥ 1 | 1024 | E, AC:llm_embedding_chunk_size | L+I | nein | A |
| `AI_LLM_EXTRA_PARAMS` | zusätzliche Anfrage-Parameter | JSON-Objekt | {} | E | L | nein | A |
| `AI_LLM_ALLOW_INTERNAL_ENDPOINTS` | interne Adressen erlaubt | bool | true | E | L | nein | D |
| `PAPERRUST_AI_API_BASE` | interne API-Basis des MCP-Servers | URL | aus Bind | E | S | nein | B |
| `PAPERRUST_AI_MCP_ENABLED` | `/mcp` ausliefern | bool | no | E | S | nein | B |
| `PAPERRUST_AI_MCP_RATE_LIMIT` | Anfragen/min/Token | int | 120 | E | L | nein | A |
| `PAPERRUST_AI_MCP_MAX_CREATES_PER_HOUR` | Neuanlagen/h/Token | int | 30 | E | L | nein | A |
| `PAPERRUST_AI_MCP_ALLOWED_ORIGINS` | erlaubte Origins | Liste | – | E | L | nein | A |
| `PAPERRUST_AI_MCP_TOOL_GROUPS` | freigeschaltete Werkzeuggruppen | Liste `read,write,ingest` | read,write | E | L | nein | A |
| `PAPERRUST_AI_MCP_ALLOW_URL_FETCH` | `add_document` darf URLs abrufen (SSRF) | bool | no | E | L | nein | D |
| `PAPERRUST_AI_MCP_MAX_UPLOAD_BYTES` | größte Datei für `add_document` | int Bytes | 1 572 864 | E | L | nein | A |
| `PAPERRUST_AI_MCP_MAX_INGESTS_PER_HOUR` | `add_document` je Token und Stunde | int | 30 | E | L | nein | A |
| `PAPERRUST_AI_MATCH_THRESHOLD` | Namensabgleich | float | 0.8 | E | L | nein | A |
| `PAPERRUST_AI_AUTO_MODE` | off/suggest/apply | Auswahl | off | E | L | nein | A |
| `PAPERRUST_AI_AUTO_MIN_CONFIDENCE` | Übernahmeschwelle | float | 0.9 | E | L | nein | A |
| `PAPERRUST_AI_AUTO_TRIGGER_TAG` | Auslöser-Tag | Tag-Name | – | E | L | nein | A |
| `PAPERRUST_AI_AUTO_MAX_NEW_OBJECTS` | neue Objekte je Lauf | int | 3 | E | L | nein | A |
| `PAPERRUST_AI_AUTO_MAX_DOCUMENTS_PER_RUN` | Dokumente je Lauf | int | 50 | E | L | nein | A |
| `PAPERRUST_AI_AUTO_CREATE_MISSING` | fehlende Objekte anlegen | bool | no | E | L | nein | A |
| `PAPERRUST_AI_RUN_MAX_DOCUMENTS_PER_HOUR` | manuelle Läufe/h | int | 100 | E | L | nein | A |
| `PAPERRUST_AI_VISION_ENABLED` | KI-Texterkennung verfügbar (Vorgabe; Oberfläche „KI → Texterkennung“ speichert in `paperrust_ai_option`) | bool | no | E | L | nein | A |
| `PAPERRUST_AI_VISION_MODEL` | Bildmodell (leer = Textmodell) | Text | – | E | L | nein | A |
| `PAPERRUST_AI_VISION_WHEN` | wann: off (nur auf Wunsch je Dokument, `ai_ocr`) / difficult (wenig Text) / always | Auswahl | difficult | E | L | nein | A |
| `PAPERRUST_AI_VISION_MAX_PAGES` | Seiten je Dokument | int ≥ 1 (≤ 200) | 10 | E | L | nein | A |
| `PAPERRUST_AI_VISION_MAX_PAGES_PER_DAY` | Seiten je Tag und Prozess | int (0 = unbegrenzt) | 500 | E | L | nein | A |
| `PAPERRUST_AI_CUSTOM_FIELDS_ENABLED` | Benutzerfelder per KI ausfüllen (Vorgabe; Felder je Feld in der Oberfläche) | bool | no | E | L | nein | A |

### 1.16 Webhooks, Freigaben

| Schlüssel | Bedeutung | Typ | Standard | Quelle | Wirksam | Geh. | Kl. |
|---|---|---|---|---|---|---|---|
| `WEBHOOKS_ALLOWED_SCHEMES` | erlaubte Schemata | Liste | http,https | E | L | nein | D |
| `WEBHOOKS_ALLOWED_PORTS` | erlaubte Ports | Liste int | alle | E | L | nein | D |
| `WEBHOOKS_ALLOW_INTERNAL_REQUESTS` | interne Ziele | bool | true | E | L | nein | D |

### 1.16a Desktop (Supervisor `desktop.toml`, Hülle `app.toml`, Desktop-Umgebung)

GUI-Schreibweg siehe 2.6. „DT“-Schlüssel sind Bootstrap für den Server, aber vom Supervisor schreibbar.

| Schlüssel | Bedeutung | Typ | Standard | Quelle | Wirksam | Geh. | Kl. |
|---|---|---|---|---|---|---|---|
| `desktop.database.port` | Wunschport eingebettetes PostgreSQL | u16 | frei gewählt | DT | S | nein | C |
| `desktop.database.user` / `password` | Cluster-Superuser | str | erzeugt | DT | 1 | Passwort ja | C |
| `desktop.database.app_user` / `app_password` | Anwendungsrolle | str | paperrust / erzeugt | DT | 1 | Passwort ja | C |
| `desktop.database.name` | DB-Name | str | paperless | DT | 1 | nein | C |
| `desktop.database.locale_provider` | builtin/icu (nur initdb) | Auswahl | icu | DT | 1 | nein | C |
| `desktop.server.port` | App-Port | u16 | frei gewählt | DT | S | nein | C |
| `desktop.server.secret_key` | SECRET_KEY | str | erzeugt | DT | S | ja | C |
| `desktop.server.time_zone` | → `TIME_ZONE` (wird nach DB umgezogen, 2.6) | IANA | UTC | DT | S | nein | B |
| `desktop.server.ocr_language` | → `OCR_LANGUAGE` (wird nach AC umgezogen, 2.6) | str | deu+eng | DT | L | nein | A |
| `desktop.server.open_browser` | Browser beim Start öffnen | bool | yes | DT | 1 | nein | C |
| `desktop.server.lan_sharing` | im LAN freigeben (Bind 0.0.0.0, `URL`, `ALLOWED_HOSTS`, `SHARE_MODE`) | bool | no | DT | S | nein | C |
| `desktop.server.lan_address` | LAN-Adresse für Links | IPv4/Name | erkannt | DT | S | nein | C |
| `desktop.setup.*` (5 Flags, inkl. `settings_migrated`) | Fortschritt Erst-Start | bool | – | DT | 1 | nein | X⁴ |
| `desktop.backup.schedule` *(neu)* | automatische Sicherung | off/daily/weekly + Uhrzeit | off | DT | L (Supervisor) | nein | C |
| `desktop.backup.keep` *(neu)* | aufbewahrte Sicherungen | int | 7 | DT | L (Supervisor) | nein | C |
| `desktop.updates.check` *(neu)* | App-Update prüfen (Tauri-Updater) | off/notify/install | notify | DT | L (Hülle) | nein | C |
| `app.mode` | local/remote | Auswahl | Frage beim Erst-Start | AT | S (Hülle) | nein | C |
| `app.active_profile`, `app.profile[]` (`name`, `url`, `flavor`, `ui`, `tls_pin`, `allow_http`, `extra_origins`, `sync`) | Fern-Profile | – | – | AT | L (Hülle) | nein | C |
| `PAPERRUST_DESKTOP_HOME` | Datenordner | Pfad | %LOCALAPPDATA%/PaperRust | E | S | nein | C |
| `PAPERRUST_DESKTOP_MODE` / `_SERVER_URL` / `_ALLOW_HTTP` | Erst-Start ohne Formular | – | – | E | 1 | nein | C |
| `PAPERRUST_DESKTOP_ADMIN_USER` / `_EMAIL` / `_PASSWORD` | Erst-Start-Superuser | – | – | E | 1 | Passwort ja | C |
| `PAPERRUST_DESKTOP_IMPORT` / `_IMPORT_DB` / `_IMPORT_MEDIA` | Erst-Start mit Import | – | – | E | 1 | nein | C |
| `PAPERRUST_DESKTOP_LOG` / `_SERVER_LOG` / `_LOG_MAX_MB` | Log-Stufe Supervisor/Server, Loggrenze | – | info/info/10 | E | S | nein | B |
| `PAPERRUST_DESKTOP_PG_BIN` / `_SERVER_BIN` / `_MIGRATE_DB_BIN` / `_STATIC` / `_ROUTES` | Installationslayout | Pfad | Paket | E | S | nein | C |
| `PAPERRUST_DATAMODEL_BACKUP` | Sicherung (`pg_dump`) vor den Datenmodell-Umbaustufen (`off` = schon selbst gesichert) | on/off | on | E | 1 | nein | C |
| `PAPERRUST_MIGRATE_WITHOUT_BACKUP` | Datenmodell-Umbaustufen auch ohne `pg_dump` einspielen (sonst Abbruch mit Meldung; nur, wenn anders gesichert) | bool | false | E | 1 | nein | C |
| `PAPERRUST_PG_BIN` | Ordner mit `pg_dump` für die Sicherung beim Umstieg | Pfad | – (PATH) | E | S | nein | C |
| `PAPERRUST_BULK_EDIT_ASYNC_MIN_DOCS` | Massenänderung ab so vielen Dokumenten im Hintergrund (0 = immer sofort) | Zahl ≥ 0 | 500 | E | S | nein | C |
| `PAPERRUST_BULK_UPDATE_COALESCE` | wartende Nacharbeiten (`bulk_update_documents`) von Massenänderungen zusammenfassen (B6) | on/off | on | E | S | nein | C |
| `PAPERRUST_MEDIA_LOCK_TIMEOUT` | höchste Wartezeit auf die Dateisperre `MEDIA_LOCK` (s, `0` = ohne Limit wie upstream; B1) | Zahl ≥ 0 | 120 | E | S | nein | C |
| `PAPERRUST_BULK_UPDATE` | Massen-Neuspeichern: `single` = alter Einzelweg (Fehlersuche) | Text | – | E | W | nein | C |
| `PAPERRUST_DESKTOP_UPDATES` / `_UPDATE_URL` / `_UPDATE_CHANNEL` | App-Updates der Hülle (aus, Adresse, Kanal stable/beta) | on/off · URL · Auswahl | on · GitHub-Release · stable | E | L (Hülle) | nein | C |
| `PAPERRUST_DESKTOP_NO_JOB` | Windows-Job-Objekt aus (Fehlersuche) | bool | no | E | S | nein | D |
| `PAPERRUST_DESKTOP_RESTART_AFTER` | intern (Neustart der Hülle) | pid | – | K | – | nein | X⁴ |

⁴ intern, nicht angezeigt. Gezählt: `user/password`, `app_user/app_password` je 2; die Namensgruppen
`PAPERRUST_DESKTOP_*` mit `/` je Einzelname; `app.active_profile`+Profile als 2.

Testvariablen (`*_TEST_DATABASE_URL`, `PAPERRUST_INDEXER_PARITY_*`, `PAPERRUST_W15_DATABASE_URL`,
`PAPERRUST_INDEXER_DB_URL`) gehören nicht zum Produkt und werden nicht inventarisiert.

### 1.17 Pro Nutzer: ui_settings (`/api/ui_settings/`, `documents_uisettings`) – bereits in der Oberfläche

Bleiben pro Nutzer (Reiter „Allgemein“, „Benachrichtigungen“, „Ansichten“ in Angular und Light-UI),
werden **nicht** zu Systemeinstellungen. Neu: systemweite *Vorgaben* für neue Nutzer
(`PAPERRUST_UI_DEFAULTS`, 1.14). 44 Schlüssel:

- Sprache/Anzeige: `language`, `general-settings:dark-mode:use-system`, `…:dark-mode:enabled`,
  `…:dark-mode:thumb-inverted`, `…:theme:color`, `…:date-display:date-locale`,
  `…:date-display:date-format`, `…:slim-sidebar`, `…:sidebar:hidden-items`,
  `…:attributes-sections-collapsed`, `…:tour-complete`, `version`
- Listen: `…:documentListSize`, `…:object-list-sizes`, `…:bulk-edit:apply-on-close`,
  `…:bulk-edit:confirmation-dialogs`
- Dokumentansicht/Bearbeiten: `…:document-details:native-pdf-viewer`,
  `…:document-details:pdf-viewer-zoom-setting`, `…:document-details:hidden-fields`,
  `…:document-editing:remove-inbox-tags`, `…:document-editing:overlay-thumbnail`,
  `…:document-editing:default-edit-mode`, `…:document-editing:auto-suggest`,
  `…:document-editing:suggestion-source`, `…:notes-enabled`
- Benachrichtigungen: `…:notifications:consumer-success`, `…:notifications:consumer-failed`
- Ansichten: `…:saved-views:dashboard-views-visible-ids`, `…:dashboard-views-sort-order`,
  `…:sidebar-views-visible-ids`, `…:sidebar-views-sort-order`, `…:sidebar-views-show-count`,
  `…:warn-on-unsaved-change`
- Rechte-Vorgaben: `…:permissions:default-owner`, `…:default-view-users`, `…:default-view-groups`,
  `…:default-edit-users`, `…:default-edit-groups`
- Suche/Update: `…:search:db-only`, `…:search:more-link`, `…:update-checking:enabled`
  (+ `…:update-checking:backend-setting` nur lesend)
- Serverseitig abgeleitet, nur lesend: `app_title`, `app_logo`, `trash_delay`, `email_enabled`,
  `ai_enabled`, `gmail_oauth_url`, `outlook_oauth_url`, `remote_ocr:configured`,
  `remote_ocr:mode`, `general-settings:auditlog-enabled`

### 1.18 Einstellungen als DB-Objekte – bereits in der Oberfläche

Nicht Teil dieses Vorhabens, im Reiter „System“ nur als Verweis aufgeführt: Mail-Konten
(inkl. Passwort/OAuth-Token, Klartext wie upstream), Mail-Regeln, Workflows (inkl. Webhooks,
geplante Auslöser), benutzerdefinierte Felder, Speicherpfade, Korrespondenten/Typen/Tags (Abgleich),
Nutzer/Gruppen/Rechte, gespeicherte Ansichten, API-Token/Geräte-Token, Freigabelinks/-bündel.

### 1.19 Zählung

Gezählt wird je Einzelschlüssel (Zeilen mit mehreren Namen entsprechend mehrfach; ohne Testvariablen,
ohne pro-Nutzer-Werte, ohne DB-Objekte).

| Klasse | Anzahl | davon ApplicationConfiguration |
|---|---|---|
| A – GUI live | 189 | 45 |
| B – GUI mit Neustart | 43 | – |
| C – Bootstrap / Installation | 94 | – (großteils Desktop-, Docker- und Installationsschlüssel) |
| D – nur Umgebung | 27 | – |
| X – wirkungslos in PaperRust / intern | 32 | – |
| **Summe System** | **385** | |
| pro Nutzer (ui_settings, 1.17) | 44 | |
| DB-Objekte bereits in der GUI (1.18) | 10 Arten | |

ApplicationConfiguration hat **45 Einstellungs-Spalten**, alle Klasse A: 13 OCR (`output_type`,
`pages`, `language`, `mode`, `archive_file_generation`, `image_dpi`, `unpaper_clean`, `deskew`,
`rotate_pages`, `rotate_pages_threshold`, `max_image_pixels`, `color_conversion_strategy`,
`user_args`), 2 allgemein (`app_title`, `app_logo`), 13 Barcode, 4 Remote-OCR, 13 KI. Jede Spalte steht
genau einmal im Inventar (als Zeile ihres Env-Namens). WP1 prüft die Zählung maschinell gegen das
Schema-Register (Test `inventory_complete`).

---

## 2. Vorrang- und Speichermodell

### 2.1 Drei Regeln

**Regel N (neu GUI-fähige Schlüssel, Klasse A/B ohne AC-Feld):**

```
Umgebung (PAPERRUST_X > PAPERLESS_X > PAPERLESS_RS_X)  >  paperrust_setting  >  Konfigurationsdatei  >  Standard
```

- Umgebung **sperrt** den Schlüssel in der GUI (Abzeichen „aus Umgebung“, Feld deaktiviert, Text
  „Gesetzt über Umgebung (`PAPERLESS_EMAIL_HOST`) – hier nicht änderbar“).
- Konfigurationsdatei liegt **unter** der DB: sie ist eine Vorgabe des Betreibers, die die GUI
  überschreiben darf (Abzeichen „überschreibt paperrust.conf“). Grund: In Docker ist die Datei meist
  schreibgeschützt eingebunden; Umsteiger sollen sie nicht erst leeren müssen.
  *Technik:* `Env::from_process` merkt sich künftig je Schlüssel die Herkunft (`Process`/`File(path)`),
  statt die Datei in dieselbe Map zu mischen (kleiner CORE-REQUEST an `pl-core/config.rs`).
- Rettungsweg bei Fehleinstellung: Variable setzen und neu starten, oder
  `paperrust manage settings reset <KEY>` (ohne laufenden Server).

**Regel AC (Felder von ApplicationConfiguration):** exakt upstream `paperless/config.py`:

```
AC-Spalte (bool: IS NOT NULL; sonst Python-„truthy“: nicht NULL, nicht 0, nicht "")  >  Umgebung  >  Datei  >  Standard
```

- Keine Sperre durch die Umgebung (upstream sperrt auch nicht). GUI zeigt beide Werte:
  „Umgebung: `deu` · in der Datenbank überschrieben mit `deu+eng`“.
- „Zurücksetzen“ = Spalte auf `NULL` (danach gilt die Umgebung). Eine `0`/`""` ist wie upstream
  gleichbedeutend mit „nicht gesetzt“ – die GUI zeigt bei solchen Werten den Hinweis
  „0 bedeutet: Umgebungs-/Standardwert“.
- Gelesen wird wie heute bei jeder Verwendung (Consumer, KI, ui_settings). Kein Cache, daher
  auch kein Trigger an der Django-Tabelle nötig.

**Regel C/D:** nur Umgebung/Datei (bzw. `desktop.toml`/`app.toml`). GUI zeigt Wert (Geheimnisse
maskiert), Herkunft und Begründung.

### 2.2 Tabellen (expand-only, ARCHITECTURE-V2 §4)

```sql
-- crates/pl-settings/migrations/0001_paperrust_setting.sql  (Rückbau: DROP TABLE …)
CREATE TABLE IF NOT EXISTS paperrust_setting (
    key          text PRIMARY KEY,           -- kanonischer Name, z. B. 'PAPERLESS_EMAIL_HOST'
    value        text,                       -- kanonischer Env-String (NULL wenn Geheimnis)
    secret_ct    bytea,                      -- nonce || ciphertext (nur Geheimnisse)
    secret_kid   text,                       -- Kennung des Ableitungsschlüssels
    rev          bigint NOT NULL,            -- aus paperrust_setting_rev
    updated_at   timestamptz NOT NULL DEFAULT now(),
    updated_by   integer,                    -- auth_user.id, bewusst ohne FK
    updated_by_name text                     -- Nutzername zum Zeitpunkt
);
CREATE SEQUENCE IF NOT EXISTS paperrust_setting_rev;
CREATE TABLE IF NOT EXISTS paperrust_setting_history (
    id bigserial PRIMARY KEY, key text NOT NULL, old_value text, new_value text,  -- Geheimnisse: '***'
    action text NOT NULL,                    -- set | reset | adopt | import | cli
    at timestamptz NOT NULL DEFAULT now(), user_id integer, user_name text, remote_addr inet
);
```

AC-Änderungen über v2 schreiben ebenfalls einen Verlaufseintrag (`key='AC:llm_model'`); Änderungen
über v1 `/api/config/` landen wie upstream im Django-auditlog (falls an) – keine Pflicht zur Lückenlosigkeit.

### 2.3 Geheimnisse

- `paperrust_setting`: XChaCha20-Poly1305; Schlüssel = HKDF-SHA256(IKM = `SECRET_KEY`,
  salt = `"paperrust-settings-v1"`, info = `"settings"`); AAD = `key`; `secret_kid` = erste 8 Byte
  SHA-256 des abgeleiteten Schlüssels (erkennt einen geänderten SECRET_KEY).
- Passt `secret_kid` nicht: Wert gilt als nicht gesetzt, Warnung im Log, GUI-Abzeichen „Geheimnis
  nicht lesbar – bitte neu eingeben“. `paperrust manage settings rekey --old-secret-key …` verschlüsselt um.
- API: nie Klartext zurück, nur `is_set` und `updated_at`. PATCH mit Wert setzt, mit `null` löscht;
  Feld leer lassen = unverändert.
- **AC-Spalten und Mail-Konten bleiben Klartext** (Django/v1 lesen sie, upstream-Export enthält sie).
  Dokumentiert als bekannte Schwäche; GUI-Hinweis „wird wie bei paperless-ngx unverschlüsselt in der
  Datenbank gespeichert – alternativ über Umgebung setzen“.
- Logs/Verlauf/Export: Geheimnisse immer `***`.

### 2.4 Validierung

- Schema-Register `pl-settings::SCHEMA: &[SettingDef]` mit `key`, Aliasen, `category`, `class`,
  `effect` (L/W/S/1, `+I`), `secret`, `ty` (bool/int/float/str/list/json/enum/path/url/cron/tz),
  Grenzen, Standard, Doku-Anker, Verbraucher (Crate), `django_consumer` (Mischbetrieb-Hinweis).
- Prüfung = **derselbe Parser** wie beim Start: der Wert wird in ein Ein-Schlüssel-`Env` gelegt und
  über die vorhandenen Funktionen (`Env::bool`, `int`, `list`, `json`, `get_choice`, Crontab-Parser
  des Zeitplaners, `chrono_tz`) gelesen. Zusätzliche Prüfungen pro Schlüssel (z. B. Regex in
  `CONSUMER_IGNORE_PATTERNS` kompilierbar, Pfad existiert und ist schreibbar, Tesseract-Sprache
  installiert, `DISABLE_REGULAR_LOGIN` nur mit SSO-Anbieter).
- AC-Felder: dieselben Regeln wie der v1-Serializer (`ApplicationConfigurationSerializer`, Choices,
  `PositiveSmallIntegerField`-Grenzen, `user_args`/`barcode_tag_mapping` JSON-Objekt).

### 2.5 Live-Übernahme und Neustart

1. PATCH schreibt in einer Transaktion `paperrust_setting` + Verlauf, `rev = nextval(…)`, danach
   `pg_notify('paperrust_settings', '{"rev":N,"keys":[…]}')`.
2. Jeder Prozess (Server, getrennte Worker) hält `Arc<ArcSwap<Settings>>` in `AppState`
   (CORE-REQUEST `pl-core/state.rs`: Feld `settings`); ein Task lauscht per `LISTEN` (Muster
   `pl-queue/src/events.rs`), lädt alle Zeilen neu, baut den Schnappschuss aus Umgebung+DB+Datei und
   tauscht ihn. Rückfall: alle 30 s `SELECT max(rev)`.
3. Verbraucher lesen pro Verwendung `state.settings.load()`. `Config` bleibt für C/D-Schlüssel und
   als Startwert.
4. **W**-Schlüssel: Teilsysteme abonnieren Änderungen und starten sich neu (Ordnerüberwachung,
   Zeitplaner mit neuen Crontabs, Worker-Pool-Größe, Sync-Sequenzer). Laufende Aufgaben laufen zu Ende.
5. **S**-Schlüssel: Schnappschuss merkt `boot_value`; weicht der wirksame Wert ab →
   `restart_pending: true` (Abzeichen „Neustart nötig“, Leiste oben „3 Änderungen werden nach einem
   Neustart wirksam – Jetzt neu starten“).
6. `+I`: nach Speichern Angebot „Index neu aufbauen“ / „Dateien umbenennen“ (vorhandene Aufgaben
   `index_reindex`, `document_renamer`).
7. Neustart: `POST /rs/api/v2/settings/restart` → geordnetes Beenden (`shutdown.rs`), Exit-Code **75**.
   Docker-Entrypoint startet `paperrust` in einer Schleife neu, solange 75 kommt; Desktop-Supervisor
   wertet 75 als „Neustart gewünscht“ (zählt nicht zu `MAX_RESTARTS`); systemd-Vorlage
   `Restart=on-failure` + `RestartForceExitStatus=75`. Aufseher melden sich mit
   `PAPERRUST_RESTART_SUPERVISED=1` (neu); ohne diesen Schalter zeigt die GUI nur „Dienst bitte neu
   starten“.
8. ~~**Mischbetrieb** (Django läuft)~~ – Geschichte: Seit 2026-11-14 gibt es keine Weiterleitung an
   Django mehr (`PAPERRUST_DJANGO_URL` ist wirkungslos, Klasse X); alle DB-Werte wirken auf PaperRust.
   Das Abzeichen „wirkt nicht auf Django-Pfade“ (`django_consumer`) entfällt damit.

### 2.6 Desktop

- Server läuft unter dem Supervisor; der setzt `PAPERRUST_SUPERVISOR_CONTROL=<run-Ordner>` (neu).
- `desktop.*`-Schlüssel in der GUI: Server schreibt `run/settings.request.json` (atomar, Inhalt
  `{id, changes}`), Supervisor prüft mit demselben Schema (`pl-settings` ohne DB-Feature), schreibt
  `desktop.toml`, startet den Server neu (bei S), schreibt `run/settings.result.json`. Muster wie
  `stop.request`/`backup.request`. **Nur von Loopback-Anfragen** (LAN-Freigabe darf nicht aus dem LAN
  umgestellt werden).
- `time_zone` und `ocr_language` werden einmalig umgezogen: beim ersten Start nach dem Update schreibt
  der Supervisor sie per `paperrust manage settings set --if-unset` nach `paperrust_setting`
  (`TIME_ZONE`) bzw. AC `language` und setzt sie **nicht mehr** als Umgebung (sonst wären sie gesperrt).
  Die Felder in `desktop.toml` bleiben lesbar (Rückweg), werden aber nicht mehr geschrieben.
- Vom Supervisor gesetzte Umgebung (`DE`) erscheint als Herkunft „Desktop-App“ (statt „Umgebung“),
  damit der Hinweis stimmt („wird von der Desktop-App verwaltet“). Erkennung: Supervisor setzt
  `PAPERRUST_SUPERVISOR_MANAGED_KEYS=<Liste>` (neu).
- `app.toml` (Betriebsart, Profile) gehört der Hülle, nicht dem Server: Einstellungen dafür nur in der
  Hülle (Light-UI-Route `/rs/app/settings/desktop` über Tauri-Befehle, sichtbar nur in Tauri). Im
  Fern-Modus ist der Server fremd – deshalb nicht über die Server-API.

### 2.7 Export/Import

- `GET /rs/api/v2/settings/export?format=json|env` – alle DB-Werte (`paperrust_setting` + AC-Spalten),
  Geheimnisse nur mit `include_secrets=true` **und** erneuter Passwortbestätigung (`password` im Body,
  daher `POST`), dann im JSON verschlüsselt mit Passphrase (wie upstream `document_exporter --passphrase`).
- `POST /rs/api/v2/settings/import?dry_run=true` – zeigt Unterschiede; ohne `dry_run` atomar.
- CLI: `paperrust manage settings list|get|set|reset|export|import|adopt|slim-env|rekey`.
- Dokument-Export (`document_exporter`): AC ist wie upstream im Manifest; `paperrust_setting` kommt als
  eigene Datei `paperrust-settings.json` daneben (paperless-ngx ignoriert sie → Rückweg offen).

### 2.8 Keine Divergenz bei ApplicationConfiguration

- v2 hat **keine eigene Speicherung** für AC-Felder; Lesen/Schreiben geht über eine gemeinsame
  Funktion in `pl-store` (bzw. bis dahin `pl-settings::appconfig`), die genau die Spalten der Django-Tabelle
  liest/schreibt. Zeile anlegen wie upstream `get_or_create(pk=1)`-Verhalten.
- Paritätstest: Wert über v2 setzen → v1 `GET /api/config/` liefert ihn; über v1 setzen → v2 zeigt ihn
  mit Herkunft `appconfig`. Läuft in `tests/parity` beide Male grün (ARCHITECTURE-V2 §4).
- `app_logo` (Datei-Upload) bleibt in v2 über `PUT /rs/api/v2/settings/app_logo` (multipart), intern
  derselbe Speicherweg wie v1.

---

## 3. API v2

Alle unter `/rs/api/v2/settings`, Fehlerformat v2 (`{"error":{"code","message","fields"}}`).

| Methode | Pfad | Zweck |
|---|---|---|
| GET | `/settings?category=&q=&class=` | Liste mit Metadaten |
| GET | `/settings/schema` | nur Schema (für Formulare, zwischenspeicherbar, ETag) |
| GET | `/settings/{key}` | Einzelwert |
| PATCH | `/settings` | `{"values":{KEY:wert,…},"expected_rev":N}` atomar; 409 `rev_conflict`, 409 `locked_by_environment`, 422 mit `fields` |
| DELETE | `/settings/{key}` | auf Standard (DB-Zeile löschen bzw. AC-Spalte NULL) |
| GET | `/settings/history?key=&cursor=` | Verlauf |
| POST | `/settings/restart` | Neustart (nur wenn `restart_supported`) |
| GET | `/settings/origin` | Umzugshilfe: Werte aus Umgebung/Datei, übernehmbar ja/nein |
| POST | `/settings/adopt` | `{"keys":[…]}` Umgebungs-/Dateiwerte in DB übernehmen |
| GET | `/settings/slim-env` | erzeugte minimale `docker-compose.env` (C/D + noch nicht übernommene) |
| POST | `/settings/check-file` | hochgeladene `paperless.conf`/`docker-compose.env`/Compose-YAML prüfen |
| POST | `/settings/export`, `/settings/import` | 2.7 |

Element der Liste:

```json
{ "key": "PAPERLESS_EMAIL_HOST", "name": "PAPERRUST_EMAIL_HOST", "aliases": ["PAPERLESS_EMAIL_HOST"], "category": "email",
  "class": "A", "effect": "live", "reindex": false, "secret": false,
  "type": {"kind": "str"}, "default": "localhost",
  "value": "smtp.example.ch", "is_set": true,
  "source": "db",              // env | file | db | appconfig | desktop | default
  "source_detail": null,        // z. B. "PAPERLESS_EMAIL_HOST" oder "/etc/paperrust.conf"
  "shadowed": {"source": "file", "value": "localhost"},
  "locked": false, "lock_reason": null,
  "restart_pending": false, "django_consumer": true,
  "updated_at": "2026-11-09T10:00:00Z", "updated_by": "admin",
  "help": "SMTP-Server für ausgehende Mails", "doc": "/rs/docs/configuration#PAPERRUST_EMAIL_HOST" }
```

`key` ist der kanonische Schlüssel (Bezeichner der API, Spalte `paperrust_setting.key`, Teil der
Verschlüsselung der Geheimnisse) und bleibt für upstream-Einstellungen `PAPERLESS_X`. Angezeigt wird
`name` (`PAPERRUST_X`, 2026-11-17); `aliases` nennt die weiteren gültigen Namen (früherer Name
`PAPERLESS_X` bzw. `PAPERLESS_RS_X`). Alle Namen werden als `key` in Pfaden und Änderungen angenommen.

Antwortkopf der Liste: `rev`, `restart_supported`, `restart_pending_count`, `mixed_mode`.

**Rechte:**
- Systemeinstellungen (lesen und schreiben): nur **Superuser** – Werte wie SMTP-Host, SSO-Gruppen,
  Pfade sind Betriebsinformation.
- AC-Schlüssel: zusätzlich wer die v1-Berechtigung für `/api/config/` hat (gleiche Prüfung wie
  `pl-authz` für ApplicationConfiguration) – sonst könnte v2 strenger/lockerer sein als v1.
- `desktop.*`: Superuser **und** Loopback (2.6).
- Restart/Export mit Geheimnissen: Superuser + Passwortbestätigung.
- Pro Nutzer: `/rs/api/v2/ui_settings` (vorhanden geplant, FRONTEND-LIGHT) bleibt; Vorgaben für neue
  Nutzer (`PAPERRUST_UI_DEFAULTS`) über `/settings`.

---

## 4. Oberfläche

### 4.1 Light-UI: Reiter „System“

Ort: `frontend-light/src/components/admin/settings/` – neuer Reiter **„System“** neben Allgemein/
Benachrichtigungen/Ansichten, Route `/rs/app/settings/system[/<kategorie>]`, sichtbar nur für
Superuser bzw. AC-Berechtigte (dann nur AC-Kategorien).

- Links Kategorien (1.1–1.16a in Nutzersprache: Allgemein & Darstellung, Dokumente & Dateinamen,
  Einlesen, Barcodes, OCR, Mail-Abruf, E-Mail-Versand, Anmeldung & SSO, Aufgaben & Zeitpläne,
  KI, Integrität & Sync, Protokolle, Netzwerk & Sicherheit, Speicher & Datenbank, Desktop).
  Oben Suchfeld (Schlüssel, Bedeutung, Env-Name), Filter „nur geänderte“, „nur Neustart nötig“.
- Je Einstellung: Bezeichnung, Hilfetext, Link „?“ → `/rs/docs/configuration#<ANKER>` (bzw.
  `PAPERRUST_DOCS_URL`), Env-Name klein und kopierbar, Steuerelement nach Typ (Schalter, Zahl mit
  Grenzen, Auswahl, Liste als Chips, JSON-Editor mit Prüfung, Crontab mit Klartext-Vorschau
  „jeden Tag um 01:00“, Zeitzonen-Auswahl, Tesseract-Sprachen aus installierten Paketen).
- Abzeichen: **„aus Umgebung“** (gesperrt, Tooltip mit Variablenname), **„Desktop-App“**,
  **„überschreibt Umgebung“** (AC), **„überschreibt paperrust.conf“**, **„Neustart nötig“**,
  **„Index neu aufbauen“**, **„Geheimnis“** (Feld leer mit Platzhalter „gesetzt“/„nicht gesetzt“),
  **„nur Mischbetrieb“** (X, grau, eingeklappt), **„wirkt nicht auf Django-Pfade“**.
- C/D-Einträge: nur Anzeige mit Satz „Wird beim Start gelesen – über Umgebung/`paperrust.conf`
  setzen“ bzw. Begründung der Klasse D. Geheimnisse maskiert.
- Speichern pro Kategorie (ein PATCH), Konflikt → Hinweis „inzwischen geändert, neu laden“.
- Leiste „Neustart nötig“ mit Knopf (wenn `restart_supported`).
- Reiter „Herkunft & Umzug“ (Abschnitt 5) und „Verlauf“.
- Texte im eigenen Katalog `frontend-light/i18n/<locale>.json` (10 Sprachen; früher `i18n-extra.json`).

### 4.2 Angular-UI (upstream, Patch)

> Entfällt seit 2026-11-13: die Angular-Oberfläche ist abgelöst (docs/FRONTEND-PARITY.md).

`frontend/patches/10-system-settings-link.patch`: auf der Seite **Konfiguration** (`/config`) ein
Hinweisbanner „Weitere Systemeinstellungen (E-Mail, Anmeldung, Zeitpläne …) → *Systemeinstellungen
öffnen*“ (Link `/rs/app/settings/system`), dazu ein Eintrag im Admin-Menü der Seitenleiste. Die
Angular-Konfigurationsseite selbst bleibt unverändert und schreibt weiter AC über v1 – dieselbe Zeile
wie v2 (2.8), also keine Divergenz. Kein Nachbau des Systemreiters in Angular.

### 4.3 Desktop

- Im Reiter „System“ Kategorie **Desktop** (nur wenn `PAPERRUST_SUPERVISOR_CONTROL` gesetzt und
  Anfrage von Loopback): LAN-Freigabe + LAN-Adresse, Port, Browser beim Start öffnen,
  Sicherung (Zeitplan, Anzahl, „jetzt sichern“), Updates prüfen, OCR-Engine (`PAPERRUST_OCR_ENGINE`,
  ist ein A-Schlüssel, steht zusätzlich bei OCR), Zeitzone/OCR-Sprache (normale Schlüssel).
- **Betriebsart/Profile** (`app.toml`): eigene Hüllen-Seite `/rs/app/settings/desktop` über
  Tauri-Befehle, im Tray „Einstellungen …“. Moduswechsel startet die Hülle neu (vorhandener
  `PAPERRUST_DESKTOP_RESTART_AFTER`-Weg).

---

## 5. Umzug für Umsteiger

1. Nach dem Umzug läuft alles unverändert: Umgebung/Datei gelten, alle Einträge stehen auf „aus
   Umgebung“ (gesperrt) bzw. „aus paperless.conf“.
2. Reiter **„Herkunft & Umzug“** listet alle Werte aus Umgebung/Datei, gruppiert:
   - *übernehmbar* (A/B): Knopf „in Datenbank übernehmen“ je Zeile und „alle übernehmen“
     → `POST /settings/adopt`. Der Wert steht dann in `paperrust_setting`, bleibt aber gesperrt,
     solange die Umgebung ihn setzt (Abzeichen „übernommen – kann aus der Umgebung entfernt werden“).
   - *muss in der Umgebung bleiben* (C/D), mit Begründung.
   - *wirkungslos in PaperRust* (X) – „kann entfernt werden“.
3. **„Schlanke Konfiguration herunterladen“** (`GET /settings/slim-env`): erzeugte
   `docker-compose.env` nur mit C/D und nicht übernommenen Werten, mit Kommentar je entfernter
   Variable. CLI: `paperrust manage settings slim-env < docker-compose.env > docker-compose.env.neu`.
4. **„Datei prüfen“**: `paperless.conf`, `docker-compose.env` oder `docker-compose.yml` hochladen
   (Parser: vorhandenes `parse_dotenv`; YAML: `environment:`/`env_file:` des Dienstes `webserver`) →
   gleiche Einteilung, ohne den laufenden Server zu ändern; „übernehmen“ schreibt die A/B-Werte in die DB.
5. Rückweg zu paperless-ngx: DB-Werte von `paperrust_setting` kennt upstream nicht →
   `paperrust manage settings export --format env` liefert die Variablen für paperless-ngx zurück.

### Minimale Docker-Konfiguration (Ziel)

```yaml
services:
  db:
    image: docker.io/library/postgres:17
    environment: { POSTGRES_DB: paperless, POSTGRES_USER: paperless, POSTGRES_PASSWORD: paperless }
    volumes: [pgdata:/var/lib/postgresql/data]
  paperrust:
    image: paperrust/paperrust
    restart: unless-stopped
    depends_on: { db: { condition: service_healthy } }
    ports: ["8000:8000"]
    volumes: [data:/usr/src/paperless/data, media:/usr/src/paperless/media,
              ./consume:/usr/src/paperless/consume, ./export:/usr/src/paperless/export]
    environment:
      PAPERLESS_DBHOST: db
      PAPERLESS_DBPASS: paperless
      PAPERLESS_SECRET_KEY: <zufällig, 64 Zeichen>
      # nur hinter Reverse-Proxy nötig, sonst später in der Oberfläche:
      # PAPERLESS_URL: https://docs.example.ch
volumes: { data: {}, media: {}, pgdata: {} }
```

`PAPERLESS_URL` ist Klasse B, wird aber hinter einem Proxy vor der ersten Anmeldung gebraucht
(CSRF-Origin) – daher im Beispiel als Kommentar. Alles andere (Zeitzone, OCR-Sprache, Mail, SSO,
Zeitpläne …) in der Oberfläche. Umgesetzt als eigene Vorlage `docker/docker-compose.slim.yml` mit
`docker/docker-compose.slim.env` (WP8); `docker-compose.postgres.yml`/`docker-compose.env` bleiben als
ausführliche Vorlage, die bisherigen Variablen bleiben gültig.

---

## 6. Arbeitspakete (je ein Agent)

Reihenfolge: WP1 → WP2 → (WP3, WP5) → (WP4a–d, WP6, WP7, WP8) → WP9. Jedes Paket: Paritätsläufe
(ARCHITECTURE-V2 §4) vor/nach unverändert, kein Eingriff in `upstream/`.

| WP | Inhalt | Eigene Dateien | Tests | Abnahme |
|---|---|---|---|---|
| **WP1 Schema-Register** | Crate `pl-settings` (ohne axum): `SettingDef`, `SCHEMA` mit allen Schlüsseln aus Abschnitt 1, Parser-Wiederverwendung aus `pl_core::config`, Schichtung (Env mit Herkunft/DB/Datei/Standard), `Settings`-Schnappschuss, AC-Vorrangfunktion | `crates/pl-settings/src/{lib,schema,layer,appconfig}.rs`; CORE-REQUEST `pl-core/config.rs` (Herkunft je Schlüssel in `Env`) | `inventory_complete` (jede `OWN_SETTINGS`, jede `PAPERLESS_*` aus `upstream/src/paperless/settings`, jede `PAPERRUST_*` per Quelltextsuche ist im Schema oder in einer Ausnahmeliste); `parse_parity` (Env- und GUI-Pfad liefern gleichen Wert für alle Typen); `appconfig_or_semantics` | Zählung A/B/C/D/X automatisch erzeugt und mit Doku abgeglichen |
| **WP2 Speicher + CLI** | Migration `paperrust_setting`/`_history`/Sequenz, Verschlüsselung, `manage settings list/get/set/reset/export/import/rekey` | `crates/pl-settings/migrations/0001_*.sql`, `src/{store,crypto}.rs`, `crates/pl-cli/src/settings.rs`; CORE-REQUEST Registrierung in `pl-cli/src/migrate.rs` | Rundlauf Ver-/Entschlüsselung, falscher SECRET_KEY → `unreadable`, Verlauf ohne Klartext, Migration idempotent | `cargo test -p pl-settings`, Migration auf Kopie der Testdatenbank |
| **WP3 Live-Übernahme + Neustart** | `ArcSwap` in `AppState`, LISTEN/NOTIFY + 30-s-Rückfall, `restart_pending`, Exit 75, Entrypoint-Schleife, Supervisor-Behandlung, systemd-Vorlage | `crates/pl-settings/src/reload.rs`; CORE-REQUEST `pl-core/state.rs`, `pl-server/src/shutdown.rs`; `docker/rootfs/usr/local/bin/paperrust-entrypoint`; `apps/desktop/src/supervisor.rs` (nur Exit-75-Zweig) | zwei Prozesse, Änderung in A ist in B < 2 s sichtbar; Exit 75 → Neustart ohne Zählung | E2E im Container: Wert ändern, Neustart über API, Wert wirksam |
| **WP4a Verbraucher Einlesen/OCR/Barcode** | Consumer, OCR-Engine-Wahl, Watcher (W), Datumserkennung, Dateinamen lesen aus Schnappschuss | `crates/pl-consumer`, `pl-ocr/src/engine_select.rs`, `pl-matching` (nur Lesestellen) | vorhandene Tests + „Änderung wirkt beim nächsten Dokument“ | Paritätsläufe unverändert |
| **WP4b Verbraucher Mail/SMTP/Anmeldung** | SMTP, OAuth-Mail, allauth-Entsprechungen, Drosselung, Sitzungsdauer, CORS/CSRF/ALLOWED_HOSTS | `pl-mail`, `pl-accounts`, `pl-auth-api`, `pl-authz` (CORE-REQUEST) | Login weiter möglich nach jeder Änderung; Aussperr-Schutz `DISABLE_REGULAR_LOGIN` | Paritätsläufe unverändert |
| **WP4c Zeitpläne/Worker/Queue/Sync/Integrität/Logs** | Zeitplaner neu laden, Pool-Größe, Sync-Parameter, `tracing`-Reload | `pl-queue`, `pl-sync`, `pl-server/src/{worker,logfile}.rs` | Crontab-Änderung ohne Neustart wirksam | – |
| **WP4d KI** | `pl-ai` liest `PAPERRUST_AI_*` aus Schnappschuss, AC wie bisher | `crates/pl-ai/src/config.rs` | MCP-Grenzen ändern ohne Neustart | – |
| **WP5 API v2** | Endpunkte Abschnitt 3, Rechte, AC über gemeinsame Funktion, multipart Logo | `crates/pl-api-v2/src/settings.rs` (+ Montage dort) | Rechte (Superuser/AC-Recht/Loopback), 409 gesperrt, 422 Validierung, Parität v1↔v2 für AC | OpenAPI-Abschnitt, Paritätsläufe unverändert |
| **WP6 Light-UI „System“** | Reiter, Kategorien, Suche, Steuerelemente, Abzeichen, Neustartleiste, Verlauf | `frontend-light/src/components/admin/settings/system/**`, `src/api/settings.ts`, `i18n-extra.json` | Komponententests, Playwright: gesperrtes Feld, Geheimnis, Neustart | Bedienbar auf 375 px Breite, hell/dunkel |
| **WP7 Desktop** | `PAPERRUST_SUPERVISOR_CONTROL`, `settings.request.json`, Umzug `time_zone`/`ocr_language`, `MANAGED_KEYS`, neue Schlüssel Sicherung/Updates, Hüllen-Seite `app.toml` | `apps/desktop/src/{config,server,settings_ctl}.rs`, `apps/desktop-shell/src/mode.rs` (Befehle), `frontend-light/src/components/desktop/**` | Anfrage nur von Loopback; LAN an/aus über GUI; Umzug idempotent | E2E auf frischem Profil (docs/DESKTOP.md) |
| **WP8 Umzugshilfe + Docker** | `/settings/origin`, `adopt`, `slim-env`, `check-file` (dotenv/YAML), CLI `adopt`/`slim-env`; verschlankte Compose-Dateien; docs/user/configuration.md Abschnitt „Woher die Einstellungen kommen“ | `crates/pl-settings/src/migration.rs`, `crates/pl-api-v2/src/settings_migration.rs`, `docker/docker-compose*.yml`, `docker/docker-compose.env`, `docs/user/configuration.md` | upstream-`docker-compose.env`-Beispiele aus `upstream/docker/compose` korrekt eingeteilt | Umstiegsszenario: ngx-Compose → PaperRust → adopt → schlanke Env → gleiche Wirkung |
| **WP9 Angular-Patch + Doku-Links** | Banner/Link auf `/config`, Menüeintrag; Doku-Anker für jeden Schlüssel prüfen | `frontend/patches/10-system-settings-link.patch`, `frontend/doc-links.json` | Patch-Serie baut; Link-Prüfer: jeder `doc`-Anker existiert | Angular-E2E unverändert grün |

---

## 7. Offene Punkte

- Passwortregeln konfigurierbar machen (`PAPERRUST_PASSWORD_MIN_LENGTH`); Mischbetrieb mit Django gibt
  es seit 2026-11-14 nicht mehr.
- `SOCIALACCOUNT_PROVIDERS` als JSON ist unhandlich – späteres Formular „OIDC-Anbieter hinzufügen“
  (Name, Issuer-URL, Client-ID, Secret) erzeugt das JSON (WP4b-Folge).
- Ob `CONSUMPTION_DIR` außerhalb von Docker GUI-fähig (B-W) werden soll – vorerst C wegen
  Volume-Bindung.
- Zählung in 1.19 wird von WP1 maschinell gegengeprüft; Abweichungen dort korrigieren.
- Ältere paperless-ngx-Namen, die es in v3.4.2 nicht mehr gibt (z. B. `PAPERLESS_CONSUMER_INOTIFY_DELAY`
  → heute `CONSUMER_STABILITY_DELAY`; Zeichenbegrenzung der Datumssuche; `FILENAME_PARSE_TRANSFORMS`),
  zeigt die Umzugshilfe als „unbekannt/veraltet“ an statt sie still zu ignorieren.

---

## 8. Umsetzungsstand (Stand 2026-11-09)

| WP | Stand | Ort |
|---|---|---|
| WP1 | fertig – Register 327 Schlüssel, `inventory_complete`, `parse_parity`, `appconfig_or_semantics` | `crates/pl-settings/src/{schema,value,layer,appconfig}.rs` |
| WP2 | fertig – Migration, Verschlüsselung, Verlauf, CLI `manage settings list/get/set/reset/export/import/rekey` | `crates/pl-settings/src/{store,crypto}.rs`, `crates/pl-cli/src/settings.rs` |
| WP3 | fertig bis auf E2E im Container – LISTEN/NOTIFY + 30-s-Rückfall, `restart_pending`, Exit 75 (Server, Entrypoint-Schleife, Desktop-Supervisor, systemd-Vorlage) | `crates/pl-settings/src/{reload,restart}.rs`, `pl-server/src/{cli,shutdown}.rs` |
| WP5 | fertig – Liste, Schema (ETag), Einzelwert, PATCH (`expected_rev`/`If-Match`), DELETE, Verlauf, Neustart (Passwort), Logo (multipart, Prüfung wie v1) | `crates/pl-api-v2/src/settings{,_logo}.rs` |
| WP4a–d | fertig bis auf die unten genannten Reste | `pl-core/src/live.rs`, `pl-envlive`, `pl-server/src/settings_live.rs` |
| WP6 | Reiter „System“ (Kategorien, Suche, Filter, Abzeichen, Sperre, Geheimnisse write-only, Neustartleiste, Herkunft & Umzug, Verlauf, Export/Import) | `frontend-light/src/components/admin/settings/system/` |
| WP7 | Supervisor-Weg `settings.request.json` (Port, Browser öffnen, LAN-Freigabe/-Adresse; Neustart bei Bedarf), Umzug `time_zone`/`ocr_language` in die DB, `MANAGED_KEYS`, `SUPERVISOR_CONTROL`. Offen: Sicherungs-Zeitplan, Updates (gehören der Hülle/`app.toml`), Hüllen-Seite `app.toml` | `apps/desktop/src/settings_ctl.rs`, `crates/pl-settings/src/desktop.rs` |
| WP8 | Herkunft, Übernehmen (`keys`/`values`), schlanke Env, Datei prüfen (dotenv/Compose-YAML), Export/Import mit Passphrase (XChaCha20-Poly1305 + PBKDF2, nicht Fernet), CLI `origin/adopt/slim-env/check-file`, schlanke Compose-Vorlage `docker/docker-compose.slim.yml` + `docker-compose.slim.env` (nur `DBHOST`, `DBPASS`, `SECRET_KEY`, optional `URL`), Doku-Abschnitt „Einstellungen in der Oberfläche“ (`docs/user/configuration.md#settings-ui`, englisch `docs/user/en/…`) | `crates/pl-settings/src/{migration,transfer}.rs`, `crates/pl-api-v2/src/settings_migration.rs`, `docker/docker-compose.slim.*` |
| Hilfetexte en | fertig – englische Kurzbeschreibung je Schlüssel (`pl_settings::help_en::HELP_EN`, Test `help_en_complete`: genau ein Eintrag je `SCHEMA`-Schlüssel), API-Feld `help_en` additiv in Liste/Einzelwert/Schema (Suche `q` findet beide Sprachen); Reiter „System“ zeigt in `de*` `help`, sonst `help_en` (über das Wörterbuch übersetzbar, Schlüssel = englischer Text) | `crates/pl-settings/src/help_en.rs`, `crates/pl-api-v2/src/settings.rs`, `frontend-light/src/components/admin/settings/system/system.ts` |

**Wie Werte wirken (Umsetzung von §2.5):**

- Beim Start lädt `pl_settings::reload::startup_overlay` die DB-Werte der Regel N **vor** dem Lesen
  der Konfiguration; `Config`/`ServerConfig` enthalten sie also ab Start (auch S-Schlüssel).
- Danach setzt jeder neue Schnappschuss eine prozessweite Überlagerung
  (`pl_core::config::set_live_overlay`, Vorrang Prozess > DB > Datei): sie wirkt auf
  `pl_core::config::var`, `Env::from_process` und `vars_live`. Nur L/W-Schlüssel ändern sich im
  Betrieb; S/1-Schlüssel behalten ihren Startwert bis zum Neustart (`restart_pending`).
- `AppState::live_config()` (Dienst `pl_core::live::LiveConfig`) liefert die neu gebaute `Config`.
- Live umgestellt: SMTP für „Dokumente mailen“ und Workflow-Mails (bei Consumer-Neuaufbau),
  Login-Drosselung, Sitzungsdauer (pl-auth-api), Konto-/SSO-Einstellungen (pl-accounts, bei
  geänderter Konfiguration neu gebaut), `FILENAME_FORMAT*` (pl-documents-write), ui_settings-
  Ableitungen (`email_enabled`, `trash_delay`), KI-Umgebungswerte (pl-ai, neu gelesen bei
  geänderter Überlagerung), Einlesen ab dem nächsten Dokument (Consumer wird neu gebaut, wenn sich
  Einstellungs-Stand oder AC-Zeile ändern – damit gelten auch v1-Änderungen der OCR-Felder ohne
  Neustart), `PAPERRUST_LOG` (tracing-Reload).
- W-Teilsysteme mit Neustart im Prozess: Zeitplaner (alle Crontabs), Ordnerüberwachung,
  Sync-Sequenzer (`settings_live::restartable`).

Ebenfalls live: Worker-Pool (`TASK_WORKERS`, `PAPERRUST_QUEUE_*`, `WORKER_TIMEOUT` – Pool startet
nach Ende laufender Aufgaben neu), `ENABLE_COMPRESSION`, `LOGOUT_REDIRECT_URL` (über pl-accounts),
direkte Leser in pl-ocr/pl-pdfops/pl-templating über `pl_envlive::var` (Überlagerung ohne pl-core).

**Offen:** (Mischbetrieb erledigt: seit 2026-11-14 keine Django-Weiterleitung mehr);
Begründungen der Umzugshilfe (`reason`) kommen nur englisch; Pfade von Tesseract/PDFium
(Klasse C) lesen weiterhin direkt die Umgebung.
