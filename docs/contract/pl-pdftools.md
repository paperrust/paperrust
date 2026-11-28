# Vertrag pl-pdftools – Schwärzen, Verkleinern, Reparieren (PdfCraft), Office → PDF (WordCraft), isoliert

Stand 2026-11-13. **PaperRust-eigen**, kein upstream-Gegenstück in paperless-ngx v3.4.2. Muster für
Rechte und neue Versionen: `src/documents/views.py::DocumentViewSet.update_version`, Quelle der
neuesten Version wie `src/documents/bulk_edit.py::_resolve_root_and_source_doc` (`latest_version`).

Bausteine: [PdfCraft](https://github.com/storytold/pdfcraft) (MIT OR Apache-2.0), Revision
`0f22627bff6cde9930817e891290f653d882bbda` (v0.21.1): `pdfcraft-cos`, `pdfcraft-redact`,
`pdfcraft-optimize`, `pdfcraft-annot`, `pdfcraft-edit`, `pdfcraft-model` (+ ihre internen Abhängigkeiten
`content`, `crypt`, `filters`, `fonts`, `forms`, `geom`, `xfa`). Nicht übernommen: UI (egui), OCR,
Signaturen, JS/XFA-Ausführung, PRC, Druck, Preflight, Barrierefreiheit, MCP, Name/Logos.

## 1. Bausteine und Abhängigkeiten

| Teil | Ort | Inhalt |
|---|---|---|
| Auftragsformat | `crates/pl-pdftools/src/job.rs` | `Job { op, memory_mb }`, `Op::{Redact, Search, Shrink, Repair, SelfTest}`, `Reply::{Redacted, Hits, Shrunk, Repaired, Failed}` (JSON, `op`/`status` als Tag) |
| Prozess-Starter | `crates/pl-pdftools/src/runner.rs` | `run(op)`, `run_with(tool, op, limits)`, `Limits::from_env()`, `tool_path()`, `RunError` |
| Suchlogik | `crates/pl-pdftools/src/matching.rs` | Begriff → Zeichenbereiche → Wortrechtecke (ohne PdfCraft) |
| Speicherwächter | `crates/pl-pdftools/src/guard.rs` | zählender `GlobalAlloc` (nur im Kindprozess installiert) |
| PdfCraft-Anbindung | `crates/pl-pdftools/src/engine.rs` (Funktion `engine`) | `redact`, `search`, `shrink`, `repair` |
| Kindprozess | `crates/pl-pdftools/src/bin/paperrust-pdftool.rs` (Funktion `engine`) | stdin → Auftrag, stdout → Antwort |
| API | `crates/pl-documents-write/src/pdftools.rs` | drei Routen (§4) |
| Einlesen | `crates/pl-consumer/src/pdftools.rs` + Haken in `consumer.rs::run_consumer` | Reparatur, Verkleinern (§5) |
| Light-UI | `frontend-light/src/components/document-detail/pdf-tools/` | `RedactDialog.svelte`, `ShrinkDialog.svelte`, `pdf-tools-api.ts`; zwei Menüeinträge in `DocumentDetail.svelte` |

Server, Consumer und Dokument-API binden `pl-pdftools` **ohne** `engine` (nur serde/tokio): PdfCraft
wird ausschließlich in `paperrust-pdftool` gelinkt.

PdfCraft hängt nicht von lopdf ab (eigenes Objektmodell `pdfcraft-cos`); der Konflikt lopdf 0.45
(PdfCraft-Vendor) ↔ 0.47 (PaperRust) entsteht nur in `pdfcraft-render`, das **nicht** benutzt wird.
PdfCraft braucht aber seine gepatchten Kopien von `hayro-jbig2` und `hayro-syntax` (ohne sie baut
`pdfcraft-filters` nicht): `[patch.crates-io]` im Workspace auf `vendor/hayro-*` derselben
PdfCraft-Revision (Git). Betrifft nur hayro-*, das sonst niemand im Workspace nutzt.

## 2. Isolation (Pflicht)

| Gefahr | Schutz | Nachweis (`crates/pl-pdftools/tests/isolation.rs`) |
|---|---|---|
| Absturz/`panic` im PdfCraft-Code | eigener Prozess; im Kind `catch_unwind` → `Reply::Failed` | `panic_in_tool_is_a_contained_error` |
| `abort`, Stapelüberlauf | eigener Prozess (Arbeitsthread mit 64 MiB Stapel); Server sieht `RunError::Crashed` | `abort_and_stack_overflow_only_end_the_child` (danach weiterer Lauf ok) |
| Endlosschleife/Hänger | Zeitgrenze `PAPERRUST_PDFTOOL_TIMEOUT` (Standard 120 s): `start_kill` + `kill_on_drop` | `hang_is_killed_at_the_time_limit` (2 s) |
| Speicherfresser, Dekompressionsbombe | zählender Allokator, Grenze `PAPERRUST_PDFTOOL_MEMORY_MB` (Standard 1536) → Abbruch nur des Kindes | `memory_hog_hits_the_memory_guard` (128 MiB) |
| kaputte/feindliche Dateien | alles oben | `hostile_and_broken_files_never_hang_or_crash_the_caller`: leer, kein PDF, nur Kopf, abgeschnitten, 200 000 `[` tief, Seitenbaum-Schleife mit riesiger `/Length`, 64-MiB-Flate-Bombe – je Repair/Search/Shrink/Redact, jeder Lauf < 90 s, keine Pfade in Fehlertexten |
| Geheimnisse | Kind bekommt geleerte Umgebung (nur `PATH`, `SystemRoot`, `TEMP`/`TMP`/`TMPDIR`, `RUST_BACKTRACE`), nur Pfade im Auftrag | – |
| riesige Antworten | stdout höchstens 4 MiB, stderr nur letzte 8 KiB (Log) | – |

Selbsttests (`Op::SelfTest`) führt nur ein Debug-Build des Kindes aus; Release antwortet `Failed`.
Windows: `CREATE_NO_WINDOW`. Keine `unwrap`/`expect` in Handlern.

## 3. Werkzeuge (Kindprozess)

- **Redact:** Datei mit `pdfcraft_cos::Document::open` lesen; vorhandene fremde Redact-Marken
  entfernen (`clear_marks`); je Seite eine Redact-Annotation (`pdfcraft_annot::add_annotation`,
  `Shape::Redact`, schwarze Füllung, ohne Überlagerungstext) aus Bereichen + Suchtreffern;
  `pdfcraft_redact::apply` (entfernt Glyphen, Bildpunkte, Pfade, Inhalte in Form-XObjects,
  überlappende Kommentare/Links/Felder; eigene Nachprüfung, fail-closed). **Scans (C-068):** vor
  `apply` werden JPEG-Bilder (`DCTDecode`, Grau/RGB, 8 Bit, ohne Masken/`/Decode`) der markierten
  Seiten entpackt und als Flate gespeichert (`scanimages::prepare`), denn PdfCraft entfernt
  JPEG-Bilder unter einer Marke sonst **ganz**; so löscht es nur die Bildpunkte unter der Marke
  (`images_cleared`). Danach (`scanimages::finish`) bekommen unberührte Bilder ihre JPEG-Daten
  zurück, geschwärzte werden aus den gelöschten Bildpunkten neu als JPEG (Qualität 90) gespeichert.
  CMYK, Masken, JPX → wie bisher ganz entfernt. Optional
  `sanitize::remove_hidden(&[Hidden::Metadata])` (meldet PdfCraft `NothingToApply`, weil keine
  Metadaten mehr da sind – `apply` entfernt das XMP schon –, ist das kein Fehler, C-066b); **voll** neu schreiben (`write_full`, nie
  inkrementell – sonst blieben die alten Objekte in der Datei). Danach eigene Nachprüfung: das
  geschriebene Ergebnis neu öffnen und nach den Begriffen suchen (seitenweise, auch über
  Zeilenumbrüche); ein Treffer → Fehler, nichts wird geschrieben. Seiten außerhalb, ungültige
  Rechtecke, nichts zu tun → Fehler.
- **OCR-Textebene (`ocr_input`, Review 2026-11-27):** Redact und Search bekommen optional das
  Archiv-PDF. Begriffe werden **je Seite in beiden** Textebenen gesucht (früher beendete ein Treffer im
  Original die Suche – bei gemischten Dokumenten blieb der Name auf Scan-Seiten lesbar). Archiv-Treffer
  werden über die Seitengeometrie (`view_matrix`/`display_size`, Skalierung bei anderer Größe) auf das
  Original abgebildet; andere Seitenzahl oder anderes Seitenverhältnis (> 2 %) → Fehler „The OCR text
  layer (archive PDF) does not match the pages of the original; nothing was changed.“ Alle Rechtecke
  beider Ebenen werden markiert (auf Scan-Seiten = Bildbereich unter den OCR-Wortboxen); `term_hits`
  zählt Archiv-Duplikate (gleiche Seite, gleicher Begriff, überlappend) einmal. **Nachprüfung 2:** die
  gleichen Marken, auf das Archiv zurückgerechnet, auf eine Kopie der OCR-Textebene anwenden und dort
  erneut suchen; ein Rest → Fehler, nichts wird geschrieben. Search-Antwort: `Hit.from_ocr` (nur im
  Archiv gefunden).
- **Search:** `pdfcraft_edit::text_lines` je Seite (Zeilen mit Wortrechtecken `ink`), Begriffe ohne
  Groß-/Kleinschreibung (zeichenweise `to_lowercase`). Seit 2026-11-27 je Seite als ein Textstrom
  (`matching::page_matches`): Leerraum zusammengefasst (auch im Begriff), Zeilen mit einem Leerzeichen
  verbunden, endet eine Zeile auf Bindestrich nach einem Buchstaben, zusätzlich „mit Bindestrich ohne
  Leerzeichen“ (Meier-Schmidt) und „ohne Bindestrich“ (Muster-/mann → Mustermann); gleiche Fundstellen
  mehrerer Varianten einmal. Treffer → Rechtecke der berührten Wörter **jeder** beteiligten Zeile
  (+0,5 pt), passt die Wortzahl nicht zu den Rechtecken → Zeilenrechteck. Höchstens `max_hits`
  (API 500, Redact intern 10 000 → darüber Fehler).
- **Shrink:** `pdfcraft_optimize::optimize` mit `lossless` (Bilder `Retain`, kein Herunterrechnen),
  `standard` (= PdfCraft `Preset::Standard`: Farbe/Grau > 225 ppi → 150 ppi JPEG 60, Mono > 450 → 300
  ppi CCITT G4), `strong` (= `Preset::Mobile`: > 144 → 96 ppi JPEG 80); immer `workers = 1`; voll mit
  Objektströmen schreiben. Bericht mit Größe vorher/nachher.
- **Repair:** nachsichtig öffnen (Querverweise neu aufbauen; `repair_log`), ohne Seiten → Fehler,
  klassisch ohne Objektströme schreiben.

## 4. API (`pl-documents-write`, Routen in `routes.toml`)

Gemeinsam: Rechte `documents.change_document` (Modell) **und** `change_document` am Wurzeldokument
(`has_perms_owner_aware`) – sonst 403 `Insufficient permissions` (Text wie `update_version`).
Unbekannte/nicht-numerische id → 404. Quelle: neueste Version (`root_document_id`, `version_index
DESC NULLS LAST, id DESC`), muss `application/pdf` sein (sonst 400 `{"detail": "This document is not a
PDF."}`). Dateien nur innerhalb von ORIGINALS_DIR/ARCHIVE_DIR (`checked_source_path`,
`checked_archive_path`). Werkzeugfehler: `Failed` → 400, `NotInstalled` → 503, `Timeout` → 504,
Absturz/Protokoll → 422; Body `{"detail": "<Nutzertext ohne Pfade>"}`.

| id | Methode, Pfad | Request (JSON) | Antwort 200 | Nebenwirkung |
|---|---|---|---|---|
| `documents.redact_search` | POST `/api/documents/{id}/redact/search/` | `terms` (Pflicht, Liste ≤ 50, je ≤ 200 Zeichen, getrimmt, doppelte/leere entfernt) | `document`, `version`, `source` (`archive`, sobald ein Treffer nur aus der OCR-Textebene stammt, sonst `original`), `truncated`, `hits[]` (`page` 1-basiert, `term`, `context` ≤ 200 Zeichen, `rects`, `from_ocr`) | keine |
| `documents.redact` | POST `/api/documents/{id}/redact/` | `areas` (≤ 2000, `{"page" ≥ 1, "rect": [4 Zahlen]}` mit Fläche), `terms`, `remove_metadata` (bool), `version_label` (≤ 64, Standard `Redacted`) | `task_id`, `document`, `source_version`, `report` | Datei in SCRATCH_DIR, `consume_file` mit `root_document_id` (PaperRust-Warteschlange, sonst 503), `owner_id`/`actor_id` = Nutzer, Auslöser Web-UI |
| `documents.shrink` | POST `/api/documents/{id}/shrink/` | `preset` (`lossless`/`standard`/`strong`, Alias `mobile`; Standard `standard`), `version_label` (Standard `Shrunk`) | `task_id` (oder `null`), `document`, `new_version`, `report` | nur wenn `size_after < size_before`: wie oben; sonst Datei verworfen |

Validierungsfehler: 400 DRF-Form (`{"areas": ["…"]}`, `{"terms": ["This field is required."]}`,
`non_field_errors` bei nichts zu tun). Archiv-PDF vorhanden und Begriffe gegeben → es geht als
`ocr_input` an das Kind (Suche und Schwärzen je Seite in Original **und** Archiv, Nachprüfung beider
Textebenen, siehe §3); keine Treffer in beiden → 400 „There is nothing to redact (no areas and no
matches).“ (früher „The search terms were not found; nothing was changed.“). Die neue Version wird neu
erkannt (OCR).

**Known difference / Grenze (2026-11-27):** die Nachprüfung erkennt die Ausgabe nicht selbst neu (kein
OCR im Kindprozess); für Scan-Seiten prüft sie gegen die OCR-Textebene des Archivs (Marken auf eine
Kopie angewandt). Was die ursprüngliche OCR nicht erkannt hat, findet auch die Nachprüfung nicht.

### 4.1 Geschwärzte Versionen beim Teilen (Stand 2026-11-19)

Ein Freigabelink (`documents_sharelink.document`) und ein Freigabe-Paket liefern genau das Dokument aus,
auf dessen id sie zeigen (upstream `SharedLinkView.get` → `serve_file(doc=share_link.document)`,
`build_share_link_bundle`). Ein Link auf das **Hauptdokument** zeigt also **Version 1** (das erste
Original), nicht die neueste, geschwärzte Version. Damit das niemand übersieht:

- **Merkliste** `paperrust_redaction` (`root_document_id`, `source_document_id`, `checksum` SHA-256,
  `created`, `actor_id`; Migration `pl_documents_write.0001_paperrust_redaction`, bei Bedarf angelegt,
  `crates/pl-documents-write/src/redaction_marks.rs`): `documents.redact` trägt nach erfolgreichem
  Werkzeuglauf die Prüfsumme der geschwärzten Datei ein, **bevor** es die Version einreiht. Fehler beim
  Eintragen brechen nicht ab (warn); die Version gilt dann als nicht geschwärzt (sichere Seite).
- **`versions[].redacted`** (bool) im Dokument-Serializer (`pl-documents/src/serializer.rs`
  `load_redacted`, nur abgefragt, wenn es Versionen gibt) und in der Antwort von
  `PATCH /api/documents/{root}/versions/{id}/`. Erkennung über die Prüfsumme, nicht über den änderbaren
  Versionsnamen.
- **`GET /api/documents/{id}/share_links/`** liefert beim Hauptdokument zusätzlich die Links auf seine
  Versionen (`document` = Versions-id).
- **Teilen-Dialog** (beide Oberflächen, `ShareLinksDialog.svelte`): Hinweis „Geschwärzt: Version n /
  Nicht geschwärzt (alles lesbar): Version m“, Versionsauswahl für neue Links (Vorauswahl: neueste
  geschwärzte Version, sonst Hauptdokument wie upstream), je Link „Version n · geschwärzt / nicht
  geschwärzt“.
- **Paket-Dialog** (`ShareLinkBundleDialog.svelte`): Hinweis „n der ausgewählten Dokumente haben
  geschwärzte Versionen“ und Schalter „Geschwärzte Version verwenden“ (Standard an → `document_ids`
  enthalten die neueste geschwärzte Versions-id statt der Root-id).

## 5. Einlesen (`pl-consumer`)

In `ConsumerPlugin.run` nach MIME-Erkennung und qpdf-Ersatz, für `application/pdf` bzw. `.pdf` mit
`application/octet-stream`:

1. Lesbarkeit mit lopdf (`pl_pdfops::pdf::load`): ok / verschlüsselt (nicht anfassen) / kaputt (öffnet
   nicht **oder** ohne Seiten – lopdf „öffnet“ vieles, findet dann aber keinen Seitenbaum).
2. Kaputt → `Op::Repair` in `<tmp>/repaired/<name>`; Ergebnis muss mit lopdf lesbar sein; dann ersetzt
   es die Arbeitskopie, MIME wird neu bestimmt, das **unveränderte Original** wird nach `<tmp>/uo/`
   kopiert und als Original gespeichert (Prüfsumme des Originals, wie beim qpdf-Ersatz). Log (info):
   `PDF could not be read; repaired it with PdfCraft and continuing with the repaired copy (…). The
   original file is kept.` Fehler → warn, weiter mit der Datei wie bisher.
3. `PAPERRUST_SHRINK_ON_INGEST_MB` > 0, Datei größer und Scan (< 50 Zeichen Text ohne Leerraum auf den
   ersten drei Seiten) → `Op::Shrink` mit `PAPERRUST_SHRINK_PRESET`; nur kleiner und lesbar ersetzt es
   die Arbeitskopie; dann wird die **verkleinerte** Datei gespeichert (`unmodified_original` verworfen).

Ohne installiertes Werkzeug: Schritte 2/3 protokollieren eine Warnung und entfallen.

## 6. Einstellungen (pl-settings, docs/SETTINGS.md §1.7)

Neu (§8): `PAPERRUST_OFFICE_TO_PDF` (A, bool, `true`), `PAPERRUST_OFFICE_TO_PDF_TIMEOUT` (A, int ≥ 0,
`0` = `PAPERRUST_PDFTOOL_TIMEOUT`). Zählung SETTINGS.md §1.19: A 157 → 159, Summe 352 → 354.

`PAPERRUST_SHRINK_ON_INGEST_MB` (A, 0), `PAPERRUST_SHRINK_PRESET` (A, standard),
`PAPERRUST_PDFTOOL_TIMEOUT` (A, 120), `PAPERRUST_PDFTOOL_MEMORY_MB` (A, 1536), `PAPERRUST_PDFTOOL_BIN`
(C, neben `paperrust`). Gelesen live über `pl_core::config::var`.

## 7. Tests

| Test | Was |
|---|---|
| `pl-pdftools` Unit | Auftragsformat, Stufen, Suchlogik, Rechteckhilfen, Allokator-Buchführung, Fehlertexte ohne Interna, fehlendes Programm |
| `pl-pdftools/tests/tools.rs` | echter Kindprozess: Suche mit Rechtecken; Schwärzen per Begriff (lopdf-Textextraktion **und** alle entpackten Ströme enthalten den Begriff nicht mehr, Rest bleibt, Metadaten weg, Original unverändert); Schwärzen per Bereich (nur der Bereich); ungültige Eingaben; Verkleinern (Bericht = Dateigrößen, > 50 % kleiner, Seite bleibt; verlustfrei rechnet nicht herunter); Reparatur kaputter Querverweise + Müll vorn |
| `pl-pdftools/tests/tools.rs` `repair_fixes_files_lopdf_cannot_read` | Datei mit Objektströmen und kaputtem Querverweis-Strom: lopdf scheitert, Reparatur stellt 2 Seiten + Text her |
| `pl-pdftools/tests/tools.rs` `redact_area_in_jpeg_scan_clears_only_that_area` | C-068: JPEG-Scan, Bereich über einem Wort → `images_cleared` 1, `images_removed` 0, Bild bleibt JPEG, unter der Marke schwarz, anderes Wort und Grund unverändert |
| `pl-pdftools/tests/tools.rs` `redact_with_remove_metadata_when_there_is_none` | C-066b: `remove_metadata` ohne vorhandene Metadaten → 200 statt „there are no redaction marks to apply“ |
| `pl-pdftools/tests/isolation.rs` | §2 |
| `pl-documents-write` Unit | Bereiche (1-basiert, Prüfung), Begriffe (Trim, Doppelte, Grenzen), Dateinamen der neuen Version |
| `pl-consumer` Unit | Verkleinern-Entscheidung, Lesbarkeit, Einlesen ohne Werkzeug läuft weiter |
| Light-UI (vitest) | Rechteckhilfen, Sammeln bestätigter Treffer, Größenformat, Fehlertexte |
| `pl-pdftools/tests/office.rs` | echter Kindprozess: docx → PDF (≥ 2 Seiten wegen Seitenumbruch, Text inkl. Tabelle per PdfCraft-Suche auffindbar); odt/rtf/md; kaputte .docx → `Failed` ohne Pfad; unbekanntes Format/fehlende Datei; **danach Schwärzen (Begriff weg, Rest bleibt) und Verkleinern auf dem Ergebnis**; synchroner Starter (Umwandlung, Zeitgrenze 2 s, `abort` → `Crashed`) |
| `pl-pdftools` Unit (§8) | `office_format`-Zuordnung, Auftrags-/Antwortform `office`/`converted`, synchroner Starter ohne Programm |
| `pl-consumer` Unit (§8.1) | Umwandelbare Typen, Tabelle → `Unsupported` (kein Fehler), Schalter |
| `pl-documents-write/tests/convert_image.rs` | Bild → PDF ohne DB: PNG, JPEG (unverändert eingebettet), mehrseitiges TIFF (3 Seiten), WebP, PNG mit Alpha, kaputtes Bild → Fehler |
| `pl-documents-write` Unit (§9) | Zuordnung Bild/Office/PDF/HEIC/Tabelle |
| Light-UI (vitest, §9) | `isConvertible`/`isImage`, Mehrfach-Umwandlung nacheinander mit Fehlersammlung |

Starten: `cargo test -p pl-pdftools --features engine -j 2` (baut `paperrust-pdftool`).

**E2E (2026-11-13, von Hand, Wegwerf-DB auf der Referenz-PostgreSQL, Debug-Build, Server mit Worker):**
Einlesen von Text-PDF, 5-MB-Scan und kaputter Objektstrom-PDF; Reparatur beim Einlesen (Inhalt
gelesen, gespeichertes Original = angelieferte Datei, Prüfsumme gleich); Suche (2 Treffer); Schwärzen
per Begriff + Metadaten (neue Version „Redacted“, Inhalt ohne Begriff, Datei ohne Begriff/Namen, alte
Version unverändert); anonym 401, ohne Änderungsrecht 403, falsche Seite 400; Verkleinern 5,1 MB →
274 KB als neue Version, falsche Stufe 400; Verkleinern beim Einlesen (`PAPERRUST_SHRINK_ON_INGEST_MB=2`:
Scan als 274 KB gespeichert, kleines Text-PDF unverändert). Light-UI im Browser: Dialog zeigt Seiten,
Rechteck ziehen, Begriffssuche markiert Treffer, „Endgültig schwärzen“ → neue Version mit Balken, Inhalt
ohne „GEHEIM“ und ohne „4711“; Verkleinern-Dialog „Stark“ 267,7 → 155,7 KB (−42 %). Skripte nicht im Repo.

## 8. Office → PDF mit WordCraft (`Op::Office`, Funktion `office`)

Quelle: WordCraft (https://github.com/storytold/wordcraft, MIT OR Apache-2.0), Revision
`a4b898f820eb28626865292fd037a155e4a07aef`, als Git-Abhängigkeit (nicht kopiert). Benutzt werden nur
`wordcraft-doc` (Dokumentmodell), `wordcraft-docx` (`read`), `wordcraft-formats` (`import_on`: ODT,
RTF, Markdown, HTML) und `wordcraft-pdf` (`export`, mit Layout aus `wordcraft-layout`, Schriften aus
`wordcraft-fonts`, PDF-Schreiber `craft-pdf`/krilla aus craft-libs). Keine egui-Oberfläche, kein
Branding. Die Funktion `office` ist Teil von `engine` (Paketierung bleibt `--features engine`).
Wurzel-`Cargo.toml`: `[patch.crates-io] krilla` → craft-libs-Revision `6e03f65…` (wie im
WordCraft-Workspace; ohne baut `craft-pdf` nicht).

Auftrag `{"op": "office", "input", "output", "format"}`; `format` = Endung ohne Punkt, nur aus
`OFFICE_FORMATS`:

| MIME-Typ | Format |
|---|---|
| `application/vnd.openxmlformats-officedocument.wordprocessingml.document` | `docx` |
| `application/vnd.ms-word.document.macroenabled.12` | `docm` |
| `application/vnd.openxmlformats-officedocument.wordprocessingml.template` | `dotx` |
| `application/vnd.oasis.opendocument.text` | `odt` |
| `application/vnd.oasis.opendocument.text-template` | `ott` |
| `text/rtf`, `application/rtf` | `rtf` |
| `text/markdown` | `md` |
| `text/html` | `html` |

Zusätzlich beim **Einlesen** und bei „In PDF umwandeln“ (`DOCUMENT_FORMATS`, `job::document_format`):

| MIME-Typ | Format | Umsetzung |
|---|---|---|
| `application/msword` | `doc` | `wordcraft-docbin` (craft-libs `craft-doc-binary`, Rev. `75c70fa…`), verschlüsselt → `The doc file is password protected.` |
| `application/vnd.ms-excel` | `xls` | `wordcraft_formats::sheet` (BIFF8) |
| `…spreadsheetml.sheet` / `application/vnd.ms-excel.sheet.macroenabled.12` | `xlsx` / `xlsm` | `wordcraft_formats::sheet` |
| `application/vnd.oasis.opendocument.spreadsheet` | `ods` | `wordcraft_formats::sheet` |
| (nur Endung) | `mht` | `wordcraft_formats::mht` – Webarchiv des Mail-Parsers (HTML-Teil + `cid:`-Bilder) |

Tabellen (`office::sheet_html`): je Blatt Überschrift (Blattname) und Tabelle mit Rahmen (8 pt, Zahlen
rechtsbündig, Werte wie WordCraft sie liest: Datumsformate als ISO-Datum), ab dem zweiten Blatt neue
Seite, leere Blätter `(empty)`; A4 mit ½-Zoll-Rand, quer ab 7 Spalten; höchstens 200 000 Zellen, danach
`(n more rows not shown)`.

Weitere Aufträge:
- `{"op": "slides", "input", "output"}` – vom Server gebaute Folien-HTML (pl-parsers
  `office::slides_html`) auf A4 quer (½-Zoll-Rand); `format` der Antwort `slides`.
- `{"op": "heic", "input", "output", "max_pixels"}` – HEIC/HEIF → PNG mit craft-heif (craft-libs
  Rev. `6e03f65…`, heic-rs `=0.1.4`, reines Rust ohne `unsafe`/C): Container-Drehung/-Spiegelung/
  -Zuschnitt angewandt, ICC-Profil übernommen, 8 Bit RGB(A); `max_pixels` 0 = 178 956 970; Fehler
  `The HEIC image could not be decoded: …`. Funktion `heif` (Teil von `engine`).

Antwort `{"status": "converted", "format", "size_before", "size_after"}` oder `failed` mit Nutzertext
ohne Pfade (`The docx file could not be read: …` ≤ 200 Zeichen Leserfehler, `The format "x" cannot be
converted.`, `The file is too large to convert.` > 256 MiB, `The PDF could not be written: …`).
Markdown/HTML ohne eigene Seitengröße: A4. Schriften: eingebaut Source Sans 3/Source Serif 4/Inter/
JetBrains Mono (OFL), sonst Systemschriften (das Kind behält dafür `WINDIR`, `LOCALAPPDATA`, `HOME`,
`XDG_DATA_HOME`, `XDG_DATA_DIRS` – keine Geheimnisse); fehlende Word-Schriften → metrischer Ersatz.

Zeitgrenze: `PAPERRUST_OFFICE_TO_PDF_TIMEOUT` (s, 0 = `PAPERRUST_PDFTOOL_TIMEOUT`), Speicher wie §2.
Isolation wie §2 (gleicher Kindprozess). Für synchrone Aufrufer (Parser in `spawn_blocking`) gibt es
`runner::run_blocking[_with]` mit `std::process`, Hilfsthreads, `try_wait`-Schleife und `kill` nach der
Zeitgrenze – gleiche Umgebung, gleiche Ausgabegrenze, gleiche Fehlerarten.

### 8.1 Beim Einlesen (`pl-consumer::office`, `pl-parsers`)

`NativeParsing::new` setzt mit `ParserRegistry::set_office_converter` den `WordCraftConverter` als
`PdfConverter` des Office-Parsers (`TikaDocumentParser`), wenn `PAPERRUST_OFFICE_TO_PDF` (Standard
`true`) und `paperrust-pdftool` gefunden. Dann (upstream-Verhalten mit Gotenberg):
`requires_pdf_rendition() = true`, `parse` liefert `archive_path` = WordCraft-PDF, Vorschaubild =
erste Seite des Archiv-PDFs (PDFium, `make_thumbnail_from_pdf`), Seitenzahl aus dem Archiv-PDF.
Seit den Lücken E9/E10/E12 auch: altes Word (`.doc`), Tabellen (`xlsx`, `xlsm`, `xls`, `ods` →
`Op::Office`), Präsentationen/Zeichnungen (`pptx`, `ppsx`, `ppt`, `odp`, `odg` → Folien über pl-parsers
`office::slides` + `slides_html`, dann `Op::Slides`) und E-Mails (`set_mail_converter`: Kopfseite als
`text/html`, HTML-Teil als `multipart/related` → `mht`). Ausgabe `<stem>.office-convert.pdf` neben der
Eingabe. HEIC (Lücke E8): `NativeParsing` wandelt `image/heic` vor OCR und Vorschaubild mit
`office::heic_to_png_blocking` (`Op::Heic`) in `heic-decoded.png` um; ohne Werkzeug bleibt es beim
bisherigen Fehler. **Fehler** (kaputte Datei, Zeitgrenze, Absturz): Warnung `Office to PDF conversion failed,
continuing without archive PDF: WordCraft could not convert <name>: <Grund>` (Ziel
`paperless.parsing.tika`), Einlesen läuft weiter wie ohne Erzeuger (Text aus dem eigenen
Office-Parser, Text-Vorschaubild, kein Archiv). Text des Dokuments kommt weiter aus dem Office-Parser
(nicht aus dem PDF).

### 8.2 Treueprüfung (2026-11-14, von Hand)

Beispiele `tests/fixtures/office/` plus eine Datei mit Kopf-/Fußzeile, verbundenen Zellen, Schriftgrößen,
Ausrichtung und 60 Absätzen; gerendert mit PyMuPDF (60 dpi) und angesehen. In Ordnung: Überschriften
(Farbe/Größe), Tabelle mit Rahmen, verbundene Zelle, Aufzählung und Nummerierung, PNG-Bild, harter und
automatischer Seitenumbruch (2 bzw. 3 Seiten), Kopf-/Fußzeile auf jeder Seite, fett/kursiv/Größe,
rechts/zentriert, Umlaute und € (auch in der Textebene: PyMuPDF und PdfCraft-Suche lesen sie richtig),
Markdown-Tabelle, RTF-Kursiv und `\page`. Lücken: Calibri → Ersatzschrift (andere Optik); ODT ohne
Seitenvorlage ohne Ränder; nicht geprüft Fußnoten, Textfelder, Diagramme, OLE, Formulare, RTL;
Änderungsverfolgung = Endfassung, Kommentare fehlen. Größen: docx 38 KB → 47 KB, rtf 235 B → 55 KB
(eingebettete Schrift-Teilmengen).

## 9. In PDF umwandeln (`pl-documents-write::convert`)

| id | Methode, Pfad | Request (JSON) | Antwort 200 | Nebenwirkung |
|---|---|---|---|---|
| `documents.convert_pdf` | POST `/api/documents/{id}/convert_pdf/` | `ocr` (bool, Standard `false`, nur Bilder), `version_label` (≤ 64, Standard `PDF`) | `task_id`, `document`, `source_version`, `kind` (`image`/`office`), `ocr` (tatsächlich OCR gelaufen) | PDF in SCRATCH_DIR, `consume_file` mit `root_document_id` (neue Version), wie §4 |

Rechte, 404, Quelle (neueste Version), Pfadprüfung und Werkzeugfehler wie §4 (gemeinsamer Code
`target_any`). Ablauf:

1. Neueste Version `application/pdf` → 400 `This document is already a PDF.`; weder Bild (auch HEIC)
   noch `job::document_format` (`OFFICE_FORMATS` + `DOCUMENT_FORMATS`: auch .doc und Tabellen) → 400
   `This document cannot be converted to PDF.`
2. **Bild** (im Serverprozess, `spawn_blocking`): HEIC/HEIF erst im Kindprozess nach PNG (`Op::Heic`,
   wie beim Einlesen; Fehler → 400 `The image could not be converted: …`), dann Pillow-Bombenprüfung (`PAPERLESS_MAX_IMAGE_PIXELS`),
   Alpha → auf Weiß gelegt (erste Seite, PNG); ohne OCR `NativeOcrmypdf::image_to_pdfa` (= upstream
   `_convert_image_to_pdfa`/img2pdf: JPEG unverändert, TIFF je Seite eine PDF-Seite, DPI aus
   `OCR_IMAGE_DPI` bzw. Bild bzw. 96, PDF/A-2b-Stempel); mit OCR `parse_document(…, produce_archive =
   true)` mit der OCR-Konfiguration wie beim Einlesen (Umgebung + `paperless_applicationconfiguration`),
   `OCR_MODE=off` wird dafür zu `auto` → Archiv-PDF mit Textebene. Keine OCR-Engine → 503 `Text
   recognition (OCR) is not available on this server. Convert without OCR.` Fehler → 400 `The image
   could not be converted: …`.
3. **Office**: `PAPERRUST_OFFICE_TO_PDF=false` → 503 `Office conversion is switched off on this
   server.`; sonst `Op::Office` im Kindprozess (§8), Fehler wie §4 (`Failed` 400, nicht installiert
   503, Zeitgrenze 504, Absturz 422).
4. Neue Version einreihen (`queue_version`, Bezeichnung `PDF`). Die alte Version bleibt. Danach ist
   die neueste Version ein PDF → Schwärzen/Verkleinern (§4) gehen.

Light-UI: `common/convert-pdf/ConvertPdfDialog.svelte` (Detailansicht: Aktionen → „In PDF
umwandeln…“, sichtbar nur für umwandelbare Typen, OCR-Haken nur bei Bildern; Dokumentliste:
Aktionen → „In PDF umwandeln…“ für die Auswahl, nicht bei „alle auswählen“, Dokumente nacheinander,
Zusammenfassung umgewandelt/nicht umgewandelt mit Grund).

### 9.1 E2E (2026-11-14, von Hand, Wegwerf-DB auf der Referenz-PostgreSQL, Debug-Build, Server mit Worker, Skripte nicht im Repo)

26/26 Prüfungen: docx einlesen → Archiv-PDF (47 KB), `page_count` 2, Vorschaubild von Seite 1 (24 KB statt
Text-Bild), Inhalt aus dem Office-Parser; rtf einlesen mit WordCraft; abgeschnittene .docx scheitert
schon im eigenen Office-Textleser (wie bisher, vor WordCraft); **WordCraft-Fehler beim Einlesen**
(Werkzeug durch ein Programm mit Müll-Antwort ersetzt): Warnung „Office to PDF conversion failed,
continuing without archive PDF: … protocol error …“, Einlesen erfolgreich ohne Archiv. `convert_pdf`:
anonym 401, ohne Änderungsrecht 403, unbekannt 404; PNG mit OCR, 3-seitiges TIFF ohne OCR, gespeicherte
RTF und DOCX → je neue Version „PDF“, alte Version bleibt (`versions` 2 Einträge); zweiter Aufruf 400
„already a PDF“; danach Suche (OCR-Text des PNG gefunden), Schwärzen (neue Version „Redacted“) auf dem
umgewandelten Bild, Verkleinern und Schwärzen auf der umgewandelten RTF (Begriff aus dem Inhalt weg).
Light-UI im Browser: Detailansicht Bild → Aktionen → „In PDF umwandeln…“ (Schwärzen/Verkleinern
ausgegraut) → Umwandeln → „Umgewandelt: 1“; nach Neuladen „Versionen 2“, Umwandeln-Eintrag weg,
Schwärzen/Verkleinern aktiv. Dokumentliste: Seite auswählen → Aktionen → „In PDF umwandeln…“ → 1
umgewandelt (ODT), 5 übersprungen mit „This document is already a PDF.“

**Nebenbefund behoben (pl-consumer):** Seit Datenmodell-Stufe 1 ist `documents_document` eine Sicht;
`Consumer::create_version` sperrte mit `SELECT … FOR UPDATE` auf der Sicht → **jede neue Version
scheiterte** („FOR UPDATE cannot be applied to the nullable side of an outer join“), auch Schwärzen und
Verkleinern. Jetzt Sperre auf `paperrust_document`, wenn vorhanden (gleiche Regel wie
`pl_store::datamodel::document_table`).

## Known differences / Grenzen

- **Teilen geschwärzter Dokumente (§4.1):** upstream kennt keine Schwärzung; die Liste
  `documents/{id}/share_links/` enthält beim Hauptdokument auch Links auf Versionen, `versions[]` hat das
  Zusatzfeld `redacted`, der Teilen-Dialog wählt bei geschwärzten Dokumenten die geschwärzte Version vor.
  Ohne Schwärzung verhält sich alles wie upstream (Link/Paket auf das Hauptdokument).

- Kein upstream-Gegenstück; die Routen sind PaperRust-Erweiterungen (nicht in der upstream-OpenAPI).
- Suche findet Begriffe nur innerhalb einer Textzeile; geschwärzt werden ganze Wörter (lieber mehr).
- Schwärzen in JPEG-Scans: das geschwärzte Bild wird neu als JPEG gespeichert (einmal mehr verlustbehaftet
  komprimiert, Qualität 90); CMYK-JPEG, JPEG 2000 und Bilder mit Masken entfernt PdfCraft unter einer
  Marke weiterhin ganz.
- Senkrechter Text unter einer Marke → PdfCraft lehnt ab (`VerticalText`), statt unsicher zu schwärzen.
- Die alte Version bleibt im Versionsverlauf (gewollt: Original bewahren). Der Nutzer muss sie selbst
  löschen, wenn auch sie weg muss (Doku und Dialog sagen das).
- Verkleinern beim Einlesen speichert die verkleinerte Datei als Original; deren Prüfsumme weicht von
  der angelieferten Datei ab → dieselbe Datei erneut eingelesen wird **nicht** als Duplikat erkannt.
- Scan-Erkennung beim Einlesen ist eine Faustregel (< 50 Zeichen Text auf den ersten drei Seiten).
- PdfCraft ist Alpha; Absicherung nur über Isolation (§2), keine eigene Fuzzing-Kampagne hier.
- Paketierung: `paperrust-pdftool` liegt neben `paperrust` – Docker (`docker/Dockerfile`, nach
  `/usr/local/bin`), Desktop (`apps/desktop/pack/pack.ps1`/`pack.sh`, Pflicht wie `paperrust`), CI
  (`extended.yml` baut es in beiden Desktop-Jobs; `ci.yml` testet `pl-pdftools --features engine`).
- Fehlt das Werkzeug (nicht gefunden oder `PAPERRUST_PDFTOOL_BIN` zeigt ins Leere): API 503 mit
  „The PDF tools are not installed on this server (paperrust-pdftool is missing) …“ (ohne Pfade);
  Einlesen versucht nichts, warnt mit Suchort und Abhilfe (`ToolLocation::hint`) und läuft mit der Datei
  wie angeliefert weiter (`IngestOutcome::skipped_no_tool`). Tests: `runner::tests::locate_reports_where_it_looked`,
  `missing_tool_is_not_installed_with_clear_message`, `pl-consumer` `missing_tool_skips_repair_and_keeps_file`,
  `pl-documents-write` `missing_tool_is_503_with_clear_message`.
- **Office beim Einlesen (§8.1) ist eine Abweichung von PaperRust ohne Gotenberg** (bisher: kein
  Archiv, Text-Vorschaubild, pl-consumer Known difference 6): jetzt wie upstream mit Gotenberg ein
  Archiv-PDF – aber von WordCraft gesetzt, nicht von LibreOffice; Layout weicht ab (siehe
  docs/user/pdf-tools.md#office-grenzen). Archiv-Prüfsumme/Seitenzahl unterscheiden sich daher von
  einer ngx-Installation mit Gotenberg.
- In PDF umwandeln: Bilder mit Transparenz → nur die erste Seite (mehrseitige TIFF mit Alpha sind
  selten); HEIC nicht (pl-documents-write nutzt `Op::Heic` noch nicht). `.doc`/Tabellen nur beim
  Einlesen (`document_format`); „In PDF umwandeln“ nutzt weiter `office_format` (pl-documents-write
  kann auf `document_format` umstellen).
- Tabellen-PDF: eine Tabelle je Blatt, Spalten auf Seitenbreite gestaucht (kein Umbruch über mehrere
  Seiten nach rechts wie LibreOffice), keine Zellformate/Farben/Diagramme, Zahlen wie in der Datei
  gespeichert (bis 15 Stellen). Folien-PDF: Text je Folie (siehe pl-parsers Known difference 17).
- HEIC: 4:2:2/4:4:4 (manche Canon/Sony „HIF“) und Bildfolgen lehnt craft-heif ab; Tiefen-/HDR-
  Zusatzbilder werden ignoriert.
- Die Bildumwandlung läuft im Serverprozess (wie das Einlesen von Bildern), nicht im Kindprozess.
- Größe: `paperrust-pdftool` Debug 16,6 MB → 52,7 MB mit WordCraft; Release-Größe nicht gemessen
  (Release-Builds sind auf dem geteilten Rechner untersagt). 40 neue Pakete im Lockfile, doppelt
  vorhanden u. a. skrifa (0.42/0.44/0.47), read-fonts (0.39–0.44), harfrust (0.12/0.13), zip (3/9).
- Text eines eingelesenen Office-Dokuments kommt weiter aus dem eigenen Office-Parser; scheitert der
  (z. B. abgeschnittenes XML), scheitert das Einlesen wie bisher – auch wenn WordCraft die Datei
  vielleicht hätte setzen können (wird nicht versucht).
