# Tabellen: Raspberry Pi 5, 2026-11-22

Erzeugt mit `python3 ../../release/tools/summarize.py raw/results > tabellen.md` (Kopie der Läufe `run_1_*`,
Rauchlauf `run_0_rs` ausgelassen, da `smoke: true`) und `python analyse.py` (Abschnitt „Effizienz“ am Ende).
Bezeichnungen an den Pi angepasst: „ngx C“ = paperless-ngx 3.4.2 mit Tika + Gotenberg, Säubern und Geraderichten
**aus** (`PAPERLESS_OCR_CLEAN=none`, `PAPERLESS_OCR_DESKEW=false`); „ngx A“ = minimal (Valkey, Säubern und
Geraderichten an wie PaperRust, liest kein Word). Container auf 3 Kernen (0–2), `THREADS_PER_WORKER=2` bei allen.
Je **ein** Lauf; Unterschiede unter etwa 10 % nicht überbewerten. Faktor = ngx-Wert / PaperRust-Wert.
Rohdaten: `rohdaten.json`, `raw/results/`.

### S1 Leerlauf (leere Datenbank, 10 min nichts tun)

| Messgröße | ngx Tika/Gotenberg ohne Säubern (C) | ngx minimal (A) | PaperRust | PaperRust vs. C | PaperRust vs. A |
|---|---|---|---|---|---|
| Erststart bis Anmeldung möglich [s] | 51.1 | 44.5 | 5.2 | 9.8× besser | 8.5× besser |
| Neustart bis Dokumentliste antwortet [s] | 29.63 (28.58–29.80) | 28.86 (28.61–29.03) | 0.91 (0.87–2.90) | 32.6× besser | 31.7× besser |
| RAM Dienste (ohne DB), Median [MiB] | 797 | 606 | 51 | 15.7× besser | 11.9× besser |
| RAM Dienste (ohne DB), Maximum [MiB] | 801 | 606 | 51 | 15.6× besser | 11.8× besser |
| RAM Datenbank, Median [MiB] | 45 | 40 | 72 | 1.6× schlechter | 1.8× schlechter |
| CPU-Zeit Dienste in 10 min [CPU-s] | 4.57 | 2.79 | 0.96 | 4.8× besser | 2.9× besser |
| CPU-Zeit Datenbank in 10 min [CPU-s] | 0.41 | 0.34 | 1.10 | 2.7× schlechter | 3.2× schlechter |
| Prozesse der Dienste (ohne DB) | 24 | 20 | 3 | 8.0× besser | 6.7× besser |
| Container (inkl. DB) | 5 | 3 | 2 | 2.5× besser | 1.5× besser |
| 30-s-Abschnitte mit Hintergrundarbeit > 0,5 CPU-s (von 20) | 3 | 1 | 0 | – | – |

### S7 Einlesen je Art (ein Dokument, Ablegen bis durchsuchbar)

