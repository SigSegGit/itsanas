//! The store: the whole of a device's local state, behind one type.
//!
//! A [`Store`] owns a user's identity, a device key, a blob directory and an
//! index, and it is the only place in the codebase where plaintext and
//! ciphertext meet. Everything above it deals in sealed bytes.
//!
//! # What this store does and does not hold
//!
//! It holds **the owner's own data**: their chunks, their index, their log.
//! Chunks this device hosts *for other people* are a different concern and will
//! live in a separate area, because they are opaque bytes with no index, no
//! plaintext and no key. Keeping the two apart means there is no code path in
//! which hosting could accidentally decrypt, and no path in which garbage
//! collection of your own data could reach a peer's.

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
};

use itsanas_crypto::{ChunkId, DeviceKeys, UserId, UserKeys, is_published_test_identity};

use crate::{
    blob::BlobStore,
    chunker::ChunkerConfig,
    error::{Result, StoreError},
    holders::HolderEvidence,
    index::Index,
    oplog::LogEntry,
};

mod budget;
mod housekeeping;
mod ledger;
mod log;
mod peers;
mod placement;
mod read;
mod write;

/// Seal purpose for file chunks.
///
/// Re-exported from the crypto crate so there is exactly one definition: a
/// second copy that drifted would derive different keys and silently produce
/// chunks nothing else can read.
pub use itsanas_crypto::seal::CHUNK_PURPOSE as CHUNK_SEAL_PURPOSE;

/// Where [`Store::note_twin`] records a twin, next to the store: read by a
/// status that cannot open the store while the daemon holds it.
pub const TWIN: &str = "twin";

/// A device's local state.
#[derive(Debug)]
pub struct Store {
    root: PathBuf,
    blobs: BlobStore,
    index: Index,
    chunker: ChunkerConfig,
    user: UserKeys,
    device: DeviceKeys,
    /// Serialises the read-modify-write cycle that produces a new version.
    ///
    /// Every mutating method takes `&self`, so two threads could otherwise read
    /// the same base version, both stamp a successor, and produce a lost
    /// update that looks causally valid — the worst kind, because nothing
    /// downstream would flag it as a conflict.
    write_lock: Mutex<()>,
    /// How much the account may hold, or `None` for no bound at all.
    budget: Mutex<Option<WriteBudget>>,
}

/// How much an account may hold, as far as a write on this device can tell.
///
/// # Why the store is told rather than working it out
///
/// The rule -- what a pledge earns, under which split, with which allowance --
/// lives in `itsanas-coord` and the pledge in the node's configuration, and
/// this crate depends on neither. And the account's size is not the store's to
/// know either: files this device has not downloaded exist only in the vault's
/// copy of other devices' logs. So the node, which holds all three, works the
/// numbers out and hands them over, and the store applies them on the one path
/// every write takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WriteBudget {
    /// Bytes the whole account may hold.
    pub allowed: u64,
    /// Bytes of the account whose content is not on this device.
    ///
    /// Zero understates the account on a device that holds only part of it,
    /// which lets that device write more than it should; it never refuses a
    /// write that fits. A file that is here *and* counted in this figure is
    /// counted twice, which errs the other way, and only for its size.
    pub elsewhere: u64,
    /// Most bytes of the account's files this device may hold locally, if
    /// known: what it held when the budget was set plus the free space less
    /// what this machine's pledge still owes other people. An absolute
    /// ceiling rather than a per-write room, so a pass that imports a hundred
    /// files is bounded by their sum, not each by the same snapshot; and an
    /// edit is charged its growth, since the file it replaces is not counted.
    /// `None` when the free space could not be read -- no bound rather than a
    /// refusal of everything. Measured in plaintext bytes, which a sealed
    /// chunk exceeds by its tag and deduplication undercuts; the margin is a
    /// few bytes a chunk, and the owed pledge is the real reserve.
    pub local_ceiling: Option<u64>,
    /// Where a pulled file's second copy lands: see [`FolderCopy`].
    pub folder_copy: FolderCopy,
}

