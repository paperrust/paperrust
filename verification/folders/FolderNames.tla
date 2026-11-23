----------------------------- MODULE FolderNames ----------------------------
(***************************************************************************)
(* PaperRust folder view: folder names stay unique per level (case-        *)
(* insensitive) under concurrent create / rename / move / delete, and a    *)
(* clash is always answered with the mapped message, never a 500.          *)
(*                                                                         *)
(* Code: crates/pl-api-v2/src/folders.rs create, patch, delete, write_err; *)
(* unique index paperrust_folder_name_uniq ON paperrust_folder             *)
(* (coalesce(parent_id, 0), lower(name)) in                                *)
(* crates/pl-api-v2/migrations/0002_paperrust_folder.sql. The code has no  *)
(* name check of its own: it relies on the index (23505 -> write_err).     *)
(*                                                                         *)
(* Same conventions as FolderTree.tla: one action per SQL statement,       *)
(* committed row (cv), pending row (nv) and writer (w), READ COMMITTED,    *)
(* locks <<proc, resource, mode>>. A unique-index insert that meets a      *)
(* version of an open transaction waits for it (PostgreSQL                 *)
(* _bt_check_unique), then errors if the clash is committed.               *)
(***************************************************************************)
EXTENDS Integers, FiniteSets, Sequences, TLC

CONSTANTS
    Folders,     \* folder ids existing at the start
    Names,       \* folder names, e.g. {"a", "A", "b"}; "A" and "a" clash (lower(name))
    NProcs,
    Kinds,       \* subset of {"create", "rename", "patchmove", "delete"}
    UseIndex     \* code: TRUE (paperrust_folder_name_uniq); broken variant: FALSE

Procs == 1..NProcs
NewF(p) == 10 + p                     \* id of the folder created by request p
Rows == Folders \cup (IF "create" \in Kinds THEN {NewF(p) : p \in Procs} ELSE {})
Low(n) == IF n = "A" THEN "a" ELSE n
Row(e, par, n) == [e |-> e, p |-> par, n |-> n]
NoRow == Row(FALSE, 0, "a")

(* create: POST /folders {name, parent}; rename: PATCH {"name"}; patchmove: PATCH {"parent": t};
   delete: DELETE ?contents=parent (subfolders move to the parent level) *)
