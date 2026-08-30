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
    collections::BTreeMap,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use itsanas_crypto::{ChunkId, DeviceId, ObjectId};
use redb::{Database, ReadableTable, TableDefinition};

use crate::{
    error::{Result, StoreError},
    holders::{self, AtRisk, Holder},
    local::LocalState,
    oplog::{FileEntry, LogEntry, SegmentEnvelope, Tombstone},
    reliability::Reliability,
};

/// Path → postcard-encoded [`FileEntry`].
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

/// The same holdings, keyed device-first.
///
/// Written and removed in the same transaction as [`HOLDERS`], so the two
/// cannot disagree. It exists because the two questions asked of that ledger
/// have opposite key shapes, and answering one of them with the wrong ordering
/// is a full table scan — fourteen million rows per audit round at a terabyte.
const HOLDINGS: TableDefinition<'_, &[u8], u64> = TableDefinition::new("holdings_by_device");

/// Device → what auditing it has shown, over time.
///
/// Detection without memory is not a defence: a host that discards what it
/// accepts is caught and re-sent every round, which costs the owner the full
/// upload each time and the host nothing at all. This is what remembers that it
/// is the fourth time.
const RELIABILITY: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("reliability");

const META_NEXT_SEQUENCE: &str = "next_sequence";
const META_HEAD_SEGMENT: &str = "head_segment";
const META_CHAIN_LENGTH: &str = "chain_length";

/// Seconds since the Unix epoch.
///
/// A clock before 1970 is a misconfigured machine, not an attack; treating it
/// as zero keeps garbage collection conservative (nothing looks old enough to
/// collect) rather than destructive.
pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The local metadata database.
#[derive(Debug)]
pub struct Index {
    db: Database,
}

