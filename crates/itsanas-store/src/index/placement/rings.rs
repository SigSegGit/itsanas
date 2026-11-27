//! Uptime rings and networks: what this node saw of each device it knows,
//! hour by hour, and the clock guard that keeps one wild note from moving them.

use itsanas_crypto::DeviceId;
use itsanas_placement::{NETWORK_BYTES, Network, Uptime};
use redb::ReadableTable;

use super::{DEVICE_NETWORK, DEVICE_UPTIME, LATEST_HOUR, hour_of};
use crate::{
    error::Result,
    index::{DEVICE_SEEN, Index, META},
};

/// `META` key: an hour far ahead of [`LATEST_HOUR`] that one note claimed,
/// waiting for a second to agree before any ring is moved to it.
const PENDING_JUMP: &str = "uptime_pending_jump";

/// The largest step forward one note may take on its own, in hours.
///
/// A clock that jumps a year ahead for one reading resets every ring it
/// touches (a gap past the window), and `LATEST_HOUR` then makes release
/// refuse for a year after the clock comes back. A week is more than any
/// real gap between two notes of a running node; a node that was really off
/// longer is confirmed by its very next note.
pub const MAX_UNCONFIRMED_JUMP_HOURS: u64 = 7 * 24;

/// How close a second note must be to a pending jump to confirm it.
const JUMP_AGREES_HOURS: u64 = 24;

/// The node clock's standing in `META`, read once per note.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Clock {
    latest: u64,
    pending: u64,
}

impl Clock {
    fn read(meta: &impl ReadableTable<&'static str, &'static [u8]>) -> Result<Self> {
        Ok(Self {
            latest: Index::read_u64(meta, LATEST_HOUR)?,
            pending: Index::read_u64(meta, PENDING_JUMP)?,
        })
    }

    /// Whether a note at `hour` may move rings, and the clock after it.
    ///
    /// Within [`MAX_UNCONFIRMED_JUMP_HOURS`] of the latest hour (or before
    /// any), yes, and a pending jump is dropped: the glitch did not repeat.
    /// Further ahead, only when a pending jump within [`JUMP_AGREES_HOURS`]
    /// before it agrees; otherwise this note becomes the pending jump and
    /// moves nothing.
    const fn judge(self, hour: u64) -> (bool, Self) {
        let ordinary =
            self.latest == 0 || hour <= self.latest.saturating_add(MAX_UNCONFIRMED_JUMP_HOURS);
        let confirmed =
            self.pending != 0 && hour >= self.pending && hour - self.pending <= JUMP_AGREES_HOURS;
        if ordinary || confirmed {
            (
                true,
                Self {
                    latest: self.latest,
                    pending: 0,
                },
            )
        } else {
            (
                false,
                Self {
                    latest: self.latest,
                    pending: hour,
                },
            )
        }
    }
}

/// What one observation would write: the ring (if it changes), and the
/// clock (if it changes).
struct Plan {
    ring: Option<Uptime>,
    clock: Option<Clock>,
}

impl Plan {
    const fn writes(&self) -> bool {
        self.ring.is_some() || self.clock.is_some()
    }
}

/// Plan one observation of a device whose ring is `existing`, in `hour`.
fn plan(existing: Option<Uptime>, clock: Clock, hour: u64, seen: bool) -> Plan {
    let (accepted, mut after) = clock.judge(hour);
    let ring = if accepted {
        let fresh = existing.is_none();
        let mut ring = existing.unwrap_or_else(|| Uptime::new(hour));
        (ring.note(hour, seen) || fresh).then_some(ring)
    } else {
        None
    };
    // The latest hour advances only from a note that was accepted and
    // written: a rejected far-future note must not drag it along.
    if ring.is_some() && hour > after.latest {
        after.latest = hour;
    }
    Plan {
        ring,
        clock: (after != clock).then_some(after),
    }
}

fn read_ring(
    rings: &impl ReadableTable<&'static [u8], &'static [u8]>,
    device: &DeviceId,
) -> Result<Option<Uptime>> {
    match rings.get(device.as_bytes().as_slice())? {
        Some(value) => Ok(Some(postcard::from_bytes(value.value())?)),
        None => Ok(None),
    }
}

