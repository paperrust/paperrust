# Fehlerbehebung

[English](en/troubleshooting.md) · [Übersicht](README.md)

Erste Anlaufstelle ist immer das Protokoll von PaperRust (stderr bzw. Dienst-Log; Ausführlichkeit
über `PAPERRUST_LOG`, z. B. `PAPERRUST_LOG=debug`) und die Seite *Einstellungen → Systemstatus* bzw.
*Aufgaben* in der Oberfläche.

<a id="start"></a>

## PaperRust startet nicht

- **Ungültige Einstellung:** PaperRust bricht wie Django mit einer Meldung ab (z. B. ungültiger
  Wahrheitswert, ungültige `PAPERRUST_BIND`-Adresse, unbekannte Routen-id in `routes.toml`). Die
  Meldung nennt die Variable bzw. Zeile.
- **`routes.toml` fehlt:** Wurde `PAPERRUST_ROUTES` gesetzt und die Datei fehlt → Abbruch. Ohne die
  Variable und ohne Datei sind alle Routen an (die Datei dient nur zum Abschalten einzelner Routen).
- **„This database is not at paperless-ngx v3.4.2“** (von `paperrust manage migrate`): Die
  Datenbank stammt von einer Version, die `manage migrate` nicht selbst nachholt – etwa paperless-ngx
  2.x oder eine neuere Version mit unbekannten Änderungen. Weg: einmal `paperrust migrate --from …`
  ([Von paperless-ngx umsteigen](umstieg.md)); ältere 3.x-Datenbanken holt `manage migrate` selbst nach.
- **Port belegt:** andere Adresse in `PAPERRUST_BIND`.

<a id="web-ui"></a>

## Oberfläche leer, „Loading…“ oder 404 auf `/`

