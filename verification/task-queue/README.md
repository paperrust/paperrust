# Formal check: task queue (PostgreSQL replacement for Celery + Redis)

PaperRust runs its background tasks (consume, index, mail, train, ...) through its own queue in
PostgreSQL (`crates/pl-queue`, table `paperrust_task_queue`). Several worker processes, on one or
several hosts, take tasks with `FOR UPDATE SKIP LOCKED`, hold them with a **lease** that a
**heartbeat** extends, and a **reaper** in every worker hands tasks of crashed workers to someone
else. Periodic tasks come from a scheduler (beat) that every worker process runs. This folder holds
small TLA+ models of that code. The TLC model checker tries **every interleaving** within the
bounds below.

Last checked against commit `83f18906` plus the uncommitted working tree of 2026-11-27 (`BeatStore::run_due`, capped durations / `checked_add` / `force_reap` in `worker.rs`) (exact files and functions: the skill
`.claude/skills/verify-task-queue/SKILL.md`), then updated for the fixes of F1-F3 (2026-11-23,
uncommitted at the time of writing; the sync commit in the skill moves to the commit that contains
them).

## What is promised, and the harm if it breaks

Promises from `docs/contract/pl-queue.md` B3 (points 2, 5, 7, 8) and Known difference 9.

| # | Property (plain words) | Harm if broken | TLA+ name (model) |
|---|---|---|---|
| P1 | A task is never executed by two workers at the same time, as long as the hosts' clocks differ by less than 20 % of the lease (contract B3.8). | The same document consumed twice at once, two mail fetches of one account, two index writers. | `ExclusiveExecution` (`QueueLease`) |
| P2 | The end state of a task (done / failed / revoked, with its callbacks, chord counter and PaperlessTask result) is written at most once ("exactly once for state and PaperlessTask"). | Chord body or error callbacks run twice, a result overwritten by a stale worker. | `TerminalOnce` (`QueueLifecycle`, also checked in `QueueLease`) |
| P3 | What `/api/tasks/` shows matches the queue: a finished task shows its real result, and a result exists only for a finished task. | "Started" forever for a finished task, "success" for a task that is still running. | `StatusConsistent` (`QueueLifecycle`) |
| P4 | No task is lost: after any crash, shutdown or lost lease it ends as done, failed (`WorkerLostError` after `MAX_DELIVERIES`) or revoked, and its PaperlessTask ends with that result. | A document that never gets consumed and a task that shows "pending"/"started" forever. | `Finishes`, `StatusSettles` (liveness, `QueueLifecycle`) |
| P5 | Ids that a document edit appends to a waiting `bulk_update_documents` task (coalescing, `pl-documents-write/src/coalesce.rs`) are processed. | Workflows, file names or index not updated for some documents. | `NoLostMerge` (`QueueLifecycle`) |
| P6 | Several schedulers (one per worker process / instance) enqueue each due time of a periodic task once. | Mail fetched twice, classifier trained twice, empty-trash run twice. | `NoSlotTwice` (`BeatReserve`) |
| P7 | A due time that is recorded as run (`last_run_at`) was really enqueued: an enqueue error or a crash never loses a scheduled run. | Mail not fetched, trash not emptied, classifier not trained until the next slot. | `NoLostSlot` (`BeatReserve`) |

Delivery is **at-least-once** by design (contract Known difference 1): after a crash or a lost
lease the task runs again, so handlers must be idempotent. The models check that the queue *state*
is exactly-once, not the handlers' side effects.

## Result

Holds for the code (configs `Lease_code_hang`, `Lease_code_fast_fail`, `Lease_code_fast_fail_no_skew`,
`Life_code`, `Life_code_same_process`, `Coalesce_code`, `Life_code_unbounded_prestart`,
`Life_code_same_process_unbounded`, `Coalesce_code_unbounded`, `Beat_code`, `Beat_code_fallback` (P7 only); with `--deep` also
`Deep_*`), **under the assumptions named in each config**. The `*_unbounded*` configs drop the two
timing assumptions the code needed before the fixes (`BoundedPrestart`, `BoundedStale`): state,
status and coalescing are now safe without them.

