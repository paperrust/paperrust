# PaperRust für Firmen – Konzept

Stand 2026-11-16. Entscheidungsvorlage, noch nichts davon umgesetzt (außer dem, was unter „Gibt es
schon“ steht). Anlass: Wunsch des Besitzers nach einem Server ohne Bildschirm („headless“), mit dem
sich jede Mitarbeiterin und jeder Mitarbeiter über die eigene Desktop-App verbindet und Dokumente
auch offline hat.

## 1. Das Bild

```
                ┌──────────────────────────────┐
                │  PaperRust-Server (Docker)   │   eine Datenbank, ein Medienordner,
                │  Firmen-Server, NAS, Cloud   │   Einlesen, OCR, Rechte, Sicherung
                └──────────────┬───────────────┘
          HTTPS, je Gerät ein eigener Schlüssel (Geräte-Token)
     ┌──────────────┬──────────┴─────┬──────────────────┐
  App Anna        App Ben         App Carla         Browser (optional)
  Sync: Tag       nur online      Sync: „Verträge“   z. B. Buchhaltung
  „Rechnungen“                    offline auf Reise
```

Der Server ist der einzige Ort, an dem Dokumente verarbeitet und dauerhaft gespeichert werden.
Die Apps sind „Fenster“ darauf; mit Sync halten sie zusätzlich eine Teilkopie zum Offline-Lesen.

## 2. Gibt es schon

| Baustein | Stand | Wo |
|---|---|---|
| Server ohne Bildschirm | fertig (Docker-Image, läuft als Dienst) | docker/, docs/DEPLOY.md |
| App „mit Server verbinden“ (2a, Server-Oberfläche im Fenster) | fertig, E2E vom 2026-11-09 | docs/DESKTOP.md §22 |
| App mit eigener Oberfläche (2b) und Sync (3, Offline-Lesen und -Suche) | gebaut, E2E vom 2026-11-09 | docs/REMOTE-APP.md, docs/SYNC.md |
| Sync-Auswahl je Gerät (Tags, Ansichten, Dokumente) mit Grenzen (2 GiB / 5 000 Dokumente) | fertig | docs/SYNC.md §4.4 |
| Rechte: jedes Gerät sieht nur, was der Benutzer sehen darf | fertig (Server prüft jede Abfrage) | pl-authz |
| Geräte-Token je Gerät, widerrufbar durch den Benutzer selbst | fertig | `/rs/api/v2/auth/devices`, pl-authz/device.rs |
| Anmeldung mit Firmenkonto (OpenID Connect, z. B. Microsoft Entra, Google, Keycloak) | im Browser fertig | docs/user/configuration.md |
| Offline-Änderungen später senden (3b, „Outbox“) | nur Entwurf | docs/SYNC.md §6 |

Ein eigener „Headless-Modus“ ist deshalb nicht nötig. Was für Firmen fehlt, ist rund um Verteilung,
Verwaltung und Anmeldung.

## 3. Was fehlt (Vorschlag in Stufen)

### Stufe F1 – Verteilen und verwalten (zuerst, klein bis mittel)

1. **Weboberfläche abschaltbar.** Neue Einstellung `PAPERRUST_WEB_UI=on|off|admin` (Standard `on`).
   `off`: Browser bekommt nur eine kurze Seite „Bitte die PaperRust-App verwenden“ (+ Download-Link),
   API und Apps laufen weiter. `admin`: Browser nur für Administratoren (Einstellungen, Benutzer).
   Empfehlung: als Wahl anbieten, nicht erzwingen – viele Firmen wollen beides.
2. **Vorkonfigurierte Installation.** Installer-Schalter für die IT, z. B.
   `PaperRust-setup.exe /S /SERVER=https://dokumente.firma.ch /MODE=sync /LOCK`:
   Server-Adresse fest eingetragen, Betriebsart gewählt, Wahl beim ersten Start übersprungen,
   mit `/LOCK` in der App nicht änderbar. Zusätzlich lesbar aus einer Richtlinie
   (Windows-Registry `HKLM\Software\Policies\PaperRust`), damit Gruppenrichtlinien/Intune gehen.
3. **Schlanke „Client“-Variante des Installers** ohne PostgreSQL, OCR und PDFium (heute ~67 MB,
   geschätzt < 20 MB). Die volle Variante bleibt für Einzelplätze.
