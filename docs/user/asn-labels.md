# ASN-Etiketten und Trennblätter

[English](en/asn-labels.md)

Mit dem **ASN-Etiketten-Generator** druckt PaperRust Etikettenbögen mit fortlaufenden
Archivnummern (ASN) als Barcode. Ein Etikett kommt auf die erste Seite jedes Papierdokuments –
danach legen Sie den ganzen Stapel auf einmal in den Scanner. PaperRust erkennt die Etiketten,
**teilt den Scan an jedem Etikett in einzelne Dokumente** und gibt jedem Dokument **die Nummer
auf seinem Etikett**. Das Papier legen Sie in Nummernfolge ab und finden es über die ASN wieder.

Die Seite finden Sie in der Oberfläche unter **Werkzeuge → ASN-Etiketten**
(`/rs/app/tools/asn`). Nötig ist das Recht „Dokumente ändern“.

## Einmalig einrichten

In **Konfiguration → Barcodes** (oder als Umgebungsvariablen) einschalten:

| Einstellung | Wert | Wirkung |
|---|---|---|
| [`PAPERRUST_CONSUMER_ENABLE_BARCODES`](configuration.md#PAPERRUST_CONSUMER_ENABLE_BARCODES) | `true` | Barcodes teilen Stapel auf |
| [`PAPERRUST_CONSUMER_ENABLE_ASN_BARCODE`](configuration.md#PAPERRUST_CONSUMER_ENABLE_ASN_BARCODE) | `true` | ASN-Barcodes werden gelesen und zugewiesen |
| [`PAPERRUST_CONSUMER_ASN_BARCODE_PREFIX`](configuration.md#PAPERRUST_CONSUMER_ASN_BARCODE_PREFIX) | `ASN` (Standard) | Präfix auf den Etiketten – der Generator übernimmt ihn automatisch |

Mit **beiden** Schaltern beginnt jede Seite mit ASN-Etikett ein neues Dokument (die Seite bleibt
erhalten), genau wie in paperless-ngx. Ein eigenes Trennblatt ist dann **nicht** nötig. Der
Generator zeigt oben einen Hinweis, wenn etwas davon fehlt. PDFium muss verfügbar sein (bei der
Desktop-App und im Docker-Abbild ist es dabei).

## Ablauf

1. **Etiketten drucken:** Bogen wählen, Startnummer prüfen (vorgeschlagen wird die nächste freie
   Nummer), Anzahl Etiketten oder Bögen wählen, **PDF herunterladen** und drucken.
2. **Aufkleben:** ein Etikett auf die **erste Seite** jedes Dokuments, am besten oben rechts auf
   eine freie Stelle. Die Reihenfolge der Nummern ist egal; jede Nummer nur einmal verwenden.
3. **Scannen:** den ganzen Stapel als **eine** PDF-Datei scannen (Einzug, auch doppelseitig) und in
   den Eingangsordner legen bzw. hochladen.
4. **Fertig:** PaperRust teilt den Stapel an jeder Seite mit Etikett auf. Jedes Teil wird danach
   normal verarbeitet und bekommt die Nummer seines Etiketts als ASN.

Hat die erste Seite des Stapels kein Etikett, wird alles bis zum ersten Etikett ein eigenes
Dokument ohne ASN. Ist eine Nummer schon vergeben, gilt wie in paperless-ngx: das Einlesen dieses
Dokuments schlägt mit „Given ASN … already exists!“ fehl (in den Aufgaben sichtbar), die übrigen
Dokumente des Stapels sind davon nicht betroffen.

### Alternative: Trennblätter

Wer keine ASN möchte, legt **Trennblätter** zwischen die Dokumente. Der Generator druckt sie mit
dem eingestellten Trenn-Barcode ([`PAPERRUST_CONSUMER_BARCODE_STRING`](configuration.md#PAPERRUST_CONSUMER_BARCODE_STRING),
Standard `PATCHT`). Voraussetzung ist nur `PAPERRUST_CONSUMER_ENABLE_BARCODES`. Das Trennblatt
wird verworfen (außer
[`PAPERRUST_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES`](configuration.md#PAPERRUST_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES)
ist an) und lässt sich beliebig oft wiederverwenden. Etiketten und Trennblätter können im selben
Stapel gemischt werden.

## Einstellungen des Generators

| Feld | Bedeutung |
|---|---|
| Etikettenbogen | Vorlage (siehe unten) oder **Eigenes Raster** mit Seitengröße, Zeilen, Spalten, Etikettengröße, Rändern und Abständen in mm |
| Code-Art | **QR** (Standard, am robustesten), **DataMatrix** (am kleinsten), **Code 128** (1D, für Handscanner; braucht breite Etiketten) |
| Text | Nummer zusätzlich als Klartext (Standard an) |
| Startnummer | Standard: nächste freie Nummer |
| Stellen | Nummer mit Nullen aufgefüllt (Standard 5: `ASN00042`); der Consumer liest die Zahl unabhängig davon |
| Anzahl / Bögen | Anzahl Etiketten oder ganze Bögen |
| Benutzte Etiketten | Auf der Vorschau angeklickte Plätze bleiben frei – so lassen sich angebrochene Bögen aufbrauchen |
| Druckerkorrektur | Verschiebung in mm (+ = nach rechts/unten), falls der Drucker versetzt druckt |
| Testdruck | Druckt Etikettenumrisse auf Normalpapier, wird nicht gespeichert |

**Nummern-Schutz:** Jeder Druck wird mit Nummernbereich, Bogen und Benutzer gespeichert. Die
Startnummer wird immer hinter die größte vergebene **und** die größte gedruckte Nummer gelegt.
Überschneidet sich ein Druck mit vorhandenen ASNs (auch im Papierkorb – ASNs sind eindeutig) oder
einem früheren Druck, warnt der Generator und druckt nur nach Bestätigung. Fehldrucke lassen sich im
Verlauf freigeben (Ersteller oder Superuser).

Die größte speicherbare ASN ist 2 147 483 647 (die Datenbankspalte ist `integer`, auch wenn
paperless-ngx Werte bis 4 294 967 295 zulässt).

## Etikettenbögen

| Vorlage | Format | Etikett | Pro Bogen | Maße |
|---|---|---|---|---|
| Avery Zweckform L4731REV-25 | A4 | 25,4 × 10 mm | 189 (7 × 27) | Datenblatt |
| HERMA 10001 | A4 | 25,4 × 10 mm | 189 (7 × 27) | Raster wie L4731 angenommen – Testdruck |
| Avery Zweckform L4736REV-25 | A4 | 45,7 × 21,2 mm | 48 (4 × 12) | Abstand ≈ 2,6 mm errechnet – Testdruck |
| HERMA 4346 | A4 | 45,7 × 21,2 mm | 48 (4 × 12) | wie L4736 angenommen – Testdruck |
| Avery Zweckform 3666 | A4 | 38 × 21,2 mm | 65 (5 × 13) | Ränder errechnet – Testdruck |
| Avery Zweckform 4780 | A4 | 48,5 × 25,4 mm | 40 (4 × 10) | Datenblatt |
| Avery 5160 | Letter | 2⅝ × 1 in | 30 (3 × 10) | Vorlage |
| Avery 5167 | Letter | 1¾ × ½ in | 80 (4 × 20) | Vorlage |
| A4 Normalpapier | A4 | 70 × 37 mm | 24 (3 × 8) | zum Schneiden / 3 × 8-Bögen |
| Letter Normalpapier | Letter | 2 × 1 in | 40 (4 × 10) | zum Schneiden |

Ablösbare Etiketten (L4731/L4736/HERMA „movables“) lassen sich rückstandsfrei entfernen und sind
für Originale empfehlenswert. Für die kleinsten Etiketten (25,4 × 10 mm) QR oder DataMatrix
verwenden – Code 128 ist dort zu dicht (der Generator warnt).

## Drucktipps

- **Mit 100 % drucken** („Tatsächliche Größe“) – **nicht** „An Seite anpassen“ oder „Auf
  Druckbereich verkleinern“. Das PDF bittet den Betrachter darum, prüfen Sie es trotzdem im
  Druckdialog.
- Papierformat im Drucker passend einstellen (A4 bzw. Letter), Papierart „Etiketten“ falls
  vorhanden.
- Zuerst einen **Testdruck** auf Normalpapier machen und ihn gegen den Etikettenbogen ins Licht
  halten. Liegen die Etiketten versetzt, die **Druckerkorrektur** (mm) setzen; der Wert gilt für
  alle Etiketten.
- Laserdrucker liefern die schärfsten Codes. Bei Tintenstrahl „hohe Qualität“ wählen.
- Scannen mit mindestens 300 dpi (Standard von
  [`PAPERRUST_CONSUMER_BARCODE_DPI`](configuration.md#PAPERRUST_CONSUMER_BARCODE_DPI)); bei
  sehr kleinen Etiketten hilft
  [`PAPERRUST_CONSUMER_BARCODE_UPSCALE`](configuration.md#PAPERRUST_CONSUMER_BARCODE_UPSCALE) = `1.5`.

## API

Alle Funktionen gibt es auch über die native API v2 (Recht `documents.change_document`):

| Methode | Pfad | Zweck |
|---|---|---|
| GET | `/rs/api/v2/asn/presets` | Etikettenbögen |
| GET | `/rs/api/v2/asn/config` | Präfix, Trenn-String, Schalter, nächste freie Nummer, Hinweise |
| POST | `/rs/api/v2/asn/preview?page=0` | SVG-Vorschau, Kollisionen, Hinweise |
| POST | `/rs/api/v2/asn/labels` | PDF (speichert den Bereich; `?record=false` Testdruck, `?allow_collisions=true` trotz Kollision) |
| GET | `/rs/api/v2/asn/printed` | gedruckte Bereiche |
| DELETE | `/rs/api/v2/asn/printed/{id}` | Bereich freigeben |
| POST | `/rs/api/v2/asn/separator` | Trennblatt-PDF (`paper`, `code`, `copies`, `lang`) |

Beispiel: `{"preset": "avery-l4731", "code": "qr", "start": 1201, "sheets": 2, "skip": [0, 1, 2]}`.
