#!/usr/bin/env python3
"""Zero-token repo map: every tracked source file + top-level symbols with line ranges.

Usage:
  python .hermes/bin/repo-map.py                 # (re)write .hermes/repo-map.md
  python .hermes/bin/repo-map.py --query Session # print only matching files/symbols (no write)
  python .hermes/bin/repo-map.py --query chain --path mixins/backcompat/engine
Output line format:
  mixins/backcompat/engine/src/chain.rs (550 lines)
    L12-45: pub struct Chain
      L20-30: pub fn new(...)   (methods of impl/trait/class are indented)
"""
from __future__ import annotations

import argparse
import ast
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / ".hermes" / "repo-map.md"
SRC_EXT = {".rs", ".java", ".py", ".toml", ".yml"}  # .toml/.yml are listed without symbols
SKIP_PARTS = {"target", "node_modules", "dist", "build", ".git", "vendor", ".hermes"}

RS_ITEM = re.compile(
    r"^(?P<vis>pub(?:\([^)]*\))?\s+)?(?:(?:async|const|unsafe|extern\s+\"[^\"]*\")\s+)*"
    r"(?P<kind>fn|struct|enum|trait|impl|mod|type|union|macro_rules!)\s*(?P<rest>.*)$"
)
RS_INNER = re.compile(
    r"^(?P<vis>pub(?:\([^)]*\))?\s+)?(?:(?:async|const|unsafe|extern\s+\"[^\"]*\")\s+)*(?P<kind>fn)\s*(?P<rest>.*)$"
)
JAVA_MODS = r"(?:(?:public|protected|private|abstract|final|static|sealed|non-sealed|strictfp)\s+)*"
JAVA_ITEM = re.compile(r"^" + JAVA_MODS + r"(?P<kind>class|interface|enum|record)\s+(?P<name>\w+)")
JAVA_INNER = re.compile(
    r"^(?!(?:return|new|else|throw|if|for|while|switch|catch|try|do)\b)"
    r"(?:(?:public|protected|private|static|final|abstract|synchronized|native|default)\s+)*"
    r"(?:<[^>]+>\s+)?(?:[\w<>\[\],.?]+\s+)?(?P<name>\w+)\s*\([^;=]*$|^(?!(?:return|new|else|throw)\b)"
    r"(?:(?:public|protected|private|static|final|abstract|synchronized|native|default)\s+)*"
    r"(?:<[^>]+>\s+)?(?:[\w<>\[\],.?]+\s+)?(?P<name2>\w+)\s*\(.*\)\s*(?:throws [\w., ]+)?\s*[{;]\s*$"
)
JS_ITEM = re.compile(
    r"^(?:export\s+(?:default\s+)?)?(?:async\s+)?(?P<kind>function\*?|class|interface|type|enum|const|let)\s+(?P<name>[A-Za-z0-9_$]+)"
)
GO_ITEM = re.compile(r"^(?P<kind>func|type)\s+(?P<rest>.*)$")


def tracked_files() -> list[Path]:
    try:
        out = subprocess.run(["git", "ls-files", "-z"], cwd=ROOT, capture_output=True, check=True).stdout
        names = [n for n in out.decode("utf-8", "replace").split("\0") if n]
    except (OSError, subprocess.CalledProcessError):
        names = [str(p.relative_to(ROOT)).replace("\\", "/") for p in ROOT.rglob("*") if p.is_file()]
    files = []
    for n in names:
        p = Path(n)
        if p.suffix in SRC_EXT and not (set(p.parts) & SKIP_PARTS):
            files.append(p)
    return sorted(files, key=lambda p: str(p).replace("\\", "/"))


def _strip_code(line: str, state: dict) -> str:
    """Remove comments/strings/char literals so brace counting is approximately right."""
    out, i, n = [], 0, len(line)
    while i < n:
        if state["block"]:
            j = line.find("*/", i)
            if j < 0:
                return "".join(out)
            state["block"], i = False, j + 2
            continue
        if state["raw"]:  # inside r#"..."# raw string
            j = line.find(state["raw"], i)
            if j < 0:
                return "".join(out)
            i, state["raw"] = j + len(state["raw"]), None
            continue
        if state["str"]:
            if line[i] == "\\":
                i += 2
                continue
            if line[i] == '"':
                state["str"] = False
            i += 1
            continue
        c = line[i]
        if line.startswith("//", i):
            break
        if line.startswith("/*", i):
            state["block"], i = True, i + 2
            continue
        m = re.match(r'b?r(#*)"', line[i:])
        if m and (i == 0 or not (line[i - 1].isalnum() or line[i - 1] == "_")):
            state["raw"], i = '"' + m.group(1), i + m.end()
            continue
        if c == '"':
            state["str"], i = True, i + 1
            continue
        if c == "'":  # char literal like '{' (lifetimes like 'a are left alone)
            m = re.match(r"'(\\.|[^\\'])'", line[i:])
            if m:
                i += m.end()
                continue
        out.append(c)
        i += 1
    return "".join(out)


