"""Generates `src/relative_dp_data.rs` from the language data of dateparser (W8, relative date expressions).

Usage (with the Python of the reference environment, where dateparser is installed in the upstream version):

    reference/venv/Scripts/python.exe -I -X utf8 crates/pl-docfilter/tools/gen_relative_dateparser.py \
        crates/pl-docfilter/src/relative_dp_data.rs

Exactly what dateparser needs for `PARSERS=["relative-time"]` is taken over – per language:
simplifications (`simplifications`, with the word-boundary wrapper from `Locale._get_simplifications`),
relative patterns (`relative-type-regex`, for `Dictionary.split` and `Locale._get_relative_translations`) and
the normalized dictionary (`NormalizedDictionary`, sorted like `_get_sorted_words_from_cache`).
Python regex is rewritten for fancy-regex: named groups → unnamed, `\\Z` → `\\z`,
lookbehinds with alternatives of different lengths → several lookbehinds of fixed length.
"""

from __future__ import annotations

import re
import sre_constants as C
import sre_parse
import sys

import dateparser
from dateparser.conf import Settings
from dateparser.languages.loader import default_loader

# Languages of the relative parser (contract pl-docfilter, W8); order like dateparser (default list).
LANGS = set("en de fr es it nl pl pt sv cs da nb fi ru uk tr hu ro sk ca el".split())


def rs(s: str) -> str:
    """Rust-Stringliteral."""
    out = ['"']
    for ch in s:
        if ch == '"':
            out.append('\\"')
        elif ch == "\\":
            out.append("\\\\")
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\t":
            out.append("\\t")
        elif ord(ch) < 0x20 or 0x7F <= ord(ch) < 0xA0 or ord(ch) in (0x200E, 0x200F):
            out.append("\\u{%x}" % ord(ch))
        else:
            out.append(ch)
    out.append('"')
    return "".join(out)


# --- Python-Regex → fancy-regex -----------------------------------------------------------

CATS = {
    C.CATEGORY_DIGIT: r"\d", C.CATEGORY_NOT_DIGIT: r"\D", C.CATEGORY_SPACE: r"\s",
    C.CATEGORY_NOT_SPACE: r"\S", C.CATEGORY_WORD: r"\w", C.CATEGORY_NOT_WORD: r"\W",
}


def ser_class(items) -> str:
    neg = False
    parts = []
    for op, av in items:
        if op is C.NEGATE:
            neg = True
        elif op is C.LITERAL:
            parts.append(re.escape(chr(av)) if chr(av) in "\\]^-[" else chr(av))
        elif op is C.RANGE:
            parts.append(f"{chr(av[0])}-{chr(av[1])}")
        elif op is C.CATEGORY:
            parts.append(CATS[av])
        else:
            raise ValueError(f"class op {op}")
    return "[" + ("^" if neg else "") + "".join(parts) + "]"


def expand(seq) -> list[str]:
    """Finite variants of a lookbehind body (fixed length per variant, non-capturing groups)."""
    outs = [""]
    for op, av in seq:
        if op is C.LITERAL:
            alts = [re.escape(chr(av))]
        elif op is C.IN:
            alts = [ser_class(av)]
        elif op is C.ANY:
            alts = ["."]
        elif op is C.CATEGORY:
            alts = [CATS[av]]
        elif op is C.AT:
            alts = {C.AT_BEGINNING: ["^"], C.AT_BEGINNING_STRING: [r"\A"]}[av]
        elif op is C.SUBPATTERN:
            alts = expand(av[3])
        elif op is C.BRANCH:
            alts = [x for b in av[1] for x in expand(b)]
        elif op in (C.MAX_REPEAT, C.MIN_REPEAT, C.POSSESSIVE_REPEAT):
            lo, hi, body = av
            inner = expand(body)
            if lo == 0 and hi == 1:
                alts = inner + [""]
            elif hi == C.MAXREPEAT and len(body) == 1 and body[0] == (C.IN, [(C.CATEGORY, C.CATEGORY_SPACE)]):
                # `\s+` / `\s*`: after sanitize_date at most one space remains
                alts = inner if lo >= 1 else inner + [""]
            else:
                raise ValueError(f"repeat {lo},{hi}")
        else:
            raise ValueError(f"op {op}")
        outs = [o + a for o in outs for a in alts]
    return outs


def lookbehind_alt(body: str) -> str:
    alts = expand(sre_parse.parse(body))
    parts = []
    for a in dict.fromkeys(alts):
        parts.append(a if a in ("^", r"\A") else f"(?<={a})")
    return "(?:" + "|".join(parts) + ")"


