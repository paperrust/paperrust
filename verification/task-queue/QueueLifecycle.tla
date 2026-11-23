--------------------------- MODULE QueueLifecycle ---------------------------
(***************************************************************************)
(* Untimed model of one task's life in PaperRust's PostgreSQL task queue   *)
(* (crates/pl-queue) and of its PaperlessTask row (what /api/tasks/ shows).*)
(*                                                                         *)
(* No clocks: any step of any runner may be delayed for any time, a        *)
(* process may pause, crash, or be shut down at any point, and a lease may *)
(* expire at any moment (LeaseExpire) - as if heartbeats were late. This   *)
(* is stronger than reality and is the right setting for the promises that *)
(* must hold WITHOUT timing assumptions: the state is written once, the    *)
(* status shown to users matches the queue, and no task is lost.           *)
(* Exclusive execution needs timing and is checked in QueueLease.tla.      *)
(*                                                                         *)
(* Every SQL statement / transaction is one atomic step: each one starts   *)
(* by locking the task row (UPDATE ... WHERE id / SELECT ... FOR UPDATE),  *)
(* so READ COMMITTED serialises them on that row. mark_started is one      *)
(* transaction that first runs the fenced lease UPDATE on the queue row    *)
(* (F2 fix); the old unfenced variant (UseFencedStart = FALSE) touched     *)
(* only documents_paperlesstask, whose final value is the last committed   *)
(* UPDATE in either order.                                                 *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets, TLC

CONSTANTS
    Runners,          \* job runners (tokio JobRunner tasks), e.g. {"a", "b"}
    SameProcess,      \* TRUE: all runners live in ONE worker process (same locked_by name)
    MaxDeliveries,    \* PAPERRUST_QUEUE_MAX_DELIVERIES
    MaxCrashes,       \* crashes + graceful shutdowns (release) in one behaviour
    MaxRetries,       \* task.retry() / autoretry budget
    AllowRevoke,      \* the API may revoke the task once (revoke(terminate=...))
    AllowAppend,      \* coalescing appends ids to the waiting task once (pl-documents-write coalesce.rs)
    BoundedPrestart,  \* assumption: a lease cannot expire between claim and mark_started+spawn
    BoundedStale,     \* assumption: while a runner still holds a Job, at most ONE newer delivery
                      \*   of the task is claimed (its pending DB calls / self-fence are faster than
                      \*   lease expiry + a whole new delivery + retry + another claim)
    UseHbFence,       \* heartbeat WHERE token/locked_by/state (pg.rs PgQueue::extend_lease)
    UseFinishFence,   \* finish_row/retry/release WHERE token/locked_by/state (pg.rs)
    ReaperFails,      \* reap_expired_leases fails jobs with attempts >= max (2nd statement)
    UseToken,         \* fence on fence_token (+1 on claim, retry, release; never reset) - F3 fix;
                      \*   FALSE = old code: fence on attempts (retry sets 0, release lowers it)
    UseFencedStart    \* mark_started = fenced lease UPDATE + PaperlessTask in one transaction,
                      \*   handler only if it matched - F2 fix; FALSE = old unconditional UPDATE

None == <<"none", 0>>
Terminal == {"done", "failed", "revoked"}
PtTerminal == {"success", "failure", "revoked"}
PtOf(s) == CASE s = "done" -> "success" [] s = "failed" -> "failure" [] OTHER -> "revoked"

Host(r) == IF SameProcess THEN "p" ELSE r
Hosts == {Host(r) : r \in Runners}

VARIABLES
    q,          \* paperrust_task_queue row
    pt,         \* documents_paperlesstask.status
    pc,         \* per runner: idle | pre | mark | run | fin | rel | term | dead
    job,        \* per runner: Job snapshot [att, tok, own, retries, revokeReq, snap]
    outcome,    \* per runner: ok | fail | retry
    hinc,       \* per host: incarnation (new pid after a crash -> new worker_name)
    crashes,
    apiDone,    \* the API revoke happened
    terminal,   \* ghost: committed terminal writes
    lostMerge,  \* ghost: a completion that did not contain appended ids
    overtakes   \* per runner: claims of the task by others since this runner's own claim

vars == <<q, pt, pc, job, outcome, hinc, crashes, apiDone, terminal, lostMerge, overtakes>>
Holding == {"pre", "mark", "run", "fin", "rel", "term"}

Name(r) == <<Host(r), hinc[Host(r)]>>      \* worker_name = paperrust@<host>:<pid>
Alive(r) == pc[r] /= "dead"

\* pg.rs fencing WHERE: fence_token = $tok AND ((state='running' AND locked_by=$me) OR state='queued')
\* (old code: attempts = $att instead of the token)
TokenMatches(r) == IF UseToken THEN q.tok = job[r].tok ELSE q.attempts = job[r].att
Fenced(r) == ~UseFinishFence \/
             /\ TokenMatches(r)
             /\ \/ q.state = "running" /\ q.owner = job[r].own
                \/ q.state = "queued"
HbFenced(r) == ~UseHbFence \/
             /\ TokenMatches(r)
             /\ \/ q.state = "running" /\ q.owner = job[r].own
                \/ q.state = "queued"
\* retry/release end the delivery's token (code); old code left attempts as the only token
EndTok == IF UseToken THEN q.tok + 1 ELSE q.tok

Init ==
    \* PgQueue::enqueue: record_pending (PaperlessTask 'pending') + INSERT, one transaction
    /\ q = [state |-> "queued", attempts |-> 0, tok |-> 0, retries |-> 0, owner |-> None, lease |-> FALSE,
            revokeReq |-> FALSE, termReq |-> FALSE, merged |-> FALSE]
    /\ pt = "pending"
    /\ pc = [r \in Runners |-> "idle"]
    /\ job = [r \in Runners |-> [att |-> 0, tok |-> 0, own |-> None, retries |-> 0, revokeReq |-> FALSE, snap |-> FALSE]]
    /\ outcome = [r \in Runners |-> "ok"]
    /\ hinc = [h \in Hosts |-> 0]
    /\ crashes = 0
    /\ apiDone = FALSE
    /\ terminal = 0
    /\ lostMerge = FALSE
    /\ overtakes = [r \in Runners |-> 0]

\* Terminal write of a fenced transaction (complete / fail_in / revoke_job): queue row + PaperlessTask
\* + callbacks/chord accounting in one transaction.
WriteTerminal(st) ==
    /\ q' = [q EXCEPT !.state = st, !.owner = None, !.lease = FALSE]
    /\ pt' = PtOf(st)
    /\ terminal' = terminal + 1

(* pg.rs PgQueue::claim (CTE ... FOR UPDATE SKIP LOCKED, UPDATE ... attempts+1, fence_token+1) *)
Claim(r) ==
    /\ pc[r] = "idle" /\ q.state = "queued"
    /\ BoundedStale => \A s \in Runners \ {r} : pc[s] \in Holding => overtakes[s] < 1
    /\ overtakes' = [s \in Runners |-> IF s = r THEN 0
                                       ELSE IF pc[s] \in Holding THEN overtakes[s] + 1 ELSE overtakes[s]]
    /\ q' = [q EXCEPT !.state = "running", !.attempts = q.attempts + 1, !.tok = q.tok + 1,
                      !.owner = Name(r), !.lease = TRUE]
    /\ job' = [job EXCEPT ![r] = [att |-> q.attempts + 1, tok |-> q.tok + 1, own |-> Name(r), retries |-> q.retries,
                                  revokeReq |-> q.revokeReq, snap |-> q.merged]]
    /\ pc' = [pc EXCEPT ![r] = "pre"]
    /\ UNCHANGED <<pt, outcome, hinc, crashes, apiDone, terminal, lostMerge>>

