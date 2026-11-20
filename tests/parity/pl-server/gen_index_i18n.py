"""Generates crates/pl-server/src/frontend/index_i18n.rs: the three translated texts of the
Angular loading screen (src/documents/templates/index.html, `{% translate %}`).

Queries Django's own gettext (`translation.override(lang)`) with the paperless catalogs compiled like
`compilemessages` (W3 overlay, see tests/parity/pl-accounts/w3_settings.py) –
the same source as pl-accounts (`i18n_data.rs`). Only entries that differ from the msgid.

The texts remain the upstream translations ("Paperless-ngx …", parity with PAPERRUST_BRANDING=off).
pl-server inserts the name PaperRust only on delivery (`frontend::render_index`,
`pl_accounts::tpl::with_site_name`) – therefore no replacement here.

Invocation (Git Bash): source tests/parity/pl-accounts/w3env.sh && "$PY" -I tests/parity/pl-server/gen_index_i18n.py
"""

import os
import sys

sys.path[:0] = [p for p in os.environ.get("PYTHONPATH", "").split(os.pathsep) if p]
import django  # noqa: E402

django.setup()
from django.conf import settings  # noqa: E402
from django.utils import translation  # noqa: E402

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", ".."))
OUT = os.path.join(REPO, "crates", "pl-server", "src", "frontend", "index_i18n.rs")

MSGIDS = [
    "Paperless-ngx is loading...",
    "Still here?! Hmm, something might be wrong.",
    "Here's a link to the docs.",
]


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
    "//! GENERIERT von tests/parity/pl-server/gen_index_i18n.py – nicht von Hand ändern.",
    "//! Übersetzungen (Django gettext, paperless-Kataloge kompiliert) des Ladebildschirms.",
    "",
    "/// msgids (Reihenfolge = Index).",
    "pub const MSGIDS: [&str; 3] = [",
]
out += [f"    {rs(m)}," for m in MSGIDS]
out += ["];", "", "/// (Sprache, [(Index in MSGIDS, Übersetzung)]).", "pub const CATALOG: &[(&str, &[(u8, &str)])] = &["]
for code, _ in settings.LANGUAGES:
    rows = []
    with translation.override(code):
        for i, m in enumerate(MSGIDS):
            t = translation.gettext(m)
            if t != m:
                rows.append(f"({i}, {rs(t)})")
    out.append(f"    ({rs(code)}, &[{', '.join(rows)}]),")
out += ["];", ""]
os.makedirs(os.path.dirname(OUT), exist_ok=True)
with open(OUT, "w", encoding="utf-8", newline="\n") as f:
    f.write("\n".join(out))
print("wrote", OUT)
