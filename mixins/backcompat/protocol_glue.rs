// horizon-backcompat, connection side: injected as `horizon_protocol::connection::backcompat`.
//
// A connection the client glue routed to an older server carries a pipeline, found by the
// `Counters` its reader and writer share. `ConnWriter::send` and `ConnReader::recv` hand their
// packets to it (two cancellable HEAD hooks); every other connection is untouched and pays one
// atomic load per packet.

pub use horizon_backcompat_engine as engine;

use super::{ConnReader, ConnWriter, Connection, Counters};
use crate::{Error, Result, adapter::ServerVersion, ids};
use std::{
    cell::Cell,
    ops::ControlFlow,
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicUsize, Ordering},
    },
};

struct Entry {
    // Kept alive by the connection; a Weak also keeps its address from being reused.
    counters: Weak<Counters>,
    pipeline: Arc<Mutex<engine::Pipeline>>,
}

static ENTRIES: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    // Set while the glue itself sends or reads through the hooked functions.
    static RAW: Cell<bool> = const { Cell::new(false) };
}

struct Raw;
impl Raw {
    fn enter() -> Self {
        RAW.with(|r| r.set(true));
        Raw
    }
}
impl Drop for Raw {
    fn drop(&mut self) {
        RAW.with(|r| r.set(false));
    }
}

fn lock(pipeline: &Mutex<engine::Pipeline>) -> MutexGuard<'_, engine::Pipeline> {
    pipeline.lock().unwrap_or_else(|e| e.into_inner())
}

/// The pinned version's ids the pipeline reads.
pub fn client_ids() -> engine::ClientIds {
    engine::ClientIds {
        handshake_intention: ids::handshake::serverbound::INTENTION,
        login_acknowledged: ids::login::serverbound::LOGIN_ACKNOWLEDGED,
        login_custom_query: ids::login::clientbound::CUSTOM_QUERY,
        login_custom_query_answer: ids::login::serverbound::CUSTOM_QUERY_ANSWER,
        finish_configuration: ids::configuration::serverbound::FINISH_CONFIGURATION,
        configuration_acknowledged: ids::play::serverbound::CONFIGURATION_ACKNOWLEDGED,
    }
}

/// Horizon's adapter tables (`crate::adapter`) as the engine's id map.
struct AdapterIds(ServerVersion);

fn state(state: engine::State) -> crate::State {
    match state {
        engine::State::Handshake => crate::State::Handshake,
        engine::State::Status => crate::State::Status,
        engine::State::Login => crate::State::Login,
        engine::State::Configuration => crate::State::Configuration,
        engine::State::Play => crate::State::Play,
    }
}

impl engine::IdMap for AdapterIds {
    fn clientbound(&self, s: engine::State, id: i32) -> Option<i32> {
        self.0.clientbound_id(state(s), id)
    }
    fn serverbound(&self, s: engine::State, id: i32) -> Option<i32> {
        self.0.serverbound_id(state(s), id)
    }
}

/// Horizon's own translation down to `protocol`, when it has one.
pub fn native_pipeline(protocol: i32) -> Option<engine::Pipeline> {
    let steps = engine::steps::chain_for(protocol, ids::play::serverbound::PUNCH, &|p| {
        ServerVersion::from_protocol(p)
            .filter(|v| *v != ServerVersion::Current)
            .map(|v| Box::new(AdapterIds(v)) as Box<dyn engine::IdMap>)
    })?;
    Some(engine::Pipeline::new(engine::Mode::Native { steps }, client_ids()))
}

/// A connection to Horizon's local ViaProxy, bound for `target` (`host:port`) at `version`.
pub fn bridge_pipeline(target: String, version: String, joiner: Option<engine::Joiner>) -> engine::Pipeline {
    engine::Pipeline::new(
        engine::Mode::Bridge {
            target,
            version,
            joiner,
        },
        client_ids(),
    )
}

/// Route `connection`'s packets through `pipeline`, before its first packet.
pub fn attach(connection: &Connection, pipeline: engine::Pipeline) {
    let mut entries = ENTRIES.lock().unwrap_or_else(|e| e.into_inner());
    entries.retain(|e| e.counters.strong_count() > 0);
    entries.push(Entry {
        counters: Arc::downgrade(&connection.reader.counters),
        pipeline: Arc::new(Mutex::new(pipeline)),
    });
    ACTIVE.store(entries.len(), Ordering::Release);
}

