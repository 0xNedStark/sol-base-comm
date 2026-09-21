#!/usr/bin/env python3
"""Make solana/Cargo.lock buildable by the SBF toolchain.

The host cargo resolves dependencies; platform-tools' older cargo (rustc 1.75)
builds from the result. Left alone, the host locks crates that toolchain cannot
read (v4 lockfiles, edition-2024 manifests, rust-version > 1.75). This script
runs `cargo build-sbf` in a loop and, each time it names a crate it cannot
handle, walks up the dependency tree to find something it can downgrade.

Run after adding or updating any dependency:

    python3 scripts/pin-lockfile.py
"""
import json
import re
import subprocess
import sys
import urllib.request

MSRV = (1, 75)
PROGRAMS = ("programs/base-caller/Cargo.toml", "programs/mock-transport/Cargo.toml")
ROOTS = {"base-caller", "mock-transport", "anchor-lang", "solana-program", "wormhole-anchor-sdk"}

# Ceilings the resolver cannot infer, because the crate declares no
# rust-version that would rule the newer release out.
#
#   proc-macro2  anchor-syn 0.30.1's IDL build calls Span::source_file(),
#                which 1.0.95 removed. Without this the IDL step fails with
#                an unrelated-looking proc-macro panic in ark-bn254.
MAX_VERSION = {"proc-macro2": "1.0.94"}


def run(cmd):
    p = subprocess.run(cmd, shell=True, capture_output=True, text=True)
    return p.returncode, p.stdout + p.stderr


def base(v):
    return v.split("+", 1)[0]


def tup(v):
    return tuple(int(x) for x in base(v).split(".")[:3])


def index_path(n):
    n = n.lower()
    if len(n) == 1:
        return f"1/{n}"
    if len(n) == 2:
        return f"2/{n}"
    if len(n) == 3:
        return f"3/{n[0]}/{n}"
    return f"{n[:2]}/{n[2:4]}/{n}"


_cache = {}


def versions(name):
    """Every non-yanked release, flagged with whether its MSRV fits."""
    if name in _cache:
        return _cache[name]
    url = f"https://index.crates.io/{index_path(name)}"
    with urllib.request.urlopen(url, timeout=30) as r:
        lines = r.read().decode().splitlines()
    out = []
    for line in lines:
        e = json.loads(line)
        if e.get("yanked"):
            continue
        rv, ok = e.get("rust_version"), True
        if rv:
            try:
                ok = tuple(int(x) for x in rv.split(".")[:2]) <= MSRV
            except ValueError:
                ok = True
        try:
            out.append((tup(e["vers"]), e["vers"], ok))
        except ValueError:
            pass
    _cache[name] = out
    return out


def candidates(cur, cands):
    """Older releases in the same semver-compatible range, MSRV-clean."""
    c, res = tup(cur), []
    for v, s, ok in cands:
        if v >= c or not ok:
            continue
        if c[0] == 0:
            if v[0] == 0 and v[1] == c[1]:
                res.append((v, s))
        elif v[0] == c[0]:
            res.append((v, s))
    return [s for _, s in sorted(res, reverse=True)]


def parents(name, ver):
    _, out = run(f"cargo tree -i {name}@{base(ver)} -e normal --prefix depth 2>&1")
    res = []
    for line in out.splitlines():
        m = re.match(r"^1([A-Za-z0-9_-]+) v(\S+)", line)
        if m and not m.group(2).endswith(")"):
            res.append((m.group(1), m.group(2)))
    return res


def update(name, ver, cand):
    for spec in (f"{name}@{base(ver)}", f"{name}@{ver}", name):
        if run(f"cargo update -p {spec} --precise {cand} 2>&1")[0] == 0:
            return True
    return False


seen = set()


def downgrade(name, ver, depth=0):
    """Downgrade this crate, or the nearest ancestor that moves it."""
    if (name, ver) in seen:
        return None
    seen.add((name, ver))
    for cand in candidates(ver, versions(name)):
        if update(name, ver, cand):
            return f"{name} {ver} -> {cand}"
    if depth >= 4:
        return None
    for pn, pv in parents(name, ver):
        if pn in ROOTS:
            continue
        got = downgrade(pn, pv, depth + 1)
        if got:
            return got
    return None


VER = r"(\d+\.\d+\.\d+(?:\+[A-Za-z0-9.\-]+)?)"
PATTERNS = [
    r"failed to parse manifest at `[^`]*/registry/src/[^/]+/([A-Za-z0-9_-]+)-" + VER + r"/Cargo\.toml`",
    r"failed to download `([A-Za-z0-9_-]+) v" + VER + "`",
    r"package `([A-Za-z0-9_-]+) v" + VER + r"` cannot be built because it requires rustc",
]


def apply_ceilings():
    """Hold crates at a known-good version before the build loop runs."""
    out = []
    for name, ceiling in MAX_VERSION.items():
        code, txt = run(f"cargo tree -i {name} -e normal --prefix depth 2>&1")
        cur = re.search(rf"^0?{re.escape(name)} v(\S+)", txt, re.M)
        if not cur or tup(cur.group(1)) <= tup(ceiling):
            continue
        if update(name, cur.group(1), ceiling):
            out.append(f"{name} {cur.group(1)} -> {ceiling} (ceiling)")
    return out


def main():
    run("sed -i 's/^version = 4$/version = 3/' Cargo.lock")
    pins = apply_ceilings()
    for p in pins:
        print(f"  pin: {p}")
    for target in PROGRAMS:
        for _ in range(80):
            code, out = run(f"cargo build-sbf --manifest-path {target} 2>&1")
            if code == 0:
                print(f"OK  {target}")
                break
            m = next((mm for p in PATTERNS for mm in [re.search(p, out)] if mm), None)
            if not m:
                print(f"unrecognised failure building {target}:\n{out[-2500:]}")
                return 1
            name, ver = m.group(1), m.group(2)
            seen.clear()
            got = downgrade(name, ver)
            if not got:
                print(f"cannot downgrade {name} {ver}\n{out[-1200:]}")
                return 1
            run("sed -i 's/^version = 4$/version = 3/' Cargo.lock")
            pins.append(got)
            print(f"  pin: {got}")
        else:
            print(f"gave up on {target}")
            return 1
    run("sed -i 's/^version = 4$/version = 3/' Cargo.lock")
    print(f"\n{len(pins)} pin(s) applied" if pins else "\nno pins needed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
