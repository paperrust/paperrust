"""Reference values per test mail from unchanged upstream code (imap_tools + email, without DB).

For every mails/*.eml: subject, from_, from_values.name, date (make_aware in TIME_ZONE),
attachments (filename, content_disposition, content_type, sha256, magic MIME) and the
.eml bytes like MailAccountHandler._process_eml (From to the front + as_bytes) → expected/.

Invocation (Git Bash): source reference/env.sh && "$PY" tests/parity/pl-mail-worker/upstream_messages.py
"""

import hashlib
import json
import os
import sys
import zoneinfo
from datetime import timezone as dt_tz
from pathlib import Path

HERE = Path(__file__).resolve().parent


def main() -> int:
    import magic
    from imap_tools import MailMessage

    tz = zoneinfo.ZoneInfo(os.environ.get("PAPERLESS_TIME_ZONE", "Europe/Zurich"))
    out_dir = HERE / "expected"
    out_dir.mkdir(exist_ok=True)
    result = {}
    for p in sorted((HERE / "mails").glob("*.eml")):
        raw = p.read_bytes()
        m = MailMessage([(b"1 (UID 1 FLAGS () BODY[] {%d}" % len(raw), raw), b")"])
        d = m.date
        if d.tzinfo is None:
            d = d.replace(tzinfo=tz)
        atts = []
        for a in m.attachments:
            atts.append(
                {
                    "filename": a.filename,
                    "content_disposition": a.content_disposition,
                    "content_type": a.content_type,
                    "sha256": hashlib.sha256(a.payload).hexdigest(),
                    "magic": magic.from_buffer(a.payload, mime=True),
                },
            )
        fv = m.from_values
        # _process_eml: last "From" header line to the front, then as_bytes()
        from_element = None
        for i, header in enumerate(m.obj._headers):
            if header[0] == "From":
                from_element = i
        if from_element:
            new_headers = [m.obj._headers.pop(from_element)]
            new_headers += m.obj._headers
            m.obj._headers = new_headers
        try:
            eml = m.obj.as_bytes()
            (out_dir / p.name).write_bytes(eml)
            eml_sha = hashlib.sha256(eml).hexdigest()
        except Exception as e:  # noqa: BLE001
            eml_sha = f"error: {type(e).__name__}"
        result[p.name] = {
            "uid": m.uid,
            "subject": m.subject,
            "from_": m.from_,
            "from_name": fv.name if fv else None,
            "date": d.isoformat(),
            "attachments": atts,
            "eml_sha256": eml_sha,
        }
    (out_dir / "messages.json").write_text(json.dumps(result, ensure_ascii=False, indent=1), encoding="utf-8")
    print(f"{len(result)} messages -> {out_dir}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
