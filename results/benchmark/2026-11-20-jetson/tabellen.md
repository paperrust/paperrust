
### S1 Leerlauf (leere Datenbank, 10 min nichts tun)

| Messgröße | ngx gleiche Arbeit (C) | ngx minimal (A) | PaperRust | PaperRust vs. C | PaperRust vs. A |
|---|---|---|---|---|---|
| Erststart bis Anmeldung möglich [s] | 82.1 | 53.2 | 3.3 | 25.2× besser | 16.3× besser |
| Neustart bis Dokumentliste antwortet [s] | 33.70 (33.48–33.71) | 33.71 (33.71–33.74) | 0.57 (0.54–0.59) | 59.1× besser | 59.1× besser |
| RAM Dienste (ohne DB), Median [MiB] | 805 | 598 | 74 | 10.9× besser | 8.1× besser |
| RAM Dienste (ohne DB), Maximum [MiB] | 810 | 601 | 77 | 10.5× besser | 7.8× besser |
| RAM Datenbank, Median [MiB] | 36 | 41 | 64 | 1.8× schlechter | 1.6× schlechter |
| CPU-Zeit Dienste in 10 min [CPU-s] | 4.21 | 3.30 | 1.09 | 3.9× besser | 3.0× besser |
| CPU-Zeit Datenbank in 10 min [CPU-s] | 0.48 | 0.47 | 1.21 | 2.5× schlechter | 2.6× schlechter |
| Prozesse der Dienste (ohne DB) | 25 | 21 | 6 | 4.2× besser | 3.5× besser |
| Container (inkl. DB) | 5 | 3 | 2 | 2.5× besser | 1.5× besser |
| 30-s-Abschnitte mit Hintergrundarbeit > 0,5 CPU-s (von 20) | 1 | 0 | 0 | – | – |

### S7 Einlesen je Art (ein Dokument, Ablegen bis durchsuchbar)

| Messgröße | ngx gleiche Arbeit (C) | ngx minimal (A) | PaperRust | PaperRust vs. C | PaperRust vs. A |
|---|---|---|---|---|---|
| PDF mit Text: Median [s] | 8.7 | 8.7 | 5.6 | 1.6× besser | 1.6× besser |
| Scan 1 Seite: Median [s] | 19.9 | 25.8 | 8.1 | 2.4× besser | 3.2× besser |
| Scan 8 Seiten: Median [s] | 26.1 | 38.2 | 12.3 | 2.1× besser | 3.1× besser |
| Foto JPG: Median [s] | 19.7 | 32.3 | 9.4 | 2.1× besser | 3.4× besser |
| Bild PNG: Median [s] | 19.6 | 31.8 | 12.0 | 1.6× besser | 2.7× besser |
| Office DOCX: Median [s] | 7.7 | – | 5.6 | 1.4× besser | – |
| PDF mit Text: p95 [s] | 8.9 | 8.9 | 6.1 | 1.5× besser | 1.5× besser |
| Scan 1 Seite: p95 [s] | 22.6 | 26.0 | 8.6 | 2.6× besser | 3.0× besser |
| Scan 8 Seiten: p95 [s] | 26.5 | 38.3 | 12.3 | 2.2× besser | 3.1× besser |
| Foto JPG: p95 [s] | 20.0 | 32.4 | 9.7 | 2.1× besser | 3.4× besser |
| Bild PNG: p95 [s] | 19.8 | 32.0 | 12.2 | 1.6× besser | 2.6× besser |
| Office DOCX: p95 [s] | 13.6 | – | 5.6 | 2.4× besser | – |
| PDF mit Text: reine Verarbeitung laut Protokoll, Median [s] | 2.81 | 2.81 | 0.11 | 24.4× besser | 24.5× besser |
| Scan 1 Seite: reine Verarbeitung laut Protokoll, Median [s] | 13.98 | 19.95 | 2.67 | 5.2× besser | 7.5× besser |
| Scan 8 Seiten: reine Verarbeitung laut Protokoll, Median [s] | 20.12 | 32.18 | 6.39 | 3.1× besser | 5.0× besser |
| Foto JPG: reine Verarbeitung laut Protokoll, Median [s] | 13.85 | 26.37 | 3.71 | 3.7× besser | 7.1× besser |
| Bild PNG: reine Verarbeitung laut Protokoll, Median [s] | 13.54 | 25.84 | 6.57 | 2.1× besser | 3.9× besser |
| Office DOCX: reine Verarbeitung laut Protokoll, Median [s] | 2.17 | – | 0.13 | 16.7× besser | – |
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
| Bild PNG: Wortquote OCR/Text [%] | 100.0 | 100.0 | 98.7 | ≈ gleich (0.99×) | ≈ gleich (0.99×) |
| Office DOCX: Wortquote OCR/Text [%] | 100.0 | – | 100.0 | ≈ gleich (1.00×) | – |
| RAM-Spitze Dienste beim Einzeldokument [MiB] | 1311 | 1028 | 813 | 1.6× besser | 1.3× besser |
| CPU-Spitze beim Einzeldokument [Kerne, 10-s-Mittel] | 3.94 | 3.98 | 2.76 | 1.4× besser | 1.4× besser |

