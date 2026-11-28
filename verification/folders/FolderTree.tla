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
(* <<"d",id>> = paperrust_document_folder row, <<"doc",id>> = document row  *)
(* (paperrust_document / documents_document). Modes as in PostgreSQL:      *)
(* "X" = FOR UPDATE / row written by DELETE / advisory, "NK" = FOR NO KEY  *)
(* UPDATE (plain UPDATE not touching the key), "S" = FOR SHARE, "KS" = FOR *)
(* KEY SHARE (taken by every foreign-key check on the referenced folder    *)
(* row). All held to                                                       *)
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
    AssignKeyShare, \* assign_folder's SELECT .. FOR KEY SHARE (code since ba8ea24b: TRUE)
    MoveTx,         \* move_documents: checks + write in one transaction after target FOR SHARE and
                    \* document rows FOR SHARE (code: TRUE); FALSE = old code (checks on the pool, autocommit write)
    DelLocks        \* delete/check_contents_movable: subfolders FOR UPDATE, parent FOR SHARE, documents
                    \* FOR SHARE (code: TRUE); FALSE = same checks as plain reads (check-then-write gap)

(* Permissions (only what the new checks read): the caller of move/delete starts with change rights on
   every folder and document; "unsharef" (PATCH /folders/{f} with new permissions/owner: visible_folder
   FOR UPDATE -> write_grants -> commit) and "unsharedoc" (document owner / set_permissions change:
   update.rs begin_root_tx or bulk_edit::lock_documents = document row FOR UPDATE -> owner/guardian
   write -> commit) take that right away. *)
OpKinds == {"patchmove", "patchroot", "delete", "movedocs", "consume", "unsharef", "unsharedoc"}
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
  \cup (IF "unsharef"  \in Kinds THEN {Op("unsharef", f, 0, 0) : f \in Folders} ELSE {})
  \cup (IF "unsharedoc" \in Kinds THEN {Op("unsharedoc", 0, 0, d) : d \in Docs} ELSE {})

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
         \* folders.rs delete: tree_lock -> visible_folder FOR UPDATE -> check_contents_movable (subfolders
         \* FOR UPDATE, SELECT document_id .. WHERE folder_id, parent FOR SHARE, documents FOR SHARE,
         \* count_unchangeable) -> UPDATE subfolders -> UPDATE/DELETE paperrust_document_folder ->
         \* DELETE paperrust_folder (ON DELETE CASCADE) -> commit
         DelTreeLk \o <<IF UseRowLock THEN "forupd_del" ELSE "read_del",
                        "del_subs", "del_docsread", "del_parent", "del_lockdocs", "del_chkdocs",
                        "children", "docs", "delete", "commit">>
    [] o.k = "movedocs" /\ MoveTx ->
         \* folders.rs move_documents: pool.begin -> visible_folder(.., RowLock::Share) -> lock_documents
         \* (FOR SHARE) -> visibility count + count_unchangeable -> INSERT .. ON CONFLICT / DELETE -> commit
         IF o.f = 0 THEN <<"mv_lockdocs", "mv_chkdocs", "mv_del", "commit">>
         ELSE <<"mv_target", "mv_lockdocs", "mv_chkdocs", "mv_write", "mv_fk", "commit">>
    [] o.k = "movedocs" /\ ~MoveTx ->
         \* broken (code before 2026-11-27): checks on the pool, then one autocommit statement
         IF o.f = 0 THEN <<"mv_chkdocs", "mv_del", "commit">>
         ELSE <<"mv_check", "mv_chkdocs", "mv_write", "mv_fk", "commit">>
    [] o.k = "unsharef" ->
         \* folders.rs patch with permissions/owner only: visible_folder FOR UPDATE -> write_grants -> commit
         <<"forupd", "revoke", "commit">>
    [] o.k = "unsharedoc" ->
         \* document permission change: document row FOR UPDATE -> owner_id / guardian rows -> commit
         <<"doc_forupd", "revoke", "commit">>
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
    loss,           \* ghost: a delete with contents=parent put a document into Unsorted (ON DELETE CASCADE)
    fpc, fpw,       \* caller may change folder f: committed / revoking open transaction (0 = none)
    dpc, dpw,       \* caller may change document d: committed / revoking open transaction
    chk,            \* objects whose change right p checked: <<"f", id>> / <<"doc", id>>
    seen,           \* delete: documents read by SELECT document_id .. WHERE folder_id = $1
    unauth,         \* ghost: a move/delete committed although a right it checked was revoked meanwhile
    unchecked       \* ghost: delete moved a subfolder/document (or into a target) it had not checked

