"""Generate the German reference table of docs/user/configuration.md.
python -I cfg_de.py <cfg.json> <header_de.md> <out.md>
"""
import json
import re
import sys

SEC = {
    "Required services": "Benötigte Dienste",
    "Message Broker": "Message Broker",
    "Database": "Datenbank",
    "Optional Services": "Optionale Dienste",
    "Tika": "Tika",
    "Email Parsing": "E-Mail-Auswertung",
    "Paths and folders": "Pfade und Ordner",
    "Logging": "Protokollierung",
    "Hosting & Security": "Betrieb und Sicherheit",
    "Authentication & SSO": "Anmeldung und SSO",
    "OCR settings": "OCR-Einstellungen",
    "Software tweaks": "Feineinstellungen",
    "Document Consumption": "Einlesen von Dokumenten",
    "Workflow webhooks": "Workflow-Webhooks",
    "Incoming Mail": "Eingehende Mails",
    "Email OAuth": "E-Mail-OAuth",
    "Encrypted Emails": "Verschlüsselte Mails",
    "Barcodes": "Barcodes",
    "Audit Trail": "Änderungsprotokoll (Audit Trail)",
    "Collate Double-Sided Documents": "Doppelseitige Dokumente zusammenführen",
    "Trash": "Papierkorb",
    "Share links": "Freigabelinks",
    "Binaries": "Programme",
    "Docker-specific options": "Docker-spezifische Optionen",
    "Frontend Settings": "Einstellungen der Oberfläche",
    "Email sending": "E-Mail-Versand",
    "Remote OCR": "Remote-OCR",
    "AI": "KI",
}

NOTES = {
    "Message Broker": ("NOTE", "**PaperRust:** braucht keinen Broker (kein Redis, kein Celery; die Warteschlange liegt in PostgreSQL). Nach dem Umstieg von paperless-ngx können der Redis-/Valkey-Dienst und `PAPERLESS_REDIS` weg; bleibt die Variable gesetzt, steht nur ein Hinweis im Log."),
    "Database": ("IMPORTANT", "**PaperRust:** Unterstützt wird nur **PostgreSQL ≥ 14** (getestet mit 18). Die Beschreibungen unten stammen von upstream, das zusätzlich SQLite und MariaDB kennt. Eine bestehende SQLite-/MariaDB-Installation zieht man vorher um, siehe [Umstieg von paperless-ngx](migration.md)."),
    "Tika": ("NOTE", "**PaperRust:** Tika und Gotenberg werden nicht gebraucht. PaperRust liest Office-Dokumente (DOCX, XLSX, PPTX, ODT/ODS/ODP, RTF) und Mails (EML, HTML) selbst. Unterschiede: keine PDF-Fassung von Office-Dateien (das Original wird gespeichert und ist durchsuchbar), Vorschaubilder werden aus dem Text gezeichnet, die Textaufteilung kann von Tika abweichen. Details: `docs/contract/pl-parsers.md`."),
    "OCR settings": ("NOTE", "**PaperRust:** OCR läuft mit Tesseract und PDFium statt OCRmyPDF, Ghostscript und unpaper. Noch nicht ausgeführt werden Bereinigen (`PAPERLESS_OCR_CLEAN`) und Geraderichten (`PAPERLESS_OCR_DESKEW`); die PDF/A-Ausgabe setzt nur die Metadaten. HEIC-Bilder können nicht gelesen werden. Details: `docs/contract/pl-ocr.md`, „Known differences“."),
    "Binaries": ("NOTE", "**PaperRust:** Die externen Programme von PaperRust sind Tesseract (`PAPERRUST_TESSERACT_PATH`, `PAPERRUST_TESSDATA_DIR`) und die PDFium-Bibliothek (`PAPERRUST_PDFIUM_PATH`), siehe [Einstellungen nur in PaperRust](#paperrust-settings)."),
    "Docker-specific options": ("NOTE", "**PaperRust:** Diese Optionen gehören zum upstream-Container-Image (`ghcr.io/paperless-ngx/paperless-ngx`). Ein PaperRust-Image ist geplant, aber noch nicht veröffentlicht."),
    "Encrypted Emails": ("WARNING", "**PaperRust:** Die GPG-Entschlüsselung von Mails ist nicht umgesetzt; verschlüsselte Mails werden unverändert verarbeitet."),
}