### S4 Schwere Last (230 Dokumente auf einmal, dabei Liste/Suche jede Sekunde)

| Messgröße | ngx gleiche Arbeit (C) | ngx minimal (A) | PaperRust | PaperRust vs. C | PaperRust vs. A |
|---|---|---|---|---|---|
| Import 230 Dokumente (200 Text + 30 Scans) [s] | 1493 | 1796 | 106 | 14.0× besser | 16.9× besser |
|   Durchsatz [Dok./min] | 9.2 | 7.7 | 129.7 | 14.0× besser | 16.9× besser |
|   davon Text-PDFs: Zeit je Dokument, Median [s] | 5.17 | 5.18 | 0.10 | 51.7× besser | 51.8× besser |
|   davon Scans: Zeit je Dokument, Median [s] | 16.4 | 22.3 | 2.6 | 6.2× besser | 8.5× besser |
| Bedienung während Import: Median [ms] | 150 | 150 | 10 | 14.6× besser | 14.5× besser |
| Bedienung während Import: p95 [ms] | 191 | 187 | 32 | 6.0× besser | 5.9× besser |
| Bedienung während Import: Fehler [Anzahl] | 0 | 0 | 0 | – | – |
| Bedienung nach Import: Median [ms] | 144 | 151 | 9 | 15.9× besser | 16.6× besser |
| Bedienung nach Import: p95 [ms] | 171 | 172 | 13 | 13.2× besser | 13.2× besser |
| RAM-Spitze Dienste [MiB] | 1410 | 838 | 800 | 1.8× besser | ≈ gleich (1.05×) |
| RAM-Spitze Datenbank [MiB] | 51 | 52 | 115 | 2.3× schlechter | 2.2× schlechter |
| CPU-Spitze [Kerne, 10-s-Mittel, max. 4] | 1.69 | 1.31 | 1.07 | 1.6× besser | 1.2× besser |
| CPU-Zeit Import, Dienste [CPU-s] | 1389 | 1611 | 98 | 14.2× besser | 16.4× besser |
| Kleinster freier RAM des Jetson [MB] | 5146 | 5580 | 5547 | ≈ gleich (1.08×) | ≈ gleich (0.99×) |
| OCR-Wortquote der 30 Scans, Median [%] | 100.0 | 100.0 | 100.0 | ≈ gleich (1.00×) | ≈ gleich (1.00×) |
| Massenbearbeitung: Tag an alle, Median [ms] | 674 | 661 | 177 | 3.8× besser | 3.7× besser |
| Massenbearbeitung: p95 [ms] | 750 | 694 | 192 | 3.9× besser | 3.6× besser |
| „Alle auswählen“ (selection_data), Median [ms] | 72 | 71 | 5 | 13.9× besser | 13.7× besser |

### S6 Platz auf der Platte (nach dem Import, 230+ Dokumente)

| Messgröße | ngx gleiche Arbeit (C) | ngx minimal (A) | PaperRust | PaperRust vs. C | PaperRust vs. A |
|---|---|---|---|---|---|
| Datenbank-Volume [MiB] | 72.5 | 88.2 | 97.8 | 1.4× schlechter | ≈ gleich (0.90×) |
| Daten (Index, Vorschaubilder …) + Medien [MiB] | 56.1 | 51.4 | 54.7 | ≈ gleich (1.03×) | ≈ gleich (0.94×) |
| Valkey-Volume [MiB] | 0.08 | 0.08 | 0.00 | – | – |
| RAM Leerlauf mit Dokumenten, Dienste [MiB] | 1317 | 702 | 420 | 3.1× besser | 1.7× besser |
| RAM Leerlauf mit Dokumenten, Datenbank [MiB] | 49 | 49 | 74 | 1.5× schlechter | 1.5× schlechter |

