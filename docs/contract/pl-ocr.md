# Vertrag: pl-ocr (Ersatz für OCRmyPDF im Python-freien Worker)

Quellen (upstream v3.4.2, unverändert): `src/paperless/parsers/tesseract.py` (`RasterisedDocumentParser`),
`src/paperless/config.py` (`OcrConfig`, `RemoteOCRConfig`), `src/paperless/models.py` (Choices,
`ApplicationConfiguration`), `src/paperless/settings/__init__.py` (`OCR_*`), `src/paperless/parsers/utils.py`,
`src/documents/consumer.py::should_produce_archive`, `src/documents/parsers.py` (Thumbnail),
`src/documents/utils.py` (`maybe_override_pixel_limit`, `remove_alpha_channel`),
`src/paperless/parsers/remote.py`. OCRmyPDF-Verhalten aus OCRmyPDF 17.12.1 (`reference/venv`):
`_pipeline.py` (`is_ocr_required`, `get_page_square_dpi`, `validate_pdfinfo_options`, `merge_sidecars`,
`triage_image_file`), `pdfinfo/info.py`, `exceptions.py`, `_options.py`.

Kein HTTP-Endpunkt: Bibliothek für den Worker (`docs/STANDALONE.md`). Keine DB-Zugriffe; die
`ApplicationConfiguration`-Zeile übergibt der Aufrufer als `AppConfigOverrides`.

## 1. Einstellungen

v3.4.2 kennt **auto/force/redo/off** (nicht mehr skip/skip_noarchive). `PAPERLESS_OCR_MODE=skip` oder
`skip_noarchive` wird beim Start still auf `auto` abgebildet; jeder andere ungültige Wert →
`ValueError: Environment variable 'PAPERLESS_OCR_MODE' has invalid value '…'. Valid choices are: auto, force, off, redo`.
Archiv-Steuerung ist eigenständig: `PAPERLESS_ARCHIVE_FILE_GENERATION` = auto/always/never.

| Umgebung | Standard | Prüfung | DB-Feld (`paperless_applicationconfiguration`) | Auflösung in `OcrConfig()` |
|---|---|---|---|---|
| `PAPERLESS_OCR_OUTPUT_TYPE` | `pdfa` | erst in `OcrConfig()` (`OutputTypeChoices(…)`) | `output_type` | `db or env` |
| `PAPERLESS_OCR_PAGES` | `None` | `int()` beim Start | `pages` | `db or env` (0 → env) |
| `PAPERLESS_OCR_LANGUAGE` | `eng` | – | `language` | `db or env` ("" → env) |
| `PAPERLESS_OCR_MODE` | `auto` | Start (s. o.) | `mode` | `db or env` |
| `PAPERLESS_ARCHIVE_FILE_GENERATION` | `auto` | Start | `archive_file_generation` | `db or env` |
| `PAPERLESS_OCR_IMAGE_DPI` | `None` | `int()` | `image_dpi` | `db or env` |
| `PAPERLESS_OCR_CLEAN` | `clean` | `CleanChoices(…)` in `OcrConfig()` | `unpaper_clean` | `db or env` |
| `PAPERLESS_OCR_DESKEW` | `true` | `str_to_bool` | `deskew` | `db if not None else env` |
| `PAPERLESS_OCR_ROTATE_PAGES` | `true` | `str_to_bool` | `rotate_pages` | `db if not None else env` |
| `PAPERLESS_OCR_ROTATE_PAGES_THRESHOLD` | `12.0` | `float()` | `rotate_pages_threshold` | `db or env` (**0.0 in der DB → env**) |
| `PAPERLESS_OCR_MAX_IMAGE_PIXELS` | `None` | `int()` | `max_image_pixels` (float) | `db or env` (0.0 in der DB → env) |
| `PAPERLESS_OCR_COLOR_CONVERSION_STRATEGY` | `RGB` | `ColorConvertChoices(…)` | `color_conversion_strategy` | `db or env` |
| `PAPERLESS_OCR_USER_ARGS` | `None` | JSON; Fehler → `{}` | `user_args` (JSON) | `db if truthy else json(env)` |
| `PAPERLESS_MAX_IMAGE_PIXELS` | `None` | `int()` | – | Pillow-Grenze in paperless (`maybe_override_pixel_limit`: 0 → aus, <0/None → Pillow-Standard 89 478 485) |
| `PAPERLESS_THREADS_PER_WORKER` | upstream `max(cores // PAPERLESS_TASK_WORKERS, 1)`; PaperRust (2026-11-23): unset = **the whole page pool per document, shared** – pool `min(cores, memory cap)` (Known difference 23, `pl_core::resources::ocr_threads_per_document`), for every `TASK_WORKERS` value (unset = `auto`, `auto`, number); the value the startup memory cap writes is the pool size itself and is used as is (only the pool size ignores it, `threads_written_by_cap`); `TASK_WORKERS` is still validated (unset/`auto` or `int()` error text unchanged) | `int()` | – | → `jobs` (per-document upper bound) |
| `PAPERLESS_REMOTE_OCR_ENGINE/_API_KEY/_ENDPOINT` | `None` | – | `remote_ocr_*` | `db or env` |
| `PAPERLESS_REMOTE_OCR_MODE` | `always` | Start (always/workflow_only) | `remote_ocr_mode` | `db or env` |
| `PAPERLESS_REMOTE_OCR_ALLOW_INTERNAL_ENDPOINTS` | `true` | bool | – | nur env |

Fehlertexte: `'x' is not a valid OutputTypeChoices` (bzw. `CleanChoices`, `ColorConvertChoices`),
`Cannot convert 'x' to a boolean.`. Rust: `config::{OcrSettings::from_env, OcrConfig::new}`.

Neue eigene Variablen (nur Werkzeugpfade): `PAPERRUST_TESSERACT_PATH`, `PAPERRUST_TESSDATA_DIR`,
`PAPERRUST_PDFIUM_PATH` (Datei oder Verzeichnis), `PAPERRUST_TESSERACT_LIB` (libtesseract: Datei oder
Verzeichnis; `off` = immer das Programm; fehlt: neben dem Programm bzw. System-`libtesseract.so.5`, siehe §8).

## 2. `construct_ocrmypdf_parameters` (Schlüssel in dieser Reihenfolge)

1. `input_file_or_options`, `output_file`, `use_threads=True`, `jobs=THREADS_PER_WORKER`, `language`,
   `output_type`, `progress_bar=False`
2. `color_conversion_strategy` nur wenn `"pdfa" in output_type`
3. genau eins: `force_ocr` (safe_fallback **oder** mode=force) › `redo_ocr` (mode=redo) ›
   `skip_text` (skip_text-Parameter **oder** mode=off) › nichts (auto)
4. clean=`clean` → `clean`; clean=`clean-final` → bei redo `clean`, sonst `clean_final`; `none` → nichts
5. `deskew` wenn deskew und mode ≠ redo
6. rotate → `rotate_pages=True`, `rotate_pages_threshold`
7. `pages="1-N"` wenn pages > 0, **sonst** `sidecar` (beides schließt sich aus)
8. nur Bilder: `get_dpi`, `calculate_a4_dpi` werden **vor** `has_alpha` ermittelt; Alpha →
   `input_file_or_options = <tempdir>/image-no-alpha`; `image_dpi` = Bild-DPI (≠0) › `OCR_IMAGE_DPI` ›
   A4-Schätzung (≠0) › `ParseError("Cannot produce archive PDF for image {path}, no DPI information is present in this image and OCR_IMAGE_DPI is not set.")`; < 70 → Warnung
9. `user_args` (`{**args, **user_args}`: vorhandene Schlüssel behalten ihre Position; Nicht-Dict →
   Warnung `… Error: '<typ>' object is not a mapping`, ignoriert)
10. `max_image_mpixels = max_image_pixel / 1e6`, wenn ≥ 0 (überschreibt auch user_args)

Rust: `args::construct_ocrmypdf_parameters` → `OcrArgs` (geordnete JSON-Map). Parität: 210 Fälle
(21 Konfigurationen × PDF/Bild, safe_fallback, skip_text) **0 Abweichungen**.

## 3. `parse(document, mime, produce_archive)` (Rust: `parser::RasterisedParser::parse`)

Vorab nur für PDFs: `text_original = extract_text(None, doc)` (pdftotext → `post_process_text`),
`original_has_text = is_born_digital_text` (Text nicht leer **und** (getaggt **oder** > 50 Zeichen)).

| Modus | produce_archive | Bedingung | Ablauf |
|---|---|---|---|
| off | nein | – | Text = Original-Text (Bilder: ""), kein Archiv, kein OCR |
| off | ja | Bild | `_convert_image_to_pdfa` (img2pdf + PDF/A-2b-Stempel); Text ""; Fehler → `ParseError("Image to PDF/A conversion failed: …")` |
| off | ja | PDF | `_convert_pdf_to_pdfa` (Ghostscript, `pdf` → Kopie); Text = Original |
| auto | nein | Text vorhanden | kein OCR, Text = Original |
| auto | ja | Text vorhanden | `ocrmypdf.ocr(skip_text)` |
| auto | * | kein Text / Bild | `ocrmypdf.ocr()` ohne Flag |
| force / redo | * | – | `ocrmypdf.ocr(force_ocr / redo_ocr)` |

