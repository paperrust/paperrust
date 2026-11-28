# Formal check: folder view under concurrency

The folder view (`/rs/api/v2/folders`, contract `docs/contract/pl-api-v2.md` "Ordner", stage 1/2) lets
several users change the folder tree, move documents and upload into folders at the same time. The
code relies on an advisory lock for tree changes (`tree_lock`), row locks (`SELECT .. FOR UPDATE`),
foreign keys and a unique index. This folder holds small TLA+ models of that code. The TLC model
checker tries **every interleaving** of the concurrent requests within the bounds below.

Last checked against commit `ba8ea24b` plus the uncommitted working tree of 2026-11-27
(`move_documents` in one transaction with `FOR SHARE` locks, `delete` with `check_contents_movable`);
exact files and functions: skill `.claude/skills/verify-folders/SKILL.md`.

## What is promised, and the harm if it breaks

| # | Property (plain words) | Harm if broken | TLA+ name |
|---|---|---|---|
| P1 | The folder tree never contains a cycle, whatever PATCH parent / PATCH parent null / DELETE requests run at the same time. | Folders vanish from the tree (no path to the top), recursive queries (`folder__id__tree`, path) loop or miss documents. | `Acyclic` |
| P2 | DELETE `?contents=parent` of a subfolder moves **all** its documents into the parent; none ends silently in "Unsorted" because another user moved it into the folder at the same moment. | Documents land somewhere nobody asked for (the delete reports "moved to parent"). | `DeleteHonorsContents` |
| P3 | No folder request ends in a raw database error (500): no deadlock, no unmapped foreign-key error. Clashes are answered with the mapped 400/404/409 messages. | Cryptic 500 for a normal concurrent click. | `NoRawError` (both models) |
| P4 | An upload into a folder that is deleted meanwhile never makes the ingestion fail; the document stays "Unsorted" (contract, stage 2). | The upload fails although the contract says it is kept. | `NoIngestFkError` |
| P6 | move-documents and DELETE with contents never act on a permission check that a concurrent share / owner change has already made stale: a change committed before the check is seen (403), one committed after waits for the move. | A user moves documents into a folder (or out of a deleted folder) after the owner took the right away a moment earlier. | `NoStaleAuthz` |
| P7 | DELETE moves only subfolders and documents that `check_contents_movable` checked (no new contents slip in between check and move). | Contents moved without a permission check. | `MovesChecked` |
| P5 | Folder names stay unique per level, case-insensitive, under concurrent create / rename / move / delete. | Two folders "Rechnungen" / "rechnungen" side by side. | `UniqueSiblingNames` (`FolderNames.tla`) |

Sanity checks (the foreign keys already guarantee them): `ParentExists`, `DocFolderExists` - no folder
and no document points to a deleted folder. "Documents are never deleted" needs no model: no folder
code path deletes from the document tables (only `paperrust_document_folder` rows; no row = "Unsorted").
"A document is in at most one folder" is the primary key `paperrust_document_folder(document_id)`.

## Result

| Config | Result |
|---|---|
| `Tree_code` (P1, P3) | **holds** |
| `Docs_code` (P1-P3, documents) | **holds** |
| `Names_code` (P3, P5) | **holds** |
| `Upload_code` (P4) | **holds** (F1 fixed in `ba8ea24b`) |
| `Upload_fix_keyshare` (3 requests) | holds |
| `Authz_code` (P1-P4, P6, P7 with share/owner changes) | **holds** |
| `--deep`: `Deep_tree_code`, `Deep_docs_code`, `Deep_authz_code` (3 concurrent requests) | hold |

Each property has been seen **failing** on a deliberately broken variant:

