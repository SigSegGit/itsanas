//! Which nodes hold which chunks.
//!
//! Every node has to reach the same answer, independently, without asking
//! anyone. If the laptop thinks a chunk lives on the Pi and the VM thinks it
//! lives on Carol's machine, the chunk quietly ends up with one replica instead
//! of three and nobody notices until something is lost.
//!
//! # Rendezvous hashing, not a hash ring
//!
//! For each chunk, every node computes a score for every candidate and takes
//! the top *n*. No shared ring state, no coordinated resharding, and — the
//! property that matters — **removing a node moves only that node's share**.
//! With naive modulo hashing, changing the node count remaps almost everything,
//! which at ITSaNAS scale means re-uploading the entire network.
//!
//! # Why there are no floating-point numbers in here
//!
//! `docs/DESIGN.md` originally specified the textbook weighted formula:
//!
//! ```text
//! score(node, chunk) = weight(node) / -ln(uniform_hash(node_id ‖ chunk_id))
//! ```
//!
//! That is correct mathematics and a latent bug. `f64::ln` is implemented by
//! the platform's libm, and libm implementations are not required to agree in
//! the last unit in the last place. A Raspberry Pi and a Windows laptop can
//! compute very slightly different scores for the same chunk. Almost always
//! that changes nothing — but when two candidates land within one ulp of each
//! other, the two machines disagree about where a chunk belongs, silently and
//! permanently. There is no error, no log line, and no way to notice except by
//! eventually losing data.
//!
//! So the weighting is done with integers instead. Each node is given a number
//! of *slots* proportional to its pledged capacity, and its score for a chunk is
//! the **highest** hash across its slots. The probability that a node holds the
//! single highest hash among all slots in the swarm is exactly its share of the
//! slots — which is the same proportional-to-capacity property the float
//! formula was for, computed in a way that every machine agrees on bit for bit.

pub mod nodeset;
pub mod repair;

pub use nodeset::{MAX_SLOTS, NodeSet, PlacementError, StorageNode};
pub use repair::{AtRisk, Census, DEFAULT_REPLICATION_FLOOR, Push, RepairPlan, plan};
