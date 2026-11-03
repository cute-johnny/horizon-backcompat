//! Which way a join goes: straight to the server, through Horizon's own translation, or through
//! ViaProxy. Decided from the server's status ping (its protocol number, as ViaFabricPlus reads it
//! from the `StatusResponse`) unless the player forced a version for that server.
use crate::{Error, Result, steps};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// The config file, in the game folder.
pub const CONFIG_FILE: &str = "horizon-backcompat.json";
/// The plugin's own folder, in the game folder: ViaProxy's jar, its run folder and log.
pub const DATA_DIR: &str = "horizon-backcompat";

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Off: every join goes straight to the server, as without the plugin.
    pub enabled: bool,
    /// How long the status ping that detects a server's version may take.
    pub detect_timeout_ms: u64,
    pub viaproxy: ViaProxyConfig,
    /// Forced versions, by `host` or `host:port` (lowercase). See [`Forced`].
    pub servers: Vec<(String, Forced)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ViaProxyConfig {
    pub enabled: bool,
    /// ViaProxy's jar. Relative paths are from the game folder.
    pub jar: PathBuf,
    /// The Java to run it with: `java` on the PATH (or `JAVA_HOME`'s) when unset.
    pub java: Option<PathBuf>,
    /// ViaProxy listens on 127.0.0.1 at this port.
    pub port: u16,
    pub startup_timeout_ms: u64,
    /// More ViaProxy CLI options, such as `["--chat-signing", "false"]`.
    pub args: Vec<String>,
}

/// A version the player forced for a server (ViaFabricPlus's manual override, for proxies that
/// hide the version of the server behind them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Forced {
    /// `"direct"`: never translate.
    Direct,
    /// A version Horizon translates itself, such as `"26.2"`.
    Native(i32),
    /// Anything else goes to ViaProxy as written: a ViaProxy version name such as `"1.8.x"`,
    /// `"1.21.4"`, `"b1.7-b1.7.3"`, `"c0.30-c0.32 (CPE)"`, or `"auto"` for its own detection.
    ViaProxy(String),
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            detect_timeout_ms: 3000,
            viaproxy: ViaProxyConfig {
                enabled: true,
                jar: Path::new(DATA_DIR).join("ViaProxy.jar"),
                java: None,
                port: 25568,
                startup_timeout_ms: 45_000,
                args: Vec::new(),
            },
            servers: Vec::new(),
        }
    }
}

impl Config {
    /// The config in `game_dir`, or the defaults when there is none. A file that does not parse is
    /// an error: joining with settings the player did not mean would be worse.
    pub fn load(game_dir: &Path) -> Result<Self> {
        match std::fs::read_to_string(game_dir.join(CONFIG_FILE)) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::new(format!("{CONFIG_FILE}: {e}"))),
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        let json: Value = serde_json::from_str(text).map_err(|e| Error::new(format!("{CONFIG_FILE}: {e}")))?;
        let mut config = Self::default();
        let bad = |key: &str| Error::new(format!("{CONFIG_FILE}: `{key}` has the wrong type"));
        if let Some(v) = json.get("enabled") {
            config.enabled = v.as_bool().ok_or_else(|| bad("enabled"))?;
        }
        if let Some(v) = json.get("detect_timeout_ms") {
            config.detect_timeout_ms = v.as_u64().ok_or_else(|| bad("detect_timeout_ms"))?;
        }
        if let Some(vp) = json.get("viaproxy") {
            let c = &mut config.viaproxy;
            if let Some(v) = vp.get("enabled") {
                c.enabled = v.as_bool().ok_or_else(|| bad("viaproxy.enabled"))?;
            }
            if let Some(v) = vp.get("jar") {
                c.jar = v.as_str().ok_or_else(|| bad("viaproxy.jar"))?.into();
            }
            if let Some(v) = vp.get("java") {
                c.java = Some(v.as_str().ok_or_else(|| bad("viaproxy.java"))?.into());
            }
            if let Some(v) = vp.get("port") {
                c.port = v
                    .as_u64()
                    .and_then(|p| u16::try_from(p).ok())
                    .filter(|&p| p != 0)
                    .ok_or_else(|| bad("viaproxy.port"))?;
            }
            if let Some(v) = vp.get("startup_timeout_ms") {
                c.startup_timeout_ms = v.as_u64().ok_or_else(|| bad("viaproxy.startup_timeout_ms"))?;
            }
            if let Some(v) = vp.get("args") {
                c.args = v
                    .as_array()
                    .ok_or_else(|| bad("viaproxy.args"))?
                    .iter()
                    .map(|a| a.as_str().map(str::to_owned).ok_or_else(|| bad("viaproxy.args")))
                    .collect::<Result<_>>()?;
            }
        }
        if let Some(servers) = json.get("servers") {
            for (address, version) in servers.as_object().ok_or_else(|| bad("servers"))? {
                let version = version.as_str().ok_or_else(|| bad("servers"))?.trim();
                config
                    .servers
                    .push((address.trim().to_ascii_lowercase(), Forced::parse(version)));
            }
        }
        Ok(config)
    }

    /// The version forced for `host` and `port`: `host:port` first, then `host` alone.
    pub fn forced(&self, host: &str, port: u16) -> Option<&Forced> {
        let host = host.trim_end_matches('.').to_ascii_lowercase();
        let with_port = format!("{host}:{port}");
        self.servers
            .iter()
            .find(|(a, _)| *a == with_port)
            .or_else(|| self.servers.iter().find(|(a, _)| *a == host))
            .map(|(_, f)| f)
    }

    /// The defaults as a file to edit.
    pub fn template() -> String {
        let d = Self::default();
        format!(
            r#"{{
  "enabled": true,
  "detect_timeout_ms": {},
  "viaproxy": {{
    "enabled": true,
    "jar": "{}",
    "port": {},
    "startup_timeout_ms": {},
    "args": []
  }},
  "servers": {{
    "example-old-server.net": "1.8.x",
    "example-proxy.net:25570": "auto",
    "example-new-server.net": "direct"
  }}
}}
"#,
            d.detect_timeout_ms,
            d.viaproxy.jar.to_string_lossy().replace('\\', "/"),
            d.viaproxy.port,
            d.viaproxy.startup_timeout_ms
        )
    }
}