### S2 Alltag (1 Person, 15 min, mit Pausen)

| Messgröße | ngx gleiche Arbeit (C) | ngx minimal (A) | PaperRust | PaperRust vs. C | PaperRust vs. A |
|---|---|---|---|---|---|
| Anfragen gesamt | 39 | 39 | 39 | ≈ gleich (1.00×) | ≈ gleich (1.00×) |
| Antwortzeit alle Anfragen, Median [ms] | 68 | 66 | 8 | 9.0× besser | 8.6× besser |
| Antwortzeit p95 [ms] | 426 | 426 | 19 | 22.9× besser | 22.9× besser |
| Antwortzeit p99 [ms] | 620 | 630 | 55 | 11.3× besser | 11.5× besser |
| Nur Lesen (ohne Hochladen/Ändern), Median [ms] | 66 | 65 | 7 | 9.0× besser | 8.9× besser |
| Nur Lesen, p95 [ms] | 219 | 426 | 18 | 11.9× besser | 23.1× besser |
| Fehlerquote [%] | 0.00 | 0.00 | 0.00 | – | – |
| RAM Dienste, Median [MiB] | 1319 | 704 | 404 | 3.3× besser | 1.7× besser |
| RAM Dienste, Spitze [MiB] | 1319 | 706 | 421 | 3.1× besser | 1.7× besser |
| RAM Datenbank, Spitze [MiB] | 55 | 52 | 94 | 1.7× schlechter | 1.8× schlechter |
| CPU-Zeit Dienste [CPU-s] | 5 | 4 | 0 | 14.7× besser | 12.4× besser |
| CPU-Spitze [Kerne] | 0.21 | 0.17 | 0.03 | 7.0× besser | 5.7× besser |

### S3 Kleines Büro (5 Personen gleichzeitig, 5 min)

| Messgröße | ngx gleiche Arbeit (C) | ngx minimal (A) | PaperRust | PaperRust vs. C | PaperRust vs. A |
|---|---|---|---|---|---|
| Anfragen gesamt | 223 | 223 | 223 | ≈ gleich (1.00×) | ≈ gleich (1.00×) |
| Antwortzeit alle Anfragen, Median [ms] | 74 | 68 | 4 | 16.6× besser | 15.0× besser |
| Antwortzeit p95 [ms] | 232 | 234 | 13 | 17.7× besser | 17.9× besser |
| Antwortzeit p99 [ms] | 359 | 296 | 32 | 11.3× besser | 9.3× besser |
| Nur Lesen (ohne Hochladen/Ändern), Median [ms] | 69 | 67 | 4 | 16.5× besser | 15.9× besser |
| Nur Lesen, p95 [ms] | 221 | 231 | 12 | 17.9× besser | 18.8× besser |
| Fehlerquote [%] | 0.00 | 0.00 | 0.00 | – | – |
| RAM Dienste, Median [MiB] | 1356 | 737 | 403 | 3.4× besser | 1.8× besser |
| RAM Dienste, Spitze [MiB] | 1359 | 737 | 431 | 3.2× besser | 1.7× besser |
| RAM Datenbank, Spitze [MiB] | 52 | 54 | 108 | 2.1× schlechter | 2.0× schlechter |
| CPU-Zeit Dienste [CPU-s] | 16 | 16 | 1 | 15.9× besser | 15.8× besser |
| CPU-Spitze [Kerne] | 0.43 | 0.42 | 0.09 | 4.8× besser | 4.7× besser |

### S3 Kleines Büro (10 Personen gleichzeitig, 5 min)