| Messgröße | ngx Tika/Gotenberg ohne Säubern (C) | ngx minimal (A) | PaperRust | PaperRust vs. C | PaperRust vs. A |
|---|---|---|---|---|---|
| PDF mit Text: Median [s] | 8.4 | 8.3 | 6.1 | 1.4× besser | 1.4× besser |
| Scan 1 Seite: Median [s] | 17.0 | 21.8 | 8.6 | 2.0× besser | 2.5× besser |
| Scan 8 Seiten: Median [s] | 29.3 | 48.3 | 16.4 | 1.8× besser | 2.9× besser |
| Foto JPG: Median [s] | 17.1 | 30.3 | 9.2 | 1.9× besser | 3.3× besser |
| Bild PNG: Median [s] | 17.3 | 30.0 | 11.4 | 1.5× besser | 2.6× besser |
| Office DOCX: Median [s] | 7.9 | – | 6.1 | 1.3× besser | – |
| PDF mit Text: p95 [s] | 8.4 | 8.5 | 6.4 | 1.3× besser | 1.3× besser |
| Scan 1 Seite: p95 [s] | 17.5 | 22.4 | 8.8 | 2.0× besser | 2.5× besser |
| Scan 8 Seiten: p95 [s] | 30.1 | 48.5 | 17.0 | 1.8× besser | 2.9× besser |
| Foto JPG: p95 [s] | 17.6 | 30.8 | 9.5 | 1.9× besser | 3.2× besser |
| Bild PNG: p95 [s] | 17.6 | 30.2 | 11.7 | 1.5× besser | 2.6× besser |
| Office DOCX: p95 [s] | 9.7 | – | 6.1 | 1.6× besser | – |
| PDF mit Text: reine Verarbeitung laut Protokoll, Median [s] | 2.47 | 2.39 | 0.46 | 5.3× besser | 5.1× besser |
| Scan 1 Seite: reine Verarbeitung laut Protokoll, Median [s] | 11.37 | 16.20 | 2.88 | 3.9× besser | 5.6× besser |
| Scan 8 Seiten: reine Verarbeitung laut Protokoll, Median [s] | 24.02 | 42.57 | 10.95 | 2.2× besser | 3.9× besser |
| Foto JPG: reine Verarbeitung laut Protokoll, Median [s] | 11.62 | 24.57 | 3.62 | 3.2× besser | 6.8× besser |
| Bild PNG: reine Verarbeitung laut Protokoll, Median [s] | 11.51 | 24.12 | 5.83 | 2.0× besser | 4.1× besser |
| Office DOCX: reine Verarbeitung laut Protokoll, Median [s] | 2.07 | – | 0.43 | 4.8× besser | – |
| PDF mit Text: erfolgreich [Anzahl] | 8 | 8 | 8 | ≈ gleich (1.00×) | ≈ gleich (1.00×) |
| Scan 1 Seite: erfolgreich [Anzahl] | 5 | 5 | 5 | ≈ gleich (1.00×) | ≈ gleich (1.00×) |
| Scan 8 Seiten: erfolgreich [Anzahl] | 3 | 3 | 3 | ≈ gleich (1.00×) | ≈ gleich (1.00×) |
| Foto JPG: erfolgreich [Anzahl] | 4 | 4 | 4 | ≈ gleich (1.00×) | ≈ gleich (1.00×) |
| Bild PNG: erfolgreich [Anzahl] | 2 | 2 | 2 | ≈ gleich (1.00×) | ≈ gleich (1.00×) |
| Office DOCX: erfolgreich [Anzahl] | 3 | 0 | 3 | ≈ gleich (1.00×) | – |
| PDF mit Text: Wortquote OCR/Text [%] | 100.0 | 100.0 | 100.0 | ≈ gleich (1.00×) | ≈ gleich (1.00×) |
| Scan 1 Seite: Wortquote OCR/Text [%] | 100.0 | 100.0 | 100.0 | ≈ gleich (1.00×) | ≈ gleich (1.00×) |
| Scan 8 Seiten: Wortquote OCR/Text [%] | 100.0 | 100.0 | 100.0 | ≈ gleich (1.00×) | ≈ gleich (1.00×) |
| Foto JPG: Wortquote OCR/Text [%] | 100.0 | 100.0 | 100.0 | ≈ gleich (1.00×) | ≈ gleich (1.00×) |
| Bild PNG: Wortquote OCR/Text [%] | 100.0 | 100.0 | 100.0 | ≈ gleich (1.00×) | ≈ gleich (1.00×) |
| Office DOCX: Wortquote OCR/Text [%] | 100.0 | – | 100.0 | ≈ gleich (1.00×) | – |
| RAM-Spitze Dienste beim Einzeldokument [MiB] | 1331 | 924 | 629 | 2.1× besser | 1.5× besser |
| CPU-Spitze beim Einzeldokument [Kerne, 10-s-Mittel] | 2.24 | 2.18 | 2.07 | ≈ gleich (1.08×) | ≈ gleich (1.05×) |

### S4 Schwere Last (230 Dokumente auf einmal, dabei Liste/Suche jede Sekunde)