(* worker.rs run_inner: `if job.revoke_requested { revoke_job(Revoked) }` (expiry is the same path) *)
PreStart(r) ==
    /\ pc[r] = "pre"
    /\ IF job[r].revokeReq
       THEN /\ IF Fenced(r) THEN WriteTerminal("revoked") ELSE UNCHANGED <<q, pt, terminal>>
            /\ pc' = [pc EXCEPT ![r] = "idle"]
       ELSE /\ pc' = [pc EXCEPT ![r] = "mark"]
            /\ UNCHANGED <<q, pt, terminal>>
    /\ UNCHANGED <<job, outcome, hinc, crashes, apiDone, lostMerge, overtakes>>

(* PgQueue::mark_started (F2 fix): one transaction = extend_lease (fenced UPDATE like the heartbeat,
   takes back a row put back by the cleaner) + paperless_task::mark_started; Heartbeat::Lost ->
   rollback, handler not started; Terminate -> revoke_job(Terminated) without running; Ok -> spawn.
   Old code (UseFencedStart = FALSE): UPDATE ... SET status='started' WHERE task_id = $1 without any
   condition, then tokio::spawn. *)
MarkStarted(r) ==
    /\ pc[r] = "mark"
    /\ IF ~UseFencedStart
       THEN /\ pt' = "started"
            /\ pc' = [pc EXCEPT ![r] = "run"]
            /\ UNCHANGED q
       ELSE IF HbFenced(r)
       THEN /\ q' = [q EXCEPT !.state = "running", !.owner = job[r].own, !.lease = TRUE]
            /\ pt' = "started"
            /\ pc' = [pc EXCEPT ![r] = IF q.termReq THEN "term" ELSE "run"]
       ELSE /\ pc' = [pc EXCEPT ![r] = "idle"]
            /\ UNCHANGED <<q, pt>>
    /\ UNCHANGED <<job, outcome, hinc, crashes, apiDone, terminal, lostMerge, overtakes>>