fn lookup(counters: &Arc<Counters>) -> Option<Arc<Mutex<engine::Pipeline>>> {
    if ACTIVE.load(Ordering::Acquire) == 0 || RAW.with(|r| r.get()) {
        return None;
    }
    let entries = ENTRIES.lock().unwrap_or_else(|e| e.into_inner());
    entries
        .iter()
        .find(|e| std::ptr::eq(e.counters.as_ptr(), Arc::as_ptr(counters)) && e.counters.strong_count() > 0)
        .map(|e| e.pipeline.clone())
}

fn failed(e: engine::Error) -> Error {
    Error::new(format!("Backward compatibility: {e}"))
}

/// `ConnWriter::send`'s HEAD: the packet as the server's version knows it.
pub fn on_send(writer: &mut ConnWriter, body: &[u8]) -> ControlFlow<Result<()>> {
    let Some(pipeline) = lookup(&writer.counters) else {
        return ControlFlow::Continue(());
    };
    let outbound = lock(&pipeline).outbound(body);
    match outbound {
        Ok(engine::Outbound::Pass) => ControlFlow::Continue(()),
        Ok(engine::Outbound::Replace(bodies)) => {
            let _raw = Raw::enter();
            for body in &bodies {
                if let Err(e) = writer.send(body) {
                    return ControlFlow::Break(Err(e));
                }
            }
            ControlFlow::Break(Ok(()))
        }
        Err(e) => ControlFlow::Break(Err(failed(e))),
    }
}

