# Formal check: parallel document ingest

PaperRust consumes several documents at the same time by default: `PAPERLESS_TASK_WORKERS` unset
means `auto` (since 2026-11-23, together with the shared OCR page pool; derived from cores and
memory), and any number > 1 does the same; `PAPERRUST_TASK_WORKERS=1` restores one document at a
time as upstream. The locks stay active in every case. Two tasks can then race on the same archive serial number
(ASN), the same file content, or the same new tag name. The code closes these races with PostgreSQL
advisory locks. This folder holds small TLA+ models of that code. The TLC model checker tries
**every interleaving** of the tasks within the bounds below and checks that nothing bad can happen.

Last checked against commit `fd5dd31b` (2026-11-23, includes the duplicate check of shrunk files `paperrust_source_checksum`) (see the skill `.claude/skills/verify-parallel-ingest/SKILL.md`
for the exact files and functions covered).

## What is promised, and the harm if it breaks

| # | Property (plain words) | Harm if broken | TLA+ name |
|---|---|---|---|
| P1 | Two uploads with the same ASN at the same time: one is stored, the other gets the normal "ASN already exists" message, never a raw database error. | The user sees a cryptic `IntegrityError`; the failure is not classified (no `asn_already_exists` status). | `NoRawUniqueError` |
| P2 | With `CONSUMER_DELETE_DUPLICATES` on, the same file uploaded several times at once is stored **at most once** - also when PaperRust stores a shrunk copy of it (`PAPERRUST_SHRINK_ON_INGEST_MB`). | Duplicate documents, although the admin asked for duplicates to be deleted (the database has no unique constraint on the checksum, so nothing else stops it). | `NoDuplicateStored` |
| P3 | A rejection is never wrong: "ASN already exists" or "duplicate" only when a stored document really has that ASN / content. With P2 and no deadlock this also means "never all of them rejected". | A document is lost (original deleted as a "duplicate" of nothing). | `RejectionJustified` |
| P4 | Tags from barcodes / sub-folders: several tasks asking for "Invoice" and "INVOICE" at once get one tag, the same id, and no error. | Two tags differing only in case, or a failed tag assignment. | `OneTagPerName`, `NoRawUniqueError`, `SameTagForSameName` (in `TagGetOrCreate.tla`) |

`UniqueAsn` (no two documents share an ASN) is also checked; the database's unique index already
guarantees it, it is there as a sanity check of the model.
TLC's built-in deadlock check is on: every run must reach the state "all tasks finished".

## Result

All properties **hold** for the code as it is (configs `Ingest_code`, `Ingest_code_keepdups`,
`Tag_code`, and with `--deep` also `Deep_ingest_code`).

Each property has been seen **failing** on a deliberately broken variant, so the check is able to
catch a regression:

| Broken variant (config) | Fails with |
|---|---|
| ASN lock removed (`Broken_no_asn_lock`) | `NoRawUniqueError` |
| checksum lock removed (`Broken_no_sum_lock`) | `NoDuplicateStored` |
| PDF shrinking on, delivered file's checksum not recorded in `paperrust_source_checksum` (the code before 2026-11-23) (`Broken_shrink_no_source`) | `NoDuplicateStored` (already without concurrency) |
| re-check moved before the lock (`Broken_lock_after_find[_dups]`) | `NoRawUniqueError` / `NoDuplicateStored` |
| `REPEATABLE READ` instead of PostgreSQL's default `READ COMMITTED` (`Broken_repeatable_read[_dups]`, `Tag_broken_repeatable_read`) | `NoRawUniqueError` / `NoDuplicateStored` |
| re-check moved after `insert_document` (it then sees its own row) (`Broken_recheck_after_insert`) | `RejectionJustified` |
| no ASN lock and no unique index (`Broken_no_lock_no_index`) | `UniqueAsn` |
| tag lock removed (`Tag_broken_no_lock_case/_same/_ids`) | `OneTagPerName` / `NoRawUniqueError` / `SameTagForSameName` |

Scope boundary, not a regression: `Scope_external_api_writer` shows that an **API edit** that sets
the same ASN on another document at the same moment (it takes no advisory lock) can still give the
consume task a raw unique-constraint error. The contract only promises P1 between consume tasks.

## Run it

