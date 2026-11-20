#!/usr/bin/env python3
"""Runs the security suite against one target and writes results/security/<stamp>-<target>.{json,md}.

    TARGET=django python tests/security/run_security.py
    TARGET=rust   python tests/security/run_security.py [-k pattern] [--no-ratelimit]

Exit: 0 green, 1 failures/errors, 2 target unreachable or no users.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import json
import os
import sys
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))


class Collect(unittest.TextTestResult):
    def __init__(self, *a, **kw):
        super().__init__(*a, **kw)
        self.records: list[dict] = []
        self.sub_failures: set[str] = set()

    def _rec(self, test, outcome, info=""):
        self.records.append({"test": test.id(), "outcome": outcome, "info": info[-2000:]})

    def addSuccess(self, test):
        super().addSuccess(test)
        self._rec(test, "pass")

    def addFailure(self, test, err):
        super().addFailure(test, err)
        self._rec(test, "fail", self._exc_info_to_string(err, test))

    def addError(self, test, err):
        super().addError(test, err)
        self._rec(test, "error", self._exc_info_to_string(err, test))

    def addSkip(self, test, reason):
        super().addSkip(test, reason)
        self._rec(test, "skip", reason)

    def addSubTest(self, test, subtest, err):
        super().addSubTest(test, subtest, err)
        if err is not None:
            kind = "fail" if issubclass(err[0], test.failureException) else "error"
            self._rec(subtest, kind, self._exc_info_to_string(err, test))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("-k", action="append", help="unittest -k pattern")
    ap.add_argument("--no-ratelimit", action="store_true")
    ap.add_argument("-v", action="store_true")
    args = ap.parse_args()
    if args.no_ratelimit:
        os.environ["SECURITY_SKIP_RATELIMIT"] = "1"

    import common  # noqa: F401  (validates TARGET, loopback)
    from plharness.http import Session

    try:
        Session(common.BASE_URL, timeout=5).get("/api/")
    except OSError as e:
        print(f"ERROR: target {common.BASE_URL} unreachable: {e}", file=sys.stderr)
        return 2
    if not common.USERS.users:
        print("ERROR: no users in catalogue", file=sys.stderr)
        return 2

    loader = unittest.TestLoader()
    if args.k:
        loader.testNamePatterns = [f"*{k}*" for k in args.k]
    suite = loader.discover(str(HERE), pattern="test_*.py", top_level_dir=str(HERE))
    started = _dt.datetime.now()
    runner = unittest.TextTestRunner(resultclass=Collect, verbosity=2 if args.v else 1)
    res: Collect = runner.run(suite)

    out = HERE.parent.parent / "results" / "security"
    out.mkdir(parents=True, exist_ok=True)
    stamp = started.strftime("%Y-%m-%d_%H%M%S") + f"-{common.TARGET}"
    counts = {k: sum(1 for r in res.records if r["outcome"] == k) for k in ("pass", "fail", "error", "skip")}
    doc = {"target": common.TARGET, "url": common.BASE_URL, "started": started.isoformat(),
           "users_source": common.USERS.source, "counts": counts, "records": res.records}
    (out / f"{stamp}.json").write_text(json.dumps(doc, indent=1), encoding="utf-8")
    lines = [f"# Sicherheits-Suite {stamp}", "", f"Ziel: `{common.TARGET}` {common.BASE_URL}", "",
             "| pass | fail | error | skip |", "|---:|---:|---:|---:|",
             f"| {counts['pass']} | {counts['fail']} | {counts['error']} | {counts['skip']} |", ""]
    for r in res.records:
        if r["outcome"] in ("fail", "error"):
            last = r["info"].strip().splitlines()[-1] if r["info"].strip() else ""
            lines.append(f"- **{r['outcome']}** `{r['test']}`: {last[:300]}")
    skips = [r for r in res.records if r["outcome"] == "skip"]
    if skips:
        lines += ["", "## Übersprungen", ""] + [f"- `{r['test']}`: {r['info']}" for r in skips]
    with (out / "history.jsonl").open("a", encoding="utf-8") as f:
        f.write(json.dumps({"stamp": stamp, **counts}) + "\n")
    (out / f"{stamp}.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"report: {out / (stamp + '.md')}")
    return 0 if res.wasSuccessful() else 1


if __name__ == "__main__":
    sys.exit(main())
