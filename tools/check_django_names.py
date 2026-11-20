#!/usr/bin/env python3
"""CI-Prüfung "keine neuen Zugriffe auf Django-Namen" (docs/DATAMODEL-V2.md §2.1/§2.2 Stufe 0).

Das eigene Datenmodell (L6) ersetzt die Django-Tabellen von paperless-ngx Stufe für Stufe durch
`paperrust_*`-Tabellen; unter dem alten Namen bleibt bis Stufe 7 eine Kompatibilitäts-Sicht. Neuer
Code soll die alten Namen nicht mehr verwenden. Dieses Skript zählt je Crate die Zeilen im
Rust-Quelltext (`crates/<crate>/src/**/*.rs`), die einen der Namen enthalten, und vergleicht mit
`tools/django-names-baseline.json`:

* mehr Treffer als im Stand → Fehler (Exit 1) mit Fundstellen,
* weniger → Hinweis, den Stand mit `--update` zu senken (Fortschritt festhalten).

Ausgenommen sind `pl-dbmigrate` (liest/schreibt das v3.4.2-Importformat) und `pl-sync-client`
(SQLite-Replik mit eigenem Schema). Tests (`tests/`, `#[cfg(test)]`-Dateien unter `src/` zählen
mit) – wer in Tests Django-Namen braucht, legt sie in `tests/`.

Aufruf:  python3 tools/check_django_names.py [--update] [--list]
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BASELINE = ROOT / "tools" / "django-names-baseline.json"
# Django tables that the own data model replaces (DATAMODEL-V2 §1)
NAMES = [
    "documents_document",
    "documents_document_tags",
    "documents_customfieldinstance",
    "documents_note",
    "documents_tag",
    "documents_correspondent",
    "documents_documenttype",
    "documents_storagepath",
    "documents_customfield",
    "guardian_userobjectpermission",
    "guardian_groupobjectpermission",
]
EXCLUDED_CRATES = {"pl-dbmigrate", "pl-sync-client"}
PATTERN = re.compile(r"\b(" + "|".join(sorted(NAMES, key=len, reverse=True)) + r")\b")


def scan() -> tuple[dict[str, dict[str, int]], dict[tuple[str, str], list[str]]]:
    counts: dict[str, dict[str, int]] = {}
    where: dict[tuple[str, str], list[str]] = {}
    for crate in sorted((ROOT / "crates").iterdir()):
        if not crate.is_dir() or crate.name in EXCLUDED_CRATES or not (crate / "src").is_dir():
            continue
        per: dict[str, int] = {}
        for f in sorted((crate / "src").rglob("*.rs")):
            rel = f.relative_to(ROOT).as_posix()
            for no, line in enumerate(f.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
                for name in set(PATTERN.findall(line)):
                    per[name] = per.get(name, 0) + 1
                    where.setdefault((crate.name, name), []).append(f"{rel}:{no}")
        if per:
            counts[crate.name] = dict(sorted(per.items()))
    return counts, where


def main(argv: list[str]) -> int:
    counts, where = scan()
    if "--update" in argv:
        BASELINE.write_text(json.dumps(counts, indent=1, sort_keys=True) + "\n", encoding="utf-8", newline="\n")
        print(f"baseline written: {sum(sum(v.values()) for v in counts.values())} lines")
        return 0
    if "--list" in argv:
        for crate, per in counts.items():
            print(crate, " ".join(f"{k}={v}" for k, v in per.items()))
        return 0
    base = json.loads(BASELINE.read_text(encoding="utf-8")) if BASELINE.exists() else {}
    worse, better = [], []
    for crate in sorted(set(counts) | set(base)):
        for name in sorted(set(counts.get(crate, {})) | set(base.get(crate, {}))):
            now, was = counts.get(crate, {}).get(name, 0), base.get(crate, {}).get(name, 0)
            if now > was:
                worse.append((crate, name, was, now))
            elif now < was:
                better.append((crate, name, was, now))
    for crate, name, was, now in worse:
        print(f"NEW use of Django name {name} in {crate}: {was} -> {now} lines")
        for w in where.get((crate, name), [])[-(now - was):]:
            print(f"    e.g. {w}")
    if worse:
        print("Use the PaperRust tables (docs/DATAMODEL-V2.md) instead, or – if unavoidable – raise the "
              "baseline with `python3 tools/check_django_names.py --update` and explain why in the commit.")
        return 1
    if better:
        print(f"{len(better)} counts went down – lower the baseline: python3 tools/check_django_names.py --update")
    print("ok: no new uses of Django table names")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