# var -> (default, meaning). Default "" -> "–".
D = {
 "PAPERLESS_REDIS": ("`redis://localhost:6379`", "Adresse des Brokers für geplante Aufgaben (Mail-Abruf, Index, Klassifikator-Training). Zugangsdaten und DB-Index können in der URL stehen."),
 "PAPERLESS_REDIS_PREFIX": ("kein Präfix", "Präfix für Schlüssel und Kanäle im Broker; erlaubt mehrere Instanzen an einem Broker."),
 "PAPERLESS_DBENGINE": ("`sqlite` (upstream)", "Datenbank-Engine: `sqlite`, `postgresql` oder `mariadb`."),
 "PAPERLESS_DBHOST": ("–", "Hostname des PostgreSQL-/MariaDB-Servers; Pflicht bei `postgresql`/`mariadb`."),
 "PAPERLESS_DBPORT": ("`5432` (PostgreSQL), `3306` (MariaDB)", "Port des Datenbankservers."),
 "PAPERLESS_DBNAME": ("`paperless`", "Name der Datenbank."),
 "PAPERLESS_DBUSER": ("`paperless`", "Datenbank-Benutzer."),
 "PAPERLESS_DBPASS": ("`paperless`", "Passwort des Datenbank-Benutzers."),
 "PAPERLESS_DB_OPTIONS": ("–", "Weitere Verbindungsoptionen als `schlüssel=wert`, kommagetrennt (z. B. `sslmode=require`, `pool.max_size=20`); unbekannte Schlüssel gehen ungeprüft an den Treiber."),
 "PAPERLESS_DBSSLMODE": ("–", "Seit v3 entfernt; stattdessen `PAPERLESS_DB_OPTIONS=\"sslmode=require\"`."),
 "PAPERLESS_DBSSLROOTCERT": ("–", "Seit v3 entfernt; stattdessen `PAPERLESS_DB_OPTIONS=\"sslrootcert=…\"`."),
 "PAPERLESS_DBSSLCERT": ("–", "Seit v3 entfernt; stattdessen `PAPERLESS_DB_OPTIONS=\"sslcert=…\"`."),
 "PAPERLESS_DBSSLKEY": ("–", "Seit v3 entfernt; stattdessen `PAPERLESS_DB_OPTIONS=\"sslkey=…\"`."),
 "PAPERLESS_DB_TIMEOUT": ("–", "Seit v3 entfernt; stattdessen `PAPERLESS_DB_OPTIONS=\"connect_timeout=30\"`."),
 "PAPERLESS_DB_POOLSIZE": ("–", "Upstream seit v3 entfernt (stattdessen `PAPERLESS_DB_OPTIONS=\"pool.max_size=10\"`)."),
 "PAPERLESS_DB_READ_CACHE_ENABLED": ("`false`", "Zwischenspeicher für Lese-Abfragen im Broker (cachalot). Die Datenbank darf dann nicht außerhalb der Anwendung geändert werden."),
 "PAPERLESS_READ_CACHE_TTL": ("`3600` (1 Stunde)", "Gültigkeit des Lese-Caches in Sekunden (1 bis 31536000)."),
 "PAPERLESS_READ_CACHE_REDIS_URL": ("`None` (gleicher Broker)", "Eigener Broker für den Lese-Cache."),
 "PAPERLESS_TIKA_ENABLED": ("`false`", "Tika-Parser für Office- und Mail-Dateien ein-/ausschalten."),
 "PAPERLESS_TIKA_ENDPOINT": ("`http://localhost:9998`", "Adresse des Tika-Servers."),
 "PAPERLESS_TIKA_GOTENBERG_ENDPOINT": ("`http://localhost:3000`", "Adresse des Gotenberg-Servers (Umwandlung nach PDF)."),
 "PAPERLESS_EMAIL_PARSE_DEFAULT_LAYOUT": ("–", "Standard-Layout eingelesener Mails: `1` Text, dann HTML; `2` HTML, dann Text; `3` nur HTML; `4` nur Text. Mail-Regeln können es überschreiben."),
 "PAPERLESS_CONSUMPTION_DIR": ("`../consume/` (relativ zu `src`)", "Eingangsordner, aus dem Dokumente eingelesen werden; muss existieren und les-/schreibbar sein."),
 "PAPERLESS_DATA_DIR": ("`../data/` (relativ zu `src`)", "Ablage für Suchindex, Klassifikator-Modell und weitere Daten."),
 "PAPERLESS_EMPTY_TRASH_DIR": ("leer (Dateien werden wirklich gelöscht)", "Ordner, in den Originale beim endgültigen Löschen verschoben statt gelöscht werden; muss vorher existieren. Früher `PAPERLESS_TRASH_DIR`."),
 "PAPERLESS_SCRATCH_DIR": ("`/tmp/paperless`", "Arbeitsordner für temporäre Dateien (Einlesen, Bearbeiten, Export)."),
 "PAPERLESS_MEDIA_ROOT": ("`../media/` (relativ zu `src`)", "Ablage der Dokumente und Vorschaubilder."),
 "PAPERLESS_STATICDIR": ("`../static/` (relativ zu `src`)", "Ordner der statischen Dateien (`STATIC_ROOT`, Angular-Oberfläche). Für PaperRust ohne Django zeigt er auf den Angular-Build samt `collectstatic`-Ergebnis."),
 "PAPERLESS_FILENAME_FORMAT": ("keiner (Funktion aus)", "Vorlage für Dateinamen im Medienordner, siehe [Dateinamen und Speicherpfade](storage-paths.md#file-name-handling)."),
 "PAPERLESS_FILENAME_FORMAT_REMOVE_NONE": ("`false`", "Platzhalter, die zu „none“ würden, im Dateinamen (auch Ordnernamen) weglassen."),
 "PAPERLESS_LOGGING_DIR": ("`PAPERLESS_DATA_DIR/log/`", "Ordner für Protokolldateien."),
 "PAPERLESS_NLTK_DIR": ("–", "Seit v3.2 entfernt und ignoriert; ein alter NLTK-Datenordner kann gelöscht werden."),
 "PAPERLESS_MODEL_FILE": ("`PAPERLESS_DATA_DIR/classification_model.pickle`", "Speicherort des (Python-)Klassifikator-Modells. PaperRust verwendet ein eigenes Modell, siehe `PAPERRUST_CLASSIFIER_MODEL_FILE`."),
 "PAPERLESS_LOGROTATE_MAX_SIZE": ("1 MiB", "Größe in Byte, ab der Protokolldateien rotiert werden."),
 "PAPERLESS_LOGROTATE_MAX_BACKUPS": ("20", "Anzahl aufbewahrter rotierter Protokolldateien."),
 "PAPERLESS_SECRET_KEY": ("kein Standard – Pflicht", "Schlüssel für Sitzungen und Signaturen; ohne ihn startet die Anwendung nicht. Bestehende Installationen behalten ihren bisherigen Wert, sonst werden Sitzungen und Tokens ungültig."),
 "PAPERLESS_URL": ("leer", "Öffentliche Adresse ohne Schrägstrich am Ende (z. B. `https://paperless.example.com`); setzt zugleich `ALLOWED_HOSTS`, `CORS_ALLOWED_HOSTS` und `CSRF_TRUSTED_ORIGINS`. Darf keinen Pfad enthalten."),
 "PAPERLESS_CSRF_TRUSTED_ORIGINS": ("leer", "Vertrauenswürdige Ursprünge für schreibende Anfragen (z. B. POST); kann über `PAPERLESS_URL` gesetzt werden."),
 "PAPERLESS_ALLOWED_HOSTS": ("`*` (alle Hosts)", "Erlaubte Hostnamen (kommagetrennt); schützt vor Host-Header-Angriffen. Im Internet unbedingt setzen."),
 "PAPERLESS_CORS_ALLOWED_HOSTS": ("`http://localhost:8000`", "Hosts, die CORS-Anfragen stellen dürfen; kann über `PAPERLESS_URL` gesetzt werden."),
 "PAPERLESS_TRUSTED_PROXIES": ("leer", "IP-Adressen vertrauenswürdiger Proxys (gegen IP-Fälschung, z. B. für fail2ban); bestimmt auch die Proxy-Anzahl für allauth."),
 "PAPERLESS_ALLAUTH_TRUSTED_PROXY_COUNT": ("Anzahl der Einträge in `PAPERLESS_TRUSTED_PROXIES`", "Anzahl vertrauenswürdiger Proxy-Stufen in `X-Forwarded-For` für allauth (Stufen, nicht Adressen)."),
 "PAPERLESS_ALLAUTH_TRUSTED_CLIENT_IP_HEADER": ("–", "Eigener Header mit der echten Client-IP (z. B. `X-Real-IP`, `CF-Connecting-IP`) für allauth; hat Vorrang vor der Proxy-Anzahl."),
 "PAPERLESS_FORCE_SCRIPT_NAME": ("– (Betrieb unter `/`)", "Unterpfad, unter dem die Anwendung läuft, z. B. `/paperless`, ohne Schrägstrich am Ende."),
 "PAPERLESS_STATIC_URL": ("`/static/`", "URL der statischen Dateien (mit Schrägstrich am Ende); bei Unterpfad passend zu `PAPERLESS_FORCE_SCRIPT_NAME` setzen."),
 "PAPERLESS_AUTO_LOGIN_USERNAME": ("– (aus)", "Meldet automatisch diesen Benutzer an. Nie bei Zugriff aus dem Internet verwenden."),
 "PAPERLESS_ADMIN_USER": ("–", "Legt beim Start einen Superuser mit diesem Namen an (braucht `PAPERLESS_ADMIN_PASSWORD`); ändert keine bestehenden Benutzer."),
 "PAPERLESS_ADMIN_MAIL": ("`root@localhost`", "E-Mail-Adresse des automatisch angelegten Superusers."),
 "PAPERLESS_ADMIN_PASSWORD": ("–", "Passwort des automatisch angelegten Superusers."),
 "PAPERLESS_COOKIE_PREFIX": ("`\"\"`", "Präfix für die Cookie-Namen, z. B. für zwei Instanzen auf demselben Host. Danach ist eine neue Anmeldung nötig."),
 "PAPERLESS_ENABLE_HTTP_REMOTE_USER": ("`false`", "Anmeldung über den Header `Remote-User` (SSO-Proxy). Der Proxy muss den Header von außen zuverlässig entfernen."),
 "PAPERLESS_ENABLE_HTTP_REMOTE_USER_API": ("`false`", "Anmeldung über den Remote-User-Header auch direkt an der API; gleiche Vorsicht wie oben."),
 "PAPERLESS_HTTP_REMOTE_USER_HEADER_NAME": ("`HTTP_REMOTE_USER`", "Name des Headers mit dem Benutzernamen (Django-`META`-Schreibweise, beginnt mit `HTTP_`)."),
 "PAPERLESS_LOGOUT_REDIRECT_URL": ("– (aus)", "Adresse, auf die nach dem Abmelden weitergeleitet wird (z. B. Abmeldeseite des SSO)."),
 "PAPERLESS_USE_X_FORWARD_HOST": ("`false`", "Django-Einstellung `USE_X_FORWARDED_HOST` für den Betrieb hinter einem Proxy."),
 "PAPERLESS_USE_X_FORWARD_PORT": ("`false`", "Django-Einstellung `USE_X_FORWARDED_PORT` für den Betrieb hinter einem Proxy."),
 "PAPERLESS_PROXY_SSL_HEADER": ("–", "Django-Einstellung `SECURE_PROXY_SSL_HEADER` als JSON-Liste, z. B. `'[\"HTTP_X_FORWARDED_PROTO\", \"https\"]'`. Sicherheitsrelevant."),
 "PAPERLESS_EMAIL_CERTIFICATE_LOCATION": ("–", "Zusätzliche Zertifikatsdatei (z. B. selbstsigniert), der bei Verbindungen zu Mailservern vertraut wird. Sicherheitsrelevant."),
 "PAPERLESS_ACCOUNT_ALLOW_SIGNUPS": ("`false`", "Selbstregistrierung neuer Konten erlauben."),
 "PAPERLESS_ACCOUNT_DEFAULT_GROUPS": ("–", "Gruppen (müssen existieren), in die neu registrierte Benutzer kommen."),
 "PAPERLESS_SOCIALACCOUNT_PROVIDERS": ("– (kein Anbieter)", "JSON-Konfiguration der Social-Login-Anbieter (django-allauth); die zugehörige App gehört in `PAPERLESS_APPS`."),
 "PAPERLESS_SOCIAL_AUTO_SIGNUP": ("`false`", "Benutzer beim Social-Login automatisch mit den übermittelten Daten registrieren."),
 "PAPERLESS_SOCIALACCOUNT_ALLOW_SIGNUPS": ("`true`", "Registrierung über eingerichtete Fremdanbieter erlauben."),
 "PAPERLESS_SOCIAL_ACCOUNT_SYNC_GROUPS": ("`false`", "Gruppenmitgliedschaften beim Login aus dem Fremdsystem (z. B. OIDC) übernehmen; Gruppen müssen gleichnamig existieren."),
 "PAPERLESS_SOCIAL_ACCOUNT_SYNC_GROUPS_CLAIM": ("`groups`", "Name des Claims mit den Gruppen (wirkt nur mit `PAPERLESS_SOCIAL_ACCOUNT_SYNC_GROUPS`)."),
 "PAPERLESS_SOCIAL_ACCOUNT_SYNC_SUPERUSER_GROUP": ("–", "Gruppe im Claim, die Superuser- und Staff-Status verleiht; fehlt sie, wird der Status beim nächsten Login entzogen."),
 "PAPERLESS_SOCIAL_ACCOUNT_SYNC_STAFF_GROUP": ("–", "Gruppe im Claim, die Staff-Status verleiht; fehlt sie (und ist der Benutzer kein Superuser), wird der Status entzogen."),
 "PAPERLESS_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR": ("`mfa`", "Werte im `amr`-Claim des OIDC-ID-Tokens, bei denen die eigene Zwei-Faktor-Abfrage entfällt."),
 "PAPERLESS_SOCIAL_ACCOUNT_DEFAULT_GROUPS": ("–", "Gruppen (müssen existieren) für Benutzer, die sich über Social-Login registrieren."),
 "PAPERLESS_ACCOUNT_DEFAULT_HTTP_PROTOCOL": ("`https`", "Protokoll für erzeugte URLs, z. B. Login-Callbacks."),
 "PAPERLESS_ACCOUNT_EMAIL_VERIFICATION": ("`optional` (ohne Mailserver `none`)", "Ob E-Mail-Adressen bei der Registrierung bestätigt werden (allauth)."),
 "PAPERLESS_ACCOUNT_EMAIL_UNKNOWN_ACCOUNTS": ("`true` (allauth)", "allauth-Einstellung `ACCOUNT_EMAIL_UNKNOWN_ACCOUNTS` (Mails an unbekannte Adressen, z. B. beim Passwort-Zurücksetzen)."),
 "PAPERLESS_DISABLE_REGULAR_LOGIN": ("`false`", "Normale Anmeldung mit Benutzername/Passwort in der Oberfläche abschalten (z. B. nach SSO-Einrichtung); API und Django-Admin bleiben möglich."),
 "PAPERLESS_REDIRECT_LOGIN_TO_SSO": ("`false`", "Login-Seite leitet automatisch zum ersten SSO-Anbieter weiter."),
 "PAPERLESS_ACCOUNT_SESSION_REMEMBER": ("`true`", "`false`: Sitzung endet beim Schließen des Browsers; sonst gilt `PAPERLESS_SESSION_COOKIE_AGE`."),
 "PAPERLESS_SESSION_COOKIE_AGE": ("`1209600` (2 Wochen)", "Gültigkeit des Sitzungs-Cookies in Sekunden."),
 "PAPERLESS_TOKEN_THROTTLE_RATE": ("`5/min`", "Ratenbegrenzung für `/api/token/` gegen Passwort-Raten (DRF-Format, z. B. `100/hour`)."),
 "PAPERLESS_OCR_LANGUAGE": ("`eng`", "OCR-Sprache(n) als Tesseract-Code (3 Buchstaben), mehrere mit `+`, z. B. `deu+eng`."),
 "PAPERLESS_OCR_MODE": ("`auto`", "Wann OCR läuft: `auto` (nur ohne vorhandenen Text), `redo` (vorhandene Textebenen ersetzen), `force` (Seiten rastern und neu erkennen), `off`."),
 "PAPERLESS_ARCHIVE_FILE_GENERATION": ("`auto`", "Wann eine PDF/A-Archivfassung entsteht: `auto` (für gescannte/Bild-Dokumente), `always`, `never`."),
 "PAPERLESS_OCR_CLEAN": ("`clean`", "Eingangsbilder vor der OCR mit unpaper bereinigen (`clean`, `clean-final`, `none`)."),
 "PAPERLESS_OCR_DESKEW": ("`true`", "Schräg eingescannte Seiten geraderichten (nicht mit Modus `redo`)."),
 "PAPERLESS_OCR_ROTATE_PAGES": ("`true`", "Um 90°/180°/270° gedrehte Seiten automatisch drehen."),
 "PAPERLESS_OCR_ROTATE_PAGES_THRESHOLD": ("`12`", "Schwelle für das automatische Drehen (Tesseract-Wert; 15 vorsichtig, 2 aggressiv)."),
 "PAPERLESS_OCR_OUTPUT_TYPE": ("`pdfa`", "Art der erzeugten PDFs: `pdf` (möglichst unverändert), `pdfa` (PDF/A-2b) oder `pdfa-1`/`pdfa-2`/`pdfa-3`."),
 "PAPERLESS_OCR_PAGES": ("– (alle Seiten)", "Nur die ersten N Seiten per OCR erkennen (N ≥ 1)."),
 "PAPERLESS_OCR_IMAGE_DPI": ("– (automatisch, A4-Größe)", "DPI-Wert für Bilder ohne eingebettete DPI-Angabe beim Umwandeln nach PDF."),
 "PAPERLESS_OCR_MAX_IMAGE_PIXELS": ("– (Pillow-Standard)", "Pixelgrenze: darüber Warnung, ab der doppelten Grenze keine OCR des Bildes; `0` schaltet die Grenze ab (Schutz vor bösartigen Dateien)."),
 "PAPERLESS_OCR_COLOR_CONVERSION_STRATEGY": ("–", "Ghostscript-Farbumwandlung beim Erzeugen eines PDF/A-Archivs: `CMYK`, `Gray`, `LeaveColorUnchanged`, `RGB` oder `UseDeviceIndependentColor`."),
 "PAPERLESS_OCR_USER_ARGS": ("–", "Zusätzliche OCRmyPDF-Optionen als JSON (Unterstriche statt Bindestrichen). Nur für vertrauenswürdige Benutzer."),
 "PAPERLESS_TASK_WORKERS": ("`1`", "Anzahl paralleler Hintergrundaufgaben (Einlesen, Mail, Index, Klassifikator); Standard 1 wie paperless-ngx (ein Dokument bekommt alle Kerne). `auto`: Kerne ÷ 2 (aufgerundet; mit gesetztem `PAPERLESS_THREADS_PER_WORKER`: Kerne ÷ diesem Wert), höchstens 4, bei wenig freiem Speicher weniger – schneller bei großen Stapeln, langsamer für ein einzelnes Dokument."),
 "PAPERLESS_THREADS_PER_WORKER": ("`max(floor(CPU-Kerne / TASK_WORKERS), 1)`", "Seiten, die beim Einlesen eines Dokuments parallel per OCR erkannt werden. Produkt mit `PAPERLESS_TASK_WORKERS` ≤ Anzahl Kerne halten."),
 "PAPERLESS_WORKER_TIMEOUT": ("`1800` s", "Zeitlimit einer Aufgabe in Sekunden; auf schwacher Hardware erhöhen."),
 "PAPERLESS_TIME_ZONE": ("`UTC`", "Zeitzone der Installation (z. B. `Europe/Zurich`)."),
 "PAPERLESS_ENABLE_NLTK": ("–", "Seit v3.2 entfernt und ignoriert; Stoppwörter/Stammformen werden für die unterstützten Sprachen immer verwendet."),
 "PAPERLESS_CLASSIFIER_MATCH_THRESHOLD": ("`0.3`", "Mindest-Konfidenz (0.0–1.0), ab der der Klassifikator Korrespondent, Dokumenttyp oder Speicherpfad zuweist."),
 "PAPERLESS_MATCH_REGEX_TIMEOUT_SECONDS": ("`0.1` s", "Zeitlimit für reguläre Ausdrücke (Datumserkennung, Zuordnungsregeln)."),
 "PAPERLESS_DATE_PARSER_LANGUAGES": ("– (aus `PAPERLESS_OCR_LANGUAGE` abgeleitet)", "Sprachen für die Datumserkennung im Text (dateparser-Codes wie `en`, `de`, `en-AU`, mit `+` kombinierbar)."),
 "PAPERLESS_EMAIL_TASK_CRON": ("alle 10 Minuten (installationsabhängiger Versatz)", "Zeitplan des Mail-Abrufs (crontab); `disable` schaltet ihn ab."),
 "PAPERLESS_TRAIN_TASK_CRON": ("`5 */1 * * *` (stündlich)", "Zeitplan des Klassifikator-Trainings; `disable` schaltet es ab."),
 "PAPERLESS_INDEX_TASK_CRON": ("`0 0 * * *` (täglich um Mitternacht)", "Zeitplan der Suchindex-Pflege; `disable` schaltet sie ab."),
 "PAPERLESS_SEARCH_LANGUAGE": ("– (aus `PAPERLESS_OCR_LANGUAGE` abgeleitet)", "Sprache des Wortstamm-Filters der Volltextsuche; eine Änderung baut den Index beim nächsten Start neu auf."),
 "PAPERLESS_ADVANCED_FUZZY_SEARCH_THRESHOLD": ("– (aus)", "Schwelle für unscharfe Treffer in der Volltextsuche (z. B. `0.5`); unscharfe Treffer stehen hinter exakten."),
 "PAPERLESS_SANITY_TASK_CRON": ("`30 0 * * sun` (sonntags 00:30)", "Zeitplan der Konsistenzprüfung; `disable` schaltet sie ab."),
 "PAPERLESS_WORKFLOW_SCHEDULED_TASK_CRON": ("`5 */1 * * *` (stündlich)", "Zeitplan der Prüfung geplanter Workflows; `disable` schaltet sie ab."),
 "PAPERLESS_ENABLE_COMPRESSION": ("`true`", "Antworten des Webservers komprimieren (hinter nginx o. Ä. besser dort)."),
 "PAPERLESS_CONVERT_MEMORY_LIMIT": ("–", "Veraltet und wirkungslos (ImageMagick wird nicht mehr verwendet)."),
 "PAPERLESS_CONVERT_TMPDIR": ("–", "Veraltet und wirkungslos (ImageMagick wird nicht mehr verwendet)."),
 "PAPERLESS_APPS": ("–", "Zusätzliche Django-Apps für `INSTALLED_APPS` (kommagetrennt), z. B. für Social-Login-Anbieter. Mit Vorsicht verwenden."),
 "PAPERLESS_MAX_IMAGE_PIXELS": ("– (Pillow-Standard)", "Maximale Bildgröße, die Pillow ohne Warnung/Fehler lädt (Schutz vor bösartigen Dateien)."),
 "PAPERLESS_CONSUMER_DISABLE": ("–", "Wenn gesetzt, ist der Eingangsordner-Dienst im Docker-Container abgeschaltet."),
 "PAPERLESS_CONSUMER_DELETE_DUPLICATES": ("`false`", "Duplikate (gleiche Prüfsumme) beim Einlesen ablehnen und aus dem Eingangsordner löschen; sonst sind Duplikate erlaubt."),
 "PAPERLESS_CONSUMER_RECURSIVE": ("`false`", "Auch Unterordner des Eingangsordners einlesen."),
 "PAPERLESS_CONSUMER_SUBDIRS_AS_TAGS": ("`false`", "Namen der Unterordner als Tags vergeben (fehlende Tags werden angelegt); braucht `PAPERLESS_CONSUMER_RECURSIVE`."),
 "PAPERLESS_CONSUMER_IGNORE_PATTERNS": ("`[]` (nur eingebaute Muster)", "Zusätzliche reguläre Ausdrücke (JSON-Liste) für Dateinamen, die ignoriert werden."),
 "PAPERLESS_CONSUMER_IGNORE_DIRS": ("`[]` (nur eingebaute Namen)", "Zusätzliche Ordnernamen (JSON-Liste), die samt Inhalt übersprungen werden."),
 "PAPERLESS_PRE_CONSUME_SCRIPT": ("leer (nichts)", "Skript, das vor dem Einlesen läuft; Daten kommen über Umgebungsvariablen, siehe [Fortgeschrittenes](advanced.md#pre-consume-script)."),
 "PAPERLESS_POST_CONSUME_SCRIPT": ("leer (nichts)", "Skript, das nach dem Einlesen läuft, siehe [Fortgeschrittenes](advanced.md#post-consume-script)."),
 "PAPERLESS_FILENAME_DATE_ORDER": ("– (aus)", "Auch im Dateinamen nach einem Datum suchen, in dieser Reihenfolge (z. B. `YMD`)."),
 "PAPERLESS_NUMBER_OF_SUGGESTED_DATES": ("`3`", "Wie viele weitere im Dokument gefundene Daten vorgeschlagen werden; `0` schaltet das ab."),
 "PAPERLESS_THUMBNAIL_FONT_NAME": ("`/usr/share/fonts/liberation/LiberationSerif-Regular.ttf`", "Schrift für Vorschaubilder von Textdateien."),
 "PAPERLESS_IGNORE_DATES": ("leer", "Kommagetrennte Daten, die bei der Datumserkennung ignoriert werden (z. B. Geburtsdatum)."),
 "PAPERLESS_DATE_ORDER": ("`DMY`", "Erwartete Reihenfolge von Tag, Monat, Jahr im Dokumenttext."),
 "PAPERLESS_ENABLE_GPG_DECRYPTOR": ("`false`", "GPG-Entschlüsselung verschlüsselter Mails vor dem Einlesen."),
 "PAPERLESS_CONSUMER_POLLING_INTERVAL": ("`0`", "`0`: Dateisystem-Ereignisse; Zahl > 0: Eingangsordner alle N Sekunden abfragen (z. B. für Netzlaufwerke)."),
 "PAPERLESS_CONSUMER_STABILITY_DELAY": ("`5.0` s", "So lange muss eine Datei unverändert sein, bevor sie eingelesen wird."),
 "PAPERLESS_WEBHOOKS_ALLOWED_SCHEMES": ("`http,https`", "Erlaubte URL-Schemata für Webhooks."),
 "PAPERLESS_WEBHOOKS_ALLOWED_PORTS": ("leer (alle Ports)", "Erlaubte Ports für Webhooks, z. B. `80,443`."),
 "PAPERLESS_WEBHOOKS_ALLOW_INTERNAL_REQUESTS": ("`true`", "`false`: Webhooks an interne Adressen (z. B. localhost) sperren."),
 "PAPERLESS_EMAIL_ALLOW_INTERNAL_HOSTS": ("`true`", "`false`: Mailkonten sperren, deren IMAP-Host auf eine nicht öffentliche Adresse zeigt."),
 "PAPERLESS_OAUTH_CALLBACK_BASE_URL": ("– (dann `PAPERLESS_URL`)", "Basisadresse für den OAuth-Callback bei Gmail/Outlook; eine davon muss für OAuth gesetzt sein."),
 "PAPERLESS_GMAIL_OAUTH_CLIENT_ID": ("–", "OAuth-Client-ID für Gmail."),
 "PAPERLESS_GMAIL_OAUTH_CLIENT_SECRET": ("–", "OAuth-Client-Secret für Gmail."),
 "PAPERLESS_OUTLOOK_OAUTH_CLIENT_ID": ("–", "OAuth-Client-ID für Outlook."),
 "PAPERLESS_OUTLOOK_OAUTH_CLIENT_SECRET": ("–", "OAuth-Client-Secret für Outlook."),
 "PAPERLESS_EMAIL_GNUPG_HOME": ("– (GPG-Standard)", "`GNUPG_HOME` für die GPG-Entschlüsselung."),
 "PAPERLESS_CONSUMER_ENABLE_BARCODES": ("`false`", "Barcodes erkennen und Dateien an Trennseiten (z. B. PATCH-T) in mehrere Dokumente aufteilen."),
 "PAPERLESS_CONSUMER_BARCODE_TIFF_SUPPORT": ("`false`", "Auch TIFF-Dateien nach Barcodes durchsuchen (werden dafür nach PDF umgewandelt)."),
 "PAPERLESS_CONSUMER_BARCODE_STRING": ("`PATCHT`", "Text des Trenn-Barcodes."),
 "PAPERLESS_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES": ("`false`", "Trennseiten beim Aufteilen behalten."),
 "PAPERLESS_CONSUMER_ENABLE_ASN_BARCODE": ("`false`", "Archiv-Seriennummer (ASN) aus einem Barcode mit Präfix setzen, z. B. `ASN00123`."),
 "PAPERLESS_CONSUMER_ASN_BARCODE_PREFIX": ("`ASN`", "Präfix, an dem ein ASN-Barcode erkannt wird."),
 "PAPERLESS_CONSUMER_BARCODE_UPSCALE": ("`0.0`", "Vergrößerungsfaktor für die Barcode-Erkennung (wirkt ab > 1.0, hilft bei kleinen Barcodes)."),
 "PAPERLESS_CONSUMER_BARCODE_DPI": ("`300`", "Auflösung beim Rastern der Seiten für die Barcode-Erkennung."),
 "PAPERLESS_CONSUMER_BARCODE_MAX_PAGES": ("`0` (alle Seiten)", "Nur die ersten N Seiten nach Barcodes durchsuchen."),
 "PAPERLESS_CONSUMER_ENABLE_TAG_BARCODE": ("`false`", "Tags aus Barcodes zuweisen bzw. anlegen, wenn der Text einem der Muster entspricht; Kommas trennen mehrere Werte."),
 "PAPERLESS_CONSUMER_TAG_BARCODE_MAPPING": ("`{\"TAG:(.*)\": \"\\\\g<1>\"}`", "JSON-Zuordnung „regulärer Ausdruck → Ersetzung“ für Tag-Barcodes."),
 "PAPERLESS_CONSUMER_TAG_BARCODE_SPLIT": ("`false`", "Dokumente auch an Tag-Barcodes aufteilen (wie bei ASN-Barcodes)."),
 "PAPERLESS_CONSUMER_STORE_BARCODE_VALUES": ("`false`", "Inhalt aller beim Einlesen gefundenen Barcodes speichern (durchsuchbar)."),
 "PAPERLESS_AUDIT_LOG_ENABLED": ("`true`", "Änderungsprotokoll für Dokumente, Dokumenttypen, Korrespondenten und Tags."),
 "PAPERLESS_CONSUMER_ENABLE_COLLATE_DOUBLE_SIDED": ("`false`", "Zwei einseitige Scans (Vorder- und Rückseiten) automatisch zu einem doppelseitigen Dokument zusammenführen."),
 "PAPERLESS_CONSUMER_COLLATE_DOUBLE_SIDED_SUBDIR_NAME": ("`double-sided`", "Unterordner des Eingangsordners, in dem die Scans zum Zusammenführen erwartet werden (wird nicht automatisch angelegt)."),
 "PAPERLESS_CONSUMER_COLLATE_DOUBLE_SIDED_TIFF_SUPPORT": ("`false`", "Auch TIFF-Dateien zusammenführen (werden dafür nach PDF umgewandelt)."),
 "PAPERLESS_EMPTY_TRASH_DELAY": ("30 Tage (mindestens 1)", "So lange bleiben Dokumente im Papierkorb, bevor sie endgültig gelöscht werden."),
 "PAPERLESS_EMPTY_TRASH_TASK_CRON": ("`0 1 * * *` (täglich)", "Zeitplan für das Leeren abgelaufener Papierkorb-Einträge."),
 "PAPERLESS_SHARE_LINK_BUNDLE_CLEANUP_CRON": ("`0 2 * * *` (täglich 02:00)", "Zeitplan für das Entfernen abgelaufener Freigabe-Bündel samt ZIP; `disable` schaltet es ab."),
 "PAPERLESS_CONVERT_BINARY": ("–", "Veraltet und wirkungslos (kein ImageMagick mehr)."),
 "PAPERLESS_GS_BINARY": ("–", "Veraltet und wirkungslos (Ghostscript wird nicht über diesen Pfad aufgerufen)."),
 "PAPERLESS_WEBSERVER_WORKERS": ("`1`", "Anzahl Prozesse des upstream-Webservers; jeder lädt die ganze Anwendung in den Speicher."),
 "PAPERLESS_BIND_ADDR": ("`::` (alle Schnittstellen)", "IP-Adresse, auf der der upstream-Webserver im Container lauscht."),
 "PAPERLESS_PORT": ("`8000`", "Port des upstream-Webservers im Container."),
 "USERMAP_UID": ("`1000`", "Benutzer-ID im Container (wie `id -u` auf dem Host); bestimmt die Eigentümer der Ordner."),
 "USERMAP_GID": ("`1000`", "Gruppen-ID im Container (wie `id -g` auf dem Host)."),
 "PAPERLESS_OCR_LANGUAGES": ("– (keine zusätzlichen)", "Zusätzliche Tesseract-Sprachen, die das Container-Image beim Start installiert (mitgeliefert: Englisch, Deutsch, Italienisch, Spanisch, Französisch)."),
 "PAPERLESS_ENABLE_FLOWER": ("–", "Startet im Container das Celery-Überwachungswerkzeug Flower."),
 "PAPERLESS_SUPERVISORD_WORKING_DIR": ("–", "Veraltet und wirkungslos (für schreibgeschützte Dateisysteme siehe `S6_READ_ONLY_ROOT`)."),
 "PAPERLESS_APP_TITLE": ("– (Standardname)", "Überschreibt den angezeigten Namen der Anwendung. PaperRust zeigt „PaperRust“, wenn die Variable nicht gesetzt ist und Branding an ist."),
 "PAPERLESS_APP_LOGO": ("–", "Pfad zu einem Logo unter `/media/logo` (Name muss „logo“ enthalten), z. B. `/logo/Atari_logo.svg`; auf der Login-Seite für alle sichtbar."),
 "PAPERLESS_ENABLE_UPDATE_CHECK": ("–", "Veraltet zugunsten einer Einstellung in der Oberfläche; wird einmalig übernommen und ignoriert, sobald diese gesetzt ist."),
 "PAPERLESS_EMAIL_HOST": ("`localhost`", "SMTP-Server für ausgehende Mails (Passwort zurücksetzen, Workflow-Mails, Dokument per Mail)."),
 "PAPERLESS_EMAIL_PORT": ("`25`", "Port des SMTP-Servers."),
 "PAPERLESS_EMAIL_HOST_USER": ("`''`", "Benutzername am SMTP-Server."),
 "PAPERLESS_EMAIL_FROM": ("`PAPERLESS_EMAIL_HOST_USER`", "Absenderadresse."),
 "PAPERLESS_EMAIL_HOST_PASSWORD": ("`''`", "Passwort am SMTP-Server."),
 "PAPERLESS_EMAIL_USE_TLS": ("`false`", "STARTTLS verwenden."),
 "PAPERLESS_EMAIL_USE_SSL": ("`false`", "SSL/TLS von Anfang an verwenden."),
 "PAPERLESS_REMOTE_OCR_ENGINE": ("– (aus)", "Entfernter OCR-Dienst; derzeit nur `azureai` (Azure AI Document Intelligence)."),
 "PAPERLESS_REMOTE_OCR_API_KEY": ("–", "API-Schlüssel des entfernten OCR-Dienstes."),
 "PAPERLESS_REMOTE_OCR_ENDPOINT": ("–", "Endpunkt des entfernten OCR-Dienstes (für Azure Pflicht)."),
 "PAPERLESS_REMOTE_OCR_MODE": ("`always`", "`always`: alle unterstützten Dokumente gehen an den entfernten Dienst; `workflow_only`: nur, wenn ein Workflow (oder „Neu verarbeiten“) es anfordert."),
 "PAPERLESS_REMOTE_OCR_ALLOW_INTERNAL_ENDPOINTS": ("`true`", "`false`: Endpunkte mit nicht öffentlicher Adresse sperren."),
 "PAPERLESS_AI_ENABLED": ("`false`", "KI-Funktionen einschalten (u. a. KI-Vorschläge); Voraussetzung für alle KI-Funktionen."),
 "PAPERLESS_AI_LLM_EMBEDDING_BACKEND": ("–", "Embedding-Anbieter für RAG: `openai-like`, `huggingface` oder `ollama`."),
 "PAPERLESS_AI_LLM_EMBEDDING_MODEL": ("– (je Anbieter, z. B. `text-embedding-3-small`)", "Embedding-Modell; ohne Angabe gilt der Standard des Anbieters."),
 "PAPERLESS_AI_LLM_EMBEDDING_API_KEY": ("– (dann `PAPERLESS_AI_LLM_API_KEY`)", "API-Schlüssel für Embeddings."),
 "PAPERLESS_AI_LLM_EMBEDDING_ENDPOINT": ("– (dann `PAPERLESS_AI_LLM_ENDPOINT`)", "Endpunkt für Embeddings."),
 "PAPERLESS_AI_LLM_EMBEDDING_CHUNK_SIZE": ("`1024`", "Blockgröße beim Zerlegen des Dokumenttexts für Embeddings; verkleinern, wenn das Modell lange Eingaben ablehnt."),
 "PAPERLESS_AI_LLM_CONTEXT_SIZE": ("`8192`", "Kontextgröße für Prompts und RAG; bei Ollama auch als `num_ctx` gesendet."),
 "PAPERLESS_AI_LLM_REQUEST_TIMEOUT": ("`120` s", "Zeitlimit für Anfragen an den KI-Anbieter."),
 "PAPERLESS_AI_LLM_BACKEND": ("–", "KI-Anbieter: `openai-like` (OpenAI-kompatible API) oder `ollama` (lokal)."),
 "PAPERLESS_AI_LLM_MODEL": ("– (`gpt-3.5-turbo` bzw. `llama3.1`)", "Sprachmodell des Anbieters."),
 "PAPERLESS_AI_LLM_API_KEY": ("–", "API-Schlüssel des KI-Anbieters (für OpenAI-kompatibel meist nötig)."),
 "PAPERLESS_AI_LLM_ENDPOINT": ("–", "Endpunkt des KI-Anbieters (für Ollama Pflicht)."),
 "PAPERLESS_AI_LLM_OUTPUT_LANGUAGE": ("– (Sprache der Oberfläche)", "Sprache der KI-Vorschläge."),
 "PAPERLESS_AI_LLM_ALLOW_INTERNAL_ENDPOINTS": ("`true`", "`false`: KI-Endpunkte mit nicht öffentlicher Adresse sperren."),
 "PAPERLESS_AI_LLM_EXTRA_PARAMS": ("leer", "Zusätzliche Parameter (JSON-Objekt) für jede LLM-Anfrage; werden ungeprüft übergeben und überschreiben eigene Werte."),
 "PAPERLESS_LLM_INDEX_TASK_CRON": ("`10 2 * * *` (täglich)", "Zeitplan für die Aktualisierung des KI-Vektorindex (nur mit KI und Embedding-Anbieter)."),
}