| Messgröße | ngx Tika/Gotenberg ohne Säubern (C) | ngx minimal (A) | PaperRust | PaperRust vs. C | PaperRust vs. A |
|---|---|---|---|---|---|
| Import 230 Dokumente (200 Text + 30 Scans) [s] | 1480 | 1627 | 185 | 8.0× besser | 8.8× besser |
|   Durchsatz [Dok./min] | 9.3 | 8.5 | 74.4 | 8.0× besser | 8.8× besser |
|   davon Text-PDFs: Zeit je Dokument, Median [s] | 5.12 | 5.11 | 0.40 | 12.8× besser | 12.8× besser |
|   davon Scans: Zeit je Dokument, Median [s] | 14.0 | 18.9 | 2.9 | 4.8× besser | 6.5× besser |
| Bedienung während Import: Median [ms] | 105 | 104 | 10 | 10.6× besser | 10.5× besser |
| Bedienung während Import: p95 [ms] | 151 | 146 | 439 | 2.9× schlechter | 3.0× schlechter |
| Bedienung während Import: Fehler [Anzahl] | 0 | 0 | 0 | – | – |
| Bedienung nach Import: Median [ms] | 105 | 103 | 11 | 9.5× besser | 9.4× besser |
| Bedienung nach Import: p95 [ms] | 140 | 151 | 16 | 8.8× besser | 9.6× besser |
| RAM-Spitze Dienste [MiB] | 1416 | 860 | 554 | 2.6× besser | 1.6× besser |
| RAM-Spitze Datenbank [MiB] | 58 | 57 | 153 | 2.7× schlechter | 2.7× schlechter |
| CPU-Spitze [Kerne, 10-s-Mittel, max. 3] | 1.36 | 1.37 | 0.95 | 1.4× besser | 1.4× besser |
| CPU-Zeit Import, Dienste [CPU-s] | 1051 | 1198 | 95 | 11.1× besser | 12.6× besser |
| Kleinster freier RAM des Pi [MB] | 6270 | 6731 | 7035 | 1.1× besser | ≈ gleich (1.05×) |
| OCR-Wortquote der 30 Scans, Median [%] | 100.0 | 100.0 | 100.0 | ≈ gleich (1.00×) | ≈ gleich (1.00×) |
| Massenbearbeitung: Tag an alle, Median [ms] | 1715 | 1534 | 210 | 8.2× besser | 7.3× besser |
| Massenbearbeitung: p95 [ms] | 2376 | 2324 | 1097 | 2.2× besser | 2.1× besser |
| „Alle auswählen“ (selection_data), Median [ms] | 58 | 56 | 5 | 12.5× besser | 12.2× besser |

### S6 Platz auf der Platte (nach dem Import, 230+ Dokumente)

| Messgröße | ngx Tika/Gotenberg ohne Säubern (C) | ngx minimal (A) | PaperRust | PaperRust vs. C | PaperRust vs. A |
|---|---|---|---|---|---|
| Datenbank-Volume [MiB] | 72.3 | 72.6 | 96.0 | 1.3× schlechter | 1.3× schlechter |
| Daten (Index, Vorschaubilder …) + Medien [MiB] | 56.1 | 51.3 | 54.7 | ≈ gleich (1.03×) | ≈ gleich (0.94×) |
| Valkey-Volume [MiB] | 0.08 | 0.08 | 0.00 | – | – |
| RAM Leerlauf mit Dokumenten, Dienste [MiB] | 1268 | 708 | 306 | 4.1× besser | 2.3× besser |
| RAM Leerlauf mit Dokumenten, Datenbank [MiB] | 54 | 54 | 81 | 1.5× schlechter | 1.5× schlechter |

### S2 Alltag (1 Person, 15 min, mit Pausen)

