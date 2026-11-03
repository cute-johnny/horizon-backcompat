//! The ViaProxy bridge: versions with no native step go through a local ViaProxy
//! (https://github.com/ViaVersion/ViaProxy), the ViaVersion stack as a standalone proxy.
//!
//! - ViaProxy runs with `--wildcard-domain-handling INTERNAL`: the client's handshake address is
//!   `original-address \7 target-host:port \7 version`, so one ViaProxy serves every server and
//!   version, and nothing about the target has to be configured in it.
//! - It runs with `--auth-method NONE`: it has no account. When an online-mode server asks it to
//!   join, the bundled ViaProxy plugin (`HorizonAuthBridge`, see `viaproxy-plugin/` in this
//!   repository) sends the client a login query on [`AUTH_CHANNEL`] with the server's hash, Horizon
//!   joins with its own signed-in account, and answers. The access token never leaves Horizon.
//! - It listens on 127.0.0.1 only.
use crate::{Error, Result, route::{DATA_DIR, ViaProxyConfig}};
use std::{
    io::Read,
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

/// The login query channel of the session join request (ViaProxy plugin <-> Horizon).
pub const AUTH_CHANNEL: &str = "horizon_backcompat:join";

/// The ViaProxy plugin that asks Horizon for session joins, built by `viaproxy-plugin/build.py`.
pub const PLUGIN_JAR: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/data/HorizonAuthBridge.jar"));
pub const PLUGIN_FILE: &str = "HorizonAuthBridge.jar";
/// The system property that tells the plugin which process started ViaProxy.
pub const PARENT_PROPERTY: &str = "horizon.backcompat.parent";

/// The handshake address that sends ViaProxy's internal wildcard handling to `target` at `version`.
pub fn handshake_host(original: &str, target: &str, version: &str) -> String {
    format!("{original}\u{7}{target}\u{7}{version}")
}

/// The answer to a session join request: `1`, or `0` and the reason in UTF-8.
pub fn encode_join_answer(result: &std::result::Result<(), String>) -> Vec<u8> {
    match result {
        Ok(()) => vec![1],
        Err(reason) => {
            let mut out = vec![0];
            let reason: String = reason.chars().take(256).collect();
            out.extend_from_slice(reason.as_bytes());
            out
        }
    }
}

/// `host:port` for the handshake's target, with IPv6 hosts in brackets.
pub fn target(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// ViaProxy's CLI arguments after `-jar <jar>`.
pub fn cli_args(config: &ViaProxyConfig) -> Vec<String> {
    let mut args: Vec<String> = [
        "cli",
        "--bind-address",
        &format!("127.0.0.1:{}", config.port),
        "--wildcard-domain-handling",
        "INTERNAL",
        "--auth-method",
        "NONE",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(config.args.iter().cloned());
    args
}

pub fn jar_path(game_dir: &Path, config: &ViaProxyConfig) -> PathBuf {
    game_dir.join(&config.jar)
}

/// ViaProxy can be used: enabled, and its jar is there (or one is already listening).
pub fn available(game_dir: &Path, config: &ViaProxyConfig) -> bool {
    config.enabled && (jar_path(game_dir, config).is_file() || listening(config.port))
}

pub fn address(config: &ViaProxyConfig) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], config.port))
}

fn listening(port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), Duration::from_millis(250)).is_ok()
}

fn java(config: &ViaProxyConfig) -> PathBuf {
    if let Some(java) = &config.java {
        return java.clone();
    }
    let exe = if cfg!(windows) { "java.exe" } else { "java" };
    match std::env::var_os("JAVA_HOME") {
        Some(home) if Path::new(&home).join("bin").join(exe).is_file() => Path::new(&home).join("bin").join(exe),
        _ => PathBuf::from("java"),
    }
}

