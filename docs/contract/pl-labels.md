# Vertrag: ASN-Etiketten-Generator (`pl-labels`, `pl-api-v2::asn`, Light-UI `tools/asn`)

PaperRust-Erweiterung **ohne upstream-Gegenstück**. Erzeugt Etikettenbögen mit ASN-Barcodes und
Trennblätter, die der vorhandene Consumer (pl-pdfops `barcodes`, 1:1 upstream
`src/documents/barcodes.py@v3.4.2`) ohne weitere Einstellungen erkennt. Benutzer-Doku:
[docs/user/asn-labels.md](../user/asn-labels.md).

## Aufteil-Verhalten (upstream, unverändert)

Quelle: `BarcodePlugin.run`/`get_separation_pages`/`asn` (upstream), `pl_pdfops::barcodes::separation_pages`,
`pl_consumer::consumer::barcode`.

- Trennseiten werden nur berechnet, wenn `CONSUMER_ENABLE_BARCODES` an ist.
- Darin: Seiten mit Wert == `CONSUMER_BARCODE_STRING` (Standard `PATCHT`) → Trennung, Seite verworfen
  (behalten mit `BARCODE_RETAIN_SPLIT_PAGES`, dann nie Seite 0).
- **Zusätzlich mit `CONSUMER_ENABLE_ASN_BARCODE`:** jede Seite ≠ 0, deren Barcode mit
  `CONSUMER_ASN_BARCODE_PREFIX` beginnt → neues Dokument **mit** dieser Seite.
- Die Teile laufen erneut durch den Consumer; das ASN-Etikett steht dann auf Seite 0 → keine
  weitere Trennung, `asn` = erste Zahl nach dem Präfix (Nicht-Ziffern entfernt).

**Entscheidung:** Upstream teilt also bereits an ASN-Etiketten – eine eigene Einstellung
„an ASN teilen“ ist **nicht** nötig und wurde nicht eingeführt. Empfohlener Ablauf: beide Schalter
an, ein Etikett je Dokument auf Seite 1, ganzer Stapel als ein Scan. Für Nutzer ohne ASN gibt es
das Trennblatt (`PATCHT`). `/asn/config` meldet fehlende Schalter als `hints`.

## Code-Inhalt

`<Präfix><Nummer mit Nullen auf digits Stellen>` (Standard `ASN` + 5 Stellen, `ASN00042`). Präfix
Standard = wirksamer `CONSUMER_ASN_BARCODE_PREFIX` (Umgebung/Oberfläche, überschrieben von
`paperless_applicationconfiguration.barcode_asn_prefix`, wenn nicht leer – wie
`BarcodeConfig.__post_init__`). Präfix: 1–32 druckbare ASCII-Zeichen, ohne Leerzeichen am Rand.
Codes: QR (ECC M, Standard), DataMatrix (ECC 200 quadratisch), Code 128 – kodiert mit rxing
(`encoders`, Apache-2.0, schon im Lock; derselbe Leser wie der Consumer).

ASN-Grenze: upstream `Document.ARCHIVE_SERIAL_NUMBER_MAX = 0xFFFFFFFF`, die Spalte ist aber
`integer` → größte speicherbare ASN 2³¹−1 (`ASN_MAX_STORABLE`); der Generator lehnt darüber ab.

## Ausgabe

- Vektor-PDF 1.4 (eigener kleiner Schreiber): Module als gefüllte Rechtecke (`re f`, Läufe je
  Zeile zusammengefasst), Text in Helvetica-Bold (Standard-14, WinAnsi, nicht eingebettet),
  Flate-Inhaltsströme, `/ViewerPreferences << /PrintScaling /None >>` (100 %).
- SVG-Vorschau aus derselben Zeichenliste (`render::Prim`) → Vorschau = Druck.
- Etikett: Rand 8 % der kürzeren Seite (0,8–2,5 mm); Ruhezone QR 2, DataMatrix 1, Code 128 10
  Module; breite Etiketten (≥ 1,6 × Höhe) Code links/Text rechts, sonst Text unter dem Code.
  Hinweise `module_too_small` (< 0,25 mm 2D, < 0,19 mm 1D), `bars_too_short` (< 5 mm),
  `text_too_small` (< 4 pt), `prefix_differs`.
- Trennblatt: Titel, großer Code (QR 80 mm bzw. Code 128 150 × 45 mm), Klartext, Erklärung de/en,
  A4/Letter, 1–50 Kopien.

## Bögen (`sheets::presets`)