| Messgröße | ngx Tika/Gotenberg ohne Säubern (C) | ngx minimal (A) | PaperRust | PaperRust vs. C | PaperRust vs. A |
|---|---|---|---|---|---|
| Anfragen gesamt | 440 | 443 | 453 | ≈ gleich (1.03×) | ≈ gleich (1.02×) |
| Antwortzeit alle Anfragen, Median [ms] | 53 | 56 | 6 | 8.2× besser | 8.6× besser |
| Antwortzeit p95 [ms] | 196 | 441 | 20 | 9.6× besser | 21.5× besser |
| Antwortzeit p99 [ms] | 583 | 632 | 67 | 8.7× besser | 9.5× besser |
| Nur Lesen (ohne Hochladen/Ändern), Median [ms] | 52 | 53 | 6 | 8.5× besser | 8.6× besser |
| Nur Lesen, p95 [ms] | 161 | 166 | 18 | 9.0× besser | 9.3× besser |
| Fehlerquote [%] | 0.00 | 0.00 | 0.00 | – | – |
| RAM Dienste, Median [MiB] | 1306 | 737 | 321 | 4.1× besser | 2.3× besser |
| RAM Dienste, Spitze [MiB] | 1388 | 832 | 369 | 3.8× besser | 2.3× besser |
| RAM Datenbank, Spitze [MiB] | 59 | 60 | 116 | 2.0× schlechter | 1.9× schlechter |
| CPU-Zeit Dienste [CPU-s] | 69 | 62 | 5 | 13.3× besser | 11.9× besser |
| CPU-Spitze [Kerne] | 0.72 | 0.68 | 0.06 | 12.0× besser | 11.3× besser |

### S3 Kleines Büro (5 Personen gleichzeitig, 5 min)

| Messgröße | ngx Tika/Gotenberg ohne Säubern (C) | ngx minimal (A) | PaperRust | PaperRust vs. C | PaperRust vs. A |
|---|---|---|---|---|---|
| Anfragen gesamt | 817 | 808 | 842 | ≈ gleich (1.03×) | ≈ gleich (1.04×) |
| Antwortzeit alle Anfragen, Median [ms] | 53 | 53 | 5 | 10.3× besser | 10.2× besser |
| Antwortzeit p95 [ms] | 191 | 181 | 19 | 10.2× besser | 9.6× besser |
| Antwortzeit p99 [ms] | 480 | 505 | 256 | 1.9× besser | 2.0× besser |
| Nur Lesen (ohne Hochladen/Ändern), Median [ms] | 53 | 52 | 5 | 11.2× besser | 11.1× besser |
| Nur Lesen, p95 [ms] | 166 | 155 | 17 | 10.0× besser | 9.3× besser |
| Fehlerquote [%] | 0.00 | 0.00 | 0.00 | – | – |
| RAM Dienste, Median [MiB] | 1340 | 764 | 330 | 4.1× besser | 2.3× besser |
| RAM Dienste, Spitze [MiB] | 1435 | 849 | 383 | 3.7× besser | 2.2× besser |
| RAM Datenbank, Spitze [MiB] | 61 | 63 | 156 | 2.6× schlechter | 2.5× schlechter |
| CPU-Zeit Dienste [CPU-s] | 90 | 92 | 5 | 16.6× besser | 17.0× besser |
| CPU-Spitze [Kerne] | 0.91 | 0.88 | 0.08 | 11.4× besser | 11.0× besser |

### S3 Kleines Büro (10 Personen gleichzeitig, 5 min)

