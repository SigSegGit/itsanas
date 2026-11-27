//! Presence and departures: where a device says it is, and its notice that
//! it is leaving.

use itsanas_crypto::DeviceId;
use redb::ReadableTable;

use super::{AVAILABILITY, AvailabilityRecord, DEPARTURES, DepartureRecord, Directory, PRESENCE};
use crate::{
    claim::{SignedDeparture, SignedPresence},
    error::{CoordError, Result},
};

impl Directory {
    /// Record where a device says it is.
    ///
    /// Refused for a device with no live claim: an unclaimed machine announcing
    /// an address is either a mistake or somebody trying to get into the node
    /// set without an account to be held responsible.
    pub fn announce(&self, signed: &SignedPresence, now: u64) -> Result<()> {
        signed.verify(now)?;
        self.keep_presence(signed, now)
    }

    /// Record a presence **without checking its signature**: a coordinator
    /// that lies, for the tests of the clients that must not believe one.
    ///
    /// A client reading `SignedPeers` checks every address against its device,
    /// and the only way to prove that check is wired is a coordinator that
    /// hands out a presence nobody signed. Behind a feature that only
    /// dev-dependencies enable, so no coordinator that is built to run has it.
    /// Make the server behind this directory close the connection on
    /// `SignedPeers`, as a coordinator older than it does: a coordinator that
    /// pretends to be old, to talk its clients down to the unsigned list.
    #[cfg(feature = "hostile")]
    #[doc(hidden)]
    pub fn play_old(&self, old: bool) {
        self.plays_old
            .store(old, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether [`Self::play_old`] is on.
    #[cfg(feature = "hostile")]
    #[doc(hidden)]
    #[must_use]
    pub fn plays_old(&self) -> bool {
        self.plays_old.load(std::sync::atomic::Ordering::Relaxed)
    }

    #[cfg(feature = "hostile")]
    #[doc(hidden)]
    pub fn plant_presence(&self, signed: &SignedPresence, now: u64) -> Result<()> {
        self.keep_presence(signed, now)
    }

    fn keep_presence(&self, signed: &SignedPresence, now: u64) -> Result<()> {
        let claim = self
            .claim_for(signed.presence.device)?
            .filter(|claim| !claim.claim.revoked)
            .ok_or_else(|| CoordError::UnclaimedDevice(signed.presence.device.short()))?;
        if self.is_suspended(claim.claim.owner)? {
            return Err(CoordError::Suspended);
        }

        let key = signed.presence.device.to_bytes();
        let mut txn = self.db.begin_write()?;

        // An announcement is a heartbeat, and this is the one write here that
        // does not need to survive a power cut.
        //
        // Presence is re-sent every round: losing the last few seconds of it
        // costs a member being unfindable until its next announcement, which is
        // the same state it is in between announcements anyway. Availability is
        // a rolling score recomputed from the same stream, so a lost update is
        // a rounding error in a number that is already an estimate. Everything
        // that cannot be reconstructed -- registrations, enrolments, escrow,
        // invitations -- keeps the default durability, and any one of those
        // commits flushes these along with it.
        //
        // What this buys is not a micro-optimisation. Every announce was an
        // fsync, so a coordinator's presence throughput was one round trip to
        // the disk per heartbeat per member. Measured through the tests that
        // model a month of them: two of them took over sixty seconds on a CI
        // Windows runner and under three on a laptop, which is the same code
        // meeting a slower disk.
        txn.set_durability(redb::Durability::None);

        {
            let mut presence = txn.open_table(PRESENCE)?;
            let mut availability = txn.open_table(AVAILABILITY)?;

            presence.insert(key.as_slice(), postcard::to_stdvec(signed)?.as_slice())?;

            let mut record: AvailabilityRecord = match availability.get(key.as_slice())? {
                Some(value) => postcard::from_bytes(value.value())?,
                // A device seen for the first time starts at the floor rather
                // than at zero or at full marks. Zero would put a brand-new
                // machine in default; full marks would let a node inflate its
                // credit by announcing once and vanishing.
                None => AvailabilityRecord {
                    per_mille: crate::accounting::AVAILABILITY_FLOOR_PER_MILLE,
                    last_tick_unix: now,
                    last_seen_unix: now,
                },
            };
            record.last_seen_unix = now;

            availability.insert(key.as_slice(), postcard::to_stdvec(&record)?.as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Record that a device said it is leaving.
    ///
    /// Refused for a device with no live claim, as for a presence: a notice
    /// from a machine nobody answers for is not history worth keeping. Only the
    /// latest notice is kept; a device that leaves, comes back and leaves again
    /// has one departure on record, the last.
    ///
    /// Durable, unlike a presence: a departure is the one event in a device's
    /// life that is not repeated every round, so losing it loses it.
    pub fn depart(&self, signed: &SignedDeparture, now: u64) -> Result<()> {
        signed.verify(now)?;
        self.claim_for(signed.departure.device)?
            .filter(|claim| !claim.claim.revoked)
            .ok_or_else(|| CoordError::UnclaimedDevice(signed.departure.device.short()))?;

        let record = DepartureRecord {
            notice: signed.clone(),
            received_unix: now,
        };
        let txn = self.db.begin_write()?;
        {
            let mut departures = txn.open_table(DEPARTURES)?;
            departures.insert(
                signed.departure.device.to_bytes().as_slice(),
                postcard::to_stdvec(&record)?.as_slice(),
            )?;
        }
        txn.commit()?;
        Ok(())
    }

    /// When **this coordinator** received a device's last departure notice.
    ///
    /// `None` for a device that never said it was leaving -- which is every
    /// device that crashed, and the difference this table exists to keep.
    pub fn departed(&self, device: DeviceId) -> Result<Option<u64>> {
        let txn = self.db.begin_read()?;
        let departures = txn.open_table(DEPARTURES)?;
        match departures.get(device.to_bytes().as_slice())? {
            Some(value) => {
                let record: DepartureRecord = postcard::from_bytes(value.value())?;
                Ok(Some(record.received_unix))
            }
            None => Ok(None),
        }
    }

    /// When **this coordinator** last heard from a device, by its own clock.
    ///
    /// Distinct from the `at_unix` inside a presence, which is the announcing
    /// device's opinion of the time and therefore not evidence: a Raspberry Pi
    /// with no real-time clock says 1970, and anybody who wants to sort first
    /// can say whatever they like. Ordering and expiry use this instead.
    ///
    /// `None` for a device never heard from, and for one silent past what
    /// [`Self::expire_presence`] was asked to keep.
    pub fn last_seen(&self, device: DeviceId) -> Result<Option<u64>> {
        let txn = self.db.begin_read()?;
        let availability = txn.open_table(AVAILABILITY)?;
        match availability.get(device.to_bytes().as_slice())? {
            Some(value) => {
                let record: AvailabilityRecord = postcard::from_bytes(value.value())?;
                Ok(Some(record.last_seen_unix))
            }
            None => Ok(None),
        }
    }

    /// Where a device was last seen.
    ///
    /// A row this build cannot read is a device not seen: presences written
    /// before the format changed (#435 added the chain's length) stay until
    /// their device announces again or [`Self::expire_presence`] erases them,
    /// and one such row from a silent device failed every listing of its
    /// account (the fleet, 2026-11-16 to 11-22: "Hit the end of buffer").
    pub fn presence_of(&self, device: DeviceId) -> Result<Option<SignedPresence>> {
        let txn = self.db.begin_read()?;
        let presence = txn.open_table(PRESENCE)?;
        Ok(presence
            .get(device.to_bytes().as_slice())?
            .and_then(|value| postcard::from_bytes(value.value()).ok()))
    }

    /// Up to `scan` addresses, in device order from just after `after`, going
    /// round. Device ids are random, so this is an even sample, the same for
    /// the same `after`, and bounded however many machines there are.
    ///
    /// # Errors
    ///
    /// If the database cannot be read.
    pub fn presences_after(&self, after: DeviceId, scan: usize) -> Result<Vec<SignedPresence>> {
        let txn = self.db.begin_read()?;
        let presence = txn.open_table(PRESENCE)?;
        let from = after.to_bytes();
        let mut out = Vec::new();
        let later = presence.range(from.as_slice()..)?;
        let earlier = presence.range(..from.as_slice())?;
        for row in later.chain(earlier).take(scan) {
            let (_, value) = row?;
            // Skipped, as in `presence_of`: one unreadable row is no reason
            // to answer nobody.
            if let Ok(signed) = postcard::from_bytes(value.value()) {
                out.push(signed);
            }
        }
        Ok(out)
    }
}
