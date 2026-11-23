---------------------------- MODULE QueueLease ----------------------------
(***************************************************************************)
(* Timed model of the lease / heartbeat / self-fencing protocol of        *)
(* PaperRust's PostgreSQL task queue (crates/pl-queue).                    *)
(*                                                                         *)
(* Question: can one task be EXECUTED by two job runners at the same time? *)
(* docs/contract/pl-queue.md B3.8 promises "at most one worker executes a  *)
(* job at a time, as long as the clocks of the worker hosts differ by no   *)
(* more than 20 % of the lease".                                           *)
(*                                                                         *)
(* One task row of paperrust_task_queue, several job runners (each runner *)
(* stands for a worker process: its select loop - claim + reaper - and one *)
(* JobRunner tokio task). Time is discrete; 1 unit = V/15 (4 s with the    *)
(* default PAPERRUST_QUEUE_VISIBILITY_TIMEOUT = 60 s), so V/3 = 5 units    *)
(* (heartbeat interval), 0.8 V = 12 units (self-fence) and V/15 = 1 unit   *)
(* (heartbeat retry after a failed attempt).                               *)
(*                                                                         *)
(* Every SQL statement / transaction is one atomic step (each takes the    *)
(* task row lock first, so READ COMMITTED serialises them on that row).    *)
(* Exception: the claim without its row lock (UseClaimLock = FALSE) is two *)
(* steps, because then the CTE's state = 'queued' is not re-checked.       *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS
    Runners,        \* job runners = worker processes, e.g. {"a", "b"}
    SkewB,          \* how far host "b"'s Utc::now() is ahead of the others (units)
    V,              \* lease = visibility_timeout_secs (15 units)
    HB,             \* heartbeat period after a success = visibility/3 (worker.rs LeaseTiming.every)
    HbRetry,        \* next attempt after a failed one = visibility/15 (LeaseTiming.retry)
    Fence,          \* self-fence = visibility*0.8 after last_ok (LeaseTiming.fence)
    MaxDelay,       \* max units one DB round trip (claim, mark_started, finish) may take
    MaxTime,        \* time bound of the model
    MaxDeliveries,  \* PAPERRUST_QUEUE_MAX_DELIVERIES
    MaxCrashes,     \* process crashes in one behaviour
    MaxRetries,     \* task.retry() calls in one behaviour
    HbFailFast,     \* a heartbeat may return an error at once (connection refused/reset, ...)
    HbHang,         \* a heartbeat may hang until the timeout(budget) fires
    UseClaimLock,   \* claim CTE has FOR UPDATE SKIP LOCKED   (pg.rs PgQueue::claim)
    UseHbFence,     \* heartbeat WHERE attempts/locked_by     (pg.rs PgQueue::heartbeat)
    UseSelfFence,   \* abort when the lease is not provable any more (worker.rs JobRunner::run_inner)
    UseFenceTimer   \* the self-fence has its own timer `sleep_until(last_ok + fence)` (F1 fix);
                    \*   FALSE = old code: checked only after a heartbeat attempt

None == <<"none", 0>>
Terminal == {"done", "failed", "revoked"}
Never == MaxTime + 100

VARIABLES
    now,        \* global (true) time
    q,          \* the task row: [state, attempts, tok (fence_token), retries, owner, until]
    inc,        \* process incarnation per runner (new pid after restart -> new locked_by)
    crashes,
    pc,         \* per runner: idle | csel | claimed | marking | running | hb | finish | dead
    job,        \* per runner: the Job it holds: [att, tok, own, retries]
    claimedAt,  \* Job::claimed_at (local Instant taken with the claim's `now`, i.e. lease start)
    lastOk,     \* worker.rs `last_ok` (send time of the last successful lease write)
    nextTick,   \* worker.rs `next_hb` (sleep_until arm of the select loop)
    hbDue,      \* when the hanging heartbeat's timeout(budget) fires
    stepDue,    \* latest time the pending DB round trip returns (bounded delay)
    outcome,    \* handler result: ok | fail | retry
    exec,       \* the handler future of this runner is running (side effects happen)
    terminal    \* ghost: number of committed terminal writes

vars == <<now, q, inc, crashes, pc, job, claimedAt, lastOk, nextTick, hbDue, stepDue, outcome, exec, terminal>>

Me(r) == <<r, inc[r]>>
Skew(r) == IF r = "b" THEN SkewB ELSE 0
Clock(r) == now + Skew(r)                 \* Utc::now() on r's host

\* The fencing WHERE clause shared by extend_lease (heartbeat, mark_started)/finish_row/retry/release
\* (pg.rs): id = $ AND fence_token = $tok AND ((state='running' AND locked_by=$me) OR state='queued')
Fenced(r) == /\ q.tok = job[r].tok
             /\ \/ q.state = "running" /\ q.owner = job[r].own
                \/ q.state = "queued"

Init ==
    /\ now = 0
    /\ q = [state |-> "queued", attempts |-> 0, tok |-> 0, retries |-> 0, owner |-> None, until |-> 0]
    /\ inc = [r \in Runners |-> 0]
    /\ crashes = 0
    /\ pc = [r \in Runners |-> "idle"]
    /\ job = [r \in Runners |-> [att |-> 0, tok |-> 0, own |-> None, retries |-> 0]]
    /\ claimedAt = [r \in Runners |-> 0]
    /\ lastOk = [r \in Runners |-> 0]
    /\ nextTick = [r \in Runners |-> Never]
    /\ hbDue = [r \in Runners |-> Never]
    /\ stepDue = [r \in Runners |-> Never]
    /\ outcome = [r \in Runners |-> "ok"]
    /\ exec = [r \in Runners |-> FALSE]
    /\ terminal = 0

(* ---------------- claim: pg.rs PgQueue::claim ------------------------- *)
\* WITH next AS (SELECT id ... WHERE state='queued' ... FOR UPDATE SKIP LOCKED)
\* UPDATE ... SET state='running', locked_by=$5, locked_until=$1+visibility, attempts=attempts+1,
\* fence_token=fence_token+1. One statement; with the row lock it is atomic on the row.
\* `$1 = Utc::now()` and `claimed_at = Instant::now()` are taken together before the query is sent,
\* so the local lease start (claimed_at, first last_ok) equals the lease start written to the row.
ClaimAtomic(r) ==
    /\ UseClaimLock
    /\ pc[r] = "idle" /\ q.state = "queued"
    /\ q' = [q EXCEPT !.state = "running", !.attempts = q.attempts + 1, !.tok = q.tok + 1,
                      !.owner = Me(r), !.until = Clock(r) + V]
    /\ job' = [job EXCEPT ![r] = [att |-> q.attempts + 1, tok |-> q.tok + 1, own |-> Me(r), retries |-> q.retries]]
    /\ claimedAt' = [claimedAt EXCEPT ![r] = now]
    /\ lastOk' = [lastOk EXCEPT ![r] = now]          \* `let mut last_ok = job.claimed_at`
    /\ pc' = [pc EXCEPT ![r] = "claimed"]
    /\ stepDue' = [stepDue EXCEPT ![r] = now + MaxDelay]
    /\ UNCHANGED <<now, inc, crashes, nextTick, hbDue, outcome, exec, terminal>>

\* Broken variant: no FOR UPDATE in the CTE. The SELECT sees the committed 'queued' row, the
\* UPDATE then waits for a concurrent claimer's row lock and re-checks only `q.id = next.id`.
ClaimSelect(r) ==
    /\ ~UseClaimLock
    /\ pc[r] = "idle" /\ q.state = "queued"
    /\ pc' = [pc EXCEPT ![r] = "csel"]
    /\ stepDue' = [stepDue EXCEPT ![r] = now + MaxDelay]
    /\ UNCHANGED <<now, q, inc, crashes, job, claimedAt, lastOk, nextTick, hbDue, outcome, exec, terminal>>

ClaimUpdate(r) ==
    /\ pc[r] = "csel"
    /\ q' = [q EXCEPT !.state = "running", !.attempts = q.attempts + 1, !.tok = q.tok + 1,
                      !.owner = Me(r), !.until = Clock(r) + V]
    /\ job' = [job EXCEPT ![r] = [att |-> q.attempts + 1, tok |-> q.tok + 1, own |-> Me(r), retries |-> q.retries]]
    /\ claimedAt' = [claimedAt EXCEPT ![r] = now]
    /\ lastOk' = [lastOk EXCEPT ![r] = now]
    /\ pc' = [pc EXCEPT ![r] = "claimed"]
    /\ stepDue' = [stepDue EXCEPT ![r] = now + MaxDelay]
    /\ UNCHANGED <<now, inc, crashes, nextTick, hbDue, outcome, exec, terminal>>

\* The claim query returns (the JobRunner is spawned).
ClaimReturn(r) ==
    /\ pc[r] = "claimed"
    /\ pc' = [pc EXCEPT ![r] = "marking"]
    /\ stepDue' = [stepDue EXCEPT ![r] = now + MaxDelay]
    /\ UNCHANGED <<now, q, inc, crashes, job, claimedAt, lastOk, nextTick, hbDue, outcome, exec, terminal>>

\* worker.rs run_inner after the revoke/expiry checks (F2 fix): `timing.valid(last_ok)` checked, then
\* `timeout(budget, mark_started)`: ONE transaction = fenced lease extension (extend_lease, row lock)
\* + PaperlessTask 'started'. Ok -> last_ok = sent, spawn the handler, next_hb = sent + every.
\* Heartbeat::Lost -> not executed, nothing written. (Terminate needs a revoke: not in this model.)
Spawn(r, nt) ==
    /\ exec' = [exec EXCEPT ![r] = TRUE]
    /\ nextTick' = [nextTick EXCEPT ![r] = nt]
    /\ pc' = [pc EXCEPT ![r] = "running"]
    /\ stepDue' = [stepDue EXCEPT ![r] = Never]

NotExecuted(r) ==
    /\ pc' = [pc EXCEPT ![r] = "idle"]
    /\ stepDue' = [stepDue EXCEPT ![r] = Never]
    /\ UNCHANGED <<exec, nextTick>>

MarkStartedOk(r) ==
    /\ pc[r] = "marking"
    /\ now - lastOk[r] < Fence                  \* lease provable before start (else MarkStartedLate)
    /\ IF Fenced(r)
       THEN /\ q' = [q EXCEPT !.state = "running", !.owner = job[r].own, !.until = Clock(r) + V]
            /\ lastOk' = [lastOk EXCEPT ![r] = now]
            /\ Spawn(r, now + HB)
       ELSE /\ UNCHANGED <<q, lastOk>>
            /\ NotExecuted(r)
    /\ UNCHANGED <<now, inc, crashes, job, claimedAt, hbDue, outcome, terminal>>

\* mark_started returns an error at once (no DB effect): the claim's lease still counts, the handler
\* starts, the next heartbeat comes after `retry`; the fence timer stops it if none succeeds.
MarkStartedErr(r) ==
    /\ HbFailFast
    /\ pc[r] = "marking"
    /\ now - lastOk[r] < Fence
    /\ Spawn(r, now + HbRetry)
    /\ UNCHANGED <<now, q, inc, crashes, job, claimedAt, lastOk, hbDue, outcome, terminal>>

\* Lease no longer provable before start (`!timing.valid(..)` or the mark_started timeout): return.
MarkStartedLate(r) ==
    /\ pc[r] = "marking"
    /\ now - lastOk[r] >= Fence
    /\ NotExecuted(r)
    /\ UNCHANGED <<now, q, inc, crashes, job, claimedAt, lastOk, hbDue, outcome, terminal>>

(* ---------------- the select loop of run_inner ------------------------ *)
\* `r = &mut handle` (also covers the deadline arm: TimeLimit -> fail).
HandlerDone(r) ==
    /\ pc[r] = "running"
    /\ \E o \in {"ok", "fail"} \cup (IF job[r].retries < MaxRetries THEN {"retry"} ELSE {}):
          outcome' = [outcome EXCEPT ![r] = o]
    /\ exec' = [exec EXCEPT ![r] = FALSE]
    /\ pc' = [pc EXCEPT ![r] = "finish"]
    /\ stepDue' = [stepDue EXCEPT ![r] = now + MaxDelay]
    /\ nextTick' = [nextTick EXCEPT ![r] = Never]
    /\ UNCHANGED <<now, q, inc, crashes, job, claimedAt, lastOk, hbDue, terminal>>

Abort(r) ==   \* handle.abort(); Outcome::Lost -> nothing written
    /\ exec' = [exec EXCEPT ![r] = FALSE]
    /\ pc' = [pc EXCEPT ![r] = "idle"]
    /\ nextTick' = [nextTick EXCEPT ![r] = Never]
    /\ hbDue' = [hbDue EXCEPT ![r] = Never]

\* After every heartbeat attempt: `if !timing.valid(last_ok, now) { abort; Lost }`; next_hb =
\* sent + every after a success, now + retry after a failure / timeout.
AfterHeartbeat(r, ok) ==
    LET lo == IF ok THEN now ELSE lastOk[r] IN
    /\ lastOk' = [lastOk EXCEPT ![r] = lo]
    /\ IF UseSelfFence /\ now - lo >= Fence
       THEN Abort(r)
       ELSE /\ nextTick' = [nextTick EXCEPT ![r] = now + (IF ok THEN HB ELSE HbRetry)]
            /\ hbDue' = [hbDue EXCEPT ![r] = Never]
            /\ pc' = [pc EXCEPT ![r] = "running"]
            /\ UNCHANGED exec

\* Self-fence timer arm (F1 fix): `sleep_until(last_ok + fence)` -> abort, Outcome::Lost. Fires
\* independently of heartbeat attempts (while a heartbeat hangs, its timeout(budget) ends at the
\* same deadline -> HbTimeout).
SelfFenceTimer(r) ==
    /\ UseSelfFence /\ UseFenceTimer
    /\ pc[r] = "running" /\ now >= lastOk[r] + Fence
    /\ Abort(r)
    /\ UNCHANGED <<now, q, inc, crashes, job, claimedAt, lastOk, stepDue, outcome, terminal>>

\* `sleep_until(next_hb)` arm with the DB reachable: heartbeat UPDATE (fenced) returns at once.
\* Ok -> last_ok = sent, locked_until = now + visibility; no row -> Heartbeat::Lost -> abort.
HbReachable(r) ==
    /\ pc[r] = "running" /\ now >= nextTick[r]
    /\ now - lastOk[r] < Fence                    \* budget > 0, else timeout(0) (see HbNoBudget)
    /\ IF Fenced(r) \/ ~UseHbFence
       THEN /\ q' = [q EXCEPT !.state = "running", !.owner = job[r].own, !.until = Clock(r) + V]
            /\ AfterHeartbeat(r, TRUE)
       ELSE /\ UNCHANGED q
            /\ Abort(r) /\ UNCHANGED lastOk
    /\ UNCHANGED <<now, inc, crashes, job, claimedAt, stepDue, outcome, terminal>>

\* Heartbeat returns an error at once (Ok(Err(e)) -> warn), no DB effect.
HbError(r) ==
    /\ HbFailFast
    /\ pc[r] = "running" /\ now >= nextTick[r]
    /\ now - lastOk[r] < Fence
    /\ AfterHeartbeat(r, FALSE)
    /\ UNCHANGED <<now, q, inc, crashes, job, claimedAt, stepDue, outcome, terminal>>

\* Budget = lease - last_ok.elapsed() is zero: timeout(0) fires, then the check aborts.
HbNoBudget(r) ==
    /\ pc[r] = "running" /\ now >= nextTick[r]
    /\ now - lastOk[r] >= Fence
    /\ AfterHeartbeat(r, FALSE)
    /\ UNCHANGED <<now, q, inc, crashes, job, claimedAt, stepDue, outcome, terminal>>

\* Heartbeat hangs (DB unreachable, row lock held by someone, ...); the select loop is blocked
\* until timeout(budget) fires.
HbStartHang(r) ==
    /\ HbHang
    /\ pc[r] = "running" /\ now >= nextTick[r]
    /\ now - lastOk[r] < Fence
    /\ pc' = [pc EXCEPT ![r] = "hb"]
    /\ hbDue' = [hbDue EXCEPT ![r] = lastOk[r] + Fence]
    /\ UNCHANGED <<now, q, inc, crashes, job, claimedAt, lastOk, nextTick, stepDue, outcome, exec, terminal>>

\* The hanging UPDATE may still reach the DB and commit (the worker sees only the timeout).
HbLateApply(r) ==
    /\ pc[r] = "hb"
    /\ Fenced(r) \/ ~UseHbFence
    /\ q' = [q EXCEPT !.state = "running", !.owner = job[r].own, !.until = Clock(r) + V]
    /\ UNCHANGED <<now, inc, crashes, pc, job, claimedAt, lastOk, nextTick, hbDue, stepDue, outcome, exec, terminal>>

HbTimeout(r) ==
    /\ pc[r] = "hb" /\ now >= hbDue[r]
    /\ AfterHeartbeat(r, FALSE)
    /\ UNCHANGED <<now, q, inc, crashes, job, claimedAt, stepDue, outcome, terminal>>

(* ---------------- finishing: complete / fail / retry (pg.rs) ---------- *)
Finish(r) ==
    /\ pc[r] = "finish"
    /\ IF Fenced(r)
       THEN CASE outcome[r] = "ok" ->
                   q' = [q EXCEPT !.state = "done", !.owner = None] /\ terminal' = terminal + 1
              [] outcome[r] = "fail" ->
                   q' = [q EXCEPT !.state = "failed", !.owner = None] /\ terminal' = terminal + 1
              [] OTHER ->   \* retry: state queued, retries+1, attempts = 0, fence_token+1
                   q' = [q EXCEPT !.state = "queued", !.retries = q.retries + 1, !.attempts = 0,
                                  !.tok = q.tok + 1, !.owner = None]
                   /\ UNCHANGED terminal
       ELSE UNCHANGED <<q, terminal>>
    /\ pc' = [pc EXCEPT ![r] = "idle"]
    /\ stepDue' = [stepDue EXCEPT ![r] = Never]
    /\ UNCHANGED <<now, inc, crashes, job, claimedAt, lastOk, nextTick, hbDue, outcome, exec>>

(* ---------------- reaper: pg.rs PgQueue::reap_expired_leases ---------- *)
\* Runs in every worker loop every visibility/2 with an arbitrary phase -> may run at any time.
\* Compares locked_until with ITS OWN Utc::now().
Reap(r) ==
    /\ pc[r] /= "dead"
    /\ q.state = "running" /\ q.until < Clock(r)
    /\ IF q.attempts < MaxDeliveries
       THEN q' = [q EXCEPT !.state = "queued", !.owner = None] /\ UNCHANGED terminal
       ELSE q' = [q EXCEPT !.state = "failed", !.owner = None] /\ terminal' = terminal + 1
    /\ UNCHANGED <<now, inc, crashes, pc, job, claimedAt, lastOk, nextTick, hbDue, stepDue, outcome, exec>>

(* ---------------- process crash and restart --------------------------- *)
Crash(r) ==
    /\ pc[r] /= "dead" /\ crashes < MaxCrashes
    /\ crashes' = crashes + 1
    /\ pc' = [pc EXCEPT ![r] = "dead"]
    /\ exec' = [exec EXCEPT ![r] = FALSE]     \* the whole process is gone
    /\ nextTick' = [nextTick EXCEPT ![r] = Never]
    /\ hbDue' = [hbDue EXCEPT ![r] = Never]
    /\ stepDue' = [stepDue EXCEPT ![r] = Never]
    /\ UNCHANGED <<now, q, inc, job, claimedAt, lastOk, outcome, terminal>>

Restart(r) ==
    /\ pc[r] = "dead"
    /\ pc' = [pc EXCEPT ![r] = "idle"]
    /\ inc' = [inc EXCEPT ![r] = inc[r] + 1]
    /\ UNCHANGED <<now, q, crashes, job, claimedAt, lastOk, nextTick, hbDue, stepDue, outcome, exec, terminal>>

(* ---------------- time ------------------------------------------------ *)
\* Time may pass only if no runner has a step that is due now: timers fire on time, and a DB
\* round trip returns within MaxDelay (the bounded-delay assumption; a paused process breaks it).
Urgent(r) ==
    CASE pc[r] \in {"csel", "claimed", "marking", "finish"} -> stepDue[r]
      [] pc[r] = "running" -> IF UseSelfFence /\ UseFenceTimer /\ lastOk[r] + Fence < nextTick[r]
                              THEN lastOk[r] + Fence ELSE nextTick[r]
      [] pc[r] = "hb"      -> hbDue[r]
      [] OTHER             -> Never

\* Pruning (no behaviour lost): when no lease is outstanding and no runner is between steps,
\* letting time pass changes nothing that any action looks at.
Tick ==
    /\ now < MaxTime
    /\ q.state = "running" \/ \E r \in Runners : pc[r] \notin {"idle", "dead"}
    /\ \A r \in Runners : Urgent(r) > now
    /\ now' = now + 1
    /\ UNCHANGED <<q, inc, crashes, pc, job, claimedAt, lastOk, nextTick, hbDue, stepDue, outcome, exec, terminal>>

Next ==
    \/ Tick
    \/ \E r \in Runners :
         \/ ClaimAtomic(r) \/ ClaimSelect(r) \/ ClaimUpdate(r) \/ ClaimReturn(r)
         \/ MarkStartedOk(r) \/ MarkStartedErr(r) \/ MarkStartedLate(r) \/ HandlerDone(r) \/ SelfFenceTimer(r)
         \/ HbReachable(r) \/ HbError(r) \/ HbNoBudget(r) \/ HbStartHang(r) \/ HbLateApply(r) \/ HbTimeout(r)
         \/ Finish(r) \/ Reap(r) \/ Crash(r) \/ Restart(r)

Spec == Init /\ [][Next]_vars

(* ---------------- properties ----------------------------------------- *)
\* P1: never two runners executing the task's handler at the same time.
ExclusiveExecution == Cardinality({r \in Runners : exec[r]}) <= 1

\* Terminal state (and with it callbacks, chord accounting, PaperlessTask result) written once.
TerminalOnce == terminal <= 1

TypeOK ==
    /\ now \in 0..MaxTime
    /\ q.state \in {"queued", "running"} \cup Terminal
    /\ pc \in [Runners -> {"idle", "csel", "claimed", "marking", "running", "hb", "finish", "dead"}]
    /\ exec \in [Runners -> BOOLEAN]
=============================================================================
