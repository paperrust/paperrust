--------------------------- MODULE ParallelIngest ---------------------------
(***************************************************************************)
(* Concurrent consume tasks racing on the same ASN and/or the same file    *)
(* checksum (PaperRust parallel ingest, commit 499c72f0).                   *)
(*                                                                         *)
(* Every action below points at the code it models. One action = one SQL  *)
(* statement (or one COMMIT/ROLLBACK) in the real code; a statement that   *)
(* can block (pg_advisory_xact_lock) is split into "statement starts"      *)
(* (where REPEATABLE READ would take its snapshot) and "lock granted".     *)
(*                                                                         *)
(* Code (crates/pl-consumer/src):                                          *)
(*   consumer.rs  Consumer::preflight        -> Preflight                  *)
(*   consumer.rs  Consumer::asn_check        -> AsnCheck                   *)
(*   consumer.rs  Consumer::store_all        -> Begin, Commit, FailInTx    *)
(*   consumer.rs  Consumer::claim_unique     -> AsnLock*, AsnFind,         *)
(*                                              SumLock*, SumFind          *)
(*   consumer.rs  Consumer::store            -> Insert (db::insert_document)*)
(*                (apply_overrides + docops::save) -> WriteAsn             *)
(*   db.rs        advisory_xact_lock, find_asn, find_duplicates            *)
(*                                                                         *)
(* PostgreSQL semantics relied on (and modelled):                          *)
(*   - READ COMMITTED (server default; the pool sets no isolation level,   *)
(*     pl-core/src/state.rs pg_connect_options): every statement reads     *)
(*     the data committed when that statement starts.                      *)
(*   - pg_advisory_xact_lock: exclusive, blocks, released only at the end  *)
(*     of the transaction (commit or rollback).                            *)
(*   - COMMIT makes the rows visible no later than it releases the locks  *)
(*     (xact.c: ProcArrayEndTransaction before lock release); modelled as  *)
(*     one atomic step.                                                     *)
(*   - documents_document.archive_serial_number is UNIQUE: a write of an   *)
(*     ASN that an uncommitted transaction also wrote waits for it; a      *)
(*     clash with a committed row raises 23505 (raw IntegrityError).       *)
(*   - documents_document.checksum is NOT unique (upstream models.py).     *)
(*                                                                         *)
(* PDF shrinking on ingest (PaperRust, consumer.rs run_consumer, `shrunk`): *)
(* the stored file - and so documents_document.checksum - is a smaller copy *)
(* with another checksum. The delivered file's checksum (st.checksum, the  *)
(* lock key) then goes to paperrust_source_checksum in the same store      *)
(* transaction (source_checksum.rs record, called in store_all right after *)
(* Consumer::store), and db::find_duplicates matches checksum,             *)
(* archive_checksum and that table. A row therefore carries the SET of     *)
(* checksums the lookup matches ("keys") and, as a ghost field for the     *)
(* properties, the checksum of the file that was delivered ("orig").       *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    N,                 \* number of concurrent consume tasks (task slots)
    Asns,              \* ASN values a task may carry; NoAsn = 0 means "no ASN given"
    Sums,              \* checksum values (file contents) a task may carry
    DeleteDuplicates,  \* settings.consumer_delete_duplicates
    \* ---- switches for broken variants (all FALSE/TRUE as in the code by default) ----
    UseAsnLock,        \* claim_unique: advisory_xact_lock("paperrust:asn:<n>")       (code: TRUE)
    UseSumLock,        \* claim_unique: advisory_xact_lock("paperrust:checksum:<s>")  (code: TRUE)
    LockAfterFind,     \* re-check BEFORE taking the lock (reordered)                  (code: FALSE)
    RepeatableRead,    \* transactions run under REPEATABLE READ                        (code: FALSE)
    RecheckAfterInsert,\* claim_unique runs after insert_document instead of first     (code: FALSE)
    UniqueAsnIndex,    \* schema: archive_serial_number UNIQUE                          (code: TRUE)
    ExternalAsnWriter, \* a concurrent API edit sets an ASN without the lock (scope boundary)
    Shrink,            \* PAPERRUST_SHRINK_ON_INGEST_MB on: any task's file may be shrunk  (code configs: TRUE)
    RecordSource       \* store_all records st.checksum in paperrust_source_checksum      (code: TRUE)

NoAsn == 0
Tasks == 1..N
Ext   == 0                                   \* id of the pre-existing doc the API edits
ExtSum == 0                                  \* checksum of that doc (Sums must not contain 0)
None   == [none |-> TRUE]                     \* "no uncommitted row"
NoSnap == [taken |-> FALSE, rows |-> {}]     \* "no snapshot taken yet"
LockKeys == ({"asn"} \X (Asns \ {NoAsn})) \cup ({"sum"} \X Sums)
NoOne == 0

Outcomes == {"running", "Stored", "AsnExists", "Duplicate", "RawUniqueError", "Failed"}

VARIABLES
    pc,         \* [Tasks -> step label]
    asn,        \* [Tasks -> ASN override of the task] (st.metadata.asn)
    sum,        \* [Tasks -> checksum of the original]  (st.checksum)
    shr,        \* [Tasks -> BOOLEAN] the stored file is a shrunk copy (st.shrunk)
    outcome,    \* [Tasks -> Outcomes]
    committed,  \* set of committed documents_document rows [by, asn, sum]
    pending,    \* [Tasks -> uncommitted row of the task's store transaction, or None]
    locks,      \* [LockKeys -> holding task, or NoOne]
    snap,       \* [Tasks -> RR snapshot (set of rows), or NoSnap]
    apiDone     \* the external API writer has run (only with ExternalAsnWriter)

vars == <<pc, asn, sum, shr, outcome, committed, pending, locks, snap, apiDone>>

\* Checksum of the stored file: the original's, or that of the shrunk copy (another value; s + 1
\* may equal another task's original, which covers "someone uploads the shrunk file itself").
StoredSum(t) == IF shr[t] THEN sum[t] + 1 ELSE sum[t]
\* Checksums db::find_duplicates matches for t's row: documents_document.checksum and, with
\* RecordSource, the paperrust_source_checksum row (source_checksum::needs_record: shrunk and
\* different).
Keys(t) == {StoredSum(t)} \cup (IF RecordSource /\ shr[t] /\ StoredSum(t) # sum[t] THEN {sum[t]} ELSE {})
\* Row written by task t (asn = value of archive_serial_number).
Row(t, a) == [by |-> t, asn |-> a, keys |-> Keys(t), orig |-> sum[t]]
\* The pre-existing document the external API writer edits.
ExtRow(a) == [by |-> Ext, asn |-> a, keys |-> {ExtSum}, orig |-> ExtSum]
\* db::find_duplicates(s) finds row d.
Matches(d, s) == s \in d.keys

-----------------------------------------------------------------------------
(* Helpers *)

\* Own uncommitted row: a transaction always sees its own writes.
Own(t) == IF pending[t] # None THEN {pending[t]} ELSE {}

\* What a statement of t's open transaction reads: snapshot in RR (once taken), else the
\* latest committed state; plus its own writes.
Visible(t) == (IF RepeatableRead /\ snap[t].taken THEN snap[t].rows ELSE committed) \cup Own(t)
\* Same, for a statement that itself takes the RR snapshot (TakeSnap in the same step).
VisibleNow(t) == IF RepeatableRead /\ ~snap[t].taken THEN committed \cup Own(t) ELSE Visible(t)

\* A statement starts inside the transaction: RR takes the snapshot at the first one.
TakeSnap(t) == snap' = [snap EXCEPT ![t] = IF RepeatableRead /\ ~snap[t].taken
                                            THEN [taken |-> TRUE, rows |-> committed] ELSE snap[t]]

ReleaseLocks(t) == [k \in LockKeys |-> IF locks[k] = t THEN NoOne ELSE locks[k]]

\* ROLLBACK (any Err out of store_all drops `tx`): row gone, locks released.
Rollback(t, out) ==
    /\ pending' = [pending EXCEPT ![t] = None]
    /\ locks'   = ReleaseLocks(t)
    /\ snap'    = [snap EXCEPT ![t] = NoSnap]
    /\ outcome' = [outcome EXCEPT ![t] = out]
    /\ pc'      = [pc EXCEPT ![t] = "done"]

Goto(t, l) == pc' = [pc EXCEPT ![t] = l]

HasAsn(t) == asn[t] # NoAsn
\* claim_unique checks the checksum only with CONSUMER_DELETE_DUPLICATES.
ChecksSum == DeleteDuplicates

\* Step order in store_all: claim_unique (ASN part, then checksum part) first, then
\* Consumer::store (insert_document, then the ASN write).
AfterInsert(t) == IF HasAsn(t) THEN "writeAsn" ELSE "files"
ClaimEnd(t)    == IF RecheckAfterInsert THEN AfterInsert(t) ELSE "insert"
AfterAsn(t)    == IF ChecksSum THEN (IF LockAfterFind THEN "sumFind" ELSE "sumLockStmt")
                  ELSE ClaimEnd(t)
ClaimStart(t)  == IF HasAsn(t) THEN (IF LockAfterFind THEN "asnFind" ELSE "asnLockStmt")
                  ELSE AfterAsn(t)
AfterBegin(t)  == IF RecheckAfterInsert THEN "insert" ELSE ClaimStart(t)

-----------------------------------------------------------------------------
(* Initial state: every combination of ASN/checksum per task within the bounds. *)

Init ==
    /\ asn \in [Tasks -> Asns]
    /\ sum \in [Tasks -> Sums]
    \* whether shrinking applies depends on the file and the tool; modelled as a free choice per
    \* task (also: the same original shrunk in one task and not in another).
    /\ shr \in [Tasks -> IF Shrink THEN BOOLEAN ELSE {FALSE}]
    /\ pc = [t \in Tasks |-> "preflight"]
    /\ outcome = [t \in Tasks |-> "running"]
    \* With ExternalAsnWriter there is one older document (no ASN) the API can edit.
    /\ committed = IF ExternalAsnWriter THEN {ExtRow(NoAsn)} ELSE {}
    /\ pending = [t \in Tasks |-> None]
    /\ locks = [k \in LockKeys |-> NoOne]
    /\ snap = [t \in Tasks |-> NoSnap]
    /\ apiDone = ~ExternalAsnWriter

-----------------------------------------------------------------------------
(* Outside the store transaction (own pool connection, autocommit SELECTs) *)

\* consumer.rs Consumer::preflight, "pre_check_duplicate": db::find_duplicates on a pool
\* connection; with CONSUMER_DELETE_DUPLICATES a hit rejects the file.
Preflight(t) ==
    /\ pc[t] = "preflight"
    /\ IF DeleteDuplicates /\ \E d \in committed : Matches(d, sum[t])
          THEN /\ outcome' = [outcome EXCEPT ![t] = "Duplicate"]
               /\ Goto(t, "done")
          ELSE /\ Goto(t, "asnCheck")
               /\ UNCHANGED outcome
    /\ UNCHANGED <<asn, sum, shr, committed, pending, locks, snap, apiDone>>

\* consumer.rs Consumer::asn_check (AsnCheckPlugin): db::find_asn on a pool connection.
\* (Parsing/OCR follows; it touches none of the modelled state.)
AsnCheck(t) ==
    /\ pc[t] = "asnCheck"
    /\ IF HasAsn(t) /\ \E d \in committed : d.asn = asn[t]
          THEN /\ outcome' = [outcome EXCEPT ![t] = "AsnExists"]
               /\ Goto(t, "done")
          ELSE /\ Goto(t, "begin")
               /\ UNCHANGED outcome
    /\ UNCHANGED <<asn, sum, shr, committed, pending, locks, snap, apiDone>>

-----------------------------------------------------------------------------
(* The store transaction: consumer.rs Consumer::store_all *)

\* `self.pool.begin()` - BEGIN (no snapshot yet, also not in RR).
Begin(t) ==
    /\ pc[t] = "begin"
    /\ Goto(t, AfterBegin(t))
    /\ UNCHANGED <<asn, sum, shr, outcome, committed, pending, locks, snap, apiDone>>

\* claim_unique: db::advisory_xact_lock(tx, "paperrust:asn:<n>") - the statement starts.
AsnLockStmt(t) ==
    /\ pc[t] = "asnLockStmt"
    /\ IF UseAsnLock THEN /\ TakeSnap(t) /\ Goto(t, "asnLockWait")
                     ELSE /\ UNCHANGED snap
                          /\ Goto(t, IF LockAfterFind THEN AfterAsn(t) ELSE "asnFind")
    /\ UNCHANGED <<asn, sum, shr, outcome, committed, pending, locks, apiDone>>

\* ... and is granted once no other transaction holds the key (blocks until then).
AsnLockWait(t) ==
    /\ pc[t] = "asnLockWait"
    /\ locks[<<"asn", asn[t]>>] = NoOne
    /\ locks' = [locks EXCEPT ![<<"asn", asn[t]>>] = t]
    /\ Goto(t, IF LockAfterFind THEN AfterAsn(t) ELSE "asnFind")
    /\ UNCHANGED <<asn, sum, shr, outcome, committed, pending, snap, apiDone>>

\* claim_unique: db::find_asn(tx, asn) -> StoreFail::AsnExists (ROLLBACK, asn_exists message).
AsnFind(t) ==
    /\ pc[t] = "asnFind"
    /\ TakeSnap(t)
    /\ LET vis == VisibleNow(t)
       IN IF \E d \in vis : d.asn = asn[t]
          THEN /\ pending' = [pending EXCEPT ![t] = None]
               /\ locks'   = ReleaseLocks(t)
               /\ outcome' = [outcome EXCEPT ![t] = "AsnExists"]
               /\ Goto(t, "done")
          ELSE /\ Goto(t, IF LockAfterFind THEN "asnLockStmt" ELSE AfterAsn(t))
               /\ UNCHANGED <<pending, locks, outcome>>
    /\ UNCHANGED <<asn, sum, shr, committed, apiDone>>

\* claim_unique: db::advisory_xact_lock(tx, "paperrust:checksum:<sum>") - statement starts.
SumLockStmt(t) ==
    /\ pc[t] = "sumLockStmt"
    /\ IF UseSumLock THEN /\ TakeSnap(t) /\ Goto(t, "sumLockWait")
                     ELSE /\ UNCHANGED snap
                          /\ Goto(t, IF LockAfterFind THEN ClaimEnd(t) ELSE "sumFind")
    /\ UNCHANGED <<asn, sum, shr, outcome, committed, pending, locks, apiDone>>

SumLockWait(t) ==
    /\ pc[t] = "sumLockWait"
    /\ locks[<<"sum", sum[t]>>] = NoOne
    /\ locks' = [locks EXCEPT ![<<"sum", sum[t]>>] = t]
    /\ Goto(t, IF LockAfterFind THEN ClaimEnd(t) ELSE "sumFind")
    /\ UNCHANGED <<asn, sum, shr, outcome, committed, pending, snap, apiDone>>

\* claim_unique: db::find_duplicates(tx, sum) -> StoreFail::Duplicate (ROLLBACK, reject_duplicate).
SumFind(t) ==
    /\ pc[t] = "sumFind"
    /\ TakeSnap(t)
    /\ LET vis == VisibleNow(t)
       IN IF \E d \in vis : Matches(d, sum[t])
          THEN /\ pending' = [pending EXCEPT ![t] = None]
               /\ locks'   = ReleaseLocks(t)
               /\ outcome' = [outcome EXCEPT ![t] = "Duplicate"]
               /\ Goto(t, "done")
          ELSE /\ Goto(t, IF LockAfterFind THEN "sumLockStmt" ELSE ClaimEnd(t))
               /\ UNCHANGED <<pending, locks, outcome>>
    /\ UNCHANGED <<asn, sum, shr, committed, apiDone>>

\* Consumer::store: db::insert_document (INSERT, checksum set, archive_serial_number NULL).
\* The paperrust_source_checksum INSERT (store_all, after Consumer::store, before COMMIT) is
\* folded into this step: other transactions only read committed rows and the own transaction
\* runs no duplicate lookup after this point (in the code order), so where between Insert and
\* Commit it happens is not observable.
Insert(t) ==
    /\ pc[t] = "insert"
    /\ TakeSnap(t)
    /\ pending' = [pending EXCEPT ![t] = Row(t, NoAsn)]
    /\ Goto(t, IF RecheckAfterInsert THEN ClaimStart(t) ELSE AfterInsert(t))
    /\ UNCHANGED <<asn, sum, shr, outcome, committed, locks, apiDone>>

\* Consumer::store: apply_overrides sets row.archive_serial_number, docops::save writes it
\* (one UPDATE). The unique index waits for an uncommitted writer of the same ASN and raises
\* 23505 against a committed one. (No 40001 here: each task updates only its own new row.)
WriteAsn(t) ==
    /\ pc[t] = "writeAsn"
    /\ UniqueAsnIndex => ~\E u \in Tasks \ {t} : pending[u] # None /\ pending[u].asn = asn[t] \* blocks
    /\ IF UniqueAsnIndex /\ \E d \in committed : d.asn = asn[t]
          THEN Rollback(t, "RawUniqueError")
          ELSE /\ pending' = [pending EXCEPT ![t] = Row(t, asn[t])]
               /\ snap' = snap
               /\ Goto(t, "files")
               /\ UNCHANGED <<outcome, locks>>
    /\ UNCHANGED <<asn, sum, shr, committed, apiDone>>

\* Everything between the row writes and COMMIT (consumption_finished, files under
\* MEDIA_LOCK, docops::save, update_filename_and_move_files, record_task_document) can
\* fail -> StoreFail::Error -> ROLLBACK. It touches no modelled row/lock.
FailInTx(t) ==
    /\ pc[t] = "files"
    /\ Rollback(t, "Failed")
    /\ UNCHANGED <<asn, sum, shr, committed, apiDone>>

\* store_all: tx.commit() - row visible, advisory locks released.
Commit(t) ==
    /\ pc[t] = "files"
    /\ committed' = committed \cup {pending[t]}
    /\ pending'   = [pending EXCEPT ![t] = None]
    /\ locks'     = ReleaseLocks(t)
    /\ snap'      = [snap EXCEPT ![t] = NoSnap]
    /\ outcome'   = [outcome EXCEPT ![t] = "Stored"]
    /\ Goto(t, "done")
    /\ UNCHANGED <<asn, sum, shr, apiDone>>

-----------------------------------------------------------------------------
(* Scope boundary: an API edit (e.g. PATCH /api/documents/<id>/ with              *)
(* archive_serial_number) is one autocommit UPDATE that takes no advisory lock.    *)
(* It waits for uncommitted writers of the same ASN like any unique-index writer. *)
ApiSetAsn ==
    /\ ~apiDone
    /\ \E a \in Asns \ {NoAsn} :
         /\ UniqueAsnIndex => ~\E u \in Tasks : pending[u] # None /\ pending[u].asn = a
         /\ committed' = IF UniqueAsnIndex /\ \E d \in committed : d.asn = a
                           THEN committed            \* API gets its own error
                           ELSE (committed \ {ExtRow(NoAsn)}) \cup {ExtRow(a)}
    /\ apiDone' = TRUE
    /\ UNCHANGED <<pc, asn, sum, shr, outcome, pending, locks, snap>>

AllDone == \A t \in Tasks : pc[t] = "done"

\* Terminal state: everyone finished (so TLC's deadlock check only flags real hangs).
Finished == AllDone /\ apiDone /\ UNCHANGED vars

Next ==
    \/ \E t \in Tasks :
         \/ Preflight(t) \/ AsnCheck(t) \/ Begin(t)
         \/ AsnLockStmt(t) \/ AsnLockWait(t) \/ AsnFind(t)
         \/ SumLockStmt(t) \/ SumLockWait(t) \/ SumFind(t)
         \/ Insert(t) \/ WriteAsn(t) \/ FailInTx(t) \/ Commit(t)
    \/ ApiSetAsn
    \/ Finished

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* Properties *)

TypeOK ==
    /\ outcome \in [Tasks -> Outcomes]
    /\ shr \in [Tasks -> BOOLEAN]
    /\ locks \in [LockKeys -> Tasks \cup {NoOne}]

\* P1 (contract pl-consumer.md #24, ASN): a consume task never fails with the raw
\* unique-constraint error; a lost ASN race ends with the AsnCheck message instead.
NoRawUniqueError == \A t \in Tasks : outcome[t] # "RawUniqueError"

\* Sanity: no two documents ever share an ASN (the UNIQUE index guarantees it).
UniqueAsn == \A d1, d2 \in committed : (d1 # d2 /\ d1.asn # NoAsn) => d1.asn # d2.asn

\* P2 (contract #24, duplicates): with CONSUMER_DELETE_DUPLICATES a delivered file content is
\* stored at most once, however many tasks consume it concurrently - also when its stored copy
\* was shrunk (then documents_document.checksum differs from the delivered file).
NoDuplicateStored ==
    DeleteDuplicates => \A d1, d2 \in committed : d1 # d2 => d1.orig # d2.orig

\* P3: a rejection is never wrong - "ASN already exists" / "duplicate" only when a
\* committed document really has that ASN / checksum. Together with P2 and deadlock
\* freedom this rules out "all concurrent tasks rejected, none stored".
RejectionJustified ==
    \A t \in Tasks :
        /\ outcome[t] = "AsnExists" => \E d \in committed : d.asn = asn[t]
        /\ outcome[t] = "Duplicate" => \E d \in committed : Matches(d, sum[t]) /\ d.by # t

=============================================================================