impl Index {
    /// Open or create the index at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let db = Database::create(path).map_err(|error| match error {
            // redb takes an exclusive lock on the file. Reporting that as a
            // generic database error leaves the operator staring at "cannot
            // acquire lock" with no idea which of their own processes is
            // holding it, so it gets its own variant and its own advice.
            redb::DatabaseError::DatabaseAlreadyOpen => StoreError::Locked(path.to_owned()),
            other => StoreError::from(other),
        })?;

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
            let _ = txn.open_table(RELIABILITY)?;
            let _ = txn.open_table(HOLDINGS)?;
        }
        txn.commit()?;

        Ok(Self { db })
    }

    // ---------------------------------------------------------------- files

    /// Insert or replace the entry for `path`, adjusting reference counts.
    ///
    /// Returns the chunks that dropped to zero references as a result, which is
    /// what an overwrite of a large file produces and what garbage collection
    /// will later reclaim.
    pub fn put_file(&self, path: &str, entry: &FileEntry) -> Result<Vec<ChunkId>> {
        let encoded = postcard::to_stdvec(entry)?;
        let txn = self.db.begin_write()?;
        let newly_unreferenced;

        {
            let mut files = txn.open_table(FILES)?;
            let mut refs = txn.open_table(CHUNK_REFS)?;
            let mut unreferenced = txn.open_table(UNREFERENCED)?;

            let previous: Option<FileEntry> = match files.get(path)? {
                Some(value) => Some(postcard::from_bytes(value.value())?),
                None => None,
            };

            let mut delta: BTreeMap<[u8; 32], i64> = BTreeMap::new();
            if let Some(previous) = &previous {
                for chunk in &previous.chunks {
                    *delta.entry(chunk.to_bytes()).or_default() -= 1;
                }
            }
            for chunk in &entry.chunks {
                *delta.entry(chunk.to_bytes()).or_default() += 1;
            }

            newly_unreferenced = Self::apply_reference_delta(&mut refs, &mut unreferenced, &delta)?;

            files.insert(path, encoded.as_slice())?;

            // A live file supersedes any tombstone at the same path, and the
            // two must never coexist — a path that is both present and deleted
            // would resolve differently depending on which table was consulted.
            txn.open_table(TOMBSTONES)?.remove(path)?;
        }

        txn.commit()?;
        Ok(newly_unreferenced)
    }

    /// Remove `path`, leaving `tombstone` in its place.
    ///
    /// Returns the chunks that lost their last reference.
    pub fn remove_file(&self, path: &str, tombstone: &Tombstone) -> Result<Vec<ChunkId>> {
        let encoded_tombstone = postcard::to_stdvec(tombstone)?;
        let txn = self.db.begin_write()?;
        let newly_unreferenced;

        {
            let mut files = txn.open_table(FILES)?;
            let mut refs = txn.open_table(CHUNK_REFS)?;
            let mut unreferenced = txn.open_table(UNREFERENCED)?;
            txn.open_table(TOMBSTONES)?
                .insert(path, encoded_tombstone.as_slice())?;

            let previous: Option<FileEntry> = match files.get(path)? {
                Some(value) => Some(postcard::from_bytes(value.value())?),
                None => None,
            };

            newly_unreferenced = match previous {
                None => Vec::new(),
                Some(previous) => {
                    let mut delta: BTreeMap<[u8; 32], i64> = BTreeMap::new();
                    for chunk in &previous.chunks {
                        *delta.entry(chunk.to_bytes()).or_default() -= 1;
                    }

                    let dropped =
                        Self::apply_reference_delta(&mut refs, &mut unreferenced, &delta)?;
                    files.remove(path)?;
                    dropped
                }
            };
        }

        txn.commit()?;
        Ok(newly_unreferenced)
    }

    /// What this device last saw on disk at `path`.
    pub fn get_local_state(&self, path: &str) -> Result<Option<LocalState>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(LOCAL)?;
        match table.get(path)? {
            Some(value) => Ok(Some(postcard::from_bytes(value.value())?)),
            None => Ok(None),
        }
    }

    /// Record what this device just wrote to, or read from, disk.
    pub fn set_local_state(&self, path: &str, state: &LocalState) -> Result<()> {
        let encoded = postcard::to_stdvec(state)?;
        let txn = self.db.begin_write()?;
        {
            txn.open_table(LOCAL)?.insert(path, encoded.as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Forget a path's disk state, after removing it from disk.
    pub fn clear_local_state(&self, path: &str) -> Result<()> {
        let txn = self.db.begin_write()?;
        {
            txn.open_table(LOCAL)?.remove(path)?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Every path this device believes it has on disk.
    pub fn local_states(&self) -> Result<Vec<(String, LocalState)>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(LOCAL)?;

        let mut out = Vec::new();
        for row in table.iter()? {
            let (key, value) = row?;
            out.push((key.value().to_owned(), postcard::from_bytes(value.value())?));
        }
        Ok(out)
    }

    /// The tombstone at `path`, if it was deleted.
    pub fn get_tombstone(&self, path: &str) -> Result<Option<Tombstone>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(TOMBSTONES)?;
        match table.get(path)? {
            Some(value) => Ok(Some(postcard::from_bytes(value.value())?)),
            None => Ok(None),
        }
    }

    /// Every tombstone, in sorted path order.
    pub fn tombstones(&self) -> Result<Vec<(String, Tombstone)>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(TOMBSTONES)?;

        let mut out = Vec::new();
        for row in table.iter()? {
            let (key, value) = row?;
            out.push((key.value().to_owned(), postcard::from_bytes(value.value())?));
        }
        Ok(out)
    }

    /// Forget a tombstone.
    ///
    /// Only safe once every device is certain to have seen the delete —
    /// otherwise a device that missed it resurrects the file.
    pub fn forget_tombstone(&self, path: &str) -> Result<()> {
        let txn = self.db.begin_write()?;
        {
            txn.open_table(TOMBSTONES)?.remove(path)?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Look up one path.
    pub fn get_file(&self, path: &str) -> Result<Option<FileEntry>> {
        let txn = self.db.begin_read()?;
        let files = txn.open_table(FILES)?;
        match files.get(path)? {
            Some(value) => Ok(Some(postcard::from_bytes(value.value())?)),
            None => Ok(None),
        }
    }

    /// Every path in the index, in sorted order.
    pub fn files(&self) -> Result<Vec<(String, FileEntry)>> {
        let txn = self.db.begin_read()?;
        let files = txn.open_table(FILES)?;

        let mut out = Vec::new();
        for row in files.iter()? {
            let (key, value) = row?;
            out.push((key.value().to_owned(), postcard::from_bytes(value.value())?));
        }
        Ok(out)
    }

    /// How many files the index holds.
    pub fn file_count(&self) -> Result<u64> {
        let txn = self.db.begin_read()?;
        let files = txn.open_table(FILES)?;
        let mut count = 0;
        for row in files.iter()? {
            row?;
            count += 1;
        }
        Ok(count)
    }

    // --------------------------------------------------------------- chunks

    /// Current reference count for `chunk`.
    pub fn reference_count(&self, chunk: &ChunkId) -> Result<u64> {
        let txn = self.db.begin_read()?;
        let refs = txn.open_table(CHUNK_REFS)?;
        Ok(refs
            .get(chunk.as_bytes().as_slice())?
            .map_or(0, |v| v.value()))
    }

    /// Chunks with no references, paired with the Unix second they became
    /// unreferenced.
    pub fn unreferenced_chunks(&self) -> Result<Vec<(ChunkId, u64)>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(UNREFERENCED)?;

        let mut out = Vec::new();
        for row in table.iter()? {
            let (key, value) = row?;
            out.push((ChunkId::from_slice(key.value())?, value.value()));
        }
        Ok(out)
    }

    /// Every chunk the index believes is live, in sorted order.
    pub fn referenced_chunks(&self) -> Result<Vec<ChunkId>> {
        let txn = self.db.begin_read()?;
        let refs = txn.open_table(CHUNK_REFS)?;

        let mut out = Vec::new();
        for row in refs.iter()? {
            let (key, value) = row?;
            if value.value() > 0 {
                out.push(ChunkId::from_slice(key.value())?);
            }
        }
        Ok(out)
    }

    /// Forget a chunk entirely, after its blob has been deleted.
    pub fn forget_chunk(&self, chunk: &ChunkId) -> Result<()> {
        let txn = self.db.begin_write()?;
        {
            // The holder ledger goes with it. Leaving the rows behind would
            // leave the repair loop working to restore the replication of a
            // chunk that no longer exists anywhere and should not.
            let mut holders = txn.open_table(HOLDERS)?;
            let mut holdings = txn.open_table(HOLDINGS)?;
            let doomed: Vec<Vec<u8>> = holders
                .range(
                    holders::range_start(chunk).as_slice()..=holders::range_end(chunk).as_slice(),
                )?
                .filter_map(|row| row.ok().map(|(key, _)| key.value().to_vec()))
                .collect();
            for key in doomed {
                // Both orderings, in this one transaction. The claim that they
                // cannot disagree is only true if every removal says so.
                if let Some((chunk, device)) = holders::split(&key) {
                    holdings.remove(holders::by_device(&device, &chunk).as_slice())?;
                }
                holders.remove(key.as_slice())?;
            }

            let mut refs = txn.open_table(CHUNK_REFS)?;
            let mut unreferenced = txn.open_table(UNREFERENCED)?;
            refs.remove(chunk.as_bytes().as_slice())?;
            unreferenced.remove(chunk.as_bytes().as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Apply a signed reference delta, maintaining the unreferenced set.
    ///
    /// Returns the chunks that fell to zero. A count that would go negative is
    /// clamped: that means the index disagrees with itself, and refusing to
    /// wrap around is what keeps the error contained to one chunk instead of
    /// turning into a `u64` underflow that makes a dead chunk look immortal.
    fn apply_reference_delta(
        refs: &mut redb::Table<'_, &'static [u8], u64>,
        unreferenced: &mut redb::Table<'_, &'static [u8], u64>,
        delta: &BTreeMap<[u8; 32], i64>,
    ) -> Result<Vec<ChunkId>> {
        let mut dropped = Vec::new();
        let timestamp = now_unix();

        for (chunk, change) in delta {
            if *change == 0 {
                continue;
            }

            let key: &[u8] = chunk.as_slice();
            let current = refs.get(key)?.map_or(0i128, |v| i128::from(v.value()));
            let updated = (current + i128::from(*change)).max(0);
            let updated = u64::try_from(updated).unwrap_or(0);

            refs.insert(key, updated)?;

            if updated == 0 {
                unreferenced.insert(key, timestamp)?;
                dropped.push(ChunkId::from_bytes(*chunk));
            } else {
                // A chunk can come back: an overwrite that restores previous
                // content re-references a chunk that was queued for collection.
                unreferenced.remove(key)?;
            }
        }

        Ok(dropped)
    }

    // ------------------------------------------------------------- segments

    /// Record a log entry that has not yet been sealed into a segment.
    ///
    /// Also advances the next-sequence counter, in the same transaction, so two
    /// entries can never be handed the same sequence number even across a crash.
    pub fn push_pending(&self, entry: &LogEntry) -> Result<()> {
        let encoded = postcard::to_stdvec(entry)?;
        let txn = self.db.begin_write()?;
        {
            let mut pending = txn.open_table(PENDING)?;
            let mut meta = txn.open_table(META)?;
            pending.insert(entry.sequence, encoded.as_slice())?;
            meta.insert(
                META_NEXT_SEQUENCE,
                (entry.sequence + 1).to_le_bytes().as_slice(),
            )?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Entries waiting to be sealed, in sequence order.
    pub fn pending_entries(&self) -> Result<Vec<LogEntry>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(PENDING)?;

        let mut out = Vec::new();
        for row in table.iter()? {
            let (_, value) = row?;
            out.push(postcard::from_bytes(value.value())?);
        }
        Ok(out)
    }

    /// Append a segment to this device's chain and drop the pending entries it
    /// covers, atomically.
    ///
    /// Doing both in one transaction is the point. Committing the segment first
    /// and clearing pending afterwards would re-emit those entries in the next
    /// segment after a crash; clearing first would lose them outright.
    ///
    /// Rejects a segment whose `previous` does not match the current head, so
    /// the on-disk chain cannot develop a hole through a local bug.
    pub fn append_segment(&self, envelope: &SegmentEnvelope) -> Result<u64> {
        let encoded = envelope.encode()?;
        let txn = self.db.begin_write()?;
        let position;

        {
            let mut segments = txn.open_table(SEGMENTS)?;
            let mut pending = txn.open_table(PENDING)?;
            let mut meta = txn.open_table(META)?;

            for sequence in envelope.first_sequence..=envelope.last_sequence {
                pending.remove(sequence)?;
            }

            let head = Self::read_object_id(&meta, META_HEAD_SEGMENT)?;
            if envelope.previous != head {
                return Err(StoreError::SegmentChainBroken {
                    segment: envelope.segment_id.short(),
                    expected: head.map_or_else(|| "none".to_owned(), |id| id.short()),
                    found: envelope
                        .previous
                        .map_or_else(|| "none".to_owned(), |id| id.short()),
                });
            }

            position = Self::read_u64(&meta, META_CHAIN_LENGTH)?;
            segments.insert(position, encoded.as_slice())?;

            meta.insert(META_CHAIN_LENGTH, (position + 1).to_le_bytes().as_slice())?;
            meta.insert(META_HEAD_SEGMENT, envelope.segment_id.as_bytes().as_slice())?;
            // Never let the counter go backwards: entries may already be
            // pending beyond this segment's range, and reusing a sequence
            // number would fork the log.
            let next = Self::read_u64(&meta, META_NEXT_SEQUENCE)?.max(envelope.last_sequence + 1);
            meta.insert(META_NEXT_SEQUENCE, next.to_le_bytes().as_slice())?;
        }

        txn.commit()?;
        Ok(position)
    }

    /// The whole chain, oldest first.
    pub fn segments(&self) -> Result<Vec<SegmentEnvelope>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(SEGMENTS)?;

        let mut out = Vec::new();
        for row in table.iter()? {
            let (_, value) = row?;
            out.push(SegmentEnvelope::decode(value.value())?);
        }
        Ok(out)
    }

    /// Segments from `position` onwards, for a peer catching up.
    pub fn segments_from(&self, position: u64) -> Result<Vec<SegmentEnvelope>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(SEGMENTS)?;

        let mut out = Vec::new();
        for row in table.range(position..)? {
            let (_, value) = row?;
            out.push(SegmentEnvelope::decode(value.value())?);
        }
        Ok(out)
    }

    /// The most recent segment this device wrote.
    pub fn head_segment(&self) -> Result<Option<ObjectId>> {
        let txn = self.db.begin_read()?;
        let meta = txn.open_table(META)?;
        Self::read_object_id(&meta, META_HEAD_SEGMENT)
    }

    /// The sequence number the next log entry should use.
    pub fn next_sequence(&self) -> Result<u64> {
        let txn = self.db.begin_read()?;
        let meta = txn.open_table(META)?;
        Ok(Self::read_u64(&meta, META_NEXT_SEQUENCE)?.max(1))
    }

    /// How many segments the chain holds.
    pub fn chain_length(&self) -> Result<u64> {
        let txn = self.db.begin_read()?;
        let meta = txn.open_table(META)?;
        Self::read_u64(&meta, META_CHAIN_LENGTH)
    }

    fn read_u64(meta: &impl ReadableTable<&'static str, &'static [u8]>, key: &str) -> Result<u64> {
        match meta.get(key)? {
            Some(value) => {
                let bytes: [u8; 8] = value.value().try_into().map_err(|_| {
                    StoreError::Corrupt(format!("metadata key {key} is not 8 bytes"))
                })?;
                Ok(u64::from_le_bytes(bytes))
            }
            None => Ok(0),
        }
    }

    fn read_object_id(
        meta: &impl ReadableTable<&'static str, &'static [u8]>,
        key: &str,
    ) -> Result<Option<ObjectId>> {
        match meta.get(key)? {
            Some(value) => Ok(Some(ObjectId::from_slice(value.value())?)),
            None => Ok(None),
        }
    }

    // ------------------------------------------------------- applied heads

    /// Record that `device`'s chain was applied up to `head`, with nothing
    /// deferred.
    pub fn set_applied_head(&self, device: &DeviceId, head: &ObjectId) -> Result<()> {
        let txn = self.db.begin_write()?;
        {
            txn.open_table(APPLIED)?
                .insert(device.as_bytes().as_slice(), head.as_bytes().as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }

    /// The head this device last applied cleanly for `device`, if any.
    pub fn applied_head(&self, device: &DeviceId) -> Result<Option<ObjectId>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(APPLIED)?;
        match table.get(device.as_bytes().as_slice())? {
            Some(value) => Ok(Some(ObjectId::from_slice(value.value())?)),
            None => Ok(None),
        }
    }

    // ---------------------------------------------------------- reliability

    /// What auditing has shown about `device`.
    pub fn reliability(&self, device: &DeviceId) -> Result<Reliability> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(RELIABILITY)?;
        match table.get(device.as_bytes().as_slice())? {
            Some(value) => Ok(postcard::from_bytes(value.value())?),
            None => Ok(Reliability::default()),
        }
    }

    /// Record one audit outcome for `device`.
    pub fn note_audit(&self, device: &DeviceId, passed: bool, now: u64) -> Result<Reliability> {
        let mut record = self.reliability(device)?;
        if passed {
            record.passed_one();
        } else {
            record.failed_one(now);
        }

        let txn = self.db.begin_write()?;
        {
            txn.open_table(RELIABILITY)?.insert(
                device.as_bytes().as_slice(),
                postcard::to_stdvec(&record)?.as_slice(),
            )?;
        }
        txn.commit()?;
        Ok(record)
    }

    /// Every device with something on its record, worst first.
    pub fn unreliable_devices(&self) -> Result<Vec<(DeviceId, Reliability)>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(RELIABILITY)?;

        let mut out = Vec::new();
        for row in table.iter()? {
            let (key, value) = row?;
            let device = DeviceId::from_slice(key.value())?;
            let record: Reliability = postcard::from_bytes(value.value())?;
            if record.failed > 0 {
                out.push((device, record));
            }
        }
        out.sort_by_key(|(device, record)| {
            (
                std::cmp::Reverse(record.consecutive_failures),
                std::cmp::Reverse(record.failed),
                *device,
            )
        });
        Ok(out)
    }

    // -------------------------------------------------------------- holders

    /// Record that `device` acknowledged holding `chunk`, at local time `now`.
    ///
    /// Idempotent: a repeated acknowledgement refreshes the timestamp rather
    /// than adding a second row, so a peer that is synced with hourly keeps one
    /// entry rather than a thousand.
    pub fn record_holder(&self, chunk: &ChunkId, device: &DeviceId, now: u64) -> Result<()> {
        self.record_holders(std::slice::from_ref(chunk), device, now)
    }

    /// Record many acknowledgements in one transaction.
    ///
    /// A sync round accepts chunks in batches, and one commit per chunk would
    /// make a large first upload spend most of its time in fsync — on a
    /// Raspberry Pi with an SD card, considerably more than most.
    pub fn record_holders(&self, chunks: &[ChunkId], device: &DeviceId, now: u64) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let txn = self.db.begin_write()?;
        {
            let mut holders = txn.open_table(HOLDERS)?;
            let mut holdings = txn.open_table(HOLDINGS)?;
            for chunk in chunks {
                holders.insert(holders::key(chunk, device).as_slice(), now)?;
                holdings.insert(holders::by_device(device, chunk).as_slice(), now)?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    /// Drop the record that `device` holds `chunk`.
    ///
    /// Called when a storage challenge fails. A record is evidence that a host
    /// once accepted a chunk, not proof that it still has it, and this is how
    /// the evidence gets withdrawn.
    pub fn forget_holder(&self, chunk: &ChunkId, device: &DeviceId) -> Result<()> {
        let txn = self.db.begin_write()?;
        {
            txn.open_table(HOLDERS)?
                .remove(holders::key(chunk, device).as_slice())?;
            txn.open_table(HOLDINGS)?
                .remove(holders::by_device(device, chunk).as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Drop every record naming `device`, whatever the chunk.
    ///
    /// For a peer that has left, or been revoked. Returns how many were
    /// dropped, so the caller can say what a departure actually cost.
    pub fn forget_device(&self, device: &DeviceId) -> Result<usize> {
        let txn = self.db.begin_write()?;
        let dropped;
        {
            let mut holders = txn.open_table(HOLDERS)?;
            let mut holdings = txn.open_table(HOLDINGS)?;

            // A range under this device rather than a walk of every row. Both
            // are collected before removing: redb will not let a table be
            // mutated while an iterator over it is alive.
            let mut doomed: Vec<ChunkId> = Vec::new();
            for row in holdings.range(
                holders::device_range_start(device).as_slice()
                    ..=holders::device_range_end(device).as_slice(),
            )? {
                let (key, _) = row?;
                if let Some((_, chunk)) = holders::split_by_device(key.value()) {
                    doomed.push(chunk);
                }
            }

            dropped = doomed.len();
            for chunk in doomed {
                holders.remove(holders::key(&chunk, device).as_slice())?;
                holdings.remove(holders::by_device(device, &chunk).as_slice())?;
            }
        }
        txn.commit()?;
        Ok(dropped)
    }

    /// Every **other** device known to hold `chunk`, sorted by device id.
    ///
    /// This device is never in the result. Whether the chunk is on this disk is
    /// a question for the blob store, not for a ledger of acknowledgements.
    pub fn remote_holders(&self, chunk: &ChunkId) -> Result<Vec<Holder>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(HOLDERS)?;

        let mut out = Vec::new();
        for row in table
            .range(holders::range_start(chunk).as_slice()..=holders::range_end(chunk).as_slice())?
        {
            let (key, value) = row?;
            if let Some((_, device)) = holders::split(key.value()) {
                out.push(Holder {
                    device,
                    confirmed_unix: value.value(),
                });
            }
        }
        Ok(out)
    }

    /// How many **other** devices are known to hold `chunk`.
    ///
    /// Add one for this device before comparing against a replication target,
    /// or use [`Index::under_replicated`], which does it for you.
    pub fn remote_holder_count(&self, chunk: &ChunkId) -> Result<usize> {
        Ok(self.remote_holders(chunk)?.len())
    }

    /// Live chunks held by fewer than `target` devices, worst first.
    ///
    /// **`target` counts this device.** A target of three asks for two remote
    /// holders, because the copy on this disk is the third. This is the one
    /// place the two are added up, and it is here rather than at every call
    /// site so that no caller has to remember to do it.
    ///
    /// Ordered by how close each chunk is to being lost rather than by chunk
    /// id, so a repair pass that is interrupted — a laptop closing, a Pi
    /// rebooting — has spent its time on the chunks with the least margin.
    /// Ordering by id would make the work random with respect to risk.
    pub fn under_replicated(&self, target: usize) -> Result<Vec<AtRisk>> {
        let txn = self.db.begin_read()?;
        let refs = txn.open_table(CHUNK_REFS)?;
        let holders_table = txn.open_table(HOLDERS)?;

        let mut out = Vec::new();
        for row in refs.iter()? {
            let (key, value) = row?;
            if value.value() == 0 {
                continue;
            }
            let chunk = ChunkId::from_slice(key.value())?;
            let held_by = holders_table
                .range(
                    holders::range_start(&chunk).as_slice()..=holders::range_end(&chunk).as_slice(),
                )?
                .count();
            // The copy on this disk. Referenced chunks are the ones this
            // node's own files use, so the blob is here unless a pull left the
            // entry ahead of its data — which `doctor` reports separately, and
            // which would make this an under-count rather than an over-count.
            let held_by = held_by + 1;
            if held_by < target {
                out.push(AtRisk {
                    chunk,
                    held_by,
                    target,
                });
            }
        }

        out.sort_by_key(|risk| (risk.held_by, risk.chunk));
        Ok(out)
    }

    /// The `limit` oldest acknowledgements held by `device`, oldest first.
    ///
    /// The ledger records *when* each holder last confirmed a chunk, and until
    /// now nothing read that field. An audit round works through the stalest
    /// records first, so every chunk a peer claims gets re-checked eventually
    /// and none is checked twice while another waits — without keeping a
    /// separate queue that could drift from the ledger it describes.
    pub fn stalest_holdings(&self, device: &DeviceId, limit: usize) -> Result<Vec<ChunkId>> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let txn = self.db.begin_read()?;
        let table = txn.open_table(HOLDINGS)?;

        // A range under this device, not a walk of every holder of every chunk.
        // The first version scanned the whole ledger and filtered, which at a
        // terabyte is fourteen million rows read for one audit of one peer,
        // every round, on the machine least able to afford it.
        let mut found: Vec<(u64, ChunkId)> = Vec::new();
        for row in table.range(
            holders::device_range_start(device).as_slice()
                ..=holders::device_range_end(device).as_slice(),
        )? {
            let (key, value) = row?;
            if let Some((_, chunk)) = holders::split_by_device(key.value()) {
                found.push((value.value(), chunk));
            }
        }

        found.sort_unstable();
        found.truncate(limit);
        Ok(found.into_iter().map(|(_, chunk)| chunk).collect())
    }

    /// Both orderings of the holder ledger, for cross-checking.
    ///
    /// Only a test calls this. The two tables are written in one transaction
    /// and therefore cannot disagree — but "cannot" is a claim, and a claim
    /// about denormalised state is worth being able to check rather than
    /// repeat.
    pub fn holder_orderings(&self) -> Result<HolderOrderings> {
        let txn = self.db.begin_read()?;

        let mut by_chunk = Vec::new();
        for row in txn.open_table(HOLDERS)?.iter()? {
            let (key, _) = row?;
            if let Some(pair) = holders::split(key.value()) {
                by_chunk.push(pair);
            }
        }

        let mut by_device = Vec::new();
        for row in txn.open_table(HOLDINGS)?.iter()? {
            let (key, _) = row?;
            if let Some((device, chunk)) = holders::split_by_device(key.value()) {
                by_device.push((chunk, device));
            }
        }

        by_chunk.sort_unstable();
        by_device.sort_unstable();
        Ok(HolderOrderings {
            by_chunk,
            by_device,
        })
    }

    /// How many (chunk, device) records the ledger holds.
    ///
    /// For `itsanas status`, so an operator can see the ledger growing rather
    /// than having to take its existence on trust.
    pub fn holder_records(&self) -> Result<u64> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(HOLDERS)?;
        let mut count = 0u64;
        for row in table.iter()? {
            row?;
            count += 1;
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> (tempfile::TempDir, Index) {
        let dir = tempfile::tempdir().expect("temp dir");
        let index = Index::open(dir.path().join("index.redb")).expect("open");
        (dir, index)
    }

    fn chunk(byte: u8) -> ChunkId {
        ChunkId::from_bytes([byte; 32])
    }

    fn entry(chunks: &[u8]) -> FileEntry {
        FileEntry {
            size: 10,
            modified_unix: 1_700_000_000,
            content_hash: [0; 32],
            chunks: chunks.iter().copied().map(chunk).collect(),
            version: crate::version::VersionVector::new(),
            author: itsanas_crypto::DeviceId::from_bytes([1; 32]),
        }
    }

    fn tombstone() -> Tombstone {
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

    #[test]
    fn a_file_round_trips() {
        let (_dir, index) = index();
        let value = entry(&[1, 2, 3]);
        index.put_file("notes.txt", &value).unwrap();

        assert_eq!(index.get_file("notes.txt").unwrap(), Some(value));
        assert_eq!(index.get_file("absent.txt").unwrap(), None);
        assert_eq!(index.file_count().unwrap(), 1);
    }

    #[test]
    fn adding_a_file_references_its_chunks() {
        let (_dir, index) = index();
        index.put_file("a.txt", &entry(&[1, 2])).unwrap();

        assert_eq!(index.reference_count(&chunk(1)).unwrap(), 1);
        assert_eq!(index.reference_count(&chunk(2)).unwrap(), 1);
        assert_eq!(index.reference_count(&chunk(3)).unwrap(), 0);
        assert!(index.unreferenced_chunks().unwrap().is_empty());
    }

    #[test]
    fn two_files_sharing_a_chunk_both_hold_it() {
        // The deduplication case. Deleting one file must not take the other's
        // data with it.
        let (_dir, index) = index();
        index.put_file("a.txt", &entry(&[1, 2])).unwrap();
        index.put_file("b.txt", &entry(&[2, 3])).unwrap();

        assert_eq!(index.reference_count(&chunk(2)).unwrap(), 2);

        let dropped = index.remove_file("a.txt", &tombstone()).unwrap();
        assert_eq!(dropped, vec![chunk(1)]);
        assert_eq!(
            index.reference_count(&chunk(2)).unwrap(),
            1,
            "a shared chunk was released when only one of its two files was deleted"
        );
    }

    #[test]
    fn overwriting_a_file_releases_only_the_chunks_it_stopped_using() {
        let (_dir, index) = index();
        index.put_file("a.txt", &entry(&[1, 2, 3])).unwrap();

        let dropped = index.put_file("a.txt", &entry(&[2, 3, 4])).unwrap();

        assert_eq!(
            dropped,
            vec![chunk(1)],
            "wrong chunks released on overwrite"
        );
        assert_eq!(index.reference_count(&chunk(2)).unwrap(), 1);
        assert_eq!(index.reference_count(&chunk(3)).unwrap(), 1);
        assert_eq!(index.reference_count(&chunk(4)).unwrap(), 1);
    }

    #[test]
    fn a_file_that_repeats_a_chunk_counts_each_occurrence() {
        // A file of ten identical blocks stores one chunk but references it ten
        // times. Getting this wrong frees live data on the first delete.
        let (_dir, index) = index();
        index.put_file("repeat.bin", &entry(&[7, 7, 7])).unwrap();
        assert_eq!(index.reference_count(&chunk(7)).unwrap(), 3);

        let dropped = index.remove_file("repeat.bin", &tombstone()).unwrap();
        assert_eq!(dropped, vec![chunk(7)]);
        assert_eq!(index.reference_count(&chunk(7)).unwrap(), 0);
    }

    #[test]
    fn removing_an_absent_file_still_records_the_tombstone() {
        // A delete arriving from a peer for a path this device never held is
        // the normal case, not an anomaly: the file was created and deleted
        // while this device was offline. The tombstone must still be recorded,
        // or a third device could later re-announce the file and resurrect it.
        let (_dir, index) = index();

        assert_eq!(
            index.remove_file("never-existed", &tombstone()).unwrap(),
            Vec::new(),
            "no chunks should be released for a file that was never here"
        );
        assert_eq!(
            index.get_tombstone("never-existed").unwrap(),
            Some(tombstone()),
            "the delete was forgotten, so this device would resurrect the file"
        );
    }

    #[test]
    fn writing_a_file_clears_any_tombstone_at_that_path() {
        // A path must never be both present and deleted: the two tables would
        // disagree and resolution would depend on which was consulted first.
        let (_dir, index) = index();

        index.put_file("a.txt", &entry(&[1])).unwrap();
        index.remove_file("a.txt", &tombstone()).unwrap();
        assert!(index.get_tombstone("a.txt").unwrap().is_some());

        index.put_file("a.txt", &entry(&[2])).unwrap();

        assert_eq!(index.get_tombstone("a.txt").unwrap(), None);
        assert!(index.get_file("a.txt").unwrap().is_some());
    }

    #[test]
    fn a_chunk_can_be_resurrected_before_it_is_collected() {
        // Delete a file, then restore identical content before garbage
        // collection runs. The chunk must leave the unreferenced set, or GC
        // will delete a blob that is live again.
        let (_dir, index) = index();
        index.put_file("a.txt", &entry(&[5])).unwrap();
        index.remove_file("a.txt", &tombstone()).unwrap();

        assert_eq!(index.unreferenced_chunks().unwrap().len(), 1);

        index.put_file("a-restored.txt", &entry(&[5])).unwrap();

        assert!(
            index.unreferenced_chunks().unwrap().is_empty(),
            "a chunk that came back into use is still queued for deletion"
        );
        assert_eq!(index.reference_count(&chunk(5)).unwrap(), 1);
    }

    #[test]
    fn state_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.redb");

        {
            let index = Index::open(&path).unwrap();
            index.put_file("a.txt", &entry(&[1, 2])).unwrap();
        }

        let index = Index::open(&path).unwrap();
        assert_eq!(index.get_file("a.txt").unwrap(), Some(entry(&[1, 2])));
        assert_eq!(index.reference_count(&chunk(1)).unwrap(), 1);
    }

    #[test]
    fn files_come_back_sorted_so_two_devices_agree_on_order() {
        let (_dir, index) = index();
        for path in ["zeta.txt", "alpha.txt", "middle.txt"] {
            index.put_file(path, &entry(&[1])).unwrap();
        }

        let paths: Vec<String> = index.files().unwrap().into_iter().map(|(p, _)| p).collect();
        assert_eq!(paths, vec!["alpha.txt", "middle.txt", "zeta.txt"]);
    }

    #[test]
    fn forgetting_a_chunk_clears_both_tables() {
        let (_dir, index) = index();
        index.put_file("a.txt", &entry(&[9])).unwrap();
        index.remove_file("a.txt", &tombstone()).unwrap();

        index.forget_chunk(&chunk(9)).unwrap();

        assert_eq!(index.reference_count(&chunk(9)).unwrap(), 0);
        assert!(index.unreferenced_chunks().unwrap().is_empty());
        assert!(index.referenced_chunks().unwrap().is_empty());
    }

    // -------------------------------------------------------------- holders

    fn device(byte: u8) -> DeviceId {
        DeviceId::from_bytes([byte; 32])
    }

    /// Both orderings of the ledger describe exactly the same pairs.
    fn assert_orderings_agree(index: &Index, after: &str) {
        let orderings = index.holder_orderings().unwrap();
        assert_eq!(
            orderings.by_chunk, orderings.by_device,
            "the two orderings of the holder ledger disagree after {after}. \
             Denormalised state that drifts is worse than the full scan it \
             replaced, because the answer somebody sees is the wrong one."
        );
    }

    #[test]
    fn the_two_orderings_never_disagree_whatever_is_done_to_the_ledger() {
        // The second ordering exists so that "what does this peer hold?" is a
        // range and not a walk of fourteen million rows. That is denormalised
        // state, which is exactly the thing this project refuses elsewhere —
        // and the refusal is only earned if every path that writes one writes
        // the other, in the same transaction.
        let (_dir, index) = index();

        index.record_holder(&chunk(1), &device(7), 1).unwrap();
        assert_orderings_agree(&index, "a single record");

        index
            .record_holders(&[chunk(1), chunk(2), chunk(3)], &device(8), 2)
            .unwrap();
        assert_orderings_agree(&index, "a batch");

        index.forget_holder(&chunk(2), &device(8)).unwrap();
        assert_orderings_agree(&index, "forgetting one holder");

        index.forget_device(&device(8)).unwrap();
        assert_orderings_agree(&index, "forgetting a device");

        index.put_file("a", &entry(&[1])).unwrap();
        index.forget_chunk(&chunk(1)).unwrap();
        assert_orderings_agree(&index, "collecting a chunk");

        let remaining = index.holder_orderings().unwrap().by_chunk;
        assert!(
            remaining.is_empty(),
            "everything was removed but {remaining:?} survived"
        );
    }

    #[test]
    fn the_stalest_holdings_of_one_device_ignore_every_other_device() {
        let (_dir, index) = index();
        index.record_holder(&chunk(1), &device(7), 500).unwrap();
        index.record_holder(&chunk(2), &device(7), 100).unwrap();
        index.record_holder(&chunk(3), &device(8), 1).unwrap();

        assert_eq!(
            index.stalest_holdings(&device(7), 10).unwrap(),
            vec![chunk(2), chunk(1)],
            "oldest confirmation first, and only this device's"
        );
        assert_eq!(index.stalest_holdings(&device(9), 10).unwrap(), Vec::new());
        assert_eq!(index.stalest_holdings(&device(7), 0).unwrap(), Vec::new());
    }

    #[test]
    fn a_recorded_holder_comes_back() {
        let (_dir, index) = index();
        index
            .record_holder(&chunk(1), &device(7), 1_700_000_000)
            .unwrap();

        let holders = index.remote_holders(&chunk(1)).unwrap();
        assert_eq!(holders.len(), 1);
        assert_eq!(holders[0].device, device(7));
        assert_eq!(holders[0].confirmed_unix, 1_700_000_000);
    }

    #[test]
    fn recording_the_same_holder_twice_refreshes_rather_than_duplicates() {
        // A peer that syncs hourly acknowledges the same chunks every hour. One
        // row per acknowledgement would grow the ledger without bound and make
        // the replica count wrong in the direction that hides a real shortage.
        let (_dir, index) = index();
        index.record_holder(&chunk(1), &device(7), 100).unwrap();
        index.record_holder(&chunk(1), &device(7), 900).unwrap();

        let holders = index.remote_holders(&chunk(1)).unwrap();
        assert_eq!(holders.len(), 1);
        assert_eq!(holders[0].confirmed_unix, 900);
    }

    #[test]
    fn holders_are_kept_apart_by_chunk() {
        let (_dir, index) = index();
        index.record_holder(&chunk(1), &device(7), 1).unwrap();
        index.record_holder(&chunk(2), &device(8), 1).unwrap();
        index.record_holder(&chunk(2), &device(9), 1).unwrap();

        assert_eq!(index.remote_holder_count(&chunk(1)).unwrap(), 1);
        assert_eq!(index.remote_holder_count(&chunk(2)).unwrap(), 2);
        assert_eq!(index.remote_holder_count(&chunk(3)).unwrap(), 0);
    }

    #[test]
    fn holders_come_back_sorted_so_two_devices_agree_on_order() {
        let (_dir, index) = index();
        for byte in [9u8, 2, 5, 1] {
            index.record_holder(&chunk(1), &device(byte), 1).unwrap();
        }
        let order: Vec<DeviceId> = index
            .remote_holders(&chunk(1))
            .unwrap()
            .into_iter()
            .map(|h| h.device)
            .collect();
        assert_eq!(
            order,
            vec![device(1), device(2), device(5), device(9)],
            "two nodes comparing ledgers must see the same order"
        );
    }

    #[test]
    fn forgetting_a_holder_leaves_the_others_alone() {
        // Called when a storage challenge fails. Removing more than the host
        // that failed would make one bad answer look like a mass departure and
        // start a repair storm.
        let (_dir, index) = index();
        index.record_holder(&chunk(1), &device(7), 1).unwrap();
        index.record_holder(&chunk(1), &device(8), 1).unwrap();

        index.forget_holder(&chunk(1), &device(7)).unwrap();

        let holders = index.remote_holders(&chunk(1)).unwrap();
        assert_eq!(holders.len(), 1);
        assert_eq!(holders[0].device, device(8));
    }

    #[test]
    fn forgetting_a_device_clears_it_from_every_chunk_and_nothing_else() {
        // A peer that left, or a laptop that was revoked. Its acknowledgements
        // stop being evidence for every chunk at once, and the records for
        // every other device have to survive — otherwise losing one peer looks
        // like losing all of them and the node re-uploads its entire store.
        let (_dir, index) = index();
        for c in 1..=5u8 {
            index.record_holder(&chunk(c), &device(7), 1).unwrap();
            index.record_holder(&chunk(c), &device(8), 1).unwrap();
        }

        assert_eq!(index.forget_device(&device(7)).unwrap(), 5);

        for c in 1..=5u8 {
            let holders = index.remote_holders(&chunk(c)).unwrap();
            assert_eq!(holders.len(), 1, "chunk {c}");
            assert_eq!(holders[0].device, device(8));
        }
    }

    #[test]
    fn a_target_counts_this_device_so_three_asks_for_two_elsewhere() {
        // The counting convention, pinned. Getting this off by one means the
        // repair loop targets two copies while reporting three, and nothing
        // ever says so — it shows up as data loss after two machines die
        // instead of after three.
        let (_dir, index) = index();
        index.put_file("a", &entry(&[1])).unwrap();

        assert_eq!(
            index.under_replicated(3).unwrap()[0].held_by,
            1,
            "with no remote holders, the local copy is the only one"
        );

        index.record_holder(&chunk(1), &device(7), 1).unwrap();
        assert_eq!(index.under_replicated(3).unwrap()[0].held_by, 2);

        index.record_holder(&chunk(1), &device(8), 1).unwrap();
        assert!(
            index.under_replicated(3).unwrap().is_empty(),
            "two remote holders plus this device meets a target of three"
        );
    }

    #[test]
    fn the_chunks_closest_to_being_lost_are_reported_first() {
        // A repair pass on a laptop gets interrupted by the lid closing. If the
        // order were by chunk id, the work done before the interruption would
        // be random with respect to risk, and the chunk with one copy left
        // could wait behind a thousand that had two.
        let (_dir, index) = index();
        index.put_file("a", &entry(&[1, 2, 3])).unwrap();
        index.record_holder(&chunk(2), &device(7), 1).unwrap();
        index.record_holder(&chunk(3), &device(7), 1).unwrap();
        index.record_holder(&chunk(3), &device(8), 1).unwrap();

        let risky = index.under_replicated(4).unwrap();
        let order: Vec<usize> = risky.iter().map(|r| r.held_by).collect();
        assert_eq!(order, vec![1, 2, 3], "worst first");
        assert!(risky[0].only_copy());
    }

    #[test]
    fn an_unreferenced_chunk_is_not_reported_as_under_replicated() {
        // Overwritten or deleted data is waiting for garbage collection. Asking
        // the repair loop to restore its replication would be work done to keep
        // something that is on its way out.
        let (_dir, index) = index();
        index.put_file("a", &entry(&[1])).unwrap();
        index.remove_file("a", &tombstone()).unwrap();

        assert!(index.under_replicated(3).unwrap().is_empty());
    }

    #[test]
    fn collecting_a_chunk_takes_its_holder_records_with_it() {
        // Otherwise the ledger accumulates rows for chunks that no longer
        // exist, and the count an operator reads to see whether the thing is
        // working drifts upward forever.
        let (_dir, index) = index();
        index.record_holder(&chunk(1), &device(7), 1).unwrap();
        index.record_holder(&chunk(1), &device(8), 1).unwrap();
        assert_eq!(index.holder_records().unwrap(), 2);

        index.forget_chunk(&chunk(1)).unwrap();

        assert_eq!(index.holder_records().unwrap(), 0);
        assert!(index.remote_holders(&chunk(1)).unwrap().is_empty());
    }

    #[test]
    fn the_ledger_survives_reopening() {
        // It is the only record of where this node put its data. Losing it on a
        // restart would make every node re-upload everything after a reboot.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.redb");
        {
            let index = Index::open(&path).unwrap();
            index.record_holder(&chunk(1), &device(7), 42).unwrap();
        }
        let index = Index::open(&path).unwrap();
        assert_eq!(
            index.remote_holders(&chunk(1)).unwrap()[0].confirmed_unix,
            42
        );
    }

    #[test]
    fn recording_a_batch_matches_recording_one_at_a_time() {
        let (_dir, index) = index();
        index
            .record_holders(&[chunk(1), chunk(2), chunk(3)], &device(7), 5)
            .unwrap();

        for c in 1..=3u8 {
            assert_eq!(
                index.remote_holders(&chunk(c)).unwrap()[0].device,
                device(7)
            );
        }
        assert_eq!(index.holder_records().unwrap(), 3);
    }

    #[test]
    fn recording_an_empty_batch_does_nothing_rather_than_opening_a_transaction() {
        let (_dir, index) = index();
        index.record_holders(&[], &device(7), 5).unwrap();
        assert_eq!(index.holder_records().unwrap(), 0);
    }
}
