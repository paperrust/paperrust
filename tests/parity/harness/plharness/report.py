"""Report: results/parity/<stamp>.json + <stamp>.md, latest.md, history.jsonl."""

from __future__ import annotations

import datetime as _dt
import json
import subprocess
from collections import Counter, defaultdict
from pathlib import Path

from .config import REPO_DIR, Config
from .rules import RuleSet
from .runner import CaseResult

OUTCOMES = ["identical", "different", "accepted", "skipped", "error", "a-only"]


def _git_rev() -> str:
    try:
        r = subprocess.run(["git", "rev-parse", "--short", "HEAD"], cwd=str(REPO_DIR),
                           capture_output=True, text=True, timeout=10)
        dirty = subprocess.run(["git", "status", "--porcelain"], cwd=str(REPO_DIR),
                               capture_output=True, text=True, timeout=30)
        rev = r.stdout.strip() or "unknown"
        return rev + ("-dirty" if dirty.stdout.strip() else "")
    except (OSError, subprocess.SubprocessError):
        return "unknown"


def summarize(results: list[CaseResult]) -> dict:
    counts = Counter(r.outcome for r in results)
    compared = sum(counts[o] for o in ("identical", "different", "accepted"))
    return {
        "compared": compared,
        **{o: counts.get(o, 0) for o in OUTCOMES},
        "leaks_b": sum(1 for r in results if r.leaks_b),
        "leaks_a": sum(1 for r in results if r.leaks_a),
        "catalog_warnings": sum(1 for r in results if r.warnings),
    }


def write(
    cfg: Config,
    results: list[CaseResult],
    rules: RuleSet,
    meta_extra: dict,
    started: _dt.datetime,
) -> tuple[Path, Path, dict]:
    out_dir = cfg.results_dir
    out_dir.mkdir(parents=True, exist_ok=True)
    stamp = started.astimezone().strftime("%Y-%m-%d_%H%M%S")
    summary = summarize(results)
    finished = _dt.datetime.now(_dt.timezone.utc)
    meta = {
        "started": started.isoformat(),
        "finished": finished.isoformat(),
        "duration_s": round((finished - started).total_seconds(), 1),
        "git": _git_rev(),
        "a_url": cfg.a_url,
        "b_url": cfg.b_url,
        "host_header": cfg.effective_host_header,
        "cutoff": cfg.cutoff.isoformat(),
        **meta_extra,
    }
    rule_usage = [
        {"name": r.name, "hits": r.hits, "cases": len(r.cases), "reason": r.reason, "ref": r.ref,
         "source": r.source}
        for r in rules.rules
    ]
    doc = {
        "meta": meta,
        "summary": summary,
        "rules": rule_usage,
        "rule_errors": rules.errors,
        "results": [r.to_dict() for r in results],
    }
    jpath = out_dir / f"{stamp}.json"
    jpath.write_text(json.dumps(doc, indent=1, ensure_ascii=False), encoding="utf-8")

    hist_path = out_dir / "history.jsonl"
    hist_entry = {"stamp": stamp, "git": meta["git"], "targets": meta.get("targets"),
                  "filter": meta.get("filter"), **summary}
    if meta.get("sync_capture") not in (None, "off"):
        hist_entry["sync_capture"] = meta["sync_capture"]
    with hist_path.open("a", encoding="utf-8") as f:
        f.write(json.dumps(hist_entry) + "\n")
    history = []
    for line in hist_path.read_text(encoding="utf-8").splitlines():
        try:
            history.append(json.loads(line))
        except ValueError:
            continue

    md = render_markdown(meta, summary, results, rule_usage, rules.errors, history, jpath.name)
    mpath = out_dir / f"{stamp}.md"
    mpath.write_text(md, encoding="utf-8")
    (out_dir / "latest.md").write_text(md, encoding="utf-8")
    (out_dir / "latest.json").write_text(json.dumps({"report": jpath.name, "summary": summary}),
                                         encoding="utf-8")
    return jpath, mpath, summary


def _esc(s: object) -> str:
    return str(s).replace("|", "\\|").replace("\n", " ")[:160]


