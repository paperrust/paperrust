# Leistungsmessung PaperRust e546b8f gegen paperless-ngx 3.4.2 (Jetson Orin Nano, 2026-11-20)

Gerät und Messprogramm wie am 18./19.11. (`../2026-11-18-jetson/bericht.md`, `../2026-11-19-jetson/bericht.md`):
NVIDIA Jetson Orin Nano, 6 Kerne, 7,5 GB RAM, NVMe, Leistungsmodus `pmode:0002`, 55 °C; Container auf Kernen 0–3,
Messprogramm auf 4–5; deu+eng, Europe/Zurich, `THREADS_PER_WORKER=4`, PostgreSQL 17. Je **eine** Runde pro
System – Streuung nicht belegt, Unterschiede unter etwa 10 % nicht überbewerten.
Alle Tabellen: `tabellen.md`; PaperRust 19.11. gegen 20.11.: `vergleich_19_20.md`;
Rohdaten: `rohdaten.json`, `raw/` (`run_*.json`, `log_*.txt`, App-Protokolle `*_applog.txt`, Messreihen `samples_*.jsonl`).

| Kürzel | Aufstellung | Lauf (20.11.) |
|---|---|---|
| PaperRust | Image `prrel_paperrust:e546b8f` (Freigabestand) + PostgreSQL 17, Standard-Einstellungen (Säubern und Geraderichten **an**, wie ngx) | 01:22–02:08 |
| ngx gleiche Arbeit (C) | ngx 3.4.2 + Valkey + Tika + Gotenberg + PostgreSQL 17, `OCR_CLEAN=none`, `OCR_DESKEW=false` | 02:09–03:26 |
| ngx minimal (A) | ngx 3.4.2 + Valkey + PostgreSQL 17, Standard (Säubern + Geraderichten an; liest kein DOCX) | 03:27–04:49 |

Runde 0 PaperRust (01:15–01:21) war ein verkürzter Rauchlauf (`smoke: true`) und ist **nicht** ausgewertet.
Grundlast zu Beginn der Läufe 0,04–0,15 (1-min-Mittel); außer zwei ruhenden fremden Containern
(`deploy-redis-1`, `deploy-mariadb-1`) liefen keine Rechenjobs (Sperrdatei `paperrust_release.lock`).
Da PaperRust jetzt selbst säubert und geraderichtet, ist **ngx minimal (A)** der Vergleich mit gleicher Arbeit;
C zeigt ngx mit Word-Unterstützung (Tika/Gotenberg), aber ohne Säubern. ngx vollständig (B) wurde nicht gemessen.

## Wichtige Einschränkung: S2/S3 ohne Hochladen und verkürzt

In den Bedien-Szenarien S2 (1 Person) und S3 (5 bzw. 10 Personen) ist der Upload-Teil bei **allen drei Systemen**
gescheitert: der Ordner `~/prrel/docs/extra` fehlte auf dem Jetson. In `raw/log_1_rs.txt`, `log_1_ngxc.txt` und
`log_1_ngxa.txt` steht je **15-mal** `FileNotFoundError: … ~/prrel/docs/extra/e000NN.pdf` (e00000–e00014:
1 in S2, 5 in S3/5, 9 in S3/10; `uploads_used: 15` in jedem `run_1_*.json`). Der Fehler entstand **im Messprogramm
beim Öffnen der Datei** – die Anfrage ging nie an den Server, deshalb „Fehler 0“.

Folgen (für alle Systeme gleich, der Vergleich zwischen den Systemen bleibt fair):
- Jeder Upload-Versuch beendete den Thread der jeweiligen simulierten Person. S2 lief deshalb nur **63–67 s statt
  900 s** (39 Anfragen statt 453 am 19.11.), S3/5 nur **105–111 s statt 300 s** (223 Anfragen). In S3/10 kam eine
  der 10 Personen nie zum Hochladen und lief die vollen 300 s (PaperRust 728, ngx je 715 Anfragen).
- Gemessen wurde also **lesende Bedienung plus Ändern (PATCH), ohne Hochladen** – nicht die geplante gemischte Last.
  Nicht-lesende Anfragen: 2 von 39 (S2), 8 von 223 (S3/5), 48 von 728 (S3/10, PaperRust).
