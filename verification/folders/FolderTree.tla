----------------------------- MODULE FolderTree -----------------------------
(***************************************************************************)
(* PaperRust folder view: concurrent tree changes, document moves and      *)
(* upload-into-folder under PostgreSQL READ COMMITTED.                     *)
(*                                                                         *)
(* Code: crates/pl-api-v2/src/folders.rs (patch, delete, move_documents,   *)
(* tree_lock, visible_folder, creates_cycle), crates/pl-consumer/src/      *)
(* consumer.rs assign_folder, schema crates/pl-api-v2/migrations/          *)
(* 0002_paperrust_folder.sql.                                              *)
(*                                                                         *)
(* One action = one SQL statement (or one wait-free code step). Rows:      *)
(* committed value (fcv/dcv), the value written by an open transaction     *)
(* (fnv/dnv) and that writer (fw/dw, 0 = none). A transaction reads its    *)
(* own writes, everything else at READ COMMITTED reads the last committed  *)
(* value at statement time (snapshot per statement).                       *)
(*                                                                         *)
(* Locks are <<proc, resource, mode>>: resource <<"t",0>> = advisory lock  *)
(* hashtext('paperrust_folder_tree'), <<"f",id>> = paperrust_folder row,    *)
(* <<"d",id>> = paperrust_document_folder row. Modes as in PostgreSQL:     *)
(* "X" = FOR UPDATE / row written by DELETE / advisory, "NK" = FOR NO KEY  *)
(* UPDATE (plain UPDATE not touching the key), "KS" = FOR KEY SHARE (taken  *)
(* by every foreign-key check on the referenced folder row). All held to   *)
(* COMMIT/ROLLBACK. A lock-wait cycle is aborted like PostgreSQL does      *)
(* (40P01) - for the folder API that is a raw 500 (db(e)), for the         *)
(* consumer a retry.                                                       *)
(***************************************************************************)
EXTENDS Integers, FiniteSets, Sequences, TLC

CONSTANTS
    Folders,        \* folder ids existing at the start, e.g. {1,2,3}
    Docs,           \* existing documents that move-documents may move
    NProcs,         \* concurrent requests / consume tasks
    Kinds,          \* operation kinds a request may be (subset of OpKinds)
    AllTrees,       \* TRUE: start from every folder forest; FALSE: from Tree
    Tree,           \* tuple: parent of folder i (0 = top level), used if ~AllTrees
    AllPlacements,  \* TRUE: every document starts in every folder / Unsorted; FALSE: DocIn
    DocIn,          \* tuple: folder of document i (0 = Unsorted), used if ~AllPlacements
    \* switches describing the code (the *_code.cfg values) or a broken variant
    UseTreeLock,    \* folders.rs patch: tree_lock() when a parent is set (`if moves`)
    DelTreeLock,    \* folders.rs delete: tree_lock()
    LockAfterCheck, \* broken: cycle check (check_parent + SELECT id, parent_id) before the lock
    UseRowLock,     \* folders.rs delete: visible_folder(.., lock = true) = SELECT .. FOR UPDATE OF f
    RepeatableRead, \* broken: REPEATABLE READ instead of the default READ COMMITTED
    AssignKeyShare  \* proposed fix: assign_folder's SELECT .. FOR KEY SHARE (code: FALSE)

OpKinds == {"patchmove", "patchroot", "delete", "movedocs", "consume"}
ASSUME Kinds \subseteq OpKinds

Procs == 1..NProcs
NewDoc(p) == 100 + p                    \* id of the document consume task p stores
AllDocs == Docs \cup (IF "consume" \in Kinds THEN {NewDoc(p) : p \in Procs} ELSE {})
GONE == -1                              \* folder row deleted
ROOT == 0                               \* parent_id NULL / no paperrust_document_folder row
PVal == {GONE, ROOT} \cup Folders

