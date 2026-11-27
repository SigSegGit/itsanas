//! Files, tombstones and local state: the path tables, and the reference
//! counts a file write moves in the same transaction.

use std::{
    collections::BTreeMap,
    sync::{MutexGuard, atomic::Ordering},
};

use itsanas_crypto::ChunkId;
use redb::ReadableTable;

use super::{CHUNK_REFS, FILES, Index, LOCAL, TOMBSTONES, UNREFERENCED, placement};
use crate::{
    error::{Result, StoreError},
    local::LocalState,
    oplog::{FileEntry, Tombstone},
};

impl Index {
    // ---------------------------------------------------------------- files

    /// Insert or replace the entry for `path`, adjusting reference counts.
    ///
    /// Returns the chunks that dropped to zero references as a result, which is
    /// what an overwrite of a large file produces and what garbage collection
    /// will later reclaim.
    pub fn put_file(&self, path: &str, entry: &FileEntry) -> Result<Vec<ChunkId>> {
        let encoded = postcard::to_stdvec(entry)?;
        let mut local_bytes = self.lock_local_bytes()?;
        let txn = self.db.begin_write()?;
        let newly_unreferenced;
        let replaced_size;

        {
            let mut files = txn.open_table(FILES)?;
            let mut refs = txn.open_table(CHUNK_REFS)?;
            let mut unreferenced = txn.open_table(UNREFERENCED)?;

            let previous: Option<FileEntry> = match files.get(path)? {
                Some(value) => Some(postcard::from_bytes(value.value())?),
                None => None,
            };

            replaced_size = previous.as_ref().map_or(0, |previous| previous.size);

            let mut delta: BTreeMap<[u8; 32], i64> = BTreeMap::new();
            if let Some(previous) = &previous {
                for chunk in &previous.chunks {
                    *delta.entry(chunk.to_bytes()).or_default() -= 1;
                }
            }
            for chunk in &entry.chunks {
                *delta.entry(chunk.to_bytes()).or_default() += 1;
            }

            newly_unreferenced = Self::apply_reference_delta(
                &mut refs,
                &mut unreferenced,
                &mut placement::ShortQueue::open(&txn)?,
                &delta,
            )?;

            files.insert(path, encoded.as_slice())?;

            // A live file supersedes any tombstone at the same path, and the
            // two must never coexist — a path that is both present and deleted
            // would resolve differently depending on which table was consulted.
            txn.open_table(TOMBSTONES)?.remove(path)?;
        }

        txn.commit()?;
        // The file's chunks are in the summary now.
        self.refs_changed.fetch_add(1, Ordering::Release);
        if let Some(total) = local_bytes.as_mut() {
            *total = total
                .saturating_sub(replaced_size)
                .saturating_add(entry.size);
        }
        Ok(newly_unreferenced)
    }

    /// Forget the local content of `path` without deleting the file.
    ///
    /// The difference from [`Index::remove_file`] is the whole point: no
    /// tombstone is written and no operation is logged, so as far as the account
    /// is concerned the file still exists. This device simply stops being one of
    /// the places it can be read from, which is what a phone told to hold two
    /// gigabytes of a forty-gigabyte account has to be able to do.
    ///
    /// Returns the chunks that lost their last reference.
    ///
    /// A path with no entry returns an empty list rather than an error: the
    /// caller is asking for a state, not performing a transaction, and "it was
    /// already not here" is that state.
    pub fn release_file(&self, path: &str) -> Result<Vec<ChunkId>> {
        let mut local_bytes = self.lock_local_bytes()?;
        let txn = self.db.begin_write()?;
        let newly_unreferenced;
        let dropped_size;

        {
            let mut files = txn.open_table(FILES)?;
            let mut refs = txn.open_table(CHUNK_REFS)?;
            let mut unreferenced = txn.open_table(UNREFERENCED)?;

            let previous: Option<FileEntry> = match files.get(path)? {
                Some(value) => Some(postcard::from_bytes(value.value())?),
                None => None,
            };
            dropped_size = previous.as_ref().map_or(0, |previous| previous.size);

            newly_unreferenced = match previous {
                None => Vec::new(),
                Some(previous) => {
                    let mut delta: BTreeMap<[u8; 32], i64> = BTreeMap::new();
                    for chunk in &previous.chunks {
                        *delta.entry(chunk.to_bytes()).or_default() -= 1;
                    }

                    let dropped = Self::apply_reference_delta(
                        &mut refs,
                        &mut unreferenced,
                        &mut placement::ShortQueue::open(&txn)?,
                        &delta,
                    )?;
                    files.remove(path)?;
                    dropped
                }
            };
        }

        txn.commit()?;
        // A count only falls here and a row at zero stays, so the summary
        // moves only if the index disagreed with itself; counted all the same.
        self.refs_changed.fetch_add(1, Ordering::Release);
        if let Some(total) = local_bytes.as_mut() {
            *total = total.saturating_sub(dropped_size);
        }
        Ok(newly_unreferenced)
    }

