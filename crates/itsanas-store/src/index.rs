//! The local index: what files exist, which chunks they are made of, and which
//! chunks are therefore still worth keeping.
//!
//! Backed by [`redb`], a pure-Rust embedded key/value store with real ACID
//! transactions. That property is not a luxury here. Adding a file touches two
//! tables — the path entry and every chunk's reference count — and a crash
//! between them would either leak chunks forever or, far worse, drop the
//! reference count of a chunk that is still in use and let garbage collection
//! delete live data. Both updates happen in one transaction.

use std::{
    path::Path,
    sync::{Mutex, atomic::AtomicU64},
    time::{SystemTime, UNIX_EPOCH},
};

use itsanas_crypto::{ChunkId, DeviceId, SymmetricKey};
use redb::{Database, ReadableTable, TableDefinition};

use crate::{
    error::{Result, StoreError},
    reliability::Reliability,
};

mod audits;
mod chain;
mod chunks;
mod coverage;
mod files;
mod ledger;
mod losses;
mod peer_notes;
mod placement;
mod roster;
mod upgrade;
pub use placement::{CensusRow, ShortPage, TRIGGER_LIMIT};
use placement::{DEVICE_NETWORK, DEVICE_PASSED, DEVICE_UPTIME, RELEASED, SHORT_SINCE};

/// Path → postcard-encoded [`FileEntry`](crate::oplog::FileEntry).
/// How fresh a contact stamp (`DEVICE_SEEN`) must be for a contact that
/// changes nothing else to leave it alone.
///
/// Every reader of the stamp asks in days ([`holders::CONFIRMED_FOR`](crate::holders::CONFIRMED_FOR), the
/// release window, `LIVE_FOR`) and the uptime ring in hours, so a stamp up
/// to fifteen minutes old answers all of them as a fresh one would. What it
/// saves is a commit per peer per round -- about twenty kilobytes each on a
/// copy-on-write engine (ROADMAP, "An idle node's disk writes").
pub const SEEN_STEP: u64 = 15 * 60;

const FILES: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("files");
/// Chunk address → how many file slots reference it.
const CHUNK_REFS: TableDefinition<'_, &[u8], u64> = TableDefinition::new("chunk_refs");
/// Chunk address → Unix second at which its reference count reached zero.
const UNREFERENCED: TableDefinition<'_, &[u8], u64> = TableDefinition::new("unreferenced");
/// Chain position → postcard-encoded [`SegmentEnvelope`].
const SEGMENTS: TableDefinition<'_, u64, &[u8]> = TableDefinition::new("segments");
/// Sequence number → postcard-encoded [`LogEntry`] not yet sealed into a segment.
///
/// Without this table, entries would accumulate in memory between segment
/// flushes and a power cut would leave the index describing files that no
/// segment ever announced — peers would never learn about them.
const PENDING: TableDefinition<'_, u64, &[u8]> = TableDefinition::new("pending");
/// Path → postcard-encoded [`Tombstone`] for a deleted file.
const TOMBSTONES: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("tombstones");
/// Path → postcard-encoded [`LocalState`]: what this device last saw on disk.
///
/// Purely local. Never replicated, never signed, meaningless on another
/// machine — it records what *this* device put in *its* folder, which is what
/// makes a missing file distinguishable from a deleted one.
const LOCAL: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("local_state");
/// Small named scalars: next sequence number, head segment, chain length.
const META: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("meta");

/// Which devices are known to hold each chunk.
///
/// Key is `chunk_id || device_id`, so every holder of one chunk is a contiguous
/// range. Value is this node's clock when the holder acknowledged it.
///
/// This is what replaced the coordinator's signed node-set epoch: an owner who
/// already keeps a log of their own chunks can record where they put them, and
/// then no global membership list has to be agreed by anybody. See
/// `holders.rs` for the argument.
const HOLDERS: TableDefinition<'_, &[u8], u64> = TableDefinition::new("holders");

