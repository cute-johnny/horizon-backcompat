//! The versions Horizon translates itself, without ViaProxy.
//!
//! Each step renumbers packets with Horizon's own id tables (`horizon_protocol::adapter`, generated
//! from both versions' packets reports by `tools/generate_packet_id_adapter.py`) and rewrites the
//! packets whose contents changed. Adding a version: generate its adapter table in Horizon, add a
//! step here with the content changes `tools/version_port.py diff` lists, and list it in [`NATIVE`].
use crate::{IdMap, Packet, Result, State, Step, wire};

/// A natively translated version: the name a player writes in the config, and its protocol.
pub struct NativeVersion {
    pub name: &'static str,
    pub protocol: i32,
}

pub const NATIVE: &[NativeVersion] = &[NativeVersion {
    name: "26.2",
    protocol: V26_2_PROTOCOL,
}];

pub fn native_by_protocol(protocol: i32) -> Option<&'static NativeVersion> {
    NATIVE.iter().find(|v| v.protocol == protocol)
}

pub fn native_by_name(name: &str) -> Option<&'static NativeVersion> {
    NATIVE.iter().find(|v| v.name == name.trim())
}

/// The steps from the pinned client down to `protocol`, newest first, or `None` when Horizon has
/// no native translation to it. `id_map` gives Horizon's id table for a protocol.
pub fn chain_for(
    protocol: i32,
    punch_id: i32,
    id_map: &dyn Fn(i32) -> Option<Box<dyn IdMap>>,
) -> Option<Vec<Box<dyn Step>>> {
    match protocol {
        V26_2_PROTOCOL => Some(vec![Box::new(V26_2::new(id_map(protocol)?, punch_id))]),
        _ => None,
    }
}

pub const V26_2_PROTOCOL: i32 = 776;
/// ServerboundSwingPacket's id in 26.2's play state (Horizon's adapter tests: "swing, which 26.2
/// has at 63").
pub const V26_2_SWING: i32 = 63;
/// InteractionHand.MAIN_HAND.
const MAIN_HAND: i32 = 0;

/// 26.3 (777) to 26.2 (776). Between them (`docs/version-ports.md`, "Replay: 26.2 -> 26.3"):
/// 26.3 added the clientbound `add_transient_block`, `post_effects` (configuration and play) and
/// `swing_animation`, which a 26.2 server never sends, renumbered 131 packets, and replaced the
/// serverbound `swing` (with its hand) by an empty `punch`. A 26.2 server swings the arm and resets
/// the attack strength on `swing`, as a 26.3 server does on `punch`.
pub struct V26_2 {
    ids: Box<dyn IdMap>,
    punch: i32,
}

impl V26_2 {
    pub fn new(ids: Box<dyn IdMap>, punch: i32) -> Self {
        Self { ids, punch }
    }
}

