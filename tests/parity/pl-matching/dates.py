"""
Parity reference for pl-matching date detection (and fuzzy/fnmatch helpers).

Runs the *unchanged* upstream code (upstream/src/documents/plugins/date_parsing/
base.py + regex_parser.py, documents/regex.py) with the Python environment in
reference/venv (dateparser 1.4.3, regex, rapidfuzz) and writes
tests/parity/pl-matching/dates_reference.json. The Rust test
crates/pl-matching/tests/parity.rs compares its results against that file.

dateparser reads the wall clock in a few places (missing day/month, relative
words like "gestern", strptime's current year). To make the reference
deterministic, `datetime` is frozen to REFERENCE_NOW inside those dateparser
modules; the Rust side uses the same instant as "now".

Usage (from the repository root):
    reference/venv/Scripts/python.exe -X utf8 tests/parity/pl-matching/dates.py
"""

from __future__ import annotations

import datetime as _dt
import importlib.util
import json
import sys
import types
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
UPSTREAM = ROOT / "upstream" / "src"
OUT = Path(__file__).resolve().parent / "dates_reference.json"

REFERENCE_NOW = _dt.datetime(2024, 3, 15, 12, 0, 0, tzinfo=_dt.UTC)

# --- minimal Django settings for documents/regex.py -------------------------
import django
from django.conf import settings as dj_settings

if not dj_settings.configured:
    dj_settings.configure(MATCH_REGEX_TIMEOUT_SECONDS=0.1, USE_TZ=True, TIME_ZONE="UTC")
django.setup()


def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    mod = importlib.util.module_from_spec(spec)
    sys.modules[name] = mod
    spec.loader.exec_module(mod)
    return mod


# package stubs so that the plugin modules can be loaded without documents/__init__
for pkg in ("documents", "documents.plugins", "documents.plugins.date_parsing"):
    m = types.ModuleType(pkg)
    m.__path__ = []
    sys.modules[pkg] = m
_load("documents.regex", UPSTREAM / "documents" / "regex.py")
base = _load(
    "documents.plugins.date_parsing.base",
    UPSTREAM / "documents" / "plugins" / "date_parsing" / "base.py",
)
regex_parser = _load(
    "documents.plugins.date_parsing.regex_parser",
    UPSTREAM / "documents" / "plugins" / "date_parsing" / "regex_parser.py",
)

# --- freeze dateparser's clock ---------------------------------------------
import dateparser.freshness_date_parser as dp_fresh  # noqa: E402
import dateparser.parser as dp_parser  # noqa: E402
import dateparser.utils.strptime as dp_strptime  # noqa: E402


class FrozenDatetime(_dt.datetime):
    @classmethod
    def now(cls, tz=None):
        if tz is None:
            return REFERENCE_NOW.replace(tzinfo=None)
        return REFERENCE_NOW.astimezone(tz)

    @classmethod
    def today(cls):
        return REFERENCE_NOW.replace(tzinfo=None)


dp_fresh.datetime = FrozenDatetime
dp_parser.datetime = FrozenDatetime
dp_strptime.datetime = FrozenDatetime

from rapidfuzz import fuzz  # noqa: E402
import fnmatch  # noqa: E402

TEXTS = [
    # English
    "Invoice date: 15 Jan 2024. Due: January 30, 2024. Order 2023-12-01.",
    "Taken on 01/02/2023 and 13/02/2023, also 12.31.2022",
    "Statement for March 2023 issued 1st April 2023",
    "Monday 2024 report, Total 1234, Sept 5, 2021",
    "bad date 99/99/9999 and 25/12/2022 and 31/02/2024",
    "Version 1.2.3 build 4.5.2010, phone 030-123-4567",
    "Printed 15-Jan-2024; archived 2019_05_06 ; x 05.06.2019_",
    "Dates 1.2.20245 2.3.19 and 10/11/12",
    "Today 2024 and Yesterday 2023",
    # German
    "Rechnung vom 15. März 2024, fällig am 1. April 2024. Gestern 2024 bezahlt.",
    "Kontoauszug Januar 2024, erstellt 03.02.2024, Jahr 2023",
    "Lieferdatum: 5. Dezember 2023 – Vertrag vom 12. Mai 2020",
    # French
    "Facture du 3 février 2024, échéance le 1er mars 2024",
    "Paris, le 15 août 2022. Mardi 2021",
    # Spanish / Italian / Portuguese / Dutch
    "Factura del 5 de mayo de 2023 y 12 enero 2022",
    "Fattura del 12 gennaio 2022, scadenza 28 febbraio 2022",
    "Lisboa, 7 de outubro de 2021 e 3 dez 2020",
    "Factuur 14 juni 2023, vervaldatum 1 juli 2023",
    # Russian / Czech / Polish / Turkish / Swedish
    "Счёт от 15 марта 2023 г. и 1 апреля 2023",
    "Faktura ze dne 3. května 2022, splatnost 17. června 2022",
    "Faktura z dnia 12 lipca 2021 r.",
    "Fatura tarihi 5 Şubat 2024",
    "Faktura 3 januari 2024 och 4 mars 2023",
    # misc
    "",
    "no dates here at all",
    "20240115 and 2024/1/5 and 5/1/24",
]

FILENAMES = [
    "report-2023-12-25.txt",
    "img_2023.01.02.jpg",
    "scan_15.03.2024_invoice.pdf",
    "notes.txt",
]

