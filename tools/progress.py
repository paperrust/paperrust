#!/usr/bin/env python3
"""Fortschrittskarte für PaperRust: docs/progress/progress.svg und progress.json.

Jedes Kästchen ist ein Stück Arbeit: grün = fertig, gelb = teilweise, grau = offen,
blau = bewusst nicht / entfällt. Die Kästchen sind nach Bereich zu Kacheln gruppiert.

Quellen (nur lesen):
  * routes.toml                     – API-Routen: "rust" = fertig, sonst offen
  * docs/UPSTREAM-SYNC.md §8        – nachgebaute paperless-ngx-Fixes nach v3.4.2
  * frontend-light/                 – Dateien der neuen Oberfläche: eigen oder von ngx übernommen
  * docs/FRONTEND-PARITY.md         – Ablösung der alten Angular-Oberfläche: Prüfliste je Bildschirm,
                                      Dialog und Aktion (vorhanden = fertig, teilweise, fehlt = offen)
  * docs/progress.toml              – Fahrplan (was sich nicht aus dem Code ablesen lässt)

Aufruf: python tools/progress.py   (ohne Abhängigkeiten, Python ≥ 3.11)
Mit --check: nur prüfen, ob die Dateien aktuell sind (Exit 1, wenn nicht).
"""

from __future__ import annotations

import json
import math
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass, field
from html import escape
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "docs" / "progress"

DONE, PART, OPEN, NA = "done", "partial", "open", "na"

WIDTH = 1000
PAD = 20
CELL = 12
STEP = 15  # Cell + spacing
TILE_GAP = 14
LABEL_H = 18
CHAR_W = 6.6  # rough width of a character at 11 px


@dataclass
class Item:
    name: str
    status: str


@dataclass
class Tile:
    name: str
    items: list[Item] = field(default_factory=list)


@dataclass
class Panel:
    title: str
    subtitle: str
    tiles: list[Tile]


def score(items: list[Item]) -> tuple[float, int]:
    """(achieved points, countable quantity); partial = half, 'na' does not count."""
    pts = sum(1.0 if i.status == DONE else 0.5 if i.status == PART else 0.0 for i in items)
    return pts, sum(1 for i in items if i.status != NA)


def pct(pts: float, total: int) -> str:
    if not total:
        return "–"
    v = 100 * pts / total
    txt = f"{v:.0f}"
    if (txt == "100" and pts < total) or (txt == "0" and pts > 0):
        txt = f"{v:.1f}".replace(".", ",")
    return f"{txt} %"


# ---------------------------------------------------------------- Data sources


def routes_panel() -> Panel:
    text = (ROOT / "routes.toml").read_text(encoding="utf-8")
    routes = tomllib.loads(text).get("routes", {})
    groups: dict[str, list[Item]] = {}
    for rid, target in sorted(routes.items()):
        area = rid.split(".", 1)[0]
        groups.setdefault(area, []).append(Item(rid, DONE if target == "rust" else OPEN))
    tiles = [Tile(k, v) for k, v in sorted(groups.items(), key=lambda kv: (-len(kv[1]), kv[0]))]
    return Panel(
        "API-Routen in Rust",
        "routes.toml · grün = von PaperRust bedient, grau = an Django weitergereicht",
        tiles,
    )


def upstream_fixes_panel() -> Panel:
    text = (ROOT / "docs" / "UPSTREAM-SYNC.md").read_text(encoding="utf-8")
    m = re.search(r"^## 8\..*?$(.*?)(?=^##? |\Z)", text, re.S | re.M)
    body = m.group(1) if m else ""
    items = []
    for line in body.splitlines():
        if not re.match(r"^\| [0-9a-f]{7,}", line):
            continue
        cols = [c.strip() for c in line.strip("|").split("|")]
        ref, topic, where = cols[0], cols[1], cols[2] if len(cols) > 2 else ""
        topic = re.sub(r"\*\*|`", "", topic)
        status = NA if re.search(r"nicht übernommen|nicht nötig", where) else DONE
        pr = re.search(r"#\d+", ref)
        items.append(Item(f"{pr.group(0) if pr else ref}: {topic}", status))
    return Panel(
        "paperless-ngx-Verbesserungen nach v3.4.2",
        "docs/UPSTREAM-SYNC.md §8 · grün = nachgebaut, blau = entfällt bei PaperRust",
        [Tile("nachgebaut aus paperless-ngx dev", items)],
    )


