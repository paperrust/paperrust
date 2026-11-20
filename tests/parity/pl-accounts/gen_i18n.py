"""Generates crates/pl-accounts/src/i18n_data.rs: translations of all texts of the account pages.

Queries Django's own gettext (`translation.override(lang)`), i.e. the same catalog
merging as upstream: LOCALE_PATHS (paperless, compiled with w3_msgfmt.py like
`compilemessages` in the Docker image) before allauth before Django. Only entries that differ
from the msgid are stored.

Invocation (Git Bash): source tests/parity/pl-accounts/w3env.sh && "$PY" -I tests/parity/pl-accounts/gen_i18n.py
"""

import os
import re
import sys

sys.path[:0] = [p for p in os.environ.get("PYTHONPATH", "").split(os.pathsep) if p]
import django  # noqa: E402

django.setup()
from django.conf import settings  # noqa: E402
from django.utils import translation  # noqa: E402

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
CRATE = os.path.join(REPO, "crates", "pl-accounts")

msgids = set()
for name in os.listdir(os.path.join(CRATE, "templates")):
    text = open(os.path.join(CRATE, "templates", name), encoding="utf-8").read()
    msgids.update(re.findall(r'_\("((?:[^"\\]|\\.)*)"\)', text))
    msgids.update(re.findall(r'blocktrans\("((?:[^"\\]|\\.)*)"', text))
for name in os.listdir(os.path.join(CRATE, "src")):
    if name.endswith(".rs") and name != "i18n_data.rs":
        text = open(os.path.join(CRATE, "src", name), encoding="utf-8").read()
        msgids.update(re.findall(r'\bgettext\("((?:[^"\\]|\\.)*)"', text))
        msgids.update(re.findall(r'\btr!\([^,]+,\s*"((?:[^"\\]|\\.)*)"', text))


def unescape(m: str) -> str:
    return re.sub(r"\\(.)", lambda x: {"n": "\n", "t": "\t"}.get(x.group(1), x.group(1)), m)


msgids = sorted(unescape(m) for m in msgids)


def rs(s: str) -> str:
    out = []
    for c in s:
        o = ord(c)
        if c == "\\":
            out.append("\\\\")
        elif c == '"':
            out.append('\\"')
        elif c == "\n":
            out.append("\\n")
        elif o < 0x20 or 0x200B <= o <= 0x200F or 0x202A <= o <= 0x202E or 0x2066 <= o <= 0x2069 or o == 0xFEFF:
            out.append("\\u{%x}" % o)
        else:
            out.append(c)
    return '"' + "".join(out) + '"'


out = [
    "//! GENERIERT von tests/parity/pl-accounts/gen_i18n.py – nicht von Hand ändern.",
    "//! Übersetzungen (Django gettext, Kataloge paperless + allauth + Django) der Kontoseiten.",
    "",
    "/// msgids in sortierter Reihenfolge.",
    "pub const MSGIDS: &[&str] = &[",
]
out += [f"    {rs(m)}," for m in msgids]
out += ["];", "", "/// (Sprache, [(Index in MSGIDS, Übersetzung)]).", "pub const CATALOG: &[(&str, &[(u16, &str)])] = &["]
for code, _ in settings.LANGUAGES:
    rows = []
    with translation.override(code):
        for i, m in enumerate(msgids):
            t = translation.gettext(m)
            if t != m:
                rows.append(f"({i}, {rs(t)})")
    out.append(f"    ({rs(code)}, &[{', '.join(rows)}]),")
out += ["];", ""]
with open(os.path.join(CRATE, "src", "i18n_data.rs"), "w", encoding="utf-8", newline="\n") as f:
    f.write("\n".join(out))
print(len(msgids), "msgids")
