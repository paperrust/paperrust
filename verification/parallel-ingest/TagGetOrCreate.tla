--------------------------- MODULE TagGetOrCreate ---------------------------
(***************************************************************************)
(* Concurrent db::get_or_create_tag calls (barcode tags, sub-directories   *)
(* as tags) for names that differ only in case.                            *)
(*                                                                         *)
(* Code: crates/pl-consumer/src/db.rs                                      *)
(*   get_or_create_tag        BEGIN; SELECT pg_advisory_xact_lock(          *)
(*                            hashtextextended('paperrust:tag:'||upper($1)));*)
(*                            get_or_create_tag_locked; COMMIT              *)
(*   get_or_create_tag_locked SELECT id ... WHERE upper(name)=upper($1)     *)
(*                            ORDER BY id LIMIT 1; else INSERT ... RETURNING *)
(* Callers: consumer.rs Consumer::barcode (tag barcodes),                  *)
(*          watcher.rs (CONSUMER_SUBDIRS_AS_TAGS) - each on its own pool    *)
(*          connection, i.e. a top-level transaction (no savepoint).       *)
(*                                                                         *)
(* Schema (upstream documents/models.py MatchingModel.Meta): UNIQUE(name,  *)
(* owner) and UNIQUE(name) WHERE owner IS NULL - case-SENSITIVE. New tags  *)
(* here have owner NULL, so the exact same name clashes (23505), a name    *)
(* differing in case does not.                                             *)
(*                                                                         *)
(* A name is modelled as <<word, spelling>>: upper(name) = word, and two   *)
(* names are equal (case-sensitive) iff both parts are equal.              *)
(***************************************************************************)
EXTENDS Integers, FiniteSets

CONSTANTS
    N,               \* concurrent callers
    Words,           \* upper(name) values
    Spellings,       \* case variants of a word, e.g. {"lower", "UPPER"}
    UseTagLock,      \* the advisory lock on upper(name)   (code: TRUE)
    RepeatableRead   \* broken variant: REPEATABLE READ    (code: FALSE)

Callers == 1..N
Names   == Words \X Spellings
Up(n)   == n[1]
None    == [none |-> TRUE]
NoSnap  == [taken |-> FALSE, rows |-> {}]
NoOne   == 0
Running == 0                \* result while the call has not returned
RawErr  == -1               \* result = the call failed with 23505

VARIABLES pc, name, result, committed, pending, locks, snap
vars == <<pc, name, result, committed, pending, locks, snap>>

\* A documents_tag row: id = the caller that inserted it (ids are unique).
TagRow(id, n) == [id |-> id, name |-> n]

Own(c) == IF pending[c] # None THEN {pending[c]} ELSE {}
VisibleNow(c) == (IF RepeatableRead /\ snap[c].taken THEN snap[c].rows ELSE committed) \cup Own(c)
TakeSnap(c) == snap' = [snap EXCEPT ![c] = IF RepeatableRead /\ ~snap[c].taken
                                            THEN [taken |-> TRUE, rows |-> committed] ELSE snap[c]]
ReleaseLocks(c) == [w \in Words |-> IF locks[w] = c THEN NoOne ELSE locks[w]]
Goto(c, l) == pc' = [pc EXCEPT ![c] = l]
MinId(S) == CHOOSE r \in S : \A r2 \in S : r.id <= r2.id

Init ==
    /\ name \in [Callers -> Names]
    /\ pc = [c \in Callers |-> "begin"]
    /\ result = [c \in Callers |-> Running]
    /\ committed = {}
    /\ pending = [c \in Callers |-> None]
    /\ locks = [w \in Words |-> NoOne]
    /\ snap = [c \in Callers |-> NoSnap]

\* sqlx::Connection::begin - BEGIN.
Begin(c) ==
    /\ pc[c] = "begin"
    /\ Goto(c, IF UseTagLock THEN "lockStmt" ELSE "select")
    /\ UNCHANGED <<name, result, committed, pending, locks, snap>>

\* SELECT pg_advisory_xact_lock(...) starts (RR would take its snapshot here) ...
LockStmt(c) ==
    /\ pc[c] = "lockStmt"
    /\ TakeSnap(c)
    /\ Goto(c, "lockWait")
    /\ UNCHANGED <<name, result, committed, pending, locks>>

\* ... and is granted when no other transaction holds upper(name).
LockWait(c) ==
    /\ pc[c] = "lockWait"
    /\ locks[Up(name[c])] = NoOne
    /\ locks' = [locks EXCEPT ![Up(name[c])] = c]
    /\ Goto(c, "select")
    /\ UNCHANGED <<name, result, committed, pending, snap>>

\* get_or_create_tag_locked: SELECT ... upper(name) = upper($1) ORDER BY id LIMIT 1.
\* Found -> return it (COMMIT releases the lock).
Select(c) ==
    /\ pc[c] = "select"
    /\ TakeSnap(c)
    /\ LET hits == {r \in VisibleNow(c) : Up(r.name) = Up(name[c])}
       IN IF hits # {}
          THEN /\ result' = [result EXCEPT ![c] = MinId(hits).id]
               /\ locks' = ReleaseLocks(c)
               /\ Goto(c, "done")
          ELSE /\ Goto(c, "insert")
               /\ UNCHANGED <<result, locks>>
    /\ UNCHANGED <<name, committed, pending>>

\* INSERT INTO documents_tag ... RETURNING id. The partial unique index on name waits for an
\* uncommitted insert of the same exact name and raises 23505 against a committed one.
Insert(c) ==
    /\ pc[c] = "insert"
    /\ ~\E d \in Callers \ {c} : pending[d] # None /\ pending[d].name = name[c]   \* blocks
    /\ IF \E r \in committed : r.name = name[c]
          THEN /\ result' = [result EXCEPT ![c] = RawErr]   \* Err -> tx dropped
               /\ locks' = ReleaseLocks(c)
               /\ Goto(c, "done")
               /\ UNCHANGED pending
          ELSE /\ pending' = [pending EXCEPT ![c] = TagRow(c, name[c])]
               /\ Goto(c, "commit")
               /\ UNCHANGED <<result, locks>>
    /\ UNCHANGED <<name, committed, snap>>

\* tx.commit()
Commit(c) ==
    /\ pc[c] = "commit"
    /\ committed' = committed \cup {pending[c]}
    /\ pending' = [pending EXCEPT ![c] = None]
    /\ locks' = ReleaseLocks(c)
    /\ result' = [result EXCEPT ![c] = c]
    /\ Goto(c, "done")
    /\ UNCHANGED <<name, snap>>

Finished == (\A c \in Callers : pc[c] = "done") /\ UNCHANGED vars

Next == (\E c \in Callers : Begin(c) \/ LockStmt(c) \/ LockWait(c) \/ Select(c)
                             \/ Insert(c) \/ Commit(c))
        \/ Finished

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
\* P4: at most one tag per case-insensitive name (what `name__iexact` get_or_create promises).
OneTagPerName == \A r1, r2 \in committed : r1 # r2 => Up(r1.name) # Up(r2.name)

\* P4b: no caller gets a raw unique-constraint error ("Failed to find or create TAG").
NoRawUniqueError == \A c \in Callers : result[c] # RawErr

\* P4c: callers asking for the same case-insensitive name get the same, existing tag id.
SameTagForSameName ==
    \A c, d \in Callers :
        (pc[c] = "done" /\ pc[d] = "done" /\ Up(name[c]) = Up(name[d])
         /\ result[c] \in Callers /\ result[d] \in Callers)
        => /\ result[c] = result[d]
           /\ \E r \in committed : r.id = result[c]

=============================================================================
