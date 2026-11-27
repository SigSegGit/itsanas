//! The rebuild at open: the chunk index and its totals made to agree with
//! the blob directories after an unclean exit or an upgrade, in batches,
//! and the segment bytes of a vault from before they counted.

use itsanas_crypto::{ChunkId, UserId};
use redb::ReadableTable;

use super::{
    CHAIN_BYTES, CHAIN_KEY_LEN, CHUNK_BYTES, CHUNKS, OPEN, OWNER_CHUNK_BYTES, OWNER_TOTALS,
    REBUILD_BATCH, SEGMENTS, TOTALS, Vault, chunk_key, fault,
};
use crate::error::Result;

impl Vault {
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
    pub(super) fn reconcile_chunks_if_needed(&self) -> Result<()> {
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
    pub(super) fn backfill_segment_bytes(&self) -> Result<()> {
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
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::super::*;

    #[test]
    fn red_team_a_crash_between_a_blob_and_its_row_is_rebuilt_at_open() {
        let dir = tempfile::tempdir().unwrap();
        let owner = keys(23).user_id();
        let (a, b, c) = (
            ChunkId::from_bytes([1; 32]),
            ChunkId::from_bytes([2; 32]),
            ChunkId::from_bytes([3; 32]),
        );
        // A vault made before packs, one file per chunk: what this test is
        // about. A shard directory makes the owner's store not new.
        let shard = dir
            .path()
            .join("owners")
            .join(owner.to_hex())
            .join("blobs")
            .join("00");
        std::fs::create_dir_all(&shard).unwrap();
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
}
