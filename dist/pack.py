#!/usr/bin/env python3
"""Pack each bundle in mixins/<id>/ into a registry-style `.hnmod` (Horizon's native mod file).

Usage:
  python dist/pack.py            # write dist/out/<id>-<version>.hnmod and .hnmod.sha256 for every bundle
  python dist/pack.py --check    # validate only, write nothing

Rules follow Horizon's docs/api/mod-files.md and docs/native-mods.md ("What a .hnmod holds"):
  - top level: `horizon-native.json` + `mixins/<id>/` only;
  - `horizon-native.json`: schema 1, id, version, game, horizon, mixins (+ optional hooks), no other field;
    id/version/horizon must equal the bundle's horizon-mixin.toml;
  - under mixins/<id>/: `.rs` and `.toml` files, `data/**` (in the bundle or a top-level folder of it),
    README*/LICENSE*/LICENCE*/COPYING*/NOTICE*/.gitignore (in the bundle or a top-level folder);
  - at most 512 entries and 64 MiB unpacked.
The repo's LICENSE is added as mixins/<id>/LICENSE when the bundle has none. The archive is reproducible:
sorted entries, fixed timestamps, forward slashes.
"""
from __future__ import annotations

import argparse
import hashlib
import io
import json
import re
import sys
import tomllib
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "dist" / "out"

GAME = "26.3"
# horizon-mixins 0.6.0 (`ddd9700`), the version the bundles were built and tested with.
MIXINS_COMMIT = "ddd9700b21997c95d95ac6ee5ecafe3830cf622a"
# Bundles that use only Horizon's native hook points may name the hook API they need instead of
# floating on a pin (docs/native-mods.md, "Mods on the hook points"). None of ours do yet.
HOOKS: dict[str, int] = {}

MAX_ENTRIES = 512
MAX_BYTES = 64 * 1024 * 1024
FIXED_DATE = (1980, 1, 1, 0, 0, 0)
ID_RE = re.compile(r"[a-z0-9_]{1,64}")
COMMIT_RE = re.compile(r"[0-9a-f]{40}")
TEXT_RE = re.compile(r"(README|LICENSE|LICENCE|COPYING|NOTICE)[^/]*|\.gitignore")


class PackError(Exception):
    pass


def allowed(rel: str) -> bool:
    """`rel` is relative to mixins/<id>/ (forward slashes)."""
    parts = rel.split("/")
    name = parts[-1]
    if name.endswith((".rs", ".toml")):
        return True
    if parts[0] == "data" or (len(parts) >= 3 and parts[1] == "data"):
        return True
    return len(parts) <= 2 and TEXT_RE.fullmatch(name) is not None


def manifest(bundle: Path) -> dict:
    try:
        toml = tomllib.loads((bundle / "horizon-mixin.toml").read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as e:
        raise PackError(f"{bundle.name}/horizon-mixin.toml: {e}") from e
    mod = toml.get("mod", {})
    mod_id, version = mod.get("id", ""), mod.get("version", "")
    horizon = toml.get("compatibility", {}).get("horizon_revision", "")
    if mod_id != bundle.name:
        raise PackError(f"{bundle.name}: [mod] id is {mod_id!r}, the folder is mixins/{bundle.name}/")
    if not ID_RE.fullmatch(mod_id):
        raise PackError(f"{bundle.name}: id {mod_id!r} is not [a-z0-9_]{{1,64}}")
    if not isinstance(version, str) or not version:
        raise PackError(f"{bundle.name}: [mod] version is empty")
    if not COMMIT_RE.fullmatch(horizon):
        raise PackError(f"{bundle.name}: [compatibility] horizon_revision must be a 40-hex lowercase commit")
    if not COMMIT_RE.fullmatch(MIXINS_COMMIT):
        raise PackError("MIXINS_COMMIT must be a 40-hex lowercase commit")
    out = {"schema": 1, "id": mod_id, "version": version, "game": GAME, "horizon": horizon,
           "mixins": MIXINS_COMMIT}
    if mod_id in HOOKS:
        if HOOKS[mod_id] < 1:
            raise PackError(f"{mod_id}: hooks must be at least 1")
        out["hooks"] = HOOKS[mod_id]
    return out


def entries(bundle: Path) -> list[tuple[str, bytes]]:
    mod_id = bundle.name
    files: dict[str, bytes] = {}
    bad = []
    for path in sorted(p for p in bundle.rglob("*") if p.is_file()):
        rel = path.relative_to(bundle).as_posix()
        if not allowed(rel):
            bad.append(rel)
            continue
        files[rel] = path.read_bytes()
    if bad:
        raise PackError(f"{mod_id}: not allowed in a .hnmod: " + ", ".join(bad))
    if "LICENSE" not in files and (ROOT / "LICENSE").is_file():
        files["LICENSE"] = (ROOT / "LICENSE").read_bytes()
    return [(f"mixins/{mod_id}/{rel}", data) for rel, data in sorted(files.items())]


def build(bundle: Path) -> tuple[dict, bytes, int]:
    meta = manifest(bundle)
    items = [("horizon-native.json", (json.dumps(meta, indent=2) + "\n").encode())] + entries(bundle)
    total = sum(len(d) for _, d in items)
    if len(items) > MAX_ENTRIES:
        raise PackError(f"{meta['id']}: {len(items)} entries, the limit is {MAX_ENTRIES}")
    if total > MAX_BYTES:
        raise PackError(f"{meta['id']}: {total} bytes unpacked, the limit is {MAX_BYTES}")
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        for name, data in items:
            info = zipfile.ZipInfo(name, FIXED_DATE)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            info.create_system = 3
            z.writestr(info, data)
    return meta, buf.getvalue(), len(items)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--check", action="store_true", help="validate only, write nothing")
    a = ap.parse_args()
    bundles = sorted(p.parent for p in (ROOT / "mixins").glob("*/horizon-mixin.toml"))
    if not bundles:
        print("pack: no mixins/<id>/horizon-mixin.toml found", file=sys.stderr)
        return 1
    failed = False
    for bundle in bundles:
        try:
            meta, data, count = build(bundle)
        except PackError as e:
            print(f"pack: {e}", file=sys.stderr)
            failed = True
            continue
        name = f"{meta['id']}-{meta['version']}.hnmod"
        digest = hashlib.sha256(data).hexdigest()
        if not a.check:
            OUT.mkdir(parents=True, exist_ok=True)
            (OUT / name).write_bytes(data)
            (OUT / (name + ".sha256")).write_text(f"{digest}  {name}\n", encoding="utf-8", newline="\n")
        print(f"{'ok' if a.check else 'wrote'} {name}: {count} entries, {len(data)} bytes, sha256 {digest}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
