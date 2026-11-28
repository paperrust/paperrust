# Freigabe-Prüfung vor der Veröffentlichung (Release Gate)

Vorgabe des Besitzers (2026-11-18): **Vor der Veröffentlichung muss alles fehlerfrei sein – bis ins Detail
geprüft, nicht grob.** Firmen sollen PaperRust einsetzen können. Eine Version wird erst veröffentlicht
(GitHub, Website, Ankündigung), wenn **jeder** Punkt unten mit Nachweis bestanden ist.

## Regeln
- **Nachweis statt Behauptung:** Jeder Punkt hat einen Befehl/Bericht unter `results/release/<version>/`
  (Logs, Zahlen, Screenshots). „Sollte gehen“ zählt nicht.
- **Kein bekannter Fehler** der Stufe kritisch/hoch/mittel offen. Niedrig nur mit Eintrag in den
  Release-Notizen und bewusster Freigabe des Besitzers.
- **Jeder gefundene Fehler bekommt einen Test**, der ihn nachstellt, bevor er behoben wird.
- **Keine wackligen Tests:** Ein Test, der mal grün, mal rot ist, gilt als Fehler und wird geklärt.
- **Unabhängige Prüfung:** Wichtige Teile prüft zusätzlich ein frischer Prüfer (eigener Agent ohne
  Vorwissen, anderes Modell), der den Code liest statt der Zusammenfassung.
- Alle Prüfungen laufen **auf dem Release-Stand** (gleicher Commit), nicht auf Zwischenständen.

## 1. Funktion
- [ ] Alle Rust-Tests aller Crates grün, `cargo clippy --all-targets -D warnings` sauber (Workspace, in
      Stücken nach AGENTS.md).
- [ ] Alle Oberflächen-Tests (vitest), svelte-check ohne Fehler, i18n 10 Sprachen vollständig, Budgets.
- [ ] **Parität zu paperless-ngx:** Paritäts-Suiten (`tests/parity`, `tests/upstream-adapter`) gegen die
      Referenz-Instanz grün; die upstream-Tests (`upstream/src/*/tests/`) sind je Route/Funktion abgebildet
      oder mit Begründung als bewusste Abweichung dokumentiert.
- [ ] **Lückenliste** `docs/LUECKEN-NGX.md`: jede Lücke geschlossen oder als bewusste Abweichung
      begründet; jede Einstellung aus upstream wirksam oder begründet.
- [ ] **Jede Route** (`routes.toml`) mit Erfolgs- und Fehlerfall getestet (Statuscodes, Fehlertexte).
- [ ] **Jede Aktion der Oberfläche** (beide Designs) per Klicktest ausgeführt, nicht nur geöffnet
      (Wächter-Tests: Menü ↔ Seite, klassisch ↔ neu).
- [ ] Alle Dateitypen eingelesen mit echten Beispielen (PDF digital/gescannt/verschlüsselt/kaputt, Bilder
      inkl. HEIC/TIFF mehrseitig, Office alt/neu, E-Mail mit HTML/Anhängen/GPG), Ergebnis inhaltlich geprüft
      (Text, Archiv-PDF/A, Vorschaubild, Seitenzahl, Datum).
- [ ] Workflows: jeder Auslöser × jede Aktion mindestens einmal; Mail-Abruf gegen lokalen IMAP-Testserver.

## 2. Daten und Umstieg
- [ ] Umstieg von paperless-ngx 1.x, 2.x, 3.x (je eine echte bzw. realistische DB inkl. Medien) –
      Zählung, Stichproben-Inhalt, Dateien, Rechte, Benutzer/Passwörter (alle Hasher), Workflows, Mailregeln.
- [ ] Rückweg zu ngx (Export/Import) verlustfrei bzw. dokumentiert.
- [ ] Sicherung/Wiederherstellung (Export, Datenbank, Medien, Ransomware-Schutz) einmal komplett
      durchgespielt, Wiederherstellung auf frischem System geprüft.
- [ ] Konsistenzprüfung (`document_sanity_checker`, eigene Prüfung) nach allen Lasttests: keine
      verwaisten/fehlenden Dateien, Prüfsummen stimmen, Suchindex vollständig.

## 3. Last, Gleichzeitigkeit, Ausfälle
- [ ] Last-Suite `tests/stress` ohne einen einzigen Fehler (5xx, Zeitüberschreitung, Inkonsistenz):
      Massenänderungen während der Verarbeitung, 10–30 Nutzer parallel, gleiches Dokument gleichzeitig,
      parallele Uploads, kaputte/riesige Dateien.
