//! Mutually authenticated TLS between devices, with no certificate authority.
pub mod auth;
pub mod error;
pub mod limits;
pub mod reach;
pub mod session;

pub use auth::{AUTH_DOMAIN, AuthHello, EXPORTER_LEN, check, prove};
pub use error::{Result, TlsError};
pub use session::{Authenticated, Bounded, Identity, accept, accept_within, connect};
