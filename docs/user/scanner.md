# Scanner verbinden

[English](en/scanner.md)

PaperRust kann Scans auf zwei Wegen annehmen, **ohne dass auf dem Scanner ein Passwort für Ihr
NAS oder Ihren Server gespeichert ist**. Beide Wege geben das Ergebnis in die normale
Verarbeitung – wie ein Upload: OCR, Zuordnung, Workflows, Rechte.

| Weg | Wo starten Sie den Scan? | Was braucht der Scanner? |
|---|---|---|
| **Scannen-Knopf** (AirScan/eSCL) | in PaperRust am Computer oder Handy | nichts – PaperRust holt die Seiten ab |
| **Scan-to-Email** (Geräte-E-Mail) | am Drucker | ein eigenes Token, das nur hochladen kann |

Der bisherige Eingangsordner (Consume-Ordner) bleibt, wie er ist.

> **Stand:** Beide Wege sind gegen einen nachgebauten Test-Scanner und einen lokalen SMTP-Client
> geprüft. **Der Test mit einem echten HP OfficeJet Pro 9010 steht noch aus.** Menünamen im
> Drucker können je nach Firmware etwas anders heißen.

## Weg 1: Scannen-Knopf (AirScan / eSCL)

Viele Netzwerkdrucker und -scanner sprechen **eSCL** (bei Apple „AirScan“), darunter der
HP OfficeJet Pro 9010. PaperRust fragt den Scanner direkt über das Netz.

### Einrichten (Superuser)

1. **Administration → Scanner** öffnen (`/rs/app/tools/scanners`).
2. **Netz durchsuchen** klicken. Gefundene Scanner mit **Hinzufügen** übernehmen.
   - Nichts gefunden? Die Adresse des Druckers eintragen (z. B. `192.168.1.20`) und
     **Mit Adresse eintragen** klicken. Die IP steht im Druckermenü unter *Netzwerk* oder auf der
     Netzwerk-Konfigurationsseite.
   - In **Docker** mit normalem Bridge-Netz findet die Suche nichts (mDNS kommt nicht in den
     Container). Dann immer die Adresse eintragen.
3. Tipp: Geben Sie dem Drucker im Router eine **feste IP-Adresse**, sonst ändert sie sich irgendwann.

PaperRust prüft beim Eintragen, ob unter der Adresse wirklich ein eSCL-Scanner antwortet.
Normale Benutzer sehen nur den Namen, nicht die Adresse.

### Scannen

1. **Scannen** in der Seitenleiste öffnen (`/rs/app/tools/scan`). Nötig ist dasselbe Recht wie
   zum Hochladen: „Dokumente hinzufügen“.
2. Scanner wählen, **Quelle** (Einzug oder Vorlagenglas), bei Bedarf **Beidseitig**,
   **Auflösung** (Standard 300 dpi) und **Farbe**. Optional einen Titel.
3. **Scannen** klicken. PaperRust holt Seite für Seite, bis der Einzug leer ist, macht daraus
   **ein PDF** und gibt es in die Verarbeitung.

Das Dokument **gehört dem Benutzer, der den Knopf gedrückt hat**, wie bei einem Upload über die
Oberfläche. Workflows mit dem Auslöser „Dokument hinzugefügt“ und der Quelle **Web-UI** greifen.

| Meldung | Was tun |
|---|---|
| Scanner nicht erreichbar | Drucker an? Im selben Netz? Adresse noch richtig? |
| Scanner belegt | Am Drucker läuft gerade etwas. Kurz warten. |
| Einzug leer | Papier in den Einzug legen oder „Vorlagenglas“ wählen. |
| Papierstau | Stau am Gerät beheben und neu scannen. Es wurde nichts gespeichert. |

## Weg 2: Scan-to-Email vom Drucker (Geräte-E-Mail)

Für Scans, die Sie **am Drucker** starten. PaperRust hat dafür einen kleinen, eingebauten
**SMTP-Empfang**. Er nimmt nur Mails von Ihren Geräten an und **leitet nie etwas weiter**.

### 1. Empfang einschalten (Superuser)

**Einstellungen → System** (oder Umgebungsvariablen), danach PaperRust neu starten:

