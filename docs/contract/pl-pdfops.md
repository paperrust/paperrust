# Vertrag: pl-pdfops (PDF-Seiten, Barcodes, TIFF→PDF, qpdf-Ersatz, Text-Vorschau)

Reine Bibliothek (keine DB, keine Routen) für den Python-freien Worker. Ersetzt alles, wofür
upstream im Worker pikepdf/qpdf, zxing-cpp, pdf2image/poppler, img2pdf und Pillow-Textzeichnen
nutzt. Nur reine Rust-Crates mit festen Versionen (lopdf 0.47.0, rxing 0.9.3, image 0.25.10,
ab_glyph 0.2.32); Rastern über pl-ocr `PdfRenderer` (PDFium als dynamische Bibliothek, wie
OCRmyPDF/pypdfium2 – kein Bauzeit-Link).

Quelle: paperless-ngx v3.4.2 (`upstream/`, unverändert); Verhalten von pikepdf 10.13.0/qpdf,
zxing-cpp 3.1.1, img2pdf 0.6.3, Pillow 13.0 aus `reference/venv` (empirisch geprüft, siehe Tests).

| Rust | upstream |
|---|---|
| `ops::{PageSpec, rotate_pdf, remove_pages, build_pdfs, validate_page_operations, page_specs_for, needs_decrypt, decrypt_pdf, PdfMerger}` | `documents/pdf_ops.py` (gleichnamig) |
| `bulk::{SourceMode, resolve_root_and_source, scratch_path, names::*, merge_input_path, merge_overrides, split_overrides, edit_overrides, version_overrides}` | `documents/bulk_edit.py` (`_resolve_root_and_source_doc`, `_scratch_path`, Dateinamen/Overrides in `rotate`, `merge`, `split`, `delete_pages`, `edit_pdf`, `remove_password`) |
| `barcodes::{BarcodeConfig, Barcode, StoredBarcode, scannable_mime_types, separation_pages, located_asn, tag_names, scan_pdf, separate_pages, read_barcode_values, BarcodeReader, RxingReader, zxing_format_name}` | `documents/barcodes.py` (`Barcode`, `BarcodePlugin.{able_to_run, asn, tags, get_separation_pages, separate_pages}`, `scannable_mime_types`, `read_barcodes_zxing`, `scan_pdf`, `read_barcode_values`), `paperless/config.py::BarcodeConfig` |
| `convert::convert_from_tiff_to_pdf` | `documents/converters.py::convert_from_tiff_to_pdf` (+ `utils.remove_alpha_channel`, img2pdf `convert`/`pil_get_dpi`) |
| `repair::{replace_input, repair_bytes, RepairOutcome}` | `consumer.py` (`qpdf --replace-input` bei `.pdf` + `application/octet-stream`), `parsers.py::_repair_pdf_with_qpdf` (`--warning-exit-0`) |
| `thumbnail::{ThumbnailFont, make_text_thumbnail, text_thumbnail_webp, render_text}` | `paperless/parsers/text.py::TextDocumentParser.get_thumbnail` |
| `scanner::{NativeBarcodes, split_pdf}` | Bausteine für pl-consumer `stages::BarcodeScanner` / `PdfPageOps::tiff_to_pdf` |
| `pdf::*` | pikepdf/qpdf-Grundsemantik (öffnen, Seiten kopieren, drehen, löschen, speichern) |
| `copy_basic_file_stats` | `documents/utils.py::copy_basic_file_stats` |

## 1. Fehler (`PdfOpsError`)

`kind()` liefert den Python-Klassennamen; `to_string()` den upstream-Text.

| Variante | Python | Text (upstream) |
|---|---|---|
| `Value` | `ValueError` | `Page numbers start at 1, got {p}`, `Output document index is out of bounds`, `Multiple output documents specified` |
| `Index` | `IndexError` | `Page {p} is out of range, the PDF has {n} pages` (build_pdfs), `Accessing nonexistent PDF page number` (remove_pages, pikepdf) |
| `Password` | `pikepdf.PasswordError` | `{pfad}: invalid password` |
| `Runtime` | `RuntimeError` (qpdf) | `QPDF::rotatePage called with an angle that is not a multiple of 90` |
| `Pdf` | `pikepdf.PdfError` | `{pfad}: {lopdf-Fehler}` (Text weicht ab, qpdf-Texte nicht nachgebildet) |
| `UnsupportedEncryption` | `PdfError` | Public-Key-/Fremd-Handler, unbekannte V/R |
| `Unavailable` | – | Renderer (PDFium) oder Schrift fehlt |
| `Io` | `OSError` | – |

