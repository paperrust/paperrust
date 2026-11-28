---------------------------- MODULE BeatReserve ----------------------------
(***************************************************************************)
(* Periodic tasks (celery beat replacement) with several schedulers.       *)
(* crates/pl-queue/src/beat.rs: every process that runs a worker also runs *)
(* a Beat (pl-server worker.rs, "scheduler"); all share paperrust_beat_state.*)
(* Beat::tick_report calls BeatStore::run_due once per entry and round.    *)
(* PgBeatStore::run_due, one transaction:                                  *)
(*   INSERT ... ON CONFLICT (name) DO NOTHING                              *)
(*   SELECT last_run_at, tz ... WHERE name = $1 FOR UPDATE                 *)
(*   is_due(last, now); not due -> COMMIT                                  *)
(*   due: TaskQueue::enqueue_in_conn(&mut tx, ..) (PgQueue: record_pending *)
(*        + INSERT paperrust_task_queue in the SAME transaction);          *)
(*        Err -> ROLLBACK (nothing recorded, due again next tick)          *)
(*        Unsupported -> queue.enqueue(..) as its own transaction while    *)
(*        the row lock is held ("fallback")                                *)
(*   UPDATE ... SET last_run_at = now, total_run_count + 1; COMMIT         *)
(* Old code (Mode = "old", before 2026-11-27): reserve() recorded          *)
(* last_run_at and committed, then tick_report enqueued; an enqueue error  *)
(* was only logged.                                                        *)
(* Each scheduler uses its own clock (Utc::now() of its host). A process   *)
(* may crash at any point (open transaction rolled back).                  *)
(*                                                                         *)
(* One entry, row already present (the first-start INSERT sets            *)
(* last_run_at = now, so it cannot make a slot due - not modelled; nor the *)
(* deliberate time-zone reset, which sets last_run_at = now like Celery).  *)
(***************************************************************************)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Schedulers,   \* e.g. {"s1", "s2", "s3"}
    SkewMax,      \* each scheduler's clock is ahead by 0..SkewMax units (chosen at start)
    Period,       \* the entry is due every Period units (crontab slots k*Period)
    MaxT,         \* time bound
    UseForUpdate, \* SELECT ... FOR UPDATE in run_due
    Mode,         \* "intx" (PgBeatStore + PgQueue: code), "fallback" (queue without enqueue_in_conn),
                  \* "old" (record + commit, then enqueue after the commit)
    EnqFail,      \* an enqueue may fail (DB error, queue down)
    Crash         \* a scheduler process may crash at any step (any number of times)

ASSUME Mode \in {"intx", "fallback", "old"}

Slots == {k \in 1..(MaxT + SkewMax) : k % Period = 0}
\* Crontab::is_due(last, now): the next slot after last_run_at is <= now
Covered(last, n) == {k \in Slots : last < k /\ k <= n}

VARIABLES
    t,        \* true time
    skew,     \* per scheduler clock offset
    last,     \* committed last_run_at
    lock,     \* row lock holder ("none" or a scheduler) - held until COMMIT/ROLLBACK
    pend,     \* uncommitted last_run_at written by the lock holder (or -1)
    pc,       \* idle | sel | enq | upd | commit | post
    myNow,    \* the scheduler's `now` of this round
    seen,     \* last_run_at read by the SELECT
    penq,     \* slots of an enqueue written inside the open transaction (intx), committed with it
    cover     \* ghost: how often each slot was enqueued (committed task rows), capped at 2

vars == <<t, skew, last, lock, pend, pc, myNow, seen, penq, cover>>

Init ==
    /\ t = 0
    /\ skew \in [Schedulers -> 0..SkewMax]
    /\ last = 0
    /\ lock = "none"
    /\ pend = -1
    /\ pc = [s \in Schedulers |-> "idle"]
    /\ myNow = [s \in Schedulers |-> 0]
    /\ seen = [s \in Schedulers |-> 0]
    /\ penq = [s \in Schedulers |-> {}]
    /\ cover = [k \in Slots |-> 0]

Due(s) == Covered(seen[s], myNow[s])
\* capped at 2: enough for both properties, keeps the state space finite when crashes repeat runs
AddCover(S) == cover' = [k \in Slots |-> IF k \in S THEN IF cover[k] < 2 THEN cover[k] + 1 ELSE 2 ELSE cover[k]]

\* ROLLBACK of s's open transaction (enqueue error or crash): row lock and own writes dropped
RollbackOf(s) ==
    /\ IF lock = s THEN lock' = "none" /\ pend' = -1 ELSE UNCHANGED <<lock, pend>>
    /\ penq' = [penq EXCEPT ![s] = {}]

\* BEGIN; INSERT ... ON CONFLICT DO NOTHING (row exists: no-op); now = Utc::now()
Begin(s) ==
    /\ pc[s] = "idle"
    /\ myNow' = [myNow EXCEPT ![s] = t + skew[s]]
    /\ pc' = [pc EXCEPT ![s] = "sel"]
    /\ UNCHANGED <<t, skew, last, lock, pend, seen, penq, cover>>

