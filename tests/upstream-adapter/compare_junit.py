#!/usr/bin/env python3
"""Compares two JUnit XML files: baseline (in-process Django) vs. adapter run (HTTP target).

    python tests/upstream-adapter/compare_junit.py baseline.xml http.xml [--out results/upstream-adapter]

Classes:
  ok            passes in both
  regression    passes in-process, fails over HTTP  -> PaperRust differs OR adapter limit
  fixed         fails in-process, passes over HTTP
  both-fail     fails in both (environment problem, not a parity signal)
  skipped       skipped in either
  missing       only in one file
Writes <out>/<stamp>.json and .md and appends history.jsonl. Exit 1 if regressions.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import json
import sys
import xml.etree.ElementTree as ET
from collections import Counter
from pathlib import Path


def outcomes(path: Path) -> dict[str, tuple[str, str]]:
    tree = ET.parse(path)
    out: dict[str, tuple[str, str]] = {}
    for tc in tree.iter("testcase"):
        name = f"{tc.get('classname', '')}::{tc.get('name', '')}"
        state, msg = "pass", ""
        for child in tc:
            if child.tag in ("failure", "error"):
                state, msg = "fail", (child.get("message") or child.text or "")[:500]
            elif child.tag == "skipped":
                state, msg = "skip", (child.get("message") or "")[:200]
        out[name] = (state, msg)
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("baseline")
    ap.add_argument("http")
    ap.add_argument("--out", default=str(Path(__file__).resolve().parents[2] / "results" / "upstream-adapter"))
    args = ap.parse_args()
    a = outcomes(Path(args.baseline))
    b = outcomes(Path(args.http))
    rows = []
    for name in sorted(set(a) | set(b)):
        sa, ma = a.get(name, ("missing", ""))
        sb, mb = b.get(name, ("missing", ""))
        if "missing" in (sa, sb):
            cls = "missing"
        elif "skip" in (sa, sb):
            cls = "skipped"
        elif sa == sb == "pass":
            cls = "ok"
        elif sa == "pass":
            cls = "regression"
        elif sb == "pass":
            cls = "fixed"
        else:
            cls = "both-fail"
        rows.append({"test": name, "class": cls, "baseline": sa, "http": sb, "message": mb or ma})
    counts = Counter(r["class"] for r in rows)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    stamp = _dt.datetime.now().strftime("%Y-%m-%d_%H%M%S")
    (out / f"{stamp}.json").write_text(json.dumps({"counts": counts, "rows": rows}, indent=1), encoding="utf-8")
    with (out / "history.jsonl").open("a", encoding="utf-8") as f:
        f.write(json.dumps({"stamp": stamp, **counts}) + "\n")
    L = [f"# Upstream-Test-Adapter {stamp}", "", "| ok | regression | fixed | both-fail | skipped | missing |",
         "|---:|---:|---:|---:|---:|---:|",
         " | ".join(["", *(str(counts.get(k, 0)) for k in
                         ("ok", "regression", "fixed", "both-fail", "skipped", "missing")), ""]).strip(), ""]
    reg = [r for r in rows if r["class"] == "regression"]
    if reg:
        L += ["## Regressionen (in-process grün, über HTTP rot)", ""]
        for r in reg[:300]:
            L.append(f"- `{r['test']}`: {r['message'][:200].replace(chr(10), ' ')}")
    (out / f"{stamp}.md").write_text("\n".join(L) + "\n", encoding="utf-8")
    print(f"report: {out / (stamp + '.md')}  {dict(counts)}")
    return 1 if reg else 0


if __name__ == "__main__":
    sys.exit(main())