NoSnap == <<>>

authv == <<fpc, fpw, dpc, dpw, chk, seen, unauth, unchecked>>
vars == <<fcv, fnv, fw, dcv, dnv, dw, locks, op, pc, res, tgt, snap, loss, authv>>

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
    [] Lbl(p) = "del_subs"   -> IF DelLocks THEN {<<R("f", c), "X">> : c \in ChildRows(p, o.f)} ELSE {}
    [] Lbl(p) = "del_parent" -> IF DelLocks THEN {<<R("f", tgt[p]), "S">>} ELSE {}
    [] Lbl(p) = "del_lockdocs" -> IF DelLocks THEN {<<R("doc", d), "S">> : d \in seen[p]} ELSE {}
    [] Lbl(p) = "mv_target"  -> {<<R("f", o.f), "S">>}
    [] Lbl(p) = "mv_lockdocs" -> {<<R("doc", o.d), "S">>}
    [] Lbl(p) = "doc_forupd" -> {<<R("doc", o.d), "X">>}
    [] Lbl(p) = "mv_write"   -> {<<R("d", o.d), "X">>}
    [] Lbl(p) = "mv_fk"      -> IF tgt[p] = 1 THEN {<<R("f", o.f), "KS">>} ELSE {}
    [] Lbl(p) = "mv_del"     -> {<<R("d", o.d), "X">>}
    [] Lbl(p) = "as_fk"      -> {<<R("f", o.f), "KS">>}
    [] Lbl(p) = "as_insert_ks" -> {<<R("f", o.f), "KS">>}
    [] OTHER                 -> {}

(* PostgreSQL row-lock conflict table: KS conflicts only with X; S with NK and X; NK with NK, S, X *)
Conflict(m1, m2) == m1 = "X" \/ m2 = "X" \/ (m1 = "NK" /\ m2 \in {"NK", "S"}) \/ (m1 = "S" /\ m2 = "NK")
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
    /\ fpc = [f \in Folders |-> TRUE]
    /\ fpw = [f \in Folders |-> 0]
    /\ dpc = [d \in AllDocs |-> TRUE]
    /\ dpw = [d \in AllDocs |-> 0]
    /\ chk = [p \in Procs |-> {}]
    /\ seen = [p \in Procs |-> {}]
    /\ unauth = FALSE
    /\ unchecked = FALSE

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
    /\ fpw' = [f \in Folders |-> IF fpw[f] = p THEN 0 ELSE fpw[f]]
    /\ dpw' = [d \in AllDocs |-> IF dpw[d] = p THEN 0 ELSE dpw[d]]
    /\ UNCHANGED <<fcv, dcv, op, tgt, snap, loss, fpc, dpc, chk, seen, unauth, unchecked>>

