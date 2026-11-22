# PaperRust 0.1.2 – Release-Notizen

[English](RELEASE-NOTES-0.1.2.en.md) · Stand 2026-11-22

Eine kleine Version nach [0.1.1](RELEASE-NOTES-0.1.1.md), vor allem für die Desktop-App. Datenbank und Dateien
bleiben unverändert – ein Update ist ein Austausch des Images bzw. Programms.

## Neu in der Desktop-App

- **PaperRust-Oberfläche auch mit einem paperless-ngx-Server:** Ist die App mit einem Server verbunden, auf dem
  noch paperless-ngx läuft, zeigt sie auf Wunsch ihre eingebaute PaperRust-Oberfläche statt der Server-Oberfläche
  (Tray → „Neue Oberfläche“ oder Einstellungen → Oberfläche). Anmeldung einmal mit Benutzername und Passwort.
  Listen, Suche und Bearbeiten gehen; Funktionen, die es nur in PaperRust gibt (KI-Assistent, Zeitleiste,
  Workflows, Rollen), zeigen „nicht verfügbar“. Zurück über „Oberfläche des Servers“.
- **Betriebsart nachträglich wechseln:** Tray → „Betriebsart wechseln …“ oder Einstellungen → Betriebsart:
  lokal ↔ mit Server verbinden, mit Bestätigung und Neustart. Lokale Daten und Server-Zugänge bleiben erhalten.
- Die PaperRust-Oberfläche startet im zuletzt gewählten Design.

## Korrigiert

- Eine unlesbare Einstellungsdatei der Desktop-App wird beim Wechsel der Betriebsart nicht mehr überschrieben
  (vorher wären alle Server-Profile verloren gegangen).
- Linux-Pakete werden wieder auf Debian 12 gebaut (glibc 2.36) und laufen auf Debian 12 und Ubuntu 22.04.

## Bekannte Grenzen

Wie [0.1.1](RELEASE-NOTES-0.1.1.md#bekannte-grenzen). Neu: Mit einem paperless-ngx-Server meldet man sich in der
PaperRust-Oberfläche nur mit Benutzername und Passwort an (kein Single Sign-on); Antworten des KI-Assistenten
über die Desktop-App erscheinen erst vollständig, nicht schrittweise.

## Update

- Docker: Image auf `ghcr.io/paperrust/paperrust:0.1.2` stellen und neu starten.
- Desktop/Linux-Paket: neues Paket installieren. Wie immer vorher sichern.
