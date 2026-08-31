//! The coordinator: a notice board that holds no keys and no data.
pub mod accounting;
pub mod claim;
pub mod directory;
pub mod error;
pub mod invitation;
pub mod protocol;
pub mod server;
pub mod service;

pub use accounting::{Assessment, DeviceContribution, MemberState, Standing, assess};
pub use claim::{NodeClaim, Presence, SignedClaim, SignedPresence};
pub use directory::{
    Account, Admission, Directory, LodgedInvitation, Registration, SignedRegistration,
};
pub use error::{CoordError, Result};
pub use invitation::{DEFAULT_VALIDITY, Invitation, SECRET_LEN, Secret, SignedInvitation, code_id};
pub use protocol::{COORD_VERSION, Request, Response};
pub use server::{CoordClient, CoordServer, DEFAULT_COORD_PORT};
pub use service::{CoordService, EscrowLimiter};
