//! Finding other ITSaNAS nodes on the same network, with no server involved.
//!
//! This is the part of discovery that needs nothing central at all. Machines in
//! one house — a laptop, a Raspberry Pi, a virtual machine on the same bridge —
//! announce themselves on the local network and find each other with no
//! configuration, no addresses typed, and no coordinator reachable.
//!
//! # Where this sits in the decentralisation argument
//!
//! [`docs/DESIGN.md`] §8 works through which of the coordinator's jobs actually
//! need it. Local discovery is the clearest case of one that does not, and it
//! covers the entire fleet this project was started for. The coordinator's
//! remaining discovery job is finding a node on a *different* network, which is
//! the part a DHT would eventually take over — and which is not worth building
//! at this size, because a DHT draws its Sybil resistance from having thousands
//! of participants to dilute an attacker among.
//!
//! # The security position, in one paragraph
//!
//! An announcement proves that the sender holds the private key for the device
//! id it claims, and nothing else. It does not prove the owner it names, it does
//! not grant access, and it is not a substitute for authentication: everything
//! discovered here is then dialled through `itsanas-tls`, which pins the
//! expected device and refuses a different one. Discovery answers "who might be
//! worth talking to"; it never answers "who is this".
//!
//! # Layering
//!
//! Depends on `itsanas-crypto` and nothing else in the project. It cannot reach
//! the store, the sync engine or the network protocol, which is what keeps it
//! reviewable on its own — it is, after all, the one component that parses
//! unsolicited packets from strangers.
//!
//! [`docs/DESIGN.md`]: https://github.com/SigSeg/itsanas/blob/main/docs/DESIGN.md

pub mod beacon;
pub mod error;
pub mod lan;
pub mod neighbours;

pub use beacon::{Announcement, BEACON_DOMAIN, BEACON_LEN, BEACON_VERSION};
pub use error::{DiscoverError, Result};
pub use lan::{ANNOUNCE_INTERVAL, DEFAULT_PORT, EXPIRY, Lan, now_unix};
pub use neighbours::{Candidate, DEFAULT_CAPACITY, Heard, Neighbour, Neighbours};
