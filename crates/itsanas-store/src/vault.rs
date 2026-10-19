//! Storage for other people's data.
//!
//! This is the half of the bargain you keep. A [`Vault`] holds sealed chunks and
//! signed log segments belonging to *other users*, and it is built so that
//! reading them is not merely forbidden but structurally impossible: the type
//! has no key material of any kind, takes none in any constructor, and calls
//! nothing that decrypts. If a future change tried to make the vault read its
//! contents, it would have to acquire a key from somewhere first, and there is
//! nowhere to get one.
//!
//! Kept deliberately separate from [`Store`](crate::store::Store), which holds
//! *your* data and does have keys. One directory, one database, one set of
//! rules each. The alternative — a single store with an `owner` column and a
//! branch that decides whether to decrypt — puts a single `if` between a
//! stranger's ciphertext and your key material.
//!
//! # What the vault checks before accepting something
//!
//! **Segments:** the envelope signature, always. A host that stored unverified
//! envelopes would become a convenient way to flood a user's peers with garbage
//! attributed to one of their devices.
//!
//! **Chunks:** nothing, and it cannot. A sealed chunk is indistinguishable from
//! random bytes to anyone without the key, so a host has no way to tell a real
//! chunk from a forgery. This is not a gap that can be closed at this layer: the
//! *owner* detects the substitution when the chunk fails to open, which is
//! exactly where the check belongs. What the vault can do is refuse to accept
//! more than it agreed to store, which is a quota question rather than an
//! authenticity one.
//!
//! # What the vault removes, and for whom
//!
//! Nothing on a peer's say-so. An owner may *mark* chunks it has copies of to
//! spare ([`Vault::mark_evictable`]); the vault removes marked chunks only when
//! asked to make room for that same owner ([`Vault::evict_marked`]), oldest
//! mark first, and never one owner's chunks to make room for another's. Who
//! may mark is the host's bound to decide, not the vault's: the vault only
//! caps how many marks an owner makes a day, whoever made them.

use std::path::{Path, PathBuf};

use itsanas_crypto::{ChunkId, DeviceId, ObjectId, UserId};
use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};

use crate::{
    blob::BlobStore,
    error::{Result, StoreError},
    oplog::SegmentEnvelope,
};

/// `owner ‖ device ‖ position` → postcard-encoded [`SegmentEnvelope`].
///
/// A composite big-endian key rather than a tuple, so a range scan over one
/// `owner ‖ device` prefix returns that device's chain in chain order.
const SEGMENTS: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("vault_segments");
/// `owner ‖ device` → how many segments are held for it.
const CHAIN_LENGTHS: TableDefinition<'_, &[u8], u64> = TableDefinition::new("vault_chain_lengths");

/// Bytes of log segment held per chain, so the pledge can count them.
///
/// # Why this exists, and what it was worth
///
/// `would_exceed_pledge` read `Vault::stats().bytes`, and `bytes` was the sum
/// of the *chunk* blobs alone. Segments live in `vault_segments` and counted
/// for nothing, so `held` stayed at zero however many arrived: every
/// `StoreSegment` passed the quota on any host whose pledge exceeded one
/// segment, for ever.
///
/// A stranger needed no account, no invitation and no coordinator -- a
/// throwaway Ed25519 key completes the handshake, and a self-signed envelope
/// with a random body is indistinguishable from a real one because nobody can
/// decrypt either. Roughly 1,280 requests at just under the 8 MiB frame limit
/// puts 10 GiB on the disk, and there is no upper bound and no segment-removal
/// API. The bytes were also invisible in `itsanas status`, which reads the same
/// field, so the operator watched a disk fill with no cause.
///
/// A running total rather than a walk, for the same reason the chunk index is
/// one: this is read on the path of every stored object.
const CHAIN_BYTES: TableDefinition<'_, &[u8], u64> = TableDefinition::new("vault_chain_bytes");
/// `owner ‖ device` → the most recent segment id held.
const HEADS: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("vault_heads");

/// Which chunks this vault holds, keyed by `owner ‖ chunk`.
///
/// The blob directories are the authority on what is on disk; this is an index
/// over them, and it exists for one reason: **order**. Reconciling with an owner
/// means hashing the ids this vault holds for them in ascending order, and the
/// only other way to get that list is `BlobStore::addresses`, a recursive walk
/// whose own documentation says it is never for a hot path. A host with a
/// million chunks would walk a million files to answer "have we both got the
/// same set", which is the question that exists to be cheap.
const CHUNKS: TableDefinition<'_, &[u8], u64> = TableDefinition::new("vault_chunks");

/// Running totals, keyed by name: [`CHUNK_BYTES`] and [`OPEN`].
///
/// # Why a total, and why it is not trusted blindly
///
/// Every `StoreChunk` and `StoreSegment` asks "would this exceed the pledge",
/// under the storing lock. Answering it with [`Vault::stats`] listed every
/// owner's blobs and stat'ed each file, so a peer spamming offers it knew would
/// be *refused* delayed every honest store behind a full walk of the disk. The
/// sum of `vault_chunks` is kept here instead, changed in the same transaction
/// as the row it sums, so the two cannot disagree after a commit.
///
/// The one place they can drift from the *disk* is a crash between a blob
/// write (or unlink) and that transaction. [`OPEN`] catches it: set when the
/// vault opens, cleared when it is dropped, so finding it set at open means
/// the last process died mid-flight, and the index and total are rebuilt from
/// the directories before anything reads them.
const TOTALS: TableDefinition<'_, &str, u64> = TableDefinition::new("vault_totals");
/// Sum of every `vault_chunks` value: the chunk half of what the pledge counts.
const CHUNK_BYTES: &str = "chunk_bytes";
/// 1 while a process has the vault open; 0 after a clean drop.
const OPEN: &str = "open";

/// `owner` → the sum of that owner's `vault_chunks` values.
///
/// [`CHUNK_BYTES`] split by owner, so "how much of this vault is ours" -- read
/// by `Node::bound_writes` on every reconcile pass -- is one lookup instead of
/// a walk of our own blobs. Same discipline as the total: changed in the
/// transaction that changes the row, rebuilt with it after an unclean open.
const OWNER_CHUNK_BYTES: TableDefinition<'_, &[u8], u64> =
    TableDefinition::new("vault_owner_chunk_bytes");
/// Set once [`OWNER_CHUNK_BYTES`] has been built: a vault from before it has
/// rows and a total but no split, and needs one rebuild.
const OWNER_TOTALS: &str = "owner_totals";

/// Where the rolling disk check ([`Vault::check_disk`]) stopped: the last
/// `vault_chunks` key it examined, under [`CHECK_CURSOR_KEY`].
///
/// Kept on disk because a daemon restarted more often than one pass takes
/// would otherwise start from the top every time and never reach the end.
const CHECK_CURSOR: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("vault_check_cursor");
const CHECK_CURSOR_KEY: &str = "chunks";

/// `owner ‖ chunk` → when the owner marked it evictable, on this host's clock.
///
/// A mark, not a deletion: the owner names a copy it has to spare, and the
/// chunk stays, served and challenged like any other, until that owner is
/// refused for room here ([`Vault::evict_marked`]). An owner that is never
/// short of room here loses nothing to a mark, and an owner that changes its
/// mind stores the chunk again, which removes the mark ([`Vault::put_chunk`]).
/// Only chunks this vault holds are marked, so the table is never larger than
/// [`CHUNKS`].
const EVICTABLE: TableDefinition<'_, &[u8], u64> = TableDefinition::new("vault_evictable");

/// `owner ‖ marked_at ‖ chunk`: the marks of [`EVICTABLE`] in the order they
/// are evicted, oldest first, so making room reads one short range instead of
/// every mark an owner has.
const EVICTION_ORDER: TableDefinition<'_, &[u8], ()> = TableDefinition::new("vault_eviction_order");

/// Device → owner ‖ the claim bytes that device presented here, as presented.
///
/// Kept so a host can hand an account its own roster (HANDOVER §10 7): the
/// owner's other machines verify each claim themselves, so a host can
/// withhold one and never forge one. Opaque here; the node layer parses.
const CLAIMS: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("vault_claims");

/// Most claims kept, of all accounts: a device key costs nothing to mint, so
/// a table any caller grows needs a ceiling. Full, a new device is not kept
/// and the owner's other machines learn its claim from it directly.
pub const MAX_KEPT_CLAIMS: u64 = 4096;

/// Most claims handed out for one account in one answer: five live devices
/// and their withdrawn predecessors.
pub const MAX_CLAIMS_PER_ANSWER: usize = 16;

/// `owner` → `(day, marks made that day)`, on this host's clock: what
/// [`MAX_EVICTABLE_PER_DAY`] counts.
const EVICT_QUOTA: TableDefinition<'_, &[u8], (u64, u64)> =
    TableDefinition::new("vault_evict_quota");

/// Most chunks one owner may mark evictable here in one day of this host's
/// clock, whichever of its devices marks them.
///
/// Every device of an account holds the account key, so a device that has
/// turned hostile -- stolen, or withdrawn and not yet known to be -- speaks for
/// the account as well as an honest one until its standing lapses. Uncapped,
/// one session marks everything this host holds for the account and a few
/// stores of junk evict it, while the account's honest devices still count
/// those copies: the ledger overstates, which is the error this system must
/// not make. Capped, such a device takes a day to cost 4096 chunks here, and
/// the audit and the owner's own walks have that day to notice. Four batches
/// of `MAX_HAVE_BATCH`: an honest owner marks at most one batch per host per
/// round, and only on a host that refused it for room.
pub const MAX_EVICTABLE_PER_DAY: u32 = 4096;

/// Most marked chunks one call to [`Vault::evict_marked`] removes, whatever
/// it was asked to free.
///
/// The call runs under the host's storing lock, on the path of a store that
/// was refused for room, so every other connection's store waits for it. An
/// honest call frees one chunk's worth -- a few marked chunks. An owner that
/// marked thousands of one-byte chunks would otherwise make one store remove
/// all of them while the host stood still.
pub const MAX_EVICT_PER_CALL: usize = 256;

/// Seconds in the day [`MAX_EVICTABLE_PER_DAY`] counts over.
const DAY: u64 = 24 * 60 * 60;

/// How long a mark stands, on this host's clock. Past it the chunk is held
/// as if never marked: answered "have", summarised, never evicted.
///
/// An owner releases a copy for `CONFIRMED_FOR` and then counts it again if
/// the host still has it. A mark that outlived that would let a store years
/// later delete a copy the owner has long since gone back to counting; one
/// that expires leaves only the window the owner itself stopped counting in.
pub const MARK_EXPIRES: u64 = 30 * DAY;

/// Whether a mark made at `marked_at` still stands at `now`. A clock that
/// stepped back reads the mark as fresh, which only delays its expiry.
const fn mark_stands(marked_at: u64, now: u64) -> bool {
    now.saturating_sub(marked_at) < MARK_EXPIRES
}

/// Bytes in an [`EVICTION_ORDER`] key.
const ORDER_KEY_LEN: usize = 32 + 8 + 32;

/// Most rows one call to [`Vault::check_disk`] or `Store::check_disk`
/// examines, whatever it is asked for.
///
/// The slice a round needs grows with the account and the interval: 190 477
/// rows at a terabyte on an hourly policy, 12 MB of keys in one `Vec` on a
/// machine whose measured peak is 17 MiB. Found by the CI reviewer. The daemon
/// calls again until its slice is done; memory stays at one batch.
pub const MAX_CHECK_BATCH: usize = 4096;

/// What one slice of [`Vault::check_disk`] found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiskCheck {
    /// Index rows examined.
    pub checked: usize,
    /// Rows whose blob was no longer on disk, now removed from the index and
    /// the totals.
    pub removed: usize,
}

/// Keys removed per transaction when an owner is forgotten.
const FORGET_BATCH: u64 = 1024;

/// Bytes in a `owner ‖ device` key.
const CHAIN_KEY_LEN: usize = 64;

fn chain_key(owner: UserId, device: DeviceId) -> Vec<u8> {
    let mut key = Vec::with_capacity(CHAIN_KEY_LEN);
    key.extend_from_slice(owner.as_bytes());
    key.extend_from_slice(device.as_bytes());
    key
}

fn segment_key(owner: UserId, device: DeviceId, position: u64) -> Vec<u8> {
    let mut key = chain_key(owner, device);
    // Big-endian so lexicographic byte order matches numeric order, which is
    // what makes a prefix range scan return the chain in the right sequence.
    key.extend_from_slice(&position.to_be_bytes());
    key
}

/// Sealed objects held on behalf of other users.
#[derive(Debug)]
pub struct Vault {
    root: PathBuf,
    db: Database,
    /// Held across a blob write or unlink and the transaction that indexes it.
    ///
    /// Without it two puts of one address with different bytes (a hostile peer
    /// on two connections, or `host_for` racing a `StoreChunk`) could index
    /// one length while the disk keeps the other, and the total would lie by
    /// the difference -- in the direction that lets the pledge be overrun.
    chunk_writes: std::sync::Mutex<()>,
    /// This process may have left the disk and the index apart, so `Drop`
    /// must not mark the close clean.
    ///
    /// Set until `open` has finished (a rebuild that failed half-way is the
    /// state the mark exists to catch), and by any chunk write that did not
    /// reach its commit -- an error or a panic after the blob was written or
    /// unlinked, ENOSPC on the redb commit being the likely one. Without it a
    /// clean shutdown after such a failure blessed the drift for good.
    suspect: std::sync::atomic::AtomicBool,
}