impl Step for V26_2 {
    fn name(&self) -> &'static str {
        "26.3 -> 26.2"
    }

    fn protocol(&self) -> i32 {
        V26_2_PROTOCOL
    }

    fn serverbound(&mut self, state: State, packet: Packet, out: &mut Vec<Packet>) -> Result<()> {
        if state == State::Play && packet.id == self.punch {
            out.push(Packet::new(V26_2_SWING, wire::varint(MAIN_HAND)));
            return Ok(());
        }
        // A packet 26.2 does not have is not sent: the server would read it as another one.
        if let Some(id) = self.ids.serverbound(state, packet.id) {
            out.push(Packet::new(id, packet.payload));
        }
        Ok(())
    }

    fn clientbound(&mut self, state: State, packet: Packet, out: &mut Vec<Packet>) -> Result<()> {
        if let Some(id) = self.ids.clientbound(state, packet.id) {
            out.push(Packet::new(id, packet.payload));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Inbound, Mode, Outbound, Pipeline,
        chain::{Intention, tests::{IDS, handshake}},
    };

    /// A few rows of the real 26.2 tables (Horizon's adapter tests read them off both reports):
    /// play clientbound keep_alive 44 -> 45 and respawn 82 -> 84; play serverbound resource_pack
    /// 50 -> 49; punch (46 in 26.3) has no 26.2 id.
    struct Rows;
    impl IdMap for Rows {
        fn clientbound(&self, state: State, id: i32) -> Option<i32> {
            match (state, id) {
                (State::Play, 44) => Some(45),
                (State::Play, 82) => Some(84),
                (State::Play, 141) => None,
                (_, id) => Some(id),
            }
        }
        fn serverbound(&self, state: State, id: i32) -> Option<i32> {
            match (state, id) {
                (State::Play, 50) => Some(49),
                (State::Play, 46) => None,
                (State::Play, 99) => None,
                (_, id) => Some(id),
            }
        }
    }

    const PUNCH: i32 = 46;

    fn play_pipeline() -> Pipeline {
        let steps = chain_for(V26_2_PROTOCOL, PUNCH, &|_| Some(Box::new(Rows) as Box<dyn IdMap>)).unwrap();
        let mut p = Pipeline::new(Mode::Native { steps }, IDS);
        p.outbound(&handshake(777, "old.example", 2)).unwrap();
        p.outbound(&Packet::new(IDS.login_acknowledged, vec![]).to_body()).unwrap();
        p.outbound(&Packet::new(IDS.finish_configuration, vec![]).to_body()).unwrap();
        assert_eq!(p.state(), State::Play);
        p
    }

    #[test]
    fn the_handshake_states_26_2s_protocol() {
        let steps = chain_for(V26_2_PROTOCOL, PUNCH, &|_| Some(Box::new(Rows) as Box<dyn IdMap>)).unwrap();
        let mut p = Pipeline::new(Mode::Native { steps }, IDS);
        let Outbound::Replace(bodies) = p.outbound(&handshake(777, "old.example", 2)).unwrap() else {
            panic!()
        };
        let sent = Packet::from_body(&bodies[0]).unwrap();
        assert_eq!(Intention::read(&sent.payload).unwrap().protocol, 776);
    }

    #[test]
    fn punch_becomes_a_main_hand_swing() {
        let mut p = play_pipeline();
        assert_eq!(
            p.outbound(&Packet::new(PUNCH, vec![]).to_body()).unwrap(),
            Outbound::Replace(vec![Packet::new(V26_2_SWING, vec![0]).to_body()])
        );
    }

    #[test]
    fn packets_are_renumbered_both_ways_and_unknown_ones_dropped() {
        let mut p = play_pipeline();
        assert_eq!(
            p.outbound(&Packet::new(50, vec![7]).to_body()).unwrap(),
            Outbound::Replace(vec![Packet::new(49, vec![7]).to_body()])
        );
        assert_eq!(
            p.outbound(&Packet::new(99, vec![7]).to_body()).unwrap(),
            Outbound::Replace(vec![])
        );
        let Inbound::Packets(keep_alive) = p.inbound(44, vec![1, 2]).unwrap() else { panic!() };
        assert_eq!(keep_alive, vec![Packet::new(45, vec![1, 2])]);
        let Inbound::Packets(none) = p.inbound(141, vec![]).unwrap() else { panic!() };
        assert!(none.is_empty());
    }

    #[test]
    fn swing_outside_play_is_not_rewritten() {
        let steps = chain_for(V26_2_PROTOCOL, PUNCH, &|_| Some(Box::new(Rows) as Box<dyn IdMap>)).unwrap();
        let mut p = Pipeline::new(Mode::Native { steps }, IDS);
        p.outbound(&handshake(777, "old.example", 2)).unwrap();
        // Login packet 46 does not exist, but it must not turn into a swing either.
        assert_eq!(p.outbound(&Packet::new(PUNCH, vec![]).to_body()).unwrap(), Outbound::Pass);
    }

    #[test]
    fn only_known_versions_have_a_chain() {
        assert!(chain_for(775, PUNCH, &|_| Some(Box::new(Rows) as Box<dyn IdMap>)).is_none());
        assert!(chain_for(V26_2_PROTOCOL, PUNCH, &|_| None).is_none());
        assert_eq!(native_by_name(" 26.2 ").unwrap().protocol, 776);
        assert_eq!(native_by_protocol(776).unwrap().name, "26.2");
    }
}