`bulk_edit.edit_pdf`/`remove_password` machen daraus `ValueError("An error occurred while editing the
document: {e}")` bzw. `("An error occurred while removing the password: {e}")` – das macht der Aufrufer.

## 2. PDF-Grundsemantik (pikepdf/qpdf, empirisch)

- **Öffnen** (`pdf::load(path, password)`): Kopf `%PDF-` wird in den ersten 1024 Bytes gesucht (qpdf;
  z. B. `invalid_pdf.pdf` mit 4 Müll-Bytes öffnet). Kaputte Querverweise: lopdf baut sie durch
  Durchsuchen neu auf. Verschlüsselt: leeres Benutzerpasswort → öffnet (`was_encrypted()`), sonst ohne
  bzw. mit falschem Passwort `PasswordError`. **Wird ein Passwort angegeben, zählt nur dieses** (qpdf):
  falsches Passwort bei einer Datei mit leerem Benutzerpasswort → `PasswordError`, obwohl sie ohne
  Passwort öffnen würde (lopdf probiert sonst zuerst ""; hier nachgeprüft über `/Encrypt`).
  Leeres Passwort = kein Passwort.
- **Verschlüsselung (lopdf 0.47)**: Standard-Sicherheits-Handler V1/V2 (RC4 40–128 Bit, R2/R3),
  V4 (RC4 oder AESV2 128 Bit, R4), V5 (AESV3 256 Bit, R5/R6, Passwort mit SASLprep). Geprüft:
  R2 RC4-40, R3 RC4-128 (`password-is-test.pdf`), R4 AES-128, R6 AES-256 (Benutzer- und
  Besitzerpasswort, Umlaut-Passwort), R4 nur Besitzerpasswort. **Nicht**: Public-Key-Handler
  (`/Adobe.PubSec`), eigene Handler (→ `UnsupportedEncryption`).
- **Speichern** (`pdf::save`): nie verschlüsselt (pikepdf `save()` ohne `encryption` entfernt sie
  ebenfalls); nur erreichbare Objekte; ab Version 1.5 Objekt-/Querverweisströme, darunter klassische
  xref-Tabelle (die Version bleibt dabei unverändert).
- **Drehen** (`pdf::rotate_page`, qpdf `rotatePage`): `angle % 90 != 0` → `RuntimeError`; relativ:
  wirksamer (auch vererbter) Wert, ungültiger alter Wert (kein Vielfaches von 90) zählt als 0;
  `neu = (alt + angle + 360) % 360` mit C-Rest (0 − 450 → −90); immer direkt in die Seite geschrieben.
- **Neues Dokument** (`pikepdf.new()`): PDF 1.3, Katalog + leerer Seitenbaum, **kein** `/Info`.
- **Seite anhängen** (`PageCopier`, qpdf `copyForeignObject` + `addPage`): je Quelle ein Objekt-Cache
  (geteilte Ressourcen einmal), vererbte `Resources/MediaBox/CropBox/Rotate` in die Seite,
  `/Parent` und Seitenbaum-Knoten werden nicht kopiert; dieselbe Seite zweimal → flache Kopie.
- **Seite löschen** (`del pdf.pages[i]`): nur aus `/Kids`, `/Count` der Vorfahren angepasst; das
  Objekt bleibt, solange z. B. Lesezeichen darauf zeigen.
- **`remove_unreferenced_resources`**: je Seite Einträge der Ressourcentypen ExtGState, ColorSpace,
  Pattern, Shading, XObject, Font, Properties entfernen, deren Name im Inhaltsstrom nicht vorkommt
  (Formular-XObjects rekursiv; nicht zerlegbarer Inhalt → unverändert). Ressourcen werden dafür
  seitenlokal kopiert, geteilte Objekte nie verändert.

## 3. `ops` (= `pdf_ops.py`)

