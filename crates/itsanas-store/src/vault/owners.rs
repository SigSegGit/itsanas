//! Owners: who this vault holds anything for, what it holds for each, and
//! forgetting an owner whose account was deleted.

use itsanas_crypto::UserId;
use redb::{ReadableTable, TableDefinition};

use super::{
    CHAIN_BYTES, CHAIN_KEY_LEN, CHAIN_LENGTHS, EVICT_QUOTA, EVICTABLE, EVICTION_ORDER, HEADS,
    SEGMENTS, Vault, VaultStats,
};
use crate::error::{Result, StoreError};

/// Keys removed per transaction when an owner is forgotten.
const FORGET_BATCH: u64 = 1024;

impl Vault {
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
        // Closed first: an open pack index is a file Windows will not delete.
        self.stores
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&owner);
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

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::super::*;

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
}