- S2/S3-Werte dieses Laufs sind **nicht** mit dem 19.11. vergleichbar (dort 63 Uploads je Lauf). Das gilt auch für
  den Leerlauf am Ende (nach S2/S3). Für Website und Gate weiter die S2/S3-Zahlen vom 19.11. verwenden oder S2/S3
  mit vorhandenem `docs/extra` wiederholen.

## Kurzfassung

- **Start:** Erststart bis Anmeldung **3,3 s** (ngx A 53,2 s, ngx C 82,1 s). Neustart bis die Dokumentliste
  antwortet **0,57 s** (0,54–0,59 s; ngx 33,7 s) – der langsamere Neustart vom 19.11. (0,82 s, Befund L2) ist weg
  (passender Commit: 808c72b „Oberfläche beim Start nicht mehr komplett einlesen“).
- **Arbeitsspeicher im Leerlauf, leere Datenbank:** PaperRust **74 MiB Dienste + 64 MiB DB = 138 MiB**,
  ngx A 598 + 41 = 639 MiB, ngx C 805 + 36 = 841 MiB.
- **Arbeitsspeicher im Leerlauf nach 230 Dokumenten (Minuten 5–10):** PaperRust **420 + 74 = 494 MiB**,
  ngx A 702 + 49 = 751 MiB, ngx C 1 317 + 49 = 1 366 MiB (19.11.: PaperRust 465 + 72 = 537 MiB).
- **Ein Dokument einlesen (Ablegen bis durchsuchbar, Median):** PDF mit Text 5,6 s (ngx A 8,7 s), Scan 1 Seite
  **8,1 s** (25,8 s; C 19,9 s), Scan 8 Seiten 12,3 s (38,2 s; C 26,1 s), Foto JPG **9,4 s** (32,3 s; C 19,7 s),
  Bild PNG 12,0 s (31,8 s; C 19,6 s), Word DOCX 5,6 s (C 7,7 s; A kann kein DOCX). Reine Verarbeitung laut Protokoll:
  PDF 0,11 s (ngx 2,81 s), Scan 1 Seite 2,67 s (A 19,95 s); der Rest bis „durchsuchbar“ ist Warten auf den
  Eingangsordner und das Suchintervall des Messprogramms.
- **Fotos/Bilder wieder erkannt:** 4/4 JPG und 2/2 PNG durchsuchbar (19.11.: 0/6, Befund L1), Wortquote JPG 100 %,
  PNG 98,7 % (ngx 100 %). Passender Commit: 9dbb039 („Säubern lässt Fotos aus“).
- **230 Dokumente auf einmal (200 Text-PDFs + 30 Scans):** **106 s = 129,7 Dok./min**; ngx A 1 796 s (7,7 Dok./min,
  16,9× langsamer), ngx C 1 493 s (9,2 Dok./min). Je Scan im Stapel 2,6 s (A 22,3 s), je Text-PDF 0,10 s (5,18 s).
  Rechenzeit der Dienste 98 CPU-s (A 1 611, C 1 389).
- **Bedienung:** Liste/Suche während des Imports Median/p95 **10 / 32 ms** (ngx A 150 / 187 ms), danach 9 / 13 ms
  (151 / 172 ms). 10 Personen gleichzeitig (ohne Hochladen, siehe oben): **4 / 16 / 34 ms** Median/p95/p99
  (ngx A 77 / 255 / 483 ms). Kein einziger Fehler (5xx/Zeitüberschreitung) in allen Szenarien und Systemen.
- **Platte nach dem Import:** Daten + Medien 54,7 MiB (A 51,4, C 56,1), Datenbank-Volume 97,8 MiB (A 88,2, C 72,5),
  logische DB-Größe 28,4 MB (A 18,2 MB, C 18,5 MB).
- **CPU im Leerlauf (10 min, leer):** Dienste 1,09 CPU-s (A 3,30), Datenbank 1,21 CPU-s (A 0,47).

## Schwächen (ehrlich)

- **Datenbank-RAM höher als ngx:** leer 64 MiB (A 41, C 36), mit Dokumenten 74 MiB (A/C 49), Spitze beim Import
  115 MiB (A 52, C 51), Spitze mit 10 Personen 112 MiB (A 54). Gesamt (Dienste + DB) trotzdem deutlich unter ngx.