/// When this node last walked its whole chunk set against one peer, per device.
///
/// A round that reconciles in one hash no longer touches the ledger at all when
/// the two sides agree — which is the point, and which would let every record
/// for that peer age quietly past [`holders::CONFIRMED_FOR`] until `release`
/// stopped working and `coverage` reported no copies. So the full walk still
/// happens, just rarely: once the last one is older than
/// [`holders::REFRESH_AFTER`], which is a quarter of the window and leaves
/// three further chances before anything could go stale.
///
/// One row per peer, against sixteen million reads per round for a terabyte.
const LEDGER_REFRESHED: TableDefinition<'_, &[u8], u64> = TableDefinition::new("ledger_refreshed");
/// When each peer last refused a chunk because its pledge was full.
const PEER_FULL: TableDefinition<'_, &[u8], u64> = TableDefinition::new("peer_full");

/// Device → until when it said it is draining (§8 0w (7)).
///
/// A device on its way out asks its owners to stop counting on it *before* it
/// goes, so repair places a replacement while the departing copy still exists
/// as a safety margin. Its records stay -- they are true -- but stop counting
/// towards replication and coverage until the row expires.
///
/// Expires by itself, never later than [`holders::MAX_DRAIN`] after it was
/// written: a drain interrupted by Ctrl-C, or a machine that rejoined, would
/// otherwise be excluded for ever, and a ledger that undercounts for ever
/// over-replicates for ever. `Leaving` removes the row with the records.
const DRAINING: TableDefinition<'_, &[u8], u64> = TableDefinition::new("draining_until");

/// Device → 1 if a claim signed by this account names it, 0 if a revocation
/// did. This account's device roster (HANDOVER §10 7): a chain signed by a
/// device not here is not applied once the roster has any row, so a host that
/// re-signs a lifted body under its own key replays nothing. A revocation is
/// final, like the claim it answers.
const ROSTER: TableDefinition<'_, &[u8], u8> = TableDefinition::new("account_roster");

/// Device → the head of that device's chain when it was last applied with
/// nothing left over.
///
/// A marker, not a cache of work. Comparing it against the vault's current head
/// answers "is there anything here I have not managed to apply yet" in two
/// lookups, instead of walking and decoding an entire chain to find out.
///
/// Only written when a round completes with **zero** deferrals, so a device
/// whose chunks were unreachable stays marked as outstanding and gets replayed
/// on the next round that can move content.
const APPLIED: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("applied_heads");

/// device id -> the longest chain length that device signed in a presence
/// this node read (`Presence::head`): what its log is known to hold at
/// least, whoever served it. Only grows.
const KNOWN_HEADS: TableDefinition<'_, &[u8], u64> = TableDefinition::new("known_heads");

/// The holder ledger read out of both of its tables, for cross-checking.
///
/// A pair of vectors would do; a named type makes the two halves impossible to
/// swap at a call site, which matters for something whose entire purpose is
/// comparing them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HolderOrderings {
    /// Pairs as the chunk-first table holds them.
    pub by_chunk: Vec<(ChunkId, DeviceId)>,
    /// The same pairs as the device-first table holds them.
    pub by_device: Vec<(ChunkId, DeviceId)>,
}

/// The same holdings, keyed device-first and ordered by a **keyed** tag.
///
/// Written and removed in the same transaction as [`HOLDERS`], so the two
/// cannot disagree. It exists because the two questions asked of that ledger
/// have opposite key shapes, and answering one of them with the wrong ordering
/// is a full table scan — fourteen million rows per audit round at a terabyte.
///
/// The order within a device is `BLAKE3_keyed(audit key, chunk)` and not the
/// chunk id, because the audit picks a record by seeking to a random cursor:
/// that weights each record by the gap before it, and a host that can compute
/// the gaps keeps the chunks behind the widest ones. See
/// [`holders::audit_tag`].
const HOLDINGS: TableDefinition<'_, &[u8], u64> = TableDefinition::new("holdings_by_device");

