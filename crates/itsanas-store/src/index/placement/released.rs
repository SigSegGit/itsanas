//! Released copies: which copies this node gave back and when, so the same
//! copy is not released twice, and when each device last passed an audit.

use itsanas_crypto::{ChunkId, DeviceId};

use super::{DEVICE_PASSED, RELEASED};
use crate::{error::Result, holders, index::Index};

impl Index {
    // ------------------------------------------------------------- released

    /// Record that this node released its claim on `device`'s copy of each of
    /// `chunks`, at `now`.
    ///
    /// # Errors
    ///
    /// If the index cannot be written.
    pub fn note_released(&self, chunks: &[ChunkId], device: &DeviceId, now: u64) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let txn = self.db.begin_write()?;
        {
            let mut released = txn.open_table(RELEASED)?;
            for chunk in chunks {
                released.insert(holders::key(chunk, device).as_slice(), now)?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    /// Of `chunks`, those whose copy on `device` this node has not released
    /// within [`holders::CONFIRMED_FOR`] of `now`, in the order given.
    pub(in crate::index) fn not_released(
        &self,
        chunks: &[ChunkId],
        device: &DeviceId,
        now: u64,
    ) -> Result<Vec<ChunkId>> {
        if chunks.is_empty() {
            return Ok(Vec::new());
        }
        let txn = self.db.begin_read()?;
        let released = txn.open_table(RELEASED)?;
        let mut out = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            let recent = released
                .get(holders::key(chunk, device).as_slice())?
                .is_some_and(|at| now.saturating_sub(at.value()) < holders::CONFIRMED_FOR);
            if !recent {
                out.push(*chunk);
            }
        }
        Ok(out)
    }

    /// Forget that this node released `device`'s copy of each of `chunks`:
    /// the host has just stored them again.
    pub(in crate::index) fn clear_released(
        &self,
        chunks: &[ChunkId],
        device: &DeviceId,
    ) -> Result<()> {
        // Read first: almost every store is of a chunk never released, and a
        // commit per accepted batch to remove nothing is what the ledger's
        // read-before-write rule exists to avoid.
        {
            let txn = self.db.begin_read()?;
            let released = txn.open_table(RELEASED)?;
            let mut any = false;
            for chunk in chunks {
                any |= released
                    .get(holders::key(chunk, device).as_slice())?
                    .is_some();
            }
            if !any {
                return Ok(());
            }
        }
        let txn = self.db.begin_write()?;
        {
            let mut released = txn.open_table(RELEASED)?;
            for chunk in chunks {
                released.remove(holders::key(chunk, device).as_slice())?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    /// When `device` last passed a storage audit here, if it has since this
    /// table existed.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn last_passed(&self, device: &DeviceId) -> Result<Option<u64>> {
        let txn = self.db.begin_read()?;
        Ok(txn
            .open_table(DEVICE_PASSED)?
            .get(device.as_bytes().as_slice())?
            .map(|value| value.value()))
    }

    /// When this node last released `device`'s copy of `chunk`, if it has.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn released_at(&self, chunk: &ChunkId, device: &DeviceId) -> Result<Option<u64>> {
        let txn = self.db.begin_read()?;
        Ok(txn
            .open_table(RELEASED)?
            .get(holders::key(chunk, device).as_slice())?
            .map(|value| value.value()))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::*;

    #[test]
    fn red_team_a_released_copy_is_not_re_recorded_on_the_hosts_word() {
        // The host still answers "have" until it evicts; re-stamping then
        // would count, for LIVE_FOR, a copy about to be deleted. Only a fresh
        // store it accepted brings the record back. Sabotages: drop
        // `not_released` from `record_holders`; or `clear_released` from
        // `record_stored`.
        let (_dir, index) = index();
        let host = device(5);
        index
            .record_holders(&[chunk(1)], &host, T0)
            .expect("record");
        index.forget_holders(&[chunk(1)], &host).expect("forget");
        index
            .note_released(&[chunk(1)], &host, T0)
            .expect("released");

        index
            .record_holders(&[chunk(1)], &host, T0 + HOUR)
            .expect("have");
        assert_eq!(
            index.holders_with_stamps(&chunk(1)).expect("read"),
            [] as [(itsanas_crypto::DeviceId, u64); 0]
        );

        // Past the window, its word counts again.
        let after = T0 + holders::CONFIRMED_FOR;
        index
            .record_holders(&[chunk(1)], &host, after)
            .expect("have");
        assert_eq!(index.holders_with_stamps(&chunk(1)).expect("read").len(), 1);

        // A fresh store ends the release at once.
        index.forget_holders(&[chunk(2)], &host).expect("noop");
        index
            .note_released(&[chunk(2)], &host, T0)
            .expect("released");
        index
            .record_stored(&[chunk(2)], &host, T0 + HOUR)
            .expect("stored");
        assert_eq!(index.holders_with_stamps(&chunk(2)).expect("read").len(), 1);
        assert_eq!(index.released_at(&chunk(2), &host).expect("read"), None);
    }
}
