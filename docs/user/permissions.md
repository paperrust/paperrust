# Benutzer, Gruppen und Rechte

[English](en/permissions.md) · [Übersicht](README.md)

<a id="permissions"></a><a id="rechte"></a>

## Wie Rechte funktionieren

PaperRust fragt bei jedem Zugriff zwei Dinge:

1. **Darf diese Person den Bereich überhaupt nutzen?** Das regeln die
   [globalen Rechte](#global-permissions), zum Beispiel „darf Dokumente ansehen“ oder „darf Tags
   anlegen“.
2. **Darf sie genau dieses Objekt sehen oder ändern?** Das regeln die
   [Objektrechte](#object-permissions). Jedes Dokument, jedes Tag, jeder Korrespondent usw. hat
   einen Eigentümer und kann anderen Benutzern oder Gruppen Lese- oder Bearbeitungsrechte geben.

Erst wenn beides passt, klappt der Zugriff. Geprüft wird immer auf dem Server, nicht nur in der
Oberfläche.

> [!TIP]
> Objektrechte wirken nur auf das Objekt selbst. Wer ein Tag privat macht, macht damit nicht die
> Dokumente mit diesem Tag privat.

Rechte eines Dokuments ändern Sie in der Dokumentansicht im Reiter **Berechtigungen**. Für viele
Dokumente auf einmal: Dokumente in der Liste auswählen und **Berechtigungen** wählen. Dort legen
Sie fest, ob die neuen Rechte die bisherigen ersetzen oder zu ihnen hinzukommen.

<a id="default-permissions"></a>

### Rechte neuer Objekte

- **In der Oberfläche angelegt** (Tag, Dokumenttyp, Speicherpfad …): Eigentümer ist, wer es
  anlegt; sonst bekommt niemand Rechte. Das ändern Sie unter *Einstellungen* bei **Berechtigungen
  für neue Objekte** (Eigentümer, Leserechte, Bearbeitungsrechte).
- **Über den Eingangsordner eingelesen:** Das Dokument hat zunächst keinen Eigentümer und keine
  zusätzlichen Rechte. Wer Eigentümer wird und wer mitlesen darf, steuern Sie mit
  [Workflows](workflows.md#workflows) („Eigentümer setzen“, „Leserechte vergeben“ …).

<a id="users-and-groups"></a><a id="benutzer-und-gruppen"></a>

## Benutzer und Gruppen

Benutzer und Gruppen verwalten Sie im PaperRust-Design unter *Verwalten → Benutzer und Gruppen*, in
der klassischen Oberfläche unter *Einstellungen → Benutzer und Gruppen*. Dafür braucht man selbst die globalen Rechte „User“ bzw. „Group“.

- Ein Benutzer bekommt alle Rechte seiner Gruppen dazu. Die Oberfläche zeigt, welche Rechte aus
  einer Gruppe stammen.
- Ein neuer Benutzer hat **keine eigenen Rechte**. Er darf nur, was seine Gruppen erlauben.
- Einfacher Weg: Gruppen nach Aufgaben anlegen (z. B. „Buchhaltung“, „Nur lesen“) und Benutzer
  diesen Gruppen zuordnen, statt Rechte einzeln zu setzen.

<a id="roles"></a>

### Rollen (PaperRust-Design)

Statt jedes Recht einzeln anzuhaken, geben Sie einem Benutzer oder einer Gruppe eine **Rolle**:

| Rolle | Darf |
|---|---|
| **Nur lesen** | Dokumente, Notizen, Tags, Korrespondenten, Dokumenttypen, Speicherpfade, Felder und gespeicherte Ansichten ansehen |
| **Bearbeiten** | dazu Dokumente hinzufügen und ändern, Notizen schreiben, eigene Ansichten und Freigabelinks anlegen |
| **Verwalten** | dazu löschen; Tags, Korrespondenten, Typen, Speicherpfade, Felder, Workflows und Mail-Konten/-Regeln pflegen; Aufgaben sehen |
| **Administrator** | alles (Superuser), auch Benutzer und System |

- **Einzelne Rechte anpassen** (aufklappbar) zeigt fünf Themen mit je *Ansehen*, *Bearbeiten*,
  *Löschen*. Wer hier etwas ändert, hat danach die Rolle „Individuell“; „Zurück zu Rolle …“ stellt
  die Rolle wieder her.
- **Eigene Rollen:** Ein Administrator kann den Rollenmodus von „klassisch“ auf „benutzerdefiniert“
  umstellen (Link in der Benutzerverwaltung). Dann erscheint *Verwalten → Rollen*, wo er eigene Rollen
  anlegt; die vier Rollen dienen als Vorlage, „Administrator“ bleibt fest.
- Gespeichert werden weiterhin die normalen Rechte wie bei paperless-ngx. Benutzer aus paperless-ngx
  erscheinen mit „Individuell“, wenn ihre Rechte zu keiner Rolle genau passen; geändert wird dabei
  nichts. Beim Zurückwechseln zu paperless-ngx gehen nur die Rollennamen verloren.

<a id="superusers"></a>

### Superuser

Ein Superuser hat jedes Recht und sieht jedes Objekt, egal wem es gehört. Nur ein Superuser kann
einen anderen Benutzer zum Superuser machen. Manche Bereiche sind nur für Superuser, etwa die
Verwaltung der [Scanner](scanner.md).

> [!TIP]
> Für die tägliche Arbeit lieber ein normales Konto verwenden – als Superuser sehen Sie auch die
> Dokumente aller anderen. Einen (weiteren) Superuser legen Sie
> [auf der Kommandozeile](administration.md#create-superuser) an, oder ein bestehender Superuser
> setzt den Haken im Benutzerdialog.

<a id="admin-status"></a>

### Admin

Der Haken **Admin** im Benutzerdialog erlaubt, den Systemstatus und die Protokolle (Logs) zu sehen.
Weitere Rechte gibt er nicht.

> [!NOTE]
> Für Umsteiger: In paperless-ngx öffnete dieser Haken („staff“) auch die Django-Verwaltung unter
> `/admin/`. Die gibt es in PaperRust nicht; alles läuft über die normale Oberfläche.

<a id="global-permissions"></a><a id="detailed-explanation-of-global-permissions"></a>

### Globale Rechte

Für jeden Bereich gibt es vier Stufen: **Ansehen**, **Hinzufügen**, **Ändern** und **Löschen**.
Sie gelten für den Bereich als Ganzes. Ob ein bestimmtes Dokument sichtbar ist, entscheiden danach
noch dessen Objektrechte.

| Bereich | Wofür |
|---|---|
| AppConfig | Seite *Konfiguration* (Einstellungen für die ganze Installation). Wirksam ab _Ändern_.<br/>**Nur vertrauenswürdigen Personen geben:** Manche Werte, etwa OCR-Argumente, gehen direkt an die Verarbeitungsprogramme. |
| Correspondent | Korrespondenten |
| CustomField | Benutzerdefinierte Felder |
| Document | Dokumente |
| DocumentType | Dokumenttypen |
| GlobalStatistics | Zahlen und Statistiken über den ganzen Bestand. Öffnet keine einzelnen Dokumente. |
| Group | Gruppen |
| MailAccount | E-Mail-Konten |
| MailRule | E-Mail-Regeln |
| Note | Notizen an Dokumenten |
| PaperlessTask | Liste der Dateiaufgaben; _Ändern_ erlaubt, Einträge zu verwerfen |
| SavedView | Gespeicherte Ansichten |
| ShareLink | Freigabelinks (Ändern gibt es hier nicht) |
| StoragePath | Speicherpfade |
| SystemMonitoring | Systemstatus, Aufgabenübersicht und die zugehörigen API-Pfade. Benutzer mit **Admin** sehen den Systemstatus auch ohne dieses Recht. |
| Tag | Tags |
| UISettings | Die gespeicherten Oberflächen-Einstellungen eines Benutzers.<br/>**Wer die Weboberfläche nutzt, braucht hier mindestens _Ansehen_.** |
| User | Andere Benutzer verwalten (*Benutzer und Gruppen*, `/api/users/`). Für das eigene Profil (*Mein Profil*, `/api/profile/`) nicht nötig. |
| Workflow | Workflows. Es gibt nur einen gemeinsamen Satz Workflows für alle; wer sie sehen darf, sieht alle. Was das für Rechte bedeutet: [Workflow-Rechte](workflows.md#workflow-permissions). |

<a id="object-permissions"></a><a id="detailed-explanation-of-object-permissions"></a>

### Objektrechte

| Recht | Was es bedeutet |
|---|---|
| Eigentümer | Sieht und ändert das Objekt. Nur der Eigentümer verteilt Rechte an andere. Bei Dokumenten darf nur er Freigabelinks erzeugen und benutzerdefinierte Felder hinzufügen oder entfernen. |
| Ansehen | Lesen, nicht ändern. Fehlt dieses Recht, zeigt die Oberfläche statt des Namens _Privat_ – etwa bei einem Tag, das Sie nicht sehen dürfen, an einem Dokument, das Sie sehen dürfen. |
| Bearbeiten | Ändern; schließt Ansehen ein. |

**Objekte ohne Eigentümer** kann jeder sehen und bearbeiten, der das passende globale Recht hat.
Das ist so, damit ältere Bestände ohne Eigentümer weiter funktionieren.

**Zugeordnete Objekte, die Sie nicht sehen dürfen:** Trägt ein Dokument ein Tag (oder einen
Korrespondenten, Dokumenttyp, Speicherpfad), das für Sie _Privat_ ist, bleibt diese Zuordnung beim
Speichern erhalten. Sie können die bekannte ID also mitschicken, auch wenn das Objekt in Suche und
Auswahllisten fehlt. Sehen oder ändern können Sie das Objekt selbst trotzdem nicht.

<a id="password-reset"></a>

## Passwort vergessen

Damit „Passwort vergessen“ auf der Anmeldeseite funktioniert, muss PaperRust Mails versenden
können. Dafür einen Mailserver eintragen
([`PAPERRUST_EMAIL_HOST`](configuration.md#PAPERRUST_EMAIL_HOST) und die zugehörigen Werte) und
[`PAPERRUST_URL`](configuration.md#PAPERRUST_URL) setzen – sonst zeigt der Link in der Mail auf die
falsche Adresse. Die Antwort der Seite verrät nicht, ob es ein Konto mit dieser Adresse gibt.

<a id="two-factor-authentication"></a>

## Zwei-Faktor-Anmeldung

Mit der Zwei-Faktor-Anmeldung (2FA) braucht man beim Anmelden außer dem Passwort einen
Einmal-Code aus einer Authenticator-App (TOTP).

So schalten Sie sie ein:

1. *Mein Profil* öffnen und dort den Bereich zur Zwei-Faktor-Anmeldung (TOTP) wählen.
2. Den QR-Code mit der Authenticator-App scannen (oder den geheimen Schlüssel abtippen).
3. Den Code aus der App eingeben und bestätigen.
4. PaperRust zeigt jetzt **10 Wiederherstellungscodes**. Sofort sicher ablegen – sie werden nie
   wieder angezeigt. Jeder Code ersetzt einmal die App, falls das Handy fehlt.

Ab dann fragt PaperRust bei jeder Anmeldung nach dem Code.

Ist das Gerät weg und sind auch die Wiederherstellungscodes verloren, kann ein Superuser die
Zwei-Faktor-Anmeldung des Benutzers unter *Benutzer und Gruppen* ausschalten.

**Anmeldung über OpenID Connect:** Prüft der Anmeldedienst schon selbst einen zweiten Faktor, fragt
PaperRust nicht noch einmal. Erkannt wird das am Wert `amr` des Dienstes; welche Werte als
„zweiter Faktor geprüft“ gelten, legt
[`PAPERRUST_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR`](configuration.md#PAPERRUST_SOCIAL_ACCOUNT_MFA_TRUSTED_AMR)
fest (Standard: `mfa`, mehrere durch Komma getrennt).
