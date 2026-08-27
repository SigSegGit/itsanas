//! The peer protocol: how two ITSaNAS nodes talk.
//!
//! Split deliberately into three layers, so that the part which parses hostile
//! input is small, synchronous, and testable without a network:
//!
//! * [`wire`] — framing. Every byte here comes from a stranger's computer.
//! * [`protocol`] — the messages themselves, and storage-challenge proofs.
//! * [`service`] — what a node does when asked something, given its store and
//!   its vault. Pure request-in, response-out; no sockets.
//!
//! A transport sits underneath and does nothing but move frames. That is why
//! the protocol can be tested exhaustively — including every hostile input —
//! before any socket exists, and why swapping the transport later changes
//! nothing above it.
//!
//! # What a peer is trusted with
//!
//! Nothing, and the layering is what keeps that true. [`service::PeerService`]
//! answers questions using a [`Store`](itsanas_store::Store) for the node's own
//! data and a [`Vault`](itsanas_store::Vault) for other people's. The vault
//! holds no keys, so there is no code path from "a peer asked me something" to
//! "I decrypted something of theirs" — not because it is forbidden, but because
//! the key is not reachable from there.

pub mod error;
pub mod protocol;
pub mod service;
pub mod session;
pub mod transport;
pub mod wire;

pub use error::{NetError, Result};
pub use protocol::{Head, PROTOCOL_VERSION, Request, Response, challenge_holds, challenge_proof};
pub use service::{PeerService, Pledge};
pub use session::{PushReport, RoundReport, pull, push, round};
pub use transport::{Exposure, IO_TIMEOUT, PeerClient, PeerServer};
pub use wire::{FrameReader, MAX_FRAME_LEN, WIRE_VERSION};
