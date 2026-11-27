//! The holder ledger: which device holds which chunk, in both orderings,
//! written and forgotten together.

use itsanas_crypto::{ChunkId, DeviceId};
use redb::ReadableTable;

use super::{
    CHUNK_REFS, DEVICE_NETWORK, DEVICE_SEEN, DRAINING, HOLDER_COUNTS, HOLDERS, HOLDINGS,
    HolderOrderings, Index, bump, now_unix, placement,
};
use crate::{
    error::Result,
    holders::{self, Holder},
};

impl Index {
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

        // Read first, and write only what has actually aged.
        //
        // A round confirms every chunk a peer holds, so writing the timestamp
        // back for all of them meant rewriting the ledger every five minutes to
        // record that nothing had changed. Measured: 962 KB per round with
        // peers against 61 KB without, on an account of nine hundred kilobytes.
        // See `holders::REFRESH_AFTER` for why a quarter of the freshness
        // window is the right interval and what it costs to be wrong about it.
        //
        // The read is a separate transaction on purpose: a stale answer here
        // makes a round write a timestamp it did not have to, which is the
        // harmless direction.
        let due = self.without_fresh_record(chunks, device, now)?;
        // A copy this node released is not taken back on the host's word.
        // The host still answers "have" until it evicts, and re-stamping the
        // record then would count, for `LIVE_FOR`, a copy the host is about
        // to delete (red team, HIGH 1). Only a fresh `StoreChunk` it accepted
        // ([`Self::record_stored`]) brings the record back.
        let due = self.not_released(&due, device, now)?;
        self.record_due(&due, device, now)
    }

    /// [`Self::record_holders`] for chunks `device` has just accepted in a
    /// `StoreChunk`: a new copy, so any release of the old one is over.
    ///
    /// # Errors
    ///
    /// If the index cannot be written.
    pub fn record_stored(&self, chunks: &[ChunkId], device: &DeviceId, now: u64) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        self.clear_released(chunks, device)?;
        let due = self.without_fresh_record(chunks, device, now)?;
        self.record_due(&due, device, now)
    }

    fn record_due(&self, due: &[ChunkId], device: &DeviceId, now: u64) -> Result<()> {
        // Contact is always recorded -- it is what every liveness question
        // in this crate reads -- but to [`SEEN_STEP`], not to the second: one
        // row rewritten every round was one commit every round, and the
        // readers ask in days.
        if due.is_empty() && self.seen_within_step(device, now)? {
            return Ok(());
        }
        let txn = self.db.begin_write()?;
        {
            txn.open_table(DEVICE_SEEN)?
                .insert(device.as_bytes().as_slice(), now)?;
            let mut holders = txn.open_table(HOLDERS)?;
            let mut holdings = txn.open_table(HOLDINGS)?;
            let mut counts = txn.open_table(HOLDER_COUNTS)?;
            for chunk in due {
                let tag = holders::audit_tag(&self.audit_key, chunk);
                if holders
                    .insert(holders::key(chunk, device).as_slice(), now)?
                    .is_none()
                {
                    bump(&mut counts, device, 1)?;
                }
                holdings.insert(holders::by_device(device, &tag, chunk).as_slice(), now)?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    /// Of `chunks`, the ones with no record for `device` younger than
    /// [`holders::REFRESH_AFTER`], in the order given.
    ///
    /// One rule, two readers. `record_holders` writes back only these, so a
    /// round that changes nothing writes nothing. And a push narrowed to the
    /// buckets a summary named asks the peer only about these: a chunk the
    /// peer confirmed within that window is not asked about again until the
    /// next full walk of the ledger, which is due exactly when such a record
    /// stops being fresh. Two definitions of "fresh" would let a record age
    /// out in the gap between them, unasked and unstamped.
    pub fn without_fresh_record(
        &self,
        chunks: &[ChunkId],
        device: &DeviceId,
        now: u64,
    ) -> Result<Vec<ChunkId>> {
        let stale_before = now.saturating_sub(holders::REFRESH_AFTER);
        let txn = self.db.begin_read()?;
        let holders_table = txn.open_table(HOLDERS)?;
        let mut out = Vec::new();
        for chunk in chunks {
            let fresh = holders_table
                .get(holders::key(chunk, device).as_slice())?
                .is_some_and(|value| value.value() >= stale_before);
            if !fresh {
                out.push(*chunk);
            }
        }
        Ok(out)
    }

    /// Of `chunks`, the ones `device` has any holder record for, fresh or not,
    /// in the order given.
    ///
    /// What a push asks a full peer about: the chunks it is recorded as
    /// holding, to keep those records honest, and nothing it would only refuse.
    pub fn with_record(&self, chunks: &[ChunkId], device: &DeviceId) -> Result<Vec<ChunkId>> {
        let txn = self.db.begin_read()?;
        let holders_table = txn.open_table(HOLDERS)?;
        let mut out = Vec::new();
        for chunk in chunks {
            if holders_table
                .get(holders::key(chunk, device).as_slice())?
                .is_some()
            {
                out.push(*chunk);
            }
        }
        Ok(out)
    }

    /// Drop many records for one device, in one transaction.
    ///
    /// A round learns about thousands of chunks at once -- every address the
    /// peer answers "missing" to is one it does not have, whatever the ledger
    /// says -- and a commit per chunk would make the correction cost more than
    /// the sync it rode in on.
    pub fn forget_holders(&self, chunks: &[ChunkId], device: &DeviceId) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let txn = self.db.begin_write()?;
        // A peer answering "missing" about chunks it never held does so in
        // the first rounds after a start: such a forget removes nothing, and
        // is not committed.
        let mut changed = false;
        {
            let mut holders = txn.open_table(HOLDERS)?;
            let mut holdings = txn.open_table(HOLDINGS)?;
            let mut counts = txn.open_table(HOLDER_COUNTS)?;
            let mut short = placement::ShortQueue::open(&txn)?;
            let refs = txn.open_table(CHUNK_REFS)?;
            for chunk in chunks {
                let tag = holders::audit_tag(&self.audit_key, chunk);
                if holders
                    .remove(holders::key(chunk, device).as_slice())?
                    .is_some()
                {
                    changed = true;
                    bump(&mut counts, device, -1)?;
                    // One copy fewer: may now be short. Queued only when a
                    // record was really removed, so a peer answering
                    // "missing" about chunks it never had queues nothing.
                    placement::enqueue_in(&mut short, &refs, chunk, now_unix())?;
                }
                changed |= holdings
                    .remove(holders::by_device(device, &tag, chunk).as_slice())?
                    .is_some();
            }
        }
        if changed {
            crate::commit(txn)?;
        } else {
            txn.abort()?;
        }
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
            let tag = holders::audit_tag(&self.audit_key, chunk);
            if txn
                .open_table(HOLDERS)?
                .remove(holders::key(chunk, device).as_slice())?
                .is_some()
            {
                bump(&mut txn.open_table(HOLDER_COUNTS)?, device, -1)?;
                placement::enqueue_in(
                    &mut placement::ShortQueue::open(&txn)?,
                    &txn.open_table(CHUNK_REFS)?,
                    chunk,
                    now_unix(),
                )?;
            }
            txn.open_table(HOLDINGS)?
                .remove(holders::by_device(device, &tag, chunk).as_slice())?;
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
            txn.open_table(HOLDER_COUNTS)?
                .remove(device.as_bytes().as_slice())?;
            txn.open_table(DRAINING)?
                .remove(device.as_bytes().as_slice())?;
            // Where a device that left was, is nobody's business any more.
            txn.open_table(DEVICE_NETWORK)?
                .remove(device.as_bytes().as_slice())?;
            // Every chunk it held is one copy down. This walk was already
            // here, over exactly these rows, so queueing them costs a lookup
            // each and no second pass.
            let mut short = placement::ShortQueue::open(&txn)?;
            let refs = txn.open_table(CHUNK_REFS)?;
            let now = now_unix();
            for chunk in doomed {
                let tag = holders::audit_tag(&self.audit_key, &chunk);
                holders.remove(holders::key(&chunk, device).as_slice())?;
                holdings.remove(holders::by_device(device, &tag, &chunk).as_slice())?;
                placement::enqueue_in(&mut short, &refs, &chunk, now)?;
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

    /// A page of the chunks `device` is recorded as holding.
    ///
    /// `after` is the raw key a previous page ended on, or `None` to start.
    /// Returns the chunks and the cursor to continue from, or `None` when the
    /// device's records are exhausted.
    ///
    /// # Why a cursor and not a limit
    ///
    /// Because a fixed prefix is a fixed list, and this project has already
    /// made that mistake once: the first audit worked through the sixteen
    /// lowest chunk ids every round for ever, so a host could keep sixteen
    /// chunks out of millions and never be caught. A sweep that only ever sees
    /// the head of the ledger leaves the tail unchecked in exactly the same
    /// way.
    pub fn holdings_page(
        &self,
        device: &DeviceId,
        after: Option<&[u8]>,
        limit: usize,
    ) -> Result<(Vec<ChunkId>, Option<Vec<u8>>)> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(HOLDINGS)?;

        let start = after.map_or_else(
            || holders::device_range_start(device).to_vec(),
            <[u8]>::to_vec,
        );
        let end = holders::device_range_end(device);

        let mut out = Vec::new();
        for row in table.range(start.as_slice()..=end.as_slice())? {
            let (key, _) = row?;
            let raw = key.value().to_vec();
            if after.is_some_and(|previous| previous == raw.as_slice()) {
                continue; // the page boundary itself, already returned
            }
            let Some((_, chunk)) = holders::split_by_device(&raw) else {
                continue;
            };
            out.push(chunk);
            if out.len() >= limit {
                // Only a full page has a continuation. A short page is the end,
                // and handing back a cursor for it would make the caller ask
                // once more for nothing.
                return Ok((out, Some(raw)));
            }
        }
        Ok((out, None))
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
    use super::super::tests::*;
    use super::super::*;
    use super::*;

    /// The idle bench of 2026-10-27, its commits counted: a peer answering
    /// "missing" about chunks it never held -- in the first rounds after a
    /// start -- had
    /// `forget_holders` commit a transaction that removed nothing, some 24 KB
    /// on redb 2. Nothing to forget now costs no commit; a record really
    /// forgotten still does, and is gone.
    #[test]
    fn red_team_forgetting_what_was_never_recorded_costs_no_commit() {
        let (_dir, index) = index();
        let (d, other) = (device(1), device(2));
        let chunks = [chunk(1), chunk(2)];
        index.record_holders(&chunks[..1], &d, 100).unwrap();

        let before = crate::commits();
        index.forget_holders(&chunks, &other).unwrap();
        assert_eq!(
            crate::commits() - before,
            0,
            "forgetting holders that were never recorded cost a commit: one per peer in the first rounds"
        );

        let before = crate::commits();
        index.forget_holders(&chunks, &d).unwrap();
        assert_eq!(
            crate::commits() - before,
            1,
            "a real forget was not committed"
        );
        assert_eq!(
            index.held_by(&d).unwrap(),
            0,
            "the record was not forgotten"
        );
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

        // And the second acknowledgement, thirteen minutes later, wrote
        // nothing: the record was nowhere near stale. That is the difference
        // between 962 KB and 61 KB of disk written per round, measured on a
        // real node. See `holders::REFRESH_AFTER`.
        assert_eq!(
            holders[0].confirmed_unix, 100,
            "a record still fresh was rewritten, which is the whole cost"
        );

        // Once it has aged past a quarter of the freshness window it is
        // rewritten, so it can never drift into being uncountable.
        index
            .record_holder(&chunk(1), &device(7), 100 + holders::REFRESH_AFTER + 1)
            .unwrap();
        let holders = index.remote_holders(&chunk(1)).unwrap();
        assert_eq!(holders.len(), 1);
        assert_eq!(holders[0].confirmed_unix, 100 + holders::REFRESH_AFTER + 1);
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
        assert_eq!(
            index.remote_holders(&chunk(1)).unwrap(),
            [] as [holders::Holder; 0]
        );
    }

    #[test]
    fn the_ledger_survives_reopening() {
        // It is the only record of where this node put its data. Losing it on a
        // restart would make every node re-upload everything after a reboot.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.redb");
        {
            let index = Index::open(&path, test_audit_key()).unwrap();
            index.record_holder(&chunk(1), &device(7), 42).unwrap();
        }
        let index = Index::open(&path, test_audit_key()).unwrap();
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

    #[test]
    fn red_team_the_holder_count_follows_the_ledger_through_every_path() {
        // §8 1c reads how much an account proved it hosts from this count, on
        // every offer. A path that adds or removes a holder row without it
        // lets the proof drift from the ledger -- upward is credit nobody
        // earned. Sabotage: drop the decrement in `forget_holders`.
        let (_dir, index) = index();
        let (d, other) = (device(1), device(2));
        let chunks = [chunk(1), chunk(2), chunk(3), chunk(4)];
        index.record_holders(&chunks, &d, 100).unwrap();
        index.record_holders(&chunks[..1], &other, 100).unwrap();
        assert_eq!(index.held_by(&d).unwrap(), 4);

        // A refresh rewrites rows and must not count them twice.
        index
            .record_holders(&chunks, &d, 100 + holders::REFRESH_AFTER + 1)
            .unwrap();
        assert_eq!(
            index.held_by(&d).unwrap(),
            4,
            "a refresh was counted as new"
        );

        index.forget_holders(&chunks[..1], &d).unwrap();
        index.forget_holders(&chunks[..1], &d).unwrap();
        assert_eq!(
            index.held_by(&d).unwrap(),
            3,
            "a forget was not counted, or twice"
        );
        index.forget_holder(&chunks[1], &d).unwrap();
        assert_eq!(index.held_by(&d).unwrap(), 2);
        index.forget_chunk(&chunks[2]).unwrap();
        assert_eq!(index.held_by(&d).unwrap(), 1);
        assert_eq!(index.forget_device(&d).unwrap(), 1);
        assert_eq!(index.held_by(&d).unwrap(), 0);
        assert_eq!(
            index.held_by(&other).unwrap(),
            1,
            "another device's count moved"
        );
    }
}
