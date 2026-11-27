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

use itsanas_crypto::{ChunkId, DeviceId, UserId};
use redb::{Database, TableDefinition};

use crate::{
    blob::BlobStore,
    error::{Result, StoreError},
};

mod chunks;
mod claims;
mod disk_check;
mod eviction;
mod owners;
mod rebuild;
mod segments;
use disk_check::CursorMemo;
pub use disk_check::{CHECK_CURSOR_STEP, DiskCheck, MAX_CHECK_BATCH};

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

/// Most claims handed out for one account in one answer, the most recently
/// presented first ([`Vault::claims_for`]). The response is a bounded sample,
/// not the account's full roster: up to 20 live devices are allowed since
/// 2026-11-20, and withdrawn claims that still present can also appear. Older
/// clients refuse a longer answer, so this does not grow.
pub const MAX_CLAIMS_PER_ANSWER: usize = 16;

/// `device` -> the number of the presentation that last kept its claim, so
/// an answer can hand out the claims presented last ([`Vault::claims_for`]).
const CLAIM_SEEN: TableDefinition<'_, &[u8], u64> = TableDefinition::new("vault_claim_seen");
/// In [`TOTALS`]: presentations kept so far, the clock of [`CLAIM_SEEN`].
const CLAIM_PRESENTATIONS: &str = "claim_presentations";

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
    /// Chunks read whole: what a request that only asks "do you have it"
    /// must never cost ([`Self::chunk_reads`]).
    reads: std::sync::atomic::AtomicU64,
    /// This process may have left the disk and the index apart, so `Drop`
    /// must not mark the close clean.
    ///
    /// Set until `open` has finished (a rebuild that failed half-way is the
    /// state the mark exists to catch), and by any chunk write that did not
    /// reach its commit -- an error or a panic after the blob was written or
    /// unlinked, ENOSPC on the redb commit being the likely one. Without it a
    /// clean shutdown after such a failure blessed the drift for good.
    suspect: std::sync::atomic::AtomicBool,
    /// The disk check's cursor between writes ([`CHECK_CURSOR_STEP`]);
    /// `None` until the first check of this process, which reads it from disk.
    check_cursor: std::sync::Mutex<Option<CursorMemo>>,
    /// Each owner's chunk store, opened once ([`Self::blobs_for`]): in pack
    /// files an open is a database and a replay, and every chunk a peer sent
    /// paid one (14 ms a put on the laptop against 3 in blob files). At most
    /// [`OWNERS_KEPT_OPEN`].
    stores: std::sync::Mutex<std::collections::BTreeMap<UserId, BlobStore>>,
}

/// How many owners' chunk stores a vault keeps open: a host holds a few
/// accounts, and each open pack index holds a file and its cache.
const OWNERS_KEPT_OPEN: usize = 64;

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

/// Commit `txn` only if it `changed` anything; abort it otherwise. An
/// aborted write writes nothing, where a commit of nothing still costs a
/// copy-on-write header and its pages (ROADMAP, "An idle node's disk
/// writes").
fn commit_if(txn: redb::WriteTransaction, changed: bool) -> Result<()> {
    if changed {
        crate::commit(txn)?;
    } else {
        txn.abort()?;
    }
    Ok(())
}

fn chunk_from_key(bytes: &[u8]) -> Option<ChunkId> {
    if bytes.len() != 64 {
        return None;
    }
    let mut chunk = [0u8; 32];
    chunk.copy_from_slice(&bytes[32..]);
    Some(ChunkId::from_bytes(chunk))
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
            let _ = txn.open_table(CLAIM_SEEN)?;
            let _ = txn.open_table(EVICT_QUOTA)?;
        }
        txn.commit()?;

        let vault = Self {
            root,
            db,
            chunk_writes: std::sync::Mutex::new(()),
            reads: std::sync::atomic::AtomicU64::new(0),
            // Suspect until the rebuild below has finished: if it fails, the
            // vault is dropped here and must leave the mark as it found it.
            suspect: std::sync::atomic::AtomicBool::new(true),
            check_cursor: std::sync::Mutex::new(None),
            stores: std::sync::Mutex::new(std::collections::BTreeMap::new()),
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
        let mut stores = self
            .stores
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(store) = stores.get(&owner) {
            return Ok(store.clone());
        }
        let store = BlobStore::open(self.root.join("owners").join(owner.to_hex()))?;
        if stores.len() >= OWNERS_KEPT_OPEN {
            stores.clear();
        }
        stores.insert(owner, store.clone());
        Ok(store)
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
    use itsanas_crypto::{DeviceKeys, MasterSecret, ObjectId, SecretBytes, UserKeys};

    use crate::oplog::{FileEntry, LogEntry, Operation, SegmentEnvelope};

    pub(super) fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().expect("temp dir");
        let vault = Vault::open(dir.path()).expect("open");
        (dir, vault)
    }

    pub(super) fn keys(byte: u8) -> UserKeys {
        UserKeys::derive(&MasterSecret::from_bytes([byte; 32]))
    }
    // Unix only: Windows refuses to rename a directory holding an open file,
    // so an unplug cannot be simulated there; an unplugged disk is a gone
    // drive letter, which `Vault::blobs_for` meets the same way.
    #[cfg(unix)]
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

    pub(super) fn device(byte: u8) -> DeviceKeys {
        DeviceKeys::from_seed(&SecretBytes::new([byte; 32]))
    }

    pub(super) fn entry(sequence: u64) -> LogEntry {
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

    pub(super) fn segment(
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

    /// The pledge reads `held_bytes`; `stats` walks the disk. A difference is
    /// a pledge enforced against a number that is not what the disk holds.
    pub(super) fn assert_total_is_the_walk(vault: &Vault, when: &str) {
        assert_eq!(
            vault.held_bytes().unwrap(),
            vault.stats().unwrap().bytes,
            "after {when}, the running total the pledge reads is not what the \
             vault holds on disk: the host would refuse or accept at the wrong byte"
        );
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

    /// Every chunk a peer sent opened its owner's pack index again, a database
    /// and a replay: 4106 one-byte chunks took a minute in packs against 13 s
    /// in blob files. Sabotage: do not keep the owner's store.
    #[test]
    fn red_team_a_vault_opens_an_owners_packs_once_not_once_a_chunk() {
        let dir = tempfile::tempdir().unwrap();
        let owner = keys(1).user_id();
        std::fs::create_dir_all(dir.path().join("owners").join(owner.to_hex())).unwrap();
        drop(BlobStore::open_packed(dir.path().join("owners").join(owner.to_hex())).unwrap());
        let vault = Vault::open(dir.path()).unwrap();
        let before = crate::pack::OPENED.with(std::cell::Cell::get);
        for index in 0..20u8 {
            vault
                .put_chunk(owner, &ChunkId::from_bytes([index; 32]), &[index])
                .unwrap();
        }
        let opened = crate::pack::OPENED.with(std::cell::Cell::get) - before;
        assert!(
            opened <= 1,
            "twenty chunks for one owner opened its pack index {opened} times: every chunk a host receives pays a database open and a replay"
        );
    }
}