def brace_symbols(lines: list[str], item_re, label, inner_re=None, inner_label=None) -> list[tuple[int, int, str]]:
    """Top-level (depth 0) items; end line = matching closing brace or terminating ';'.
    With inner_re, depth-1 items inside a top-level block (methods of impl/trait/class) are listed too."""
    syms, depth, state = [], 0, {"block": False, "str": False, "raw": None}
    pending = None  # [start_line, label, seen_open_brace]
    inner = None  # same shape, for a depth-1 item
    for idx, raw in enumerate(lines, 1):
        stripped = raw.strip()
        clean = not state["block"] and not state["str"] and not state["raw"]
        if depth == 0 and pending is None and clean:
            m = item_re.match(stripped)
            if m:
                pending = [idx, label(m), False]
        elif depth == 1 and pending and pending[2] and inner is None and inner_re and clean:
            m = inner_re.match(stripped)
            if m:
                inner = [idx, "  " + inner_label(m), False]
        code = _strip_code(raw, state)
        for ch in code:
            if ch == "{":
                depth += 1
                if inner and depth == 2:
                    inner[2] = True
                if pending and depth == 1:
                    pending[2] = True
            elif ch == "}":
                depth = max(0, depth - 1)
                if inner and inner[2] and depth == 1:
                    syms.append((inner[0], idx, inner[1]))
                    inner = None
                if pending and pending[2] and depth == 0:
                    syms.append((pending[0], idx, pending[1]))
                    pending = None
            elif ch == ";":
                if inner and not inner[2] and depth == 1:
                    syms.append((inner[0], idx, inner[1]))
                    inner = None
                elif pending and not pending[2] and depth == 0:
                    syms.append((pending[0], idx, pending[1]))
                    pending = None
    if pending:
        syms.append((pending[0], len(lines), pending[1]))
    syms.sort(key=lambda s: (s[0], -s[1]))
    return syms


def rs_label(m) -> str:
    rest = re.split(r"\s*(?:\{|where\b|;)", m.group("rest"), maxsplit=1)[0].strip()
    rest = re.sub(r"\s+", " ", rest)[:110]
    return f"{(m.group('vis') or '').strip()} {m.group('kind')} {rest}".strip()


def java_label(m) -> str:
    return f"{m.group('kind')} {m.group('name')}"


def java_inner_label(m) -> str:
    return f"{m.group('name') or m.group('name2')}()"


def rs_inner_label(m) -> str:
    return rs_label(m)


def go_label(m) -> str:
    return f"{m.group('kind')} {re.split(r'\s*\{', m.group('rest'))[0][:110]}"


def py_symbols(text: str) -> list[tuple[int, int, str]]:
    try:
        tree = ast.parse(text)
    except SyntaxError:
        return []
    out = []
    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            kind = "class" if isinstance(node, ast.ClassDef) else "def"
            out.append((node.lineno, getattr(node, "end_lineno", node.lineno), f"{kind} {node.name}"))
            if isinstance(node, ast.ClassDef):
                for sub in node.body:
                    if isinstance(sub, (ast.FunctionDef, ast.AsyncFunctionDef)):
                        out.append((sub.lineno, getattr(sub, "end_lineno", sub.lineno), f"  def {node.name}.{sub.name}"))
    return out


def symbols_for(path: Path) -> tuple[int, list[tuple[int, int, str]]]:
    try:
        text = (ROOT / path).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return 0, []
    lines = text.splitlines()
    ext = path.suffix
    if ext == ".rs":
        syms = brace_symbols(lines, RS_ITEM, rs_label, RS_INNER, rs_inner_label)
    elif ext == ".java":
        syms = brace_symbols(lines, JAVA_ITEM, java_label, JAVA_INNER, java_inner_label)
    elif ext == ".py":
        syms = py_symbols(text)
    else:
        syms = []
    return len(lines), syms


def render(entries) -> list[str]:
    out = []
    for path, n, syms in entries:
        out.append(f"{str(path).replace(chr(92), '/')} ({n} lines)")
        out.extend(f"  L{a}-{b}: {s}" for a, b, s in syms)
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--query", "-q", help="case-insensitive substring/regex; print matches only")
    ap.add_argument("--path", help="restrict to files under this path prefix")
    a = ap.parse_args()

    files = tracked_files()
    if a.path:
        pre = a.path.replace("\\", "/").rstrip("/")
        files = [f for f in files if str(f).replace("\\", "/").startswith(pre)]
    entries = [(f, *symbols_for(f)) for f in files]

    if a.query:
        rx = re.compile(a.query, re.I)
        hits = []
        for path, n, syms in entries:
            ms = [s for s in syms if rx.search(s[2])]
            if ms or rx.search(str(path)):
                hits.append((path, n, ms))
        lines = render(hits)
        cap = 120
        print("\n".join(lines[:cap]))
        if len(lines) > cap:
            print(f"... {len(lines) - cap} more lines; narrow with --path or a stricter --query")
        return 0 if hits else 1

    body = render(entries)
    nsym = sum(len(s) for _, _, s in entries)
    header = [
        "# Repo map (auto-generated by .hermes/bin/repo-map.py; do not edit)",
        f"# {len(entries)} files, {nsym} top-level symbols. Format: path (lines) / L<start>-<end>: symbol",
        "# Large file: grep it or use `repo-map.py --query X` instead of reading it whole.",
        "",
    ]
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text("\n".join(header + body) + "\n", encoding="utf-8")
    print(f"repo-map: {len(entries)} files, {nsym} symbols -> {OUT.relative_to(ROOT)} ({OUT.stat().st_size // 1024} KB)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
