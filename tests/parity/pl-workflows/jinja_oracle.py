"""Oracle for the Rust port of `validate_workflow_template`.

Rebuilds the paperless Jinja environment (src/documents/templating/environment.py + workflows.py +
filepath.py, v3.4.2) with real Jinja2 3.1.6 and writes for every template from
`jinja_cases.txt` (one template per line, `\\n` for line break) the result to
`crates/pl-workflows/src/jinja/oracle.json`.

Invocation (without Django, only jinja2 + markupsafe on the path):
    python -I jinja_oracle.py <path-to-site-packages-with-jinja2> <repo-root>
"""

import json
import sys
from pathlib import Path

sys.path.insert(0, sys.argv[1])

from jinja2 import StrictUndefined  # noqa: E402
from jinja2 import TemplateAssertionError  # noqa: E402
from jinja2 import TemplateSyntaxError  # noqa: E402
from jinja2.meta import find_undeclared_variables  # noqa: E402
from jinja2.sandbox import SandboxedEnvironment  # noqa: E402

env = SandboxedEnvironment(
    trim_blocks=True,
    lstrip_blocks=True,
    keep_trailing_newline=False,
    autoescape=False,
    extensions=["jinja2.ext.loopcontrols"],
)
env.undefined = StrictUndefined
for name in ("get_cf_value", "datetime", "slugify", "localize_date"):
    env.filters[name] = lambda *a, **k: None

KNOWN = {
    "correspondent", "document_type", "added", "added_year", "added_year_short", "added_month",
    "added_month_name", "added_month_name_short", "added_day", "added_time", "owner_username",
    "original_filename", "filename", "created", "created_year", "created_year_short",
    "created_month", "created_month_name", "created_month_name_short", "created_day",
    "created_time", "doc_title", "doc_url", "doc_id", "custom_fields",
}


def validate(text):
    try:
        ast = env.parse(text)
        undeclared = find_undeclared_variables(ast)
    except TemplateAssertionError as e:
        return f"Template assertion error: {e}"
    except TemplateSyntaxError as e:
        return f"Template syntax error: {e}"
    except Exception as e:  # upstream: 500
        return f"INTERNAL {type(e).__name__}"
    unknown = undeclared - KNOWN
    if unknown:
        # sorted, like the Rust port (upstream: set order)
        return f"Template references unknown placeholders: {', '.join(sorted(unknown))}"
    return None


root = Path(sys.argv[2])
cases_file = root / "tests/parity/pl-workflows/jinja_cases.txt"
out = []
for line in cases_file.read_text(encoding="utf-8").splitlines():
    if not line or line.startswith("##"):
        continue
    tpl = line.replace("\\n", "\n").replace("\\t", "\t")
    out.append({"template": tpl, "result": validate(tpl)})

# Random templates (fixed seed, reproducible): 1) fragments, 2) structured templates
import random  # noqa: E402

FRAGS = [
    "{{", "}}", "{%", "%}", "{#", "#}", "{%-", "-%}", "{{-", "-}}", " ", "\n", "x", "y", "doc_id",
    "correspondent", "|", "upper", "nope", "(", ")", "[", "]", "{", "}", ",", ".", ":", "1", "2.5",
    "'s'", '"t"', "if", "else", "elif", "endif", "for", "in", "endfor", "set", "=", "endset", "is",
    "not", "and", "or", "defined", "~", "+", "-", "*", "**", "//", "%", "==", "!=", "<", ">",
    "macro", "endmacro", "call", "endcall", "with", "endwith", "filter", "endfilter", "raw",
    "endraw", "block", "endblock", "loop", "break", "continue", "true", "none", "abc", "\t", "@",
]
rng = random.Random(4242)
seen = {c["template"] for c in out}
while len(out) < 1500:
    tpl = "".join(rng.choice(FRAGS) for _ in range(rng.randint(1, 14)))
    if tpl not in seen:
        seen.add(tpl)
        out.append({"template": tpl, "result": validate(tpl)})

EXPRS = ["x", "doc_id", "correspondent|upper", "x|nope", "1 + y", "(a, b)", "[1, z]", "{'k': v}",
         "f(a, k=b)", "x is defined", "x is nope", "x if y else z", "not q", "x.y[0]", "'a' ~ b"]


def stmt(d):
    k = rng.randint(0, 9 if d < 3 else 3)
    e = rng.choice(EXPRS)
    if k <= 2:
        return "{{ " + e + " }}"
    if k == 3:
        return "txt\n  "
    if k == 4:
        other = ("{% else %}" + stmt(d + 1)) if rng.random() < 0.5 else ""
        return "{% if " + e + " %}" + stmt(d + 1) + other + "{% endif %}"
    if k == 5:
        return "{% for i in " + e + " %}" + stmt(d + 1) + "{% endfor %}"
    if k == 6:
        return "{% set " + rng.choice(["v", "x", "loop", "a, b"]) + " = " + e + " %}"
    if k == 7:
        return "{% with w = " + e + " %}" + stmt(d + 1) + "{% endwith %}"
    if k == 8:
        return "{% macro m" + str(d) + "(p, q=1) %}" + stmt(d + 1) + "{% endmacro %}"
    return "{% filter " + rng.choice(["upper", "nope"]) + " %}" + stmt(d + 1) + "{% endfilter %}"


while len(out) < 2500:
    tpl = "".join(stmt(0) for _ in range(rng.randint(1, 4)))
    if tpl not in seen:
        seen.add(tpl)
        out.append({"template": tpl, "result": validate(tpl)})

(root / "crates/pl-workflows/src/jinja/oracle.json").write_text(
    json.dumps(out, ensure_ascii=False, indent=1), encoding="utf-8",
)
print(len(out), "cases")
