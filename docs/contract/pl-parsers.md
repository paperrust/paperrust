# Vertrag: pl-parsers

Reine Bibliothek (keine DB, keine Routen) – Grundbaustein des Python-freien Workers. Ersetzt die
Textextraktion, die upstream über Parser-Plugins, libmagic und Apache Tika macht.
Quelle: paperless-ngx v3.4.2 (`upstream/`), CPython 3.12 `email`, imap_tools 1.15, humanize 4.15,
pikepdf 10.13, libmagic (Docker: file 5.46, Referenz: file 5.48 aus `reference/bin/libmagic`).
Nur reine Rust-Abhängigkeiten, kein Netzwerk, kein Tika/Gotenberg.

## Module

| Modul | Upstream-Quelle | Inhalt |
|---|---|---|
| `lib` | `paperless/parsers/__init__.py` (`ParserProtocol`, `MetadataEntry`, `ParserContext`) | Trait `DocumentParser`, `MetadataEntry {namespace, prefix, key, value}` (Feldreihenfolge = JSON von `/api/documents/{id}/metadata/`), `ParseContext` (mailrule_id, `PAPERLESS_TIME_ZONE`, `Limits`), `ParsedDocument` (text, date, archive_path), `ParseError` |
| `registry` | `paperless/parsers/registry.py` (`register_defaults`, `get_parser_for_file`, `all_parsers`), `documents/parsers.py` (`is_mime_type_supported`, `get_default_file_extension`, `is_file_ext_supported`, `get_supported_file_extensions`) | `ParserRegistry`, `RegistryConfig {tika_enabled, extended_formats}` |
| `mime` | `documents/consumer.py` (`magic.from_file(working_copy, mime=True)`), libmagic Magdir/readcdf.c/is_csv.c/is_json.c | `detect_file`, `detect_bytes`, `cdf_classify` |
| `text` | `paperless/parsers/text.py` (`TextDocumentParser`), `utils.py` (`read_file_handle_unicode_errors`) | Kodierungserkennung, Text-Parser |
| `mail` (+`header`, `message`) | `paperless/parsers/mail.py` (`MailDocumentParser.parse/build_formatted_text/strip_text/extract_metadata/parse_file_to_message`), imap_tools `message.py`/`utils.py`, CPython `email.feedparser`/`message`/`header`/`utils`/`_parseaddr`, humanize `naturalsize` | EML-Parser |
| `office` (+`ooxml`, `odf`, `rtf`) | `paperless/parsers/tika.py` (`TikaDocumentParser`) | DOCX/XLSX/PPTX/PPSX/ODT/ODS/ODP/ODG/RTF ohne Tika |
| `pdf` | `paperless/parsers/utils.py` (`PDF_TEXT_MIN_LENGTH`, `is_tagged_pdf`, `extract_pdf_text`, `post_process_text`, `is_born_digital_text`, `pdf_born_digital_text`, `get_page_count_for_pdf`, `extract_pdf_metadata`), `tesseract.py` (`RasterisedDocumentParser` ohne OCR), pikepdf `_xmp.py` | PDF-Text, Seitenzahl, getaggt, XMP-Metadaten |
| `html` | `mail.py` (`tika_parse` → Tika `text/html`) | `html_to_text` (Tika-Ersatz), `HtmlDocumentParser` (Erweiterung) |
| `msg` | – (Erweiterung) | Outlook-MSG |
| `limits` | – | Grenzen, `Deadline` |
| `pystr` | CPython `str.isspace/strip/isprintable/splitlines`, Codec-Namen | Python-String-Semantik |

## Parser-Schnittstelle (`ParserProtocol` → `DocumentParser`)

upstream ist zustandsbehaftet (`parse()` füllt, `get_text()/get_date()/get_archive_path()` lesen);
hier liefert `parse()` ein `ParsedDocument`. Weitere Methoden: `class_name` (upstream-Klassenname),
`name`, `supported_mime_types` (`{mime: endung}` in upstream-Reihenfolge), `score(mime, filename, path)
-> Option<i32>`, `can_produce_archive`, `requires_pdf_rendition`, `uses_remote_service`,
`page_count`, `extract_metadata` (darf nie fehlschlagen → leere Liste + Warnung),
`thumbnail` (nicht umgesetzt → `Unsupported`).

### Auswahl (`get_parser_for_file`)

