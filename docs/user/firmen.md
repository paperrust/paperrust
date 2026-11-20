# Für Firmen: Betrieb und Rechte

[English](en/firmen.md) · [Übersicht](README.md)

PaperRust eignet sich auch für Büros und kleine Firmen: ein Server, viele Mitarbeitende, jede und
jeder sieht nur, was er sehen darf. Diese Seite fasst zusammen, was es dafür **heute** gibt. Pläne
für mehr (Verteilen der App, Geräteverwaltung, Anbindung an Buchhaltung) stehen im Konzept
[docs/FIRMEN.md](../FIRMEN.md) – davon ist noch nichts umgesetzt.

## Betrieb

- **Ein Server, kein Bildschirm nötig:** PaperRust läuft als Docker-Container oder Systemdienst
  (`paperrust serve --worker`) neben PostgreSQL, z. B. auf einem Firmen-Server, einer NAS oder in der
  Cloud ([Installation](installation.md)). Er braucht wenig Speicher und startet in Sekunden neu.
- **Zugriff:** im Browser oder mit der Desktop-App im Modus „mit Server verbinden“; die App kann
  ausgewählte Dokumente auch offline vorhalten ([docs/REMOTE-APP.md](../REMOTE-APP.md)). Jedes Gerät
  bekommt einen eigenen Schlüssel, den der Benutzer selbst widerrufen kann.
- **Hinter einem Reverse Proxy** mit HTTPS betreiben; `PAPERRUST_URL` setzen
  ([Konfiguration](configuration.md)).
- **Mehrere Prozesse** (Webserver und Worker getrennt) gehen mit `PAPERRUST_CACHE=postgres`
  ([Konfiguration](configuration.md#message-broker)).
- **Einstellungen** stellt ein Administrator in der Oberfläche ein (*Verwalten → Konfiguration → Server*);
  in der Umgebung müssen nur Datenbank und `SECRET_KEY` stehen
  ([Einstellungen in der Oberfläche](configuration.md#settings-ui)).

## Anmeldung

- **Firmenkonto:** Anmeldung über OpenID Connect (z. B. Microsoft Entra ID, Keycloak, Authentik) oder
  direkt über Microsoft, Google, GitHub, GitLab, Nextcloud, Discord, Gitea; auf Wunsch nur noch so
  ([Fortgeschritten → SSO](advanced.md#openid-connect-and-social-authentication)).
- **Anmeldung über den Proxy** (Remote-User-Header), etwa mit Authelia
  ([Fortgeschritten](advanced.md#remote-user-authentication)).
- **Zwei-Faktor-Anmeldung** (TOTP) je Benutzer ([Rechte](permissions.md#two-factor-authentication)).
- **E-Mail-Bestätigung** neuer Konten auf Wunsch Pflicht
  ([Fortgeschritten](advanced.md#email-verification)).

## Rechte

- **Rollen:** *Nur lesen*, *Bearbeiten*, *Verwalten*, *Administrator* – oder eigene Rollen. Eine
  Rolle gibt man einem Benutzer oder einer ganzen Gruppe (z. B. „Buchhaltung“)
  ([Rechte → Rollen](permissions.md#roles)).
- **Je Dokument:** „Sichtbar für“ – nur ich, mein Team oder bestimmte Personen. Auch für viele
  Dokumente auf einmal.
- **Automatisch:** Workflows können Eigentümer und Rechte beim Einlesen setzen
  ([Automatik](automatik.md)).
- Jede Abfrage prüft der Server; die Suche, die KI und die Apps sehen nur, was der Benutzer sehen darf.

## Nachvollziehen und Schützen

- **Verlauf** je Dokument: wer hat wann was geändert (Audit-Log, wie paperless-ngx).
- **Integritätsprüfung** der Dateien nach Zeitplan ([Integritätsprüfung](integrity.md)).
- **Ransomware-Schutz:** Wächter mit Nur-Lese-Modus und verschlüsselte Sicherungen mit Versionen,
  auch auf S3 mit Object Lock ([Schutz vor Ransomware](ransomware.md)).
- **KI** nur, wenn eingeschaltet; für vertrauliche Unterlagen ein eigenes Modell im Firmennetz
  verwenden ([KI](ai.md)).

## Was noch fehlt

- Vorkonfigurierte Installation der App für die IT (Server-Adresse fest vorgegeben), schlanke
  App ohne Datenbank, Geräteübersicht für Administratoren (verlorenes Gerät sperren).
- Offline-Änderungen in der App, die später gesendet werden.
- Anbindung an ERP/Buchhaltung.

Alles das ist im Konzept [docs/FIRMEN.md](../FIRMEN.md) beschrieben, aber nicht umgesetzt.
