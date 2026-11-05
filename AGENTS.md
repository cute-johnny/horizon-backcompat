# Horizon backward-compat mod — lean agent rules (token-optimal workflow)

Native Horizon mod (Rust) + ViaProxy plugin (Java) so Horizon (MC 26.3, protocol 777) can join older servers.
Read `README.md` / `NOTES.md` **only for the section your task touches** (routing, ViaProxy bridge, pins, Windows quirks).

## Workflow rules (always)
1. **Map first.** Locate code with `python .hermes/bin/repo-map.py --query <regex> [--path mixins/backcompat/engine]`
   (prints `L<start>-<end>: symbol`; methods of impl/trait/class are indented). `.hermes/repo-map.md` is small
   but grep it, don't read it whole. Then `read_file(path, offset=start, limit=end-start+1)`.
   Never read a whole file >150 lines (`chain.rs` 549, `protocol_glue.rs` 424, `route.rs` 287, `viaproxy.rs` 265).
2. **Batch in 1 turn.** Inspecting >2 files or filtering search output → one `execute_code` script printing only the slices.
3. **Gated flow.** Scout → Plan (`.hermes/plans/current.md`) → user approval → Execute → Verify.
   No source edits during Scout/Plan. Use `/scout-plan` and `/execute-plan`.
4. **Silent verify + circuit breaker.** Verify only via `python .hermes/bin/verify.py`
   (`-p horizon-backcompat-engine [test-filter]`, `--clippy`, `--full`, `--no-java`, `--no-rust`).
   No raw `cargo test/build` or `javac` in terminal. Max **2** auto-fix attempts, then stop, log under
   `## Blockers` in `current.md`, ask the user.
5. Release: `python dist/pack.py` → `dist/out/backcompat-<v>.hnmod` + `.sha256`; tag `backcompat-<v>`; GitHub
   release on cute-johnny/horizon-backcompat. edgewarn lives in `../horizon-edgewarn` (own repo).
6. After adding/moving files: `python .hermes/bin/repo-map.py` (no git hook runs it; also commit the regenerated `.hermes/repo-map.md`).

## Layout & what verify covers
- `mixins/backcompat/engine/` — standalone Rust crate `horizon-backcompat-engine` (chain, route, steps, viaproxy, wire). **Verified.**
- `mixins/backcompat/*.rs` + `horizon-mixin.toml` — mixin glue/hooks; compile only when staged into Horizon
  (`horizon-mixins apply`). **Not verified here**: keep edits minimal and re-read them carefully.
- `viaproxy-plugin/` — `HorizonAuthBridge` ViaProxy plugin (Java 17). Verified with javac against the ViaProxy jar
  (`../research/upstream/viaproxy-jar/`, or `VIAPROXY_JAR`); skipped (reported) if the jar is missing.
  `viaproxy-plugin/build.py --viaproxy <jar>` rebuilds `engine/data/HorizonAuthBridge.jar`; run it after plugin edits.
- `e2e/MockLegacyServer.java` — mock legacy server for end-to-end checks (javac-verified only).

## Environment
- **New machine / fresh clone / cloud session?** Read `ENVIRONMENT.md` first, then run
  `python .hermes/bin/setup-env.py` (recreates `../research/upstream/` + the ViaProxy jar), then `verify.py`.
- The repo is on GitHub (private): `cute-johnny/horizon-backcompat`, branch `main`. Commit small, push when
  verify passes. Never commit secrets/tokens, `target/`, or `dist/out/`.
- Things only the user's own PC can do: play in Horizon, real-server tests, launcher native builds. Say so
  instead of guessing when you are elsewhere.
- Windows: cargo uses `+stable-x86_64-pc-windows-gnu` (no MSVC `link.exe`); verify.py does this automatically.
- JDK 17+ needed (`JAVA_HOME` else PATH). Pins: Horizon `31f83499…`, horizon-mixins `ddd9700` (see `NOTES.md`).
- `horizon-mixins apply` on Windows needs a fresh `--out` dir every time.

## Conventions
- Non-test Rust: no `unwrap/expect/panic!`/`unsafe`; wire/packet data is hostile input (bounds-checked reads, capped allocations).
- Keep the plugin jar reproducible (fixed `--date`); don't commit build output (`/target`, `/viaproxy-plugin/build`).