/// `device` → how many holder records name it: how much of this node's data
/// that device is recorded as holding, in one lookup (§8 1c).
///
/// A host deciding how much to store for an account needs what that account's
/// devices hold for it, on every offer; counting `HOLDINGS` rows would be a
/// range scan per chunk stored. Kept in the transaction that adds or removes
/// the row it counts, like the vault's byte totals, so the two cannot drift
/// while running; built once from `HOLDINGS` for a store written before it.
const HOLDER_COUNTS: TableDefinition<'_, &[u8], u64> = TableDefinition::new("holder_counts");
/// Set in `META` once `HOLDER_COUNTS` has been built.
const HOLDER_COUNTS_BUILT: &str = "holder_counts_built";

/// Whether `device` has an unexpired [`DRAINING`] row at `now`.
fn draining_in(
    table: &impl ReadableTable<&'static [u8], u64>,
    device: &DeviceId,
    now: u64,
) -> Result<bool> {
    Ok(table
        .get(device.as_bytes().as_slice())?
        .is_some_and(|until| until.value() > now))
}

/// Whether `device`'s copies may be counted on: neither draining nor paused.
/// A paused host failed its audits, and the record it refreshes by saying
/// `Hosted` is a word, not a copy.
fn counted_in(
    draining: &impl ReadableTable<&'static [u8], u64>,
    reliability: &impl ReadableTable<&'static [u8], &'static [u8]>,
    device: &DeviceId,
    now: u64,
) -> Result<bool> {
    if draining_in(draining, device, now)? {
        return Ok(false);
    }
    Ok(!reliability
        .get(device.as_bytes().as_slice())?
        .map(|value| postcard::from_bytes::<Reliability>(value.value()))
        .transpose()?
        .is_some_and(|record| record.paused))
}

/// Add `delta` to `device`'s count, never below zero.
fn bump(counts: &mut redb::Table<'_, &[u8], u64>, device: &DeviceId, delta: i64) -> Result<()> {
    let key = device.as_bytes().as_slice();
    let now = counts.get(key)?.map_or(0, |value| value.value());
    let next = if delta >= 0 {
        now.saturating_add(delta.unsigned_abs())
    } else {
        now.saturating_sub(delta.unsigned_abs())
    };
    if next == 0 {
        counts.remove(key)?;
    } else {
        counts.insert(key, next)?;
    }
    Ok(())
}

/// Chunks this node's files need whose bytes are not on this disk.
///
/// A queue, not a cache. Two things find local loss and they used to ignore
/// each other: `doctor` walks every file and knows the whole answer in one
/// pass, and the daemon's repair scan samples a slice per round and takes
/// **fifty-five days** to reach a given chunk at a terabyte. Somebody running
/// `doctor` because a file would not open therefore learned the answer and had
/// no way to act on it.
///
/// Both now write here, and repair drains this before it samples anything. A
/// human asking the question is the fastest detector this system has; not
/// wiring it to the thing that fixes losses was leaving the best signal on the
/// floor.
///
/// Entries are removed when the chunk comes back, or when it turns out to be
/// present after all — a blob can reappear because a filesystem was repaired
/// or a backup restored underneath.
const LOSSES: TableDefinition<'_, &[u8], u64> = TableDefinition::new("known_losses");

/// Tables written by earlier versions that nothing reads any more.
///
/// Dropped on open. Naming them in one list is the point: the alternative is a
/// file that accumulates a fossil per schema change and no record of which name
/// is current.
const SUPERSEDED_TABLES: &[&str] = &["reliability"];

/// Device → the one chunk it was last offered while its sanction was in force.
///
/// A paused peer is sent a single chunk per round so that it has something it
/// can prove. That only works if the audit then *asks about that chunk*, and
/// the audit picks its questions from the holder ledger, where one fresh record
/// sits among however many thousand stale ones. Written down explicitly rather
/// than inferred from a timestamp, because "the newest record" is precisely the
/// one an ordered audit never reaches.
///
/// Cleared when the peer passes, which is the moment the sanction lifts.
const PROBES: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("probe_offered");

