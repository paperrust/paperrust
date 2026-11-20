# Re-implementation of Django 5.2 signing/session/hasher code, verbatim algorithms.
import hashlib, hmac, base64, zlib, json, sys, os
out = sys.argv[1]
def salted_hmac(key_salt, value, secret, algorithm="sha256"):
    hasher = getattr(hashlib, algorithm)
    key = hasher(key_salt.encode() + secret.encode()).digest()
    return hmac.new(key, msg=value.encode(), digestmod=hasher)
def b64_encode(s): return base64.urlsafe_b64encode(s).strip(b"=")
BASE62 = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz"
def b62_encode(s):
    if s == 0: return "0"
    sign = "-" if s < 0 else ""; s = abs(s); encoded = ""
    while s > 0:
        s, rem = divmod(s, 62); encoded = BASE62[rem] + encoded
    return sign + encoded
def signature(salt, value, key):
    return b64_encode(salted_hmac(salt + "signer", value, key).digest()).decode()
def dumps(obj, salt, key, compress, ts):
    data = json.dumps(obj, separators=(",", ":")).encode("latin-1")
    is_compressed = False
    if compress:
        compressed = zlib.compress(data)
        if len(compressed) < (len(data) - 1):
            data = compressed; is_compressed = True
    base64d = b64_encode(data).decode()
    if is_compressed: base64d = "." + base64d
    value = "%s:%s" % (base64d, b62_encode(ts))
    return "%s:%s" % (value, signature(salt, value, key))
def w(name, s):
    with open(os.path.join(out, name), "w", newline="") as f: f.write(s)
pw = "pbkdf2_sha256$1000000$abc$def"
h = salted_hmac("django.contrib.auth.models.AbstractBaseUser.get_session_auth_hash", pw, "test-secret").hexdigest()
w("session_auth_hash.txt", h)
sess = {"_auth_user_id": "1", "_auth_user_backend": "django.contrib.auth.backends.ModelBackend", "_auth_user_hash": h}
w("session_json.txt", json.dumps(sess, separators=(",", ":")))
w("session_data.txt", dumps(sess, "django.contrib.sessions.SessionStore", "test-secret", True, 1700000000))
w("signed_plain.txt", dumps({"a": 1}, "my.salt", "k", False, 1700000000))
print(b62_encode(1700000000))
# PBKDF2 vectors (Django PBKDF2PasswordHasher.encode)
def pbkdf2(algo, password, salt, iterations):
    hash = hashlib.pbkdf2_hmac(algo, password.encode(), salt.encode(), iterations)
    return base64.b64encode(hash).decode("ascii").strip()
w("pbkdf2_sha256.txt", "pbkdf2_sha256$%d$%s$%s" % (1000, "seasalt1234567890abcde", pbkdf2("sha256", "lètmein", "seasalt1234567890abcde", 1000)))
w("pbkdf2_sha1.txt", "pbkdf2_sha1$%d$%s$%s" % (1000, "seasalt2", pbkdf2("sha1", "lètmein", "seasalt2", 1000)))
# csrf masking example (django.middleware.csrf._mask_cipher_secret with fixed mask)
chars = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
secret = "A" * 16 + "z9" * 8
mask = "0123456789abcdefghijklmnopqrstuv"
pairs = zip((chars.index(x) for x in secret), (chars.index(x) for x in mask))
cipher = "".join(chars[(x + y) % len(chars)] for x, y in pairs)
w("csrf_secret.txt", secret); w("csrf_masked.txt", mask + cipher)
for s in [b"YWRtaW46cGFzcw==", b"YWRt aW46cGFzcw==", b"YWRtaW46cGFzcw==junk", b"YWRtaW46cGFzcw", b"Y=WRtaW46cGFzcw==", b"YWRtaW46cGFzcwA="]:
    try: print(s, base64.b64decode(s))
    except Exception as e: print(s, "ERR", e)
