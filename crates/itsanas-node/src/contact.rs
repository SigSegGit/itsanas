//! When a node contacts its coordinator, and when it has no reason to.
//!
//! Until §8 0o phase 2 every round dialled the coordinator, announced, and
//! asked for the account's devices: 288 connections a day from every node at
//! the default interval, whether or not anything needed them. Three machines
//! on one LAN that had found each other by broadcast still made every one.
//!
//! The two halves of that connection have opposite economics, and the first
//! version of this rule got it wrong by treating them as one ("dial when the
//! round reached nobody": a Pi that found the VM by broadcast would then never
//! publish, and a laptop elsewhere would never find it).
//!
//! * **Publishing** is what makes a machine findable by somebody who is not
//!   here, so it never depends on having met somebody who is. It happens at
//!   every start, whenever the address this machine would publish changes, and
//!   once an hour otherwise, so a presence never expires under a running node.
//! * **Reading** never costs a connection of its own. The account's devices
//!   are asked for on every connection a publication opens anyway, and on no
//!   other. The handover first said to read only when a listed device stopped
//!   answering; that never learns of a machine enrolled after this one
//!   started, because a device nobody has listed cannot go missing. On a
//!   connection that is already open, the read is one indexed lookup.
//!
//! A node that never moves therefore makes one connection an hour instead of
//! twelve.
//!
//! Every instant here is this machine's own [`Instant`](std::time::Instant). A peer's clock never
//! decides anything (HANDOVER §6), and neither does the coordinator's order:
//! the addresses of a device are a set, dialled in the order of *this
//! machine's* record of which one last worked.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use itsanas_crypto::DeviceId;

use crate::coordinator::is_private_address;

/// A publication is repeated this often under a running node that neither
/// moves nor restarts.
///
/// `PRESENCE_TTL` is seven days, so this leaves 168 chances to refresh before
/// an address expires, and `device list` says how long a machine has been
/// silent to within an hour. The account's devices are read on the same
/// connection, so a machine enrolled while this one runs, or one that moved,
/// is known here within the hour.
///
/// An hour of this process's [`Instant`], which on Linux
/// and macOS does not advance while the machine is suspended: a laptop asleep
/// most of the day publishes after an hour *awake*, unless it woke somewhere
/// else, which is a change and publishes at once. Its presence stays valid --
/// the lifetime is seven days -- but `device list` reports it silent for
/// longer than it was.
pub const PUBLISH_EVERY: Duration = Duration::from_secs(3600);

/// How many devices the address book holds.
///
/// The list comes from the coordinator, whose answer is not verified yet --
/// `Response::Peers` drops the device signatures, which phase 2b carries
/// through. Until then a hostile coordinator decides what arrives here, and
/// without a bound it decides how much memory the daemon uses too. An account
/// with more machines than this has a problem no address book solves.
pub const MAX_DEVICES: usize = 256;

/// How many addresses are kept for one device.
///
/// A laptop that moves collects one per network, and a round dials each
/// until one answers, so every dead one costs a connect timeout. Past this, the addresses that
/// never worked are dropped first, then the one that worked longest ago: a
/// replayed list of stale addresses can push out nothing that has answered
/// since.
pub const MAX_ADDRESSES: usize = 4;

/// What to ask the coordinator this round.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Due {
    /// Announce where this machine is.
    pub publish: bool,
    /// Ask where the account's other devices are.
    pub read: bool,
}

impl Due {
    /// Whether the coordinator is dialled at all.
    #[must_use]
    pub fn any(&self) -> bool {
        self.publish || self.read
    }
}

#[derive(Clone, Debug)]
struct Candidate {
    address: String,
    /// When this machine last completed a sync at this address.
    worked: Option<Instant>,
}

/// This node's side of its coordinator: what it last published, when it last
/// read, and where the account's other devices were last known to be.
///
/// Held in memory by the daemon. A restart empties it, and that costs nothing
/// extra: a start publishes anyway, and the read rides on that connection.
#[derive(Debug, Default)]
pub struct Contact {
    /// The address the last successful publication was *for*, as
    /// [`Self::due`] was given it, and when. Compared with what `due` is given
    /// next rather than with what the coordinator was sent: the two are worked
    /// out by different sockets, and a node whose probe and connection
    /// disagreed would otherwise publish every round, for ever.
    published: Option<(Option<String>, Instant)>,
    /// When the account's devices were last read.
    read_at: Option<Instant>,
    book: BTreeMap<DeviceId, Vec<Candidate>>,
}

impl Contact {
    /// A node that has published nothing and knows nobody.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// What this round should ask of the coordinator.
    ///
    /// `address` is where this machine would publish itself now, as far as it
    /// can tell without dialling (`None` if it cannot tell).
    #[must_use]
    pub fn due(&self, address: Option<&str>, now: Instant) -> Due {
        let publish = match &self.published {
            None => true,
            Some((for_address, at)) => {
                let moved = match (address, for_address.as_deref()) {
                    (Some(now_at), Some(then_at)) => now_at != then_at,
                    // Could not tell then, can now, or the reverse: something
                    // changed, and a publication is what finds out what.
                    (Some(_), None) | (None, Some(_)) => true,
                    (None, None) => false,
                };
                moved || now.saturating_duration_since(*at) >= PUBLISH_EVERY
            }
        };
        // A read that failed on its own is retried at once: until one works,
        // this node knows none of its devices' addresses.
        let read = publish || self.read_at.is_none();
        Due { publish, read }
    }

