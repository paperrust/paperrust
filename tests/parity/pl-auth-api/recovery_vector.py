"""Recovery-code vector, same algorithm as allauth 65.20.1 RecoveryCodes.generate_codes."""
import hmac
import sys
from hashlib import sha1

seed = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff0011223344556677"
h = hmac.new(key=seed.encode("ascii"), msg=None, digestmod=sha1)
out = []
for i in range(10):
    h.update((f"{i:3},").encode())
    value = int.from_bytes(h.digest()[:4], byteorder="big", signed=False) % 10**8
    out.append(str(value).zfill(8))
open(sys.argv[1], "w", newline="\n").write(",".join(out) + "\n")
