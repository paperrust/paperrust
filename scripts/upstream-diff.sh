#!/usr/bin/env bash
# upstream-diff.sh vALT vNEU – what an upstream release means for paperless-rs.
#
#   scripts/upstream-diff.sh v3.4.2 v3.5.0
#   UPSTREAM_DIFF_SCHEMA=1 scripts/upstream-diff.sh v3.4.2 v3.5.0   # also try to build OpenAPI schemas
#
# Reads only the upstream git repository (upstream/, tags must exist: git -C upstream fetch --tags).
# The upstream working tree is never touched (files are read with `git show` / `git archive`).
#
# Report: results/upstream-diff/<ALT>..<NEU>.md (+ .json), printed summary.
#  1. OpenAPI diff: schemas from reference/schema/<tag>.json|.yml if present, otherwise (only with
#     UPSTREAM_DIFF_SCHEMA=1) generated with `manage.py spectacular` from `git archive` exports in a
#     temp dir using $PY (needs the dependencies of that version). oasdiff is used when installed,
#     otherwise a built-in structural diff (paths, methods, parameters, schema properties).
#  2. new / changed / removed migrations
#  3. changed Python files -> affected Rust crates via UPSTREAM_MAP.md (unmapped files listed)
#  4. changed src-ui services/models/query mapping (what the frontend sends/expects)
#  5. RED GATE: Celery settings, tantivy version, documents/search/_schema.py, _tokenizer.py,
#     API versions (ALLOWED_VERSIONS/DEFAULT_VERSION)
#
# Exit: 0 no red gate, 1 red gate, 2 usage/tool problem.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UP="$REPO/upstream"
OLD="${1:-}"; NEW="${2:-}"
if [[ -z "$OLD" || -z "$NEW" ]]; then sed -n '2,22p' "$0"; exit 2; fi
command -v git >/dev/null || { echo "git missing"; exit 2; }
PYTHON="${UPSTREAM_DIFF_PYTHON:-}"
if [[ -z "$PYTHON" ]]; then
  for c in python3 python py; do
    if command -v "$c" >/dev/null 2>&1 && "$c" -c "import sys; sys.exit(0 if sys.version_info >= (3, 9) else 1)" 2>/dev/null; then
      PYTHON="$c"; break
    fi
  done
fi
[[ -n "$PYTHON" ]] || { echo "python >= 3.9 missing"; exit 2; }
for t in "$OLD" "$NEW"; do
  git -C "$UP" rev-parse --verify --quiet "$t^{commit}" >/dev/null || {
    echo "unknown upstream revision: $t (git -C upstream fetch --tags?)"; exit 2; }
done

OUT="$REPO/results/upstream-diff"
mkdir -p "$OUT"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# ---- optional: OpenAPI schemas ------------------------------------------------
schema_for() { # tag -> path or empty
  local tag="$1"
  for ext in json yml yaml; do
    [[ -f "$REPO/reference/schema/$tag.$ext" ]] && { echo "$REPO/reference/schema/$tag.$ext"; return; }
  done
  if [[ -n "${UPSTREAM_DIFF_SCHEMA:-}" && -n "${PY:-}" ]]; then
    local dir="$TMP/src-$tag"
    mkdir -p "$dir"
    git -C "$UP" archive "$tag" src | tar -x -C "$dir" 2>/dev/null || return
    ( cd "$dir/src" && PAPERLESS_DBENGINE=sqlite PAPERLESS_DATA_DIR="$TMP/data-$tag" \
        PAPERLESS_SECRET_KEY=schema-only DJANGO_SETTINGS_MODULE=paperless.settings \
        "$PY" manage.py spectacular --format openapi-json --file "$TMP/$tag.json" >"$TMP/schema-$tag.log" 2>&1 ) \
      && echo "$TMP/$tag.json"
  fi
}
S_OLD="$(schema_for "$OLD")"; S_NEW="$(schema_for "$NEW")"
OASDIFF_OUT=""
if [[ -n "$S_OLD" && -n "$S_NEW" ]] && command -v oasdiff >/dev/null 2>&1; then
  oasdiff breaking "$S_OLD" "$S_NEW" >"$TMP/oasdiff-breaking.txt" 2>&1
  oasdiff changelog "$S_OLD" "$S_NEW" >"$TMP/oasdiff-changelog.txt" 2>&1
  OASDIFF_OUT="$TMP/oasdiff-changelog.txt"
fi

export UP OLD NEW OUT S_OLD S_NEW OASDIFF_OUT REPO
export PYTHONIOENCODING=utf-8 PYTHONUTF8=1
"$PYTHON" - <<'PYEOF'
import fnmatch, json, os, re, subprocess, sys

UP, OLD, NEW, OUT, REPO = (os.environ[k] for k in ("UP", "OLD", "NEW", "OUT", "REPO"))
S_OLD, S_NEW, OAS = os.environ.get("S_OLD"), os.environ.get("S_NEW"), os.environ.get("OASDIFF_OUT")