def find_close(p: str, i: int) -> int:
    """Index of the closing parenthesis of the group that opens at p[i]."""
    depth, j, in_cls = 0, i, False
    while j < len(p):
        ch = p[j]
        if ch == "\\":
            j += 2
            continue
        if in_cls:
            if ch == "]":
                in_cls = False
        elif ch == "[":
            in_cls = True
        elif ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
            if depth == 0:
                return j
        j += 1
    raise ValueError(f"unbalanced {p!r}")


def conv(p: str) -> str:
    p = re.sub(r"\(\?P<\w+>", "(", p)
    p = p.replace(r"\Z", r"\z")
    start = 0
    while True:
        i = p.find("(?<=", start)
        if i < 0:
            break
        j = find_close(p, i)
        body = p[i + 4 : j]
        if "(?<=" in body:
            raise ValueError(f"nested lookbehind {p!r}")
        rep = lookbehind_alt(body)
        p = p[:i] + rep + p[j + 1 :]
        start = i + len(rep)
    return p


def conv_repl(r: str) -> str:
    out = r.replace("$", "$$")
    out = re.sub(r"\\g<(\d+)>", r"${\1}", out)
    out = re.sub(r"\\(\d+)", r"${\1}", out)
    if "\\" in out:
        raise ValueError(f"replacement {r!r}")
    return out


def main(out_path: str) -> None:
    st = Settings()
    assert st.NORMALIZE, "dateparser NORMALIZE ist Standard"
    order = []
    for loc in default_loader.get_locales():
        if loc.shortname in LANGS and loc.shortname not in order:
            order.append(loc.shortname)
    assert set(order) == LANGS, order
    lines = [
        "// GENERATED by tools/gen_relative_dateparser.py – do not edit.",
        f"// Source: dateparser {dateparser.__version__} (BSD-3-Clause, language data from Unicode CLDR),",
        "// Data for `PARSERS=[\"relative-time\"]`; language order as in dateparser.",
        "#![allow(clippy::all)]",
        "",
        "use crate::relative_dp::DpLang;",
        "",
        "pub(crate) static DP_LANGS: &[DpLang] = &[",
    ]
    for code in order:
        loc = default_loader.get_locale(code)
        assert not loc.info.get("no_word_spacing"), code
        d = loc._get_dictionary(st)
        simpl = []
        for s in loc._generate_simplifications(normalize=True):
            (k, v), = s.items()
            # without (?i): the text is already lowercase here (`_simplify`), and so are the patterns
            pat = r"(?:\A|(?<=[\W_]))" + conv(k) + r"(?=\z|[\W_])"
            rep = conv_repl(v)
            if "${" in rep and "(?<=" in k and re.search(r"\(\?<=[^)]*\(", k):
                raise ValueError(f"group numbering would shift: {k!r}")
            simpl.append((pat, rep))
        rel_strings = [conv(x) for x in d._get_sorted_relative_strings_from_cache()]
        rel_trans = []
        for pattern, repl in loc._get_relative_translations(st).items():
            if re.search(r"\\(?!1)\d", repl):
                raise ValueError(f"relative replacement {repl!r}")
            rel_trans.append((conv(pattern.pattern), repl.replace("\\1", "{n}")))
        words = []
        for k in d._get_sorted_words_from_cache():
            v = d[k]
            words.append((k, "" if v is None else str(v)))
        lines.append("    DpLang {")
        lines.append(f"        code: {rs(code)},")
        lines.append(f"        russian_ordinals: {'true' if loc.info.get('name') == 'ru' else 'false'},")
        lines.append("        simplifications: &[")
        lines += [f"            ({rs(a)}, {rs(b)})," for a, b in simpl]
        lines.append("        ],")
        lines.append("        relative_strings: &[")
        lines += [f"            {rs(a)}," for a in rel_strings]
        lines.append("        ],")
        lines.append("        relative_translations: &[")
        lines += [f"            ({rs(a)}, {rs(b)})," for a, b in rel_trans]
        lines.append("        ],")
        lines.append("        words: &[")
        lines += [f"            ({rs(a)}, {rs(b)})," for a, b in words]
        lines.append("        ],")
        lines.append("    },")
    lines.append("];")
    with open(out_path, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(lines) + "\n")
    print(out_path, len(order), "Sprachen")


if __name__ == "__main__":
    main(sys.argv[1])
