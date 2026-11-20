# Messaufbau 1 Mio. Dokumente (PERF-SCALE)

Ergebnisse und Auswertung: [`docs/PERF-SCALE.md`](../../../docs/PERF-SCALE.md).
Alles läuft gegen die Referenz-PostgreSQL (`reference/README.md`, Port 54329), aber nur in der
eigenen DB `pl_scale_1m` – nie in `paperless`.

| Datei | Zweck |
|---|---|
| `gen.py` | synthetischer Datensatz (Seed 20261110), lädt per COPY, baut Sync-Tabellen nach |
| `serve.ps1` | startet eine Messinstanz losgelöst (`Start-Process`, PID-Datei, Log) |
| `run.py` | Lastmessung c=1/c=8, admin + eingeschränkter Nutzer, Szenarien, Profilmodus |
| `explain.sql` | EXPLAIN (ANALYZE, BUFFERS) der langsamsten SQL-Formen |

## Aufbau (≈ 45 min, ≈ 10 GB Platte)

```bash
source reference/env.sh
W=<arbeitsordner>                       # data/, media/ der Messinstanz
PGPASSWORD=$PG_SUPERPASS "$PG_BIN/psql" -U postgres -d postgres \
  -c "CREATE DATABASE pl_scale_1m OWNER paperless" \
  -c "ALTER DATABASE pl_scale_1m SET effective_cache_size = '4GB'"
# Schema v3.4.2 + paperrust_* (Sync-Trigger, Queue …)
PAPERLESS_DBNAME=pl_scale_1m PAPERLESS_DATA_DIR=$W/data PAPERLESS_MEDIA_ROOT=$W/media \
  target/release/paperrust.exe manage migrate
# Daten (Superuser wegen session_replication_role=replica beim COPY)
"$PY" -I tests/bench/scale/gen.py --docs 1000000 --media-root $W/media \
  --dsn "host=127.0.0.1 port=54329 dbname=pl_scale_1m user=postgres password=$PG_SUPERPASS"
# Suchindex / Klassifikator (Zeit messen)
… paperrust.exe manage document_index reindex
… paperrust.exe manage document_create_classifier
```

Kleiner Probelauf: `--docs 10000 --rename-docs 500 --media-docs 100 --correspondents 500`.

## Messen

```powershell
powershell -NoProfile -File tests/bench/scale/serve.ps1 -Exe <paperrust.exe> -Work $W [-Worker]
```

```bash
DSN="host=127.0.0.1 port=54329 dbname=pl_scale_1m user=paperless password=paperless"
"$PY" -I tests/bench/scale/run.py --dsn "$DSN" --out results/bench/scale/<datum>            # Endpunkte
"$PY" -I tests/bench/scale/run.py --dsn "$DSN" --out … --scenarios bulk,rename,ingest     # braucht -Worker
# Abfragen je Request: Server mit -Log "warn,paperrust::access=info,sqlx::query=debug"
"$PY" -I tests/bench/scale/run.py --dsn "$DSN" --out … --profile-log $W/serve-main.log
```

Tokens (lokale Testwerte): admin `5ca1e0000000000000000000000000000000ad01`, eingeschränkter
Nutzer `restricted` `5ca1e0000000000000000000000000000000ee02`.

Die Schreibszenarien sind weitgehend umkehrbar (Tag hinzufügen/entfernen, Speicherpfad nach
`--rename-window` Sekunden abbrechen und Ausgangszustand per SQL + Dateiverschiebung wiederherstellen,
eingelesene Dokumente löschen + Papierkorb leeren). **Nicht** umkehrbar: die „Document Updated“-
Workflows, die der `bulk_edit`-Nachlauf auf die ausgewählten Dokumente anwendet (Zuweisungen von
Tags/Korrespondent/Typ). Für einen sauberen Neuvergleich DB neu erzeugen oder aus dem Dump laden.

Vorsicht im geteilten Cluster: Nach großen `bulk_edit`-Läufen kann `paperrust_sync_sequence()` sehr
lange laufen und `DROP DATABASE … WITH (FORCE)` anderer Nutzer blockieren (docs/PERF-SCALE.md 4.2).
Messinstanz dann beenden und `SELECT paperrust_sync_rebuild();` ausführen.

## DB sichern / wiederherstellen (statt neu erzeugen)

```bash
"$PG_BIN/pg_dump" -Fc -Z 6 -U postgres -d pl_scale_1m -f pl_scale_1m.dump      # Größe nicht gemessen (DB 4,9 GB, Inhalt komprimiert)
"$PG_BIN/createdb" -U postgres -O paperless pl_scale_1m
"$PG_BIN/pg_restore" -U postgres -d pl_scale_1m -j 3 --no-owner --role=paperless pl_scale_1m.dump
```

Dateien (`media/`, ≈ 52 000 Kleinstdateien) erzeugt nur `gen.py`; nach einem Restore entweder
`gen.py` gegen eine Wegwerf-DB mit gleichem Seed laufen lassen oder `media/` mitsichern.
Aufräumen: `DROP DATABASE pl_scale_1m WITH (FORCE)` und den Arbeitsordner löschen.