/// The synced folder's copy of a pulled file, which [`Store::pull_room`]
/// must charge too.
///
/// A pull writes a file twice when a folder is configured: its chunks into the
/// store, then the plaintext into the folder (`export`). The ceiling counted
/// the first only, so a pull of X under a room of X put 2X on the disk -- the
/// second X out of space the pledge owes other people.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FolderCopy {
    /// No synced folder: a pull writes the store's copy only.
    #[default]
    None,
    /// The folder shares the store's volume: each byte pulled lands twice on
    /// the disk the ceiling measures. `local_then` is the store's local bytes
    /// when the budget was set, so the bytes pulled since can be charged
    /// twice too.
    SameVolume { local_then: u64 },
    /// The folder is on another volume, which had `room` bytes free when the
    /// budget was set: the folder's copy is charged there.
    OtherVolume { local_then: u64, room: u64 },
}

/// What one garbage-collection pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    /// Blobs deleted from disk.
    pub blobs_removed: usize,
    /// Bytes reclaimed.
    pub bytes_reclaimed: u64,
    /// Chunks that were unreferenced but still inside the grace period.
    pub retained_in_grace: usize,
    /// Chunks the index had queued whose blob was already gone.
    pub already_absent: usize,
}

/// What an integrity check found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IntegrityReport {
    /// Files checked.
    pub files_checked: usize,
    /// Chunks referenced by the index but missing from disk, with the path that
    /// wanted them.
    pub missing_chunks: Vec<(String, ChunkId)>,
    /// Blobs on disk that the index does not account for at all. These are
    /// leaked, not dangerous — usually a crash between writing a blob and
    /// committing its index entry.
    pub orphan_blobs: Vec<ChunkId>,
    /// Files whose reassembled content did not match its recorded hash.
    pub corrupt_files: Vec<String>,
    /// Whether the operation-log chain is intact.
    pub chain_intact: bool,
}

impl IntegrityReport {
    /// Whether the store is fully healthy.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.missing_chunks.is_empty() && self.corrupt_files.is_empty() && self.chain_intact
    }
}

/// What releasing a file's local content freed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReleaseReport {
    /// Chunks deleted from this device.
    pub chunks: usize,
    /// Bytes reclaimed.
    pub bytes: u64,
}

/// What [`Store::release`] did, or why it would not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Release {
    /// The content is gone from this device and the file is still in the
    /// account.
    Gone(ReleaseReport),
    /// This device was not holding it, so there was nothing to release.
    NotHere,
    /// Refused: fewer than [`SAFE_TO_RELEASE`](crate::holders::SAFE_TO_RELEASE) other live machines are
    /// known to hold this chunk, so letting go of it here would take the number
    /// of copies below the floor this project promises.
    NotSafeYet {
        /// The chunk that stopped it.
        chunk: ChunkId,
        /// What this node could say about who else holds it.
        evidence: HolderEvidence,
    },
}

/// Coarse size and count statistics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreStats {
    pub files: u64,
    pub live_chunks: usize,
    pub pending_collection: usize,
    /// How many (chunk, device) acknowledgements the placement ledger holds.
    ///
    /// Reported so an operator can see that replication is being recorded at
    /// all. Zero with a non-empty store means every chunk exists only here.
    pub holder_records: u64,
    pub bytes_on_disk: u64,
    pub segments: u64,
    pub unsealed_entries: usize,
}

impl Store {
    /// Open (creating if needed) a store at `root`.
    ///
    /// Refuses the published test identities. Their recovery phrases are
    /// printed in `docs/TEST-USERS.md`, so anyone at all can derive their keys;
    /// a store holding real data under one of them offers no protection
    /// whatsoever.
    /// Whether another process holds this store's index.
    ///
    /// See [`Index::is_locked`]: this is the question a command asks before it
    /// asks somebody for a passphrase.
    #[must_use]
    pub fn is_locked(root: impl AsRef<Path>) -> bool {
        Index::is_locked(root.as_ref().join(crate::index::INDEX_FILE))
    }