/// Marks the vault suspect unless defused: held across a blob write or
/// unlink and the commit that indexes it, so an early return or a panic in
/// between is remembered.
struct Unsure<'a> {
    suspect: &'a std::sync::atomic::AtomicBool,
    armed: bool,
}

impl<'a> Unsure<'a> {
    fn arm(suspect: &'a std::sync::atomic::AtomicBool) -> Self {
        Self {
            suspect,
            armed: true,
        }
    }

    fn defuse(mut self) {
        self.armed = false;
    }
}

impl Drop for Unsure<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.suspect
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
thread_local! {
    /// A failure to inject at a named point, so the tests can fail the
    /// steps the crash handling exists for.
    static FAULT: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

/// Fail here if a test asked for it; nothing in a normal build.
// The `Result` is the point: callers `?` it, and only tests make it fail.
#[cfg_attr(not(test), allow(clippy::unnecessary_wraps))]
fn fault(point: &'static str) -> Result<()> {
    #[cfg(test)]
    if FAULT.with(std::cell::Cell::get) == Some(point) {
        return Err(StoreError::Corrupt(format!("injected fault: {point}")));
    }
    let _ = point;
    Ok(())
}

/// What a vault is holding, for quota accounting and `itsanas status`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VaultStats {
    pub owners: usize,
    pub chunks: usize,
    pub segments: u64,
    /// Every foreign byte on this disk: chunk blobs **and** log segments.
    ///
    /// This is what the pledge is measured against. Segments were missing from
    /// it, which made the quota unenforceable for half the objects a peer can
    /// send -- see `CHAIN_BYTES`.
    pub bytes: u64,
    /// The segment half of [`Self::bytes`], so an operator can see it.
    pub segment_bytes: u64,
}

/// `owner ‖ chunk`, so one owner's chunks are a contiguous, ordered range.
/// Rows the rebuild after an unclean exit writes, or checks, per
/// transaction: what it holds in memory at once. Two under test, so the
/// small vaults the tests build cross many batch boundaries.
const REBUILD_BATCH: usize = if cfg!(test) { 2 } else { 4096 };

fn chunk_key(owner: UserId, chunk: &ChunkId) -> [u8; 64] {
    let mut out = [0u8; 64];
    out[..32].copy_from_slice(owner.as_bytes().as_slice());
    out[32..].copy_from_slice(chunk.as_bytes().as_slice());
    out
}

fn chunk_range_start(owner: UserId) -> [u8; 64] {
    chunk_key(owner, &ChunkId::from_bytes([0x00; 32]))
}

fn chunk_range_end(owner: UserId) -> [u8; 64] {
    chunk_key(owner, &ChunkId::from_bytes([0xFF; 32]))
}

fn chunk_from_key(bytes: &[u8]) -> Option<ChunkId> {
    if bytes.len() != 64 {
        return None;
    }
    let mut chunk = [0u8; 32];
    chunk.copy_from_slice(&bytes[32..]);
    Some(ChunkId::from_bytes(chunk))
}

/// `owner ‖ marked_at ‖ chunk`, big-endian, so one owner's marks are a
/// contiguous range in the order they were made.
fn order_key(owner: UserId, marked_at: u64, chunk: &ChunkId) -> [u8; ORDER_KEY_LEN] {
    let mut out = [0u8; ORDER_KEY_LEN];
    out[..32].copy_from_slice(owner.as_bytes().as_slice());
    out[32..40].copy_from_slice(&marked_at.to_be_bytes());
    out[40..].copy_from_slice(chunk.as_bytes().as_slice());
    out
}

fn order_range(owner: UserId) -> ([u8; ORDER_KEY_LEN], [u8; ORDER_KEY_LEN]) {
    let mut low = [0u8; ORDER_KEY_LEN];
    low[..32].copy_from_slice(owner.as_bytes().as_slice());
    let mut high = [0xFFu8; ORDER_KEY_LEN];
    high[..32].copy_from_slice(owner.as_bytes().as_slice());
    (low, high)
}

/// Drop `owner`'s mark on `address`, if there is one, inside `txn`: called in
/// the transaction that stores or removes the chunk, so a mark never outlives
/// the decision it was about.
fn unmark(txn: &redb::WriteTransaction, owner: UserId, address: &ChunkId) -> Result<()> {
    let marked_at = txn
        .open_table(EVICTABLE)?
        .remove(chunk_key(owner, address).as_slice())?
        .map(|at| at.value());
    if let Some(at) = marked_at {
        txn.open_table(EVICTION_ORDER)?
            .remove(order_key(owner, at, address).as_slice())?;
    }
    Ok(())
}

impl Vault {
    /// Open or create a vault at `root`.
    ///
    /// Takes no keys. There is deliberately no constructor that does.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_owned();
        std::fs::create_dir_all(&root).map_err(|error| StoreError::io(root.clone(), error))?;

        let db = Database::create(root.join("vault.redb"))?;
        let txn = db.begin_write()?;
        {
            let _ = txn.open_table(SEGMENTS)?;
            let _ = txn.open_table(CHAIN_LENGTHS)?;
            let _ = txn.open_table(CHAIN_BYTES)?;
            let _ = txn.open_table(HEADS)?;
            let _ = txn.open_table(CHUNKS)?;
            let _ = txn.open_table(TOTALS)?;
            let _ = txn.open_table(OWNER_CHUNK_BYTES)?;
            let _ = txn.open_table(CHECK_CURSOR)?;
            // Created here, not on first write: a vault from before eviction
            // has none of them, and a read of a missing table is an error.
            let _ = txn.open_table(EVICTABLE)?;
            let _ = txn.open_table(EVICTION_ORDER)?;
            let _ = txn.open_table(CLAIMS)?;
            let _ = txn.open_table(EVICT_QUOTA)?;
        }
        txn.commit()?;

        let vault = Self {
            root,
            db,
            chunk_writes: std::sync::Mutex::new(()),
            // Suspect until the rebuild below has finished: if it fails, the
            // vault is dropped here and must leave the mark as it found it.
            suspect: std::sync::atomic::AtomicBool::new(true),
        };
        vault.reconcile_chunks_if_needed()?;
        vault.backfill_segment_bytes()?;

