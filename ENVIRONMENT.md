# Working environment (for agents and humans)

How to continue work on this repo from a fresh machine / container / cloud session. Rules for *how
to work* live in [AGENTS.md](AGENTS.md); this file is about *what the machine needs*.

## What lives where

| Thing | Location | In git? |
|---|---|---|
| Native mod + engine crate | `mixins/backcompat/` | yes |
| ViaProxy plugin (Java) | `viaproxy-plugin/` | yes (built jar `mixins/backcompat/engine/data/HorizonAuthBridge.jar` is committed) |
| Mock legacy server | `e2e/` | yes |
| Packaging | `dist/pack.py` → `dist/out/` | script yes, output no |
| Agent tooling | `.hermes/bin/` (`repo-map.py`, `verify.py`, `setup-env.py`), `.hermes/plans/` | yes |
| Upstream checkouts (Horizon, horizon-mixins, ViaProxy jar) | `../research/upstream/` (sibling of this repo) | **no** — recreate with `setup-env.py` |
| edgewarn (separate mod) | `../horizon-edgewarn` (own repo `cute-johnny/horizon-edgewarn`) | separate repo |
| Local research scratch (`../research/{probe,reports,stage,tools}`) | sibling dir | **no**, only on the original PC; not needed to build/verify |

The repo expects to sit at `<anything>/<this-repo>` with `../research/upstream/` next to it. Any
parent directory name works.

## Setup on a new machine

1. Install: **git**, **Python 3.10+**, **Rust via rustup** (stable), **JDK 17+** (`JAVA_HOME` or on `PATH`).
   - Windows only: also `rustup toolchain install stable-x86_64-pc-windows-gnu`. There is no MSVC
     `link.exe` on the original PC and Git Bash's `/usr/bin/link` shadows it; `verify.py` selects the GNU
     toolchain automatically (override with `VERIFY_CARGO_TOOLCHAIN`, empty = rustup default — use that on
     Linux/macOS, where it is already the default).
2. Clone this repo (private, needs `gh auth login` or a PAT with `repo` scope):
   ```bash
   gh repo clone cute-johnny/horizon-backcompat
   cd horizon-backcompat
   ```
3. Recreate the upstream checkouts and the ViaProxy jar the plugin compiles against:
   ```bash
   python .hermes/bin/setup-env.py          # or --check to only report
   ```
   This pins Horizon `31f83499…`, horizon-mixins `ddd9700…`, ViaProxy `3.4.14`. If you keep the jar
   elsewhere, set `VIAPROXY_JAR=<path>` instead.
4. Sanity check:
   ```bash
   python .hermes/bin/repo-map.py
   python .hermes/bin/verify.py -p horizon-backcompat-engine
   ```
   Expect one line `VERIFY PASS`. If the jar is missing the Java plugin check is *skipped and reported*,
   not failed.

## What can and cannot be done in each kind of environment

- **Any machine (Linux/macOS/Windows/cloud):** edit engine crate and plugin, run `verify.py`, run
  `dist/pack.py` to build `.hnmod` files, edit docs.
- **Not verifiable anywhere by `verify.py`:** the loose mixin glue `mixins/backcompat/*.rs` (compiles only
  when staged into Horizon). Verifying it needs `horizon-mixins apply` + a launcher build (≈1.5–10 min
  incremental/cold, many minutes first time) — only do that on a machine with the Horizon launcher, and
  never pass `--trust-native-code` on the user's behalf.
- **Needs the user's PC:** playing in Horizon, real-server tests (e.g. kokscraft.pl), anything using
  `%APPDATA%\horizon\horizon-backcompat.json`, the launcher instance `horizon`.
- **Cloud/headless sessions:** no Horizon launcher and no GUI; stick to engine, plugin, tooling and docs.

## Platform gotchas

- `horizon-mixins apply` on Windows needs a fresh `--out` dir every time (replacing a stage only works on Linux).
- Shell commands in docs are bash; on Windows run them in Git Bash.
- `.hermes/bin/__pycache__` is ignored; don't commit build output (`/target`, `/viaproxy-plugin/build`,
  `/dist/out`, `/stage`).
- Line endings: the repo is edited on Windows; keep LF in source. Don't mass-reformat.

## Publishing notes

- This GitHub repo is **private** and includes the agent tooling (`AGENTS.md`, `.hermes/`, this file).
  Those are *not* inside the `.hnmod` (`dist/pack.py` only zips `mixins/<id>/`). If the repo is ever made
  public, review `.hermes/plans/` and this file first.
- Releases for players: `python dist/pack.py` → `dist/out/backcompat-<v>.hnmod` + `.sha256`, tag
  `backcompat-<v>`, GitHub release. Pins to bump together when moving Horizon: `horizon-mixin.toml`
  (`horizon_revision`), `dist/pack.py`, `.hermes/bin/setup-env.py`, `NOTES.md`.
