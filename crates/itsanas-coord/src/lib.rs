//! The coordinator: a notice board that holds no keys and no data.
pub mod accounting;
pub mod claim;
pub mod directory;
pub mod error;

pub use accounting::{Assessment, DeviceContribution, MemberState, Standing, assess};
pub use claim::{NodeClaim, Presence, SignedClaim, SignedPresence};
pub use directory::{Account, Directory, Registration, SignedRegistration};
pub use error::{CoordError, Result};