impl Forced {
    pub fn parse(version: &str) -> Self {
        if version.eq_ignore_ascii_case("direct") {
            Self::Direct
        } else if let Some(native) = steps::native_by_name(version) {
            Self::Native(native.protocol)
        } else {
            Self::ViaProxy(version.to_owned())
        }
    }
}

/// What the status ping found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Detected {
    /// The protocol number the server reported (the legacy ping's for servers before 1.7).
    Protocol(i32),
    /// No answer: the join goes ahead as usual and fails as it would.
    Unreachable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Direct,
    Native { protocol: i32 },
    ViaProxy { version: String },
}

/// The route for a join to `host:port`. `bridge` says whether ViaProxy can be used (enabled, and
/// its jar is there).
pub fn decide(config: &Config, host: &str, port: u16, pinned: i32, detected: &Detected, bridge: bool) -> Route {
    if !config.enabled {
        return Route::Direct;
    }
    let via = |version: &str| {
        if bridge {
            Route::ViaProxy {
                version: version.to_owned(),
            }
        } else {
            Route::Direct
        }
    };
    match config.forced(host, port) {
        Some(Forced::Direct) => return Route::Direct,
        Some(Forced::Native(protocol)) => return Route::Native { protocol: *protocol },
        Some(Forced::ViaProxy(version)) => return via(version),
        None => {}
    }
    match detected {
        Detected::Unreachable => Route::Direct,
        // A server with ViaVersion reports the client's own protocol when it can translate it.
        Detected::Protocol(p) if *p == pinned => Route::Direct,
        Detected::Protocol(p) if steps::native_by_protocol(*p).is_some() => Route::Native { protocol: *p },
        // ViaProxy pings again and picks the exact version itself: protocol numbers alone are
        // ambiguous before 1.7 (ViaLegacy) and on April Fools versions.
        Detected::Protocol(_) => via("auto"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINNED: i32 = 777;

    #[test]
    fn the_defaults_parse_back_from_the_template() {
        let mut parsed = Config::parse(&Config::template()).unwrap();
        assert_eq!(parsed.servers.len(), 3);
        parsed.servers.clear();
        assert_eq!(parsed, Config::default());
    }

    #[test]
    fn detection_picks_direct_native_or_viaproxy() {
        let c = Config::default();
        let route = |d: Detected, bridge| decide(&c, "mc.example", 25565, PINNED, &d, bridge);
        assert_eq!(route(Detected::Protocol(777), true), Route::Direct);
        assert_eq!(route(Detected::Protocol(776), true), Route::Native { protocol: 776 });
        assert_eq!(route(Detected::Protocol(47), true), Route::ViaProxy { version: "auto".into() });
        assert_eq!(route(Detected::Protocol(47), false), Route::Direct);
        assert_eq!(route(Detected::Unreachable, true), Route::Direct);
    }

    #[test]
    fn a_forced_version_wins_over_detection() {
        let c = Config::parse(
            r#"{"servers": {"Hub.Example": "1.8.x", "hub.example:25570": "direct", "new.example": "26.2"}}"#,
        )
        .unwrap();
        let p777 = Detected::Protocol(777);
        assert_eq!(
            decide(&c, "hub.example", 25565, PINNED, &p777, true),
            Route::ViaProxy { version: "1.8.x".into() }
        );
        assert_eq!(decide(&c, "hub.example", 25570, PINNED, &p777, true), Route::Direct);
        assert_eq!(
            decide(&c, "new.example.", 25565, PINNED, &Detected::Unreachable, true),
            Route::Native { protocol: 776 }
        );
    }

    #[test]
    fn disabled_means_direct() {
        let c = Config::parse(r#"{"enabled": false, "servers": {"a": "1.8.x"}}"#).unwrap();
        assert_eq!(decide(&c, "a", 25565, PINNED, &Detected::Protocol(47), true), Route::Direct);
    }

    #[test]
    fn a_wrong_type_is_an_error_not_a_default() {
        assert!(Config::parse(r#"{"viaproxy": {"port": 70000}}"#).is_err());
        assert!(Config::parse(r#"{"enabled": "yes"}"#).is_err());
        assert!(Config::parse("{").is_err());
    }
}