/// Note one observation of `device` in `now`'s hour, writing only on change.
///
/// Returns whether the ring was written. The caller decides whether the device
/// is known; this only spares a write when the hour is already marked, which
/// is every session after the first in an hour.
fn note_ring(
    rings: &mut redb::Table<'_, &'static [u8], &'static [u8]>,
    meta: &mut redb::Table<'_, &'static str, &'static [u8]>,
    device: &DeviceId,
    now: u64,
    seen: bool,
) -> Result<bool> {
    let planned = plan(
        read_ring(rings, device)?,
        Clock::read(meta)?,
        hour_of(now),
        seen,
    );
    apply(rings, meta, device, &planned)
}

fn apply(
    rings: &mut redb::Table<'_, &'static [u8], &'static [u8]>,
    meta: &mut redb::Table<'_, &'static str, &'static [u8]>,
    device: &DeviceId,
    planned: &Plan,
) -> Result<bool> {
    if let Some(clock) = planned.clock {
        meta.insert(LATEST_HOUR, clock.latest.to_le_bytes().as_slice())?;
        meta.insert(PENDING_JUMP, clock.pending.to_le_bytes().as_slice())?;
    }
    let Some(ring) = &planned.ring else {
        return Ok(false);
    };
    rings.insert(
        device.as_bytes().as_slice(),
        postcard::to_stdvec(ring)?.as_slice(),
    )?;
    Ok(true)
}

/// Whether this node has had to do with `device`: a ring, or a contact.
/// A network row: `None` when absent or written by a version whose tag this
/// one does not know.
fn read_network(
    table: &impl ReadableTable<&'static [u8], &'static [u8]>,
    device: &DeviceId,
) -> Result<Option<(Network, u64)>> {
    let Some(value) = table.get(device.as_bytes().as_slice())? else {
        return Ok(None);
    };
    let bytes = value.value();
    if bytes.len() != NETWORK_BYTES + 8 {
        return Ok(None);
    }
    let mut at = [0u8; 8];
    at.copy_from_slice(&bytes[NETWORK_BYTES..]);
    Ok(Network::from_bytes(&bytes[..NETWORK_BYTES])
        .map(|network| (network, u64::from_le_bytes(at))))
}

fn known(
    rings: &impl ReadableTable<&'static [u8], &'static [u8]>,
    seen: &impl ReadableTable<&'static [u8], u64>,
    device: &DeviceId,
) -> Result<bool> {
    let key = device.as_bytes().as_slice();
    Ok(rings.get(key)?.is_some() || seen.get(key)?.is_some())
}

