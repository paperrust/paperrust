"""Generates `tests/data/w8_dateparser.tsv`: expressions per language and the result of dateparser itself.

    reference/venv/Scripts/python.exe -I -X utf8 crates/pl-docfilter/tools/gen_w8_corpus.py \
        crates/pl-docfilter/tests/data/w8_dateparser.tsv [--full]

Called like upstream `DateQueryField.to_internal_value` (src/documents/filters.py) with
`DATE_PARSER_LANGUAGES=[<lang>]`: `locales=[lang, "en"]`, `PARSERS=["relative-time"]`,
`RELATIVE_BASE=2024-08-22 23:30 UTC`. Candidates from the dateparser language data (units, `ago`, `in`,
number words, `relative-type`, `relative-type-regex`) in several word orders plus fixed edge cases.
Without `--full` a fixed, evenly spread selection per language is written (size of the test file).
"""

from __future__ import annotations

import datetime
import importlib
import re
import sys
from zoneinfo import ZoneInfo

import dateparser

LANGS = "en de fr es it nl pl pt sv cs da nb fi ru uk tr hu ro sk ca el".split()
UNITKEYS = ["decade", "year", "month", "week", "day", "hour", "minute", "second"]
BASE = datetime.datetime(2024, 8, 22, 23, 30, tzinfo=ZoneInfo("UTC"))
PER_LANG = 160
EXTRA = [
    "", "  ", "3", "days", "ago", "in", "3 days ago", "in 3 days", "in 3 days ago", "3 days 2 hours ago",
    "1 month 3 days ago", "in 1.5 days", "1.5 months ago", "+3 days", "-3 days", "3 decades ago",
    "in 2 decades", "3 Days Ago", "3days ago", "3d ago", "today", "yesterday", "now", "tomorrow",
    "last week", "next month", "a week ago", "999999 years ago", "in 99999999 days", "3 days ago 10:00",
    "inside 3 days", "1 year, 2 months ago", "3 weeks, ago", "foo 3 days", "January 2024",
]


def dp(expr: str, code: str) -> str:
    try:
        r = dateparser.parse(
            expr,
            locales=[code, "en"],
            settings={"PARSERS": ["relative-time"], "RELATIVE_BASE": BASE, "TIMEZONE": "UTC", "RETURN_AS_TIMEZONE_AWARE": True},
        )
    except (ValueError, OverflowError):
        return "None"
    return "None" if r is None else r.astimezone(BASE.tzinfo).date().isoformat()


def candidates(code: str) -> list[str]:
    info = importlib.import_module("dateparser.data.date_translation_data." + code).info
    units = sorted({w for k in UNITKEYS for w in info.get(k, [])})
    ago, fut = sorted(set(info.get("ago", []))), sorted(set(info.get("in", [])))
    nums = sorted(
        k for d in info.get("simplifications", []) for k, v in d.items()
        if re.fullmatch(r"\d+", str(v)) and re.fullmatch(r"[^\\()\[\]{}|*+?^$.]+", k)
    )
    cand = set(EXTRA)
    for v in info.get("relative-type", {}).values():
        cand.update(v)
    for pats in info.get("relative-type-regex", {}).values():
        for p in pats:
            e = re.sub(r"\(\\d\+\+?\[\.,\]\?\\d\*\+?\)", "3", p).replace("\\s*", " ").replace("\\s+", " ")
            if not re.search(r"[\\()\[\]|*+?]", e):
                cand.add(re.sub(" +", " ", e).strip())
    for u in units:
        for n in ("3", "1", "2,5"):
            cand.add(f"{n} {u}")
            cand.update(f"{a} {n} {u}" for a in ago + fut)
            cand.update(f"{n} {u} {a}" for a in ago + fut)
    for n in nums:
        for u in units[:12]:
            cand.update(f"{a} {n} {u}" for a in (ago[:1] + fut[:1]))
            cand.add(f"{n} {u} {ago[0]}" if ago else f"{n} {u}")
    return sorted(c for c in cand if "\t" not in c and "\n" not in c)


def main(out: str, full: bool) -> None:
    lines = [f"# erzeugt von tools/gen_w8_corpus.py mit dateparser {dateparser.__version__}; Basis 2024-08-22 23:30 UTC"]
    for code in LANGS:
        cand = candidates(code)
        if not full and len(cand) > PER_LANG:
            step = len(cand) / PER_LANG
            cand = [cand[int(i * step)] for i in range(PER_LANG)]
        for e in cand:
            lines.append(f"{code}\t{e}\t{dp(e, code)}")
        print(code, len(cand), flush=True)
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main(sys.argv[1], "--full" in sys.argv)