- **Datenbank-CPU im Leerlauf höher:** 1,21 statt 0,47 CPU-s in 10 min (2,6× ngx A) – absolut sehr wenig.
- **Platte:** Datenbank-Volume +11 % gegen ngx A (97,8 gegen 88,2 MiB), logische DB-Größe 1,56× (28,4 gegen
  18,2 MB), Dateien +6 % (54,7 gegen 51,4 MiB). Laut Commit 8b41f12 hat der Besitzer einen festen Sockel erlaubt,
  pro Dokument liegt PaperRust nicht über ngx (siehe `docs/RELEASE-GATE.md`).
- **RAM-Spitze beim Import:** Dienste 800 MiB – nur knapp unter ngx A (838 MiB, 1,05×).
- **Gegen den 19.11. schlechter (Details `vergleich_19_20.md`):** Massenbearbeitung „Tag an alle“ Median 177 statt
  100 ms (p95 gleich: 192 statt 197 ms; die Einzelwerte liegen in zwei Lagen, 75–111 ms und meist 165–200 ms; am 18.11.
  waren es 174 ms) und Liste/Suche während des Imports p95 32 statt 18 ms (Median gleich 10 ms; nur 6 von 107 Anfragen über
  30 ms). Ursache jeweils unklar.
- **Bild PNG:** Wortquote 98,7 % statt 100 % (2 Dokumente; ngx 100 %).
- Prozesse der Dienste: Tabelle zeigt 6 (Maximum); das war **ein** 2-s-Messpunkt in S1, zu Beginn und Ende von
  S1 und in allen anderen Abschnitten 3 Prozesse (`tini`, Entrypoint-Shell, `paperrust`).

## Wichtigste Zahlen (aus `tabellen.md` bzw. `raw/run_1_*.json`; PaperRust / ngx A / ngx C)

| Messgröße | PaperRust | ngx A | ngx C | PaperRust 19.11. |
|---|---|---|---|---|
| Erststart bis Anmeldung [s] | 3,3 | 53,2 | 82,1 ¹ | 3,3 |
| Neustart bis Liste [s] (Median aus 3) | 0,57 | 33,71 | 33,70 | 0,82 |
| RAM leer, Dienste / DB [MiB] | 74 / 64 | 598 / 41 | 805 / 36 | 107 / 64 |
| Einzel-Scan 1 Seite bis durchsuchbar [s] | 8,1 | 25,8 | 19,9 | 8,1 |
| Foto JPG / Bild PNG bis durchsuchbar [s] | 9,4 / 12,0 | 32,3 / 31,8 | 19,7 / 19,6 | – / – (nicht gefunden) |
| Import 230 Dok. [s] | 106 | 1 796 | 1 493 | 110 |
| Scan je Dokument im Stapel, Median [s] | 2,6 | 22,3 | 16,4 | 2,8 |
| Bedienung während Import, Median / p95 [ms] | 10 / 32 | 150 / 187 | 150 / 191 | 10 / 18 |
| RAM-Spitze Import, Dienste / DB [MiB] | 800 / 115 | 838 / 52 | 1 410 / 51 | 766 / 113 |
| **RAM Leerlauf mit Dokumenten (Min. 5–10), Dienste / DB [MiB]** | **420 / 74** | **702 / 49** | 1 317 / 49 | 465 / 72 |
| DB-Volume / logische DB-Größe [MiB / MB] | 97,8 / 28,4 | 88,2 / 18,2 | 72,5 / 18,5 | 96,5 / 27,0 |
| Daten + Medien [MiB] | 54,7 | 51,4 | 56,1 | 54,7 |
| Massenbearbeitung Median / p95 [ms] | 177 / 192 | 661 / 694 | 674 / 750 | 100 / 197 |
| 10 Nutzer (ohne Upload ²): Median / p95 / p99 [ms] | 4 / 16 / 34 | 77 / 255 / 483 | 77 / 260 / 568 | (5 / 18 / 30 mit Upload) |
| RAM Leerlauf am Ende (nach S2/S3 ²), Dienste / DB [MiB] | 391 / 76 | 780 / 49 | 1 384 / 49 | (633 / 74) |
| Fehler (5xx/Zeitüberschreitung) in allen Szenarien | 0 | 0 | 0 | 0 |

¹ ngx C startete am 19.11. in 53,5 s; heute „HTTP nach 81,27 s“ (`log_1_ngxc.txt`). Ursache unklar, betrifft nicht
PaperRust. ² Nicht mit dem 19.11. vergleichbar (siehe Einschränkung oben).

