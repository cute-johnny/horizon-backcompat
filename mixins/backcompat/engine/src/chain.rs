//! The per-connection pipeline: connection state tracking and the protocol chain.
//!
//! As in ViaVersion, the state is tracked from what the client sends, since each state change is
//! the client's packet: the handshake's intent opens status or login, `login_acknowledged` opens
//! the configuration, `finish_configuration` play, and `configuration_acknowledged` the
//! configuration again. A packet is translated in the state it was sent in; the change applies to
//! the packets after it. The client sends each of those packets from the network thread before it
//! reads the server's next packet, so the reader always sees the state the server is in.
use crate::{
    Error, Packet, Result, State,
    viaproxy,
    wire::{self, Reader},
};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

/// Packet ids between the pinned protocol and an older one, per state (Horizon's
/// `horizon_protocol::adapter` tables, generated from both versions' packets reports).
pub trait IdMap: Send {
    /// The client's id for a packet the server sent as `id`. `None`: the client has no such packet.
    fn clientbound(&self, state: State, id: i32) -> Option<i32>;
    /// The server's id for a packet the client sent as `id`. `None`: the server has no such
    /// packet, so it is not sent.
    fn serverbound(&self, state: State, id: i32) -> Option<i32>;
}

/// One protocol step, ViaVersion style: it translates between the protocol above it (closer to the
/// client) and [`Step::protocol`] below it. Each call appends zero or more packets to `out`, so a
/// step can drop, rewrite or split a packet.
pub trait Step: Send {
    /// A name for logs, such as `"26.3 -> 26.2"`.
    fn name(&self) -> &'static str;
    /// The protocol on the server's side of this step.
    fn protocol(&self) -> i32;
    fn serverbound(&mut self, state: State, packet: Packet, out: &mut Vec<Packet>) -> Result<()>;
    fn clientbound(&mut self, state: State, packet: Packet, out: &mut Vec<Packet>) -> Result<()>;
}

/// The pinned client's own ids the pipeline reads (from `horizon_protocol::ids`, so a regenerated
/// table is followed without editing this crate).
#[derive(Clone, Copy, Debug)]
pub struct ClientIds {
    pub handshake_intention: i32,
    /// Serverbound, login: the client entered the configuration.
    pub login_acknowledged: i32,
    /// Clientbound, login: a login plugin request (the bridge's session join request).
    pub login_custom_query: i32,
    /// Serverbound, login: its answer.
    pub login_custom_query_answer: i32,
    /// Serverbound, configuration: the client entered play.
    pub finish_configuration: i32,
    /// Serverbound, play: the client went back to the configuration.
    pub configuration_acknowledged: i32,
}

/// Runs an online-mode session join for the server hash it is given (Horizon's
/// `auth::join_server` with the signed-in account). `Err` holds the reason to report.
pub type Joiner = Arc<dyn Fn(&str) -> std::result::Result<(), String> + Send + Sync>;

pub enum Mode {
    /// Horizon translates: the handshake states the last step's protocol, and every packet goes
    /// through the steps.
    Native { steps: Vec<Box<dyn Step>> },
    /// The connection goes to Horizon's own local ViaProxy, which translates. The handshake's
    /// address names the real server (`target`, `host:port`) and its version (a ViaProxy version
    /// name, or `auto`); the protocol stays the client's. Only this mode answers the session join
    /// requests of [`viaproxy::AUTH_CHANNEL`]: a server reached directly never gets one answered.
    Bridge {
        target: String,
        version: String,
        joiner: Option<Joiner>,
    },
}

/// What to do with a packet the client sends.
#[derive(Debug, PartialEq, Eq)]
pub enum Outbound {
    /// Send it as it is.
    Pass,
    /// Send these bodies (VarInt id + payload) instead, in order; none drops it.
    Replace(Vec<Vec<u8>>),
}

/// What a packet the server sent became.
pub enum Inbound {
    /// Hand these to the client, in order; none drops it.
    Packets(Vec<Packet>),
    /// The bridge asks for an online-mode session join. Run `joiner` with `hash` (outside any
    /// lock: it is an HTTPS call), give its result to [`Pipeline::join_result`], then hand `packet`
    /// to the client: its answer to the request carries the result.
    Join {
        transaction: i32,
        hash: String,
        joiner: Option<Joiner>,
        packet: Packet,
    },
}