| Messgröße | ngx Tika/Gotenberg ohne Säubern (C) | ngx minimal (A) | PaperRust | PaperRust vs. C | PaperRust vs. A |
|---|---|---|---|---|---|
| Anfragen gesamt | 1560 | 1560 | 1628 | ≈ gleich (1.04×) | ≈ gleich (1.04×) |
| Antwortzeit alle Anfragen, Median [ms] | 61 | 60 | 5 | 11.5× besser | 11.3× besser |
| Antwortzeit p95 [ms] | 355 | 292 | 43 | 8.3× besser | 6.9× besser |
| Antwortzeit p99 [ms] | 648 | 705 | 345 | 1.9× besser | 2.0× besser |
| Nur Lesen (ohne Hochladen/Ändern), Median [ms] | 59 | 58 | 5 | 12.0× besser | 11.8× besser |
| Nur Lesen, p95 [ms] | 219 | 206 | 20 | 11.2× besser | 10.5× besser |
| Fehlerquote [%] | 0.00 | 0.00 | 0.00 | – | – |
| RAM Dienste, Median [MiB] | 1441 | 868 | 347 | 4.2× besser | 2.5× besser |
| RAM Dienste, Spitze [MiB] | 1547 | 983 | 400 | 3.9× besser | 2.5× besser |
| RAM Datenbank, Spitze [MiB] | 64 | 67 | 198 | 3.1× schlechter | 2.9× schlechter |
| CPU-Zeit Dienste [CPU-s] | 182 | 179 | 11 | 17.2× besser | 17.0× besser |
| CPU-Spitze [Kerne] | 1.15 | 1.13 | 0.13 | 8.8× besser | 8.7× besser |

### S2 Alltag je Endpunkt