(* mark_started returns an error (nothing committed): the handler starts on the claim's lease
   (worker.rs: only while timing.valid; untimed here); all later writes are fenced. *)
MarkStartedErr(r) ==
    /\ UseFencedStart
    /\ pc[r] = "mark"
    /\ pc' = [pc EXCEPT ![r] = "run"]
    /\ UNCHANGED <<q, pt, job, outcome, hinc, crashes, apiDone, terminal, lostMerge, overtakes>>

(* select loop: handler finished (also TimeLimit / Panic -> fail) *)
HandlerDone(r) ==
    /\ pc[r] = "run"
    /\ \E o \in {"ok", "fail"} \cup (IF job[r].retries < MaxRetries THEN {"retry"} ELSE {}):
          outcome' = [outcome EXCEPT ![r] = o]
    /\ pc' = [pc EXCEPT ![r] = "fin"]
    /\ UNCHANGED <<q, pt, job, hinc, crashes, apiDone, terminal, lostMerge, overtakes>>

(* PgQueue::heartbeat: UPDATE SET state='running', locked_by=$me, locked_until=... WHERE <fence>
   RETURNING terminate_requested. No row -> Heartbeat::Lost -> abort. *)
Heartbeat(r) ==
    /\ pc[r] = "run"
    /\ IF HbFenced(r)
       THEN /\ q' = [q EXCEPT !.state = "running", !.owner = job[r].own, !.lease = TRUE]
            /\ pc' = [pc EXCEPT ![r] = IF q.termReq THEN "term" ELSE "run"]
       ELSE /\ pc' = [pc EXCEPT ![r] = "idle"]
            /\ UNCHANGED q
    /\ UNCHANGED <<pt, job, outcome, hinc, crashes, apiDone, terminal, lostMerge, overtakes>>

(* self-fence: no successful heartbeat for 80 % of the lease -> abort, nothing written *)
SelfFence(r) ==
    /\ pc[r] = "run"
    /\ pc' = [pc EXCEPT ![r] = "idle"]
    /\ UNCHANGED <<q, pt, job, outcome, hinc, crashes, apiDone, terminal, lostMerge, overtakes>>

(* Worker::run shutdown (also the worker-pool restart of pl-server settings_live, same process):
   grace expired -> abort -> PgQueue::release *)
Shutdown(r) ==
    /\ pc[r] = "run" /\ crashes < MaxCrashes
    /\ crashes' = crashes + 1
    /\ pc' = [pc EXCEPT ![r] = "rel"]
    /\ UNCHANGED <<q, pt, job, outcome, hinc, apiDone, terminal, lostMerge, overtakes>>

(* PgQueue::release: SET state='queued', attempts=GREATEST(attempts-1,0), fence_token+1,
   locked_by=NULL WHERE <fence> *)
Release(r) ==
    /\ pc[r] = "rel"
    /\ IF Fenced(r)
       THEN q' = [q EXCEPT !.state = "queued", !.attempts = IF q.attempts > 0 THEN q.attempts - 1 ELSE 0,
                           !.tok = EndTok, !.owner = None, !.lease = FALSE]
       ELSE UNCHANGED q
    /\ pc' = [pc EXCEPT ![r] = "idle"]
    /\ UNCHANGED <<pt, job, outcome, hinc, crashes, apiDone, terminal, lostMerge, overtakes>>

