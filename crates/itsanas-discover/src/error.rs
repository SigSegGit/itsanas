//! Why an announcement was refused.
//!
//! The variants are separate on purpose. A node that never finds its
//! neighbours needs to be told the difference between "nothing is arriving"
//! (a firewall, or broadcast being dropped by the router), "something is
//! arriving but it is not ours" (a port collision), and "ours is arriving and
//! failing to verify" (a build mismatch, or somebody playing games). Those are
//! three completely different things to go and fix, and a single opaque
//! `InvalidPacket` would hide which one is happening.

use std::io;

use thiserror::Error;

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, DiscoverError>;

/// Everything that can go wrong finding neighbours.
#[derive(Debug, Error)]
pub enum DiscoverError {
    /// Not the fixed announcement size, so no field was read at all.
    #[error("a discovery packet must be exactly {expected} bytes, got {got}")]
    WrongLength {
        /// The size actually received.
        got: usize,
        /// The only size this version accepts.
        expected: usize,
    },

    /// Something else is using this port. Discarded in one comparison.
    #[error("not an ITSaNAS discovery packet")]
    NotOurs,

    /// A future announcement format. Refused rather than reinterpreted.
    #[error("discovery packet version {got} is not understood; this build speaks version 1")]
    UnknownVersion {
        /// The version claimed by the packet.
        got: u8,
    },

    /// The signature did not verify against the claimed device.
    #[error("discovery packet signature does not match the device it claims to be")]
    BadSignature,

    /// An announcement pointing at a port nothing can listen on.
    #[error("discovery packet advertises port 0, which nothing can serve")]
    NoPort,

    /// The socket itself failed.
    #[error("local network discovery: {0}")]
    Io(#[from] io::Error),
}

impl DiscoverError {
    /// Whether this packet was addressed to us at all.
    ///
    /// A listener uses this to keep quiet about the ordinary case of sharing a
    /// port with unrelated traffic, while still reporting a packet that was
    /// meant for ITSaNAS and failed. Logging every foreign datagram would turn
    /// a busy network into a log flood, and a flooded log is not read.
    #[must_use]
    pub const fn is_foreign_traffic(&self) -> bool {
        matches!(self, Self::NotOurs | Self::WrongLength { .. })
    }
}