| Funktion | Verhalten |
|---|---|
| `rotate_pdf(src, dst, degrees)` | Datei öffnen (ohne Passwort), jede Seite relativ drehen, Ressourcen bereinigen, speichern. Info, XMP, Lesezeichen, Version bleiben. |
| `remove_pages(src, dst, pages)` | Duplikate weg, sortiert; `< 1` → `ValueError` (vor dem Öffnen); höchste zuerst entfernen; zu große Nummer → `IndexError("Accessing nonexistent PDF page number")`. Info/XMP/Lesezeichen bleiben. |
| `build_pdfs(src, outputs, make_dst)` | Alle Nummern `< 1` → `ValueError` vor dem Öffnen; alle `> n` → `IndexError` vor jeder Ausgabe; je Ausgabe neues PDF (1.3, ohne Info/XMP/Lesezeichen), Seiten in Reihenfolge (Duplikate erlaubt), `rotate ≠ 0` relativ; `make_dst(i)` unmittelbar vor dem Speichern. Leere Liste → PDF mit 0 Seiten. |
| `validate_page_operations(ops, single_output)` | leer → `Output document index is out of bounds`; `single_output` und max(doc) > 0 → `Multiple output documents specified`; ein `doc < 0` oder `≥ len(ops)` → out of bounds; Ergebnis `max(doc)+1`. |
| `page_specs_for(ops, n)` | `edit_pdf`: `page_specs[op.doc or 0].append(PageSpec(page, rotate or 0))` |
| `needs_decrypt(src)` | ohne Passwort öffnen: `was_encrypted()`; `PasswordError` → `true`; andere Fehler weiter. |
| `decrypt_pdf(src, make_dst, password)` | mit Passwort öffnen (falsch → `PasswordError`, `make_dst` nicht aufgerufen), bereinigen, unverschlüsselt speichern. Info/XMP/Lesezeichen bleiben. |
| `PdfMerger::{new, add, add_document, save}` | Startversion "1.3"; `add` öffnet ohne Passwort (Fehler → Aufrufer überspringt), Version = `max` als **Zeichenkette** wie Python; `save` mit dieser Mindestversion, ohne Info. |

`PageSpec`/`PageOperation` sind `serde`-fähig (`{"page":1,"rotate":90,"doc":0}`).

## 4. bulk_edit-Abläufe (für pl-documents-write)

Alle PDF-Methoden erzeugen **neue Dokumente/Versionen über `consume_file`**; Archiv, Prüfsummen,
Vorschaubild, Seitenzahl, Inhalt entstehen dabei im Consumer neu (auch bei `rotate`: upstream v3.4.2
dreht nicht mehr an Ort und Stelle). `trigger_source` aus dem Aufruf (Standard `web_ui`),
`ConsumableDocument(source=ConsumeFolder, original_file=…, root_document_id=…)`.