    /// Record a publication the coordinator accepted, for `address` as it was
    /// given to [`Self::due`].
    pub fn published(&mut self, address: Option<&str>, now: Instant) {
        self.published = Some((address.map(str::to_owned), now));
    }

    /// Take in the coordinator's list of the account's other devices.
    ///
    /// A device no longer listed is dropped: it was withdrawn, or has been
    /// silent past the presence's lifetime, and in both cases this node has no
    /// address for it worth dialling. For the others, the listed address joins
    /// the set of candidates. It replaces nothing and takes no rank from being
    /// new: the one that last worked is still dialled first.
    pub fn read(&mut self, found: &[(DeviceId, String)], now: Instant) {
        self.read_at = Some(now);
        let listed: BTreeSet<DeviceId> = found.iter().map(|(device, _)| *device).collect();
        self.book.retain(|device, _| listed.contains(device));

        for (device, address) in found {
            if !self.book.contains_key(device) && self.book.len() >= MAX_DEVICES {
                continue;
            }
            let candidates = self.book.entry(*device).or_default();
            if candidates.iter().any(|known| known.address == *address) {
                continue;
            }
            candidates.push(Candidate {
                address: address.clone(),
                worked: None,
            });
            if candidates.len() > MAX_ADDRESSES {
                sort_best_first(candidates);
                candidates.truncate(MAX_ADDRESSES);
            }
        }
    }

    /// Record that a sync with `device` at `address` completed.
    ///
    /// Only for a device the coordinator listed: a device found on the LAN or
    /// typed into the configuration has its own route and is not this book's
    /// business.
    pub fn worked(&mut self, device: DeviceId, address: &str, now: Instant) {
        if let Some(candidates) = self.book.get_mut(&device)
            && let Some(candidate) = candidates.iter_mut().find(|c| c.address == address)
        {
            candidate.worked = Some(now);
        }
    }

    /// Every address known for the account's other devices, in dialling order:
    /// for each device, the one that last worked first, then the ones that can
    /// work from anywhere before the private ones.
    #[must_use]
    pub fn candidates(&self) -> Vec<(DeviceId, String)> {
        let mut out = Vec::new();
        for (device, candidates) in &self.book {
            let mut ordered = candidates.clone();
            sort_best_first(&mut ordered);
            out.extend(
                ordered
                    .into_iter()
                    .map(|candidate| (*device, candidate.address)),
            );
        }
        out
    }
}

