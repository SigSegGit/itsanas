//! Chunks: storing, serving and removing an owner's sealed chunks, the
//! running totals the pledge reads, and the ordered list and summary a
//! reconciliation compares.

use itsanas_crypto::{ChunkId, UserId};
use redb::{ReadableTable, ReadableTableMetadata};

use super::{
    CHAIN_BYTES, CHAIN_KEY_LEN, CHUNK_BYTES, CHUNKS, EVICTABLE, OWNER_CHUNK_BYTES, TOTALS, Unsure,
    Vault, chunk_from_key, chunk_key, chunk_range_end, chunk_range_start, eviction::mark_stands,
    eviction::unmark, fault,
};
use crate::error::{Result, StoreError};

impl Vault {
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
        // ever. But committed only if that changed something: a commit is
        // some 24 KB whatever it holds, and a peer re-offering a chunk held
        // here at its size did so in the first rounds after a start.
        let txn = self.db.begin_write()?;
        let old;
        {
            old = txn
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
        let unmarked = unmark(&txn, owner, address)?;
        if old == Some(size) && !unmarked {
            txn.abort()?;
        } else {
            crate::commit(txn)?;
        }
        unsure.defuse();
        Ok(stored)
    }

    /// Serve a sealed chunk.
    pub fn get_chunk(&self, owner: UserId, address: &ChunkId) -> Result<Option<Vec<u8>>> {
        self.reads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.blobs_for(owner)?.get(address)
    }

    /// How many chunks this vault has read whole since it opened.
    #[must_use]
    pub fn chunk_reads(&self) -> u64 {
        self.reads.load(std::sync::atomic::Ordering::Relaxed)
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
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::super::*;

    /// The idle bench of 2026-10-27, its commits counted: a peer offering a
    /// chunk this vault already holds -- in the first rounds after a start,
    /// between two accounts hosting each other -- re-indexed it and committed, "costing
    /// nothing" as the comment said; a commit is some 24 KB on redb 2
    /// (`examples/commit_cost.rs`). Unchanged now costs none; a chunk new,
    /// or one marked evictable (stored again is wanted again), still does.
    #[test]
    fn red_team_a_chunk_stored_again_unchanged_costs_no_commit() {
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        let address = ChunkId::from_bytes([7; 32]);
        let before = crate::commits();
        assert!(vault.put_chunk(owner, &address, b"sealed").unwrap());
        assert_eq!(crate::commits() - before, 1, "a new chunk was not indexed");

        let before = crate::commits();
        assert!(!vault.put_chunk(owner, &address, b"sealed").unwrap());
        assert_eq!(
            crate::commits() - before,
            0,
            "a chunk this vault already held, offered again, cost a commit: one per peer \
             in the first rounds after a start"
        );

        vault.mark_evictable(owner, &[address], 1_000).unwrap();
        let before = crate::commits();
        vault.put_chunk(owner, &address, b"sealed").unwrap();
        assert_eq!(
            crate::commits() - before,
            1,
            "a chunk marked evictable and stored again was not unmarked"
        );
        assert_eq!(
            vault.mark_evictable(owner, &[address], 2_000).unwrap(),
            1,
            "the chunk stored again was still marked, and would be evicted"
        );
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
}
