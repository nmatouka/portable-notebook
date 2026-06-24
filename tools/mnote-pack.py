#!/usr/bin/env python3
"""mnote-pack — bundle a marimo notebook + its exotic wheels into a .mnote zip.

A .mnote is either a bare marimo .py, OR (this tool's output) a zip containing:
    notebook.py        the marimo notebook source
    wheels/*.whl        pure-Python wheels the notebook needs that aren't baked
    manifest.json       {"packages": {name: <pyodide-lock entry>}} for those wheels

The player merges manifest.json into the served pyodide-lock.json and serves the
wheels locally, so the notebook runs offline with no PyPI access (spec §5 tier 2).
The dependency closure is resolved HERE (authoring time), keeping the player simple.

v1 scope: pure-Python wheels (py3-none-any). C-extension packages need
Pyodide-built wheels and are out of scope for tier-2 bundling.

Usage: mnote-pack.py <notebook.(py|mnote)> <out.mnote> [--baked-dir app/frontend]
"""
import sys, os, re, json, zipfile, hashlib, tempfile, subprocess, argparse, glob
from email.parser import Parser

WHEEL_RE = re.compile(r"^(?P<name>.+?)-(?P<ver>\d[^-]*)-.*\.whl$")


def norm(n):  # PEP 503 normalization
    return re.sub(r"[-_.]+", "-", n).lower()


def pep723_deps(src):
    m = re.search(r"# /// script(.*?)# ///", src, re.DOTALL)
    if not m:
        return []
    body = "".join(l[2:] if l.startswith("# ") else l[1:] for l in m.group(1).splitlines())
    deps = re.search(r"dependencies\s*=\s*\[(.*?)\]", body, re.DOTALL)
    if not deps:
        return []
    return [re.match(r"[A-Za-z0-9_.\-]+", d.strip().strip('"\'')).group(0)
            for d in deps.group(1).split(",") if d.strip().strip('"\'')]


def wheel_metadata(whl):
    """Return (name, version, requires_dist_names, top_level_imports)."""
    with zipfile.ZipFile(whl) as z:
        meta_name = next(n for n in z.namelist() if n.endswith(".dist-info/METADATA"))
        meta = Parser().parsestr(z.read(meta_name).decode("utf-8", "replace"))
        reqs = []
        for r in meta.get_all("Requires-Dist") or []:
            if ";" in r:  # skip optional/extra/marker deps for v1
                continue
            nm = re.match(r"[A-Za-z0-9_.\-]+", r.strip())
            if nm:
                reqs.append(norm(nm.group(0)))
        tops = []
        tl = meta_name.rsplit("/", 1)[0] + "/top_level.txt"
        if tl in z.namelist():
            tops = [l.strip() for l in z.read(tl).decode().splitlines() if l.strip()]
    return meta.get("Name"), meta.get("Version"), reqs, tops


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("notebook")
    ap.add_argument("output")
    ap.add_argument("--baked-dir", default="app/frontend",
                    help="frontend whose vendored wheels + lock count as already-present")
    a = ap.parse_args()

    src = open(a.notebook, encoding="utf-8").read()
    deps = pep723_deps(src)

    # What's already available in the player (don't re-bundle these). A package
    # counts as baked only if its resolved wheel is physically vendored — the lock
    # lists ~373 names but the player ships only the handful of wheels it vendored.
    vendored = {os.path.basename(w)
                for w in glob.glob(os.path.join(a.baked_dir, "**", "*.whl"), recursive=True)}
    lock_path = os.path.join(a.baked_dir, "_vendor", "wasm.marimo.app", "pyodide-lock.json")
    lock_pkgs = json.load(open(lock_path))["packages"] if os.path.exists(lock_path) else {}
    baked = set()
    for nm, e in lock_pkgs.items():  # e.g. lock 'marimo' -> vendored marimo_base wheel
        if os.path.basename(e.get("file_name", "").split("?")[0]) in vendored:
            baked.add(norm(nm))
    for w in vendored:  # plus any vendored wheel not named in the lock
        if (m := WHEEL_RE.match(w)):
            baked.add(norm(m.group("name")))
    print(f"declared deps: {deps}")
    print(f"vendored wheels: {len(vendored)}; baked (available) names: {len(baked)}")

    needed = [d for d in deps if norm(d) not in baked]
    if not needed:
        print("nothing to bundle; all deps are baked. (Output a bare .py instead.)")
    tmp = tempfile.mkdtemp()
    if needed:
        print(f"pip download (pure-python closure) for: {needed}")
        subprocess.run([sys.executable, "-m", "pip", "download", "--only-binary=:all:",
                        "--dest", tmp, *needed], check=True)

    manifest = {"packages": {}}
    bundled = []
    for whl in glob.glob(os.path.join(tmp, "*.whl")):
        base = os.path.basename(whl)
        if not base.endswith("-py3-none-any.whl"):
            print(f"  skip (not pure-Python): {base}")
            continue
        name, ver, reqs, tops = wheel_metadata(whl)
        if norm(name) in baked:
            print(f"  skip (already baked): {base}")
            continue
        sha = hashlib.sha256(open(whl, "rb").read()).hexdigest()
        manifest["packages"][norm(name)] = {
            "name": norm(name), "version": ver, "file_name": base,
            "install_dir": "site", "package_type": "package",
            "sha256": sha, "unvendored_tests": False,
            "imports": tops or [norm(name).replace("-", "_")],
            # keep only deps that will resolve (bundled now or known to the lock)
            "depends": [r for r in reqs if r not in baked],
        }
        bundled.append(whl)
        print(f"  bundle: {base}  (depends={manifest['packages'][norm(name)]['depends']})")

    with zipfile.ZipFile(a.output, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("notebook.py", src)
        z.writestr("manifest.json", json.dumps(manifest, indent=2))
        for whl in bundled:
            z.write(whl, "wheels/" + os.path.basename(whl))
    print(f"wrote {a.output} with {len(bundled)} bundled wheel(s)")


if __name__ == "__main__":
    main()
