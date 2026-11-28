# PaperRust 0.1.0 – Release-Notizen

[English](RELEASE-NOTES-0.1.0.en.md) · Stand 2026-11-20 (Entwurf, Freigabe nach
[docs/RELEASE-GATE.md](RELEASE-GATE.md) steht aus)

Die erste Version von PaperRust. Diese Seite sagt, was PaperRust ist, was es kann, was neu ist
gegenüber paperless-ngx – und ehrlich, was (noch) nicht geht.

> **Bitte gesund skeptisch sein.** PaperRust 0.1.0 ist eine erste Version. Der Kern ist gründlich geprüft –
> aber nicht mit Ihren Dokumenten. Vor dem Umstieg: eine **vollständige Sicherung** von paperless-ngx machen
> (Export oder Datenbank und Medienordner), PaperRust zuerst **mit einer Kopie ausprobieren** und paperless-ngx
> erst abschalten, wenn alles passt. Nichts blind übernehmen. Der Weg zurück zu paperless-ngx bleibt offen.

## Was ist PaperRust?

PaperRust ist ein Archiv für Ihre Dokumente: Papier einscannen oder Dateien ablegen, PaperRust
erkennt den Text, ordnet Absender, Dokumenttyp und Schlagworte zu und macht alles durchsuchbar.

