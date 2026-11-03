# backcompat (Horizon native mod)

Lets [Horizon](https://github.com/horizon-rust/horizon) (Minecraft: Java Edition 26.3) join servers on older
Minecraft versions: 26.2 is translated in Rust, everything else goes through a local
[ViaProxy](https://github.com/ViaVersion/ViaProxy) that the mod starts on demand.

- Needs `ViaProxy.jar` (3.4.14 or later) saved as `horizon-backcompat/ViaProxy.jar` in the instance's game folder,
  and Java 17+. Joining 26.2 servers needs neither.
- Config: `%APPDATA%\horizon\horizon-backcompat.json`.
- Pinned to Horizon `31f83499b6c2cff7a8e7bf45f8173a7a95c2767c` (Nightly 2026-11-02).

Source, install steps and limits: https://github.com/cute-johnny/horizon-backcompat. MIT licence.
