"""Executes the case catalogue against target A (Django) and B (PaperRust)."""

from __future__ import annotations

import json
import os
import subprocess
import time
from dataclasses import dataclass, field
from typing import Any
from urllib.parse import parse_qs, urlsplit

from . import cases as cases_mod
from .cases import Case, Catalog, Unresolved
from .config import REFERENCE_DIR, REPO_DIR, Config
from .diff import Diff, compare
from .http import Response, Session, session_login
from .leaks import LeakScanner
from .normalize import Normalizer
from .rules import RuleSet
from .users import UserCatalog


@dataclass
class CaseResult:
    id: str
    base_id: str
    crate: str
    route: str
    user: str
    api_version: str
    outcome: str  # identical | different | accepted | skipped | error | a-only
    reason: str = ""
    diffs: list[dict] = field(default_factory=list)
    accepted: list[dict] = field(default_factory=list)
    steps: list[dict] = field(default_factory=list)
    leaks_a: list[dict] = field(default_factory=list)
    leaks_b: list[dict] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)
    normalized: dict[str, int] = field(default_factory=dict)

    def to_dict(self) -> dict:
        return {k: v for k, v in self.__dict__.items() if v not in ([], {}, "")}


class ResetError(RuntimeError):
    pass


class SqlUnavailable(RuntimeError):
    pass


def run_sql(query: str) -> Response:
    """State check of a write case: ``{"sql": "select ..."}`` step.

    Runs the query read-only (``default_transaction_read_only``) against the database named in
    ``PARITY_SQL_DB`` (the private copy the write case just changed, e.g. ``pl_w2_ref``) and
    returns the rows as a JSON array in a synthetic 200 response, so the normal comparison
    and normalisation (timestamps >= cutoff) apply. Without ``PARITY_SQL_DB`` the case is
    skipped - SQL checks never default to the shared reference database.
    """
    db = os.environ.get("PARITY_SQL_DB")
    if not db:
        raise SqlUnavailable("sql step needs PARITY_SQL_DB (private database copy)")
    psql = os.environ.get("PARITY_PSQL") or str(REFERENCE_DIR / "pg" / "pgsql" / "bin" / "psql.exe")
    env = dict(os.environ, PGPASSWORD=os.environ.get("PARITY_SQL_PASSWORD", "paperless"))
    wrapped = f"select coalesce(json_agg(t), '[]'::json) from ({query.rstrip().rstrip(';')}) t"
    t0 = time.perf_counter()
    r = subprocess.run(
        [psql, "-X", "-q", "-At", "-v", "ON_ERROR_STOP=1",
         "-h", os.environ.get("PARITY_SQL_HOST", "127.0.0.1"),
         "-p", os.environ.get("PARITY_SQL_PORT", "54329"),
         "-U", os.environ.get("PARITY_SQL_USER", "paperless"), "-d", db,
         "-c", "set default_transaction_read_only = on", "-c", wrapped],
        capture_output=True, env=env, timeout=120,
    )
    ms = (time.perf_counter() - t0) * 1000
    if r.returncode != 0:
        msg = r.stderr.decode("utf-8", "replace").strip()[-300:]
        body = json.dumps({"sql_error": msg}).encode()
        return Response(599, "SQL error", [("Content-Type", "application/json")], body, ms, "SQL", "sql:")
    out = r.stdout.decode("utf-8").strip()
    return Response(200, "OK", [("Content-Type", "application/json")], out.encode("utf-8"), ms, "SQL", "sql:")


