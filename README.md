# ARCHIVED

It was made for old version of horizon only to make me be able to join older servers and play once on server.

# Horizon backward compatibility

A native mod for [Horizon](https://github.com/horizon-rust/horizon) (the Rust rewrite of Minecraft:
Java Edition 26.3) that lets the client **join servers on older Minecraft versions**. It is Horizon's
roadmap item `older-servers`, built the way ViaFabricPlus is: protocol translation in the network
pipeline, chosen per server by auto-detection or a manual override.

| Server version | How it is joined |
| --- | --- |
| 26.3 (protocol 777), or any server that reports 777 (ViaVersion on the server) | directly, untouched |
| 26.2 (776) | **native**: translated in Rust inside Horizon, no Java |
| everything else ViaProxy supports: 26.1 … 1.7, 1.6.4 … 1.0, Beta, Alpha, Classic, April Fools, Bedrock (WIP) | **ViaProxy bridge**: a local [ViaProxy](https://github.com/ViaVersion/ViaProxy) the mod starts on demand |

Online-mode servers work through the bridge too: ViaProxy has no account, and asks Horizon to do the
session join with Horizon's own signed-in Microsoft account. The token never leaves Horizon.

## How it works

```
Horizon (26.3)                                                     server
  net::run ─► connect_tcp ──HEAD──► route: status ping ─► protocol?
                                        │ 777 / "direct" ─────────────────────────────► server
                                        │ 776 / "26.2" ─► Connection ─► pipeline ─────► server (26.2)
                                        └ other / forced ─► ViaProxy (127.0.0.1) ───► server (any)
  ConnWriter::send ──HEAD──► pipeline.outbound   (state tracking, steps newest → oldest)
  ConnReader::recv ──HEAD──► pipeline.inbound    (steps oldest → newest, queue for 1→n packets)
```

- **Route** (`engine/src/route.rs`): before the socket opens, `connect_tcp`'s HEAD pings the server
  (the server list's own status ping, the legacy ping for pre-1.7 servers) and reads the protocol
  number from the `StatusResponse`, the auto-negotiation in ViaFabricPlus. A forced version in the
  config wins over detection, for proxies that hide the backend's version.
- **Pipeline** (`engine/src/chain.rs`): per connection, attached to it by the `Counters` its reader
  and writer share. As in ViaVersion, the connection state is tracked from what the client sends
  (handshake intent, `login_acknowledged`, `finish_configuration`, `configuration_acknowledged`),
  and packets run through version **steps**. A step may drop, rewrite or split a packet.
- **26.2 step** (`engine/src/steps.rs`): Horizon's own generated id tables
  (`horizon_protocol::adapter`, until now unused) renumber the 131 moved packets per state, the
  handshake states protocol 776, the serverbound `punch` becomes 26.2's `swing` (main hand), and
  26.3-only serverbound packets are not sent.
- **ViaProxy bridge** (`engine/src/viaproxy.rs`): ViaProxy runs headless with
  `--wildcard-domain-handling INTERNAL --auth-method NONE`, bound to `127.0.0.1`. The handshake's
  address becomes `host \7 host:port \7 version` so one ViaProxy serves every server. The bundled
  ViaProxy plugin `HorizonAuthBridge` (`viaproxy-plugin/`) answers ViaProxy's `JoinServerRequestEvent`
  by sending Horizon a login query (`horizon_backcompat:join`) with the server hash; Horizon calls
  `auth::join_server` with its account and answers. Only connections Horizon opened to its own
  local ViaProxy answer that query, so a server reached directly can never make Horizon join
  another server. The plugin also stops ViaProxy when the Horizon process that started it exits.

## Layout

```
mixins/backcompat/            the horizon-mixins bundle (the mod)
  horizon-mixin.toml          4 HEAD hooks, 2 support modules, 1 crate
  engine/                     horizon-backcompat-engine: pipeline, steps, routing, ViaProxy supervisor
    data/HorizonAuthBridge.jar  the ViaProxy plugin, built by viaproxy-plugin/build.py
  protocol_glue.rs            → horizon_protocol::connection::backcompat (registry, send/recv bodies)
  client_glue.rs              → horizon_client_lib::net::backcompat (detection, routing, account)
  send.rs recv.rs connect.rs start.rs   the hook helpers
viaproxy-plugin/              HorizonAuthBridge (Java, ViaProxy plugin API)
e2e/MockLegacyServer.java     a 1.8 online-mode server for the bridge's end-to-end test
dist/pack.py                  packs mixins/<id>/ into dist/out/<id>-<version>.hnmod (+ .sha256)
```

Hook points (raw targets; Horizon's named hook catalog has none on the packet pipeline):

| Target | Kind | Why |
| --- | --- | --- |
| `horizon_protocol::connection::ConnWriter::send` | cancellable HEAD | translate or drop what the client sends |
| `horizon_protocol::connection::ConnReader::recv` | cancellable HEAD | translate what the server sends |
| `horizon_client_lib::net::connect_tcp` | cancellable HEAD | detect the version, open the routed connection |
| `horizon_client_lib::net::Session::start` | HEAD | remember the account for bridged session joins |
| `horizon_client_lib::motion::MotionWorld::player_movement_tick_with_fluids`, `::dry_tick` | HEAD | a Horizon fix: a spectator the server denies flight falls, as in vanilla (anti-bot gravity checks) |

Pinned to Horizon `31f83499b6c2cff7a8e7bf45f8173a7a95c2767c` (Nightly 2026-11-02) and horizon-mixins
`ddd9700` (0.6.0, plan schema 6).

## Install

Native mods are compiled into the game on your own computer. Read this first:

- **They aren't sandboxed.** A native mod runs with your rights. The launcher shows a trust notice and a red warning
  before every build. This mod is not reviewed by the Horizon registry: read the source in this repository first.
- **The first build takes a while.** The launcher downloads its own Rust once, then compiles Horizon with the mod
  (Horizon's docs say 20-30 minutes for the first build; later builds are faster). After a Horizon update the
  instance keeps playing the build it has until you rebuild.
- **Pinned to one Horizon.** 0.1.0 builds on Horizon `31f83499b6c2cff7a8e7bf45f8173a7a95c2767c` (Nightly 2026-11-02)
  and horizon-mixins `ddd9700` (0.6.0), also after newer Nightlies come out. It hooks Horizon's network pipeline
  directly (there are no stable hook points there), so it can't follow Horizon updates by itself: each new Horizon
  needs a new release of this mod. Mixed with mods that pin a newer Horizon, the launcher asks you to "Build anyway".

Steps:

1. Get ViaProxy (only needed for versions other than 26.2): download `ViaProxy-<version>.jar` from
   [ViaProxy's releases](https://github.com/ViaVersion/ViaProxy/releases) (3.4.14 or later: it
   supports 26.3 clients) and save it as `horizon-backcompat/ViaProxy.jar` in the instance's game
   folder. ViaProxy needs Java 17+ (`java` on the PATH, `JAVA_HOME`, or `viaproxy.java` below).
2. In the Horizon launcher, open **Mods**, pick the instance on the left, choose **Add from GitHub** and paste
   `https://github.com/cute-johnny/horizon-backcompat` (its latest release). From the command line:
   `horizon-launcher mods add-github https://github.com/cute-johnny/horizon-backcompat --instance <name> --trust-native-code`.
   Or download `backcompat-<version>.hnmod` from [Releases](https://github.com/cute-johnny/horizon-backcompat/releases)
   (check it against the `.sha256` file next to it).
3. Press **Build** on the "Native build of {instance}" row, and confirm "I trust these mods: build".
4. Join any server from the multiplayer screen. Older servers still show the red version label in
   the list (the list's compatibility check is untouched); joining works.

**Update:** `horizon-launcher mods update-github backcompat`. **Uninstall:** `horizon-launcher mods remove backcompat`,
then build again. Works beside [edgewarn](https://github.com/cute-johnny/horizon-edgewarn) (parkour edge warning).

## Configuration

`horizon-backcompat.json` in the game folder, written with the defaults on the first join:

```json
{
  "enabled": true,
  "detect_timeout_ms": 3000,
  "viaproxy": {
    "enabled": true,
    "jar": "horizon-backcompat/ViaProxy.jar",
    "port": 25568,
    "startup_timeout_ms": 45000,
    "args": []
  },
  "servers": {
    "example-old-server.net": "1.8.x",
    "example-proxy.net:25570": "auto",
    "example-new-server.net": "direct"
  }
}
```

- `servers` forces a version per `host` or `host:port`: `"direct"`, `"26.2"` (native), or any
  ViaProxy version name (`java -jar ViaProxy.jar cli --list-versions`), or `"auto"` for ViaProxy's
  own detection. Unlisted servers are detected.
- `viaproxy.java` sets the Java executable; `viaproxy.args` adds ViaProxy CLI options, such as
  `["--chat-signing", "false"]`. ViaProxy's own translation settings are in
  `horizon-backcompat/viaproxy/ViaLoader/`, its log in `horizon-backcompat/viaproxy/viaproxy.log`.
- A ViaProxy you run yourself on `viaproxy.port` is used as it is; it needs
  `--wildcard-domain-handling INTERNAL` and the plugin in its `plugins/` folder for online mode.

## Build and test

Needs Rust (the `stable-x86_64-pc-windows-gnu` toolchain on Windows without the MSVC build tools),
Python 3, and a JDK 17+ for the plugin.

```sh
# The ViaProxy plugin (compiled against ViaProxy's jar, copied into engine/data/)
python viaproxy-plugin/build.py --viaproxy path/to/ViaProxy-3.4.14.jar

# The engine alone
cargo test

# The .hnmod for a release (reproducible); --check validates only
python dist/pack.py

# The bundle against Horizon: plan, stage, and the glue's tests inside the staged workspace
horizon-mixins plan  --workspace path/to/horizon --mods mixins --out stage/plan.json
horizon-mixins apply --workspace path/to/horizon --mods mixins --out stage/modded
cd stage/modded
cargo test --locked -p horizon-protocol -p horizon-backcompat-engine
cargo check --locked -p horizon-client-lib --no-default-features

# The bridge end to end: ViaProxy + the plugin + a 1.8 online-mode mock server
HBC_VIAPROXY_JAR=path/to/ViaProxy-3.4.14.jar HBC_MOCK=path/to/e2e/MockLegacyServer.java \
  cargo test --locked -p horizon-protocol --lib backcompat -- --ignored --nocapture
```

## Limits

- **Native 26.2 covers what the packets reports show:** packet ids, the new packets and
  `swing`/`punch`. A field that changed inside a packet both versions share (an item component, a
  registry entry's shape) is not in those reports and is not translated yet. Such a packet fails to
  decode and Horizon reports it as a decode error rather than crashing. To send a 26.2 server
  through ViaProxy instead, force it to `"auto"` in `servers`.
- **Through ViaProxy, Horizon's own client physics stays 26.3's.** ViaFabricPlus also patches the
  client's movement, collisions and interaction to the target version. Horizon has no such
  per-version physics, so strict anti-cheats on old servers (1.8 PvP especially) may flag movement
  that differs between versions.
- Older servers still show as incompatible in the server list.
- The online-mode session join through the bridge needs the launcher's Microsoft sign-in, as any
  online-mode server does.
- **Proxies can fool auto-detection.** Some proxies (Velocity networks such as kokscraft.pl) echo the
  client's protocol in the status ping, so the mod joins directly and the server refuses 26.3. Force the
  version in `servers` (for example `"kokscraft.pl": "1.21.11"`).
- Anti-bot plugins may still notice the client: its brand is `horizon`, and it sends view distance 33
  (vanilla caps it at 32).
- Spectators the server denies flight now fall, as in vanilla. This is a fix for a Horizon bug, made by the
  `spectator-flight-*` hooks; they will be dropped once Horizon fixes it.
