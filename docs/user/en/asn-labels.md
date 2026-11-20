# ASN labels and separator sheets

[Deutsch](../asn-labels.md)

With the **ASN label generator** PaperRust prints label sheets with consecutive archive serial
numbers (ASN) as barcodes. Stick one label on the first page of every paper document – then put
the whole stack into the scanner at once. PaperRust detects the labels, **splits the scan into
separate documents at every label** and gives each document **the number on its label**. File the
paper in number order and find it again by its ASN.

The page is in the interface under **Tools → ASN labels** (`/rs/app/tools/asn`). It requires the
"change documents" permission.

## One-time setup

Enable in **Configuration → Barcodes** (or as environment variables):

| Setting | Value | Effect |
|---|---|---|
| [`PAPERRUST_CONSUMER_ENABLE_BARCODES`](configuration.md#PAPERRUST_CONSUMER_ENABLE_BARCODES) | `true` | barcodes split batches |
| [`PAPERRUST_CONSUMER_ENABLE_ASN_BARCODE`](configuration.md#PAPERRUST_CONSUMER_ENABLE_ASN_BARCODE) | `true` | ASN barcodes are read and assigned |
| [`PAPERRUST_CONSUMER_ASN_BARCODE_PREFIX`](configuration.md#PAPERRUST_CONSUMER_ASN_BARCODE_PREFIX) | `ASN` (default) | prefix on the labels – the generator uses it automatically |

With **both** switches on, every page carrying an ASN label starts a new document (the page is
kept), exactly as in paperless-ngx. A separate separator sheet is **not** needed then. The
generator shows a notice at the top if anything is missing. PDFium must be available (it ships
with the desktop app and the Docker image).

## Workflow

1. **Print labels:** pick the sheet, check the start number (the next free number is suggested),
   choose the number of labels or sheets, **download the PDF** and print it.
2. **Stick:** one label on the **first page** of each document, ideally on an empty spot at the
   top right. The order of the numbers does not matter; use every number only once.
3. **Scan:** scan the whole stack as **one** PDF (feeder, duplex is fine) and drop it into the
   consumption folder or upload it.
4. **Done:** PaperRust splits the stack at every page with a label. Each part is then processed
   normally and gets the number of its label as ASN.

If the first page of the stack has no label, everything up to the first label becomes a document
without ASN. If a number is already taken, the behaviour is the same as in paperless-ngx: consuming
that document fails with "Given ASN … already exists!" (visible in the tasks); the other documents
of the stack are not affected.

### Alternative: separator sheets

If you do not want ASNs, put **separator sheets** between the documents. The generator prints them
with the configured separator barcode
([`PAPERRUST_CONSUMER_BARCODE_STRING`](configuration.md#PAPERRUST_CONSUMER_BARCODE_STRING), default
`PATCHT`). Only `PAPERRUST_CONSUMER_ENABLE_BARCODES` is required. The separator sheet is discarded
(unless
[`PAPERRUST_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES`](configuration.md#PAPERRUST_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES)
is on) and can be reused any number of times. Labels and separator sheets can be mixed in one
stack.

## Generator settings

| Field | Meaning |
|---|---|
| Label sheet | preset (see below) or **custom layout** with page size, rows, columns, label size, margins and gaps in mm |
| Code type | **QR** (default, most robust), **DataMatrix** (smallest), **Code 128** (1D, for hand scanners; needs wide labels) |
| Text | number also printed as plain text (on by default) |
| Start number | default: next free number |
| Digits | number padded with zeros (default 5: `ASN00042`); the consumer reads the number either way |
| Count / sheets | number of labels or whole sheets |
| Used labels | positions clicked in the preview stay empty – use up partially used sheets |
| Printer calibration | shift in mm (+ = right/down) if your printer prints with an offset |
| Test print | prints label outlines on plain paper, not recorded |

**Number protection:** every print is recorded with its number range, sheet and user. The start
number is always placed after the largest assigned **and** the largest printed number. If a print
overlaps existing ASNs (including the trash – ASNs are unique) or an earlier print, the generator
warns and prints only after confirmation. Misprints can be released in the history (creator or
superuser).

The largest storable ASN is 2,147,483,647 (the database column is `integer`, although
paperless-ngx accepts values up to 4,294,967,295).

## Label sheets

| Preset | Paper | Label | Per sheet | Dimensions |
|---|---|---|---|---|
| Avery Zweckform L4731REV-25 | A4 | 25.4 × 10 mm | 189 (7 × 27) | data sheet |
| HERMA 10001 | A4 | 25.4 × 10 mm | 189 (7 × 27) | grid assumed like L4731 – test print |
| Avery Zweckform L4736REV-25 | A4 | 45.7 × 21.2 mm | 48 (4 × 12) | gap ≈ 2.6 mm computed – test print |
| HERMA 4346 | A4 | 45.7 × 21.2 mm | 48 (4 × 12) | assumed like L4736 – test print |
| Avery Zweckform 3666 | A4 | 38 × 21.2 mm | 65 (5 × 13) | margins computed – test print |
| Avery Zweckform 4780 | A4 | 48.5 × 25.4 mm | 40 (4 × 10) | data sheet |
| Avery 5160 | Letter | 2⅝ × 1 in | 30 (3 × 10) | template |
| Avery 5167 | Letter | 1¾ × ½ in | 80 (4 × 20) | template |
| A4 plain paper | A4 | 70 × 37 mm | 24 (3 × 8) | for cutting / 3 × 8 sheets |
| Letter plain paper | Letter | 2 × 1 in | 40 (4 × 10) | for cutting |

Removable labels (L4731/L4736/HERMA "movables") come off without residue and are recommended for
originals. Use QR or DataMatrix for the smallest labels (25.4 × 10 mm) – Code 128 is too dense
there (the generator warns).

## Printing tips

- **Print at 100 %** ("actual size") – **not** "fit to page" or "shrink to printable area". The
  PDF asks the viewer for this, but check the print dialog anyway.
- Set the matching paper size in the printer (A4 or Letter) and the paper type "labels" if
  available.
- Do a **test print** on plain paper first and hold it against the label sheet in front of a
  light. If the labels are offset, set the **printer calibration** (mm); it applies to all labels.
- Laser printers produce the sharpest codes. On inkjets choose "high quality".
- Scan with at least 300 dpi (default of
  [`PAPERRUST_CONSUMER_BARCODE_DPI`](configuration.md#PAPERRUST_CONSUMER_BARCODE_DPI)); for very
  small labels [`PAPERRUST_CONSUMER_BARCODE_UPSCALE`](configuration.md#PAPERRUST_CONSUMER_BARCODE_UPSCALE)
  = `1.5` helps.

## API

Everything is also available through the native API v2 (permission `documents.change_document`):

| Method | Path | Purpose |
|---|---|---|
| GET | `/rs/api/v2/asn/presets` | label sheets |
| GET | `/rs/api/v2/asn/config` | prefix, separator string, switches, next free number, hints |
| POST | `/rs/api/v2/asn/preview?page=0` | SVG preview, collisions, hints |
| POST | `/rs/api/v2/asn/labels` | PDF (records the range; `?record=false` test print, `?allow_collisions=true` despite collisions) |
| GET | `/rs/api/v2/asn/printed` | printed ranges |
| DELETE | `/rs/api/v2/asn/printed/{id}` | release a range |
| POST | `/rs/api/v2/asn/separator` | separator sheet PDF (`paper`, `code`, `copies`, `lang`) |

Example: `{"preset": "avery-l4731", "code": "qr", "start": 1201, "sheets": 2, "skip": [0, 1, 2]}`.