/// Worked most recently first, then never-worked public before never-worked
/// private. Stable, so ties keep the order they arrived in.
fn sort_best_first(candidates: &mut [Candidate]) {
    candidates.sort_by(|a, b| {
        b.worked
            .cmp(&a.worked)
            .then_with(|| is_private_address(&a.address).cmp(&is_private_address(&b.address)))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROUND: Duration = Duration::from_secs(300);
    const HOME: &str = "192.168.1.10:9797";

    fn device(byte: u8) -> DeviceId {
        DeviceId::from_bytes([byte; 32])
    }

    fn listing(devices: &[u8]) -> Vec<(DeviceId, String)> {
        devices
            .iter()
            .map(|&d| (device(d), format!("192.168.1.{d}:9797")))
            .collect()
    }

    /// The whole justification for this step: a machine that is always on and
    /// never moves goes from 288 connections a day to 24, and no read ever
    /// opens a connection of its own.
    #[test]
    fn a_machine_that_never_moves_dials_the_coordinator_once_an_hour() {
        let mut contact = Contact::new();
        let start = Instant::now();
        let (mut connections, mut publications, mut reads) = (0, 0, 0);
        for round in 0..288u32 {
            let now = start + ROUND * round;
            let due = contact.due(Some(HOME), now);
            connections += u32::from(due.any());
            if due.publish {
                publications += 1;
                contact.published(Some(HOME), now);
            }
            if due.read {
                reads += 1;
                contact.read(&listing(&[2]), now);
            }
        }
        assert_eq!(
            (connections, publications, reads),
            (24, 24, 24),
            "a Pi at home should dial once an hour, reading on the connection its \
             publication opens; anything more is the heartbeat this step removes"
        );
    }

    /// Found by the Rodin audit of 2026-09-21, against the first version of
    /// the rule: "dial the coordinator when the round reached nobody". The Pi
    /// wakes, finds the VM by broadcast, has therefore reached somebody, and
    /// never publishes; a laptop at a friend's house then finds nothing. This
    /// rule is not given who was reached, so it cannot make that mistake.
    #[test]
    fn red_team_a_machine_that_found_everybody_by_broadcast_still_publishes() {
        let mut contact = Contact::new();
        let start = Instant::now();

        assert!(
            contact.due(Some(HOME), start).publish,
            "a machine must publish at start whoever it has met, or nobody \
             elsewhere can ever find it"
        );
        contact.published(Some(HOME), start);
        contact.read(&listing(&[2, 3]), start);

        assert!(
            contact.due(Some(HOME), start + PUBLISH_EVERY).publish,
            "a presence refreshed only at start expires under a running node"
        );
    }

    #[test]
    fn a_machine_that_changes_network_publishes_at_once() {
        let mut contact = Contact::new();
        let start = Instant::now();
        contact.published(Some("192.168.1.30:9797"), start);
        contact.read(&[], start);

        let next = start + ROUND;
        assert!(
            contact.due(Some("10.0.0.7:9797"), next).publish,
            "a laptop that moved must say where it is now, not in an hour"
        );
        assert!(
            !contact.due(Some("192.168.1.30:9797"), next).any(),
            "an unchanged address inside the hour needs no connection"
        );
        assert!(
            contact.due(None, next).publish,
            "losing the route is a change too, and a publication is what finds out"
        );
    }

    /// Why reads ride on every publication rather than waiting for a listed
    /// device to stop answering: a machine enrolled after this one started is
    /// on no list here, so it can never stop answering, and this node would
    /// never dial it.
    #[test]
    fn a_machine_enrolled_after_this_one_started_is_dialled_within_the_hour() {
        let mut contact = Contact::new();
        let start = Instant::now();
        contact.published(Some(HOME), start);
        contact.read(&listing(&[2]), start);

        for round in 1..12u32 {
            assert!(
                !contact.due(Some(HOME), start + ROUND * round).any(),
                "round {round} dialled the coordinator with nothing owed"
            );
        }
        let hour = start + PUBLISH_EVERY;
        assert!(
            contact.due(Some(HOME), hour).read,
            "a machine enrolled after this one started would never be dialled by it"
        );
        contact.read(&listing(&[2, 3]), hour);
        assert!(
            contact.candidates().iter().any(|(d, _)| *d == device(3)),
            "the read did not put the new machine in the book"
        );
    }

    /// The coordinator's answer is not verified until phase 2b, so for now it
    /// decides what arrives in this table. It must not decide its size.
    #[test]
    fn red_team_a_coordinator_cannot_grow_the_address_book_without_bound() {
        let mut contact = Contact::new();
        let now = Instant::now();
        let flood: Vec<(DeviceId, String)> = (0..=u16::MAX)
            .map(|n| {
                let mut id = [0u8; 32];
                id[..2].copy_from_slice(&n.to_be_bytes());
                (
                    DeviceId::from_bytes(id),
                    format!("10.0.{}.{}:9797", n >> 8, n & 0xff),
                )
            })
            .collect();
        contact.read(&flood, now);
        assert_eq!(
            contact.book.len(),
            MAX_DEVICES,
            "one answer filled the table"
        );

        let one = flood[0].0;
        let addresses: Vec<(DeviceId, String)> = (0..1000)
            .map(|n| (one, format!("10.1.{}.{}:9797", n >> 8, n & 0xff)))
            .collect();
        contact.read(&addresses, now);
        assert!(
            contact.book.values().all(|c| c.len() <= MAX_ADDRESSES),
            "one device was given a thousand addresses and kept them"
        );
    }

    /// A relayed or replayed address is an opinion; an address that answered
    /// is evidence. A list of stale addresses for the Pi -- from a coordinator
    /// that lies, or one that is only out of date -- must not push the address
    /// that works down the dialling order, where a round's connect timeouts
    /// would be spent on the stale ones first.
    #[test]
    fn red_team_addresses_that_never_worked_do_not_displace_one_that_did() {
        let mut contact = Contact::new();
        let start = Instant::now();
        let pi = device(2);
        contact.read(&[(pi, "192.168.1.20:9797".to_owned())], start);
        contact.worked(pi, "192.168.1.20:9797", start);

        let stale: Vec<(DeviceId, String)> = (0..20)
            .map(|n| (pi, format!("203.0.113.{n}:9797")))
            .collect();
        contact.read(&stale, start + ROUND);

        let order = contact.candidates();
        assert_eq!(
            order.first().map(|(_, address)| address.as_str()),
            Some("192.168.1.20:9797"),
            "the address that answered was pushed down by addresses that never did"
        );
        assert!(order.len() <= MAX_ADDRESSES, "the replay was kept in full");
    }

    #[test]
    fn a_device_no_longer_listed_is_forgotten() {
        let mut contact = Contact::new();
        let now = Instant::now();
        contact.read(&listing(&[2, 3]), now);
        contact.read(&listing(&[2]), now);
        let devices: BTreeSet<DeviceId> =
            contact.candidates().into_iter().map(|(d, _)| d).collect();
        assert_eq!(
            devices,
            [device(2)].into(),
            "a withdrawn device kept its place and would be dialled every round"
        );
    }
}