/// The most session joins one bridged connection may ask for (ViaProxy asks once per login).
const MAX_JOINS: usize = 2;

pub struct Pipeline {
    mode: Mode,
    ids: ClientIds,
    state: State,
    /// Join results waiting for the client's answer, by transaction.
    answers: HashMap<i32, Vec<u8>>,
    joins: usize,
    /// Translated packets the reader has not handed to the client yet.
    pending: VecDeque<Packet>,
}

impl Pipeline {
    pub fn new(mode: Mode, ids: ClientIds) -> Self {
        Self {
            mode,
            ids,
            state: State::Handshake,
            answers: HashMap::new(),
            joins: 0,
            pending: VecDeque::new(),
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    /// A short description for logs.
    pub fn describe(&self) -> String {
        match &self.mode {
            Mode::Native { steps } => {
                let names: Vec<_> = steps.iter().map(|s| s.name()).collect();
                format!("native [{}]", names.join(", "))
            }
            Mode::Bridge { target, version, .. } => format!("ViaProxy -> {target} ({version})"),
        }
    }

    /// A packet the client is about to send (`body`: VarInt id + payload).
    pub fn outbound(&mut self, body: &[u8]) -> Result<Outbound> {
        let packet = Packet::from_body(body)?;
        let state = self.state;
        let next = self.next_state(state, &packet)?;
        let result = if state == State::Handshake {
            self.handshake(packet)
        } else {
            self.translate_outbound(state, packet, body)
        };
        // Only a packet that was handled moves the state on.
        if result.is_ok() {
            self.state = next;
        }
        result
    }

    fn next_state(&self, state: State, packet: &Packet) -> Result<State> {
        let ids = &self.ids;
        Ok(match state {
            State::Handshake if packet.id == ids.handshake_intention => {
                match Intention::read(&packet.payload)?.intent {
                    1 => State::Status,
                    // 2 login, 3 transfer (a login the previous server sent us to).
                    2 | 3 => State::Login,
                    other => return Err(Error::new(format!("unknown handshake intent {other}"))),
                }
            }
            State::Login if packet.id == ids.login_acknowledged => State::Configuration,
            State::Configuration if packet.id == ids.finish_configuration => State::Play,
            State::Play if packet.id == ids.configuration_acknowledged => State::Configuration,
            other => other,
        })
    }

    fn handshake(&mut self, packet: Packet) -> Result<Outbound> {
        if packet.id != self.ids.handshake_intention {
            return Err(Error::new(format!("unexpected handshake packet {}", packet.id)));
        }
        let mut intention = Intention::read(&packet.payload)?;
        match &self.mode {
            Mode::Native { steps } => match steps.last() {
                Some(oldest) => intention.protocol = oldest.protocol(),
                None => return Ok(Outbound::Pass),
            },
            Mode::Bridge {
                target, version, ..
            } => {
                intention.host = viaproxy::handshake_host(&intention.host, target, version);
            }
        }
        Ok(Outbound::Replace(vec![
            Packet::new(packet.id, intention.write()).to_body(),
        ]))
    }

    fn translate_outbound(&mut self, state: State, packet: Packet, body: &[u8]) -> Result<Outbound> {
        let ids = self.ids;
        match &mut self.mode {
            Mode::Native { steps } => {
                let mut packets = vec![packet];
                for step in steps.iter_mut() {
                    let mut out = Vec::with_capacity(packets.len());
                    for p in packets {
                        step.serverbound(state, p, &mut out)?;
                    }
                    packets = out;
                }
                Ok(unchanged_or_replaced(packets, body))
            }
            Mode::Bridge { .. } => {
                if state == State::Login && packet.id == ids.login_custom_query_answer {
                    let mut r = Reader::new(&packet.payload);
                    let transaction = r.varint()?;
                    if let Some(answer) = self.answers.remove(&transaction) {
                        // ServerboundCustomQueryAnswerPacket: transaction, then an optional payload
                        // (a present flag and the rest of the packet).
                        let mut payload = wire::varint(transaction);
                        payload.push(1);
                        payload.extend_from_slice(&answer);
                        return Ok(Outbound::Replace(vec![
                            Packet::new(packet.id, payload).to_body(),
                        ]));
                    }
                }
                Ok(Outbound::Pass)
            }
        }
    }

    /// A packet the server sent, as the connection read it.
    pub fn inbound(&mut self, id: i32, payload: Vec<u8>) -> Result<Inbound> {
        let state = self.state;
        let packet = Packet::new(id, payload);
        match &mut self.mode {
            Mode::Native { steps } => {
                let mut packets = vec![packet];
                for step in steps.iter_mut().rev() {
                    let mut out = Vec::with_capacity(packets.len());
                    for p in packets {
                        step.clientbound(state, p, &mut out)?;
                    }
                    packets = out;
                }
                Ok(Inbound::Packets(packets))
            }
            Mode::Bridge { joiner, .. } => {
                if state == State::Login && packet.id == self.ids.login_custom_query {
                    let mut r = Reader::new(&packet.payload);
                    let transaction = r.varint()?;
                    let channel = r.string(32767)?;
                    if channel == viaproxy::AUTH_CHANNEL {
                        self.joins += 1;
                        if self.joins > MAX_JOINS {
                            return Err(Error::new("ViaProxy asked for too many session joins"));
                        }
                        let hash = std::str::from_utf8(r.rest())
                            .map_err(|_| Error::new("session join request is not UTF-8"))?
                            .to_owned();
                        if hash.is_empty() || hash.len() > 64 {
                            return Err(Error::new("bad session join request"));
                        }
                        return Ok(Inbound::Join {
                            transaction,
                            hash,
                            joiner: joiner.clone(),
                            packet,
                        });
                    }
                }
                Ok(Inbound::Packets(vec![packet]))
            }
        }
    }

    /// The result of the session join [`Inbound::Join`] asked for. The client's answer to that
    /// transaction carries it to ViaProxy.
    pub fn join_result(&mut self, transaction: i32, result: std::result::Result<(), String>) {
        self.answers
            .insert(transaction, viaproxy::encode_join_answer(&result));
    }

    /// Queue translated packets for the reader.
    pub fn queue(&mut self, packets: impl IntoIterator<Item = Packet>) {
        self.pending.extend(packets);
    }

    pub fn next_pending(&mut self) -> Option<Packet> {
        self.pending.pop_front()
    }
}

/// `Pass` when the steps left the packet as the client wrote it, so it is not encoded again.
fn unchanged_or_replaced(packets: Vec<Packet>, body: &[u8]) -> Outbound {
    if let [only] = packets.as_slice() {
        let mut r = Reader::new(body);
        if r.varint().ok() == Some(only.id) && body[r.position()..] == only.payload[..] {
            return Outbound::Pass;
        }
    }
    Outbound::Replace(packets.iter().map(Packet::to_body).collect())
}

/// ClientIntentionPacket.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Intention {
    pub protocol: i32,
    pub host: String,
    pub port: u16,
    pub intent: i32,
}

impl Intention {
    pub fn read(payload: &[u8]) -> Result<Self> {
        let mut r = Reader::new(payload);
        Ok(Self {
            protocol: r.varint()?,
            // Vanilla reads 255; the bridge's address is longer, and ViaProxy reads more.
            host: r.string(32767)?,
            port: r.u16()?,
            intent: r.varint()?,
        })
    }

