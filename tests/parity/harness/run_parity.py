#!/usr/bin/env python3
"""Paritäts-Diff Django (A) vs. PaperRust (B).

Usage (from the repo root, Python >= 3.11, stdlib only):

    python tests/parity/harness/run_parity.py                 # all catalogues, A vs B
    python tests/parity/harness/run_parity.py --crate pl-objects --case 'tags.*'
    python tests/parity/harness/run_parity.py --targets a     # only Django: validates the
                                                              # catalogue + leak scan
    python tests/parity/harness/run_parity.py --list          # print expanded cases
    python tests/parity/harness/run_parity.py --no-write      # skip write cases (no reset)

Exit codes: 0 = no difference, no error, no leak in B
            1 = differences / errors / leaks in B / catalogue errors (with --strict)
            2 = setup problem (target unreachable, non-loopback target, no users)
"""

from __future__ import annotations

import argparse
import datetime as _dt
import fnmatch
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from plharness import cases as cases_mod  # noqa: E402
from plharness import config, report, rules, users  # noqa: E402
from plharness.http import NonLoopbackTarget, Session, assert_loopback  # noqa: E402
from plharness.runner import Runner  # noqa: E402


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--a-url")
    ap.add_argument("--b-url")
    ap.add_argument("--targets", choices=["ab", "a"], default="ab")
    ap.add_argument("--crate", action="append", help="limit to crate dir(s) under tests/parity/")
    ap.add_argument("--case", action="append", help="fnmatch on case id (base or expanded)")
    ap.add_argument("--user", action="append", help="limit user references")
    ap.add_argument("--no-write", action="store_true", help="skip write cases")
    ap.add_argument("--write-only", action="store_true", help="only write cases (e.g. W2 private copy)")
    ap.add_argument("--list", action="store_true", help="list expanded cases and exit")
    ap.add_argument("--strict", action="store_true", help="catalogue/rule errors fail the run")
    ap.add_argument("--results-dir", help="override results/parity")
    ap.add_argument("--users-file")
    args = ap.parse_args(argv)

    cfg = config.from_env()
    if args.a_url:
        cfg.a_url = args.a_url.rstrip("/")
    if args.b_url:
        cfg.b_url = args.b_url.rstrip("/")
    if args.users_file:
        cfg.users_file = args.users_file
    if args.results_dir:
        cfg.results_dir = Path(args.results_dir)
    started = _dt.datetime.now(_dt.timezone.utc)

    try:
        assert_loopback(cfg.a_url)
        if args.targets == "ab":
            assert_loopback(cfg.b_url)
    except NonLoopbackTarget as e:
        print(f"ERROR: {e}", file=sys.stderr)
        return 2

    catalog = cases_mod.load(cfg.parity_dir, args.crate)
    if args.case:
        catalog.cases = [c for c in catalog.cases
                         if any(fnmatch.fnmatchcase(c.id, p) or fnmatch.fnmatchcase(c.base_id, p) for p in args.case)]
    if args.user:
        catalog.cases = [c for c in catalog.cases if c.user in args.user]
    if args.write_only:
        catalog.cases = [c for c in catalog.cases if c.write]
    for e in catalog.errors:
        print(f"catalogue: {e}", file=sys.stderr)
    if args.list:
        for c in catalog.cases:
            print(f"{c.id}\t{c.route}\t{'write' if c.write else 'read'}\t{c.source}")
        print(f"{len(catalog.cases)} cases from {len(catalog.files)} files", file=sys.stderr)
        return 0

    ucat = users.load(cfg.users_file)
    if not ucat.users:
        print("ERROR: no users (reference/users.json, reference/README.md or PARITY_USERS_FILE)",
              file=sys.stderr)
        return 2

    for side in (["a"] if args.targets == "a" else ["a", "b"]):
        url = cfg.a_url if side == "a" else cfg.b_url
        try:
            Session(url, host_header=cfg.effective_host_header, timeout=10).get("/api/", follow=False)
        except OSError as e:
            print(f"ERROR: target {side.upper()} {url} unreachable: {e}", file=sys.stderr)
            return 2

    rs = rules.RuleSet()
    rs.load_dir(cfg.parity_dir)
    for e in rs.errors:
        print(f"rules: {e}", file=sys.stderr)

    print(f"{len(catalog.cases)} cases, users from {ucat.source}, targets={args.targets}")
    runner = Runner(cfg, ucat, rs, targets=args.targets, allow_write=not args.no_write)
    results = runner.run(catalog)
    meta = {
        "targets": args.targets,
        "filter": {"crate": args.crate, "case": args.case, "user": args.user} if (args.crate or args.case or args.user) else None,
        "catalogue_files": catalog.files,
        "catalogue_errors": catalog.errors,
        "users_source": ucat.source,
        "resets": runner.resets,
        "vars": {k: v for k, v in runner.var_values.items()},
        "var_errors": runner.var_errors,
        # ARCHITECTURE-V2 §4.3: run with installed sync change capture (set by w2-env.sh / caller)
        "sync_capture": (os.environ.get("PARITY_SYNC_CAPTURE") or "off").lower(),
    }
    jpath, mpath, summary = report.write(cfg, results, rs, meta, started)
    print(f"report: {mpath}")
    print(" ".join(f"{k}={v}" for k, v in summary.items()))
    bad = summary["different"] or summary["error"] or summary["leaks_b"]
    if args.strict and (catalog.errors or rs.errors):
        bad = True
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