Externe Parser zuerst, dann eingebaute in `register_defaults`-Reihenfolge: Text, Remote, Tika, Mail,
Tesseract. Kandidat nur, wenn MIME in `supported_mime_types`; `allow_remote=False` schließt
`uses_remote_service` aus; `score is None` → übersprungen; höchster Score gewinnt, bei Gleichstand der
erste. Alle eingebauten Scores sind 10. Tika: `None`, wenn `TIKA_ENABLED` aus. Remote (Azure): ohne
konfigurierte Engine immer `None` → hier nicht nachgebildet.

| Parser (upstream) | MIME → Endung | archive / pdf_rendition |
|---|---|---|
| `TextDocumentParser` | text/plain .txt, text/csv .csv, application/csv .csv | false / false |
| `TikaDocumentParser` | application/msword .doc, …wordprocessingml.document .docx, application/vnd.ms-excel .xls, …spreadsheetml.sheet .xlsx, application/vnd.ms-powerpoint .ppt, …presentationml.presentation .pptx, …presentationml.slideshow .ppsx, …opendocument.presentation .odp, …spreadsheet .ods, …text .odt, …graphics .odg, text/rtf .rtf | false / **true** |
| `MailDocumentParser` | message/rfc822 .eml | false / **true** |
| `RasterisedDocumentParser` | application/pdf .pdf, image/jpeg .jpg, image/png .png, image/tiff .tif, image/gif .gif, image/bmp .bmp, image/webp .webp, image/heic .heic | true / false |
| `RemoteDocumentParser` | pdf, png, jpeg, tiff .tiff, bmp, gif, webp (nur mit Azure-Konfiguration) | – |

`get_default_file_extension(mime)`: Endung des gewählten Parsers (Score beachtet, also mit Tika-Schalter),
sonst `mimetypes.guess_extension`. `get_supported_file_extensions()`: alle Parser *ohne* Score, je MIME
alle `mimetypes.guess_all_extensions` + Parser-Endung.

### Rust-Registry

`RegistryConfig::default()` = `tika_enabled: true` (Office-Parser statt Tika-Server, Standalone-Ziel),
`extended_formats: false`. `tika_enabled: false` bildet upstream ohne Tika exakt nach.
`extended_formats: true` ergänzt `HtmlDocumentParser` (text/html, application/xhtml+xml) und
`MsgDocumentParser` (application/vnd.ms-outlook) – upstream lehnt beide ab.

## MIME-Erkennung (libmagic)

Der Consumer nutzt nur `magic.from_file(path, mime=True)` (keine Endung). Ausnahme upstream: Endung
`.pdf` und MIME `application/octet-stream` → `qpdf --replace-input` und erneute Erkennung (externes
Programm, **nicht** hier; Aufgabe des Workers).

Nachgebildet (empirisch kalibriert, `reference.json` → `mime_probes`):
- leer → `inode/x-empty`; 1 Byte → `application/octet-stream`.
- Signaturen: `%PDF-` am Anfang (PaperRust: auch nach bis zu 1024 Bytes Müll, Known difference 14); PNG nur mit `IHDR`; JPEG `FF D8 FF`; `GIF8`; BMP mit gültiger
  DIB-Kopfgröße; TIFF (II*/MM*/BigTIFF); RIFF/WEBP; ftyp-Marken (heic/heix → image/heic, mif1 →
  image/heif, msf1, hevc, avif, sonst video/mp4); gzip, 7z, rar, MZ.
- ZIP: OOXML-Regel (erster Name `word/|ppt/|xl/|visio/` direkt; sonst Regex
  `[Content_Types].xml|_rels/.rels|docProps|customXml` ab 0x1E, dann bis zu 3 weitere lokale Köpfe:
  Suche beginnt 26 Bytes nach Regex-Ende bzw. Namensanfang des letzten Kopfs, Fenster 10000 Bytes;
  3. Suche erfolglos oder 4. Name unbekannt → "Microsoft OOXML" = `application/octet-stream`).
  Folge: ppsx → pptx-MIME. **PaperRust-Zusatz (Known difference 13):** greift die Regel nicht
  oder endet sie bei „Microsoft OOXML“, werden alle lokalen Köpfe und das Zentralverzeichnis
  gelesen; gibt es `[Content_Types].xml` und einen Eintrag `word/` / `ppt/` / `xl/` (in dieser
  Reihenfolge), gilt die Datei als DOCX/PPTX/XLSX. ODF/EPUB: Eintrag `mimetype` zuerst und unkomprimiert,
  Inhalt ab Offset 38; sonst `application/zip`.