    pub fn write(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.host.len() + 10);
        wire::write_varint(&mut out, self.protocol);
        wire::write_string(&mut out, &self.host);
        out.extend_from_slice(&self.port.to_be_bytes());
        wire::write_varint(&mut out, self.intent);
        out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 26.3's ids (horizon_protocol::ids at the pinned revision).
    pub(crate) const IDS: ClientIds = ClientIds {
        handshake_intention: 0,
        login_acknowledged: 3,
        login_custom_query: 4,
        login_custom_query_answer: 2,
        finish_configuration: 3,
        configuration_acknowledged: 16,
    };

    pub(crate) fn handshake(protocol: i32, host: &str, intent: i32) -> Vec<u8> {
        Packet::new(
            0,
            Intention {
                protocol,
                host: host.into(),
                port: 25565,
                intent,
            }
            .write(),
        )
        .to_body()
    }

    /// Adds 1000 to every id both ways, and stands for protocol 1.
    struct Shift;
    impl Step for Shift {
        fn name(&self) -> &'static str {
            "shift"
        }
        fn protocol(&self) -> i32 {
            1
        }
        fn serverbound(&mut self, _: State, p: Packet, out: &mut Vec<Packet>) -> Result<()> {
            out.push(Packet::new(p.id + 1000, p.payload));
            Ok(())
        }
        fn clientbound(&mut self, _: State, p: Packet, out: &mut Vec<Packet>) -> Result<()> {
            out.push(Packet::new(p.id - 1000, p.payload));
            Ok(())
        }
    }