| id | Etikett (mm) | Raster | Quelle / Status |
|---|---|---|---|
| `avery-l4731` (Standard) | 25,4 × 10 | 7 × 27, Abstand x 2,54 | Avery-Datenblatt (Teilung 27,94 / 10, Ränder 8,48 / 13,5) – geprüft |
| `herma-10001` | 25,4 × 10 | 7 × 27 | Größe/Anzahl HERMA; Raster wie L4731 **angenommen** |
| `avery-l4736` | 45,7 × 21,2 | 4 × 12, Abstand x ≈ 2,6 | Größe/Anzahl Avery; Abstand aus Teilung 48,3 **errechnet** |
| `herma-4346` | 45,7 × 21,2 | 4 × 12 | wie L4736 **angenommen** |
| `avery-3666` | 38 × 21,2 | 5 × 13, ohne Abstände | Größe/Anzahl Avery; Ränder **errechnet** |
| `avery-4780` | 48,5 × 25,4 | 4 × 10, ohne Abstände | Avery-Datenblatt (Ränder 8 / 21,5) – geprüft |
| `avery-5160` | 66,675 × 25,4 (2⅝ × 1 in) | 3 × 10 | Avery-Vorlage (Rand 3/16 in, oben ½ in, Teilung 2,75 in) – geprüft |
| `avery-5167` | 44,45 × 12,7 (1¾ × ½ in) | 4 × 20 | Avery-Vorlage (Rand 0,3 in, oben ½ in, Teilung 2,05 in) – geprüft |
| `a4-plain-3x8` | 70 × 37 | 3 × 8 | gängiges 3 × 8-Format (z. B. Avery 3474) |
| `letter-plain-4x10` | 50,8 × 25,4 | 4 × 10 | Schneideraster |

`verified=false` → in der Oberfläche „mit Testdruck prüfen“. Eigenes Raster: Seite 10–600 mm,
Etikett ≥ 5 mm, 1–60 Zeilen/Spalten, alles innerhalb der Seite (± 0,05 mm).

## Auftrag (`LabelJob`, JSON, unbekannte Felder → 400)

`range` (Nummernkreis-Id, nur mit Schalter, s. u.), `preset`, `sheet`, `code` (`qr`|`data_matrix`|`code128`), `show_text` (true), `prefix`, `digits`
(1–10, 5), `start` (Std. nächste freie), `count` **oder** `sheets` (ohne beides 1 Bogen), `skip`
(benutzte Plätze nur auf Bogen 1, zeilenweise 0-basiert), `start_offset`, `offset_x`/`offset_y`
(Druckerkorrektur ± 20 mm, verschiebt alles), `outline` (Umrisse drucken). Grenzen: 20 000
Etiketten, 200 Bögen. Fehler → 400 `invalid` mit `fields.<feld>`.

## Nummernvergabe, Kollisionen (`store`, Tabelle `paperrust_asn_print`)

- Migration `pl_labels.0001_paperrust_asn_print` (expand-only, idempotent), registriert in
  `pl_cli::migrate::paperrust_migrations()`; die API legt die Tabelle beim ersten Aufruf ebenfalls
  an (`CREATE … IF NOT EXISTS`), falls `paperrust migrate` noch nicht lief.
- `next_asn`: `documents_next` = v1 `/api/documents/next_asn/` (max nicht gelöschter + 1),
  `suggested` = max(alle ASN inkl. Papierkorb/Versionen, größte gedruckte) + 1 – ASNs sind für alle
  Zeilen eindeutig, eine ASN im Papierkorb ist nicht frei.
- Kollision = vorhandene Dokument-ASN im Bereich (bis 50 gelistet + Gesamtzahl) oder Überlappung
  mit gespeichertem Druck. `POST /asn/labels` speichert in einer Transaktion unter
  `pg_advisory_xact_lock` (keine Doppelvergabe bei gleichzeitigen Drucken); bei Kollision 409
  `asn_collision` mit `collisions`, außer `?allow_collisions=true`. `?record=false` (Testdruck)
  speichert nicht.
- Verlauf `used` = Dokumente, die inzwischen eine Nummer des Bereichs tragen.
- Freigabe (`DELETE /asn/printed/{id}`): Ersteller oder Superuser.

## Nummernkreise (optional, `ranges`, Tabellen `paperrust_asn_config`/`paperrust_asn_range`)

Standard ist **aus**: eine fortlaufende Nummernfolge, alles wie oben. Nur mit dem Admin-Schalter
„Mehrere Nummernkreise“ wirkt Folgendes.

- Migration `pl_labels.0002_paperrust_asn_ranges` (expand-only, idempotent), über
  `store::migrations::ALL` automatisch in `pl_cli::migrate::paperrust_migrations()`; die API legt sie
  beim ersten Aufruf ebenfalls an. Rückbau: beide Tabellen droppen.
- `paperrust_asn_config` (eine Zeile, `id = 1`): `ranges_enabled` (Std. false),
  `location_field_name` (Std. „Ablageort“, 1–128 Zeichen). Fehlt die Zeile → Standardwerte.