App-Protokolle: PaperRust 0 ERROR (14 WARN: fehlende DPI-Angabe bei Bildern, nicht lesbare XMP-Metadaten),
keine Neustarts, kein OOM, Exit-Code 0 bei allen drei Systemen. S4-Aufgaben: PaperRust 256 × SUCCESS,
ngx A 257 SUCCESS + 3 REVOKED, ngx C 258 SUCCESS + 1 REVOKED (wie am 19.11. ngx mit einzelnen REVOKED).

## Bewertung gegen das Release-Gate (Punkt „Leistungsvergleich“)

- Speicher im Leerlauf **mit** Dokumenten (Dienste + DB) mindestens so gut wie ngx minimal: **ja** (494 gegen
  751 MiB); DB allein weiter schlechter (74 gegen 49 MiB).
- Plattenbedarf: absolut **nein** (DB-Volume +11 %, logisch 1,56×, Dateien +6 %); nach Gate-Entscheidung
  (fester Sockel erlaubt, 8b41f12) gesondert zu bewerten – hier nicht neu geprüft.
- Geschwindigkeit in allen gemessenen Szenarien besser als ngx: **ja**, jetzt auch Fotos/Bilder.
- „Keine Speicher-Optimierung, die die Geschwindigkeit messbar verschlechtert“: Import 4 % schneller, Neustart
  30 % schneller als am 19.11.; zwei Einzelwerte schlechter (Massenbearbeitung Median, p95 während Import), ohne
  erkennbaren Zusammenhang mit Speicher-Änderungen. S2/S3 mit Uploads ist für e546b8f **nicht** gemessen.

## Methode und Grenzen

- Messprogramm `~/prrel/tools/bench.py` auf dem Jetson (ob seit 19.11. geändert, hier nicht geprüft; Ablauf laut
  Log S1 → S7 → S4 → S6 → S2 → S3/5 → S3/10 → 5 min Leerlauf am Ende). Jeder Lauf mit eigenem PostgreSQL 17 und frischen Volumes, Abfragen über
  die REST-API mit Token.
- RAM: PSS aller Prozesse je Container alle 2 s; „Dienste“ = alles außer PostgreSQL. S6 „Leerlauf mit Dokumenten“:
  10 min, in der Tabelle Median über 10 min, im Text Minuten 5–10 (bei allen drei Systemen hier gleich).
- „Durchsuchbar“: das Messprogramm sucht etwa alle 0,5 s nach einem Kennwort; „reine Verarbeitung“ aus dem
  App-Protokoll.
- Ein Gerät, eine Runde je System, saubere erfundene Dokumente, nur API-Messung (keine Browser-Darstellung).
- S2/S3 ohne Hochladen und verkürzt (siehe oben). S5 (großes Archiv) nicht gemessen – letzte Werte vom 18.11.
  (c97f15c). Image-Größe von e546b8f in diesem Lauf nicht erfasst (5f5de7c: 668 MB).

## Für die Website

Die Website nennt die Zahlen vom 19.11. (5f5de7c). Geänderte Werte alt → neu (PaperRust fett, wo es um PaperRust
geht). Nicht aufgeführte Zahlen bleiben gleich oder sollten bleiben.

**Allgemein:** Messdatum „18./19. November“ → „20. November 2026“, Entwicklungsstand `5f5de7c` → `e546b8f`
(Läufe 01:22–04:49 Uhr), Quelle `2026-11-19-jetson` → `2026-11-20-jetson`.

**Startseite (Fakten):**
- Import 230 Dokumente: **1 min 50 s → 1 min 46 s** (ngx 25–30 min bleibt: 1 493 / 1 796 s).
- Speicher Leerlauf mit Dokumenten inkl. DB: **537 → 494 MiB**; ngx minimal 752 → 751 MiB, mit Tika/Gotenberg
  1 438 → 1 366 MiB.
- „10 Personen: 95 % schneller als 18 ms“ (ngx 307 ms): **nicht ändern** – neuer Wert (16 ms / ngx 255 ms) ohne Uploads.
- Image 668 MB: nicht neu gemessen, bleibt.

