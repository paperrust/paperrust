#!/usr/bin/env python3
"""Geheimnis-Suchlauf über die ganze Git-Geschichte (alle Refs, jede hinzugefügte Zeile).

    python tests/security/scan_history.py [--out results/release/security/secrets-history.md]

Gibt NUR Fundstellen aus (Commit, Datei, Art, Zeilenhash) – **nie den Wert**. Bekannte lokale
Testwerte (reference/users.json, reference/README.md, Schein-Tokens) werden als "Testwert"
eingestuft, nicht als Befund. Muster angelehnt an gitleaks (Standardregeln) plus PaperRust-eigene
(Datenbank-Dumps, .env, echte personenbezogene Daten: IBAN, Mailadressen außerhalb von
Beispiel-Domains, Telefonnummern).
"""

from __future__ import annotations

import argparse
import hashlib
import re
import subprocess
import sys
from collections import defaultdict
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent.parent

RULES: list[tuple[str, re.Pattern]] = [
    ("private-key", re.compile(r"-----BEGIN (?:RSA |EC |DSA |OPENSSH |ENCRYPTED |PGP )?PRIVATE KEY")),
    ("aws-access-key", re.compile(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b")),
    ("github-token", re.compile(r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36}\b|\bgithub_pat_[A-Za-z0-9_]{60,}")),
    ("gitea-token", re.compile(r"(?i)gitea[_-]?token\s*[:=]\s*['\"]?[0-9a-f]{40}")),
    ("anthropic-key", re.compile(r"\bsk-ant-[A-Za-z0-9_-]{20,}")),
    ("openai-key", re.compile(r"\bsk-(?:proj-)?[A-Za-z0-9]{32,}")),
    ("google-api-key", re.compile(r"\bAIza[0-9A-Za-z_-]{35}\b")),
    ("slack-token", re.compile(r"\bxox[abprs]-[A-Za-z0-9-]{10,}")),
    ("stripe-key", re.compile(r"\b(?:sk|rk)_live_[A-Za-z0-9]{20,}")),
    ("jwt", re.compile(r"\beyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}")),
    ("azure-key", re.compile(r"(?i)(?:azure|cognitive|documentintelligence)[A-Za-z_]*key\s*[:=]\s*['\"]?[0-9a-f]{32}\b")),
    ("password-assignment", re.compile(
        r"(?i)\b(?:password|passwd|pwd|secret|api[_-]?key|token|dbpass)\b[\"']?\s*[:=]\s*[\"']([^\"'\s{}$<>()]{8,})[\"']")),
    ("url-credentials", re.compile(r"\b[a-z][a-z0-9+.-]*://[^/\s:@'\"]+:([^/\s:@'\"]{4,})@[^\s'\"]+")),
    ("iban", re.compile(r"\b[A-Z]{2}\d{2}(?: ?[A-Z0-9]{4}){3,7}(?: ?[A-Z0-9]{1,3})?\b")),
    ("email", re.compile(r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b")),
    ("phone", re.compile(r"(?<![\w.])\+(?:41|49|43)[ \d]{9,14}\b")),
]

# Files that are sensitive as a whole (dumps, environments, keys, databases)
FILE_RULES: list[tuple[str, re.Pattern]] = [
    ("env-file", re.compile(r"(^|/)\.env(\.[^/]*)?$")),
    ("db-dump", re.compile(r"\.(?:dump|sql\.gz|bak|backup|sqlite3?|db)$|(^|/)[^/]*dump[^/]*\.sql$", re.I)),
    ("key-file", re.compile(r"\.(?:pem|key|p12|pfx|jks|keystore|ppk)$|(^|/)id_(?:rsa|ed25519|ecdsa)$", re.I)),
    ("credentials-file", re.compile(r"(^|/)(?:credentials|secrets?)(\.[a-z]+)?$|\.netrc$|\.pgpass$|\.npmrc$", re.I)),
]

# Test values and example domains: are counted, but not reported as a finding
EXAMPLE_DOMAINS = re.compile(
    r"@(?:example\.(?:com|org|net)|[a-z0-9.-]+\.(?:invalid|test|example|localhost|local)|localhost|"
    r"anthropic\.com|users\.noreply\.github\.com|paperless-ngx\.com|paperless\.com|test\.com|"
    r"mail\.com|domain\.(?:com|tld)|email\.com|foo\.(?:com|bar)|bar\.com|x\.y|b\.c|a\.b|host|server|"
    r"[a-z0-9-]+\.(?:example)|python\.org|github\.com|gmail\.com\.invalid)\b", re.I)
SAFE_PATHS = re.compile(r"^(?:upstream/|target/|reference/(?:venv|downloads|pg|bin|data|logs|run)/|"
                        r"frontend(?:-light)?/node_modules/|fuzz/corpus/|fuzz/artifacts/)")
TEST_VALUE_HINT = re.compile(
    r"(?i)(?:local[-_ ]?only|paperless|admin|alice|bob|carol|dave|test|example|dummy|fake|changeme|secret[-_]?key[-_]?"
    r"local|placeholder|xxxx|0123456789|a0a0a0|a1a1a1|b0b0b0|c0c0c0|d0d0d0|postgres-local|wrong|probe|hunter2|"
    r"geheim|passwort|password|correct-horse|sample|mock|schein|demo)")


def classify(kind: str, value: str, line: str, path: str) -> str:
    if kind == "email":
        if EXAMPLE_DOMAINS.search(value) or value.lower().startswith(("noreply@", "no-reply@")):
            return "example"
        return "finding"
    if kind in ("password-assignment", "url-credentials") and TEST_VALUE_HINT.search(value):
        return "testvalue"
    if kind == "iban":
        # Only checksum-valid IBANs count; known example IBANs (DE89 3704 0044 0532 0130 00 and the like)
        compact = value.replace(" ", "")
        if not iban_ok(compact):
            return "ignore"
        if compact in KNOWN_EXAMPLE_IBANS:
            return "example"
        return "finding"
    if kind == "jwt" and ("test" in path.lower() or "fixture" in path.lower()):
        return "testvalue"
    if kind == "phone" and ("test" in path.lower()):
        return "testvalue"
    return "finding"


KNOWN_EXAMPLE_IBANS = {"DE89370400440532013000", "CH9300762011623852957", "GB82WEST12345698765432",
                       "CH5604835012345678009", "DE02120300000000202051", "AT611904300234573201",
                       "CH4431999123000889012", "DE75512108001245126199", "CH9300762011623852957"}


def iban_ok(s: str) -> bool:
    if not re.fullmatch(r"[A-Z]{2}\d{2}[A-Z0-9]{11,30}", s):
        return False
    r = s[4:] + s[:4]
    n = "".join(str(int(c, 36)) for c in r)
    return int(n) % 97 == 1


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=str(REPO / "results/release/security/secrets-history.md"))
    args = ap.parse_args()
    proc = subprocess.Popen(
        ["git", "-c", "core.quotepath=off", "log", "--all", "-p", "--no-color", "--no-ext-diff", "-U0",
         "--format=COMMIT %H %ad", "--date=short", "--diff-filter=AM", "--no-renames"],
        cwd=REPO, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    assert proc.stdout
    commit = date = path = ""
    findings: dict[tuple, set] = defaultdict(set)  # (kind, path, class) -> {commit}
    file_hits: dict[tuple, set] = defaultdict(set)
    counts: dict[str, int] = defaultdict(int)
    binary_files: set[str] = set()
    for raw in proc.stdout:
        line = raw.decode("utf-8", "replace").rstrip("\n")
        if line.startswith("COMMIT "):
            _, commit, date = line.split(" ", 2)
            continue
        if line.startswith("+++ "):
            path = line[6:] if line.startswith("+++ b/") else line[4:]
            if path != "/dev/null":
                for kind, rx in FILE_RULES:
                    if rx.search(path) and not SAFE_PATHS.search(path):
                        file_hits[(kind, path)].add(commit[:10])
            continue
        if line.startswith("Binary files ") and " and b/" in line:
            binary_files.add(line.rsplit(" and b/", 1)[1].replace(" differ", ""))
            continue
        if not line.startswith("+") or line.startswith("+++"):
            continue
        if SAFE_PATHS.search(path):
            continue
        body = line[1:]
        if len(body) > 4000:
            body = body[:4000]
        for kind, rx in RULES:
            for m in rx.finditer(body):
                val = m.group(1) if m.groups() and m.group(1) else m.group(0)
                cls = classify(kind, val, body, path)
                counts[f"{kind}:{cls}"] += 1
                if cls in ("finding", "testvalue", "example"):
                    h = hashlib.sha256(val.encode()).hexdigest()[:10]
                    findings[(kind, path, cls, h)].add(commit[:10])
    proc.wait()

    real = sorted(k for k in findings if k[2] == "finding")
    lines = ["# Geheimnis-Suchlauf über die Git-Geschichte", "",
             f"Alle Refs (`git log --all -p`, hinzugefügte Zeilen), {len(set(c for v in findings.values() for c in v))} "
             "Commits mit Treffern. **Keine Werte**, nur Art, Datei, Commit(s) und ein Kurz-Hash des Werts "
             "(SHA-256, 10 Zeichen) zum Wiedererkennen.", "",
             "## Zählung je Regel/Einstufung", "", "| Regel:Einstufung | Treffer |", "|---|---:|"]
    lines += [f"| {k} | {v} |" for k, v in sorted(counts.items())]
    lines += ["", "## Verdächtige Dateien (Name)", ""]
    lines += [f"- {kind}: `{p}` (Commits {', '.join(sorted(c)[:5])})" for (kind, p), c in sorted(file_hits.items())] or ["- keine"]
    lines += ["", "## Treffer zur Prüfung (Einstufung finding)", "",
              "| Art | Datei | Wert-Hash | Commits |", "|---|---|---|---|"]
    for k in real:
        cs = sorted(findings[k])
        lines.append(f"| {k[0]} | `{k[1]}` | {k[3]} | {', '.join(cs[:4])}{' …' if len(cs) > 4 else ''} |")
    lines += ["", "## Testwerte/Beispiele (nur Anzahl je Datei)", ""]
    per_file: dict[str, int] = defaultdict(int)
    for k in findings:
        if k[2] != "finding":
            per_file[k[1]] += 1
    lines += [f"- `{p}`: {n}" for p, n in sorted(per_file.items(), key=lambda x: -x[1])[:60]]
    lines += ["", f"## Binärdateien in der Geschichte ({len(binary_files)})", ""]
    lines += [f"- `{p}`" for p in sorted(binary_files)[:400]]
    Path(args.out).parent.mkdir(parents=True, exist_ok=True)
    Path(args.out).write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"findings={len(real)} files={len(file_hits)} binaries={len(binary_files)} -> {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