Op(k, f, t, n) == [k |-> k, f |-> f, t |-> t, n |-> n]
Ops ==
       (IF "create" \in Kinds THEN {Op("create", 0, t, n) : t \in {0} \cup Folders, n \in Names} ELSE {})
  \cup (IF "rename" \in Kinds THEN {Op("rename", f, 0, n) : f \in Folders, n \in Names} ELSE {})
  \cup (IF "patchmove" \in Kinds THEN {o \in {Op("patchmove", f, t, "a") : f \in Folders, t \in Folders} : o.f # o.t} ELSE {})
  \cup (IF "delete" \in Kinds THEN {Op("delete", f, 0, "a") : f \in Folders} ELSE {})

Steps(o) ==
  CASE o.k = "create" ->
         \* create: tree_lock + check_parent only with a parent; INSERT .. RETURNING id; commit
         IF o.t = 0 THEN <<"insert", "commit">> ELSE <<"lock", "chkparent", "insert", "commit">>
    [] o.k = "rename"    -> <<"forupd", "update", "commit">>                 \* patch without parent: no tree_lock
    [] o.k = "patchmove" -> <<"lock", "forupd", "chkparent", "readtree", "update", "commit">>
    [] o.k = "delete"    -> <<"lock", "forupd", "children", "delete", "commit">>

VARIABLES cv, nv, w, locks, op, pc, res
vars == <<cv, nv, w, locks, op, pc, res>>

Lbl(p) == Steps(op[p])[pc[p]]
Done(p) == pc[p] > Len(Steps(op[p]))
V(p, r) == IF w[r] = p THEN nv[r] ELSE cv[r]

RECURSIVE Walk(_, _, _, _)
Walk(p, f, cur, n) == IF cur = 0 \/ ~V(p, cur).e THEN FALSE ELSE IF cur = f THEN TRUE
                      ELSE IF n = 0 THEN TRUE ELSE Walk(p, f, V(p, cur).p, n - 1)

(* Row versions request p wants to write in its next statement: set of <<row, new version>> *)
Writes(p) ==
  LET o == op[p] IN
  IF Done(p) THEN {} ELSE
  CASE Lbl(p) = "insert"   -> {<<NewF(p), Row(TRUE, o.t, o.n)>>}
    [] Lbl(p) = "update"   -> {<<o.f, IF o.k = "rename" THEN Row(TRUE, V(p, o.f).p, o.n)
                                      ELSE Row(TRUE, o.t, V(p, o.f).n)>>}
    [] Lbl(p) = "children" -> {<<c, Row(TRUE, V(p, o.f).p, V(p, c).n)>> : c \in {c \in Rows \ {o.f} : V(p, c).e /\ V(p, c).p = o.f}}
    [] OTHER               -> {}

Clash(a, b) == a.e /\ b.e /\ a.p = b.p /\ Low(a.n) = Low(b.n)
(* index entries that differ from the row's current one (unchanged key: no new index entry) *)
NewKey(p, r, v) == ~(V(p, r).e /\ V(p, r).p = v.p /\ Low(V(p, r).n) = Low(v.n))
(* paperrust_folder_name_uniq: wait for open transactions holding a clashing version *)
UniqWaitFor(p) ==
  IF ~UseIndex THEN {} ELSE
  {w[r2] : r2 \in {r2 \in Rows : w[r2] \notin {0, p}
                     /\ \E x \in Writes(p) : x[1] # r2 /\ NewKey(p, x[1], x[2]) /\ (Clash(x[2], cv[r2]) \/ Clash(x[2], nv[r2]))}}
(* ... and fail (23505) on a committed or own clashing version *)
UniqError(p) ==
  /\ UseIndex
  /\ \/ \E x \in Writes(p), r2 \in Rows :
           /\ x[1] # r2 /\ NewKey(p, x[1], x[2]) /\ Clash(x[2], V(p, r2))
           /\ \A y \in Writes(p) : y[1] # r2    \* rows moved by the same statement count with their new version
     \/ \E x, y \in Writes(p) : x[1] # y[1] /\ Clash(x[2], y[2])

R(kind, id) == <<kind, id>>
Need(p) ==
  LET o == op[p] IN
  IF Done(p) THEN {} ELSE
  CASE Lbl(p) = "lock"     -> {<<R("t", 0), "X">>}
    [] Lbl(p) = "forupd"   -> {<<R("f", o.f), "X">>}
    [] Lbl(p) = "insert"   -> IF o.t # 0 THEN {<<R("f", o.t), "KS">>} ELSE {}
    [] Lbl(p) = "update"   -> IF o.k = "patchmove" THEN {<<R("f", o.t), "KS">>} ELSE {}
    [] Lbl(p) = "children" -> {<<R("f", x[1]), "NK">> : x \in Writes(p)}
                              \cup (IF Writes(p) # {} /\ V(p, o.f).p # 0 THEN {<<R("f", V(p, o.f).p), "KS">>} ELSE {})
    [] Lbl(p) = "delete"   -> {<<R("f", o.f), "X">>}
    [] OTHER               -> {}

Conflict(m1, m2) == m1 = "X" \/ m2 = "X" \/ (m1 = "NK" /\ m2 = "NK")
Blockers(p) == {q \in Procs \ {p} : \E n \in Need(p), l \in locks : l[1] = q /\ l[2] = n[1] /\ Conflict(n[2], l[3])}
               \cup UniqWaitFor(p)
G(p) == ~Done(p) /\ Blockers(p) = {}
Acquire(p) == locks' = locks \cup {<<p, n[1], n[2]>> : n \in Need(p)}

RECURSIVE ReachN(_, _)
ReachN(S, n) == IF n = 0 THEN S ELSE ReachN(S \cup UNION {Blockers(r) : r \in S}, n - 1)
InCycle(p) == ~Done(p) /\ p \in ReachN(Blockers(p), NProcs)

UniqueStart(t) == \A a, b \in Folders : a # b => ~Clash(t[a], t[b])
RECURSIVE Cyc(_, _, _, _)
Cyc(t, f, c, n) == IF c = 0 THEN FALSE ELSE IF c = f THEN TRUE ELSE IF n = 0 THEN TRUE ELSE Cyc(t, f, t[c].p, n - 1)

Init ==
    /\ cv \in {t \in [Rows -> {Row(TRUE, par, n) : par \in {0} \cup Folders, n \in Names} \cup {NoRow}] :
                 /\ \A r \in Rows : (r \in Folders) = t[r].e
                 /\ \A r \in Rows \ Folders : t[r] = NoRow
                 /\ UniqueStart(t)
                 /\ \A f \in Folders : ~Cyc(t, f, t[f].p, Cardinality(Folders))}
    /\ nv = cv
    /\ w = [r \in Rows |-> 0]
    /\ locks = {}
    /\ op \in [Procs -> Ops]
    /\ pc = [p \in Procs |-> 1]
    /\ res = [p \in Procs |-> "none"]

Adv(p) == pc' = [pc EXCEPT ![p] = @ + 1]
Abort(p, kind) ==
    /\ nv' = [r \in Rows |-> IF w[r] = p THEN cv[r] ELSE nv[r]]
    /\ w' = [r \in Rows |-> IF w[r] = p THEN 0 ELSE w[r]]
    /\ locks' = {l \in locks : l[1] # p}
    /\ res' = [res EXCEPT ![p] = kind]
    /\ pc' = [pc EXCEPT ![p] = Len(Steps(op[p])) + 1]
    /\ UNCHANGED <<cv, op>>

(* apply Writes(p) as pending versions *)
DoWrites(p) ==
    /\ nv' = [r \in Rows |-> IF \E x \in Writes(p) : x[1] = r THEN CHOOSE v \in {x[2] : x \in {y \in Writes(p) : y[1] = r}} : TRUE ELSE nv[r]]
    /\ w' = [r \in Rows |-> IF \E x \in Writes(p) : x[1] = r THEN p ELSE w[r]]

SLock(p) == /\ G(p) /\ Lbl(p) = "lock"
            /\ Acquire(p) /\ Adv(p) /\ UNCHANGED <<cv, nv, w, op, res>>

(* visible_folder(.., true): SELECT .. FOR UPDATE; gone -> 404 *)
SForUpd(p) == /\ G(p) /\ Lbl(p) = "forupd"
              /\ IF ~V(p, op[p].f).e THEN Abort(p, "err_mapped")
                 ELSE Acquire(p) /\ Adv(p) /\ UNCHANGED <<cv, nv, w, op, res>>

(* check_parent: parent gone -> 400 fields.parent *)
SChkParent(p) == /\ G(p) /\ Lbl(p) = "chkparent"
                 /\ IF ~V(p, op[p].t).e THEN Abort(p, "err_mapped")
                    ELSE Adv(p) /\ UNCHANGED <<cv, nv, w, locks, op, res>>

(* patch: creates_cycle -> 400 fields.parent *)
SReadTree(p) == /\ G(p) /\ Lbl(p) = "readtree"
                /\ IF Walk(p, op[p].f, op[p].t, Cardinality(Rows)) THEN Abort(p, "err_mapped")
                   ELSE Adv(p) /\ UNCHANGED <<cv, nv, w, locks, op, res>>

(* create: INSERT INTO paperrust_folder; patch: UPDATE paperrust_folder SET name, parent_id ..
   23505 -> write_err -> 400 fields.name "A folder with this name already exists here."; 23503 -> 400 fields.parent *)
SWrite(p) == /\ G(p) /\ Lbl(p) \in {"insert", "update"}
             /\ LET par == IF op[p].k = "rename" THEN 0 ELSE op[p].t IN
                IF par # 0 /\ ~cv[par].e THEN Abort(p, "err_mapped")
                ELSE IF UniqError(p) THEN Abort(p, "err_mapped")
                ELSE DoWrites(p) /\ Acquire(p) /\ Adv(p) /\ UNCHANGED <<cv, op, res>>

(* delete: UPDATE paperrust_folder SET parent_id = target WHERE parent_id = f; 23505 -> 409 Conflict
   "A subfolder has the same name as a folder at the target level. Rename it first." *)
SChildren(p) == /\ G(p) /\ Lbl(p) = "children"
                /\ IF UniqError(p) THEN Abort(p, "err_mapped")
                   ELSE DoWrites(p) /\ Acquire(p) /\ Adv(p) /\ UNCHANGED <<cv, op, res>>

(* delete: DELETE FROM paperrust_folder WHERE id = f (a subfolder still pointing at it -> 500) *)
SDelete(p) == /\ G(p) /\ Lbl(p) = "delete"
              /\ IF \E c \in Rows \ {op[p].f} : V(p, c).e /\ V(p, c).p = op[p].f THEN Abort(p, "err_raw")
                 ELSE /\ nv' = [nv EXCEPT ![op[p].f] = NoRow]
                      /\ w' = [w EXCEPT ![op[p].f] = p]
                      /\ Acquire(p) /\ Adv(p) /\ UNCHANGED <<cv, op, res>>

SCommit(p) == /\ G(p) /\ Lbl(p) = "commit"
              /\ cv' = [r \in Rows |-> IF w[r] = p THEN nv[r] ELSE cv[r]]
              /\ w' = [r \in Rows |-> IF w[r] = p THEN 0 ELSE w[r]]
              /\ locks' = {l \in locks : l[1] # p}
              /\ res' = [res EXCEPT ![p] = "ok"]
              /\ pc' = [pc EXCEPT ![p] = Len(Steps(op[p])) + 1]
              /\ UNCHANGED <<nv, op>>

DeadlockAbort(p) == InCycle(p) /\ Abort(p, "err_raw")

Finished == (\A p \in Procs : Done(p)) /\ UNCHANGED vars

Next == \/ \E p \in Procs :
            \/ SLock(p) \/ SForUpd(p) \/ SChkParent(p) \/ SReadTree(p) \/ SWrite(p)
            \/ SChildren(p) \/ SDelete(p) \/ SCommit(p) \/ DeadlockAbort(p)
        \/ Finished

Spec == Init /\ [][Next]_vars

(* P5: no two live folders at the same level whose names differ only in case *)
UniqueSiblingNames == \A a, b \in Rows : a # b => ~Clash(cv[a], cv[b])
(* P3 (names part): a clash is answered with the mapped 400/409 message, never a 500 *)
NoRawError == \A p \in Procs : res[p] # "err_raw"
=============================================================================