| Broken variant (config) | Fails with |
|---|---|
| `tree_lock()` removed from patch and delete (`Broken_no_tree_lock`) | `Acyclic` (4 folders, see below) |
| `tree_lock()` removed, 3 folders (`Broken_no_tree_lock_deadlock`) | `NoRawError` (deadlock 40P01) |
| parent/cycle check before the lock (`Broken_check_before_lock`) | `Acyclic` |
| `REPEATABLE READ` instead of PostgreSQL's default `READ COMMITTED` (`Broken_repeatable_read`) | `Acyclic` |
| delete reads the folder without `FOR UPDATE` (`Broken_delete_no_for_update`; checks only P2 + sanity, since BFS otherwise first finds a deadlock 40P01 between the delete and a move-documents holding `FOR SHARE` on the folder - also caused by the missing `FOR UPDATE`) | `DeleteHonorsContents` |
| delete without tree lock and without `FOR UPDATE` (`Broken_delete_no_locks`) | `NoRawError` (FK error on DELETE) |
| `assign_folder` without `FOR KEY SHARE` - old code, F1 (`Broken_upload_no_keyshare`) | `NoIngestFkError` |
| move-documents as before 2026-11-27: checks on the pool, autocommit write (`Broken_move_no_locks`) | `NoStaleAuthz` |
| delete's content checks as plain reads, no `FOR UPDATE`/`FOR SHARE` (`Broken_delete_check_no_locks`) | `NoStaleAuthz` |
| unique index missing (`Names_broken_no_index`) | `UniqueSiblingNames` |

Worth knowing: without `tree_lock` two crossing moves (A into B, B into A) cannot build a cycle - the
`FOR UPDATE` of the moved folder and the foreign-key lock (`FOR KEY SHARE`) on the new parent collide,
and PostgreSQL aborts one with a deadlock (500). A cycle needs four folders and two moves that touch no
common row: start `f3` top level, `f2` and `f4` inside `f3`, `f1` inside `f2`; request 1 moves `f2`
into `f4`, request 2 moves `f4` into `f1`. Each check passes alone, both commit:
`f2 -> f4 -> f1 -> f2`. `tree_lock` serialises both moves, so the second one sees the first.

## Findings

### F1 (fixed in `ba8ea24b`): upload into a folder that is being deleted failed the ingestion (P4, now `Broken_upload_no_keyshare`)

Trace (`run.sh --trace Upload_code`), in the code's words:

1. User A sends `DELETE /folders/2?contents=...`: `tree_lock`, `visible_folder(.., lock = true)` holds
   `FOR UPDATE` on folder 2, moves its subfolders.
2. The consume task of an upload with `folder = 2` reaches `assign_folder`
   (`crates/pl-consumer/src/consumer.rs`, `fn assign_folder`, line 1906 at `ba8ea24b`; statement at line 1910):
   `INSERT INTO paperrust_document_folder .. SELECT $1, f.id FROM paperrust_folder f WHERE f.id = $2`.
   The `SELECT` reads without a lock and still sees folder 2 (the delete is not committed), so a row is
   inserted.
