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

use itsanas_crypto::{ChunkId, ObjectId};
use redb::{Database, ReadableTable, TableDefinition};

use crate::{
    error::{Result, StoreError},
    oplog::{FileEntry, LogEntry, SegmentEnvelope, Tombstone},
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
/// Small named scalars: next sequence number, head segment, chain length.
const META: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("meta");

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
            let _ = txn.open_table(META)?;
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
}