/// Device → what auditing it has shown, over time.
///
/// Detection without memory is not a defence: a host that discards what it
/// accepts is caught and re-sent every round, which costs the owner the full
/// upload each time and the host nothing at all. This is what remembers that it
/// is the fourth time.
/// Named `_v2` because the record gained a field and postcard is not
/// self-describing: old rows would not decode, and a read that errors on every
/// peer is worse than one that starts everybody clean. What is lost on upgrade
/// is diagnostic counters and a sanction that re-earns itself within three
/// rounds, which is a cheap price for not having to guess at old bytes.
const RELIABILITY: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("peer_reliability_v2");

/// Device → this node's clock when it was last heard from.
///
/// Separate from the per-chunk holder timestamps, and the difference is the
/// whole point. "Does this device still hold this chunk" is answered by the
/// audit, one slice per round. "Is this device still there" is answered every
/// time it answers anything -- and only the second question scales.
///
/// The arithmetic that forced this: the audit checks sixteen chunks per peer
/// per round at three hundred seconds a round, so a chunk waits
/// `chunks / 16` rounds for its turn. Past about sixty-four thousand chunks --
/// four or five gigabytes -- the oldest chunk cannot be re-confirmed inside a
/// fortnight, and a per-chunk freshness rule would report **zero complete
/// copies** for a perfectly healthy fleet whose every audit passes. The rule
/// would break at exactly the size this project is for.
///
/// A new table rather than a field on `Reliability`, because that record is
/// postcard-encoded and adding a field would make every existing row
/// undecodable on machines already running.
const DEVICE_SEEN: TableDefinition<'_, &[u8], u64> = TableDefinition::new("device_last_seen");

const META_NEXT_SEQUENCE: &str = "next_sequence";
const META_HEAD_SEGMENT: &str = "head_segment";
const META_CHAIN_LENGTH: &str = "chain_length";

/// Seconds since the Unix epoch.
///
/// A clock before 1970 is a misconfigured machine, not an attack; treating it
/// as zero keeps garbage collection conservative (nothing looks old enough to
/// collect) rather than destructive.
/// Seconds since the epoch, by this machine's clock.
///
/// Exported because callers that must decide "is this record still standing in
/// for a live copy" need the same clock the records were written with, and
/// three implementations of the same four lines is how two of them end up
/// disagreeing about what a second is.
#[must_use]
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The local metadata database.
#[derive(Debug)]
pub struct Index {
    db: Database,
    /// Scrambles the order in which each peer's holdings are audited.
    ///
    /// Held in memory, derived from the master secret on every open, and never
    /// written to this file — writing it here would hand it to anybody who
    /// reads the file, and the whole point is that the *remote host* cannot
    /// compute it.
    audit_key: SymmetricKey,
    /// Sum of the sizes of every file in `FILES`, once somebody has asked.
    ///
    /// Kept rather than recounted because every write has to know it, and a
    /// folder's first import is thousands of writes: recounting each time
    /// would make that import quadratic. `None` until first asked. Every writer
    /// of `FILES` holds this lock across its transaction and adjusts the total
    /// after committing, so a count taken on another thread cannot be replaced
    /// by one read before the commit it missed.
    local_bytes: Mutex<Option<u64>>,
    /// Bumped after every committed write to `CHUNK_REFS`.
    refs_changed: AtomicU64,
    /// The last [`Index::chunk_summary`], with the [`Self::refs_changed`] it
    /// was read at. A quiet round asks for it once per peer, and computing it
    /// reads every chunk id: sixteen million rows at a terabyte, every five
    /// minutes, on an SD card (ROADMAP, "What a round still costs locally").
    summary: Mutex<Option<(u64, Vec<crate::summary::Digest>)>>,
    /// How many times the summary was read from the table, for its test.
    summary_reads: AtomicU64,
}

