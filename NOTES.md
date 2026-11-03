# Project notes (summary of the build session, 2026-11-03)

## Goal

A plugin for Horizon (github.com/horizon-rust/horizon, Minecraft Java 26.3 rewritten in Rust,
protocol 777) that lets it join servers on older versions. It follows the ViaFabricPlus approach
from the user's guide (`~/.gemini/antigravity/scratch/viafabricplus-technical-guide.md`):
protocol translation in the network pipeline, with auto-detection plus a manual override.

## Decisions

- **Native mod, not `.hmod`.** `.hmod` files are content-only (data and assets, no code). Packet
  translation needs code, so this is a horizon-mixins bundle (`mixins/backcompat/`), compiled by the
  launcher into its own game build.
- **Approach chosen by the user: native chain + ViaProxy.**
  - 26.2 (776) is translated in Rust using Horizon's own `horizon_protocol::adapter` id tables,
    plus `punch` -> `swing`.
  - Every other version goes through a local ViaProxy (3.4.14+, supports 26.3 clients) with
    `--wildcard-domain-handling INTERNAL --auth-method NONE` on 127.0.0.1:25568.
  - Online mode: the `HorizonAuthBridge` ViaProxy plugin asks Horizon to do the session join over
    the login query `horizon_backcompat:join`, so the token never leaves Horizon. Only connections
    to the local ViaProxy answer that query. The plugin stops ViaProxy when Horizon exits.
- **Hooks** are raw-target cancellable HEADs on `ConnWriter::send`, `ConnReader::recv` and
  `net::connect_tcp`, plus a HEAD on `Session::start`. Horizon's named hook catalog has nothing
  on the packet pipeline.

## Pins and environment

- Horizon `31f83499b6c2cff7a8e7bf45f8173a7a95c2767c`, horizon-mixins `ddd9700` (0.6.0).
- Windows: use `cargo +stable-x86_64-pc-windows-gnu`. There is no MSVC `link.exe`, and Git
  Bash's `/usr/bin/link` shadows it anyway. The JDK is at `C:\Program Files\Java\jdk-25.0.4.1`.
- `horizon-mixins apply` on Windows needs a fresh `--out` directory each time (replacing an
  existing stage only works on Linux).
- Upstream clones, the ViaProxy jar and the staged builds are still in
  `../research/upstream/` and `../research/stage/`. They can be deleted, or re-created with git
  and `horizon-mixins apply`.

## Test results

- Engine: 27/27 tests pass. Staged `horizon-protocol`: 179/179 pass, plus 4 glue tests.
  `horizon-client-lib` type-checks.
- Live end to end with a mock 1.8 online-mode server (`e2e/`): passes. The session hash matched.
- Launcher build: `horizon-launcher native-build mixins/backcompat --instance horizon
  --trust-native-code` (the user approved the trust flag) produced `native-31f8349-bd1d711780d9`
  in 9 min 22 s. The Horizon instance plays it.
- Packaged `.hnmod` files (`dist/pack.py`), 2026-11-03: both pass Horizon's `cargo horizon-mod check`.
  Unpacked and built with the launcher into a separate `packtest` instance (user approved the trust flag):
  edgewarn alone 5 min 54 s, backcompat alone 1 min 28 s, both together 1 min 27 s (incremental). All built;
  not yet played in game from the packaged files.
- Real server, kokscraft.pl (Velocity, supports 1.8 and 1.19.4-1.21.11):
  - The status ping echoes the client's protocol, so auto-detection picks Direct and the server
    refuses 26.3. The fix was the override `"kokscraft.pl": "1.21.11"` in
    `%APPDATA%\horizon\horizon-backcompat.json`.
  - Forced through ViaProxy: the online-mode join, configuration and play all worked, terrain
    loaded, 0 decode errors.
  - The anti-bot lobby put the player in spectator mode at y~450-560 above empty End chunks with
    abilities flags 0 (no flight), and kicked after ~22 s ("ANTI-BOT ... code: 500"). It is a
    gravity check: vanilla's `LocalPlayer.aiStep` forces a spectator's flight only inside
    `if (abilities.mayfly)`, so a vanilla client falls. Horizon (`motion.rs`,
    `player_movement_tick_with_fluids`) forces flight for every spectator, so it sent
    `player_abilities flying=true` and hovered. Not the VPN: the same kick happened without it.
    Found with a logging relay between Horizon and ViaProxy (`../research/tools/relay_log.py`).
  - **Fixed (2026-11-03)** by the `spectator-flight-*` HEAD injections (expression mixins refuse
    the tick's `?`): `native-31f8349-7d8d36998a0e` passes the check and reaches the lobby. Still
    a Horizon bug upstream (present on main `b193a8771`); worth reporting there.

## Open items

- Report the spectator-flight bug to Horizon; drop the injections once it is fixed there.
- Other things an anti-bot could flag: the brand is `horizon`, and `client_information` sends
  view distance 33 (vanilla caps it at 32).
- ~~Package as `.hnmod` / publish on GitHub~~: done, `dist/pack.py` + GitHub releases
  (cute-johnny/horizon-backcompat). edgewarn moved to its own repo (cute-johnny/horizon-edgewarn).
- Possibly: fall back automatically when a direct join is kicked with a "version not supported"
  message, since proxies like this one make auto-detection unreliable.
- Native 26.2 doesn't translate field-level changes inside shared packets. Per-version client
  physics (as in ViaFabricPlus) is not implemented.