- OLE2/CDF: SummaryInformation-AppName (enthält Word/Excel/PowerPoint…) → msword/vnd.ms-excel/
  vnd.ms-powerpoint, sonst `application/vnd.ms-office`; ohne Summary Stream-Präfixe (Book, Workbook,
  WordDocument, PowerPoint, DigitalSignature, `__substg1.0_` → vnd.ms-outlook); sonst/ungültig
  `application/x-ole-storage`.
- Text = keine Bytes aus `00–06, 0E–19, 1C–1F, 7F` (ein NUL am Ende erlaubt) oder UTF-16 mit BOM.
  Dann in Reihenfolge: JSON (nur Objekt/Array, nachsichtig bei `,}`; mehrere Werte zeilenweise →
  `application/x-ndjson`), CSV (is_csv: nur Kommas, gleiche Anzahl je Zeile, ≥ 2 Zeilen, Anführungs-
  zeichen, keine Leerzeile), RTF (`{\rtf` + reines ASCII), XML (`<?xml` + Leerraum; svg/xhtml),
  `%PDF-` in den ersten 1024 Bytes, PostScript, PHP, Shebang-Interpreter, Mail (`Return-Path:`/
  `Delivered-To:` ohne Groß/Klein; `Received:`, `From:`, `To:`, `Cc:`, `Date:`, `Subject:`,
  `Relay-Version:`, `Forward to ` exakt am Anfang → message/rfc822; `Path:`/`Xref:` → message/news;
  `From ` → application/mbox), vCard/vCalendar, PGP/PEM/SSH, diff, HTML (Suche in 4096 Bytes, ohne
  Groß/Klein: `<!doctype html`, `<head>`/`<head `, `<title`, `<html`, `<script`, `<style`, `<table`,
  `<a href=`), TeX, Quelltext (C/C++/Python/Makefile/troff am Zeilenanfang), sonst `text/plain`.
  **Wichtig für Parität:** `MIME-Version:` oder `X-Mozilla-Status:` am Anfang → `text/plain` → Text-
  statt Mail-Parser (wie upstream).

## Text (`TextDocumentParser`, `read_file_handle_unicode_errors`)

BOM UTF-16 LE/BE → `utf-16`, BOM UTF-8 → `utf-8-sig`, sonst `utf-8`, strikt. Bei Fehler (auch ungerade
UTF-16-Länge, einzelne Surrogate): Warnung `Unicode error during text reading, continuing` und
`raw.decode("utf-8", errors="replace")` auf den **rohen** Bytes (BOM bleibt als U+FEFF). Danach NUL
entfernt. Datum `None`, Seitenzahl `None`, Metadaten `[]`. Markdown ist für libmagic `text/plain` →
dieser Parser; CSV ebenso.

## E-Mail (`MailDocumentParser`)

`parse_file_to_message`: `MailMessage.from_bytes` (CPython feedparser, compat32); kein Absender →
`ParseError("Could not parse {p}: Could not parse {p}: Missing 'from'")` (doppelt, wie upstream).
Naives Datum → `make_aware(TIME_ZONE)` (zoneinfo fold=0: mehrdeutig → frühere, Lücke → Offset davor).

Formatierter Text (exakt):

```
Subject: {subject}\n\n
From: {from.full | ''}\n\n
To: {', '.join(to.full)}\n\n
[CC: {...}\n\n]            # nur wenn vorhanden
[BCC: {...}\n\n]
[Attachments: {name} ({naturalsize(size, binary, "%.2f")}), ...\n\n]
[HTML content: {strip_text(tika(html))}]
\n\n{strip_text(text)}
```

- `strip_text`: jeder Python-Whitespace-Lauf (`\s+`, inkl. U+001C–1F, U+0085, U+00A0 …) → ein
  Leerzeichen, dann `strip()`.
- `subject`: `''.join(decode_value(*p) for p in decode_header(raw))`; Folding (`\r\n `) bleibt
  erhalten, wenn keine kodierten Wörter vorkommen; 8-Bit-Rohbytes → UTF-8 (`ignore`), kodierte
  Wörter darin werden dann nicht dekodiert.
- Adressen: `getaddresses(strict=True)` (Anzahl = 1 + Kommas außerhalb von Anführungszeichen, sonst
  keine Adresse; Klammern müssen balanciert sein), Name nur aus dem **ersten** `decode_header`-Stück,
  `full` = `"Name <mail>"` / Name / Mail; Kommentare werden Name, wenn kein Anzeigename.
