//! A node on disk, and the one sync round that knows what a device keeps.
//!
//! # Why this is a library and not part of the command line
//!
//! Everything here is what a *shell* needs before it can do anything: unseal
//! the keystore with a passphrase, read the configuration beside it, open the
//! store and the vault, and run a round that honours what this device was told
//! to hold. A terminal needs it, and so does an Android application, a tray
//! icon, or a filesystem driver.
//!
//! It lived inside the command-line binary until the second shell needed it.
//! Rewriting the keystore handling for Android would have meant two
//! implementations of the most security-sensitive glue in the project, drifting
//! apart from the day the second one was written — the passphrase, the key
//! derivation, the refusal of published test identities, the "a node already
//! exists here" guard. One copy, used by both.

#![forbid(unsafe_code)]

pub mod config;
pub mod error;
pub mod keeping;
pub mod node;

pub use config::Config;
pub use error::{NodeError, Result};
pub use keeping::{KeepingReport, round};
pub use node::{Node, SNAPSHOT};
