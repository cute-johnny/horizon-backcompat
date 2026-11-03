#!/usr/bin/env python3
"""Build HorizonAuthBridge.jar (the ViaProxy plugin) and copy it into the engine crate's data.

    python viaproxy-plugin/build.py --viaproxy path/to/ViaProxy-3.4.14.jar

Needs a JDK 17+ (`javac` and `jar` from JAVA_HOME, else the PATH). The ViaProxy jar is only the
compile classpath; it is not copied anywhere.
"""

import argparse
import os
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ENGINE_DATA = HERE.parent / "mixins" / "backcompat" / "engine" / "data"


def tool(name):
    exe = name + (".exe" if os.name == "nt" else "")
    home = os.environ.get("JAVA_HOME")
    if home and (Path(home) / "bin" / exe).is_file():
        return str(Path(home) / "bin" / exe)
    found = shutil.which(name)
    if not found:
        sys.exit(f"{name} not found: install a JDK 17+ or set JAVA_HOME")
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--viaproxy", type=Path, required=True, help="ViaProxy's jar, to compile against")
    args = parser.parse_args()
    build = HERE / "build"
    classes = build / "classes"
    shutil.rmtree(build, ignore_errors=True)
    classes.mkdir(parents=True)
    sources = sorted(str(p) for p in (HERE / "src" / "main" / "java").rglob("*.java"))
    subprocess.run(
        [tool("javac"), "--release", "17", "-Xlint:all", "-Werror", "-proc:none",
         "-cp", str(args.viaproxy), "-d", str(classes), *sources],
        check=True,
    )
    shutil.copy(HERE / "src" / "main" / "resources" / "viaproxy.yml", classes / "viaproxy.yml")
    jar = build / "HorizonAuthBridge.jar"
    # --date: the same sources make the same bytes, so the game rebuild sees no change.
    subprocess.run(
        [tool("jar"), "--create", "--file", str(jar), "--date", "2026-01-01T00:00:00Z", "-C", str(classes), "."],
        check=True,
    )
    ENGINE_DATA.mkdir(parents=True, exist_ok=True)
    shutil.copy(jar, ENGINE_DATA / "HorizonAuthBridge.jar")
    print(ENGINE_DATA / "HorizonAuthBridge.jar")


if __name__ == "__main__":
    main()
