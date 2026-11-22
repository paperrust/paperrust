#!/usr/bin/env python3
"""PaperRust Desktop (macOS): copy Mach-O programs/libraries plus every non-system dylib they need into
one directory and rewrite all references to `@loader_path/<name>` (the macOS counterpart of the
Linux RPATH `$ORIGIN` approach in pack.sh). Used by pack-macos.sh for the Homebrew Tesseract.

    macos-bundle-dylibs.py bundle --dest DIR [--manifest FILE] ENTRY...
        ENTRY = Mach-O file (e.g. /opt/homebrew/bin/tesseract, .../libtesseract.5.dylib); it is copied
        under its own file name, its dependencies under the name they are referenced by.
        Afterwards every file in DIR only references /usr/lib, /System or @loader_path/<file in DIR>.
        Files are NOT signed here: pack-macos.sh ad-hoc signs every Mach-O after all changes.
    macos-bundle-dylibs.py minos FILE...
        Highest minimum macOS version (LC_BUILD_VERSION minos / LC_VERSION_MIN_MACOSX) of the files.
    macos-bundle-dylibs.py check DIR
        Fails if a Mach-O in DIR (recursively) references a library outside /usr/lib, /System and
        @loader_path/@executable_path/@rpath (i.e. an absolute path of the build machine).

Needs the Xcode command line tools (otool, install_name_tool). Only standard library.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys

SYSTEM_PREFIXES = ("/usr/lib/", "/System/")


def run(*args):
    r = subprocess.run(args, capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit("ERROR: %s failed (%d): %s" % (" ".join(args), r.returncode, r.stderr.strip()))
    return r.stdout


def is_macho(path):
    try:
        with open(path, "rb") as f:
            magic = f.read(4)
    except OSError:
        return False
    return magic in (
        b"\xcf\xfa\xed\xfe", b"\xce\xfa\xed\xfe",  # thin 64/32, little endian
        b"\xfe\xed\xfa\xcf", b"\xfe\xed\xfa\xce",
        b"\xca\xfe\xba\xbe", b"\xbe\xba\xfe\xca",  # universal (also Java class files – rare here)
    )


def install_id(path):
    out = run("otool", "-D", path).splitlines()
    return out[1].strip() if len(out) > 1 else None


def deps(path):
    """Referenced libraries (without the file's own install name)."""
    own = install_id(path)
    out = []
    for line in run("otool", "-L", path).splitlines()[1:]:
        line = line.strip()
        if not line or line.endswith(":"):  # "(architecture x):" header of universal files
            continue
        name = line.split(" (compatibility", 1)[0].strip()
        if name and name != own and name not in out:
            out.append(name)
    return out


def load_commands(path):
    """Load commands as dicts of their key/value lines (otool -l)."""
    cmds, cur = [], None
    for line in run("otool", "-l", path).splitlines():
        line = line.strip()
        if line.startswith("Load command"):
            cur = {}
            cmds.append(cur)
            continue
        if cur is None or not line:
            continue
        k, _, v = line.partition(" ")
        cur.setdefault(k, v.strip())
    return cmds


def rpaths(path):
    out = []
    for c in load_commands(path):
        if c.get("cmd") == "LC_RPATH":
            out.append(re.sub(r"\s*\(offset \d+\)$", "", c.get("path", "")))
    return out


def resolve(ref, referrer, exe_dir, extra_rpaths):
    """Path on the build machine for the reference `ref` of the file `referrer`."""
    loader_dirs = [os.path.dirname(referrer), os.path.dirname(os.path.realpath(referrer))]

    def subst(p):
        if p.startswith("@loader_path/"):
            out = [os.path.join(d, p[len("@loader_path/"):]) for d in loader_dirs]
        elif p.startswith("@executable_path/"):
            out = [os.path.join(exe_dir, p[len("@executable_path/"):])]
        else:
            out = [p]
        return out

    if ref.startswith("@rpath/"):
        rest = ref[len("@rpath/"):]
        for rp in rpaths(referrer) + extra_rpaths:
            for d in subst(rp):
                cand = os.path.join(d, rest)
                if os.path.isfile(cand):
                    return os.path.normpath(cand)
        return None
    for cand in subst(ref):
        if os.path.isfile(cand):
            return os.path.normpath(cand)
    return None


def cellar(path):
    """(formula, version) of a Homebrew keg path, else None."""
    m = re.search(r"/Cellar/([^/]+)/([^/]+)/", os.path.realpath(path))
    return (m.group(1), m.group(2)) if m else None


def bundle(dest, entries, manifest):
    os.makedirs(dest, exist_ok=True)
    exe_dir = os.path.dirname(os.path.realpath(entries[0]))
    exe_rpaths = rpaths(entries[0])
    # name in dest -> source path (real path); queue of (source as referenced, name)
    placed = {}
    queue = [(e, os.path.basename(e)) for e in entries]
    changes = {}  # name -> [(old reference, new reference)]
    while queue:
        src, name = queue.pop(0)
        real = os.path.realpath(src)
        if name in placed:
            if placed[name] != real:
                sys.exit("ERROR: two different files want the name %s: %s and %s" % (name, placed[name], real))
            continue
        if not is_macho(real):
            sys.exit("ERROR: %s is not a Mach-O file" % real)
        placed[name] = real
        target = os.path.join(dest, name)
        shutil.copyfile(real, target)
        os.chmod(target, 0o755)
        changes[name] = []
        for ref in deps(real):
            if ref.startswith(SYSTEM_PREFIXES):
                continue
            found = resolve(ref, src, exe_dir, exe_rpaths)
            if not found:
                sys.exit("ERROR: %s references %s, which cannot be found on this machine" % (real, ref))
            dep_name = os.path.basename(ref)
            changes[name].append((ref, "@loader_path/" + dep_name))
            queue.append((found, dep_name))
        print("    %-40s <- %s" % (name, real), file=sys.stderr)

    for name, ch in changes.items():
        target = os.path.join(dest, name)
        args = ["install_name_tool"]
        if install_id(target):
            args += ["-id", "@loader_path/" + name]
        for old, new in ch:
            args += ["-change", old, new]
        # rpaths of the build machine are no longer needed (all references are @loader_path now)
        for rp in rpaths(target):
            args += ["-delete_rpath", rp]
        if len(args) > 1:
            args.append(target)
            run(*args)

    problems = check(dest)
    if problems:
        sys.exit("ERROR: references outside the bundle remain:\n  " + "\n  ".join(problems))

    if manifest:
        files = []
        for name in sorted(placed):
            c = cellar(placed[name])
            files.append({"file": name, "source": placed[name], "formula": c[0] if c else None, "version": c[1] if c else None})
        with open(manifest, "w", encoding="utf-8") as f:
            json.dump({"files": files}, f, indent=2)
            f.write("\n")
    print("    %d files in %s" % (len(placed), dest), file=sys.stderr)


def check(root):
    problems = []
    for d, _, fs in os.walk(root):
        for fn in fs:
            p = os.path.join(d, fn)
            if os.path.islink(p) or not is_macho(p):
                continue
            for ref in deps(p):
                if ref.startswith(SYSTEM_PREFIXES):
                    continue
                if ref.startswith("@loader_path/"):
                    # must exist relative to the file
                    if not os.path.exists(os.path.join(d, ref[len("@loader_path/"):])):
                        problems.append("%s: %s (missing)" % (p, ref))
                    continue
                if ref.startswith(("@executable_path/", "@rpath/", "@loader_path/")):
                    continue
                problems.append("%s: %s" % (p, ref))
    return problems


def version_tuple(v):
    return tuple(int(x) for x in re.findall(r"\d+", v)[:3]) or (0,)


def minos(files):
    best = (0,)
    for f in files:
        if os.path.islink(f) or not is_macho(f):
            continue
        for c in load_commands(f):
            v = None
            if c.get("cmd") == "LC_BUILD_VERSION":
                v = c.get("minos")
            elif c.get("cmd") == "LC_VERSION_MIN_MACOSX":
                v = c.get("version")
            if v:
                t = version_tuple(v)
                if t > best:
                    best = t
    return ".".join(str(x) for x in best) if best != (0,) else ""


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("bundle")
    b.add_argument("--dest", required=True)
    b.add_argument("--manifest")
    b.add_argument("entries", nargs="+")
    m = sub.add_parser("minos")
    m.add_argument("files", nargs="+")
    c = sub.add_parser("check")
    c.add_argument("dir")
    a = ap.parse_args()
    if a.cmd == "bundle":
        bundle(os.path.abspath(a.dest), a.entries, a.manifest)
    elif a.cmd == "minos":
        print(minos(a.files))
    elif a.cmd == "check":
        problems = check(os.path.abspath(a.dir))
        if problems:
            print("\n".join(problems))
            sys.exit(1)


if __name__ == "__main__":
    main()