class Runner:
    def __init__(
        self,
        cfg: Config,
        users: UserCatalog,
        rules: RuleSet,
        *,
        targets: str = "ab",
        allow_write: bool = True,
        log=print,
    ) -> None:
        self.cfg = cfg
        self.users = users
        self.rules = rules
        self.targets = targets
        self.allow_write = allow_write
        self.log = log
        self.scanner = LeakScanner(users.secrets(), cfg.sensitive_paths)
        self._sessions: dict[tuple[str, str, str], Session] = {}
        self.var_values: dict[str, Any] = {}
        self.var_errors: dict[str, str] = {}
        self.resets = 0

    # ---------------------------------------------------------------- sessions
    def base(self, side: str) -> str:
        return self.cfg.a_url if side == "a" else self.cfg.b_url

    def session(self, side: str, user_ref: str, api_version: str) -> Session | None:
        key = (side, user_ref, api_version)
        if key in self._sessions:
            return self._sessions[key]
        res = self.users.resolve(user_ref)
        if res is None:
            return None
        mode, auth, user = res
        s = Session(self.base(side), host_header=self.cfg.effective_host_header,
                    timeout=self.cfg.timeout, auth=auth, api_version=api_version)
        if mode == "session":
            # share one login per side+user across API versions
            shared = self._sessions.get((side, user_ref, "*"))
            if shared is None:
                shared = Session(self.base(side), host_header=self.cfg.effective_host_header,
                                 timeout=self.cfg.timeout)
                if not session_login(shared, user.username, user.password):
                    raise RuntimeError(f"session login failed for {user_ref} on {side}")
                self._sessions[(side, user_ref, "*")] = shared
            s.cookies = dict(shared.cookies)
            s.auth = ("session",)
        self._sessions[key] = s
        return s

    def drop_sessions(self) -> None:
        self._sessions.clear()

    # ---------------------------------------------------------------- reset
    def reset(self) -> None:
        if not self.cfg.reset_cmd:
            raise ResetError("no reset command (reference/reset.sh missing, PARITY_RESET_CMD unset)")
        t0 = time.perf_counter()
        r = subprocess.run(self.cfg.reset_cmd, cwd=str(REPO_DIR), capture_output=True, text=True,
                           timeout=600)
        if r.returncode != 0:
            raise ResetError(f"reset failed ({r.returncode}): {r.stderr.strip()[-500:]}")
        self.resets += 1
        self.drop_sessions()  # sessions may not survive a snapshot restore
        self.log(f"  reset ok ({(time.perf_counter() - t0):.1f}s)")

    # ---------------------------------------------------------------- vars
    def resolve_vars(self, catalog: Catalog) -> None:
        pending = dict(catalog.vars)
        for _ in range(len(pending) + 1):
            progressed = False
            for name, spec in list(pending.items()):
                deps = cases_mod.vars_in(spec)
                if deps - set(self.var_values):
                    if deps & set(self.var_errors):
                        self.var_errors[name] = f"depends on unresolved {sorted(deps & set(self.var_errors))}"
                        pending.pop(name)
                        progressed = True
                    continue
                pending.pop(name)
                progressed = True
                try:
                    self.var_values[name] = self._resolve_var(cases_mod.substitute(spec, self.var_values))
                except Exception as e:  # noqa: BLE001 - recorded, case gets skipped
                    if "default" in spec:
                        self.var_values[name] = spec["default"]
                    else:
                        self.var_errors[name] = f"{type(e).__name__}: {e}"
            if not pending or not progressed:
                break
        for name in pending:
            self.var_errors[name] = "unresolvable (cyclic or missing dependency)"

    def _resolve_var(self, spec: dict) -> Any:
        if "value" in spec:
            return spec["value"]
        s = self.session("a", spec.get("user", "admin"), str(spec.get("api_version", "10")))
        if s is None:
            raise KeyError(f"user {spec.get('user', 'admin')} not in catalogue")
        r = s.request(spec.get("method", "GET"), spec["get"], query=spec.get("query"),
                      json_body=spec.get("body"))
        if r.status >= 400:
            raise RuntimeError(f"HTTP {r.status} for {spec['get']}")
        doc = cases_mod.json_path(r.json(), spec.get("json", "$"))
        where = spec.get("where")
        where_not = spec.get("where_not")
        if where or where_not:
            if not isinstance(doc, list):
                raise TypeError("where needs a list")
            sel = [x for x in doc
                   if all(cases_mod.json_path(x, k) == v for k, v in (where or {}).items())
                   and not any(cases_mod.json_path(x, k) == v for k, v in (where_not or {}).items())]
            if not sel:
                raise LookupError("no element matches where/where_not")
            doc = sel[0]
        if spec.get("field"):
            doc = cases_mod.json_path(doc, spec["field"])
        if doc is None:
            raise LookupError("value is null")
        return doc

    # ---------------------------------------------------------------- run
    def run(self, catalog: Catalog) -> list[CaseResult]:
        self.resolve_vars(catalog)
        reads = [c for c in catalog.cases if not c.write]
        writes = [c for c in catalog.cases if c.write]
        results: list[CaseResult] = []
        for c in reads:
            results.append(self.run_case(c))
        if writes:
            if not self.allow_write:
                for c in writes:
                    results.append(self._skip(c, "write cases disabled (--no-write)"))
            elif not self.cfg.reset_cmd:
                for c in writes:
                    results.append(self._skip(c, "write case needs snapshot reset; reference/reset.sh missing"))
            else:
                try:
                    self.reset()
                except ResetError as e:
                    for c in writes:
                        results.append(self._skip(c, f"reset failed: {e}"))
                    return results
                for c in writes:
                    results.append(self.run_case(c))
        return results

    def _skip(self, c: Case, reason: str) -> CaseResult:
        return CaseResult(c.id, c.base_id, c.crate, c.route, c.user, c.api_version, "skipped", reason)

    def run_case(self, c: Case) -> CaseResult:
        res = CaseResult(c.id, c.base_id, c.crate, c.route, c.user, c.api_version, "identical")
        if c.raw.get("skip"):
            res.outcome, res.reason = "skipped", str(c.raw["skip"])
            return res
        needed = cases_mod.vars_in(c.raw) | set(c.raw.get("requires", []))
        captured_names: set[str] = set()
        for st in c.raw.get("steps") or []:
            captured_names |= set((st.get("capture") or {}).keys())
        needed -= captured_names
        missing = sorted(n for n in needed if n not in self.var_values)
        if missing:
            why = "; ".join(f"{n}: {self.var_errors.get(n, 'not defined')}" for n in missing)
            res.outcome, res.reason = "skipped", f"unresolved vars ({why})"
            return res
        if self.users.resolve(c.user) is None:
            res.outcome, res.reason = "skipped", f"user {c.user!r} not in catalogue ({self.users.source})"
            return res
        sides = "a" if self.targets == "a" else "ab"
        try:
            per_side: dict[str, list[Response]] = {}
            for side in sides:
                if c.write and side == "b":
                    self.reset()
                per_side[side] = self._run_steps(c, side)
            if c.write:
                self.reset()
        except ResetError as e:
            res.outcome, res.reason = "error", f"reset: {e}"
            return res
        except Unresolved as e:
            res.outcome, res.reason = "skipped", f"unresolved capture/var {e}"
            return res
        except SqlUnavailable as e:
            if c.write:
                try:
                    self.reset()
                except ResetError:
                    pass
            res.outcome, res.reason = "skipped", str(e)
            return res
        except (OSError, RuntimeError) as e:
            res.outcome, res.reason = "error", f"{type(e).__name__}: {e}"
            return res

        steps = c.steps()
        all_open: list[Diff] = []
        for i, st in enumerate(steps):
            ra = per_side["a"][i]
            allow = set(st.get("leak_allow") or [])
            res.leaks_a += [dict(l.to_dict(), step=i) for l in self.scanner.scan_response(ra, allow)]
            exp = st.get("expect_status") if "sql" not in st else [200]
            if exp and ra.status not in exp:
                res.warnings.append(f"step {i}: A returned {ra.status}, catalogue expects {exp}")
            info = {"method": ra.method, "url": ra.url.replace(self.cfg.a_url, ""),
                    "a": {"status": ra.status, "ms": round(ra.elapsed_ms, 1)}}
            if "b" in per_side:
                rb = per_side["b"][i]
                res.leaks_b += [dict(l.to_dict(), step=i) for l in self.scanner.scan_response(rb, allow)]
                info["b"] = {"status": rb.status, "ms": round(rb.elapsed_ms, 1)}
                norm = Normalizer(self.cfg.cutoff, write=c.write)
                diffs = self._compare(ra, rb, st, norm)
                for k, v in norm.applied.items():
                    res.normalized[k] = res.normalized.get(k, 0) + v
                if len(steps) > 1:
                    for d in diffs:
                        d.path = f"step{i}:{d.path}"
                all_open += diffs
            res.steps.append(info)

        if "b" not in per_side:
            res.outcome = "a-only"
            return res
        open_, accepted = self.rules.classify(c.id, all_open)
        res.diffs = [d.to_dict() for d in open_]
        res.accepted = [dict(d.to_dict(), rule=name) for d, name in accepted]
        if open_:
            res.outcome = "different"
        elif accepted:
            res.outcome = "accepted"
        return res

    def _run_steps(self, c: Case, side: str) -> list[Response]:
        values = dict(self.var_values)
        out: list[Response] = []
        for st in c.steps():
            st = cases_mod.substitute(st, values)
            if "sql" in st:
                out.append(run_sql(st["sql"]))
                continue
            user = st.get("user", c.user)
            s = self.session(side, user, c.api_version)
            if s is None:
                raise RuntimeError(f"user {user} unavailable")
            r = s.request(
                st.get("method", "GET"),
                st["path"],
                query=st.get("query"),
                json_body=st.get("json"),
                form=st.get("form"),
                data=st["data"].encode() if isinstance(st.get("data"), str) else st.get("data"),
                content_type=st.get("content_type"),
                headers=st.get("headers"),
                follow=bool(st.get("follow")),
                csrf=st.get("csrf", True),
            )
            out.append(r)
            for name, path in (st.get("capture") or {}).items():
                try:
                    values[name] = cases_mod.json_path(r.json(), path)
                except (KeyError, IndexError, TypeError, ValueError):
                    raise Unresolved(f"{name} from {path} (HTTP {r.status})") from None
        return out

    def _compare(self, ra: Response, rb: Response, st: dict, norm: Normalizer) -> list[Diff]:
        mode = st.get("compare", "full")
        ha = norm.headers(ra.headers, st.get("compare_headers"))
        hb = norm.headers(rb.headers, st.get("compare_headers"))
        a_json = ra.is_json and ra.json() is not None
        b_json = rb.is_json and rb.json() is not None
        ordering = _ordering_param(ra.url)
        if a_json:
            ba = norm.json(ra.json(), ordering)
        elif ra.content_type.startswith("text/"):
            ba = norm.text(ra.text)
        else:
            ba = ra.body
        if b_json:
            bb = norm.json(rb.json(), ordering)
        elif rb.content_type.startswith("text/"):
            bb = norm.text(rb.text)
        else:
            bb = rb.body
        if mode == "json-only":
            ha, hb, mode = {}, {}, "full"
        return compare(ra.status, rb.status, ha, hb, ba, bb, a_json, b_json, mode)


def _ordering_param(url: str) -> str | None:
    """``ordering`` of the request (for the tie rule in ``Normalizer.json``)."""
    v = parse_qs(urlsplit(url).query).get("ordering")
    return v[0] if v else None