Nach `ocr()`: `archive_path` nur wenn produce_archive; Text = `extract_text(sidecar, archive)`; leer →
`NoTextFoundException`. Ausnahmen:
- `DigitalSignatureError`/`EncryptedPdfError` → Warnung, Text = Original (falls has_text), **kein Archiv**
- `SubprocessOutputError` → `ParseError("SubprocessOutputError: {e}. See logs for more information.")` (+ Ghostscript-Hinweis)
- `NoTextFoundException`/`InputFileError` (inkl. `TaggedPDFError`)/`PriorOcrFoundError` → zweiter Lauf mit
  `force_ocr`, Dateien `archive-fallback.pdf`/`sidecar-fallback.txt`; Fehler darin → `ParseError("{Klasse}: {e}")`
- sonst → `ParseError("{Klasse}: {e}")`
- am Ende: Text leer → Original (falls has_text), sonst "" + Warnung.

`extract_text(sidecar, pdf)`: Sidecar nur wenn vorhanden, Modus ≠ redo (Sidecar enthält dann nur neuen
Text) und **ohne** `[OCR skipped on page`; sonst pdftotext des Archivs (fehlt die Datei → `None`).

Parität: 82 Abläufe (alle Modi × Archiv × Textlagen, alle Ausnahmeklassen erste/zweite Stufe, Bilder,
img2pdf-/Ghostscript-Fehler) – Text, Archivpfad, Fehlertext, **jede** `ocr()`-Argumentliste und jeder
`extract_text`-Aufruf: **0 Abweichungen**.

## 4. Archiv-Entscheidung (`should_produce_archive`)

`requires_pdf_rendition` → ja; `not can_produce_archive` → nein; `always` → ja; `never` → nein;
auto: `image/*` → ja, PDF → nein wenn born-digital, sonst ja; andere → nein.
Rust: `parser::should_produce_archive` (96 Kombinationen gegen upstream: 0 Abweichungen).
Tesseract-Parser: `can_produce_archive=True`, `requires_pdf_rendition=False`, `score=10` für
`application/pdf, image/jpeg, png, tiff, gif, bmp, webp, heic`.

## 5. Bilder