| Methode | Ablauf (upstream) | pl-pdfops |
|---|---|---|
| `rotate(doc_ids, degrees, source_mode, user)` | Dokumente per `id__in`, je Wurzel nur das erste Paar (`setdefault`); Nicht-PDF-Quelle → Warnung `Document {root} is not a PDF, skipping rotation.`; `rotate_pdf` nach `scratch/{root}_rotated.pdf`; Overrides `from_document(root)` + `actor_id`; `consume_file(root_document_id=root)`; Fehler je Dokument nur geloggt. | `resolve_root_and_source`, `scratch_path`, `names::rotated`, `ops::rotate_pdf`, `version_overrides` |
| `merge(doc_ids, metadata_document_id, delete_originals, archive_fallback, source_mode, user)` | Reihenfolge = `doc_ids`; Eingabe `merge_input_path`; nicht lesbar → `Error merging document {id}, it will not be included in the merge: {e}`; keines → `No documents were merged`; Datei `names::merged(affected)`; Overrides `merge_overrides` (Titel `"{t} (merged)"`, `owner_id=user`, `skip_asn_if_exists` ohne delete_originals, ASN-Übergabe mit delete_originals); mit delete_originals: `release_archive_serial_numbers(affected)` → `consume_file` mit `link=[delete(affected)]`, `link_error=[restore_archive_serial_numbers_task(backup)]` | `PdfMerger`, `names::merged`, `merge_overrides` |
| `merge_as_versions(doc_ids, root_document_id, version_label, user)` | **kein PDF-Teil**: Transaktion mit `select_for_update` (nach id), Quellen bekommen `root_document`, fortlaufenden `version_index`, ASN `None` (+`version_label`), `bulk_update`; Wurzel: `modified`, erste Quell-ASN, wenn die Wurzel keine hat (Log `Document {root} took archive serial number …`), restliche → Warnung; Audit-Log `{"Merged As Versions": ["None", ids]}` mit `additional_data`; Index-Entfernen im Batch (Sperre → `remove_document_from_index` countdown 60); `bulk_update_documents([root])` (trigger system); WebSocket `documents_deleted(source_ids)` | – (pl-documents-write) |
| `split(doc_ids, pages, delete_originals, source_mode, user)` | nur `doc_ids[0]`; `build_pdfs` mit `names::split(doc.id, first, last)` (ausgewählte id!); je Teil `split_overrides` (Titel `"{title} (split {i+1})"` des *ausgewählten* Dokuments); delete_originals: ASN freigeben + `chord(consume…, delete([id])).on_error(restore)`; sonst `group`. Fehler nur geloggt (`Error splitting document {id}: {e}`). | `ops::build_pdfs`, `names::split`, `split_overrides` |
| `delete_pages(doc_ids, pages, source_mode, user)` | `remove_pages` → `{root}_pages_deleted.pdf`, neue Version (`version_overrides`). Fehler nur geloggt. | `ops::remove_pages`, `names::pages_deleted` |
| `edit_pdf(doc_ids, operations, delete_original, update_document, include_metadata, source_mode, user)` | `validate_page_operations(single_output=update_document)`, `page_specs_for`; update_document: eine Ausgabe `{root}_edited.pdf` als neue Version; sonst `{root}_edit_{i}.pdf` (ab 1) als neue Dokumente; Overrides `edit_overrides(..., for_edit=true)`; delete_original: ASN freigeben + chord + restore. **Fehler → `ValueError("An error occurred while editing the document: {e}")`**. | `ops::{validate_page_operations, page_specs_for, build_pdfs}`, `names::{edited, edit_part}`, `edit_overrides` |
| `remove_password(doc_ids, password, update_document, delete_original, include_metadata, source_mode, user, source_paths_by_id)` | je Dokument: `needs_decrypt` false → Info-Log und weiter; `decrypt_pdf` → `{root}_unprotected.pdf`; update_document → neue Version, sonst neues Dokument (`chord(..., delete([id]))` ohne ASN-Logik bei delete_original); Overrides `edit_overrides(..., for_edit=false)`. **Fehler → `ValueError("An error occurred while removing the password: {e}")`**. | `ops::{needs_decrypt, decrypt_pdf}`, `names::unprotected`, `edit_overrides` |

## 5. Barcodes (= `barcodes.py`)

- `BarcodeConfig::from_env()`: `PAPERLESS_CONSUMER_ENABLE_BARCODES`, `…_BARCODE_TIFF_SUPPORT`,
  `…_BARCODE_STRING` (PATCHT), `…_BARCODE_RETAIN_SPLIT_PAGES`, `…_ENABLE_ASN_BARCODE`,
  `…_ASN_BARCODE_PREFIX` (ASN), `…_BARCODE_UPSCALE` (0.0), `…_BARCODE_DPI` (300),
  `…_BARCODE_MAX_PAGES` (0 = alle), `…_ENABLE_TAG_BARCODE`, `…_TAG_BARCODE_MAPPING`
  (`{"TAG:(.*)": "\\g<1>"}`, JSON-Reihenfolge bleibt), `…_TAG_BARCODE_SPLIT`, `…_STORE_BARCODE_VALUES`.
  DB-Overrides (`ApplicationConfiguration.barcode_*`) setzt der Aufrufer. `able_to_run(mime)`,
  `scannable_mime_types` (`application/pdf`, mit TIFF-Schalter + `image/tiff`).
