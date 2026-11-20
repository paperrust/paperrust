# Benchmark paperless-ngx 3.4.2 gegen PaperRust (Jetson Orin Nano)

Ergebnis vom 2026-11-18: `results/benchmark/2026-11-18-jetson/bericht.md` (Rohdaten daneben).

## Aufstellungen

| Kürzel | Inhalt | Container |
|---|---|---|
| `ngxb` | ngx „vollständig“ nach `docker-compose.postgres-tika.yml`: ngx + Valkey + Tika + Gotenberg + PostgreSQL 17 | 5 |
| `ngxc` | wie `ngxb`, aber `PAPERLESS_OCR_CLEAN=none`, `PAPERLESS_OCR_DESKEW=false` („gleiche Arbeit“ wie PaperRust, das beides noch nicht kann) | 5 |
| `ngxa` | ngx „minimal“: ngx + Valkey + PostgreSQL 17 (kann kein DOCX) | 3 |
| `rs` | PaperRust (ein Container) + PostgreSQL 17 | 2 |

Alle: deu+eng, Europe/Zurich, `THREADS_PER_WORKER=4`, OCR-Modus und Archiv-PDF auf Standard (`auto`),
frische Volumes je Lauf, Container auf Kernen 0–3, Messprogramm auf 4–5.

## Dateien

| Datei | Zweck |
|---|---|
| `lib.sh`, `build.sh` | PaperRust-Image nativ (arm64) bauen, in eigenem BuildKit-Builder `prbench_builder` |
| `gen_docs.py` | erfundene Testdokumente: 200 Text-PDFs + 30 Scans (`text/`, `scan/`, Manifest mit Solltext), `--detail` 25 Einzeldokumente (Text-PDF, Scan 1/8 Seiten, Fotos JPG/PNG schief, DOCX), `--extra N` eindeutige Text-PDFs |
| `memsample.py` | RAM (PSS je Container-cgroup, `memory.stat` anon) und CPU-Zeit (`cpu.stat`); läuft als root; schreibt `.abort`, wenn < 900 MB frei |
| `bench.py` | ein Lauf: `run <runde> <ngxa\|ngxb\|ngxc\|rs> [--smoke] [--short]` (S1 Leerlauf, S7 Einlesen je Art, S4 230 Dokumente + Bedienung + Massenbearbeitung, S6 Platte, S2 1 Nutzer 15 min, S3 5/10 Nutzer je 5 min; `--short`: S1 nur 1 min, ohne S2/S3), `big <aufstellung> <anzahl>` (S5 größeres Archiv) |
| `breakdown.py` | zerlegt die S7-Zeit je Dokument aus dem App-Protokoll (Warten auf Eingangsordner, Verarbeitung, bis Suche) |
| `run_all.sh` | Warteschlange aller Runden mit wechselnder Reihenfolge, hält die Sperre, wartet auf fremde Rechenjobs |
| `summarize.py` | Tabellen (Median, Spannweite, Faktor gegen jede ngx-Aufstellung) und `rohdaten.json` |

## Wiederholen (auf dem Jetson, Arbeitsordner `~/prbench`)

```sh
# Quelle: git archive <commit> + upstream/{LICENSE,src/documents/static,src/documents/resources} nach ~/prbench/src
bash ~/prbench/tools/build.sh                       # Image prbench_paperrust:local
sudo docker pull ghcr.io/paperless-ngx/paperless-ngx:3.4.2 postgres:17 valkey/valkey:9-alpine \
     gotenberg/gotenberg:8.38 apache/tika:3.3.1.0
python3 ~/prbench/tools/gen_docs.py ~/prbench/docs
python3 ~/prbench/tools/gen_docs.py ~/prbench/docs --detail
python3 ~/prbench/tools/gen_docs.py ~/prbench/docs --extra 10000
for s in rs ngxb ngxa; do taskset -c 4,5 python3 ~/prbench/tools/bench.py run 0 $s --smoke; done   # Funktionstest
setsid nohup bash ~/prbench/tools/run_all.sh > ~/prbench/run_all.log 2>&1 < /dev/null &
taskset -c 4,5 python3 ~/prbench/tools/bench.py big ngxb <anzahl>   # S5, je Aufstellung einmal
python3 ~/prbench/tools/summarize.py ~/prbench/results > tabellen.md
```

Regeln des geteilten Rechners (`~/jobs/REGELN.md`): Sperre `~/jobs/paperrust_bench.lock`, vor dem Start
≥ 2000 MB frei; liegen fremde `mpc_*.lock`, wartet `run_all.sh` bis zu 2 h, danach laufen die Container
nur auf 2 Kernen (`cpuset 0-1`, steht in den Rohdaten).

Hinweise:
- Der ngx-Container übernimmt den Eingangsordner (Besitzer UID 1000); `bench.py` holt ihn vor jedem Lauf
  per `sudo chown` zurück.
- Beide Systeme warten standardmäßig 5 s, bis eine Datei im Eingangsordner „ruhig“ ist
  (`CONSUMER_STABILITY_DELAY`). Diese 5 s stecken in jeder S7-Zeit „bis durchsuchbar“;
  `breakdown.py` weist die reine Verarbeitungszeit getrennt aus.
- Das `docker/Dockerfile` im Stand c97f15c ließ sich nicht parsen; `build.sh` nutzte eine korrigierte
  Kopie (im Repo inzwischen behoben).