STATUS_DE = {
    "read by PaperRust with the same meaning.": "liest PaperRust gleich",
}


def status_de(var, st):
    if st in STATUS_DE:
        return STATUS_DE[st]
    if st.startswith("not read by the Rust code"):
        return "nicht im Rust-Code gelesen (wirkt nur auf Django/Celery im Mischbetrieb)"
    if st.startswith("not needed: PaperRust extracts"):
        return "nicht nötig: PaperRust liest Office-/Mail-Text selbst (kein Tika, kein Gotenberg)"
    if st.startswith("not used: PaperRust is a single"):
        return "entfällt: PaperRust ist ein einzelner Prozess mit mehreren Threads (begrenzbar mit `TOKIO_WORKER_THREADS`)"
    if st.startswith("not used by PaperRust itself"):
        return "von PaperRust nicht genutzt: Adresse ist `PAPERRUST_BIND`; nur für den upstream-Webserver"
    if st.startswith("not applicable: PaperRust has no Celery"):
        return "entfällt: kein Celery, daher kein Flower"
    if st.startswith("PaperRust supports **only"):
        return "**nur `postgresql`**; SQLite/MariaDB vorher umziehen ([Umstieg](migration.md))"
    if st.startswith("not read by PaperRust (no cachalot"):
        return "nicht gelesen (kein Lese-Cache im Rust-Code); wirkt im Mischbetrieb auf Django. Nach einem DB-Umzug `invalidate_cachalot` ausführen"
    if st.startswith("read, but not carried out yet: PaperRust has no unpaper"):
        return "gelesen, aber noch nicht ausgeführt (kein unpaper); nur Warnung im Log"
    if st.startswith("read, but not carried out yet: no deskewing"):
        return "gelesen, aber noch nicht ausgeführt (kein Geraderichten); nur Warnung im Log"
    if st.startswith("read. Without Ghostscript"):
        return "gelesen; ohne Ghostscript setzt PDF/A nur die Metadaten, die Datei wird nicht neu geschrieben"
    if st.startswith("not implemented: encrypted mails"):
        return "nicht umgesetzt: verschlüsselte Mails werden unverändert verarbeitet"
    if st.startswith("not used: GPG decryption"):
        return "entfällt (GPG-Entschlüsselung nicht umgesetzt)"
    if st == "":
        if var == "PAPERLESS_DB_POOLSIZE":
            return "upstream entfernt; PaperRust liest den Wert weiterhin als Poolgröße (sonst `PAPERRUST_DB_MAX_CONNECTIONS`)"
        if var.startswith("USERMAP_"):
            return "nur upstream-Container-Image"
        return "entfällt (upstream entfernt)"
    raise SystemExit(f"unknown status for {var}: {st}")