3. The delete moves the documents, deletes folder 2 and commits.
4. The foreign-key check of the insert (it waited for the `FOR UPDATE`) now finds folder 2 gone:
   `23503`. `assign_folder` returns the error, `apply_overrides` (`assign_folder(..).map_err(..)?`,
   line 1469) fails, `store_all` rolls back and the ingestion fails - against the contract
   ("Ist der Ordner inzwischen gelöscht, bleibt das Dokument „Unsortiert“ ..., die Verarbeitung
   scheitert nicht"). `23503` is not in the retry set (40P01/40001) of `run_consumer`.

The window is short (the delete transaction is a few statements), so it is rare, but real.
Proposed fix (checked as `Upload_fix_keyshare`, holds): lock the folder in the `SELECT`,
`... FROM paperrust_folder f WHERE f.id = $2 FOR KEY SHARE`. Under READ COMMITTED it waits for the
deleting transaction and then skips the deleted row -> 0 rows -> "stays unsorted" warning, no error.
(Alternative: a savepoint around the statement and treat 23503 like 0 rows.) Fixed this way in `ba8ea24b`.

### Old race (fixed 2026-11-27, working tree): permission check and write apart (P6)

`Broken_move_no_locks`, in the code's words: user A sends move-documents for document 1; the old code
checked `change_document` on the pool, then user B (owner) took the right away (document row
`FOR UPDATE`, owner/guardian write, commit), then A's autocommit `INSERT .. ON CONFLICT` /
`DELETE` ran anyway. Same for DELETE with contents (`Broken_delete_check_no_locks`): subfolder 2 of
folder 1 checked as changeable, `PATCH /folders/2` (permissions) commits, the delete moves folder 2
anyway. The code now locks what it checked (target `FOR SHARE`, documents `FOR SHARE`, subfolders
`FOR UPDATE`) in the same transaction as the write; `Authz_code` holds.

### Observation (sequential, mapped): deleting "A" that contains a subfolder "a"

`DELETE /folders/{A}?contents=parent` first moves the subfolders to A's level, then deletes A. A
subfolder named like A itself then clashes with A (still present) -> 409 "A subfolder has the same
name as a folder at the target level. Rename it first." Mapped, not a 500, so `NoRawError` holds;
mentioned because the user may find it surprising. Not a concurrency issue.

## Run it

```bash
bash verification/folders/run.sh          # 16 checks, about 8-12 minutes
bash verification/folders/run.sh --deep   # plus 3 concurrent requests (tree, documents, permissions)
bash verification/folders/run.sh --trace Upload_code   # print one counterexample
```

Needs Java 11+. `run.sh` downloads TLA+ tools **v1.7.4** (`tla2tools.jar`, SHA-256
`936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`) into `tools/` (gitignored) and
checks the hash. TLC runs with `-Xmx2g` and 2 workers. Exit code 0 = every check behaved as expected:
code configs pass, broken configs fail with the named property.

## Bounds

- `FolderTree` tree configs: 4 folders, **every** starting forest (folder ids are interchangeable:
  TLC symmetry), 2 concurrent requests, each any PATCH parent (to any other folder), PATCH parent null,
  DELETE with `contents=parent` or `unsorted`. `--deep` (`Deep_tree_code`): 3 requests on every forest of
  3 folders (~500 000 states).
- `FolderTree` document configs: folder 2 inside 1, folder 3 top level; 2 documents in every
  placement; requests: DELETE (both modes), move-documents (one document, to any folder or "Unsorted"),
  PATCH parent / parent null. 2 requests (~280 000 states); `--deep` (`Deep_docs_code`): 3 requests,
  1 document in every placement (~3.1 million states).
- Upload configs: same tree, 1 document, DELETE / move-documents / upload into any folder; 2 requests
  (`Upload_fix_keyshare`: 3, ~1.2 million states).
- `Authz_code`: same tree, 1 document in every placement, 2 requests from DELETE (both modes),
  move-documents, PATCH parent / parent null, and a second user's share change on any folder
  (`unsharef`) or owner/permission change on the document (`unsharedoc`). The caller starts with
  change rights on everything; a change only takes rights away. ~59 000 states; `--deep` (`Deep_authz_code`): 3 requests, ~5 million states (~30 min).
- `FolderNames`: 2 existing folders in every valid tree and naming, names "a", "A", "b" ("a" = "A"),
  2 requests: create (any parent, any name), rename, PATCH parent, DELETE `contents=parent`.

A pass covers the model within these bounds, not every size.

## Assumptions (what the model takes as given)

- PostgreSQL `READ COMMITTED` (PaperRust sets no isolation level, `pl-core/src/state.rs`). Each
  statement reads the latest committed rows plus its own writes. An `UPDATE/DELETE .. WHERE col = x`
  waits for other writers of the rows it matches and re-checks them on the newest version; the model
  evaluates the whole statement once all those locks are free (slightly stronger than PostgreSQL,
  which fixes the candidate rows at statement start - only matters for rows that start matching while
  it waits, which `tree_lock` / `FOR UPDATE` rule out in the code configs).
- Lock modes as in PostgreSQL: `FOR UPDATE` and `DELETE` conflict with everything; a plain `UPDATE`
  not touching the key takes `FOR NO KEY UPDATE`; every foreign-key check takes `FOR KEY SHARE` on the
  referenced folder row (conflicts only with `FOR UPDATE`). All held to COMMIT/ROLLBACK. A lock-wait
  cycle is aborted (40P01): a 500 for the folder API (`db(e)`), a retry for the consumer.
- A foreign-key check that waited for a deleting transaction fails (23503) when that one commits.
- `REPEATABLE READ` variant: snapshot at the start of the first statement (before its lock wait);
  writing a row changed since the snapshot -> 40001; FK checks use the latest rows.
- Unique index `paperrust_folder_name_uniq`: an insert/update meeting a clashing version of an open
  transaction waits for it, then fails (23505) if the clash is committed; an update that keeps the key
  adds no index entry.
- move-documents is modelled for one document (the real statement handles a list in one statement;
  rows are locked one by one in id order, which is covered as long as each row behaves as here).
  The visibility count and `count_unchangeable` are one step (same rows, same locks; losing the view
  right implies losing the change right). `document_lock_table` (catalog query) is not a step.
- `check_contents_movable`: the subfolders' `FOR UPDATE` takes all row locks in one step; a
  subfolder without change right aborts at that step (the code decides after the parent check -
  same rows read, same 403 class). Caller is no superuser (superusers skip the document check).
- Permission changes: folder shares/owner change only in `patch` under `FOR UPDATE` of the folder;
  document owner/permissions only under the document row lock (`update.rs` `begin_root_tx`,
  `bulk_edit::lock_documents` before `set_permissions`). Read from the code on 2026-11-27.

## Not covered

- `create` in `FolderTree` (a new folder is a leaf: cannot close a cycle; its names are in
  `FolderNames`), granting rights (only revocation is modelled), `If-Match` revisions,
  the counts in `GET /folders`.
- The extras import / `manage migrate` paths that write the tables, the `ensure()` DDL race, the
  owner/group cleanup triggers, document deletion (trash) triggers.
- Upload checks before the consume task (`check_upload_target` in `upload.rs`): it is a plain read; a
  folder deleted after it is handled by `assign_folder` (that is what P4 is about).
- The v1/v2 folder filters (read only).
- Rights that change without touching the folder or document row: group membership
  (`auth_user_groups`), user/group permissions, superuser/active flag. Not modelled, but closed in the
  code since 2026-11-28 by the same pattern the model checks: move/delete first lock the caller's
  `auth_user` row and groups `FOR SHARE` and re-read the rights (`folders.rs` `fresh_caller`); the
  pl-auth-api writers (users update/delete, groups update/delete) and roles apply lock the user/group
  row `FOR UPDATE`. DB tests: `move_honours_concurrent_user_permission_revoke`,
  `move_honours_concurrent_group_permission_revoke` (`crates/pl-api-v2/tests/folders_api.rs`).
  Writers outside PaperRust (Django admin of a mixed deployment) do not take these locks.
- Assumption checked against a stage-1 DB (2026-11-28): document permission writers and the move lock
  the same base rows. Locks through the view `documents_document` fail on stage-1 DBs, so all document
  row locks go through `pl_authz::grants::document_lock_table` (base table `paperrust_document` from
  stage 1, also before the refresher has seen stage 4); `crates/pl-consumer/tests/lock_table_db.rs`.

## How to read a counterexample

TLC prints a numbered list of states; each names the action (e.g. `<SAsFk line ...>`), and every
action in the `.tla` file has a comment with the code statement it stands for. Follow `op` (what each
request does), `pc` (index into its statement list `Steps(op)`), `fcv`/`dcv` (committed
`parent_id` / folder of each document; `-1` = folder deleted, `0` = top level / "Unsorted"),
`fnv`/`fw` and `dnv`/`dw` (written but not committed, and by whom), `locks` and `res` (outcome).
If the pstack `/statechart` skill is available, use it to turn a violation into its shortest path in
plain words, or to draw the state chart of a passing model.