def git(*a):
    return subprocess.run(["git", "-C", UP, *a], capture_output=True, text=True, encoding="utf-8",
                          errors="replace").stdout

def show(rev, path):
    return git("show", f"{rev}:{path}")

changes = []  # (status, path, newpath)
for line in git("diff", "--name-status", "-M", OLD, NEW).splitlines():
    parts = line.split("\t")
    st = parts[0][0]
    if st == "R":
        changes.append(("R", parts[1], parts[2]))
    else:
        changes.append((st, parts[1], parts[1]))
paths = sorted({p for _, a, b in changes for p in (a, b)})

# ---- UPSTREAM_MAP.md ----------------------------------------------------------
rows = []
map_path = os.path.join(REPO, "UPSTREAM_MAP.md")
if os.path.exists(map_path):
    for line in open(map_path, encoding="utf-8"):
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) < 4 or not line.lstrip().startswith("|") or set(cells[0]) <= set("-: "):
            continue
        if cells[0].lower() == "crate":
            continue
        globs = re.findall(r"`([^`]+)`", cells[2])
        rows.append({"crate": cells[0], "module": cells[1], "globs": globs, "routes": cells[3]})

def glob_match(path, g):
    if fnmatch.fnmatchcase(path, g):
        return True
    if "**" in g:
        rx = re.escape(g).replace(r"\*\*", ".*").replace(r"\*", "[^/]*").replace(r"\?", ".")
        return re.fullmatch(rx, path) is not None
    return False

py_changed = [p for p in paths if p.endswith(".py") and p.startswith("src/") and "/tests/" not in p
              and "/migrations/" not in p]
affected, unmapped = {}, []
for p in py_changed:
    hit = [r for r in rows if any(glob_match(p, g) for g in r["globs"])]
    if not hit:
        unmapped.append(p)
    for r in hit:
        affected.setdefault(r["crate"], set()).add((p, r["module"], r["routes"]))

# ---- migrations ---------------------------------------------------------------
migs = [(s, b if s != "D" else a) for s, a, b in changes if "/migrations/" in b and b.endswith(".py")
        and not b.endswith("__init__.py")]

# ---- frontend -----------------------------------------------------------------
ui_patterns = ["src-ui/src/app/services/**", "src-ui/src/app/data/**", "src-ui/src/app/utils/query-params.ts",
               "src-ui/src/app/utils/filter-rules.ts", "src-ui/src/app/interceptors/**",
               "src-ui/src/environments/**"]
ui = [p for p in paths if any(glob_match(p, g) for g in ui_patterns) and not p.endswith(".spec.ts")]

# ---- red gates ----------------------------------------------------------------
red = []
def settings_text(rev):
    out = []
    for f in git("ls-tree", "-r", "--name-only", rev, "src/paperless/settings").split():
        out.append(show(rev, f))
    out.append(show(rev, "src/paperless/settings.py"))
    return "\n".join(out)
so, sn = settings_text(OLD), settings_text(NEW)
def celery_lines(t):
    return sorted({l.strip() for l in t.splitlines() if re.search(r"\bCELERY_\w*", l)})
co, cn = celery_lines(so), celery_lines(sn)
if co != cn:
    red.append({"gate": "Celery-Settings", "removed": sorted(set(co) - set(cn)), "added": sorted(set(cn) - set(co))})
if any(p == "src/paperless/celery.py" for p in paths):
    red.append({"gate": "Celery-App (src/paperless/celery.py)", "added": ["changed"], "removed": []})
def ver_lines(t, rx):
    return sorted({l.strip() for l in t.splitlines() if re.search(rx, l)})
to = ver_lines(show(OLD, "pyproject.toml"), r'"tantivy|whoosh-compat')
tn = ver_lines(show(NEW, "pyproject.toml"), r'"tantivy|whoosh-compat')
if to != tn:
    red.append({"gate": "tantivy-Version", "removed": sorted(set(to) - set(tn)), "added": sorted(set(tn) - set(to))})
for f in ("src/documents/search/_schema.py", "src/documents/search/_tokenizer.py"):
    if f in paths:
        red.append({"gate": f, "added": ["changed"], "removed": []})
vo = ver_lines(so, r"ALLOWED_VERSIONS|DEFAULT_VERSION")
vn = ver_lines(sn, r"ALLOWED_VERSIONS|DEFAULT_VERSION")
if vo != vn:
    red.append({"gate": "API-Versionen", "removed": sorted(set(vo) - set(vn)), "added": sorted(set(vn) - set(vo))})

# ---- OpenAPI ------------------------------------------------------------------
def load_schema(p):
    txt = open(p, encoding="utf-8").read()
    if p.endswith(".json"):
        return json.loads(txt)
    try:
        import yaml
        return yaml.safe_load(txt)
    except ImportError:
        return None