    pub fn open(root: impl AsRef<Path>, user: UserKeys, device: DeviceKeys) -> Result<Self> {
        Self::open_inner(root, user, device, ChunkerConfig::default(), false)
    }

    /// Open with a non-default chunker, for tuning experiments.
    pub fn open_with_chunker(
        root: impl AsRef<Path>,
        user: UserKeys,
        device: DeviceKeys,
        chunker: ChunkerConfig,
    ) -> Result<Self> {
        Self::open_inner(root, user, device, chunker, false)
    }

    /// Open a store for one of the published test identities.
    ///
    /// Exists so the fixture users in `itsanas-testkit` can be exercised
    /// end to end. Named to be impossible to reach for by accident, and never
    /// called from a code path that handles a real user's data.
    pub fn open_for_testing(
        root: impl AsRef<Path>,
        user: UserKeys,
        device: DeviceKeys,
        chunker: ChunkerConfig,
    ) -> Result<Self> {
        Self::open_inner(root, user, device, chunker, true)
    }

    fn open_inner(
        root: impl AsRef<Path>,
        user: UserKeys,
        device: DeviceKeys,
        chunker: ChunkerConfig,
        allow_published_test_identity: bool,
    ) -> Result<Self> {
        if !allow_published_test_identity && is_published_test_identity(&user.user_id()) {
            return Err(StoreError::PublishedTestIdentity(user.user_id().short()));
        }

        let root = root.as_ref().to_owned();
        std::fs::create_dir_all(&root).map_err(|error| StoreError::io(root.clone(), error))?;

        let blobs = BlobStore::open(&root)?;
        // A previous run may have died mid-write. Clearing staging on open is
        // the only moment we can be sure no write is in flight.
        blobs.sweep_staging()?;

        let index = Index::open(
            root.join(crate::index::INDEX_FILE),
            user.audit_order_key().clone(),
        )?;

        // Every store cuts under its account's key: its chunk sizes must not
        // let a host recognise a file it has a copy of (`chunker/keyed.rs`).
        let chunker = chunker.keyed(&user.chunking_key());
        Ok(Self {
            root,
            blobs,
            index,
            chunker,
            user,
            device,
            write_lock: Mutex::new(()),
            budget: Mutex::new(None),
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn owner(&self) -> UserId {
        self.user.user_id()
    }

    #[must_use]
    pub const fn chunker(&self) -> &ChunkerConfig {
        &self.chunker
    }

    /// Bytes of the account's files whose content is on this device.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn local_bytes(&self) -> Result<u64> {
        self.index.local_bytes()
    }

    /// Coarse statistics, cheap enough for a status command.
    pub fn stats(&self) -> Result<StoreStats> {
        Ok(StoreStats {
            files: self.index.file_count()?,
            live_chunks: self.index.referenced_chunks()?.len(),
            pending_collection: self.index.unreferenced_chunks()?.len(),
            holder_records: self.index.holder_records()?,
            bytes_on_disk: self.blobs.total_bytes()?,
            segments: self.index.chain_length()?,
            unsealed_entries: self.index.pending_entries()?.len(),
        })
    }

    /// Operations this device has written and not yet sealed into a segment.
    ///
    /// Read by [`catalogue`](mod@crate::catalogue), which would otherwise lose sight
    /// of a file written and released before the next flush.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn unsealed_entries(&self) -> Result<Vec<LogEntry>> {
        self.index.pending_entries()
    }

    /// The blob store, for the network layer to serve sealed bytes from.
    #[must_use]
    pub const fn blobs(&self) -> &BlobStore {
        &self.blobs
    }
}
