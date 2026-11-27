//! Availability, measured rather than asserted, and what each device
//! contributes.

use itsanas_crypto::UserId;
use redb::ReadableTable;

use super::{AVAILABILITY, AvailabilityRecord, Directory, SMOOTHING_ALPHA_PER_MILLE, TICK_SECONDS};
use crate::{accounting::DeviceContribution, error::Result};

impl Directory {
    /// Fold one period's observation into every device's availability.
    ///
    /// **Nothing calls this, and that is a decision rather than an omission.**
    /// `ECONOMICS.md` removed availability from the coordinator: a member
    /// measures their own, each pair measures each other, and a coordinator may
    /// publish a hint that nothing depends on. Its doc comment said "called on
    /// a timer" until 2026-09-21, which was a present tense for something no
    /// timer has ever called -- and reading it as live is what held a whole
    /// step up while somebody waited for a decision about a heartbeat that
    /// feeds nothing.
    ///
    /// Kept because the arithmetic is right and tested, and the bilateral model
    /// needs the same shape between two peers. A caller that feeds this into
    /// entitlement is a bug.
    ///
    /// A device heard from since its last tick would count as up for that
    /// period; one that was not counts as down. Nothing a node says about
    /// itself enters the calculation.
    pub fn tick(&self, now: u64) -> Result<usize> {
        let txn = self.db.begin_write()?;
        let mut folded = 0;

        {
            let mut availability = txn.open_table(AVAILABILITY)?;

            let mut updates: Vec<(Vec<u8>, AvailabilityRecord)> = Vec::new();
            for row in availability.iter()? {
                let (key, value) = row?;
                let mut record: AvailabilityRecord = postcard::from_bytes(value.value())?;

                let elapsed = now.saturating_sub(record.last_tick_unix);
                if elapsed < TICK_SECONDS {
                    continue;
                }

                // Fold one observation per elapsed period, capped so a
                // coordinator that was itself down for a month does not
                // annihilate everyone's standing in a single pass.
                let periods = (elapsed / TICK_SECONDS).min(32);
                for period in 0..periods {
                    let period_end = record
                        .last_tick_unix
                        .saturating_add((period + 1) * TICK_SECONDS);
                    let seen = record.last_seen_unix.saturating_add(TICK_SECONDS) >= period_end;
                    record.per_mille = fold(record.per_mille, seen);
                }

                record.last_tick_unix = now;
                updates.push((key.value().to_vec(), record));
                folded += 1;
            }

            for (key, record) in updates {
                availability.insert(key.as_slice(), postcard::to_stdvec(&record)?.as_slice())?;
            }
        }

        txn.commit()?;
        Ok(folded)
    }

    /// **Nothing on the live path may call this, and that is a decision.**
    ///
    /// `ECONOMICS.md` removed availability from the coordinator: "A member measures
    /// their own, and each pair measures each other. A coordinator may still
    /// publish a hint; nothing depends on it." So this measures a hint, and a
    /// caller that feeds it into entitlement is a **bug**, not a feature returning.
    ///
    /// It is kept rather than deleted because the arithmetic is right and tested,
    /// and the bilateral model needs the same shape between two peers. It is
    /// documented here because its silence is misleading: tested, public and
    /// unused reads as "finished and waiting", and `scripts/check-wired.py`
    /// deliberately counts tests as callers, so nothing flags it. That silence
    /// cost three exchanges in 2026-09: a whole step was held up waiting for a
    /// decision about a heartbeat that feeds nothing.
    /// What each of a member's live devices contributes.
    pub fn contributions(&self, user: UserId) -> Result<Vec<DeviceContribution>> {
        let txn = self.db.begin_read()?;
        let availability = txn.open_table(AVAILABILITY)?;

        let mut out = Vec::new();
        for signed in self.live_claims_of(user)? {
            let key = signed.claim.device.to_bytes();
            let per_mille = match availability.get(key.as_slice())? {
                Some(value) => postcard::from_bytes::<AvailabilityRecord>(value.value())?.per_mille,
                None => crate::accounting::AVAILABILITY_FLOOR_PER_MILLE,
            };

            out.push(DeviceContribution {
                device: signed.claim.device,
                pledged_bytes: signed.claim.pledged_bytes,
                availability_per_mille: per_mille,
            });
        }

        out.sort_by_key(|contribution| contribution.device.to_bytes());
        Ok(out)
    }
}

/// Fold one observation into a smoothed per-mille average.
fn fold(current: u16, seen: bool) -> u16 {
    let observation: u64 = if seen { 1000 } else { 0 };
    let smoothed = (u64::from(current) * (1000 - SMOOTHING_ALPHA_PER_MILLE)
        + observation * SMOOTHING_ALPHA_PER_MILLE)
        / 1000;

    u16::try_from(smoothed.min(1000)).unwrap_or(1000)
}