/// The index's filename inside a store root.
///
/// Named once because two callers need it: [`Index::open`]'s caller, which
/// opens it, and [`Index::is_locked`], which asks whether somebody else already
/// has. A literal in both places is a rename away from a probe that silently
/// always answers "not locked".
pub const INDEX_FILE: &str = "index.redb";

/// How long [`Index::open`] waits out a lock before reporting
/// [`StoreError::Locked`]: far longer than an [`Index::is_locked`] probe holds
/// it, short enough that a command beside a running daemon still answers at
/// once to a person.
pub const LOCK_PATIENCE: std::time::Duration = std::time::Duration::from_secs(2);

impl Index {
    /// Whether another process already holds this index.
    ///
    /// # Why this exists
    ///
    /// `itsanas status` answers from the daemon's snapshot when the store is
    /// locked -- which is the normal state of a machine doing its job. It
    /// reached that arm through an open that resolves the passphrase *first*,
    /// so the fallback was unreachable exactly when it applied: the command
    /// demanded the keystore secret and would then have printed a plaintext
    /// file sitting in the node home. Asking about the lock has to come before
    /// asking for a key.
    ///
    /// Opening and immediately dropping takes the same lock `open` would and
    /// releases it. It is **not** free of side effects, and an earlier version
    /// of this comment claimed it was: redb repairs a database that was not
    /// closed cleanly, so probing a store left behind by a crash runs that
    /// repair. That is safe here for a reason worth stating rather than
    /// assuming — the probe only reaches the repair when the database is *not*
    /// locked, which is exactly when the caller is about to open it for real
    /// one line later and trigger the identical repair. The probe moves the
    /// work earlier; it does not add any.
    ///
    /// A missing file, or one redb cannot read at all, answers "not locked":
    /// the caller's ordinary open then produces the real error with the real
    /// advice, rather than this guessing on its behalf.
    #[must_use]
    pub fn is_locked(path: impl AsRef<Path>) -> bool {
        matches!(
            Database::open(path.as_ref()),
            Err(redb::DatabaseError::DatabaseAlreadyOpen)
        )
    }

    /// Open or create the index at `path`.
    ///
    /// `audit_key` orders each peer's holdings for the audit. It is a parameter
    /// rather than a default because a fixed one would silently give every
    /// installation the same order, which is the property being removed.
    pub fn open(path: impl AsRef<Path>, audit_key: SymmetricKey) -> Result<Self> {
        let path = path.as_ref();
        // `is_locked` probes by opening, which holds the same exclusive lock
        // for an instant. A daemon starting in that instant -- the tray polls
        // `status --brief` while the logon task starts the daemon -- would
        // fail with `Locked` and stay down. A probe lets go within
        // milliseconds and a running daemon never does, so a short retry
        // tells the two apart: `Locked` is reported once the lock has been
        // held for `LOCK_PATIENCE`.
        let started = std::time::Instant::now();
        let mut tries: u64 = 0;
        let db = loop {
            match Database::create(path) {
                Ok(db) => break db,
                Err(redb::DatabaseError::DatabaseAlreadyOpen)
                    if started.elapsed() < LOCK_PATIENCE =>
                {
                    // A pause that varies: at a fixed 20 ms, a probe coming back
                    // just as regularly met every try for the whole patience,
                    // once in 100 opens on the ARM runner.
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(1 + tries % 7));
                }
                // redb takes an exclusive lock on the file. Reporting that as
                // a generic database error leaves the operator staring at
                // "cannot acquire lock" with no idea which of their own
                // processes is holding it, so it gets its own variant and its
                // own advice.
                Err(redb::DatabaseError::DatabaseAlreadyOpen) => {
                    return Err(StoreError::Locked(path.to_owned()));
                }
                Err(other) => return Err(StoreError::from(other)),
            }
        };

