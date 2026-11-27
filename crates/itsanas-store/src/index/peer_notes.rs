//! What a peer told this node about itself: its ledger walked, its pledge
//! full, its drain, and what its leaving would expose.

use itsanas_crypto::DeviceId;

use super::{
    CHUNK_REFS, DRAINING, HOLDER_COUNTS, HOLDERS, HOLDINGS, Index, LEDGER_REFRESHED, PEER_FULL,
    draining_in,
};
use crate::{
    error::Result,
    holders::{self},
};

impl Index {
    /// Up to `limit` live chunks starting where `cursor` lands, wrapping.
    ///
    /// For the repair scan, which asks "which of these are missing from disk?"
    /// A bounded walk from a moving start, not the whole table: at a terabyte
    /// there are fourteen million live chunks and stat-ing all of them every
    /// round would cost more than the loss it is looking for. Successive calls
    /// with fresh cursors cover the store over time.
    /// Whether the whole ledger for `device` is due a walk.
    ///
    /// A round that reconciles in one hash does not touch the ledger when the
    /// two sides agree, so without this every record for that peer would age
    /// past [`holders::CONFIRMED_FOR`] in silence — `release` would stop
    /// working and `coverage` would report no copies, on an account nothing had
    /// gone wrong with. The walk still happens, once every
    /// [`holders::REFRESH_AFTER`] rather than every round.
    pub fn ledger_walk_due(&self, device: &DeviceId, now: u64) -> Result<bool> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(LEDGER_REFRESHED)?;
        let last = table
            .get(device.as_bytes().as_slice())?
            .map_or(0, |value| value.value());
        Ok(last < now.saturating_sub(holders::REFRESH_AFTER))
    }

    /// Record that the whole ledger for `device` has just been walked.
    pub fn note_ledger_walk(&self, device: &DeviceId, now: u64) -> Result<()> {
        let txn = self.db.begin_write()?;
        {
            txn.open_table(LEDGER_REFRESHED)?
                .insert(device.as_bytes().as_slice(), now)?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Record that `device` refused a chunk at `now` because its pledge was
    /// full.
    pub fn note_peer_full(&self, device: &DeviceId, now: u64) -> Result<()> {
        let txn = self.db.begin_write()?;
        {
            txn.open_table(PEER_FULL)?
                .insert(device.as_bytes().as_slice(), now)?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Record that `device` is draining until `until`, capped at
    /// [`holders::MAX_DRAIN`] past `now` whatever it asked.
    ///
    /// Written only for a device this ledger counts as holding something, so
    /// a stranger minting keys cannot grow the table or force a commit per
    /// request. Never extended: a grant stands as written, and a new one is
    /// refused until as long again has passed since it ended, so a holder
    /// cannot renew itself into being ignored for good. Returns whether a
    /// grant was written.
    pub fn note_draining(&self, device: &DeviceId, until: u64, now: u64) -> Result<bool> {
        let until = until.min(now.saturating_add(holders::MAX_DRAIN));
        let key = device.as_bytes().as_slice();
        {
            let txn = self.db.begin_read()?;
            let holds = txn
                .open_table(HOLDER_COUNTS)?
                .get(key)?
                .is_some_and(|count| count.value() > 0);
            let cooling = txn
                .open_table(DRAINING)?
                .get(key)?
                .is_some_and(|ends| now < ends.value().saturating_add(holders::MAX_DRAIN));
            if !holds || cooling {
                return Ok(false);
            }
        }
        let txn = self.db.begin_write()?;
        {
            txn.open_table(DRAINING)?.insert(key, until)?;
            // Its copies stop counting now; the chunks they leave short are
            // queued in the same transaction, so a crash cannot leave a drain
            // granted and nothing working to replace what it takes away.
            Self::enqueue_device_in(&txn, device, now)?;
        }
        txn.commit()?;
        Ok(true)
    }

    /// Whether `device` is draining at `now`.
    pub fn is_draining(&self, device: &DeviceId, now: u64) -> Result<bool> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(DRAINING)?;
        draining_in(&table, device, now)
    }

    /// How many live chunks `device` holds that no other live, non-draining
    /// device holds: what this account would be one replica short of if that
    /// device went now. The copy on this disk is not counted, so 0 means
    /// "another machine has every one of them", not "the target is met".
    ///
    /// Asked by the draining device about itself only (the service passes the
    /// device its connection proved), so it teaches a stranger nothing.
    pub fn exposed_by(&self, device: &DeviceId, now: u64) -> Result<u64> {
        let live_since = now.saturating_sub(holders::LIVE_FOR);
        let txn = self.db.begin_read()?;
        let refs = txn.open_table(CHUNK_REFS)?;
        let holders_table = txn.open_table(HOLDERS)?;
        let holdings = txn.open_table(HOLDINGS)?;
        let draining = txn.open_table(DRAINING)?;

        let mut exposed = 0u64;
        for row in holdings.range(
            holders::device_range_start(device).as_slice()
                ..=holders::device_range_end(device).as_slice(),
        )? {
            let (key, _) = row?;
            let Some((_, chunk)) = holders::split_by_device(key.value()) else {
                continue;
            };
            if refs
                .get(chunk.as_bytes().as_slice())?
                .is_none_or(|count| count.value() == 0)
            {
                continue;
            }
            let mut elsewhere = false;
            for holder in holders_table.range(
                holders::range_start(&chunk).as_slice()..=holders::range_end(&chunk).as_slice(),
            )? {
                let (key, confirmed) = holder?;
                let Some(other) = holders::device_from_key(key.value()) else {
                    continue;
                };
                if other != *device
                    && confirmed.value() >= live_since
                    && !draining_in(&draining, &other, now)?
                {
                    elsewhere = true;
                    break;
                }
            }
            if !elsewhere {
                exposed += 1;
            }
        }
        Ok(exposed)
    }

    /// When `device` last refused a chunk for a full pledge, if ever.
    pub fn peer_full_since(&self, device: &DeviceId) -> Result<Option<u64>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(PEER_FULL)?;
        Ok(table
            .get(device.as_bytes().as_slice())?
            .map(|value| value.value()))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::*;

    #[test]
    fn red_team_a_drain_expires_and_cannot_exclude_a_holder_for_a_week() {
        // An interrupted drain, or a device asking for a month, must not make
        // its owner ignore a real holder -- and re-replicate around it -- for
        // longer than a person waiting at a screen.
        let (_dir, index) = index();
        index.put_file("a", &entry(&[1])).unwrap();
        let now = 10_000;
        index.record_holder(&chunk(1), &device(7), now).unwrap();
        index
            .note_draining(&device(7), now + 30 * 24 * 3600, now)
            .unwrap();
        assert!(index.is_draining(&device(7), now).unwrap());
        let later = now + holders::MAX_DRAIN + 1;
        assert!(
            !index.is_draining(&device(7), later).unwrap(),
            "the owner took the device's word for how long to ignore it"
        );

        // Renewing is refused: not while the grant runs, and not until as
        // long again has passed after it ended.
        let renewed = now + holders::MAX_DRAIN - 1;
        assert!(
            !index
                .note_draining(&device(7), renewed + 600, renewed)
                .unwrap()
        );
        assert!(
            !index.is_draining(&device(7), later).unwrap(),
            "a holder renewed its own exclusion past the cap"
        );
        assert!(!index.note_draining(&device(7), later + 600, later).unwrap());
        let cooled = now + 2 * holders::MAX_DRAIN;
        assert!(
            index
                .note_draining(&device(7), cooled + 600, cooled)
                .unwrap()
        );

        // `Leaving` takes the row with the records.
        index.forget_device(&device(7)).unwrap();
        assert!(!index.is_draining(&device(7), cooled).unwrap());
    }

    #[test]
    fn red_team_a_stranger_draining_writes_nothing() {
        // Device keys are free. A row per stranger, with a commit each, would
        // be a disk and fsync amplifier on a Pi for anybody who can connect.
        let (_dir, index) = index();
        assert!(!index.note_draining(&device(9), 10_600, 10_000).unwrap());
        assert!(
            !index.is_draining(&device(9), 10_000).unwrap(),
            "a device holding nothing here got a draining row"
        );
    }
}
