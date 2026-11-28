# Mehrbenutzer-Last S2/S3, Nachlauf (arm64-Testgerät, 2026-11-20)

Nachlauf, weil S2/S3 in der Messung vom 20.11. früh (`../bericht.md`) ohne Hochladen liefen (fehlende
Zusatzdokumente im Messskript, alle Nutzer-Threads brachen ab). Jetzt mit 1 000 Zusatzdokumenten, Uploads
funktionieren. Gerät, Kern-Zuteilung (Container 0–3, Messprogramm 4–5), Seed und Abläufe wie am 19.11.;
Stand PaperRust **bf77585** (Release-Stand nach Kommentar-Übersetzung, ohne Verhaltensänderung gegenüber
e546b8f in diesen Pfaden), ngx 3.4.2 „minimal“ (A) und „gleiche Arbeit“ (C). Je eine Runde.

Unterschied zum 19.11.: Messskript-Kopie mit `--s23only` (ohne S1/S7/S6-Leerlauf), daher 230 statt 255
Dokumente vor S2. Rohdaten: `raw/` (Logs, run_s23_*.json).

| Szenario | Messgröße | ngx C | ngx A | **PaperRust** | 19.11. PaperRust |
|---|---|---:|---:|---:|---:|
| S2 Alltag (1 Person, 15 min) | Anfragen | 486 | 486 | 507 | 453 |
| | Median [ms] | 67.8 | 70.3 | **5.8** | 6 |
| | p95 [ms] | 210.9 | 214.2 | **18.4** | 21 |
| | p99 [ms] | 519.4 | 558.7 | **43.6** | 43 |
| S3 5 Personen (5 min) | Anfragen | 759 | 760 | 798 | 846 |
| | Median [ms] | 78.7 | 78.0 | **4.7** | 5 |
| | p95 [ms] | 256.6 | 247.0 | **13.7** | 17 |
| | p99 [ms] | 483.0 | 496.5 | **31.1** | 31 |
| S3 10 Personen (5 min) | Anfragen | 1 550 | 1 563 | 1 677 | 1 636 |
| | Median [ms] | 83.8 | 82.2 | **4.4** | 5 |
| | p95 [ms] | 346.1 | 294.6 | **14.4** | 18 |
| | p99 [ms] | 662.5 | 569.7 | **29.8** | 30 |
| alle | Fehler | 0 | 0 | **0** | 0 |
| alle | Uploads im Lauf | 55 | 58 | 66 | 63 |

**Ergebnis:** PaperRust antwortet unter Mehrbenutzer-Last 12- bis 19-mal schneller als ngx (Median), p95 und
p99 ähnlich. Gegenüber dem 19.11. gleich oder leicht besser, keine Verschlechterung. Keine Fehler.

Grenzen: eine Runde je System; RAM/CPU-Werte dieses Nachlaufs sind wegen des verkürzten Ablaufs nicht mit
dem 19.11. vergleichbar und hier nicht aufgeführt (Rohdaten in `raw/`).