| Einstellung | Wert | Bedeutung |
|---|---|---|
| [`PAPERRUST_SMTP_INTAKE`](configuration.md#paperrust_smtp_intake) | `on` | Empfang an (Standard: aus – dann lauscht nichts) |
| `PAPERRUST_SMTP_INTAKE_LISTEN` | `0.0.0.0:2525` | Adresse und Port |
| `PAPERRUST_SMTP_INTAKE_ALLOWED_IPS` | z. B. `192.168.1.20` | nur diese Absender (leer = alle) |
| `PAPERRUST_SMTP_INTAKE_MAX_MB` | `50` | größte Nachricht |

Port 25 ist unter Linux nur mit Administratorrechten möglich, deshalb ist 2525 der Standard.
Fast alle Drucker erlauben einen eigenen Port.

**Docker:** Den Port zusätzlich freigeben, z. B. mit
[`docker/docker-compose.smtp-intake.yml`](../../docker/docker-compose.smtp-intake.yml):

```bash
docker compose -f docker-compose.postgres.yml -f docker-compose.smtp-intake.yml up -d
```

Im normalen Bridge-Netz von Docker sieht PaperRust als Absender oft die Gateway-Adresse von
Docker (z. B. `172.17.0.1`), nicht die des Druckers. IP-Listen wirken dann nicht wie gedacht.
Binden Sie den Port in diesem Fall nur an die LAN-Adresse des Hosts (z. B. `192.168.1.10:2525:2525`)
und lassen Sie die IP-Liste leer – das Token schützt trotzdem.

**Desktop-App:** Der Port wird nur geöffnet, wenn der Empfang an ist. Windows fragt beim ersten
Mal nach der Firewall-Freigabe – nur für **private Netzwerke** erlauben.

### 2. Gerät anlegen und Token erzeugen (Superuser)

1. **Administration → Scanner**, Abschnitt **Scan-to-Email (Geräte)**.
2. **Gerätename** (z. B. „HP Büro“), **Besitzer der Dokumente** und optional die **IP des
   Druckers** eintragen, **Token anlegen** klicken.
3. Das Token (`scan-…`) **sofort kopieren**. Es wird nur einmal gezeigt und nur als Prüfwert
   gespeichert.

Das Token kann **nur hochladen**: nichts lesen, nichts ändern, nichts löschen. Es gilt nicht für
die Weboberfläche oder die API. Sie können es jederzeit **widerrufen** – es wirkt sofort.

Das Gerät kann sich auf zwei Arten ausweisen:

- **Anmeldung (empfohlen):** Token als **SMTP-Passwort**, Benutzername beliebig.
- **Empfängeradresse:** an `scan-…@ihr-server` senden (für Geräte ohne SMTP-Anmeldung).

### 3. HP OfficeJet Pro 9010 einrichten (EWS)

> Noch nicht am echten Gerät geprüft. Die Begriffe stammen aus der HP-Weboberfläche dieser
> Druckerreihe und können je nach Firmware leicht abweichen.

1. Die IP-Adresse des Druckers im Browser öffnen (**Embedded Web Server**, EWS). Bei der Warnung
   zum Zertifikat fortfahren. Mit dem Administratorkennwort des Druckers anmelden (steht oft auf
   einem Aufkleber am Gerät).
2. Reiter **Scannen** → **Scan an E-Mail** (*Scan to Email*) → **Profile für ausgehende E-Mail**
   (*Outgoing Email Profiles*) → **Neu**.
3. Eintragen:

   | Feld im Drucker | Wert |
   |---|---|
   | E-Mail-Adresse (Absender) | z. B. `drucker@zuhause.local` (beliebig) |
   | Anzeigename | z. B. `HP Büro` |
   | SMTP-Server | IP oder Name des PaperRust-Rechners, z. B. `192.168.1.10` |
   | SMTP-Port | `2525` (bzw. Ihr `PAPERRUST_SMTP_INTAKE_LISTEN`-Port) |
   | SSL/TLS verwenden | **aus** – außer Sie haben ein Zertifikat eingerichtet (siehe unten) |
   | Server erfordert Authentifizierung | **an** |
   | Benutzername | beliebig, z. B. `hp` |
   | Kennwort | das Token `scan-…` |

4. **Test** bzw. **Speichern**. Danach unter **Scan an E-Mail** einen Empfänger anlegen, z. B.
   `paperrust@zuhause.local` (die Adresse ist egal, die Anmeldung zählt).
5. Am Drucker: **Scannen → E-Mail**, Profil und Empfänger wählen, Dateityp **PDF**, scannen.

Der Drucker schickt das PDF als Anhang. PDF, JPEG und TIFF werden angenommen (am Inhalt erkannt,
nicht am Dateinamen); andere Anhänge werden ignoriert. Hat eine Mail gar keinen passenden
Anhang, lehnt PaperRust sie ab – der Drucker zeigt dann einen Fehler.

Das Dokument gehört dem **Besitzer, den Sie beim Gerät gewählt haben**. Workflows mit der
Quelle **API-Upload** greifen (nicht „Eingangsordner“ – passen Sie solche Workflows an, wenn der
Drucker bisher in den Eingangsordner gescannt hat).

### 4. Alten Netzwerkordner-Zugang vom Drucker löschen

Wenn der Drucker bisher **in einen Netzwerkordner (SMB)** gescannt hat, steht dort das Passwort
Ihres NAS. Sobald Scan-to-Email läuft:

1. Im EWS: **Scannen** → **Scan an Netzwerkordner** (*Scan to Network Folder*) → das alte
   Profil wählen → **Löschen**.
2. Auf dem NAS das Konto, das der Drucker benutzt hat, **deaktivieren oder sein Passwort
   ändern** – am besten war es ein eigenes Konto nur für den Drucker.
3. Prüfen, ob der Drucker das Passwort noch an anderer Stelle hat (Adressbuch, Schnellzugriff).

## Sicherheit

- **Kein Relay:** Der SMTP-Empfang leitet nie eine Mail weiter und verschickt nichts. Ohne
  gültiges Token werden Empfänger mit „Relaying denied“ abgelehnt.
- **Nur hochladen:** Geräte-Token gelten nur hier. Sie sind nur als SHA-256-Prüfwert gespeichert,
  erscheinen nie im Log und lassen sich sofort widerrufen. Ist der Besitzer deaktiviert, wirkt das
  Token nicht mehr.
- **Grenzen:** Nachrichtengröße, Nachrichten je Minute und Gerät
  (`PAPERRUST_SMTP_INTAKE_RATE_PER_MINUTE`, Standard 30), nach 5 Fehlanmeldungen sperrt PaperRust
  die IP für 10 Minuten, höchstens 16 Verbindungen gleichzeitig.
- **Absender-IPs:** global (`PAPERRUST_SMTP_INTAKE_ALLOWED_IPS`) und je Gerät einschränkbar.
- **Verschlüsselung:** Ohne Zertifikat geht das Token **unverschlüsselt** durchs Netz. Dann nur im
  eigenen, vertrauenswürdigen LAN betreiben und den Port **nie** ins Internet weiterleiten.
  Mit Zertifikat:
  - `PAPERRUST_SMTP_INTAKE_TLS_CERT` und `PAPERRUST_SMTP_INTAKE_TLS_KEY` (PEM-Dateien) setzen →
    der Empfang bietet **STARTTLS** an. Mit `PAPERRUST_SMTP_INTAKE_REQUIRE_TLS=on` ist die
    Anmeldung nur noch verschlüsselt möglich.
  - Manche Drucker (oft bei „SSL“ und Port 465) wollen **direktes TLS** statt STARTTLS:
    `PAPERRUST_SMTP_INTAKE_IMPLICIT_TLS=on` und als Port z. B. `0.0.0.0:465` bzw. einen freien
    Port. Ob der HP OfficeJet Pro 9010 STARTTLS oder direktes TLS nutzt, ist noch nicht geprüft.
  - Ein selbst ausgestelltes Zertifikat reicht meist; viele Drucker prüfen es nicht.
- **Scannen-Knopf:** PaperRust verbindet sich nur mit Scannern, die ein Superuser eingetragen hat
  (Benutzer können keine Adressen angeben). Bei `https` (`_uscans`) wird das Zertifikat des
  Druckers nicht geprüft, weil Drucker fast immer selbst ausgestellte Zertifikate haben.

Technische Einzelheiten: [docs/contract/pl-scan.md](../contract/pl-scan.md).
