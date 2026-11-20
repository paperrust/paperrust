"""W23: evaluation of the standalone E2E (steps.json + access log of PaperRust).

    python -I w23_e2e_summary.py <out-dir>

Reports: steps (count, errors, network errors 4xx/5xx), requests > 2 s according to the access log
(`paperrust::access ... ms=`), 404 on metadata/social_account_providers, Redis messages in the log.
"""
import json
import re
import sys
from pathlib import Path

out = Path(sys.argv[1])
steps = json.loads((out / "steps.json").read_text(encoding="utf-8"))["steps"]
log = (out / "rust-access.log").read_text(encoding="utf-8", errors="replace") if (out / "rust-access.log").exists() else ""
log = re.sub(r"\x1b\[[0-9;]*m", "", log)

print(f"# W23 Standalone-E2E ({out.name})\n")
errs = [s for s in steps if s.get("error")]
net = [(s["id"], e) for s in steps for e in s.get("netErrors", [])]
print(f"- Schritte: {len(steps)}, mit Fehler: {len(errs)}, Netzwerkfehler (4xx/5xx/abgebrochen): {len(net)}")
for s in errs:
    print(f"  - {s['id']}: {str(s['error'])[:200]}")
for sid, e in net:
    print(f"  - {sid}: {e.get('method')} {e.get('url')} -> {e.get('status')} {e.get('error', '')}")
bad404 = [e for _, e in net if e.get("status") == 404 and ("/metadata/" in e.get("url", "") or "social_account_providers" in e.get("url", ""))]
print(f"- 404 auf metadata/social_account_providers: {len(bad404)}")

slow = []
total = 0
for line in log.splitlines():
    if "paperrust::access" not in line:
        continue
    total += 1
    m = re.search(r"\bms=(\d+)", line)
    if m and int(m.group(1)) > 2000:
        meth = re.search(r"method=(\S+)", line)
        path = re.search(r"path=(\S+)", line)
        slow.append((int(m.group(1)), meth.group(1) if meth else "?", path.group(1) if path else "?"))
print(f"- Requests laut Zugriffslog: {total}, davon > 2 s: {len(slow)}")
for ms, meth, path in sorted(slow, reverse=True)[:30]:
    print(f"  - {ms} ms {meth} {path}")
redis_lines = [l for l in log.splitlines() if re.search(r"redis", l, re.I)]
print(f"- Logzeilen mit 'redis': {len(redis_lines)}")
for l in redis_lines[:10]:
    print(f"  - {l[:220]}")
print("\n| Schritt | ms | Fehler |\n|---|---:|---|")
for s in steps:
    print(f"| {s['id']} | {s['ms']} | {(s.get('error') or '')[:60]} |")