- `scan_pdf(pdf, ScanSettings{dpi, upscale, max_pages}, renderer, reader)`: Seitenzahl über lopdf
  (verschlüsselt → `PasswordError`, wie pikepdf), je Seite bis `max_pages` rastern mit `dpi`
  (Graustufen, ITU-R 601 wie Pillow `L`), `upscale > 1.0` → `resize(round(w·f), round(h·f))` (Python-
  Rundung, Catmull-Rom = Pillow BICUBIC), lesen mit `BarcodeReader`; leere Texte entfallen.
  `Barcode{page (0-basiert), value, format}`; `stored()` → Seite 1-basiert.
- `RxingReader`: rxing `GenericMultipleBarcodeReader` mit `TryHarder` + `AlsoInverted` (zxing-cpp:
  try_harder/try_rotate/try_invert). Ersatz für zxing-cpp `try_downscale`: findet die volle
  Auflösung nichts, wird verkleinert (1/2, 1/3, 1/6, kürzere Seite ≥ 300 px) erneut gelesen, bis eine
  Stufe Treffer hat (alle Stufen zu vereinigen ergab Doppel- und UPC-E-Fehltreffer). Treffer aus einer
  Stufe mit gleichem Text/Format und gleicher Lage werden nicht doppelt gezählt. Formatnamen = zxing-cpp-3-Enum (`DocumentBarcode.Format`):
  Aztec, Codabar, Code39, Code93, Code128, DataMatrix, EAN8, EAN13, ITF, MaxiCode, PDF417, QRCode,
  MicroQRCode, RMQRCode, DataBar, DataBarExp, Telepen, UPCA, UPCE, EAN5, DXFilmEdge, sonst OtherBarcode.
- `separation_pages(codes, cfg)` (`get_separation_pages`, nur bei `barcodes_enabled`): Trenner
  (`value == barcode_string`) → Seite: `retain` (mit retain nicht Seite 0); ASN-Codes (`starts_with`
  Präfix, nicht Seite 0) → `true`; mit `tag_split && enable_tag` Tag-Codes (Muster passt, IGNORECASE,
  nicht Seite 0) → `true`. Spätere Regeln überschreiben frühere.
- `located_asn`: erster Code mit Präfix, Präfix ab, `strip`, Nicht-Ziffern weg, `int()`; Fehler →
  `Err("invalid literal for int() with base 10: '…'")` (upstream: Warnung
  `Failed to parse ASN number because: …`).
- `tag_names`: je Code, an `,` getrennt, erste passende Zuordnung (pl-matching `safe_regex_match`/
  `safe_regex_sub`, IGNORECASE); leere Ersetzung → Rohwert; leere Ergebnisse entfallen. Liefert
  `(roh, name)`; `Tag.objects.get_or_create(name__iexact=name)` macht der Aufrufer.
- `separate_pages(pdf, pages, fname, out_dir, stats_source)`: neue Dokumente (pikepdf `Pdf.new()`, 1.3,
  ohne Info) an Trennseiten (Wert = Seite behalten), leere Teile entfallen, Namen
  `{fname}_document_{i}.pdf`, Dateizeiten von `stats_source`.
- `read_barcode_values(path, mime, cfg, work_dir, renderer, reader)`: `None` bei nicht scanbarem Typ,
  TIFF vorher umwandeln.

### Einbindung in pl-consumer (Adapter, im pl-consumer-Crate)

```rust
struct PdfOpsBarcodes(pl_pdfops::scanner::NativeBarcodes);
impl BarcodeScanner for PdfOpsBarcodes {
    fn available(&self) -> bool { self.0.available() }
    fn scan(&self, pdf: &Path, tmp: &Path) -> Result<Vec<Barcode>, StageError> {
        self.0.scan(pdf, tmp)
            .map(|v| v.into_iter().map(|b| Barcode { page: b.page, value: b.value, format: b.format }).collect())
            .map_err(|e| StageError::Other(e.to_string()))
    }
    fn split(&self, pdf: &Path, pages: &[(i64, bool)], tmp: &Path) -> Result<Vec<PathBuf>, StageError> {
        self.0.split(pdf, pages, tmp).map_err(|e| StageError::Other(e.to_string()))
    }
}
// PdfPageOps::tiff_to_pdf → pl_pdfops::convert::convert_from_tiff_to_pdf(tiff, tempdir)
```