def render_markdown(meta, summary, results, rule_usage, rule_errors, history, json_name) -> str:
    L: list[str] = []
    L.append(f"# Paritäts-Diff {meta['started'][:19]}")
    L.append("")
    L.append(f"- A (Django): `{meta['a_url']}`  B (PaperRust): `{meta['b_url']}`  Host: `{meta['host_header']}`")
    L.append(f"- Git: `{meta['git']}`  Dauer: {meta['duration_s']} s  Stichtag: `{meta['cutoff']}`")
    if meta.get("filter"):
        L.append(f"- Filter: `{meta['filter']}`")
    if meta.get("sync_capture") not in (None, "off"):
        L.append(f"- Sync-Änderungserfassung (PARITY_SYNC_CAPTURE): `{meta['sync_capture']}`")
    L.append(f"- Rohdaten: `{json_name}`")
    L.append("")
    L.append("| verglichen | identisch | verschieden | nur akzeptiert | übersprungen | Fehler | nur A | Lecks B | Lecks A |")
    L.append("|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
    s = summary
    L.append(f"| {s['compared']} | {s['identical']} | {s['different']} | {s['accepted']} | {s['skipped']} "
             f"| {s['error']} | {s['a-only']} | {s['leaks_b']} | {s['leaks_a']} |")
    L.append("")

    by_crate: dict[str, Counter] = defaultdict(Counter)
    for r in results:
        by_crate[r.crate][r.outcome] += 1
    L.append("## Je Crate")
    L.append("")
    L.append("| Crate | identisch | verschieden | akzeptiert | übersprungen | Fehler | nur A |")
    L.append("|---|---:|---:|---:|---:|---:|---:|")
    for crate in sorted(by_crate):
        c = by_crate[crate]
        L.append(f"| {crate} | {c['identical']} | {c['different']} | {c['accepted']} | {c['skipped']} "
                 f"| {c['error']} | {c['a-only']} |")
    L.append("")

    by_route: dict[str, Counter] = defaultdict(Counter)
    for r in results:
        by_route[r.route or "?"][r.outcome] += 1
    bad_routes = [rt for rt, c in by_route.items() if c["different"] or c["error"]]
    clean_routes = [rt for rt, c in by_route.items()
                    if not c["different"] and not c["error"] and (c["identical"] or c["accepted"])]
    L.append("## Routen")
    L.append("")
    L.append(f"- ohne Abweichung (Kandidaten für `rust` in routes.toml): {len(clean_routes)}")
    L.append(f"- mit Abweichung/Fehler: {len(bad_routes)}")
    if bad_routes:
        L.append("")
        L.append("| Route | verschieden | Fehler | identisch |")
        L.append("|---|---:|---:|---:|")
        for rt in sorted(bad_routes):
            c = by_route[rt]
            L.append(f"| {rt} | {c['different']} | {c['error']} | {c['identical']} |")
    L.append("")

    diffs = [r for r in results if r.outcome in ("different", "error")]
    if diffs:
        L.append("## Abweichungen (erste 100)")
        L.append("")
        for r in diffs[:100]:
            L.append(f"### `{r.id}` – {r.outcome}")
            if r.reason:
                L.append(f"- {r.reason}")
            for st in r.steps:
                L.append(f"- {st.get('method')} `{_esc(st.get('url'))}` A={st.get('a', {}).get('status')} "
                         f"B={st.get('b', {}).get('status')}")
            for d in r.diffs[:10]:
                L.append(f"  - **{d['kind']}** `{_esc(d['path'])}`: A=`{_esc(d.get('a'))}` B=`{_esc(d.get('b'))}`")
            if len(r.diffs) > 10:
                L.append(f"  - … {len(r.diffs) - 10} weitere")
            L.append("")

    leaks_b = [r for r in results if r.leaks_b]
    leaks_a = [r for r in results if r.leaks_a]
    if leaks_b or leaks_a:
        L.append("## Leck-Prüfung")
        L.append("")
        for label, items, attr in (("B (PaperRust) – Fehler", leaks_b, "leaks_b"),
                                   ("A (Django, upstream) – Befund", leaks_a, "leaks_a")):
            if not items:
                continue
            L.append(f"### {label}")
            for r in items[:50]:
                for lk in getattr(r, attr)[:5]:
                    L.append(f"- `{r.id}` **{lk['category']}**: `{_esc(lk['excerpt'])}`")
            L.append("")

    if rule_usage or rule_errors:
        L.append("## Akzeptierte Abweichungen (Regeln)")
        L.append("")
        if rule_usage:
            L.append("| Regel | Treffer | Fälle | Begründung | Verweis |")
            L.append("|---|---:|---:|---|---|")
            for ru in rule_usage:
                stale = " (unbenutzt)" if ru["hits"] == 0 else ""
                L.append(f"| {ru['name']}{stale} | {ru['hits']} | {ru['cases']} | {_esc(ru['reason'])} | {_esc(ru['ref'])} |")
        for e in rule_errors:
            L.append(f"- Regel-Fehler: {_esc(e)}")
        L.append("")

    warn = [r for r in results if r.warnings]
    if warn:
        L.append("## Katalog-Hinweise (A weicht von expect_status ab)")
        L.append("")
        for r in warn[:80]:
            L.append(f"- `{r.id}`: {_esc('; '.join(r.warnings))}")
        L.append("")

    skipped = Counter(r.reason for r in results if r.outcome == "skipped")
    if skipped:
        L.append("## Übersprungen (Gründe)")
        L.append("")
        for reason, n in skipped.most_common(30):
            L.append(f"- {n}× {_esc(reason)}")
        L.append("")

    L.append("## Historie (letzte 20 Läufe)")
    L.append("")
    L.append("| Lauf | Git | Ziele | verglichen | identisch | verschieden | akzeptiert | übersprungen | Fehler | Lecks B |")
    L.append("|---|---|---|---:|---:|---:|---:|---:|---:|---:|")
    for h in history[-20:]:
        L.append(f"| {h.get('stamp')} | {h.get('git')} | {h.get('targets')} | {h.get('compared')} | {h.get('identical')} "
                 f"| {h.get('different')} | {h.get('accepted')} | {h.get('skipped')} | {h.get('error')} | {h.get('leaks_b')} |")
    L.append("")
    return "\n".join(L)