openapi = {"status": "übersprungen (kein Schema; reference/schema/<tag>.json oder UPSTREAM_DIFF_SCHEMA=1)"}
if S_OLD and S_NEW:
    a, b = load_schema(S_OLD), load_schema(S_NEW)
    if a is None or b is None:
        openapi = {"status": "Schema nicht lesbar (YAML ohne PyYAML?)"}
    else:
        def ops(s):
            out = {}
            for path, item in (s.get("paths") or {}).items():
                for m, op in item.items():
                    if m in ("get", "post", "put", "patch", "delete"):
                        params = sorted(f"{p.get('in')}:{p.get('name')}" for p in op.get("parameters", []) if isinstance(p, dict))
                        out[f"{m.upper()} {path}"] = params
            return out
        oa, ob = ops(a), ops(b)
        def props(s):
            out = {}
            for name, sc in ((s.get("components") or {}).get("schemas") or {}).items():
                out[name] = list((sc.get("properties") or {}).keys())
            return out
        pa, pb = props(a), props(b)
        openapi = {
            "status": "ok",
            "added_operations": sorted(set(ob) - set(oa)),
            "removed_operations": sorted(set(oa) - set(ob)),
            "changed_parameters": sorted(k for k in set(oa) & set(ob) if oa[k] != ob[k]),
            "changed_schemas": {k: {"added": [x for x in pb[k] if x not in pa[k]],
                                    "removed": [x for x in pa[k] if x not in pb[k]],
                                    "reordered": [x for x in pa[k] if x in pb[k]] != [x for x in pb[k] if x in pa[k]]}
                                for k in set(pa) & set(pb) if pa[k] != pb[k]},
            "added_schemas": sorted(set(pb) - set(pa)),
            "removed_schemas": sorted(set(pa) - set(pb)),
        }
        if OAS and os.path.exists(OAS):
            openapi["oasdiff_changelog"] = open(OAS, encoding="utf-8", errors="replace").read()[:20000]

urls_changed = "src/paperless/urls.py" in paths
report = {"old": OLD, "new": NEW, "files_changed": len(paths), "red": red, "migrations": migs,
          "affected": {k: sorted(v) for k, v in affected.items()}, "unmapped": unmapped,
          "frontend": ui, "openapi": openapi, "urls_changed": urls_changed, "map_rows": len(rows)}
base = os.path.join(OUT, f"{OLD}..{NEW}")
open(base + ".json", "w", encoding="utf-8").write(json.dumps(report, indent=1, ensure_ascii=False))

L = [f"# upstream {OLD} → {NEW}", "", f"- geänderte Dateien: {len(paths)}",
     f"- **Rotes Gate: {'JA' if red else 'nein'}**", ""]
if red:
    L += ["## Rotes Gate", ""]
    for r in red:
        L.append(f"### {r['gate']}")
        for x in r.get("removed", []):
            L.append(f"- − `{x}`")
        for x in r.get("added", []):
            L.append(f"- + `{x}`")
        L.append("")
L += ["## Migrationen", ""]
L += [f"- {s} `{p}`" for s, p in migs] or ["- keine"]
L += ["", "## Betroffene Crates (UPSTREAM_MAP.md)", ""]
if not rows:
    L.append("- UPSTREAM_MAP.md fehlt oder ist leer")
for crate in sorted(affected):
    L.append(f"### {crate}")
    for p, mod, routes in sorted(affected[crate]):
        L.append(f"- `{p}` → {mod} (Routen: {routes})")
    L.append("")
if unmapped:
    L += ["### Nicht zugeordnet", ""] + [f"- `{p}`" for p in unmapped] + [""]
if urls_changed:
    L += ["- `src/paperless/urls.py` geändert: Routen-Liste/routes.toml und Basis-Fallkatalog prüfen", ""]
L += ["## Frontend (src-ui Services/Modelle)", ""]
L += [f"- `{p}`" for p in ui] or ["- keine"]
L += ["", "## OpenAPI", "", f"- Status: {openapi.get('status')}"]
if openapi.get("status") == "ok":
    for k in ("added_operations", "removed_operations", "changed_parameters", "added_schemas", "removed_schemas"):
        if openapi[k]:
            L.append(f"- {k}: " + ", ".join(f"`{x}`" for x in openapi[k][:80]))
    for k, v in sorted(openapi["changed_schemas"].items()):
        L.append(f"- Schema `{k}`: +{v['added']} −{v['removed']}{' (Reihenfolge geändert)' if v['reordered'] else ''}")
open(base + ".md", "w", encoding="utf-8").write("\n".join(L) + "\n")
print("\n".join(L[:6]))
print(f"report: {base}.md")
sys.exit(1 if red else 0)
PYEOF