`NativeBarcodes::new(renderer, ScanSettings{dpi, upscale, max_pages})` (Werte inkl. DB-Overrides)
bzw. `from_env()`. Die Passwort-Meldung `File is likely password protected, not checking for barcodes:
{e}` liefert `NativeBarcodes::detect` (der Consumer meldet heute alle Fehler als `Exception during
barcode scanning`; mit `e.kind() == "PasswordError"` unterscheidbar). `split` benennt nach dem Stamm
von `pdf` (= Stamm des Originals, auch nach TIFF-Umwandlung).

## 6. TIFF → PDF (`convert_from_tiff_to_pdf`, img2pdf-Ersatz)

Ziel `target/<name>.pdf`, Dateizeiten vom TIFF. Alle Bilder des TIFF (pl-ocr `decode_frames`), Alpha
verworfen (nicht verrechnet), Seitengröße = Pixel / dpi · 72 pt mit img2pdf-DPI (gerundet; fehlt, 0/0
oder Pillow-Standard 1/1 → 96); CCITT-Gruppe-4-Daten (ein Streifen, Photometric 0/1, FillOrder 1/2) wie
img2pdf unverändert als `/CCITTFaxDecode` (`/K -1`, Photometric 1 → `/BlackIs1 true`), JPEG-Daten
(Kompression 7, ein Streifen, 8 Bit, Grau/YCbCr/RGB, `JPEGTables` eingefügt) unverändert als `/DCTDecode`
(RGB ohne YCbCr → `/ColorTransform 0`) – `convert::tiff_raw_frames`; sonst verlustfrei Flate (reines
Schwarzweiß als 1 Bit, sonst 8-Bit Grau/RGB);
PDF 1.3, `/Info` nur `CreationDate`/`ModDate` (UTC, wie img2pdf). Pixelgrenze
`PAPERLESS_MAX_IMAGE_PIXELS` wie `maybe_override_pixel_limit`.

## 7. qpdf-Ersatz (`repair`)

`replace_input(path)` = `qpdf --replace-input`: Kopf in den ersten 1024 Bytes suchen (Müll davor fällt
weg), lesen (Querverweise ggf. neu aufgebaut), sauber neu schreiben (atomar über Nachbardatei).
`RepairOutcome::Clean` (Exit 0) bzw. `RepairedWithWarnings` (Querverweise rekonstruiert; qpdf schreibt
und endet mit **Exit 3**). Kein PDF/unlesbar → `Pdf`-Fehler, Datei unverändert (qpdf Exit 2).
Upstream-Folge im Consumer: ohne `--warning-exit-0` löst Exit 3 in `run_subprocess` eine Ausnahme
aus → `Error attempting to clean PDF: …`, MIME wird **nicht** neu bestimmt (obwohl die Datei ersetzt
ist). Der Aufrufer bildet das über `outcome.exit_code()` nach; der Vorschaubild-Pfad
(`_repair_pdf_with_qpdf`, `--warning-exit-0`) akzeptiert beide.

## 8. Text-Vorschaubild (`thumbnail`)

`make_text_thumbnail(path, font, tempdir)` → `tempdir/thumb.webp`: > 50 MiB → Text
`[File too large to preview]`, sonst UTF-8 mit Ersatzzeichen, universelle Zeilenenden, erste 100 000
Zeichen; Bild 500×700 weiß, Text ab (5, 5), Em 20 px, Zeilenabstand = Oberlänge (aufgerundet) + 4,
schwarz mit Kantenglättung, Unterschneidung aus der Schrift. Schrift `PAPERLESS_THUMBNAIL_FONT_NAME`
(Standard `/usr/share/fonts/liberation/LiberationSerif-Regular.ttf`; fehlt sie → `Unavailable`,
Aufrufer nimmt dann wie bisher `document.webp`). Für **Office/Mail** (upstream: erste Seite des
Gotenberg-PDFs) kann der Consumer `text_thumbnail_webp(extrahierter_text, font)` statt des
Standardbilds verwenden (keine weitere Abhängigkeit nötig; kein Layout/keine Bilder des Originals).

## Tests

