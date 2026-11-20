# PDF-Werkzeuge: Schwärzen, Verkleinern, Reparieren

[English](en/pdf-tools.md)

PaperRust bringt drei PDF-Werkzeuge mit (dazu [In PDF umwandeln](#in-pdf-umwandeln) für Bilder und Office-Dateien). Sie beruhen auf Bausteinen von
[PdfCraft](https://github.com/storytold/pdfcraft) (MIT OR Apache-2.0) und laufen in einem eigenen,
abgeschotteten Programm (`paperrust-pdftool`) mit Zeit- und Speichergrenze. Eine kaputte oder
bösartige PDF-Datei kann deshalb den Server weder aufhängen noch abstürzen lassen – im schlimmsten
Fall schlägt nur dieser eine Vorgang fehl.

Schwärzen und Verkleinern legen immer eine **neue Version** des Dokuments an. Die bisherige Fassung
bleibt im Versionsverlauf erhalten.

<a id="schwaerzen"></a>
## Schwärzen

Dokument öffnen → **Aktionen** → **Schwärzen…**

- **Bereiche markieren:** Mit der Maus Rechtecke über die Stellen ziehen, die verschwinden sollen.
  Ein Rechteck lässt sich mit dem × wieder entfernen.
- **Begriffe suchen:** Rechts Begriffe eintragen (einer pro Zeile, z. B. ein Name oder eine
  Kontonummer) und **Suchen**. Alle Treffer sind vorgewählt und werden auf den Seiten markiert.
  Haken entfernen, was stehen bleiben soll. Groß-/Kleinschreibung spielt keine Rolle; geschwärzt wird
  jeweils das ganze Wort, das den Treffer enthält.
- **Metadaten entfernen** (optional): löscht Titel, Autor und XMP-Metadaten aus der PDF-Datei.
- **Endgültig schwärzen** legt die neue Version an.

Was „Schwärzen“ hier bedeutet: Der Inhalt unter den Markierungen wird **wirklich aus der Datei
entfernt** – Text, Bilder (die betroffenen Bildpunkte werden gelöscht) und Vektorgrafik. Es wird
nicht nur ein schwarzer Kasten darübergelegt. Danach prüft PaperRust das Ergebnis: Ist ein
gesuchter Begriff noch lesbar, wird **nichts** gespeichert und eine Fehlermeldung erscheint. Die
neue Version wird wie ein neues Dokument eingelesen (Texterkennung, Vorschaubild, Suchindex); der
geschwärzte Text steht danach auch nicht im Inhalt der neuen Version.

**Scans:** Bei gescannten Dokumenten steht der Text nur in der erkannten Textebene (OCR) des
Archiv-PDFs. Findet die Suche im Original nichts, sucht PaperRust dort und schwärzt die passenden
Bildbereiche im Original.

> **Wichtig:** Die alte Version mit dem ungeschwärzten Inhalt bleibt im Versionsverlauf. Muss auch
> sie verschwinden, löschen Sie sie dort (Versionen-Menü im Dokument). Ältere Sicherungen und
> bereits verschickte Kopien enthalten den Inhalt natürlich weiterhin.

**Teilen geschwärzter Dokumente:** Ein Freigabelink zeigt genau eine Version. Hat ein Dokument
geschwärzte Versionen, zeigt der Teilen-Dialog einen Hinweis: welche Version geschwärzt ist und
welche alles lesbar zeigt. Für neue Links ist die neueste geschwärzte Version vorgewählt; jede
Version lässt sich auswählen. Bei jedem bestehenden Link steht, welche Version er zeigt. Auch beim
Freigabe-Paket (mehrere Dokumente) erscheint ein Hinweis; dort ist „Geschwärzte Version verwenden“
eingeschaltet. Ältere Links auf das Hauptdokument zeigen weiterhin **Version 1** (nicht geschwärzt).

Grenzen: Begriffe über einen Zeilenumbruch hinweg werden nicht gefunden – markieren Sie solche
Stellen als Rechteck. Senkrechter Text (z. B. Japanisch vertikal) unter einer Markierung wird
abgelehnt statt unsicher geschwärzt.

<a id="verkleinern"></a>
## Verkleinern

Dokument öffnen → **Aktionen** → **Verkleinern…** und eine Stufe wählen:

| Stufe | Was passiert |
|---|---|
| **Verlustfrei** | Bilder bleiben unverändert. Doppelte und ungenutzte Objekte werden entfernt, unkomprimierte Daten komprimiert. |
| **Standard** | zusätzlich: Farb- und Graustufenbilder über 225 dpi auf 150 dpi, JPEG-Qualität 60; Schwarzweiß-Scans auf 300 dpi (verlustfrei CCITT G4). Gut zum Archivieren. |
| **Stark** | Bilder über 144 dpi auf 96 dpi, JPEG-Qualität 80. Zum Lesen am Bildschirm; Kleingedrucktes kann unscharf werden. |

Danach zeigt PaperRust die Größe vorher und nachher. Eine neue Version entsteht nur, wenn die Datei
tatsächlich kleiner wird.

<a id="verkleinern-beim-einlesen"></a>
### Verkleinern beim Einlesen

Mit [`PAPERRUST_SHRINK_ON_INGEST_MB`](configuration.md#paperrust_shrink_on_ingest_mb) (Standard `0` =
aus) werden **Scans** über dieser Größe schon beim Einlesen verkleinert, mit der Stufe aus
[`PAPERRUST_SHRINK_PRESET`](configuration.md#paperrust_shrink_preset). Als Scan gilt ein PDF, auf
dessen ersten drei Seiten weniger als 50 Zeichen Text stehen. Gespeichert wird dann die kleinere
Datei (das ist der Sinn der Einstellung); wird sie nicht kleiner, bleibt alles wie es war. Beide
Werte lassen sich auch in der Oberfläche unter **Einstellungen → System** ändern (Bereich Einlesen).

<a id="in-pdf-umwandeln"></a>
## In PDF umwandeln (Bilder und Office-Dateien)

Schwärzen und Verkleinern gehen nur mit PDFs. Bilder und Office-Dateien lassen sich deshalb vorher
in ein PDF umwandeln: Dokument öffnen → **Aktionen** → **In PDF umwandeln…** (oder in der
Dokumentliste mehrere Dokumente auswählen → **Aktionen** → **In PDF umwandeln…**). Das PDF wird eine
**neue Version** des Dokuments; die bisherige Fassung (das Bild bzw. die Office-Datei) bleibt im
Versionsverlauf erhalten. Danach stehen Schwärzen und Verkleinern zur Verfügung.

| Art | Formate | Wie |
|---|---|---|
| Bilder | PNG, JPEG, TIFF (auch mehrseitig, eine PDF-Seite je TIFF-Seite), WebP, BMP, GIF | JPEG wird unverändert eingebettet, die übrigen verlustfrei. Bilder mit Transparenz werden auf weißen Grund gelegt. Mit **Text erkennen (OCR)** bekommt das PDF eine unsichtbare Textebene (Tesseract, Sprache wie beim Einlesen) und wird durchsuchbar. |
| Office | Word (.docx, .docm, .dotx), OpenDocument-Text (.odt, .ott), RTF; außerdem Markdown und HTML-Dokumente | Umwandlung mit [WordCraft](https://github.com/storytold/wordcraft) (MIT OR Apache-2.0) im abgeschotteten Programm `paperrust-pdftool`. Der Text bleibt echter Text (durchsuchbar, schwärzbar); OCR ist nicht nötig. |

Über „In PDF umwandeln“ nicht umgewandelt werden: Tabellen (.xlsx, .xls, .ods), Präsentationen
(.pptx, .ppt, .odp), altes Word (.doc), HEIC-Fotos und PDFs (sind schon PDF). Tabellen,
Präsentationen, altes Word und E-Mails bekommen ihr PDF aber schon beim Einlesen (nächster Abschnitt).
Für schon eingelesene Dateien: Dokument neu verarbeiten.

**Rechte:** wie Schwärzen – wer das Dokument ändern darf.

<a id="office-beim-einlesen"></a>
### Office-Dateien beim Einlesen

Word-, OpenDocument- und RTF-Dateien bekommen schon **beim Einlesen** ein echtes Archiv-PDF (mit
WordCraft), so wie es paperless-ngx mit Gotenberg macht. Ebenso:

- **Altes Word (.doc):** gesetzt wie eine .docx (WordCraft liest das Word-97-Format).
- **Tabellen (.xlsx, .xls, .ods):** je Tabellenblatt eine Seite mit Überschrift und Tabelle; ab sieben
  Spalten quer. Sehr große Tabellen werden nach 200 000 Zellen abgeschnitten (Hinweiszeile).
- **Präsentationen (.pptx, .ppsx, .ppt, .odp) und Zeichnungen (.odg):** je Folie eine Seite (A4 quer)
  mit Foliennummer, Titel, Text und Notizen.
- **E-Mails (.eml):** eine Kopfseite mit Absender, Empfängern, Datum, Betreff, Anhängen und dem Text;
  hat die Mail einen HTML-Teil, folgt er auf eigenen Seiten (mit eingebetteten Bildern). Die
  Reihenfolge stellt die Mailregel ein (**PDF-Layout**: Text, dann HTML / HTML, dann Text / nur HTML /
  nur Text); ohne Regel gilt [`PAPERLESS_EMAIL_PARSE_DEFAULT_LAYOUT`](configuration.md) (Standard:
  Text, dann HTML).

Der durchsuchbare Text kommt weiter aus PaperRusts eigenen Lesern, nicht aus dem PDF. Das Vorschaubild ist dann
die erste Seite des PDFs statt eines reinen Textbildes, die Seitenzahl wird angezeigt, und die
Vorschau im Browser zeigt das gesetzte Dokument. Abschalten mit
[`PAPERRUST_OFFICE_TO_PDF`](configuration.md#paperrust_office_to_pdf)`=false`.

Kann WordCraft eine Datei nicht umwandeln (kaputte Datei, Zeitgrenze, Absturz des Werkzeugs), steht im
Protokoll „Office to PDF conversion failed, continuing without archive PDF: …“ und das Einlesen läuft
wie bisher weiter: das Original wird ohne Archiv-PDF gespeichert, mit Text-Vorschaubild.

<a id="office-grenzen"></a>
### Grenzen der Office-Umwandlung

WordCraft ist noch Alpha-Software; das Ergebnis sieht meist aus wie in Word, aber nicht immer
pixelgleich. Geprüft (Überschriften, Tabellen, Aufzählungen, nummerierte Listen, Bilder,
Seitenumbrüche, Umlaute und €) – bekannte Abweichungen:

- **Schriften:** Wird z. B. Calibri verlangt und ist nicht installiert (Docker-Abbild), sieht der Text
  anders aus als in Word (im Test: Calibri durch eine Serifenschrift ersetzt). Inhalt und Umbrüche
  bleiben ähnlich, das Schriftbild nicht.
- **OpenDocument ohne Seitenvorlage:** Fehlt in einer .odt die Seitenvorlage (selten, nur bei von
  Programmen erzeugten Dateien), setzt WordCraft den Text ohne Seitenränder; LibreOffice nimmt dann
  2 cm.
- **Nicht geprüft** (kann fehlen oder abweichen): Fußnoten, Textfelder/Formen, Diagramme, eingebettete
  Objekte (OLE), Formularfelder, Rechts-nach-links-Schrift. Änderungsverfolgung wird als Endfassung
  gesetzt (ohne Markierungen), Kommentare erscheinen nicht.
- **Tabellen:** keine Zellfarben, Rahmenarten, Diagramme oder Druckbereiche; breite Tabellen werden auf
  die Seitenbreite gestaucht statt auf weitere Seiten umbrochen. Zahlen stehen so da, wie sie in der
  Datei gespeichert sind (Datumszellen als Datum).
- **Präsentationen:** nur der Text je Folie (Titel, Absätze, Notizen) – keine Bilder, Formen,
  Diagramme oder Folienhintergründe.
- **E-Mails:** HTML-Mails werden ohne Browser gesetzt – Tabellen, Absätze, Bilder und einfache
  Stile ja, aufwendige CSS-Layouts nicht.

Schriften: Fehlt eine im Dokument verlangte Schrift (z. B. Calibri, Aptos), nimmt WordCraft eine
installierte oder eine metrisch ähnliche freie Schrift; mitgeliefert sind Source Sans 3, Source
Serif 4, Inter und JetBrains Mono (SIL OFL). Zeilen- und Seitenumbrüche können sich dadurch leicht
verschieben.

<a id="reparieren"></a>
## Reparieren beim Einlesen

Lässt sich eine PDF-Datei beim Einlesen nicht lesen (z. B. beschädigte Querverweise, abgeschnittene
Datei, Müll vor dem Dateianfang), versucht PaperRust automatisch eine nachsichtige Reparatur und
liest mit der reparierten Kopie weiter. Im Protokoll steht dann
„PDF could not be read; repaired it with PdfCraft …“ mit einer kurzen Beschreibung, was repariert
wurde. **Als Original gespeichert wird die unveränderte Datei**, so wie sie angeliefert wurde.
Schlägt die Reparatur fehl, läuft das Einlesen wie bisher weiter (und meldet gegebenenfalls den
Lesefehler).

## Einstellungen und Betrieb

| Variable | Standard | Bedeutung |
|---|---|---|
| [`PAPERRUST_PDFTOOL_TIMEOUT`](configuration.md#paperrust_pdftool_timeout) | 120 | Zeitgrenze je Vorgang in Sekunden |
| [`PAPERRUST_PDFTOOL_MEMORY_MB`](configuration.md#paperrust_pdftool_memory_mb) | 1536 | Speichergrenze des Werkzeugs |
| [`PAPERRUST_PDFTOOL_BIN`](configuration.md#paperrust_pdftool_bin) | neben `paperrust` | Pfad zum Werkzeug |
| [`PAPERRUST_OFFICE_TO_PDF`](configuration.md#paperrust_office_to_pdf) | true | Office-Dateien beim Einlesen in ein Archiv-PDF umwandeln |
| [`PAPERRUST_OFFICE_TO_PDF_TIMEOUT`](configuration.md#paperrust_office_to_pdf_timeout) | 0 | Zeitgrenze je Office-Umwandlung (0 = wie `PAPERRUST_PDFTOOL_TIMEOUT`) |

Das Werkzeug `paperrust-pdftool` muss neben dem Programm `paperrust` liegen (oder über
`PAPERRUST_PDFTOOL_BIN` angegeben sein). Fehlt es, melden Schwärzen und Verkleinern „nicht
installiert“; das Einlesen läuft ohne Reparatur weiter.

**Rechte:** Schwärzen und Verkleinern dürfen alle, die das Dokument **ändern** dürfen (globales
Recht „Dokument ändern“ und Änderungsrecht am Dokument).

Für Programmierer: die Endpunkte stehen unter [REST-API](api.md#pdf-werkzeuge) bzw. im Vertrag
`docs/contract/pl-pdftools.md`.

---

PdfCraft ist laut eigener Beschreibung noch Alpha-Software. PaperRust verwendet nur die Bausteine für
Schwärzen, Verkleinern und Reparieren (nicht die Oberfläche, OCR, Signaturen o. Ä.) und führt sie
ausschließlich abgeschottet aus. „PdfCraft“/„ArtCraft“ und ihre Logos sind Marken ihrer Inhaber;
PaperRust ist nicht mit ihnen verbunden.
