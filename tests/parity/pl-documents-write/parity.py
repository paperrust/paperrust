#!/usr/bin/env python3
"""Paritätsvergleich Django ↔ PaperRust für pl-documents-write.

Standard: nur Anfragen, die upstream **nichts schreiben** (Validierungsfehler, 403, 415, 401).
Mit ``--allow-writes`` zusätzlich schreibende Fälle – NUR gegen eine wegwerfbare Datenbank-Kopie,
nie gegen die Referenzdaten (AGENTS.md / WAVE1.md).

Aufruf:
    python parity.py --django http://127.0.0.1:8000 --rust http://127.0.0.1:8100 \
        --user admin --password admin [--allow-writes]

Vergleicht Status, Content-Type (ohne Parameter) und Körper (JSON strukturell inkl. Schlüssel-
reihenfolge, sonst Text). Ausgabe: eine Zeile pro Fall, am Ende ``same=N different=M``.
"""

from __future__ import annotations

import argparse
import base64
import json
import sys
import urllib.error
import urllib.request

READ_ONLY_CASES: list[tuple[str, str, object, str]] = [
    # (name, path, body, content_type)
    ("bulk_edit: leerer Body", "/api/documents/bulk_edit/", None, "application/json"),
    ("bulk_edit: falscher Content-Type", "/api/documents/bulk_edit/", "x=1", "text/plain"),
    ("bulk_edit: ungültige Methode", "/api/documents/bulk_edit/", {"documents": [1], "method": "nope"}, "application/json"),
    ("bulk_edit: documents kein Array", "/api/documents/bulk_edit/", {"documents": "1", "method": "add_tag", "parameters": {"tag": 1}}, "application/json"),
    ("bulk_edit: documents Elementfehler", "/api/documents/bulk_edit/", {"documents": [1, "a", None], "method": "add_tag", "parameters": {"tag": 1}}, "application/json"),
    ("bulk_edit: ohne documents", "/api/documents/bulk_edit/", {"method": "add_tag", "parameters": {"tag": 1}}, "application/json"),
    ("bulk_edit: excluded ohne all", "/api/documents/bulk_edit/", {"documents": [], "excluded_documents": [1], "method": "add_tag", "parameters": {"tag": 1}}, "application/json"),
    ("bulk_edit: unbekanntes Dokument", "/api/documents/bulk_edit/", {"documents": [999999999], "method": "add_tag", "parameters": {"tag": 1}}, "application/json"),
    ("bulk_edit: tag fehlt", "/api/documents/bulk_edit/", {"documents": [], "method": "add_tag", "parameters": {}}, "application/json"),
    ("bulk_edit: tag unbekannt", "/api/documents/bulk_edit/", {"documents": [], "method": "remove_tag", "parameters": {"tag": 999999999}}, "application/json"),
    ("bulk_edit: correspondent fehlt", "/api/documents/bulk_edit/", {"documents": [], "method": "set_correspondent", "parameters": {}}, "application/json"),
    ("bulk_edit: correspondent unbekannt", "/api/documents/bulk_edit/", {"documents": [], "method": "set_correspondent", "parameters": {"correspondent": 999999999}}, "application/json"),
    ("bulk_edit: document_type unbekannt", "/api/documents/bulk_edit/", {"documents": [], "method": "set_document_type", "parameters": {"document_type": 999999999}}, "application/json"),
    ("bulk_edit: storage_path fehlt", "/api/documents/bulk_edit/", {"documents": [], "method": "set_storage_path", "parameters": {}}, "application/json"),
    ("bulk_edit: modify_tags ohne remove", "/api/documents/bulk_edit/", {"documents": [], "method": "modify_tags", "parameters": {"add_tags": []}}, "application/json"),
    ("bulk_edit: modify_tags kein Array", "/api/documents/bulk_edit/", {"documents": [], "method": "modify_tags", "parameters": {"add_tags": "1", "remove_tags": []}}, "application/json"),
    ("bulk_edit: modify_tags Floats", "/api/documents/bulk_edit/", {"documents": [], "method": "modify_tags", "parameters": {"add_tags": [1.0], "remove_tags": []}}, "application/json"),
    ("bulk_edit: modify_tags unbekannt", "/api/documents/bulk_edit/", {"documents": [], "method": "modify_tags", "parameters": {"add_tags": [999999999], "remove_tags": []}}, "application/json"),
    ("bulk_edit: set_permissions fehlt", "/api/documents/bulk_edit/", {"documents": [], "method": "set_permissions", "parameters": {}}, "application/json"),
    ("bulk_edit: set_permissions bool", "/api/documents/bulk_edit/", {"documents": [], "method": "set_permissions", "parameters": {"set_permissions": False}}, "application/json"),
    ("bulk_edit: set_permissions users bool", "/api/documents/bulk_edit/", {"documents": [], "method": "set_permissions", "parameters": {"set_permissions": {"view": {"users": False}}}}, "application/json"),
    ("bulk_edit: set_permissions unbekannte Aktion", "/api/documents/bulk_edit/", {"documents": [], "method": "set_permissions", "parameters": {"set_permissions": {"zzz": {}, "aaa": {}}}}, "application/json"),
    ("bulk_edit: set_permissions Nutzer fehlt", "/api/documents/bulk_edit/", {"documents": [], "method": "set_permissions", "parameters": {"set_permissions": {"view": {"users": [999999999]}}}}, "application/json"),
    ("bulk_edit: owner unbekannt", "/api/documents/bulk_edit/", {"documents": [], "method": "set_permissions", "parameters": {"set_permissions": None, "owner": 999999999}}, "application/json"),
    ("bulk_edit: reprocess remote_ocr Text", "/api/documents/bulk_edit/", {"documents": [], "method": "reprocess", "parameters": {"remote_ocr": "yes"}}, "application/json"),
    ("bulk_edit: source_mode ungültig", "/api/documents/bulk_edit/", {"documents": [], "method": "add_tag", "parameters": {"tag": 1, "source_mode": "x"}}, "application/json"),
    ("bulk_edit: parameters Liste", "/api/documents/bulk_edit/", {"documents": [], "method": "add_tag", "parameters": []}, "application/json"),
    ("bulk_edit: all kein Bool", "/api/documents/bulk_edit/", {"all": "vielleicht", "method": "add_tag", "parameters": {"tag": 1}}, "application/json"),
    ("reprocess: leerer Body", "/api/documents/reprocess/", None, "application/json"),
    ("reprocess: Liste statt Objekt", "/api/documents/reprocess/", [1, 2], "application/json"),
    ("reprocess: remote_ocr ungültig", "/api/documents/reprocess/", {"documents": [], "remote_ocr": "kaputt"}, "application/json"),
    ("reprocess: unbekanntes Dokument", "/api/documents/reprocess/", {"documents": [999999999]}, "application/json"),
]

