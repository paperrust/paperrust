# Paralleles Einlesen auf dem Raspberry Pi 5 (2026-11-22/23)

Gerät: Raspberry Pi 5 8 GB (HutoPi-Kit mit Lüfter, microSD), Docker. PaperRust aus `main` (Commit 818547ef,
arm64 selbst gebaut, thin LTO), ein Abbild für alle Läufe. Messskript `results/release/tools/bench.py`
(PRBENCH_CPUS=0-2 → 3 Kerne für PaperRust, Messung auf Kern 3), Variablen über `PRBENCH_RS_ENV`.

| Lauf | Einstellung | 255 Dokumente einlesen | einzelner Scan 8 Seiten | einzelner Scan 1 Seite | p95 10 Nutzer |
|---|---|---|---|---|---|
| ser | `TASK_WORKERS=1`, Threads automatisch (= 3) | 214 s | 14,8 s | 8,6 s | 150 ms |
| par | beides automatisch (2 Aufgaben × 1 Thread) | **162 s (−24 %)** | **24,1 s (+63 %)** | 8,5 s | 228 ms |
| par3 | `THREADS_PER_WORKER=3`, Aufgaben automatisch | 205 s | 14,9 s | 8,6 s | 196 ms |

Hinweis zu par3: Mit gesetzten 3 Threads rechnet die Automatik `ceil(3 Kerne ÷ 3) = 1` Aufgabe – par3 ist
also dieselbe Einstellung wie ser. Der Unterschied 214 s ↔ 205 s zeigt die Streuung zwischen Läufen (≈ 5 %).

Alle Läufe: 0 Fehler unter Last (1, 5, 10 Nutzer), Leerlauf mit Dokumenten 320–400 MiB.

## Entscheidung

Das automatische parallele Einlesen macht Stapel ein Viertel schneller, aber einzelne mehrseitige Scans um
zwei Drittel langsamer, weil jedes Dokument nur noch einen Teil der Kerne bekommt. Geschwindigkeit einzelner
Dokumente hat Vorrang → in 0.1.2 bleibt der Standard **eine Aufgabe** (wie paperless-ngx). Parallel per
`PAPERRUST_TASK_WORKERS=auto` oder einer Zahl. Die Fehlerbehebungen gegen doppelte ASN, doppelte Dokumente und
doppelte Tags bleiben aktiv (formal geprüft, `verification/parallel-ingest/`).

Nächster Schritt: Kerne dynamisch verteilen (ein Dokument allein bekommt alle Kerne, mehrere teilen sie sich),
dann erneut messen.

Rohdaten: `log_*.txt`, `run_ser_rs.json`, `run_par_rs.json` (par3-JSON folgt nach Laufende).
