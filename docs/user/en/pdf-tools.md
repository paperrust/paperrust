# PDF tools: redact, shrink, repair

[Deutsch](../pdf-tools.md)

PaperRust comes with three PDF tools (plus [Convert to PDF](#in-pdf-umwandeln) for images and office files). They are built on components of
[PdfCraft](https://github.com/storytold/pdfcraft) (MIT OR Apache-2.0) and run in a separate,
isolated program (`paperrust-pdftool`) with a time and memory limit. A broken or malicious PDF
can therefore neither hang nor crash the server – at worst this one operation fails.

Redact and shrink always create a **new version** of the document. The previous version stays in
the version history.

<a id="schwaerzen"></a>
## Redact

Open the document → **Actions** → **Redact…**

- **Mark areas:** drag rectangles with the mouse over what has to go. Remove a rectangle with its ×.
- **Search terms:** enter terms on the right (one per line, e.g. a name or an account number) and
  press **Search**. All matches are preselected and marked on the pages. Untick what should stay.
  Case does not matter; the whole word containing the match is redacted.
- **Remove metadata** (optional): deletes title, author and XMP metadata from the PDF file.
- **Redact permanently** creates the new version.

What "redact" means here: the content under the marks is **really removed from the file** – text,
images (the covered pixels are deleted) and vector graphics. It is not just a black box drawn on
top. Afterwards PaperRust checks the result: if a searched term is still readable, **nothing** is
saved and an error is shown. The new version is ingested like a new document (OCR, thumbnail,
search index); the redacted text is not in the content of the new version either.

**Scans:** in scanned documents the text only exists in the recognised text layer (OCR) of the
archive PDF. If the search finds nothing in the original, PaperRust searches there and blacks out
the matching image areas in the original.

> **Important:** the old version with the unredacted content stays in the version history. If it
> must go too, delete it there (versions menu of the document). Older backups and copies already
> sent out still contain the content, of course.

**Sharing redacted documents:** a share link shows exactly one version. If a document has redacted
versions, the share dialog shows a notice: which version is redacted and which one is fully
readable. For new links the newest redacted version is preselected; any version can be chosen. Each
existing link says which version it shows. Share bundles (several documents) show a notice too;
there "Use the redacted version" is switched on. Older links to the main document still show
**version 1** (not redacted).

Limits: terms across a line break are not found – mark such places as a rectangle. Vertical text
(e.g. vertical Japanese) under a mark is refused rather than redacted unsafely.

<a id="verkleinern"></a>
## Shrink

Open the document → **Actions** → **Shrink…** and pick a level:

| Level | What happens |
|---|---|
| **Lossless** | Images stay as they are. Duplicate and unused objects are removed, uncompressed data is compressed. |
| **Standard** | additionally: colour and greyscale images above 225 dpi to 150 dpi, JPEG quality 60; black-and-white scans to 300 dpi (lossless CCITT G4). Good for archiving. |
| **Strong** | images above 144 dpi to 96 dpi, JPEG quality 80. For reading on screen; small print may get blurry. |

PaperRust then shows the size before and after. A new version is only created if the file really
gets smaller.

<a id="verkleinern-beim-einlesen"></a>
### Shrink on ingest

With [`PAPERRUST_SHRINK_ON_INGEST_MB`](configuration.md#paperrust_shrink_on_ingest_mb) (default `0` =
off), **scans** larger than this are shrunk on ingest already, with the level from
[`PAPERRUST_SHRINK_PRESET`](configuration.md#paperrust_shrink_preset). A PDF counts as a scan when its
first three pages have fewer than 50 characters of text. The smaller file is then stored (that is the
point of the setting); if it does not get smaller, nothing changes. Both values can also be changed
in the UI under **Settings → System** (consumer section).

<a id="in-pdf-umwandeln"></a>
## Convert to PDF (images and office files)

Redact and shrink only work with PDFs. Images and office files can therefore be converted to a PDF
first: open the document → **Actions** → **Convert to PDF…** (or select several documents in the
document list → **Actions** → **Convert to PDF…**). The PDF becomes a **new version** of the
document; the previous version (the image or office file) stays in the version history. Afterwards
redact and shrink are available.

| Kind | Formats | How |
|---|---|---|
| Images | PNG, JPEG, TIFF (multi-page too, one PDF page per TIFF page), WebP, BMP, GIF | JPEG is embedded unchanged, the others losslessly. Images with transparency are placed on a white background. With **Recognise text (OCR)** the PDF gets an invisible text layer (Tesseract, same language as on ingest) and becomes searchable. |
| Office | Word (.docx, .docm, .dotx), OpenDocument text (.odt, .ott), RTF; also Markdown and HTML documents | Converted with [WordCraft](https://github.com/storytold/wordcraft) (MIT OR Apache-2.0) in the isolated program `paperrust-pdftool`. Text stays real text (searchable, redactable); no OCR needed. |

Not converted by "Convert to PDF": spreadsheets (.xlsx, .xls, .ods), presentations (.pptx, .ppt,
.odp), legacy Word (.doc), HEIC photos and PDFs (already PDF). Spreadsheets, presentations, legacy
Word and e-mails get their PDF when they are consumed, though (next section). For files that were
already consumed: reprocess the document.

**Permissions:** as for redact – whoever may change the document.

<a id="office-beim-einlesen"></a>
### Office files on ingest

Word, OpenDocument and RTF files get a real archive PDF **on ingest** (with WordCraft), as
paperless-ngx does with Gotenberg. The same goes for:

- **Legacy Word (.doc):** typeset like a .docx (WordCraft reads the Word 97 format).
- **Spreadsheets (.xlsx, .xls, .ods):** one page per sheet with heading and table; landscape from
  seven columns on. Very large sheets are cut off after 200,000 cells (with a note line).
- **Presentations (.pptx, .ppsx, .ppt, .odp) and drawings (.odg):** one page per slide (A4
  landscape) with slide number, title, text and notes.
- **E-mails (.eml):** a header page with sender, recipients, date, subject, attachments and the text;
  if the mail has an HTML part, it follows on its own pages (with embedded images). The order is set
  by the mail rule (**PDF layout**: text then HTML / HTML then text / HTML only / text only); without
  a rule [`PAPERLESS_EMAIL_PARSE_DEFAULT_LAYOUT`](configuration.md) applies (default: text then HTML).

The searchable text still comes from PaperRust's own readers, not from the PDF. The thumbnail is then
the first page of the PDF instead of a plain text image, the page count is shown and the browser
preview shows the typeset document. Switch off with [`PAPERRUST_OFFICE_TO_PDF`](configuration.md#paperrust_office_to_pdf)`=false`.

If WordCraft cannot convert a file (broken file, time limit, tool crash), the log says "Office to PDF
conversion failed, continuing without archive PDF: …" and ingest continues as before: the original
is stored without an archive PDF, with a text thumbnail.

<a id="office-grenzen"></a>
### Limits of the office conversion

WordCraft is still alpha software; the result usually looks like in Word, but not always pixel for
pixel. Checked (headings, tables, bullet lists, numbered lists, pictures, page breaks, umlauts and
€) – known differences:

- **Fonts:** if e.g. Calibri is requested and not installed (Docker image), the text looks different
  from Word (in the test: Calibri was replaced by a serif font). Content and breaks stay similar, the
  typeface does not.
- **OpenDocument without a page style:** if an .odt has no page style (rare, only files written by
  programs), WordCraft sets the text without page margins; LibreOffice uses 2 cm then.
- **Not checked** (may be missing or differ): footnotes, text boxes/shapes, charts, embedded objects
  (OLE), form fields, right-to-left scripts. Tracked changes are set as the final text (without
  markup), comments do not appear.
- **Spreadsheets:** no cell colours, border styles, charts or print areas; wide tables are squeezed
  to the page width instead of wrapping onto further pages. Numbers appear as stored in the file
  (date cells as dates).
- **Presentations:** only the text per slide (title, paragraphs, notes) – no pictures, shapes,
  charts or slide backgrounds.
- **E-mails:** HTML mails are typeset without a browser – tables, paragraphs, pictures and simple
  styles yes, elaborate CSS layouts no.

Fonts: if a font the document asks for is missing (e.g. Calibri, Aptos), WordCraft uses an installed
or a metric-compatible free font; Source Sans 3, Source Serif 4, Inter and JetBrains Mono (SIL OFL)
are bundled. Line and page breaks can shift slightly as a result.

<a id="reparieren"></a>
## Repair on ingest

If a PDF cannot be read during ingest (e.g. damaged cross-references, truncated file, junk before
the file header), PaperRust automatically tries a tolerant repair and continues with the repaired
copy. The log then says "PDF could not be read; repaired it with PdfCraft …" with a short
description of what was repaired. **The unmodified file, as delivered, is stored as the original.**
If the repair fails, ingest continues as before (and reports the read error if there is one).

## Settings and operation

| Variable | Default | Meaning |
|---|---|---|
| [`PAPERRUST_PDFTOOL_TIMEOUT`](configuration.md#paperrust_pdftool_timeout) | 120 | Time limit per operation in seconds |
| [`PAPERRUST_PDFTOOL_MEMORY_MB`](configuration.md#paperrust_pdftool_memory_mb) | 1536 | Memory limit of the tool |
| [`PAPERRUST_PDFTOOL_BIN`](configuration.md#paperrust_pdftool_bin) | next to `paperrust` | Path to the tool |
| [`PAPERRUST_OFFICE_TO_PDF`](configuration.md#paperrust_office_to_pdf) | true | Convert office files into an archive PDF on ingest |
| [`PAPERRUST_OFFICE_TO_PDF_TIMEOUT`](configuration.md#paperrust_office_to_pdf_timeout) | 0 | Time limit per office conversion (0 = same as `PAPERRUST_PDFTOOL_TIMEOUT`) |

The tool `paperrust-pdftool` must sit next to the `paperrust` program (or be given with
`PAPERRUST_PDFTOOL_BIN`). Without it, redact and shrink report "not installed"; ingest continues
without repair.

**Permissions:** everyone who may **change** the document can redact and shrink it (global
permission "change document" and change permission on the document).

For developers: the endpoints are listed under [REST API](api.md#pdf-werkzeuge) and in the contract
`docs/contract/pl-pdftools.md`.

---

According to its own description, PdfCraft is still alpha software. PaperRust only uses the
components for redacting, shrinking and repairing (not its UI, OCR, signatures or the like) and runs
them in isolation only. "PdfCraft"/"ArtCraft" and their logos are trademarks of their owners;
PaperRust is not affiliated with them.
