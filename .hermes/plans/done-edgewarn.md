# Task: Edge-Warning mod (parkour edge feedback) for Horizon 26.3, built beside backcompat
Status: COMPLETED (steps 1-8 except the in-game run; native-build not done)

## 1. Objective & Constraints
- New Horizon mixin bundle `mixins/edgewarn/` (Rust, same layout and pins as `mixins/backcompat/`: Horizon `31f83499…`, horizon-mixins `ddd9700`).
- While the local player is on the ground or in a jump and is about to leave the block they stand on, give automatic feedback so they never need to look down: **visual** (HUD edge cue) + **audio** (short tick whose rate rises toward the edge). Client-only, no packets, so it is safe on any server.
- "Compatible with above" = the bundle can be staged TOGETHER with `backcompat` (`horizon-mixins apply`) with no hook, module or id clash, and it works on connections routed through the native chain or ViaProxy. Backcompat already HEADs `MotionWorld::player_movement_tick_with_fluids` and `::dry_tick` (horizon-mixin.toml), so edgewarn must NOT hook those.
- Pure logic goes in a standalone crate `mixins/edgewarn/engine` (no Horizon types) so `verify.py` can test it. Glue is not verifiable here (same as backcompat): keep it tiny.
- Non-test Rust: no unwrap/expect/panic/unsafe.

## 2. Target Code Slices (Exact Line Ranges)
Horizon source (read-only reference) = `../research/upstream/horizon`:
- `crates/horizon-client/src/game/native_hooks.rs` `L2087-2089`: `native_tick(&mut self)` — per-tick named hook: run edge detection + sound here.
- same file `L2246-2248`: `native_hud(&mut self, batch:&mut HudBatch, corners:&mut Corners)` — draw the visual cue here with `native_quad` (`L8239`) / `native_fill`.
- same file `L2092-2100`: `native_block_highlight(...)` — NOT used (cancelling it would change vanilla outline; out of scope).
- same file `L7475`: `native_play_sound` is **authoritative/server-only** (`if !self.native_authoritative … return false`) — do NOT use for a client cue. Use the client-side local sound path (`play_local_sound`, `crates/horizon-client/src/mod_api/generated_host.rs:704`, `game/mod_api_host_generated.rs:283`) — confirm its signature in step 1.
- `crates/horizon-client/src/motion.rs` `L273-…`: `struct PlayerMotion` — fields to read: `position: Vec3`, `velocity: Vec3`, `ground: GroundState` (`.on_ground`), `fall_distance`, `shift_key_down`, `flying`, `spectator`, `yaw`.
- same file `L2142-2151`: `collision_shape(BlockPos)` / `collision_boxes(Aabb)` — the support-surface query (block collision under the feet).
Our repo:
- `mixins/backcompat/horizon-mixin.toml` (whole file, 105 lines): template for the manifest; its 6 `[[injection]]` ids/targets that edgewarn must avoid.
- `Cargo.toml` (5 lines): add `"mixins/edgewarn/engine"` to `members`.
- `.hermes/bin/verify.py`: check how it picks crates (`-p`); add edgewarn engine if crates are hard-coded.

