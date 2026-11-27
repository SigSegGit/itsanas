//! Eviction: the marks an owner puts on copies it can spare, the daily cap
//! on them, and removing marked chunks to make room for that same owner.

use itsanas_crypto::{ChunkId, UserId};
use redb::ReadableTable;

use super::{
    CHUNK_BYTES, CHUNKS, DAY, EVICT_QUOTA, EVICTABLE, EVICTION_ORDER, MARK_EXPIRES,
    MAX_EVICT_PER_CALL, MAX_EVICTABLE_PER_DAY, OWNER_CHUNK_BYTES, TOTALS, Unsure, Vault, chunk_key,
};
use crate::error::Result;

/// Whether a mark made at `marked_at` still stands at `now`. A clock that
/// stepped back reads the mark as fresh, which only delays its expiry.
pub(super) const fn mark_stands(marked_at: u64, now: u64) -> bool {
    now.saturating_sub(marked_at) < MARK_EXPIRES
}

/// Bytes in an [`EVICTION_ORDER`] key.
const ORDER_KEY_LEN: usize = 32 + 8 + 32;

/// `owner ‖ marked_at ‖ chunk`, big-endian, so one owner's marks are a
/// contiguous range in the order they were made.
fn order_key(owner: UserId, marked_at: u64, chunk: &ChunkId) -> [u8; ORDER_KEY_LEN] {
    let mut out = [0u8; ORDER_KEY_LEN];
    out[..32].copy_from_slice(owner.as_bytes().as_slice());
    out[32..40].copy_from_slice(&marked_at.to_be_bytes());
    out[40..].copy_from_slice(chunk.as_bytes().as_slice());
    out
}

fn order_range(owner: UserId) -> ([u8; ORDER_KEY_LEN], [u8; ORDER_KEY_LEN]) {
    let mut low = [0u8; ORDER_KEY_LEN];
    low[..32].copy_from_slice(owner.as_bytes().as_slice());
    let mut high = [0xFFu8; ORDER_KEY_LEN];
    high[..32].copy_from_slice(owner.as_bytes().as_slice());
    (low, high)
}

/// Drop `owner`'s mark on `address`, if there is one, inside `txn`: called in
/// the transaction that stores or removes the chunk, so a mark never outlives
/// the decision it was about.
/// Whether a mark was removed.
pub(super) fn unmark(
    txn: &redb::WriteTransaction,
    owner: UserId,
    address: &ChunkId,
) -> Result<bool> {
    let marked_at = txn
        .open_table(EVICTABLE)?
        .remove(chunk_key(owner, address).as_slice())?
        .map(|at| at.value());
    if let Some(at) = marked_at {
        txn.open_table(EVICTION_ORDER)?
            .remove(order_key(owner, at, address).as_slice())?;
    }
    Ok(marked_at.is_some())
}

