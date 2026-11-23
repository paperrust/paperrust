# PaperRust 0.1.3 – Release notes

[Deutsch](RELEASE-NOTES-0.1.3.md) · As of 2026-11-23

A small release after [0.1.2](RELEASE-NOTES-0.1.2.en.md): faster batch imports without making single documents
slower. Database and files are unchanged – updating means replacing the image or program.

## New

- **Several documents at once with a shared core pool (now the default):** all documents share the CPU cores page
  by page. A document on its own gets all cores; when several arrive, the cores are redistributed after every page.
  Measured on 3 cores (NAS, like a Raspberry Pi 5): a single 8-page scan 15.5 s as before, OCR of 30 scans in a
  batch 55 instead of 95 seconds, no errors. For one document at a time set `PAPERRUST_TASK_WORKERS=1`.
- **Containers with a memory limit:** PaperRust follows the container's memory limit (cgroup v1/v2) instead of the
  whole machine's memory, so it doesn't start too much at once.
- The startup message shows the number of tasks and the size of the shared core pool.

## Known limits

As in [0.1.2](RELEASE-NOTES-0.1.2.en.md#known-limits). Work outside page OCR (PDF/A, thumbnails, text extraction)
does not count towards the shared pool yet.

## Updating

- Docker: set the image to `ghcr.io/paperrust/paperrust:0.1.3` and restart.
- Desktop/Linux package: install the new package. As always, back up first.