PaperRust ist aus [paperless-ngx](https://github.com/paperless-ngx/paperless-ngx) 3.4.2 hervorgegangen
und neu in Rust geschrieben. Es versteht dieselbe Datenbank, dieselben Dateien und dieselbe
REST-API. Es ist aber **ein einziges Programm** plus PostgreSQL – ohne Python, Redis, Celery, Tika und
Gotenberg. PaperRust ist ein unabhängiges Projekt, nicht mit dem paperless-ngx-Team verbunden, und
steht wie paperless-ngx unter der GPL-3.0.

## Hauptfunktionen

- **Einlesen** aus Eingangsordner, Hochladen (auch per Ziehen), E-Mail-Postfächern, Scanner
  (Scannen-Knopf über AirScan/eSCL, „Scan an E-Mail“ vom Drucker) und REST-API.
- **Dateitypen:** PDF, Bilder (PNG, JPEG, TIFF, GIF, WebP, **HEIC** vom iPhone), Text, Office (DOCX,
  XLSX, PPTX, ODT/ODS/ODP/ODG, RTF, auch die alten **DOC/XLS/PPT**), E-Mails (EML) – Office und Mails
  bekommen ein **Archiv-PDF** (Mail-PDF mit wählbarem Layout).
- **Texterkennung** mit Tesseract, mit eingebautem **Säubern** und **Geraderichten** schiefer Scans,
  Archiv als **PDF/A**, Verkleinern der Archivdatei, wahlweise Windows-Texterkennung oder
  Azure-Cloud-Erkennung.
- **Ordnen:** Korrespondenten, Dokumenttypen, verschachtelte Tags, Speicherpfade (mit Baukasten),
  eigene Felder, ASN-Nummern (auch mehrere Nummernkreise), gespeicherte Ansichten; automatische
  Zuordnung, die aus Ihren Entscheidungen lernt.
- **Suchen:** Volltext mit Feld-, Datums- und Platzhaltersuche, ähnliche Dokumente, globale Suche.
- **Teilen:** Freigabelinks (auch für mehrere Dokumente), Versand per E-Mail, Rechte je Dokument.
- **Dateiversionen** eines Dokuments, Papierkorb, Änderungsverlauf.
- **Mehrere Benutzer** mit Rollen, Gruppen, Zwei-Faktor-Anmeldung, Anmeldung über OpenID Connect,
  GitHub, GitLab, Google, Microsoft, Nextcloud, Discord, Gitea, E-Mail-Bestätigung.
- **Desktop-App für Windows und Linux** (x86_64) mit allem dabei (PostgreSQL, Tesseract, PDFium) –
  unter Windows auch als Fenster auf einen Server, mit Offline-Kopie ausgewählter Dokumente.
  macOS ist geplant.
- **Oberfläche in 10 Sprachen** (de, en, fr, it, es, nl, pt-BR, pl, cs, sv), Handbuch eingebaut
  (`/rs/docs/`, auch offline).

## Neu gegenüber paperless-ngx

- **Ein Programm statt fünf Dienste:** `paperrust serve --worker` ersetzt Webserver, Celery-Worker,
  Zeitplaner und Eingangsordner; Warteschlange und Zwischenspeicher liegen in PostgreSQL. Ein
  Docker-Image mit 0,65 GB statt vier Images mit 6,5 GB.
- **Eigene Oberfläche (PaperRust-Design):** Arbeitsplatz mit Liste, Vorschau und Angaben
  nebeneinander, Posteingang mit „Erledigt“, Startseite aus Bausteinen, Zeitstrahl mit Fristen,
  Befehlsleiste (Strg + K), die auch **Einstellungen findet**, Tags und Korrespondenten **direkt im
  Feld anlegen**, **Rundgang** in zwölf Schritten, Hell/Dunkel. Die klassische Oberfläche im Stil von
  paperless-ngx bleibt wählbar.
- **Automatik:** Workflows als lesbarer Satz, Karten „Wenn – Nur wenn – Dann – Sonst“, Vorlagen,
  Vorschläge aus dem eigenen Archiv, **Testlauf** mit Vorher/Nachher vor dem Anwenden, **Verlauf** je
  Workflow, **Sonst-Zweig**, Auslöser „Von Hand“ und „E-Mail kommt an“
  ([Automatik](user/automatik.md)).
- **KI eingebaut** (aus, bis Sie sie einschalten; lokal mit Ollama oder mit OpenAI-kompatiblen
  Diensten): Vorschläge mit **Prüfliste** (je Feld übernehmen, ändern, verwerfen, rückgängig),
  Benutzerfelder ausfüllen, **KI-Texterkennung** mit Bildmodell, eigene Prompts, Altbestand
  nachbearbeiten, KI-Bedingung in Workflows, Workflow-Entwurf aus einer Beschreibung, Chat mit
  Quellen, **MCP-Server** für externe Agenten. Ersetzt paperless-ai und paperless-gpt
  ([KI](user/ai.md#replaces)).
- **PDF-Werkzeuge:** echtes **Schwärzen** (der Inhalt wird entfernt, nicht überdeckt, mit Prüfung),
  **Verkleinern**, **In PDF umwandeln**, Reparieren kaputter PDFs beim Einlesen – abgeschottet in
  einem eigenen Programm mit Zeit- und Speichergrenze ([PDF-Werkzeuge](user/pdf-tools.md)). Beim
  **Teilen** zeigt PaperRust, welche Version geschwärzt ist, und wählt sie für neue Links und Pakete vor.
- **Schutz vor Ransomware:** Wächter, der bei verdächtigen Änderungen auf Nur-Lesen schaltet;
  verschlüsselte Sicherungen mit Versionen, auch auf S3 mit Object Lock; geplante
  Integritätsprüfung aller Dateien ([Ransomware](user/ransomware.md)).
- **Einstellungen in der Oberfläche** statt nur Umgebungsvariablen, mit Herkunftsanzeige, Verlauf,
  Export/Import und Umzug aus der `docker-compose.env`.
- **Rollen** (Nur lesen, Bearbeiten, Verwalten, Administrator oder eigene) und „Sichtbar für“ je
  Dokument.
- **Scanner-Anbindung** ohne NAS-Passwort am Gerät, **ASN-Etiketten** drucken, Stapel per
  Trennblatt aufteilen.
- **Desktop-App für Windows ohne Anmeldung:** lokal meldet die App ihren Eigentümer automatisch an
  (Schlüssel je Start, nur für dieses Windows-Konto lesbar); Zugriff von anderen Geräten bleibt mit
  Passwort ([Installation](user/installation.md#desktop-login)).
- **Neue Verwaltungsbefehle:** `settings`, `backup`, `guard`, `datamodel`, `sync`, `timeline_backfill`,
  dazu alle bekannten (`document_exporter`, `document_archiver`, `document_fuzzy_match`,
  `document_consumer`, `check` …) – [Administration](user/administration.md).

<a id="leistung"></a>

## Leistung

Gemessen am 20.11.2026 auf einem **kleinen arm64-Rechner** (6 Kerne, 8 GB RAM, NVMe) in
Docker, je Aufstellung **ein Lauf**, mit **255 erfundenen Dokumenten**. Verglichen wurde PaperRust
(Freigabestand `e546b8f`, Standard-Einstellungen) mit paperless-ngx 3.4.2 in der sparsamsten
Aufstellung (Valkey, ohne Tika/Gotenberg, Standard-Einstellungen), beide mit PostgreSQL 17 und je
4 Kernen. Beide säubern und begradigen Scans – also gleiche Arbeit. Bericht:
[results/benchmark/2026-11-20-arm64/bericht.md](../results/benchmark/2026-11-20-arm64/bericht.md),
alle Tabellen: [tabellen.md](../results/benchmark/2026-11-20-arm64/tabellen.md).

| | paperless-ngx | PaperRust |
|---|---:|---:|
| Erststart bis Anmeldung möglich | 53,2 s | 3,3 s |
| Neustart bis die Dokumentliste antwortet | 33,7 s | 0,57 s |
| Liste/Suche, während 230 Dokumente eingelesen werden (Median / 95 %) | 150 / 187 ms | 10 / 32 ms |
| 10 Personen gleichzeitig (Median / 95 %) ¹ | 77 / 255 ms | 4 / 16 ms |
| 230 Dokumente einlesen (200 Text-PDFs, 30 Scans) | 29 min 56 s | 1 min 46 s |
| Ein Scan (1 Seite) bis durchsuchbar | 25,8 s | 8,1 s |
| Ein Handyfoto (JPG) bis durchsuchbar | 32,3 s | 9,4 s |
| Arbeitsspeicher frisch gestartet (Dienste + Datenbank) | 598 + 41 MiB | 74 + 64 MiB |
| Arbeitsspeicher im Leerlauf nach dem Einlesen (Dienste + Datenbank) | 702 + 49 MiB | 420 + 74 MiB |
| Großes Archiv (2 230 Dok.), 8 Anfragen gleichzeitig: Anfragen/s ² | 7,3 | 360 |
| Docker-Images ohne PostgreSQL ² | 3,47 GB (2 Container) | 0,65 GB (1 Container) |

¹ Am 20.11. ohne Hochladen und verkürzt gemessen (im Messprogramm fehlte der Ordner mit den
Upload-Dateien; gilt für beide Systeme gleich). Mit Hochladen am 19.11. (Stand `5f5de7c`): 87 / 307 ms
gegen 5 / 18 ms. ² Messung vom 18.11.2026 (Stand `c97f15c`); das große Archiv gegen paperless-ngx
vollständig (mit Tika und Gotenberg), nicht wiederholt.

Bedingungen und Einschränkungen:

- paperless-ngx mit Tika und Gotenberg (kann dann Word-Dateien), aber ohne Säubern/Geraderichten,
  braucht für den Stapel 25 Minuten (1 493 s) und für einen Scan 19,9 s; dabei belegt es nach dem
  Einlesen 1 317 + 49 MiB.
- **Wo PaperRust schlechter ist:** Die Datenbank braucht mehr Arbeitsspeicher (frisch 64 statt
  41 MiB, nach dem Einlesen 74 statt 49 MiB, Spitze beim Einlesen 115 statt 52 MiB). Auf der Platte
  belegt die Datenbank 11 % mehr (97,8 statt 88,2 MiB, logisch 28,4 statt 18,2 MB), die Dateien 6 %
  mehr (54,7 statt 51,4 MiB); beim großen Archiv (18.11.) war die Datenbank doppelt so groß.
  Die Spitze beim Einlesen liegt mit 800 MiB (Dienste) nur knapp unter paperless-ngx (838 MiB).
- Nur ein Gerät, saubere erfundene Dokumente, nur API-Messung (keine Darstellungszeit im Browser).
  Unterschiede unter etwa 10 % nicht überbewerten. 10 000+ Dokumente nicht gemessen.

**Prüfungen vor der Freigabe** (Stand 2026-11-20): 24-Stunden-Dauerlauf unter gemischter Last ohne
Server-Fehler, Speicher stabil ([Bericht](../results/release/soak/2026-11-19-24h/BERICHT.md));
unabhängige Sicherheitsprüfung des Codes, alle Funde behoben
([Bericht](../results/release/review/security-2026-11-19.md));
echter Installationstest der Desktop-App (Update, Deinstallation) und Docker-Rauchtest amd64.

<a id="umstieg"></a>

## Umstieg von paperless-ngx

- **Ein Befehl:** `paperrust migrate --from <paperless-Ordner | Datenbank-Adresse | Export>` – oder
  in der Desktop-App „Von paperless-ngx umsteigen“. PaperRust prüft vorher alles, zeigt den
  Fortschritt und am Ende einen **Abgleich** (Dokumente, Tags, Benutzer, Dateien).
- **Versionen:** paperless-ngx 2.20 (und 2.19.6), 3.0 bis 3.4.2 und die Entwicklerversion nach
  3.4.2; Datenbank PostgreSQL direkt, SQLite und MariaDB werden nach PostgreSQL kopiert.
- **Was mitkommt:** Dokumente mit Dateien, Tags, Korrespondenten, Typen, Speicherpfade, Felder,
  Notizen, Ansichten, Workflows, Mail-Regeln, Benutzer **mit Passwörtern** und Rechte.
- **Sicherung:** Wird die bestehende Datenbank umgestellt, sichert PaperRust sie vorher selbst.
- **Einbahnstraße:** Danach kann paperless-ngx dieselbe Datenbank **nicht mehr öffnen**. Zurück geht
  es nur über die Sicherung oder über einen Export aus PaperRust (Format von paperless-ngx 3.4.2).
  Wer unsicher ist, steigt in eine **Kopie** um (`--target`), dann bleibt paperless-ngx unberührt.
- Getestet mit Testdatenbanken 2.20.15, 3.1.3, 3.4.2 und einer echten 3.1.3-Datenbank (ohne
  Prüfung der Dateien).

Schritt für Schritt: [Von paperless-ngx umsteigen](user/umstieg.md).

<a id="grenzen"></a>

## Bekannte Grenzen

Ehrlich gesagt – das geht (noch) nicht oder anders als bei paperless-ngx:

**Installation und Betrieb**

- Der **Windows-Installer ist nicht digital signiert**; Windows SmartScreen warnt. Prüfsumme
  vergleichen. Eine Code-Signatur über die SignPath Foundation ist geplant.
- Das **Linux-Paket** (`tar.gz`, nur x86_64) ist neu: auf frischem Ubuntu 24.04 und Debian 12 geprüft
  (Start, Anmeldung, Texterkennung), aber noch wenig im Alltag erprobt. Es braucht glibc 2.36 oder neuer und darf nicht als root laufen; ohne WebKitGTK 4.1 und libayatana-appindicator
  öffnet es sich im Browser statt in einem eigenen Fenster.
- Für **macOS** gibt es noch keine Desktop-App (geplant; braucht einen Mac und ein Apple-Entwicklerkonto).
- Das **Docker-Image** (`ghcr.io/paperrust/paperrust`, amd64 und arm64) ist für arm64 gemessen und für
  amd64 im Rauchtest geprüft. Fertige Server-Pakete (deb/rpm) gibt es nicht.
- **Nicht eigens geprüft:** Safari (macOS/iOS), Firefox, Android-Browser und Screenreader (Kontraste und
  Tastatur sind geprüft). Die **NAS-Vorlagen** (Synology, UGREEN, Asustor, Unraid, CasaOS) wurden nicht auf
  einem echten NAS gestartet. Rückmeldungen dazu sind sehr willkommen.
- **Nur PostgreSQL** (ab 14). SQLite/MariaDB werden beim Umstieg kopiert; danach können Sortierung und
  Groß-/Kleinschreibung bei Umlauten leicht anders sein.
- Für volle Funktion braucht es **Tesseract**, **PDFium** und **`paperrust-pdftool`** (im Image und in
  der Desktop-App enthalten). Fehlen sie, laufen manche Funktionen nur mit Hinweis im Log weiter.
  `gpg` (verschlüsselte Mails) und `pdftoppm` (Barcodes ohne PDFium) sind nicht im Image.
- Mehrere Prozesse (Webserver und Worker getrennt) brauchen `PAPERRUST_CACHE=postgres`.
- Ohne `--worker` laufen keine Zeitpläne, kein Mail-Abruf und keine Ordnerüberwachung.
- Kein `/admin/` (Django-Verwaltung) und keine allauth-„headless“-API.
- **`PAPERLESS_URL` setzen**, wenn PaperRust aus dem Netz erreichbar ist: ohne sie stammen Links in
  Passwort-Mails aus der Anfrage (wie bei paperless-ngx). Hinter HTTPS-Proxy die Cookie-/Proxy-
  Einstellungen aus der Betriebsanleitung setzen.
- Ein Export aus **paperless-ngx 1.x** lässt sich nicht übernehmen (auch nicht in paperless-ngx 3):
  erst auf 2.20 aktualisieren.
- In der Desktop-App gibt es lokal kein „Abmelden“ (automatische Anmeldung).

**Einlesen, Texterkennung, Dateien**

- **PDF/A** wird erzeugt und selbst geprüft, aber nicht mit veraPDF bestätigt. Bettet das Original
  Schriften nicht ein, entsteht ein normales PDF (Hinweis im Log).
- Einige **OCRmyPDF-Optionen** aus `OCR_USER_ARGS` wirken nicht (z. B. `unpaper_args`,
  `remove_background`, `fast_web_view`); es gibt eine Warnung.
- Säubern und Geraderichten sind eigene Umsetzungen: ähnliches Ergebnis, aber nicht pixelgleich.
- **Office-PDFs** sehen anders aus als mit LibreOffice/Gotenberg: Präsentationen nur als Textseiten
  (ohne Bilder und Formen), Tabellen ohne Farben und Diagramme. Office-Metadaten sind spärlicher als
  bei Tika.
- **HEIC:** keine Fotos mit 4:2:2/4:4:4-Farbunterabtastung und keine Bildfolgen.
- **TIFF → PDF** wird immer neu komprimiert; die Dateien sind größer als bei paperless-ngx.
- **Vorschaubilder** sind etwa 1,6-mal so groß wie bei paperless-ngx.
- **Barcodes** liest PaperRust mit eigenen Bausteinen; bei verzerrten oder unscharfen Scans kann
  das Ergebnis abweichen.
- Ein gescheiterter Upload aus der Oberfläche wird gelöscht und muss neu hochgeladen werden.

**Suchen, Ordnen, Workflows**

- Die **automatische Zuordnung** hat ein eigenes Format: Sie lernt nach dem Umstieg neu, Vorschläge
  können bei einzelnen Dokumenten abweichen.
- **Reguläre Ausdrücke:** Sonderformen von Pythons `regex` (z. B. `{e<=1}`, Rekursion) gehen nicht.
- **Relative Daten** in Feld-Abfragen nur in den 10 Oberflächensprachen; einige Jinja-Feinheiten in
  Vorlagen fehlen.
- Der **Sonst-Zweig** von Workflows ist eine PaperRust-Erweiterung: paperless-ngx und der Export
  kennen ihn noch nicht (im Export landen Sonst-Aktionen derzeit als lose Aktionen).
- Der **Testlauf** prüft bei „Nach Zeitplan“ nur die Bedingungen und stellt „Einlesen“ nur nach.
- Was die **KI** automatisch ändert, steht im KI-Protokoll (mit Rückgängig), nicht im normalen
  Änderungsverlauf des Dokuments.

**Mail, Anmeldung, API**

- **Social Login** nur mit OpenID Connect, GitHub, GitLab, Google, Microsoft, Nextcloud, Discord, Gitea; andere
  allauth-Anbieter fehlen. Gegen die echten Anbieter nicht getestet.
- **Gmail/Outlook per OAuth** nur mit Testwerten geprüft; erprobt ist das Konto mit (App-)Passwort.
- Ohne funktionierenden Mailserver meldet „Passwort vergessen“ trotzdem „gesendet“ (Fehler im Log).
- **Fehlermeldungen** von API und Anmeldeseiten kommen nur auf Englisch.
- Die Oberfläche hat **10 Sprachen** statt über 30; andere Sprachen sehen Englisch.
- Die REST-API nimmt auf den meisten Adressen nur JSON (keine Formulare, keine „Browsable API“).
- **HuggingFace-Embeddings** nur über einen eigenen Dienst (text-embeddings-inference) oder die
  HuggingFace-API, nicht im Programm selbst.

**Nicht gegen echte Gegenstellen geprüft:** Azure-Texterkennung, S3-Sicherung, Scanner-Hardware,
Social-Login-Anbieter, OAuth-Mail, TLS bei IMAP/SMTP, GPG mit echtem Schlüsselbund, KI-Texterkennung
mit echtem Bildmodell. Alles ist mit Tests und nachgestellten Antworten abgedeckt.

Die vollständigen Abweichungen je Baustein stehen in `docs/contract/*.md` (Abschnitt „Known
differences“), offene Lücken gegenüber paperless-ngx in [docs/LUECKEN-NGX.md](LUECKEN-NGX.md).

<a id="anforderungen"></a>

## Systemanforderungen

**Server (Docker oder direkt)**

- Linux auf **x86-64 (amd64)** oder **ARM 64 Bit (arm64)**, z. B. Raspberry Pi 4/5 mit 64-Bit-System,
  ARM-NAS. 32-Bit-ARM wird nicht unterstützt. Windows geht mit selbst gebautem Programm.
- **PostgreSQL 14 oder neuer** (geprüft mit 17 und 18).
- **Arbeitsspeicher:** Empfehlung ab 2 GB für das ganze Gerät bei einem kleinen Archiv (gemessen:
  494 MiB für PaperRust und Datenbank im Leerlauf nach dem Einlesen, Spitze beim Einlesen
  915 MiB, siehe [Leistung](#leistung)).
- **Platte:** Platz für Ihre Dokumente plus rund 30–40 % für Archiv-PDFs, Vorschaubilder, Suchindex
  und Datenbank; Image 0,65 GB plus PostgreSQL 0,69 GB.
- Für Texterkennung: Tesseract 5 mit Sprachdaten, PDFium (beides im Docker-Image).

**Desktop-App**

- **Windows 10 oder 11, 64 Bit** (x86-64). Installer rund 68 MB, bringt PostgreSQL 17, Tesseract 5
  (Deutsch, Englisch) und PDFium mit. Unter Windows 10 kann das Microsoft-WebView2 nötig sein.
- **Linux x86_64** mit glibc 2.36 oder neuer (Debian 12, Ubuntu 24.04, Fedora 37 oder neuer), als
  `tar.gz` mit PostgreSQL 17, Tesseract 5 (Deutsch, Englisch) und PDFium. Für ein eigenes Fenster
  WebKitGTK 4.1 und libayatana-appindicator, sonst Browser. Nicht als root starten.
- Kein macOS-Paket (geplant).

**Browser:** ein aktueller Browser. Automatisch getestet wird mit Chromium (Chrome, Edge); Firefox und
Safari sollten gehen, sind aber nicht eigens getestet.

## Lizenz

GPL-3.0-only. PaperRust beruht auf paperless-ngx (GPL-3.0, © die paperless-ngx-Autoren). Fehler in
PaperRust bitte nicht bei paperless-ngx melden.