### Findings (all fixed 2026-11-23)

**F1 - self-fence fired at 100 % of the lease, not at 80 %, when heartbeats failed fast** (P1).
Was: `worker.rs` checked `last_ok.elapsed() >= lease` only after a heartbeat attempt, and attempts
happened every visibility/3 (33 %, 67 %, 100 %). A heartbeat that failed at once (connection
refused, "too many connections") passed the 67 % check, so the runner lived until 100 % while
another worker (clock up to 8 s ahead, or with a slow claim + `mark_started`, because the local
lease start was taken after the claim returned) could already reap and run the task.
**Fix** (`crates/pl-queue/src/worker.rs`, `LeaseTiming` + `JobRunner::run_inner`): a separate
select arm `sleep_until(last_ok + 0.8 V)` aborts the handler independently of heartbeat attempts;
a failed or timed-out heartbeat is retried after V/15 (4 s) instead of V/3; `claimed_at` is taken
together with the claim's `now` (`pg.rs` `PgQueue::claim`), so the local lease start equals the one
written to the row. Model: `SelfFenceTimer`, `AfterHeartbeat` (`HbRetry`), claim sets `lastOk`.
Former configs `Finding_fast_fail_heartbeat`/`_no_skew` are now `Lease_code_fast_fail`/`_no_skew`
(pass); the old behaviour is `Broken_fence_only_after_heartbeat` (fails).

**F2 - `mark_started` was not fenced and the handler started without checking the lease** (P3, P4).
Was: after a pause of a whole lease between `claim` and `mark_started`, another worker could take
and finish the task; the paused runner then set the PaperlessTask back to `started` for ever and ran
the handler a second time. **Fix**: `PgQueue::mark_started` (`pg.rs`) is one transaction = the
heartbeat's fenced lease `UPDATE` (`extend_lease`, row lock) + `paperless_task::mark_started`; it
returns `Heartbeat::Lost` (rollback, nothing written) when another delivery owns or finished the
row, `Terminate` when a terminate-revoke arrived meanwhile (revoked without running). `run_inner`
checks the local lease (`timing.valid`) before and after, runs `mark_started` under
`timeout(budget)`, and starts the handler only on `Ok` (or on a DB error while the claim's lease is
still provable - then nothing was written and the fence timer applies). Model: `MarkStarted`
(`UseFencedStart`), `MarkStartedErr`; Lease: `MarkStartedOk/Err/Late`. Former
`Finding_stale_mark_started` is now `Life_code_unbounded_prestart` (pass, with revoke); old code:
`Broken_unfenced_mark_started` (fails).

**F3 - the fencing token repeated inside one worker process** (P3, P5). Was: the fence
`(attempts, locked_by)` came back after a retry (`attempts = 0`) or release (`attempts - 1`) because
`locked_by` is the process name shared by all job runners; a late `complete()` of an old delivery
was accepted for a newer one (appended coalesced ids lost, newer run discarded). **Fix**: new column
`fence_token` (`crates/pl-queue/migrations/0002_paperrust_task_queue_fence_token.sql`, idempotent,
in `pl_queue::migrations::ALL`), `+1` on every claim, retry and release, never reset; every fenced
`WHERE` (heartbeat/`mark_started` via `extend_lease`, `finish_row` for complete/fail/revoke,
`retry`, `release`) compares `fence_token` instead of `attempts`. `attempts` keeps its meaning for
`MAX_DELIVERIES` and the coalescing condition `attempts = 0`. Model: `q.tok`, `TokenMatches`,
`EndTok` (`UseToken`). Former `Finding_token_reuse_same_process`/`Finding_coalesce_lost_ids` are now
`Life_code_same_process_unbounded`/`Coalesce_code_unbounded` (pass); old code:
`Broken_attempts_token_coalesce` (only the token reverted, fails with `NoLostMerge`) and
`Broken_old_token_same_process` (token and `mark_started` reverted - the original F3 trace).
Note: with the fenced `mark_started` alone, the attempts token no longer breaks `StatusConsistent`
in that config; `NoLostMerge` still catches it.

