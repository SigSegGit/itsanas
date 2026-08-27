//! Making several devices agree about a set of files.
//!
//! This crate is the answer to the question the whole project exists for: your
//! Pi wrote a file at 3am, your laptop was shut, and by the time you open the
//! laptop the Pi is off. How does the laptop get the file?
//!
//! Not by talking to the Pi — it cannot. The Pi published a signed, sealed
//! operation log to whichever hosts were online, and those hosts hold it
//! without being able to read it. The laptop fetches that log from a stranger's
//! machine, verifies it, replays it, and pulls the chunks it names. The Pi never
//! has to come back.
//!
//! # What this crate is and is not responsible for
//!
//! It decides **what should exist**: whose version of a file wins, which edits
//! were concurrent, whether a delete beat an edit. It does not decide **where
//! bytes come from** — that is [`ChunkSource`], which the simulation implements
//! over memory and M4 will implement over QUIC. The split is what lets
//! convergence be tested exhaustively without a network.
//!
//! # The property that matters
//!
//! **Convergence.** Any set of devices that eventually exchange everything must
//! end up with byte-identical file trees, no matter what order things arrived
//! in, how often the network split, or which device died halfway through. That
//! is not a property you can establish by reasoning about it once and moving on;
//! it is established by [`sim`], which runs adversarial multi-device scenarios
//! deterministically and asserts convergence in every one.
//!
//! # Example
//!
//! ```
//! use itsanas_sync::sim::Swarm;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut swarm = Swarm::new(3)?;
//!
//! // The Pi writes while the laptop is asleep, then goes offline for good.
//! swarm.device(1).write("notes.txt", b"written on the Pi")?;
//! swarm.device(1).publish()?;
//! swarm.set_online(1, false);
//!
//! // The laptop wakes and catches up from a host that cannot read the data.
//! swarm.settle()?;
//!
//! assert_eq!(swarm.device(0).read("notes.txt")?.unwrap(), b"written on the Pi");
//! # Ok(())
//! # }
//! ```

pub mod conflict;
pub mod engine;
pub mod error;
#[cfg(feature = "simulation")]
pub mod sim;
pub mod source;

pub use conflict::{CONFLICT_MARKER, sibling_path, wins_original_path};
pub use engine::{Applied, Divergence, Outcome, SyncReport, apply_segments, diff};
pub use error::{Result, SyncError};
pub use source::{ChunkSource, EmptySource};
