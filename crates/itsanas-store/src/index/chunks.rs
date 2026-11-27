//! Chunks: reference counts, the live set and its pages, the summary a
//! reconciliation compares, and forgetting a chunk once collected.

use std::{collections::BTreeMap, sync::atomic::Ordering};

use itsanas_crypto::ChunkId;
use redb::{ReadableTable, ReadableTableMetadata};

use super::{
    CHUNK_REFS, HOLDER_COUNTS, HOLDERS, HOLDINGS, Index, LOSSES, UNREFERENCED, bump, now_unix,
    placement,
};
use crate::{
    error::{Result, StoreError},
    holders::{self},
};

impl Index {
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

    /// A summary of the chunks this device holds, for reconciling with a peer.
    ///
    /// One sequential scan of the reference table, which is keyed by chunk id
    /// and therefore already in the order the summary requires. See
    /// [`crate::summary`] for what it is for and why the ordering is a
    /// contract rather than a detail.
    /// How many chunks are live: what one pass of the daemon's disk check has
    /// to cover.
    pub fn live_chunk_count(&self) -> Result<u64> {
        let txn = self.db.begin_read()?;
        Ok(txn.open_table(CHUNK_REFS)?.len()?)
    }

    /// The summary of what this node can actually serve of its own account:
    /// the live chunks less the ones known to be missing from disk.
    ///
    /// What a peer of the same account is answered with. Since §8 4c an
    /// agreeing summary re-stamps holder records without a `HaveChunks`, so a
    /// summary that kept counting a chunk the disk lost would keep this node
    /// counted as its copy. [`Self::chunk_summary`] stays the set this node
    /// *wants* held, which is what it compares against others.
    ///
    /// A merge of two sorted tables, so the cost is the live set's plus the
    /// losses', with no lookup per chunk.
    pub fn held_summary(&self) -> Result<Vec<crate::summary::Digest>> {
        let txn = self.db.begin_read()?;
        let refs = txn.open_table(CHUNK_REFS)?;
        let losses = txn.open_table(LOSSES)?;

        let mut failed: Option<StoreError> = None;
        let mut lost = losses.iter()?.peekable();
        let digests = crate::summary::buckets(refs.iter()?.filter_map(|row| {
            let outcome = row
                .map_err(StoreError::from)
                .and_then(|(key, _)| ChunkId::from_slice(key.value()).map_err(StoreError::from));
            let chunk = match outcome {
                Ok(chunk) => chunk,
                Err(error) => {
                    failed.get_or_insert(error);
                    return None;
                }
            };
            // Advance the losses past everything below this chunk.
            while let Some(next) = lost.peek() {
                match next {
                    Ok((key, _)) if key.value() < chunk.as_bytes().as_slice() => {
                        lost.next();
                    }
                    Ok(_) => break,
                    Err(_) => {
                        if let Some(Err(error)) = lost.next() {
                            failed.get_or_insert(StoreError::from(error));
                        }
                    }
                }
            }
            let is_lost = matches!(lost.peek(), Some(Ok((key, _))) if key.value() == chunk.as_bytes().as_slice());
            (!is_lost).then_some(chunk)
        }));

        match failed {
            Some(error) => Err(error),
            None => Ok(digests),
        }
    }

    pub fn chunk_summary(&self) -> Result<Vec<crate::summary::Digest>> {
        // Read before the transaction: a write committed after this is either
        // in what is read (and the next call reads again for nothing) or
        // counted after it, never cached under a count it is not in.
        let changed = self.refs_changed.load(Ordering::Acquire);
        if let Ok(cached) = self.summary.lock()
            && let Some((at, digests)) = cached.as_ref()
            && *at == changed
        {
            return Ok(digests.clone());
        }
        let digests = self.summary_read()?;
        if let Ok(mut cached) = self.summary.lock() {
            *cached = Some((changed, digests.clone()));
        }
        Ok(digests)
    }