    /// Remove `path`, leaving `tombstone` in its place.
    ///
    /// Returns the chunks that lost their last reference.
    pub fn remove_file(&self, path: &str, tombstone: &Tombstone) -> Result<Vec<ChunkId>> {
        let encoded_tombstone = postcard::to_stdvec(tombstone)?;
        let mut local_bytes = self.lock_local_bytes()?;
        let txn = self.db.begin_write()?;
        let newly_unreferenced;
        let dropped_size;

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
            dropped_size = previous.as_ref().map_or(0, |previous| previous.size);

            newly_unreferenced = match previous {
                None => Vec::new(),
                Some(previous) => {
                    let mut delta: BTreeMap<[u8; 32], i64> = BTreeMap::new();
                    for chunk in &previous.chunks {
                        *delta.entry(chunk.to_bytes()).or_default() -= 1;
                    }

                    let dropped = Self::apply_reference_delta(
                        &mut refs,
                        &mut unreferenced,
                        &mut placement::ShortQueue::open(&txn)?,
                        &delta,
                    )?;
                    files.remove(path)?;
                    dropped
                }
            };
        }

        txn.commit()?;
        // A count only falls here and a row at zero stays, so the summary
        // moves only if the index disagreed with itself; counted all the same.
        self.refs_changed.fetch_add(1, Ordering::Release);
        if let Some(total) = local_bytes.as_mut() {
            *total = total.saturating_sub(dropped_size);
        }
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

    /// Forget every tombstone dated from `floor` up to, not including,
    /// `cutoff`; how many went. One write transaction.
    ///
    /// Before `floor` a date is a clock that was never set, and that trace is
    /// kept: a delete dated 1970 by a machine with no battery would otherwise
    /// go the moment it arrived. A tombstone that does not decode is kept too.
    pub fn forget_tombstones_between(&self, floor: u64, cutoff: u64) -> Result<usize> {
        let txn = self.db.begin_write()?;
        let gone = {
            let mut table = txn.open_table(TOMBSTONES)?;
            let mut old = Vec::new();
            for row in table.iter()? {
                let (path, value) = row?;
                if let Ok(tombstone) = postcard::from_bytes::<Tombstone>(value.value())
                    && (floor..cutoff).contains(&tombstone.removed_unix)
                {
                    old.push(path.value().to_owned());
                }
            }
            for path in &old {
                table.remove(path.as_str())?;
            }
            old.len()
        };
        txn.commit()?;
        Ok(gone)
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

    /// Total size of the files this index holds: the part of the account whose
    /// content is on this device.
    ///
    /// Logical sizes, as the writing device recorded them -- not what the blobs
    /// take after deduplication and sealing. An allowance is stated in the
    /// sizes a person sees in a listing, and a total that shrank because two
    /// of their files happen to share chunks would be one nobody can check.
    pub fn local_bytes(&self) -> Result<u64> {
        let mut cached = self.lock_local_bytes()?;
        if let Some(total) = *cached {
            return Ok(total);
        }

        let txn = self.db.begin_read()?;
        let files = txn.open_table(FILES)?;
        let mut total: u64 = 0;
        for row in files.iter()? {
            let (_, value) = row?;
            let entry: FileEntry = postcard::from_bytes(value.value())?;
            total = total.saturating_add(entry.size);
        }

        *cached = Some(total);
        Ok(total)
    }

    fn lock_local_bytes(&self) -> Result<MutexGuard<'_, Option<u64>>> {
        self.local_bytes.lock().map_err(|_| {
            StoreError::Corrupt("the index's byte count was poisoned by a panic".to_owned())
        })
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
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::super::*;
    use super::*;

    /// Traces older than the account keeps go; a recent one stays, and so does
    /// one dated by a clock never set, which looks fifty years old and is not.
    /// Sabotage: drop the floor, or let the cutoff day itself go.
    #[test]
    fn red_team_only_traces_older_than_kept_go_and_a_clock_never_set_keeps_its_own() {
        let (_dir, index) = fresh();
        let day = crate::trash::DAY;
        let now = 1_800_000_000;
        let dated = |at: u64| Tombstone {
            removed_unix: at,
            ..tombstone()
        };
        index
            .remove_file("old.txt", &dated(now - 400 * day))
            .unwrap();
        index
            .remove_file("recent.txt", &dated(now - 10 * day))
            .unwrap();
        index
            .remove_file("on-the-day.txt", &dated(now - 365 * day))
            .unwrap();
        index.remove_file("pi-in-1970.txt", &dated(5)).unwrap();

        let floor = crate::traces::CLOCK_FLOOR;
        let gone = index
            .forget_tombstones_between(floor, now - 365 * day)
            .unwrap();

        assert_eq!(gone, 1, "not exactly the one old trace went");
        assert!(
            index.get_tombstone("old.txt").unwrap().is_none(),
            "a trace older than kept stayed"
        );
        for kept in ["recent.txt", "on-the-day.txt"] {
            assert!(
                index.get_tombstone(kept).unwrap().is_some(),
                "{kept}: a trace still within its days went"
            );
        }
        assert!(
            index.get_tombstone("pi-in-1970.txt").unwrap().is_some(),
            "a delete dated by a clock never set went at once: a machine still holding the file brings it back"
        );
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
        assert_eq!(
            index.unreferenced_chunks().unwrap(),
            [] as [(itsanas_crypto::ChunkId, u64); 0]
        );
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
    fn state_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.redb");

        {
            let index = Index::open(&path, test_audit_key()).unwrap();
            index.put_file("a.txt", &entry(&[1, 2])).unwrap();
        }

        let index = Index::open(&path, test_audit_key()).unwrap();
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
}