- `text`: alle Nicht-multipart-Teile ohne (nichtleeren) Dateinamen mit Typ `text/plain`, auch aus
  eingebetteten `message/rfc822`; `html`: dito `text/html`, `<meta … charset=…>` → `charset=utf-8`.
  Dekodierung: `get_payload(decode=True)` (base64 nachsichtig, quoted-printable wie `a2b_qp`,
  uuencode), Zeichensatz-Parameter (RFC 2231), unbekannt → UTF-8, Fehler werden ignoriert.
- Anhänge: Teile mit `Content-ID` oder Dateiname (auch leer) oder Typ `message/rfc822`; Name über
  `get_filename()` (RFC 2231 inkl. Fortsetzungen) + `decode_header`; 8-Bit-Namen über den zweiten
  Versuch von imap_tools; Größe = Länge der dekodierten Nutzlast, bei `message/rfc822` die
  Länge von `as_bytes()` (Näherung, s. u.).

`extract_metadata`: je Kopfzeile (klein, erstes Auftreten, alle Werte mit `", "` verbunden, roh
inkl. Folding) `{namespace:"", prefix:"header", key, value}`; Kopfzeilen mit Nicht-ASCII-Bytes
fallen weg (upstream: `UnicodeEncodeError` durch Surrogate); dann `attachments` = `"{name}({size})"`
mit `", "`, `date` = `strftime("%Y-%m-%d %H:%M:%S %Z")` (`UTC`, `UTC+01:00`, `CET`/`CEST` bei
naiven Daten); sortiert nach (prefix, key). Fehler → `[]`.

`get_date` = Mail-Datum (fehlend/ungültig → 1900-01-01 naiv → aware).

**Archiv-PDF (Lücke E12, `mail/pdf.rs`)** – upstream `generate_pdf`/`mail_to_html`/
`generate_pdf_from_html` (src/paperless/parsers/mail.py@v3.4.2): nur mit eingehängtem
`PdfConverter` (`ParserRegistry::set_mail_converter`; im Worker WordCraft im Kindprozess
`paperrust-pdftool`). Dann `requires_pdf_rendition() == true`.
1. Kopfseite `email_as_html.html` wie `email_msg_template.html`: Datum
   (`localtime(mail.date)` in `TIME_ZONE`, `%Y-%m-%d %H:%M`), From, Subject (fett), To, CC, BCC,
   Attachments (`name (naturalsize)`), Trennlinie, Text (`mail.text.strip()`). Jeder Wert über
   `clean_html` (maskieren, Links/E-Mail-Adressen verlinken, `\n` → `<br>`); leere Zeilen entfallen.
   Tabelle mit Inline-Stilen statt Tailwind-Raster (WordCraft kennt kein CSS-Grid).
2. Hat die Mail einen HTML-Teil: `index.mht` (multipart/related) mit dem HTML-Teil
   (`<script` → `<div hidden `, `</script` → `</div`) und allen Anhängen mit `Content-ID` als Teile
   (upstream: Anhänge als Gotenberg-Ressourcen, `cid:`-Verweise umbenannt).
3. Zusammenfügen nach Layout (`MailRule.PdfLayout`): 1 Text, dann HTML; 2 HTML, dann Text; 3 nur
   HTML; 4 nur Text; 0/Unbekanntes wie der `_`-Zweig (Text, dann HTML). Ohne HTML-Teil nur die
   Kopfseite. Ergebnis `merged.pdf` in `ParseContext::work_dir` (Worker: Parser-Ordner der Aufgabe; ohne
   Angabe neben der Mail – ebenso für das Office-PDF `<stem>.office-convert.pdf`). Das Layout kommt aus
   `ParseContext::mail_pdf_layout` (der Worker setzt: mit Regel `rule.pdf_layout`, sonst
   `PAPERLESS_EMAIL_PARSE_DEFAULT_LAYOUT`, Standard 1).

## Office (`TikaDocumentParser` → `OfficeDocumentParser`)

upstream: Tika `as_text` → `content.strip()`, `parsed.created` als Datum (naiv → `make_aware`),
PDF über Gotenberg (immer), Seitenzahl aus dem Archiv-PDF, Metadaten = alle Tika-Felder als
`{namespace:"", prefix:"", key, value}`. Ohne Tika (`TIKA_ENABLED=false`) sind alle Office-MIME-Typen
"Unsupported mime type".