(* complete (done + mark_success + link + group), fail (failed + mark_failure + link_error + group),
   retry (queued, retries+1, attempts=0, fence_token+1; PaperlessTask stays 'started') *)
Finish(r) ==
    /\ pc[r] = "fin"
    /\ IF Fenced(r)
       THEN CASE outcome[r] = "ok" ->
                   /\ WriteTerminal("done")
                   /\ lostMerge' = (lostMerge \/ (q.merged /\ ~job[r].snap))
              [] outcome[r] = "fail" ->
                   WriteTerminal("failed") /\ UNCHANGED lostMerge
              [] OTHER ->
                   /\ q' = [q EXCEPT !.state = "queued", !.retries = q.retries + 1, !.attempts = 0,
                                     !.tok = EndTok, !.owner = None, !.lease = FALSE]
                   /\ UNCHANGED <<pt, terminal, lostMerge>>
       ELSE UNCHANGED <<q, pt, terminal, lostMerge>>
    /\ pc' = [pc EXCEPT ![r] = "idle"]
    /\ UNCHANGED <<job, outcome, hinc, crashes, apiDone, overtakes>>

(* Heartbeat::Terminate -> PgQueue::revoke_job(Terminated) *)
Terminate(r) ==
    /\ pc[r] = "term"
    /\ IF Fenced(r) THEN WriteTerminal("revoked") ELSE UNCHANGED <<q, pt, terminal>>
    /\ pc' = [pc EXCEPT ![r] = "idle"]
    /\ UNCHANGED <<job, outcome, hinc, crashes, apiDone, lostMerge, overtakes>>

(* Time passes without a successful heartbeat: locked_until < now. Under BoundedPrestart not while
   the current owner is still between claim and spawn (needs a pause of a whole lease there). *)
LeaseExpire ==
    /\ q.state = "running" /\ q.lease
    /\ BoundedPrestart =>
         ~\E r \in Runners : pc[r] \in {"pre", "mark"} /\ job[r].att = q.attempts /\ job[r].own = q.owner
    /\ q' = [q EXCEPT !.lease = FALSE]
    /\ UNCHANGED <<pt, pc, job, outcome, hinc, crashes, apiDone, terminal, lostMerge, overtakes>>

(* PgQueue::reap_expired_leases, run by every live worker loop; one transaction:
   1) attempts < max -> queued (attempts unchanged); 2) attempts >= max -> fail_in(WorkerLostError) *)
Reap(r) ==
    /\ Alive(r)
    /\ q.state = "running" /\ ~q.lease
    /\ IF q.attempts < MaxDeliveries
       THEN /\ q' = [q EXCEPT !.state = "queued", !.owner = None]
            /\ UNCHANGED <<pt, terminal>>
       ELSE /\ ReaperFails
            /\ WriteTerminal("failed")
    /\ UNCHANGED <<pc, job, outcome, hinc, crashes, apiDone, lostMerge, overtakes>>

(* PgQueue::revoke_impl: SELECT ... FOR UPDATE; queued -> revoked + mark_revoked;
   running -> revoke_requested = true, terminate_requested |= terminate *)
ApiRevoke ==
    /\ AllowRevoke /\ ~apiDone
    /\ apiDone' = TRUE
    /\ CASE q.state = "queued" ->
              /\ q' = [q EXCEPT !.state = "revoked", !.revokeReq = TRUE]
              /\ pt' = "revoked" /\ terminal' = terminal + 1
         [] q.state = "running" ->
              /\ \E t \in BOOLEAN : q' = [q EXCEPT !.revokeReq = TRUE, !.termReq = q.termReq \/ t]
              /\ UNCHANGED <<pt, terminal>>
         [] OTHER -> UNCHANGED <<q, pt, terminal>>
    /\ UNCHANGED <<pc, job, outcome, hinc, crashes, lostMerge, overtakes>>

(* pl-documents-write coalesce.rs MERGE_SQL: append ids to a waiting bulk_update_documents task:
   state='queued' AND attempts=0 AND NOT revoke_requested ... FOR UPDATE SKIP LOCKED *)
