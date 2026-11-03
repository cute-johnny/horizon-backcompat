// horizon-backcompat, client side: injected as `horizon_client::net::backcompat`.
//
// `connect_tcp`'s HEAD decides each join's route before the socket opens: a server on the pinned
// version, or one forced `direct`, goes on as usual; otherwise this opens the connection itself (to
// the server for Horizon's own translation, or to the local ViaProxy) and attaches the pipeline the
// connection hooks run. `Session::start`'s HEAD remembers the signed-in account for the ViaProxy
// bridge's online-mode session joins.

use crate::auth::{AuthSession, JoinError};
use horizon_protocol::connection::{Connection, backcompat as wire};
use std::{
    ops::ControlFlow,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use wire::engine::{
    self,
    route::{Config, Detected, Route},
};

static ACCOUNT: Mutex<Option<Arc<AuthSession>>> = Mutex::new(None);

/// `Session::start`'s HEAD: the account the next connection joins as.
pub(crate) fn remember_account(auth: Option<Arc<AuthSession>>) {
    *ACCOUNT.lock().unwrap_or_else(|e| e.into_inner()) = auth;
}

fn describe(error: &JoinError) -> String {
    match error.reason() {
        Ok(key) => key.to_owned(),
        Err(message) => message.to_owned(),
    }
}

fn joiner() -> Option<engine::Joiner> {
    let account = ACCOUNT.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    Some(Arc::new(move |hash: &str| {
        crate::auth::join_server(&account, hash).map_err(|e| describe(&e))
    }))
}

/// The server's version, from the status ping the server list uses (the legacy ping for servers
/// before 1.7).
fn detect(host: &str, port: u16, timeout: Duration) -> Detected {
    let address = engine::viaproxy::target(host, port);
    match super::ping(&address, timeout) {
        Ok((json, _)) => json["version"]["protocol"]
            .as_i64()
            .and_then(|p| i32::try_from(p).ok())
            .map_or(Detected::Unreachable, Detected::Protocol),
        Err(_) => match super::ping_legacy(&address, timeout) {
            Ok(legacy) => Detected::Protocol(legacy.protocol),
            Err(_) => Detected::Unreachable,
        },
    }
}

fn write_template(game_dir: &Path) {
    let file = game_dir.join(engine::route::CONFIG_FILE);
    if !file.exists() {
        let _ = std::fs::write(&file, Config::template());
    }
}

fn opened(
    connection: horizon_protocol::Result<Connection>,
    read_timeout: Duration,
) -> Result<Connection, String> {
    let c = connection.map_err(|e| e.to_string())?;
    c.reader
        .set_read_timeout(Some(read_timeout))
        .map_err(|e| e.to_string())?;
    Ok(c)
}

/// `connect_tcp`'s HEAD.
pub(crate) fn connect(
    host: &str,
    port: u16,
    relayed: bool,
    read_timeout: Duration,
) -> ControlFlow<Result<(Connection, String, u16), String>> {
    // The friends relay is a Horizon server on Horizon's version.
    if relayed {
        return ControlFlow::Continue(());
    }
    let game_dir = std::env::current_dir().unwrap_or_default();
    write_template(&game_dir);
    let config = match Config::load(&game_dir) {
        Ok(config) => config,
        Err(e) => return ControlFlow::Break(Err(format!("Backward compatibility: {e}"))),
    };
    if !config.enabled {
        return ControlFlow::Continue(());
    }
    let detected = match config.forced(host, port) {
        Some(_) => Detected::Unreachable,
        None => detect(host, port, Duration::from_millis(config.detect_timeout_ms)),
    };
    let bridge = engine::viaproxy::available(&game_dir, &config.viaproxy);
    let route = engine::route::decide(
        &config,
        host,
        port,
        horizon_protocol::PROTOCOL_VERSION,
        &detected,
        bridge,
    );
    log::info!("[backcompat] {host}:{port}: {detected:?} -> {route:?}");
    match route {
        Route::Direct => ControlFlow::Continue(()),
        Route::Native { protocol } => {
            let Some(pipeline) = wire::native_pipeline(protocol) else {
                return ControlFlow::Break(Err(format!(
                    "Backward compatibility: no native translation to protocol {protocol}"
                )));
            };
            let resolved = Connection::connect_server_with_address(host, port, Duration::from_secs(10));
            let (connection, host, port) = match resolved {
                Ok((c, h, p)) => match opened(Ok(c), read_timeout) {
                    Ok(c) => (c, h, p),
                    Err(e) => return ControlFlow::Break(Err(e)),
                },
                Err(e) => return ControlFlow::Break(Err(e.to_string())),
            };
            log::info!("[backcompat] {}", pipeline.describe());
            wire::attach(&connection, pipeline);
            ControlFlow::Break(Ok((connection, host, port)))
        }
        Route::ViaProxy { version } => {
            let address = match engine::viaproxy::ensure_running(&game_dir, &config.viaproxy) {
                Ok(address) => address,
                Err(e) => return ControlFlow::Break(Err(format!("Backward compatibility: {e}"))),
            };
            let local = Connection::connect("127.0.0.1", address.port(), Duration::from_secs(10));
            let connection = match opened(local, read_timeout) {
                Ok(c) => c,
                Err(e) => return ControlFlow::Break(Err(e)),
            };
            let pipeline = wire::bridge_pipeline(engine::viaproxy::target(host, port), version, joiner());
            log::info!("[backcompat] {}", pipeline.describe());
            wire::attach(&connection, pipeline);
            ControlFlow::Break(Ok((connection, host.to_owned(), port)))
        }
    }
}

thread_local! {
    /// The spectator-flight fix's note from the movement tick to the travel it runs.
    static GROUNDED_SPECTATOR: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The movement tick's HEAD: a spectator neither flying nor allowed to fly this tick.
pub(crate) fn note_grounded_spectator(grounded: bool) {
    GROUNDED_SPECTATOR.with(|g| g.set(grounded));
}

/// The travel's HEAD: whether the tick that runs it noted such a spectator (once).
pub(crate) fn take_grounded_spectator() -> bool {
    GROUNDED_SPECTATOR.with(|g| g.replace(false))
}