| Messgröße | ngx Tika/Gotenberg ohne Säubern (C) | ngx minimal (A) | PaperRust | PaperRust vs. C | PaperRust vs. A |
|---|---|---|---|---|---|
| dashboard/recent Median [ms] | 84 | 78 | 7 | 12.5× besser | 11.6× besser |
| dashboard/recent p95 [ms] | 99 | 92 | 10 | 9.4× besser | 8.8× besser |
| dashboard/recent Fehler | 0 | 0 | 0 | – | – |
| dashboard/saved_views Median [ms] | 29 | 20 | 3 | 9.9× besser | 7.0× besser |
| dashboard/saved_views p95 [ms] | 31 | 29 | 4 | 7.3× besser | 7.0× besser |
| dashboard/saved_views Fehler | 0 | 0 | 0 | – | – |
| dashboard/statistics Median [ms] | 33 | 26 | 2 | 15.0× besser | 11.7× besser |
| dashboard/statistics p95 [ms] | 37 | 36 | 6 | 6.5× besser | 6.4× besser |
| dashboard/statistics Fehler | 0 | 0 | 0 | – | – |
| dashboard/tasks Median [ms] | 78 | 76 | 8 | 10.2× besser | 10.0× besser |
| dashboard/tasks p95 [ms] | 92 | 95 | 9 | 9.9× besser | 10.2× besser |
| dashboard/tasks Fehler | 0 | 0 | 0 | – | – |
| edit/detail Median [ms] | 75 | 72 | 8 | 10.0× besser | 9.6× besser |
| edit/detail p95 [ms] | 428 | 100 | 15 | 29.1× besser | 6.8× besser |
| edit/detail Fehler | 0 | 0 | 0 | – | – |
| edit/patch Median [ms] | 528 | 500 | 52 | 10.1× besser | 9.5× besser |
| edit/patch p95 [ms] | 992 | 960 | 87 | 11.4× besser | 11.0× besser |
| edit/patch Fehler | 0 | 0 | 0 | – | – |
| filter/filter_date Median [ms] | 126 | 123 | 10 | 12.6× besser | 12.3× besser |
| filter/filter_date p95 [ms] | 150 | 153 | 17 | 9.0× besser | 9.2× besser |
| filter/filter_date Fehler | 0 | 0 | 0 | – | – |
| filter/filter_tag Median [ms] | 95 | 96 | 10 | 9.3× besser | 9.4× besser |
| filter/filter_tag p95 [ms] | 101 | 445 | 13 | 7.6× besser | 33.5× besser |
| filter/filter_tag Fehler | 0 | 0 | 0 | – | – |
| filter/filter_text Median [ms] | 136 | 136 | 10 | 13.1× besser | 13.1× besser |
| filter/filter_text p95 [ms] | 144 | 148 | 15 | 9.8× besser | 10.1× besser |
| filter/filter_text Fehler | 0 | 0 | 0 | – | – |
| filter/filter_type Median [ms] | 129 | 136 | 10 | 12.4× besser | 13.1× besser |
| filter/filter_type p95 [ms] | 149 | 140 | 12 | 12.7× besser | 11.9× besser |
| filter/filter_type Fehler | 0 | 0 | 0 | – | – |
| list/documents_page Median [ms] | 118 | 118 | 9 | 13.1× besser | 13.1× besser |
| list/documents_page p95 [ms] | 155 | 457 | 13 | 11.8× besser | 34.9× besser |
| list/documents_page Fehler | 0 | 0 | 0 | – | – |
| open/detail Median [ms] | 60 | 72 | 7 | 8.5× besser | 10.3× besser |
| open/detail p95 [ms] | 89 | 110 | 10 | 9.2× besser | 11.4× besser |
| open/detail Fehler | 0 | 0 | 0 | – | – |
| open/metadata Median [ms] | 48 | 49 | 5 | 9.1× besser | 9.2× besser |
| open/metadata p95 [ms] | 66 | 61 | 9 | 7.1× besser | 6.6× besser |
| open/metadata Fehler | 0 | 0 | 0 | – | – |
| open/notes Median [ms] | 47 | 45 | 3 | 16.7× besser | 16.2× besser |
| open/notes p95 [ms] | 55 | 54 | 4 | 14.9× besser | 14.6× besser |
| open/notes Fehler | 0 | 0 | 0 | – | – |
| open/preview Median [ms] | 32 | 31 | 4 | 9.0× besser | 8.9× besser |
| open/preview p95 [ms] | 45 | 36 | 6 | 7.7× besser | 6.1× besser |
| open/preview Fehler | 0 | 0 | 0 | – | – |
| open/suggestions Median [ms] | 52 | 54 | 9 | 5.7× besser | 6.0× besser |
| open/suggestions p95 [ms] | 84 | 72 | 16 | 5.4× besser | 4.6× besser |
| open/suggestions Fehler | 0 | 0 | 0 | – | – |
| open/thumb Median [ms] | 31 | 31 | 4 | 8.6× besser | 8.5× besser |
| open/thumb p95 [ms] | 38 | 36 | 6 | 6.5× besser | 6.1× besser |
| open/thumb Fehler | 0 | 0 | 0 | – | – |
| search/autocomplete Median [ms] | 23 | 23 | 2 | 14.6× besser | 14.2× besser |
| search/autocomplete p95 [ms] | 24 | 24 | 3 | 8.4× besser | 8.4× besser |
| search/autocomplete Fehler | 0 | 0 | 0 | – | – |
| search/fulltext Median [ms] | 165 | 167 | 19 | 8.8× besser | 8.8× besser |
| search/fulltext p95 [ms] | 531 | 457 | 29 | 18.5× besser | 15.9× besser |
| search/fulltext Fehler | 0 | 0 | 0 | – | – |
| start/correspondents Median [ms] | 24 | 80 | 4 | 6.3× besser | 20.9× besser |
| start/correspondents p95 [ms] | 24 | 80 | 4 | 6.3× besser | 20.9× besser |
| start/correspondents Fehler | 0 | 0 | 0 | – | – |
| start/custom_fields Median [ms] | 29 | 27 | 4 | 8.1× besser | 7.5× besser |
| start/custom_fields p95 [ms] | 29 | 27 | 4 | 8.1× besser | 7.5× besser |
| start/custom_fields Fehler | 0 | 0 | 0 | – | – |
| start/document_types Median [ms] | 24 | 60 | 4 | 6.2× besser | 15.9× besser |
| start/document_types p95 [ms] | 24 | 60 | 4 | 6.2× besser | 15.9× besser |
| start/document_types Fehler | 0 | 0 | 0 | – | – |
| start/storage_paths Median [ms] | 24 | 26 | 4 | 6.5× besser | 7.1× besser |
| start/storage_paths p95 [ms] | 24 | 26 | 4 | 6.5× besser | 7.1× besser |
| start/storage_paths Fehler | 0 | 0 | 0 | – | – |
| start/tags Median [ms] | 95 | 63 | 8 | 12.3× besser | 8.2× besser |
| start/tags p95 [ms] | 95 | 63 | 8 | 12.3× besser | 8.2× besser |
| start/tags Fehler | 0 | 0 | 0 | – | – |
| start/ui_settings Median [ms] | 139 | 50 | 4 | 33.8× besser | 12.1× besser |
| start/ui_settings p95 [ms] | 139 | 50 | 4 | 33.8× besser | 12.1× besser |
| start/ui_settings Fehler | 0 | 0 | 0 | – | – |
| upload/post_document Median [ms] | 72 | 64 | 14 | 5.0× besser | 4.4× besser |
| upload/post_document p95 [ms] | 103 | 107 | 32 | 3.3× besser | 3.4× besser |
| upload/post_document Fehler | 0 | 0 | 0 | – | – |