def esc(s):
    return s.replace("|", "\\|")


def main():
    data = json.load(open(sys.argv[1], encoding="utf-8"))
    header = open(sys.argv[2], encoding="utf-8").read().rstrip() + "\n\n"
    out = [header]
    rows_open = False
    count = 0

    def close():
        nonlocal rows_open
        if rows_open:
            out.append("")
            rows_open = False

    for o in data:
        if o["type"] == "sec":
            close()
            title = o["title"]
            de = SEC[title]
            sl = re.sub(r"[^\w\- ]", "", title.lower()).strip().replace(" ", "-")
            idl = list(o["ids"]) + ([sl] if sl not in o["ids"] else [])
            ids = "".join(f'<a id="{i}"></a>' for i in idl)
            if ids:
                out.append(ids + "\n")
            out.append(("#" * o["lvl"]) + " " + de + "\n")
            if title in NOTES:
                kind, txt = NOTES[title]
                out.append(f"> [!{kind}]\n> {txt}\n")
            continue
        head = o["head"]
        m = re.search(r"(PAPERLESS_[A-Z0-9_]+|USERMAP_[A-Z]+)", head)
        var = m.group(1)
        if var not in D:
            raise SystemExit("missing " + var)
        std, desc = D[var]
        form = re.search(r"`([^`]+)`", head).group(1)
        if head.startswith("~~"):
            form = f"~~{form}~~"
        else:
            form = f"`{form}`"
        anchors = "".join(f'<a id="{i}"></a>' for i in o["ids"])
        if not rows_open:
            out.append("| Variable | Standard | Bedeutung | PaperRust | Details |")
            out.append("|---|---|---|---|---|")
            rows_open = True
        low = var.lower()
        out.append(f"| {anchors}{esc(form)} | {esc(std or '–')} | {esc(desc)} | {esc(status_de(var, o['status']))} | [EN](en/configuration.md#{low}) |")
        count += 1
    close()
    open(sys.argv[3], "w", encoding="utf-8", newline="\n").write("\n".join(out).rstrip() + "\n")
    print("rows", count)


main()