        // Create every table up front. redb returns an error when a read
        // transaction opens a table that has never been written, so a fresh
        // store would otherwise fail its first read rather than report empty.
        let txn = db.begin_write()?;
        {
            let _ = txn.open_table(FILES)?;
            let _ = txn.open_table(CHUNK_REFS)?;
            let _ = txn.open_table(UNREFERENCED)?;
            let _ = txn.open_table(SEGMENTS)?;
            let _ = txn.open_table(PENDING)?;
            let _ = txn.open_table(TOMBSTONES)?;
            let _ = txn.open_table(LOCAL)?;
            let _ = txn.open_table(META)?;
            let _ = txn.open_table(HOLDERS)?;
            let _ = txn.open_table(APPLIED)?;
            let _ = txn.open_table(KNOWN_HEADS)?;
            let _ = txn.open_table(RELIABILITY)?;
            let _ = txn.open_table(DEVICE_SEEN)?;
            let _ = txn.open_table(LEDGER_REFRESHED)?;
            let _ = txn.open_table(PEER_FULL)?;
            let _ = txn.open_table(DRAINING)?;
            let _ = txn.open_table(HOLDINGS)?;
            let _ = txn.open_table(HOLDER_COUNTS)?;
            let _ = txn.open_table(PROBES)?;
            let _ = txn.open_table(LOSSES)?;
            // Placement's tables. Created here like every other, so a store
            // written before them reads as "no rings, nothing queued" rather
            // than failing its first read.
            let _ = txn.open_table(DEVICE_UPTIME)?;
            let _ = txn.open_table(SHORT_SINCE)?;
            let _ = txn.open_table(RELEASED)?;
            let _ = txn.open_table(DEVICE_PASSED)?;
            let _ = txn.open_table(DEVICE_NETWORK)?;
            let _ = txn.open_table(ROSTER)?;
        }
        txn.commit()?;