Append ==
    /\ AllowAppend /\ ~q.merged
    /\ q.state = "queued" /\ q.attempts = 0 /\ ~q.revokeReq /\ ~q.termReq
    /\ q' = [q EXCEPT !.merged = TRUE]
    /\ UNCHANGED <<pt, pc, job, outcome, hinc, crashes, apiDone, terminal, lostMerge, overtakes>>

(* process crash: every runner of that host stops; restart gets a new pid -> new worker_name *)
Crash(h) ==
    /\ \E r \in Runners : Host(r) = h /\ Alive(r)
    /\ crashes < MaxCrashes
    /\ crashes' = crashes + 1
    /\ pc' = [r \in Runners |-> IF Host(r) = h THEN "dead" ELSE pc[r]]
    /\ UNCHANGED <<q, pt, job, outcome, hinc, apiDone, terminal, lostMerge, overtakes>>

Restart(h) ==
    /\ \A r \in Runners : Host(r) = h => pc[r] = "dead"
    /\ \E r \in Runners : Host(r) = h
    /\ pc' = [r \in Runners |-> IF Host(r) = h THEN "idle" ELSE pc[r]]
    /\ hinc' = [hinc EXCEPT ![h] = hinc[h] + 1]
    /\ UNCHANGED <<q, pt, job, outcome, crashes, apiDone, terminal, lostMerge, overtakes>>

Progress ==
    \/ \E r \in Runners : Claim(r) \/ PreStart(r) \/ MarkStarted(r) \/ HandlerDone(r)
                          \/ Release(r) \/ Finish(r) \/ Terminate(r) \/ Reap(r)
    \/ LeaseExpire
    \/ \E h \in Hosts : Restart(h)

Next ==
    \/ Progress
    \/ \E r \in Runners : Heartbeat(r) \/ SelfFence(r) \/ Shutdown(r) \/ MarkStartedErr(r)
    \/ ApiRevoke \/ Append
    \/ \E h \in Hosts : Crash(h)
    \/ (q.state \in Terminal /\ \A r \in Runners : pc[r] \in {"idle", "dead"} /\ UNCHANGED vars)

\* Fairness: a live worker keeps working (claims, finishes, reaps), leases of silent owners expire,
\* crashed processes are restarted (systemd/docker). Crashes, shutdowns, heartbeats, revokes need
\* no fairness.
Fairness ==
    /\ \A r \in Runners : WF_vars(Claim(r)) /\ WF_vars(PreStart(r)) /\ WF_vars(MarkStarted(r))
                          /\ WF_vars(HandlerDone(r)) /\ WF_vars(Release(r)) /\ WF_vars(Finish(r))
                          /\ WF_vars(Terminate(r)) /\ WF_vars(Reap(r))
    /\ WF_vars(LeaseExpire)
    /\ \A h \in Hosts : WF_vars(Restart(h))

Spec == Init /\ [][Next]_vars
FairSpec == Spec /\ Fairness

(* ---------------- properties ----------------------------------------- *)
\* The terminal state (with link/link_error callbacks, chord accounting and the PaperlessTask result)
\* is committed exactly once at most ("exactly once for state and PaperlessTask", contract B3.8).
TerminalOnce == terminal <= 1

\* What /api/tasks/ shows matches the queue: a finished task shows its real result, and a
\* PaperlessTask result (success/failure/revoked) exists only for a finished task.
StatusConsistent ==
    /\ q.state \in Terminal => pt = PtOf(q.state)
    /\ pt \in PtTerminal => q.state \in Terminal

\* Ids appended to a waiting task are processed by the delivery that completes it.
NoLostMerge == ~lostMerge

\* Liveness: the task is never lost - it ends finished, and the PaperlessTask ends with its result
\* (no eternal 'pending'/'started').
Finishes == <>[](q.state \in Terminal)
StatusSettles == <>[](pt = PtOf(q.state) /\ q.state \in Terminal)

TypeOK ==
    /\ q.state \in {"queued", "running"} \cup Terminal
    /\ pt \in {"pending", "started"} \cup PtTerminal
    /\ pc \in [Runners -> {"idle", "pre", "mark", "run", "fin", "rel", "term", "dead"}]
=============================================================================
