//! Backward compatibility for Horizon: join servers that speak an older protocol than the pinned
//! client (26.3, protocol 777).
//!
//! Two layers, as in the ViaFabricPlus architecture:
//!
//! - [`chain`]: a per-connection [`Pipeline`] that tracks the connection state from the packets the
//!   client sends and runs them through version [`Step`]s, newest first (ViaVersion's protocol
//!   chain). Clientbound packets go through the same steps in reverse. [`steps`] holds the steps
//!   Horizon translates natively.
//! - [`viaproxy`]: for versions with no native step, the connection goes to a local ViaProxy, which
//!   carries the whole ViaVersion / ViaBackwards / ViaRewind / ViaLegacy stack (Classic to 26.3).
//!   The handshake names the real server and version (ViaProxy's internal wildcard domain), and a
//!   ViaProxy plugin asks Horizon to do the online-mode session join, so the account's token never
//!   leaves Horizon.
//!
//! [`route`] decides between them from the server's status ping and the player's overrides.
//!
//! This crate has no dependency on Horizon: the glue compiled into `horizon-protocol` and
//! `horizon-client-lib` hands it Horizon's packet ids and does the I/O.
pub mod chain;
pub mod route;
pub mod steps;
pub mod viaproxy;
pub mod wire;

pub use chain::{ClientIds, IdMap, Inbound, Joiner, Mode, Outbound, Pipeline, Step};

/// A connection state, as both sides track it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum State {
    Handshake,
    Status,
    Login,
    Configuration,
    Play,
}

/// One packet: its id in the protocol of the side that holds it, and its payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub id: i32,
    pub payload: Vec<u8>,
}

impl Packet {
    pub fn new(id: i32, payload: Vec<u8>) -> Self {
        Self { id, payload }
    }

    /// A packet body as the connection frames it: VarInt id, then the payload.
    pub fn from_body(body: &[u8]) -> Result<Self> {
        let mut r = wire::Reader::new(body);
        let id = r.varint()?;
        Ok(Self::new(id, r.rest().to_vec()))
    }

    pub fn to_body(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.payload.len() + 5);
        wire::write_varint(&mut out, self.id);
        out.extend_from_slice(&self.payload);
        out
    }
}

/// A translation failure. The glue ends the connection with it, as a decode error would.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(pub String);

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;