- `cargo test -p pl-pdfops -j 1`: 25 Unit-Tests ohne DB/PDFium: qpdf-Drehsemantik (alle Grenzfälle
  aus der Referenz), Seitenkopie (Vererbung, Duplikate, kein Info), Ressourcen-Bereinigung,
  Seitenbaum, Lesefehler, `rotate_pdf`/`remove_pages`/`build_pdfs`/`validate_page_operations`/
  `PdfMerger`/`needs_decrypt`, Barcode-Logik (Trennseiten mit retain/ASN/Tags, ASN-Parsing, Tag-
  Zuordnung mit Ersetzung, MIME/able_to_run), `separate_pages`, rxing liest erzeugten QR + Code128,
  TIFF→PDF (mehrseitig, DPI), qpdf-Ersatz (Müll vor dem Kopf, kaputter startxref, kein PDF),
  Text-Vorschau (Windows-/Linux-Systemschrift, sonst übersprungen), bulk-Entscheidungslogik.
- Parität (`tests/parity/pl-pdfops/`, ohne DB):
  1. `source reference/env.sh && "$PY" tests/parity/pl-pdfops/gen_reference.py` – erzeugt Hilfs-PDFs
     (vererbte Attribute, Lesezeichen+XMP, PDF 1.4, R2/R4/R6/Besitzer-only-Verschlüsselung mit pikepdf)
     und führt unverändertes upstream (`documents.pdf_ops`, `converters`, `barcodes.scan_pdf`,
     `BarcodePlugin.get_separation_pages/asn/tags/separate_pages`) aus. pdf2image (poppler fehlt
     in der Windows-Referenz) wird durch pypdfium2 mit gleicher DPI ersetzt; `Tag` durch eine Attrappe.
  2. `cargo test -p pl-pdfops -j 1 --test parity -- --ignored` (PDFium: `PAPERRUST_PDFIUM_PATH`,
     Standard die pdfium.dll aus `reference/venv/…/pypdfium2_raw` – dieselbe wie oben). Für
     erträgliche Laufzeit im Debug-Build mit `--config 'profile.dev.package.rxing.opt-level=3'`.
  3. `"$PY" tests/parity/pl-pdfops/compare.py` – liest beide Ausgaben mit pikepdf + pypdfium2:
     Seitenzahl, Text je Seite, wirksame Drehung, MediaBox, Version, Info-Schlüssel/Titel, XMP,
     Lesezeichen, Verschlüsselung, Rasterabweichung; Barcodes (Seite/Wert/Format), Trennseiten, ASN,
     Tags, gespeicherte Werte, Teil-PDFs.
  40 Operationen (rotate ×8, remove_pages ×4, build ×7, merge ×3, needs_decrypt ×3, decrypt ×8,
  validate ×4, TIFF ×3), 27 Barcode-Dateien × 8 Konfigurationen, 2 Scan-Varianten
  (600 dpi + Upscale 1.5, max_pages=1). Ergebnis: siehe Abschnitt „Paritätsstand“.

## Paritätsstand (2026-11-08)

`compare.py`: **0 Unterschiede**.
- PDF-Operationen: alle 40 Fälle gleich (Status, Fehlerart, Fehlertext, und je Ausgabe Seitenzahl,
  Text je Seite, Drehung, MediaBox, Version, Info-Schlüssel/Titel, XMP, Lesezeichen,
  Verschlüsselung, Raster ≤ 2 Graustufen mittlere Abweichung).
- Barcodes: alle 27 Dateien (inkl. 2 TIFF und dem verschlüsselten PDF → `PasswordError`) gleiche
  Codes (Seite, Wert, Format, auch Anzahl – z. B. 6 Codes auf einer Seite in `many-qr-codes.pdf`),
  alle 8 Konfigurationen gleiche Trennseiten, ASN, Warnungen, Tags, gespeicherte Werte und Teil-PDFs
  (Seitenzahl/Text/Drehung); beide Scan-Varianten (600 dpi + Upscale 1.5; max_pages=1) gleich.
  Ohne die Verkleinerungsstufen fehlten `many-qr-codes.pdf` und `patch-code-t-middle-distorted.pdf`.
- Laufzeit: Rust-Parität ~15 min im Debug-Build mit optimiertem rxing (≈ 4 s je Seite bei 300 dpi,
  unoptimiert ≈ 50 s), upstream ~3 min.

## Known differences