/// `ConnReader::recv`'s HEAD: the next packet as the client knows it.
pub fn on_recv(reader: &mut ConnReader) -> ControlFlow<Result<(i32, Vec<u8>)>> {
    let Some(pipeline) = lookup(&reader.counters) else {
        return ControlFlow::Continue(());
    };
    loop {
        if let Some(p) = lock(&pipeline).next_pending() {
            return ControlFlow::Break(Ok((p.id, p.payload)));
        }
        // Not under the pipeline's lock: the writer must not wait for the server.
        let read = {
            let _raw = Raw::enter();
            reader.recv()
        };
        let (id, payload) = match read {
            Ok(packet) => packet,
            Err(e) => return ControlFlow::Break(Err(e)),
        };
        let inbound = lock(&pipeline).inbound(id, payload);
        match inbound {
            Ok(engine::Inbound::Packets(packets)) => lock(&pipeline).queue(packets),
            Ok(engine::Inbound::Join {
                transaction,
                hash,
                joiner,
                packet,
            }) => {
                // An HTTPS call to the session service: not under the lock.
                let result = match joiner {
                    Some(join) => join(&hash),
                    None => Err("Horizon is not signed in to a Microsoft account".to_owned()),
                };
                if let Err(reason) = &result {
                    eprintln!("[backcompat] online-mode join through ViaProxy failed: {reason}");
                }
                let mut p = lock(&pipeline);
                p.join_result(transaction, result);
                p.queue([packet]);
            }
            Err(e) => return ControlFlow::Break(Err(failed(e))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packets::{handshake, login, play};
    use crate::connection::Pipe;

    fn intention(protocol: i32, host: &str) -> Vec<u8> {
        handshake::Serverbound::Intention(handshake::Intention {
            protocol,
            host: host.into(),
            port: 25565,
            intent: handshake::LOGIN,
        })
        .encode()
    }

    fn into_play(client: &mut Connection, server: &mut Connection) {
        client
            .send(&login::Serverbound::LoginAcknowledged(crate::packets::Empty).encode())
            .unwrap();
        server.recv().unwrap();
        client
            .send(&crate::packets::configuration::Serverbound::FinishConfiguration(crate::packets::Empty).encode())
            .unwrap();
        server.recv().unwrap();
    }

    #[test]
    fn the_engine_reads_horizons_own_handshake_and_ids() {
        let ids = client_ids();
        assert_eq!(ids.handshake_intention, 0);
        let body = intention(crate::PROTOCOL_VERSION, "old.example");
        let packet = engine::Packet::from_body(&body).unwrap();
        let read = engine::chain::Intention::read(&packet.payload).unwrap();
        assert_eq!(read.protocol, crate::PROTOCOL_VERSION);
        assert_eq!(read.host, "old.example");
        assert_eq!(read.intent, handshake::LOGIN);
    }

    #[test]
    fn a_26_2_connection_is_translated_both_ways_over_a_real_connection() {
        let (a, b) = Pipe::pair();
        let (mut client, mut server) = (Connection::local(a), Connection::local(b));
        attach(&client, native_pipeline(776).expect("26.2 has adapter tables"));

        client.send(&intention(crate::PROTOCOL_VERSION, "old.example")).unwrap();
        let (id, payload) = server.recv().unwrap();
        let mut r = crate::buf::Reader::new(&payload);
        assert_eq!((id, r.varint().unwrap()), (0, 776), "the handshake states 26.2's protocol");

        into_play(&mut client, &mut server);
        // punch -> 26.2's swing (63) with the main hand.
        client.send(&play::Serverbound::Punch(crate::packets::Empty).encode()).unwrap();
        assert_eq!(server.recv().unwrap(), (63, vec![0]));
        // resource_pack: 26.3's id -> 26.2's 49, payload untouched.
        let renumbered = ServerVersion::V26_2
            .serverbound_id(crate::State::Play, ids::play::serverbound::RESOURCE_PACK)
            .unwrap();
        let mut body = Vec::new();
        crate::buf::write_varint(&mut body, ids::play::serverbound::RESOURCE_PACK);
        body.extend_from_slice(b"payload");
        client.send(&body).unwrap();
        assert_eq!(server.recv().unwrap(), (renumbered, b"payload".to_vec()));

        // keep_alive: 26.2's 44 reaches the client as 26.3's id.
        let mut keep_alive = Vec::new();
        crate::buf::write_varint(&mut keep_alive, 44);
        keep_alive.extend_from_slice(&7i64.to_be_bytes());
        server.send(&keep_alive).unwrap();
        let (id, payload) = client.recv().unwrap();
        assert_eq!(id, ids::play::clientbound::KEEP_ALIVE);
        assert!(matches!(
            play::Clientbound::decode(id, &payload).unwrap(),
            play::Clientbound::KeepAlive(k) if k.id == 7
        ));
    }

    #[test]
    fn the_bridge_asks_for_a_join_and_answers_viaproxy_through_horizons_own_codecs() {
        let (a, b) = Pipe::pair();
        let (mut client, mut server) = (Connection::local(a), Connection::local(b));
        let joiner: engine::Joiner = Arc::new(|hash: &str| {
            if hash == "4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48" { Ok(()) } else { Err("wrong".into()) }
        });
        attach(
            &client,
            bridge_pipeline("old.example:25565".into(), "auto".into(), Some(joiner)),
        );
        client.send(&intention(crate::PROTOCOL_VERSION, "old.example")).unwrap();
        let (_, payload) = server.recv().unwrap();
        let intention = match handshake::Serverbound::decode(0, &payload) {
            Ok(handshake::Serverbound::Intention(i)) => i,
            other => panic!("{other:?}"),
        };
        assert_eq!(intention.protocol, crate::PROTOCOL_VERSION);
        assert_eq!(intention.host, "old.example\u{7}old.example:25565\u{7}auto");

        // ViaProxy's plugin asks; Horizon's login loop sees an ordinary query and answers None.
        server
            .send(
                &login::Clientbound::CustomQuery(login::CustomQuery {
                    transaction: 0x4842_0000,
                    channel: engine::viaproxy::AUTH_CHANNEL.into(),
                    data: b"4ed1f46bbe04bc756bcb17c0c7ce3e4632f06a48".to_vec(),
                })
                .encode(),
            )
            .unwrap();
        let (id, payload) = client.recv().unwrap();
        let login::Clientbound::CustomQuery(q) = login::Clientbound::decode(id, &payload).unwrap() else {
            panic!()
        };
        client
            .send(
                &login::Serverbound::CustomQueryAnswer(login::CustomQueryAnswer {
                    transaction: q.transaction,
                    payload: None,
                })
                .encode(),
            )
            .unwrap();
        let (id, payload) = server.recv().unwrap();
        let login::Serverbound::CustomQueryAnswer(answer) = login::Serverbound::decode(id, &payload).unwrap() else {
            panic!()
        };
        assert_eq!(answer.transaction, 0x4842_0000);
        assert_eq!(answer.payload, Some(vec![1]), "joined");
    }

    /// The whole bridge, live: Horizon's connection and login answers, the engine, ViaProxy with
    /// the HorizonAuthBridge plugin, and a 1.8 online-mode server (e2e/MockLegacyServer.java) that
    /// prints the session hash it would check. The hash Horizon joined with must be that one.
    #[test]
    #[ignore = "needs Java, HBC_VIAPROXY_JAR (ViaProxy's jar) and HBC_MOCK (e2e/MockLegacyServer.java)"]
    fn viaproxy_bridge_end_to_end() {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};
        let jar = std::env::var_os("HBC_VIAPROXY_JAR").expect("HBC_VIAPROXY_JAR");
        let mock = std::env::var_os("HBC_MOCK").expect("HBC_MOCK");
        let java = std::path::PathBuf::from(std::env::var_os("HBC_JAVA").unwrap_or_else(|| "java".into()));

        let mut server = Command::new(&java).arg(&mock).stdout(Stdio::piped()).spawn().unwrap();
        let mut lines = BufReader::new(server.stdout.take().unwrap()).lines();
        let first = lines.next().unwrap().unwrap();
        let server_port: u16 = first.strip_prefix("PORT ").unwrap().parse().unwrap();

        let game_dir = std::env::temp_dir().join(format!("hbc-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&game_dir).unwrap();
        let mut config = engine::route::Config::default().viaproxy;
        config.jar = jar.into();
        config.java = Some(java.clone());
        config.port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let address = engine::viaproxy::ensure_running(&game_dir, &config).unwrap();

        let mut client = Connection::connect("127.0.0.1", address.port(), std::time::Duration::from_secs(10)).unwrap();
        client.reader.set_read_timeout(Some(std::time::Duration::from_secs(60))).unwrap();
        let joined = Arc::new(Mutex::new(None::<String>));
        let record = joined.clone();
        let joiner: engine::Joiner = Arc::new(move |hash: &str| {
            *record.lock().unwrap() = Some(hash.to_owned());
            Ok(())
        });
        attach(
            &client,
            bridge_pipeline(format!("127.0.0.1:{server_port}"), "1.8.x".into(), Some(joiner)),
        );
        client.send(&intention(crate::PROTOCOL_VERSION, "127.0.0.1")).unwrap();
        client
            .send(
                &login::Serverbound::Hello(login::ServerHello {
                    name: "Steve".into(),
                    uuid: 0x1234,
                })
                .encode(),
            )
            .unwrap();
        // As Horizon's login loop: every query is answered with no payload.
        let ended: String;
        loop {
            let (id, payload) = match client.recv() {
                Ok(packet) => packet,
                Err(e) => {
                    ended = e.to_string();
                    break;
                }
            };
            match login::Clientbound::decode(id, &payload).unwrap() {
                login::Clientbound::CustomQuery(q) => client
                    .send(
                        &login::Serverbound::CustomQueryAnswer(login::CustomQueryAnswer {
                            transaction: q.transaction,
                            payload: None,
                        })
                        .encode(),
                    )
                    .unwrap(),
                login::Clientbound::Compression(p) => client.set_compression(p.threshold),
                login::Clientbound::Disconnect(d) => {
                    ended = d.reason_json;
                    break;
                }
                other => panic!("unexpected {other:?}"),
            }
        }
        let output: Vec<String> = lines.map_while(|l| l.ok()).collect();
        let _ = server.wait();
        engine::viaproxy::stop();
        eprintln!("client ended with: {ended}\nserver: {output:?}");
        assert!(output.contains(&"PROTOCOL 47".to_owned()), "ViaProxy spoke 1.8 to the server");
        assert!(output.contains(&"NAME Steve".to_owned()));
        let hash = output.iter().find_map(|l| l.strip_prefix("HASH ")).expect("the server got the key");
        assert_eq!(joined.lock().unwrap().as_deref(), Some(hash), "Horizon joined with the server's hash");
        let _ = std::fs::remove_dir_all(game_dir);
    }

    #[test]
    fn connections_without_a_pipeline_are_untouched() {
        let (a, b) = Pipe::pair();
        let (mut client, mut server) = (Connection::local(a), Connection::local(b));
        client.send(&intention(crate::PROTOCOL_VERSION, "new.example")).unwrap();
        let (_, payload) = server.recv().unwrap();
        let mut r = crate::buf::Reader::new(&payload);
        assert_eq!(r.varint().unwrap(), crate::PROTOCOL_VERSION);
    }
}