4. **Geräteübersicht für Administratoren.** Neue Seite unter Verwalten: alle Geräte aller Benutzer
   (Name, Benutzer, zuletzt gesehen, Sync-Umfang), „Gerät sperren“. Beim nächsten Kontakt löscht die
   App dann die lokale Kopie und meldet sich ab (verlorener Laptop, Austritt). Heute kann nur der
   Benutzer selbst seine Geräte sehen und widerrufen.

### Stufe F2 – Anmeldung und Vorgaben (mittel)

5. **App-Anmeldung mit Firmenkonto.** Heute braucht das Geräte-Token Benutzername + Passwort.
   Neu: die App öffnet den System-Browser zur Firmen-Anmeldung (OpenID Connect, „Gerätecode“ oder
   Rücksprung auf `http://127.0.0.1:<port>`), der Server stellt danach das Geräte-Token aus.
   Kein Passwort in der App, MFA erledigt der Firmen-Anbieter.
6. **Sync-Vorgaben durch den Admin.** Je Gruppe: Sync erlaubt ja/nein, Höchstgrenzen, Pflicht-Auswahl
   (z. B. „Team Einkauf bekommt immer Tag Lieferanten“), Dateien ja/nein (nur Text), offline höchstens
   N Tage ohne Kontakt, danach lokale Kopie sperren.
7. **Verschlüsselte lokale Kopie.** Replikat und Dateicache verschlüsselt, Schlüssel im
   Betriebssystem-Schlüsselbund (Windows DPAPI / macOS Keychain). Heute liegen sie im Profilordner.

### Stufe F3 – Offline arbeiten (größer)

8. **Offline-Änderungen (3b).** Tags, Titel, Notizen offline ändern und Dokumente offline erfassen;
   beim nächsten Kontakt senden. Konflikte: Server gewinnt, die eigene Fassung bleibt sichtbar
   (Entwurf steht in docs/SYNC.md §6).
9. **Protokoll für Prüfer.** Wer hat wann von welchem Gerät welches Dokument geöffnet oder
   heruntergeladen (Audit-Log gibt es für Änderungen schon, nicht für das Lesen).

### Stufe F4 – Anbindung an ERP und Buchhaltung (mittel bis groß)

Ziel: Ein Dokument bekommt in PaperRust z. B. den Tag „Buchhaltung“ → es landet automatisch im
ERP oder in der Buchhaltung (als Beleg mit PDF), und Rückmeldungen von dort (Belegnummer, „bezahlt“)
kommen zurück.

**Gibt es schon als Grundlage:** Workflows mit Auslöser „Dokument hinzugefügt / geändert“ und
Aktion „Webhook“ (upstream-kompatibel). Damit kann man heute schon eine eigene Schnittstelle
anstoßen – aber nur mit Bastelei auf der Gegenseite und ohne Rückweg.

**Vorschlag:**

10. **Auslöser „Tag hinzugefügt / entfernt“** für Workflows (heute nur „Dokument geändert“ mit
    Filter). Damit ist „Tag Buchhaltung gesetzt“ ein sauberer, einmaliger Auslöser.
11. **Aktion „An System übergeben“** mit fertigen Anschlüssen (je Anschluss eine kleine
    Zuordnung in der Oberfläche: welches PaperRust-Feld wohin):

    | System | Weg | Ergebnis dort |
    |---|---|---|
    | ERPNext | REST-API mit API-Schlüssel | Eingangsrechnung als Entwurf + PDF als Anhang |
    | Odoo | JSON-RPC/REST mit API-Schlüssel | Lieferantenrechnung (Entwurf) + PDF im Chatter |
    | bexio, Abacus (Schweiz) | REST-API / OAuth | Kreditorenbeleg + PDF |
    | SAP Business One | Service Layer (REST) | Eingangsrechnung als Entwurf + Anhang |
    | SAP S/4HANA | Anhang-/Dokumentdienst (OData) | PDF an bestehenden Beleg hängen; Belegerstellung meist über die Firmen-IT |
    | alles andere | Webhook (gibt es) oder Ablage in einen Ordner / per Mail (DMS-Eingang des ERP) | wie heute, mit festem Format |

12. **Rückweg:** Belegnummer und Status aus dem ERP landen in einem benutzerdefinierten Feld
    (z. B. „ERP-Beleg“ = Link, „Bezahlt“ = ja/nein), entweder per Abfrage alle paar Minuten oder per
    Webhook vom ERP. Danach kann ein Workflow z. B. den Tag „Buchhaltung“ durch „Verbucht“ ersetzen.