(* Requests. patchmove: PATCH /folders/{f} {"parent": t}; patchroot: PATCH {"parent": null};
   delete: DELETE /folders/{f}?contents=parent (t = 1) or unsorted (t = 0);
   movedocs: POST /folders/move-documents {"documents":[d], "folder": f or null (f = 0)};
   consume: upload with field folder = f, the consume task's store_all transaction (assign_folder). *)
Op(k, f, t, d) == [k |-> k, f |-> f, t |-> t, d |-> d]
Ops ==
       (IF "patchmove" \in Kinds THEN {o \in {Op("patchmove", f, t, 0) : f \in Folders, t \in Folders} : o.f # o.t} ELSE {})
  \cup (IF "patchroot" \in Kinds THEN {Op("patchroot", f, 0, 0) : f \in Folders} ELSE {})
  \cup (IF "delete"    \in Kinds THEN {Op("delete", f, m, 0) : f \in Folders, m \in {0, 1}} ELSE {})
  \cup (IF "movedocs"  \in Kinds THEN {Op("movedocs", f, 0, d) : f \in {0} \cup Folders, d \in Docs} ELSE {})
  \cup (IF "consume"   \in Kinds THEN {Op("consume", f, 0, 0) : f \in Folders} ELSE {})

TreeLk == IF UseTreeLock THEN <<"lock">> ELSE <<>>
DelTreeLk == IF DelTreeLock THEN <<"lock">> ELSE <<>>

(* Statement sequence of each request, in code order. *)
Steps0(o) ==
  CASE o.k = "patchmove" ->
         \* folders.rs patch: tree_lock -> visible_folder FOR UPDATE -> check_parent -> SELECT id, parent_id
         \* + creates_cycle -> UPDATE paperrust_folder -> commit
         IF LockAfterCheck THEN <<"chkparent", "readtree">> \o TreeLk \o <<"forupd", "update", "commit">>
         ELSE TreeLk \o <<"forupd", "chkparent", "readtree", "update", "commit">>
    [] o.k = "patchroot" ->
         \* folders.rs patch with parent null: moves = false -> no tree_lock
         <<"forupd", "update", "commit">>
    [] o.k = "delete" ->
         \* folders.rs delete: tree_lock -> visible_folder FOR UPDATE -> UPDATE subfolders -> UPDATE/DELETE
         \* paperrust_document_folder -> DELETE paperrust_folder (ON DELETE CASCADE) -> commit
         DelTreeLk \o <<IF UseRowLock THEN "forupd_del" ELSE "read_del", "children", "docs", "delete", "commit">>
    [] o.k = "movedocs" ->
         \* folders.rs move_documents (no transaction: one autocommit statement after the checks)
         IF o.f = 0 THEN <<"mv_del">> ELSE <<"mv_check", "mv_write", "mv_fk">>
    [] o.k = "consume" ->
         \* consumer.rs store_all ... apply_overrides -> assign_folder -> rest of store_all (files) -> commit
         IF AssignKeyShare THEN <<"as_insert_ks", "rest", "commit">>
         ELSE <<"as_insert", "as_fk", "rest", "commit">>

(* REPEATABLE READ takes the transaction snapshot when the first statement starts, i.e. before
   that statement (tree_lock / FOR UPDATE) waits for a lock: modelled as its own step "rr_snap". *)
Steps(o) == (IF RepeatableRead /\ o.k \in {"patchmove", "patchroot", "delete"} THEN <<"rr_snap">> ELSE <<>>) \o Steps0(o)

VARIABLES
    fcv, fnv, fw,   \* paperrust_folder.parent_id per folder: committed / pending / writer
    dcv, dnv, dw,   \* paperrust_document_folder.folder_id per document (0 = no row = Unsorted)
    locks,          \* set of <<proc, resource, mode>>
    op, pc, res,    \* request per proc, index into Steps(op), outcome
    tgt,            \* delete: target of the contents; movedocs: 1 = row written (FK check due)
    snap,           \* REPEATABLE READ snapshot of parent_id per proc (NoSnap = not taken)
    loss            \* ghost: a delete with contents=parent put a document into Unsorted (ON DELETE CASCADE)

NoSnap == <<>>

vars == <<fcv, fnv, fw, dcv, dnv, dw, locks, op, pc, res, tgt, snap, loss>>

Lbl(p) == Steps(op[p])[pc[p]]
Done(p) == pc[p] > Len(Steps(op[p]))

-----------------------------------------------------------------------------
(* Reads *)
Snap1(p) == IF RepeatableRead /\ snap[p] = NoSnap THEN fcv ELSE snap[p]
FP(p, f) == IF fw[f] = p THEN fnv[f] ELSE IF RepeatableRead THEN Snap1(p)[f] ELSE fcv[f]
(* foreign-key checks always look at the latest committed row (under REPEATABLE READ a row the
   snapshot misses makes them fail with 40001 instead) *)
FL(p, f) == IF fw[f] = p THEN fnv[f] ELSE fcv[f]
DP(p, d) == IF dw[d] = p THEN dnv[d] ELSE dcv[d]

RECURSIVE Walk(_, _, _, _)
Walk(p, f, cur, n) ==
    IF cur \in {ROOT, GONE} THEN FALSE
    ELSE IF cur = f THEN TRUE
    ELSE IF n = 0 THEN TRUE
    ELSE Walk(p, f, FP(p, cur), n - 1)
(* folders.rs creates_cycle over "SELECT id, parent_id FROM paperrust_folder" (deleted rows absent) *)
CreatesCycle(p, f, t) == Walk(p, f, t, Cardinality(Folders))

(* Rows an UPDATE/DELETE ... WHERE <col> = f touches: visible rows matching; it waits for other
   writers of those rows and re-evaluates on the latest committed version (EvalPlanQual). *)
ChildRows(p, f) == {c \in Folders \ {f} : FP(p, c) = f}
RefRows(p, f) == {c \in Folders \ {f} : FL(p, c) = f}     \* rows the FK (NO ACTION) check of DELETE f finds
DocRows(p, f) == {d \in AllDocs : DP(p, d) = f}

-----------------------------------------------------------------------------
(* Lock requests of the next statement of p *)
R(kind, id) == <<kind, id>>
Need(p) ==
  LET o == op[p] IN
  IF Done(p) THEN {} ELSE
  CASE Lbl(p) = "lock"       -> {<<R("t", 0), "X">>}
    [] Lbl(p) = "forupd"     -> {<<R("f", o.f), "X">>}
    [] Lbl(p) = "forupd_del" -> {<<R("f", o.f), "X">>}
    [] Lbl(p) = "update"     -> IF o.k = "patchmove" THEN {<<R("f", o.t), "KS">>} ELSE {}
    [] Lbl(p) = "children"   -> {<<R("f", c), "NK">> : c \in ChildRows(p, o.f)}
                                \cup (IF ChildRows(p, o.f) # {} /\ tgt[p] # ROOT THEN {<<R("f", tgt[p]), "KS">>} ELSE {})
    [] Lbl(p) = "docs"       -> {<<R("d", d), "X">> : d \in DocRows(p, o.f)}
                                \cup (IF DocRows(p, o.f) # {} /\ tgt[p] # ROOT THEN {<<R("f", tgt[p]), "KS">>} ELSE {})
    [] Lbl(p) = "delete"     -> {<<R("f", o.f), "X">>}
                                \cup {<<R("d", d), "X">> : d \in DocRows(p, o.f)}       \* cascade
                                \cup {<<R("f", c), "KS">> : c \in RefRows(p, o.f)}      \* NO ACTION check
    [] Lbl(p) = "mv_write"   -> {<<R("d", o.d), "X">>}
    [] Lbl(p) = "mv_fk"      -> IF tgt[p] = 1 THEN {<<R("f", o.f), "KS">>} ELSE {}
    [] Lbl(p) = "mv_del"     -> {<<R("d", o.d), "X">>}
    [] Lbl(p) = "as_fk"      -> {<<R("f", o.f), "KS">>}
    [] Lbl(p) = "as_insert_ks" -> {<<R("f", o.f), "KS">>}
    [] OTHER                 -> {}

Conflict(m1, m2) == m1 = "X" \/ m2 = "X" \/ (m1 = "NK" /\ m2 = "NK")
Blockers(p) == {q \in Procs \ {p} :
                  \E n \in Need(p), l \in locks : l[1] = q /\ l[2] = n[1] /\ Conflict(n[2], l[3])}
Free(p) == Blockers(p) = {}
Acquire(p, extra) == locks' = locks \cup {<<p, n[1], n[2]>> : n \in Need(p) \cup extra}

RECURSIVE ReachN(_, _)
ReachN(S, n) == IF n = 0 THEN S ELSE ReachN(S \cup UNION {Blockers(r) : r \in S}, n - 1)
InCycle(p) == ~Done(p) /\ p \in ReachN(Blockers(p), NProcs)

-----------------------------------------------------------------------------
(* Values for the tuple constants (TLC .cfg files cannot write tuples; use `Tree <- TreeFlat`). *)
TreeFlat == <<0, 0, 0>>           \* three top-level folders
TreeOneChild == <<0, 1, 0>>       \* 2 inside 1, 3 top level
NoDocIn == <<>>

(* Tree configs use model values for Folders and this symmetry (folder ids are interchangeable there). *)
FolderPerms == Permutations(Folders)

RECURSIVE InitCycle(_, _, _, _)
InitCycle(t, f, c, n) == IF c = ROOT THEN FALSE ELSE IF c = f THEN TRUE ELSE IF n = 0 THEN TRUE
                         ELSE InitCycle(t, f, t[c], n - 1)


Init ==
    /\ fcv \in (IF AllTrees
                THEN {t \in [Folders -> {ROOT} \cup Folders] :
                        \A f \in Folders : ~InitCycle(t, f, t[f], Cardinality(Folders))}
                ELSE {[f \in Folders |-> Tree[f]]})
    /\ fnv = fcv
    /\ fw = [f \in Folders |-> 0]
    /\ dcv \in (IF AllPlacements
                THEN {g \in [AllDocs -> {ROOT} \cup Folders] : \A d \in AllDocs \ Docs : g[d] = ROOT}
                ELSE {[d \in AllDocs |-> IF d \in Docs THEN DocIn[d] ELSE ROOT]})
    /\ dnv = dcv
    /\ dw = [d \in AllDocs |-> 0]
    /\ locks = {}
    /\ op \in [Procs -> Ops]
    /\ pc = [p \in Procs |-> 1]
    /\ res = [p \in Procs |-> "none"]
    /\ tgt = [p \in Procs |-> 0]
    /\ snap = [p \in Procs |-> NoSnap]
    /\ loss = FALSE

(* p may run its next statement: not finished and no conflicting lock held by another transaction *)
G(p) == ~Done(p) /\ Free(p)

Adv(p) == pc' = [pc EXCEPT ![p] = @ + 1]
TakeSnap(p) == snap' = [snap EXCEPT ![p] = Snap1(p)]

(* ROLLBACK: drop own writes and locks *)
Abort(p, kind) ==
    /\ fnv' = [f \in Folders |-> IF fw[f] = p THEN fcv[f] ELSE fnv[f]]
    /\ fw' = [f \in Folders |-> IF fw[f] = p THEN 0 ELSE fw[f]]
    /\ dnv' = [d \in AllDocs |-> IF dw[d] = p THEN dcv[d] ELSE dnv[d]]
    /\ dw' = [d \in AllDocs |-> IF dw[d] = p THEN 0 ELSE dw[d]]
    /\ locks' = {l \in locks : l[1] # p}
    /\ res' = [res EXCEPT ![p] = kind]
    /\ pc' = [pc EXCEPT ![p] = Len(Steps(op[p])) + 1]
    /\ UNCHANGED <<fcv, dcv, op, tgt, snap, loss>>

(* COMMIT: own writes become visible, locks released *)
CommitTx(p) ==
    /\ fcv' = [f \in Folders |-> IF fw[f] = p THEN fnv[f] ELSE fcv[f]]
    /\ fw' = [f \in Folders |-> IF fw[f] = p THEN 0 ELSE fw[f]]
    /\ dcv' = [d \in AllDocs |-> IF dw[d] = p THEN dnv[d] ELSE dcv[d]]
    /\ dw' = [d \in AllDocs |-> IF dw[d] = p THEN 0 ELSE dw[d]]
    /\ locks' = {l \in locks : l[1] # p}
    /\ res' = [res EXCEPT ![p] = "ok"]
    /\ pc' = [pc EXCEPT ![p] = Len(Steps(op[p])) + 1]
    \* delete with contents=parent below a parent: its own "docs" statement moved rows to tgt (# 0), so an
    \* own row ending as "no row" was removed by the ON DELETE CASCADE of the folder row
    /\ loss' = (loss \/ (op[p].k = "delete" /\ op[p].t = 1 /\ tgt[p] # ROOT
                         /\ \E d \in AllDocs : dw[d] = p /\ dnv[d] = ROOT))
    /\ UNCHANGED <<fnv, dnv, op, tgt, snap>>

RRChanged(p, S) == RepeatableRead /\ \E c \in S : fcv[c] # Snap1(p)[c]

(* --- statements ---------------------------------------------------------- *)

(* start of the first statement under REPEATABLE READ: transaction snapshot *)
SRRSnap(p) ==
    /\ G(p) /\ Lbl(p) = "rr_snap"
    /\ snap' = [snap EXCEPT ![p] = fcv] /\ Adv(p)
    /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, locks, op, res, tgt, loss>>

(* tree_lock: SELECT pg_advisory_xact_lock(hashtext('paperrust_folder_tree')) *)
SLock(p) ==
    /\ G(p) /\ Lbl(p) = "lock"
    /\ Acquire(p, {}) /\ Adv(p) /\ TakeSnap(p)
    /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, op, res, tgt, loss>>

(* patch: visible_folder(&mut *tx, &a, id, true) - SELECT .. FOR UPDATE OF f; gone -> 404 *)
SForUpd(p) ==
    /\ G(p) /\ Lbl(p) = "forupd"
    /\ IF RRChanged(p, {op[p].f}) THEN Abort(p, "err_raw")                \* 40001
       ELSE IF FP(p, op[p].f) = GONE THEN Abort(p, "err_mapped")         \* NotFound
       ELSE /\ Acquire(p, {}) /\ Adv(p) /\ TakeSnap(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, op, res, tgt, loss>>

(* patch: check_parent (plain visible_folder of the new parent); gone -> 400 fields.parent *)
SChkParent(p) ==
    /\ G(p) /\ Lbl(p) = "chkparent"
    /\ IF FP(p, op[p].t) = GONE THEN Abort(p, "err_mapped")
       ELSE /\ Adv(p) /\ TakeSnap(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, locks, op, res, tgt, loss>>

(* patch: SELECT id, parent_id FROM paperrust_folder + creates_cycle -> 400 fields.parent *)
SReadTree(p) ==
    /\ G(p) /\ Lbl(p) = "readtree"
    /\ IF CreatesCycle(p, op[p].f, op[p].t) THEN Abort(p, "err_mapped")
       ELSE /\ Adv(p) /\ TakeSnap(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, locks, op, res, tgt, loss>>

(* patch: UPDATE paperrust_folder SET .., parent_id = $3 .. WHERE id = $1 (FK check on the new parent;
   23503 -> write_err -> 400 fields.parent) *)
SUpdate(p) ==
    /\ G(p) /\ Lbl(p) = "update"
    /\ LET o == op[p]
           new == IF o.k = "patchroot" THEN ROOT ELSE o.t IN
       IF RRChanged(p, {o.f}) THEN Abort(p, "err_raw")
       ELSE IF new # ROOT /\ fcv[new] = GONE THEN Abort(p, "err_mapped")
       ELSE /\ fnv' = [fnv EXCEPT ![o.f] = new]
            /\ fw' = [fw EXCEPT ![o.f] = p]
            /\ Acquire(p, {}) /\ Adv(p) /\ TakeSnap(p)
            /\ UNCHANGED <<fcv, dcv, dnv, dw, op, res, tgt, loss>>

(* delete: visible_folder(&mut *tx, &a, id, true) (or without FOR UPDATE in the broken variant);
   target = parent_id (contents=parent) or NULL *)
SDelRead(p) ==
    /\ G(p) /\ Lbl(p) \in {"forupd_del", "read_del"}
    /\ IF RRChanged(p, {op[p].f}) /\ Lbl(p) = "forupd_del" THEN Abort(p, "err_raw")
       ELSE IF FP(p, op[p].f) = GONE THEN Abort(p, "err_mapped")
       ELSE /\ tgt' = [tgt EXCEPT ![p] = IF op[p].t = 1 THEN FP(p, op[p].f) ELSE ROOT]
            /\ Acquire(p, {}) /\ Adv(p) /\ TakeSnap(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, op, res, loss>>

(* delete: UPDATE paperrust_folder SET parent_id = target .. WHERE parent_id = f
   (errors other than 23505 -> db(e) -> 500) *)
SChildren(p) ==
    /\ G(p) /\ Lbl(p) = "children"
    /\ LET o == op[p]
           M == ChildRows(p, o.f) IN
       IF RRChanged(p, M) THEN Abort(p, "err_raw")
       ELSE IF M # {} /\ tgt[p] # ROOT /\ fcv[tgt[p]] = GONE THEN Abort(p, "err_raw")
       ELSE /\ fnv' = [c \in Folders |-> IF c \in M THEN tgt[p] ELSE fnv[c]]
            /\ fw' = [c \in Folders |-> IF c \in M THEN p ELSE fw[c]]
            /\ Acquire(p, {}) /\ Adv(p) /\ TakeSnap(p)
            /\ UNCHANGED <<fcv, dcv, dnv, dw, op, res, tgt, loss>>

(* delete: UPDATE paperrust_document_folder SET folder_id = target WHERE folder_id = f, or
   DELETE FROM paperrust_document_folder WHERE folder_id = f when the target is "Unsorted" *)
SDocs(p) ==
    /\ G(p) /\ Lbl(p) = "docs"
    /\ LET o == op[p]
           M == DocRows(p, o.f) IN
       IF M # {} /\ tgt[p] # ROOT /\ fcv[tgt[p]] = GONE THEN Abort(p, "err_raw")
       ELSE /\ dnv' = [d \in AllDocs |-> IF d \in M THEN tgt[p] ELSE dnv[d]]
            /\ dw' = [d \in AllDocs |-> IF d \in M THEN p ELSE dw[d]]
            /\ Acquire(p, {}) /\ Adv(p) /\ TakeSnap(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, op, res, tgt, loss>>

(* delete: DELETE FROM paperrust_folder WHERE id = f. Document rows still pointing at f go with
   ON DELETE CASCADE; a subfolder still pointing at f violates parent_id's FK (NO ACTION) -> db(e) -> 500 *)
SDelete(p) ==
    /\ G(p) /\ Lbl(p) = "delete"
    /\ LET o == op[p]
           MD == DocRows(p, o.f)
           MC == RefRows(p, o.f) IN
       IF MC # {} THEN Abort(p, "err_raw")
       ELSE /\ fnv' = [fnv EXCEPT ![o.f] = GONE]
            /\ fw' = [fw EXCEPT ![o.f] = p]
            /\ dnv' = [d \in AllDocs |-> IF d \in MD THEN ROOT ELSE dnv[d]]
            /\ dw' = [d \in AllDocs |-> IF d \in MD THEN p ELSE dw[d]]
            /\ Acquire(p, {}) /\ Adv(p) /\ TakeSnap(p)
            /\ UNCHANGED <<fcv, dcv, op, res, tgt, loss>>

SCommit(p) ==
    /\ G(p) /\ Lbl(p) = "commit"
    /\ CommitTx(p)

(* move_documents: resolve_folder + visible_folder(pool, ..) (plain read); gone -> 400 fields.folder *)
SMvCheck(p) ==
    /\ G(p) /\ Lbl(p) = "mv_check"
    /\ IF fcv[op[p].f] = GONE THEN Abort(p, "err_mapped")
       ELSE /\ Adv(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, locks, op, res, tgt, snap, loss>>

(* move_documents: INSERT .. ON CONFLICT (document_id) DO UPDATE .. WHERE folder_id IS DISTINCT FROM ..
   - row written (or only locked when already in that folder) *)
SMvWrite(p) ==
    /\ G(p) /\ Lbl(p) = "mv_write"
    /\ LET o == op[p] IN
       IF dcv[o.d] = o.f
       THEN /\ tgt' = [tgt EXCEPT ![p] = 0]
            /\ Acquire(p, {}) /\ Adv(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, op, res, snap, loss>>
       ELSE /\ tgt' = [tgt EXCEPT ![p] = 1]
            /\ dnv' = [dnv EXCEPT ![o.d] = o.f]
            /\ dw' = [dw EXCEPT ![o.d] = p]
            /\ Acquire(p, {}) /\ Adv(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, op, res, snap, loss>>

(* same statement: FK check on folder_id (23503 -> 400 fields.folder), then autocommit *)
SMvFk(p) ==
    /\ G(p) /\ Lbl(p) = "mv_fk"
    /\ IF tgt[p] = 1 /\ fcv[op[p].f] = GONE THEN Abort(p, "err_mapped")
       ELSE CommitTx(p)

(* move_documents with folder null: DELETE FROM paperrust_document_folder WHERE document_id = ANY($1), autocommit *)
SMvDel(p) ==
    /\ G(p) /\ Lbl(p) = "mv_del"
    /\ dcv' = [dcv EXCEPT ![op[p].d] = ROOT]
    /\ dnv' = [dnv EXCEPT ![op[p].d] = ROOT]
    /\ res' = [res EXCEPT ![p] = "ok"]
    /\ pc' = [pc EXCEPT ![p] = Len(Steps(op[p])) + 1]
    /\ UNCHANGED <<fcv, fnv, fw, dw, locks, op, tgt, snap, loss>>

(* assign_folder: INSERT INTO paperrust_document_folder .. SELECT $1, f.id FROM paperrust_folder f
   WHERE f.id = $2 .. - the SELECT reads without a lock; folder gone -> 0 rows -> "stays unsorted" *)
SAsInsert(p) ==
    /\ G(p) /\ Lbl(p) = "as_insert"
    /\ LET o == op[p]
           d == NewDoc(p) IN
       IF fcv[o.f] = GONE
       THEN /\ pc' = [pc EXCEPT ![p] = @ + 2]          \* skip the FK check, no row written
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, locks, op, res, tgt, snap, loss>>
       ELSE /\ dnv' = [dnv EXCEPT ![d] = o.f]
            /\ dw' = [dw EXCEPT ![d] = p]
            /\ Acquire(p, {<<R("d", d), "X">>}) /\ Adv(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, op, res, tgt, snap, loss>>

(* same statement: FK check of folder_id (KEY SHARE on the folder row, waits for a deleting
   transaction). Folder deleted meanwhile -> 23503 -> assign_folder Err -> store_all fails *)
SAsFk(p) ==
    /\ G(p) /\ Lbl(p) = "as_fk"
    /\ IF fcv[op[p].f] = GONE THEN Abort(p, "ingest_fk_fail")
       ELSE /\ Acquire(p, {}) /\ Adv(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, op, res, tgt, snap, loss>>

(* proposed fix: .. FROM paperrust_folder f WHERE f.id = $2 FOR KEY SHARE - waits for a deleting
   transaction, a deleted row is skipped (READ COMMITTED) -> 0 rows -> unsorted, no FK error *)
SAsInsertKs(p) ==
    /\ G(p) /\ Lbl(p) = "as_insert_ks"
    /\ LET o == op[p]
           d == NewDoc(p) IN
       IF fcv[o.f] = GONE
       THEN /\ Adv(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, locks, op, res, tgt, snap, loss>>
       ELSE /\ dnv' = [dnv EXCEPT ![d] = o.f]
            /\ dw' = [dw EXCEPT ![d] = p]
            /\ Acquire(p, {<<R("d", d), "X">>}) /\ Adv(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, op, res, tgt, snap, loss>>

(* rest of store_all after apply_overrides (files, more inserts) before tx.commit() *)
SRest(p) ==
    /\ G(p) /\ Lbl(p) = "rest"
    /\ Adv(p)
    /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, locks, op, res, tgt, snap, loss>>

(* PostgreSQL deadlock detector: one transaction of a lock-wait cycle gets 40P01 *)
DeadlockAbort(p) ==
    /\ InCycle(p)
    /\ Abort(p, IF op[p].k = "consume" THEN "retry" ELSE "err_raw")

Finished == /\ \A p \in Procs : Done(p)
            /\ UNCHANGED vars

Next == \/ \E p \in Procs :
            \/ SRRSnap(p) \/ SLock(p) \/ SForUpd(p) \/ SChkParent(p) \/ SReadTree(p) \/ SUpdate(p)
            \/ SDelRead(p) \/ SChildren(p) \/ SDocs(p) \/ SDelete(p) \/ SCommit(p)
            \/ SMvCheck(p) \/ SMvWrite(p) \/ SMvFk(p) \/ SMvDel(p)
            \/ SAsInsert(p) \/ SAsFk(p) \/ SAsInsertKs(p) \/ SRest(p)
            \/ DeadlockAbort(p)
        \/ Finished

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
(* Properties *)

RECURSIVE Closure(_, _)
Closure(Rel, n) == IF n = 0 THEN Rel
                   ELSE Closure(Rel \cup {e \in Folders \X Folders :
                                            \E b \in Folders : <<e[1], b>> \in Rel /\ <<b, e[2]>> \in Rel}, n - 1)
Edges == {<<a, fcv[a]>> : a \in {x \in Folders : fcv[x] \in Folders}}

(* P1: the committed folder tree never has a cycle *)
Acyclic == \A f \in Folders : <<f, f>> \notin Closure(Edges, Cardinality(Folders))

(* sanity (foreign keys): no folder or document points at a deleted folder *)
ParentExists == \A f \in Folders : fcv[f] \in Folders => fcv[fcv[f]] # GONE
DocFolderExists == \A d \in AllDocs : dcv[d] # ROOT => fcv[dcv[d]] # GONE

(* P2: DELETE ?contents=parent of a subfolder never leaves one of its documents in "Unsorted" *)
DeleteHonorsContents == ~loss

(* P3: no request of the folder API ends in a raw database error (500: deadlock, serialization
   failure, unmapped FK error) *)
NoRawError == \A p \in Procs : res[p] # "err_raw"

(* P4: an upload into a folder that is deleted meanwhile never fails the ingestion (contract stage 2:
   the document stays "Unsorted") *)
NoIngestFkError == \A p \in Procs : res[p] # "ingest_fk_fail"

TypeOK ==
    /\ fcv \in [Folders -> PVal] /\ fnv \in [Folders -> PVal] /\ fw \in [Folders -> {0} \cup Procs]
    /\ dcv \in [AllDocs -> {ROOT} \cup Folders] /\ dw \in [AllDocs -> {0} \cup Procs]
=============================================================================