1. **Rastern**: upstream pdf2image/pdftoppm (poppler), hier PDFium (pl-ocr). Pixel weichen leicht ab;
   Barcode-Erkennung kann in Grenzfällen (verzerrt/unscharf) abweichen.
2. **Barcode-Leser**: rxing (ZXing-Java-Port) statt zxing-cpp 3.1. Gleiche Formatnamen, auf allen
   upstream-Beispielen gleiche Ergebnisse, aber eigene Erkennungsleistung (andere Scans können in
   Grenzfällen abweichen; die Verkleinerungsstufen sind eine Näherung an `try_downscale`). Code39
   nur im Standardmodus (zxing-cpp meldet hier ebenfalls `Code39`). Ohne optimiertes rxing sehr
   langsam (CORE-REQUEST).
3. **`PdfError`-Texte** stammen von lopdf, nicht von qpdf.
4. **Datei-Layout**: lopdf schreibt eigene Objektnummern/Ströme (ab 1.5 Objektströme) – Inhalt,
   Seiten, Drehung, Metadaten wie upstream, Bytes nicht. Ressourcen-Bereinigung ist konservativ (jeder
   Name im Inhaltsstrom gilt als Verweis), Dateien können etwas größer bleiben.
5. **TIFF→PDF** (Lücke E18): dpi je Einzelbild (Pillow-Regeln: `ResolutionUnit` 2/fehlt → Wert,
   3 → ·2,54, 1 → 96) und Orientation wie img2pdf als Seiten-`/Rotate` (3 → 180, 6 → 90, 8 → 270) –
   wie upstream; G4 wird wie bei img2pdf unverändert übernommen. Abweichend: JPEG im TIFF wird ebenfalls
   unverändert übernommen (img2pdf dekodiert und schreibt Flate – Pixel gleich, Datei bei uns ~5× kleiner);
   G4/JPEG mit mehreren Streifen oder Kacheln sowie gespiegelte Seiten werden verlustfrei als Flate neu
   kodiert. Gespiegelte Orientation (2/4/5/7) stellt PaperRust in den Pixeln richtig, img2pdf bricht dort
   mit `ExifOrientationError` ab.
6. **ASN mit Nicht-ASCII-Ziffern** (z. B. arabisch-indisch): Python `\D`/`int()` akzeptiert sie, hier
   nur ASCII-Ziffern; ASN > i64 → Parse-Fehler (Python: beliebig groß, scheitert dann in der DB).
7. **Text-Vorschau**: WebP verlustfrei (image-webp hat keinen verlustbehafteten Kodierer; Pillow:
   Qualität 80), Glyphen über ab_glyph statt FreeType (keine Hinting-Unterschiede nachgebildet).
   Office/Mail: Text statt gerenderter erster Seite (upstream braucht Gotenberg).
8. **qpdf-Warnungen**: nur „Querverweise neu aufgebaut“ zählt als Warnung (Exit 3); qpdf kennt
   weitere Warnungen (z. B. reparierte Ströme), die hier als `Clean` gelten.
9. **Verschlüsselung**: Public-Key-Handler und fremde Handler nicht unterstützt (upstream/qpdf
   ebenfalls nicht für Public-Key) → `UnsupportedEncryption` statt `PasswordError`.

## CORE-REQUEST

- `[workspace.dependencies]`: `lopdf = { version = "=0.47.0", default-features = false }`,
  `image = { version = "=0.25.10", default-features = false, features = [...] }` (beide schon in
  pl-ocr/pl-parsers/pl-consumer fest), neu `rxing = { version = "=0.9.3", default-features = false,
  features = ["full_barcode_format_support", "multi_barcode_readers", "decoders", "encoding_rs"] }`
  (Apache-2.0, ohne die `image`-Funktion – die pinnt image =0.25.8 + imageproc),
  `ab_glyph = "=0.2.32"` (Apache-2.0), dev `tiff = "=0.11.3"`, sowie `pl-ocr`/`pl-pdfops` als
  Pfad-Abhängigkeiten.
- `[profile.dev.package.rxing] opt-level = 3`: rxing ist unoptimiert ~50 s je Seite bei 300 dpi
  (optimiert < 1 s); betrifft Tests/Debug-Worker.