def git_files(repo: Path, *paths: str) -> list[str]:
    try:
        out = subprocess.run(
            ["git", "-C", str(repo), "ls-files", *paths],
            capture_output=True, text=True, encoding="utf-8", check=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError):
        return []
    return [p for p in out.splitlines() if p]


def light_ui_panel() -> Panel:
    files = git_files(ROOT, "frontend-light/src", "frontend-light/i18n")
    groups: dict[str, list[Item]] = {}

    def add(group: str, path: str, status: str) -> None:
        groups.setdefault(group, []).append(Item(path.removeprefix("frontend-light/"), status))

    for f in files:
        rel = f.removeprefix("frontend-light/")
        if rel.startswith("src/styles/upstream/"):
            add("Styles von ngx", f, OPEN)
        elif rel.startswith("i18n/"):
            # own catalog (dotted keys) since 2026-11-13; the old ngx catalog had _keys.json
            add("eigene Übersetzungen", f, DONE)
        else:
            parts = rel.split("/")
            add(parts[1] if len(parts) > 2 else "src", f, DONE)
    order = sorted(groups.items(), key=lambda kv: (kv[1][0].status != DONE, -len(kv[1]), kv[0]))
    return Panel(
        "Neue Oberfläche: eigener Code",
        "frontend-light/ · grün = eigene Datei, grau = von paperless-ngx übernommen (Styles)",
        [Tile(k, v) for k, v in order],
    )


PARITY_STATUS = {"vorhanden": DONE, "eingebettet": DONE, "über": DONE, "teilweise": PART, "fehlt": OPEN, "entfällt": NA}


def parity_status(cell: str) -> str:
    """First word of a status cell (without Markdown) → status; unknown aborts."""
    word = re.sub(r"[*_`]", "", cell).strip().split(" ")[0].lower() if cell.strip() else ""
    if word not in PARITY_STATUS:
        raise SystemExit(f"docs/FRONTEND-PARITY.md: unbekannter Status {cell!r}")
    return PARITY_STATUS[word]


def parity_rows() -> list[tuple[str, str, str]]:
    """(area, element, status) from the checklist; status = worse value of classic
    design and PaperRust design (there "embedded" and "via classic" count as reachable)."""
    path = ROOT / "docs" / "FRONTEND-PARITY.md"
    if not path.exists():
        return []
    rows, inside = [], False
    rank = {OPEN: 0, PART: 1, DONE: 2, NA: 3}
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("## "):
            inside = line.startswith("## Prüfliste")
            continue
        if not inside or not line.startswith("|") or line.startswith("|---") or line.startswith("| #"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) < 6:
            continue
        _, area, element, _src, classic, design2 = cells[:6]
        st = min(parity_status(classic), parity_status(design2), key=lambda x: rank[x])
        rows.append((area, element, st))
    return rows


def angular_panel() -> Panel:
    groups: dict[str, list[Item]] = {}
    for area, element, st in parity_rows():
        groups.setdefault(area, []).append(Item(element, st))
    return Panel(
        "Alte Oberfläche (Angular) abgelöst",
        "docs/FRONTEND-PARITY.md · je Bildschirm/Dialog/Aktion von ngx v3.4.2: grün = in der eigenen Oberfläche vorhanden",
        [Tile(k, v) for k, v in groups.items()],
    )


