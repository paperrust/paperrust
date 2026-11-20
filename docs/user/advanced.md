# Fortgeschrittene Themen

[English](en/advanced.md) · [Übersicht](README.md)

Diese Seite erklärt Funktionen, die Ihnen Handarbeit abnehmen: automatische Zuordnung, eigene
Skripte beim Einlesen, Barcodes, doppelseitiges Scannen mit einfachem Einzug und die Anmeldung über
einen fremden Anmeldedienst (SSO).

<a id="matching"></a><a id="matching-tags-correspondents-document-types-and-storage-paths"></a><a id="zuordnung"></a>

## Automatische Zuordnung

Tags, Korrespondenten, Dokumenttypen und Speicherpfade können sich selbst an neue Dokumente
hängen. Dafür hat jedes dieser Objekte zwei Felder:

- **Zuordnung:** ein Suchtext, z. B. `stadtwerke`.
- **Algorithmus:** wie der Suchtext mit dem Dokumenttext verglichen wird.

Beim Einlesen geht PaperRust alle Objekte durch und prüft, ob ihr Suchtext zum erkannten Text des
neuen Dokuments passt. Passt er, wird das Objekt gesetzt. Ein Beispiel: Ein Tag `Strom` mit dem
Suchtext `stadtwerke musterstadt` und dem Algorithmus _Genauer Text_ landet auf jedem neuen Dokument, in
dem genau diese Wörter in dieser Reihenfolge stehen.

Die Algorithmen im Überblick:

| Algorithmus | Passt, wenn … |
| --- | --- |
| **Keine** | nie – das Objekt wird nicht automatisch gesetzt. |
| **Ein beliebiges Wort** | mindestens eines der Wörter im Text vorkommt. `Sparkasse Volksbank` passt auf Dokumente mit „Sparkasse“ _oder_ „Volksbank“. |
| **Alle Wörter** | jedes der Wörter im Text vorkommt, egal in welcher Reihenfolge. |
| **Genauer Text** | der ganze Suchtext genau so im Text steht. |
| **Regulärer Ausdruck** | das Muster irgendwo im Text einen Treffer findet. |
| **Ähnliches Wort** | ein Textstück dem Suchtext sehr ähnlich ist (mindestens 90 von 100 Punkten). Hilft bei kleinen Texterkennungsfehlern. Satzzeichen werden vorher entfernt. |
| **Automatisch** | PaperRust es aus Ihren bisherigen Dokumenten gelernt hat. Ein Suchtext ist nicht nötig, siehe [unten](#automatic-matching). |

Bei _Ein beliebiges Wort_ und _Alle Wörter_ halten doppelte Anführungszeichen mehrere Wörter zusammen:
`"Deutsche Bahn" DB` passt auf „Deutsche Bahn“ oder „DB“, aber nicht auf „Deutsche Post Bahnhof“.

Mit dem Schalter **Groß-/Kleinschreibung ignorieren** spielt die Schreibweise keine Rolle. Ein
leerer Suchtext passt nie (außer bei _Automatisch_).

So probieren Sie es aus: Suchtext und Algorithmus beim Objekt eintragen, speichern, dann ein neues
Dokument einlesen. Hat alles gepasst, trägt das Dokument die Zuordnung schon nach dem Einlesen.
Schon vorhandene Dokumente ändern sich dadurch nicht.

> [!NOTE]
> Reguläre Ausdrücke schreiben Sie in der gewohnten Python-Schreibweise. PaperRust übersetzt sie
> intern. Sehr seltene Sonderformen (z. B. Lookbehinds mit variabler Länge oder rekursive Muster)
> kennt PaperRust nicht; solche Muster gelten als ungültig und treffen nie. Statt eines Zeitlimits
> gibt es eine feste Obergrenze für den Suchaufwand, damit ein ungeschicktes Muster das Einlesen
> nicht aufhält.

<a id="automatic-matching"></a>

### Automatisch lernen lassen

Steht ein Objekt auf _Automatisch_, schaut PaperRust sich an, welchen Dokumenten Sie es bisher
gegeben haben, und lernt daraus. Dahinter steckt ein kleines neuronales Netz.

Beispiel: Alle Abrechnungen Ihrer Krankenkasse tragen den Korrespondenten „Krankenkasse“, und der
steht auf _Automatisch_. Nach einigen Dokumenten erkennt PaperRust neue Abrechnungen selbst.

Damit das klappt, sollten Sie Folgendes wissen:

- **Es lernt nicht sofort.** Das Modell wird regelmäßig neu trainiert, standardmäßig stündlich
  (Einstellung [`PAPERRUST_TRAIN_TASK_CRON`](configuration.md#PAPERRUST_TRAIN_TASK_CRON)).
  Änderungen wirken also erst nach dem nächsten Training.
- **Der Posteingang zählt nicht.** Dokumente mit einem Posteingangs-Tag werden beim Lernen
  übersprungen. So lernt PaperRust nur von Dokumenten, die Sie schon fertig eingeordnet haben.
- **Der Inhalt muss zum Objekt passen.** Eine Abrechnung enthält meist Name und Versichertennummer –
  daraus lässt sich lernen. Ein Tag wie „Später erledigen“ hängt nicht am Text und lässt sich nicht
  lernen.
- **Es braucht genug Beispiele.** Ein Korrespondent mit nur einem einzigen Dokument wird kaum
  erkannt. Je mehr Dokumente, desto besser.
- **Es braucht auch Gegenbeispiele.** Gibt es nur zwei Korrespondenten und stehen beide auf
  _Automatisch_, bekommt jedes neue Dokument einen der beiden – auch wenn keiner passt. Mit
  wachsendem Bestand löst sich das meist von selbst.

Das gelernte Modell liegt in einer eigenen Datei (Standard `DATA_DIR/paperrust_classifier.model`,
änderbar mit `PAPERRUST_CLASSIFIER_MODEL_FILE`). Nach einem Umstieg von paperless-ngx wird das
Modell beim ersten Training neu erstellt; die alte ngx-Modelldatei wird nicht verwendet. Einzelne
Vorschläge können deshalb etwas anders ausfallen als vorher.

<a id="consume-hooks"></a><a id="hooking-into-the-consumption-process"></a>

## Eigene Skripte beim Einlesen

Sie können PaperRust bei jedem Dokument ein eigenes Programm starten lassen – einmal **vor** dem
Einlesen und/oder einmal **danach**. Das Skript kann in jeder Sprache geschrieben sein.

So richten Sie es ein:

1. Skript schreiben und an einem Ort ablegen, den PaperRust lesen kann.
2. Das Skript ausführbar machen (unter Linux: `chmod +x` und eine Startzeile wie
   `#!/bin/sh`). PaperRust startet die Datei direkt, ohne zusätzliche Shell und ohne Argumente.
3. Den Pfad eintragen: [`PAPERRUST_PRE_CONSUME_SCRIPT`](configuration.md#PAPERRUST_PRE_CONSUME_SCRIPT)
   für vorher, [`PAPERRUST_POST_CONSUME_SCRIPT`](configuration.md#PAPERRUST_POST_CONSUME_SCRIPT)
   für danach.

Wichtig zu wissen:

- **PaperRust wartet auf das Skript.** Ein langsames Skript bremst jedes Einlesen. Soll etwas
  Langes im Hintergrund laufen, starten Sie es im Skript als eigenen Prozess und beenden das
  Skript gleich danach.
- **Ausgaben landen im Protokoll.** Jede Zeile der normalen Ausgabe wird als Info, jede Zeile der
  Fehlerausgabe als Warnung ins Server-Protokoll geschrieben.
- **Der Rückgabewert zählt.** Endet das Skript mit einem anderen Code als 0, gilt die Aufgabe als
  fehlgeschlagen (Meldung „Error while executing … script“). Gibt es die Skriptdatei nicht, schlägt
  die Aufgabe ebenfalls fehl.
- Fehlende Werte stehen in den Variablen meist als Text `None`, beim Eigentümer als leerer Text.

<a id="pre-consume-script"></a><a id="pre-consumption-script"></a>

### Skript vor dem Einlesen

Läuft, sobald PaperRust eine neue Datei angenommen und eine Arbeitskopie angelegt hat – vor
Texterkennung, Barcodes und allem anderen. Das Skript bekommt diese Umgebungsvariablen:

| Variable | Inhalt |
| --- | --- |
| `DOCUMENT_SOURCE_PATH` | Pfad der angelieferten Originaldatei |
| `DOCUMENT_WORKING_PATH` | Pfad der Arbeitskopie, mit der PaperRust weiterarbeitet |
| `TASK_ID` | Kennung der Einlese-Aufgabe (kann leer sein) |

Wollen Sie die Datei verändern, dann **nur die Arbeitskopie** (`DOCUMENT_WORKING_PATH`). Ändern
Sie das Original im Eingangsordner, hält PaperRust es womöglich für eine neue Datei und liest es ein
zweites Mal ein. Ändern Sie die Datei außerdem immer gleich (gleiche Eingabe → gleiches Ergebnis),
sonst erkennt PaperRust Doppelte nicht mehr zuverlässig.

Beispiel: Faxe kommen mit einem Deckblatt. Ein Skript entfernt bei Dateien aus dem Unterordner
`fax` die erste Seite (mit dem Werkzeug `qpdf`, das Sie selbst installieren):

`/opt/paperrust/fax-deckblatt-weg.sh`

```sh
#!/bin/sh
# Nur Dateien aus dem Unterordner "fax" anfassen
case "$DOCUMENT_SOURCE_PATH" in
  */fax/*)
    echo "Entferne Deckblatt von $DOCUMENT_SOURCE_PATH"
    qpdf --replace-input "$DOCUMENT_WORKING_PATH" --pages . 2-z --
    ;;
esac
```

Eintrag in der Konfiguration:

```bash
PAPERRUST_PRE_CONSUME_SCRIPT=/opt/paperrust/fax-deckblatt-weg.sh
```

<a id="post-consume-script"></a><a id="post-consumption-script"></a>

### Skript nach dem Einlesen

Läuft, wenn das Dokument fertig gespeichert ist. Es kann das Einlesen nicht mehr rückgängig machen
und sollte die gespeicherten Dateien nicht verändern (dafür gibt es die Bearbeiten-Funktionen der
Oberfläche). Das Skript bekommt diese Umgebungsvariablen:

| Variable | Inhalt |
| --- | --- |
| `DOCUMENT_ID` | Nummer des Dokuments |
| `DOCUMENT_FILE_NAME` | Dateiname für Downloads, ohne Ordner (Datum, Korrespondent, Titel, Endung) |
| `DOCUMENT_TYPE` | Dokumenttyp |
| `DOCUMENT_CREATED` | Ausstellungsdatum (`JJJJ-MM-TT`) |
| `DOCUMENT_MODIFIED` | Zeitpunkt der letzten Änderung |
| `DOCUMENT_ADDED` | Zeitpunkt des Hinzufügens |
| `DOCUMENT_SOURCE_PATH` | Pfad der gespeicherten Originaldatei |
| `DOCUMENT_ARCHIVE_PATH` | Pfad der Archivfassung (PDF/A), falls es eine gibt |
| `DOCUMENT_THUMBNAIL_PATH` | Pfad des Vorschaubilds |
| `DOCUMENT_DOWNLOAD_URL` | Pfad zum Herunterladen, z. B. `/api/documents/42/download/` |
| `DOCUMENT_THUMBNAIL_URL` | Pfad des Vorschaubilds, z. B. `/api/documents/42/thumb/` |
| `DOCUMENT_OWNER` | Benutzername des Eigentümers |
| `DOCUMENT_CORRESPONDENT` | Korrespondent |
| `DOCUMENT_TAGS` | Tags, durch Komma getrennt |
| `DOCUMENT_ORIGINAL_FILENAME` | ursprünglicher Dateiname |
| `TASK_ID` | Kennung der Einlese-Aufgabe (kann leer sein) |

Die beiden URL-Werte sind Pfade ohne Rechnernamen; setzen Sie Ihre eigene Adresse davor.

Beispiel: Jedes neue Dokument wird in einer einfachen Liste festgehalten, die Sie z. B. in einer
Tabellenkalkulation öffnen können:

`/opt/paperrust/neue-dokumente.sh`

```sh
#!/bin/sh
LISTE=/srv/paperrust/neue-dokumente.csv
[ -f "$LISTE" ] || echo "Zeit;ID;Korrespondent;Tags;Datei" > "$LISTE"
echo "$(date '+%Y-%m-%d %H:%M');$DOCUMENT_ID;$DOCUMENT_CORRESPONDENT;$DOCUMENT_TAGS;$DOCUMENT_FILE_NAME" >> "$LISTE"
```

```bash
PAPERRUST_POST_CONSUME_SCRIPT=/opt/paperrust/neue-dokumente.sh
```

> [!NOTE]
> `DOCUMENT_FILE_NAME` wird in PaperRust einfacher gebildet als in paperless-ngx: Datum,
> Korrespondent und Titel mit Endung, unzulässige Zeichen ersetzt, ohne zusätzliches Kürzen.

<a id="pdf-recovery"></a><a id="automatic-recovery-of-invalid-pdfs"></a>

## Beschädigte PDFs reparieren

Manche PDFs sind fehlerhaft aufgebaut, etwa von alten Scannern oder abgebrochenen Downloads.
PaperRust versucht in zwei Fällen, so eine Datei vor dem Einlesen zu reparieren:

- **Falsch erkannter Dateityp:** Endet eine Datei auf `.pdf`, wird aber nicht als PDF erkannt,
  baut PaperRust ihre inneren Verweise neu auf und prüft den Dateityp danach noch einmal.
- **PDF lässt sich nicht lesen:** Dann versucht PaperRust eine nachsichtige Reparatur, siehe
  [PDF-Werkzeuge: Reparieren beim Einlesen](pdf-tools.md#reparieren).

In beiden Fällen arbeitet PaperRust mit der reparierten Kopie weiter. **Gespeichert wird die
unveränderte Originaldatei.** Klappt die Reparatur nicht, läuft das Einlesen normal weiter und
meldet gegebenenfalls einen Fehler. Ein zusätzliches Programm wie `qpdf` ist dafür nicht nötig.

<a id="barcodes"></a>

## Barcodes

Barcodes auf Ihren Seiten können PaperRust Anweisungen geben: hier ein neues Dokument beginnen,
diese Archivnummer vergeben, dieses Tag setzen. Wie Sie das einschalten, steht in der
[Konfiguration](configuration.md#barcodes). Die Funktionen lassen sich einzeln einschalten,
wirken aber zusammen wie unten beschrieben.

> **Tipp:** Passende ASN-Etiketten und Trennblätter druckt PaperRust selbst – siehe
> [ASN-Etiketten](asn-labels.md).

Gut zu wissen:

- Barcodes werden in PDFs gesucht, in TIFF-Dateien nur, wenn
  [eingeschaltet](configuration.md#PAPERRUST_CONSUMER_BARCODE_TIFF_SUPPORT).
- Es zählt nur der **Inhalt** eines Codes, nicht seine Art. Erkannt werden die üblichen
  Strich- und 2D-Codes, unter anderem QR-Code, Data Matrix, Aztec, PDF417, Code 128, Code 39,
  Code 93, Codabar, EAN-8, EAN-13, UPC-A, UPC-E und Interleaved 2 of 5.
- Für die Barcode-Suche werden die Seiten mit der PDF-Bibliothek PDFium in Bilder umgewandelt.
  Fehlt PDFium, ist die Barcode-Erkennung aus.
- Nach einem Umstieg: Aufgeteilte Dokumente enthalten dieselben Seiten wie bei paperless-ngx, die
  Dateien sind aber nicht Byte für Byte gleich und haben daher andere Prüfsummen.

<a id="document-splitting"></a>

### Dokumente trennen

Sie können einen ganzen Stapel auf einmal scannen und PaperRust trennt ihn in einzelne Dokumente.
Dazu legen Sie zwischen die Dokumente ein **Trennblatt** mit einem Barcode, dessen Inhalt der
eingestellten Trenn-Zeichenfolge entspricht (Standard `PATCHT`).

Standardverhalten:

- Das Trennblatt wird **weggeworfen**.
- Das nächste Dokument beginnt mit der Seite **nach** dem Trennblatt.

Ist [`PAPERRUST_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES`](configuration.md#PAPERRUST_CONSUMER_BARCODE_RETAIN_SPLIT_PAGES)
eingeschaltet, bleibt die Seite mit dem Trenn-Barcode erhalten und wird die **erste Seite** des
neuen Dokuments.

<a id="archive-serial-number-assignment"></a>

### Archivnummer (ASN) vergeben

Beginnt der Inhalt eines Barcodes mit dem ASN-Präfix (Standard `ASN`), nimmt PaperRust die Zahl
dahinter als Archivnummer des Dokuments. Aus `ASN00123` wird die Archivnummer 123. So finden Sie
über die Nummer auf dem Aufkleber schnell zum Papier im Ordner.

Ist zusätzlich das Trennen eingeschaltet, beginnt an jedem ASN-Barcode ein neues Dokument. Anders
als beim Trennblatt **bleibt diese Seite** im Dokument – sie ist ja meist die erste Seite des
Schreibens, auf die Sie den Aufkleber geklebt haben.

<a id="tag-assignment"></a>

### Tags per Barcode

PaperRust kann aus Barcodes Tags machen. Standardmäßig wird aus einem Barcode `TAG:Rechnung` das
Tag „Rechnung“; fehlt das Tag, legt PaperRust es an. Welche Inhalte zu welchen Tags werden, legen
Sie mit [`PAPERRUST_CONSUMER_TAG_BARCODE_MAPPING`](configuration.md#PAPERRUST_CONSUMER_TAG_BARCODE_MAPPING)
fest; eingeschaltet wird die Funktion mit
[`PAPERRUST_CONSUMER_ENABLE_TAG_BARCODE`](configuration.md#PAPERRUST_CONSUMER_ENABLE_TAG_BARCODE).

<a id="splitting-on-tag-barcodes"></a>

#### An Tag-Barcodes trennen

Normalerweise setzen Tag-Barcodes nur Tags. Mit
[`PAPERRUST_CONSUMER_TAG_BARCODE_SPLIT`](configuration.md#PAPERRUST_CONSUMER_TAG_BARCODE_SPLIT) `=true`
beginnt an jeder Seite mit Tag-Barcode zusätzlich ein neues Dokument:

- Die Seite mit dem Tag-Barcode **bleibt** im neuen Dokument.
- Jedes Teil-Dokument bekommt **nur die Tags seiner eigenen Seiten**.
- Ein Stapel kann beliebig viele solcher Trennstellen haben.
- ASN-Barcodes funktionieren gleichzeitig; jedes Teil bekommt seine eigene Nummer und seine Tags.

So können Sie einen gemischten Stapel in einem Durchgang trennen und gleich einordnen.

**Beispiel:** 6 Seiten, `TAG:Rechnung` auf Seite 3, `TAG:Quittung` auf Seite 5. Ergebnis: drei
Dokumente – Seiten 1–2 ohne Tag, Seiten 3–4 mit „Rechnung“, Seiten 5–6 mit „Quittung“.

<a id="barcode-contents"></a>

### Barcode-Inhalte speichern

Auf Wunsch merkt sich PaperRust den Inhalt **aller** gefundenen Barcodes am Dokument, etwa
Zahlungscodes oder QR-Codes auf Rechnungen. Einschalten mit
[`PAPERRUST_CONSUMER_STORE_BARCODE_VALUES`](configuration.md#PAPERRUST_CONSUMER_STORE_BARCODE_VALUES).

- In der Oberfläche stehen sie im Reiter **Metadaten** mit Seite, Art und Inhalt; der Inhalt lässt
  sich kopieren.
- Über die API kommen sie im Feld `barcodes` von `/api/documents/{id}/metadata/`.
- Sie sind [durchsuchbar](usage.md#searching-barcodes), z. B. `barcodes:DE89370400440532013000`.
- Durchsucht werden nur die ersten
  [`PAPERRUST_CONSUMER_BARCODE_MAX_PAGES`](configuration.md#PAPERRUST_CONSUMER_BARCODE_MAX_PAGES)
  Seiten. Für schon vorhandene Dokumente: neu verarbeiten lassen.
- Jede Version eines Dokuments hat ihre eigenen Barcodes; angezeigt und gesucht werden die der
  neuesten Version.

<a id="collate"></a><a id="automatic-collation-of-double-sided-documents"></a>

## Doppelseitig scannen mit einfachem Einzug

> [!NOTE]
> Kann Ihr Scanner selbst beide Seiten scannen, brauchen Sie das hier nicht.

Diese Funktion ist zunächst aus. Einschalten: siehe [Konfiguration](configuration.md#collate).

<a id="summary"></a>

### Worum es geht

Viele Scanner mit Einzug scannen nur eine Seite des Blatts. Für doppelseitige Briefe scannen Sie den
Stapel dann zweimal: einmal die Vorderseiten, einmal die Rückseiten. PaperRust fügt die beiden Scans
zu **einem** Dokument zusammen und bringt die Seiten in die richtige Reihenfolge.

<a id="usage-example"></a>

### So geht's

Beispiel: ein Brief mit 3 Blättern, also 6 Seiten.

1. Stapel normal einlegen, Seite 1 zuerst. Der Scanner erfasst die Seiten **1, 3, 5**.
2. Die Datei in den Doppelseiten-Unterordner des Eingangsordners legen (Standardname
   `double-sided`). **Den Ordner müssen Sie selbst anlegen.** PaperRust nimmt den Scan an und legt
   ihn zur Seite.
3. Stapel als Ganzes umdrehen, ohne die Blätter neu zu sortieren, und noch einmal scannen. Jetzt
   kommen die Seiten **6, 4, 2**.
4. Auch diese Datei in den Unterordner legen. PaperRust dreht die Reihenfolge des zweiten Scans
   um, mischt beide Scans und liest das Ergebnis (Seiten 1 bis 6) ganz normal ein.

> [!TIP]
> Leere Rückseiten **am Ende** dürfen Sie beim zweiten Scan weglassen. Ist Seite 6 leer, reichen
> die Seiten 2 und 4. Leere Seiten **mittendrin** müssen aber mitgescannt werden, sonst verrutscht
> die Reihenfolge.

<a id="things-that-could-go-wrong"></a>

### Wenn etwas schiefgeht

- **Zweiter Scan hat mehr Seiten als der erste:** Dann gehören die Scans nicht zusammen (oft hat
  der Einzug im ersten Durchgang Blätter verschluckt). PaperRust verwirft beide und meldet einen
  Fehler. Fangen Sie neu an: erst die Vorderseiten, dann die Rückseiten.
- **Reihenfolge und Abstand:** Die beiden Scans müssen nacheinander und einzeln ankommen, während
  PaperRust läuft. PaperRust schaut standardmäßig jede Sekunde in den Eingangsordner. Haben Sie
  [`PAPERRUST_CONSUMER_POLLING_INTERVAL`](configuration.md#PAPERRUST_CONSUMER_POLLING_INTERVAL)
  höher gesetzt, sollte der Wert kleiner sein als die Zeit zwischen Ihren beiden Scans.
- **Zweiter Scan vergessen:** Ein einzelner Vorderseiten-Scan wartet höchstens **30 Minuten**.
  Danach gilt der nächste Scan wieder als neuer erster Scan, und der alte wird verworfen. So werden
  nicht versehentlich Seiten von verschiedenen Tagen gemischt.

<a id="interaction-with-subdirs-as-tags"></a>

### Zusammen mit „Unterordner als Tags“

Nutzen Sie [Unterordner als Tags](configuration.md#consume_config), funktioniert das auch mit dem
Doppelseiten-Ordner. Er darf an beliebiger Stelle im Pfad stehen: `double-sided/Steuer/2025` und
`Steuer/2025/double-sided` ergeben beide ein Dokument mit den Tags „Steuer“ und „2025“ – ohne ein Tag
„double-sided“.

<a id="interaction-with-document-splitting"></a>

### Zusammen mit Trennblättern

[Trennblätter](#document-splitting) funktionieren auch hier. Ist das Trennblatt nur einseitig
bedruckt, landet seine Rückseite aber als (leere) erste Seite im nächsten Dokument. Abhilfe: ein
Trennblatt, das den Trenn-Barcode auf **beiden** Seiten trägt. Dann fallen beide Seiten weg.

<a id="sso-and-third-party-authentication-with-paperrust"></a><a id="sso"></a>

## Anmeldung über andere Dienste (SSO)

Normalerweise melden sich Benutzer mit Name und Passwort direkt bei PaperRust an. Haben Sie schon
einen zentralen Anmeldedienst (z. B. Authentik, Keycloak, Authelia), kann PaperRust ihn nutzen.
Es gibt zwei Wege.

<a id="remote-user-authentication"></a>

### Über einen vorgeschalteten Proxy (Remote-User)

Ein Reverse-Proxy vor PaperRust übernimmt die Anmeldung und schickt den Benutzernamen in einem
HTTP-Header mit. PaperRust vertraut diesem Header und meldet den Benutzer an; gibt es ihn noch
nicht, wird er angelegt.

- Einschalten mit [`PAPERRUST_ENABLE_HTTP_REMOTE_USER`](configuration.md#PAPERRUST_ENABLE_HTTP_REMOTE_USER)
  (für die Oberfläche) bzw. [`PAPERRUST_ENABLE_HTTP_REMOTE_USER_API`](configuration.md#PAPERRUST_ENABLE_HTTP_REMOTE_USER_API)
  (auch für die API).
- Name des Headers: [`PAPERRUST_HTTP_REMOTE_USER_HEADER_NAME`](configuration.md#PAPERRUST_HTTP_REMOTE_USER_HEADER_NAME)
  (Standard: `Remote-User`).
- Wohin es nach dem Abmelden geht: [`PAPERRUST_LOGOUT_REDIRECT_URL`](configuration.md#PAPERRUST_LOGOUT_REDIRECT_URL).

> [!WARNING]
> PaperRust prüft nicht, woher der Header kommt. Schalten Sie das nur ein, wenn PaperRust
> ausschließlich über den Proxy erreichbar ist und der Proxy einen von außen mitgeschickten Header
> immer entfernt. Sonst kann sich jeder als beliebiger Benutzer ausgeben.

<a id="openid-connect-and-social-authentication"></a>

### Über OpenID Connect (OIDC)

PaperRust kann sich direkt mit einem Anmeldedienst verbinden, der **OpenID Connect** spricht. Auf
der Anmeldeseite erscheint dann ein Knopf für diesen Dienst.

Einrichtung in zwei Schritten:

1. **Beim Anmeldedienst** eine neue Anwendung (Client) anlegen. Als Rücksprung-Adresse eintragen:
   `https://<ihre-paperrust-adresse>/accounts/oidc/<provider_id>/login/callback/`
   (`<provider_id>` wählen Sie selbst, siehe unten). Client-ID und Geheimnis notieren.
2. **Bei PaperRust** zwei Einstellungen setzen, hier am Beispiel Authentik:

```conf
PAPERRUST_APPS="allauth.socialaccount.providers.openid_connect"
PAPERRUST_SOCIALACCOUNT_PROVIDERS='{"openid_connect": {"APPS": [{
  "provider_id": "authentik",
  "name": "Firmen-Login",
  "client_id": "<CLIENT_ID>",
  "secret": "<CLIENT_SECRET>",
  "settings": {"server_url": "https://auth.example.org/application/o/paperrust"}
}]}}'
```

`name` ist die Beschriftung des Knopfs. `server_url` ist die Adresse des Dienstes (ohne Schrägstrich am Ende); PaperRust
hängt `/.well-known/openid-configuration` an. Sie können auch gleich die volle Adresse mit
`/.well-known/` eintragen. Mehrere Dienste sind möglich: einfach weitere
Einträge in die Liste `APPS`. Alle Einstellungen dazu stehen in der
[Konfiguration](configuration.md#PAPERRUST_SOCIALACCOUNT_PROVIDERS), z. B. ob sich neue Benutzer
selbst registrieren dürfen ([`PAPERRUST_SOCIALACCOUNT_ALLOW_SIGNUPS`](configuration.md#PAPERRUST_SOCIALACCOUNT_ALLOW_SIGNUPS),
[`PAPERRUST_SOCIAL_AUTO_SIGNUP`](configuration.md#PAPERRUST_SOCIAL_AUTO_SIGNUP)) und ob Gruppen vom
Anmeldedienst übernommen werden ([`PAPERRUST_SOCIAL_ACCOUNT_SYNC_GROUPS`](configuration.md#PAPERRUST_SOCIAL_ACCOUNT_SYNC_GROUPS)).

**Bestehendes Konto verbinden:** Melden Sie sich wie gewohnt an, öffnen Sie im Benutzermenü
**Mein Profil** und verbinden Sie dort das Konto beim Anmeldedienst. Danach geht die Anmeldung auch
über den Knopf.

<a id="social-login"></a>

**GitHub, GitLab, Google, Microsoft, Nextcloud, Discord, Gitea:** Diese Anbieter gehen auch direkt über deren eigene
Schnittstelle (wie bei paperless-ngx mit den allauth-Paketen). Sie tragen die passende App in
`PAPERRUST_APPS` ein (mehrere durch Kommas getrennt) und den Anbieter mit Client-ID und Geheimnis in
`PAPERRUST_SOCIALACCOUNT_PROVIDERS`, zum Beispiel:

```conf
PAPERRUST_APPS="allauth.socialaccount.providers.github,allauth.socialaccount.providers.google"
PAPERRUST_SOCIALACCOUNT_PROVIDERS='{"github": {"APPS": [{"client_id": "<ID>", "secret": "<SECRET>"}]},
  "google": {"APPS": [{"client_id": "<ID>", "secret": "<SECRET>"}]}}'
```

Rücksprung-Adresse beim Anbieter: `https://<ihre-paperrust-adresse>/accounts/<anbieter>/login/callback/`
(z. B. `…/accounts/github/login/callback/`). Andere allauth-Anbieter (etwa Facebook) kennt PaperRust
nicht; sie werden mit einem Hinweis im Log übergangen, die Anmeldung mit Passwort und den übrigen
Anbietern geht weiter.

<a id="email-verification"></a>

**E-Mail-Adresse bestätigen:** Mit
[`PAPERRUST_ACCOUNT_EMAIL_VERIFICATION`](configuration.md#PAPERRUST_ACCOUNT_EMAIL_VERIFICATION)`=mandatory`
muss jedes Konto seine Mail-Adresse bestätigen, bevor die Anmeldung klappt. PaperRust schickt dazu
eine Mail mit Link. Das braucht einen eingerichteten [E-Mail-Versand](configuration.md#email-sending)
und eine Mail-Adresse bei jedem Konto; ein Konto ohne Adresse kann sich dann nicht anmelden (Hinweis
im Log). Mit `optional` wird nur bei der Registrierung eine Bestätigung verschickt, die Anmeldung
geht aber auch ohne.

Was nicht geht:

- Die Signatur des ID-Tokens wird nicht zusätzlich geprüft. Das Token kommt direkt und verschlüsselt
  vom Anmeldedienst; so hält es auch paperless-ngx.

<a id="disabling-regular-login"></a>

### Nur noch über den Anmeldedienst anmelden

Läuft die Anmeldung über SSO, können Sie die normale Anmeldung mit Name und Passwort abschalten
([`PAPERRUST_DISABLE_REGULAR_LOGIN`](configuration.md#PAPERRUST_DISABLE_REGULAR_LOGIN)). Mit
[`PAPERRUST_REDIRECT_LOGIN_TO_SSO`](configuration.md#PAPERRUST_REDIRECT_LOGIN_TO_SSO) landen Benutzer
statt auf der Anmeldeseite gleich beim Anmeldedienst.

<a id="gpg-decryptor"></a><a id="decryption-of-encrypted-emails-before-consumption"></a>

## Verschlüsselte E-Mails entschlüsseln

Mit [`PAPERLESS_ENABLE_GPG_DECRYPTOR`](configuration.md#PAPERRUST_ENABLE_GPG_DECRYPTOR) entschlüsselt
PaperRust GPG-verschlüsselte Mails vor dem Einlesen, mit den Schlüsseln aus
[`PAPERLESS_EMAIL_GNUPG_HOME`](configuration.md#PAPERRUST_EMAIL_GNUPG_HOME) (sonst dem Standardordner
von GnuPG). Dafür muss das Programm `gpg` installiert sein – im PaperRust-Docker-Image ist es nicht
enthalten. Fehlt es oder klappt die Entschlüsselung nicht, wird die Mail nicht verarbeitet und ein
Fehler steht im Log.

<a id="requirements"></a><a id="setup"></a><a id="troubleshooting"></a>