**Geschwindigkeit:**
- Erster Start: ngx minimal 53,2 s (gleich), ngx ohne Säubern 53,5 → 82,1 s (Ausreißer, Ursache unklar – ggf.
  beim alten Wert lassen und erwähnen), PaperRust 3,3 s (gleich).
- Neustart: 33,72 / 33,72 / **0,82 s → 33,71 / 33,70 / 0,57 s**; den Satz „In der Messung am Tag davor startete
  PaperRust in 0,54 s neu … Ursache …“ streichen bzw. ersetzen (behoben).
- Tabelle 1/5/10 Personen und Einzelschritte: **beim 19.11. lassen** (dieser Lauf ohne Uploads und verkürzt).

**Einlesen (ngx minimal / ngx ohne Säubern / PaperRust):**
- PDF mit Text 8,6 / 8,5 / 5,6 s → 8,7 / 8,7 / **5,6 s**
- Scan 1 Seite 25,7 / 19,9 / 8,1 s → 25,8 / 19,9 / **8,1 s**
- Scan 8 Seiten 37,9 / 26,1 / 11,8 s → 38,2 / 26,1 / **12,3 s**
- Handyfoto JPG 32,0 / 19,9 s / „nicht gefunden ¹“ → 32,3 / 19,7 / **9,4 s** (Fußnote ¹ streichen)
- Bild PNG 31,2 / 19,4 s / „nicht gefunden ¹“ → 31,8 / 19,6 / **12,0 s**
- Word DOCX –/7,7/5,6 s: gleich
- Erkannte Wörter: 100 % → PaperRust bei PNG **98,7 %**, sonst 100 %
- Stapel: alle 230 nach 1 792 / 1 495 / 110 s → 1 796 / 1 493 / **106 s**; Durchsatz 7,7 / 9,2 / 125 → 7,7 / 9,2 /
  **129,7 Dok./min**; je PDF 5,18 / 5,16 / 0,11 s → 5,18 / 5,17 / **0,10 s**; je Scan 22,2 / 16,3 / 2,8 s →
  22,3 / 16,4 / **2,6 s**; Rechenzeit 1 599 / 1 382 / 101 → 1 611 / 1 389 / **98 CPU-s**
- Liste/Suche während des Einlesens Median/95 %: 144/181 / 146/182 / 10/18 ms → 150/187 / 150/191 / **10/32 ms**
- „Tag an alle“ Median/95 %: 643/674 / 654/718 / 100/197 ms → 661/694 / 674/750 / **177/192 ms**
- „Alle auswählen“: 70 / 71 / 6 ms → 71 / 72 / **5 ms**

**Speicher, Platte, Hardware (ngx minimal / ngx ohne Säubern / PaperRust):**
- Frisch gestartet Dienste/DB: 596/36 / 910/41 / 107/64 → 598/41 / 805/36 / **74/64 MiB**; Summe 632 / 951 / 171 →
  639 / 841 / **138 MiB**
- Leerlauf mit Dokumenten (Min. 5–10): 702/50 / 1 388/50 / 465/72 → 702/49 / 1 317/49 / **420/74 MiB**; Summe
  752 / 1 438 / 537 → 751 / 1 366 / **494 MiB**
- Leerlauf nach den Bedien-Tests und Spitze mit 10 Personen: **beim 19.11. lassen** (nicht vergleichbar)
- Spitze beim Einlesen Dienste/DB: 838/54 / 1 524/52 / 766/113 → 838/52 / 1 410/51 / **800/115 MiB**
- Prozesse der Dienste: 21 / 24 / 3 → 21 / 25 / **3** (Tabelle zeigt 6 als Maximum eines einzelnen Messpunkts)
- Rechenzeit Leerlauf Dienste: 3,30 / 4,18 / 1,10 → 3,30 / 4,21 / **1,09 CPU-s**; Datenbank 0,49 / 0,50 / 1,25 →
  0,47 / 0,48 / **1,21 CPU-s**
- Datenbank-Volume 88,4 / 72,5 / 96,5 → 88,2 / 72,5 / **97,8 MiB**; logische DB-Größe 18,4 / 18,5 / 27,0 →
  18,2 / 18,5 / **28,4 MB**; Dateien 51,4 / 56,1 / 54,7 MiB: gleich
- Fließtext „537 MiB … 752 MiB … 1 438 MiB“ → „494 MiB … 751 MiB … 1 366 MiB“