- `paperrust_asn_range`: `name` (1–64), `code` (Kürzel, 1–8 druckbare ASCII-Zeichen, eindeutig ohne
  Groß/klein), `location` (≤ 128 = Länge eines Text-Benutzerfelds), `first_asn`/`last_asn`
  (0 ≤ first ≤ last ≤ 2³¹−1), `note` (≤ 2000). **Keine Überschneidungen** zwischen Bereichen; geprüft
  serverseitig in einer Transaktion unter `pg_advisory_xact_lock(7052041511)` (`ranges::validate`),
  Fehler → 400 `invalid` mit `fields.{name|code|location|note|first_asn|last_asn}`.
- Belegung je Bereich: `used` = Dokumente (inkl. Papierkorb) mit ASN im Bereich; `next_asn` =
  max(größte Dokument-ASN im Bereich, größte gedruckte Nummer im Bereich) + 1, mindestens
  `first_asn`, `null` wenn > `last_asn` (erschöpft); `remaining` = `last_asn − next_asn + 1`.
- Druck mit `LabelJob.range` (Id): Schalter aus → 400 `fields.range`; ohne `start` wird `next_asn`
  des Bereichs vorgeschlagen, erschöpft → 400 `fields.range`; Start außerhalb → `fields.start`, Ende
  hinter `last_asn` → `fields.count` (Druck wird abgelehnt). Kollisionsprüfung wie bisher.
  Etikett: zusätzlich kleine Zeile `<Kürzel> · <Ablageort>` (Helvetica, ≥ 3,5 pt, sonst gekürzt mit
  „…“ oder weggelassen) unter der Nummer – **nur** im Layout „Code links, Text rechts“ mit Text.
  Der Barcode-Inhalt bleibt `<Präfix><Nummer>` (Scannen/Kompatibilität unverändert).
- Ablageort-Hook (`pl_consumer::asn_location`): bekommt ein Dokument eine ASN aus einem Bereich mit
  Ablageort, wird das Benutzerfeld `location_field_name` (bei Bedarf einmal als Typ `string`
  angelegt; gibt es den Namen mit anderem Typ → nichts) auf den Ablageort gesetzt. Gesetzt wird nur,
  wenn das Feld fehlt/leer ist oder den Ablageort **eines** Bereichs trägt (vom Hook gesetzt);
  ein von Hand eingetragener anderer Wert bleibt immer. Aufgerufen
  - im Consumer am Ende von `apply_overrides` (ASN aus Barcode oder Vorgabe; Feld neu angelegt über
    `create_custom_field_instance` mit Audit-Log),
  - beim Dokument-PATCH/PUT (`pl_documents_write::update`), wenn sich die ASN ändert und die Anfrage
    das Feld nicht selbst setzt (über `cfi::update_or_create`, mit Audit-Log).
  Eigene (verschachtelte) Transaktion; Fehler werden nur protokolliert (`paperrust.asn_location`).

## Rechte

Alle Routen: `documents.change_document` (ASN vergeben = Dokument ändern; inaktive Nutzer nie).
Trennblatt mit abweichendem `value`: nur Superuser. Nummernkreise lesen: wie alle Routen; Schalter
(`PUT /asn/config`) und Nummernkreise anlegen/ändern/löschen: nur Superuser (sonst 403).

## Tests

- `cargo test -p pl-labels --lib` – Raster in Seitengrenzen, mm↔pt, Inhalte, Planung
  (skip/offset/Bögen), Validierung, PDF-Struktur (xref), SVG, Etikett-Layout innerhalb des Etiketts.
- `cargo test -p pl-labels --test roundtrip` – PDF → PDFium (300 dpi) → Etikett auf synthetische
  Scan-Seite → Bild-PDF → `scan_pdf`/`RxingReader` → richtige ASN + Aufteilung (QR/DataMatrix/
  Code 128 auf L4731, 4780, 5160, 5167) und Trennblatt (QR/Code 128). Ohne PDFium übersprungen.
- `cargo test -p pl-api-v2 --test asn_api -- --ignored` gegen eine DB-Kopie (siehe Dateikopf).

## Known differences

Keine (kein upstream-Gegenstück). Die Barcode-Erkennung selbst bleibt unverändert.

Nummernkreise, offen/bewusst:
- Ablageort-Hook nicht in Massenbearbeitung (`bulk_edit`), Zusammenführen (`merge` übernimmt ASN)
  und MCP/KI-Pfaden – nur Consumer und Einzel-PATCH/PUT.
- Ändert der Consumer-Hook eine vorhandene leere Instanz (nur möglich, wenn eine Vorgabe das Feld
  leer setzt), geschieht das per `UPDATE` ohne Audit-Eintrag.
- Zusatzzeile auf dem Etikett nur im Layout „Code links, Text rechts“ (die meisten Bögen); bei
  „Code oben“ und Code 128 entfällt sie, damit der Code nicht kleiner wird.
- Kein „Ordnerrücken“-Etikett (optional, nicht umgesetzt).
