# PaperRust 0.1.1 – Release notes

[Deutsch](RELEASE-NOTES-0.1.1.md) · As of 2026-11-22

A maintenance release after [0.1.0](RELEASE-NOTES-0.1.0.en.md): fixes from the first real-world test with a
copy of a real collection, better text recognition and a reworked AI assistant. Database and files are
unchanged – updating from 0.1.0 means replacing the image or program.

## Fixed

- **Empty search after switching:** after switching from paperless-ngx the search index could stay empty
  (0 hits until a manual rebuild). PaperRust now rebuilds it on start when it is empty or holds less than
  half of the documents, and the switch marks it for a rebuild.
- **Photos and image files:** images are sent to text recognition in grayscale. On deskewed colour photos
  single words (umlauts) were lost otherwise.
- **Case-insensitive rules:** regular expressions with lookaround in matching and workflow rules did not
  ignore case as configured.
- **Taken over from paperless-ngx after 3.4.2:** passwords in settings always masked with a fixed-length mask,
  portable filename filter in workflows, input fields report values while typing.

## New

- **German and English text recognition by default** (`deu+eng` instead of `eng` only). Costs 3–7 % time,
  German text is read much better. Without German language data it stays `eng`.
- **Automatic language (optional):** `PAPERRUST_OCR_LANGUAGE_AUTO=true` detects the text language and reads a
  document in another installed language (e.g. French) a second time with it.
- **AI assistant:**
  - finds relevant passages without embeddings too (keywords from the question for full-text search)
  - shows progress with seconds
  - keeps running when you leave the page
  - "Open N sources" shows exactly those documents
- **Sidebar:** "AI assistant" entry when AI is enabled; collapsible to icons on wide screens.
- **AI settings:** fields for the embedding service and AI index status with a button to update it.

## Known limitations

Unchanged from [0.1.0](RELEASE-NOTES-0.1.0.en.md#known-limits). Note on search: several words require
all of them (AND) – exactly like paperless-ngx 3.4.2 (checked 22 Nov). Older paperless-ngx versions (e.g. 3.1.3)
combined them with OR and returned more hits; to get that, type `word1 OR word2`.

## Updating

- Docker: set the image to `ghcr.io/paperrust/paperrust:0.1.1` and restart. The search index is rebuilt on
  first start if needed.
- Desktop/Linux package: install the new package.
- As always, back up first.