Pillow-Semantik (Rust `imageinfo`, ohne Pixel zu dekodieren): `get_dpi = round(info["dpi"][0])`
(Banker's rounding), `calculate_a4_dpi = int(width / (21/2.54))`, `has_alpha = mode in ("RGBA","LA")`.
Details je Format im Modulkopf (PNG nur pHYs-Einheit Meter; JPEG JFIF 1/2, sonst EXIF, EXIF ohne
Auflösung → 72; TIFF ohne Auflösungstags → **1 dpi**; BMP immer px/m; HEIC `ispe`+`clap`+`irot`).
Parität: 43 Bilder (31 erzeugt, 12 upstream-Beispiele) – dpi, A4-dpi, Alpha, Größe: 0 Abweichungen.
`remove_alpha_channel` verwirft Alpha (kein Verrechnen), alle Einzelbilder.
Pixelgrenze: paperless `MAX_IMAGE_PIXELS` für eigene Pillow-Aufrufe (Fehler ab 2× Grenze:
`Image size (N pixels) exceeds limit of M pixels, could be decompression bomb DOS attack.`;
`get_dpi`/`calculate_a4_dpi` schlucken ihn, `has_alpha` nicht); OCRmyPDF `max_image_mpixels`
(Standard 250, 0 = aus) für das OCR-Werkzeug.

## 6. Seitenzahl, Thumbnail, Metadaten

- `get_page_count`: nur PDF (lopdf; unlesbar/verschlüsselt → `None`), Bilder `None` → `pdfwrite::page_count`.
- `get_thumbnail` = `make_thumbnail_from_pdf(archive or original)`: dpi = min(300, 500·72/w, 5000·72/h),
  2× Überabtastung, Lanczos auf ≤ 500 px Breite, WebP → `thumbnail::make_thumbnail_from_pdf` (PDFium).
  Vor dem (verlustfreien) Kodieren 32 Stufen je Kanal (`thumbnail::posterize`, auch für
  `make_thumbnail_from_image`): Vorschaubilder ~halb so groß (lokal 42 → 20 KiB), Grau bleibt Grau, 0/255 bleiben.
  Fehler: upstream qpdf-Reparatur und dann `documents/resources/document.webp`; hier Fehler an den Aufrufer.
- `extract_metadata` (XMP über pikepdf): **nicht hier**, bereits in `pl-parsers::pdf::extract_pdf_metadata`.

## 7. Remote-OCR (`RemoteDocumentParser`)

`score` 20 (vor Tesseract), wenn `engine == "azureai"` und api_key und endpoint gesetzt; Typen ohne HEIC.
`parse`: ungültige Engine → Text "" (Warnung); PDF ohne Archivwunsch → lokaler Text (pdftotext); sonst
Azure Document Intelligence `prebuilt-read` (Text + durchsuchbares PDF als Archiv), Endpunkt mit
`validate_outbound_http_url` (auch nach Umleitungen), Fehler → `ParseError("Azure AI Vision parsing failed: …")`.
`remote_ocr_mode=workflow_only` → nur wenn Workflow/Benutzer es anfordert.
Rust: Konfiguration/Entscheidung (`config::RemoteOcrConfig`, `parser::remote_decision`) und
**`remote::parse(cfg, tempdir, dokument, mime, produce_archive, extract_pdf_text)`** → `RemoteOutcome
{ text, archive_path }` (Quelle `paperless/parsers/remote.py::RemoteDocumentParser.parse`,
`_azure_ai_vision_parse`; `paperless/network.py::validate_outbound_http_url`, `is_public_ip`).
REST statt Python-SDK (`azure-ai-documentintelligence` 1.0, API-Version `2024-11-30`):
1. `POST {endpoint}/documentintelligence/documentModels/prebuilt-read:analyze?api-version=2024-11-30&outputContentFormat=text&output=pdf`,
   Kopf `Ocp-Apim-Subscription-Key`, Rumpf `{"base64Source": …}` (= `AnalyzeDocumentRequest(bytes_source)`,
   `output_content_format=TEXT`, `output=[PDF]`) → 202 + `Operation-Location`;
2. `GET Operation-Location` bis `status=succeeded` (Wartezeit `Retry-After`, sonst 1 s; höchstens 30 min),
   `failed/canceled` → Fehler; Text = `analyzeResult.content`;
3. `GET …/analyzeResults/{resultId}/pdf?api-version=…` → `tempdir/archive.pdf`.
Fehler: ungültiger Endpunkt → `ParseError("Invalid remote OCR endpoint: <ValueError-Text>")` vor jeder
Anfrage; sonst `ParseError("Azure AI Vision parsing failed: (Code) Message")` (Azure-Fehler-JSON) bzw.
Transportfehler ohne URL. Der Schlüssel steht in keinem Log/Fehlertext. Jede Folgeanfrage
(Abfrage-URL, Weiterleitungen) wird erneut mit `validate_outbound_http_url` geprüft; Weiterleitungen
und Abfrage-URLs auf einen anderen Host werden abgelehnt (upstream: Schlüssel-Kopf in
`blocked_redirect_headers`, Ziel erneut validiert). HTTP-Aufruf in eigenem Thread mit eigener
Tokio-Laufzeit (aus synchronem und asynchronem Kontext aufrufbar).
**Nicht hier** (andere Crates): Registrierung als Parser mit `score` 20 (pl-parsers), Laden der
AC-Spalten `remote_ocr_*` und `remote_ocr_mode=workflow_only`/Workflow-Aktion/`reprocess remote_ocr`
(pl-consumer, pl-workflows). Tests: `tests/remote_azure.rs` (lokaler Mock-Server).

## 8. Architektur in Rust

```
RasterisedParser (Steuerlogik 1:1)
   └─ OcrBackend  ── NativeOcrmypdf (ersetzt ocrmypdf.ocr, img2pdf+pikepdf, Ghostscript, pdftotext)
                       ├─ OcrEngine    ── TesseractCli: libtesseract im Prozess (tesslib, C-API) | Programm (tsv txt, -l osd --psm 0)
                       ├─ PdfRenderer  ── PdfiumRenderer (libloading, eigene 14-Funktionen-FFI) | UnavailableRenderer
                       └─ pdfwrite (ArchiveBuilder-Funktionen, lopdf): Textebene, Bildseiten, Seitenersatz, PDF/A
ImageOps ── FsImageOps (Pillow-Semantik)
```

`NativeOcrmypdf` bildet OCRmyPDF nach:
- Prüfungen: verschlüsselt (auch leeres Passwort) → `EncryptedPdfError`; `/AcroForm /SigFlags & 1` →
  `DigitalSignatureError` (außer `invalidate_digital_signatures`); `NeedsRendering` (XFA) → `InputFileError`;
  Formular + redo → `InputFileError`; Tagged/StructTree ohne Modus-Flag → `TaggedPDFError` (InputFileError);
  mehrere Modus-Flags oder redo+deskew/clean_final → `BadArgsError`.
- je Seite `is_ocr_required`: `--pages` außerhalb → überspringen; Text auf der Seite: ohne Flag
  `PriorOcrFoundError("page already has text! …")`, force → rastern, redo → neu, skip → überspringen;
  Seite ohne Bilder und nicht verlustfrei (deskew/clean_final/force) → überspringen (force: rastern).
- DPI = max(Bild-DPI oder 400, 400 bei Vektor/Text, oversample), gedeckelt auf 600.
- `rotate_pages`: Tesseract-OSD, Drehung wenn Konfidenz ≥ Schwelle → `/Rotate` += Winkel.
- skip/auto: Textebene über den unveränderten Inhalt; redo: alte unsichtbare Textebene (Tr 3) entfernen,
  zum OCR eine Kopie **ohne jeden Text** rastern (Sidecar enthält nur neuen Text); force: Seite durch
  RGB-Raster ersetzen (Annotationen/AcroForm entfallen).
- Vorverarbeitung je OCR-Seite (`src/preprocess.rs`, OCRmyPDF-Reihenfolge: OSD-Drehung → deskew → clean):
  - `deskew`: Schräglage auf verkleinertem Graubild (längste Seite ≤ 1800 px, Projektionsprofil wie
    Leptonica `pixFindSkew`, ±15°, grob 0,25°, fein 0,02°); dunkle Randflächen zählen nicht mit. Ab 0,1°
    wird das Bild einmal in voller Größe gedreht (bilinear, gleiche Größe, weiß aufgefüllt). Erkennung
    und Archivseite nutzen das gedrehte Bild; die Seite wird durch das Bild ersetzt (JPEG Qualität 75,
    wenn alle Bilder der Seite JPEG waren, sonst verlustfrei Flate; farblose Farbraster → Grau).
  - `clean` (unpaper-Ersatz, nur Erkennungsbild): dunkle Randflächen/Scannerkanten vom Bildrand aus,
    Lochungen (4–10 mm, rund, äußere 8 %), schwach graue Kacheln (dunkelster Punkt ≥ 192) und kleine
    Flecken (≤ 6 Punkte bei 300 dpi, skaliert mit dpi²) werden weiß.
    **Fotos werden nicht gesäubert** (`looks_like_photo`, Jetson-Prüfung L1, 2026-11-19): Median der
    hellen Punkte (≥ 128) unter 225 = graues, ungleich beleuchtetes Papier → leere Maske,
    `CleanReport.skipped_photo`, Log info „page looks like a photo (grey paper); not cleaned“. Auf Fotos
    löschte das Säubern i-/Umlaut-Punkte als Flecken und zerlegte den Grund in Kacheln (0/6 statt 6/6
    Suchwörter). Test `preprocess::tests::clean_skips_photos_keeps_scans`.
  - `clean_final`: zusätzlich gesäubertes Bild ins Archiv (Farbe bleibt; Seite wird ersetzt).
  - Kombinationen wie OCRmyPDF: redo + deskew/clean_final → `BadArgsError` (args.rs setzt bei redo
    clean statt clean-final und lässt deskew weg); skip/force/auto erlauben alles.
- Bilder: `triage_image_file` (DPI ≤ 96 ohne `image_dpi` → `DpiError`, Alpha → Fehler), img2pdf-Seite
  (Pixel/dpi·72 pt, JPEG unverändert, EXIF-Drehung 3/6/8 als `/Rotate`), alle TIFF-Seiten.
- `jobs` (= `PAPERLESS_THREADS_PER_WORKER`, fehlt: Kernzahl wie OCRmyPDF): Seiten-OCR parallel
  (`pipeline::ordered`). Rastern (PDFium, nicht threadsicher) bleibt seriell auf dem Aufrufer-Thread,
  Tesseract-Läufe (`OMP_THREAD_LIMIT=1`) in bis zu `jobs` Threads, höchstens `jobs + 1` Raster
  gleichzeitig; Ergebnis in Seitenreihenfolge, bei Fehlern gewinnt die erste Seite in Seitenreihenfolge.
  `jobs = 1` ist die frühere serielle Schleife. Archiv/Sidecar sind unabhängig von `jobs` identisch
  (Tests `crates/pl-ocr/tests/parallel.rs`); Messung `results/bench/ingest/`.
  Seit 2026-11-23 zusätzlich **gemeinsamer Seiten-Pool** (`src/permits.rs`, `pipeline::ordered_shared`,
  Known difference 23): jede Seitenaufgabe (Rastern + Erkennen) hält eine von prozessweit `Kerne`
  Erlaubnissen; `jobs` ist nur noch die Obergrenze je Dokument. Die Erlaubnis wird auf dem
  Aufrufer-Thread vor dem Rastern geholt, wandert mit der Seite zum Worker und wird nach der Seite
  zurückgegeben (höchstens `jobs` Raster je Dokument, höchstens Pool-Größe insgesamt). Seiten ohne
  Arbeit (`Skip`) brauchen keine Erlaubnis. `NativeOcrmypdf::page_pool = None` = Verhalten vorher.
- Tesseract-Weg (`crates/pl-ocr/src/tesslib.rs`): Ist eine `libtesseract` **derselben Version** wie das
  Programm vorhanden, laufen OSD und Erkennung im Prozess über die C-API (dynamisch geladen, kein
  Bauzeit-Link): je (Sprachen, psm) ein Pool initialisierter Instanzen, höchstens so viele wie gleichzeitig
  arbeitende Seiten, prozessweit über Dokumente hinweg; nach jeder Seite `Clear` +
  `ClearAdaptiveClassifier`. **Freigabe im Leerlauf** (2026-11-18): ein Thread `ocr-engine-reaper`
  löscht Instanzen, die länger als `PAPERRUST_OCR_ENGINE_IDLE_SECS` (Standard 180 s, `0` = nie, live
  aus der Oberfläche) unbenutzt im Pool liegen (`TessLib::release_idle`, `tesslib::release_idle_all`),
  danach `tesslib::release_to_os`: unter Linux/glibc `malloc_trim(0)` (Tesseract/Leptonica allozieren
  über `malloc` der C-Laufzeit, die Arenen der beendeten Seiten-Threads gäben sonst nichts zurück),
  unter Windows nichts nötig, plus optionaler Haken `set_release_hook`. Während eines Stapels bleiben
  die Instanzen geladen (Durchsatz unverändert); die erste Erkennung nach einer Freigabe lädt die
  Sprachmodelle neu (lokal deu+eng ~0,2 s je Instanz). Aufrufe bilden `tesseract … -c tessedit_create_tsv=1 -c tessedit_create_txt=1`
  bzw. `-l osd --psm 0` nach (Init-Variablen, `user_defined_dpi`, `AnalyseLayout` vor
  `DetectOrientationScript` wie `ProcessPage`/`GetOsdText`); OpenMP wird mit
  `omp_set_max_active_levels(0)` auf einen Thread gesetzt (wie `OMP_THREAD_LIMIT=1`). Fundort:
  `PAPERRUST_TESSERACT_LIB` (Datei/Verzeichnis, `off` schaltet ab), sonst neben dem Programm
  (`libtesseract*.dll`; `libtesseract.so.5`, `../lib/…`), sonst (nicht Windows) System-`libtesseract.so.5`
  (Debian-Paket `libtesseract5`, Abhängigkeit von `tesseract-ocr`). tessdata: `--tessdata-dir`-Wert, unter
  Windows sonst `<Programmverzeichnis>/tessdata` (wie tesseract.exe), sonst eingebauter Pfad/`TESSDATA_PREFIX`.
  Ohne Bibliothek: Programm mit unkomprimierter TIFF-Datei (Auflösung 0 wie PNM; Leptonica liest sie
  schneller). Ergebnisgleichheit (OSD, Wörter/Boxen, Text, Archivinhalt) mit dem Programm+PNM-Weg:
  `crates/pl-ocr/tests/engine_lib.rs`; Messung `results/bench/ingest/NOTES-engine.md`.
- Sidecar wie `merge_sidecars` (`\f` zwischen Seiten, `[OCR skipped on page(s) a-b]`); 5 Fälle gegen
  OCRmyPDF: 0 Abweichungen.
- Textebene: Type0/Identity-H „GlyphLessFont“, **eingebettet** (`src/glyphless.rs`: selbst erzeugte
  TrueType-Datei mit zwei leeren Glyphen, Vorschub 500/1000 em, `FontFile2` mit `Length1`;
  `CIDToGIDMap`-Strom bildet jede CID auf GID 1 ab, `/DW 500` = Vorschub im Schriftprogramm, wie
  Tesseracts `pdf.ttf`), ToUnicode je Zeichen (auch außerhalb der BMP), eine `BT…ET` je Zeile, `3 Tr`,
  `Tz` auf Wortbreite, direkt im Seiteninhalt (lopdf/pl-parsers finden den Text); Inhalt der
  Originalseite in `q…Q` geklammert.
- `OCR_USER_ARGS` (OCRmyPDF 17.12 `_options.py`/`cli.py`), ausgewertet in `native::parse_options`:
  | Schlüssel | Wirkung |
  |---|---|
  | `optimize` 0–3 (Standard 1), `jpeg_quality`/`jpg_quality`, `png_quality` | Archiv-Verkleinerung (§9a) |
  | `pdfa_image_compression` `auto`/`lossless`/`jpeg` | `jpeg`: verlustfreie 8-Bit-Bilder → JPEG (Qualität `jpeg_quality`, sonst 95 wie `-dJPEGQ`) |
  | `color_conversion_strategy` | §9b (nur bei PDF/A, wie upstream) |
  | `title`, `author`, `subject`, `keywords` | Info + XMP |
  | `mode` `force`/`redo`/`skip`/`auto` | wie die Flags |
  | `tesseract_pagesegmode`, `tesseract_oem`, `tesseract_thresholding` (`otsu`/`adaptive-otsu`/`sauvola` oder 0–2), `tesseract_config`, `user_words`, `user_patterns` | Tesseract `--psm`, `--oem`, `-c thresholding_method=N`, Konfigdateien, `--user-words`, `--user-patterns`; außer psm erzwingen sie den Programmweg (libtesseract-Pool kennt sie nicht); andere Engines: Warnung |
  | `tesseract_non_ocr_timeout` | Zeitlimit der Orientierungserkennung |
  | `tesseract_downsample_large_images` + `tesseract_downsample_above` (32767), `max_ocr_image_mpixels` | Erkennungsbild verkleinern (sichtbare Seite bleibt) |
  | `remove_vectors` | Vektorgrafik (Pfad-Malbefehle → `n`, `sh` weg) im Seiteninhalt vor dem Rastern für die OCR entfernen (Overlay/Redo-Seiten) |
  | `ocr_engine` `none` | keine Texterkennung, nur Ausgabe (PDF/A) |
  | `quiet`, `verbose`, `keep_temporary_files`, `no_overwrite`, `progress_bar`, `use_threads` | ohne Wirkung (Debug-Log) |
  | `jbig2_lossy`, `jbig2_page_group_size` | Warnung wie OCRmyPDF 17 („deprecated and will be ignored“) |
  | `unpaper_args` | wirkt auf das eingebaute Säubern (`preprocess::CleanFilters`): `--no-border-scan`/`--no-mask-scan`/`--no-border` → keine Randflächen, `--no-blackfilter` → keine Lochungen, `--no-grayfilter` → kein Graufilter, `--no-noisefilter` → keine Flecken; OCRmyPDF-Standardschalter (`--layout`, `--mask-scan-size`, `--no-border-align`, `--no-mask-center`, `--no-deskew`, `--no-blurfilter`) still; anderes → Warnung mit den Schaltern |
  | `pdf_renderer` `auto`/`hocr`/`sandwich`/`fpdf2`, `rasterizer` `auto`/`pypdfium`, `sidecar_format` `txt` | entsprechen dem eingebauten Verhalten (still); andere Werte (`hocrdebug`, `ghostscript`, `hocr` als Sidecar) → Warnung (`native::value_note`) |
  | `remove_background` | `false` still; `true` → Warnung (OCRmyPDF: NotImplementedError) |
  | `fast_web_view` | Wert ≥ 10000 (Linearisierung praktisch aus) still; sonst Warnung: Archiv wird nicht linearisiert (lopdf kann nicht linearisieren) |
  | `jbig2_threshold`, `plugins` | Warnung mit Grund (`native::unsupported_reason`) |
  | alles andere | Warnung „not supported … ignored“ |
  Ungültige Werte (z. B. `optimize: 7`) → `BadArgsError` wie OCRmyPDF.

## 9. PDF/A: Stand (E3)

upstream: OCRmyPDF/Ghostscript `generate_pdfa` (`-dPDFA=N -dPDFACompatibilityPolicy=1`) bzw.
`_convert_pdf_to_pdfa` (Ghostscript direkt). PaperRust ohne Ghostscript, `pdfwrite::finish_with_report`:

1. Info: `Producer`, `ModDate`, unlesbares `CreationDate` ersetzt, `Creator`, `--title/--author/
   --subject/--keywords`, `Trapped` nur True/False (→ `pdf:Trapped`), Nicht-Text-Einträge entfernt.
2. Bereinigung `pdfa::normalize(doc, part)` (wie Ghostscript-Policy 1: Unzulässiges entfernen):
   Katalog: `Names/JavaScript`, `AA`, `Requirements`, `Perms`; `AcroForm` `NeedAppearances`/`XFA`;
   eingebettete Dateien (PDF/A-1/-2: entfernt; PDF/A-3: `AFRelationship`, `UF`, `Subtype`, Katalog-`/AF`);
   optionaler Inhalt (PDF/A-1 entfernt; PDF/A-2/3 `Name` ergänzt, `AS` entfernt). Überall `AA`
   entfernt, Aktionen `Launch/Sound/Movie/ResetForm/ImportData/JavaScript` (+ PDF/A-2/3 `Hide/
   SetOCGState/Rendition/Trans/GoTo3DView`, PDF/A-1 `set-state/no-op/Hide`) und nicht-standard
   `Named`-Aktionen entfernt (`A`, `OpenAction`, `Next`). Ströme: `F/FFilter/FDecodeParms` weg, LZW → Flate;
   Bilder: `Alternates`/`OPI` weg, `Interpolate` false, `Intent` gültig; Form-XObjects `OPI/Ref/PS`
   weg; ExtGState: `TR` weg, `TR2` → Default, `HTP` weg, `RI`/`BM` gültig. Anmerkungen: verbotene Typen
   (PDF/A-1-Liste; PDF/A-2/3: 3D/Sound/Screen/Movie/RichMedia, FileAttachment außer PDF/A-3) entfernt,
   Flags druckbar (Print an, Hidden/Invisible/NoView/ToggleNoView aus), `AP` nur `N`, fehlendes
   Erscheinungsbild (PDF/A-2/3, außer Popup/Link/Fläche 0) als leerer Form-XObject. Nicht eingebettete
   „GlyphLessFont“ (fremde OCR-Textebenen, alte PaperRust-Archive) wird durch die eingebettete ersetzt.
   PDF/A-1: Transparenzgruppen weg, ExtGState `SMask` None, `CA/ca` 1, `BM` Normal, Bild-`SMask` auf
   Weiß verrechnet (dekodierbare Bilder), Anmerkungs-`CA` weg.
3. Farbumwandlung §9b, Optimierung §9a.
4. XMP (`pdfaid:part`, `conformance` B, dc/pdf/xmp aus Info gespiegelt), OutputIntent `GTS_PDFA1` mit
   eigenem sRGB-ICC-v2-Profil, Version 1.4 (PDF/A-1) bzw. 1.7, Trailer-`/ID`, Trailer nur
   `Root/Info/ID/Size`, PDF/A-1 klassische Xref-Tabelle (sonst bei optimize ≥ 1 Objekt-Streams wie
   OCRmyPDF `object_stream_mode=generate`).
5. Selbstprüfung `pdfa::check(bytes, part)` (veraPDF lokal nicht vorhanden): Kopf/Binärkommentar,
   Version, Objekt-/Xref-Streams (PDF/A-1), `Encrypt`, `/ID`, XMP vorhanden/unkomprimiert (PDF/A-1)/
   `pdfaid` passend, Info↔XMP (Title, Author, Subject, Keywords, Creator, Producer, Create/ModDate als
   Zeitpunkt), OutputIntent + ICC-Kopf (Größe, `acsp`, Klasse mntr/prtr, Version, N), mehrere Profile,
   DeviceRGB/CMYK ohne passenden Intent (Inhaltsoperatoren, Inline-Bilder, Bilder, Ressourcen-Farbräume
   inkl. Indexed/Separation/DeviceN-Alternativen, Shadings, Gruppen), Schriften eingebettet (PDF/A-2/3:
   nur unsichtbar (Tr 3) genutzte ausgenommen), `CIDToGIDMap` bei CIDFontType2, Breiten der eigenen
   GlyphLessFont, verbotene Aktionen, `AA` (Katalog/Seite/Anmerkung), JavaScript, Anmerkungstypen/
   Flags/`AP`, `NeedAppearances`, XFA, externe Ströme, LZW, JPX (PDF/A-1), `Interpolate`, `Alternates/OPI`,
   PostScript-XObjects, ExtGState `TR/TR2/HTP`, Transparenz (PDF/A-1), eingebettete Dateien,
   optionaler Inhalt (PDF/A-1), Implementierungsgrenzen (PDF/A-1 6.1.12).
   **Besteht die Datei nicht, wird der PDF/A-Anspruch entfernt** (XMP-`pdfaid` durch gleich lange
   Leerzeichen ersetzt) und gewarnt: „Output is not PDF/A-N conformant after conversion (…); the PDF/A
   claim is omitted“ – wie Ghostscript, das bei nicht umsetzbaren Bestandteilen ohne Anspruch schreibt.

**Ersatzschriften (Lücke E3, `pdfa_fonts`):** nicht eingebettete einfache, nicht symbolische Schriften
(`Type1`/`MMType1`/`TrueType`) bekommen wie bei Ghostscript eine eingebettete Ersatzschrift: Liberation
Sans/Serif/Mono (metrisch gleich zu Helvetica/Arial, Times, Courier; Schnitt aus Name/Schriftbeschreibung)
als TrueType-Teilmenge (nur die Glyphen der 256 Codes, Bestandteile von Kompositglyphen; `GSUB`/`GPOS`/
`kern`/`hdmx`/`DSIG` entfallen). Breiten der eingebetteten Glyphen = `/Widths` des PDFs (fehlen sie, aus
der Schrift); Kodierung `WinAnsiEncoding` + `Differences` mit denselben Glyphennamen. Schriftordner:
`PAPERRUST_PDFA_FONT_DIR`, dann `/usr/share/fonts/liberation` (Docker-Image: `fonts-liberation`) u. a.
Größe: Helvetica-Bold → Teilmenge ≈ 1/3 der Schriftdatei oder weniger (Test `unembedded_standard_font_gets_substitute`).

**Grenzen (dokumentiert, nicht behebbar ohne Ghostscript/Schriftdateien):**
- Nicht ersetzt werden symbolische Schriften (Symbol, ZapfDingbats, Wingdings …), CID-Schriften
  (`Type0`) und Schriften mit Glyphennamen außerhalb der Standardkodierungen; ohne Liberation-Dateien
  (z. B. Windows-Desktop) bleibt alles wie vorher → kein PDF/A-Anspruch (Warnung). Die Ersatzschrift
  sieht anders aus als die (nicht mitgelieferte) Originalschrift – wie bei Ghostscript.
- Breitenabgleich (6.2.11.5) und Zeichensatz-/cmap-Regeln nur für die eigene Schrift geprüft.
- Separation/DeviceN mit CMYK-Alternative, CMYK-Shadings, CMYK-Inline-Bilder werden nicht umgerechnet
  (Prüfung meldet sie → kein Anspruch). JPX in PDF/A-1 wird nicht umkodiert.
- Keine Prüfung von XMP-Erweiterungsschemata, Zeichensätzen von Type1-Schriften (CharSet), Halftones,
  Seitengrenzen-Größen, Strukturbaum (nur Stufe B).

### 9a. Archiv-Verkleinerung (E5, `src/optimize.rs`, `src/jbig2.rs`)

Quelle OCRmyPDF 17.12 `optimize.py`/`builtin_plugins/optimize.py`. Standard `optimize=1` wie OCRmyPDF
(gilt für die OCR-Ausgabe und `_convert_pdf_to_pdfa`; nicht für `image_to_pdfa`/img2pdf).
| Stufe | Wirkung |
|---|---|
| 0 | nichts (klassisch gespeichert) |
| 1 | 1-Bit-Bilder (auch ImageMasks) → JBIG2 generisch, verlustfrei (eigener MQ-Kodierer, T.88 Vorlage 0; ersetzt jbig2enc); 8-Bit-Bilder ohne JPEG verlustfrei neu: farbloses RGB → Grau, reines Schwarz/Weiß → 1 Bit → JBIG2, ≤ 256 Farben → Indexed, Flate mit PNG-Prädiktor je Zeile; Flate über JPEG (< 10000 px); unbenutzte Objekte entfernt; Objekt-Streams (außer PDF/A-1) |
| 2 | + JPEG neu mit `jpeg_quality` (Standard 75) wenn kleiner; Farbreduktion RGB → ≤ 256 Farben (Median-Cut) wenn PSNR ≥ 20 + 0,2·(png_quality−10) dB (Näherung an pngquant `--quality min-max`, Standard 70) |
| 3 | wie 2 mit Qualität 40/30, Flate-Stufe 9, Flate über alle JPEGs |
Jede Neukodierung nur, wenn kleiner. CMYK-JPEGs werden nicht neu kodiert. Masken (`SMask`) nur verlustfrei.
Messung (`tests/archive_features.rs`, Umwandlung nach PDF/A-2, Bytes; Stufe 1 rendert mit PDFium
pixelgleich zu Stufe 0):
| Datei | Original | Stufe 0 | 1 | 2 | 3 |
|---|---|---|---|---|---|
| synthetisch (Text-Scan A4 150 dpi RGB + Foto verlustfrei + Foto JPEG q95) | – | 354 709 | 189 165 | 97 623 | 75 948 |
| `multi-page-images.pdf` (Scan, 1 Bit in Flate) | 150 479 | 155 938 | 24 734 | 23 702 | 18 837 |
| `multi-page-mixed.pdf` | 160 736 | 166 085 | 34 054 | 33 022 | 28 157 |
| `rotated.pdf` (Graustufen-Scan) | 1 322 136 | 1 327 530 | 1 181 575 | 1 101 530 | 737 208 |
| `simple-digital.pdf` | 22 926 | 23 696 | 13 472 | 13 472 | 13 472 |
| `with-form.pdf` | 12 022 | 17 332 | 15 741 | 15 741 | 15 741 |
| `results/bench/ingest/scan-16p.pdf` (JPEG-Scan) | 476 510 | 482 001 | 479 512 | 479 511 | 479 510 |
Stufe 0 ist größer als das Original (sRGB-Profil + XMP-Paket mit Füllraum, klassische Xref-Tabelle).
Alle sieben Dateien bestehen nach der Umwandlung die Selbstprüfung (PDF/A-2-Anspruch gesetzt).

### 9b. Farbumwandlung (E4, `src/color.rs`)

upstream reicht `color_conversion_strategy` nur bei PDF/A an Ghostscript (OCR-Ausgabe und
`_convert_pdf_to_pdfa`); hier ebenso nur bei PDF/A. Bilder (8 Bit, Grau/RGB/CMYK, ICC, Indexed nur
Tabelle; JPEG nach Umrechnung Qualität 95 wie `-dJPEGQ=95`) und Vektorfarben (`g/G rg/RG k/K`,
`cs/CS` + `sc/scn` in Geräteräumen, auch Ressourcen-Namen) in Seiten, Form-XObjects, Kachelmustern.
`LeaveColorUnchanged`: nur DeviceCMYK → RGB (sonst PDF/A-Verstoß); `RGB`: CMYK (Device + ICC) → RGB;
`Gray`: alles → Grau; `UseDeviceIndependentColor`: Bilder → `ICCBased` (sRGB bzw. eigenes Grau-Profil
`icc::gray_profile`), Vektorfarben bleiben Device (mit OutputIntent zulässig); `CMYK`: bei PDF/A → wie
RGB + Warnung (kein CMYK-OutputIntent-Profil vorhanden). CMYK→RGB naiv (R = (1−C)(1−K)), Grau nach
ITU-R 601.

## 10. Tests

- Speicher (2026-11-18): `tesslib::tests::{idle_secs_parsing, release_to_os_is_harmless}`,
  `thumbnail::tests::{posterize_keeps_extremes_and_gray, posterized_thumbnail_is_smaller}`,
  `tests/engine_lib.rs::library_releases_idle_instances_and_recreates` (Freigabe + gleiches Ergebnis
  danach; gibt warm/kalt-Zeit aus, lokal deu+eng 44 ms / 243 ms).

- Unit-Tests (65, ohne Werkzeuge): Einstellungen inkl. `test_ocr_config.py`, alle Fälle aus
  `test_tesseract_custom_settings.py` und `TestOcrmypdfParameters`, `test_parse_modes.py` (alle Klassen),
  Mock-Varianten von `TestParsePdf` (signed/encrypted/gs-Fehler), Fallback-Pfade, `should_produce_archive`,
  Textebene ist mit lopdf wieder extrahierbar (inkl. Umlaut/€/Nicht-BMP), Drehmatrizen, Strip-Text,
  ICC-Kopf, Sidecar-Format, OCRmyPDF-Seitenplanung.
- `tests/archive_features.rs` (11): PDF/A-Selbstprüfung der OCR-Ausgabe für PDF/A-1/2/3 (Textebene
  extrahierbar, Schrift eingebettet, PDF/A-1 ohne Objekt-Streams); born-digital-PDF mit JavaScript,
  OpenAction, Seiten-AA, Sound-Anmerkung, Anmerkung ohne Flags/AP, CMYK-Bild mit SMask, ExtGState
  (ca/BM/TR), eingebetteter Datei, NeedAppearances → nach Bereinigung 0 Verstöße je Teil; sichtbare
  nicht eingebettete Helvetica → Anspruch entfällt; fremde GlyphLessFont ohne Einbettung → ersetzt
  (PDF/A-1 konform); Prüfung erkennt fehlendes XMP/Intent/DeviceRGB; Farbstrategien (Gray, ICC, RGB,
  CMYK→RGB, ohne PDF/A wirkungslos; gerendert ≤ 6 Graustufen Abweichung); Optimierungsstufen 0–3
  (Größen, Stufe 1 pixelgleich gerendert); JBIG2 mit PDFium gerendert = Original (3 Größen);
  `OCR_USER_ARGS`-Auswertung; Metadaten-Argumente in Info/XMP; echte Beispieldateien (Größen je Stufe,
  Stufe 1 pixelgleich, PDF/A-2-Selbstprüfung bestanden).
- `tests/remote_azure.rs` (5): lokaler Mock-Server (127.0.0.1): Ablauf Text + Archiv-PDF inkl.
  Abfrage mit `Retry-After`, base64-Rumpf, Schlüssel-Kopf; 401 → ParseError ohne Schlüssel; interner
  Endpunkt bei `allow_internal=false` gesperrt (keine Anfrage); Abfrage-URL auf anderem Host abgelehnt;
  Entscheidungen ohne Azure (lokaler Text, keine Engine).
- `tests/parity.rs` (5): gegen `tests/parity/pl-ocr/reference.json` (Erzeuger `gen_reference.py`,
  echtes upstream + Pillow 13 + OCRmyPDF 17, kein DB-Zugriff): Bilder, Argumente, `parse()`-Abläufe,
  Text-Helfer, Sidecars, Archiv-Entscheidung – 0 Abweichungen.
- `tests/tools.rs` (12): Ende-zu-Ende mit PDFium und Tesseract an den upstream-Beispieldateien
  (Bilder png/alpha/no-dpi/jpg/bmp/tif/gif/webp, mehrseitige TIFFs inkl. Alpha, Bild-PDF auto/ohne Archiv/
  redo pages=2/force pages=1, mixed skip + Sidecar-Marker, mixed redo, digital pages=2 in allen Modi,
  signed, encrypted, Formular, tagged ohne Text, rotated, PDF/A-Ausgabetypen, Thumbnail, off-Modus,
  HEIC als bekannte Abweichung). Fehlt ein Werkzeug: `SKIP` auf stderr.
- Werkzeuge: PDFium aus `reference/venv/.../pypdfium2_raw/pdfium.dll` (PyPI-Wheel, Prüfsumme in
  `reference/wheel-provenance.txt`); Tesseract 5.5.4 aus dem offiziellen MSYS2-Repo per
  `tests/parity/pl-ocr/fetch_tesseract.py` (SHA-256 jeder Paketdatei gegen `ucrt64.db`, 45 MB, nach
  `tests/parity/pl-ocr/.tools/`, git-ignoriert).

## 11. Known differences

0. **Säubern lässt Fotos aus** (§8, L1): unpaper säubert jedes Bild; PaperRust überspringt Seiten mit
   grauem Papier (Fotos), weil das eingebaute Säubern dort Text zerstört. Scans (weißes Papier) wie bisher.

1. **pdftotext → lopdf** für Textextraktion (Original und Archiv): Leerzeichen/Zeilenumbrüche/Reihenfolge
   können abweichen (`-layout` fehlt); born-digital-Entscheidung nur an der 50-Zeichen-Grenze betroffen.
   Bei Seiten mit vorhandenem Text **und** neuer OCR-Ebene (redo, mixed) liefert lopdf erst den
   Originaltext, dann die OCR-Ebene (pdftotext sortiert nach Position) – z. B. `single-page-mixed.pdf`
   (redo): alle drei Sätze vorhanden, aber Satz 2/3 vertauscht. Abhilfe später: positionsbasierte
   Extraktion (PDFium `FPDFText_*`) im Worker.
2. **Eigene Vorverarbeitung statt unpaper/Leptonica** (`preprocess.rs`): gleiche Wirkung, andere Pixel.
   Abweichungen: (a) deskew mit Winkel < 0,1° (oder ohne erkennbare Textzeilen) und ohne clean-final
   lässt die Seite **unverändert** (OCRmyPDF rastert bei deskew jede Bildseite neu, auch ungedreht) –
   kleineres Archiv, Originalqualität. (b) clean-final ohne gefundene Verschmutzung lässt die Seite
   ebenfalls unverändert. (c) Säubern entspricht nicht OCRmyPDFs unpaper-Aufruf (`--layout none
   --mask-scan-size 100 --no-grayfilter --no-blackfilter …`): zusätzlich leichte Graufilterung
   (Schwelle 192 statt unpaper 128, damit hellgrauer Text bleibt) und Lochungsentfernung; keine
   Blur-Filterung, keine Masken-Zentrierung; schwarze Flächen nur am Rand (keine im Innern).
   (d) `unpaper_args` aus `OCR_USER_ARGS` wirkt nur über die Filter-Schalter (§8, siehe 12). (e) Suchbereich der Schräglage ±15°
   (Leptonica sucht ±7°, mit Fallback weiter). (f) Ersetzte Seiten in Farbe gerastert, wenn deskew oder
   clean-final aktiv ist (OCRmyPDF wählt Farbe/Grau/s-w nach den Bildern der Seite).
3. **Kein Ghostscript**: PDF/A durch Bereinigung + Selbstprüfung (§9) statt Neuschreiben; nicht
   eingebettete Schriften bekommen eine Liberation-Ersatzschrift (§9), symbolische/CID-Schriften nicht →
   dann Datei ohne PDF/A-Anspruch (Warnung). Ghostscript kodiert Bilder nach eigener Wahl neu. `continue_on_soft_render_error`
   ist wirkungslos. Optimierung eigene Kodierer (§9a): JBIG2 nur generisch-verlustfrei (wie jbig2enc ohne
   `-s`), Farbreduktion Median-Cut statt pngquant (andere Pixel, PSNR-Schwelle statt pngquant-Qualität).
   Keine Linearisierung (`fast_web_view`; OCRmyPDF linearisiert Dateien > 1 MB standardmäßig).
4. **Tesseract-Ausgabe**: TSV-Wortboxen statt hOCR; Zeilengrundlinie = Unterkante der Wortboxen
   (OCRmyPDF nutzt die hOCR-Baseline) → Markierung der Textauswahl kann leicht versetzt sein; Inhalt gleich.
5. **HEIC**: keine HEVC-Dekodierung in reinem Rust → `ParseError` (upstream: pillow_heif). Kopfdaten (Größe,
   Alpha) werden korrekt gelesen.
6. **Lazy Tesseract**: Werkzeug und Sprachdaten werden erst geprüft, wenn eine Seite wirklich OCR braucht
   (OCRmyPDF prüft immer vorab) – born-digital-PDFs mit skip_text laufen ohne Tesseract.
7. **Raster-DPI ≤ 600** (RAM-Schutz); OCRmyPDF hat keine Obergrenze.
8. **Redo**: Text wird über Operatoren im Seiteninhalt erkannt/entfernt, nicht in Form-XObjects;
   OCRmyPDF analysiert mit pdfminer. Seiten mit „korruptem“ Text werden nicht gesondert gewarnt.
9. **remove_alpha**: Ausgabe als PNG (ein Bild) bzw. mehrseitiges TIFF statt im Originalformat, ohne
   ICC/DPI-Kopie (wird nur intern weiterverwendet; `image_dpi` stammt aus dem Original).
10. **Thumbnail**: WebP verlustfrei statt Pillow-Qualität 80 (größere Datei); kein qpdf-Reparaturversuch.
11. **Formularfelder**: PDFium rendert Widgets nur mit vorhandenem Erscheinungsbild (keine
    FPDF_FFLDraw-Formularumgebung); bei force entfallen Annotationen/AcroForm.
12. **OCRmyPDF-Optionen** aus `OCR_USER_ARGS`: Liste in §8. `unpaper_args` wirkt nur über die
    Filter-Schalter (eigener Säuberer statt unpaper); `remove_background` (OCRmyPDF: NotImplementedError,
    hier nur Warnung statt Abbruch), `fast_web_view` (keine Linearisierung), Plugins, `jbig2_lossy`
    wirken nicht (Warnung je Schlüssel); `pdf_renderer`/`rasterizer`/`sidecar_format` wirken nur mit den
    Werten, die dem eingebauten Verhalten entsprechen. `remove_vectors` wirkt nur auf Overlay/Redo-Seiten und nur im Seiteninhalt (nicht in
    Form-XObjects); bei force gerasterte Seiten werden mit Vektorgrafik erkannt.
13. **img2pdf-Spiegel-Orientierungen** (EXIF 2/4/5/7) werden ignoriert statt gespiegelt.
14. **Riesige Seiten** (Testprotokoll 2026-11-16 C-017): statt `DecompressionBombError` wird eine Seite,
    die mit der gewünschten Auflösung über `max_image_mpixels` (bzw. 400 Mio. Bildpunkte / 65000 je
    Seite) käme, mit entsprechend geringerer Auflösung gerastert (`render::fit_dpi`, Warnung im Log;
    `Raster::dpi` nennt die tatsächliche Auflösung). Abgelehnt wird nur noch, wenn dafür weniger als
    50 dpi (`native::MIN_FIT_DPI`) nötig wären. OCRmyPDF/PIL bricht bei > 2 × `max_image_mpixels` ab;
    zwischen 1× und 2× rastert PaperRust jetzt gröber statt mit voller Auflösung. Der PDFium-Renderer
    selbst verkleinert ebenso (Barcode-Suche, Vorschaubild).
15. **PDFium-Suche** (A-294): ohne `PAPERRUST_PDFIUM_PATH` (Umgebung oder in der Oberfläche gespeichert)
    zuerst neben dem Programm und in dessen Unterordner `pdfium/` (Desktop-Paket), dann der Suchpfad des
    Systems (`render::pdfium_candidates`).

16. **Farbumwandlung** (§9b): naive CMYK↔RGB-Formeln statt ICC-Farbmanagement (Ghostscript nutzt
    Profile); `CMYK` bei PDF/A wirkt wie `RGB`; Vektorfarben bei `UseDeviceIndependentColor` bleiben
    Device-Farben; Separation/DeviceN/Shadings/Inline-Bilder werden nicht umgerechnet.
17. **PDF/A-Anspruch nur nach bestandener Selbstprüfung** (§9 Punkt 5); veraPDF-Abgleich steht aus
    (Java ist auf dem Entwicklungsrechner vorhanden, veraPDF nicht installiert).
18. **Remote-OCR**: REST statt Azure-SDK; Abfrage höchstens 30 min (SDK unbegrenzt); Weiterleitungen
    und Abfrage-URLs nur auf denselben Host (SDK: Schlüssel bei fremdem Host entfernt, Anfrage folgt);
    User-Agent `PaperRust/<version> remote-ocr` (nicht über `pl_core::brand`, pl-ocr hängt nicht von
    pl-core ab).
18a. **Remote-OCR: festgenagelte Adresse, kein Proxy, Größengrenze** (Sicherheitsprüfung 2026-11-19 F10):
    bei `REMOTE_OCR_ALLOW_INTERNAL_ENDPOINTS=false` wird der Hostname einmal aufgelöst und geprüft
    (`remote::validate_and_pin`), danach verbinden alle Anfragen (Analyse, Abfrage, PDF, Weiterleitungen
    auf denselben Host) nur mit dieser Adresse (wie upstream `PinnedHostHTTPTransport`); Folge-URLs werden
    dann nicht erneut per DNS geprüft, nur auf gleichen Ursprung. `HTTP(S)_PROXY` wird nicht verwendet
    (upstream: httpx mit `trust_env`). Jede Antwort höchstens 64 MiB (`remote::MAX_RESPONSE_BYTES`,
    upstream unbegrenzt) → `ParseError("Azure AI Vision parsing failed: response from Azure exceeds the
    limit of 64 MiB")`. Tests: `remote_azure::connection_goes_to_pinned_address`,
    `oversized_response_is_refused`, `validate_and_pin_returns_address`.
19. **Vorschaubild-Kodierung**: verlustfreies WebP nach Reduktion auf 32 Stufen je Farbkanal statt
    Pillow-WebP Qualität 80 (verlustbehaftet). Die image-Crate hat keinen verlustbehafteten Encoder;
    ohne Reduktion war ein Vorschaubild ~3,5× so groß wie bei ngx (42 statt 12 KiB), jetzt ~1,6×.
    Leichte Tonwertstufen in Verläufen möglich (bei 500 px Breite nicht sichtbar).
20. **Tesseract-Instanzen werden nach Leerlauf entladen** (`PAPERRUST_OCR_ENGINE_IDLE_SECS`, Standard
    180 s). Kein upstream-Gegenstück (OCRmyPDF startet je Seite einen Prozess).

21. **Bilddateien in Graustufen erkennen** (2026-11-21): PNG/JPEG/TIFF … gehen wie PDF-Seiten als eigenes
   Graustufenbild (ITU-R-601-Helligkeit) an die Texterkennung, nicht farbig (OCRmyPDF/Tesseract wandeln selbst um).
   Anlass: Jetson-Prüfung, geradegerichtetes Handyfoto `c_photo_00.png` – farbig verlor ein Wort die Umlaut-Punkte
   (Wortquote 97,4 %, paperless-ngx 100 %), in Graustufen bei allen Winkeln 2,5–4,5° richtig. Archiv-/Anzeigebild
   bleibt farbig. Test: `tests/image_ocr_gray.rs`.

22. **Standard-OCR-Sprache `deu+eng`** (2026-11-21; paperless-ngx `eng`): ohne `PAPERLESS_OCR_LANGUAGE` erkennt
   PaperRust Deutsch und Englisch (`config::DEFAULT_OCR_LANGUAGE`). Messung `results/benchmark/2026-11-21-ocr-lang`:
   deutsche Seiten mit `eng` 88,6 % der Wörter, mit `deu+eng` 100 %; Zeit +3–7 %. Fehlen beim Standard die
   deutschen Daten, wird mit Warnung `eng` genommen (nur beim Standard; explizit gewählte, fehlende Sprachen
   scheitern weiter mit `MissingDependencyError`). Test: `tests/language_default.rs`. Die Suchsprache
   (`pl-core`, Stemming) folgt weiter `PAPERLESS_OCR_LANGUAGE` mit Standard `eng`.

23. **Kerne werden zwischen gleichzeitigen Dokumenten geteilt** (2026-11-23; OCRmyPDF/upstream: feste
   `jobs = THREADS_PER_WORKER = max(Kerne // TASK_WORKERS, 1)` je Dokument). Anlass: Raspberry Pi 5,
   3 Kerne (`results/benchmark/2026-11-23-pi5-parallel/bericht.md`): mit `TASK_WORKERS=auto` bekam ein
   allein laufender 8-seitiger Scan nur `Kerne ÷ Aufgaben` Threads (24 s statt 14,8 s). Jetzt
   (`src/permits.rs`): ein prozessweiter Pool mit `Kerne` Seiten-Erlaubnissen (bei ungesetztem
   `THREADS_PER_WORKER` zusätzlich nach `pl_core::resources::memory_threads_cap(1, freier Speicher)`
   begrenzt, d. h. Pool × 100 MB ≤ halber freier Speicher). Jede Seitenaufgabe (Rastern + Erkennen
   einer Seite) läuft unter einer Erlaubnis. Ein Dokument allein nimmt alle freien Erlaubnisse (bis zu
   seinem `jobs`); mehrere teilen: eine frei werdende Erlaubnis geht an das wartende Dokument mit den
   **wenigsten** gehaltenen Erlaubnissen (bei Gleichstand: wer zuerst wartete). Ein neues Dokument
   bekommt damit spätestens nach einer Seitenzeit eine Erlaubnis, keines verhungert, nie mehr als
   Pool-Größe Seitenaufgaben gleichzeitig. Sperrreihenfolge: ein Mutex + Condvar im Pool, sonst nichts;
   gewartet wird nur ohne gehaltene Erlaubnis (Halter warten nie auf weitere) → verklemmungsfrei.
   Standard für `jobs` bei ungesetztem `THREADS_PER_WORKER` ist deshalb **der ganze Pool** =
   `min(Kerne, Speichergrenze)` (für jede `TASK_WORKERS`-Einstellung; vorher `Kerne ÷ TASK_WORKERS`;
   die Speichergrenze schreibt `pl_core::resources::apply_ocr_resource_cap` als
   `PAPERLESS_THREADS_PER_WORKER = memory_threads_cap(1, Speicher)` in die Umgebung, gilt also als
   `jobs` unverändert; nur `permits::threads_set_by_user` erkennt sie, damit der Pool nicht auf `Kerne`
   springt). Seit 2026-11-23 ist `TASK_WORKERS` ungesetzt = `auto` (pl-queue Known difference 16), der
   Pool ist also im Standard aktiv geteilt; ein gesetzter Wert ist die Obergrenze je
   Dokument, der Pool bleibt bei `Kerne` (keine Überbelegung mehr mit z. B. `TASK_WORKERS=2`,
   `THREADS_PER_WORKER=4` auf 4 Kernen: zusammen höchstens 4 statt 8 Seiten). Nur die Reihenfolge der
   Arbeit ändert sich: Seitenreihenfolge, Archiv-Bytes und Text sind gleich (Tests
   `pipeline::tests::shared_*`, `permits::tests::*`, `tests/parallel.rs`). Nicht vom Pool erfasst (wie
   vorher): Arbeit außerhalb der Seitenschleife (PDF/A-Bau, Verkleinerung, Vorschaubild, Textauszug)
   und die Zeilen-Threads von `preprocess::rotate` bei großen Seiten (bis zu 8, innerhalb einer
   Erlaubnis). Tesseract selbst bleibt je Instanz einfädig (`OMP_THREAD_LIMIT=1` bzw.
   `omp_set_max_active_levels(0)` je Worker-Thread in `tesslib`). Zeitmessung:
   `examples/page_pool_bench.rs`.

## 12. Was läuft ohne externe Programme / was braucht Werkzeuge

Ohne externe Programme (reines Rust): Einstellungen, Argumente, komplette `parse()`-Steuerlogik,
Archiv-Entscheidung, Bild-Kopfdaten/DPI/Alpha, Text-Helfer, Seitenzahl, Textextraktion (lopdf),
Textebene schreiben, Bild→PDF (img2pdf-Ersatz), PDF/A-Bereinigung + Selbstprüfung, Farbumwandlung,
Optimierung inkl. JBIG2 (`off`-Modus komplett), Remote-OCR-Aufruf (HTTPS), skip_text auf
born-digital-PDFs (keine Seite braucht OCR), Sidecar.
PDFium (dynamische Bibliothek): Rastern von PDF-Seiten für OCR (auto/redo/force auf PDFs), Thumbnails.
Tesseract (Programm + Sprachdaten, `osd` für rotate_pages): jede tatsächliche Texterkennung
(Bilder, Scan-PDF-Seiten, force/redo).

## 13. Betriebssystem-eigene OCR-Engines (optional, Standard bleibt Tesseract)

Auswahl `PAPERRUST_OCR_ENGINE` (Alias `PAPERLESS_OCR_ENGINE`), Modul `engine_select`:

| Wert | Ergebnis |
|---|---|
| fehlt / `tesseract` | unverändert Tesseract (keine WinRT-Initialisierung, gleiche Engine-Instanz wie bisher) |
| `windows` | `winocr::WindowsOcr`; fehlt `Windows.Media.Ocr` (anderes OS, Windows N ohne Media-Features) → Warnung (einmal) + Tesseract |
| `auto` | Windows OCR, wenn vorhanden **und** `check_languages(PAPERLESS_OCR_LANGUAGE)` ok; sonst Tesseract; ohne Tesseract Windows OCR |
| `macos` | reserviert (Apple Vision, nicht umgesetzt) → Warnung + Tesseract |
| ungültig | Warnung + Tesseract |

Einstieg für den Aufrufer: `engine_select::engine_from_env(tesseract: Option<TesseractCli>, languages) ->
Option<Box<dyn OcrEngine>>` – ersetzt in `pl-consumer::native::NativeParsing::parse` das bisherige
`self.tesseract.clone().map(Box::new)` (eine Zeile; ohne Einstellung identisches Ergebnis).

Nur der Erkenner wechselt: `PageOcr` (Wörter mit Pixelboxen, Zeilenkennung, Seitentext) geht in
denselben Weg (`pdfwrite::text_layer_content`, Sidecar, `post_process_text`, PDF/A-Stempel).

**Trait-Erweiterung:** `OcrEngine::check_languages(&self, languages)` (Standard: wie bisher
`check_installed(self.languages()?, …)`); die freie Funktion `engine::check_languages` ruft die
Methode. Für Tesseract unverändert.

### Windows (`Windows.Media.Ocr`, `crate::winocr`, nur `cfg(windows)`)

- Abhängigkeit: `windows` 0.62.2 (MIT OR Apache-2.0), nur Features `Foundation`,
  `Foundation_Collections`, `Globalization`, `Graphics_Imaging`, `Media_Ocr`, `Storage_Streams`
  (Bauzeit pl-ocr +~60 s einmalig im Debug-Build).
- Raster → `Gray8`-`SoftwareBitmap` (RGB → Luma 299/587/114) → `RecognizeAsync`; Warten über
  Abschluss-Callback + Kanal (kein STA/Nachrichtenschleife nötig, `timeout` → `Cancel` +
  `SubprocessOutputError`). Bilder > `MaxImageDimension` (10 000 px) werden verkleinert erkannt,
  Boxen zurückgerechnet.
- `OcrLine`/`OcrWord` → `Word { line: (1, Absatz, Zeile), conf: -1 }` (Windows liefert keine
  Konfidenz). Seitentext: Zeilentext (`OcrLine.Text`) mit `\n`, Leerzeile bei Lücke > Median-Zeilenhöhe
  oder Sprung nach oben (neue Spalte), `\f` am Ende wie Tesseract.
- Sprachen (`engine_select::plan_languages`): Tesseract-Codes → BCP-47 (`deu`→`de-DE`, Vergleich nach
  Hauptsprache, bei `zh`/`sr` mit Schrift). Windows erkennt mit **einem** Sprachmodell: erste angeforderte
  Sprache mit installiertem Paket. Weitere Sprachen ohne Paket: gleiche Schrift → Warnung, über das
  Modell mit erkannt (`deu+eng` mit nur `de-DE`); andere Schrift, unbekannter Code oder gar kein Paket →
  `MissingDependencyError: Windows OCR has no language pack … Add-WindowsCapability -Online -Name
  "Language.OCR~~~<tag>~0.0.1.0"`. `languages()` = Tesseract-Codes der installierten Pakete.
- Orientierung: Windows OCR erkennt keine 90°/180°-Drehung (`TextAngle` ist nur der Schräglauf).
  `orientation_raster` nutzt Tesseract-OSD (`TesseractCli`, Bibliothek oder Programm), wenn beim
  Erzeugen übergeben (`engine_from_env` tut das); ohne Tesseract keine Drehung (Warnung einmal).
- Nebenläufigkeit: WinRT-Aufrufe aus beliebigen Pipeline-Threads (windows-core initialisiert MTA bei
  Bedarf). `OcrEngine`-Instanzen in einem prozessweiten Pool je Sprach-Tag, jede Instanz nur von einem
  Thread gleichzeitig benutzt; nach Zeitüberschreitung wird die Instanz verworfen. Test:
  6 Threads gleichzeitig = seriell (`tests/winocr.rs`).
- Freigabe im Leerlauf wie bei Tesseract: der gemeinsame Aufräum-Thread (`tesslib`) löscht Erkenner,
  die länger als `PAPERRUST_OCR_ENGINE_IDLE_SECS` (Standard 180 s, `0` = nie) unbenutzt im Pool liegen
  (`winocr::IdlePool`, `winocr::release_idle_all`).

### macOS Vision (Entwurf, nicht umgesetzt)

Hier ist kein Mac vorhanden. `cargo check -p pl-ocr --target aarch64-apple-darwin` läuft ohne SDK
(am 2026-11-09 mit dem Platzhalter geprüft); eine Vision-Umsetzung ließe sich so zwar kompilieren, aber
weder ausführen noch testen – daher bewusst nicht umgesetzt. `crate::macocr` ist ein
Platzhalter (`Unavailable`), `PAPERRUST_OCR_ENGINE=macos` fällt auf Tesseract zurück. Entwurf:

- Crates `objc2` 0.6, `objc2-foundation` 0.3, `objc2-vision` 0.3 (`VNRecognizeTextRequest`,
  `VNImageRequestHandler`), `objc2-core-graphics` 0.3 (CGImage aus dem Graustufen-Raster:
  `CGDataProviderCreateWithData` + `CGImageCreate`, `kCGImageAlphaNone`, DeviceGray), alle MIT/Apache.
- Ablauf je Seite (in einem `autoreleasepool`, Vision ist synchron und threadsicher je Request):
  `VNRecognizeTextRequest::new()`, `setRecognitionLevel(Accurate)`, `setUsesLanguageCorrection(true)`,
  `setRecognitionLanguages` aus den Tesseract-Codes (`de-DE`, `en-US` – Vision akzeptiert mehrere
  Sprachen gleichzeitig, Prüfung über `supportedRecognitionLanguagesAndReturnError`),
  `VNImageRequestHandler::initWithCGImage_options` → `performRequests_error`.
- Ergebnis: `VNRecognizedTextObservation` = Zeile; `topCandidates(1)[0]` liefert String + Konfidenz
  (0–1 → ×100). Wortboxen: für jedes Wort (Leerzeichen-Trennung im String) `boundingBoxForRange_error`
  → normierte Koordinaten mit Ursprung **unten links** → Pixel: `x0 = x·W`, `y0 = (1 − y − h)·H`.
  Absätze wie Windows (Lückenregel).
- Orientierung: Vision normalisiert nicht selbst; wie Windows über Tesseract-OSD, alternativ
  `VNDetectTextRectanglesRequest` auf 4 Drehungen (teuer).
- Kompilieren: `rustup target add aarch64-apple-darwin` + `cargo check --target aarch64-apple-darwin`
  (geht auch unter Windows); Laufzeittests wie `tests/winocr.rs` nur auf einem Mac.

### Qualität/Zeit

Messung und Empfehlung: `results/bench/ocr-quality/RESULTS.md` (Skripte `tests/bench/ocr-quality/`).

### Tests

- `engine_select` (Unit): Auswahlmatrix, `parse`, Standard reicht Tesseract unverändert durch,
  Sprachschlüssel/-zuordnung, Fehlermeldungen.
- `winocr` (Unit, plattformunabhängig): Wörter/Boxen/Zeilen/Absätze/Text, Rückrechnung bei Verkleinerung,
  Graustufen, Maßstab.
- `tests/winocr.rs` (echtes WinRT, sonst `SKIP`): Erkennung + Boxen, parallel = seriell, fehlendes
  Sprachpaket, Auswahl, Einleseweg `parse_document` (PNG + mehrseitiges TIFF, Textebene im Archiv).

## 12. Upstream-Fixes nach v3.4.2 (paperless-ngx `dev`)

- **d05e96cda (#14734) – vom Parser verwaltete OCR-Argumente.** Quelle: `paperless/config.py`
  (`OCR_RESERVED_USER_ARGS`), `parsers/tesseract.py::construct_ocrmypdf_parameters`. Rust:
  `args::OCR_RESERVED_USER_ARGS = ["input_file_or_options", "output_file", "sidecar"]`; diese
  Schlüssel aus `user_args` (DB oder `PAPERLESS_OCR_USER_ARGS`) werden mit Warnung „Ignoring OCR user
  arguments managed by paperless: …“ (sortiert, `, `) übergangen, alle anderen gelten weiter. Test:
  `args::tests::ocr_user_args_cannot_override_reserved`. Die API lehnt sie zusätzlich ab (pl-system).
- **759f9cd22 (#14741) – Vorschaubild für Bilder ohne Archiv-PDF.** Quelle:
  `parsers/tesseract.py::get_thumbnail` + `documents/parsers.py::encode_thumbnail_webp`. Rust:
  `thumbnail::make_thumbnail_from_image(bild, tempdir)` – Alpha auf Weiß, auf 500×5000 verkleinert
  (nie vergrößert), WebP `convert.webp`; Fehler → `OcrError::Parse("Unable to encode thumbnail from
  …")`. Aufrufer: pl-consumer `NativeParsing::thumbnail`. Test:
  `thumbnail::tests::image_without_archive_has_image_thumbnail` (JPEG, PNG, TIFF, GIF, BMP, WebP,
  PNG mit Alpha; kaputte Datei → Fehler).
  Known difference: die image-Crate liest kein HEIC (Pillow mit pillow-heif schon); dort entsteht
  weiter das Standardbild (Rückfall im Aufrufer), und WebP ist verlustfrei kodiert (wie schon bei
  `make_thumbnail_from_pdf`, siehe 11.).