## 3. Execution Checklist
Feedback design (decided, see notes): HUD = a thin bar at screen bottom-centre-of-crosshair that goes green→amber→red as the gap to the edge shrinks, and flashes on the side of the edge being approached; audio = quiet tick, interval 8→2 ticks as the gap shrinks, a different pitch for "already past the edge while still airborne-able (coyote)".
- [x] Step 1 (scout inside Horizon, read-only): confirm (a) how a named hook reaches `PlayerMotion` of the local player from `Game` (`native_tick`/`native_hud` self), (b) exact `play_local_sound` signature + a usable vanilla event (e.g. `minecraft:block.note_block.hat`), (c) names/versions of `tick` and `hud` in `<workspace>/native-hooks.json` (`docs/crates-data-and-named-hooks.md` L25-50). Record results under Notes.
- [x] Step 2 (`mixins/edgewarn/engine/src/lib.rs`, new): pure fn `edge_gap(support: &[Aabb], feet: Aabb, vel_xz: [f64;2], lookahead_ticks) -> Option<EdgeState{gap, side}>` + `Cue{level: 0..=3, tick_interval}` mapping. Unit tests: centred on block (no cue), 0.3 from edge (level1), at edge (level3), moving away (no cue), standing on a neighbouring block (no cue), sneaking at edge (suppressed), flying/spectator (suppressed), NaN/inf inputs (no cue, no panic).
- [x] Step 3 (`mixins/edgewarn/engine/Cargo.toml`, new + root `Cargo.toml`): crate `horizon-edgewarn-engine`, no deps; add to workspace members.
- [x] Step 4 (`mixins/edgewarn/horizon-mixin.toml`, new): `[mod] id="edgewarn"`, same `[compatibility] horizon_revision`, `[[crate]] id="engine"` (dependent `horizon-client`), `[[module]] id="edgewarn-glue" name="edgewarn"` (own module name, so it never collides with `backcompat`), two named-hook `[[injection]]`s (ids `edgewarn-tick`, `edgewarn-hud`, kind HEAD). Priority 10 (after backcompat's 0). Do not target `player_movement_tick_with_fluids` or `dry_tick`.
- [x] Step 5 (`mixins/edgewarn/glue.rs`, `tick.rs`, `hud.rs`, new): tick = read player, query collision boxes under feet, call engine, store `Cue` in a `static`/Game-owned cell, play tick sound at the cue interval; hud = draw bar from stored `Cue`. Add a config file `%APPDATA%\horizon\horizon-edgewarn.json` (enabled, volume, hud on/off, sensitivity), same pattern as backcompat's JSON config.
- [x] Step 6: `python .hermes/bin/repo-map.py` (no git hook here).
- [x] Step 7: Verify `python .hermes/bin/verify.py -p horizon-edgewarn-engine` (max 2 auto-fix attempts, then log under Blockers and ask).
- [x] Step 8 (compat check, needs the research checkout): `horizon-mixins apply` with BOTH bundles into a fresh `--out` dir (Windows needs a fresh dir) and `cargo check` the staged `horizon-client`; then a launcher `native-build` of both with `--trust-native-code` (needs the user's approval; ~9 min). This is the only real proof of "compatible"; glue cannot be verified by `verify.py`.

## 4. Blockers / Notes
- Open question A (default if unanswered: ground + airborne cue, sneak suppressed): should sneaking at an edge (safe-edge) mute the cue?
- Open question B (default: bar + tick): HUD style, bar at the crosshair or a screen-edge glow?
- The tick sound API is unverified: if only `native_play_sound` (server-only) can play sounds, fall back to the client `play_local_sound`; if neither is reachable from `native_tick`, ship visual-only first and report.
- Auto-detection quirk in NOTES.md (proxy servers) is irrelevant here: the mod is client-side and version-agnostic. Per-version client physics is not implemented in backcompat, so the cue uses the 26.3 physics even on old servers; lookahead is deliberately short to limit mismatch.
- Anti-cheat: pure local cue, sends nothing, reads nothing the player can't see.

## 5. Result
- Answers: sneaking mutes the cue; bar first, `style: "crosshair"` is reserved in the config and draws the bar for now.
- `verify.py -p horizon-edgewarn-engine`: PASS, 22 tests.
- `horizon-mixins apply` with backcompat + edgewarn together (8 injections, 4 modules, 2 crates) and `cargo check -p horizon-client` on the staged Horizon: PASS (one fix: `AutoJumpWorld` trait import for `collision_boxes`).
- NOT done: launcher `native-build` (needs --trust-native-code approval) and an in-game run. Unconfirmed there: block boxes from `collision_boxes` are in world coordinates; `block.note_block.hat` passes `local_sound_ok`; `batch.width/height` are GUI pixels.
- v1 limits: warns about the take-off edge only (grounded); no landing-edge cue while airborne.
