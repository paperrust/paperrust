# E-Mail-Eingang

[English](en/mail.md) · [Übersicht](README.md)

<a id="incoming-mail"></a><a id="incoming-email"></a>

## Dokumente aus E-Mails übernehmen

Viele Rechnungen und Briefe kommen heute per Mail. PaperRust kann Ihr Postfach regelmäßig
abrufen und Anhänge (oder ganze Mails) als Dokumente ablegen. Sie brauchen dafür zwei Dinge. Im
PaperRust-Design richten Sie Konten unter *Verwalten → E-Mail* ein und Regeln unter *Automatik →
E-Mail-Abruf* (dort sehen Sie auch, welche Workflows nach einer Regel laufen – siehe
[Automatik](automatik.md#mail)). In der klassischen Oberfläche liegt beides unter *Verwalten → E-Mail*.

1. **Ein E-Mail-Konto** – die Zugangsdaten zum Postfach (IMAP-Server, Port, Verschlüsselung SSL
   oder STARTTLS, Benutzername, Passwort).
2. **Mindestens eine Regel** – sie sagt, welche Mails übernommen werden und was danach mit ihnen
   passiert.

Bei jedem Abruf geht PaperRust so vor:

1. Anmelden beim Mailserver.
2. Je Regel (in der eingestellten Reihenfolge) die passenden Mails im gewählten Ordner suchen.
3. Aus jeder neuen Mail die passenden Dateien zum Einlesen einreihen. Dabei bekommen die Dokumente
   die Angaben aus der Regel (Tags, Dokumenttyp, Korrespondent, Titel, Eigentümer).
4. Sobald alle Dateien einer Mail eingelesen sind: die **Aktion** der Regel auf diese Mail anwenden
   (z. B. als gelesen markieren).

Mails, die zu keiner Regel passen, fasst PaperRust nicht an.

PaperRust ruft standardmäßig **alle 10 Minuten** ab. Den Zeitplan ändern Sie mit
[`PAPERRUST_EMAIL_TASK_CRON`](configuration.md#PAPERRUST_EMAIL_TASK_CRON). Sofort abrufen: Knopf
**Jetzt abrufen** beim Konto, oder auf der Kommandozeile
[`paperrust manage mail_fetcher`](administration.md#mail).

<a id="mail-rules"></a>

### Was eine Regel festlegt

**Welche Mails**

- **Ordner** – meist `INBOX`. Unterordner werden mit dem Trennzeichen des Servers geschrieben,
  oft `.` oder `/` (manchmal `|`). Findet PaperRust einen Ordner nicht, schreibt es alle Ordner des
  Kontos ins Protokoll – dort sehen Sie die richtige Schreibweise.
- **Höchstalter (Tage)** – ältere Mails werden übergangen.
- **Absender / Empfänger / Betreff / Nachrichtentext enthält** – leere Felder gelten nicht. Eine
  Mail muss **alle** ausgefüllten Bedingungen erfüllen.

**Was übernommen wird**

- **Was importiert wird:** nur die Anhänge, die ganze Mail als `.eml`-Datei, oder beides (Mail als
  `.eml` und jeder Anhang einzeln).
- **Art der Anhänge:** nur echte Anhänge, oder alle Dateien, auch eingebettete (z. B. Bilder im
  Mailtext).
- **Nur Dateien mit diesem Namen / Dateien mit diesem Namen überspringen:** Muster wie `*.pdf`
  oder `*rechnung*`, mehrere durch Komma getrennt, Groß-/Kleinschreibung egal.
- Es werden nur Dateitypen übernommen, die PaperRust lesen kann (PDF, Bilder, Text, Office, …).
  Hat eine Mail keine passende Datei, wird sie als „verarbeitet, nichts übernommen“ vermerkt.

**Welche Angaben das Dokument bekommt**

- **Titel übernehmen aus:** Betreff der Mail, Dateiname des Anhangs, oder gar nicht (dann wie
  sonst beim Einlesen).
- **Tags** und **Dokumenttyp** der Regel.
- **Korrespondent übernehmen aus:** leer lassen, Absenderadresse, Absendername, oder ein fest
  gewählter Korrespondent. Bei Adresse oder Name legt PaperRust den Korrespondenten an, falls es ihn
  noch nicht gibt.
- **Eigentümer der Regel wird Eigentümer des Dokuments** – gilt für Anhänge. Eine als `.eml`
  übernommene Mail bekommt den Regel-Eigentümer immer.

**Reihenfolge**

- Regeln laufen nach dem Feld **Reihenfolge** ab.
- **Hier aufhören:** Hat diese Regel aus einer Mail etwas übernommen, werden die übrigen Regeln
  für dieses Konto übersprungen.
- Tipp: Eine letzte „Auffangregel“ ohne Filter, die ein Tag wie „Mail – prüfen“ setzt, sammelt
  alles, was die Regeln davor nicht erfasst haben.

<a id="mail-actions"></a>

### Was danach mit der Mail passiert

Die Aktion verhindert, dass dieselbe Mail auf anderem Weg ein zweites Mal übernommen wird. Sie
geschieht nur bei Mails, aus denen tatsächlich etwas eingelesen wurde.

| Aktion | Wirkung | Welche Mails die Regel beachtet |
|---|---|---|
| Als gelesen markieren (Standard) | Mail wird als gelesen markiert | nur ungelesene – eine Mail, die Sie vorher schon gelesen haben, übernimmt PaperRust also nicht |
| Markieren | setzt die Markierung („Fahne“, wichtig) | nur nicht markierte |
| Mit Tag versehen | setzt ein eigenes Schlagwort (IMAP-„Keyword“, bei Gmail ein Label) | nur Mails ohne dieses Schlagwort |
| In diesen Ordner verschieben | verschiebt die Mail in den angegebenen Ordner | alle im Regel-Ordner |
| Löschen | löscht die Mail sofort | alle im Regel-Ordner – **mit Vorsicht verwenden** |

Für „Mit Tag versehen“ und „Verschieben“ tragen Sie das Schlagwort bzw. den Zielordner bei **Wert
für die Aktion** ein. Nicht jeder Mailserver kennt eigene Schlagwörter.

**Farbige Markierungen in Apple Mail:** als Schlagwort `apple:<farbe>` eintragen, z. B.
`apple:green`. Möglich sind `red`, `orange`, `yellow`, `blue`, `green`, `violet` und `grey`.

> [!TIP]
> „Als gelesen markieren“ ist die harmloseste Wahl – es geht nichts verloren. Wer es ordentlicher
> mag: im Postfach einen Ordner „PaperRust“ anlegen, Mails dort hinein verschieben lassen (von
> Hand oder per Filter im Mailprogramm) und die Regel sie nach dem Übernehmen in einen Ordner
> „Archiviert“ verschieben lassen.

<a id="processed-mail"></a><a id="verarbeitete-mails"></a>

### Verarbeitete Mails

PaperRust merkt sich jede Mail, die eine Regel schon bearbeitet hat – auch wenn nichts übernommen
wurde oder ein Fehler auftrat. Erkannt wird die Mail an ihrer Nummer auf dem Server (IMAP-`UID`).
So wird keine Mail doppelt verarbeitet.

Die Liste finden Sie bei der jeweiligen Regel unter **Verarbeitete E-Mails anzeigen**. Dort sehen
Sie auch, ob es geklappt hat oder warum nicht.

- **Schlägt das Einlesen einer Datei fehl**, wird die Mail als fehlgeschlagen vermerkt und die
  Aktion **nicht** ausgeführt. Die Mail bleibt also, wo sie war.
- **Eine Mail erneut verarbeiten:** ihren Eintrag in der Liste löschen. Beim nächsten Abruf kommt
  sie wieder dran (sofern sie noch zur Regel passt).

<a id="oauth-email-setup"></a>

### Gmail und Outlook mit OAuth

Bei Gmail und Outlook können Sie statt eines Passworts die Anmeldung über OAuth2 verwenden.

1. Beim Anbieter (Google bzw. Microsoft) eine eigene App registrieren und dort Client-ID und
   Client-Secret abholen. Als Rückleitungs-Adresse
   `<Ihre PaperRust-Adresse>/api/oauth/callback/` eintragen.
2. Client-ID und Secret in PaperRust eintragen ([Einstellungen für OAuth](configuration.md#email_oauth)).
3. [`PAPERRUST_OAUTH_CALLBACK_BASE_URL`](configuration.md#PAPERRUST_OAUTH_CALLBACK_BASE_URL) oder
   [`PAPERRUST_URL`](configuration.md#PAPERRUST_URL) auf die Adresse setzen, unter der Sie PaperRust
   im Browser öffnen.
4. Unter *Einstellungen → E-Mail* **Gmail-Konto verbinden** bzw. **Outlook-Konto verbinden**
   wählen und beim Anbieter anmelden.
5. Im neu angelegten Konto Ihre E-Mail-Adresse eintragen und speichern.

Danach verhält sich das Konto wie jedes andere; PaperRust erneuert die Anmeldung selbst.

> [!NOTE]
> Der OAuth-Weg ist in PaperRust neu und bisher nur mit Testwerten geprüft, nicht mit echten
> Google- oder Microsoft-Konten. Klappt es nicht, stehen Einzelheiten im Protokoll. Der erprobte Weg
> ist ein Konto mit Passwort (bei Gmail ein App-Passwort).

<a id="mail-notes"></a>

### Gut zu wissen

- **Mailserver im eigenen Netz:** erlaubt (Standard). Mit
  [`PAPERRUST_EMAIL_ALLOW_INTERNAL_HOSTS`](configuration.md#PAPERRUST_EMAIL_ALLOW_INTERNAL_HOSTS)`=false`
  lässt PaperRust nur öffentliche Adressen zu.
- **Zeitlimit:** Antwortet der Server bei einem Schritt 60 Sekunden lang nicht, bricht PaperRust
  den Abruf ab und versucht es beim nächsten Mal erneut.
- **Verschlüsselte Mails (GPG):** Mit
  [`PAPERRUST_ENABLE_GPG_DECRYPTOR`](configuration.md#PAPERRUST_ENABLE_GPG_DECRYPTOR)`=true`
  entschlüsselt PaperRust sie vor dem Einlesen. Dafür muss das Programm `gpg` mit Ihrem Schlüssel
  installiert sein (im Docker-Image ist es nicht enthalten). Fehlt es, steht ein Fehler im Log.
- **`.eml`-Dateien:** PaperRust liest den Text der Mail selbst aus (ohne Tika) und macht daraus ein
  PDF (Kopfzeilen, Text, bei HTML-Mails auch der HTML-Teil mit eingebetteten Bildern). Welche Teile
  ins PDF kommen, bestimmt die Einstellung **PDF-Layout** der Regel bzw.
  [`PAPERRUST_EMAIL_PARSE_DEFAULT_LAYOUT`](configuration.md#PAPERRUST_EMAIL_PARSE_DEFAULT_LAYOUT).
  Dafür braucht es `paperrust-pdftool` (liegt bei).