Each property has been seen **failing** on a broken variant:

| Broken variant (config) | Fails with |
|---|---|
| no self-fence, only `Heartbeat::Lost` aborts (`Broken_no_self_fence`) | `ExclusiveExecution` |
| self-fence checked only after a heartbeat attempt, retry after V/3 - old code, F1 (`Broken_fence_only_after_heartbeat`) | `ExclusiveExecution` |
| claim CTE without `FOR UPDATE SKIP LOCKED` (`Broken_claim_no_row_lock`) | `ExclusiveExecution` |
| clock of host b 16 s ahead, beyond the 20 % margin (`Broken_skew_beyond_margin`; 12 s passes) | `ExclusiveExecution` |
| `mark_started` unfenced, handler starts anyway - old code, F2 (`Broken_unfenced_mark_started`) | `StatusConsistent` |
| fence on `attempts` instead of `fence_token` - old token, F3 (`Broken_attempts_token_coalesce`) | `NoLostMerge` |
| old token and unfenced `mark_started` in one process - original F3 trace (`Broken_old_token_same_process`) | `StatusConsistent` |
| `finish_row`/`retry`/`release` without the fencing `WHERE` (`Broken_finish_no_fence`) | `TerminalOnce` |
| heartbeat without the fencing `WHERE` - a stale heartbeat sets a revoked task back to `running` (`Broken_hb_no_fence`) | `StatusConsistent` |
| reaper without its second statement (no `WorkerLostError`) (`Broken_reaper_no_fail`) | liveness (`Finishes`) |
| beat `run_due` without `FOR UPDATE` (`Beat_broken_no_for_update`) | `NoSlotTwice` |
| beat records `last_run_at` and commits, then enqueues; error only logged - old code before 2026-11-27 (`Beat_broken_record_before_enqueue`) | `NoLostSlot` |
| documented, not a bug: queue without `enqueue_in_conn` (fallback), crash between the separate enqueue and the beat's COMMIT repeats the run (`Beat_fallback_crash_repeats`) | `NoSlotTwice` |

## Run it

```bash
bash verification/task-queue/run.sh          # 24 checks, about 8-12 minutes
bash verification/task-queue/run.sh --deep   # plus deeper bounds (several minutes more)
bash verification/task-queue/run.sh --trace Broken_fence_only_after_heartbeat   # one counterexample
```

Needs Java 11+. `run.sh` downloads TLA+ tools **v1.7.4** (`tla2tools.jar`, SHA-256
`936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`) into `tools/` (gitignored) and
checks the hash. TLC runs with `-Xmx2g` and 2 workers. Exit code 0 = every check behaved as expected.

## The three models

- `QueueLease.tla` - **timed**. One task, two worker processes `a`/`b` (each: select loop with
  claim and reaper, one JobRunner). Time unit = V/15 (4 s at the default 60 s lease): heartbeat every
  5 after a success, retry 1 after a failure, self-fence 12 after the last success, lease 15.
  Heartbeats and `mark_started` succeed, fail at once, or hang (the hanging UPDATE may still
  commit). Host `b`'s clock is `SkewB` ahead (2 = 8 s in the code configs; 3 = 12 s also passes,
  4 = 16 s fails). A DB round trip (claim, `mark_started`, finish) takes up to `MaxDelay`. Bounds:
  time 32 (36 deep), 1 crash (2 deep), no retry, 3 deliveries.
- `QueueLifecycle.tla` - **untimed**. Any step may be delayed, any process may pause, crash
  (restart with a new pid) or shut down gracefully (`release`), and a lease may expire at any time.
  Includes revoke from the API (with and without `terminate`), retry, the reaper with
  `MAX_DELIVERIES`, coalescing, and the PaperlessTask row. Bounds: 2 runners (separate processes or
  one process), 2 deliveries (3 deep), 2 crashes/shutdowns, 1 retry.