        let index = Self {
            db,
            audit_key,
            local_bytes: Mutex::new(None),
            refs_changed: AtomicU64::new(0),
            summary: Mutex::new(None),
            summary_reads: AtomicU64::new(0),
        };
        index.rebuild_holdings_if_stale()?;
        index.build_holder_counts_if_missing()?;
        index.drop_superseded_tables()?;
        Ok(index)
    }

    /// How many of this node's chunks `device` is recorded as holding.
    pub fn held_by(&self, device: &DeviceId) -> Result<u64> {
        let txn = self.db.begin_read()?;
        Ok(txn
            .open_table(HOLDER_COUNTS)?
            .get(device.as_bytes().as_slice())?
            .map_or(0, |value| value.value()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oplog::{FileEntry, Tombstone};

    /// The same as `index()`, under a name a local binding does not shadow.
    ///
    /// A test that wants two stores cannot call `index()` for the second one:
    /// the first has already bound the name.
    pub(super) fn fresh() -> (tempfile::TempDir, Index) {
        index()
    }

    pub(super) fn index() -> (tempfile::TempDir, Index) {
        let dir = tempfile::tempdir().expect("temp dir");
        let index = Index::open(dir.path().join("index.redb"), test_audit_key()).expect("open");
        (dir, index)
    }

    /// `is_locked` takes the lock for an instant; an open landing in that
    /// instant -- the daemon starting at logon while the tray polls -- must
    /// wait it out, not fail with `Locked` and leave the node down. A lock
    /// really held still reads as `Locked`, once `LOCK_PATIENCE` has passed.
    #[test]
    fn red_team_an_open_racing_a_lock_probe_still_opens() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(INDEX_FILE);
        drop(Index::open(&path, test_audit_key()).expect("create"));

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let prober = {
            let (path, stop) = (path.clone(), stop.clone());
            std::thread::spawn(move || {
                // A poller, not a spin: the tray asks every few seconds. A
                // probe in a tight loop holds the lock nearly all the time
                // (on Linux an open never finds it free), which no caller
                // does; 5 ms apart is still far denser than the tray.
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = Index::is_locked(&path);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            })
        };
        let mut failed = 0;
        for _ in 0..100 {
            match Index::open(&path, test_audit_key()) {
                Ok(index) => drop(index),
                Err(StoreError::Locked(_)) => failed += 1,
                Err(other) => panic!("open failed for another reason: {other}"),
            }
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        prober.join().expect("prober");
        assert_eq!(
            failed, 0,
            "{failed} of 100 opens met a status probe's lock and gave up: a daemon \
             starting at logon beside the tray stays down"
        );

        let held = Index::open(&path, test_audit_key()).expect("open");
        assert!(
            matches!(
                Index::open(&path, test_audit_key()),
                Err(StoreError::Locked(_))
            ),
            "a lock held for good must still be reported, after the wait"
        );
        drop(held);
    }

    pub(super) fn chunk(byte: u8) -> ChunkId {
        ChunkId::from_bytes([byte; 32])
    }

    pub(super) fn entry(chunks: &[u8]) -> FileEntry {
        FileEntry {
            size: 10,
            modified_unix: 1_700_000_000,
            content_hash: [0; 32],
            chunks: chunks.iter().copied().map(chunk).collect(),
            version: crate::version::VersionVector::new(),
            author: itsanas_crypto::DeviceId::from_bytes([1; 32]),
        }
    }

    pub(super) fn tombstone() -> Tombstone {
        Tombstone {
            version: crate::version::VersionVector::new(),
            removed_unix: 1_700_000_100,
            author: itsanas_crypto::DeviceId::from_bytes([1; 32]),
        }
    }

    #[test]
    fn a_fresh_index_reads_as_empty_rather_than_erroring() {
        let (_dir, index) = index();
        assert_eq!(index.files().unwrap(), Vec::new());
        assert_eq!(index.unreferenced_chunks().unwrap(), Vec::new());
        assert_eq!(index.chain_length().unwrap(), 0);
        assert_eq!(index.head_segment().unwrap(), None);
        assert_eq!(index.next_sequence().unwrap(), 1);
    }

    // -------------------------------------------------------------- holders

    /// One fixed audit key for every test index.
    ///
    /// Fixed so the tests are deterministic; a *fixed* key in production would
    /// be the bug this whole ordering exists to remove, which is why
    /// `Index::open` demands one rather than defaulting.
    pub(super) fn test_audit_key() -> itsanas_crypto::SymmetricKey {
        itsanas_crypto::SecretBytes::new([0x5A; 32])
    }

    pub(super) fn device(byte: u8) -> DeviceId {
        DeviceId::from_bytes([byte; 32])
    }

    /// The order the audit actually walks a device's holdings in.
    ///
    /// Recovered the only way a caller can: sweep cursors from low to high and
    /// note each new record as it first appears. Coverage is partial — the
    /// narrowest gaps need a finer sweep than is worth running — and that is
    /// fine, because what these tests measure is whether the sequence is
    /// *sorted by chunk id*, which does not need every record to show up.
    pub(super) fn observed_audit_order(index: &Index, holder: &DeviceId) -> Vec<ChunkId> {
        let mut order: Vec<ChunkId> = Vec::new();
        for high in 0..=255u8 {
            for low in (0..=255u8).step_by(8) {
                let cursor = [high, low, 0, 0, 0, 0, 0, 0];
                for drawn in index.chunks_to_challenge(holder, &[cursor]).unwrap() {
                    if !order.contains(&drawn) {
                        order.push(drawn);
                    }
                }
            }
        }
        order
    }

    /// What share of adjacent steps in `order` move *up* the chunk-id ordering.
    ///
    /// One if the audit walks chunks in id order, which is what the unkeyed
    /// version did. About a half if the two orders are independent.
    pub(super) fn share_ascending_by_id(order: &[ChunkId]) -> f64 {
        if order.len() < 2 {
            return 1.0;
        }
        let up = order.windows(2).filter(|pair| pair[0] < pair[1]).count();
        #[allow(clippy::cast_precision_loss)]
        {
            up as f64 / (order.len() - 1) as f64
        }
    }
}