def roadmap_panel() -> Panel:
    data = tomllib.loads((ROOT / "docs" / "progress.toml").read_text(encoding="utf-8"))
    tiles = []
    for g in data.get("group", []):
        items = [Item(i["name"], i.get("status", OPEN)) for i in g.get("items", [])]
        bad = [i.status for i in items if i.status not in (DONE, PART, OPEN, NA)]
        if bad:
            raise SystemExit(f"docs/progress.toml: unbekannter Status {bad} in {g.get('name')}")
        tiles.append(Tile(g["name"], items))
    return Panel("Fahrplan: weg von paperless-ngx", "docs/progress.toml", tiles)


# ---------------------------------------------------------------- Drawing


def tile_cols(tile: Tile, max_w: int) -> int:
    n = max(len(tile.items), 1)
    cols = max(min(n, 5), math.ceil(math.sqrt(n * 2.2)))
    label_cols = math.ceil((len(tile.name) * CHAR_W + 60) / STEP)
    cols = max(cols, min(label_cols, n if n > 5 else label_cols))
    return max(1, min(cols, (max_w // STEP)))


def panel_head(p: Panel, y0: int) -> list[str]:
    pts, total = score([i for t in p.tiles for i in t.items])
    head = f"{escape(p.title)}: {pct(pts, total)}"
    nums = f"({pts:g}/{total})"
    return [
        f'<text class="h" x="{PAD}" y="{y0 + 18}">{head} <tspan class="sub">{escape(nums)}</tspan></text>',
        f'<text class="sub" x="{PAD}" y="{y0 + 36}">{escape(p.subtitle)}</text>',
    ]


def draw_list_panel(p: Panel, y0: int) -> tuple[list[str], int]:
    """Roadmap: one row per area, large boxes, name and percent on the left."""
    out = panel_head(p, y0)
    big, step, label_w = 18, 23, 300
    y = y0 + 56
    for t in p.tiles:
        tp, tt = score(t.items)
        out.append(f'<text class="t2" x="{PAD}" y="{y + 14}">{escape(t.name)}</text>')
        out.append(f'<text class="pc" x="{PAD + label_w - 12}" y="{y + 14}" text-anchor="end">{pct(tp, tt)}</text>')
        for k, it in enumerate(t.items):
            cx = PAD + label_w + k * step
            out.append(f'<rect class="{it.status}" x="{cx}" y="{y}" width="{big}" height="{big}" rx="2">'
                       f'<title>{escape(it.name)}</title></rect>')
        y += step + 7
    return out, y + 22


def draw_panel(p: Panel, y0: int) -> tuple[list[str], int]:
    out = panel_head(p, y0)
    y = y0 + 52
    x = PAD
    row_h = 0
    inner_w = WIDTH - 2 * PAD
    for t in p.tiles:
        cols = tile_cols(t, inner_w)
        rows = math.ceil(max(len(t.items), 1) / cols)
        w = cols * STEP - (STEP - CELL)
        h = LABEL_H + rows * STEP
        tp, tt = score(t.items)
        label = f"{t.name} {pct(tp, tt)}" if tt else t.name
        occupied = max(w, math.ceil(len(label) * CHAR_W))
        if x > PAD and x + occupied > WIDTH - PAD:
            x = PAD
            y += row_h + TILE_GAP
            row_h = 0
        out.append(f'<g><title>{escape(t.name)}: {tp:g}/{tt}</title>'
                   f'<text class="t" x="{x}" y="{y + 12}">{escape(label)}</text></g>')
        for k, it in enumerate(t.items):
            cx = x + (k % cols) * STEP
            cy = y + LABEL_H + (k // cols) * STEP
            out.append(f'<rect class="{it.status}" x="{cx}" y="{cy}" width="{CELL}" height="{CELL}" rx="1">'
                       f'<title>{escape(it.name)}</title></rect>')
        x += occupied + TILE_GAP
        row_h = max(row_h, h)
    return out, y + row_h + 28


STYLE = """
.bg{fill:#0d1117}.h{fill:#e6edf3;font:600 15px system-ui,-apple-system,Segoe UI,sans-serif}
.sub{fill:#8b949e;font:400 11px system-ui,-apple-system,Segoe UI,sans-serif}
.t{fill:#c9d1d9;font:500 11px system-ui,-apple-system,Segoe UI,sans-serif}
.t2{fill:#c9d1d9;font:500 13px system-ui,-apple-system,Segoe UI,sans-serif}
.pc{fill:#8b949e;font:600 13px system-ui,-apple-system,Segoe UI,sans-serif}
.done{fill:#2ea043}.partial{fill:#d29922}.open{fill:#484f58}.na{fill:#388bfd}
.line{stroke:#30363d}
@media (prefers-color-scheme: light){
.bg{fill:#ffffff}.h{fill:#1f2328}.sub{fill:#59636e}.t{fill:#1f2328}.t2{fill:#1f2328}.pc{fill:#59636e}
.done{fill:#1a7f37}.partial{fill:#bf8700}.open{fill:#d1d9e0}.na{fill:#0969da}.line{stroke:#d1d9e0}}
"""


def render(panels: list[Panel]) -> str:
    body: list[str] = []
    y = PAD
    body.append(f'<text class="h" x="{PAD}" y="{y + 16}">PaperRust – Fortschritt</text>')
    lx = WIDTH - PAD - 470
    for cls, txt in ((DONE, "fertig"), (PART, "teilweise"), (OPEN, "offen / von ngx"), (NA, "entfällt")):
        body.append(f'<rect class="{cls}" x="{lx}" y="{y + 5}" width="{CELL}" height="{CELL}" rx="1"/>')
        body.append(f'<text class="sub" x="{lx + 17}" y="{y + 15}">{txt}</text>')
        lx += 30 + len(txt) * 7
    y += 40
    for i, p in enumerate(panels):
        if i:
            body.append(f'<line class="line" x1="{PAD}" x2="{WIDTH - PAD}" y1="{y - 12}" y2="{y - 12}"/>')
        part, y = (draw_list_panel if p is panels[0] else draw_panel)(p, y)
        body.extend(part)
    h = y
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {WIDTH} {h}" width="{WIDTH}" height="{h}" '
        f'role="img" aria-label="PaperRust Fortschrittskarte">\n<style>{STYLE}</style>\n'
        f'<rect class="bg" width="{WIDTH}" height="{h}" rx="6"/>\n' + "\n".join(body) + "\n</svg>\n"
    )


def summary(panels: list[Panel]) -> dict:
    res = []
    for p in panels:
        items = [i for t in p.tiles for i in t.items]
        pts, total = score(items)
        res.append({
            "title": p.title,
            "percent": round(100 * pts / total, 1) if total else None,
            "points": pts,
            "countable": total,
            "counts": {s: sum(1 for i in items if i.status == s) for s in (DONE, PART, OPEN, NA)},
            "tiles": [{"name": t.name, "points": score(t.items)[0], "countable": score(t.items)[1]}
                      for t in p.tiles],
        })
    return {"panels": res}


def main() -> int:
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")
    panels = [roadmap_panel(), light_ui_panel(), angular_panel(), upstream_fixes_panel(), routes_panel()]
    svg = render(panels)
    js = json.dumps(summary(panels), ensure_ascii=False, indent=2) + "\n"
    targets = {OUT / "progress.svg": svg, OUT / "progress.json": js}
    if "--check" in sys.argv:
        stale = [p for p, c in targets.items() if not p.exists() or p.read_text(encoding="utf-8") != c]
        for p in stale:
            print(f"veraltet: {p.relative_to(ROOT)}")
        return 1 if stale else 0
    OUT.mkdir(parents=True, exist_ok=True)
    for p, c in targets.items():
        p.write_text(c, encoding="utf-8", newline="\n")
    for s in summary(panels)["panels"]:
        print(f"{s['title']}: {s['percent']} % ({s['points']:g}/{s['countable']})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