- `BeatReserve.tla` - 3 schedulers with clocks up to 1 unit apart, one entry due every 2 units,
  time 4; every interleaving of their `run_due` transactions (SELECT FOR UPDATE, enqueue, UPDATE,
  COMMIT), with enqueue errors (rollback) and crashes at any step. `Mode`: `intx` = code with
  `PgQueue::enqueue_in_conn` (enqueue in the beat's transaction), `fallback` = queue without it
  (separate enqueue while the row lock is held), `old` = record first, enqueue after the commit.

A pass covers the model within these bounds.

## Assumptions

- Every SQL statement / transaction is one atomic step. Each one locks the task row first (UPDATE by
  id, `SELECT ... FOR UPDATE`, or the claim CTE), so PostgreSQL's `READ COMMITTED` runs them one after
  another on that row and re-checks the `WHERE` on the newest row version. `mark_started` touches
  only `documents_paperlesstask`; its final value is the last committed UPDATE in either order.
  Exception: the broken claim without row lock is split in SELECT + UPDATE (then the CTE's
  `state = 'queued'` is not re-checked).
- `QueueLease`: timers fire on time (a tokio interval tick may be late, never early) and DB calls
  return within `MaxDelay`; a process that is paused longer breaks P1 for any lease-based design -
  that is outside the promise.
- `QueueLifecycle` has two optional timing assumptions as constants: `BoundedPrestart` (no pause of
  a whole lease between claim and `mark_started`) and `BoundedStale` (while a runner still holds a
  job, at most one newer delivery is claimed). The fixed code needs neither (`*_unbounded*` configs);
  the bounded configs are kept because they were the baseline before the fixes.
- `mark_started` returning an error is modelled as "nothing committed" (`MarkStartedErr`); a commit
  whose reply is lost is the same as `MarkStarted` succeeding (it was fenced at commit time).
- The reaper may run at any moment (its period is V/2 with an arbitrary phase); `eta`/countdown of
  a retry is not modelled (the task is claimable at once - more interleavings, not fewer).
- Liveness assumes live workers keep working, silent leases expire, and crashed processes are
  restarted (weak fairness); crashes, shutdowns and heartbeats need no fairness.

## Not covered

- Side effects of handlers (delivery is at-least-once; idempotency of each handler is not checked).
- `handle.abort()` only stops the handler at its next `.await`; blocking work in `spawn_blocking`
  or a child process (OCR) keeps running after an abort - P1 assumes the abort stops execution.
- A tokio timer may fire late under heavy load (never early); the self-fence then aborts late. The
  model assumes timers on time (`QueueLease`), as before.
- Rolling upgrade with old and new workers at the same time: an old worker neither bumps nor checks
  `fence_token` (restart all workers together, migration 0002 note).
- Groups/chords (`group_part_done`), `link`/`link_error` contents, `expires` (same path as revoke),
  priorities/queues, `purge_finished` and the status read from `documents_paperlesstask` after purge,
  LISTEN/NOTIFY wake-ups, DB error backoff, `already_pending` in `pl-documents-write/src/index.rs`.
- The beat's time-zone reset branch (two instances with different `PAPERLESS_TIME_ZONE` - a
  misconfiguration; sets `last_run_at = now` on purpose, like Celery), the first-start `INSERT`
  (sets `last_run_at = now`), `MemoryBeatStore::run_due` (single process, ticks sequential).
- Worker duration caps (`secs_or` capped at `MAX_DURATION_SECS`, `checked_add` in `fence_at` /
  `next_after_*`, config limit 1 year) only matter beyond the modelled ratios: within the config
  limits nothing overflows and `fence_at = last_ok + 0.8 V` as modelled; on overflow the fence fires
  at once (safe side for P1). `force_reap` keeps "the reaper may run at any time".
- More than one task row (tasks are independent in the claim; ordering/fairness between tasks is not
  a promise here).