- Die gebaute Oberfläche fehlt: `frontend-light/dist` bzw. `PAPERRUST_LIGHT_UI_DIR` muss
  `index.html` und `m.html` enthalten. Fehlt sie, zeigt `/rs/app/` eine Fehlerseite (503). Siehe
  [Installation → Statische Dateien](installation.md#static-files).
- Anmeldeseite ohne Aussehen: `PAPERRUST_STATICDIR` zeigt nicht auf die statischen Dateien
  (`scripts/build-static.sh`).
- Steht eine Route in `routes.toml` auf `"off"`, antwortet sie mit 404.
- Neue statische Dateien werden erst nach einem Neustart erkannt.

<a id="consumer"></a><a id="no-files-are-added-by-the-consumer"></a>

## Dateien im Eingangsordner werden nicht eingelesen

- Läuft der Worker? Nur `paperrust serve --worker` (oder `--worker-only`) überwacht den Ordner.
- `PAPERRUST_CONSUMPTION_DIR` richtig und für den PaperRust-Benutzer les- und schreibbar? Fehlt der
  Ordner, steht ein Fehler im Log, der Server läuft weiter.
- Mit `PAPERRUST_CONSUMER_POLLING_INTERVAL=0` (Standard) meldet das Betriebssystem neue Dateien;
  klappt das nicht (z. B. Netzlaufwerk), sieht PaperRust jede Sekunde nach. Werte über 0 = Abfrage
  in diesem Abstand. Dateien werden erst nach `PAPERRUST_CONSUMER_STABILITY_DELAY` übernommen. Ignorierte Muster: `PAPERRUST_CONSUMER_IGNORE_PATTERNS`.

<a id="upload"></a>

## Hochladen in der Oberfläche schlägt fehl

- Läuft ein Worker (`serve --worker` oder ein eigener `--worker-only`-Prozess)? Ohne Worker bleibt
  die Aufgabe in der Warteschlange stehen.
- Steht `"documents.post_document" = "off"` in `routes.toml`? Dann antwortet die Route mit 404.

<a id="ocr"></a>

## Kein Text erkannt / „OCR failed“

- Tesseract gefunden? `PAPERRUST_TESSERACT_PATH`, Sprachdaten in `PAPERRUST_TESSDATA_DIR`, Sprachen
  in `PAPERRUST_OCR_LANGUAGE` (z. B. `deu+eng`) müssen als `*.traineddata` vorhanden sein.
  Tesseract wird erst geprüft, wenn eine Seite wirklich Texterkennung braucht.
- PDFium gefunden? `PAPERRUST_PDFIUM_PATH`. Ohne PDFium gibt es keine Vorschaubilder von PDFs, keine
  Barcodes und keine Texterkennung gescannter PDFs.
- HEIC-Bilder (iPhone) dekodiert das Hilfsprogramm `paperrust-pdftool`; fehlt es, scheitert das
  Einlesen mit „HEIC decoding is not supported“. Fotos mit 4:2:2/4:4:4-Farbunterabtastung (manche
  Canon-/Sony-Kameras) und HEIC-Bildfolgen werden nicht gelesen.
- Säubern (`PAPERRUST_OCR_CLEAN`) und Geraderichten (`PAPERRUST_OCR_DESKEW`) erledigt PaperRust
  selbst; `clean-final` und `deskew` ersetzen auch das Seitenbild im Archiv-PDF.

<a id="office"></a>

## Office-Dokument ohne Vorschau-PDF

PaperRust wandelt Office-Dateien (auch .doc/.xls/.ppt, Tabellen, Präsentationen) und Mails mit dem
Hilfsprogramm `paperrust-pdftool` in ein Archiv-PDF um (im Docker-Image enthalten,
`PAPERRUST_OFFICE_TO_PDF=true` als Standard). Nur wenn das Programm fehlt, abgeschaltet ist oder die
Umwandlung scheitert (Log), bleibt es beim Original: es wird gespeichert und durchsucht, das
Vorschaubild zeigt den extrahierten Text.

<a id="classifier"></a><a id="classifier-error-no-training-data-available"></a>

## „No automatic matching items, not training“

Kein Tag, Korrespondent, Dokumenttyp oder Speicherpfad hat die Zuordnung *Automatisch*. Mindestens
einem Objekt den Algorithmus „Automatisch“ geben und ein paar Dokumente zuordnen; danach
`paperrust manage document_create_classifier`.

<a id="search"></a>

## Suche findet nichts

Passt der Suchindex nicht (z. B. nach einem Wechsel von `PAPERRUST_SEARCH_LANGUAGE` oder nach einem
Update), baut ihn der Worker beim Start neu auf (`serve --worker`). Ohne Worker sucht PaperRust leer,
bis neu indexiert wurde: `paperrust manage document_index reindex`.

<a id="redis"></a>

## Meldungen über Redis im Log

PaperRust verbindet sich nie mit Redis. Steht noch `PAPERLESS_REDIS` in der Konfiguration (aus
paperless-ngx übernommen), meldet das Log nur einen Hinweis: Redis kann abgeschaltet und die
Variable entfernt werden. Auf der Systemstatus-Seite beschreiben die Felder „Redis“ den Cache
von PaperRust (prozesslokal oder `PAPERRUST_CACHE=postgres`), nicht einen Redis-Dienst.

<a id="missing"></a>

## Eine Funktion antwortet mit 404

Siehe [Was ohne Django fehlt](installation.md#standalone-limits) (nur noch `/admin/` und die allauth-„headless“-API). Die Startmeldung listet die
fehlenden Pfade. Eine Weiterleitung an Django als Ausweg gibt es nicht mehr.

<a id="mail"></a>

## Mail-Abruf

- Zeitplan `PAPERRUST_EMAIL_TASK_CRON`; sofort: `paperrust manage mail_fetcher`.
- Fehler je Nachricht unter *Mail → Verarbeitete Mails*.
- Jeder IMAP-Schritt hat ein Zeitlimit von 60 s.
- OAuth (Gmail/Outlook) geht ohne Django; geprüft bisher nur mit Testwerten, nicht mit echten Konten ([Mail](mail.md)).

<a id="collation"></a><a id="postgresql-warns-about-a-collation-version-mismatch"></a>

## PostgreSQL warnt vor „collation version mismatch“

Wie bei paperless-ngx: nach einem Betriebssystem-/ICU-Update `REINDEX DATABASE paperless;` und
`ALTER DATABASE paperless REFRESH COLLATION VERSION;` (als Eigentümer bzw. Superuser).

<a id="report"></a>

## Fehler melden

Bitte mit Version, Betriebsart (Server/Docker/Desktop), Protokollauszug und Schritten zum
Nachstellen im Issue-Tracker von PaperRust – nicht bei paperless-ngx. Sicherheitslücken vertraulich:
[SECURITY.md](../../SECURITY.md).
