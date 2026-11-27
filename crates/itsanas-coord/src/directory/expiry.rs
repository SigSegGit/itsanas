//! What stops being kept: silent addresses, retired names and escrow counts
//! whose time is over.

use redb::ReadableTable;

use super::{
    AVAILABILITY, AvailabilityRecord, Directory, ESCROW_TRIES, PRESENCE, RETIRED,
    escrow::escrow_try, window_over,
};
use crate::{claim::SignedPresence, error::Result};

impl Directory {
    /// Erase the address and the availability record of every device silent
    /// for longer than `older_than`. Returns how many devices that was.
    ///
    /// Silence is this coordinator's own record, as in [`Self::last_seen`]: a
    /// Raspberry Pi with no real-time clock says 1970 in every announcement.
    /// The date inside a presence can only keep a row, and only for
    /// [`crate::claim::MAX_CLOCK_SKEW`] past when it was heard. A date after
    /// `now`, in either place, means this clock was set back, and that is no
    /// evidence of silence.
    ///
    /// The enrolment stays: a machine switched off for two months is still the
    /// account's, and its next announcement brings back all of this. A row
    /// that cannot be read has no date, so nothing keeps it but a record of
    /// having heard the device.
    ///
    /// One write transaction for the pass: read first and erased in a second,
    /// an address announced between the two would be the one erased.
    pub fn expire_presence(&self, now: u64, older_than: u64) -> Result<usize> {
        let txn = self.db.begin_write()?;
        let erased = {
            let mut presence = txn.open_table(PRESENCE)?;
            let mut availability = txn.open_table(AVAILABILITY)?;

            let mut silent = Vec::new();
            for row in presence.iter()? {
                let (device, value) = row?;
                // A row nobody can read serves nobody: erased whatever the
                // device's record says, rather than kept for a month.
                let Ok(signed) = postcard::from_bytes::<SignedPresence>(value.value()) else {
                    silent.push(device.value().to_vec());
                    continue;
                };
                let said = signed.presence.at_unix;
                let heard = availability
                    .get(device.value())?
                    .and_then(|record| {
                        postcard::from_bytes::<AvailabilityRecord>(record.value()).ok()
                    })
                    .map_or(0, |record| record.last_seen_unix);
                if now.saturating_sub(heard.max(said)) > older_than {
                    silent.push(device.value().to_vec());
                }
            }

            for device in &silent {
                presence.remove(device.as_slice())?;
                availability.remove(device.as_slice())?;
            }
            silent.len()
        };
        txn.commit()?;
        Ok(erased)
    }

    /// Drop every retired name whose `until` has passed. Returns how many.
    ///
    /// The same line `refuse_if_retired` draws, so this changes nothing anybody
    /// is told: only that the name of somebody who deleted their account stops
    /// being in this file once it no longer has a use here.
    pub fn expire_retired_names(&self, now: u64) -> Result<usize> {
        let txn = self.db.begin_write()?;
        let mut purged = 0;
        txn.open_table(RETIRED)?.retain(|_, until| {
            let free = until <= now;
            purged += usize::from(free);
            !free
        })?;
        txn.commit()?;
        Ok(purged)
    }

    /// Drop every count of escrow fetches whose `window` is over, and any that
    /// cannot be read. Returns how many.
    ///
    /// A count whose window is over is restarted by the next fetch of that name
    /// anyway, so this gives nobody a budget they did not have. What it removes
    /// is the name: any stranger can write one here, real or invented.
    pub fn expire_escrow_tries(&self, now: u64, window: u64) -> Result<usize> {
        let txn = self.db.begin_write()?;
        let mut purged = 0;
        txn.open_table(ESCROW_TRIES)?.retain(|_, value| {
            let over = escrow_try(value).is_none_or(|(_, since)| window_over(since, now, window));
            purged += usize::from(over);
            !over
        })?;
        txn.commit()?;
        Ok(purged)
    }
}
