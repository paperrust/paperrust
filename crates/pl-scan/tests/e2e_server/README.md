# E2E im laufenden Server (Scanner-Anbindung)

Durchgeführt am 2026-11-13, Ergebnis in docs/contract/pl-scan.md („Ende-zu-Ende im laufenden Server“).

1. Wegwerf-DB: `CREATE DATABASE pl_scan_e2e TEMPLATE paperless_snapshot OWNER paperless`, Datenordner
   aus `reference/snapshot/data.tar` nach `$E2E_DIR/run`, `paperrust.exe` nach `$E2E_DIR/bin/paperrust-scan.exe` kopieren.
2. `bash run_server.sh migrate`, dann `bash run_server.sh` (App 127.0.0.1:8131, SMTP 127.0.0.1:2531, Light-UI aus `frontend-light/dist`).
3. Test-Scanner: `node fake-escl.mjs 8732 ../data/caps_officejet.xml <bild.jpg> 2`
4. Gerät per API anlegen (Antwort als `dev.json` speichern), dann `node smtp-send.mjs 2531 dev.json <datei.pdf> <name.pdf>`.
5. Aufräumen: Prozesse beenden, `DROP DATABASE pl_scan_e2e WITH (FORCE)`, `$E2E_DIR` löschen.
