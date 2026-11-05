# Task: Make backcompat + edgewarn shippable on GitHub (installable by other players)
Status: COMPLETED (local scope; GitHub publish deferred by user; Step 6 deferred to a later version)

## 1. Objective & Constraints
- A player installs each mod from the launcher (Mods page, install from a GitHub repository/folder, or later the registry) instead of us running `horizon-mixins apply` / `native-build` by hand. Sources: `research/upstream/horizon/docs/native-mods.md` (L8-60 pin rules, L545-575 `.hnmod` contents, L1455-1480 install flow), `docs/api/mod-files.md` (L35-80 `horizon-native.json`, allowed files, limits).
- What is happening now, and why it is not shippable (scouted, not guessed):
  1. **No package.** The repo has no `horizon-native.json`, no `.hnmod`, no LICENSE, no git repo, no release. Only local-folder builds work (`native-build mixins --trust-native-code`).
  2. **Hard pin.** Both bundles pin Horizon `31f83499...` (Nightly 2026-11-02) and horizon-mixins `ddd9700`. A pinned mod keeps building (Nightlies keep a `nightly-source/<date>-<short>` tag) but never follows Horizon updates. A player can only float with `"hooks": N`, which needs a bundle that uses ONLY catalog hook points. Backcompat uses raw targets (4 net + 2 motion): it cannot float. Edgewarn uses `tick`/`hud` (catalog) but also injects a module into `motion.rs` (not a hook point): it cannot float until that module is gone.
  3. **Player cost.** Every install is native code compiled on the player's PC: red trust warning, Rust toolchain download, first build 20-30 min per the docs (4 min here, incremental), a rebuild on each Horizon update. Unavoidable for native mods; document it, don't hide it.
  4. **Backcompat is not drop-in.** It needs `ViaProxy.jar` downloaded by hand (README Install step 1) and Java 17+. Edgewarn needs neither.
  5. **Bundles never built alone.** We built `mixins/` (both together). A player may install only one; each alone is untested.
  6. **Registry listing** is a reviewed PR to `horizon-rust/horizon-mods`; later versions are our GitHub releases. Installing from our own GitHub repo needs no review.
- Constraints: no behaviour changes except optional Step 6; trusting native code is the player's decision (never pass `--trust-native-code` on anyone's behalf); an `.hnmod` may hold only `.rs`, `.toml`, `data/**`, README/LICENSE/NOTICE/.gitignore (<=512 files, 64 MiB).

## 2. Target Code Slices (Exact Line Ranges)
- `mixins/backcompat/horizon-mixin.toml` (`L1-91`): `[mod] version`, `[compatibility] horizon_revision`; must equal `horizon-native.json`.
- `mixins/edgewarn/horizon-mixin.toml` (`L1-56`): same; `[[module]] id="edgewarn-motion"` (motion.rs) blocks `hooks`.
- `mixins/edgewarn/motion_glue.rs` (whole, ~30 lines): the only reason edgewarn is not catalog-only.
- `README.md` (`L1-163`, Install section ~L78-100): replace "stage and build it yourself" with launcher install steps + the ViaProxy step.
- `.gitignore` (2 lines): add `stage/`, `dist/out/`.
- New: `LICENSE`, `dist/pack.py`, per-bundle `README.md` in `mixins/<id>/`, optional `.github/workflows/release.yml`.

## 3. Execution Checklist
- [x] Step 1 (ask user): licence (suggest MIT, like horizon-mixins), GitHub owner/repo name, one repo with two mods vs two repos, registry listing vs own releases only.
- [x] Step 2: `git init`, LICENSE, extend `.gitignore`, per-bundle README inside `mixins/<id>/` (ships in the `.hnmod`).
- [x] Step 3 (`dist/pack.py`): per bundle write `horizon-native.json` {schema 1, id, version, game "26.3", horizon = pinned commit, mixins = full 40-hex commit of `ddd9700` (resolve in `research/upstream/horizon-mixins`), optional hooks}; zip `mixins/<id>/...` with forward slashes and fixed timestamps (reproducible); refuse files not allowed (`.jar` only under `data/`), >512 files or >64 MiB; read `[mod] id/version` and `horizon_revision` from the toml and fail on any mismatch.
- [x] Step 4: validate each `.hnmod` with Horizon's own checker `cargo horizon-mod check <file> --kind <native kind>` (confirm the kind name in `crates/cargo-horizon-mod/src/main.rs` first; build that tool from `research/upstream/horizon`).
- [x] Step 5 (user must approve trust flag): fresh-build EACH bundle alone, then both: `horizon-launcher native-build <unpacked hnmod folder> --instance <test> --trust-native-code`. Record times. Don't reuse the dev stage dirs.
- [ ] (deferred) Step 6 (optional, after Step 5): make edgewarn catalog-only (drop `motion_glue.rs` + `edgewarn-motion`, read support boxes through a public API or ask upstream for a helper) so it can set `"hooks": N` and survive Horizon updates. Needs a new in-game check; keep the pinned version as fallback.
- [x] Step 7: README: honest install steps (launcher > Mods > install from GitHub, trust notice, first build 4-30 min, ViaProxy.jar + Java 17 for backcompat, config files, uninstall), compatibility table (Horizon commit/Nightly date), known limits from NOTES.md "Open items".
- [~] Step 8 (local tags done; GitHub release not yet, per user): tag `backcompat-0.1.0` / `edgewarn-0.1.0`, attach `.hnmod` + sha256 to a GitHub release; optional minisign signatures; optional registry PR to `horizon-rust/horizon-mods`.
- [x] Step 9: `python .hermes/bin/verify.py` (engines still pass), `pack.py --check`, regenerate `.hermes/repo-map.md`.

## 4. Blockers / Notes
- Needs from user: Step 1 answers; approval for the trust-flag builds in Step 5; a GitHub account/repo. I won't publish or push anything unless told.
- The `.hnmod` can't carry ViaProxy (third-party jar, licence unchecked): keep it a documented manual download, or have the mod fetch it on first use (a design change, not in this plan).
- The Windows fresh `--out` rule only affects our dev loop (`horizon-mixins apply`), not players: the launcher stages by itself.
- Previous plan kept as `.hermes/plans/done-edgewarn.md`.

## 5. Execution results (2026-11-03)
- Step 1 answers: MIT (cute-johnny); two repos cute-johnny/horizon-backcompat (this folder) + cute-johnny/horizon-edgewarn (`../horizon-edgewarn`, own workspace); own GitHub releases, no registry PR; .hermes/ + AGENTS.md local only (gitignored), NOTES.md published.
- `dist/pack.py` (identical in both repos): reproducible; refuses disallowed files (tested); `cargo horizon-mod check` passes both.
- Launcher builds into a separate `packtest` instance: edgewarn alone 354 s, backcompat alone 89 s, both 92 s, all exit 0. The `horizon` instance is untouched.
- Commits: backcompat becf1b4 (tag backcompat-0.1.0), edgewarn 2bfb90b (tag edgewarn-0.1.0). Nothing pushed.
- To publish later: `gh repo create cute-johnny/<repo> --public --source . --push`, `git push --tags`, `gh release create <tag> dist/out/*.hnmod dist/out/*.sha256`.
- Notes: the git author email is the global one (a personal address); consider a GitHub noreply email before pushing. Global core.autocrlf=true means a fresh checkout may change line endings, so the .hnmod bytes can differ per machine (release the files built here).