        let txn = vault.db.begin_write()?;
        {
            txn.open_table(TOTALS)?.insert(OPEN, 1)?;
        }
        txn.commit()?;
        vault
            .suspect
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Ok(vault)
    }

    /// Drop the vault as a crash would: without the clean-close mark.
    #[cfg(test)]
    pub(crate) fn abandon(self) {
        self.suspect
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// The chunk-writes lock, recovered if a panic poisoned it.
    ///
    /// Poisoning means a panic between a blob write and its index row, which
    /// is the crash case in miniature; the panic's `Unsure` marks the vault
    /// suspect so the next open rebuilds, and
    /// refusing every later write would turn one bug into a dead host.
    fn chunk_writes(&self) -> std::sync::MutexGuard<'_, ()> {
        self.chunk_writes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Per-owner blob directory.
    ///
    /// Separate directories rather than one namespace keyed by owner, so that
    /// evicting a user who left, or auditing how much space one peer occupies,
    /// is a directory operation rather than a full scan.
    ///
    /// Refused while `vault.redb` is not at the root: the vault's disk was
    /// unplugged or unmounted under a running node, and creating the owner's
    /// directories would rebuild a vault on the bare mount point, on the
    /// system disk, where every chunk then looks lost (found by
    /// `red_team_an_unplugged_vault_disk_pauses_the_daemon_and_its_return_resumes_it`).
    fn blobs_for(&self, owner: UserId) -> Result<BlobStore> {
        if !self.root.join("vault.redb").is_file() {
            return Err(StoreError::io(
                self.root.clone(),
                std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "the vault's disk is not there (unplugged or unmounted)",
                ),
            ));
        }
        BlobStore::open(self.root.join("owners").join(owner.to_hex()))
    }

    // ---------------------------------------------------------------- chunks

    /// Accept a sealed chunk for storage.
    ///
    /// Returns whether it was newly stored. The bytes are opaque and are not
    /// validated — see the module docs for why that is not a gap at this layer.
    pub fn put_chunk(&self, owner: UserId, address: &ChunkId, sealed: &[u8]) -> Result<bool> {
        let _writing = self.chunk_writes();
        let unsure = Unsure::arm(&self.suspect);
        let blobs = self.blobs_for(owner)?;
        let stored = blobs.put(address, sealed)?;
        fault("after the blob")?;
        // The size on disk, not `sealed.len()`: a re-put of a held address
        // keeps the old file, and indexing the new length would let a peer
        // store 8 MiB, re-offer the address with 1 byte, and have the total
        // forget the difference -- over and over, past the pledge.
        let size = blobs
            .size_of(address)?
            .ok_or_else(|| StoreError::Corrupt("a chunk just written is not on disk".to_owned()))?;
        // Indexed whether or not it was new: a blob present without its index
        // row is exactly the state that makes a reconciliation disagree for
        // ever, and re-inserting an existing key costs nothing.
        let txn = self.db.begin_write()?;
        {
            let old = txn
                .open_table(CHUNKS)?
                .insert(chunk_key(owner, address).as_slice(), size)?
                .map(|old| old.value());
            let mut totals = txn.open_table(TOTALS)?;
            let total = totals.get(CHUNK_BYTES)?.map_or(0, |total| total.value());
            let total = total.saturating_sub(old.unwrap_or(0)).saturating_add(size);
            totals.insert(CHUNK_BYTES, total)?;
            let mut per_owner = txn.open_table(OWNER_CHUNK_BYTES)?;
            let key = owner.as_bytes().as_slice();
            let mine = per_owner.get(key)?.map_or(0, |total| total.value());
            per_owner.insert(
                key,
                mine.saturating_sub(old.unwrap_or(0)).saturating_add(size),
            )?;
        }
        // Stored again, so wanted again: an owner that re-sends a chunk it
        // marked is counting this copy, and evicting it later would take a
        // copy the owner believes in.
        unmark(&txn, owner, address)?;
        txn.commit()?;
        unsure.defuse();
        Ok(stored)
    }

    /// Serve a sealed chunk.
    pub fn get_chunk(&self, owner: UserId, address: &ChunkId) -> Result<Option<Vec<u8>>> {
        self.blobs_for(owner)?.get(address)
    }

    /// Whether this vault holds a chunk.
    pub fn has_chunk(&self, owner: UserId, address: &ChunkId) -> Result<bool> {
        Ok(self.blobs_for(owner)?.contains(address))
    }

    /// Drop a chunk.
    pub fn remove_chunk(&self, owner: UserId, address: &ChunkId) -> Result<bool> {
        let _writing = self.chunk_writes();
        let unsure = Unsure::arm(&self.suspect);
        let removed = self.blobs_for(owner)?.remove(address)?;
        let txn = self.db.begin_write()?;
        {
            let old = txn
                .open_table(CHUNKS)?
                .remove(chunk_key(owner, address).as_slice())?
                .map(|old| old.value());
            if let Some(old) = old {
                let mut totals = txn.open_table(TOTALS)?;
                let total = totals.get(CHUNK_BYTES)?.map_or(0, |total| total.value());
                totals.insert(CHUNK_BYTES, total.saturating_sub(old))?;
                let mut per_owner = txn.open_table(OWNER_CHUNK_BYTES)?;
                let key = owner.as_bytes().as_slice();
                let mine = per_owner.get(key)?.map_or(0, |total| total.value());
                per_owner.insert(key, mine.saturating_sub(old))?;
            }
        }
        // A mark is about a chunk held here; one that is gone has none.
        unmark(&txn, owner, address)?;
        txn.commit()?;
        unsure.defuse();
        Ok(removed)
    }

    /// Mark those of `chunks` this vault holds for `owner` as evictable to make
    /// room for `owner` ([`Self::evict_marked`]), at most
    /// [`MAX_EVICTABLE_PER_DAY`] new marks per owner per day of `now`.
    ///
    /// Returns how many of `chunks` are marked now -- by this call or an
    /// earlier one, each counted once. A chunk not held is not marked: a mark
    /// on nothing would let a later store of that chunk arrive evictable. A
    /// chunk already marked keeps its first mark, so its place in the
    /// eviction order cannot be refreshed by asking again.
    ///
    /// Who may ask is not decided here: the caller (the peer service) only
    /// calls this for a device its bound says speaks for `owner`.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn mark_evictable(&self, owner: UserId, chunks: &[ChunkId], now: u64) -> Result<u32> {
        let today = now / DAY;
        let asked: std::collections::BTreeSet<&ChunkId> = chunks.iter().collect();
        let txn = self.db.begin_write()?;
        let marked = {
            let held = txn.open_table(CHUNKS)?;
            let mut marks = txn.open_table(EVICTABLE)?;
            let mut order = txn.open_table(EVICTION_ORDER)?;
            let mut quota = txn.open_table(EVICT_QUOTA)?;
            let key = owner.as_bytes().as_slice();
            // A clock that steps back stays on the day it had reached:
            // setting it back must not buy a fresh day's marks.
            let (day, mut made) = match quota.get(key)?.map(|row| row.value()) {
                Some((counted, made)) if counted >= today => (counted, made),
                _ => (today, 0),
            };
            let mut marked = 0u32;
            for chunk in asked {
                let row = chunk_key(owner, chunk);
                if marks.get(row.as_slice())?.is_some() {
                    marked = marked.saturating_add(1);
                    continue;
                }
                if held.get(row.as_slice())?.is_none() || made >= u64::from(MAX_EVICTABLE_PER_DAY) {
                    continue;
                }
                marks.insert(row.as_slice(), now)?;
                order.insert(order_key(owner, now, chunk).as_slice(), ())?;
                made += 1;
                marked = marked.saturating_add(1);
            }
            quota.insert(key, (day, made))?;
            marked
        };
        txn.commit()?;
        Ok(marked)
    }

    /// Whether `owner` has marked `address` evictable here.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn is_evictable(&self, owner: UserId, address: &ChunkId) -> Result<bool> {
        let txn = self.db.begin_read()?;
        Ok(txn
            .open_table(EVICTABLE)?
            .get(chunk_key(owner, address).as_slice())?
            .is_some())
    }

    /// Which of `chunks` carry a standing mark of `owner` at `now`.
    ///
    /// The peer service answers these as missing: a host that still said
    /// "have" for a chunk it is about to evict would have the owner re-stamp
    /// the copy it just released, and count it after the eviction (red team,
    /// HIGH 1). Answered missing, the owner re-offers it, and the store that
    /// follows removes the mark ([`Self::put_chunk`]).
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn marked_among(
        &self,
        owner: UserId,
        chunks: &[ChunkId],
        now: u64,
    ) -> Result<std::collections::BTreeSet<ChunkId>> {
        let txn = self.db.begin_read()?;
        let marks = txn.open_table(EVICTABLE)?;
        let mut out = std::collections::BTreeSet::new();
        for chunk in chunks {
            if marks
                .get(chunk_key(owner, chunk).as_slice())?
                .is_some_and(|at| mark_stands(at.value(), now))
            {
                out.insert(*chunk);
            }
        }
        Ok(out)
    }

    /// Remove `owner`'s marked chunks, oldest mark first, until `bytes_needed`
    /// are freed or [`MAX_EVICT_PER_CALL`] chunks have gone; returns the bytes
    /// freed.
    ///
    /// Only `owner`'s: the caller is making room for `owner`, and a mark is
    /// that owner's word about its own copies, never permission to drop
    /// somebody else's. Sizes are the index's, the same ones the totals the
    /// pledge reads are made of, so what this returns is what those totals
    /// lose. A mark whose chunk is no longer indexed frees nothing and is
    /// dropped.
    ///
    /// # Errors
    ///
    /// The database's or the filesystem's.
    ///
    /// Marks older than [`MARK_EXPIRES`] at `now` are never evicted; the ones
    /// met on the way are pruned. And nothing is deleted unless the standing
    /// marks within [`MAX_EVICT_PER_CALL`] cover `bytes_needed`: a partial
    /// eviction frees too little for the store that asked, which is refused
    /// anyway, and the copies are gone for nothing (red team, LOW).
    pub fn evict_marked(&self, owner: UserId, bytes_needed: u64, now: u64) -> Result<u64> {
        if bytes_needed == 0 {
            return Ok(0);
        }
        // Held from the choice to the commit, so no store of a chosen chunk
        // -- which would unmark it -- lands in between and is then deleted.
        let _writing = self.chunk_writes();
        let (chosen, expired, room) = self.choose_evictions(owner, bytes_needed, now)?;
        if !expired.is_empty() {
            let txn = self.db.begin_write()?;
            {
                let mut marks = txn.open_table(EVICTABLE)?;
                let mut order = txn.open_table(EVICTION_ORDER)?;
                for (key, chunk) in &expired {
                    marks.remove(chunk_key(owner, chunk).as_slice())?;
                    order.remove(key.as_slice())?;
                }
            }
            txn.commit()?;
        }
        if chosen.is_empty() || room < bytes_needed {
            return Ok(0);
        }

        let unsure = Unsure::arm(&self.suspect);
        let blobs = self.blobs_for(owner)?;
        for (_, chunk) in &chosen {
            blobs.remove(chunk)?;
        }
        let txn = self.db.begin_write()?;
        let freed = {
            let mut table = txn.open_table(CHUNKS)?;
            let mut marks = txn.open_table(EVICTABLE)?;
            let mut order = txn.open_table(EVICTION_ORDER)?;
            let mut freed = 0u64;
            for (key, chunk) in &chosen {
                let row = chunk_key(owner, chunk);
                let old = table.remove(row.as_slice())?.map_or(0, |old| old.value());
                freed = freed.saturating_add(old);
                marks.remove(row.as_slice())?;
                order.remove(key.as_slice())?;
            }
            freed
        };
        {
            let mut totals = txn.open_table(TOTALS)?;
            let total = totals.get(CHUNK_BYTES)?.map_or(0, |total| total.value());
            totals.insert(CHUNK_BYTES, total.saturating_sub(freed))?;
            let mut per_owner = txn.open_table(OWNER_CHUNK_BYTES)?;
            let key = owner.as_bytes().as_slice();
            let mine = per_owner.get(key)?.map_or(0, |total| total.value());
            per_owner.insert(key, mine.saturating_sub(freed))?;
        }
        txn.commit()?;
        unsure.defuse();
        Ok(freed)
    }

    /// The standing marks [`Self::evict_marked`] would remove, oldest first,
    /// the expired ones met on the way, and the bytes the chosen ones free.
    #[allow(clippy::type_complexity)]
    fn choose_evictions(
        &self,
        owner: UserId,
        bytes_needed: u64,
        now: u64,
    ) -> Result<(
        Vec<([u8; ORDER_KEY_LEN], ChunkId)>,
        Vec<([u8; ORDER_KEY_LEN], ChunkId)>,
        u64,
    )> {
        let txn = self.db.begin_read()?;
        let order = txn.open_table(EVICTION_ORDER)?;
        let held = txn.open_table(CHUNKS)?;
        let (low, high) = order_range(owner);
        let mut chosen = Vec::new();
        let mut expired = Vec::new();
        let mut room = 0u64;
        for row in order.range(low.as_slice()..=high.as_slice())? {
            // Expired marks count towards the bound too: a walk past a
            // million stale marks under the storing lock is the stall the
            // bound exists to prevent.
            if room >= bytes_needed || chosen.len() + expired.len() >= MAX_EVICT_PER_CALL {
                break;
            }
            let (key, _) = row?;
            let Ok(key) = <[u8; ORDER_KEY_LEN]>::try_from(key.value()) else {
                continue;
            };
            let mut at = [0u8; 8];
            at.copy_from_slice(&key[32..40]);
            let mut chunk = [0u8; 32];
            chunk.copy_from_slice(&key[40..]);
            let chunk = ChunkId::from_bytes(chunk);
            if !mark_stands(u64::from_be_bytes(at), now) {
                expired.push((key, chunk));
                continue;
            }
            let size = held
                .get(chunk_key(owner, &chunk).as_slice())?
                .map_or(0, |size| size.value());
            room = room.saturating_add(size);
            chosen.push((key, chunk));
        }
        Ok((chosen, expired, room))
    }

    /// Free everything held for `owner`: its chunks, then its log chains.
    ///
    /// For an account its coordinator says was deleted (HANDOVER §8 0w (8)).
    /// Chunk by chunk through [`Self::remove_chunk`], so the running totals
    /// the pledge is checked against stay true; then every chain row under
    /// the owner's prefix in one transaction. Returns (chunks, segments).
    ///
    /// # Errors
    ///
    /// The database's or the filesystem's.
    pub fn forget_owner(&self, owner: UserId) -> Result<(u64, u64)> {
        let mut chunks = 0u64;
        for address in self.chunks_for(owner)? {
            if self.remove_chunk(owner, &address)? {
                chunks += 1;
            }
        }
        let first = owner.as_bytes().to_vec();
        let mut last = first.clone();
        last.extend_from_slice(&[0xff; CHAIN_KEY_LEN + 8]);
        let mut segments = 0u64;
        // In batches, each its own transaction: an owner with millions of
        // segments must not have all their keys held in memory at once.
        loop {
            let removed = self.remove_batch(SEGMENTS, &first, &last)?;
            segments += removed;
            if removed < FORGET_BATCH {
                break;
            }
        }
        while self.remove_batch(HEADS, &first, &last)? == FORGET_BATCH {}
        for definition in [CHAIN_LENGTHS, CHAIN_BYTES, EVICTABLE] {
            while self.remove_batch(definition, &first, &last)? == FORGET_BATCH {}
        }
        // The chunks took their marks with them; what is left is marks on
        // chunks the index had already lost, and the day's count, which a
        // returning account must not inherit.
        while self.remove_batch(EVICTION_ORDER, &first, &last)? == FORGET_BATCH {}
        let txn = self.db.begin_write()?;
        txn.open_table(EVICT_QUOTA)?
            .remove(owner.as_bytes().as_slice())?;
        txn.commit()?;
        // The owner's own blob directory, now empty of chunks: left behind it
        // keeps the owner in `owners()`, and every later round asks about it.
        let directory = self.root.join("owners").join(owner.to_hex());
        match std::fs::remove_dir_all(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(StoreError::io(directory, error)),
        }
        Ok((chunks, segments))
    }

    /// Remove up to [`FORGET_BATCH`] keys in `first..=last`; how many went.
    fn remove_batch<V: redb::Value + 'static>(
        &self,
        definition: TableDefinition<'_, &[u8], V>,
        first: &[u8],
        last: &[u8],
    ) -> Result<u64> {
        let txn = self.db.begin_write()?;
        let removed = {
            let mut table = txn.open_table(definition)?;
            let keys: Vec<Vec<u8>> = table
                .range(first..=last)?
                .take(usize::try_from(FORGET_BATCH).unwrap_or(usize::MAX))
                .map(|row| row.map(|(key, _)| key.value().to_vec()))
                .collect::<std::result::Result<_, _>>()?;
            for key in &keys {
                table.remove(key.as_slice())?;
            }
            keys.len() as u64
        };
        txn.commit()?;
        Ok(removed)
    }

    /// Every foreign byte this vault holds, without walking it.
    ///
    /// Equal to [`Self::stats`]`.bytes` -- chunk blobs plus log segments --
    /// and it is what the pledge is checked against. `stats` walks the
    /// directories and stays the answer for `itsanas status`; this reads two
    /// running totals, so a request that will be refused costs a lookup, not
    /// a pass over every file. See `TOTALS` for why the two cannot drift.
    pub fn held_bytes(&self) -> Result<u64> {
        let txn = self.db.begin_read()?;
        let mut held = txn
            .open_table(TOTALS)?
            .get(CHUNK_BYTES)?
            .map_or(0, |total| total.value());
        // One row per device chain, not per object: small whatever the vault
        // holds.
        for row in txn.open_table(CHAIN_BYTES)?.iter()? {
            let (_, bytes) = row?;
            held = held.saturating_add(bytes.value());
        }
        Ok(held)
    }

    /// Keep the claim `device` presented for `owner`, replacing its last one.
    /// Not kept when [`MAX_KEPT_CLAIMS`] devices are already kept.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn keep_claim(&self, device: DeviceId, owner: UserId, claim: &[u8]) -> Result<()> {
        let txn = self.db.begin_write()?;
        {
            let mut claims = txn.open_table(CLAIMS)?;
            let key = device.as_bytes().as_slice();
            if claims.get(key)?.is_none() && claims.len()? >= MAX_KEPT_CLAIMS {
                return Ok(());
            }
            let mut row = owner.as_bytes().to_vec();
            row.extend_from_slice(claim);
            claims.insert(key, row.as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }

    /// The claims kept for `owner`, at most [`MAX_CLAIMS_PER_ANSWER`].
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn claims_for(&self, owner: UserId) -> Result<Vec<Vec<u8>>> {
        let txn = self.db.begin_read()?;
        let mut out = Vec::new();
        for row in txn.open_table(CLAIMS)?.iter()? {
            let (_, value) = row?;
            let bytes = value.value();
            if bytes.len() > 32 && bytes[..32] == *owner.as_bytes() {
                out.push(bytes[32..].to_vec());
                if out.len() == MAX_CLAIMS_PER_ANSWER {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// What this vault holds for one owner -- chunks and segments -- without
    /// walking it.
    ///
    /// Equal to [`Self::stats_for`]`.bytes`, from two running totals: the
    /// owner's row in `vault_owner_chunk_bytes` and their chains' rows in
    /// `vault_chain_bytes` (one per device). `stats_for` walks the owner's
    /// blob directory and stays the answer for `status`.
    pub fn held_bytes_for(&self, owner: UserId) -> Result<u64> {
        let txn = self.db.begin_read()?;
        let key = owner.as_bytes();
        let mut held = txn
            .open_table(OWNER_CHUNK_BYTES)?
            .get(key.as_slice())?
            .map_or(0, |total| total.value());
        // A range over this owner's chains: the key starts with the owner, so
        // this costs the owner's devices, not every chain the vault holds. It
        // runs on every offer a host weighs against an account's share.
        let mut low = [0u8; CHAIN_KEY_LEN];
        low[..32].copy_from_slice(key.as_slice());
        let mut high = [0xFFu8; CHAIN_KEY_LEN];
        high[..32].copy_from_slice(key.as_slice());
        for row in txn
            .open_table(CHAIN_BYTES)?
            .range(low.as_slice()..=high.as_slice())?
        {
            let (_, bytes) = row?;
            held = held.saturating_add(bytes.value());
        }
        Ok(held)
    }

    /// Every chunk address held for one owner, in ascending order.
    ///
    /// From the index rather than the directory, so it is a range scan instead
    /// of a recursive walk, and sorted — which is what
    /// [`crate::summary`] requires of both sides.
    pub fn chunks_for(&self, owner: UserId) -> Result<Vec<ChunkId>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(CHUNKS)?;

        let mut out = Vec::new();
        for row in
            table.range(chunk_range_start(owner).as_slice()..=chunk_range_end(owner).as_slice())?
        {
            let (key, _) = row?;
            if let Some(chunk) = chunk_from_key(key.value()) {
                out.push(chunk);
            }
        }
        Ok(out)
    }

    /// How many chunk rows the index holds, for every owner: what one pass of
    /// [`Self::check_disk`] has to cover.
    pub fn chunk_rows(&self) -> Result<u64> {
        let txn = self.db.begin_read()?;
        Ok(txn.open_table(CHUNKS)?.len()?)
    }

    /// Check up to `limit` index rows against the disk, from where the last
    /// call stopped, and remove each row whose blob is gone.
    ///
    /// # Why the index has to be checked against the disk
    ///
    /// The summary an owner compares ([`Self::chunk_summary`]) is read from the
    /// index; `HaveChunks` is answered from the blob files. Until §8 4c the
    /// owner's full walk asked `HaveChunks` about everything every
    /// `REFRESH_AFTER`, so a blob that vanished from this disk behind the
    /// index -- a bad sector, a careless `rm`, a restored backup -- was noticed
    /// within that time. Since 4c an owner whose summary agrees with this one
    /// re-stamps its records without asking, and the index is the only thing
    /// it hears. Without this check an honest host that lost files would keep
    /// counting as a copy of them for as long as it kept agreeing.
    ///
    /// Removing the row is what makes the loss visible: the summary of that
    /// owner's bucket changes, the owner's next round lists the bucket, finds
    /// the chunk missing and sends it again. The bytes leave the totals in the
    /// same transaction, so the pledge stops counting what is not there.
    ///
    /// The daemon sizes its slice so a whole pass fits in `REFRESH_AFTER`
    /// ([`crate::holders::rows_per_round`]) and calls this until the slice is
    /// done; one call examines at most [`MAX_CHECK_BATCH`] rows. The cursor
    /// wraps: a call that reaches the last row starts the next one from the
    /// first.
    ///
    /// # Errors
    ///
    /// If the index cannot be read or written.
    pub fn check_disk(&self, limit: usize) -> Result<DiskCheck> {
        let mut check = DiskCheck::default();
        let limit = limit.min(MAX_CHECK_BATCH);
        if limit == 0 {
            return Ok(check);
        }

        let (rows, reached_end) = {
            let txn = self.db.begin_read()?;
            let cursor = txn
                .open_table(CHECK_CURSOR)?
                .get(CHECK_CURSOR_KEY)?
                .map(|value| value.value().to_vec());
            let table = txn.open_table(CHUNKS)?;
            let mut rows: Vec<[u8; 64]> = Vec::with_capacity(limit.min(1 << 16));
            let range = match &cursor {
                Some(after) => table.range::<&[u8]>((
                    std::ops::Bound::Excluded(after.as_slice()),
                    std::ops::Bound::Unbounded,
                ))?,
                None => table.range::<&[u8]>(..)?,
            };
            let mut reached_end = true;
            for row in range {
                if rows.len() == limit {
                    reached_end = false;
                    break;
                }
                let (key, _) = row?;
                if let Ok(key) = <[u8; 64]>::try_from(key.value()) {
                    rows.push(key);
                }
            }
            (rows, reached_end)
        };

        let mut gone: Vec<[u8; 64]> = Vec::new();
        let mut owner_blobs: Option<(UserId, BlobStore)> = None;
        for key in &rows {
            check.checked += 1;
            let mut owner_bytes = [0u8; 32];
            owner_bytes.copy_from_slice(&key[..32]);
            let owner = UserId::from_bytes(owner_bytes);
            let Some(address) = chunk_from_key(key) else {
                continue;
            };
            if owner_blobs.as_ref().is_none_or(|(held, _)| *held != owner) {
                owner_blobs = Some((owner, self.blobs_for(owner)?));
            }
            if let Some((_, blobs)) = &owner_blobs
                && !blobs.contains(&address)
            {
                gone.push(*key);
            }
        }

        // Under the writes lock, and each row re-checked inside it: a put that
        // landed between the read above and here has written its blob first,
        // so a row whose file exists now is kept.
        let _writing = self.chunk_writes();
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(CHUNKS)?;
            let mut totals = txn.open_table(TOTALS)?;
            let mut per_owner = txn.open_table(OWNER_CHUNK_BYTES)?;
            for key in &gone {
                let mut owner_bytes = [0u8; 32];
                owner_bytes.copy_from_slice(&key[..32]);
                let owner = UserId::from_bytes(owner_bytes);
                let Some(address) = chunk_from_key(key) else {
                    continue;
                };
                if self.blobs_for(owner)?.contains(&address) {
                    continue;
                }
                let Some(old) = table.remove(key.as_slice())?.map(|old| old.value()) else {
                    continue;
                };
                check.removed += 1;
                unmark(&txn, owner, &address)?;
                let total = totals.get(CHUNK_BYTES)?.map_or(0, |total| total.value());
                totals.insert(CHUNK_BYTES, total.saturating_sub(old))?;
                let mine = per_owner
                    .get(owner_bytes.as_slice())?
                    .map_or(0, |total| total.value());
                per_owner.insert(owner_bytes.as_slice(), mine.saturating_sub(old))?;
            }
            let mut cursor = txn.open_table(CHECK_CURSOR)?;
            match rows.last() {
                Some(last) if !reached_end => {
                    cursor.insert(CHECK_CURSOR_KEY, last.as_slice())?;
                }
                _ => {
                    cursor.remove(CHECK_CURSOR_KEY)?;
                }
            }
        }
        txn.commit()?;
        // A full pass done: give back the space of what was removed since,
        // for owners whose chunks are in packs (blob files gave it back at
        // removal). Once a pass, not per removal: a compaction moves up to a
        // pack's worth of bytes, which a request must not wait for.
        if reached_end {
            for owner in self.owners()? {
                self.blobs_for(owner)?.compact()?;
            }
        }
        Ok(check)
    }

    /// A summary of what this vault holds for one owner.
    ///
    /// The answer to "have we both got the same set", in one hash. See
    /// [`crate::summary`].
    pub fn chunk_summary(&self, owner: UserId) -> Result<Vec<crate::summary::Digest>> {
        self.chunk_summary_at(owner, crate::now_unix())
    }

    /// [`Self::chunk_summary`] at `now`, leaving out every chunk with a
    /// standing mark: the summary must agree with what `HaveChunks` answers,
    /// and that answers marked chunks as missing ([`Self::marked_among`]).
    /// An agreeing bucket re-stamps records without asking, so a marked
    /// chunk summarised as held is a released copy counted again.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn chunk_summary_at(&self, owner: UserId, now: u64) -> Result<Vec<crate::summary::Digest>> {
        // Streamed, for the same reason the owner's side is: a host holding a
        // terabyte for somebody would otherwise build a `Vec` of sixteen
        // million ids to answer one question about them.
        let txn = self.db.begin_read()?;
        let table = txn.open_table(CHUNKS)?;
        let marks = txn.open_table(EVICTABLE)?;
        let range =
            table.range(chunk_range_start(owner).as_slice()..=chunk_range_end(owner).as_slice())?;

        let mut failed: Option<StoreError> = None;
        let digests = crate::summary::buckets(range.filter_map(|row| match row {
            Ok((key, _)) => {
                match marks.get(key.value()) {
                    Ok(Some(at)) if mark_stands(at.value(), now) => return None,
                    Ok(_) => {}
                    Err(error) => {
                        failed.get_or_insert(StoreError::from(error));
                        return None;
                    }
                }
                chunk_from_key(key.value())
            }
            Err(error) => {
                failed.get_or_insert(StoreError::from(error));
                None
            }
        }));

        match failed {
            Some(error) => Err(error),
            None => Ok(digests),
        }
    }

    /// Rebuild the chunk index and its total from the directories, when they
    /// cannot be trusted.
    ///
    /// Three cases, each one walk: a vault from before the index (blobs, no
    /// rows), a vault from before the total (rows, no total), and a vault the
    /// last process did not close (`OPEN` still set), where a crash between a
    /// blob write or unlink and its transaction may have left a blob unindexed
    /// or a row with no blob. The directories are the authority; the index is
    /// made to agree with them, rows removed as well as added. A clean close
    /// costs nothing at the next open.
    fn reconcile_chunks_if_needed(&self) -> Result<()> {
        let txn = self.db.begin_read()?;
        let totals = txn.open_table(TOTALS)?;
        let unclean = totals.get(OPEN)?.is_some_and(|open| open.value() != 0);
        let untotalled = totals.get(CHUNK_BYTES)?.is_none() || totals.get(OWNER_TOTALS)?.is_none();
        drop(totals);
        drop(txn);
        if !unclean && !untotalled {
            return Ok(());
        }
        fault("rebuilding")?;

        // In batches, never the whole vault in memory: a terabyte hosted in
        // 64 KiB chunks is 16 million rows, and the map this used to build
        // (1.2 GB) failed or swapped on a 4 GB Pi after a power cut, so the
        // node crash-looped. Safe to stop anywhere: `OPEN` stays set until
        // the whole rebuild is done, so the next open walks again.
        let owners = self.owners()?;
        let mut per_owner: std::collections::BTreeMap<[u8; 32], u64> =
            std::collections::BTreeMap::new();
        for owner in &owners {
            let blobs = self.blobs_for(*owner)?;
            // A crash mid-write also leaves its staging file, counted by
            // nothing and never reclaimed unless swept here.
            blobs.sweep_staging()?;
            let mut batch: Vec<([u8; 64], u64)> = Vec::with_capacity(REBUILD_BATCH);
            let mut bytes = 0u64;
            blobs.for_each_address(&mut |address| {
                if let Some(size) = blobs.size_of(&address)? {
                    batch.push((chunk_key(*owner, &address), size));
                    bytes = bytes.saturating_add(size);
                }
                if batch.len() >= REBUILD_BATCH {
                    self.insert_rows(&std::mem::take(&mut batch))?;
                }
                Ok(())
            })?;
            self.insert_rows(&batch)?;
            if bytes > 0 {
                per_owner.insert(*owner.as_bytes(), bytes);
            }
        }
        self.remove_stale_rows(&owners.iter().copied().collect())?;

        let txn = self.db.begin_write()?;
        {
            let total = per_owner
                .values()
                .fold(0u64, |sum, b| sum.saturating_add(*b));
            let mut totals = txn.open_table(TOTALS)?;
            totals.insert(CHUNK_BYTES, total)?;
            totals.insert(OWNER_TOTALS, 1)?;
            // Rewritten whole: an owner whose blobs are all gone keeps no row.
            let mut owners = txn.open_table(OWNER_CHUNK_BYTES)?;
            owners.retain(|_, _| false)?;
            for (owner, bytes) in &per_owner {
                owners.insert(owner.as_slice(), *bytes)?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    /// One batch of the rebuild's rows, in its own transaction.
    fn insert_rows(&self, rows: &[([u8; 64], u64)]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(CHUNKS)?;
            for (key, size) in rows {
                table.insert(key.as_slice(), *size)?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    /// Remove the rows whose blob is gone, or whose owner the vault no longer
    /// lists, a batch at a time: the directories are the authority.
    fn remove_stale_rows(&self, owners: &std::collections::BTreeSet<UserId>) -> Result<()> {
        let mut after: Option<Vec<u8>> = None;
        loop {
            let mut stale = Vec::new();
            let mut last = None;
            {
                let txn = self.db.begin_read()?;
                let table = txn.open_table(CHUNKS)?;
                let rows = match &after {
                    Some(key) => table.range::<&[u8]>((
                        std::ops::Bound::Excluded(key.as_slice()),
                        std::ops::Bound::Unbounded,
                    ))?,
                    None => table.range::<&[u8]>(..)?,
                };
                for row in rows.take(REBUILD_BATCH) {
                    let (key, _) = row?;
                    let key = key.value().to_vec();
                    if !self.row_has_blob(&key, owners)? {
                        stale.push(key.clone());
                    }
                    last = Some(key);
                }
            }
            if !stale.is_empty() {
                let txn = self.db.begin_write()?;
                {
                    let mut table = txn.open_table(CHUNKS)?;
                    for key in &stale {
                        table.remove(key.as_slice())?;
                    }
                }
                txn.commit()?;
            }
            match last {
                Some(key) => after = Some(key),
                None => return Ok(()),
            }
        }
    }

    fn row_has_blob(
        &self,
        key: &[u8],
        owners: &std::collections::BTreeSet<UserId>,
    ) -> Result<bool> {
        if key.len() != 64 {
            return Ok(false);
        }
        let owner = UserId::from_slice(&key[..32])?;
        if !owners.contains(&owner) {
            return Ok(false);
        }
        let mut chunk = [0u8; 32];
        chunk.copy_from_slice(&key[32..]);
        Ok(self
            .blobs_for(owner)?
            .size_of(&ChunkId::from_bytes(chunk))?
            .is_some())
    }

    /// Total up the segments a vault already holds, once.
    ///
    /// Same reasoning as the chunk reconciliation, and the same cost: one walk on the
    /// first start after an upgrade. Without it a vault that filled up before
    /// segments counted would report zero for them for ever, which is the bug
    /// this table exists to fix, preserved.
    fn backfill_segment_bytes(&self) -> Result<()> {
        let txn = self.db.begin_read()?;
        let done = txn.open_table(CHAIN_BYTES)?.iter()?.next().is_some();
        let empty = txn.open_table(SEGMENTS)?.iter()?.next().is_none();
        drop(txn);
        if done || empty {
            return Ok(());
        }

        let mut totals: std::collections::BTreeMap<Vec<u8>, u64> =
            std::collections::BTreeMap::new();
        {
            let txn = self.db.begin_read()?;
            let segments = txn.open_table(SEGMENTS)?;
            for row in segments.iter()? {
                let (key, value) = row?;
                let raw = key.value();
                // The segment key is the chain key with the index appended, so
                // the chain key is its prefix. Taken by length rather than by
                // parsing, because the shape is fixed by `segment_key`.
                if raw.len() < CHAIN_KEY_LEN {
                    continue;
                }
                let chain = raw[..CHAIN_KEY_LEN].to_vec();
                let bytes = value.value().len() as u64;
                *totals.entry(chain).or_default() += bytes;
            }
        }

        let txn = self.db.begin_write()?;
        {
            let mut table = txn.open_table(CHAIN_BYTES)?;
            for (chain, bytes) in totals {
                table.insert(chain.as_slice(), bytes)?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    // -------------------------------------------------------------- segments

    /// Accept a segment for storage, after verifying its signature.
    ///
    /// Returns whether it was newly stored. Rejects a segment that does not
    /// continue the chain this vault already holds for that device, so a host
    /// cannot be induced to store a chain with a hole in it and then serve that
    /// hole to a peer as though it were complete.
    pub fn put_segment(&self, envelope: &SegmentEnvelope) -> Result<bool> {
        // Before anything is written. A host that stored unverified envelopes
        // would be a convenient way to attribute garbage to someone's device.
        envelope.verify_signature()?;

        let encoded = envelope.encode()?;
        let key = chain_key(envelope.owner, envelope.device);

        let txn = self.db.begin_write()?;
        let stored;

        {
            let mut segments = txn.open_table(SEGMENTS)?;
            let mut lengths = txn.open_table(CHAIN_LENGTHS)?;
            let mut chain_bytes = txn.open_table(CHAIN_BYTES)?;
            let mut heads = txn.open_table(HEADS)?;

            let length = lengths.get(key.as_slice())?.map_or(0, |v| v.value());
            let head = match heads.get(key.as_slice())? {
                Some(value) => Some(ObjectId::from_slice(value.value())?),
                None => None,
            };

            if head == Some(envelope.segment_id) {
                // Already the tip. Re-offering is normal, not an error.
                stored = false;
            } else if envelope.previous == head {
                segments.insert(
                    segment_key(envelope.owner, envelope.device, length).as_slice(),
                    encoded.as_slice(),
                )?;
                lengths.insert(key.as_slice(), length + 1)?;
                let so_far = chain_bytes.get(key.as_slice())?.map_or(0, |v| v.value());
                chain_bytes.insert(key.as_slice(), so_far.saturating_add(encoded.len() as u64))?;
                heads.insert(key.as_slice(), envelope.segment_id.as_bytes().as_slice())?;
                stored = true;
            } else {
                return Err(StoreError::SegmentChainBroken {
                    segment: envelope.segment_id.short(),
                    expected: head.map_or_else(|| "none".to_owned(), |id| id.short()),
                    found: envelope
                        .previous
                        .map_or_else(|| "none".to_owned(), |id| id.short()),
                });
            }
        }

        txn.commit()?;
        Ok(stored)
    }

    /// Segments held for one device's chain, oldest first.
    ///
    /// `after` resumes from just past a segment the caller already has; `limit`
    /// caps the response so one request cannot ask for an unbounded assembly.
    pub fn segments_for(
        &self,
        owner: UserId,
        device: DeviceId,
        after: Option<ObjectId>,
        limit: usize,
    ) -> Result<Vec<SegmentEnvelope>> {
        let prefix = chain_key(owner, device);
        let txn = self.db.begin_read()?;
        let table = txn.open_table(SEGMENTS)?;

        let start = segment_key(owner, device, 0);
        let end = segment_key(owner, device, u64::MAX);

        let mut out = Vec::new();
        let mut skipping = after.is_some();

        for row in table.range(start.as_slice()..=end.as_slice())? {
            let (key, value) = row?;
            if !key.value().starts_with(&prefix) {
                break;
            }

            let envelope = SegmentEnvelope::decode(value.value())?;

            if skipping {
                // Resume *after* the named segment, so the caller does not
                // receive one it already has on every round.
                if Some(envelope.segment_id) == after {
                    skipping = false;
                }
                continue;
            }

            out.push(envelope);
            if out.len() >= limit {
                break;
            }
        }

        // `after` naming a segment this vault does not hold means the caller is
        // ahead of us, or is talking about a different chain. Returning
        // everything would re-send history it already has; returning nothing is
        // the honest answer.
        if skipping {
            return Ok(Vec::new());
        }

        Ok(out)
    }

    /// Chain tips held for one owner, across all their devices.
    pub fn heads_for(&self, owner: UserId) -> Result<Vec<(DeviceId, ObjectId, u64)>> {
        let txn = self.db.begin_read()?;
        let heads = txn.open_table(HEADS)?;
        let lengths = txn.open_table(CHAIN_LENGTHS)?;

        let mut out = Vec::new();
        for row in heads.iter()? {
            let (key, value) = row?;
            let key = key.value();

            if key.len() != CHAIN_KEY_LEN || !key.starts_with(owner.as_bytes()) {
                continue;
            }

            let device = DeviceId::from_slice(&key[32..])?;
            let head = ObjectId::from_slice(value.value())?;
            let length = lengths.get(key)?.map_or(0, |v| v.value());

            out.push((device, head, length));
        }

        Ok(out)
    }

    /// Every owner this vault holds anything for.
    ///
    /// Unions two sources, and needs both. Deriving the list from the segment
    /// table alone misses a peer whose chunks are held but whose log is not —
    /// which is the *normal* state for a host, since chunk data is the bulk of
    /// what gets stored and a host may hold chunks for a device whose segments
    /// went to a different host entirely. Quota accounting built on the segment
    /// table alone reports zero bytes for such a peer and lets the disk fill.
    pub fn owners(&self) -> Result<Vec<UserId>> {
        let mut out: Vec<UserId> = Vec::new();

        let txn = self.db.begin_read()?;
        let heads = txn.open_table(HEADS)?;
        for row in heads.iter()? {
            let (key, _) = row?;
            let key = key.value();
            if key.len() != CHAIN_KEY_LEN {
                continue;
            }
            let owner = UserId::from_slice(&key[..32])?;
            if !out.contains(&owner) {
                out.push(owner);
            }
        }

        let directory = self.root.join("owners");
        match std::fs::read_dir(&directory) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|error| StoreError::io(directory.clone(), error))?;
                    // A directory whose name is not a user id was not created
                    // by us; ignoring it is right, because everything derived
                    // from this list eventually decides what to delete.
                    if let Some(name) = entry.file_name().to_str()
                        && let Ok(owner) = name.parse::<UserId>()
                        && !out.contains(&owner)
                    {
                        out.push(owner);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(StoreError::io(directory, error)),
        }

        out.sort_unstable();
        Ok(out)
    }

    /// What this vault holds for one owner.
    ///
    /// Worth separating from [`Self::stats`], because a vault legitimately
    /// holds two very different things: other people's data, which is the
    /// hosting bargain, and *your own account's* segments from your other
    /// devices, which is what lets this machine relay them onwards. Reporting
    /// the two as one number tells an operator they are hosting for a stranger
    /// when they are not.
    pub fn stats_for(&self, owner: UserId) -> Result<VaultStats> {
        let blobs = self.blobs_for(owner)?;
        let prefix = owner.as_bytes();

        let txn = self.db.begin_read()?;
        let lengths = txn.open_table(CHAIN_LENGTHS)?;

        let mut segments = 0u64;
        for row in lengths.iter()? {
            let (key, value) = row?;
            if key.value().starts_with(prefix) {
                segments = segments.saturating_add(value.value());
            }
        }

        let chain_bytes = txn.open_table(CHAIN_BYTES)?;
        let mut segment_bytes = 0u64;
        for row in chain_bytes.iter()? {
            let (key, value) = row?;
            if key.value().starts_with(prefix) {
                segment_bytes = segment_bytes.saturating_add(value.value());
            }
        }

        Ok(VaultStats {
            owners: 1,
            chunks: blobs.addresses()?.len(),
            segments,
            // Both halves, for the same reason as `stats`: this is what an
            // operator reads to answer "how much of my disk is theirs".
            bytes: blobs.total_bytes()?.saturating_add(segment_bytes),
            segment_bytes,
        })
    }

    /// What this vault is holding, in total.
    pub fn stats(&self) -> Result<VaultStats> {
        let owners = self.owners()?;
        let mut stats = VaultStats {
            owners: owners.len(),
            ..VaultStats::default()
        };

        for owner in &owners {
            let blobs = self.blobs_for(*owner)?;
            stats.chunks += blobs.addresses()?.len();
            stats.bytes = stats.bytes.saturating_add(blobs.total_bytes()?);
        }

        let txn = self.db.begin_read()?;
        let lengths = txn.open_table(CHAIN_LENGTHS)?;
        for row in lengths.iter()? {
            let (_, value) = row?;
            stats.segments = stats.segments.saturating_add(value.value());
        }

        // Segments are foreign data on this disk exactly as chunks are, and
        // `bytes` is what the pledge is measured against. Leaving them out made
        // the quota unenforceable for half the objects a peer can send.
        let chain_bytes = txn.open_table(CHAIN_BYTES)?;
        for row in chain_bytes.iter()? {
            let (_, value) = row?;
            stats.segment_bytes = stats.segment_bytes.saturating_add(value.value());
            stats.bytes = stats.bytes.saturating_add(value.value());
        }

        Ok(stats)
    }
}

impl Drop for Vault {
    /// Mark the close clean, so the next open trusts the totals.
    ///
    /// Best effort: if this fails the mark stays set and the next open walks
    /// the directories once, which is slower and still right.
    fn drop(&mut self) {
        if self.suspect.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let mark = || -> Result<()> {
            let txn = self.db.begin_write()?;
            {
                txn.open_table(TOTALS)?.insert(OPEN, 0)?;
            }
            txn.commit()?;
            Ok(())
        };
        let _ = mark();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};

    use crate::oplog::{FileEntry, LogEntry, Operation};

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = Vault::open(dir.path()).expect("open");
        (dir, vault)
    }

    fn keys(byte: u8) -> UserKeys {
        UserKeys::derive(&MasterSecret::from_bytes([byte; 32]))
    }

    #[test]
    fn red_team_a_vault_whose_disk_went_writes_nothing_on_the_bare_mount_point() {
        // A USB disk unplugged under a running node: the open database
        // handle keeps working, but creating an owner's directories would
        // rebuild a vault on the system disk, where it would vanish under
        // the disk when it came back.
        let base = tempfile::tempdir().expect("temp dir");
        let root = base.path().join("disk").join("vault");
        let vault = Vault::open(&root).expect("open");
        let owner = keys(1).user_id();
        let address = ChunkId::from_bytes([7; 32]);
        std::fs::rename(base.path().join("disk"), base.path().join("unplugged")).expect("unplug");
        assert!(vault.put_chunk(owner, &address, b"sealed").is_err());
        assert!(vault.get_chunk(owner, &address).is_err() || !root.exists());
        assert!(
            !root.exists(),
            "the vault rebuilt itself on the bare mount point"
        );
    }

    fn device(byte: u8) -> DeviceKeys {
        DeviceKeys::from_seed(&SecretBytes::new([byte; 32]))
    }

    fn entry(sequence: u64) -> LogEntry {
        LogEntry {
            sequence,
            recorded_unix: 1_700_000_000 + sequence,
            operation: Operation::Upsert {
                path: format!("file-{sequence}.txt"),
                entry: FileEntry {
                    size: 1,
                    modified_unix: 1_700_000_000,
                    content_hash: [0; 32],
                    chunks: Vec::new(),
                    version: crate::version::VersionVector::new(),
                    author: DeviceId::from_bytes([1; 32]),
                },
            },
        }
    }

    fn segment(
        user: &UserKeys,
        dev: &DeviceKeys,
        previous: Option<ObjectId>,
        sequence: u64,
    ) -> SegmentEnvelope {
        SegmentEnvelope::create(
            user.oplog_root(),
            user.user_id(),
            dev,
            previous,
            vec![entry(sequence)],
        )
        .expect("segment")
    }

    #[test]
    fn a_chunk_round_trips_without_the_vault_ever_holding_a_key() {
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        let address = ChunkId::from_bytes([9; 32]);

        assert!(vault.put_chunk(owner, &address, b"sealed bytes").unwrap());
        assert_eq!(
            vault.get_chunk(owner, &address).unwrap().unwrap(),
            b"sealed bytes"
        );
        assert!(vault.has_chunk(owner, &address).unwrap());
    }

    #[test]
    fn two_owners_chunks_do_not_collide_even_at_the_same_address() {
        // Chunk ids are blinded per user, so a collision across owners should
        // not happen — but the vault must not depend on that for correctness.
        let (_dir, vault) = vault();
        let alice = keys(1).user_id();
        let bob = keys(2).user_id();
        let address = ChunkId::from_bytes([7; 32]);

        vault.put_chunk(alice, &address, b"alice's bytes").unwrap();
        vault.put_chunk(bob, &address, b"bob's bytes").unwrap();

        assert_eq!(
            vault.get_chunk(alice, &address).unwrap().unwrap(),
            b"alice's bytes"
        );
        assert_eq!(
            vault.get_chunk(bob, &address).unwrap().unwrap(),
            b"bob's bytes",
            "one owner's chunk overwrote another's at the same address"
        );
    }

    #[test]
    fn a_segment_with_a_bad_signature_is_refused_before_it_is_stored() {
        // A host that stored unverified envelopes becomes a way to flood a
        // user's peers with garbage attributed to one of their devices.
        let (_dir, vault) = vault();
        let user = keys(3);
        let dev = device(3);

        let mut forged = segment(&user, &dev, None, 1);
        forged.sealed_body[0] ^= 0xFF;

        assert!(
            vault.put_segment(&forged).is_err(),
            "a segment with a broken signature was accepted for storage"
        );
        assert_eq!(
            vault.heads_for(user.user_id()).unwrap(),
            [] as [(itsanas_crypto::DeviceId, itsanas_crypto::ObjectId, u64); 0]
        );
    }

    #[test]
    fn a_chain_is_stored_and_served_in_order() {
        let (_dir, vault) = vault();
        let user = keys(4);
        let dev = device(4);

        let first = segment(&user, &dev, None, 1);
        let second = segment(&user, &dev, Some(first.segment_id), 2);
        let third = segment(&user, &dev, Some(second.segment_id), 3);

        for envelope in [&first, &second, &third] {
            assert!(vault.put_segment(envelope).unwrap());
        }

        let served = vault
            .segments_for(user.user_id(), dev.device_id(), None, 10)
            .unwrap();

        assert_eq!(served.len(), 3);
        assert_eq!(served[0].segment_id, first.segment_id);
        assert_eq!(served[1].segment_id, second.segment_id);
        assert_eq!(served[2].segment_id, third.segment_id);
        crate::oplog::validate_chain(&served).expect("the served chain must validate");
    }

    #[test]
    fn a_segment_that_does_not_continue_the_chain_is_refused() {
        // Otherwise a host can be induced to store a chain with a hole and then
        // serve that hole to a peer as though it were complete.
        let (_dir, vault) = vault();
        let user = keys(5);
        let dev = device(5);

        let first = segment(&user, &dev, None, 1);
        let second = segment(&user, &dev, Some(first.segment_id), 2);
        let third = segment(&user, &dev, Some(second.segment_id), 3);

        vault.put_segment(&first).unwrap();

        assert!(
            vault.put_segment(&third).is_err(),
            "a segment was stored on top of a chain it does not continue"
        );
        assert_eq!(
            vault
                .segments_for(user.user_id(), dev.device_id(), None, 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn re_offering_the_current_tip_is_accepted_as_a_no_op() {
        // Peers re-offer freely; there is no acknowledgement telling them to
        // stop. This must not be an error, and must not duplicate the segment.
        let (_dir, vault) = vault();
        let user = keys(6);
        let dev = device(6);
        let first = segment(&user, &dev, None, 1);

        assert!(vault.put_segment(&first).unwrap());
        assert!(!vault.put_segment(&first).unwrap());

        assert_eq!(
            vault
                .segments_for(user.user_id(), dev.device_id(), None, 10)
                .unwrap()
                .len(),
            1,
            "re-offering the tip duplicated it"
        );
    }

    #[test]
    fn resuming_after_a_segment_skips_what_the_caller_already_has() {
        let (_dir, vault) = vault();
        let user = keys(7);
        let dev = device(7);

        let first = segment(&user, &dev, None, 1);
        let second = segment(&user, &dev, Some(first.segment_id), 2);
        let third = segment(&user, &dev, Some(second.segment_id), 3);
        for envelope in [&first, &second, &third] {
            vault.put_segment(envelope).unwrap();
        }

        let served = vault
            .segments_for(user.user_id(), dev.device_id(), Some(first.segment_id), 10)
            .unwrap();

        assert_eq!(served.len(), 2);
        assert_eq!(served[0].segment_id, second.segment_id);

        // Resuming after the tip yields nothing.
        assert_eq!(
            vault
                .segments_for(user.user_id(), dev.device_id(), Some(third.segment_id), 10)
                .unwrap(),
            [] as [crate::oplog::SegmentEnvelope; 0]
        );
    }

    #[test]
    fn the_limit_caps_the_response() {
        let (_dir, vault) = vault();
        let user = keys(8);
        let dev = device(8);

        let mut previous = None;
        for sequence in 1..=5 {
            let envelope = segment(&user, &dev, previous, sequence);
            vault.put_segment(&envelope).unwrap();
            previous = Some(envelope.segment_id);
        }

        assert_eq!(
            vault
                .segments_for(user.user_id(), dev.device_id(), None, 2)
                .unwrap()
                .len(),
            2,
            "the limit was ignored, so one request can ask for unbounded work"
        );
    }

    #[test]
    fn resuming_after_an_unknown_segment_returns_nothing_rather_than_everything() {
        let (_dir, vault) = vault();
        let user = keys(9);
        let dev = device(9);
        vault.put_segment(&segment(&user, &dev, None, 1)).unwrap();

        let served = vault
            .segments_for(
                user.user_id(),
                dev.device_id(),
                Some(ObjectId::from_bytes([0xEE; 32])),
                10,
            )
            .unwrap();

        assert!(
            served.is_empty(),
            "an unknown resume point caused the whole chain to be re-sent"
        );
    }

    #[test]
    fn heads_are_reported_per_device_and_scoped_to_one_owner() {
        let (_dir, vault) = vault();
        let alice = keys(10);
        let bob = keys(11);
        let alice_laptop = device(10);
        let alice_pi = device(11);
        let bob_vm = device(12);

        let a1 = segment(&alice, &alice_laptop, None, 1);
        let a2 = segment(&alice, &alice_laptop, Some(a1.segment_id), 2);
        let p1 = segment(&alice, &alice_pi, None, 1);
        let b1 = segment(&bob, &bob_vm, None, 1);

        for envelope in [&a1, &a2, &p1, &b1] {
            vault.put_segment(envelope).unwrap();
        }

        let mut heads = vault.heads_for(alice.user_id()).unwrap();
        heads.sort_by_key(|(device, _, _)| device.to_bytes());

        assert_eq!(heads.len(), 2, "expected one head per Alice device");
        assert!(
            heads
                .iter()
                .all(|(device, _, _)| *device == alice_laptop.device_id()
                    || *device == alice_pi.device_id()),
            "Bob's device appeared in Alice's heads"
        );

        let laptop = heads
            .iter()
            .find(|(device, _, _)| *device == alice_laptop.device_id())
            .unwrap();
        assert_eq!(laptop.1, a2.segment_id);
        assert_eq!(laptop.2, 2);
    }

    #[test]
    fn one_owners_segments_are_never_served_under_another_owners_name() {
        let (_dir, vault) = vault();
        let alice = keys(12);
        let bob = keys(13);
        let dev = device(13);

        vault.put_segment(&segment(&alice, &dev, None, 1)).unwrap();

        assert!(
            vault
                .segments_for(bob.user_id(), dev.device_id(), None, 10)
                .unwrap()
                .is_empty(),
            "Alice's segments were served as Bob's"
        );
        assert_eq!(
            vault.heads_for(bob.user_id()).unwrap(),
            [] as [(itsanas_crypto::DeviceId, itsanas_crypto::ObjectId, u64); 0]
        );
    }

    #[test]
    fn an_owner_whose_chunks_are_held_but_whose_log_is_not_still_counts() {
        // The normal state for a host: chunk data is the bulk of what gets
        // stored, and the segments for that device may have gone elsewhere.
        // Counting only segment-bearing owners reports zero bytes here and the
        // quota check that depends on it lets the disk fill.
        let (_dir, vault) = vault();
        let guest = keys(20).user_id();

        vault
            .put_chunk(guest, &ChunkId::from_bytes([1; 32]), &[0u8; 900])
            .unwrap();

        assert_eq!(vault.owners().unwrap(), vec![guest]);

        let stats = vault.stats().unwrap();
        assert_eq!(stats.owners, 1);
        assert_eq!(stats.chunks, 1);
        assert_eq!(stats.segments, 0);
        assert_eq!(
            stats.bytes, 900,
            "a chunk-only owner was invisible to accounting"
        );
    }

    #[test]
    fn per_owner_stats_separate_hosting_from_relaying() {
        // A vault holds two different things: other people's data, and your own
        // account's segments from your other devices. Reporting them as one
        // number tells an operator they are hosting for a stranger when they
        // are not.
        let (_dir, vault) = vault();
        let mine = keys(21);
        let stranger = keys(22);
        let dev = device(21);

        vault.put_segment(&segment(&mine, &dev, None, 1)).unwrap();
        vault
            .put_chunk(
                stranger.user_id(),
                &ChunkId::from_bytes([1; 32]),
                &[0u8; 500],
            )
            .unwrap();

        let own = vault.stats_for(mine.user_id()).unwrap();
        assert_eq!(own.segments, 1);
        assert_eq!(
            own.bytes - own.segment_bytes,
            0,
            "no foreign chunks are held for my own account"
        );
        // The segment relayed for my own account does weigh something, and it
        // now says so. It used to report zero, which is how a disk filled with
        // segments read as an empty vault.
        assert!(own.segment_bytes > 0, "a relayed segment weighed nothing");

        let theirs = vault.stats_for(stranger.user_id()).unwrap();
        assert_eq!(theirs.bytes, 500);
        assert_eq!(theirs.segment_bytes, 0);
        assert_eq!(theirs.segments, 0);

        let total = vault.stats().unwrap();
        assert_eq!(total.owners, 2);
        assert_eq!(total.bytes - total.segment_bytes, 500, "the chunk half");
        assert_eq!(total.segments, 1);
    }

    #[test]
    fn red_team_forgetting_a_deleted_owner_frees_its_bytes_and_nobody_elses() {
        let (_dir, vault) = vault();
        let gone = keys(16);
        let stays = keys(17);
        let dev = device(16);
        vault.put_segment(&segment(&gone, &dev, None, 1)).unwrap();
        vault.put_segment(&segment(&stays, &dev, None, 1)).unwrap();
        vault
            .put_chunk(gone.user_id(), &ChunkId::from_bytes([3; 32]), &[0u8; 400])
            .unwrap();
        vault
            .put_chunk(stays.user_id(), &ChunkId::from_bytes([3; 32]), &[0u8; 250])
            .unwrap();
        let kept = vault.held_bytes_for(stays.user_id()).unwrap();

        assert_eq!(vault.forget_owner(gone.user_id()).unwrap(), (1, 1));
        assert_eq!(
            vault.held_bytes_for(gone.user_id()).unwrap(),
            0,
            "a deleted account's chunks or chains stayed on a host's disk and in its pledge"
        );
        assert_eq!(
            vault.held_bytes().unwrap(),
            kept,
            "the running total the pledge reads disagrees with what is left"
        );
        assert_eq!(
            vault.held_bytes_for(stays.user_id()).unwrap(),
            kept,
            "forgetting one owner took another's bytes"
        );
        assert_eq!(vault.owners().unwrap(), vec![stays.user_id()]);
    }

    #[test]
    fn stats_account_for_every_owner() {
        let (_dir, vault) = vault();
        let alice = keys(14);
        let bob = keys(15);
        let dev = device(14);

        vault.put_segment(&segment(&alice, &dev, None, 1)).unwrap();
        vault.put_segment(&segment(&bob, &dev, None, 1)).unwrap();
        vault
            .put_chunk(alice.user_id(), &ChunkId::from_bytes([1; 32]), &[0u8; 100])
            .unwrap();
        vault
            .put_chunk(bob.user_id(), &ChunkId::from_bytes([2; 32]), &[0u8; 250])
            .unwrap();

        let stats = vault.stats().unwrap();
        assert_eq!(stats.owners, 2);
        assert_eq!(stats.chunks, 2);
        assert_eq!(stats.segments, 2);
        // `bytes` is every foreign byte, chunks *and* segments. It used to be
        // the chunk half alone, which is what let a stranger fill a host's disk
        // with segments while the pledge check read zero for ever.
        assert_eq!(stats.bytes - stats.segment_bytes, 350, "the chunk half");
        assert!(stats.segment_bytes > 0, "two segments weighed nothing");
    }

    #[test]
    fn red_team_segments_count_against_the_pledge_like_any_other_foreign_byte() {
        // The hole: `would_exceed_pledge` read `stats().bytes`, and `bytes`
        // summed the chunk blobs alone. Segments live in their own table and
        // counted for nothing, so `held` stayed at zero however many arrived --
        // every `StoreSegment` passed the quota, for ever, on any host whose
        // pledge exceeded one segment.
        //
        // It needed no account and no invitation: a throwaway device key
        // completes the handshake, and a self-signed envelope full of random
        // bytes is indistinguishable from a real one because nobody can decrypt
        // either. There is no segment-removal API and redb does not shrink, so
        // the space was not reclaimable even by an operator who noticed -- and
        // they would not have, because `itsanas status` reads the same field.
        let (_dir, vault) = vault();
        let user = keys(31);
        let dev = device(31);

        let before = vault.stats().unwrap();
        assert_eq!(before.segment_bytes, 0);

        vault.put_segment(&segment(&user, &dev, None, 1)).unwrap();
        let after = vault.stats().unwrap();
        assert!(
            after.segment_bytes > 0,
            "a stored segment weighed nothing against the pledge"
        );
        assert_eq!(
            after.bytes, after.segment_bytes,
            "the vault holds no chunks, so every byte here is segment"
        );

        // And it accumulates rather than being overwritten by the next link.
        let first = after.segment_bytes;
        let head = vault.heads_for(user.user_id()).unwrap()[0].1;
        let second = segment(&user, &dev, Some(head), 2);
        vault.put_segment(&second).unwrap();
        assert!(
            vault.stats().unwrap().segment_bytes > first,
            "a second segment did not add to the total"
        );
    }

    /// The pledge reads `held_bytes`; `stats` walks the disk. A difference is
    /// a pledge enforced against a number that is not what the disk holds.
    fn assert_total_is_the_walk(vault: &Vault, when: &str) {
        assert_eq!(
            vault.held_bytes().unwrap(),
            vault.stats().unwrap().bytes,
            "after {when}, the running total the pledge reads is not what the \
             vault holds on disk: the host would refuse or accept at the wrong byte"
        );
    }

    #[test]
    fn red_team_the_held_total_is_the_walk_after_every_kind_of_write() {
        let dir = tempfile::tempdir().unwrap();
        let alice = keys(20).user_id();
        let bob = keys(21).user_id();
        let (a, b) = (ChunkId::from_bytes([1; 32]), ChunkId::from_bytes([2; 32]));
        {
            let vault = Vault::open(dir.path()).unwrap();
            assert_total_is_the_walk(&vault, "opening an empty vault");

            vault.put_chunk(alice, &a, &[7u8; 1000]).unwrap();
            vault.put_chunk(bob, &a, &[8u8; 300]).unwrap();
            vault.put_chunk(alice, &b, &[9u8; 50]).unwrap();
            assert_total_is_the_walk(&vault, "puts for two owners");

            // A re-put keeps the file already there. Both directions matter:
            // smaller is the one that under-counts and lets the disk fill.
            vault.put_chunk(alice, &a, &[7u8; 1000]).unwrap();
            vault.put_chunk(alice, &a, &[1u8; 1]).unwrap();
            vault.put_chunk(bob, &a, &[2u8; 9000]).unwrap();
            assert_total_is_the_walk(&vault, "re-puts of held addresses at other sizes");

            vault.remove_chunk(alice, &a).unwrap();
            vault.remove_chunk(alice, &a).unwrap();
            vault
                .remove_chunk(bob, &ChunkId::from_bytes([3; 32]))
                .unwrap();
            assert_total_is_the_walk(&vault, "deletes, one repeated and one of nothing");

            let user = keys(22);
            vault
                .put_segment(&segment(&user, &device(22), None, 1))
                .unwrap();
            assert_total_is_the_walk(&vault, "a segment");
        }
        let vault = Vault::open(dir.path()).unwrap();
        assert_total_is_the_walk(&vault, "a clean close and reopen");
        assert!(
            vault.held_bytes().unwrap() > 0,
            "the total was lost on reopen"
        );
    }

    #[test]
    fn red_team_a_crash_between_a_blob_and_its_row_is_rebuilt_at_open() {
        let dir = tempfile::tempdir().unwrap();
        let owner = keys(23).user_id();
        let (a, b, c) = (
            ChunkId::from_bytes([1; 32]),
            ChunkId::from_bytes([2; 32]),
            ChunkId::from_bytes([3; 32]),
        );
        let vault = Vault::open(dir.path()).unwrap();
        vault.put_chunk(owner, &a, &[1u8; 400]).unwrap();
        vault.put_chunk(owner, &b, &[2u8; 200]).unwrap();

        // What a crash leaves: a blob renamed into place whose row was never
        // committed, and a blob unlinked whose row was never removed.
        let blobs = vault.blobs_for(owner).unwrap();
        blobs.put(&c, &[3u8; 5000]).unwrap();
        blobs.remove(&a).unwrap();
        let staging = dir
            .path()
            .join("owners")
            .join(owner.to_hex())
            .join("tmp")
            .join("left-by-a-crash.tmp");
        std::fs::write(&staging, [0u8; 4096]).unwrap();
        vault.abandon();

        let vault = Vault::open(dir.path()).unwrap();
        assert_total_is_the_walk(&vault, "a crash mid-write");
        assert_eq!(
            vault.chunks_for(owner).unwrap(),
            vec![b, c],
            "the index still disagrees with the disk after a crash: \
             reconciliation would ask for a chunk held and offer one gone"
        );
        assert!(
            !staging.exists(),
            "a crash's staging file survived the rebuild: disk the pledge never counts"
        );
    }

    #[test]
    fn red_team_a_rebuild_in_batches_agrees_with_the_disk_across_every_boundary() {
        // The rebuild after an unclean exit no longer holds the vault in one
        // map (1.2 GB for a terabyte, a crash loop on a Pi); written and
        // checked two rows at a time here, it must still end where one pass did.
        let dir = tempfile::tempdir().unwrap();
        let alice = keys(26).user_id();
        let bob = keys(27).user_id();
        let vault = Vault::open(dir.path()).unwrap();
        let mut kept = Vec::new();
        for n in 1..=9u8 {
            let chunk = ChunkId::from_bytes([n; 32]);
            vault
                .put_chunk(alice, &chunk, &vec![n; 100 * usize::from(n)])
                .unwrap();
            vault.put_chunk(bob, &chunk, &[n; 10]).unwrap();
            if n % 2 == 1 {
                kept.push(chunk);
            }
        }
        let blobs = vault.blobs_for(alice).unwrap();
        for n in (2..=8u8).step_by(2) {
            blobs.remove(&ChunkId::from_bytes([n; 32])).unwrap();
        }
        vault.abandon();

        let vault = Vault::open(dir.path()).unwrap();
        assert_eq!(
            vault.chunks_for(alice).unwrap(),
            kept,
            "a batch boundary kept a row whose blob is gone, or lost one that is there"
        );
        assert_eq!(vault.chunks_for(bob).unwrap().len(), 9);
        assert_total_is_the_walk(&vault, "a rebuild in batches");
    }

    /// Run `body` with a failure injected at `point`.
    fn failing_at<T>(point: &'static str, body: impl FnOnce() -> T) -> T {
        FAULT.with(|fault| fault.set(Some(point)));
        let out = body();
        FAULT.with(|fault| fault.set(None));
        out
    }

    #[test]
    fn red_team_a_write_that_fails_after_its_blob_is_rebuilt_after_a_clean_close() {
        let dir = tempfile::tempdir().unwrap();
        let owner = keys(25).user_id();
        {
            let vault = Vault::open(dir.path()).unwrap();
            vault
                .put_chunk(owner, &ChunkId::from_bytes([1; 32]), &[1u8; 100])
                .unwrap();
            // The blob lands and its commit fails (ENOSPC on redb, say); the
            // process carries on and later shuts down cleanly.
            let failed = failing_at("after the blob", || {
                vault.put_chunk(owner, &ChunkId::from_bytes([2; 32]), &[2u8; 5000])
            });
            assert!(failed.is_err(), "the injected fault did not fire");
        }
        let vault = Vault::open(dir.path()).unwrap();
        assert_total_is_the_walk(&vault, "a failed write and a clean close");
    }

    #[test]
    fn red_team_held_bytes_for_one_owner_needs_no_walk() {
        // `Node::held_for_others` reads this on every reconcile pass; it was
        // `stats_for`, two walks of the owner's blobs each time. It must equal
        // the walk while nothing goes behind the vault's back, and -- the
        // proof that it does not walk -- keep its answer when a blob file is
        // deleted under it, until an unclean open rebuilds from the disk.
        let dir = tempfile::tempdir().unwrap();
        let (ours, theirs) = (UserId::from_bytes([1; 32]), UserId::from_bytes([2; 32]));
        let vault = Vault::open(dir.path()).unwrap();
        for (seed, owner, size) in [(1u8, ours, 4000usize), (2, ours, 1000), (3, theirs, 500)] {
            vault
                .put_chunk(owner, &ChunkId::from_bytes([seed; 32]), &vec![seed; size])
                .unwrap();
        }
        vault
            .remove_chunk(ours, &ChunkId::from_bytes([2; 32]))
            .unwrap();
        assert_eq!(vault.held_bytes_for(ours).unwrap(), 4000);
        assert_eq!(
            vault.held_bytes_for(ours).unwrap(),
            vault.stats_for(ours).unwrap().bytes,
            "the running total drifted from the disk"
        );
        assert_eq!(vault.held_bytes_for(theirs).unwrap(), 500);

        let blob = dir.path().join("owners").join(ours.to_hex());
        for entry in walkdir_files(&blob) {
            std::fs::remove_file(entry).unwrap();
        }
        assert_eq!(
            vault.held_bytes_for(ours).unwrap(),
            4000,
            "held_bytes_for walked the blobs: every reconcile pass pays for it"
        );

        vault.abandon();
        let vault = Vault::open(dir.path()).unwrap();
        assert_eq!(
            vault.held_bytes_for(ours).unwrap(),
            0,
            "an unclean open did not rebuild the per-owner total from the disk"
        );
        assert_eq!(vault.held_bytes_for(theirs).unwrap(), 500);
    }

    fn walkdir_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    out.push(path);
                }
            }
        }
        out
    }

    #[test]
    fn red_team_an_open_that_fails_mid_rebuild_leaves_the_vault_unclean() {
        let dir = tempfile::tempdir().unwrap();
        let owner = keys(26).user_id();
        let vault = Vault::open(dir.path()).unwrap();
        vault
            .put_chunk(owner, &ChunkId::from_bytes([1; 32]), &[1u8; 100])
            .unwrap();
        vault
            .blobs_for(owner)
            .unwrap()
            .put(&ChunkId::from_bytes([2; 32]), &[2u8; 5000])
            .unwrap();
        vault.abandon();

        // The rebuild after the crash fails (an antivirus lock, a permission);
        // the retry must rebuild again, not trust the total it never fixed.
        let failed = failing_at("rebuilding", || Vault::open(dir.path()));
        assert!(failed.is_err(), "the injected fault did not fire");
        drop(failed);

        let vault = Vault::open(dir.path()).unwrap();
        assert_total_is_the_walk(&vault, "a rebuild that failed once");
    }

    #[test]
    fn a_vault_from_before_the_total_is_totalled_at_open() {
        let dir = tempfile::tempdir().unwrap();
        let owner = keys(24).user_id();
        {
            let vault = Vault::open(dir.path()).unwrap();
            vault
                .put_chunk(owner, &ChunkId::from_bytes([5; 32]), &[5u8; 777])
                .unwrap();
            // An upgraded vault: rows, no total, closed cleanly.
            let txn = vault.db.begin_write().unwrap();
            txn.open_table(TOTALS).unwrap().remove(CHUNK_BYTES).unwrap();
            txn.commit().unwrap();
        }
        let vault = Vault::open(dir.path()).unwrap();
        assert_total_is_the_walk(&vault, "an upgrade from a vault with no total");
    }

    #[test]
    fn everything_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let user = keys(16);
        let dev = device(16);
        let first = segment(&user, &dev, None, 1);

        {
            let vault = Vault::open(dir.path()).unwrap();
            vault.put_segment(&first).unwrap();
            vault
                .put_chunk(user.user_id(), &ChunkId::from_bytes([3; 32]), b"bytes")
                .unwrap();
        }

        let vault = Vault::open(dir.path()).unwrap();
        assert_eq!(
            vault
                .segments_for(user.user_id(), dev.device_id(), None, 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            vault
                .get_chunk(user.user_id(), &ChunkId::from_bytes([3; 32]))
                .unwrap()
                .unwrap(),
            b"bytes"
        );
    }

    /// Three chunks for one owner, ids in key order.
    fn three_chunks(vault: &Vault, owner: UserId) -> [ChunkId; 3] {
        let chunks = [
            ChunkId::from_bytes([0x10; 32]),
            ChunkId::from_bytes([0x20; 32]),
            ChunkId::from_bytes([0x30; 32]),
        ];
        for (index, chunk) in chunks.iter().enumerate() {
            vault
                .put_chunk(owner, chunk, &vec![u8::try_from(index).unwrap(); 1000])
                .unwrap();
        }
        chunks
    }

    /// Lose a blob the way a disk does: the file goes, the index row stays.
    fn lose_behind_the_index(vault: &Vault, owner: UserId, chunk: &ChunkId) {
        vault.blobs_for(owner).unwrap().damage(chunk).unwrap();
    }

    #[test]
    fn red_team_a_blob_lost_behind_the_index_is_found_within_one_pass() {
        // §8 4c. An owner whose summary agrees with this host's re-stamps its
        // records without asking, and the summary is read from the index, not
        // the disk. A blob that vanished behind the index left the summary
        // agreeing for ever: the owner counted a copy this host could not
        // serve. Sabotage: keep the row when the blob is missing.
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        let chunks = three_chunks(&vault, owner);
        let before = vault.chunk_summary(owner).unwrap();
        let held = vault.held_bytes_for(owner).unwrap();

        lose_behind_the_index(&vault, owner, &chunks[1]);
        assert_eq!(
            vault.chunk_summary(owner).unwrap(),
            before,
            "fixture: the index alone should not have noticed"
        );

        let mut removed = 0;
        for _ in 0..3 {
            removed += vault.check_disk(1).unwrap().removed;
        }
        assert_eq!(removed, 1, "one pass of the check missed the lost blob");
        assert_ne!(
            vault.chunk_summary(owner).unwrap(),
            before,
            "the summary still says the lost chunk is here: its owner keeps counting this \
             host as a copy it cannot serve"
        );
        assert_eq!(vault.chunks_for(owner).unwrap(), vec![chunks[0], chunks[2]]);
        assert_eq!(
            vault.held_bytes_for(owner).unwrap(),
            held - 1000,
            "the pledge still counts bytes that are not on the disk"
        );
    }

    #[test]
    fn red_team_the_disk_check_resumes_where_it_stopped_after_a_restart() {
        // A daemon restarted more often than a pass takes would otherwise check
        // the first rows for ever and never reach the last. Sabotage: do not
        // keep the cursor on disk.
        let dir = tempfile::tempdir().unwrap();
        let owner = keys(1).user_id();
        let chunks = {
            let vault = Vault::open(dir.path()).unwrap();
            let chunks = three_chunks(&vault, owner);
            lose_behind_the_index(&vault, owner, &chunks[2]);
            assert_eq!(vault.check_disk(2).unwrap().removed, 0);
            chunks
        };
        let vault = Vault::open(dir.path()).unwrap();
        assert_eq!(
            vault.check_disk(2).unwrap().removed,
            1,
            "after a restart the check started from the top again, and the last row was \
             never reached"
        );
        assert!(!vault.chunks_for(owner).unwrap().contains(&chunks[2]));
    }

    #[test]
    fn red_team_one_disk_check_call_holds_at_most_one_batch() {
        // A slice of 190 477 rows asked for in one call was 12 MB of keys at
        // once on a Pi. Sabotage: drop the cap.
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        for index in 0..(MAX_CHECK_BATCH + 10) {
            let mut id = [0u8; 32];
            id[..8].copy_from_slice(&(index as u64).to_be_bytes());
            vault
                .put_chunk(owner, &ChunkId::from_bytes(id), &[1])
                .unwrap();
        }
        assert_eq!(
            vault.check_disk(usize::MAX).unwrap().checked,
            MAX_CHECK_BATCH,
            "one call held more than a batch of keys in memory"
        );
    }

    #[test]
    fn the_disk_check_keeps_every_row_whose_blob_is_there() {
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        three_chunks(&vault, owner);
        let before = vault.chunk_summary(owner).unwrap();
        let check = vault.check_disk(100).unwrap();
        assert_eq!(
            check,
            DiskCheck {
                checked: 3,
                removed: 0
            }
        );
        assert_eq!(vault.chunk_summary(owner).unwrap(), before);
        assert_eq!(vault.chunk_rows().unwrap(), 3);
    }

    fn chunk(byte: u8) -> ChunkId {
        ChunkId::from_bytes([byte; 32])
    }

    const MARKED_AT: u64 = 1_800_000_000;

    #[test]
    fn red_team_a_mark_on_a_chunk_not_held_marks_nothing() {
        // A mark on nothing would let a later store of that chunk arrive
        // already evictable, so the next refusal for room takes a copy the
        // owner just sent and counts. Sabotage: drop the `held` check.
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        vault.put_chunk(owner, &chunk(1), &[1; 10]).unwrap();

        assert_eq!(
            vault
                .mark_evictable(owner, &[chunk(1), chunk(2)], MARKED_AT)
                .unwrap(),
            1
        );
        vault.put_chunk(owner, &chunk(2), &[2; 10]).unwrap();
        assert!(!vault.is_evictable(owner, &chunk(2)).unwrap());
        assert!(vault.is_evictable(owner, &chunk(1)).unwrap());
    }

    #[test]
    fn red_team_an_expired_mark_is_neither_evicted_nor_answered_marked() {
        // An owner counts a released copy again after CONFIRMED_FOR. A mark
        // that never expired would let a store years later delete a copy
        // the owner had long gone back to counting. Sabotages: drop the
        // expiry check in `choose_evictions`; or in `marked_among`.
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        vault.put_chunk(owner, &chunk(1), &[1; 10]).unwrap();
        vault.mark_evictable(owner, &[chunk(1)], MARKED_AT).unwrap();
        let later = MARKED_AT + MARK_EXPIRES;
        assert!(
            vault
                .marked_among(owner, &[chunk(1)], MARKED_AT + 1)
                .unwrap()
                .contains(&chunk(1))
        );
        assert!(
            vault
                .marked_among(owner, &[chunk(1)], later)
                .unwrap()
                .is_empty()
        );
        assert_eq!(vault.evict_marked(owner, 10, later).unwrap(), 0);
        assert!(vault.has_chunk(owner, &chunk(1)).unwrap());
        // And it was pruned on the way.
        assert!(!vault.is_evictable(owner, &chunk(1)).unwrap());
    }

    #[test]
    fn red_team_eviction_deletes_nothing_when_the_marks_cannot_cover_the_need() {
        // Freeing 20 bytes towards a need of 30 still leaves the store
        // refused: the copies would be gone for nothing. Sabotage: drop the
        // `room < bytes_needed` check.
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        for byte in 1..=2 {
            vault.put_chunk(owner, &chunk(byte), &[byte; 10]).unwrap();
        }
        vault
            .mark_evictable(owner, &[chunk(1), chunk(2)], MARKED_AT)
            .unwrap();
        assert_eq!(vault.evict_marked(owner, 30, MARKED_AT).unwrap(), 0);
        assert!(vault.has_chunk(owner, &chunk(1)).unwrap());
        assert!(vault.has_chunk(owner, &chunk(2)).unwrap());
        assert_eq!(vault.evict_marked(owner, 20, MARKED_AT).unwrap(), 20);
    }

    #[test]
    fn a_marked_chunk_that_is_stored_again_is_no_longer_evictable() {
        // An owner that re-sends a chunk it marked counts this copy again;
        // evicting it would take a copy the owner believes in. Sabotage: drop
        // the `unmark` in `put_chunk`.
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        for byte in 1..=3 {
            vault.put_chunk(owner, &chunk(byte), &[byte; 10]).unwrap();
        }
        vault
            .mark_evictable(owner, &[chunk(1), chunk(2), chunk(3)], MARKED_AT)
            .unwrap();
        vault.put_chunk(owner, &chunk(2), &[2; 10]).unwrap();

        assert!(!vault.is_evictable(owner, &chunk(2)).unwrap());
        assert_eq!(vault.evict_marked(owner, 20, MARKED_AT).unwrap(), 20);
        assert!(vault.has_chunk(owner, &chunk(2)).unwrap());
        assert!(!vault.has_chunk(owner, &chunk(1)).unwrap());
        assert!(!vault.has_chunk(owner, &chunk(3)).unwrap());
    }

    #[test]
    fn red_team_eviction_never_takes_another_owners_chunks() {
        // A mark is one owner's word about its own copies. Making room for
        // that owner by dropping somebody else's would lose a copy nobody
        // agreed to lose. Sabotage: range over every owner's marks.
        let (_dir, vault) = vault();
        let (alice, bob) = (keys(1).user_id(), keys(2).user_id());
        vault.put_chunk(alice, &chunk(1), &[1; 10]).unwrap();
        vault.put_chunk(bob, &chunk(2), &[2; 10]).unwrap();
        vault.mark_evictable(bob, &[chunk(2)], MARKED_AT).unwrap();

        assert_eq!(vault.evict_marked(alice, 10, MARKED_AT).unwrap(), 0);
        assert!(vault.has_chunk(bob, &chunk(2)).unwrap());
        assert_eq!(vault.held_bytes_for(bob).unwrap(), 10);
    }

    #[test]
    fn eviction_takes_the_oldest_marks_first_and_stops_when_there_is_room() {
        // Freeing more than asked loses copies for nothing; the totals must
        // fall by exactly what went, or the pledge reads a host as fuller
        // than it is. Sabotage: ignore `bytes_needed`.
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        for byte in 1..=4 {
            vault.put_chunk(owner, &chunk(byte), &[byte; 10]).unwrap();
        }
        // Marked out of address order, so oldest-first is not address order.
        vault.mark_evictable(owner, &[chunk(3)], MARKED_AT).unwrap();
        vault
            .mark_evictable(owner, &[chunk(1)], MARKED_AT + 1)
            .unwrap();
        vault
            .mark_evictable(owner, &[chunk(2)], MARKED_AT + 2)
            .unwrap();

        assert_eq!(vault.evict_marked(owner, 15, MARKED_AT).unwrap(), 20);
        assert!(!vault.has_chunk(owner, &chunk(3)).unwrap());
        assert!(!vault.has_chunk(owner, &chunk(1)).unwrap());
        assert!(vault.has_chunk(owner, &chunk(2)).unwrap());
        assert!(vault.has_chunk(owner, &chunk(4)).unwrap());
        assert_eq!(vault.held_bytes_for(owner).unwrap(), 20);
        assert_eq!(vault.held_bytes().unwrap(), 20);
    }

    #[test]
    fn red_team_an_owner_marks_at_most_a_days_cap_even_with_its_clock_set_back() {
        // A device of the account turned hostile would otherwise mark all the
        // account holds here in one session. Sabotages: skip the cap; reset
        // the count when the day number goes down.
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        let many: Vec<ChunkId> = (0..MAX_EVICTABLE_PER_DAY + 2)
            .map(|i| {
                let mut bytes = [0u8; 32];
                bytes[..4].copy_from_slice(&i.to_be_bytes());
                ChunkId::from_bytes(bytes)
            })
            .collect();
        {
            // Held, without a blob each: only the index decides what is held.
            let txn = vault.db.begin_write().unwrap();
            {
                let mut table = txn.open_table(CHUNKS).unwrap();
                for id in &many {
                    table.insert(chunk_key(owner, id).as_slice(), 1).unwrap();
                }
            }
            txn.commit().unwrap();
        }
        let today = 20_000 * DAY;
        let (first, rest) = many.split_at(MAX_EVICTABLE_PER_DAY as usize);

        assert_eq!(
            vault.mark_evictable(owner, first, today).unwrap(),
            MAX_EVICTABLE_PER_DAY
        );
        assert_eq!(vault.mark_evictable(owner, rest, today + 60).unwrap(), 0);
        assert_eq!(vault.mark_evictable(owner, rest, today - DAY).unwrap(), 0);
        assert_eq!(vault.mark_evictable(owner, rest, today + DAY).unwrap(), 2);
    }

    #[test]
    fn forgetting_an_owner_forgets_its_marks_and_its_days_count() {
        // A returning account must not find its old chunks' marks waiting,
        // nor a day's cap already spent. Sabotage: leave EVICT_QUOTA alone.
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        vault.put_chunk(owner, &chunk(1), &[1; 10]).unwrap();
        vault.mark_evictable(owner, &[chunk(1)], MARKED_AT).unwrap();
        vault.forget_owner(owner).unwrap();

        let txn = vault.db.begin_read().unwrap();
        assert!(txn.open_table(EVICTABLE).unwrap().is_empty().unwrap());
        assert!(txn.open_table(EVICTION_ORDER).unwrap().is_empty().unwrap());
        assert!(txn.open_table(EVICT_QUOTA).unwrap().is_empty().unwrap());
    }

    #[test]
    fn a_vault_from_before_eviction_opens_and_marks() {
        // An older vault has none of the eviction tables, and a read of a
        // missing table is an error: every `MayEvict` and every refusal for
        // room would fail. Sabotage: drop the `open_table` lines in `open`.
        let dir = tempfile::tempdir().unwrap();
        let owner = keys(1).user_id();
        {
            let vault = Vault::open(dir.path()).unwrap();
            vault.put_chunk(owner, &chunk(1), &[1; 10]).unwrap();
            let txn = vault.db.begin_write().unwrap();
            txn.delete_table(EVICTABLE).unwrap();
            txn.delete_table(EVICTION_ORDER).unwrap();
            txn.delete_table(EVICT_QUOTA).unwrap();
            txn.commit().unwrap();
        }
        let vault = Vault::open(dir.path()).unwrap();
        assert!(!vault.is_evictable(owner, &chunk(1)).unwrap());
        assert_eq!(vault.evict_marked(owner, 10, MARKED_AT).unwrap(), 0);
        assert_eq!(
            vault.mark_evictable(owner, &[chunk(1)], MARKED_AT).unwrap(),
            1
        );
    }
}
