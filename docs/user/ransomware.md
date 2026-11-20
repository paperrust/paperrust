# Schutz vor Ransomware

[English](en/ransomware.md) · [Integritätsprüfung](integrity.md) · [Administration](administration.md)

Erpresser-Programme („Ransomware“) verschlüsseln Dateien und verlangen Geld für den Schlüssel. Der
typische Fall bei PaperRust: PaperRust läuft in Docker auf einer Synology-NAS, ein Windows-PC im
Heimnetz fängt sich Ransomware ein, und die verschlüsselt alles, was er über Netzwerkfreigaben
beschreiben kann – auch den Medienordner von PaperRust, wenn der als Freigabe erreichbar ist.

PaperRust hilft auf zwei Wegen:

1. **Wächter + Nur-Lese-Modus:** PaperRust bemerkt, wenn Dateien im Archiv ohne PaperRust verändert
   werden, und sperrt dann alle Schreibzugriffe, damit nicht noch mehr kaputtgeht.
2. **Automatische, verschlüsselte Sicherungen mit Versionen:** Damit lässt sich der Stand von vor dem
   Angriff zurückholen – auch dann, wenn der Angreifer an die Zugangsdaten kommt (S3 mit Object Lock).

> **Wichtig:** Kein Programm kann verhindern, dass ein befallener PC Dateien verschlüsselt, auf die er
> schreiben darf. Der beste Schutz ist, dass er gar nicht erst schreiben darf (siehe
> [Ratschläge für die NAS](#nas-advice)), und eine Sicherung, die er nicht erreicht.

## Was PaperRust tut – und was nicht

| PaperRust tut | PaperRust kann nicht |
|---|---|
| erkennt Änderungen an Originalen, Archivdateien und Vorschaubildern, die nicht von PaperRust stammen | verhindern, dass ein PC mit Schreibrecht Dateien verschlüsselt |
| erkennt typische Endungen (`.locked`, `.encrypted`, …), Erpresser-Notizen und verschlüsselt aussehende Inhalte | Angriffe auf andere Freigaben oder Programme der NAS erkennen |
| schaltet ab einer Schwelle in den **Nur-Lese-Modus** (kein Einlesen, keine Änderungen) | bereits verschlüsselte Dateien entschlüsseln |
| meldet den Alarm im Banner, per Mail an die Admins und im Protokoll, mit Liste der betroffenen Dateien | eine Sicherung ersetzen, die auf demselben Gerät liegt wie das Original |
| sichert Datenbank und Dateien verschlüsselt, dedupliziert, mit Versionen | Schutz bieten, wenn die NAS selbst (Admin-Konto, DSM) übernommen wird – dafür sind unveränderliche Snapshots und eine Kopie außer Haus da |
| sperrt Sicherungen auf S3 mit **Object Lock** gegen Löschen/Überschreiben | eine Sicherung auf einer beschreibbaren Freigabe vor dem befallenen PC schützen |

## Der Wächter

Der Wächter läuft im Worker von PaperRust und beobachtet `MEDIA_ROOT/documents/`:

- **Dateisystem-Ereignisse**, wo verfügbar (Linux/Docker: inotify; Windows; macOS). Auch Änderungen,
  die über eine SMB-Freigabe der NAS ankommen, sieht er, weil Samba auf demselben System schreibt.
- **Regelmäßige Durchsicht** (Standard alle 15 Minuten): vergleicht Größe und Änderungszeit aller
  Dateien mit dem letzten Stand – ohne Inhalte zu lesen.
- **Stichprobe:** bei jeder Durchsicht werden einige Dokumente (Standard 20) vollständig gehasht und
  mit der gespeicherten Prüfsumme verglichen, reihum über alle Dokumente. So fällt auch eine
  Verschlüsselung auf, die Größe und Änderungszeit beibehält.

PaperRust selbst verändert Originale nur über bekannte Vorgänge – neue Version, Umbenennen per
Speicherpfad, Neu verarbeiten, Papierkorb leeren – und danach passen Datei und Datenbank immer
zusammen. Der Wächter wartet deshalb nach jeder Änderung eine Minute und prüft dann:

| Fund | Bedeutung |
|---|---|
| *Ohne PaperRust geändert* | Datei gehört zu einem Dokument, aber ihre SHA-256-Prüfsumme passt nicht mehr |
| *Sieht verschlüsselt aus* | wie oben, und der Inhalt ist zufällig (hohe Entropie) statt z. B. `%PDF…` |
| *Erpresser-Notiz* | Dateien wie `README_DECRYPT.txt`, `HOW_TO_RESTORE_FILES…` |
| *Ransomware-Endung* | neue Datei mit `.locked`, `.encrypted`, `.crypt`, `.wncry`, … |
| *Datei fehlt* | ein Dokument verweist auf eine Datei, die es nicht mehr gibt |
| *Unerwartete Endung* | z. B. eine `.png` im Archivordner, in den PaperRust nur PDFs schreibt |
| *Falscher Dateikopf* | Endung `.pdf`, aber kein PDF-Kopf |

Kommen in **10 Minuten 20 oder mehr** solcher Funde zusammen (einstellbar), schaltet PaperRust in den
**Nur-Lese-Modus**:

- Hochladen, Einlesen (Ordner, Mail, Scanner), Bearbeiten und Löschen sind gesperrt. Schreibende
  API-Aufrufe bekommen `423 Locked` mit
  `{"detail": "PaperRust is in read-only mode …", "code": "read_only_mode"}`.
- Ansehen, Suchen und Herunterladen gehen weiter. Die Integritätsprüfung und die Sicherung laufen
  weiter (alte Sicherungen werden dann aber **nicht** aufgeräumt, damit gute Stände bleiben).
- Alle Nutzer sehen ein rotes Banner; Superuser kommen von dort zur Seite **Administration → Schutz**.
- Die Admins bekommen eine Mail (wenn `PAPERRUST_EMAIL_HOST` eingerichtet ist; Empfänger:
  alle aktiven Superuser mit E-Mail-Adresse oder `PAPERRUST_GUARD_MAIL`).
- Im Protokoll (`paperrust.log`, Logger `paperrust.guard`) stehen der Alarm und die betroffenen Dateien.

### Was tun bei einem Alarm?

1. **Den befallenen Rechner finden und vom Netz trennen** (WLAN aus, Kabel ziehen). Auf der NAS unter
   *Protokollcenter → Dateiübertragung* sieht man, welcher Rechner zuletzt geschrieben hat.
2. In PaperRust **Administration → Schutz** öffnen und die betroffenen Dateien ansehen. Auf der
   Kommandozeile zeigt `paperrust manage guard scan` alle Abweichungen im ganzen Archiv.
3. **Beschädigte Dateien zurückholen**, am besten aus einem Btrfs-Snapshot der NAS (schnell) oder aus
   der PaperRust-Sicherung (siehe [Wiederherstellen](#restore)).
4. Mit `paperrust manage guard scan` prüfen, dass nichts mehr fehlt, dann auf der Seite **Schutz**
   (oder `paperrust manage guard lift`) den Nur-Lese-Modus aufheben.

Ein Fehlalarm (z. B. jemand hat Dateien von Hand im Medienordner umbenannt) wird genauso aufgehoben.

### Einstellungen des Wächters

| Variable | Standard | Bedeutung |
|---|---|---|
| `PAPERRUST_GUARD` | `on` | Wächter ein/aus |
| `PAPERRUST_GUARD_THRESHOLD` | `20` | so viele verdächtige Dateien … |
| `PAPERRUST_GUARD_WINDOW_MINUTES` | `10` | … in so vielen Minuten schalten auf „nur lesen“ |
| `PAPERRUST_GUARD_SCAN_INTERVAL` | `900` | Sekunden zwischen zwei Durchsichten (`0` = keine) |
| `PAPERRUST_GUARD_SAMPLE_FILES` | `20` | je Durchsicht vollständig gehashte Dokumente |
| `PAPERRUST_GUARD_WATCH` | `on` | Dateisystem-Ereignisse nutzen |
| `PAPERRUST_GUARD_EXTRA_EXTENSIONS` | – | weitere verdächtige Endungen, kommagetrennt |
| `PAPERRUST_GUARD_MAIL` | alle Superuser | Empfänger der Alarm-Mail, kommagetrennt |

Alle Werte lassen sich auch in der Oberfläche unter *Einstellungen → System → Integrität* ändern; der
Wächter startet dann neu.

**Speicher und Last:** Die Durchsicht merkt sich je Datei etwa 20 Byte (bei 1 Million Dokumenten mit
je drei Dateien rund 60 MB). Gelesen werden nur Verzeichnisse und Dateiattribute; vollständig gehasht
werden nur geänderte Dateien und die Stichprobe.

## Verschlüsselte Sicherungen

PaperRust sichert jede Nacht (Standard 02:15) die **Datenbank** und **alle Dateien** unter
`MEDIA_ROOT` an ein zweites Ziel:

- **Ziel:** ein Ordner (z. B. eine eingebundene USB-Platte oder eine Freigabe auf einem *anderen*
  Gerät) oder ein **S3-kompatibler Speicher** (z. B. Synology C2 Object Storage, Backblaze B2, Wasabi,
  MinIO auf einem zweiten Gerät).
- **Dedupliziert:** Jede Datei wird unter ihrer SHA-256-Prüfsumme abgelegt. Unveränderte Dateien werden
  nie erneut übertragen – auch bei einer Million Dokumenten wandern nachts nur die neuen.
- **Verschlüsselt** auf dem Server, bevor etwas das Gerät verlässt, mit [age](https://age-encryption.org)
  (X25519, ChaCha20-Poly1305). Zum Sichern braucht PaperRust nur den *öffentlichen* Schlüssel; zum
  Wiederherstellen braucht man die **Passphrase**, die nur Sie kennen.
- **Versionen:** Jeder Lauf ist ein eigener Stand. Aufbewahrt werden standardmäßig 7 tägliche,
  4 wöchentliche und 12 monatliche Stände.
- **S3 Object Lock:** Mit `PAPERRUST_BACKUP_S3_OBJECT_LOCK=compliance` (oder `governance`) bekommt jedes
  Objekt eine Sperrfrist (Standard 30 Tage). Während dieser Frist kann es **niemand** löschen oder
  überschreiben – auch nicht mit gestohlenen Zugangsdaten. Objekte, die spätere Stände weiter
  brauchen, verlängert PaperRust automatisch.

### Einrichten

1. Ziel festlegen, z. B. in der `docker-compose.env`:

   ```sh
   # Ordner (z. B. USB-Platte, die in den Container eingebunden ist)
   PAPERRUST_BACKUP_TARGET=/backup
   # oder S3
   PAPERRUST_BACKUP_TARGET=s3://mein-bucket/paperrust
   PAPERRUST_BACKUP_S3_ENDPOINT=https://eu-002.s3.synologyc2.net
   PAPERRUST_BACKUP_S3_REGION=eu-002
   PAPERRUST_BACKUP_S3_ACCESS_KEY=…
   PAPERRUST_BACKUP_S3_SECRET_KEY=…
   PAPERRUST_BACKUP_S3_OBJECT_LOCK=compliance
   PAPERRUST_BACKUP_S3_LOCK_DAYS=30
   ```

   Für Object Lock muss das Bucket **mit Object Lock angelegt** sein (das lässt sich bei den meisten
   Anbietern nur beim Anlegen einschalten). Der Zugangsschlüssel braucht nur Lesen, Schreiben,
   Auflisten und `PutObjectRetention` – nicht das Recht, Sperrfristen zu umgehen.

2. Einmal einrichten – dabei entsteht der Schlüssel, geschützt durch Ihre Passphrase (mindestens
   12 Zeichen):

   ```sh
   docker compose exec -e PAPERRUST_BACKUP_PASSPHRASE='…' webserver paperrust manage backup init --show-key
   ```

   **Passphrase und den angezeigten geheimen Schlüssel (`AGE-SECRET-KEY-1…`) sicher und getrennt
   aufbewahren**, z. B. ausgedruckt im Tresor. Ohne beides ist die Sicherung wertlos. Die Passphrase
   wird nirgends gespeichert.

3. Fertig: Die Sicherung läuft nach Zeitplan (`PAPERRUST_BACKUP_CRON`, Standard `15 2 * * *`).
   Von Hand: **Administration → Schutz → Jetzt sichern** oder `paperrust manage backup run`.

Auf der Seite **Schutz** stehen Ziel, Zeitplan, letzter Erfolg, Umfang und alle Läufe mit Fehlern.

<a id="restore"></a>

### Wiederherstellen

```sh
# Stände anzeigen
paperrust manage backup list
# Vollständigkeit prüfen (alle Objekte vorhanden)
PAPERRUST_BACKUP_PASSPHRASE='…' paperrust manage backup check --snapshot latest
# Dateien in einen neuen Ordner und die Datenbank in eine neue, leere Datenbank zurückholen
PAPERRUST_BACKUP_PASSPHRASE='…' paperrust manage backup restore --snapshot 20261114T021500Z \
    --media /neu/media --database
```

- `--database` legt zuerst das Schema an (`manage migrate`) und ersetzt dann alle Tabellen durch den
  gesicherten Stand. Dafür `PAPERRUST_DB*` auf eine **neue, leere** Datenbank zeigen lassen.
- `--media DIR` schreibt alle Dateien des Stands nach `DIR`; jede Datei wird nach dem Entschlüsseln
  gegen ihre Prüfsumme geprüft. Vorhandene, abweichende Dateien werden nur mit `--overwrite` ersetzt.
- Statt der Passphrase geht auch die Schlüsseldatei: `--key-file schluessel.txt`.
- Für einen Stand vor dem Angriff einfach dessen Namen aus `backup list` angeben.

### Einstellungen der Sicherung

| Variable | Standard | Bedeutung |
|---|---|---|
| `PAPERRUST_BACKUP_TARGET` | – | Ordner oder `s3://bucket/präfix` |
| `PAPERRUST_BACKUP_CRON` | `15 2 * * *` | Zeitplan, `disable` = aus |
| `PAPERRUST_BACKUP_DATABASE` | `on` | Datenbank mitsichern |
| `PAPERRUST_BACKUP_KEEP_DAILY` / `_WEEKLY` / `_MONTHLY` | `7` / `4` / `12` | Aufbewahrung |
| `PAPERRUST_BACKUP_S3_ENDPOINT` | – | `https://host[:port]` (Pfad-Adressierung) |
| `PAPERRUST_BACKUP_S3_REGION` | `us-east-1` | Region |
| `PAPERRUST_BACKUP_S3_ACCESS_KEY` / `_SECRET_KEY` | – | Zugang (der Geheimschlüssel ist in der Oberfläche nur schreibbar) |
| `PAPERRUST_BACKUP_S3_OBJECT_LOCK` | `off` | `governance` oder `compliance` |
| `PAPERRUST_BACKUP_S3_LOCK_DAYS` | `30` | Sperrfrist je Objekt |
| `PAPERRUST_BACKUP_PASSPHRASE` | – | nur für `init`/`check`/`restore`, nur aus der Umgebung, nie gespeichert |

**Grenzen:** Die Namen der Objekte sind die SHA-256-Prüfsummen der Klartexte. Wer Zugriff aufs Ziel
hat, kann damit prüfen, ob eine *ihm bekannte* Datei im Archiv liegt – Inhalte und Dateinamen sieht er
nicht. Ein lokaler Ordner als Ziel ist **nicht** vor Ransomware geschützt, wenn der befallene PC ihn
beschreiben kann; dafür S3 mit Object Lock oder unveränderliche Snapshots nutzen.

<a id="nas-advice"></a>

## Ratschläge für die NAS

Diese Schritte schützen mehr als jedes Programm:

1. **Den Medienordner nicht als SMB-Freigabe anbieten.** PaperRust braucht keine Freigabe; Dokumente
   kommen über die Oberfläche, die Mail oder den Scanner hinein. Liegt `media` in einem freigegebenen
   Ordner, für diesen Ordner allen Nutzern außer dem Docker-Konto das Schreibrecht entziehen (DSM:
   *Systemsteuerung → Gemeinsamer Ordner → Bearbeiten → Berechtigungen*).
2. **Unveränderliche Btrfs-Snapshots einschalten** (DSM 7.2: *Snapshot Replication → Snapshots →
   Einstellungen → Unveränderliche Snapshots*, z. B. stündlich, 7 Tage unveränderlich). Ein
   unveränderlicher Snapshot kann bis zum Ablauf nicht gelöscht werden – auch nicht von einem
   Administrator. Damit ist der Stand von vor einer Stunde fast immer da.
3. **Eine Sicherung außer Haus** (3-2-1-Regel: drei Kopien, zwei Medien, eine außer Haus), z. B. die
   PaperRust-Sicherung auf S3 mit Object Lock oder Hyper Backup auf ein Gerät bei Verwandten.
4. **Den Scan-Eingang statt einer beschreibbaren Consume-Freigabe nutzen.** Scanner schicken per
   E-Mail an den SMTP-Empfang von PaperRust oder werden über den Scannen-Knopf angesprochen (siehe
   [Scanner](scanner.md)). Eine beschreibbare `consume`-Freigabe ist ein Einfallstor; wer sie
   behält, sollte sie wenigstens nur für das Scanner-Konto beschreibbar machen.
5. Das DSM-Admin-Konto deaktivieren, 2-Faktor-Anmeldung einschalten, DSM und Pakete aktuell halten.

## Ausprobieren

Den Alarm gefahrlos testen – **nur mit einer Kopie**:

```sh
cp -r media /tmp/media-test && touch /tmp/media-test/.paperrust-test-media
# PaperRust mit PAPERRUST_MEDIA_ROOT=/tmp/media-test und einer Kopie der Datenbank starten, dann:
node crates/pl-guard/tests/simulate-ransomware.mjs /tmp/media-test --count 30
```

Das Skript „verschlüsselt“ 30 Originale zu `*.locked` und legt eine Erpresser-Notiz ab. Nach etwa
einer Minute schaltet PaperRust in den Nur-Lese-Modus. Ohne die Markierungsdatei
`.paperrust-test-media` verweigert das Skript die Arbeit.