impl Index {
    /// Record that `device` answered, in its ring and in `DEVICE_SEEN`, in one
    /// transaction.
    ///
    /// For sessions this node dialled or a peer it already knows: the caller
    /// vouches that `device` is somebody. Inbound sessions from anyone go
    /// through [`Index::note_seen_if_known`].
    pub(in crate::index) fn note_seen_in(&self, device: &DeviceId, now: u64) -> Result<()> {
        // The hour already marked and the contact stamp fresh: nothing to
        // write, so no commit (`SEEN_STEP`). Every session after the first
        // in an hour is this one.
        if self.seen_within_step(device, now)? {
            let txn = self.db.begin_read()?;
            let rings = txn.open_table(DEVICE_UPTIME)?;
            let meta = txn.open_table(META)?;
            if !plan(
                read_ring(&rings, device)?,
                Clock::read(&meta)?,
                hour_of(now),
                true,
            )
            .writes()
            {
                return Ok(());
            }
        }
        let txn = self.db.begin_write()?;
        {
            txn.open_table(DEVICE_SEEN)?
                .insert(device.as_bytes().as_slice(), now)?;
            note_ring(
                &mut txn.open_table(DEVICE_UPTIME)?,
                &mut txn.open_table(META)?,
                device,
                now,
                true,
            )?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Record that this node tried `device` in `now`'s hour and did not reach
    /// it. Nothing for a device it has never had to do with.
    ///
    /// # Errors
    ///
    /// If the index cannot be read or written.
    pub fn note_unreached(&self, device: &DeviceId, now: u64) -> Result<bool> {
        self.note_if_known(device, now, false)
    }

    /// [`Index::note_seen`] for an inbound session: only for a device already
    /// known, so a stranger dialling in leaves no row. Returns whether it was.
    ///
    /// Only the ring, never `DEVICE_SEEN`: contact there is what every
    /// liveness question reads, `under_replicated` in `everyone` among them,
    /// and an inbound session (which proves nothing about what the peer
    /// holds) must not change how `everyone` counts copies (red team, 5b).
    /// Read first, and a write transaction only for a known device whose
    /// ring or clock this note changes: an inbound session per connection
    /// would otherwise be a commit per connection.
    ///
    /// # Errors
    ///
    /// If the index cannot be read or written.
    pub fn note_seen_if_known(&self, device: &DeviceId, now: u64) -> Result<bool> {
        self.note_if_known(device, now, true)
    }

    fn note_if_known(&self, device: &DeviceId, now: u64, seen: bool) -> Result<bool> {
        {
            let txn = self.db.begin_read()?;
            let rings = txn.open_table(DEVICE_UPTIME)?;
            if !known(&rings, &txn.open_table(DEVICE_SEEN)?, device)? {
                return Ok(false);
            }
            let planned = plan(
                read_ring(&rings, device)?,
                Clock::read(&txn.open_table(META)?)?,
                hour_of(now),
                seen,
            );
            if !planned.writes() {
                return Ok(true);
            }
        }
        // Planned again inside the write: another session may have written
        // between the read and here, and the read's plan would undo it.
        let txn = self.db.begin_write()?;
        note_ring(
            &mut txn.open_table(DEVICE_UPTIME)?,
            &mut txn.open_table(META)?,
            device,
            now,
            seen,
        )?;
        txn.commit()?;
        Ok(true)
    }

    /// Record that `device` was seen from `network` at `now`. Only for a
    /// device already known, for the same reason as its ring, and only a
    /// write when the network changed or the stamp is an hour old. Returns
    /// whether the device was known.
    ///
    /// # Errors
    ///
    /// If the index cannot be read or written.
    pub fn note_network(&self, device: &DeviceId, network: Network, now: u64) -> Result<bool> {
        {
            let txn = self.db.begin_read()?;
            if !known(
                &txn.open_table(DEVICE_UPTIME)?,
                &txn.open_table(DEVICE_SEEN)?,
                device,
            )? {
                return Ok(false);
            }
            if let Some((held, at)) = read_network(&txn.open_table(DEVICE_NETWORK)?, device)?
                && held == network
                && hour_of(at) == hour_of(now)
            {
                return Ok(true);
            }
        }
        let mut row = [0u8; NETWORK_BYTES + 8];
        row[..NETWORK_BYTES].copy_from_slice(&network.to_bytes());
        row[NETWORK_BYTES..].copy_from_slice(&now.to_le_bytes());
        let txn = self.db.begin_write()?;
        txn.open_table(DEVICE_NETWORK)?
            .insert(device.as_bytes().as_slice(), row.as_slice())?;
        txn.commit()?;
        Ok(true)
    }

    /// The network `device` was last seen from, if at or after `since`.
    ///
    /// A stale row groups nothing: a box's address is handed to another
    /// customer after a reconnection, and a laptop that left the office a
    /// month ago is not behind its router any more.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn network_since(&self, device: &DeviceId, since: u64) -> Result<Option<Network>> {
        let txn = self.db.begin_read()?;
        Ok(read_network(&txn.open_table(DEVICE_NETWORK)?, device)?
            .and_then(|(network, at)| (at >= since).then_some(network)))
    }

    /// `device`'s ring, or `None` if this node has never observed it.
    ///
    /// # Errors
    ///
    /// If the index cannot be read, or the row does not decode.
    pub fn uptime(&self, device: &DeviceId) -> Result<Option<Uptime>> {
        let txn = self.db.begin_read()?;
        let rings = txn.open_table(DEVICE_UPTIME)?;
        match rings.get(device.as_bytes().as_slice())? {
            Some(value) => Ok(Some(postcard::from_bytes(value.value())?)),
            None => Ok(None),
        }
    }

    /// Every ring, sorted by device. One row per device this node knows, which
    /// is what the roster is built from anyway.
    ///
    /// # Errors
    ///
    /// If the index cannot be read, or a row does not decode.
    pub fn uptimes(&self) -> Result<Vec<(DeviceId, Uptime)>> {
        let txn = self.db.begin_read()?;
        let rings = txn.open_table(DEVICE_UPTIME)?;
        let mut out = Vec::new();
        for row in rings.iter()? {
            let (key, value) = row?;
            out.push((
                DeviceId::from_slice(key.value())?,
                postcard::from_bytes(value.value())?,
            ));
        }
        Ok(out)
    }

    /// The highest hour any ring was written at; 0 before the first.
    ///
    /// A clock that now reads earlier than this has stepped back, and a
    /// release decided on it would read every ring as from the future.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn latest_hour_written(&self) -> Result<u64> {
        let txn = self.db.begin_read()?;
        Self::read_u64(&txn.open_table(META)?, LATEST_HOUR)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::*;
    use super::*;

    use crate::holders;

    #[test]
    fn note_seen_writes_the_ring_and_the_contact_together() {
        let (_dir, index) = index();
        index.note_seen(&device(7), T0).expect("seen");
        let ring = index.uptime(&device(7)).expect("read").expect("a ring");
        assert_eq!(ring.seen_hours(), 1);
        assert_eq!(ring.last_seen_hour(), Some(T0 / HOUR));
        assert_eq!(index.last_seen(&device(7)).expect("read"), Some(T0));
        assert_eq!(index.latest_hour_written().expect("read"), T0 / HOUR);
    }

    #[test]
    fn a_ring_is_rewritten_only_when_the_observation_changes_it() {
        let (_dir, index) = index();
        let txn = index.db.begin_write().expect("txn");
        {
            let mut rings = txn.open_table(DEVICE_UPTIME).expect("rings");
            let mut meta = txn.open_table(META).expect("meta");
            let d = device(7);
            assert!(note_ring(&mut rings, &mut meta, &d, T0, true).expect("first"));
            assert!(
                !note_ring(&mut rings, &mut meta, &d, T0 + 60, true).expect("same hour"),
                "a second session in the same hour must not write"
            );
            assert!(note_ring(&mut rings, &mut meta, &d, T0 + HOUR, true).expect("next hour"));
            assert!(
                !note_ring(&mut rings, &mut meta, &d, T0, true).expect("step back"),
                "a clock stepping back changes nothing and writes nothing"
            );
        }
        txn.commit().expect("commit");
    }

    #[test]
    fn the_latest_hour_written_never_moves_back_with_the_clock() {
        let (_dir, index) = index();
        index.note_seen(&device(7), T0 + 5 * HOUR).expect("seen");
        index.note_seen(&device(8), T0).expect("seen");
        assert_eq!(index.latest_hour_written().expect("read"), T0 / HOUR + 5);
    }

    fn network(text: &str) -> Network {
        Network::of(text.parse().expect("address")).expect("a network")
    }

    #[test]
    fn red_team_a_stranger_leaves_no_network_and_a_stale_one_groups_nothing() {
        let (_dir, index) = index();
        let stranger = device(9);
        assert!(
            !index
                .note_network(&stranger, network("203.0.113.7"), T0)
                .expect("note")
        );
        assert_eq!(index.network_since(&stranger, 0).expect("read"), None);

        let host = device(1);
        index.note_seen(&host, T0).expect("seen");
        assert!(
            index
                .note_network(&host, network("203.0.113.7"), T0)
                .expect("note")
        );
        assert_eq!(
            index.network_since(&host, T0).expect("read"),
            Some(network("203.0.113.9"))
        );
        // Older than the window the caller asks for: no longer evidence.
        assert_eq!(index.network_since(&host, T0 + 1).expect("read"), None);
        // A move is recorded, and the old network is gone with it.
        index
            .note_network(&host, network("198.51.100.7"), T0 + HOUR)
            .expect("note");
        assert_eq!(
            index.network_since(&host, T0).expect("read"),
            Some(network("198.51.100.1"))
        );
        // A device that leaves takes its row with it.
        index.forget_device(&host).expect("forget");
        assert_eq!(index.network_since(&host, 0).expect("read"), None);
    }

    #[test]
    fn red_team_a_stranger_never_gets_a_ring() {
        let (_dir, index) = index();
        let stranger = device(9);
        for hour in 0..50 {
            assert!(
                !index
                    .note_unreached(&stranger, T0 + hour * HOUR)
                    .expect("unreached")
            );
            assert!(
                !index
                    .note_seen_if_known(&stranger, T0 + hour * HOUR)
                    .expect("inbound")
            );
        }
        assert_eq!(index.uptime(&stranger).expect("read"), None);
        assert_eq!(index.last_seen(&stranger).expect("read"), None);
        assert_eq!(
            index.uptimes().expect("read"),
            [] as [(itsanas_crypto::DeviceId, itsanas_placement::Uptime); 0]
        );
        assert_eq!(index.latest_hour_written().expect("read"), 0);
    }

    #[test]
    fn a_known_device_is_noted_reached_or_not() {
        let (_dir, index) = index();
        let known = device(3);
        index.note_seen(&known, T0).expect("seen");
        assert!(index.note_unreached(&known, T0 + HOUR).expect("unreached"));
        assert!(
            index
                .note_seen_if_known(&known, T0 + 2 * HOUR)
                .expect("inbound")
        );
        let ring = index.uptime(&known).expect("read").expect("ring");
        assert_eq!(ring.attempted_hours(), 3);
        assert_eq!(ring.seen_hours(), 2);
        // The inbound session moved the ring only: contact is what
        // `everyone` counts copies by, and an inbound session proves nothing.
        assert_eq!(index.last_seen(&known).expect("read"), Some(T0));

        // Known by contact alone (a holder record), with no ring yet.
        index
            .record_holder(&chunk(1), &device(4), T0)
            .expect("record");
        assert!(
            index
                .note_unreached(&device(4), T0 + HOUR)
                .expect("unreached")
        );
        assert_eq!(
            index.uptimes().expect("all").len(),
            2,
            "one ring per known device"
        );
    }

    #[test]
    fn red_team_a_lone_far_future_note_moves_no_ring_and_no_latest_hour() {
        // One reading a year ahead would reset every ring it touched and set
        // LATEST_HOUR a year on, so release refused for a year after the
        // clock came back. Sabotage: accept every forward jump in `judge`.
        let (_dir, index) = index();
        let d = device(7);
        index.note_seen(&d, T0).expect("seen");
        let ring = index.uptime(&d).expect("read").expect("ring");
        let glitch = T0 + 365 * 24 * HOUR;
        index.note_seen(&d, glitch).expect("glitch");
        assert_eq!(index.uptime(&d).expect("read"), Some(ring));
        assert_eq!(index.latest_hour_written().expect("read"), T0 / HOUR);

        // The clock comes back: the next ordinary note works as usual.
        index.note_seen(&d, T0 + HOUR).expect("back");
        assert_eq!(index.latest_hour_written().expect("read"), T0 / HOUR + 1);

        // A node really off for a month: two agreeing notes accept the jump.
        let month = T0 + 30 * 24 * HOUR;
        index.note_seen(&d, month).expect("first");
        assert_eq!(index.latest_hour_written().expect("read"), T0 / HOUR + 1);
        index.note_seen(&d, month + HOUR).expect("second");
        assert_eq!(index.latest_hour_written().expect("read"), month / HOUR + 1);
    }

    #[test]
    fn red_team_an_inbound_session_does_not_change_everyones_count() {
        // `everyone` must behave exactly as before placement. Contact is what
        // `under_replicated` reads for liveness, and an inbound session proves
        // nothing about what the peer holds: refreshed by one, a host gone
        // quiet a week ago would count again and repair would stop. Sabotage:
        // write `DEVICE_SEEN` in `note_if_known`.
        let dir = tempfile::tempdir().expect("temp dir");
        let index = Index::open(dir.path().join("index.redb"), key()).expect("open");
        file(&index, "a", &[chunk(1)]);
        index
            .record_holders(&[chunk(1)], &device(5), T0)
            .expect("record");
        let now = T0 + holders::LIVE_FOR + HOUR;
        let before = index.under_replicated(2, now).expect("count");
        assert!(index.note_seen_if_known(&device(5), now).expect("inbound"));
        assert_eq!(index.under_replicated(2, now).expect("count"), before);
        assert_eq!(index.last_seen(&device(5)).expect("read"), Some(T0));
    }
}