Hier: DOCX (Kopfzeilen, Körper, Fuß-/Endnoten, Fußzeilen; Tabellenzellen mit Tab, Zeilen je Zeile;
gelöschter Text und Feldcodes nicht), XLSX (je Blatt Name, dann je Zeile `\t` + Zellen mit `\t`;
Shared/Inline Strings, Bool `TRUE/FALSE`, Zahlen roh), PPTX/PPSX (Folien in `sldIdLst`-Reihenfolge,
Absätze, Notizen), ODT/ODS/ODP/ODG (`text:s`, `text:tab`, `text:line-break`, Tabellen, wiederholte
Zellen/Zeilen bis 100), RTF (Codepage `\ansicpg`, `\'hh`, `\uN`/`\ucN`, Ziele wie `fonttbl`, `pict`,
`\*` übersprungen, `\info` → Metadaten). Datum = `dcterms:created`. Metadaten mit Tika-Schlüsseln:
`dc:title, dc:subject, dc:creator, meta:keyword, dc:description, meta:last-author, cp:revision,
cp:category, …, dcterms:created, dcterms:modified, extended-properties:Application/AppVersion/
Company/…, meta:page-count, xmpTPg:NPages, meta:word-count, …, Content-Type`.
DOC/XLS/PPT (OLE2, Lücke E9, `office/ole.rs`, nach [MS-DOC]/[MS-XLS]/[MS-PPT]/[MS-OLEPS]):
- Word 97–2003: FIB → Clx/Stückliste (komprimiert cp1252, sonst UTF-16LE), alle Textteile in
  CP-Reihenfolge (Haupttext, Fußnoten, Kopf-/Fußzeilen, Kommentare, Endnoten, Textfelder);
  Absatz/Zeilenumbruch → `\n`, Zelle → Tab, Feldbefehle weg (Feldergebnis bleibt). Word 6/95 ohne
  Schnellspeicherung: Text zwischen `fcMin`/`fcMac`. `fEncrypted` → `Encrypted`.
- Excel 97–2003 (BIFF8 `Workbook`, BIFF5 `Book`): Form wie XLSX (Blattname, je Zeile `\t` + Zellen);
  SST über CONTINUE-Grenzen, LabelSst/Label/Number/RK/MulRk/Formel-Ergebnis (Zahl, Text, Bool)/
  BoolErr; Zahlen ohne Zellformat (ganze Zahlen ohne `.0`). `FILEPASS` → `Encrypted`.
- PowerPoint 97–2003: CurrentUser → UserEditAtom-Kette → PersistDirectory (neueste Fassung),
  Folien in SlideListWithText-Reihenfolge, Text aus TextChars/TextBytes-Atomen (Liste und
  Folienzeichnung), Notizen über `SlideAtom.notesIdRef`; Rückfall ohne Verzeichnis: Folien-Container
  in Dateireihenfolge. Je Folie Titel, Zeilen, Notizen, Leerzeile.
- Metadaten aus `SummaryInformation`/`DocumentSummaryInformation` mit denselben Tika-Schlüsseln wie
  OOXML (Titel, Betreff, Autor, Stichwörter, Kommentar, letzter Autor, Revision, Kategorie,
  Druck-/Erstell-/Änderungsdatum ISO-UTC, Anwendung, Firma, Manager, Vorlage, Seiten (+
  `xmpTPg:NPages`), Folien, Wörter, Zeichen, `Content-Type`); Datum = `dcterms:created`.

Folien für das Archiv-PDF (Lücke E10): `office::slides` (PPTX/PPSX: Titel-Platzhalter
`title`/`ctrTitle`, übrige Absätze, Notizen; ODP/ODG: `draw:page`, Rahmen
`presentation:class="title"`, `presentation:notes`; PPT wie oben) und `office::slides_html` (eine
Folie je Seite mit Seitenumbruch, Nummer `n / N`, Titel als Überschrift, Notizen klein). Der
Erzeuger (pl-consumer `WordCraftConverter`) setzt das HTML im Kindprozess auf A4 quer.

## PDF (`RasterisedDocumentParser` ohne OCR, `utils`)

- `get_page_count`: lopdf (nachsichtig, repariert Querverweise, Junk vor `%PDF-` in den ersten
  1024 Bytes wird übersprungen); nicht lesbar/passwortgeschützt → `None` + Warnung.