# Writing cases: only with --allow-writes (disposable DB!). An empty selection still writes a
# PaperlessTask row for FK methods (bulk_update_documents with []).
WRITE_CASES: list[tuple[str, str, object, str]] = [
    ("bulk_edit: set_correspondent leer", "/api/documents/bulk_edit/", {"documents": [], "method": "set_correspondent", "parameters": {"correspondent": None}}, "application/json"),
    ("bulk_edit: modify_tags leer", "/api/documents/bulk_edit/", {"documents": [], "method": "modify_tags", "parameters": {"add_tags": [], "remove_tags": []}}, "application/json"),
    ("bulk_edit: unerwarteter Parameter", "/api/documents/bulk_edit/", {"documents": [], "method": "add_tag", "parameters": {"tag": 1, "foo": 1}}, "application/json"),
    ("reprocess: leere Auswahl", "/api/documents/reprocess/", {"documents": []}, "application/json"),
]


def request(base: str, path: str, body: object, content_type: str, auth: str | None):
    if body is None:
        data = b""
    elif isinstance(body, str):
        data = body.encode()
    else:
        data = json.dumps(body).encode()
    req = urllib.request.Request(base + path, data=data, method="POST")
    req.add_header("Content-Type", content_type)
    req.add_header("Accept", "application/json; version=10")
    if auth:
        req.add_header("Authorization", auth)
    try:
        with urllib.request.urlopen(req, timeout=30) as resp:
            return resp.status, resp.headers.get("Content-Type", ""), resp.read()
    except urllib.error.HTTPError as e:
        return e.code, e.headers.get("Content-Type", ""), e.read()


def normalize(ct: str, raw: bytes):
    main = ct.split(";")[0].strip()
    if main == "application/json":
        try:
            # object_pairs_hook: also compare the order of the keys
            return main, json.loads(raw, object_pairs_hook=lambda p: p)
        except ValueError:
            return main, raw
    return main, raw


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--django", default="http://127.0.0.1:8000")
    ap.add_argument("--rust", default="http://127.0.0.1:8100")
    ap.add_argument("--user", required=True)
    ap.add_argument("--password", required=True)
    ap.add_argument("--allow-writes", action="store_true")
    args = ap.parse_args()

    auth = "Basic " + base64.b64encode(f"{args.user}:{args.password}".encode()).decode()
    cases = list(READ_ONLY_CASES)
    # without login
    cases_noauth = [("bulk_edit: ohne Anmeldung", "/api/documents/bulk_edit/", {}, "application/json")]
    if args.allow_writes:
        cases += WRITE_CASES

    same = different = 0
    for (name, path, body, ct), with_auth in [(c, True) for c in cases] + [(c, False) for c in cases_noauth]:
        a = auth if with_auth else None
        d = request(args.django, path, body, ct, a)
        r = request(args.rust, path, body, ct, a)
        nd = (d[0],) + normalize(d[1], d[2])
        nr = (r[0],) + normalize(r[1], r[2])
        if nd == nr:
            same += 1
            print(f"same       {name}")
        else:
            different += 1
            print(f"DIFFERENT  {name}\n  django: {nd!r}\n  rust:   {nr!r}")
    print(f"same={same} different={different}")
    return 0 if different == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