```bash
bash verification/parallel-ingest/run.sh          # all 17 checks, about 1 min
bash verification/parallel-ingest/run.sh --deep   # plus 4 concurrent tasks, about 11 min more
bash verification/parallel-ingest/run.sh --trace Broken_no_asn_lock   # print one counterexample
```

Needs Java 11+. `run.sh` downloads TLA+ tools **v1.7.4** (`tla2tools.jar`, SHA-256
`936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`) into `tools/` (gitignored) and
checks the hash. TLC runs with `-Xmx2g` and 2 workers. Exit code 0 = every check behaved as
expected (code configs pass, broken configs fail with the named property).

## Bounds

- `ParallelIngest`: 3 concurrent consume tasks, ASN values {none, 1, 2}, 2 different file contents,
  every combination of ASN/content/"shrunk or not" per task (1728 starting states, ~2.5 million
  states). `--deep`: 4 tasks, ASN {none, 1} (~44 million states).
- `TagGetOrCreate`: 4 concurrent callers, one tag word in two spellings ("lower"/"UPPER"), every
  combination.
- Each task may also fail at any point between writing its row and COMMIT (disk full, workflow
  error ...), which rolls back and releases its locks.

A pass covers the model within these bounds. The pattern is the same for any number of tasks, but
that is an argument, not something TLC proved.

## Assumptions (what the model takes as given)

- PostgreSQL runs at its default isolation level `READ COMMITTED`; PaperRust sets none
  (`pl-core/src/state.rs` `pg_connect_options`). If an administrator changes
  `default_transaction_isolation` to `repeatable read`, P1, P2 and P4 break (see the broken variants).
- `pg_advisory_xact_lock` is exclusive, blocks, and is released only at COMMIT/ROLLBACK; COMMIT makes
  the rows visible no later than it releases the lock.
- ASN is unique in the database (`paperrust_document_asn_key` / `documents_document_archive_serial_number_key`),
  the checksum is not, tag names are unique only case-sensitively (for tags without owner).
- PDF shrinking: any task's file may or may not be shrunk (free choice, also for the same content).
  A shrunk copy gets another checksum (in the model "original + 1", which may equal another task's
  original: someone uploads the shrunk file itself). The row then carries the stored checksum and,
  in `paperrust_source_checksum`, the delivered file's checksum (the lock key); `find_duplicates`
  matches both. That INSERT runs later in the same transaction than `insert_document`; the model folds
  it into `Insert`, which is not observable (others read only committed rows, the own transaction
  makes no lookup after it).
- Only new documents (no new versions: `claim_unique` skips the ASN for versions).
- 64-bit hash collisions of the lock keys are ignored (they would only make unrelated tasks wait).

## Not covered

- Writers outside the consumer (API edit / bulk edit of ASN, tags created in the UI): they take no
  advisory lock. See `Scope_external_api_writer`.
- File names on disk (`MEDIA_LOCK`, unique-name search, unique DB columns), search index, classifier
  cache, the task queue (`pl-queue` claiming and redelivery was not changed by `499c72f0` apart from
  the worker count).
- The retry loop for deadlocks/serialization failures (`40P01`/`40001`) in `run_consumer`: the model
  shows these locks cannot form a cycle among themselves, so the loop is not exercised here.
- A pre-consume script that changes the file: the stored checksum then differs from the delivered
  file and no source row is written, so the same original is not found as a duplicate (same as
  paperless-ngx 3.4.2; contract pl-consumer.md Known difference 25).

## How to read a counterexample

TLC prints a numbered list of states. Each state names the action that led to it (e.g.
`<AsnLockWait line ...>`); every action in the `.tla` file has a comment with the code it stands
for. Follow `pc` (where each task is), `committed` (stored rows), `pending` (rows written but not yet
committed) and `locks`. Example from `Broken_no_asn_lock`, in the code's words:

1. Task 1 and task 2 both carry ASN 1. Both pass `asn_check` (nothing stored yet).
2. Task 1 opens the store transaction, `claim_unique` runs `find_asn` - nothing found.
3. Task 2 does the same - also nothing found (task 1 has not committed).
4. Task 1 inserts and writes ASN 1, commits.
5. Task 2 writes ASN 1 - the unique index raises `23505`; the task fails with a raw database error
   instead of "Given ASN 1 already exists".

If the pstack `/statechart` skill is available, use it to turn a violation into its shortest path in
plain words, or to draw the state chart of a passing model.