- `is_tagged`: Katalog `/MarkInfo /Marked` wahr.
- `extract_pdf_metadata` = pikepdf `open_metadata().items()`: nur XMP (`/Metadata`), **kein**
  DocInfo-Rückfall. Nur `rdf:Description` mit `rdf:about=""` direkt unter `rdf:RDF`; erst Attribute
  (ohne rdf-Namensraum), dann Kindelemente, je Schlüssel der *erste* Wert über alle Descriptions;
  Text (ungetrimmt) wenn nicht leer, sonst `rdf:Alt` → erstes `li`, `rdf:Seq` → `" ".join(str(e))`,
  `rdf:Bag` → `str(set)` (`{'a', 'b'}`), sonst `""`. Präfix aus pikepdf `DEFAULT_NAMESPACES`;
  unbekannter Namensraum → Eintrag fällt weg. Ungültiges XMP → zweiter Versuch ohne unzulässige
  Bytes, sonst `[]`.
- Text (Lücke E14, `pdftext.rs`): Inhaltsströme inkl. Form-XObjects mit Textzustand ausführen,
  Textstücke mit Position/Breite (Schrift-`Widths`/`W`, sonst ½ em) sammeln, zu Zeilen gleicher
  Grundlinie gruppieren (oben → unten, links → rechts, Lücke > 0,15 em → Leerzeichen), Ligaturen
  zerlegen, Seiten mit `\x0c` – wie `pdftotext -layout` nach `post_process_text`. Seite so nicht
  lesbar → einfacher lopdf-Text.
- `pdf_born_digital_text`: Text (siehe oben) → `post_process_text` (Python-
  Regeln inkl. `$`-Semantik) → born-digital, wenn Text und (getaggt oder > 50 Zeichen).
- `parse`: born-digital PDF → Text; sonst und für Bilder `ParseError::OcrRequired` (OCR ist ein
  eigener Baustein).

## Robustheit (`Limits`)

Standard: Datei ≤ 512 MiB, ZIP ≤ 20 000 Einträge, Eintrag ≤ 256 MiB entpackt, gesamt ≤ 1 GiB,
Kompressionsverhältnis ≤ 200:1 ab 16 MiB, Verschachtelung ≤ 256 (XML, RTF; MIME ≤ 64), Text ≤ 64 MiB,
PDF ≤ 5000 Seiten, Zeitlimit 300 s (geprüft in XML-/RTF-Schleifen und je PDF-Seite). Gezählt wird
beim tatsächlichen Entpacken (gefälschte Größenangaben helfen nicht). Panics in lopdf werden
abgefangen. Fehler → `ParseError::{LimitExceeded, Timeout, Invalid, Encrypted, Unsupported,
OcrRequired, Upstream, Io}`, nie Absturz (`tests/robustness.rs`: alle Beispiele abgeschnitten und
mit Zufallsbytes verfälscht).

## Paritätstests

- `tests/parity/pl-parsers/gen_samples.py` erzeugt `samples/` (Text, EML, PDF via fpdf2/pikepdf,
  Bilder via Pillow/pillow_heif, Office als ZIP+XML von Hand, ZIP-/MIME-Proben unter `samples/mime/`).
- `tests/parity/pl-parsers/gen_reference.py` (mit `source reference/env.sh`) schreibt
  `reference.json` mit upstream-Ergebnissen: libmagic-MIME, Parserwahl mit/ohne Tika,
  Standardendungen, `read_file_handle_unicode_errors`, Mail (Text mit Tika-Platzhalter
  `TIKAHTML{len(html)}`, Datum, Fehlertext, Metadaten, Anhänge, html/text), PDF (Seitenzahl, getaggt,
  Metadaten). Kein DB-Zugriff.
- `crates/pl-parsers/tests/parity.rs` vergleicht alles exakt; Office-/PDF-Text und HTML→Text mit
  eigenen Erwartungen (kein lokaler Tika/pdftotext).

Ergebnis: MIME 66/67 Beispiele + alle 125 Proben gleich (Ausnahme unten), Parserwahl, Endungen, Text,
Mail (13 Mails inkl. RFC 2047/2231, Folding, 8-Bit-Kopfzeilen, Digest, eingebettete Nachrichten,
fehlende Grenze, DST-Lücke) und PDF-Metadaten/Seitenzahl/Tagging: 0 Abweichungen.

