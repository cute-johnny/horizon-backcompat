#!/usr/bin/env python3
"""Recreate the sibling `../research/upstream/` checkouts this repo is verified against.

Usage:
  python .hermes/bin/setup-env.py            # check tools, clone/pin upstream repos, download the ViaProxy jar
  python .hermes/bin/setup-env.py --check    # only report what is present / missing (changes nothing)

Layout it creates (next to this repo, NOT inside it):
  ../research/upstream/horizon/                         pinned Horizon commit
  ../research/upstream/horizon-mixins/                  pinned horizon-mixins commit
  ../research/upstream/horizon-native-mod-template/     (reference only)
  ../research/upstream/viaproxy-jar/ViaProxy-<v>.jar    jar the Java plugin is compiled against
"""
from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
UP = ROOT.parent / "research" / "upstream"

# Keep in sync with NOTES.md "Pins and environment" and dist/pack.py.
REPOS = {
    "horizon": ("https://github.com/horizon-rust/horizon", "31f83499b6c2cff7a8e7bf45f8173a7a95c2767c"),
    "horizon-mixins": ("https://github.com/horizon-rust/horizon-mixins", "ddd9700b21997c95d95ac6ee5ecafe3830cf622a"),
    "horizon-native-mod-template": ("https://github.com/horizon-rust/horizon-native-mod-template", None),
}
VIAPROXY_VERSION = "3.4.14"
VIAPROXY_URL = f"https://github.com/ViaVersion/ViaProxy/releases/download/v{VIAPROXY_VERSION}/ViaProxy-{VIAPROXY_VERSION}.jar"


def run(*cmd: str, cwd: Path | None = None) -> None:
    subprocess.run(cmd, cwd=cwd, check=True)


def check_tools() -> None:
    for tool, hint in (("git", "install git"), ("cargo", "install rustup (https://rustup.rs)"),
                       ("javac", "install JDK 17+ and set JAVA_HOME"), ("python", "")):
        found = shutil.which(tool) or (tool == "python" and sys.executable)
        print(f"  [{'ok' if found else 'MISSING'}] {tool}" + ("" if found else f"  -> {hint}"))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true", help="report only, change nothing")
    args = ap.parse_args()

    print("Tools:")
    check_tools()
    print(f"Upstream dir: {UP}")

    if not args.check:
        UP.mkdir(parents=True, exist_ok=True)

    for name, (url, commit) in REPOS.items():
        dest = UP / name
        if dest.exists():
            print(f"  [ok] {name}")
            continue
        if args.check:
            print(f"  [MISSING] {name}")
            continue
        print(f"  cloning {name} ...")
        run("git", "clone", url, str(dest))
        if commit:
            run("git", "checkout", "--detach", commit, cwd=dest)

    jar_dir = UP / "viaproxy-jar"
    jar = jar_dir / f"ViaProxy-{VIAPROXY_VERSION}.jar"
    if jar.exists():
        print(f"  [ok] {jar.name}")
    elif args.check:
        print(f"  [MISSING] {jar.name}  (or set VIAPROXY_JAR)")
    else:
        jar_dir.mkdir(parents=True, exist_ok=True)
        print(f"  downloading {VIAPROXY_URL}")
        urllib.request.urlretrieve(VIAPROXY_URL, jar)

    if not args.check:
        print("Done. Next: python .hermes/bin/verify.py")
    return 0


if __name__ == "__main__":
    sys.exit(main())
