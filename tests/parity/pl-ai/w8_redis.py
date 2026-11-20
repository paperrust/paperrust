"""Minimal RESP client: only delete/list keys with prefix w8 (no foreign access)."""
import socket
import sys

s = socket.create_connection(("127.0.0.1", int(sys.argv[1])), 3)
f = s.makefile("rb")


def cmd(*a):
    a = [x if isinstance(x, bytes) else str(x).encode() for x in a]
    s.sendall(b"*%d\r\n" % len(a) + b"".join(b"$%d\r\n%s\r\n" % (len(x), x) for x in a))
    return read()


def read():
    line = f.readline().rstrip(b"\r\n")
    t, rest = line[:1], line[1:]
    if t in (b"+", b"-", b":"):
        return rest
    if t == b"$":
        n = int(rest)
        return None if n < 0 else f.read(n + 2)[:-2]
    if t == b"*":
        return [read() for _ in range(int(rest))]


cur = b"0"
keys = []
while True:
    cur, ks = cmd(b"SCAN", cur, b"MATCH", b"w8*", b"COUNT", b"1000")
    keys += ks or []
    if cur == b"0":
        break
if sys.argv[2] == "flush":
    if keys:
        cmd(b"DEL", *keys)
else:
    for k in keys:
        print(k.decode())
