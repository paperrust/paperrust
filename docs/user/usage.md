# Nutzung

[English](en/usage.md) · [Übersicht](README.md)

Diese Seite erklärt den Alltag mit PaperRust: was die Begriffe bedeuten, wo Sie in der Oberfläche
was finden, wie Dokumente hereinkommen, wie Sie sie wiederfinden und wie Sie Papier und Archiv
sinnvoll zusammen führen. Einen Scanner brauchen Sie nicht zwingend; wenn Sie einen anschließen
möchten, hilft [Scanner verbinden](scanner.md).

<a id="terms-and-definitions"></a><a id="begriffe"></a>

## Begriffe

Im Hintergrund arbeiten zwei Dinge zusammen: das **Einlesen** (es nimmt neue Dateien an, erkennt den
Text und ordnet zu) und die **Weboberfläche** (dort arbeiten Sie mit den Dokumenten). Beides steckt
im selben Programm.

Was ein Dokument an Angaben haben kann:

| Begriff | Bedeutung |
| --- | --- |
| **Dokument** | eine eingelesene Datei samt allem, was PaperRust darüber weiß |
| **Korrespondent** | wer das Dokument geschickt hat oder wer es bekommt – Person, Firma, Amt |
| **Tag** | ein Etikett. Ein Dokument kann beliebig viele Tags tragen. Deshalb gibt es keine Ordner: Ein Dokument steckt nie nur in einer Schublade. |
| **Dokumenttyp** | welche Art Dokument es ist, z. B. Rechnung, Vertrag, Kontoauszug |
| **Speicherpfad** | wo und unter welchem Namen die Datei auf der Platte liegt, siehe [Speicherpfade](storage-paths.md#storage-paths) |
| **Hinzugefügt** | wann PaperRust das Dokument eingelesen hat. Dieses Datum bleibt fest. |
| **Erstellt** | wann das Dokument ausgestellt wurde, z. B. Rechnungsdatum oder Datum eines Briefs |
| **ASN** (Archivnummer) | die Nummer, unter der das Papier-Original in Ihrem Ordner liegt, siehe [Empfohlener Arbeitsablauf](#usage-recommended-workflow) |
| **Inhalt** | der erkannte Text. Er dient der Suche und der automatischen Zuordnung. |
| **Benutzerdefinierte Felder** | eigene Zusatzangaben wie Betrag oder Vertragsnummer, siehe [Benutzerdefinierte Felder](custom-fields.md) |

<a id="the-web-ui"></a>

## Die Weboberfläche

Sie öffnen PaperRust im Browser unter `/rs/app/` (die Startadresse `/` leitet dorthin weiter).
Ohne eigene Wahl öffnet sich das **PaperRust-Design** (Adresse `/rs/app/m/`):

- **Links die Navigation:** **Startseite**, **Posteingang** (neue, noch nicht bearbeitete Dokumente),
  **Alle Dokumente**, Ihre gespeicherten Ansichten, **Automatik** (Workflows, E-Mail-Abruf,
  KI-Vorschläge – siehe [Automatik](automatik.md)), **Verwalten** (Tags, Korrespondenten,
  Dokumenttypen, Speicherpfade, benutzerdefinierte Felder, Benutzer und Gruppen, Konfiguration,
  Aufgaben, Protokolle, Papierkorb, Scannen, Schutz, Freigabelinks usw.) und **Einstellungen**.
  Was Sie nicht dürfen, wird nicht angezeigt.
- **Oben:** das Suchfeld (auch mit **Strg + K**, siehe [Globale Suche](#global-search)), der Knopf
  **Hochladen**, die Benachrichtigungen und das Benutzermenü (Mein Profil, Hell/Dunkel,
  Tastenkürzel, Dokumentation, **Rundgang starten**, Zur klassischen Oberfläche wechseln, Abmelden).
- **Arbeitsplatz:** Liste, Vorschau und Angaben stehen nebeneinander. Nur die einzelnen Bereiche
  rollen, Kopfleiste und Navigation bleiben stehen. Auf dem Handy wird daraus eine Leiste unten.

Neu bei PaperRust? Der **Rundgang** zeigt in zwölf Schritten die wichtigsten Stellen. Sie starten
ihn im Benutzermenü (**Rundgang starten**) oder unter *Einstellungen*.

Die **klassische Oberfläche** sieht aus wie paperless-ngx. Sie wählen sie unter *Einstellungen →
Darstellung → Design der Oberfläche* (gilt für Ihr Konto) oder einmalig über „Zur klassischen
Oberfläche wechseln“ im Benutzermenü. Beide arbeiten mit denselben Daten. Wo sich die Bedienung
unterscheidet, nennt diese Seite beide Wege.

<a id="sprache"></a><a id="language"></a>

### Sprache der Oberfläche

Die Oberfläche spricht **10 Sprachen**: Deutsch, Englisch, Französisch, Italienisch, Spanisch,
Niederländisch, Portugiesisch (Brasilien), Polnisch, Tschechisch und Schwedisch. Alle Texte hat
PaperRust selbst geschrieben.

Gewählt wird unter *Einstellungen → Allgemein → Sprache*. Ohne Wahl nimmt PaperRust die Sprache des
Browsers. Ist das keine der zehn (etwa Japanisch oder Dänisch), erscheinen die Texte auf Englisch,
Datum und Zahlen aber im Format Ihrer Sprache. Schweizerdeutsch (`de-CH`) bekommt die deutschen Texte.

Für Umsteiger: paperless-ngx hatte über 30 Sprachen. Wer dort eine andere als die zehn eingestellt
hatte, sieht nach dem Umstieg Englisch, bis er neu wählt.

<a id="dashboard"></a>

### Startseite

Die **Startseite** (in der englischen Oberfläche „Dashboard“) erscheint nach der Anmeldung. Anfangs
ist sie fast leer. Sie füllen sie mit [gespeicherten Ansichten](#saved-views), die Sie als Kachel
auf der Startseite einblenden – etwa „Im Posteingang“ oder „Noch zu erledigen“. Dateien können Sie
hier hochladen oder einfach irgendwo in der Oberfläche fallen lassen.

<a id="document-list"></a>

### Dokumentliste

Unter **Dokumente** sehen Sie alle Dokumente, auf die Sie Zugriff haben – als Tabelle oder als
Karten. Über der Liste filtern Sie nach Tags, Korrespondenten, Dokumenttypen, Speicherpfaden,
Datum, benutzerdefinierten Feldern und mehr. Das Textfeld der Filterleiste sucht in Titel und
Inhalt, nach ASN oder mit der [erweiterten Suche](#document-searches).

Haken Sie Dokumente an, um viele auf einmal zu ändern: Tags, Korrespondent, Dokumenttyp,
Speicherpfad, benutzerdefinierte Felder oder „Sichtbar für“ (Rechte) setzen, löschen, Workflows
ausführen und mehr. Im PaperRust-Design sammelt das Menü **PDF** die Dateiaktionen (Drehen,
Zusammenführen, Als Versionen zusammenführen, Neu verarbeiten, In PDF umwandeln); **Senden** bietet
einen Freigabelink für mehrere Dokumente oder eine E-Mail, **Herunterladen** Originale oder
Archivdateien.

<a id="document-detail"></a>

### Dokumentdetails

Ein Klick auf ein Dokument öffnet die Detailansicht mit Vorschau und Angaben. In Reitern finden Sie
den erkannten Inhalt, Metadaten der Datei, Notizen, Rechte und den Verlauf. Änderungen speichert das
PaperRust-Design von selbst.

**Neu anlegen direkt im Feld:** Fehlt ein Tag, Korrespondent, Dokumenttyp oder Speicherpfad, tippen
Sie den Namen ins Auswahlfeld und wählen **„… neu anlegen“**. PaperRust legt den Eintrag an und weist
ihn sofort zu.

**Das Aktionsmenü** des Dokuments (im PaperRust-Design oben rechts) enthält: PDF-Editor, Schwärzen,
Verkleinern, In PDF umwandeln, Neu verarbeiten, Drucken, Original herunterladen, per E-Mail senden,
Freigabelinks und Workflow ausführen. Was davon erscheint, hängt vom Dateityp und Ihren Rechten ab.
Die PDF-Werkzeuge erklärt [PDF-Werkzeuge](pdf-tools.md).

<a id="document-file-versions"></a>

### Dateiversionen eines Dokuments

Ein Dokument kann mehrere **Dateiversionen** haben – wie ein Stapel, bei dem die neueste Datei oben
liegt. Das entsteht zum Beispiel, wenn Sie Seiten drehen oder löschen, eine bessere Kopie
nachreichen oder zwei Dokumente zusammenlegen.

- Jede Version hat ihre eigene Datei und ihren eigenen erkannten Text.
- Titel, Tags, Korrespondent, Dokumenttyp, Speicherpfad und benutzerdefinierte Felder gehören dem
  Dokument als Ganzem (der „Wurzel“), nicht einer einzelnen Version.
- Suche und angezeigter Inhalt nehmen immer die neueste Version.
- In den Dokumentdetails wählen Sie eine ältere Version aus. Vorschau, Inhalt, Datei-Metadaten und
  Herunterladen zeigen dann diese Version.
- Auf der Platte bekommen Versionsdateien den normalen Dateinamen mit Endung `_v1`, `_v2` usw.
- Löschen Sie eine Version (nicht die Wurzel), bleiben alle Angaben erhalten; die nächstjüngere
  Version rückt nach.

**Zusammenlegen:** In der Dokumentliste zwei oder mehr Dokumente anhaken und **Als Versionen
zusammenführen** wählen. Sie bestimmen, welches Dokument die Wurzel ist – dessen Angaben und Rechte
bleiben, die anderen werden zu Versionen. Die Wurzel darf schon Versionen haben, die anderen
Dokumente nicht. Einzeln geht es über das Menü **Versionen** eines Dokuments: **Vorhanden** wählt
ein anderes Dokument und hängt es als Version an.

Dabei gilt:

- Die angehängten Dokumente verlieren ihre ASN. Hat die Wurzel keine, bekommt sie die erste davon;
  sonst werden die Nummern frei, und das wird im Verlauf vermerkt.
- In der Oberfläche lässt sich das Zusammenlegen nicht rückgängig machen.
- Wird die Wurzel gelöscht, gehen alle ihre Versionen mit in den Papierkorb.

<a id="management-lists"></a>

### Verwaltungslisten

Unter **Verwalten** pflegen Sie Tags, Korrespondenten, Dokumenttypen, Speicherpfade und
benutzerdefinierte Felder (Bereich **Attribute**), außerdem gespeicherte Ansichten, Freigabelinks
und E-Mail-Konten. In jeder Liste können Sie Einträge anlegen, ändern, löschen und ihre Rechte
festlegen. Benutzer und Gruppen, Konfiguration (Anwendung und Server), Aufgaben, Protokolle und Papierkorb
liegen im selben Bereich, sichtbar je nach Ihren Rechten. Workflows und Mail-Regeln stehen im
PaperRust-Design im Bereich [Automatik](automatik.md), in der klassischen Oberfläche unter
*Verwalten*.

<a id="nested-tags"></a>

### Verschachtelte Tags

Tags lassen sich ineinander hängen, zum Beispiel `Versicherung` → `Auto` → `Haftpflicht`. Ein Tag
hat dann einen übergeordneten Tag; bis zu 5 Ebenen sind möglich. Das hat Folgen:

- Bekommt ein Dokument `Haftpflicht`, erhält es automatisch auch `Auto` und `Versicherung`.
- Nehmen Sie einem Dokument `Auto` weg, verschwindet auch `Haftpflicht`.
- Hängen Sie einen bestehenden Tag nachträglich unter einen anderen, bekommen alle Dokumente mit
  diesem Tag den neuen übergeordneten Tag dazu.

<a id="adding-documents-to-paperrust"></a>

## Dokumente zu PaperRust hinzufügen

Egal auf welchem Weg eine Datei hereinkommt, PaperRust macht damit immer dasselbe:

1. **Text erkennen (OCR)**, falls die Datei noch keinen Text hat. Bei digital erzeugten PDFs ist er
   meist schon da.
2. **Archivfassung erzeugen:** ein PDF/A mit auswählbarem Text. Ob das passiert, steuern Sie mit
   [`PAPERRUST_ARCHIVE_FILE_GENERATION`](configuration.md#PAPERRUST_ARCHIVE_FILE_GENERATION):
   `auto` (Standard) lässt PDFs mit vorhandenem Text in Ruhe, `never` erzeugt nie eine
   Archivfassung. Mehr dazu unter [OCR-Einstellungen](configuration.md#ocr).
3. **Zuordnen:** Tags, Korrespondent, Dokumenttyp usw. werden automatisch gesetzt, dann wird das
   Dokument gespeichert.

Das Original bewahrt PaperRust immer unverändert auf; die Archivfassung liegt daneben. Ändern
können Sie den Dateistapel nur bewusst, etwa mit den [PDF-Aktionen](#pdf-actions) – die legen dann
eine neue Version an.

Wege nach PaperRust: Eingangsordner, Hochladen in der Oberfläche, [Scan-Eingang](scanner.md),
[E-Mail](mail.md) und die [REST-API](#rest-api).

<a id="consumption-directory"></a><a id="eingangsordner"></a><a id="the-consumption-directory"></a>

### Der Eingangsordner

Der Eingangsordner ist ein Briefkasten: Was Sie hineinlegen, holt PaperRust ab, liest es ein und
legt es in seiner eigenen Ablage (dem Medienordner) ab. **Danach ist die Datei aus dem
Eingangsordner verschwunden** – sie liegt jetzt sicher in PaperRust, an dem Ort, den Ihre
Einstellungen und Speicherpfade vorgeben.

PaperRust schaut den Ordner regelmäßig nach. Wie oft, legt
[`PAPERRUST_CONSUMER_POLLING_INTERVAL`](configuration.md#PAPERRUST_CONSUMER_POLLING_INTERVAL) fest;
der Standard `0` bedeutet einmal pro Sekunde. Das erledigt der Hintergrunddienst von PaperRust selbst
(`paperrust serve --worker`), Sie müssen nichts Zusätzliches starten.

Wie die Dateien in den Ordner kommen, entscheiden Sie. Läuft PaperRust auf Ihrem eigenen Rechner,
ziehen Sie sie einfach hinein. Auf einem Server bietet sich eine Netzwerkfreigabe an (z. B. mit
[Samba](https://www.samba.org/)) oder ein FTP-Server wie [ProFTPD](http://www.proftpd.org/). Bequemer
und ohne gespeichertes Server-Passwort im Scanner geht es mit dem [Scan-Eingang](scanner.md).

<a id="web-ui-upload"></a>

### Hochladen über die Weboberfläche

Klicken Sie oben auf **Hochladen** oder ziehen Sie Dateien an irgendeine Stelle der Oberfläche. Den
Fortschritt sehen Sie oben rechts in der Verarbeitungsanzeige.

<a id="usage-mobile_upload"></a><a id="mobile-upload"></a>

### Hochladen vom Mobilgerät

Die Oberfläche funktioniert auch im Browser auf dem Handy; dort können Sie Fotos und Dateien
hochladen. Viele Apps, die für paperless-ngx geschrieben wurden, nutzen dieselbe REST-API und
arbeiten deshalb in der Regel auch mit PaperRust – geprüft ist das aber nicht für jede App. Für
Dokumentscanner im Netzwerk siehe [Scanner verbinden](scanner.md).

<a id="rest-api"></a>

### REST-API

Programme können Dateien direkt an die API schicken, siehe
[Dokumente hochladen](api.md#file-uploads).

<a id="duplicate-documents"></a>

### Doppelte Dokumente

Kommt eine Datei herein, die es schon gibt (gleiche Prüfsumme von Original oder Archivfassung),
**liest PaperRust sie trotzdem ein** und schreibt eine Warnung ins Protokoll. So geht nichts
verloren, falls Sie die Kopie absichtlich wollten.

Doppelte finden Sie so:

- In den Dokumentdetails erscheint der Reiter **Duplikate** mit allen gleichen Dokumenten, die Sie
  sehen dürfen (auch solche im Papierkorb).
- In der Dokumentliste wählen Sie im Auswahlmenü des Textfilters **Duplikate**.
- Über die API: `has_duplicates=true`.

Sollen Doppelte gar nicht erst hinein, setzen Sie
[`PAPERRUST_CONSUMER_DELETE_DUPLICATES`](configuration.md#PAPERRUST_CONSUMER_DELETE_DUPLICATES) auf
`true`. Dann wird die neue Datei gelöscht, und die Aufgabe endet mit dem Fehler „Document already
exists“ und einem Link auf das vorhandene Dokument. (So verhielt sich paperless-ngx vor Version 3.)

<a id="document-suggestions"></a>

## Vorschläge für Dokumente

PaperRust lernt aus Ihren bisherigen Dokumenten, welche Tags, Korrespondenten, Dokumenttypen und
Speicherpfade zu welchem Inhalt passen. Das ist ein kleines, lokales Lernverfahren, keine
Sprach-KI. In den Dokumentdetails erscheinen die Vorschläge neben den Feldern; Sie übernehmen sie mit
einem Klick oder ignorieren sie.

Für Dokumente mit Posteingangs-Tag holt PaperRust die Vorschläge beim Öffnen automatisch. Wollen Sie
das nicht, schalten Sie unter *Einstellungen* „Vorschläge für Dokumente im Posteingang automatisch
abrufen“ aus; dann gibt es Vorschläge nur über den Knopf **Vorschlagen**. Ist ein KI-Dienst
eingerichtet, kann er die Vorschläge zusätzlich liefern, siehe [KI-Funktionen](ai.md).

<a id="sharing"></a><a id="freigaben"></a><a id="sharing-documents-from-paperrust"></a>

## Dokumente teilen

Innerhalb von PaperRust teilen Sie Dokumente über [Rechte](permissions.md#object-permissions): Wer
Lese- oder Schreibrecht hat, sieht das Dokument. Nach außen gibt es drei Wege:
[Freigabelinks](#share-links), [Versand per E-Mail](#email-sharing) und Workflows, die automatisch
eine [E-Mail](workflows.md#workflow-action-email) oder einen
[Webhook](workflows.md#workflow-action-webhook) schicken.

<a id="share-links"></a>

### Freigabelinks

Ein Freigabelink ist eine Adresse, über die jemand eine Datei herunterladen kann, **ohne sich
anzumelden**. Er sieht so aus: `{paperrust-url}/share/{zufälliger-code}`.

- **Ein Dokument:** in den Dokumentdetails **Senden** → Freigabelink. Auf Wunsch mit
  Ablaufdatum.
- **Mehrere Dokumente:** in der Dokumentliste auswählen und „Freigabelink für mehrere Dokumente
  erstellen“. PaperRust packt im Hintergrund ein ZIP-Archiv und gibt Ihnen einen einzigen Link. Den
  Stand (ausstehend, in Arbeit, bereit, fehlgeschlagen) sehen Sie unter „Freigabelinks für mehrere
  Dokumente verwalten“; dort können Sie fehlgeschlagene Pakete neu erstellen oder Links löschen.
- Alle Links verwalten Sie außerdem unter *Verwalten → Freigabelinks*.
- Ist ein Link abgelaufen oder gelöscht, landet der Besucher auf der Anmeldeseite.

> [!TIP]
> Steht vor PaperRust ein Reverse Proxy mit eigener Anmeldung, müssen Sie den Pfad `/share/`
> womöglich davon ausnehmen, sonst kommen Fremde nicht an die Datei. Überlegen Sie gut, was Sie
> öffentlich machen.

<a id="share-links-desktop"></a>

#### Freigabelinks in der Desktop-App

Die Desktop-App ist nur auf Ihrem eigenen Computer erreichbar (`http://127.0.0.1:…`). Ein
Freigabelink funktioniert deshalb **nur auf diesem Computer**; der Dialog sagt das auch. Um ein
Dokument trotzdem weiterzugeben:

- **herunterladen** und die Datei weitergeben, oder
- **per E-Mail senden** (**Senden** → **E-Mail**, siehe [Versand per E-Mail](#email-sharing); dafür
  muss ein Mailserver eingerichtet sein), oder
- die **Freigabe im Heimnetz** einschalten: In `desktop.toml` (Ordner `%LOCALAPPDATA%\PaperRust` bzw.
  `~/.local/share/paperrust`) unter `[server]` `lan_sharing = true` eintragen und die App neu
  starten. Dann ist sie auch unter `http://<Adresse-im-Netz>:<Port>` erreichbar, und Freigabelinks
  funktionieren auf Geräten im selben Netz, z. B. auf dem Handy im WLAN. Zum Arbeiten muss man sich
  weiterhin anmelden; ohne Anmeldung gehen nur die Freigabelinks. Windows fragt beim ersten Start
  nach – erlauben Sie nur **private Netzwerke**. Stimmt die erkannte Adresse nicht, setzen Sie
  `lan_address = "192.168.1.20"` (oder einen Rechnernamen). Nicht in öffentlichen WLANs benutzen.

Links, die aus dem Internet erreichbar sind, gibt es mit einem PaperRust-Server unter eigener
Adresse (auch für Fernzugriff und Abgleich der App), siehe `docs/REMOTE-APP.md` im Projekt.

<a id="email-sharing"></a>

### Versand per E-Mail

Ist ein [Mailserver für den Versand](configuration.md#email-sending) eingerichtet, bietet **Senden**
in den Dokumentdetails die Option **E-Mail**. Das Dokument geht dann als Anhang an die angegebene
Adresse. Automatisch, etwa für jede neue Rechnung, geht das mit einer
[Workflow-Aktion](workflows.md#workflow-action-email).

<a id="pdf-actions"></a>

## PDF-Aktionen

Für PDFs (nur für diese) bringt PaperRust einfache Bearbeitungen mit:

| Aktion | Wo |
| --- | --- |
| Seiten drehen | Dokumentliste (Auswahl → **Drehen**) und PDF-Editor |
| Seiten löschen oder umsortieren | PDF-Editor |
| Dokument aufteilen | PDF-Editor („Hier teilen“) |
| Dokumente zusammenführen | Dokumentliste, mehrere Dokumente auswählen |

Den **PDF-Editor** öffnen Sie in den Dokumentdetails (PaperRust-Design: Aktionsmenü →
**PDF-Editor**; mehrere Dokumente: Menü **PDF** über der Liste). Er zeigt alle Seiten als Vorschaubilder; dort
drehen, verschieben und löschen Sie Seiten oder setzen Trennstellen.

Drehen und Seiten löschen legen eine **neue Dateiversion** an; die bisherige Datei bleibt als ältere
[Version](#document-file-versions) erhalten. Die neue Datei ist aber eine andere Datei: Eine digitale
Signatur gilt für sie nicht mehr.

Schwärzen, Verkleinern und Reparieren von PDFs beschreibt die Seite [PDF-Werkzeuge](pdf-tools.md).

<a id="document-history"></a>

## Dokumentverlauf

PaperRust schreibt jede Änderung an einem Dokument mit: wer wann welches Feld geändert hat. Sie sehen
das im Reiter **Verlauf** der Dokumentdetails. Änderungen durch Workflows oder andere automatische
Abläufe stehen dort unter „System“. Abschalten lässt sich das mit
[`PAPERRUST_AUDIT_LOG_ENABLED`](configuration.md#PAPERRUST_AUDIT_LOG_ENABLED) (Standard: an).

<a id="papierkorb"></a><a id="document-trash"></a>

## Papierkorb

Gelöschte Dokumente landen erst im **Papierkorb** (unter *Verwalten → Papierkorb*). Dort bleiben
Datei und alle Angaben vollständig erhalten, und Sie können das Dokument jederzeit zurückholen.

Endgültig weg ist ein Dokument erst, wenn Sie es im Papierkorb löschen oder die Frist abläuft. Die
Frist stellen Sie mit [`PAPERRUST_EMPTY_TRASH_DELAY`](configuration.md#PAPERRUST_EMPTY_TRASH_DELAY)
ein; Standard sind 30 Tage.

Wer eine letzte Sicherung möchte, setzt
[`PAPERRUST_EMPTY_TRASH_DIR`](configuration.md#PAPERRUST_EMPTY_TRASH_DIR). Beim endgültigen Löschen
wird dann die Originaldatei in diesen Ordner verschoben. Archivfassung und alle Angaben in der
Datenbank sind trotzdem weg.

<a id="basic-searching"></a><a id="best-practices"></a>

## Bewährte Vorgehensweisen

PaperRust schreibt Ihnen keine Ordnung vor. Ein paar Ideen, die sich bewährt haben:

- **Dokumenttyp = was es ist.** Rechnung, Vertrag, Lohnabrechnung. Wer früher einen Ordner
  „Quittungen“ hatte, legt dafür einen Dokumenttyp an. Nicht jedes Dokument braucht einen Typ –
  für ein Einzelstück lohnt sich keiner.
- **Tag = Thema oder Zustand.** Ein Dokument kann viele Tags haben, ein Blatt Papier aber nur in
  einem Ordner liegen. Eine Autoversicherungs-Rechnung kann also gleichzeitig `Auto`,
  `Versicherung` und `Steuer 2026` tragen.
- **Tags für Projekte auf Zeit.** Sie planen einen Umzug und brauchen Mietvertrag, Gehaltsnachweise
  und Ausweiskopie? Ein Tag `Umzug` sammelt alles, ohne die übrige Ordnung anzufassen. Danach den Tag
  löschen oder behalten.
- **Korrespondenten**, wo sie beim Finden helfen – die Bank, die Krankenkasse, der Vermieter.
- **Gespeicherte Ansichten** für alles, was Sie oft brauchen. Sie sind wie beschriftete Ordner auf
  dem Schreibtisch, die sich von selbst füllen.

Tags, die sich in der Praxis bewähren:

- `Posteingang` – neu und noch nicht angesehen
- `Erledigen` – hier muss noch etwas passieren (bezahlen, antworten, kündigen)
- `Unvollständig` – Angaben fehlen, die Sie später nachtragen
- `Per Mail` – kam über den Mail-Abruf herein
- ein Tag je Konto, Fahrzeug oder Familienmitglied

<a id="saved-views"></a><a id="gespeicherte-ansichten"></a>

## Gespeicherte Ansichten

Haben Sie in der Dokumentliste Filter, Sortierung und Spalten so eingestellt, wie Sie es brauchen,
sichern Sie das über *Ansichten → Speichern als…*. Eine gespeicherte Ansicht

- erscheint auf Wunsch in der Seitenleiste, auf der Startseite oder an beiden Stellen,
- hat eine eigene Seitengröße und Anzeige (Tabelle, kleine oder große Karten),
- lässt sich über Rechte mit anderen Benutzern teilen.

Ändern und sortieren unter *Verwalten → Gespeicherte Ansichten*. Über die API: `/api/saved_views/`.

<a id="searching"></a><a id="suche"></a><a id="basic-usage_searching"></a>

## Suche

<a id="global-search"></a>

### Globale Suche

Das Suchfeld oben sucht überall gleichzeitig: Dokumente, gespeicherte Ansichten, Tags,
Korrespondenten, Dokumenttypen, Speicherpfade, Benutzer, Gruppen, Mail-Konten und -Regeln, Workflows
und benutzerdefinierte Felder. Nötig sind mindestens 3 Zeichen. Pro Art zeigt sie bis zu 3 Treffer,
und nur Dinge, die Sie sehen dürfen.

Dokumente sucht sie im Volltext (Titel und Inhalt). Schalten Sie unter *Einstellungen* „Ergebnisse
der erweiterten Suche weglassen“ ein, sucht sie nur noch im Titel. Mit dem Knopf neben dem Suchfeld
öffnen Sie die vollständige Trefferliste in der Dokumentliste.

Im PaperRust-Design ist das Suchfeld zugleich eine **Befehlsleiste** (öffnen mit **Strg + K**):

- **Befehle:** zu einer Seite springen, hochladen, neuer Workflow, Hell/Dunkel, Schnellfilter usw.
- **Einstellungen finden:** Tippen Sie etwa „Datum“ oder „OCR“. Unten erscheinen passende
  Einstellungen der Oberfläche und – für Administratoren – alle Server-Einstellungen (gesucht wird in
  Name, Variablenname und Stichwörtern). Ein Klick öffnet den richtigen Reiter und lässt die
  Einstellung kurz aufleuchten.
- **Frage stellen:** „Frage stellen: …“ schickt den Text an den Assistenten (Fragen ans Archiv). Ohne
  KI zeigt er die passendsten Stellen aus Ihren Dokumenten, mit KI eine Antwort mit Quellen.

<a id="document-searches"></a>

### Dokumentsuche

Die Dokumentsuche hilft bei Fragen wie „Wo ist die Rechnung für den Staubsauger vom Frühjahr?“.
PaperRust vergleicht Ihre Eingabe mit Titel, Inhalt, Korrespondent, Dokumenttyp und Tags und
sortiert die Treffer: Was am besten passt, steht oben.

Ohne besondere Zeichen gilt: Ein Dokument muss **alle** eingegebenen Wörter enthalten. Außerdem:

- Die **Reihenfolge** der Wörter ist egal: `strom abrechnung` = `abrechnung strom`.
- **Akzente** spielen keine Rolle: `creme` findet auch `crème`.
- **Satz- und Trennzeichen** werden beim Einlesen entfernt. `4711` findet deshalb auch `KD-4711/03`.

Notizen, benutzerdefinierte Felder und Barcodes durchsucht die einfache Suche nicht; dafür gibt es
eigene Feldnamen (siehe unten). Wer genauer suchen will, nutzt die folgenden Regeln.

<a id="combining-terms"></a>

#### Begriffe kombinieren

```
miete AND (2025 OR 2026)
versicherung NOT kfz
"bitte überweisen Sie"
```

- `AND` (und), `OR` (oder) und `NOT` (nicht) schreiben Sie **groß**. Klammern fassen zusammen.
- Stehen Wörter ohne Verknüpfung nebeneinander, gilt `AND`.
- In Anführungszeichen sucht PaperRust genau diese Wortfolge.

> [!WARNING]
> Ein Minus davor schließt **nicht** aus: `versicherung -kfz` findet Dokumente, die beide Wörter
> enthalten. Richtig ist `versicherung NOT kfz`.

<a id="searching-by-field"></a>

#### Nach Feld suchen

Mit `feldname:wert` suchen Sie nur in einem bestimmten Feld:

```
correspondent:stadtwerke type:rechnung
correspondent:"deutsche bahn"
tag:steuer,2025
page_count:[10 to 50]
checksum:3a7bd3e2*
```

| Feld | Sucht in |
| --- | --- |
| `title` | Titel |
| `content` | erkanntem Text |
| `correspondent` | Korrespondent |
| `document_type` oder `type` | Dokumenttyp |
| `storage_path` oder `path` | Speicherpfad |
| `tag` | Tags |
| `original_filename` | ursprünglichem Dateinamen |
| `asn` | Archivnummer |
| `page_count` | Seitenzahl |
| `num_notes` | Anzahl der Notizen |
| `checksum` | Prüfsumme der Originaldatei |
| `created` | Erstellt-Datum |
| `added` | Datum des Hinzufügens |
| `modified` | Datum der letzten Änderung |

Wichtig dabei:

- Der Feldname gilt nur für das **direkt folgende** Wort. Besteht der Wert aus mehreren Wörtern,
  setzen Sie ihn in Anführungszeichen.
- Mehrere Tags mit Komma bedeuten „alle davon“: `tag:steuer,2025` findet nur Dokumente mit beiden.
- `asn`, `page_count` und `num_notes` sind Zahlen. Bereiche wie `asn:[100 to 200]` gehen,
  Platzhalter nicht.
- `checksum` findet nur die vollständige Prüfsumme in Kleinbuchstaben. Für den Anfang hängen Sie `*`
  an.
- Für `created`, `added` und `modified` siehe [Nach Datum suchen](#searching-by-date).
- Ein unbekannter Feldname wird wie normaler Text gesucht.

<a id="wildcards"></a>

#### Platzhalter

```
versich*
title:Kontoauszug*
202?
19[6-9]?
```

- `*` steht für beliebig viele Zeichen, `?` für genau ein Zeichen.
- `[…]` steht für ein Zeichen aus einer Auswahl oder einem Bereich, `[!…]` für ein Zeichen, das
  **nicht** darin vorkommt. `19[6-9]?` trifft also 1960 bis 1999.
- Eckige Klammern wirken nur zusammen mit `*` oder `?` als Platzhalter; allein werden sie als Text
  gesucht. Ausnahme: Ein Bereich für genau ein Zeichen ohne weiteren Platzhalter (z. B.
  `title:202[0-5]`) wird mit einer Fehlermeldung abgelehnt. Mit Platzhalter klappt es:
  `title:202[0-5]*`.

> [!NOTE]
> Ist eine [Suchsprache](configuration.md#PAPERRUST_SEARCH_LANGUAGE) mit Wortstamm-Erkennung
> eingestellt, speichert PaperRust Wörter in ihrer Grundform. `rechnung*` findet dann auch
> „Rechnungen“. Ein Präfix, das länger ist als der gespeicherte Stamm, findet dagegen manchmal nichts.
> Findet ein Platzhalter nichts, kürzen Sie den Anfang.

<a id="searching-by-date"></a>

#### Nach Datum suchen

```
created:2024
added:"previous month"
created:[2020-01 to 2022-06]
modified:[-3 days to now]
```

Diese Wörter stehen jeweils für einen ganzen Zeitraum (mit oder ohne Anführungszeichen): `today`,
`yesterday`, `tomorrow`, `previous week`, `this month`, `previous month`, `previous quarter`,
`this year`, `previous year`.

Weitere Schreibweisen:

| Beispiel | Bedeutet | Anführungszeichen |
| --- | --- | --- |
| `created:2024`, `created:2024-07`, `created:2024-07-15` | dieses Jahr, dieser Monat, dieser Tag | nicht nötig |
| `added:march` | März des laufenden Jahres | nicht nötig |
| `added:"next friday"`, `added:"last friday"` | dieser eine Tag | nötig |
| `created:"3 march 2021"` | dieser eine Tag | nötig |
| `added:"2024-07-15T08:30:00Z"` | genau dieser Zeitpunkt | nötig |

**Zeiträume** schreiben Sie als `[von to bis]`. Jede Grenze kann eine der Formen oben sein oder eine
relative Angabe wie `-2 weeks`, `now-30d` oder `now`. Anführungszeichen braucht es in Bereichen
nicht; wenn Sie welche setzen, nur einfache (`['-2 weeks' to now]`) – doppelte werden abgelehnt.

> [!WARNING]
> `now`, `noon`, `midnight` und relative Angaben wie `-2 weeks` funktionieren nur als Grenze eines
> Bereichs. Allein meinen sie einen einzigen Augenblick und finden daher nichts. Schreiben Sie also
> `added:[-2 weeks to now]`, nicht `added:"-2 weeks"`. Ein Wochentag allein (`friday`) und
> Formulierungen wie `3 days ago` oder `this week` werden nicht verstanden.

Datumsangaben richten sich nach der Zeitzone aus `PAPERRUST_TIME_ZONE`.

<a id="searching-custom-fields"></a>

#### Benutzerdefinierte Felder durchsuchen

Name und Wert jedes benutzerdefinierten Felds stehen im Suchindex, aber nur mit Feldnamen:

```
custom_fields.value:4711
custom_fields.name:Kundennummer
custom_fields.name:Kundennummer custom_fields.value:4711
```

- `custom_fields.value:` sucht im Wert irgendeines Felds; die Kurzform ist `custom_fields:`.
- `custom_fields.name:` sucht im Namen des Felds (mehrere Wörter in Anführungszeichen).
- Beides zusammen findet Dokumente, bei denen genau dieses Feld diesen Wert hat.

Auch hier fallen Trennzeichen weg: Ein Wert `KD-4711/03` ist als `kd`, `4711` und `03` gespeichert,
jedes Stück ist einzeln auffindbar.

> [!NOTE]
> Für Datumsfelder unter den benutzerdefinierten Feldern gibt es keine relativen Angaben wie
> `[now to 2 weeks]`. Nutzen Sie dafür die Filter der Dokumentliste.

<a id="searching-notes"></a>

#### Notizen durchsuchen

Auch Notizen findet nur die Suche mit Feldnamen:

```
notes.note:rückruf
notes.user:maria
notes.user:maria notes.note:steuerberater
```

`notes.note:` sucht im Text, `notes.user:` nach dem Verfasser. `notes:` ist die Kurzform von
`notes.note:`.

<a id="searching-barcodes"></a>

#### Barcodes durchsuchen

Wenn PaperRust [Barcode-Inhalte speichert](advanced.md#barcode-contents), finden Sie sie so:

```
barcodes.value:RF18539007547034
barcodes.format:qrcode
barcodes:bezahlcode
```

`barcodes.value:` (Kurzform `barcodes:`) sucht im Inhalt, `barcodes.format:` nach der Art des Codes.
Trennzeichen fallen weg, jedes Stück eines Codes ist einzeln suchbar.

Alle Regeln lassen sich frei kombinieren. Schreibweisen, die hier nicht stehen, funktionieren
möglicherweise nicht wie erwartet.

> [!NOTE]
> Soll die Suche auch Wörter mit kleinen Tippfehlern finden, setzen Sie
> [`PAPERRUST_ADVANCED_FUZZY_SEARCH_THRESHOLD`](configuration.md#PAPERRUST_ADVANCED_FUZZY_SEARCH_THRESHOLD).
> Solche ungefähren Treffer stehen dann unter den genauen.

<a id="keyboard-shortcuts--hotkeys"></a>

## Tastenkürzel

Drücken Sie <kbd>Shift</kbd> + <kbd>?</kbd>, um die Tastenkürzel zu sehen. Die Liste zeigt jeweils
nur die Kürzel, die auf der aktuellen Seite funktionieren.

<a id="usage-recommended-workflow"></a><a id="the-recommended-workflow"></a>

## Empfohlener Arbeitsablauf

Ein einfacher Ablauf, der auch Papier berücksichtigt, das Sie aufbewahren müssen. Grundidee:
**Gesucht wird nur in PaperRust.** Der Papierordner ist bloß ein Lager, sortiert nach einer einzigen
laufenden Nummer.

<a id="preparations-in-paperrust"></a>

### Vorbereitung in PaperRust

Legen Sie zwei Tags an:

- einen **Posteingangs-Tag** (beim Anlegen „Posteingangs-Tag“ anhaken) – jedes neue Dokument
  bekommt ihn automatisch,
- einen Tag **Erledigen** für Dokumente, bei denen noch etwas zu tun ist.

<a id="processing-of-the-physical-documents"></a>

### Bearbeitung der Papierdokumente

Sammeln Sie neue Post in einer Ablage. Von Zeit zu Zeit nehmen Sie sich den Stapel vor, Blatt für
Blatt:

1. **Muss das Original aufbewahrt werden?** (Verträge, Urkunden, Zeugnisse …)
2. **Ja:** Nächste freie Nummer draufschreiben oder ein [ASN-Etikett](asn-labels.md) aufkleben,
   scannen, in **einen** Ordner legen – nur nach Nummer sortiert.
3. **Nein:** scannen und wegwerfen.

Mit ASN-Etiketten erkennt PaperRust die Nummer beim Einlesen selbst und trägt sie ein (wenn die
Barcode-Erkennung eingeschaltet ist). Ist ein Ordner voll, schreiben Sie den Bereich außen drauf
(z. B. „ASN 1–350“), stellen ihn weg und beginnen den nächsten.

Brauchen Sie später ein Original: in PaperRust suchen, ASN ablesen, Ordner mit diesem Bereich nehmen,
Blatt herausziehen.

<a id="processing-of-documents-in-paperrust"></a>

### Bearbeitung in PaperRust

Öffnen Sie die Dokumente mit Posteingangs-Tag und prüfen Sie jedes kurz:

1. ASN eintragen (falls nicht automatisch erkannt).
2. Korrespondent und Dokumenttyp setzen – nicht Pflicht, aber sehr hilfreich beim Finden.
3. Einen verständlichen Titel vergeben, z. B. „Kaufvertrag Fahrrad“.
4. Datum prüfen. PaperRust liest es aus dem Text; bei schlechtem Scan oder mehreren Daten liegt es
   manchmal daneben.
5. Posteingangs-Tag entfernen (das kann PaperRust beim Speichern auch automatisch tun, siehe
   Einstellungen).

> [!TIP]
> Je mehr Dokumente Sie so einordnen, desto besser werden die automatischen Zuordnungen. Sie können
> auch feste Regeln festlegen, siehe [Zuordnung](advanced.md#matching).

<a id="task-management"></a>

### Aufgabenverwaltung

Für Dokumente, bei denen Sie noch etwas tun müssen:

- **Wenn Sie regelmäßig in PaperRust arbeiten:** Tag **Erledigen** vergeben und eine gespeicherte
  Ansicht „Erledigen“ auf die Startseite legen. Ist die Sache erledigt, Tag entfernen.
- **Wenn Sie PaperRust nur als Archiv nutzen:** neben die Ablage für neue Post eine zweite für
  „Erledigen“ stellen. Ist die Sache erledigt, wandert das Blatt in den normalen Stapel.

<a id="remote-ocr"></a>

## Remote-OCR

> [!IMPORTANT]
> Ausgeschaltet, bis Sie es ausdrücklich einschalten.

Statt den Text selbst zu erkennen, kann PaperRust Dokumente an einen Online-Dienst schicken. Derzeit
geht das nur mit
[Azure AI Document Intelligence](https://azure.microsoft.com/en-us/products/ai-services/ai-document-intelligence)
von Microsoft. Dafür brauchen Sie ein Azure-Konto; der Dienst kostet Geld (es gibt eine kleine
kostenlose Stufe) und hat Grenzen bei Dateigröße und Seitenzahl. Microsoft hat mit PaperRust nichts
zu tun. Bedenken Sie: Die Dokumente verlassen dabei Ihren Rechner.

PaperRust ruft dieselbe Schnittstelle auf wie paperless-ngx (Modell `prebuilt-read`, Text und
durchsuchbares PDF). Geprüft ist das mit nachgestellten Antworten, noch nicht gegen den echten
Azure-Dienst.

Einrichten über die [Einstellungen zu Remote-OCR](configuration.md#PAPERRUST_REMOTE_OCR_ENGINE), als
Umgebungsvariable oder unter *Verwalten → Konfiguration*.

Zwei Betriebsarten ([`PAPERRUST_REMOTE_OCR_MODE`](configuration.md#PAPERRUST_REMOTE_OCR_MODE)):

- **Standard:** Jedes Dokument eines passenden Dateityps geht an den Dienst, die lokale OCR wird
  übersprungen.
- **`workflow_only`:** Alles wird lokal erkannt, außer eine
  [Workflow-Aktion „Remote-OCR“](workflows.md#workflow-action-remote-ocr) schaltet den Dienst für ein
  Dokument ein. Außerdem können Sie dann bei **Neu verarbeiten** für einzelne Dokumente Remote-OCR
  wählen.

<a id="architecture"></a>

## Aufbau

Alles steckt in einem Programm, `paperrust`:

- **`paperrust serve`** – der Webserver: Weboberfläche, REST-API, Anmeldung, Downloads und
  Live-Meldungen.
- **`paperrust serve --worker`** – zusätzlich die Hintergrundarbeit im selben Prozess: Einlesen
  (OCR, Barcodes, Zuordnung), Mail-Abruf, Workflows, geplante Aufgaben und der Eingangsordner. Mit
  `--worker-only` läuft nur die Hintergrundarbeit, ohne Webserver.
- **`paperrust manage <befehl>`** – Verwaltungsbefehle wie `migrate` oder `document_exporter`,
  siehe [Administration](administration.md).
- **PostgreSQL** speichert alles, auch die Warteschlange der Hintergrundaufgaben. Redis, Celery,
  Django und Python braucht PaperRust nicht.

Für einen vorsichtigen Umstieg konnte PaperRust auch vor einer laufenden paperless-ngx-Installation
stehen; das geht nur noch bis PaperRust v0.1.0, siehe
[Mischbetrieb](installation.md#mixed-operation).
