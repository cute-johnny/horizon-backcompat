#!/usr/bin/env python3
"""Silent-on-pass, surgical-on-fail verification wrapper.

Usage:
  python .hermes/bin/verify.py                      # fast default: cargo check + javac (plugin, mock server)
  python .hermes/bin/verify.py -p horizon-backcompat-engine           # + tests of one cargo package
  python .hermes/bin/verify.py -p horizon-backcompat-engine chain::   # + filtered tests
  python .hermes/bin/verify.py --full               # whole Rust test suite
  python .hermes/bin/verify.py --clippy             # clippy -D warnings instead of cargo check
  python .hermes/bin/verify.py --no-java | --no-rust
Contract: exit 0 -> one line "✅ VERIFY PASS (...)"; exit != 0 -> <= 40 lines of failures (file:line + message).
Covers: the standalone Rust workspace (mixins/backcompat/engine) and the Java sources. The loose mixin
files (mixins/backcompat/*.rs) only compile when staged into Horizon, so they are NOT checked here.
Env: VERIFY_CARGO_TOOLCHAIN (default stable-x86_64-pc-windows-gnu on Windows; empty = rustup default),
     VIAPROXY_JAR (default ../research/upstream/viaproxy-jar/*.jar; plugin check is skipped if absent),
     JAVA_HOME (else javac from PATH).
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Callable

ROOT = Path(__file__).resolve().parents[2]
MAX_LINES = 40
TIMEOUT = int(os.environ.get("VERIFY_TIMEOUT", "1800"))

_reconf = getattr(sys.stdout, "reconfigure", None)  # Windows consoles default to cp1252
if _reconf:
    _reconf(encoding="utf-8", errors="replace")


def run(cmd: list[str]) -> tuple[int, str, float]:
    t = time.time()
    try:
        p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, encoding="utf-8",
                           errors="replace", timeout=TIMEOUT)
        return p.returncode, (p.stdout or "") + "\n" + (p.stderr or ""), time.time() - t
    except FileNotFoundError:
        return 127, f"{cmd[0]}: not installed", time.time() - t
    except subprocess.TimeoutExpired:
        return 124, f"{' '.join(cmd)}: timed out after {TIMEOUT}s", time.time() - t


# ---------------------------------------------------------------- parsers (failure -> compact lines)

def parse_cargo_json(out: str) -> list[str]:
    """Compiler diagnostics from --message-format=json: 'file:line:col: error[E0308]: msg'."""
    res, seen = [], set()
    for line in out.splitlines():
        if not line.startswith("{"):
            continue
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if msg.get("reason") != "compiler-message":
            continue
        d = msg.get("message") or {}
        if d.get("level") not in ("error", "warning"):
            continue
        if d.get("level") == "warning" and not d.get("_deny"):
            # warnings only matter for clippy -D warnings, where they arrive as level=error
            continue
        span = next((s for s in d.get("spans", []) if s.get("is_primary")), None)
        loc = f"{span['file_name']}:{span['line_start']}:{span['column_start']}" if span else "?"
        code = (d.get("code") or {}).get("code")
        text = f"{loc}: {d['level']}{f'[{code}]' if code else ''}: {d.get('message', '').strip()}"
        if span and span.get("label"):
            text += f" ({span['label']})"
        if text not in seen:
            seen.add(text)
            res.append(text)
    return res


def parse_cargo_test(out: str) -> list[str]:
    res = []
    blocks = re.split(r"^---- (.+?) stdout ----$", out, flags=re.M)
    # blocks: [pre, name1, body1, name2, body2, ...]
    for name, body in zip(blocks[1::2], blocks[2::2]):
        res.append(f"FAIL {name}")
        keep = []
        for ln in body.splitlines():
            s = ln.rstrip()
            if not s or s.startswith(("note: run with", "stack backtrace", "failures:")):
                continue
            if re.match(r"^\s*\d+: ", s) or s.strip().startswith("at "):
                continue  # backtrace frames
            keep.append("  " + s.strip())
            if len(keep) >= 6:
                break
        res.extend(keep)
    if not res:  # e.g. compile error inside tests, or a crash without the standard block
        res = parse_generic(out)
    return res


def parse_pytest(out: str) -> list[str]:
    res = [ln for ln in out.splitlines() if re.match(r"^(FAILED|ERROR) ", ln)]
    res += [ln.strip() for ln in out.splitlines() if re.match(r"^\S+\.py:\d+: ", ln.strip())]
    res += [ln.rstrip() for ln in out.splitlines() if ln.startswith("E  ")][:15]
    return res or parse_generic(out)


def parse_generic(out: str) -> list[str]:
    pat = re.compile(r"(error|Error|ERROR|FAIL|failed|panicked|✗|×|\b\S+:\d+:\d+)")
    lines = [ln.rstrip() for ln in out.splitlines() if ln.strip() and pat.search(ln)]
    lines = [ln for ln in lines if not re.search(r"\b0 (failed|errors?)\b", ln)]
    return lines or [ln.rstrip() for ln in out.strip().splitlines()[-15:]]


# ---------------------------------------------------------------- checks per ecosystem

def cargo() -> list[str]:
    tc = os.environ.get("VERIFY_CARGO_TOOLCHAIN", "stable-x86_64-pc-windows-gnu" if os.name == "nt" else "")
    return ["cargo", f"+{tc}"] if tc else ["cargo"]


def cargo_checks(a) -> list[tuple[str, list[str], Callable, Callable]]:
    """(name, cmd, failure_parser, pass_summary)."""
    checks = []
    scope = ["-p", a.package] if a.package else ["--workspace"]
    if a.clippy:
        checks.append(("clippy", [*cargo(), "clippy", *scope, "--all-targets", "--message-format=json", "--", "-D", "warnings"],
                       parse_cargo_json, lambda o: "ok"))
    else:
        checks.append(("cargo check", [*cargo(), "check", *scope, "--all-targets", "--message-format=json"],
                       parse_cargo_json, lambda o: "ok"))
    if a.package or a.full or a.filter:
        cmd = [*cargo(), "test", *scope, "--quiet"]
        if a.filter:
            cmd += ["--", a.filter]
        checks.append(("cargo test", cmd, parse_cargo_test, summarize_cargo_tests))
    return checks


def summarize_cargo_tests(out: str) -> str:
    passed = sum(int(m) for m in re.findall(r"test result: ok\. (\d+) passed", out))
    return f"{passed} passed"


def python_checks(a):
    checks = []
    if shutil.which("ruff"):
        checks.append(("ruff", ["ruff", "check", "--quiet", "."], parse_generic, lambda o: "ok"))
    cmd = [sys.executable, "-m", "pytest", "-q", "-x", "--no-header", "-p", "no:cacheprovider"]
    if a.filter:
        cmd += ["-k", a.filter] if not os.path.exists(ROOT / a.filter) else [a.filter]
    checks.append(("pytest", cmd, parse_pytest,
                   lambda o: (re.search(r"\d+ passed[^\n]*", o) or [None])[0] or "ok"))
    return checks


def node_checks(a):
    checks = []
    pkg = json.loads((ROOT / "package.json").read_text(encoding="utf-8"))
    scripts = pkg.get("scripts", {})
    npx = shutil.which("npx") or "npx"
    if (ROOT / "tsconfig.json").exists():
        checks.append(("tsc", [npx, "tsc", "--noEmit", "--pretty", "false"], parse_generic, lambda o: "ok"))
    if "test" in scripts and (a.full or a.filter):
        npm = shutil.which("npm") or "npm"
        cmd = [npm, "test", "--silent"] + (["--", a.filter] if a.filter else [])
        checks.append(("npm test", cmd, parse_generic, lambda o: "ok"))
    return checks


def javac_tool() -> str:
    exe = "javac" + (".exe" if os.name == "nt" else "")
    home = os.environ.get("JAVA_HOME")
    if home and (Path(home) / "bin" / exe).is_file():
        return str(Path(home) / "bin" / exe)
    return shutil.which("javac") or "javac"


def parse_javac(out: str) -> list[str]:
    return [ln.rstrip() for ln in out.splitlines() if re.search(r"\.java:\d+: (error|warning)", ln)] or parse_generic(out)


def viaproxy_jar() -> Path | None:
    env = os.environ.get("VIAPROXY_JAR")
    if env:
        return Path(env) if Path(env).is_file() else None
    return next(iter(sorted((ROOT.parent / "research" / "upstream" / "viaproxy-jar").glob("ViaProxy*.jar"))), None)


def java_checks(a):
    """javac into a scratch dir (nothing is written to the repo). Flags mirror viaproxy-plugin/build.py."""
    checks = []
    scratch = Path(os.environ.get("TMPDIR") or os.environ.get("TEMP") or ".") / "verify-javac"
    flags = [javac_tool(), "--release", "17", "-Xlint:all", "-Werror", "-proc:none"]
    mock = ROOT / "e2e" / "MockLegacyServer.java"
    if mock.exists():
        checks.append(("javac e2e", [*flags, "-d", str(scratch / "e2e"), str(mock)], parse_javac, lambda o: "ok"))
    plugin_src = sorted((ROOT / "viaproxy-plugin" / "src" / "main" / "java").rglob("*.java"))
    if plugin_src:
        jar = viaproxy_jar()
        if jar:
            checks.append(("javac plugin", [*flags, "-cp", str(jar), "-d", str(scratch / "plugin"), *map(str, plugin_src)],
                           parse_javac, lambda o: "ok"))
        else:
            SKIPPED.append("javac plugin skipped (no ViaProxy jar; set VIAPROXY_JAR)")
    return checks


SKIPPED: list[str] = []


def detect(a):
    checks = []
    if (ROOT / "Cargo.toml").exists() and not a.no_rust:
        checks += cargo_checks(a)
    if not a.no_java:
        checks += java_checks(a)
    if any((ROOT / f).exists() for f in ("pyproject.toml", "setup.cfg", "pytest.ini", "tox.ini")):
        checks += python_checks(a)
    if (ROOT / "package.json").exists():
        checks += node_checks(a)
    return checks


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("filter", nargs="?", help="test name filter (cargo test -- X / pytest -k X) or test path")
    ap.add_argument("-p", "--package", help="cargo package to check+test (e.g. horizon-backcompat-engine)")
    ap.add_argument("--full", action="store_true", help="run the whole test suite")
    ap.add_argument("--clippy", action="store_true", help="Rust: clippy -D warnings instead of cargo check")
    ap.add_argument("--no-java", action="store_true", help="skip javac checks")
    ap.add_argument("--no-rust", action="store_true", help="skip cargo checks")
    a = ap.parse_args()

    checks = detect(a)
    if not checks:
        print("❌ VERIFY: no known project type (Cargo.toml / pyproject.toml / package.json) at " + str(ROOT))
        return 2

    summary = []
    for name, cmd, parse, ok_summary in checks:
        code, out, secs = run(cmd)
        if code == 0:
            summary.append(f"{name}: {ok_summary(out)} in {secs:.1f}s")
            continue
        lines = parse(out) or [f"exit code {code}"]
        print(f"❌ VERIFY FAIL [{name}] exit={code} ({secs:.1f}s) — {len(lines)} issue line(s)")
        cmd_s = " ".join(Path(c).name if i == 0 else c for i, c in enumerate(cmd))
        print(f"   cmd: {cmd_s}")
        body = lines[: MAX_LINES - 3]
        print("\n".join(body))
        if len(lines) > len(body):
            print(f"... {len(lines) - len(body)} more line(s) truncated")
        if summary:
            print("passed before failure: " + "; ".join(summary))
        return 1
    print(f"✅ VERIFY PASS ({'; '.join(summary + SKIPPED)})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