| Messgröße | ngx gleiche Arbeit (C) | ngx minimal (A) | PaperRust | PaperRust vs. C | PaperRust vs. A |
|---|---|---|---|---|---|
| Anfragen gesamt | 715 | 715 | 728 | ≈ gleich (1.02×) | ≈ gleich (1.02×) |
| Antwortzeit alle Anfragen, Median [ms] | 77 | 77 | 4 | 17.9× besser | 18.0× besser |
| Antwortzeit p95 [ms] | 260 | 255 | 16 | 16.0× besser | 15.6× besser |
| Antwortzeit p99 [ms] | 568 | 483 | 34 | 16.5× besser | 14.0× besser |
| Nur Lesen (ohne Hochladen/Ändern), Median [ms] | 69 | 72 | 4 | 17.7× besser | 18.4× besser |
| Nur Lesen, p95 [ms] | 253 | 233 | 13 | 19.6× besser | 18.0× besser |
| Fehlerquote [%] | 0.00 | 0.00 | 0.00 | – | – |
| RAM Dienste, Median [MiB] | 1374 | 779 | 398 | 3.5× besser | 2.0× besser |
| RAM Dienste, Spitze [MiB] | 1384 | 784 | 428 | 3.2× besser | 1.8× besser |
| RAM Datenbank, Spitze [MiB] | 56 | 54 | 112 | 2.0× schlechter | 2.1× schlechter |
| CPU-Zeit Dienste [CPU-s] | 56 | 55 | 3 | 17.2× besser | 16.8× besser |
| CPU-Spitze [Kerne] | 1.02 | 0.82 | 0.07 | 14.6× besser | 11.7× besser |

### S2 Alltag je Endpunkt

