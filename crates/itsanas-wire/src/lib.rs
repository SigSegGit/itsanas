//! Framing, shared by every protocol in ITSaNAS.
//!
//! Extracted into its own crate for a reason that is about safety rather than
//! tidiness: the coordinator and the peer protocol both need framing, and the
//! coordinator must not be able to reach the sync engine or the store. If it
//! got its framing from `itsanas-net`, it would link both — and "the
//! coordinator cannot touch your data" would rest on nobody having written the
//! call rather than on the call being impossible to write.
//!
//! Everything this crate parses comes from a stranger's computer. It is
//! deliberately boring: fixed header, explicit length, hard ceiling, no
//! recursion, and no allocation sized by a number the peer chose until that
//! number has been checked.

mod stream;
mod wire;

pub use stream::{Connection, StreamError};
pub use wire::{
    FrameReader, HEADER_LEN, MAX_FRAME_LEN, WIRE_VERSION, WireError, decode, encode, payload_len,
};

/// Result type for framing operations.
pub type Result<T> = std::result::Result<T, WireError>;
