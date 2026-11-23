---------------------------- MODULE BeatReserve ----------------------------
(***************************************************************************)
(* Periodic tasks (celery beat replacement) with several schedulers.       *)
(* crates/pl-queue/src/beat.rs: every process that runs a worker also runs *)
(* a Beat (pl-server worker.rs, "scheduler"); all share paperrust_beat_state.*)
(* PgBeatStore::reserve, one transaction per entry and round:              *)
(*   INSERT ... ON CONFLICT (name) DO NOTHING                              *)
(*   SELECT last_run_at, tz ... WHERE name = $1 FOR UPDATE                 *)
(*   is_due(last, now)  ->  UPDATE ... SET last_run_at = now               *)
(*   COMMIT; then Beat::tick_report enqueues the task (own transaction).   *)
(* Each scheduler uses its own clock (Utc::now() of its host).             *)
(*                                                                         *)
(* Question: can one due time ("slot") of an entry be enqueued twice?      *)
(* One entry, row already present (the first-start INSERT sets            *)
(* last_run_at = now, so it cannot make a slot due - not modelled).        *)
(***************************************************************************)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Schedulers,   \* e.g. {"s1", "s2", "s3"}
    SkewMax,      \* each scheduler's clock is ahead by 0..SkewMax units (chosen at start)
    Period,       \* the entry is due every Period units (crontab slots k*Period)
    MaxT,         \* time bound
    UseForUpdate  \* SELECT ... FOR UPDATE in reserve (beat.rs PgBeatStore::reserve)

Slots == {k \in 1..(MaxT + SkewMax) : k % Period = 0}
\* Crontab::is_due(last, now): the next slot after last_run_at is <= now
Covered(last, n) == {k \in Slots : last < k /\ k <= n}

VARIABLES
    t,        \* true time
    skew,     \* per scheduler clock offset
    last,     \* committed last_run_at
    lock,     \* row lock holder ("none" or a scheduler) - held until COMMIT
    pend,     \* uncommitted last_run_at written by the lock holder (or -1)
    pc,       \* idle | sel | upd | commit
    myNow,    \* the scheduler's `now` of this round
    seen,     \* last_run_at read by the SELECT
    cover     \* ghost: how often each slot was enqueued

vars == <<t, skew, last, lock, pend, pc, myNow, seen, cover>>

Init ==
    /\ t = 0
    /\ skew \in [Schedulers -> 0..SkewMax]
    /\ last = 0
    /\ lock = "none"
    /\ pend = -1
    /\ pc = [s \in Schedulers |-> "idle"]
    /\ myNow = [s \in Schedulers |-> 0]
    /\ seen = [s \in Schedulers |-> 0]
    /\ cover = [k \in Slots |-> 0]

\* BEGIN; INSERT ... ON CONFLICT DO NOTHING (row exists: no-op); now = Utc::now()
Begin(s) ==
    /\ pc[s] = "idle"
    /\ myNow' = [myNow EXCEPT ![s] = t + skew[s]]
    /\ pc' = [pc EXCEPT ![s] = "sel"]
    /\ UNCHANGED <<t, skew, last, lock, pend, seen, cover>>

\* SELECT last_run_at ... FOR UPDATE: waits for the row lock, then reads the newest committed
\* row (READ COMMITTED). Without FOR UPDATE: plain read of the committed value, no lock.
Select(s) ==
    /\ pc[s] = "sel"
    /\ IF UseForUpdate
       THEN /\ lock = "none"
            /\ lock' = s
       ELSE UNCHANGED lock
    /\ seen' = [seen EXCEPT ![s] = last]
    /\ pc' = [pc EXCEPT ![s] = IF Covered(last, myNow[s]) /= {} THEN "upd" ELSE "commit"]
    /\ UNCHANGED <<t, skew, last, pend, myNow, cover>>

\* UPDATE ... SET last_run_at = now WHERE name = $3 (row lock; waits if another tx holds it,
\* then re-checks only `name = $3`)
Update(s) ==
    /\ pc[s] = "upd"
    /\ lock \in {"none", s}
    /\ lock' = s
    /\ pend' = myNow[s]
    /\ pc' = [pc EXCEPT ![s] = "commit"]
    /\ UNCHANGED <<t, skew, last, myNow, seen, cover>>

\* COMMIT (releases the lock); if it was due, tick_report enqueues the task afterwards.
Commit(s) ==
    /\ pc[s] = "commit"
    /\ IF lock = s
       THEN /\ last' = IF pend >= 0 THEN pend ELSE last
            /\ lock' = "none" /\ pend' = -1
       ELSE UNCHANGED <<last, lock, pend>>
    /\ LET due == Covered(seen[s], myNow[s]) IN
       cover' = [k \in Slots |-> IF k \in due THEN cover[k] + 1 ELSE cover[k]]
    /\ pc' = [pc EXCEPT ![s] = "idle"]
    /\ UNCHANGED <<t, skew, myNow, seen>>

Tick ==
    /\ t < MaxT
    /\ t' = t + 1
    /\ UNCHANGED <<skew, last, lock, pend, pc, myNow, seen, cover>>

Done == t = MaxT /\ \A s \in Schedulers : pc[s] = "idle" /\ UNCHANGED vars

Next == Tick \/ Done \/ \E s \in Schedulers : Begin(s) \/ Select(s) \/ Update(s) \/ Commit(s)

Spec == Init /\ [][Next]_vars

\* Each due time of a periodic task is enqueued at most once across all schedulers.
NoSlotTwice == \A k \in Slots : cover[k] <= 1

TypeOK == lock \in Schedulers \cup {"none"} /\ pc \in [Schedulers -> {"idle", "sel", "upd", "commit"}]
=============================================================================