impl Vault {
    /// Mark those of `chunks` this vault holds for `owner` as evictable to make
    /// room for `owner` ([`Self::evict_marked`]), at most
    /// [`MAX_EVICTABLE_PER_DAY`] new marks per owner per day of `now`.
    ///
    /// Returns how many of `chunks` are marked now -- by this call or an
    /// earlier one, each counted once. A chunk not held is not marked: a mark
    /// on nothing would let a later store of that chunk arrive evictable. A
    /// chunk already marked keeps its first mark, so its place in the
    /// eviction order cannot be refreshed by asking again.
    ///
    /// Who may ask is not decided here: the caller (the peer service) only
    /// calls this for a device its bound says speaks for `owner`.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn mark_evictable(&self, owner: UserId, chunks: &[ChunkId], now: u64) -> Result<u32> {
        let today = now / DAY;
        let asked: std::collections::BTreeSet<&ChunkId> = chunks.iter().collect();
        let txn = self.db.begin_write()?;
        let marked = {
            let held = txn.open_table(CHUNKS)?;
            let mut marks = txn.open_table(EVICTABLE)?;
            let mut order = txn.open_table(EVICTION_ORDER)?;
            let mut quota = txn.open_table(EVICT_QUOTA)?;
            let key = owner.as_bytes().as_slice();
            // A clock that steps back stays on the day it had reached:
            // setting it back must not buy a fresh day's marks.
            let (day, mut made) = match quota.get(key)?.map(|row| row.value()) {
                Some((counted, made)) if counted >= today => (counted, made),
                _ => (today, 0),
            };
            let mut marked = 0u32;
            for chunk in asked {
                let row = chunk_key(owner, chunk);
                if marks.get(row.as_slice())?.is_some() {
                    marked = marked.saturating_add(1);
                    continue;
                }
                if held.get(row.as_slice())?.is_none() || made >= u64::from(MAX_EVICTABLE_PER_DAY) {
                    continue;
                }
                marks.insert(row.as_slice(), now)?;
                order.insert(order_key(owner, now, chunk).as_slice(), ())?;
                made += 1;
                marked = marked.saturating_add(1);
            }
            quota.insert(key, (day, made))?;
            marked
        };
        txn.commit()?;
        Ok(marked)
    }

    /// Whether `owner` has marked `address` evictable here.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn is_evictable(&self, owner: UserId, address: &ChunkId) -> Result<bool> {
        let txn = self.db.begin_read()?;
        Ok(txn
            .open_table(EVICTABLE)?
            .get(chunk_key(owner, address).as_slice())?
            .is_some())
    }

    /// Which of `chunks` carry a standing mark of `owner` at `now`.
    ///
    /// The peer service answers these as missing: a host that still said
    /// "have" for a chunk it is about to evict would have the owner re-stamp
    /// the copy it just released, and count it after the eviction (red team,
    /// HIGH 1). Answered missing, the owner re-offers it, and the store that
    /// follows removes the mark ([`Self::put_chunk`]).
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn marked_among(
        &self,
        owner: UserId,
        chunks: &[ChunkId],
        now: u64,
    ) -> Result<std::collections::BTreeSet<ChunkId>> {
        let txn = self.db.begin_read()?;
        let marks = txn.open_table(EVICTABLE)?;
        let mut out = std::collections::BTreeSet::new();
        for chunk in chunks {
            if marks
                .get(chunk_key(owner, chunk).as_slice())?
                .is_some_and(|at| mark_stands(at.value(), now))
            {
                out.insert(*chunk);
            }
        }
        Ok(out)
    }

    /// Remove `owner`'s marked chunks, oldest mark first, until `bytes_needed`
    /// are freed or [`MAX_EVICT_PER_CALL`] chunks have gone; returns the bytes
    /// freed.
    ///
    /// Only `owner`'s: the caller is making room for `owner`, and a mark is
    /// that owner's word about its own copies, never permission to drop
    /// somebody else's. Sizes are the index's, the same ones the totals the
    /// pledge reads are made of, so what this returns is what those totals
    /// lose. A mark whose chunk is no longer indexed frees nothing and is
    /// dropped.
    ///
    /// # Errors
    ///
    /// The database's or the filesystem's.
    ///
    /// Marks older than [`MARK_EXPIRES`] at `now` are never evicted; the ones
    /// met on the way are pruned. And nothing is deleted unless the standing
    /// marks within [`MAX_EVICT_PER_CALL`] cover `bytes_needed`: a partial
    /// eviction frees too little for the store that asked, which is refused
    /// anyway, and the copies are gone for nothing (red team, LOW).
    pub fn evict_marked(&self, owner: UserId, bytes_needed: u64, now: u64) -> Result<u64> {
        if bytes_needed == 0 {
            return Ok(0);
        }
        // Held from the choice to the commit, so no store of a chosen chunk
        // -- which would unmark it -- lands in between and is then deleted.
        let _writing = self.chunk_writes();
        let (chosen, expired, room) = self.choose_evictions(owner, bytes_needed, now)?;
        if !expired.is_empty() {
            let txn = self.db.begin_write()?;
            {
                let mut marks = txn.open_table(EVICTABLE)?;
                let mut order = txn.open_table(EVICTION_ORDER)?;
                for (key, chunk) in &expired {
                    marks.remove(chunk_key(owner, chunk).as_slice())?;
                    order.remove(key.as_slice())?;
                }
            }
            txn.commit()?;
        }
        if chosen.is_empty() || room < bytes_needed {
            return Ok(0);
        }

        let unsure = Unsure::arm(&self.suspect);
        let blobs = self.blobs_for(owner)?;
        for (_, chunk) in &chosen {
            blobs.remove(chunk)?;
        }
        let txn = self.db.begin_write()?;
        let freed = {
            let mut table = txn.open_table(CHUNKS)?;
            let mut marks = txn.open_table(EVICTABLE)?;
            let mut order = txn.open_table(EVICTION_ORDER)?;
            let mut freed = 0u64;
            for (key, chunk) in &chosen {
                let row = chunk_key(owner, chunk);
                let old = table.remove(row.as_slice())?.map_or(0, |old| old.value());
                freed = freed.saturating_add(old);
                marks.remove(row.as_slice())?;
                order.remove(key.as_slice())?;
            }
            freed
        };
        {
            let mut totals = txn.open_table(TOTALS)?;
            let total = totals.get(CHUNK_BYTES)?.map_or(0, |total| total.value());
            totals.insert(CHUNK_BYTES, total.saturating_sub(freed))?;
            let mut per_owner = txn.open_table(OWNER_CHUNK_BYTES)?;
            let key = owner.as_bytes().as_slice();
            let mine = per_owner.get(key)?.map_or(0, |total| total.value());
            per_owner.insert(key, mine.saturating_sub(freed))?;
        }
        txn.commit()?;
        unsure.defuse();
        Ok(freed)
    }

    /// The standing marks [`Self::evict_marked`] would remove, oldest first,
    /// the expired ones met on the way, and the bytes the chosen ones free.
    #[allow(clippy::type_complexity)]
    fn choose_evictions(
        &self,
        owner: UserId,
        bytes_needed: u64,
        now: u64,
    ) -> Result<(
        Vec<([u8; ORDER_KEY_LEN], ChunkId)>,
        Vec<([u8; ORDER_KEY_LEN], ChunkId)>,
        u64,
    )> {
        let txn = self.db.begin_read()?;
        let order = txn.open_table(EVICTION_ORDER)?;
        let held = txn.open_table(CHUNKS)?;
        let (low, high) = order_range(owner);
        let mut chosen = Vec::new();
        let mut expired = Vec::new();
        let mut room = 0u64;
        for row in order.range(low.as_slice()..=high.as_slice())? {
            // Expired marks count towards the bound too: a walk past a
            // million stale marks under the storing lock is the stall the
            // bound exists to prevent.
            if room >= bytes_needed || chosen.len() + expired.len() >= MAX_EVICT_PER_CALL {
                break;
            }
            let (key, _) = row?;
            let Ok(key) = <[u8; ORDER_KEY_LEN]>::try_from(key.value()) else {
                continue;
            };
            let mut at = [0u8; 8];
            at.copy_from_slice(&key[32..40]);
            let mut chunk = [0u8; 32];
            chunk.copy_from_slice(&key[40..]);
            let chunk = ChunkId::from_bytes(chunk);
            if !mark_stands(u64::from_be_bytes(at), now) {
                expired.push((key, chunk));
                continue;
            }
            let size = held
                .get(chunk_key(owner, &chunk).as_slice())?
                .map_or(0, |size| size.value());
            room = room.saturating_add(size);
            chosen.push((key, chunk));
        }
        Ok((chosen, expired, room))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::super::*;
    use redb::ReadableTableMetadata;

    fn chunk(byte: u8) -> ChunkId {
        ChunkId::from_bytes([byte; 32])
    }

    const MARKED_AT: u64 = 1_800_000_000;

    #[test]
    fn red_team_a_mark_on_a_chunk_not_held_marks_nothing() {
        // A mark on nothing would let a later store of that chunk arrive
        // already evictable, so the next refusal for room takes a copy the
        // owner just sent and counts. Sabotage: drop the `held` check.
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        vault.put_chunk(owner, &chunk(1), &[1; 10]).unwrap();

        assert_eq!(
            vault
                .mark_evictable(owner, &[chunk(1), chunk(2)], MARKED_AT)
                .unwrap(),
            1
        );
        vault.put_chunk(owner, &chunk(2), &[2; 10]).unwrap();
        assert!(!vault.is_evictable(owner, &chunk(2)).unwrap());
        assert!(vault.is_evictable(owner, &chunk(1)).unwrap());
    }

    #[test]
    fn red_team_an_expired_mark_is_neither_evicted_nor_answered_marked() {
        // An owner counts a released copy again after CONFIRMED_FOR. A mark
        // that never expired would let a store years later delete a copy
        // the owner had long gone back to counting. Sabotages: drop the
        // expiry check in `choose_evictions`; or in `marked_among`.
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        vault.put_chunk(owner, &chunk(1), &[1; 10]).unwrap();
        vault.mark_evictable(owner, &[chunk(1)], MARKED_AT).unwrap();
        let later = MARKED_AT + MARK_EXPIRES;
        assert!(
            vault
                .marked_among(owner, &[chunk(1)], MARKED_AT + 1)
                .unwrap()
                .contains(&chunk(1))
        );
        assert!(
            vault
                .marked_among(owner, &[chunk(1)], later)
                .unwrap()
                .is_empty()
        );
        assert_eq!(vault.evict_marked(owner, 10, later).unwrap(), 0);
        assert!(vault.has_chunk(owner, &chunk(1)).unwrap());
        // And it was pruned on the way.
        assert!(!vault.is_evictable(owner, &chunk(1)).unwrap());
    }

    #[test]
    fn red_team_eviction_deletes_nothing_when_the_marks_cannot_cover_the_need() {
        // Freeing 20 bytes towards a need of 30 still leaves the store
        // refused: the copies would be gone for nothing. Sabotage: drop the
        // `room < bytes_needed` check.
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        for byte in 1..=2 {
            vault.put_chunk(owner, &chunk(byte), &[byte; 10]).unwrap();
        }
        vault
            .mark_evictable(owner, &[chunk(1), chunk(2)], MARKED_AT)
            .unwrap();
        assert_eq!(vault.evict_marked(owner, 30, MARKED_AT).unwrap(), 0);
        assert!(vault.has_chunk(owner, &chunk(1)).unwrap());
        assert!(vault.has_chunk(owner, &chunk(2)).unwrap());
        assert_eq!(vault.evict_marked(owner, 20, MARKED_AT).unwrap(), 20);
    }

    #[test]
    fn a_marked_chunk_that_is_stored_again_is_no_longer_evictable() {
        // An owner that re-sends a chunk it marked counts this copy again;
        // evicting it would take a copy the owner believes in. Sabotage: drop
        // the `unmark` in `put_chunk`.
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        for byte in 1..=3 {
            vault.put_chunk(owner, &chunk(byte), &[byte; 10]).unwrap();
        }
        vault
            .mark_evictable(owner, &[chunk(1), chunk(2), chunk(3)], MARKED_AT)
            .unwrap();
        vault.put_chunk(owner, &chunk(2), &[2; 10]).unwrap();

        assert!(!vault.is_evictable(owner, &chunk(2)).unwrap());
        assert_eq!(vault.evict_marked(owner, 20, MARKED_AT).unwrap(), 20);
        assert!(vault.has_chunk(owner, &chunk(2)).unwrap());
        assert!(!vault.has_chunk(owner, &chunk(1)).unwrap());
        assert!(!vault.has_chunk(owner, &chunk(3)).unwrap());
    }

    #[test]
    fn red_team_eviction_never_takes_another_owners_chunks() {
        // A mark is one owner's word about its own copies. Making room for
        // that owner by dropping somebody else's would lose a copy nobody
        // agreed to lose. Sabotage: range over every owner's marks.
        let (_dir, vault) = vault();
        let (alice, bob) = (keys(1).user_id(), keys(2).user_id());
        vault.put_chunk(alice, &chunk(1), &[1; 10]).unwrap();
        vault.put_chunk(bob, &chunk(2), &[2; 10]).unwrap();
        vault.mark_evictable(bob, &[chunk(2)], MARKED_AT).unwrap();

        assert_eq!(vault.evict_marked(alice, 10, MARKED_AT).unwrap(), 0);
        assert!(vault.has_chunk(bob, &chunk(2)).unwrap());
        assert_eq!(vault.held_bytes_for(bob).unwrap(), 10);
    }

    #[test]
    fn eviction_takes_the_oldest_marks_first_and_stops_when_there_is_room() {
        // Freeing more than asked loses copies for nothing; the totals must
        // fall by exactly what went, or the pledge reads a host as fuller
        // than it is. Sabotage: ignore `bytes_needed`.
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        for byte in 1..=4 {
            vault.put_chunk(owner, &chunk(byte), &[byte; 10]).unwrap();
        }
        // Marked out of address order, so oldest-first is not address order.
        vault.mark_evictable(owner, &[chunk(3)], MARKED_AT).unwrap();
        vault
            .mark_evictable(owner, &[chunk(1)], MARKED_AT + 1)
            .unwrap();
        vault
            .mark_evictable(owner, &[chunk(2)], MARKED_AT + 2)
            .unwrap();

        assert_eq!(vault.evict_marked(owner, 15, MARKED_AT).unwrap(), 20);
        assert!(!vault.has_chunk(owner, &chunk(3)).unwrap());
        assert!(!vault.has_chunk(owner, &chunk(1)).unwrap());
        assert!(vault.has_chunk(owner, &chunk(2)).unwrap());
        assert!(vault.has_chunk(owner, &chunk(4)).unwrap());
        assert_eq!(vault.held_bytes_for(owner).unwrap(), 20);
        assert_eq!(vault.held_bytes().unwrap(), 20);
    }

    #[test]
    fn red_team_an_owner_marks_at_most_a_days_cap_even_with_its_clock_set_back() {
        // A device of the account turned hostile would otherwise mark all the
        // account holds here in one session. Sabotages: skip the cap; reset
        // the count when the day number goes down.
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        let many: Vec<ChunkId> = (0..MAX_EVICTABLE_PER_DAY + 2)
            .map(|i| {
                let mut bytes = [0u8; 32];
                bytes[..4].copy_from_slice(&i.to_be_bytes());
                ChunkId::from_bytes(bytes)
            })
            .collect();
        {
            // Held, without a blob each: only the index decides what is held.
            let txn = vault.db.begin_write().unwrap();
            {
                let mut table = txn.open_table(CHUNKS).unwrap();
                for id in &many {
                    table.insert(chunk_key(owner, id).as_slice(), 1).unwrap();
                }
            }
            txn.commit().unwrap();
        }
        let today = 20_000 * DAY;
        let (first, rest) = many.split_at(MAX_EVICTABLE_PER_DAY as usize);

        assert_eq!(
            vault.mark_evictable(owner, first, today).unwrap(),
            MAX_EVICTABLE_PER_DAY
        );
        assert_eq!(vault.mark_evictable(owner, rest, today + 60).unwrap(), 0);
        assert_eq!(vault.mark_evictable(owner, rest, today - DAY).unwrap(), 0);
        assert_eq!(vault.mark_evictable(owner, rest, today + DAY).unwrap(), 2);
    }

    #[test]
    fn forgetting_an_owner_forgets_its_marks_and_its_days_count() {
        // A returning account must not find its old chunks' marks waiting,
        // nor a day's cap already spent. Sabotage: leave EVICT_QUOTA alone.
        let (_dir, vault) = vault();
        let owner = keys(1).user_id();
        vault.put_chunk(owner, &chunk(1), &[1; 10]).unwrap();
        vault.mark_evictable(owner, &[chunk(1)], MARKED_AT).unwrap();
        vault.forget_owner(owner).unwrap();

        let txn = vault.db.begin_read().unwrap();
        assert!(txn.open_table(EVICTABLE).unwrap().is_empty().unwrap());
        assert!(txn.open_table(EVICTION_ORDER).unwrap().is_empty().unwrap());
        assert!(txn.open_table(EVICT_QUOTA).unwrap().is_empty().unwrap());
    }

    #[test]
    fn a_vault_from_before_eviction_opens_and_marks() {
        // An older vault has none of the eviction tables, and a read of a
        // missing table is an error: every `MayEvict` and every refusal for
        // room would fail. Sabotage: drop the `open_table` lines in `open`.
        let dir = tempfile::tempdir().unwrap();
        let owner = keys(1).user_id();
        {
            let vault = Vault::open(dir.path()).unwrap();
            vault.put_chunk(owner, &chunk(1), &[1; 10]).unwrap();
            let txn = vault.db.begin_write().unwrap();
            txn.delete_table(EVICTABLE).unwrap();
            txn.delete_table(EVICTION_ORDER).unwrap();
            txn.delete_table(EVICT_QUOTA).unwrap();
            txn.commit().unwrap();
        }
        let vault = Vault::open(dir.path()).unwrap();
        assert!(!vault.is_evictable(owner, &chunk(1)).unwrap());
        assert_eq!(vault.evict_marked(owner, 10, MARKED_AT).unwrap(), 0);
        assert_eq!(
            vault.mark_evictable(owner, &[chunk(1)], MARKED_AT).unwrap(),
            1
        );
    }
}