\* SELECT last_run_at ... FOR UPDATE: waits for the row lock, then reads the newest committed
\* row (READ COMMITTED). Without FOR UPDATE: plain read of the committed value, no lock.
\* is_due: not due -> COMMIT; due -> enqueue (code) / UPDATE (old reserve)
Select(s) ==
    /\ pc[s] = "sel"
    /\ IF UseForUpdate
       THEN /\ lock = "none"
            /\ lock' = s
       ELSE UNCHANGED lock
    /\ seen' = [seen EXCEPT ![s] = last]
    /\ pc' = [pc EXCEPT ![s] = IF Covered(last, myNow[s]) = {} THEN "commit"
                               ELSE IF Mode = "old" THEN "upd" ELSE "enq"]
    /\ UNCHANGED <<t, skew, last, pend, myNow, penq, cover>>

\* run_due: queue.enqueue_in_conn(&mut tx, ..) (intx: rows written in our transaction, visible
\* with our COMMIT) or, on Unsupported, queue.enqueue(..) committed on its own (fallback).
EnqOk(s) ==
    /\ pc[s] = "enq"
    /\ IF Mode = "intx"
       THEN penq' = [penq EXCEPT ![s] = Due(s)] /\ UNCHANGED cover
       ELSE AddCover(Due(s)) /\ UNCHANGED penq
    /\ pc' = [pc EXCEPT ![s] = "upd"]
    /\ UNCHANGED <<t, skew, last, lock, pend, myNow, seen>>

\* enqueue error -> `let _ = tx.rollback()`, return Ok((Some(Err), rem)): nothing recorded
EnqErr(s) ==
    /\ EnqFail
    /\ pc[s] = "enq"
    /\ RollbackOf(s)
    /\ pc' = [pc EXCEPT ![s] = "idle"]
    /\ UNCHANGED <<t, skew, last, myNow, seen, cover>>

\* UPDATE ... SET last_run_at = now (, total_run_count + 1) WHERE name = $3 (row lock; waits if
\* another tx holds it, then re-checks only `name = $3`)
Update(s) ==
    /\ pc[s] = "upd"
    /\ lock \in {"none", s}
    /\ lock' = s
    /\ pend' = myNow[s]
    /\ pc' = [pc EXCEPT ![s] = "commit"]
    /\ UNCHANGED <<t, skew, last, myNow, seen, penq, cover>>

\* COMMIT (releases the lock): last_run_at and (intx) the task row become visible together.
\* Old code: if it was due, tick_report enqueues afterwards ("post").
Commit(s) ==
    /\ pc[s] = "commit"
    /\ IF lock = s
       THEN /\ last' = IF pend >= 0 THEN pend ELSE last
            /\ lock' = "none" /\ pend' = -1
       ELSE UNCHANGED <<last, lock, pend>>
    /\ AddCover(penq[s])
    /\ penq' = [penq EXCEPT ![s] = {}]
    /\ pc' = [pc EXCEPT ![s] = IF Mode = "old" /\ Due(s) # {} THEN "post" ELSE "idle"]
    /\ UNCHANGED <<t, skew, myNow, seen>>

\* old code: tick_report: self.queue.enqueue(..) after the reservation was committed;
\* an error was only logged ("the run stays reserved, like Celery")
PostOk(s) ==
    /\ pc[s] = "post"
    /\ AddCover(Due(s))
    /\ pc' = [pc EXCEPT ![s] = "idle"]
    /\ UNCHANGED <<t, skew, last, lock, pend, myNow, seen, penq>>

PostErr(s) ==
    /\ EnqFail
    /\ pc[s] = "post"
    /\ pc' = [pc EXCEPT ![s] = "idle"]
    /\ UNCHANGED <<t, skew, last, lock, pend, myNow, seen, penq, cover>>

\* process crash anywhere in a round: PostgreSQL rolls the open transaction back
CrashS(s) ==
    /\ Crash
    /\ pc[s] # "idle"
    /\ RollbackOf(s)
    /\ pc' = [pc EXCEPT ![s] = "idle"]
    /\ UNCHANGED <<t, skew, last, myNow, seen, cover>>

Tick ==
    /\ t < MaxT
    /\ t' = t + 1
    /\ UNCHANGED <<skew, last, lock, pend, pc, myNow, seen, penq, cover>>

Done == t = MaxT /\ \A s \in Schedulers : pc[s] = "idle" /\ UNCHANGED vars

Next == Tick \/ Done \/ \E s \in Schedulers :
            Begin(s) \/ Select(s) \/ EnqOk(s) \/ EnqErr(s) \/ Update(s) \/ Commit(s)
            \/ PostOk(s) \/ PostErr(s) \/ CrashS(s)

Spec == Init /\ [][Next]_vars

\* Each due time of a periodic task is enqueued at most once across all schedulers.
NoSlotTwice == \A k \in Slots : cover[k] <= 1

\* A due time that is recorded as run (committed last_run_at >= k) was really enqueued: no
\* scheduled run is lost by an enqueue error or a crash after the record. (Old code: a scheduler
\* still about to enqueue after its commit - pc = "post" - does not count as a loss yet.)
NoLostSlot == \A k \in Slots :
                (k <= last /\ ~\E s \in Schedulers : pc[s] = "post" /\ k \in Due(s)) => cover[k] >= 1

TypeOK == /\ lock \in Schedulers \cup {"none"}
          /\ pc \in [Schedulers -> {"idle", "sel", "enq", "upd", "commit", "post"}]
=============================================================================