    fn body(id: i32, payload: &[u8]) -> Vec<u8> {
        Packet::new(id, payload.to_vec()).to_body()
    }

    #[test]
    fn the_state_follows_the_clients_packets() {
        let mut p = Pipeline::new(Mode::Native { steps: vec![] }, IDS);
        assert_eq!(p.state(), State::Handshake);
        p.outbound(&handshake(777, "a", 2)).unwrap();
        assert_eq!(p.state(), State::Login);
        p.outbound(&body(0, b"hello")).unwrap();
        assert_eq!(p.state(), State::Login);
        p.outbound(&body(IDS.login_acknowledged, b"")).unwrap();
        assert_eq!(p.state(), State::Configuration);
        p.outbound(&body(IDS.finish_configuration, b"")).unwrap();
        assert_eq!(p.state(), State::Play);
        // 3 is not a state change in play.
        p.outbound(&body(3, b"")).unwrap();
        assert_eq!(p.state(), State::Play);
        p.outbound(&body(IDS.configuration_acknowledged, b"")).unwrap();
        assert_eq!(p.state(), State::Configuration);

        let mut status = Pipeline::new(Mode::Native { steps: vec![] }, IDS);
        status.outbound(&handshake(777, "a", 1)).unwrap();
        assert_eq!(status.state(), State::Status);
        let mut transfer = Pipeline::new(Mode::Native { steps: vec![] }, IDS);
        transfer.outbound(&handshake(777, "a", 3)).unwrap();
        assert_eq!(transfer.state(), State::Login);
    }

    #[test]
    fn a_native_handshake_states_the_oldest_steps_protocol() {
        let mut p = Pipeline::new(Mode::Native { steps: vec![Box::new(Shift)] }, IDS);
        let Outbound::Replace(bodies) = p.outbound(&handshake(777, "mc.example", 2)).unwrap() else {
            panic!("the handshake is rewritten");
        };
        let sent = Packet::from_body(&bodies[0]).unwrap();
        let intention = Intention::read(&sent.payload).unwrap();
        assert_eq!(intention.protocol, 1);
        assert_eq!(intention.host, "mc.example");
        assert_eq!(intention.intent, 2);
    }

    #[test]
    fn steps_run_newest_first_out_and_oldest_first_in() {
        let mut p = Pipeline::new(Mode::Native { steps: vec![Box::new(Shift), Box::new(Shift)] }, IDS);
        p.outbound(&handshake(777, "a", 2)).unwrap();
        assert_eq!(
            p.outbound(&body(5, b"x")).unwrap(),
            Outbound::Replace(vec![body(2005, b"x")])
        );
        let Inbound::Packets(packets) = p.inbound(2007, b"y".to_vec()).unwrap() else {
            panic!()
        };
        assert_eq!(packets, vec![Packet::new(7, b"y".to_vec())]);
    }

    #[test]
    fn an_untouched_packet_is_passed_without_encoding_it_again() {
        let mut p = Pipeline::new(Mode::Native { steps: vec![] }, IDS);
        p.outbound(&handshake(777, "a", 2)).unwrap();
        assert_eq!(p.outbound(&body(1, b"abc")).unwrap(), Outbound::Pass);
    }