CONFIGS = [
    {"languages": ["en"], "tz": "UTC", "fname_order": "YMD", "order": "DMY"},
    {"languages": ["en", "de"], "tz": "Europe/Berlin", "fname_order": None, "order": "DMY"},
    {"languages": ["de"], "tz": "Europe/Berlin", "fname_order": "DMY", "order": "DMY"},
    {"languages": ["en"], "tz": "America/New_York", "fname_order": None, "order": "MDY"},
    {"languages": ["fr", "es", "it", "pt", "nl"], "tz": "UTC", "fname_order": None, "order": "DMY"},
    {"languages": ["ru", "cs", "pl", "tr", "sv"], "tz": "UTC", "fname_order": None, "order": "DMY"},
    {"languages": [], "tz": "UTC", "fname_order": None, "order": "DMY"},
    {"languages": ["en"], "tz": "UTC", "fname_order": None, "order": "YMD"},
]

EXTRA_STRINGS = [
    "15/01/2024", "01/15/2024", "2024/01/15", "January 15, 2024", "15 Jan 2024",
    "15.01.2024", "2024-01-15", "not a date", "32/13/2024", "", "abc123xyz",
    "99/99/9999", "11.01.10", "15.01 2024", "March 2024", "Monday 2024",
    "Gestern 2024", "Heute 2024", "1st April 2023", "UTC 2024", "Jan 2024 EST",
]


def iso(d):
    return None if d is None else d.isoformat()


def main() -> None:
    results = []
    for ci, c in enumerate(CONFIGS):
        cfg = base.DateParserConfig(
            languages=c["languages"],
            timezone_str=c["tz"],
            ignore_dates={_dt.date(2023, 12, 1)},
            reference_time=REFERENCE_NOW,
            filename_date_order=c["fname_order"],
            content_date_order=c["order"],
        )
        parser = regex_parser.RegexDateParserPlugin(cfg)
        for fi, fname in enumerate(FILENAMES):
            for ti, text in enumerate(TEXTS):
                if fi > 0 and ti > 3:
                    continue  # keep the file small: filenames only with a few texts
                dates = [iso(d) for d in parser.parse(fname, text)]
                matches = [m.group(0) for m in regex_parser.RegexDateParserPlugin.DATE_REGEX.finditer(text)]
                # same as parser.parse but immune to upstream's 0.1 s finditer timeout
                # (the timeout also counts the time dateparser spends per match)
                full = []
                if c["fname_order"]:
                    for m in regex_parser.RegexDateParserPlugin.DATE_REGEX.finditer(fname):
                        full.append(parser._process_match(m, c["fname_order"]))
                for m in regex_parser.RegexDateParserPlugin.DATE_REGEX.finditer(text):
                    full.append(parser._process_match(m, c["order"]))
                full = [iso(d) for d in full if d is not None]
                results.append(
                    {"config": ci, "filename": fname, "content": text, "dates": dates,
                     "dates_full": full, "upstream_timed_out": dates != full, "regex_matches": matches},
                )
    parse_string = []
    for ci, c in enumerate(CONFIGS):
        cfg = base.DateParserConfig(
            languages=c["languages"], timezone_str=c["tz"], ignore_dates=set(),
            reference_time=REFERENCE_NOW, filename_date_order=None, content_date_order=c["order"],
        )
        parser = regex_parser.RegexDateParserPlugin(cfg)
        strings = list(EXTRA_STRINGS)
        for t in TEXTS:
            strings += [m.group(0) for m in regex_parser.RegexDateParserPlugin.DATE_REGEX.finditer(t)]
        for s in dict.fromkeys(strings):
            parse_string.append({"config": ci, "string": s, "order": c["order"],
                                 "result": iso(parser._parse_string(s, c["order"]))})

    fuzz_cases = []
    pairs = [
        ("springfield miss", "1220 main street springf eld miss"),
        ("Springfield Miss", "1220 Main Street Springfield Mich"),
        ("fuzzy wuzzy was a bear", "wuzzy fuzzy was a bear"),
        ("abc", "abc"), ("", ""), ("", "x"), ("abcd", "xabcy"), ("hello world", "hello"),
        ("a" * 70 + "b", "x" * 10 + "a" * 69 + "bb"),
        ("Rechnung Müller", "Die rechnung von Herrn Muller ist da"),
    ]
    for a, b in pairs:
        fuzz_cases.append({"a": a, "b": b, "score": fuzz.partial_ratio(a, b),
                           "cutoff90": fuzz.partial_ratio(a, b, score_cutoff=90)})

    fn_cases = []
    for pat in ["*.pdf", "*simple*", "a?[!b]c", "[a-c]*", "[z-a]", "[!]", "x[y", "*[a-]*", "[]a]", "**", "?"]:
        for name in ["simple.pdf", "abc", "x[y", "a-b", "]", "a", ""]:
            fn_cases.append({"pattern": pat, "name": name, "translate": fnmatch.translate(pat),
                             "match": fnmatch.fnmatchcase(name, pat)})

    OUT.write_text(json.dumps({
        "reference_now": REFERENCE_NOW.isoformat(),
        "configs": CONFIGS,
        "parse": results,
        "parse_string": parse_string,
        "fuzz": fuzz_cases,
        "fnmatch": fn_cases,
    }, ensure_ascii=False, indent=1), "utf-8")
    print(f"wrote {OUT}: {len(results)} parse cases, {len(parse_string)} strings", file=sys.stderr)


if __name__ == "__main__":
    main()
