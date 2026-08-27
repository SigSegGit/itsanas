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
//! # Where to start reading
//!
//! [`engine`] holds the decision table — what happens for every combination of
//! local state and incoming claim. [`sim`] holds a worked example and the
//! adversarial scenarios. The example lives there rather than here because
//! `sim` is behind the `simulation` feature, and a crate-level example using it
//! would fail to compile for anyone who turned that feature off.

pub mod conflict;
pub mod engine;
pub mod error;
#[cfg(feature = "simulation")]
pub mod sim;
pub mod source;

pub use conflict::{CONFLICT_MARKER, sibling_path, wins_original_path, with_marker};
pub use engine::{Applied, Divergence, Outcome, SyncReport, apply_segments, diff};
pub use error::{Result, SyncError};
pub use source::{ChunkSource, EmptySource};