- [ ] **Dauerlauf 24 h** mit gemischter Last (arm64-Kleinrechner oder NAS-ähnlich): Speicher stabil, keine
      wachsenden Handles/Verbindungen, keine Fehler im Log.
- [ ] Ausfälle: Server-Abbruch mitten im Einlesen, Datenbank weg/zurück, Platte voll, Netz weg beim
      Mail-Abruf, KI-Dienst nicht erreichbar → nichts verloren, nichts doppelt, klare Meldungen, Erholung
      ohne Handarbeit.
- [ ] Großes Archiv (≥ 100 000 Dokumente, Stichprobe 1 Mio.): Liste/Suche/Filter/Massenaktionen in den
      Zielzeiten (`docs/PERF-SCALE.md`).
- [ ] **Leistungsvergleich gegen paperless-ngx** (`tools/bench-arm`, gleiche Hardware, Release-Stand):
      Rangfolge des Besitzers: **1. Geschwindigkeit, 2. Speicher.** Geschwindigkeit in allen Szenarien besser
      als ngx (Start, Bedienung, Einlesen je Art, Last, viele Nutzer); Speicher im Leerlauf **mit** Dokumenten
      (Dienste + Datenbank) und Plattenbedarf **mindestens so gut wie ngx minimal**; keine Speicher-Optimierung,
      die die Geschwindigkeit messbar verschlechtert. Plattenbedarf der Datenbank (Entscheidung des Besitzers
      2026-11-19): bewertet wird der Bedarf **pro Dokument** (mindestens so gut wie ngx); ein fester Sockel durch
      PaperRust-eigene Tabellen (einige MB, wächst nicht mit den Dokumenten) ist erlaubt
      (`results/release/2026-11-19-platte-und-start.md`).

## 4. Sicherheit
- [ ] Sicherheits-Suite `tests/security` grün; **Rechte-Matrix**: jede Route × Rolle (anonym, Nutzer ohne
      Rechte, Leser, Bearbeiter, Besitzer, Admin, Superuser) × fremdes/eigenes Objekt geprüft.
- [ ] Anmeldung: Passwort, 2FA, Social/OIDC (gegen lokale Test-Anbieter), Sperren/Drosselung über
      mehrere Prozesse, Sitzungen, CSRF, Token.
- [ ] Eingaben: Fuzzing der Parser (PDF, Bilder, Office, E-Mail, ZIP-Import) mit cargo-fuzz über
      mehrere Stunden ohne Absturz/Hänger außerhalb der Kindprozess-Grenzen.
- [ ] Abhängigkeiten: `cargo audit`/`cargo deny` ohne offene Sicherheitsmeldung, npm audit; Lizenzen
      geprüft (NOTICE vollständig).
- [ ] Keine Geheimnisse in Repo/Geschichte/Logs/Fehlermeldungen (Suchlauf über die ganze Git-Geschichte).
- [ ] Unabhängige Sicherheitsprüfung des Codes durch einen frischen Prüfer (Anmeldung, Rechte, Dateien,
      Fremdaufrufe/SSRF, Vorlagen-Sandbox).

## 5. Plattformen und Betrieb
- [ ] Docker-Paket amd64 + arm64 gebaut, gestartet, Grundablauf (Anlegen, Einlesen, Suchen, Sichern).
- [ ] Desktop Windows: alle Betriebsarten, echte Installation/Deinstallation/Update auf einem
      Testrechner (nicht dem des Besitzers).
- [ ] NAS-Vorlagen (Synology/QNAP/Unraid/CasaOS) mindestens einmal echt gestartet.
- [ ] Browser: aktuelle Chrome/Edge, Firefox, Safari (macOS/iOS), Android; Handy-Ansicht.
- [ ] Barrierefreiheit: Tastatur-Bedienung, Kontraste, Screenreader-Stichprobe.

## 6. Doku und Auftritt
- [ ] Doku (de/en) stimmt mit dem Verhalten überein: jede Einstellung, jeder Befehl, jede Anleitung
      einmal nachvollzogen.
- [ ] Release-Notizen vollständig, bekannte Grenzen ehrlich genannt.
- [ ] Website: Impressum/Datenschutz vollständig, Downloads mit Prüfsummen, Leistungszahlen nur gemessen
      und mit Bedingungen.

## Freigabe
Freigabe nur durch den Besitzer, auf Grundlage des Berichts `results/release/<version>/FREIGABE.md`
(alle Punkte mit Nachweis-Link). Fällt ein Punkt durch: beheben, Test ergänzen, **betroffene Prüfungen
komplett wiederholen**.
