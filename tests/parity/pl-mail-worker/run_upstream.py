"""Upstream run of the mail worker parity: unchanged paperless_mail code against the local
fake IMAP server and the own DB copy pl_w6_test_up.

`documents.tasks.consume_file` is replaced by a recording Celery task (the
reference environment has neither OCR nor Ghostscript; what is compared here is the mail side:
files, overrides, ProcessedMail, IMAP actions, correspondents). Celery eager, cache local.

Environment: PL_W6_OUT (JSON result), PL_W6_RUNS (number of runs, default 1), PAPERLESS_DBNAME,
PAPERLESS_SCRATCH_DIR. Invocation via run_upstream.sh.
"""

import hashlib
import json
import os
import sys
from pathlib import Path


def main() -> int:
    import django

    django.setup()
    from celery import shared_task

    import paperless_mail.mail as mail_mod
    from paperless_mail.tasks import process_mail_accounts

    recorded = []

    @shared_task(name="pl_w6_record_consume")
    def record_consume(input_doc=None, overrides=None):
        p = Path(input_doc.original_file)
        data = p.read_bytes()
        # .eml: Zeilenenden angleichen (PaperRust schreibt seit upstream fix a1d76f3a1 CRLF, v3.4.2 LF)
        if p.suffix == ".eml":
            data = data.replace(b"\r\n", b"\n")
        ov = {k: v for k, v in vars(overrides).items() if v is not None}
        recorded.append(
            {
                "source": int(input_doc.source),
                "mailrule_id": input_doc.mailrule_id,
                "mime_type": input_doc.mime_type,
                "file_name": p.name if p.suffix != ".eml" else "<tmp>.eml",
                "sha256": hashlib.sha256(data).hexdigest(),
                "overrides": ov,
            },
        )
        return "recorded"

    mail_mod.consume_file = record_consume

    results = []
    for _ in range(int(os.environ.get("PL_W6_RUNS", "1"))):
        r = process_mail_accounts.apply(kwargs={"account_ids": None})
        results.append(r.get(propagate=False) if r.successful() else f"error: {r.result!r}")
    out = {"results": results, "consumed": recorded}
    Path(os.environ["PL_W6_OUT"]).write_text(json.dumps(out, ensure_ascii=False, indent=1, default=str), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