### S5 Größeres Archiv


ngxc Runde 1: Fehler=None cpuset=0-2 Aufgaben S4={'SUCCESS': 260, 'REVOKED': 1} OOM=False

ngxa Runde 1: Fehler=None cpuset=0-2 Aufgaben S4={'SUCCESS': 256, 'REVOKED': 1} OOM=False

rs Runde 1: Fehler=None cpuset=0-2 Aufgaben S4={'SUCCESS': 259} OOM=False

### Effizienz beim Import der 230 Dokumente (aus `analyse.py`, Fenster je System: Ablegen bis alle eingelesen)

CPU-Sekunden = alle Container zusammen (Dienste + PostgreSQL, bei ngx auch Valkey, Tika, Gotenberg), aus
`cpu.stat usage_usec` der Container. Voller Takt = Anteil der 5-s-Proben aus `thermal.csv` mit 2 400 MHz.
Temperatur-Minuten = Fläche der Temperatur über dem Leerlauf-Median desselben Systems (S1, frisch).
Rohwerte (Schlüssel wie in `analyse.json`; Spalten rs = PaperRust, ngxa = ngx A, ngxc = ngx C):

| | rs | ngxa | ngxc |
|---|---|---|---|
| t_import_start_local | 15:20:22 | 16:33:26 | 18:15:13 |
| t_import_end_local | 15:23:27 | 17:00:34 | 18:39:53 |
| import_seconds | 185.47 | 1627.39 | 1480.14 |
| cpu_s_all | 99.68 | 1252.96 | 1102.77 |
| docs_per_cpu_min | 138.44 | 11.01 | 12.51 |
| cpu_s_per_doc | 0.433 | 5.448 | 4.795 |
| mean_util_pct_of_3 | 17.9 | 25.7 | 24.8 |
| peak_cores_10s | 0.95 | 1.37 | 1.36 |
| full_clock_min | 2.92 | 26.67 | 24.17 |
| full_clock_share_pct | 97.2 | 98.5 | 98.3 |
| temp_mean | 56.7 | 57.7 | 57.9 |
| temp_max | 60.4 | 61.5 | 60.9 |
| idle_temp_median | 47.7 | 47.7 | 48.3 |
| temp_above_idle_mean_k | 9.0 | 10.0 | 9.6 |
| temp_minutes_above_idle | 26.6 | 270.7 | 236.1 |
| idle_full_clock_share_pct | 0.0 | 1.7 | 5.0 |
| idle_cpu_s_per_min | 0.206 | 0.315 | 0.5 |
| throttled_in_window | ['0x0'] | ['0x0'] | ['0x0'] |
| idle_window_local | ['15:05:17', '15:15:14'] | ['16:13:42', '16:23:39'] | ['17:57:30', '18:07:26'] |
ngxa factor vs rs: {'duration': 8.8, 'cpu_s': 12.6, 'full_clock_min': 9.1, 'temp_minutes': 10.2}
ngxc factor vs rs: {'duration': 8.0, 'cpu_s': 11.1, 'full_clock_min': 8.3, 'temp_minutes': 8.9}
thermal overall: {'samples': 3141, 'throttled_values': ['0x0'], 'temp_min': 45.5, 'temp_max': 64.2}