| Messgröße | ngx gleiche Arbeit (C) | ngx minimal (A) | PaperRust | PaperRust vs. C | PaperRust vs. A |
|---|---|---|---|---|---|
| dashboard/recent Median [ms] | 108 | 100 | 11 | 9.8× besser | 9.1× besser |
| dashboard/recent p95 [ms] | 110 | 101 | 14 | 8.0× besser | 7.3× besser |
| dashboard/recent Fehler | 0 | 0 | 0 | – | – |
| dashboard/saved_views Median [ms] | 29 | 29 | 4 | 7.0× besser | 6.9× besser |
| dashboard/saved_views p95 [ms] | 30 | 30 | 4 | 7.0× besser | 6.9× besser |
| dashboard/saved_views Fehler | 0 | 0 | 0 | – | – |
| dashboard/statistics Median [ms] | 35 | 35 | 7 | 5.0× besser | 4.9× besser |
| dashboard/statistics p95 [ms] | 35 | 35 | 9 | 3.8× besser | 3.8× besser |
| dashboard/statistics Fehler | 0 | 0 | 0 | – | – |
| dashboard/tasks Median [ms] | 108 | 107 | 7 | 14.7× besser | 14.6× besser |
| dashboard/tasks p95 [ms] | 108 | 108 | 8 | 14.3× besser | 14.2× besser |
| dashboard/tasks Fehler | 0 | 0 | 0 | – | – |
| edit/detail Median [ms] | 426 | 77 | 16 | 27.5× besser | 4.9× besser |
| edit/detail p95 [ms] | 426 | 77 | 16 | 27.5× besser | 4.9× besser |
| edit/detail Fehler | 0 | 0 | 0 | – | – |
| edit/patch Median [ms] | 247 | 238 | 55 | 4.5× besser | 4.4× besser |
| edit/patch p95 [ms] | 247 | 238 | 55 | 4.5× besser | 4.4× besser |
| edit/patch Fehler | 0 | 0 | 0 | – | – |
| filter/filter_date Median [ms] | 147 | 158 | 13 | 11.7× besser | 12.5× besser |
| filter/filter_date p95 [ms] | 162 | 159 | 15 | 10.8× besser | 10.6× besser |
| filter/filter_date Fehler | 0 | 0 | 0 | – | – |
| filter/filter_tag Median [ms] | 70 | 426 | 6 | 10.7× besser | 65.5× besser |
| filter/filter_tag p95 [ms] | 70 | 426 | 6 | 10.7× besser | 65.5× besser |
| filter/filter_tag Fehler | 0 | 0 | 0 | – | – |
| list/documents_page Median [ms] | 154 | 147 | 10 | 15.0× besser | 14.3× besser |
| list/documents_page p95 [ms] | 154 | 147 | 10 | 15.0× besser | 14.3× besser |
| list/documents_page Fehler | 0 | 0 | 0 | – | – |
| open/detail Median [ms] | 86 | 79 | 9 | 9.8× besser | 9.0× besser |
| open/detail p95 [ms] | 91 | 80 | 9 | 10.1× besser | 8.9× besser |
| open/detail Fehler | 0 | 0 | 0 | – | – |
| open/metadata Median [ms] | 60 | 62 | 10 | 6.0× besser | 6.2× besser |
| open/metadata p95 [ms] | 61 | 65 | 12 | 5.0× besser | 5.3× besser |
| open/metadata Fehler | 0 | 0 | 0 | – | – |
| open/notes Median [ms] | 65 | 65 | 4 | 18.5× besser | 18.5× besser |
| open/notes p95 [ms] | 65 | 65 | 4 | 15.2× besser | 15.1× besser |
| open/notes Fehler | 0 | 0 | 0 | – | – |
| open/preview Median [ms] | 44 | 44 | 5 | 8.5× besser | 8.4× besser |
| open/preview p95 [ms] | 44 | 44 | 6 | 7.5× besser | 7.5× besser |
| open/preview Fehler | 0 | 0 | 0 | – | – |
| open/suggestions Median [ms] | 344 | 348 | 11 | 31.6× besser | 31.9× besser |
| open/suggestions p95 [ms] | 620 | 630 | 14 | 44.3× besser | 45.0× besser |
| open/suggestions Fehler | 0 | 0 | 0 | – | – |
| open/thumb Median [ms] | 44 | 46 | 6 | 7.5× besser | 7.7× besser |
| open/thumb p95 [ms] | 45 | 48 | 7 | 6.4× besser | 6.8× besser |
| open/thumb Fehler | 0 | 0 | 0 | – | – |
| search/autocomplete Median [ms] | 24 | 24 | 2 | 12.2× besser | 12.1× besser |
| search/autocomplete p95 [ms] | 24 | 26 | 2 | 12.2× besser | 13.1× besser |
| search/autocomplete Fehler | 0 | 0 | 0 | – | – |
| search/fulltext Median [ms] | 216 | 216 | 18 | 11.7× besser | 11.8× besser |
| search/fulltext p95 [ms] | 219 | 232 | 19 | 11.8× besser | 12.5× besser |
| search/fulltext Fehler | 0 | 0 | 0 | – | – |
| start/correspondents Median [ms] | 38 | 40 | 7 | 5.3× besser | 5.4× besser |
| start/correspondents p95 [ms] | 38 | 40 | 7 | 5.3× besser | 5.4× besser |
| start/correspondents Fehler | 0 | 0 | 0 | – | – |
| start/custom_fields Median [ms] | 44 | 49 | 4 | 10.0× besser | 11.2× besser |
| start/custom_fields p95 [ms] | 44 | 49 | 4 | 10.0× besser | 11.2× besser |
| start/custom_fields Fehler | 0 | 0 | 0 | – | – |
| start/document_types Median [ms] | 110 | 66 | 5 | 23.0× besser | 13.7× besser |
| start/document_types p95 [ms] | 110 | 66 | 5 | 23.0× besser | 13.7× besser |
| start/document_types Fehler | 0 | 0 | 0 | – | – |
| start/storage_paths Median [ms] | 78 | 116 | 4 | 17.3× besser | 25.8× besser |
| start/storage_paths p95 [ms] | 78 | 116 | 4 | 17.3× besser | 25.8× besser |
| start/storage_paths Fehler | 0 | 0 | 0 | – | – |
| start/tags Median [ms] | 56 | 55 | 9 | 6.2× besser | 6.1× besser |
| start/tags p95 [ms] | 56 | 55 | 9 | 6.2× besser | 6.1× besser |
| start/tags Fehler | 0 | 0 | 0 | – | – |
| start/ui_settings Median [ms] | 66 | 59 | 4 | 16.8× besser | 15.2× besser |
| start/ui_settings p95 [ms] | 66 | 59 | 4 | 16.8× besser | 15.2× besser |
| start/ui_settings Fehler | 0 | 0 | 0 | – | – |

### S5 Größeres Archiv


ngxc Runde 1: Fehler=None cpuset=0-3 Aufgaben S4={'SUCCESS': 258, 'REVOKED': 1} OOM=False

ngxa Runde 1: Fehler=None cpuset=0-3 Aufgaben S4={'SUCCESS': 257, 'REVOKED': 3} OOM=False

rs Runde 1: Fehler=None cpuset=0-3 Aufgaben S4={'SUCCESS': 256} OOM=False