    fn bridge(joiner: Option<Joiner>) -> Pipeline {
        Pipeline::new(
            Mode::Bridge {
                target: "old.example:25565".into(),
                version: "1.8.x".into(),
                joiner,
            },
            IDS,
        )
    }

    fn custom_query(transaction: i32, channel: &str, data: &[u8]) -> Vec<u8> {
        let mut payload = wire::varint(transaction);
        wire::write_string(&mut payload, channel);
        payload.extend_from_slice(data);
        payload
    }

    #[test]
    fn a_bridged_handshake_names_the_target_and_keeps_the_clients_protocol() {
        let mut p = bridge(None);
        let Outbound::Replace(bodies) = p.outbound(&handshake(777, "old.example", 2)).unwrap() else {
            panic!()
        };
        let intention = Intention::read(&Packet::from_body(&bodies[0]).unwrap().payload).unwrap();
        assert_eq!(intention.protocol, 777);
        assert_eq!(intention.host, "old.example\u{7}old.example:25565\u{7}1.8.x");
    }

    #[test]
    fn the_bridge_answers_its_session_join_request_with_the_result() {
        let joiner: Joiner = Arc::new(|hash: &str| {
            if hash == "-5c1d" { Ok(()) } else { Err("wrong hash".into()) }
        });
        let mut p = bridge(Some(joiner));
        p.outbound(&handshake(777, "old.example", 2)).unwrap();
        let request = custom_query(0x4842_0001, viaproxy::AUTH_CHANNEL, b"-5c1d");
        let Inbound::Join {
            transaction,
            hash,
            joiner,
            packet,
        } = p.inbound(IDS.login_custom_query, request.clone()).unwrap()
        else {
            panic!("a join request")
        };
        assert_eq!((transaction, hash.as_str()), (0x4842_0001, "-5c1d"));
        assert_eq!(packet.payload, request);
        p.join_result(transaction, (joiner.unwrap())(&hash));

        // The client answers with no payload; the answer leaves with the result.
        let mut answer = wire::varint(transaction);
        answer.push(0);
        let Outbound::Replace(bodies) = p.outbound(&body(IDS.login_custom_query_answer, &answer)).unwrap()
        else {
            panic!()
        };
        let mut expected = wire::varint(transaction);
        expected.extend_from_slice(&[1, 1]);
        assert_eq!(bodies, vec![body(IDS.login_custom_query_answer, &expected)]);
        // Other transactions pass.
        let mut other = wire::varint(3);
        other.push(0);
        assert_eq!(p.outbound(&body(IDS.login_custom_query_answer, &other)).unwrap(), Outbound::Pass);
    }

    #[test]
    fn other_login_queries_reach_the_client_untouched() {
        let mut p = bridge(None);
        p.outbound(&handshake(777, "old.example", 2)).unwrap();
        let query = custom_query(9, "velocity:player_info", b"\x01");
        let Inbound::Packets(packets) = p.inbound(IDS.login_custom_query, query.clone()).unwrap() else {
            panic!()
        };
        assert_eq!(packets, vec![Packet::new(IDS.login_custom_query, query)]);
    }

    #[test]
    fn a_native_connection_never_answers_a_join_request() {
        // A server reached directly could relay another server's hash: only the local bridge asks.
        let mut p = Pipeline::new(Mode::Native { steps: vec![] }, IDS);
        p.outbound(&handshake(777, "a", 2)).unwrap();
        let request = custom_query(1, viaproxy::AUTH_CHANNEL, b"abc");
        assert!(matches!(
            p.inbound(IDS.login_custom_query, request).unwrap(),
            Inbound::Packets(_)
        ));
    }

    #[test]
    fn a_bridge_refuses_endless_join_requests() {
        let mut p = bridge(None);
        p.outbound(&handshake(777, "a", 2)).unwrap();
        for _ in 0..MAX_JOINS {
            assert!(p.inbound(IDS.login_custom_query, custom_query(1, viaproxy::AUTH_CHANNEL, b"h")).is_ok());
        }
        assert!(p.inbound(IDS.login_custom_query, custom_query(1, viaproxy::AUTH_CHANNEL, b"h")).is_err());
    }
}
