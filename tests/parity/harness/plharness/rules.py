"""Accepted deviations as named rules.

Files: ``tests/parity/accepted.json`` (global, harness owner) and
``tests/parity/<crate>/accepted.json`` (crate owner). Format::

    {"rules": [
      {"name": "objects.document_count-v9",          # unique, stable
       "reason": "upstream bug X, see contract",     # required
       "ref": "docs/contract/pl-objects.md#known-differences",   # required
       "case": "tags.list*",                         # fnmatch on case id (default "*")
       "kind": ["value"],                            # optional list of diff kinds
       "path": "\\$\\.results\\[\\d+\\]\\.document_count",  # optional regex (fullmatch)
       "a": 3, "b": 4,                               # optional exact values
       "target": "b"                                 # optional; documentation only
      }
    ]}

A diff covered by a rule is counted as "accepted" (per rule) and does not make the case
"different". Rules without ``reason``/``ref`` are rejected at load time. Unused rules are
listed in the report (stale rules should be removed).
"""

from __future__ import annotations

import fnmatch
import json
import re
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from .diff import Diff

_MISSING = object()


@dataclass
class Rule:
    name: str
    reason: str
    ref: str
    source: str
    case: str = "*"
    kinds: list[str] | None = None
    path: re.Pattern | None = None
    a: Any = _MISSING
    b: Any = _MISSING
    hits: int = 0
    cases: set = field(default_factory=set)

    def matches(self, case_id: str, d: Diff) -> bool:
        if not fnmatch.fnmatchcase(case_id, self.case):
            return False
        if self.kinds and d.kind not in self.kinds:
            return False
        if self.path is not None and not self.path.fullmatch(d.path):
            return False
        if self.a is not _MISSING and d.a != self.a:
            return False
        if self.b is not _MISSING and d.b != self.b:
            return False
        return True


class RuleSet:
    def __init__(self) -> None:
        self.rules: list[Rule] = []
        self.errors: list[str] = []

    def load_file(self, p: Path) -> None:
        try:
            data = json.loads(p.read_text(encoding="utf-8"))
        except (OSError, ValueError) as e:
            self.errors.append(f"{p}: {e}")
            return
        names = {r.name for r in self.rules}
        for i, raw in enumerate(data.get("rules", [])):
            where = f"{p}#rules[{i}]"
            name = raw.get("name")
            if not name or not raw.get("reason") or not raw.get("ref"):
                self.errors.append(f"{where}: name, reason and ref are required")
                continue
            if name in names:
                self.errors.append(f"{where}: duplicate rule name {name!r}")
                continue
            names.add(name)
            try:
                pat = re.compile(raw["path"]) if raw.get("path") else None
            except re.error as e:
                self.errors.append(f"{where}: bad path regex: {e}")
                continue
            kinds = raw.get("kind")
            if isinstance(kinds, str):
                kinds = [kinds]
            self.rules.append(
                Rule(
                    name=name,
                    reason=raw["reason"],
                    ref=raw["ref"],
                    source=str(p),
                    case=raw.get("case", "*"),
                    kinds=kinds,
                    path=pat,
                    a=raw.get("a", _MISSING),
                    b=raw.get("b", _MISSING),
                )
            )

    def load_dir(self, parity_dir: Path) -> None:
        g = parity_dir / "accepted.json"
        if g.exists():
            self.load_file(g)
        for p in sorted(parity_dir.glob("*/accepted.json")):
            if p.parent.name == "harness":
                continue
            self.load_file(p)

    def classify(self, case_id: str, diffs: list[Diff]) -> tuple[list[Diff], list[tuple[Diff, str]]]:
        """Split diffs into (unaccepted, [(diff, rule name)])."""
        open_: list[Diff] = []
        accepted: list[tuple[Diff, str]] = []
        for d in diffs:
            rule = next((r for r in self.rules if r.matches(case_id, d)), None)
            if rule is None:
                open_.append(d)
            else:
                rule.hits += 1
                rule.cases.add(case_id)
                accepted.append((d, rule.name))
        return open_, accepted
