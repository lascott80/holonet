"""Packages a release build of Holonet as a zip (milestone 7 phase 9).

    python scripts/package.py --target aarch64-pc-windows-msvc --arch arm64

Reads target/<target>/release/holonet.exe (or target/release/holonet.exe
with --exe), and writes dist/holonet-<version>-windows-<arch>.zip holding:

    holonet.exe
    README.txt
    LICENSE.txt                        Holonet's own licence (MIT)
    licenses/fonts/...                 the bundled fonts' licences and notices
    licenses/THIRD-PARTY-LICENSES.txt  every Rust crate built into the exe

The crate list comes from `cargo metadata` for that target, and each
crate's licence text from its own LICENSE, COPYING or NOTICE files.
Standard library only.
"""

import argparse
import json
import os
import re
import subprocess
import sys
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
APP = "holonet-app"
FONTS = os.path.join(ROOT, "crates", APP, "assets", "fonts")
LICENCE_FILE = re.compile(r"^(LICEN[CS]E|COPYING|NOTICE|UNLICENSE)", re.I)


def metadata(target):
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout
    return json.loads(out)


def built_into_app(meta):
    """The packages holonet.exe links: normal dependencies, followed from
    holonet-app (build and dev dependencies are left out)."""
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    start = next(p["id"] for p in meta["packages"] if p["name"] == APP)
    seen, todo = set(), [start]
    while todo:
        pid = todo.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid]["deps"]:
            if any(k["kind"] is None for k in dep["dep_kinds"]):
                todo.append(dep["pkg"])
    workspace = set(meta["workspace_members"])
    return sorted((packages[p] for p in seen if p not in workspace), key=lambda p: (p["name"], p["version"]))


def third_party(meta):
    parts = [
        "Third-party software in Holonet\n",
        "================================\n\n",
        "holonet.exe is built from the Rust crates below. Each is listed with its\n",
        "licence as its author declared it, followed by the licence files the\n",
        "crate ships. Where a crate offers a choice of licences, Holonet uses it\n",
        "under any one of them.\n\n",
    ]
    crates = built_into_app(meta)
    for p in crates:
        parts.append(f"  {p['name']} {p['version']}  ({p.get('license') or 'see its licence file'})\n")
    for p in crates:
        parts.append("\n\n" + "=" * 78 + "\n")
        parts.append(f"{p['name']} {p['version']}\n")
        parts.append(f"Licence: {p.get('license') or 'see below'}\n")
        if p.get("repository"):
            parts.append(f"Source: {p['repository']}\n")
        parts.append("=" * 78 + "\n")
        folder = os.path.dirname(p["manifest_path"])
        files = sorted(f for f in os.listdir(folder) if LICENCE_FILE.match(f) and os.path.isfile(os.path.join(folder, f)))
        if p.get("license_file"):
            extra = os.path.relpath(os.path.join(folder, p["license_file"]), folder)
            if extra not in files and os.path.isfile(os.path.join(folder, extra)):
                files.append(extra)
        if not files:
            parts.append("\nThe crate ships no licence file; its declared licence is above.\n")
        for f in files:
            with open(os.path.join(folder, f), encoding="utf-8", errors="replace") as fh:
                parts.append(f"\n--- {f} ---\n\n{fh.read().strip()}\n")
    return "".join(parts), len(crates)


def font_files():
    """(path in the zip, file on disk) for every font licence and notice."""
    out = [("licenses/fonts/README.md", os.path.join(FONTS, "README.md"))]
    for name in sorted(os.listdir(FONTS)):
        if name.endswith("-OFL.txt"):
            out.append((f"licenses/fonts/{name}", os.path.join(FONTS, name)))
    nerd = os.path.join(FONTS, "jetbrainsmono-nerd")
    for name in ("OFL.txt", "NOTICE.md"):
        out.append((f"licenses/fonts/jetbrainsmono-nerd/{name}", os.path.join(nerd, name)))
    return out


def readme(version, arch):
    return f"""Holonet {version} for Windows ({arch})
{'=' * (len(version) + len(arch) + 24)}

Holonet is a personal SSH and SFTP client for Windows, written in Rust.

Running it
----------
Extract this folder anywhere and run holonet.exe. There is no installer;
to remove Holonet, delete the folder.

The build is not signed, so Windows may show "Windows protected your PC"
the first time. Choose "More info", then "Run anyway". Holonet never
installs updates itself: Settings shows a notice when a newer release is
published at https://github.com/lascott80/holonet/releases.

Your data
---------
Hosts, keys, snippets and settings are kept in one encrypted vault file:

    %APPDATA%\\Holonet\\vault.holonet

Session logs, when you turn logging on for a host, are encrypted files in
vault.holonet.logs beside it. Settings has "Export encrypted file" to keep
a backup of the vault. Holonet cannot open the vault without its password.

Licences
--------
Holonet is under the MIT licence (LICENSE.txt). The bundled fonts and the
Rust crates built into holonet.exe keep their own licences, in licenses\\.
""".replace("\n", "\r\n")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--target", required=True, help="Rust target triple, for the crate list")
    ap.add_argument("--arch", required=True, help="arm64 or x64, for the zip name")
    ap.add_argument("--exe", help="holonet.exe to package (default target/<target>/release)")
    ap.add_argument("--out", default=os.path.join(ROOT, "dist"))
    args = ap.parse_args()

    meta = metadata(args.target)
    version = next(p["version"] for p in meta["packages"] if p["name"] == APP)
    exe = args.exe or os.path.join(ROOT, "target", args.target, "release", "holonet.exe")
    if not os.path.isfile(exe):
        sys.exit(f"no {exe}: build it first")

    name = f"holonet-{version}-windows-{args.arch}"
    os.makedirs(args.out, exist_ok=True)
    zip_path = os.path.join(args.out, f"{name}.zip")
    licences, count = third_party(meta)
    with zipfile.ZipFile(zip_path, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        z.write(exe, f"{name}/holonet.exe")
        z.writestr(f"{name}/README.txt", readme(version, args.arch))
        z.write(os.path.join(ROOT, "LICENSE"), f"{name}/LICENSE.txt")
        for inside, path in font_files():
            z.write(path, f"{name}/{inside}")
        z.writestr(f"{name}/licenses/THIRD-PARTY-LICENSES.txt", licences)
    print(f"wrote {zip_path} ({os.path.getsize(zip_path) // 1024} KB, {count} crates listed)")


if __name__ == "__main__":
    main()
