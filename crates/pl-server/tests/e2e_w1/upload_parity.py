"""post_document error parity: same invalid requests to Django (8000, read-only: every case is
rejected before anything is written) and PaperRust (8110). Compares status, content type, body."""

import sys

import requests

A = "http://127.0.0.1:8000/api/documents/post_document/"
B = "http://127.0.0.1:8110/api/documents/post_document/"
ALICE = {"Authorization": "Token a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a002"}
CAROL = {"Authorization": "Token c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c0c004"}
BIN = ("b.bin", b"\x00\x01\x02binary", "application/octet-stream")

cases = [
    ("anonymous", {}, dict(files={"document": BIN})),
    ("bad token", {"Authorization": "Token nope"}, dict(files={"document": BIN})),
    ("no add perm", CAROL, dict(files={"document": BIN})),
    ("json", ALICE, dict(json={})),
    ("form urlencoded", ALICE, dict(data={"title": "x"})),
    ("empty body", ALICE, dict(data=b"")),
    ("no file", ALICE, dict(files={"title": (None, "x")})),
    ("file as text", ALICE, dict(files={"document": (None, "notafile")})),
    ("empty file", ALICE, dict(files={"document": ("e.txt", b"", "text/plain")})),
    ("binary", ALICE, dict(files={"document": BIN})),
    ("pdf name recoverable", ALICE, dict(files={"document": ("x.pdf", b"\x00\x01\x02binary", "application/pdf")}, data={"tags": "999"})),
]
fields = [
    {"created": "abc"}, {"created": "2025-13-01"}, {"created": "2025-01-01T25:00"},
    {"title": "  "}, {"title": "a\x00b"},
    {"correspondent": "abc"}, {"correspondent": "9999"}, {"correspondent": "99999999999"}, {"correspondent": "1.0"},
    {"document_type": "-1"}, {"storage_path": " 9999 "},
    {"tags": ["1", "abc"]}, {"tags": ["999", "1"]}, {"tags": ""},
    {"archive_serial_number": "-1"}, {"archive_serial_number": "4294967296"}, {"archive_serial_number": "x"},
    {"archive_serial_number": "1" * 1001}, {"archive_serial_number": "1e3"},
    {"custom_fields": "{x"}, {"custom_fields": '"x"'}, {"custom_fields": "1.5"}, {"custom_fields": "[null]"},
    {"custom_fields": '{"abc": 1}'}, {"custom_fields": '{"999": 1}'}, {"custom_fields": "[1, 999]"},
    {"custom_fields": '{"5": "abc"}'}, {"custom_fields": '{"5": 99999999999}'}, {"custom_fields": '{"5": "1.0"}'},
    {"custom_fields": '{"2": "nourl"}'}, {"custom_fields": '{"2": "http://"}'},
    {"custom_fields": '{"3": "2025-02-30"}'}, {"custom_fields": '{"7": "12.345"}'}, {"custom_fields": '{"7": "EUR12"}'},
    {"custom_fields": '{"8": "1234567890123"}'}, {"custom_fields": '{"10": "nope"}'}, {"custom_fields": '{"10": 1}'},
    {"custom_fields": '{"9": 5}'}, {"custom_fields": '{"9": [1, 1]}'}, {"custom_fields": '{"9": [99999]}'},
    {"custom_fields": '{"1": "' + "x" * 129 + '"}'},
    {"from_webui": "maybe"}, {"from_webui": "null"},
]
for f in fields:
    data = {k: v for k, v in f.items()}
    cases.append((f"field {f}", ALICE, dict(files={"document": BIN}, data=data)))

diffs = 0
for name, headers, kw in cases:
    ra = requests.post(A, headers=headers, timeout=10, **kw)
    rb = requests.post(B, headers=headers, timeout=10, **kw)
    same = (ra.status_code, ra.headers.get("content-type"), ra.content) == (rb.status_code, rb.headers.get("content-type"), rb.content)
    skip = {"date", "server", "content-length"}
    ha = {k.lower(): v for k, v in ra.headers.items() if k.lower() not in skip}
    hb = {k.lower(): v for k, v in rb.headers.items() if k.lower() not in skip}
    if ha != hb:
        print(f"HEADER DIFF {name}: django-only {sorted(set(ha.items()) - set(hb.items()))} rust-only {sorted(set(hb.items()) - set(ha.items()))}")
        same = False
    if not same:
        diffs += 1
        print(f"DIFF {name}\n  django {ra.status_code} {ra.headers.get('content-type')} {ra.text[:300]}\n  rust   {rb.status_code} {rb.headers.get('content-type')} {rb.text[:300]}")
    if ra.status_code < 400:
        print(f"WARNING {name}: django accepted ({ra.status_code}) - case wrote to the reference!")
print(f"{len(cases)} cases, {diffs} differences")
sys.exit(1 if diffs else 0)
