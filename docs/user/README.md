# PaperRust – Benutzer-Dokumentation

[English](en/README.md)

PaperRust ist ein schneller, schlanker und kompatibler Ersatz für
[paperless-ngx](https://github.com/paperless-ngx/paperless-ngx): eigene Weboberfläche, aber dieselbe
REST-API und dieselbe Datenbank – als ein einzelnes Rust-Programm ohne Python, Redis, Celery, Tika und
Gotenberg. Neu in Version 0.1.0 und bekannte Grenzen: [Release-Notizen](../RELEASE-NOTES-0.1.0.md).

## Einstieg

| Seite | Inhalt |
|---|---|
| [Erste Schritte](getting-started.md) | Betriebsart wählen, einrichten, erste Dokumente |
| [Installation](installation.md) | Server mit PostgreSQL, Docker, Desktop-App |
| [Von paperless-ngx umsteigen](umstieg.md) | Schritt für Schritt in einfachen Worten: ein Befehl, Abgleich am Ende |
| [Umstieg von paperless-ngx (ausführlich)](migration.md) | gleiche Datenbank, SQLite/MariaDB → PostgreSQL, Export/Import, zurück |
| [Konfiguration](configuration.md) | alle Variablen (`PAPERRUST_*` und kompatibel `PAPERLESS_*`) |

## Nutzung

| Seite | Inhalt |
|---|---|
| [Nutzung](usage.md) | Begriffe, Oberfläche (PaperRust-Design und klassisch), Einlesen, Suche, gespeicherte Ansichten, Freigaben, Papierkorb, Verlauf |
| [Automatik](automatik.md) | Workflows mit Satz-Ansicht, Vorlagen, Testlauf, Verlauf und Sonst-Zweig; E-Mail-Abruf; KI-Vorschläge |
| [Mail](mail.md) | Mail-Konten und -Regeln, verarbeitete Mails |
| [Workflows](workflows.md) | Nachschlagen: Auslöser, Filter, Aktionen, Platzhalter |
| [Custom Fields](custom-fields.md) | eigene Felder |
| [Benutzer und Rechte](permissions.md) | Benutzer, Gruppen, globale und Objektrechte, MFA |
| [Dateinamen und Speicherpfade](storage-paths.md) | Ablage-Vorlagen |
| [ASN-Etiketten](asn-labels.md) | Etiketten mit Archivnummer drucken, Stapel automatisch aufteilen, Trennblätter |
| [Scanner verbinden](scanner.md) | Scannen-Knopf (AirScan/eSCL) und Scan-to-Email vom Drucker ohne NAS-Passwort am Gerät |
| [Fortgeschritten](advanced.md) | Zuordnung (Matching), Barcodes, Einlese-Skripte, doppelseitige Scans, SSO |
| [KI-Funktionen](ai.md) | ersetzt paperless-ai/-gpt: Vorschläge mit Prüfliste, Benutzerfelder, KI-Texterkennung, Chat, MCP-Server |
| [PDF-Werkzeuge](pdf-tools.md) | Schwärzen, Verkleinern, In PDF umwandeln, Reparieren beim Einlesen |

## Betrieb

| Seite | Inhalt |
|---|---|
| [Administration](administration.md) | Sicherung, `paperrust manage`, Konsistenzprüfung, Index, Klassifikator |
| [Integritätsprüfung](integrity.md) | geplante Prüfung von Dateien und Prüfsummen, inkrementeller Modus, JSON-Bericht |
| [Schutz vor Ransomware](ransomware.md) | Wächter mit Nur-Lese-Modus, verschlüsselte Sicherungen mit Versionen (auch S3 mit Object Lock), Ratschläge für die NAS |
| [REST-API](api.md) | API (wie paperless-ngx) und PaperRust-Erweiterungen |
| [Sicherheit](security.md) | Betrieb, Rechte, Import, KI |
| [Für Firmen](firmen.md) | Betrieb mit vielen Mitarbeitenden: Anmeldung, Rollen, Schutz – und was noch fehlt |
| [Häufige Fragen](faq.md) | |
| [Fehlerbehebung](troubleshooting.md) | |

Zuordnung der Doku-Links der Oberfläche: [LINKMAP](LINKMAP.md). Entwickler-Dokumentation:
[docs/ARCHITECTURE.md](../ARCHITECTURE.md), [docs/UPSTREAM-SYNC.md](../UPSTREAM-SYNC.md),
[CONTRIBUTING.md](../../CONTRIBUTING.md) (Englisch).

---

PaperRust beruht auf paperless-ngx (GPL-3.0, © die paperless-ngx-Autoren) und steht ebenfalls unter
der GPL-3.0. Es ist ein unabhängiges Projekt und nicht mit dem paperless-ngx-Team verbunden. Diese
Dokumentation ist eigener Text.