    /// [`Self::chunk_summary`] read from the table, every row.
    fn summary_read(&self) -> Result<Vec<crate::summary::Digest>> {
        self.summary_reads.fetch_add(1, Ordering::Relaxed);
        let txn = self.db.begin_read()?;
        let refs = txn.open_table(CHUNK_REFS)?;

        // Streamed, not collected. Materialising every id first is a `Vec` of
        // sixteen million entries at a terabyte -- 512 MB in one allocation, on
        // machines whose measured peak is 17 MiB. That is the exact fault this
        // work removed from the sweep, and it came back here in the commit that
        // removed it; `summary::buckets` was already written to take an
        // iterator.
        let mut failed: Option<StoreError> = None;
        let digests = crate::summary::buckets(refs.iter()?.filter_map(|row| {
            let outcome = row
                .map_err(StoreError::from)
                .and_then(|(key, _)| ChunkId::from_slice(key.value()).map_err(StoreError::from));
            match outcome {
                Ok(chunk) => Some(chunk),
                Err(error) => {
                    failed.get_or_insert(error);
                    None
                }
            }
        }));

        // A key the index cannot decode means the table is damaged. Reporting a
        // summary computed from what could be read would tell a peer the two
        // sides differ in a way no listing could then explain.
        match failed {
            Some(error) => Err(error),
            None => Ok(digests),
        }
    }

    /// One page of the chunks this device still has on disk, in chunk order.
    ///
    /// Everything the index knows a blob for: referenced by a live file, **and**
    /// unreferenced but not yet collected. The second half is not an oversight.
    /// A device that edits a file and then uploads still holds the superseded
    /// chunks until garbage collection takes them, and a replacement device
    /// replaying the log needs them to apply the operation they belonged to.
    /// Leaving them out made a restore defer two operations out of thirteen and
    /// say nothing -- caught by
    /// `a_replacement_device_pulls_a_whole_corpus_back_from_a_stranger`, which
    /// exists because that failure is silent.
    ///
    /// **That dependency is itself fragile and is written down in
    /// `docs/ROADMAP.md`:** once collection has taken those chunks, nobody can
    /// serve them, and a device restoring later defers those operations for
    /// ever. The replay wants the *last* operation per path, not every one of
    /// them; it does not know that yet.
    ///
    /// Straight, not a ring: `live_chunks_from` wraps, which is right for a
    /// sampler that should never run out of things to look at and wrong for a
    /// sweep that has to know when it has finished. `after` is exclusive;
    /// `None` starts at the beginning. The second element is where to continue,
    /// or `None` when there is nothing after this page.
    ///
    /// # Why this exists rather than `BlobStore::addresses`
    ///
    /// That function walks the fan-out directories and its own documentation
    /// says it is "only used by garbage collection and integrity checking,
    /// never on a hot path". The push loop called it every round, per peer. At a
    /// terabyte that is a recursive `readdir` over sixteen million files every
    /// five minutes, and a `Vec` of sixteen million chunk ids — **537 MB
    /// resident, allocated in one go, on a machine whose measured peak is
    /// 17 MiB**. The account size at which the sweep becomes expensive on the
    /// wire is far past the size at which it kills the process.
    ///
    /// The index already knows which chunks are live and keeps them in order,
    /// so the answer was one table scan away.
    pub fn live_chunks_page(
        &self,
        after: Option<&ChunkId>,
        limit: usize,
    ) -> Result<(Vec<ChunkId>, Option<ChunkId>)> {
        if limit == 0 {
            return Ok((Vec::new(), None));
        }

        let txn = self.db.begin_read()?;
        let refs = txn.open_table(CHUNK_REFS)?;

        let start: Vec<u8> = after.map_or_else(Vec::new, |chunk| chunk.as_bytes().to_vec());
        let mut out = Vec::with_capacity(limit);

        for row in refs.range(start.as_slice()..)? {
            let (key, _) = row?;
            let chunk = ChunkId::from_slice(key.value())?;
            if after.is_some_and(|previous| previous == &chunk) {
                continue; // the page boundary itself, already returned
            }
            out.push(chunk);
            if out.len() == limit {
                return Ok((out, Some(chunk)));
            }
        }

        Ok((out, None))
    }

    pub fn live_chunks_from(&self, cursor: &ChunkId, limit: usize) -> Result<Vec<ChunkId>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let txn = self.db.begin_read()?;
        let refs = txn.open_table(CHUNK_REFS)?;