## Known differences

1. **Office-Text ≈ Tika-Text** (Lücke E13): Absätze/Zeilenumbrüche (`w:br`, `text:line-break`, `a:br`)
   → `\n`, Tabs → `\t`, Tabellenzellen → Tab, Blätter wie Tika (Blattname, je Zeile Tab + Zellen).
   Excel-Zahlen (XLSX und XLS) werden wie bei Tika/POI `DataFormatter` (en-US) nach ihrem Zellformat
   ausgegeben (`office::numfmt`): eingebaute Formate 0–49, eigene Datums-/Zeitformate (y/m/d/h/s,
   AM/PM, `[h]`, englische Monats-/Tagesnamen, Datumssystem 1904), einfache Zahlenformate (Dezimalstellen,
   Tausender, Prozent, Abschnitte, Texte, `[$€-407]`), kaufmännisch gerundet. Rest: wissenschaftliche
   und Bruch-Formate → „General“; Reihenfolge von Kopf-/Fußzeilen und Foliennotizen kann abweichen;
   lokalisierte Formate (`[$-407]` mit deutschen Monatsnamen) bleiben englisch wie POI en-US.
2. **Office-Metadaten**: Tika-Schlüssel für Kern- (inkl. `dc:identifier`, `cp:version`), App- (inkl.
   `dc:publisher` aus Company, `extended-properties:DocSecurity`/`DocSecurityString`, `Notes`,
   `HiddenSlides`, `PresentationFormat`, `meta:character-count-with-spaces`; Zähler nur > 0,
   `xmpTPg:NPages` aus Seiten oder Folien) und benutzerdefinierte Eigenschaften (`custom:<Name>`).
   Bewusst nicht: Tikas Verarbeitungsangaben (`X-TIKA:*`, `resourceName`, `Content-Length`) – sie
   beschreiben den Tika-Lauf, nicht das Dokument. Mehrfachwerte (Listen) als ein Text.
3. **PDF nur mit Erzeuger**: Office- und Mail-PDF entstehen nur mit eingehängtem `PdfConverter`
   (Worker: WordCraft im Kindprozess). Ohne ihn ist `requires_pdf_rendition()` `false` (upstream
   `true`), der Worker speichert das Original ohne Archiv, Seitenzahl `None`. Scheitert der
   Erzeuger, geht es mit Warnung ohne Archiv weiter (upstream Mail: `ParseError` → Einlesen
   scheitert; Office: ebenso).
4. **HTML in Mails**: eigener Konverter statt Tika; wegen `strip_text` fast immer gleich, aber
   `<title>` wird (wie Tika) vorangestellt, Entitäten nach HTML5, `<script>/<style>` entfallen.
5. **PDF-Text**: eigene Lesereihenfolge statt poppler (Lücke E14). Auf den upstream-Beispielen gleich
   wie `pdftotext -layout` nach `post_process_text`, bis auf Zeichen, die poppler umschreibt
   (z. B. typografische Anführungszeichen). Ohne Schriftprogramme: Breiten nur aus dem
   PDF-Wörterbuch (Lücken-Erkennung kann bei Schriften ohne `Widths` abweichen); gedrehter Text
   wird nach seinem Anker einsortiert. Kaputte PDFs, die qpdf/pikepdf noch reparieren, liest lopdf
   eventuell nicht (dann Seitenzahl `None`). Der OCR-Pfad (pl-ocr) hat einen eigenen Textleser.
6. **XMP `rdf:Bag`**: upstream iteriert ein Python-`set` (Reihenfolge zufällig je Prozess); hier
   Dokumentreihenfolge ohne Duplikate.
7. **MIME**: keine vollständige libmagic. Zufällige Binärdaten (libmagic z. B. `image/x-tga`) und
   exotische Formate → `application/octet-stream`; Quelltext-/Skript-Heuristiken nur für die
   häufigsten Fälle (C, C++, Python, Shell, Perl, Ruby, Node, awk, Tcl, Lua, PHP, Makefile, troff,
   TeX, diff). Sehr kurze Sonderfälle (z. B. 2-Byte-Datei mit DEL) können abweichen.
   OLE2-Einordnung ist aus readcdf.c nachgebaut, aber mangels echter DOC/XLS/MSG-Dateien nur mit
   erzeugten Verbunddateien geprüft.
