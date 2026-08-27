//! Mutually authenticated TLS between devices, with no certificate authority.
pub mod auth;
pub mod error;
pub mod session;

pub use auth::{AUTH_DOMAIN, AuthHello, EXPORTER_LEN, check, prove};
pub use error::{Result, TlsError};
pub use session::{Authenticated, Identity, accept, connect};
