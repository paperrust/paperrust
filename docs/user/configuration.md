# Konfiguration

[English](en/configuration.md) · [Übersicht](README.md)

PaperRust liest seine Einstellungen aus Umgebungsvariablen. Alle Variablen heißen `PAPERRUST_…`.
Die Namen von paperless-ngx (`PAPERLESS_…`) und die alten Namen `PAPERLESS_RS_…` gelten weiterhin
(siehe [Namen der Variablen](#naming)). Wer von paperless-ngx umsteigt, kann seine `paperless.conf`
oder `docker-compose.env` also einfach weiterverwenden. Fast alles lässt sich außerdem in der Oberfläche einstellen. In der
Umgebung müssen dann nur noch die Datenbank, der `SECRET_KEY` und ein paar Startwerte stehen –
siehe [Einstellungen in der Oberfläche](#settings-ui).

Diese Seite hat zwei Teile: zuerst, wie die Einstellungen zusammenspielen und welche es nur in
PaperRust gibt; danach die [Referenz](#reference) aller Einstellungen, die PaperRust mit
paperless-ngx teilt.

<a id="naming"></a>

## Namen der Variablen: `PAPERRUST_*` und `PAPERLESS_*`

| Form | Beispiel | Status |
|---|---|---|
| `PAPERRUST_<NAME>` | `PAPERRUST_OCR_LANGUAGE=deu+eng` | **Hauptname** jeder Einstellung |
| `PAPERLESS_<NAME>` | `PAPERLESS_OCR_LANGUAGE=deu+eng` | gleichwertiger Zweitname, damit Dateien von paperless-ngx weiter funktionieren |
| `PAPERLESS_RS_<NAME>` | `PAPERLESS_RS_BIND` | alter Arbeitsname des Projekts; wird noch angenommen, PaperRust warnt aber einmal je Name |

Stehen `PAPERRUST_<NAME>` und `PAPERLESS_<NAME>` beide in der Umgebung, zählt `PAPERRUST_<NAME>`.
Die verbindliche Namensliste steht in [docs/BRANDING.md](../BRANDING.md) (Abschnitt
„Umgebungsvariablen“).

> [!NOTE]
> Django und Celery gibt es in PaperRust nicht mehr; Variablen nur für sie (etwa ein früheres
> `PAPERRUST_DJANGO_URL`) sind wirkungslos und erzeugen höchstens einen Hinweis im Log.

<a id="config-file"></a>

## Woher die Einstellungen kommen

PaperRust sucht jeden Wert in dieser Reihenfolge und nimmt den ersten Treffer:

1. Umgebung des Prozesses.
2. In der Oberfläche gespeicherte Werte (*Einstellungen → System*, Tabelle `paperrust_setting`),
   siehe [Einstellungen in der Oberfläche](#settings-ui).
3. Eine Konfigurationsdatei: der Pfad aus `PAPERRUST_CONFIGURATION_PATH` (oder
   `PAPERLESS_CONFIGURATION_PATH`), sonst die erste vorhandene Datei aus `../paperrust.conf`,
   `../paperless.conf`, `/etc/paperrust.conf`, `/etc/paperless.conf`,
   `/usr/local/etc/paperrust.conf`, `/usr/local/etc/paperless.conf`. Was schon in der Umgebung
   steht, ersetzt die Datei nicht.
4. Eingebauter Standardwert.

Eine Ausnahme gibt es: Die Optionen der Seite *Einstellungen → Konfiguration* (im PaperRust-Design
*Verwalten → Konfiguration → Anwendung*; OCR, Barcodes, KI,
Titel und Logo; Tabelle `paperless_applicationconfiguration`) gehen der Umgebung vor, sobald sie in
der Datenbank einen Wert haben. Das verhält sich genauso wie bei paperless-ngx.

<a id="settings-ui"></a>

## Einstellungen in der Oberfläche

Unter **Einstellungen → System** (`/rs/app/settings/system`; im PaperRust-Design *Verwalten →
Konfiguration → Server*) stellt man systemweite Dinge ein:
E-Mail-Versand, Anmeldung und SSO, Zeitpläne, Einlesen, OCR, KI, Protokolle, Zeitzone und vieles
mehr. Den Reiter sehen Superuser. Wer nur das Recht für die Seite *Konfiguration* hat, sieht nur
deren Optionen. Die Werte liegen in der Datenbank (Tabelle `paperrust_setting`) und gelten für jeden
Prozess, der an dieser Datenbank hängt.

Zu jeder Einstellung zeigt die Seite Bezeichnung, aktuellen Wert, Herkunft und Variablennamen. Das
„?“ springt zum passenden Eintrag auf dieser Seite. Die Abzeichen bedeuten:

| Abzeichen | Bedeutung |
|---|---|
| **aus Umgebung** | Die Variable steht in der Umgebung, das Feld ist deshalb gesperrt. Ändern: Variable entfernen oder anpassen, dann neu starten. |
| **überschreibt Umgebung** | Option der Seite *Konfiguration*; ihr Wert in der Datenbank ersetzt den aus der Umgebung. |
| **überschreibt paperrust.conf** | Der Wert in der Datenbank ersetzt einen Wert aus der Konfigurationsdatei. |
| **Neustart nötig** | Der Wert wirkt erst nach einem Neustart. Läuft PaperRust unter einem Aufseher (Docker-Entrypoint, systemd-Vorlage, Desktop-App), bietet die Leiste oben „Jetzt neu starten“ an. |
| **Index neu aufbauen** | Nach der Änderung Suchindex bzw. Dateinamen neu aufbauen. |
| **Geheimnis** | Der Wert wird nie angezeigt, nur „gesetzt“ oder „nicht gesetzt“. Ein leeres Feld lässt ihn, wie er ist. |
| **ohne Wirkung** | Stammt aus paperless-ngx und wirkt in PaperRust nicht; kann entfernt werden. |

Die meisten Änderungen gelten sofort, etwa SMTP, Sitzungsdauer, Dateinamen-Vorlage oder
Protokoll-Filter; Einstellungen zum Einlesen gelten ab dem nächsten Dokument. Zeitpläne,
Ordnerüberwachung und Worker startet PaperRust bei Bedarf selbst neu. Gespeichert wird je
Kategorie. Hat inzwischen jemand anderes dieselbe Kategorie geändert, meldet die Seite einen
Konflikt. **Verlauf** zeigt, wer wann was geändert hat (Geheimnisse als `***`).
**Einstellungen exportieren/importieren** sichert die Werte aus der Datenbank als JSON; Geheimnisse
kommen nur mit, wenn man eine Passphrase angibt, und sind dann damit verschlüsselt.

### Vorrang

| Art der Einstellung | Reihenfolge (erste gewinnt) | In der Oberfläche |
|---|---|---|
| Normale Einstellungen | Umgebung → Datenbank → Konfigurationsdatei → Standard | änderbar, solange die Umgebung sie nicht setzt |
| Optionen der Seite *Konfiguration* (OCR, Barcodes, KI, Titel/Logo) | Datenbank (wenn gesetzt) → Umgebung → Konfigurationsdatei → Standard; `0` oder leer gilt als „nicht gesetzt“ | immer änderbar; „Zurücksetzen“ lässt wieder die Umgebung gelten |
| Startwerte (Datenbank, `SECRET_KEY`, Pfade, Adresse …) | Umgebung → Konfigurationsdatei → Standard | nur Anzeige mit dem Hinweis „über Umgebung/`paperrust.conf` setzen“ (Desktop-App: in deren Einstellungen) |
| Grenze zum Host (Proxy-Köpfe, Remote-User, Auto-Login, Skripte, Zugriff auf interne Adressen, `DEBUG`) | nur Umgebung | nur Anzeige – ein Administrator-Konto in der Oberfläche soll weder Programme auf dem Server starten noch die Anmeldung aushebeln können |
| Ohne Wirkung in PaperRust (Tika, Flower, Django-Cache …) | – | ausgeblendet; sichtbar mit dem Schalter „Start- und Umgebungswerte anzeigen“ |

Dass bei normalen Einstellungen die Umgebung gewinnt, ist Absicht. Wer mit Compose-Dateien
arbeitet, behält so die Kontrolle. Und eine verkorkste Einstellung lässt sich immer reparieren:
Variable setzen und neu starten, oder ohne laufenden Server
`paperrust manage settings reset <VARIABLE>`.

Geheimnisse (SMTP-Passwort, OAuth-Secrets, SSO-Anbieter …) speichert PaperRust verschlüsselt. Der
Schlüssel dafür wird aus `PAPERRUST_SECRET_KEY` abgeleitet. Wer den `SECRET_KEY` wechselt, ruft
danach `paperrust manage settings rekey --old-secret-key <ALT>` auf. Im Klartext liegen – wie bei
paperless-ngx – die API-Schlüssel der Seite *Konfiguration* (KI, Remote-OCR) und die Passwörter der
Mailkonten.

### Umzug: Umgebung in die Datenbank übernehmen

Nach dem Umstieg von paperless-ngx läuft alles wie vorher: Jede Variable gilt weiter, die Einträge
tragen das Abzeichen „aus Umgebung“. Wer die Umgebung verschlanken will, nimmt den Reiter
**Herkunft & Umzug** (in *Einstellungen → System*):

1. Er zeigt alle Werte aus Umgebung, `paperrust.conf` (bzw. `paperless.conf`) und Desktop-App und sortiert sie:
   *übernehmbar*, *muss in der Umgebung bleiben* (mit Grund), *ohne Wirkung* oder *veraltet*. Die
   letzten beiden kann man einfach löschen.
2. **In Datenbank übernehmen** (je Zeile) oder **Alle in die Datenbank übernehmen** kopiert die
   übernehmbaren Werte in die Datenbank. Solange die Variable noch in der Umgebung steht, bleibt
   das Feld gesperrt („bereits in der Datenbank – kann aus der Umgebung entfernt werden“).
3. **Schlanke Konfiguration herunterladen** liefert eine `docker-compose.env`, die nur noch
   enthält, was in der Umgebung bleiben muss. Jede weggelassene Variable steht als Kommentar
   darin.
4. **Konfigurationsdatei prüfen**: `paperrust.conf`, `paperless.conf`, `docker-compose.env` oder
   `docker-compose.yml` hochladen. Bei YAML zählen `environment:` und `env_file:` des Dienstes
   `webserver`, sonst des ersten Dienstes mit `PAPERRUST_*`- oder `PAPERLESS_*`-Variablen. Die Datei wird genauso
   sortiert; geändert wird erst, wenn man bestätigt.
5. Die schlanke Datei einsetzen und den Container neu starten.

Auf der Kommandozeile geht dasselbe (im Container mit
`docker compose exec -T paperrust paperrust manage …`):

```sh
paperrust manage settings origin                       # Herkunft und Einteilung anzeigen
paperrust manage settings adopt --all                  # alle übernehmbaren Werte in die DB
paperrust manage settings adopt PAPERRUST_EMAIL_HOST   # nur eine Variable
paperrust manage settings slim-env < docker-compose.env > docker-compose.env.neu
paperrust manage settings check-file docker-compose.yml
```

Zurück zu paperless-ngx: Die Tabelle `paperrust_setting` kennt paperless-ngx nicht.
`paperrust manage settings export --format env` gibt die Werte deshalb als Variablen aus.

### Schlanke Docker-Konfiguration

Für eine neue Installation liegen `docker/docker-compose.slim.yml` und
`docker/docker-compose.slim.env` bei. In der Umgebung steht dort nur:

| Variable | Wozu |
|---|---|
| `PAPERRUST_DBHOST` | Datenbank-Host; damit gilt `PAPERRUST_DBENGINE=postgresql` (`DBNAME`/`DBUSER` sind `paperless`) |
| `PAPERRUST_DBPASS` | Datenbank-Passwort (gleich wie `POSTGRES_PASSWORD` des Dienstes `db`) |
| `PAPERRUST_SECRET_KEY` | Pflicht; zugleich Schlüssel für die verschlüsselten Einstellungen |
| `PAPERRUST_URL` | optional; nur hinter einem Reverse Proxy schon vor der ersten Anmeldung nötig (CSRF), sonst in der Oberfläche |

```sh
cd docker
cp docker-compose.slim.env .env.paperrust      # anpassen (SECRET_KEY!), env_file in der YAML umstellen
docker compose -f docker-compose.slim.yml up -d
docker compose -f docker-compose.slim.yml exec paperrust createsuperuser
```

Danach Zeitzone, OCR-Sprache, E-Mail-Versand, Anmeldung usw. unter *Einstellungen → System*
setzen. Die ausführlichen Vorlagen (`docker-compose.postgres.yml` mit `docker-compose.env`) gehen
weiterhin; jede Variable, die dort steht, sperrt das passende Feld in der Oberfläche.

<a id="paperrust-settings"></a>

## Einstellungen, die es nur in PaperRust gibt

Angegeben ist der Hauptname `PAPERRUST_*`. Standardwerte laut Quellcode (Stand 2026-11-18).

### Server

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="paperrust_bind"></a>`PAPERRUST_BIND` | `127.0.0.1:8080` | Adresse `IP:PORT`, auf der PaperRust lauscht. Ein ungültiger Wert bricht den Start ab. Eine Adresse außerhalb des eigenen Rechners muss man ausdrücklich setzen; PaperRust warnt dann im Log. Im Docker-Image baut der Entrypoint die Adresse aus [`PAPERRUST_BIND_ADDR`/`PAPERRUST_PORT`](#paperrust_bind_addr), wenn `PAPERRUST_BIND` fehlt (dort `0.0.0.0:8000`). |
| <a id="paperrust_base_dir"></a>`PAPERRUST_BASE_DIR` | Arbeitsverzeichnis | Wurzel der Installation; darunter liegen standardmäßig `media/`, `data/`, `consume/` und `static/`. |
| <a id="paperrust_routes"></a>`PAPERRUST_ROUTES` | `routes.toml` im Arbeitsverzeichnis | Datei, die je Route festlegt, wer sie bedient. Ist der Pfad gesetzt, die Datei aber nicht da, startet PaperRust nicht. Docker-Image und Desktop-App bringen die Datei mit. |
| <a id="paperrust_light_ui"></a>`PAPERRUST_LIGHT_UI` | `on` | `off` schaltet die Oberfläche unter `/rs/app/` ab. |
| <a id="paperrust_light_ui_dir"></a>`PAPERRUST_LIGHT_UI_DIR` | `STATICDIR/../light` | Ordner mit den Dateien der Oberfläche. |
| <a id="paperrust_api_v2"></a>`PAPERRUST_API_V2` | `on` | `off` schaltet die eigene v2-API ab. |
| <a id="paperrust_db_max_connections"></a>`PAPERRUST_DB_MAX_CONNECTIONS` | `10` | Größe des Datenbank-Pools, falls weder `pool.max_size` in [`PAPERRUST_DB_OPTIONS`](#paperrust_db_options) noch [`PAPERRUST_DB_POOLSIZE`](#paperrust_db_poolsize) gesetzt ist. |
| <a id="paperrust_db_require_auth"></a>`PAPERRUST_DB_REQUIRE_AUTH` | – (keine Prüfung) | Erlaubte Anmeldeverfahren an der Datenbank, z. B. `scram-sha-256`. PaperRust prüft das beim Start. Nur über die Umgebung. |
| <a id="paperrust_instance_id"></a>`PAPERRUST_INSTANCE_ID` | – | Kennung dieser Instanz; erscheint als `instance` in `/__rs/health`. |
| <a id="paperrust_log"></a>`PAPERRUST_LOG` | `info` | Wie gesprächig das Log ist (`tracing`-Schreibweise), z. B. `info,sqlx=warn`. |
| <a id="paperrust_log_files"></a>`PAPERRUST_LOG_FILES` | `yes` | `no`: keine Protokolldateien `paperrust.log`/`mail.log` in [`PAPERRUST_LOGGING_DIR`](#paperrust_logging_dir); die Seite „Protokolle“ bleibt dann leer. Zeilenformat `[Zeit] [STUFE] [Logger] Nachricht`; Rotation siehe [Protokollierung](#logging). |
| <a id="paperrust_docs_url"></a>`PAPERRUST_DOCS_URL` | – | Diese Hilfe ist ins Programm eingebaut und liegt unter `/rs/docs/` (in der Sprache der Oberfläche, auch offline). Ist hier eine http(s)-Adresse mit `/` am Ende gesetzt (gleicher Aufbau wie `docs/user/`), leitet `/rs/docs/…` dorthin um. |
| <a id="paperrust_share_mode"></a>`PAPERRUST_SHARE_MODE` | – | Nur Desktop-App (setzt sie selbst): `local` zeigt im Freigabe-Dialog „Links funktionieren nur auf diesem Computer“; `lan` baut Freigabelinks mit der Adresse im Heimnetz. Auf einem Server bleibt das ungesetzt. |
| <a id="paperrust_share_url"></a>`PAPERRUST_SHARE_URL` | – (dann `PAPERRUST_URL`) | Adresse in Freigabelinks bei `PAPERRUST_SHARE_MODE=lan`. |
| <a id="paperrust_stop_on_stdin_eof"></a>`PAPERRUST_STOP_ON_STDIN_EOF` | `no` | `yes`: sauber beenden, sobald die Standardeingabe geschlossen wird (nutzt die Desktop-App). |
| <a id="paperrust_init"></a>`PAPERRUST_INIT` | `yes` | Nur Docker-Image: `no` überspringt beim Start Datenbank-Migration und Anlegen des Administrators (z. B. für einen zweiten Prozess mit `--worker-only`). |
| <a id="paperrust_init_db_timeout"></a>`PAPERRUST_INIT_DB_TIMEOUT` | `300` s | Nur Docker-Image: so lange wartet der Start auf die Datenbank. |
| <a id="paperrust_migrate_source"></a><a id="paperrust_migrate_target"></a>`PAPERRUST_MIGRATE_SOURCE`, `PAPERRUST_MIGRATE_TARGET` (auch `PAPERLESS_MIGRATE_*`) | – | Quelle und Ziel für `paperrust-migrate-db` ([Umstieg](migration.md#sqlite-mariadb)). |
| <a id="paperrust_migrate_api_token"></a>`PAPERRUST_MIGRATE_API_TOKEN` | – | API-Token für das Umzugswerkzeug. |
| <a id="paperrust_pg_bin"></a>`PAPERRUST_PG_BIN` | – | Ordner mit `pg_dump` für die Sicherung beim Umstieg. |
| <a id="paperrust_datamodel_backup"></a>`PAPERRUST_DATAMODEL_BACKUP` | `on` | Vor den Umbauschritten am Datenmodell eine Sicherung mit `pg_dump` anlegen. |
| <a id="PAPERRUST_MIGRATE_WITHOUT_BACKUP"></a><a id="paperrust_migrate_without_backup"></a>`PAPERRUST_MIGRATE_WITHOUT_BACKUP` | `false` | Fehlt `pg_dump`, bricht ein Umbau des Datenmodells ab, statt ohne Sicherung weiterzumachen. `true` macht trotzdem weiter – nur, wenn Sie selbst gesichert haben. |
| <a id="PAPERRUST_DB_ACQUIRE_TIMEOUT"></a><a id="paperrust_db_acquire_timeout"></a>`PAPERRUST_DB_ACQUIRE_TIMEOUT` | `30` s | So lange wartet eine Anfrage höchstens auf eine freie Datenbank-Verbindung. Geht vor `PAPERRUST_DB_TIMEOUT`. Wirkt nach Neustart. |
| <a id="PAPERRUST_DB_LONGPOLL_CONNECTIONS"></a><a id="paperrust_db_longpoll_connections"></a>`PAPERRUST_DB_LONGPOLL_CONNECTIONS` | `8` | Eigene Datenbank-Verbindungen für Apps, die beim Abgleich auf Änderungen warten. So nehmen wartende Apps den anderen Anfragen keine Verbindung weg. |
| <a id="PAPERRUST_MEDIA_LOCK_TIMEOUT"></a><a id="paperrust_media_lock_timeout"></a>`PAPERRUST_MEDIA_LOCK_TIMEOUT` | `120` s | So lange wartet ein Schreibvorgang höchstens auf die Sperre der Dateiablage (`0` = ohne Grenze). Danach bricht er mit einer Meldung ab, statt zu hängen. |
| <a id="paperrust_remote_version_url"></a>`PAPERRUST_REMOTE_VERSION_URL` | Release-Adresse des Projekts | Wo die Prüfung „neue Version verfügbar“ nachsieht (nur wenn [`PAPERRUST_ENABLE_UPDATE_CHECK`](#paperrust_enable_update_check) an ist). |
| <a id="paperrust_django_url"></a>`PAPERRUST_DJANGO_URL` | – | **Entfernt.** Die Weiterleitung an Django gibt es nicht mehr; jede Route bedient PaperRust selbst. Ist die Variable gesetzt, meldet das Log beim Start einen Hinweis, sonst passiert nichts. |

### Aufgaben-Warteschlange und Worker

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="paperrust_queue"></a>`PAPERRUST_QUEUE` (auch `PAPERRUST_QUEUE_BACKEND`) | – | **Ohne Wirkung.** Es gibt nur noch die eingebaute Warteschlange in PostgreSQL. Ein alter Wert wie `celery` erzeugt einen Hinweis im Log, nie einen Abbruch. |
| <a id="paperrust_worker_name"></a>`PAPERRUST_WORKER_NAME` | `paperrust@<host>:<pid>` | Name des Workers in den Aufgaben-Einträgen. |
| <a id="paperrust_queue_names"></a>`PAPERRUST_QUEUE_NAMES` | `celery` | Welche Warteschlangen dieser Worker abarbeitet (`celery` ist der historische Name der Standard-Warteschlange). |
| <a id="paperrust_queue_poll_interval"></a>`PAPERRUST_QUEUE_POLL_INTERVAL` | `1` s | So oft schaut der Worker nach, falls keine Benachrichtigung (`NOTIFY`) von PostgreSQL kommt. |
| <a id="paperrust_queue_visibility_timeout"></a>`PAPERRUST_QUEUE_VISIBILITY_TIMEOUT` | `60` s | So lange ist eine laufende Aufgabe für einen Worker reserviert; ein Lebenszeichen verlängert die Frist. |
| <a id="paperrust_queue_max_deliveries"></a>`PAPERRUST_QUEUE_MAX_DELIVERIES` | `3` | Wie oft eine Aufgabe nach einem Absturz des Workers neu verteilt wird, bevor sie mit `WorkerLostError` scheitert. |
| <a id="paperrust_queue_result_expires"></a>`PAPERRUST_QUEUE_RESULT_EXPIRES` | `3600` s | So lange bleiben erledigte Einträge in der Warteschlange. |
| <a id="paperrust_queue_shutdown_grace"></a>`PAPERRUST_QUEUE_SHUTDOWN_GRACE` | `30` s | So viel Zeit bekommen laufende Aufgaben beim Beenden. |
| <a id="paperrust_index_writer"></a>`PAPERRUST_INDEX_WRITER` | `queue` | Wie der Suchindex nach Änderungen über die API nachgeführt wird: `queue` = als Hintergrundaufgabe in der Warteschlange (alter Name `celery` geht weiter); `rust` = sofort in der Anfrage; `off` = gar nicht. |
| <a id="paperrust_bulk_edit_async_min_docs"></a>`PAPERRUST_BULK_EDIT_ASYNC_MIN_DOCS` | `500` | Ab so vielen Dokumenten läuft eine Massenänderung im Hintergrund (`0` = immer sofort). |
| <a id="PAPERRUST_WORKFLOW_RUN_RETENTION_DAYS"></a><a id="paperrust_workflow_run_retention_days"></a>`PAPERRUST_WORKFLOW_RUN_RETENTION_DAYS` | `90` | So viele Tage bleibt der [Verlauf der Workflows](automatik.md#history) erhalten (`0` = unbegrenzt). Wirkt sofort. |
| <a id="PAPERRUST_BULK_UPDATE_COALESCE"></a><a id="paperrust_bulk_update_coalesce"></a>`PAPERRUST_BULK_UPDATE_COALESCE` | `on` | Wartende Nacharbeiten mehrerer Massenänderungen zu einer zusammenfassen (schneller, weniger Last). Nur zur Fehlersuche auf `off`. |

Redis braucht PaperRust nicht. Der Zwischenspeicher liegt im eigenen Prozess. Bedienen mehrere
Prozesse dieselbe Installation (getrennter `--worker-only`-Prozess, mehrere Instanzen hinter einem
Lastverteiler), müssen sie sich Anmelde-Drosselung, Sperre gegen wiederverwendete TOTP-Codes und
das Verwerfen veralteter Zwischenstände teilen: dafür [`PAPERRUST_CACHE=postgres`](#message-broker)
in jedem Prozess setzen. PaperRust warnt beim Start, wenn es so einen Aufbau erkennt.

### OCR, PDF und Klassifikator

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="paperrust_tesseract_path"></a>`PAPERRUST_TESSERACT_PATH` | `tesseract` im `PATH` | Tesseract-Programm für die Texterkennung. |
| <a id="paperrust_tesseract_lib"></a>`PAPERRUST_TESSERACT_LIB` | Standardsuche | Tesseract als Bibliothek (`libtesseract`): Datei oder Ordner; `off` nutzt nur das Programm. |
| <a id="paperrust_tessdata_dir"></a>`PAPERRUST_TESSDATA_DIR` | Tesseract-Standard | Ordner mit den Sprachdaten (`*.traineddata`). |
| <a id="paperrust_ocr_engine"></a>`PAPERRUST_OCR_ENGINE` | `tesseract` | Welcher Texterkenner arbeitet: `tesseract`; `windows` (die eingebaute Texterkennung von Windows; fehlt sie, warnt PaperRust und nimmt Tesseract); `auto` (Windows-Erkennung, wenn sie da ist und alle Sprachen aus `PAPERRUST_OCR_LANGUAGE` kann, sonst Tesseract); `macos` (vorgesehen, noch nicht umgesetzt → Tesseract). Archiv-PDF und Textebene entstehen in jedem Fall gleich. Siehe die Hinweise unter der Tabelle. |
| <a id="PAPERRUST_OCR_ENGINE_IDLE_SECS"></a><a id="paperrust_ocr_engine_idle_secs"></a>`PAPERRUST_OCR_ENGINE_IDLE_SECS` | `180` | Nach so vielen Sekunden ohne Texterkennung gibt PaperRust den Speicher der Tesseract-Sprachmodelle wieder frei (`0` = nie). Spart Arbeitsspeicher, wenn länger nichts eingelesen wird; das nächste Dokument braucht dann etwas länger. Wirkt sofort. |
| <a id="paperrust_pdfium_path"></a>`PAPERRUST_PDFIUM_PATH` | Standardsuche | PDFium-Bibliothek. Sie zeichnet PDF-Seiten für OCR, Vorschaubilder und Barcode-Erkennung. Ohne PDFium gibt es keine Vorschaubilder von PDFs und keine Barcodes. |
| <a id="paperrust_pdfa_font_dir"></a>`PAPERRUST_PDFA_FONT_DIR` | Standardsuche | Ordner mit den Liberation-Schriften. Fehlt im Original eine Standardschrift (Helvetica/Arial, Times, Courier), bettet PaperRust beim PDF/A-Archiv eine gleich breite Ersatzschrift ein. Ohne Ersatzschrift bleibt das Archiv ohne PDF/A-Kennzeichnung. Im Docker-Image schon vorhanden. |
| <a id="paperrust_classifier_model_file"></a>`PAPERRUST_CLASSIFIER_MODEL_FILE` | `DATA_DIR/paperrust_classifier.model` | Modelldatei der automatischen Zuordnung (eigenes Format; das Modell von paperless-ngx wird nicht benutzt). |
| <a id="paperrust_import_max_unzipped_bytes"></a>`PAPERRUST_IMPORT_MAX_UNZIPPED_BYTES` | 1 GiB + 100 × Archivgröße | Obergrenze, wenn `document_importer` ein ZIP entpackt (Schutz vor ZIP-Bomben). |
| <a id="paperrust_shrink_on_ingest_mb"></a>`PAPERRUST_SHRINK_ON_INGEST_MB` | `0` (aus) | Scans (PDFs mit kaum Text), die größer als so viele MB sind, beim Einlesen verkleinern; gespeichert wird die kleinere Datei. Siehe [PDF-Werkzeuge](pdf-tools.md#verkleinern-beim-einlesen). |
| <a id="paperrust_shrink_preset"></a>`PAPERRUST_SHRINK_PRESET` | `standard` | Stärke dafür: `lossless`, `standard` (150 dpi, JPEG 60) oder `strong` (96 dpi). |
| <a id="paperrust_pdftool_timeout"></a>`PAPERRUST_PDFTOOL_TIMEOUT` | `120` | Zeitgrenze in Sekunden je Lauf des PDF-Werkzeugs (Schwärzen, Verkleinern, Reparieren). |
| <a id="paperrust_pdftool_memory_mb"></a>`PAPERRUST_PDFTOOL_MEMORY_MB` | `1536` | Speichergrenze des PDF-Werkzeugs in MB (`0` = ohne Grenze). |
| <a id="paperrust_pdftool_bin"></a>`PAPERRUST_PDFTOOL_BIN` | neben `paperrust` | Pfad zu `paperrust-pdftool`. Fehlt das Programm, gibt es kein Schwärzen und Verkleinern; das Einlesen läuft ohne Reparatur weiter. |
| <a id="paperrust_office_to_pdf"></a>`PAPERRUST_OFFICE_TO_PDF` | `true` | Word-, OpenDocument- und RTF-Dateien beim Einlesen mit WordCraft (in `paperrust-pdftool`) in ein Archiv-PDF umwandeln; Vorschaubild dann von Seite 1. Scheitert die Umwandlung, wird ohne Archiv-PDF weiter eingelesen. Siehe [PDF-Werkzeuge](pdf-tools.md#office-beim-einlesen). |
| <a id="paperrust_office_to_pdf_timeout"></a>`PAPERRUST_OFFICE_TO_PDF_TIMEOUT` | `0` | Zeitgrenze in Sekunden je Office-Umwandlung (beim Einlesen und bei „In PDF umwandeln“); `0` = wie `PAPERRUST_PDFTOOL_TIMEOUT`. |

Zur Windows-Texterkennung: Sie arbeitet mit **einer** Sprache, nämlich der ersten aus
`PAPERRUST_OCR_LANGUAGE`, für die ein Windows-Sprachpaket installiert ist. Weitere Sprachen mit
derselben Schrift (z. B. `eng` neben `deu`) erkennt dieses Paket mit. Fehlt eine Sprache mit
anderer Schrift, meldet PaperRust einen Fehler samt Befehl zum Nachinstallieren. Gedrehte Seiten
erkennt weiterhin Tesseract, wenn es installiert ist.

### Oberfläche und Branding

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="paperrust_branding"></a>`PAPERRUST_BRANDING` | `on` | `off`: Die alte Angular-Oberfläche wird ohne PaperRust-Name, -Logo und -Farbe ausgeliefert, also genau wie bei paperless-ngx. |
| <a id="paperrust_accent"></a>`PAPERRUST_ACCENT` | `#B7410E` | Akzentfarbe (Hex). PaperRust dunkelt sie selbst ab, bis die Schrift auf Knöpfen gut lesbar ist (Kontrast 4,5:1). |
| <a id="paperrust_default_language"></a>`PAPERRUST_DEFAULT_LANGUAGE` | – | Sprache der Anmeldeseite und für Benutzer, die keine Sprache gewählt haben. |
| <a id="paperrust_ui_defaults"></a>`PAPERRUST_UI_DEFAULTS` | – | Vorgaben für die Oberflächen-Einstellungen neuer Benutzer. |
| <a id="paperrust_timeline_extract_dates"></a>`PAPERRUST_TIMELINE_EXTRACT_DATES` | `no` | Zeitstrahl: Fristen und Laufzeiten im Dokumenttext suchen. |
| <a id="paperrust_timeline_date_fields"></a>`PAPERRUST_TIMELINE_DATE_FIELDS` | – | Zeitstrahl: welche benutzerdefinierten Felder welche Art Datum enthalten (JSON). |

Name und Logo stellt man mit [`PAPERRUST_APP_TITLE`](#paperrust_app_title) und
[`PAPERRUST_APP_LOGO`](#paperrust_app_logo) ein. Mehr dazu: [docs/BRANDING.md](../BRANDING.md) und
[docs/OPERATIONS.md](../OPERATIONS.md).

### Anmeldung

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="paperrust_password_min_length"></a>`PAPERRUST_PASSWORD_MIN_LENGTH` | `8` | Mindestlänge neuer Passwörter. |
| <a id="paperrust_mfa_totp_issuer"></a>`PAPERRUST_MFA_TOTP_ISSUER` | `PaperRust` | Name, unter dem PaperRust in der Authenticator-App erscheint. |

### Integritätsprüfung und Abgleich

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="paperrust_integrity_incremental"></a>`PAPERRUST_INTEGRITY_INCREMENTAL` | `no` | `yes`: unveränderte Dateien bei der Integritätsprüfung nicht jedes Mal neu durchrechnen. |
| <a id="paperrust_integrity_full_hash_days"></a>`PAPERRUST_INTEGRITY_FULL_HASH_DAYS` | `28` | Nach so vielen Tagen wird eine Datei trotzdem wieder ganz geprüft (`0` = immer). |
| <a id="paperrust_integrity_threads"></a>`PAPERRUST_INTEGRITY_THREADS` | `0` (automatisch, höchstens 4) | Wie viele Dateien gleichzeitig gelesen werden. |
| <a id="paperrust_integrity_report_keep"></a>`PAPERRUST_INTEGRITY_REPORT_KEEP` | `10` | Wie viele Prüfberichte (JSON) aufbewahrt werden (`0` = keine). |
| <a id="paperrust_sync_sequencer"></a>`PAPERRUST_SYNC_SEQUENCER` | `on` | Abgleich für Desktop- und Mobil-Apps im Worker mitlaufen lassen. |
| <a id="paperrust_sync_tombstone_days"></a>`PAPERRUST_SYNC_TOMBSTONE_DAYS` | `180` | So lange merkt sich PaperRust gelöschte Einträge für den Abgleich. |

Weitere Schalter wie `PAPERRUST_SYNC_INTERVAL`, `PAPERRUST_SYNC_RETENTION_INTERVAL`,
`PAPERRUST_SYNC_BATCH`, `PAPERRUST_SEARCH_ENGINE`,
`PAPERRUST_TEXT_HIT_CACHE`, `PAPERRUST_TEXT_HIT_CACHE_WORKERS` und `PAPERRUST_BULK_UPDATE` sind
für die Fehlersuche gedacht. Im Normalbetrieb lässt man sie auf dem Standardwert.

Sicherung und Ransomware-Schutz (`PAPERRUST_BACKUP_*`, `PAPERRUST_GUARD_*`) beschreibt die Seite
[Schutz vor Ransomware](ransomware.md).

### KI-Erweiterungen

Was die Funktionen tun, steht unter [KI](ai.md). Alles ist zunächst aus.

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="paperrust_ai_mcp_enabled"></a>`PAPERRUST_AI_MCP_ENABLED` | `no` | Eingebauten MCP-Server unter `/mcp` einschalten. |
| <a id="paperrust_ai_mcp_allowed_origins"></a>`PAPERRUST_AI_MCP_ALLOWED_ORIGINS` | leer | Erlaubte `Origin`-Werte, durch Komma getrennt. Leer = nur Anfragen ohne `Origin`. |
| <a id="paperrust_ai_mcp_rate_limit"></a>`PAPERRUST_AI_MCP_RATE_LIMIT` | `120` | Anfragen je Minute und Token an `/mcp`. |
| <a id="paperrust_ai_mcp_max_creates_per_hour"></a>`PAPERRUST_AI_MCP_MAX_CREATES_PER_HOUR` | `30` | Neue Tags, Korrespondenten usw. je Token und Stunde über MCP. |
| <a id="paperrust_ai_mcp_tool_groups"></a>`PAPERRUST_AI_MCP_TOOL_GROUPS` | `read,write` | Welche MCP-Werkzeuggruppen freigeschaltet sind: `read`, `write`, `ingest`. |
| <a id="paperrust_ai_mcp_allow_url_fetch"></a>`PAPERRUST_AI_MCP_ALLOW_URL_FETCH` | `no` | `yes`: `add_document` darf Dateien von http(s)-Adressen holen. Nur über die Umgebung. |
| <a id="paperrust_ai_mcp_max_upload_bytes"></a>`PAPERRUST_AI_MCP_MAX_UPLOAD_BYTES` | `1572864` (1,5 MiB) | Größte Datei für `add_document`. |
| <a id="paperrust_ai_mcp_max_ingests_per_hour"></a>`PAPERRUST_AI_MCP_MAX_INGESTS_PER_HOUR` | `30` | `add_document` je Token und Stunde. |
| <a id="paperrust_ai_match_threshold"></a>`PAPERRUST_AI_MATCH_THRESHOLD` | `0.8` | Ab dieser Ähnlichkeit gilt ein vorhandenes Objekt als „dasselbe“, damit keine Doppelten entstehen. |
| <a id="paperrust_ai_auto_mode"></a>`PAPERRUST_AI_AUTO_MODE` | `off` | Automatik: `off`, `suggest` (nur Vorschläge zum Prüfen) oder `apply` (ab der Sicherheitsschwelle übernehmen). |
| <a id="paperrust_ai_auto_min_confidence"></a>`PAPERRUST_AI_AUTO_MIN_CONFIDENCE` | `0.9` | Mindest-Sicherheit für `apply`. |
| <a id="paperrust_ai_auto_max_new_objects"></a>`PAPERRUST_AI_AUTO_MAX_NEW_OBJECTS` | `3` | Höchstens so viele neue Objekte je Lauf. |
| <a id="paperrust_ai_auto_max_documents_per_run"></a>`PAPERRUST_AI_AUTO_MAX_DOCUMENTS_PER_RUN` | `50` | Dokumente je automatischem Lauf. |
| <a id="paperrust_ai_auto_trigger_tag"></a>`PAPERRUST_AI_AUTO_TRIGGER_TAG` | leer | Name eines Tags, das die Verarbeitung eines Dokuments auslöst. |
| <a id="paperrust_ai_trigger_tag_cron"></a>`PAPERRUST_AI_TRIGGER_TAG_CRON` | `*/5 * * * *` | Wann nach diesem Tag gesucht wird (`disable` = nie). |
| <a id="paperrust_ai_auto_create_missing"></a>`PAPERRUST_AI_AUTO_CREATE_MISSING` | `no` | Der Automatik erlauben, fehlende Objekte anzulegen. |
| <a id="paperrust_ai_run_max_documents_per_hour"></a>`PAPERRUST_AI_RUN_MAX_DOCUMENTS_PER_HOUR` | `100` | Dokumente je Benutzer und Stunde über `POST /api/x-ai/run/`. |
| <a id="PAPERRUST_AI_CUSTOM_FIELDS_ENABLED"></a><a id="paperrust_ai_custom_fields_enabled"></a>`PAPERRUST_AI_CUSTOM_FIELDS_ENABLED` | `no` | Benutzerfelder (Betrag, Rechnungsnummer …) von der KI ausfüllen lassen; welche Felder, wählt man unter *Einstellungen → KI*. |
| <a id="PAPERRUST_AI_VISION_ENABLED"></a><a id="paperrust_ai_vision_enabled"></a>`PAPERRUST_AI_VISION_ENABLED` | `no` | KI-Texterkennung mit einem Bildmodell verfügbar machen. |
| <a id="PAPERRUST_AI_VISION_MODEL"></a><a id="paperrust_ai_vision_model"></a>`PAPERRUST_AI_VISION_MODEL` | leer (= Textmodell) | Name des Bildmodells, z. B. `qwen2.5vl` oder `gpt-4o-mini`. |
| <a id="PAPERRUST_AI_VISION_WHEN"></a><a id="paperrust_ai_vision_when"></a>`PAPERRUST_AI_VISION_WHEN` | `difficult` | Wann: `off`, `difficult` (nur wenn die normale Erkennung wenig Text liefert) oder `always`. |
| <a id="PAPERRUST_AI_VISION_MAX_PAGES"></a><a id="paperrust_ai_vision_max_pages"></a>`PAPERRUST_AI_VISION_MAX_PAGES` | `10` | Höchstens so viele Seiten je Dokument. |
| <a id="PAPERRUST_AI_VISION_MAX_PAGES_PER_DAY"></a><a id="paperrust_ai_vision_max_pages_per_day"></a>`PAPERRUST_AI_VISION_MAX_PAGES_PER_DAY` | `500` | Höchstens so viele Seiten je Tag (`0` = unbegrenzt) – Schutz vor Kosten bei Cloud-Anbietern. |
| <a id="paperrust_ai_api_base"></a>`PAPERRUST_AI_API_BASE` | `http://127.0.0.1:<PAPERRUST_PORT oder 8000>` | Adresse, die die MCP-Werkzeuge aufrufen. Sie bekommt die API-Tokens der Benutzer zu sehen – darum nur auf PaperRust selbst zeigen lassen. Läuft PaperRust nicht auf Port 8000, hier die eigene Adresse eintragen (z. B. `http://127.0.0.1:8080`). |

### Scanner-Empfang (SMTP)

Ein kleiner Mail-Empfang nur für Scanner und Drucker („Scan an E-Mail“), siehe
[Scanner verbinden](scanner.md). Alle Werte wirken nach einem Neustart. Ist der Empfang aus, lauscht
nichts.

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="paperrust_smtp_intake"></a>`PAPERRUST_SMTP_INTAKE` | `off` | `on` schaltet den Empfang ein. |
| <a id="paperrust_smtp_intake_listen"></a>`PAPERRUST_SMTP_INTAKE_LISTEN` | `0.0.0.0:2525` | Adresse `IP:PORT`. In Docker zusätzlich den Port freigeben (`docker/docker-compose.smtp-intake.yml`). |
| <a id="paperrust_smtp_intake_max_mb"></a>`PAPERRUST_SMTP_INTAKE_MAX_MB` | `50` | Größte Nachricht in MB (1–1024). |
| <a id="paperrust_smtp_intake_allowed_ips"></a>`PAPERRUST_SMTP_INTAKE_ALLOWED_IPS` | leer | Erlaubte Absender als IP oder Netz (`192.168.1.0/24`), getrennt durch Komma oder Leerzeichen. Leer = alle. |
| <a id="paperrust_smtp_intake_rate_per_minute"></a>`PAPERRUST_SMTP_INTAKE_RATE_PER_MINUTE` | `30` | Nachrichten je Minute und Gerät. |
| <a id="paperrust_smtp_intake_tls_cert"></a>`PAPERRUST_SMTP_INTAKE_TLS_CERT` / `_TLS_KEY` | leer | Zertifikat und Schlüssel (PEM) für STARTTLS. Nur über die Umgebung. Ohne sie geht das Token unverschlüsselt übers Netz – dann nur im eigenen Heimnetz verwenden. |
| <a id="paperrust_smtp_intake_require_tls"></a>`PAPERRUST_SMTP_INTAKE_REQUIRE_TLS` | `off` | Anmeldung erst nach STARTTLS zulassen. |
| <a id="paperrust_smtp_intake_implicit_tls"></a>`PAPERRUST_SMTP_INTAKE_IMPLICIT_TLS` | `off` | Verschlüsselung ab dem ersten Byte (SMTPS, meist Port 465) statt STARTTLS. |

### Desktop-App

Diese Variablen setzt meist die Desktop-App selbst: `PAPERRUST_DESKTOP_HOME`,
`PAPERRUST_DESKTOP_LOG`, `PAPERRUST_DESKTOP_SERVER_LOG`, `PAPERRUST_DESKTOP_LOG_MAX_MB`,
`PAPERRUST_DESKTOP_PG_BIN`, `PAPERRUST_DESKTOP_SERVER_BIN`, `PAPERRUST_DESKTOP_MIGRATE_DB_BIN`,
`PAPERRUST_DESKTOP_STATIC`, `PAPERRUST_DESKTOP_ROUTES`, `PAPERRUST_DESKTOP_MODE`,
`PAPERRUST_DESKTOP_SERVER_URL`, `PAPERRUST_DESKTOP_ALLOW_HTTP`,
`PAPERRUST_DESKTOP_ADMIN_{USER,EMAIL,PASSWORD}`, `PAPERRUST_DESKTOP_IMPORT{,_DB,_MEDIA}`,
`PAPERRUST_DESKTOP_UPDATES`, `PAPERRUST_DESKTOP_UPDATE_URL`, `PAPERRUST_DESKTOP_UPDATE_CHANNEL`,
`PAPERRUST_DESKTOP_NO_JOB`. Beschreibung: [docs/DESKTOP.md](../DESKTOP.md).

<a id="paperrust-differences"></a>

## Was PaperRust anders macht

Für Umsteiger das Wichtigste in Kürze. Die Einzelheiten stehen bei den Einstellungen in der
Referenz.

- **Datenbank:** nur PostgreSQL ([`PAPERRUST_DBENGINE`](#paperrust_dbengine)). Eine Installation
  mit SQLite oder MariaDB zieht man vorher um, siehe [Umstieg von paperless-ngx](migration.md).
- **Kein Redis, kein Celery:** Die Aufgaben-Warteschlange liegt in PostgreSQL, der Zwischenspeicher
  im Prozess oder (bei mehreren Prozessen) ebenfalls in PostgreSQL.
- **Kein Tika, kein Gotenberg:** Text aus Office-Dateien und Mails liest PaperRust selbst. Die
  PDF-Fassung (Archiv-PDF) erzeugt das mitgelieferte Hilfsprogramm `paperrust-pdftool`
  ([`PAPERRUST_OFFICE_TO_PDF`](pdf-tools.md#office-beim-einlesen)); fehlt es, bleibt das Original
  durchsuchbar und das Vorschaubild wird aus dem Text gezeichnet.
- **OCR:** Tesseract und PDFium statt OCRmyPDF, Ghostscript und unpaper. Säubern
  (`PAPERRUST_OCR_CLEAN`) und Geraderichten (`PAPERRUST_OCR_DESKEW`) erledigt PaperRust selbst,
  ohne Zusatzprogramme. PDF/A-Archive werden bereinigt, Farben umgewandelt und selbst geprüft;
  besteht ein PDF die Prüfung nicht (z. B. nicht eingebettete Schriften im Original), entfällt die
  PDF/A-Kennzeichnung.
- **Ein Programm:** PaperRust lauscht auf [`PAPERRUST_BIND`](#paperrust_bind). Die Optionen des
  Webservers von paperless-ngx (`PAPERLESS_WEBSERVER_WORKERS` usw.) entfallen.

Vollständige Listen der Unterschiede je Baustein: `docs/contract/*.md`, Abschnitt „Known differences“.

---

<a id="reference"></a>

# Referenz: gemeinsame Einstellungen mit paperless-ngx

Alle Einstellungen in diesem Teil gibt es auch in paperless-ngx, mit gleicher
Bedeutung; dort heißen sie `PAPERLESS_<NAME>`, und dieser Name gilt auch in PaperRust. Die meisten kann man
in der Oberfläche setzen (*Einstellungen → System*). Wo PaperRust sich anders verhält, steht es in
der Beschreibung.

Schreibweise der Werte: Ja/Nein-Werte verstehen `true`/`false`, `yes`/`no` und `1`/`0`. Zeitpläne
sind Cron-Ausdrücke mit fünf Feldern (Minute, Stunde, Tag, Monat, Wochentag), z. B. `0 3 * * *` für
„täglich um 3 Uhr“; `disable` schaltet die Aufgabe ab. Listen trennt man mit Komma.

<a id="required-services"></a><a id="database"></a>

## Datenbank

PaperRust braucht **PostgreSQL ab Version 14** (getestet mit 18). Andere Datenbanken gehen nicht.

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_DBENGINE"></a><a id="paperrust_dbengine"></a><a id="PAPERLESS_DBENGINE"></a><a id="paperless_dbengine"></a>`PAPERRUST_DBENGINE` | `postgresql` | Art der Datenbank. In PaperRust ist nur `postgresql` möglich. Bei paperless-ngx war ohne Angabe SQLite der Standard – wer von dort kommt, zieht vorher um ([Umstieg](migration.md)). |
| <a id="PAPERRUST_DBHOST"></a><a id="paperrust_dbhost"></a><a id="PAPERLESS_DBHOST"></a><a id="paperless_dbhost"></a>`PAPERRUST_DBHOST` | – | Rechnername oder Adresse des PostgreSQL-Servers. |
| <a id="PAPERRUST_DBPORT"></a><a id="paperrust_dbport"></a><a id="PAPERLESS_DBPORT"></a><a id="paperless_dbport"></a>`PAPERRUST_DBPORT` | `5432` | Port des Datenbankservers. |
| <a id="PAPERRUST_DBNAME"></a><a id="paperrust_dbname"></a><a id="PAPERLESS_DBNAME"></a><a id="paperless_dbname"></a>`PAPERRUST_DBNAME` | `paperless` | Name der Datenbank. |
| <a id="PAPERRUST_DBUSER"></a><a id="paperrust_dbuser"></a><a id="PAPERLESS_DBUSER"></a><a id="paperless_dbuser"></a>`PAPERRUST_DBUSER` | `paperless` | Benutzer für die Datenbank. |
| <a id="PAPERRUST_DBPASS"></a><a id="paperrust_dbpass"></a><a id="PAPERLESS_DBPASS"></a><a id="paperless_dbpass"></a>`PAPERRUST_DBPASS` | `paperless` | Passwort dieses Benutzers. |
| <a id="PAPERRUST_DB_OPTIONS"></a><a id="paperrust_db_options"></a><a id="PAPERLESS_DB_OPTIONS"></a><a id="paperless_db_options"></a>`PAPERRUST_DB_OPTIONS` | – | Weitere Verbindungsoptionen als `schlüssel=wert`, durch Komma getrennt. Wichtige Schlüssel: `sslmode`, `sslrootcert`, `sslcert`, `sslkey`, `connect_timeout`, `pool.max_size`, `application_name`. Diese Angaben gehen den einzelnen Variablen unten vor. |
| <a id="PAPERRUST_DBSSLMODE"></a><a id="paperrust_dbsslmode"></a><a id="PAPERLESS_DBSSLMODE"></a><a id="paperless_dbsslmode"></a>`PAPERRUST_DBSSLMODE` | `prefer` | Verschlüsselung der Verbindung (`disable`, `prefer`, `require`, `verify-ca`, `verify-full`). Älterer Weg; besser `sslmode=…` in `PAPERRUST_DB_OPTIONS`. |
| <a id="PAPERRUST_DBSSLROOTCERT"></a><a id="paperrust_dbsslrootcert"></a><a id="PAPERLESS_DBSSLROOTCERT"></a><a id="paperless_dbsslrootcert"></a>`PAPERRUST_DBSSLROOTCERT` | – | Datei mit dem Stammzertifikat des Servers. Älterer Weg; besser `sslrootcert=…`. |
| <a id="PAPERRUST_DBSSLCERT"></a><a id="paperrust_dbsslcert"></a><a id="PAPERLESS_DBSSLCERT"></a><a id="paperless_dbsslcert"></a>`PAPERRUST_DBSSLCERT` | – | Client-Zertifikat. Älterer Weg; besser `sslcert=…`. |
| <a id="PAPERRUST_DBSSLKEY"></a><a id="paperrust_dbsslkey"></a><a id="PAPERLESS_DBSSLKEY"></a><a id="paperless_dbsslkey"></a>`PAPERRUST_DBSSLKEY` | – | Schlüssel zum Client-Zertifikat. Älterer Weg; besser `sslkey=…`. |
| <a id="PAPERRUST_DB_TIMEOUT"></a><a id="paperrust_db_timeout"></a><a id="PAPERLESS_DB_TIMEOUT"></a><a id="paperless_db_timeout"></a>`PAPERRUST_DB_TIMEOUT` | – | Zeitgrenze in Sekunden für den Verbindungsaufbau. Älterer Weg; besser `connect_timeout=…`. |
| <a id="PAPERRUST_DB_POOLSIZE"></a><a id="paperrust_db_poolsize"></a><a id="PAPERLESS_DB_POOLSIZE"></a><a id="paperless_db_poolsize"></a>`PAPERRUST_DB_POOLSIZE` | – (dann [`PAPERRUST_DB_MAX_CONNECTIONS`](#paperrust_db_max_connections)) | Höchstzahl gleichzeitiger Verbindungen. Älterer Weg; besser `pool.max_size=…`. |

paperless-ngx hat die älteren Variablen (`DBSSL*`, `DB_TIMEOUT`, `DB_POOLSIZE`) in Version 3
gestrichen. PaperRust liest sie weiter, damit alte Konfigurationen nicht brechen.

Beispiel für eine verschlüsselte Verbindung mit eigener Zertifizierungsstelle:

```sh
PAPERRUST_DB_OPTIONS="sslmode=verify-full,sslrootcert=/certs/ca.pem,connect_timeout=10"
```

<a id="message-broker"></a>

## Zwischenspeicher (Cache) und Redis

PaperRust braucht kein Redis. Wer von paperless-ngx umsteigt, kann den Redis-/Valkey-Dienst danach
entfernen. Bedienen mehrere PaperRust-Prozesse dieselbe Installation (Vorlage
`docker/docker-compose.multiprocess.yml`), teilen sie sich den Zwischenspeicher über eine Tabelle in
derselben PostgreSQL-Datenbank.

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="paperrust_cache"></a>`PAPERRUST_CACHE` | `local` | `local` = Zwischenspeicher im eigenen Prozess. `postgres` = gemeinsam für alle Prozesse in der Tabelle `paperrust_cache` (nötig bei getrennten Web- und Worker-Prozessen oder mehreren Instanzen). Gilt nach Neustart. |
| <a id="PAPERRUST_REDIS"></a><a id="paperrust_redis"></a><a id="PAPERLESS_REDIS"></a><a id="paperless_redis"></a>`PAPERRUST_REDIS` | – | **Ohne Wirkung.** Aus paperless-ngx übernommen: Redis wird nicht mehr verbunden, das Log nennt nur einen Hinweis. Kann entfernt werden. |
| <a id="PAPERRUST_REDIS_PREFIX"></a><a id="paperrust_redis_prefix"></a><a id="PAPERLESS_REDIS_PREFIX"></a><a id="paperless_redis_prefix"></a>`PAPERRUST_REDIS_PREFIX` | – | **Ohne Wirkung.** |
| <a id="PAPERRUST_CACHE_BACKEND"></a><a id="paperrust_cache_backend"></a><a id="PAPERLESS_CACHE_BACKEND"></a><a id="paperless_cache_backend"></a>`PAPERRUST_CACHE_BACKEND` | – | Wird nur gelesen, wenn `PAPERRUST_CACHE` fehlt: `…RedisCache` oder `…DatabaseCache` wirken wie `PAPERRUST_CACHE=postgres`, alles andere wie `local`. |

<a id="paths-and-folders"></a>

## Pfade und Ordner

Relative Standardpfade beziehen sich auf [`PAPERRUST_BASE_DIR`](#paperrust_base_dir).

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_CONSUMPTION_DIR"></a><a id="paperrust_consumption_dir"></a><a id="PAPERLESS_CONSUMPTION_DIR"></a><a id="paperless_consumption_dir"></a>`PAPERRUST_CONSUMPTION_DIR` | `consume/` | Eingangsordner. Was hier landet, liest PaperRust ein. Der Ordner muss existieren, PaperRust braucht Lese- und Schreibrechte. |
| <a id="PAPERRUST_DATA_DIR"></a><a id="paperrust_data_dir"></a><a id="PAPERLESS_DATA_DIR"></a><a id="paperless_data_dir"></a>`PAPERRUST_DATA_DIR` | `data/` | Suchindex, Modell der automatischen Zuordnung, Protokolle und weitere Arbeitsdaten. |
| <a id="PAPERRUST_MEDIA_ROOT"></a><a id="paperrust_media_root"></a><a id="PAPERLESS_MEDIA_ROOT"></a><a id="paperless_media_root"></a>`PAPERRUST_MEDIA_ROOT` | `media/` | Die Dokumente selbst: Originale, Archivfassungen, Vorschaubilder. In die Sicherung! |
| <a id="PAPERRUST_SCRATCH_DIR"></a><a id="paperrust_scratch_dir"></a><a id="PAPERLESS_SCRATCH_DIR"></a><a id="paperless_scratch_dir"></a>`PAPERRUST_SCRATCH_DIR` | `/tmp/paperrust` | Ordner für Zwischendateien beim Einlesen, Bearbeiten und Exportieren. |
| <a id="PAPERRUST_STATICDIR"></a><a id="paperrust_staticdir"></a><a id="PAPERLESS_STATICDIR"></a><a id="paperless_staticdir"></a>`PAPERRUST_STATICDIR` | `static/` | Statische Dateien der alten Angular-Oberfläche. Die neue Oberfläche liegt daneben (siehe [`PAPERRUST_LIGHT_UI_DIR`](#paperrust_light_ui_dir)). |
| <a id="PAPERRUST_EMPTY_TRASH_DIR"></a><a id="paperrust_empty_trash_dir"></a><a id="PAPERLESS_EMPTY_TRASH_DIR"></a><a id="paperless_empty_trash_dir"></a>`PAPERRUST_EMPTY_TRASH_DIR` | leer | Ist ein Ordner gesetzt, verschiebt PaperRust Dateien beim endgültigen Löschen dorthin, statt sie zu löschen. Der Ordner muss schon existieren. Älterer Name: `PAPERRUST_TRASH_DIR`. |
| <a id="PAPERRUST_FILENAME_FORMAT"></a><a id="paperrust_filename_format"></a><a id="PAPERLESS_FILENAME_FORMAT"></a><a id="paperless_filename_format"></a>`PAPERRUST_FILENAME_FORMAT` | – (aus) | Vorlage für Ordner und Dateinamen im Medienordner, z. B. `{{ created_year }}/{{ correspondent }}/{{ title }}`. Ohne Vorlage heißen die Dateien nur nach ihrer Nummer. Siehe [Dateinamen und Speicherpfade](storage-paths.md#file-name-handling). |
| <a id="PAPERRUST_FILENAME_FORMAT_REMOVE_NONE"></a><a id="paperrust_filename_format_remove_none"></a><a id="PAPERLESS_FILENAME_FORMAT_REMOVE_NONE"></a><a id="paperless_filename_format_remove_none"></a>`PAPERRUST_FILENAME_FORMAT_REMOVE_NONE` | `false` | Leere Platzhalter ganz weglassen, statt `none` in den Pfad zu schreiben. |
| <a id="PAPERRUST_LOGGING_DIR"></a><a id="paperrust_logging_dir"></a><a id="PAPERLESS_LOGGING_DIR"></a><a id="paperless_logging_dir"></a>`PAPERRUST_LOGGING_DIR` | `data/log/` | Ordner für die Protokolldateien. |
| <a id="PAPERRUST_MODEL_FILE"></a><a id="paperrust_model_file"></a><a id="PAPERLESS_MODEL_FILE"></a><a id="paperless_model_file"></a>`PAPERRUST_MODEL_FILE` | `data/classification_model.pickle` | Modell von paperless-ngx. PaperRust nutzt es nicht, sondern ein eigenes, siehe [`PAPERRUST_CLASSIFIER_MODEL_FILE`](#paperrust_classifier_model_file). |

Wer die Dateinamen-Vorlage ändert, ruft danach `document_renamer` auf; bestehende Dateien werden
sonst erst bei der nächsten Änderung des Dokuments umbenannt.

<a id="logging"></a>

## Protokollierung

PaperRust schreibt `paperrust.log` (alles; eine ältere `paperless.log` bleibt in der Ansicht sichtbar) und `mail.log` (Mail-Abruf) in
[`PAPERRUST_LOGGING_DIR`](#paperrust_logging_dir). Wie ausführlich, steuert
[`PAPERRUST_LOG`](#paperrust_log).

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_LOGROTATE_MAX_SIZE"></a><a id="paperrust_logrotate_max_size"></a><a id="PAPERLESS_LOGROTATE_MAX_SIZE"></a><a id="paperless_logrotate_max_size"></a>`PAPERRUST_LOGROTATE_MAX_SIZE` | `1048576` (1 MiB) | Ab dieser Größe in Byte beginnt eine neue Protokolldatei. |
| <a id="PAPERRUST_LOGROTATE_MAX_BACKUPS"></a><a id="paperrust_logrotate_max_backups"></a><a id="PAPERLESS_LOGROTATE_MAX_BACKUPS"></a><a id="paperless_logrotate_max_backups"></a>`PAPERRUST_LOGROTATE_MAX_BACKUPS` | `20` | So viele alte Protokolldateien bleiben liegen. |

<a id="hosting-and-security"></a><a id="hosting--security"></a>

## Betrieb und Sicherheit

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_SECRET_KEY"></a><a id="paperrust_secret_key"></a><a id="PAPERLESS_SECRET_KEY"></a><a id="paperless_secret_key"></a>`PAPERRUST_SECRET_KEY` | – (Pflicht) | Geheimer Schlüssel für Sitzungen, Signaturen und die verschlüsselten Einstellungen. Ohne ihn – oder mit dem Platzhalter `change-me` – startet PaperRust nicht. Beim Umstieg den bisherigen Wert behalten, sonst sind alle angemeldet gewesenen Benutzer abgemeldet. |
| <a id="PAPERRUST_URL"></a><a id="paperrust_url"></a><a id="PAPERLESS_URL"></a><a id="paperless_url"></a>`PAPERRUST_URL` | – | Öffentliche Adresse ohne `/` am Ende und ohne Pfad, z. B. `https://dokumente.example.org`. Daraus ergeben sich auch erlaubte Hosts, CORS und CSRF (die drei Variablen darunter). Wird außerdem für Links in Mails und für OAuth gebraucht. |
| <a id="PAPERRUST_CSRF_TRUSTED_ORIGINS"></a><a id="paperrust_csrf_trusted_origins"></a><a id="PAPERLESS_CSRF_TRUSTED_ORIGINS"></a><a id="paperless_csrf_trusted_origins"></a>`PAPERRUST_CSRF_TRUSTED_ORIGINS` | – (aus `PAPERRUST_URL`) | Adressen, von denen schreibende Anfragen (Formulare, POST) angenommen werden. Nur nötig, wenn PaperRust unter mehreren Adressen erreichbar ist. |
| <a id="PAPERRUST_ALLOWED_HOSTS"></a><a id="paperrust_allowed_hosts"></a><a id="PAPERLESS_ALLOWED_HOSTS"></a><a id="paperless_allowed_hosts"></a>`PAPERRUST_ALLOWED_HOSTS` | `*`, solange auch `PAPERRUST_URL` fehlt | Hostnamen, unter denen PaperRust antwortet. Ist nur `PAPERRUST_URL` gesetzt, gilt deren Hostname. Außer bei `*` sind `localhost`, `127.0.0.1` und `[::1]` immer erlaubt (Zugriff vom eigenen Rechner, Healthcheck). Eine leere Liste fügt sonst nichts hinzu; `*` erlaubt jeden Host. |
| <a id="PAPERRUST_CORS_ALLOWED_HOSTS"></a><a id="paperrust_cors_allowed_hosts"></a><a id="PAPERLESS_CORS_ALLOWED_HOSTS"></a><a id="paperless_cors_allowed_hosts"></a>`PAPERRUST_CORS_ALLOWED_HOSTS` | `http://localhost:8000` | Fremde Webseiten, die per Browser auf die API zugreifen dürfen. |
| <a id="PAPERRUST_TRUSTED_PROXIES"></a><a id="paperrust_trusted_proxies"></a><a id="PAPERLESS_TRUSTED_PROXIES"></a><a id="paperless_trusted_proxies"></a>`PAPERRUST_TRUSTED_PROXIES` | – | Adressen der Reverse Proxys, denen PaperRust die weitergereichte Client-Adresse glaubt. Wichtig für richtige Adressen im Log (z. B. für fail2ban). Nur über die Umgebung. |
| <a id="PAPERRUST_ALLAUTH_TRUSTED_PROXY_COUNT"></a><a id="paperrust_allauth_trusted_proxy_count"></a><a id="PAPERLESS_ALLAUTH_TRUSTED_PROXY_COUNT"></a><a id="paperless_allauth_trusted_proxy_count"></a>`PAPERRUST_ALLAUTH_TRUSTED_PROXY_COUNT` | Anzahl Einträge in `PAPERRUST_TRUSTED_PROXIES` | Wie viele Proxy-Stufen in `X-Forwarded-For` stehen (Stufen, nicht Adressen). Bestimmt, welche Adresse als Client gilt, z. B. für die Anmelde-Drosselung. Nur über die Umgebung. |
| <a id="PAPERRUST_ALLAUTH_TRUSTED_CLIENT_IP_HEADER"></a><a id="paperrust_allauth_trusted_client_ip_header"></a><a id="PAPERLESS_ALLAUTH_TRUSTED_CLIENT_IP_HEADER"></a><a id="paperless_allauth_trusted_client_ip_header"></a>`PAPERRUST_ALLAUTH_TRUSTED_CLIENT_IP_HEADER` | – | Kopfzeile, in der der Proxy die echte Client-Adresse mitschickt, z. B. `X-Real-IP` oder `CF-Connecting-IP`. Geht der Proxy-Anzahl vor. Nur über die Umgebung. |
| <a id="PAPERRUST_FORCE_SCRIPT_NAME"></a><a id="paperrust_force_script_name"></a><a id="PAPERLESS_FORCE_SCRIPT_NAME"></a><a id="paperless_force_script_name"></a>`PAPERRUST_FORCE_SCRIPT_NAME` | – (unter `/`) | Unterpfad, wenn PaperRust nicht an der Wurzel läuft, z. B. `/dokumente` (ohne `/` am Ende). |
| <a id="PAPERRUST_STATIC_URL"></a><a id="paperrust_static_url"></a><a id="PAPERLESS_STATIC_URL"></a><a id="paperless_static_url"></a>`PAPERRUST_STATIC_URL` | `/static/` | Adresse der statischen Dateien der alten Oberfläche (mit `/` am Ende). Bei einem Unterpfad passend setzen, z. B. `/dokumente/static/`. |
| <a id="PAPERRUST_AUTO_LOGIN_USERNAME"></a><a id="paperrust_auto_login_username"></a><a id="PAPERLESS_AUTO_LOGIN_USERNAME"></a><a id="paperless_auto_login_username"></a>`PAPERRUST_AUTO_LOGIN_USERNAME` | – (aus) | Jeder Besucher ist automatisch als dieser Benutzer angemeldet. Nur für einen Rechner, den sonst niemand erreicht – nie aus dem Internet erreichbar machen. Nur über die Umgebung. |
| <a id="PAPERRUST_ADMIN_USER"></a><a id="paperrust_admin_user"></a><a id="PAPERLESS_ADMIN_USER"></a><a id="paperless_admin_user"></a>`PAPERRUST_ADMIN_USER` | – | Legt beim Start einen Superuser mit diesem Namen an, falls es ihn noch nicht gibt. Braucht `PAPERRUST_ADMIN_PASSWORD`. Bestehende Benutzer bleiben unverändert. |
| <a id="PAPERRUST_ADMIN_MAIL"></a><a id="paperrust_admin_mail"></a><a id="PAPERLESS_ADMIN_MAIL"></a><a id="paperless_admin_mail"></a>`PAPERRUST_ADMIN_MAIL` | `root@localhost` | Mail-Adresse dieses Superusers. |
| <a id="PAPERRUST_ADMIN_PASSWORD"></a><a id="paperrust_admin_password"></a><a id="PAPERLESS_ADMIN_PASSWORD"></a><a id="paperless_admin_password"></a>`PAPERRUST_ADMIN_PASSWORD` | – | Passwort dieses Superusers. Nach dem ersten Start aus der Konfiguration entfernen. |
| <a id="PAPERRUST_COOKIE_PREFIX"></a><a id="paperrust_cookie_prefix"></a><a id="PAPERLESS_COOKIE_PREFIX"></a><a id="paperless_cookie_prefix"></a>`PAPERRUST_COOKIE_PREFIX` | leer | Vorsilbe für die Namen der Cookies. Nötig, wenn zwei Installationen unter demselben Hostnamen laufen. Nach einer Änderung muss man sich neu anmelden. |
| <a id="PAPERRUST_ENABLE_HTTP_REMOTE_USER"></a><a id="paperrust_enable_http_remote_user"></a><a id="PAPERLESS_ENABLE_HTTP_REMOTE_USER"></a><a id="paperless_enable_http_remote_user"></a>`PAPERRUST_ENABLE_HTTP_REMOTE_USER` | `false` | Anmeldung über einen vorgeschalteten SSO-Proxy, der den Benutzernamen in einer Kopfzeile mitschickt. Siehe Warnung unten. Nur über die Umgebung. |
| <a id="PAPERRUST_ENABLE_HTTP_REMOTE_USER_API"></a><a id="paperrust_enable_http_remote_user_api"></a><a id="PAPERLESS_ENABLE_HTTP_REMOTE_USER_API"></a><a id="paperless_enable_http_remote_user_api"></a>`PAPERRUST_ENABLE_HTTP_REMOTE_USER_API` | `false` | Dasselbe auch für Aufrufe der API. Nur über die Umgebung. |
| <a id="PAPERRUST_HTTP_REMOTE_USER_HEADER_NAME"></a><a id="paperrust_http_remote_user_header_name"></a><a id="PAPERLESS_HTTP_REMOTE_USER_HEADER_NAME"></a><a id="paperless_http_remote_user_header_name"></a>`PAPERRUST_HTTP_REMOTE_USER_HEADER_NAME` | `HTTP_REMOTE_USER` | Name dieser Kopfzeile in der Schreibweise mit `HTTP_` davor und Unterstrichen (aus `X-Auth-User` wird `HTTP_X_AUTH_USER`). Nur über die Umgebung. |
| <a id="PAPERRUST_LOGOUT_REDIRECT_URL"></a><a id="paperrust_logout_redirect_url"></a><a id="PAPERLESS_LOGOUT_REDIRECT_URL"></a><a id="paperless_logout_redirect_url"></a>`PAPERRUST_LOGOUT_REDIRECT_URL` | – | Seite, auf die man nach dem Abmelden kommt, z. B. die Abmeldeseite des SSO-Dienstes. |
| <a id="PAPERRUST_USE_X_FORWARD_HOST"></a><a id="paperrust_use_x_forward_host"></a><a id="PAPERLESS_USE_X_FORWARD_HOST"></a><a id="paperless_use_x_forward_host"></a>`PAPERRUST_USE_X_FORWARD_HOST` | `false` | Den Hostnamen aus der Kopfzeile `X-Forwarded-Host` des Proxys übernehmen. Nur über die Umgebung. |
| <a id="PAPERRUST_USE_X_FORWARD_PORT"></a><a id="paperrust_use_x_forward_port"></a><a id="PAPERLESS_USE_X_FORWARD_PORT"></a><a id="paperless_use_x_forward_port"></a>`PAPERRUST_USE_X_FORWARD_PORT` | `false` | Den Port aus `X-Forwarded-Port` übernehmen. Nur über die Umgebung. |
| <a id="PAPERRUST_PROXY_SSL_HEADER"></a><a id="paperrust_proxy_ssl_header"></a><a id="PAPERLESS_PROXY_SSL_HEADER"></a><a id="paperless_proxy_ssl_header"></a>`PAPERRUST_PROXY_SSL_HEADER` | – | Woran PaperRust erkennt, dass die Verbindung zum Proxy verschlüsselt war, als JSON-Liste, z. B. `["HTTP_X_FORWARDED_PROTO", "https"]`. Nur setzen, wenn der Proxy diese Kopfzeile von außen immer überschreibt. Nur über die Umgebung. |
| <a id="PAPERRUST_EMAIL_CERTIFICATE_LOCATION"></a><a id="paperrust_email_certificate_location"></a><a id="PAPERLESS_EMAIL_CERTIFICATE_LOCATION"></a><a id="paperless_email_certificate_location"></a>`PAPERRUST_EMAIL_CERTIFICATE_LOCATION` | – | Zusätzliche Zertifikatsdatei (z. B. selbst ausgestellt), der PaperRust beim Abruf von Mailservern vertraut. |
| <a id="PAPERRUST_DEBUG"></a><a id="paperrust_debug"></a><a id="PAPERLESS_DEBUG"></a><a id="paperless_debug"></a>`PAPERRUST_DEBUG` | `false` | Fehlersuche-Modus. Im Betrieb immer aus lassen. Nur über die Umgebung. |

> [!WARNING]
> Mit `PAPERRUST_ENABLE_HTTP_REMOTE_USER` glaubt PaperRust jedem, der die Kopfzeile schickt. Das
> ist nur sicher, wenn PaperRust ausschließlich über den Proxy erreichbar ist und der Proxy eine
> von außen mitgeschickte Kopfzeile immer entfernt. Sonst kann sich jeder als beliebiger Benutzer
> ausgeben.

<a id="authentication"></a><a id="authentication--sso"></a>

### Anmeldung und SSO

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_ACCOUNT_ALLOW_SIGNUPS"></a><a id="paperrust_account_allow_signups"></a><a id="PAPERLESS_ACCOUNT_ALLOW_SIGNUPS"></a><a id="paperless_account_allow_signups"></a>`PAPERRUST_ACCOUNT_ALLOW_SIGNUPS` | `false` | Besucher dürfen sich selbst ein Konto anlegen. |
| <a id="PAPERRUST_ACCOUNT_DEFAULT_GROUPS"></a><a id="paperrust_account_default_groups"></a><a id="PAPERLESS_ACCOUNT_DEFAULT_GROUPS"></a><a id="paperless_account_default_groups"></a>`PAPERRUST_ACCOUNT_DEFAULT_GROUPS` | – | Gruppen, in die selbst angelegte Konten kommen. Die Gruppen müssen schon existieren. |
| <a id="PAPERRUST_SOCIALACCOUNT_PROVIDERS"></a><a id="paperrust_socialaccount_providers"></a><a id="PAPERLESS_SOCIALACCOUNT_PROVIDERS"></a><a id="paperless_socialaccount_providers"></a>`PAPERRUST_SOCIALACCOUNT_PROVIDERS` | – (kein Anbieter) | Anmeldung über fremde Anbieter (OpenID Connect, z. B. Authentik, Keycloak, sowie GitHub, GitLab, Google, Microsoft, Nextcloud, Discord, Gitea), als JSON. Beispiel unten. |
| <a id="PAPERRUST_SOCIAL_AUTO_SIGNUP"></a><a id="paperrust_social_auto_signup"></a><a id="PAPERLESS_SOCIAL_AUTO_SIGNUP"></a><a id="paperless_social_auto_signup"></a>`PAPERRUST_SOCIAL_AUTO_SIGNUP` | `false` | Bei der ersten Anmeldung über einen Anbieter das Konto gleich anlegen, ohne Formular. |
| <a id="PAPERRUST_SOCIALACCOUNT_ALLOW_SIGNUPS"></a><a id="paperrust_socialaccount_allow_signups"></a><a id="PAPERLESS_SOCIALACCOUNT_ALLOW_SIGNUPS"></a><a id="paperless_socialaccount_allow_signups"></a>`PAPERRUST_SOCIALACCOUNT_ALLOW_SIGNUPS` | `true` | Neue Konten über einen Anbieter zulassen. |
| <a id="PAPERRUST_SOCIAL_ACCOUNT_SYNC_GROUPS"></a><a id="paperrust_social_account_sync_groups"></a><a id="PAPERLESS_SOCIAL_ACCOUNT_SYNC_GROUPS"></a><a id="paperless_social_account_sync_groups"></a>`PAPERRUST_SOCIAL_ACCOUNT_SYNC_GROUPS` | `false` | Bei jeder Anmeldung die Gruppen aus dem Anbieter übernehmen. Gruppen mit gleichem Namen müssen in PaperRust existieren. |
| <a id="PAPERRUST_SOCIAL_ACCOUNT_SYNC_GROUPS_CLAIM"></a><a id="paperrust_social_account_sync_groups_claim"></a><a id="PAPERLESS_SOCIAL_ACCOUNT_SYNC_GROUPS_CLAIM"></a><a id="paperless_social_account_sync_groups_claim"></a>`PAPERRUST_SOCIAL_ACCOUNT_SYNC_GROUPS_CLAIM` | `groups` | In welchem Feld (Claim) der Anbieter die Gruppen schickt. |
| <a id="PAPERRUST_SOCIAL_ACCOUNT_SYNC_SUPERUSER_GROUP"></a><a id="paperrust_social_account_sync_superuser_group"></a><a id="PAPERLESS_SOCIAL_ACCOUNT_SYNC_SUPERUSER_GROUP"></a><a id="paperless_social_account_sync_superuser_group"></a>`PAPERRUST_SOCIAL_ACCOUNT_SYNC_SUPERUSER_GROUP` | – | Wer beim Anbieter in dieser Gruppe ist, wird Superuser. Fehlt die Gruppe bei der nächsten Anmeldung, verliert er das Recht wieder. |
| <a id="PAPERRUST_SOCIAL_ACCOUNT_SYNC_STAFF_GROUP"></a><a id="paperrust_social_account_sync_staff_group"></a><a id="PAPERLESS_SOCIAL_ACCOUNT_SYNC_STAFF_GROUP"></a><a id="paperless_social_account_sync_staff_group"></a>`PAPERRUST_SOCIAL_ACCOUNT_SYNC_STAFF_GROUP` | – | Ebenso für den Staff-Status (Verwaltungsrechte ohne Superuser). |
| <a id="PAPERRUST_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR"></a><a id="paperrust_social_account_mfa_trusted_amr"></a><a id="PAPERLESS_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR"></a><a id="paperless_social_account_mfa_trusted_amr"></a>`PAPERRUST_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR` | `mfa` | Meldet der Anbieter einen dieser Werte im Feld `amr`, gilt der zweite Faktor als erledigt und PaperRust fragt ihn nicht noch einmal ab. |
| <a id="PAPERRUST_SOCIAL_ACCOUNT_DEFAULT_GROUPS"></a><a id="paperrust_social_account_default_groups"></a><a id="PAPERLESS_SOCIAL_ACCOUNT_DEFAULT_GROUPS"></a><a id="paperless_social_account_default_groups"></a>`PAPERRUST_SOCIAL_ACCOUNT_DEFAULT_GROUPS` | – | Gruppen für Konten, die über einen Anbieter neu entstehen. Die Gruppen müssen existieren. |
| <a id="PAPERRUST_ACCOUNT_DEFAULT_HTTP_PROTOCOL"></a><a id="paperrust_account_default_http_protocol"></a><a id="PAPERLESS_ACCOUNT_DEFAULT_HTTP_PROTOCOL"></a><a id="paperless_account_default_http_protocol"></a>`PAPERRUST_ACCOUNT_DEFAULT_HTTP_PROTOCOL` | `https` | Protokoll in erzeugten Links, z. B. für die Rücksprungadresse des Anbieters. |
| <a id="PAPERRUST_ACCOUNT_EMAIL_VERIFICATION"></a><a id="paperrust_account_email_verification"></a><a id="PAPERLESS_ACCOUNT_EMAIL_VERIFICATION"></a><a id="paperless_account_email_verification"></a>`PAPERRUST_ACCOUNT_EMAIL_VERIFICATION` | `optional` (ohne Mailserver `none`) | Ob neue Konten ihre Mail-Adresse bestätigen müssen: `mandatory`, `optional` oder `none`. `mandatory` braucht einen funktionierenden Mailserver und eine Mail-Adresse bei jedem Konto, sonst wartet die Anmeldung auf die Bestätigung. |
| <a id="PAPERRUST_ACCOUNT_EMAIL_UNKNOWN_ACCOUNTS"></a><a id="paperrust_account_email_unknown_accounts"></a><a id="PAPERLESS_ACCOUNT_EMAIL_UNKNOWN_ACCOUNTS"></a><a id="paperless_account_email_unknown_accounts"></a>`PAPERRUST_ACCOUNT_EMAIL_UNKNOWN_ACCOUNTS` | `true` | Auch an Adressen ohne Konto eine Mail schicken, etwa beim Zurücksetzen des Passworts. `false` verrät nicht, dass es die Adresse nicht gibt, schickt aber auch keine Mail. |
| <a id="PAPERRUST_DISABLE_REGULAR_LOGIN"></a><a id="paperrust_disable_regular_login"></a><a id="PAPERLESS_DISABLE_REGULAR_LOGIN"></a><a id="paperless_disable_regular_login"></a>`PAPERRUST_DISABLE_REGULAR_LOGIN` | `false` | Anmeldung mit Benutzername und Passwort in der Oberfläche abschalten, sodass nur noch SSO bleibt. Die API nimmt weiter Passwörter und Tokens an. |
| <a id="PAPERRUST_REDIRECT_LOGIN_TO_SSO"></a><a id="paperrust_redirect_login_to_sso"></a><a id="PAPERLESS_REDIRECT_LOGIN_TO_SSO"></a><a id="paperless_redirect_login_to_sso"></a>`PAPERRUST_REDIRECT_LOGIN_TO_SSO` | `false` | Die Anmeldeseite leitet sofort zum ersten SSO-Anbieter weiter. |
| <a id="PAPERRUST_ACCOUNT_SESSION_REMEMBER"></a><a id="paperrust_account_session_remember"></a><a id="PAPERLESS_ACCOUNT_SESSION_REMEMBER"></a><a id="paperless_account_session_remember"></a>`PAPERRUST_ACCOUNT_SESSION_REMEMBER` | `true` | `false`: Die Anmeldung endet, wenn der Browser geschlossen wird. Sonst gilt `PAPERRUST_SESSION_COOKIE_AGE`. |
| <a id="PAPERRUST_SESSION_COOKIE_AGE"></a><a id="paperrust_session_cookie_age"></a><a id="PAPERLESS_SESSION_COOKIE_AGE"></a><a id="paperless_session_cookie_age"></a>`PAPERRUST_SESSION_COOKIE_AGE` | `1814400` (3 Wochen) | So lange bleibt man angemeldet, in Sekunden. |
| <a id="PAPERRUST_SESSION_CLEANUP_TASK_CRON"></a><a id="paperrust_session_cleanup_task_cron"></a><a id="PAPERLESS_SESSION_CLEANUP_TASK_CRON"></a><a id="paperless_session_cleanup_task_cron"></a>`PAPERRUST_SESSION_CLEANUP_TASK_CRON` | `0 3 * * *` (täglich 3 Uhr) | Wann abgelaufene Anmeldungen aus der Datenbank gelöscht werden. |
| <a id="PAPERRUST_TOKEN_THROTTLE_RATE"></a><a id="paperrust_token_throttle_rate"></a><a id="PAPERLESS_TOKEN_THROTTLE_RATE"></a><a id="paperless_token_throttle_rate"></a>`PAPERRUST_TOKEN_THROTTLE_RATE` | `5/min` | Wie oft man je Zeitraum ein API-Token mit Passwort abholen darf (Schutz gegen Passwort-Raten), z. B. `20/hour`. |

Beispiel für einen OpenID-Connect-Anbieter (Rücksprungadresse beim Anbieter:
`https://<PAPERRUST_URL>/accounts/oidc/<provider_id>/login/callback/`):

```sh
PAPERRUST_APPS="allauth.socialaccount.providers.openid_connect"
PAPERRUST_SOCIALACCOUNT_PROVIDERS='{"openid_connect": {"APPS": [{"provider_id": "authentik", "name": "Authentik", "client_id": "<id>", "secret": "<secret>", "settings": {"server_url": "https://auth.example.org/application/o/paperrust/.well-known/openid-configuration"}}]}}'
```

<a id="ocr"></a><a id="ocr-settings"></a>

## OCR

PaperRust erkennt Text mit Tesseract (unter Windows wahlweise mit der eingebauten
Windows-Texterkennung, siehe [`PAPERRUST_OCR_ENGINE`](#paperrust_ocr_engine)) und zeichnet die
Seiten mit PDFium. Die Optionen dieser Gruppe kann man auch auf der Seite *Einstellungen →
Konfiguration* setzen; dort gesetzte Werte gehen der Umgebung vor.

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_OCR_LANGUAGE"></a><a id="paperrust_ocr_language"></a><a id="PAPERLESS_OCR_LANGUAGE"></a><a id="paperless_ocr_language"></a>`PAPERRUST_OCR_LANGUAGE` | `deu+eng` | Sprache(n) der Dokumente als dreibuchstabiger Tesseract-Code, mehrere mit `+`, z. B. `fra+eng`. Die Sprachdaten müssen installiert sein. Standard `deu+eng` (paperless-ngx: `eng`): eine zweite Sprache kostet nur 3–7 % Zeit, mit nur `eng` gehen bei deutschen Texten Umlaute und ß verloren. Fehlen beim Standard die deutschen Daten, nimmt PaperRust `eng`. |
| <a id="PAPERRUST_OCR_LANGUAGE_AUTO"></a><a id="paperrust_ocr_language_auto"></a>`PAPERRUST_OCR_LANGUAGE_AUTO` | `false` | Sprache automatisch: Nach der Erkennung bestimmt PaperRust die Sprache des Textes. Ist es eine andere als eingestellt (z. B. Französisch bei `deu+eng`) und sind die Sprachdaten installiert, wird das Dokument noch einmal mit dieser Sprache erkannt. Kostet nur bei solchen Dokumenten Zeit (eine zweite Erkennung); gilt erst ab etwa 200 Buchstaben und sicherer Erkennung. Gemessen: Französisch mit `deu+eng` 93 % der Wörter, mit `fra` 100 %. |
| <a id="PAPERRUST_OCR_MODE"></a><a id="paperrust_ocr_mode"></a><a id="PAPERLESS_OCR_MODE"></a><a id="paperless_ocr_mode"></a>`PAPERRUST_OCR_MODE` | `auto` | Wann erkannt wird: `auto` = nur Seiten ohne Text; `redo` = vorhandene Textebene durch neue ersetzen; `force` = alle Seiten als Bild behandeln und neu erkennen (verändert die Seiten); `off` = nie, nur vorhandenen Text übernehmen. |
| <a id="PAPERRUST_ARCHIVE_FILE_GENERATION"></a><a id="paperrust_archive_file_generation"></a><a id="PAPERLESS_ARCHIVE_FILE_GENERATION"></a><a id="paperless_archive_file_generation"></a>`PAPERRUST_ARCHIVE_FILE_GENERATION` | `auto` | Wann zusätzlich eine durchsuchbare Archivfassung (PDF/A) entsteht: `auto` = für Scans und Bilder; `always`; `never`. |
| <a id="PAPERRUST_OCR_CLEAN"></a><a id="paperrust_ocr_clean"></a><a id="PAPERLESS_OCR_CLEAN"></a><a id="paperless_ocr_clean"></a>`PAPERRUST_OCR_CLEAN` | `clean` | Scans vor der Erkennung säubern: Flecken, dunkle Ränder/Scannerkanten, Lochungen und schwach graue Flächen werden weiß (eigener Ersatz für `unpaper`). `clean`: nur das Bild für die Erkennung, im Archiv bleibt das Original. `clean-final`: das gesäuberte Bild kommt auch ins Archiv-PDF. `none`: aus. Bei OCR-Modus `redo` wird `clean-final` zu `clean`. |
| <a id="PAPERRUST_OCR_DESKEW"></a><a id="paperrust_ocr_deskew"></a><a id="PAPERLESS_OCR_DESKEW"></a><a id="paperless_ocr_deskew"></a>`PAPERRUST_OCR_DESKEW` | `true` | Schief eingezogene Seiten geraderichten (bis ±15°). Die Seite wird vor der Erkennung gedreht; das geraderichtete Bild kommt auch ins Archiv-PDF. Seiten, die schon gerade sind (unter 0,1°), bleiben unverändert. Wirkt nicht bei OCR-Modus `redo`. |
| <a id="PAPERRUST_OCR_ROTATE_PAGES"></a><a id="paperrust_ocr_rotate_pages"></a><a id="PAPERLESS_OCR_ROTATE_PAGES"></a><a id="paperless_ocr_rotate_pages"></a>`PAPERRUST_OCR_ROTATE_PAGES` | `true` | Seiten, die auf dem Kopf oder quer liegen, richtig herum drehen. Braucht die Tesseract-Daten `osd`. |
| <a id="PAPERRUST_OCR_ROTATE_PAGES_THRESHOLD"></a><a id="paperrust_ocr_rotate_pages_threshold"></a><a id="PAPERLESS_OCR_ROTATE_PAGES_THRESHOLD"></a><a id="paperless_ocr_rotate_pages_threshold"></a>`PAPERRUST_OCR_ROTATE_PAGES_THRESHOLD` | `12` | Wie sicher die Erkennung sein muss, bevor gedreht wird. Höher = vorsichtiger (etwa 15), niedriger = eifriger (etwa 2). |
| <a id="PAPERRUST_OCR_OUTPUT_TYPE"></a><a id="paperrust_ocr_output_type"></a><a id="PAPERLESS_OCR_OUTPUT_TYPE"></a><a id="paperless_ocr_output_type"></a>`PAPERRUST_OCR_OUTPUT_TYPE` | `pdfa` | Format der Archivfassung: `pdf`, `pdfa` (entspricht PDF/A-2b), `pdfa-1`, `pdfa-2` oder `pdfa-3`. PaperRust schreibt PDF/A ohne Ghostscript: Textebenen-Schrift eingebettet, sRGB-Farbprofil, XMP-Metadaten, JavaScript/verbotene Aktionen/Zusatzaktionen entfernt, Anmerkungen druckbar, eingebettete Dateien bei PDF/A-1/-2 entfernt, bei PDF/A-1 Transparenz geglättet. Danach prüft es die Datei selbst gegen die wichtigsten PDF/A-Regeln. Besteht sie nicht (meist: Schriften des Originals sind nicht eingebettet), bleibt die Datei ein normales PDF ohne PDF/A-Kennzeichen; das Log nennt den Grund. |
| <a id="PAPERRUST_OCR_PAGES"></a><a id="paperrust_ocr_pages"></a><a id="PAPERLESS_OCR_PAGES"></a><a id="paperless_ocr_pages"></a>`PAPERRUST_OCR_PAGES` | `0` (alle) | Nur die ersten N Seiten erkennen. Spart Zeit bei langen Dokumenten. |
| <a id="PAPERRUST_OCR_IMAGE_DPI"></a><a id="paperrust_ocr_image_dpi"></a><a id="PAPERLESS_OCR_IMAGE_DPI"></a><a id="paperless_ocr_image_dpi"></a>`PAPERRUST_OCR_IMAGE_DPI` | – (geschätzt) | Auflösung für Bilder, die keine Angabe dazu enthalten. Ohne Angabe schätzt PaperRust sie aus der Bildgröße. |
| <a id="PAPERRUST_OCR_MAX_IMAGE_PIXELS"></a><a id="paperrust_ocr_max_image_pixels"></a><a id="PAPERLESS_OCR_MAX_IMAGE_PIXELS"></a><a id="paperless_ocr_max_image_pixels"></a>`PAPERRUST_OCR_MAX_IMAGE_PIXELS` | – (Standardgrenze) | Größte Bildfläche in Pixeln, die erkannt wird. Darüber gibt es eine Warnung, ab der doppelten Fläche wird das Bild übersprungen. `0` hebt die Grenze auf. Schützt vor absichtlich riesigen Dateien. |
| <a id="PAPERRUST_OCR_COLOR_CONVERSION_STRATEGY"></a><a id="paperrust_ocr_color_conversion_strategy"></a><a id="PAPERLESS_OCR_COLOR_CONVERSION_STRATEGY"></a><a id="paperless_ocr_color_conversion_strategy"></a>`PAPERRUST_OCR_COLOR_CONVERSION_STRATEGY` | `RGB` | Farbumwandlung für PDF/A (`CMYK`, `Gray`, `LeaveColorUnchanged`, `RGB`, `UseDeviceIndependentColor`), wirkt auf Bilder und Vektorfarben der Archivfassung. `Gray` = alles grau, `RGB` = CMYK wird RGB, `UseDeviceIndependentColor` = Bilder mit Farbprofil, `LeaveColorUnchanged` = nur CMYK wird RGB (PDF/A verlangt das). `CMYK` geht bei PDF/A nicht (kein CMYK-Profil) und wirkt dann wie `RGB` (Warnung im Log). |
| <a id="PAPERRUST_OCR_USER_ARGS"></a><a id="paperrust_ocr_user_args"></a><a id="PAPERLESS_OCR_USER_ARGS"></a><a id="paperless_ocr_user_args"></a>`PAPERRUST_OCR_USER_ARGS` | – | Zusätzliche OCRmyPDF-Optionen als JSON-Objekt, z. B. `{"optimize": 2, "jpeg_quality": 60}`. Wirksam: `optimize` (0–3, Standard 1: verlustfrei, 1-Bit-Bilder als JBIG2; 2/3: zusätzlich JPEG-Qualität und Farbreduktion), `jpeg_quality`, `png_quality`, `pdfa_image_compression` (`jpeg`/`lossless`/`auto`), `title`, `author`, `subject`, `keywords`, `mode`, `tesseract_pagesegmode`, `tesseract_oem`, `tesseract_thresholding`, `tesseract_config`, `user_words`, `user_patterns`, `tesseract_timeout`, `tesseract_non_ocr_timeout`, `tesseract_downsample_large_images`/`tesseract_downsample_above`, `max_ocr_image_mpixels`, `remove_vectors`, `ocr_engine` (`none` = ohne Texterkennung) sowie die Optionen, die PaperRust selbst setzt. Ohne Wirkung (Warnung je Option): `jbig2_lossy` (auch in OCRmyPDF abgekündigt), `unpaper_args`, `remove_background`, `fast_web_view`, Plugins. |

Was PaperRust bei der OCR sonst noch anders macht: HEIC-Bilder liest es über `paperrust-pdftool`
(außer 4:2:2/4:4:4-Fotos und Bildfolgen), und die Textauswahl im Archiv-PDF kann leicht versetzt liegen. Vollständige Liste:
`docs/contract/pl-ocr.md`, „Known differences“.

<a id="software_tweaks"></a><a id="software-tweaks"></a>

## Leistung, Zeitpläne und Suche

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_TASK_WORKERS"></a><a id="paperrust_task_workers"></a><a id="PAPERLESS_TASK_WORKERS"></a><a id="paperless_task_workers"></a>`PAPERRUST_TASK_WORKERS` | `1` | Wie viele Hintergrundaufgaben (Einlesen, Mail, Index, Training) gleichzeitig laufen – beim Stapel-Import also, wie viele Dokumente gleichzeitig eingelesen werden. Mit `1` (wie paperless-ngx) bekommt ein einzelnes Dokument alle Kerne und ist am schnellsten fertig. `auto` wählt den Wert nach Kernen und Speicher: Kerne ÷ 2 (aufgerundet; mit gesetztem `PAPERRUST_THREADS_PER_WORKER`: Kerne ÷ diesem Wert), höchstens 4, bei wenig freiem Speicher weniger – große Stapel werden damit schneller, ein einzelnes Dokument langsamer (Raspberry Pi 5: 255 Dokumente 162 s statt 214 s, ein 8-seitiger Scan 24 s statt 15 s). Eine Zahl gilt unverändert. |
| <a id="PAPERRUST_THREADS_PER_WORKER"></a><a id="paperrust_threads_per_worker"></a><a id="PAPERLESS_THREADS_PER_WORKER"></a><a id="paperless_threads_per_worker"></a>`PAPERRUST_THREADS_PER_WORKER` | Kerne ÷ `TASK_WORKERS`, mindestens 1 | Wie viele Seiten eines Dokuments gleichzeitig erkannt werden. |
| <a id="PAPERRUST_WORKER_TIMEOUT"></a><a id="paperrust_worker_timeout"></a><a id="PAPERLESS_WORKER_TIMEOUT"></a><a id="paperless_worker_timeout"></a>`PAPERRUST_WORKER_TIMEOUT` | `1800` s | Nach dieser Zeit wird eine Aufgabe abgebrochen. Auf langsamen Rechnern mit großen Dokumenten erhöhen. |
| <a id="PAPERRUST_TIME_ZONE"></a><a id="paperrust_time_zone"></a><a id="PAPERLESS_TIME_ZONE"></a><a id="paperless_time_zone"></a>`PAPERRUST_TIME_ZONE` | `UTC` | Zeitzone, z. B. `Europe/Berlin`. Bestimmt Datumsangaben und Zeitpläne. |
| <a id="PAPERRUST_CLASSIFIER_MATCH_THRESHOLD"></a><a id="paperrust_classifier_match_threshold"></a><a id="PAPERLESS_CLASSIFIER_MATCH_THRESHOLD"></a><a id="paperless_classifier_match_threshold"></a>`PAPERRUST_CLASSIFIER_MATCH_THRESHOLD` | `0.3` | Ab dieser Sicherheit (0.0 bis 1.0) setzt die automatische Zuordnung Korrespondent, Dokumenttyp oder Speicherpfad. |
| <a id="PAPERRUST_DATE_PARSER_LANGUAGES"></a><a id="paperrust_date_parser_languages"></a><a id="PAPERLESS_DATE_PARSER_LANGUAGES"></a><a id="paperless_date_parser_languages"></a>`PAPERRUST_DATE_PARSER_LANGUAGES` | – (aus `PAPERRUST_OCR_LANGUAGE`) | Sprachen, in denen nach Datumsangaben gesucht wird, als Zweibuchstaben-Code, z. B. `de+en`. Hilft bei Monatsnamen wie „März“. |
| <a id="PAPERRUST_EMAIL_TASK_CRON"></a><a id="paperrust_email_task_cron"></a><a id="PAPERLESS_EMAIL_TASK_CRON"></a><a id="paperless_email_task_cron"></a>`PAPERRUST_EMAIL_TASK_CRON` | alle 10 Minuten | Wann Mailkonten abgerufen werden. Die genaue Minute verschiebt PaperRust je Installation etwas, damit nicht alle Server gleichzeitig abrufen. |
| <a id="PAPERRUST_TRAIN_TASK_CRON"></a><a id="paperrust_train_task_cron"></a><a id="PAPERLESS_TRAIN_TASK_CRON"></a><a id="paperless_train_task_cron"></a>`PAPERRUST_TRAIN_TASK_CRON` | `5 */1 * * *` (stündlich) | Wann die automatische Zuordnung neu lernt. |
| <a id="PAPERRUST_INDEX_TASK_CRON"></a><a id="paperrust_index_task_cron"></a><a id="PAPERLESS_INDEX_TASK_CRON"></a><a id="paperless_index_task_cron"></a>`PAPERRUST_INDEX_TASK_CRON` | `0 0 * * *` (täglich 0 Uhr) | Wann der Suchindex aufgeräumt wird. |
| <a id="PAPERRUST_SANITY_TASK_CRON"></a><a id="paperrust_sanity_task_cron"></a><a id="PAPERLESS_SANITY_TASK_CRON"></a><a id="paperless_sanity_task_cron"></a>`PAPERRUST_SANITY_TASK_CRON` | `30 0 * * sun` (sonntags 0:30) | Wann die Konsistenzprüfung läuft (fehlende Dateien, falsche Prüfsummen). |
| <a id="PAPERRUST_WORKFLOW_SCHEDULED_TASK_CRON"></a><a id="paperrust_workflow_scheduled_task_cron"></a><a id="PAPERLESS_WORKFLOW_SCHEDULED_TASK_CRON"></a><a id="paperless_workflow_scheduled_task_cron"></a>`PAPERRUST_WORKFLOW_SCHEDULED_TASK_CRON` | `5 */1 * * *` (stündlich) | Wann zeitgesteuerte Workflows geprüft werden. |
| <a id="PAPERRUST_SEARCH_LANGUAGE"></a><a id="paperrust_search_language"></a><a id="PAPERLESS_SEARCH_LANGUAGE"></a><a id="paperless_search_language"></a>`PAPERRUST_SEARCH_LANGUAGE` | – (aus `PAPERRUST_OCR_LANGUAGE`) | Sprache der Volltextsuche, damit z. B. „Rechnungen“ auch „Rechnung“ findet. Nach einer Änderung baut PaperRust den Index beim nächsten Start neu auf. |
| <a id="PAPERRUST_ADVANCED_FUZZY_SEARCH_THRESHOLD"></a><a id="paperrust_advanced_fuzzy_search_threshold"></a><a id="PAPERLESS_ADVANCED_FUZZY_SEARCH_THRESHOLD"></a><a id="paperless_advanced_fuzzy_search_threshold"></a>`PAPERRUST_ADVANCED_FUZZY_SEARCH_THRESHOLD` | – (aus) | Schwelle für ungefähre Treffer in der Volltextsuche, z. B. `0.5`. Ungefähre Treffer stehen hinter genauen. |
| <a id="PAPERRUST_ENABLE_COMPRESSION"></a><a id="paperrust_enable_compression"></a><a id="PAPERLESS_ENABLE_COMPRESSION"></a><a id="paperless_enable_compression"></a>`PAPERRUST_ENABLE_COMPRESSION` | `true` | Antworten komprimiert senden. Steht ein Reverse Proxy davor, der das schon macht, kann man es hier abschalten. |
| <a id="PAPERRUST_MAX_IMAGE_PIXELS"></a><a id="paperrust_max_image_pixels"></a><a id="PAPERLESS_MAX_IMAGE_PIXELS"></a><a id="paperless_max_image_pixels"></a>`PAPERRUST_MAX_IMAGE_PIXELS` | – (Standardgrenze) | Größte Bildfläche in Pixeln, die PaperRust überhaupt öffnet (für Vorschaubilder usw.). `0` hebt die Grenze auf. |
| <a id="PAPERRUST_APPS"></a><a id="paperrust_apps"></a><a id="PAPERLESS_APPS"></a><a id="paperless_apps"></a>`PAPERRUST_APPS` | – | Zusätzliche Module, durch Komma getrennt. In PaperRust nur noch dafür da, SSO-Anbieter einzuschalten, z. B. `allauth.socialaccount.providers.openid_connect`. |

<a id="consume_config"></a><a id="document-consumption"></a>

## Einlesen von Dokumenten

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_CONSUMER_DELETE_DUPLICATES"></a><a id="paperrust_consumer_delete_duplicates"></a><a id="PAPERLESS_CONSUMER_DELETE_DUPLICATES"></a><a id="paperless_consumer_delete_duplicates"></a>`PAPERRUST_CONSUMER_DELETE_DUPLICATES` | `false` | Eine Datei, die es schon gibt (gleiche Prüfsumme), ablehnen und aus dem Eingangsordner löschen. Sonst werden Doppelte normal eingelesen. |
| <a id="PAPERRUST_CONSUMER_RECURSIVE"></a><a id="paperrust_consumer_recursive"></a><a id="PAPERLESS_CONSUMER_RECURSIVE"></a><a id="paperless_consumer_recursive"></a>`PAPERRUST_CONSUMER_RECURSIVE` | `false` | Auch Unterordner des Eingangsordners beobachten. |
| <a id="PAPERRUST_CONSUMER_SUBDIRS_AS_TAGS"></a><a id="paperrust_consumer_subdirs_as_tags"></a><a id="PAPERLESS_CONSUMER_SUBDIRS_AS_TAGS"></a><a id="paperless_consumer_subdirs_as_tags"></a>`PAPERRUST_CONSUMER_SUBDIRS_AS_TAGS` | `false` | Die Namen der Unterordner als Tags vergeben; fehlende Tags werden angelegt. Braucht `PAPERRUST_CONSUMER_RECURSIVE`. Aus `consume/Steuer/2025/beleg.pdf` werden die Tags „Steuer“ und „2025“. |
| <a id="PAPERRUST_CONSUMER_IGNORE_PATTERNS"></a><a id="paperrust_consumer_ignore_patterns"></a><a id="PAPERLESS_CONSUMER_IGNORE_PATTERNS"></a><a id="paperless_consumer_ignore_patterns"></a>`PAPERRUST_CONSUMER_IGNORE_PATTERNS` | `[]` | Zusätzliche Muster (reguläre Ausdrücke als JSON-Liste) für Dateinamen, die liegen bleiben sollen. Übliche Systemdateien wie `.DS_Store`, `desktop.ini` oder `Thumbs.db` überspringt PaperRust ohnehin. |
| <a id="PAPERRUST_CONSUMER_IGNORE_DIRS"></a><a id="paperrust_consumer_ignore_dirs"></a><a id="PAPERLESS_CONSUMER_IGNORE_DIRS"></a><a id="paperless_consumer_ignore_dirs"></a>`PAPERRUST_CONSUMER_IGNORE_DIRS` | `[]` | Zusätzliche Ordnernamen (JSON-Liste), die samt Inhalt übersprungen werden. Einige Systemordner (z. B. `@eaDir` von Synology) sind schon eingebaut. |
| <a id="PAPERRUST_PRE_CONSUME_SCRIPT"></a><a id="paperrust_pre_consume_script"></a><a id="PAPERLESS_PRE_CONSUME_SCRIPT"></a><a id="paperless_pre_consume_script"></a>`PAPERRUST_PRE_CONSUME_SCRIPT` | – | Programm, das vor dem Einlesen jeder Datei läuft. Siehe [Fortgeschrittenes](advanced.md#pre-consume-script). Nur über die Umgebung. |
| <a id="PAPERRUST_POST_CONSUME_SCRIPT"></a><a id="paperrust_post_consume_script"></a><a id="PAPERLESS_POST_CONSUME_SCRIPT"></a><a id="paperless_post_consume_script"></a>`PAPERRUST_POST_CONSUME_SCRIPT` | – | Programm, das nach dem Einlesen läuft. Siehe [Fortgeschrittenes](advanced.md#post-consume-script). Nur über die Umgebung. |
| <a id="PAPERRUST_FILENAME_DATE_ORDER"></a><a id="paperrust_filename_date_order"></a><a id="PAPERLESS_FILENAME_DATE_ORDER"></a><a id="paperless_filename_date_order"></a>`PAPERRUST_FILENAME_DATE_ORDER` | – (aus) | Auch im Dateinamen nach einem Datum suchen, in dieser Reihenfolge, z. B. `YMD` für `2025-03-14_rechnung.pdf`. Ein Datum im Dateinamen geht dem im Text vor. |
| <a id="PAPERRUST_DATE_ORDER"></a><a id="paperrust_date_order"></a><a id="PAPERLESS_DATE_ORDER"></a><a id="paperless_date_order"></a>`PAPERRUST_DATE_ORDER` | `DMY` | In welcher Reihenfolge Tag, Monat und Jahr im Text stehen. Für US-Dokumente `MDY`. |
| <a id="PAPERRUST_NUMBER_OF_SUGGESTED_DATES"></a><a id="paperrust_number_of_suggested_dates"></a><a id="PAPERLESS_NUMBER_OF_SUGGESTED_DATES"></a><a id="paperless_number_of_suggested_dates"></a>`PAPERRUST_NUMBER_OF_SUGGESTED_DATES` | `3` | Wie viele weitere gefundene Daten beim Bearbeiten als Vorschlag erscheinen. `0` = keine. |
| <a id="PAPERRUST_IGNORE_DATES"></a><a id="paperrust_ignore_dates"></a><a id="PAPERLESS_IGNORE_DATES"></a><a id="paperless_ignore_dates"></a>`PAPERRUST_IGNORE_DATES` | – | Daten, die nie als Dokumentdatum gelten sollen, z. B. das eigene Geburtsdatum. Durch Komma getrennt, im Format `JJJJ-MM-TT`. |
| <a id="PAPERRUST_THUMBNAIL_FONT_NAME"></a><a id="paperrust_thumbnail_font_name"></a><a id="PAPERLESS_THUMBNAIL_FONT_NAME"></a><a id="paperless_thumbnail_font_name"></a>`PAPERRUST_THUMBNAIL_FONT_NAME` | `/usr/share/fonts/liberation/LiberationSerif-Regular.ttf` | Schriftdatei für Vorschaubilder von Text- und Office-Dateien. Fehlt sie, gibt es ein schlichtes Standardbild. |
| <a id="PAPERRUST_CONSUMER_POLLING_INTERVAL"></a><a id="paperrust_consumer_polling_interval"></a><a id="PAPERLESS_CONSUMER_POLLING_INTERVAL"></a><a id="paperless_consumer_polling_interval"></a>`PAPERRUST_CONSUMER_POLLING_INTERVAL` | `0` | Wie oft PaperRust im Eingangsordner nach neuen Dateien sieht, in Sekunden. `0` heißt wie in paperless-ngx: auf Meldungen des Betriebssystems warten; klappt das nicht (z. B. Netzlaufwerk), sieht PaperRust jede Sekunde nach. Größere Werte = Abfrage in diesem Abstand, entlasten langsame Laufwerke. |
| <a id="PAPERRUST_CONSUMER_STABILITY_DELAY"></a><a id="paperrust_consumer_stability_delay"></a><a id="PAPERLESS_CONSUMER_STABILITY_DELAY"></a><a id="paperless_consumer_stability_delay"></a>`PAPERRUST_CONSUMER_STABILITY_DELAY` | `5` s | So lange muss eine Datei unverändert bleiben, bevor PaperRust sie liest. Verhindert, dass halb kopierte Dateien eingelesen werden. |

<a id="workflow-webhooks"></a>

## Workflow-Webhooks

Diese drei Grenzen legen fest, wohin Workflows Webhooks schicken dürfen. Sie gelten nur über die
Umgebung, damit ein Konto in der Oberfläche keine Anfragen ins interne Netz auslösen kann.

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_WEBHOOKS_ALLOWED_SCHEMES"></a><a id="paperrust_webhooks_allowed_schemes"></a><a id="PAPERLESS_WEBHOOKS_ALLOWED_SCHEMES"></a><a id="paperless_webhooks_allowed_schemes"></a>`PAPERRUST_WEBHOOKS_ALLOWED_SCHEMES` | `http,https` | Erlaubte Adressarten. |
| <a id="PAPERRUST_WEBHOOKS_ALLOWED_PORTS"></a><a id="paperrust_webhooks_allowed_ports"></a><a id="PAPERLESS_WEBHOOKS_ALLOWED_PORTS"></a><a id="paperless_webhooks_allowed_ports"></a>`PAPERRUST_WEBHOOKS_ALLOWED_PORTS` | – (alle) | Erlaubte Ports, z. B. `443`. |
| <a id="PAPERRUST_WEBHOOKS_ALLOW_INTERNAL_REQUESTS"></a><a id="paperrust_webhooks_allow_internal_requests"></a><a id="PAPERLESS_WEBHOOKS_ALLOW_INTERNAL_REQUESTS"></a><a id="paperless_webhooks_allow_internal_requests"></a>`PAPERRUST_WEBHOOKS_ALLOW_INTERNAL_REQUESTS` | `true` | `false` sperrt Ziele im eigenen Netz (z. B. `localhost`, `192.168.…`). |

<a id="incoming_mail"></a><a id="incoming-mail"></a>

## Eingehende Mails

Die Mailkonten selbst richtet man in der Oberfläche ein, siehe [E-Mail](mail.md). Hier stehen nur
die Einstellungen für alle Konten.

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_EMAIL_ALLOW_INTERNAL_HOSTS"></a><a id="paperrust_email_allow_internal_hosts"></a><a id="PAPERLESS_EMAIL_ALLOW_INTERNAL_HOSTS"></a><a id="paperless_email_allow_internal_hosts"></a>`PAPERRUST_EMAIL_ALLOW_INTERNAL_HOSTS` | `true` | `false` sperrt Mailkonten, deren Server im eigenen Netz liegt. Nur über die Umgebung. |

<a id="email_oauth"></a><a id="email-oauth"></a>

### Anmeldung bei Gmail und Outlook (OAuth)

Für Gmail- und Outlook-Konten mit OAuth braucht man eine eigene App beim jeweiligen Anbieter. Deren
Rücksprungadresse ist `<Basisadresse>/api/oauth/callback/`.

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_OAUTH_CALLBACK_BASE_URL"></a><a id="paperrust_oauth_callback_base_url"></a><a id="PAPERLESS_OAUTH_CALLBACK_BASE_URL"></a><a id="paperless_oauth_callback_base_url"></a>`PAPERRUST_OAUTH_CALLBACK_BASE_URL` | – (dann `PAPERRUST_URL`) | Basisadresse für die Rücksprungadresse. Eine der beiden muss gesetzt sein. |
| <a id="PAPERRUST_GMAIL_OAUTH_CLIENT_ID"></a><a id="paperrust_gmail_oauth_client_id"></a><a id="PAPERLESS_GMAIL_OAUTH_CLIENT_ID"></a><a id="paperless_gmail_oauth_client_id"></a>`PAPERRUST_GMAIL_OAUTH_CLIENT_ID` | – | Client-ID der Google-App. |
| <a id="PAPERRUST_GMAIL_OAUTH_CLIENT_SECRET"></a><a id="paperrust_gmail_oauth_client_secret"></a><a id="PAPERLESS_GMAIL_OAUTH_CLIENT_SECRET"></a><a id="paperless_gmail_oauth_client_secret"></a>`PAPERRUST_GMAIL_OAUTH_CLIENT_SECRET` | – | Geheimnis der Google-App. |
| <a id="PAPERRUST_OUTLOOK_OAUTH_CLIENT_ID"></a><a id="paperrust_outlook_oauth_client_id"></a><a id="PAPERLESS_OUTLOOK_OAUTH_CLIENT_ID"></a><a id="paperless_outlook_oauth_client_id"></a>`PAPERRUST_OUTLOOK_OAUTH_CLIENT_ID` | – | Client-ID der Microsoft-App. |
| <a id="PAPERRUST_OUTLOOK_OAUTH_CLIENT_SECRET"></a><a id="paperrust_outlook_oauth_client_secret"></a><a id="PAPERLESS_OUTLOOK_OAUTH_CLIENT_SECRET"></a><a id="paperless_outlook_oauth_client_secret"></a>`PAPERRUST_OUTLOOK_OAUTH_CLIENT_SECRET` | – | Geheimnis der Microsoft-App. |

<a id="encrypted_emails"></a><a id="encrypted-emails"></a>

### Verschlüsselte Mails

Mit `PAPERLESS_ENABLE_GPG_DECRYPTOR=true` entschlüsselt PaperRust GPG-verschlüsselte Mails vor dem
Einlesen, Schlüssel aus `PAPERLESS_EMAIL_GNUPG_HOME` (sonst Standardordner von GnuPG). Nötig ist das
Programm `gpg` (nicht im Docker-Image). Fehlt es oder scheitert die Entschlüsselung, wird die Mail
nicht verarbeitet und ein Fehler steht im Log. Siehe [Verschlüsselte E-Mails](advanced.md).

<a id="barcodes"></a>

## Barcodes

Barcodes können einen Stapel Scans in mehrere Dokumente teilen, eine Archivnummer (ASN) setzen
oder Tags vergeben. Die Erkennung braucht PDFium. Auch diese Optionen kann man auf der Seite
*Einstellungen → Konfiguration* setzen.

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_CONSUMER_ENABLE_BARCODES"></a><a id="paperrust_consumer_enable_barcodes"></a><a id="PAPERLESS_CONSUMER_ENABLE_BARCODES"></a><a id="paperless_consumer_enable_barcodes"></a>`PAPERRUST_CONSUMER_ENABLE_BARCODES` | `false` | Nach Trennblättern suchen und die Datei an jedem Trennblatt teilen. |
| <a id="PAPERRUST_CONSUMER_BARCODE_TIFF_SUPPORT"></a><a id="paperrust_consumer_barcode_tiff_support"></a><a id="PAPERLESS_CONSUMER_BARCODE_TIFF_SUPPORT"></a><a id="paperless_consumer_barcode_tiff_support"></a>`PAPERRUST_CONSUMER_BARCODE_TIFF_SUPPORT` | `false` | Auch TIFF-Dateien nach Barcodes durchsuchen. |
| <a id="PAPERRUST_CONSUMER_BARCODE_STRING"></a><a id="paperrust_consumer_barcode_string"></a><a id="PAPERLESS_CONSUMER_BARCODE_STRING"></a><a id="paperless_consumer_barcode_string"></a>`PAPERRUST_CONSUMER_BARCODE_STRING` | `PATCHT` | Inhalt des Barcodes auf dem Trennblatt. |
| <a id="PAPERRUST_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES"></a><a id="paperrust_consumer_barcode_retain_split_pages"></a><a id="PAPERLESS_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES"></a><a id="paperless_consumer_barcode_retain_split_pages"></a>`PAPERRUST_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES` | `false` | Das Trennblatt behalten (als erste Seite des folgenden Dokuments), statt es wegzuwerfen. |
| <a id="PAPERRUST_CONSUMER_ENABLE_ASN_BARCODE"></a><a id="paperrust_consumer_enable_asn_barcode"></a><a id="PAPERLESS_CONSUMER_ENABLE_ASN_BARCODE"></a><a id="paperless_consumer_enable_asn_barcode"></a>`PAPERRUST_CONSUMER_ENABLE_ASN_BARCODE` | `false` | Archivnummer aus einem Barcode übernehmen, z. B. `ASN00123` → 123. Ein ASN-Barcode trennt zugleich das Dokument. |
| <a id="PAPERRUST_CONSUMER_ASN_BARCODE_PREFIX"></a><a id="paperrust_consumer_asn_barcode_prefix"></a><a id="PAPERLESS_CONSUMER_ASN_BARCODE_PREFIX"></a><a id="paperless_consumer_asn_barcode_prefix"></a>`PAPERRUST_CONSUMER_ASN_BARCODE_PREFIX` | `ASN` | Vorsilbe, an der ein ASN-Barcode erkannt wird. |
| <a id="PAPERRUST_CONSUMER_BARCODE_UPSCALE"></a><a id="paperrust_consumer_barcode_upscale"></a><a id="PAPERLESS_CONSUMER_BARCODE_UPSCALE"></a><a id="paperless_consumer_barcode_upscale"></a>`PAPERRUST_CONSUMER_BARCODE_UPSCALE` | `0.0` | Seiten vor der Suche vergrößern, z. B. `1.5`. Hilft bei kleinen Barcodes; wirkt erst über 1.0 und kostet Speicher. |
| <a id="PAPERRUST_CONSUMER_BARCODE_DPI"></a><a id="paperrust_consumer_barcode_dpi"></a><a id="PAPERLESS_CONSUMER_BARCODE_DPI"></a><a id="paperless_consumer_barcode_dpi"></a>`PAPERRUST_CONSUMER_BARCODE_DPI` | `300` | Auflösung, mit der Seiten für die Suche gezeichnet werden. |
| <a id="PAPERRUST_CONSUMER_BARCODE_MAX_PAGES"></a><a id="paperrust_consumer_barcode_max_pages"></a><a id="PAPERLESS_CONSUMER_BARCODE_MAX_PAGES"></a><a id="paperless_consumer_barcode_max_pages"></a>`PAPERRUST_CONSUMER_BARCODE_MAX_PAGES` | `0` (alle) | Nur die ersten N Seiten durchsuchen. |
| <a id="PAPERRUST_CONSUMER_ENABLE_TAG_BARCODE"></a><a id="paperrust_consumer_enable_tag_barcode"></a><a id="PAPERLESS_CONSUMER_ENABLE_TAG_BARCODE"></a><a id="paperless_consumer_enable_tag_barcode"></a>`PAPERRUST_CONSUMER_ENABLE_TAG_BARCODE` | `false` | Tags aus Barcodes vergeben (und fehlende anlegen), wenn der Inhalt zu einem Muster aus `PAPERRUST_CONSUMER_TAG_BARCODE_MAPPING` passt. Ein Barcode darf mehrere Tags mit Komma enthalten. |
| <a id="PAPERRUST_CONSUMER_TAG_BARCODE_MAPPING"></a><a id="paperrust_consumer_tag_barcode_mapping"></a><a id="PAPERLESS_CONSUMER_TAG_BARCODE_MAPPING"></a><a id="paperless_consumer_tag_barcode_mapping"></a>`PAPERRUST_CONSUMER_TAG_BARCODE_MAPPING` | `{"TAG:(.*)": "\\g<1>"}` | Zuordnung „Muster → Tag-Name“ als JSON. Der Standard macht aus `TAG:Steuer` das Tag „Steuer“. |
| <a id="PAPERRUST_CONSUMER_TAG_BARCODE_SPLIT"></a><a id="paperrust_consumer_tag_barcode_split"></a><a id="PAPERLESS_CONSUMER_TAG_BARCODE_SPLIT"></a><a id="paperless_consumer_tag_barcode_split"></a>`PAPERRUST_CONSUMER_TAG_BARCODE_SPLIT` | `false` | Auch an Tag-Barcodes ein neues Dokument beginnen. |
| <a id="PAPERRUST_CONSUMER_STORE_BARCODE_VALUES"></a><a id="paperrust_consumer_store_barcode_values"></a><a id="PAPERLESS_CONSUMER_STORE_BARCODE_VALUES"></a><a id="paperless_consumer_store_barcode_values"></a>`PAPERRUST_CONSUMER_STORE_BARCODE_VALUES` | `false` | Den Inhalt aller gefundenen Barcodes beim Dokument speichern, damit man danach suchen kann. |

Beispiel: Barcodes `ABT:Buchhaltung` sollen das Tag „Buchhaltung“ ergeben, `TAG:…` weiter wie
gewohnt:

```sh
PAPERRUST_CONSUMER_TAG_BARCODE_MAPPING='{"TAG:(.*)": "\\g<1>", "ABT:(.*)": "\\g<1>"}'
```

<a id="audit-trail"></a>

## Änderungsprotokoll

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_AUDIT_LOG_ENABLED"></a><a id="paperrust_audit_log_enabled"></a><a id="PAPERLESS_AUDIT_LOG_ENABLED"></a><a id="paperless_audit_log_enabled"></a>`PAPERRUST_AUDIT_LOG_ENABLED` | `true` | Festhalten, wer wann was an Dokumenten, Dokumenttypen, Korrespondenten und Tags geändert hat. Abschalten löscht vorhandene Einträge nicht. |

<a id="collate"></a><a id="collate-double-sided-documents"></a>

## Doppelseitige Scans zusammenführen

Für Scanner, die nur einseitig scannen können: Man scannt erst alle Vorderseiten, dann den Stapel
umgedreht alle Rückseiten. Beide Dateien kommen nacheinander in einen eigenen Unterordner des
Eingangsordners; PaperRust mischt die Seiten zu einem Dokument.

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_CONSUMER_ENABLE_COLLATE_DOUBLE_SIDED"></a><a id="paperrust_consumer_enable_collate_double_sided"></a><a id="PAPERLESS_CONSUMER_ENABLE_COLLATE_DOUBLE_SIDED"></a><a id="paperless_consumer_enable_collate_double_sided"></a>`PAPERRUST_CONSUMER_ENABLE_COLLATE_DOUBLE_SIDED` | `false` | Zusammenführen einschalten. |
| <a id="PAPERRUST_CONSUMER_COLLATE_DOUBLE_SIDED_SUBDIR_NAME"></a><a id="paperrust_consumer_collate_double_sided_subdir_name"></a><a id="PAPERLESS_CONSUMER_COLLATE_DOUBLE_SIDED_SUBDIR_NAME"></a><a id="paperless_consumer_collate_double_sided_subdir_name"></a>`PAPERRUST_CONSUMER_COLLATE_DOUBLE_SIDED_SUBDIR_NAME` | `double-sided` | Name des Unterordners. Man legt ihn selbst an. |
| <a id="PAPERRUST_CONSUMER_COLLATE_DOUBLE_SIDED_TIFF_SUPPORT"></a><a id="paperrust_consumer_collate_double_sided_tiff_support"></a><a id="PAPERLESS_CONSUMER_COLLATE_DOUBLE_SIDED_TIFF_SUPPORT"></a><a id="paperless_consumer_collate_double_sided_tiff_support"></a>`PAPERRUST_CONSUMER_COLLATE_DOUBLE_SIDED_TIFF_SUPPORT` | `false` | Auch TIFF-Dateien zusammenführen. |

Die Rückseiten-Datei muss gleich viele Seiten haben wie die Vorderseiten-Datei und spätestens
30 Minuten nach ihr ankommen.

<a id="trash"></a>

## Papierkorb

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_EMPTY_TRASH_DELAY"></a><a id="paperrust_empty_trash_delay"></a><a id="PAPERLESS_EMPTY_TRASH_DELAY"></a><a id="paperless_empty_trash_delay"></a>`PAPERRUST_EMPTY_TRASH_DELAY` | `30` Tage | So lange liegen gelöschte Dokumente im Papierkorb, bevor sie endgültig verschwinden (mindestens 1). |
| <a id="PAPERRUST_EMPTY_TRASH_TASK_CRON"></a><a id="paperrust_empty_trash_task_cron"></a><a id="PAPERLESS_EMPTY_TRASH_TASK_CRON"></a><a id="paperless_empty_trash_task_cron"></a>`PAPERRUST_EMPTY_TRASH_TASK_CRON` | `0 1 * * *` (täglich 1 Uhr) | Wann der Papierkorb von abgelaufenen Einträgen befreit wird. |

<a id="share-links"></a>

## Freigabelinks

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_SHARE_LINK_BUNDLE_CLEANUP_CRON"></a><a id="paperrust_share_link_bundle_cleanup_cron"></a><a id="PAPERLESS_SHARE_LINK_BUNDLE_CLEANUP_CRON"></a><a id="paperless_share_link_bundle_cleanup_cron"></a>`PAPERRUST_SHARE_LINK_BUNDLE_CLEANUP_CRON` | `0 2 * * *` (täglich 2 Uhr) | Wann abgelaufene Freigabe-Bündel samt ZIP-Datei entfernt werden. |

<a id="binaries"></a>

## Programme

PaperRust braucht nur wenige Hilfsprogramme: Tesseract
([`PAPERRUST_TESSERACT_PATH`](#paperrust_tesseract_path),
[`PAPERRUST_TESSDATA_DIR`](#paperrust_tessdata_dir)), die PDFium-Bibliothek
([`PAPERRUST_PDFIUM_PATH`](#paperrust_pdfium_path)) und das eigene PDF-Werkzeug
([`PAPERRUST_PDFTOOL_BIN`](#paperrust_pdftool_bin)). Die Programmpfade von paperless-ngx
(`PAPERLESS_CONVERT_BINARY`, `PAPERLESS_GS_BINARY`) werden [ignoriert](#ignored).

<a id="docker"></a><a id="docker-specific-options"></a>

## Docker-Image

Diese Variablen wertet nur der Startvorgang des PaperRust-Docker-Images aus, nicht das Programm
selbst. Das Image wird derzeit aus dem Repository gebaut, siehe [Installation](installation.md#docker).

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="USERMAP_UID"></a><a id="usermap_uid"></a>`USERMAP_UID` | `1000` | Benutzer-ID, unter der PaperRust im Container läuft (auf dem Host mit `id -u` nachsehen). Wirkt nur, wenn der Container als root startet (`user: root`). Ersatzweise gilt `PUID`. |
| <a id="USERMAP_GID"></a><a id="usermap_gid"></a>`USERMAP_GID` | `1000` | Gruppen-ID dazu (`id -g`). Ersatzweise gilt `PGID`. |
| <a id="PAPERRUST_OCR_LANGUAGES"></a><a id="paperrust_ocr_languages"></a><a id="PAPERLESS_OCR_LANGUAGES"></a><a id="paperless_ocr_languages"></a>`PAPERRUST_OCR_LANGUAGES` | – | Zusätzliche Tesseract-Sprachen, die beim Start nachinstalliert werden, z. B. `tur ces`. Geht nur, wenn der Container als root startet. Besser: die Sprachen schon beim Bauen wählen (`--build-arg TESSERACT_LANGS="eng deu fra ita spa"`, das ist auch der Standard). |
| <a id="PAPERRUST_BIND_ADDR"></a><a id="paperrust_bind_addr"></a><a id="PAPERLESS_BIND_ADDR"></a><a id="paperless_bind_addr"></a>`PAPERRUST_BIND_ADDR` | `0.0.0.0` | Adresse, auf der PaperRust im Container lauscht – nur, wenn [`PAPERRUST_BIND`](#paperrust_bind) nicht gesetzt ist. |
| <a id="PAPERRUST_PORT"></a><a id="paperrust_port"></a><a id="PAPERLESS_PORT"></a><a id="paperless_port"></a>`PAPERRUST_PORT` | `8000` | Port dazu. |

<a id="frontend-settings"></a>

## Name, Logo und Versionsprüfung

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_APP_TITLE"></a><a id="paperrust_app_title"></a><a id="PAPERLESS_APP_TITLE"></a><a id="paperless_app_title"></a>`PAPERRUST_APP_TITLE` | – („PaperRust“) | Eigener Name der Anwendung, z. B. „Familienarchiv“. |
| <a id="PAPERRUST_APP_LOGO"></a><a id="paperrust_app_logo"></a><a id="PAPERLESS_APP_LOGO"></a><a id="paperless_app_logo"></a>`PAPERRUST_APP_LOGO` | – | Eigenes Logo, als Pfad unterhalb von `media/` mit „logo“ im Namen, z. B. `/logo/familie.svg`. Am einfachsten lädt man es auf der Seite *Einstellungen → Konfiguration* hoch. Das Logo ist auch auf der Anmeldeseite zu sehen, also für jeden. |
| <a id="PAPERRUST_ENABLE_UPDATE_CHECK"></a><a id="paperrust_enable_update_check"></a><a id="PAPERLESS_ENABLE_UPDATE_CHECK"></a><a id="paperless_enable_update_check"></a>`PAPERRUST_ENABLE_UPDATE_CHECK` | `default` | Ob PaperRust nach neuen Versionen sieht. Normalerweise schaltet man das in der Oberfläche um; die Variable wird nur einmal übernommen, solange dort nichts gewählt ist. Quelle: [`PAPERRUST_REMOTE_VERSION_URL`](#paperrust_remote_version_url). |

<a id="email-sending"></a>

## E-Mail-Versand

PaperRust verschickt Mails zum Zurücksetzen von Passwörtern, aus Workflows und beim Versand von
Dokumenten. Ohne Mailserver funktioniert alles andere trotzdem.

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_EMAIL_HOST"></a><a id="paperrust_email_host"></a><a id="PAPERLESS_EMAIL_HOST"></a><a id="paperless_email_host"></a>`PAPERRUST_EMAIL_HOST` | `localhost` | Mailserver für ausgehende Mails (SMTP). |
| <a id="PAPERRUST_EMAIL_PORT"></a><a id="paperrust_email_port"></a><a id="PAPERLESS_EMAIL_PORT"></a><a id="paperless_email_port"></a>`PAPERRUST_EMAIL_PORT` | `25` | Port des Mailservers; üblich sind 587 (STARTTLS) oder 465 (SSL). |
| <a id="PAPERRUST_EMAIL_HOST_USER"></a><a id="paperrust_email_host_user"></a><a id="PAPERLESS_EMAIL_HOST_USER"></a><a id="paperless_email_host_user"></a>`PAPERRUST_EMAIL_HOST_USER` | leer | Benutzername am Mailserver. |
| <a id="PAPERRUST_EMAIL_HOST_PASSWORD"></a><a id="paperrust_email_host_password"></a><a id="PAPERLESS_EMAIL_HOST_PASSWORD"></a><a id="paperless_email_host_password"></a>`PAPERRUST_EMAIL_HOST_PASSWORD` | leer | Passwort am Mailserver. Wird verschlüsselt gespeichert, wenn man es in der Oberfläche setzt. |
| <a id="PAPERRUST_EMAIL_FROM"></a><a id="paperrust_email_from"></a><a id="PAPERLESS_EMAIL_FROM"></a><a id="paperless_email_from"></a>`PAPERRUST_EMAIL_FROM` | – (dann `PAPERRUST_EMAIL_HOST_USER`) | Absenderadresse. |
| <a id="PAPERRUST_EMAIL_USE_TLS"></a><a id="paperrust_email_use_tls"></a><a id="PAPERLESS_EMAIL_USE_TLS"></a><a id="paperless_email_use_tls"></a>`PAPERRUST_EMAIL_USE_TLS` | `false` | Verbindung mit STARTTLS verschlüsseln (meist Port 587). |
| <a id="PAPERRUST_EMAIL_USE_SSL"></a><a id="paperrust_email_use_ssl"></a><a id="PAPERLESS_EMAIL_USE_SSL"></a><a id="paperless_email_use_ssl"></a>`PAPERRUST_EMAIL_USE_SSL` | `false` | Von Anfang an verschlüsselt verbinden (meist Port 465). Nicht zusammen mit `USE_TLS`. |
| <a id="PAPERRUST_EMAIL_SUBJECT_PREFIX"></a><a id="paperrust_email_subject_prefix"></a>`PAPERRUST_EMAIL_SUBJECT_PREFIX` | `[PaperRust] ` | Steht vor dem Betreff der Mails, die PaperRust selbst verschickt (z. B. Passwort zurücksetzen). |

<a id="remote-ocr"></a>

## Texterkennung in der Cloud

Statt Tesseract kann ein Cloud-Dienst den Text erkennen. Derzeit geht nur Azure AI Document
Intelligence. Die Dokumente verlassen dabei den eigenen Rechner.
Der Dienst liefert den Text und ein durchsuchbares PDF, das zur Archivfassung wird. Der API-Schlüssel
geht nur an die eingetragene Adresse und erscheint in keinem Log. Mit `workflow_only` geht ein Dokument
nur in die Cloud, wenn ein Workflow mit der Aktion „Remote OCR“ greift oder beim Neu-Verarbeiten
„Remote OCR“ gewählt wird; sonst erkennt Tesseract den Text.

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_REMOTE_OCR_ENGINE"></a><a id="paperrust_remote_ocr_engine"></a><a id="PAPERLESS_REMOTE_OCR_ENGINE"></a><a id="paperless_remote_ocr_engine"></a>`PAPERRUST_REMOTE_OCR_ENGINE` | – (aus) | Dienst; derzeit nur `azureai`. |
| <a id="PAPERRUST_REMOTE_OCR_API_KEY"></a><a id="paperrust_remote_ocr_api_key"></a><a id="PAPERLESS_REMOTE_OCR_API_KEY"></a><a id="paperless_remote_ocr_api_key"></a>`PAPERRUST_REMOTE_OCR_API_KEY` | – | API-Schlüssel des Dienstes. |
| <a id="PAPERRUST_REMOTE_OCR_ENDPOINT"></a><a id="paperrust_remote_ocr_endpoint"></a><a id="PAPERLESS_REMOTE_OCR_ENDPOINT"></a><a id="paperless_remote_ocr_endpoint"></a>`PAPERRUST_REMOTE_OCR_ENDPOINT` | – | Adresse des Dienstes; für Azure Pflicht. |
| <a id="PAPERRUST_REMOTE_OCR_MODE"></a><a id="paperrust_remote_ocr_mode"></a><a id="PAPERLESS_REMOTE_OCR_MODE"></a><a id="paperless_remote_ocr_mode"></a>`PAPERRUST_REMOTE_OCR_MODE` | `always` | `always` = jedes passende Dokument geht an den Dienst; `workflow_only` = nur, wenn ein Workflow oder „Neu verarbeiten“ es verlangt. |
| <a id="PAPERRUST_REMOTE_OCR_ALLOW_INTERNAL_ENDPOINTS"></a><a id="paperrust_remote_ocr_allow_internal_endpoints"></a><a id="PAPERLESS_REMOTE_OCR_ALLOW_INTERNAL_ENDPOINTS"></a><a id="paperless_remote_ocr_allow_internal_endpoints"></a>`PAPERRUST_REMOTE_OCR_ALLOW_INTERNAL_ENDPOINTS` | `true` | `false` sperrt Adressen im eigenen Netz. Nur über die Umgebung. |

<a id="ai"></a>

## KI

Was die KI-Funktionen tun, steht unter [KI](ai.md). Dort gibt es auch die PaperRust-eigenen
[Erweiterungen](#paperrust-settings).

| Variable | Standard | Bedeutung |
|---|---|---|
| <a id="PAPERRUST_AI_ENABLED"></a><a id="paperrust_ai_enabled"></a><a id="PAPERLESS_AI_ENABLED"></a><a id="paperless_ai_enabled"></a>`PAPERRUST_AI_ENABLED` | `false` | Hauptschalter für alle KI-Funktionen. |
| <a id="PAPERRUST_AI_LLM_BACKEND"></a><a id="paperrust_ai_llm_backend"></a><a id="PAPERLESS_AI_LLM_BACKEND"></a><a id="paperless_ai_llm_backend"></a>`PAPERRUST_AI_LLM_BACKEND` | – | Anbieter des Sprachmodells: `openai-like` (jede OpenAI-kompatible Schnittstelle) oder `ollama` (läuft auf dem eigenen Rechner). |
| <a id="PAPERRUST_AI_LLM_MODEL"></a><a id="paperrust_ai_llm_model"></a><a id="PAPERLESS_AI_LLM_MODEL"></a><a id="paperless_ai_llm_model"></a>`PAPERRUST_AI_LLM_MODEL` | – (`gpt-3.5-turbo` bzw. `llama3.1`) | Name des Modells. |
| <a id="PAPERRUST_AI_LLM_API_KEY"></a><a id="paperrust_ai_llm_api_key"></a><a id="PAPERLESS_AI_LLM_API_KEY"></a><a id="paperless_ai_llm_api_key"></a>`PAPERRUST_AI_LLM_API_KEY` | – | API-Schlüssel; bei OpenAI-kompatiblen Diensten meist nötig. |
| <a id="PAPERRUST_AI_LLM_ENDPOINT"></a><a id="paperrust_ai_llm_endpoint"></a><a id="PAPERLESS_AI_LLM_ENDPOINT"></a><a id="paperless_ai_llm_endpoint"></a>`PAPERRUST_AI_LLM_ENDPOINT` | – | Adresse des Anbieters; bei Ollama Pflicht, z. B. `http://ollama:11434`. |
| <a id="PAPERRUST_AI_LLM_OUTPUT_LANGUAGE"></a><a id="paperrust_ai_llm_output_language"></a><a id="PAPERLESS_AI_LLM_OUTPUT_LANGUAGE"></a><a id="paperless_ai_llm_output_language"></a>`PAPERRUST_AI_LLM_OUTPUT_LANGUAGE` | – (Sprache der Oberfläche) | In welcher Sprache die Vorschläge kommen. |
| <a id="PAPERRUST_AI_LLM_CONTEXT_SIZE"></a><a id="paperrust_ai_llm_context_size"></a><a id="PAPERLESS_AI_LLM_CONTEXT_SIZE"></a><a id="paperless_ai_llm_context_size"></a>`PAPERRUST_AI_LLM_CONTEXT_SIZE` | `8192` | Wie viel Text auf einmal ans Modell geht (in Tokens). Bei Ollama wird der Wert auch als `num_ctx` mitgeschickt. |
| <a id="PAPERRUST_AI_LLM_REQUEST_TIMEOUT"></a><a id="paperrust_ai_llm_request_timeout"></a><a id="PAPERLESS_AI_LLM_REQUEST_TIMEOUT"></a><a id="paperless_ai_llm_request_timeout"></a>`PAPERRUST_AI_LLM_REQUEST_TIMEOUT` | `120` s | Zeitgrenze für eine Anfrage an den Anbieter. |
| <a id="PAPERRUST_AI_LLM_EMBEDDING_BACKEND"></a><a id="paperrust_ai_llm_embedding_backend"></a><a id="PAPERLESS_AI_LLM_EMBEDDING_BACKEND"></a><a id="paperless_ai_llm_embedding_backend"></a>`PAPERRUST_AI_LLM_EMBEDDING_BACKEND` | – | Anbieter für Embeddings (die Zahlenvektoren für die inhaltliche Suche und den Dokument-Chat): `openai-like`, `huggingface` oder `ollama`. |
| <a id="PAPERRUST_AI_LLM_EMBEDDING_MODEL"></a><a id="paperrust_ai_llm_embedding_model"></a><a id="PAPERLESS_AI_LLM_EMBEDDING_MODEL"></a><a id="paperless_ai_llm_embedding_model"></a>`PAPERRUST_AI_LLM_EMBEDDING_MODEL` | – (je Anbieter, z. B. `text-embedding-3-small`) | Modell für Embeddings. |
| <a id="PAPERRUST_AI_LLM_EMBEDDING_API_KEY"></a><a id="paperrust_ai_llm_embedding_api_key"></a><a id="PAPERLESS_AI_LLM_EMBEDDING_API_KEY"></a><a id="paperless_ai_llm_embedding_api_key"></a>`PAPERRUST_AI_LLM_EMBEDDING_API_KEY` | – (dann `PAPERRUST_AI_LLM_API_KEY`) | API-Schlüssel für Embeddings. |
| <a id="PAPERRUST_AI_LLM_EMBEDDING_ENDPOINT"></a><a id="paperrust_ai_llm_embedding_endpoint"></a><a id="PAPERLESS_AI_LLM_EMBEDDING_ENDPOINT"></a><a id="paperless_ai_llm_embedding_endpoint"></a>`PAPERRUST_AI_LLM_EMBEDDING_ENDPOINT` | – (dann `PAPERRUST_AI_LLM_ENDPOINT`) | Adresse für Embeddings. |
| <a id="PAPERRUST_AI_LLM_EMBEDDING_CHUNK_SIZE"></a><a id="paperrust_ai_llm_embedding_chunk_size"></a><a id="PAPERLESS_AI_LLM_EMBEDDING_CHUNK_SIZE"></a><a id="paperless_ai_llm_embedding_chunk_size"></a>`PAPERRUST_AI_LLM_EMBEDDING_CHUNK_SIZE` | `1024` | In wie große Stücke der Dokumenttext zerlegt wird. Kleiner stellen, wenn das Modell zu lange Eingaben ablehnt. |
| <a id="PAPERRUST_AI_LLM_EXTRA_PARAMS"></a><a id="paperrust_ai_llm_extra_params"></a><a id="PAPERLESS_AI_LLM_EXTRA_PARAMS"></a><a id="paperless_ai_llm_extra_params"></a>`PAPERRUST_AI_LLM_EXTRA_PARAMS` | leer | Weitere Felder als JSON-Objekt, die jeder Anfrage unverändert beigelegt werden, z. B. `{"temperature": 0.2}`. Sie überschreiben PaperRusts eigene Werte. |
| <a id="PAPERRUST_AI_LLM_ALLOW_INTERNAL_ENDPOINTS"></a><a id="paperrust_ai_llm_allow_internal_endpoints"></a><a id="PAPERLESS_AI_LLM_ALLOW_INTERNAL_ENDPOINTS"></a><a id="paperless_ai_llm_allow_internal_endpoints"></a>`PAPERRUST_AI_LLM_ALLOW_INTERNAL_ENDPOINTS` | `true` | `false` sperrt Adressen im eigenen Netz. Nur über die Umgebung. |
| <a id="PAPERRUST_LLM_INDEX_TASK_CRON"></a><a id="paperrust_llm_index_task_cron"></a><a id="PAPERLESS_LLM_INDEX_TASK_CRON"></a><a id="paperless_llm_index_task_cron"></a>`PAPERRUST_LLM_INDEX_TASK_CRON` | `10 2 * * *` (täglich 2:10 Uhr) | Wann der KI-Index aktualisiert wird. Läuft nur, wenn KI an und ein Embedding-Anbieter gesetzt ist. |

<a id="ignored"></a><a id="optional-services"></a><a id="tika"></a><a id="email-parsing"></a>

## Wird nicht gebraucht oder ignoriert

Diese Variablen von paperless-ngx kann man stehen lassen; PaperRust stört sich nicht daran. Die
meisten haben keine Wirkung; Zeilen mit „wirkt“ gelten inzwischen wieder (sie stehen hier, weil
man sie bei paperless-ngx oft im Zusammenhang mit Tika oder Celery gesetzt hat). Beim Aufräumen hilft [Herkunft & Umzug](#settings-ui).

| Variable | Grund |
|---|---|
| <a id="PAPERRUST_TIKA_ENABLED"></a><a id="paperrust_tika_enabled"></a><a id="PAPERLESS_TIKA_ENABLED"></a><a id="paperless_tika_enabled"></a>`PAPERLESS_TIKA_ENABLED` | PaperRust liest Office-Dateien und Mails selbst; Tika ist nicht nötig. |
| <a id="PAPERRUST_TIKA_ENDPOINT"></a><a id="paperrust_tika_endpoint"></a><a id="PAPERLESS_TIKA_ENDPOINT"></a><a id="paperless_tika_endpoint"></a>`PAPERLESS_TIKA_ENDPOINT` | wie oben, kein Tika. |
| <a id="PAPERRUST_TIKA_GOTENBERG_ENDPOINT"></a><a id="paperrust_tika_gotenberg_endpoint"></a><a id="PAPERLESS_TIKA_GOTENBERG_ENDPOINT"></a><a id="paperless_tika_gotenberg_endpoint"></a>`PAPERLESS_TIKA_GOTENBERG_ENDPOINT` | kein Gotenberg; die PDF-Fassung von Office-Dateien und Mails erzeugt `paperrust-pdftool`. |
| <a id="PAPERRUST_EMAIL_PARSE_DEFAULT_LAYOUT"></a><a id="paperrust_email_parse_default_layout"></a><a id="PAPERLESS_EMAIL_PARSE_DEFAULT_LAYOUT"></a><a id="paperless_email_parse_default_layout"></a>`PAPERLESS_EMAIL_PARSE_DEFAULT_LAYOUT` | **wirkt wieder**: Reihenfolge im PDF einer eingelesenen Mail ohne Mailregel (1 = Text, dann HTML (Standard), 2 = HTML, dann Text, 3 = nur HTML, 4 = nur Text); mit Mailregel gilt deren „PDF-Layout“. Siehe [Office-Dateien beim Einlesen](pdf-tools.md#office-beim-einlesen). |
| <a id="PAPERRUST_DB_READ_CACHE_ENABLED"></a><a id="paperrust_db_read_cache_enabled"></a><a id="PAPERLESS_DB_READ_CACHE_ENABLED"></a><a id="paperless_db_read_cache_enabled"></a>`PAPERLESS_DB_READ_CACHE_ENABLED` | Lese-Zwischenspeicher von Django; PaperRust hat keinen. |
| <a id="PAPERRUST_READ_CACHE_TTL"></a><a id="paperrust_read_cache_ttl"></a><a id="PAPERLESS_READ_CACHE_TTL"></a><a id="paperless_read_cache_ttl"></a>`PAPERLESS_READ_CACHE_TTL` | gehört zum Lese-Zwischenspeicher von Django. |
| <a id="PAPERRUST_READ_CACHE_REDIS_URL"></a><a id="paperrust_read_cache_redis_url"></a><a id="PAPERLESS_READ_CACHE_REDIS_URL"></a><a id="paperless_read_cache_redis_url"></a>`PAPERLESS_READ_CACHE_REDIS_URL` | gehört zum Lese-Zwischenspeicher von Django. |
| <a id="PAPERRUST_CHANNELS_BACKEND"></a><a id="paperrust_channels_backend"></a><a id="PAPERLESS_CHANNELS_BACKEND"></a><a id="paperless_channels_backend"></a>`PAPERLESS_CHANNELS_BACKEND` | Django-Channels; PaperRust schickt Statusmeldungen selbst. |
| <a id="PAPERRUST_MATCH_REGEX_TIMEOUT_SECONDS"></a><a id="paperrust_match_regex_timeout_seconds"></a><a id="PAPERLESS_MATCH_REGEX_TIMEOUT_SECONDS"></a><a id="paperless_match_regex_timeout_seconds"></a>`PAPERLESS_MATCH_REGEX_TIMEOUT_SECONDS` | **wirkt**: Zeitlimit je Zuordnungs-Regex wie in paperless-ngx (Python-`regex`-Sondersyntax wie `{e<=1}` bleibt ungültig). |
| <a id="PAPERRUST_ENABLE_GPG_DECRYPTOR"></a><a id="paperrust_enable_gpg_decryptor"></a><a id="PAPERLESS_ENABLE_GPG_DECRYPTOR"></a><a id="paperless_enable_gpg_decryptor"></a>`PAPERLESS_ENABLE_GPG_DECRYPTOR` | **wirkt**: siehe [Verschlüsselte Mails](#encrypted-emails) (Programm `gpg` nötig). |
| <a id="PAPERRUST_EMAIL_GNUPG_HOME"></a><a id="paperrust_email_gnupg_home"></a><a id="PAPERLESS_EMAIL_GNUPG_HOME"></a><a id="paperless_email_gnupg_home"></a>`PAPERLESS_EMAIL_GNUPG_HOME` | **wirkt**: GnuPG-Ordner für die Mail-Entschlüsselung. |
| <a id="PAPERRUST_CONVERT_BINARY"></a><a id="paperrust_convert_binary"></a><a id="PAPERLESS_CONVERT_BINARY"></a><a id="paperless_convert_binary"></a>`PAPERLESS_CONVERT_BINARY` | ImageMagick wird nicht benutzt. |
| <a id="PAPERRUST_CONVERT_MEMORY_LIMIT"></a><a id="paperrust_convert_memory_limit"></a><a id="PAPERLESS_CONVERT_MEMORY_LIMIT"></a><a id="paperless_convert_memory_limit"></a>`PAPERLESS_CONVERT_MEMORY_LIMIT` | ImageMagick wird nicht benutzt. |
| <a id="PAPERRUST_CONVERT_TMPDIR"></a><a id="paperrust_convert_tmpdir"></a><a id="PAPERLESS_CONVERT_TMPDIR"></a><a id="paperless_convert_tmpdir"></a>`PAPERLESS_CONVERT_TMPDIR` | ImageMagick wird nicht benutzt. |
| <a id="PAPERRUST_GS_BINARY"></a><a id="paperrust_gs_binary"></a><a id="PAPERLESS_GS_BINARY"></a><a id="paperless_gs_binary"></a>`PAPERLESS_GS_BINARY` | Ghostscript wird nicht benutzt. |
| <a id="PAPERRUST_OCR_SKIP_ARCHIVE_FILE"></a><a id="paperrust_ocr_skip_archive_file"></a><a id="PAPERLESS_OCR_SKIP_ARCHIVE_FILE"></a><a id="paperless_ocr_skip_archive_file"></a>`PAPERLESS_OCR_SKIP_ARCHIVE_FILE` | alter Name; heute [`PAPERRUST_ARCHIVE_FILE_GENERATION`](#paperrust_archive_file_generation). |
| <a id="PAPERRUST_ENABLE_NLTK"></a><a id="paperrust_enable_nltk"></a><a id="PAPERLESS_ENABLE_NLTK"></a><a id="paperless_enable_nltk"></a>`PAPERLESS_ENABLE_NLTK` | schon in paperless-ngx 3.2 entfallen. |
| <a id="PAPERRUST_NLTK_DIR"></a><a id="paperrust_nltk_dir"></a><a id="PAPERLESS_NLTK_DIR"></a><a id="paperless_nltk_dir"></a>`PAPERLESS_NLTK_DIR` | schon in paperless-ngx 3.2 entfallen; ein alter NLTK-Ordner kann weg. |
| <a id="PAPERRUST_WEBSERVER_WORKERS"></a><a id="paperrust_webserver_workers"></a><a id="PAPERLESS_WEBSERVER_WORKERS"></a><a id="paperless_webserver_workers"></a>`PAPERLESS_WEBSERVER_WORKERS` | PaperRust ist ein einziger Prozess mit mehreren Threads (begrenzbar mit `TOKIO_WORKER_THREADS`); das Docker-Image weist beim Start darauf hin. |
| <a id="PAPERRUST_CONSUMER_DISABLE"></a><a id="paperrust_consumer_disable"></a><a id="PAPERLESS_CONSUMER_DISABLE"></a><a id="paperless_consumer_disable"></a>`PAPERLESS_CONSUMER_DISABLE` | **wirkt**: jeder nicht leere Wert schaltet die Überwachung des Eingangsordners im Worker ab (der Befehl `document_consumer` bleibt nutzbar). |
| <a id="PAPERRUST_ENABLE_FLOWER"></a><a id="paperrust_enable_flower"></a><a id="PAPERLESS_ENABLE_FLOWER"></a><a id="paperless_enable_flower"></a>`PAPERLESS_ENABLE_FLOWER` | Flower überwacht Celery; ohne Celery kein Flower. Laufende Aufgaben zeigt die Oberfläche. |
| <a id="PAPERRUST_SUPERVISORD_WORKING_DIR"></a><a id="paperrust_supervisord_working_dir"></a><a id="PAPERLESS_SUPERVISORD_WORKING_DIR"></a><a id="paperless_supervisord_working_dir"></a>`PAPERLESS_SUPERVISORD_WORKING_DIR` | schon in paperless-ngx ohne Wirkung. |
