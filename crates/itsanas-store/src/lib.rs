//! Local storage for ITSaNAS: chunking, sealed blobs, the index and the
//! operation log.
//!
//! This crate is the layer directly above [`itsanas_crypto`]. It turns a file
//! into content-defined chunks, seals each one, writes it to a
//! content-addressed directory, records what it did in a transactional index,
//! and appends a signed entry to the device's operation log so that peers who
//! were asleep can catch up later.
//!
//! # The shape of it
//!
//! ```text
//! write_file(path, bytes)
//!   ├── chunker    split on content, not offsets, so an edit is cheap
//!   ├── crypto     blind the address, seal the bytes
//!   ├── blob       write ciphertext to <root>/blobs/ab/cd/…
//!   ├── index      path → chunk list, chunk → refcount   (one transaction)
//!   └── oplog      pending entry, later sealed into a signed segment
//! ```
//!
//! # What a host learns
//!
//! Nothing that matters. A host receives sealed chunks addressed by blinded
//! identifiers and sealed log segments in signed envelopes. It can verify that
//! a segment is authentic and it can see roughly how much data exists and when
//! it moved. It cannot read a filename, a path, or a byte of content.
//!
//! # Example
//!
//! ```
//! use itsanas_crypto::{DeviceKeys, MasterSecret, UserKeys};
//! use itsanas_store::Store;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let directory = tempfile::tempdir()?;
//! let user = UserKeys::derive(&MasterSecret::from_bytes([7; 32]));
//! let device = DeviceKeys::generate()?;
//!
//! let store = Store::open(directory.path(), user, device)?;
//! store.write_file("notes/todo.txt", b"buy milk")?;
//!
//! assert_eq!(store.read_file("notes/todo.txt")?.unwrap(), b"buy milk");
//!
//! // Announce the change to peers.
//! let segment = store.flush_segment()?.expect("one pending write");
//! segment.verify_signature()?;
//! # Ok(())
//! # }
//! ```

pub mod blob;
pub mod chunker;
pub mod error;
pub mod holders;
pub mod index;
pub mod local;
pub mod oplog;
pub mod path;
pub mod store;
pub mod vault;
pub mod version;

pub use blob::BlobStore;
pub use chunker::split_stream;
pub use chunker::{Chunk, ChunkerConfig, Chunks};
pub use error::{Result, StoreError};
pub use holders::{AtRisk, Holder};
pub use index::Index;
pub use local::LocalState;
pub use oplog::{
    FileEntry, LogEntry, Operation, SegmentBody, SegmentEnvelope, Tombstone, validate_chain,
};
pub use store::{GcReport, IntegrityReport, Store, StoreStats};
pub use vault::{Vault, VaultStats};
pub use version::{CausalOrder, VersionVector};