PermC(x) == IF x[1] = "f" THEN fpc[x[2]] ELSE dpc[x[2]]
Wrote(p) == (\E f \in Folders : fw[f] = p) \/ (\E d \in AllDocs : dw[d] = p)
PcOf(p, l) == CHOOSE i \in 1..Len(Steps(op[p])) : Steps(op[p])[i] = l
Goto(p, l) == pc' = [pc EXCEPT ![p] = PcOf(p, l)]
AddChk(p, S) == chk' = [chk EXCEPT ![p] = @ \cup S]
CheckedF(p) == {x[2] : x \in {y \in chk[p] : y[1] = "f"}}
CheckedD(p) == {x[2] : x \in {y \in chk[p] : y[1] = "doc"}}

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
    /\ fpc' = [f \in Folders |-> IF fpw[f] = p THEN FALSE ELSE fpc[f]]
    /\ fpw' = [f \in Folders |-> IF fpw[f] = p THEN 0 ELSE fpw[f]]
    /\ dpc' = [d \in AllDocs |-> IF dpw[d] = p THEN FALSE ELSE dpc[d]]
    /\ dpw' = [d \in AllDocs |-> IF dpw[d] = p THEN 0 ELSE dpw[d]]
    \* the move/delete writes now although a right its checks relied on was revoked after the check
    /\ unauth' = (unauth \/ (op[p].k \in {"movedocs", "delete"} /\ Wrote(p)
                             /\ \E x \in chk[p] : ~PermC(x)))
    /\ UNCHANGED <<fnv, dnv, op, tgt, snap, chk, seen, unchecked>>

RRChanged(p, S) == RepeatableRead /\ \E c \in S : fcv[c] # Snap1(p)[c]

(* --- statements ---------------------------------------------------------- *)

(* start of the first statement under REPEATABLE READ: transaction snapshot *)
SRRSnap(p) ==
    /\ G(p) /\ Lbl(p) = "rr_snap"
    /\ snap' = [snap EXCEPT ![p] = fcv] /\ Adv(p)
    /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, locks, op, res, tgt, loss, authv>>

(* tree_lock: SELECT pg_advisory_xact_lock(hashtext('paperrust_folder_tree')) *)
SLock(p) ==
    /\ G(p) /\ Lbl(p) = "lock"
    /\ Acquire(p, {}) /\ Adv(p) /\ TakeSnap(p)
    /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, op, res, tgt, loss, authv>>