/// The ViaProxy this process started.
static CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// Start ViaProxy unless it already listens, and wait until it does. Its run folder is
/// `<game>/horizon-backcompat/viaproxy/`, its output `viaproxy.log` there.
pub fn ensure_running(game_dir: &Path, config: &ViaProxyConfig) -> Result<SocketAddr> {
    let address = address(config);
    let mut child = CHILD.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = child.as_mut()
        && c.try_wait().ok().flatten().is_some()
    {
        *child = None;
    }
    if listening(config.port) {
        return Ok(address);
    }
    let run_dir = game_dir.join(DATA_DIR).join("viaproxy");
    let log = run_dir.join("viaproxy.log");
    if child.is_none() {
        let jar = jar_path(game_dir, config);
        if !jar.is_file() {
            return Err(Error::new(format!(
                "ViaProxy is not installed: put ViaProxy's jar at {}",
                jar.display()
            )));
        }
        install_plugin(&run_dir)?;
        let out = std::fs::File::create(&log).map_err(|e| Error::new(format!("{}: {e}", log.display())))?;
        let err = out.try_clone().map_err(|e| Error::new(e.to_string()))?;
        let mut command = Command::new(java(config));
        command
            // The plugin stops ViaProxy when this process is gone.
            .arg(format!("-D{PARENT_PROPERTY}={}", std::process::id()))
            .arg("-jar")
            .arg(&jar)
            .args(cli_args(config))
            .current_dir(&run_dir)
            .env("VP_RUN_DIR", &run_dir)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW: no console window pops up beside the game.
            command.creation_flags(0x0800_0000);
        }
        let spawned = command
            .spawn()
            .map_err(|e| Error::new(format!("could not start ViaProxy with {}: {e}", java(config).display())))?;
        *child = Some(spawned);
    }
    let deadline = Instant::now() + Duration::from_millis(config.startup_timeout_ms);
    loop {
        if listening(config.port) {
            return Ok(address);
        }
        if let Some(c) = child.as_mut()
            && let Ok(Some(status)) = c.try_wait()
        {
            *child = None;
            return Err(Error::new(format!(
                "ViaProxy stopped ({status}): {}",
                last_lines(&log, 3)
            )));
        }
        if Instant::now() >= deadline {
            return Err(Error::new(format!(
                "ViaProxy did not start listening on {address} in time; see {}",
                log.display()
            )));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Stop the ViaProxy this process started, if any.
pub fn stop() {
    if let Some(mut c) = CHILD.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = c.kill();
        let _ = c.wait();
    }
}

fn install_plugin(run_dir: &Path) -> Result<()> {
    let plugins = run_dir.join("plugins");
    std::fs::create_dir_all(&plugins).map_err(|e| Error::new(format!("{}: {e}", plugins.display())))?;
    let file = plugins.join(PLUGIN_FILE);
    if std::fs::read(&file).ok().as_deref() != Some(PLUGIN_JAR) {
        std::fs::write(&file, PLUGIN_JAR).map_err(|e| Error::new(format!("{}: {e}", file.display())))?;
    }
    Ok(())
}

fn last_lines(path: &Path, n: usize) -> String {
    let mut text = String::new();
    if let Ok(mut f) = std::fs::File::open(path) {
        let _ = f.read_to_string(&mut text);
    }
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::route::Config;

    #[test]
    fn the_internal_wildcard_address_has_three_parts() {
        let host = handshake_host("old.example", &target("old.example", 25565), "auto");
        let parts: Vec<_> = host.split('\u{7}').collect();
        assert_eq!(parts, ["old.example", "old.example:25565", "auto"]);
        assert_eq!(target("::1", 25565), "[::1]:25565");
    }

    #[test]
    fn join_answers_carry_the_reason() {
        assert_eq!(encode_join_answer(&Ok(())), [1]);
        assert_eq!(encode_join_answer(&Err("no".into())), [0, b'n', b'o']);
    }

    #[test]
    fn viaproxy_listens_locally_with_internal_wildcards_and_no_account() {
        let mut c = Config::default().viaproxy;
        c.args = vec!["--chat-signing".into(), "false".into()];
        let args = cli_args(&c);
        assert_eq!(
            args,
            [
                "cli",
                "--bind-address",
                "127.0.0.1:25568",
                "--wildcard-domain-handling",
                "INTERNAL",
                "--auth-method",
                "NONE",
                "--chat-signing",
                "false"
            ]
        );
    }

    #[test]
    fn the_plugin_jar_is_bundled() {
        // A jar is a zip: local file header magic.
        assert_eq!(&PLUGIN_JAR[..4], b"PK\x03\x04");
    }

    #[test]
    fn the_plugin_is_installed_once_and_kept_current() {
        let dir = std::env::temp_dir().join(format!("hbc-plugin-{}", std::process::id()));
        install_plugin(&dir).unwrap();
        let file = dir.join("plugins").join(PLUGIN_FILE);
        assert_eq!(std::fs::read(&file).unwrap(), PLUGIN_JAR);
        std::fs::write(&file, b"old").unwrap();
        install_plugin(&dir).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), PLUGIN_JAR);
        let _ = std::fs::remove_dir_all(dir);
    }
}