13. **Rechnungsdaten erkennen,** damit im ERP nicht nur ein PDF ankommt: Betrag, Währung, Datum,
    Lieferant, Rechnungsnummer, IBAN. In der Schweiz zuerst die **QR-Rechnung** auslesen (genormter
    QR-Code, sehr zuverlässig), danach ZUGFeRD/Factur-X/XRechnung (Daten stecken im PDF), erst dann
    Texterkennung/KI als Vorschlag.
14. **Sicher und nachvollziehbar:** Zugangsdaten der Systeme verschlüsselt gespeichert und nie
    angezeigt; jede Übergabe mit Ergebnis im Protokoll am Dokument („an Odoo übergeben, Beleg 2026/0042“);
    Fehler → Wiederholen-Knopf, keine Doppelbuchung (Kennung je Übergabe, das ERP prüft auf Doppel).

15. **Nicht nur Buchhaltung: ein Anschluss-Baukasten.** Die Anschlüsse aus Punkt 11 bauen auf einem
    gemeinsamen Gerüst auf (Auslöser → Feldzuordnung → Übergabe → Rückweg → Protokoll), damit weitere
    Systemarten wenig Aufwand sind:

    | Systemart | Beispiele | typischer Fall |
    |---|---|---|
    | Lernplattformen (LMS) | Moodle, ILIAS, Canvas | Schulungsnachweis/Zertifikat mit Tag „Weiterbildung“ → in die Teilnehmerakte des Kurses; Kursunterlagen aus einem Tag als Material bereitstellen; Abschlüsse aus dem LMS als Dokument zurück ins Archiv |
    | Personal (HR) | Abacus HR, Personio, SAP SuccessFactors | Tag „Personaldossier“ + Feld „Mitarbeiter“ → Dokument ins Dossier |
    | Kunden (CRM) | Odoo CRM, HubSpot, Salesforce | Vertrag/Offerte mit Korrespondent X → an den Kunden-Datensatz hängen |
    | Ablage/Zusammenarbeit | SharePoint/OneDrive, Nextcloud, Google Drive | Kopie in einen Team-Ordner, Link zurück am Dokument |
    | Eigene Systeme | Webhook, Ordner, Mail | wie heute, mit festem Format und Rückweg |

    Für Moodle und ILIAS gibt es offene Web-Schnittstellen (REST mit Token); welche Aktionen
    möglich sind, hängt von den Rechten des Schnittstellen-Benutzers ab.

Empfohlene Reihenfolge: 10 + 13 (QR-Rechnung) → das Gerüst (15) mit einem ersten Anschluss (der, den
der Besitzer selbst nutzt) → 12 → weitere Anschlüsse.

## 4. Bewusst nicht

- Kein eigener zweiter Servertyp „headless“: es bleibt **ein** Server, die Weboberfläche ist nur eine
  Einstellung.
- Keine Verarbeitung (OCR) auf den Apps: Einlesen bleibt auf dem Server, die App legt Dokumente nur ab.
- Kein Peer-to-Peer zwischen Apps: alles läuft über den Server (einfacher, Rechte an einem Ort).

## 5. Reihenfolge und grober Aufwand

| Stufe | Inhalt | Aufwand grob |
|---|---|---|
| F1 | Weboberfläche abschaltbar, vorkonfigurierte Installation, Client-Installer, Geräteübersicht | 1–2 Wochen Agentenarbeit |
| F2 | App-Anmeldung mit Firmenkonto, Sync-Vorgaben, verschlüsselte Kopie | 2–3 Wochen |
| F3 | Offline-Änderungen, Lese-Protokoll | 3+ Wochen |
| F4 | Tag-Auslöser, QR-Rechnung, erster ERP-Anschluss, Rückweg | 2–3 Wochen, je weiterer Anschluss ~1 Woche |

Vorher nötig: der komplette Desktop-Test aller Betriebsarten mit dem aktuellen Stand (2b/3 wurden
zuletzt am 2026-11-09 geprüft, vor dem Datenmodell-Umbau).

## 6. Offene Fragen an den Besitzer

1. Soll die Weboberfläche in Firmen standardmäßig an bleiben (Empfehlung) oder aus?
2. Welche Firmen-Anmeldung zuerst: Microsoft Entra (Office 365) oder allgemein OpenID Connect?
3. Nur Windows-Apps oder auch macOS (und später Handy)?
4. Ist Offline-Ändern (F3) wichtig oder reicht Offline-Lesen?
5. Welches System zuerst (ERP/Buchhaltung wie ERPNext, Odoo, bexio, Abacus, SAP – oder ein LMS wie Moodle/ILIAS)? Und F4 vor oder nach F1–F3?