(* patch: visible_folder(&mut *tx, &a, id, true) - SELECT .. FOR UPDATE OF f; gone -> 404 *)
SForUpd(p) ==
    /\ G(p) /\ Lbl(p) = "forupd"
    /\ IF RRChanged(p, {op[p].f}) THEN Abort(p, "err_raw")                \* 40001
       ELSE IF FP(p, op[p].f) = GONE THEN Abort(p, "err_mapped")         \* NotFound
       ELSE /\ Acquire(p, {}) /\ Adv(p) /\ TakeSnap(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, op, res, tgt, loss, authv>>

(* patch: check_parent (plain visible_folder of the new parent); gone -> 400 fields.parent *)
SChkParent(p) ==
    /\ G(p) /\ Lbl(p) = "chkparent"
    /\ IF FP(p, op[p].t) = GONE THEN Abort(p, "err_mapped")
       ELSE /\ Adv(p) /\ TakeSnap(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, locks, op, res, tgt, loss, authv>>

(* patch: SELECT id, parent_id FROM paperrust_folder + creates_cycle -> 400 fields.parent *)
SReadTree(p) ==
    /\ G(p) /\ Lbl(p) = "readtree"
    /\ IF CreatesCycle(p, op[p].f, op[p].t) THEN Abort(p, "err_mapped")
       ELSE /\ Adv(p) /\ TakeSnap(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, locks, op, res, tgt, loss, authv>>

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
            /\ UNCHANGED <<fcv, dcv, dnv, dw, op, res, tgt, loss, authv>>

(* delete: visible_folder(&mut *tx, &a, id, true) (or without FOR UPDATE in the broken variant);
   target = parent_id (contents=parent) or NULL *)
SDelRead(p) ==
    /\ G(p) /\ Lbl(p) \in {"forupd_del", "read_del"}
    /\ IF RRChanged(p, {op[p].f}) /\ Lbl(p) = "forupd_del" THEN Abort(p, "err_raw")
       ELSE IF FP(p, op[p].f) = GONE THEN Abort(p, "err_mapped")
       ELSE /\ tgt' = [tgt EXCEPT ![p] = IF op[p].t = 1 THEN FP(p, op[p].f) ELSE ROOT]
            /\ Acquire(p, {}) /\ Adv(p) /\ TakeSnap(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, op, res, loss, authv>>

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
            /\ unchecked' = (unchecked \/ ~(M \subseteq CheckedF(p))
                             \/ (M # {} /\ tgt[p] # ROOT /\ tgt[p] \notin CheckedF(p)))
            /\ UNCHANGED <<fcv, dcv, dnv, dw, op, res, tgt, loss, fpc, fpw, dpc, dpw, chk, seen, unauth>>

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
            /\ unchecked' = (unchecked \/ ~(M \subseteq CheckedD(p))
                             \/ (M # {} /\ tgt[p] # ROOT /\ tgt[p] \notin CheckedF(p)))
            /\ UNCHANGED <<fcv, fnv, fw, dcv, op, res, tgt, loss, fpc, fpw, dpc, dpw, chk, seen, unauth>>

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
            /\ UNCHANGED <<fcv, dcv, op, res, tgt, loss, authv>>

SCommit(p) ==
    /\ G(p) /\ Lbl(p) = "commit"
    /\ CommitTx(p)

(* --- delete: check_contents_movable (runs for both contents modes; caller is no superuser) ---- *)
DataUnch == <<fcv, fnv, fw, dcv, dnv, dw, op, res, tgt, snap, loss>>

(* SELECT f.id, <change cond> FROM paperrust_folder f WHERE f.parent_id = $3 ORDER BY f.id FOR UPDATE OF f.
   The row locks are taken in one step (the model has no partial lock sets); a subfolder that a
   concurrent patchroot moved out is re-checked and skipped (ChildRows recomputed after the wait).
   A subfolder without change right -> 403 DELETE_FORBIDDEN_FOLDERS (code decides it after the
   parent check, same statement result). *)
SDelSubs(p) ==
    /\ G(p) /\ Lbl(p) = "del_subs"
    /\ LET S == ChildRows(p, op[p].f) IN
       IF DelLocks /\ RRChanged(p, S) THEN Abort(p, "err_raw")
       ELSE IF \E c \in S : ~fpc[c] THEN Abort(p, "err_mapped")
       ELSE /\ AddChk(p, {<<"f", c>> : c \in S})
            /\ Acquire(p, {}) /\ Adv(p)
            /\ UNCHANGED <<DataUnch, fpc, fpw, dpc, dpw, seen, unauth, unchecked>>

(* SELECT document_id FROM paperrust_document_folder WHERE folder_id = $1 ORDER BY document_id (no lock).
   Nothing to move -> Ok (no parent/document check); target None -> no parent check. *)
SDelDocsRead(p) ==
    /\ G(p) /\ Lbl(p) = "del_docsread"
    /\ LET S == DocRows(p, op[p].f) IN
       /\ seen' = [seen EXCEPT ![p] = S]
       /\ Goto(p, IF S = {} /\ CheckedF(p) = {} THEN "children"
                  ELSE IF tgt[p] # ROOT THEN "del_parent"
                  ELSE IF S # {} THEN "del_lockdocs" ELSE "children")
       /\ UNCHANGED <<DataUnch, locks, fpc, fpw, dpc, dpw, chk, unauth, unchecked>>

(* visible_folder(&mut *tx, a, target, RowLock::Share) = SELECT .. FOR SHARE OF f; gone or no change
   right -> 403 DELETE_FORBIDDEN_PARENT *)
SDelParent(p) ==
    /\ G(p) /\ Lbl(p) = "del_parent"
    /\ LET t == tgt[p] IN
       IF FP(p, t) = GONE \/ ~fpc[t] THEN Abort(p, "err_mapped")
       ELSE /\ AddChk(p, {<<"f", t>>})
            /\ Acquire(p, {})
            /\ Goto(p, IF seen[p] # {} THEN "del_lockdocs" ELSE "children")
            /\ UNCHANGED <<DataUnch, fpc, fpw, dpc, dpw, seen, unauth, unchecked>>

(* lock_documents: SELECT id FROM <document table> WHERE id = ANY($1) ORDER BY id FOR SHARE
   (document_lock_table's catalog query before it reads nothing the model has) *)
SDelLockDocs(p) ==
    /\ G(p) /\ Lbl(p) = "del_lockdocs"
    /\ Acquire(p, {}) /\ Adv(p)
    /\ UNCHANGED <<DataUnch, fpc, fpw, dpc, dpw, chk, seen, unauth, unchecked>>

(* count_unchangeable(.., include_deleted = true) > 0 -> 403 DELETE_FORBIDDEN_DOCUMENTS *)
SDelChkDocs(p) ==
    /\ G(p) /\ Lbl(p) = "del_chkdocs"
    /\ IF \E d \in seen[p] : ~dpc[d] THEN Abort(p, "err_mapped")
       ELSE /\ AddChk(p, {<<"doc", d>> : d \in seen[p]})
            /\ Adv(p)
            /\ UNCHANGED <<DataUnch, locks, fpc, fpw, dpc, dpw, seen, unauth, unchecked>>

(* --- permission changes by another user ------------------------------------------------ *)

(* document row FOR UPDATE (update.rs begin_root_tx / bulk_edit::lock_documents_sorted) *)
SDocForUpd(p) ==
    /\ G(p) /\ Lbl(p) = "doc_forupd"
    /\ Acquire(p, {}) /\ Adv(p)
    /\ UNCHANGED <<DataUnch, fpc, fpw, dpc, dpw, chk, seen, unauth, unchecked>>

(* write_grants (folder) / owner_id + guardian rows (document): the caller loses the change right *)
SRevoke(p) ==
    /\ G(p) /\ Lbl(p) = "revoke"
    /\ IF op[p].k = "unsharef"
       THEN fpw' = [fpw EXCEPT ![op[p].f] = p] /\ UNCHANGED dpw
       ELSE dpw' = [dpw EXCEPT ![op[p].d] = p] /\ UNCHANGED fpw
    /\ Adv(p)
    /\ UNCHANGED <<DataUnch, locks, fpc, dpc, chk, seen, unauth, unchecked>>

(* --- move_documents --------------------------------------------------------------------- *)

(* broken/old: resolve_folder + visible_folder(pool, ..) (plain read); gone -> 400 fields.folder,
   no change right -> 403 *)
SMvCheck(p) ==
    /\ G(p) /\ Lbl(p) = "mv_check"
    /\ IF fcv[op[p].f] = GONE \/ ~fpc[op[p].f] THEN Abort(p, "err_mapped")
       ELSE /\ Adv(p) /\ AddChk(p, {<<"f", op[p].f>>})
            /\ UNCHANGED <<DataUnch, locks, fpc, fpw, dpc, dpw, seen, unauth, unchecked>>

(* code: visible_folder(&mut *tx, &a, fid, RowLock::Share) = SELECT .. FOR SHARE OF f (waits for a
   deleting / re-sharing transaction); gone -> 400 fields.folder, no change right -> 403 *)
SMvTarget(p) ==
    /\ G(p) /\ Lbl(p) = "mv_target"
    /\ IF fcv[op[p].f] = GONE \/ ~fpc[op[p].f] THEN Abort(p, "err_mapped")
       ELSE /\ Acquire(p, {}) /\ Adv(p) /\ AddChk(p, {<<"f", op[p].f>>})
            /\ UNCHANGED <<DataUnch, fpc, fpw, dpc, dpw, seen, unauth, unchecked>>

(* code: lock_documents(&mut tx, table, &ids32) = SELECT id .. ORDER BY id FOR SHARE *)
SMvLockDocs(p) ==
    /\ G(p) /\ Lbl(p) = "mv_lockdocs"
    /\ Acquire(p, {}) /\ Adv(p)
    /\ UNCHANGED <<DataUnch, fpc, fpw, dpc, dpw, chk, seen, unauth, unchecked>>

(* visibility count + count_unchangeable(.., include_deleted = false) (old code: the same counts on
   the pool). Visibility is not modelled separately: it reads the same rows under the same locks,
   and a lost view right implies a lost change right. Not changeable -> 403 *)
SMvChkDocs(p) ==
    /\ G(p) /\ Lbl(p) = "mv_chkdocs"
    /\ IF ~dpc[op[p].d] THEN Abort(p, "err_mapped")
       ELSE /\ Adv(p) /\ AddChk(p, {<<"doc", op[p].d>>})
            /\ UNCHANGED <<DataUnch, locks, fpc, fpw, dpc, dpw, seen, unauth, unchecked>>

(* move_documents: INSERT .. ON CONFLICT (document_id) DO UPDATE .. WHERE folder_id IS DISTINCT FROM ..
   - row written (or only locked when already in that folder) *)
SMvWrite(p) ==
    /\ G(p) /\ Lbl(p) = "mv_write"
    /\ LET o == op[p] IN
       IF dcv[o.d] = o.f
       THEN /\ tgt' = [tgt EXCEPT ![p] = 0]
            /\ Acquire(p, {}) /\ Adv(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, op, res, snap, loss, authv>>
       ELSE /\ tgt' = [tgt EXCEPT ![p] = 1]
            /\ dnv' = [dnv EXCEPT ![o.d] = o.f]
            /\ dw' = [dw EXCEPT ![o.d] = p]
            /\ Acquire(p, {}) /\ Adv(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, op, res, snap, loss, authv>>

(* same statement: FK check on folder_id (23503 -> 400 fields.folder); then tx.commit() (code) or
   the autocommit (old code) as the following "commit" step *)
SMvFk(p) ==
    /\ G(p) /\ Lbl(p) = "mv_fk"
    /\ IF tgt[p] = 1 /\ fcv[op[p].f] = GONE THEN Abort(p, "err_mapped")
       ELSE /\ Acquire(p, {}) /\ Adv(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, op, res, tgt, snap, loss, authv>>

(* move_documents with folder null: DELETE FROM paperrust_document_folder WHERE document_id = ANY($1)
   (row lock X until commit; a missing row deletes nothing) *)
SMvDel(p) ==
    /\ G(p) /\ Lbl(p) = "mv_del"
    /\ LET d == op[p].d IN
       IF dcv[d] = ROOT
       THEN /\ Adv(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, locks, op, res, tgt, snap, loss, authv>>
       ELSE /\ dnv' = [dnv EXCEPT ![d] = ROOT]
            /\ dw' = [dw EXCEPT ![d] = p]
            /\ Acquire(p, {}) /\ Adv(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, op, res, tgt, snap, loss, authv>>

(* assign_folder: INSERT INTO paperrust_document_folder .. SELECT $1, f.id FROM paperrust_folder f
   WHERE f.id = $2 .. - the SELECT reads without a lock; folder gone -> 0 rows -> "stays unsorted" *)
SAsInsert(p) ==
    /\ G(p) /\ Lbl(p) = "as_insert"
    /\ LET o == op[p]
           d == NewDoc(p) IN
       IF fcv[o.f] = GONE
       THEN /\ pc' = [pc EXCEPT ![p] = @ + 2]          \* skip the FK check, no row written
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, locks, op, res, tgt, snap, loss, authv>>
       ELSE /\ dnv' = [dnv EXCEPT ![d] = o.f]
            /\ dw' = [dw EXCEPT ![d] = p]
            /\ Acquire(p, {<<R("d", d), "X">>}) /\ Adv(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, op, res, tgt, snap, loss, authv>>

(* same statement: FK check of folder_id (KEY SHARE on the folder row, waits for a deleting
   transaction). Folder deleted meanwhile -> 23503 -> assign_folder Err -> store_all fails *)
SAsFk(p) ==
    /\ G(p) /\ Lbl(p) = "as_fk"
    /\ IF fcv[op[p].f] = GONE THEN Abort(p, "ingest_fk_fail")
       ELSE /\ Acquire(p, {}) /\ Adv(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, op, res, tgt, snap, loss, authv>>

(* proposed fix: .. FROM paperrust_folder f WHERE f.id = $2 FOR KEY SHARE - waits for a deleting
   transaction, a deleted row is skipped (READ COMMITTED) -> 0 rows -> unsorted, no FK error *)
SAsInsertKs(p) ==
    /\ G(p) /\ Lbl(p) = "as_insert_ks"
    /\ LET o == op[p]
           d == NewDoc(p) IN
       IF fcv[o.f] = GONE
       THEN /\ Adv(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, locks, op, res, tgt, snap, loss, authv>>
       ELSE /\ dnv' = [dnv EXCEPT ![d] = o.f]
            /\ dw' = [dw EXCEPT ![d] = p]
            /\ Acquire(p, {<<R("d", d), "X">>}) /\ Adv(p)
            /\ UNCHANGED <<fcv, fnv, fw, dcv, op, res, tgt, snap, loss, authv>>

(* rest of store_all after apply_overrides (files, more inserts) before tx.commit() *)
SRest(p) ==
    /\ G(p) /\ Lbl(p) = "rest"
    /\ Adv(p)
    /\ UNCHANGED <<fcv, fnv, fw, dcv, dnv, dw, locks, op, res, tgt, snap, loss, authv>>

(* PostgreSQL deadlock detector: one transaction of a lock-wait cycle gets 40P01 *)
DeadlockAbort(p) ==
    /\ InCycle(p)
    /\ Abort(p, IF op[p].k = "consume" THEN "retry" ELSE "err_raw")

Finished == /\ \A p \in Procs : Done(p)
            /\ UNCHANGED vars

Next == \/ \E p \in Procs :
            \/ SRRSnap(p) \/ SLock(p) \/ SForUpd(p) \/ SChkParent(p) \/ SReadTree(p) \/ SUpdate(p)
            \/ SDelRead(p) \/ SChildren(p) \/ SDocs(p) \/ SDelete(p) \/ SCommit(p)
            \/ SDelSubs(p) \/ SDelDocsRead(p) \/ SDelParent(p) \/ SDelLockDocs(p) \/ SDelChkDocs(p)
            \/ SDocForUpd(p) \/ SRevoke(p)
            \/ SMvCheck(p) \/ SMvTarget(p) \/ SMvLockDocs(p) \/ SMvChkDocs(p)
            \/ SMvWrite(p) \/ SMvFk(p) \/ SMvDel(p)
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

(* P5: move-documents / DELETE with contents never commits a move whose permission check was made
   stale by a share/owner change committed between the check and the write (check-then-write gap) *)
NoStaleAuthz == ~unauth

(* P6: DELETE moves only subfolders/documents (and only into a target) that check_contents_movable checked *)
MovesChecked == ~unchecked

TypeOK ==
    /\ fcv \in [Folders -> PVal] /\ fnv \in [Folders -> PVal] /\ fw \in [Folders -> {0} \cup Procs]
    /\ dcv \in [AllDocs -> {ROOT} \cup Folders] /\ dw \in [AllDocs -> {0} \cup Procs]
    /\ fpc \in [Folders -> BOOLEAN] /\ fpw \in [Folders -> {0} \cup Procs]
    /\ dpc \in [AllDocs -> BOOLEAN] /\ dpw \in [AllDocs -> {0} \cup Procs]
=============================================================================
