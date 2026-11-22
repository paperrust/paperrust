# PaperRust 0.1.2 – Release notes

[Deutsch](RELEASE-NOTES-0.1.2.md) · As of 2026-11-22

A small release after [0.1.1](RELEASE-NOTES-0.1.1.en.md), mainly for the desktop app. Database and files are
unchanged – updating means replacing the image or program.

## New in the desktop app

- **PaperRust interface also with a paperless-ngx server:** when the app is connected to a server that still runs
  paperless-ngx, it can show its built-in PaperRust interface instead of the server's own one (tray → "New
  interface" or Settings → Interface). Sign in once with user name and password. Lists, search and editing work;
  PaperRust-only features (AI assistant, timeline, workflows, roles) show "not available". Back via "Server's
  interface".
- **Change the operating mode after setup:** tray → "Change mode …" or Settings → Mode: local ↔ connect to a
  server, with confirmation and restart. Local data and server profiles are kept.
- The PaperRust interface starts in the design chosen last.

## Fixed

- An unreadable settings file of the desktop app is no longer overwritten when changing the mode (before, all
  server profiles would have been lost).
- Linux packages are built on Debian 12 again (glibc 2.36) and run on Debian 12 and Ubuntu 22.04.

## Known limits

As in [0.1.1](RELEASE-NOTES-0.1.1.en.md#known-limitations). New: with a paperless-ngx server, the PaperRust interface
only signs in with user name and password (no single sign-on); AI assistant answers through the desktop app
appear when complete, not step by step.

## Updating

- Docker: set the image to `ghcr.io/paperrust/paperrust:0.1.2` and restart.
- Desktop/Linux package: install the new package. As always, back up first.