        let mut out = Vec::with_capacity(limit);
        let lowest: &[u8] = &[];
        for start in [cursor.as_bytes().as_slice(), lowest] {
            for row in refs.range(start..)? {
                let (key, value) = row?;
                if value.value() > 0 {
                    let chunk = ChunkId::from_slice(key.value())?;
                    // The wrap can revisit what the first pass already saw.
                    if !out.contains(&chunk) {
                        out.push(chunk);
                    }
                    if out.len() == limit {
                        return Ok(out);
                    }
                }
            }
        }
        Ok(out)
    }

    /// One live chunk of this node's own, chosen by where `cursor` lands.
    ///
    /// For picking the probe handed to a paused peer. The peer must not choose
    /// that chunk, and in the first version it did: the probe was taken from
    /// the peer's own answer to "what are you missing?", so a host could name
    /// one small chunk, keep it, and buy its way back with that. The owner
    /// draws instead, from its own live set, and a host has no say at all.
    ///
    /// The draw is weighted by the gap before each chunk, and `CHUNK_REFS` is
    /// keyed by the chunk id, which the peer knows — so it *can* work out
    /// which chunk a given cursor would pick. That is not the same weakness as
    /// in the audit order and it does not matter here: predicting the probe
    /// buys nothing, because the peer has no way to obtain that chunk except
    /// from this node, and refusing it keeps the peer paused.
    ///
    /// `None` when this node has no live chunks — nothing to prove anything
    /// with, and nothing worth saying about it.
    pub fn live_chunk_near(&self, cursor: &ChunkId) -> Result<Option<ChunkId>> {
        Ok(self.live_chunks_from(cursor, 1)?.into_iter().next())
    }

    /// Forget a chunk entirely, after its blob has been deleted.
    /// Forget that this device holds `chunk`, while keeping what it knows about
    /// who else does.
    ///
    /// The difference from [`Index::forget_chunk`] is the whole point, and it
    /// was found by running the same file through twice. `forget_chunk` erases
    /// the holder ledger deliberately: a chunk deleted because its file was
    /// deleted must not keep the repair loop working to restore data nobody
    /// wants. A chunk *released* is the opposite case -- the file is still in
    /// the account, and the copies elsewhere are exactly what made letting go
    /// of it safe.
    ///
    /// Erasing them cost two things at once. The device stopped being able to
    /// say how well replicated its own account was, and it could never release
    /// that file again: `itsanas get` brings the content back, the ledger now
    /// knows one holder instead of two, and the next round refuses to let go
    /// and leaves the device permanently over its limit. Observed on a
    /// Raspberry Pi, stuck at 380 KiB against a 300 KiB limit, round after
    /// round, saying so each time.
    pub fn release_chunk(&self, chunk: &ChunkId) -> Result<()> {
        // Not a loss: this device let it go on purpose.
        self.clear_loss(chunk)?;
        let txn = self.db.begin_write()?;
        {
            let mut refs = txn.open_table(CHUNK_REFS)?;
            let mut unreferenced = txn.open_table(UNREFERENCED)?;
            refs.remove(chunk.as_bytes().as_slice())?;
            unreferenced.remove(chunk.as_bytes().as_slice())?;
        }
        // Out of the live set, so out of placement's queue too.
        Self::purge_chunk_in(&txn, chunk)?;
        txn.commit()?;
        // Let go: out of the summary.
        self.refs_changed.fetch_add(1, Ordering::Release);
        Ok(())
    }

    pub fn forget_chunk(&self, chunk: &ChunkId) -> Result<()> {
        // A chunk nothing references any more cannot be a loss. Leaving the
        // entry would make repair ask peers, for ever, about data this node
        // deliberately threw away.
        self.clear_loss(chunk)?;
        let txn = self.db.begin_write()?;
        {
            // The holder ledger goes with it. Leaving the rows behind would
            // leave the repair loop working to restore the replication of a
            // chunk that no longer exists anywhere and should not.
            let mut holders = txn.open_table(HOLDERS)?;
            let mut holdings = txn.open_table(HOLDINGS)?;
            let mut counts = txn.open_table(HOLDER_COUNTS)?;
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
                    let tag = holders::audit_tag(&self.audit_key, &chunk);
                    holdings.remove(holders::by_device(&device, &tag, &chunk).as_slice())?;
                    if holders.remove(key.as_slice())?.is_some() {
                        bump(&mut counts, &device, -1)?;
                    }
                    continue;
                }
                holders.remove(key.as_slice())?;
            }

            let mut refs = txn.open_table(CHUNK_REFS)?;
            let mut unreferenced = txn.open_table(UNREFERENCED)?;
            refs.remove(chunk.as_bytes().as_slice())?;
            unreferenced.remove(chunk.as_bytes().as_slice())?;
        }
        // A collected chunk left in the queue would be "short" for ever, and
        // the safety net would keep trying to place data nobody wants.
        Self::purge_chunk_in(&txn, chunk)?;
        txn.commit()?;
        // Collected: out of the summary.
        self.refs_changed.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Apply a signed reference delta, maintaining the unreferenced set.
    ///
    /// Returns the chunks that fell to zero. A count that would go negative is
    /// clamped: that means the index disagrees with itself, and refusing to
    /// wrap around is what keeps the error contained to one chunk instead of
    /// turning into a `u64` underflow that makes a dead chunk look immortal.
    pub(super) fn apply_reference_delta(
        refs: &mut redb::Table<'_, &'static [u8], u64>,
        unreferenced: &mut redb::Table<'_, &'static [u8], u64>,
        short: &mut placement::ShortQueue<'_>,
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

            // A chunk's first reference is the moment it has no copy anywhere
            // but here. Queued in this transaction, one insert per new chunk,
            // so a crash cannot leave a new chunk that nothing will place
            // until the census happens to reach it.
            if current == 0 && updated > 0 {
                short.insert_new(key, timestamp)?;
            }

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
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::super::*;
    use super::*;

    /// A quiet round reads the summary from memory, not from every row of the
    /// table; and after each of the five writers of `CHUNK_REFS` it is the
    /// summary read afresh, never the one from before the write. Sabotage:
    /// drop any one writer's bump (it then answers what the account held
    /// before); drop the cache (the quiet round reads every row again).
    #[test]
    fn red_team_the_summary_is_read_once_while_quiet_and_again_after_every_write() {
        let (_dir, index) = index();
        let fresh = |index: &Index| index.summary_read().expect("read");
        let check = |index: &Index, after: &str| {
            assert_eq!(
                index.chunk_summary().expect("summary"),
                fresh(index),
                "after {after} the summary is the one from before it: a peer is told the account holds what it no longer does"
            );
        };

        index.put_file("a", &entry(&[1, 2])).expect("put");
        index.put_file("b", &entry(&[3])).expect("put");
        let first = index.chunk_summary().expect("summary");
        let reads = index.summary_reads.load(Ordering::Relaxed);
        for _ in 0..3 {
            assert_eq!(index.chunk_summary().expect("summary"), first);
        }
        assert_eq!(
            index.summary_reads.load(Ordering::Relaxed),
            reads,
            "a quiet round read every chunk id again to say nothing changed"
        );

        index.put_file("c", &entry(&[4])).expect("put");
        check(&index, "put_file");
        index.release_file("c").expect("release");
        check(&index, "release_file");
        index.remove_file("b", &tombstone()).expect("remove");
        check(&index, "remove_file");
        index.release_chunk(&chunk(3)).expect("release chunk");
        check(&index, "release_chunk");
        index.put_file("d", &entry(&[5])).expect("put");
        index.chunk_summary().expect("cached");
        index.remove_file("d", &tombstone()).expect("remove");
        index.chunk_summary().expect("cached");
        index.forget_chunk(&chunk(5)).expect("forget");
        check(&index, "forget_chunk");
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
    fn forgetting_a_chunk_clears_both_tables() {
        let (_dir, index) = index();
        index.put_file("a.txt", &entry(&[9])).unwrap();
        index.remove_file("a.txt", &tombstone()).unwrap();

        index.forget_chunk(&chunk(9)).unwrap();

        assert_eq!(index.reference_count(&chunk(9)).unwrap(), 0);
        assert_eq!(
            index.unreferenced_chunks().unwrap(),
            [] as [(itsanas_crypto::ChunkId, u64); 0]
        );
        assert_eq!(
            index.referenced_chunks().unwrap(),
            [] as [itsanas_crypto::ChunkId; 0]
        );
    }
}
