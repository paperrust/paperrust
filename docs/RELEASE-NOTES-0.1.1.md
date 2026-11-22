# PaperRust 0.1.1 – Release-Notizen

[English](RELEASE-NOTES-0.1.1.en.md) · Stand 2026-11-22

Eine Wartungsversion nach [0.1.0](RELEASE-NOTES-0.1.0.md): Korrekturen aus dem ersten Echttest mit einer
Kopie einer echten Sammlung, bessere Texterkennung und ein überarbeiteter KI-Assistent. Datenbank und
Dateien bleiben unverändert – ein Update von 0.1.0 ist ein Austausch des Images bzw. Programms.

## Korrigiert

- **Suche nach dem Umstieg leer:** Nach dem Umstieg von paperless-ngx konnte der Suchindex leer bleiben
  (0 Treffer bis zu einem manuellen Neuaufbau). PaperRust baut ihn jetzt beim Start selbst neu auf, wenn er
  leer ist oder weniger als die Hälfte der Dokumente enthält, und der Umstieg markiert ihn zum Neuaufbau.
- **Fotos und Bilddateien:** Bilder gehen in Graustufen an die Texterkennung. Bei geradegerichteten
  Farbfotos gingen sonst einzelne Wörter (Umlaute) verloren.
- **Groß-/Kleinschreibung bei Regeln:** Reguläre Ausdrücke mit Lookaround in Zuordnungs- und
  Workflow-Regeln ignorierten die Groß-/Kleinschreibung nicht wie eingestellt.
- **Aus paperless-ngx nach 3.4.2 übernommen:** Passwörter in Einstellungen immer mit gleich langer Maske,
  portabler Dateinamen-Filter in Workflows, Eingabefelder melden Werte schon beim Tippen.

## Neu

- **Texterkennung Deutsch und Englisch als Standard** (`deu+eng` statt nur `eng`). Kostet 3–7 % Zeit,
  deutsche Texte werden deutlich besser gelesen. Ohne deutsche Sprachdaten bleibt es bei `eng`.
- **Sprache automatisch (optional):** `PAPERRUST_OCR_LANGUAGE_AUTO=true` erkennt die Sprache des Textes und
  liest ein Dokument in einer anderen installierten Sprache (z. B. Französisch) ein zweites Mal damit.
- **KI-Assistent:**
  - findet auch ohne Embeddings passende Stellen (Stichwörter aus der Frage für die Volltextsuche)
  - zeigt Fortschritt mit Sekunden
  - läuft weiter, wenn man die Seite verlässt
  - „N Quellen öffnen“ zeigt genau diese Dokumente
- **Seitenleiste:** Eintrag „KI-Assistent“, wenn KI eingeschaltet ist; auf breiten Bildschirmen auf Symbole
  einklappbar.
- **KI-Einstellungen:** Felder für den Embedding-Dienst und Stand des KI-Index mit Knopf zum Aktualisieren.

## Bekannte Grenzen

Unverändert gegenüber [0.1.0](RELEASE-NOTES-0.1.0.md#bekannte-grenzen). Neu bekannt: Eine Suche mit zwei
Wörtern verlangt beide Wörter (UND); paperless-ngx 3.1.3 lieferte im Vergleichstest mehr Treffer. Das wird
für die nächste Version geprüft.

## Update

- Docker: Image auf `ghcr.io/paperrust/paperrust:0.1.1` stellen und neu starten. Der Suchindex wird beim
  ersten Start bei Bedarf neu aufgebaut.
- Desktop/Linux-Paket: neues Paket installieren.
- Wie immer vorher sichern.
