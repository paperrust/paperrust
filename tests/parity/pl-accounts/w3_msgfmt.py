"""Compiles upstream/src/locale/*/LC_MESSAGES/django.po to <out>/<lang>/LC_MESSAGES/django.mo.

Like `manage.py compilemessages` in the Docker image (GNU msgfmt): `#, fuzzy` entries are
skipped, empty msgstr as well. The reference instance has no .mo files (upstream ships only
.po), so paperless's own texts would never be translated there. Only for the W3 parity instance.

Invocation: python -I w3_msgfmt.py <upstream/src/locale> <out>
"""

import ast
import os
import struct
import sys


def parse_po(path):
    entries = []
    cur = {}
    key = None
    fuzzy = False

    def flush():
        nonlocal cur, fuzzy
        if "msgid" in cur and not fuzzy:
            entries.append(cur)
        cur, fuzzy = {}, False

    with open(path, encoding="utf-8") as f:
        for raw in f:
            line = raw.strip()
            if not line:
                flush()
                key = None
                continue
            if line.startswith("#,") and "fuzzy" in line:
                fuzzy = True
                continue
            if line.startswith("#"):
                continue
            if line.startswith('"'):
                cur[key] = cur.get(key, "") + ast.literal_eval(line)
                continue
            k, _, v = line.partition(" ")
            if k == "msgid" and "msgid" in cur:
                flush()
            key = k
            cur[key] = ast.literal_eval(v)
    flush()
    return entries


def compile_mo(entries):
    msgs = {}
    for e in entries:
        mid = e["msgid"]
        if "msgctxt" in e:
            mid = e["msgctxt"] + "\x04" + mid
        if "msgid_plural" in e:
            mid = mid + "\x00" + e["msgid_plural"]
            forms = [e[k] for k in sorted(k for k in e if k.startswith("msgstr["))]
            if not any(forms):
                continue
            mstr = "\x00".join(forms)
        else:
            mstr = e.get("msgstr", "")
            if not mstr and e["msgid"] != "":
                continue
        msgs[mid] = mstr
    keys = sorted(msgs)
    ids = b""
    strs = b""
    offsets = []
    for k in keys:
        kb, vb = k.encode(), msgs[k].encode()
        offsets.append((len(ids), len(kb), len(strs), len(vb)))
        ids += kb + b"\x00"
        strs += vb + b"\x00"
    n = len(keys)
    keystart = 7 * 4 + 16 * n
    valuestart = keystart + len(ids)
    koffsets, voffsets = [], []
    for o1, l1, o2, l2 in offsets:
        koffsets += [l1, o1 + keystart]
        voffsets += [l2, o2 + valuestart]
    out = struct.pack("Iiiiiii", 0x950412DE, 0, n, 7 * 4, 7 * 4 + n * 8, 0, 0)
    out += struct.pack(f"{len(koffsets)}i", *koffsets)
    out += struct.pack(f"{len(voffsets)}i", *voffsets)
    return out + ids + strs


def main():
    src, out = sys.argv[1], sys.argv[2]
    for lang in sorted(os.listdir(src)):
        po = os.path.join(src, lang, "LC_MESSAGES", "django.po")
        if not os.path.isfile(po):
            continue
        dst = os.path.join(out, lang, "LC_MESSAGES")
        os.makedirs(dst, exist_ok=True)
        with open(os.path.join(dst, "django.mo"), "wb") as f:
            f.write(compile_mo(parse_po(po)))


if __name__ == "__main__":
    main()
