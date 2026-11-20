"""Small IMAP test server for the outage suite (127.0.0.1 only, standard library only).

Supports what pl-mail sends on fetching (cf. crates/pl-mail/tests/support/fake_imap.rs):
CAPABILITY, NOOP, LOGIN, SELECT/EXAMINE, STATUS, LIST, UID SEARCH (UNSEEN/SEEN/UNFLAGGED/FLAGGED/
ALL, rest ignored), UID FETCH, UID STORE, UID COPY/MOVE, EXPUNGE, LOGOUT.

Fault injection (set attributes at runtime):
  offline = True        close new connections immediately (like "network gone"/port closed)
  drop_on = "FETCH"     on the next command with this name (e.g. FETCH, STORE) cut the connection hard
                        before answering; drop_count = how often
  stall_on = "FETCH"    hang instead (no answer) until stall_secs have passed
"""
from __future__ import annotations

import re
import socket
import threading
import time


class Msg:
    def __init__(self, uid, raw, flags=()):
        self.uid = uid
        self.raw = raw
        self.flags = set(flags)


class ImapStub:
    def __init__(self, user="chaos", password="chaos-imap-local"):
        self.user, self.password = user, password
        self.folders = {"INBOX": {"uidvalidity": 4711, "uidnext": 1, "msgs": []}, "Archive": {"uidvalidity": 4712, "uidnext": 1, "msgs": []}}
        self.lock = threading.Lock()
        self.offline = False
        self.drop_on = None
        self.drop_count = 0
        self.stall_on = None
        self.stall_secs = 0
        self.log = []
        self.conns = 0
        self.sock = socket.socket()
        self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.sock.bind(("127.0.0.1", 0))
        self.port = self.sock.getsockname()[1]
        self.sock.listen(20)
        self.stop = False
        threading.Thread(target=self._accept, daemon=True).start()

    def append(self, raw: bytes, folder="INBOX", flags=()):
        with self.lock:
            f = self.folders[folder]
            uid = f["uidnext"]
            f["uidnext"] += 1
            f["msgs"].append(Msg(uid, raw, flags))
            return uid

    def flags(self, folder="INBOX"):
        with self.lock:
            return {m.uid: sorted(m.flags) for m in self.folders[folder]["msgs"]}

    def close(self):
        self.stop = True
        try:
            self.sock.close()
        except OSError:
            pass

    def _accept(self):
        while not self.stop:
            try:
                c, _ = self.sock.accept()
            except OSError:
                return
            if self.offline:
                c.close()
                continue
            self.conns += 1
            threading.Thread(target=self._serve, args=(c,), daemon=True).start()

    @staticmethod
    def _tokens(s):
        return re.findall(r'"(?:[^"\\]|\\.)*"|\([^)]*\)|\S+', s)

    @staticmethod
    def _unq(s):
        return s[1:-1].replace('\\"', '"').replace("\\\\", "\\") if s.startswith('"') else s

    @staticmethod
    def _uidset(s, all_uids):
        out = set()
        for part in s.split(","):
            if ":" in part:
                a, b = part.split(":")
                hi = max(all_uids) if all_uids else 0
                a = hi if a == "*" else int(a)
                b = hi if b == "*" else int(b)
                out.update(range(min(a, b), max(a, b) + 1))
            elif part == "*":
                out.update(all_uids[-1:])
            elif part.isdigit():
                out.add(int(part))
        return out

    def _serve(self, c: socket.socket):
        f = c.makefile("rb")
        sel = None

        def send(b):
            c.sendall(b if isinstance(b, bytes) else b.encode())
        try:
            send("* OK chaos IMAP ready\r\n")
            while True:
                line = f.readline()
                if not line:
                    return
                line = line.decode("utf-8", "replace").rstrip("\r\n")
                tag, _, rest = line.partition(" ")
                cmd, _, args = rest.partition(" ")
                cmd = cmd.upper()
                sub = args.split(" ", 1)[0].upper() if cmd == "UID" else cmd
                self.log.append(sub if cmd == "UID" else cmd)
                if self.offline or (self.drop_on and sub == self.drop_on and self.drop_count > 0):
                    if not self.offline:
                        self.drop_count -= 1
                    c.shutdown(socket.SHUT_RDWR)
                    c.close()
                    return
                if self.stall_on and sub == self.stall_on:
                    time.sleep(self.stall_secs)
                    c.close()
                    return
                if cmd == "CAPABILITY":
                    send(f"* CAPABILITY IMAP4rev1 AUTH=PLAIN MOVE\r\n{tag} OK done\r\n")
                elif cmd == "NOOP":
                    send(f"{tag} OK done\r\n")
                elif cmd == "LOGIN":
                    t = self._tokens(args)
                    if len(t) == 2 and self._unq(t[0]) == self.user and self._unq(t[1]) == self.password:
                        send(f"{tag} OK LOGIN completed\r\n")
                    else:
                        send(f"{tag} NO [AUTHENTICATIONFAILED] Invalid credentials\r\n")
                elif cmd in ("SELECT", "EXAMINE"):
                    name = self._unq(args.strip())
                    with self.lock:
                        fo = self.folders.get(name)
                        if fo:
                            sel = name
                            send(f"* {len(fo['msgs'])} EXISTS\r\n* 0 RECENT\r\n* OK [UIDVALIDITY {fo['uidvalidity']}] ok\r\n"
                                 f"{tag} OK [READ-WRITE] SELECT completed\r\n")
                        else:
                            sel = None
                            send(f"{tag} NO no such mailbox\r\n")
                elif cmd == "STATUS":
                    name = self._unq(self._tokens(args)[0])
                    with self.lock:
                        fo = self.folders.get(name)
                    send(f"* STATUS \"{name}\" (UIDVALIDITY {fo['uidvalidity']})\r\n{tag} OK done\r\n" if fo else f"{tag} NO no such mailbox\r\n")
                elif cmd == "LIST":
                    out = "".join(f"* LIST (\\HasNoChildren) \"/\" \"{n}\"\r\n" for n in self.folders)
                    send(out + f"{tag} OK LIST completed\r\n")
                elif cmd == "EXPUNGE":
                    with self.lock:
                        if sel:
                            self.folders[sel]["msgs"] = [m for m in self.folders[sel]["msgs"] if "\\Deleted" not in m.flags]
                    send(f"{tag} OK EXPUNGE completed\r\n")
                elif cmd == "LOGOUT":
                    send(f"* BYE bye\r\n{tag} OK LOGOUT completed\r\n")
                    return
                elif cmd == "UID" and sel:
                    _, _, sargs = args.partition(" ")
                    with self.lock:
                        fo = self.folders[sel]
                        msgs = list(fo["msgs"])
                    uids = [m.uid for m in msgs]
                    if sub == "SEARCH":
                        u = sargs.upper()
                        hits = []
                        for m in msgs:
                            if "UNSEEN" in u and "\\Seen" in m.flags:
                                continue
                            if re.search(r"(^|\s)SEEN", u) and "UNSEEN" not in u and "\\Seen" not in m.flags:
                                continue
                            if "UNFLAGGED" in u and "\\Flagged" in m.flags:
                                continue
                            hits.append(str(m.uid))
                        send(f"* SEARCH{' ' if hits else ''}{' '.join(hits)}\r\n{tag} OK SEARCH completed\r\n")
                    elif sub == "FETCH":
                        s = self._uidset(sargs.split(" ", 1)[0], uids)
                        out = bytearray()
                        for seq, m in enumerate(msgs, 1):
                            if m.uid in s:
                                out += (f"* {seq} FETCH (UID {m.uid} FLAGS ({' '.join(sorted(m.flags))}) RFC822.SIZE {len(m.raw)} "
                                        f"BODY[] {{{len(m.raw)}}}\r\n").encode() + m.raw + b")\r\n"
                        send(bytes(out) + f"{tag} OK FETCH completed\r\n".encode())
                    elif sub == "STORE":
                        t = self._tokens(sargs)
                        s = self._uidset(t[0], uids)
                        op = t[1].upper()
                        vals = [v for x in t[2:] for v in x.strip("()").split()]
                        out = ""
                        with self.lock:
                            for seq, m in enumerate(fo["msgs"], 1):
                                if m.uid in s:
                                    if op.startswith("+"):
                                        m.flags.update(vals)
                                    elif op.startswith("-"):
                                        m.flags.difference_update(vals)
                                    out += f"* {seq} FETCH (UID {m.uid} FLAGS ({' '.join(sorted(m.flags))}))\r\n"
                        send(out + f"{tag} OK STORE completed\r\n")
                    elif sub in ("COPY", "MOVE"):
                        t = self._tokens(sargs)
                        s = self._uidset(t[0], uids)
                        dest = self._unq(t[1])
                        with self.lock:
                            if dest not in self.folders:
                                send(f"{tag} NO [TRYCREATE] no such mailbox\r\n")
                                continue
                            d = self.folders[dest]
                            for m in [m for m in fo["msgs"] if m.uid in s]:
                                d["msgs"].append(Msg(d["uidnext"], m.raw, m.flags))
                                d["uidnext"] += 1
                            if sub == "MOVE":
                                fo["msgs"] = [m for m in fo["msgs"] if m.uid not in s]
                        send(f"{tag} OK {sub} completed\r\n")
                    else:
                        send(f"{tag} BAD unknown UID command\r\n")
                else:
                    send(f"{tag} BAD unknown command\r\n")
        except OSError:
            return
        finally:
            try:
                c.close()
            except OSError:
                pass


def mail_with_pdf(subject, pdf: bytes, filename, sender="chaos@example.test"):
    import email.mime.application
    import email.mime.multipart
    import email.mime.text
    import email.utils
    m = email.mime.multipart.MIMEMultipart()
    m["Subject"] = subject
    m["From"] = sender
    m["To"] = "inbox@example.test"
    m["Date"] = email.utils.formatdate(localtime=True)
    m["Message-ID"] = email.utils.make_msgid(domain="chaos.test")
    m.attach(email.mime.text.MIMEText("Testmail der Ausfall-Suite", "plain", "utf-8"))
    a = email.mime.application.MIMEApplication(pdf, _subtype="pdf")
    a.add_header("Content-Disposition", "attachment", filename=filename)
    m.attach(a)
    return m.as_bytes()