8. **`message/rfc822`-Anhangsgröße**: upstream `len(as_bytes())` (Generator schreibt Kopfzeilen neu);
   hier Rohbytes mit LF-Zeilenenden – gleich für einfache Nachrichten, bei langen/gefalteten
   Kopfzeilen kann die Größe abweichen.
9. **Unterstützte Endungen**: Python-`mimetypes` liest unter Linux `/etc/mime.types` bzw. unter
   Windows die Registry; hier nur die eingebaute Python-Tabelle (wie `MimeTypes(filenames=())`).
10. **Python-`str.isprintable`** (Adressbereinigung) nur für die üblichen Steuer-/Format-/Trenn-
    zeichen nachgebildet, nicht für alle nicht zugewiesenen Codepunkte.
11. **Erweiterungen** (nur mit `extended_formats`): HTML-Dokumente und Outlook-MSG (Text im
    EML-Format, Metadaten `header`/`attachments`/`date`). upstream lehnt beide ab.
12. **`HeaderParseError`** (ungültiges Base64 in kodiertem Wort) lässt upstream den Consumer scheitern;
    hier wird nachsichtig dekodiert.
13. **OOXML unabhängig von der Eintragsreihenfolge** (Testprotokoll 2026-11-16 C-008): DOCX aus
    python-docx (`[Content_Types].xml`, `_rels/`, `docProps/…`, dann `word/…`) erkannte die
    libmagic-Nachbildung als `application/octet-stream`, der Upload wurde abgelehnt (laut
    Testprotokoll erkennt libmagic im Referenz-venv genau diese Datei als DOCX). Jetzt: Paket mit
    `[Content_Types].xml` und `word/`/`ppt/`/`xl/`-Eintrag irgendwo im Archiv → DOCX/PPTX/XLSX.
    Dadurch weichen bewusst von libmagic 5.48 ab: Probe `office_late_word.docx` (libmagic
    `application/octet-stream`) sowie `zip_ooxml_second_word` und `zip_ooxml_big_first` (libmagic
    `application/zip`), alle jetzt DOCX (`tests/parity.rs` `MIME_KNOWN_DIFF`/`MIME_PROBE_KNOWN_DIFF`).
    Upstream lehnt solche Dateien teils ab; PaperRust nimmt sie an.
14. **PDF mit Müll vor `%PDF-`** (Testprotokoll 2026-11-16 C-073): libmagic erkennt `%PDF-` ab Byte 0
    und nur bei Textdateien auch in den ersten 1024 Bytes; Müll mit Binärbytes davor ergab
    `application/octet-stream` (abgelehnt). PaperRust: `%PDF-<Ziffer>.<Ziffer>` in den ersten 1024
    Bytes **und** danach ein indirektes Objekt (`<n> <n> obj`) → `application/pdf`, auch bei
    Binärmüll (so lesen es PDF-Leser; der Consumer repariert die Datei bei Bedarf über pl-pdftools,
    §5 dort). Binärdateien, die die Zeichenfolge ohne Objekt enthalten, bleiben, was sie waren. Probe
    `pdf_leading_junk.pdf` weicht dadurch bewusst von libmagic ab (`MIME_KNOWN_DIFF`).
15. **Alte Office-Formate (OLE2)**: eigener Leser statt Apache POI. Text wie bei OOXML formatiert
    (POI setzt andere Zeilenumbrüche/Tabs); Excel-Zahlen nach Zellformat wie bei XLSX (Punkt 1);
    PowerPoint: Masterfolien-, Kommentar- und eingebettete OLE-Texte fehlen; Word 6/95
    schnellgespeichert → `Unsupported`; verschlüsselte Dateien → `Encrypted` (POI/Tika ebenso
    ohne Passwort nicht lesbar).
16. **Mail-PDF-Gestaltung**: WordCraft statt Chromium – gleiche Felder, Reihenfolge und Texte, aber
    andere Schrift, Abstände und Seitenumbrüche; HTML-Teil ohne JavaScript/CSS-Layout (Tabellen,
    Absätze, Bilder, Inline-Stile). `PdfLayout` 0 bei Regel und unbekannte Werte → Text, dann HTML
    (upstream: unbekannter Wert → `ValueError`). Fehlende Mailregel → Standard-Layout statt
    `MailRule.DoesNotExist`.
17. **Folien-PDF**: schlichte Textseiten (Titel, Absätze, Notizen) je Folie statt gerenderter Folie
    (LibreOffice); Formen, Bilder, Diagramme und Folienlayout fehlen.
