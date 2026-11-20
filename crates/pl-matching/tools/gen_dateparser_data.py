"""
Generates crates/pl-matching/data/dateparser.json from the installed Python
`dateparser` package (the one paperless-ngx v3.4.2 uses, see
reference/venv: dateparser 1.4.3).

The Rust port (src/dates/dateparser/*) does not re-implement dateparser's
data loading. Instead this script dumps the *derived* per-locale structures
exactly as dateparser builds them at runtime with NORMALIZE=True (default)
and SKIP_TOKENS=["t"] (default):

  - normalized dictionary (key -> English translation or null for skip words)
  - sorted relative strings (for the split/match relative regexes)
  - relative translations (compiled pattern text + replacement)
  - simplifications (compiled pattern text + replacement)
  - no_word_spacing (per dataset), info["name"] (per locale)

plus the language order, the locale table, the timezone offset table and two
small Unicode tables (Nd zero code points, isdigit-but-not-isdecimal chars).

Run:  reference/venv/Scripts/python.exe -I crates/pl-matching/tools/gen_dateparser_data.py
"""

import hashlib
import json
import sys
import unicodedata
from pathlib import Path

import dateparser
from dateparser.conf import settings as default_settings
from dateparser.data import language_locale_dict
from dateparser.data import language_order
from dateparser.languages.loader import LocaleDataLoader
from dateparser.timezone_parser import _tz_offsets

OUT = Path(__file__).resolve().parent.parent / "data" / "dateparser.json"

settings = default_settings.replace(
    mod_settings={
        "DATE_ORDER": "DMY",
        "PREFER_DAY_OF_MONTH": "first",
        "RETURN_AS_TIMEZONE_AWARE": True,
        "TIMEZONE": "UTC",
    },
    **{
        "DATE_ORDER": "DMY",
        "PREFER_DAY_OF_MONTH": "first",
        "RETURN_AS_TIMEZONE_AWARE": True,
        "TIMEZONE": "UTC",
    },
)
assert settings.NORMALIZE is True
assert list(settings.SKIP_TOKENS) == ["t"]

loader = LocaleDataLoader()


def derive(shortname: str) -> dict:
    locale = loader.get_locale(shortname)
    dictionary = locale._get_dictionary(settings)
    entries = [[k, v] for k, v in dictionary._dictionary.items()]
    rel_strings = dictionary._get_sorted_relative_strings_from_cache()
    rel_tr = [
        [p.pattern, r] for p, r in locale._get_relative_translations(settings).items()
    ]
    simpl = []
    for s in locale._get_simplifications(settings):
        p, r = next(iter(s.items()))
        simpl.append([p.pattern, r])
    return locale.info.get("name"), {
        "nws": bool(dictionary._no_word_spacing),
        "dict": entries,
        "rel": list(rel_strings),
        "rel_tr": rel_tr,
        "simpl": simpl,
    }


def main() -> None:
    all_locales = []
    for lang in language_order:
        all_locales.append(lang)
        for loc in language_locale_dict.get(lang, []):
            if loc != lang:
                all_locales.append(loc)

    datasets = []
    by_hash = {}
    locale_map = {}
    for shortname in all_locales:
        name, d = derive(shortname)
        blob = json.dumps(d, ensure_ascii=False, sort_keys=True)
        h = hashlib.sha1(blob.encode("utf-8")).hexdigest()
        if h not in by_hash:
            by_hash[h] = len(datasets)
            datasets.append(d)
        locale_map[shortname] = [by_hash[h], name]

    tz = [
        [name, info["regex"].pattern, int(info["offset"].total_seconds())]
        for name, info in _tz_offsets
    ]

    nd_zero = [
        c
        for c in range(0x110000)
        if unicodedata.category(chr(c)) == "Nd" and unicodedata.decimal(chr(c)) == 0
    ]
    digit_not_decimal = [
        c
        for c in range(0x110000)
        if chr(c).isdigit() and not chr(c).isdecimal()
    ]

    out = {
        "dateparser_version": dateparser.__version__,
        "unidata_version": unicodedata.unidata_version,
        "language_order": list(language_order),
        "language_locale_dict": {k: list(v) for k, v in language_locale_dict.items()},
        "locales": locale_map,
        "datasets": datasets,
        "tz": tz,
        "nd_zero": nd_zero,
        "digit_not_decimal": digit_not_decimal,
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(out, ensure_ascii=False, separators=(",", ":")), "utf-8")
    print(
        f"wrote {OUT} ({OUT.stat().st_size} bytes, {len(locale_map)} locales, "
        f"{len(datasets)} datasets, {len(tz)} tz)",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
