//! What was heard on the local network, kept bounded.
//!
//! # The receiver's clock decides, never the sender's
//!
//! An announcement carries the sender's clock, and this table ignores it for
//! every decision it makes. The freshest evidence about where a device lives is
//! *that a signed announcement from it arrived here, just now, from that
//! address*. The number inside the packet adds nothing to that, and trusting it
//! would introduce two failure modes for no gain:
//!
//! - A Raspberry Pi 4 has no real-time clock. After a reboot it announces
//!   itself believing it is 1970. Under supersession-by-sender-clock its new
//!   address would be rejected in favour of a stale one, and the machine would
//!   be unreachable until NTP ran — precisely when it has just come back.
//! - An attacker sets the field to whatever they like. Making a security
//!   decision on an attacker-controlled integer is the kind of thing this
//!   project has a rule against.
//!
//! The cost of ignoring it is that a replayed announcement briefly points at
//! the replayer. That costs one dial, which TLS device pinning refuses, and the
//! next honest announcement — at most one interval away — corrects the entry.
//! Self-healing beats clever.
//!
//! # Why there is a capacity at all
//!
//! A device id is a freely generated keypair, so an attacker on the same
//! network can mint unlimited *valid* announcements. Without a bound that is an
//! unbounded allocation driven by a stranger, on a machine that may be a
//! Raspberry Pi.
//!
//! With a bound, the remaining attack is eviction: flood until the honest
//! entries are pushed out. That is what [`Neighbours::protect`] answers —
//! devices already known to be real, from configuration or from a previous
//! successful authentication, are never evicted to make room for a stranger. A
//! flood can therefore deny *discovery of new peers*, which is annoying, and
//! cannot deny *reaching the peers already known*, which would be an outage.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::net::{IpAddr, SocketAddr};

use itsanas_crypto::{DeviceId, UserId};

use crate::beacon::Announcement;

/// How many devices a node remembers hearing from.
///
/// Sized for a household, not a campus: a hundred and twenty-eight entries is
/// far more than any real local network will hold and still trivial memory on a
/// Pi. Raising it does not make discovery better, it makes a flood cheaper.
pub const DEFAULT_CAPACITY: usize = 128;

/// A device heard from on the local network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Neighbour {
    /// The user this device claims to belong to. Unverified — a sorting hint.
    pub owner: UserId,
    /// The device, proved by the announcement's signature.
    pub device: DeviceId,
    /// Where to reach it: the UDP source address, with the announced TCP port.
    pub address: SocketAddr,
    /// The sender's clock when it signed. Diagnostics only; nothing reads it.
    pub sent_unix: u64,
    /// Our own clock when the announcement arrived. This is what ages entries.
    pub heard_unix: u64,
}

/// What recording an announcement changed.
///
/// Returned so a caller can log a new machine appearing without logging the
/// same machine repeating itself every interval. A daemon that printed every
/// beacon would produce a log nobody reads, which is the same as no log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heard {
    /// A device not previously known.
    New,
    /// A known device, now at a different address.
    Moved {
        /// Where it used to be.
        from: SocketAddr,
    },
    /// A known device, same address. The ordinary case.
    Refreshed,
    /// The table was full of protected entries and this one was dropped.
    Ignored,
}

/// Somewhere worth trying, and who to expect there.
///
/// `mine` carries whether the announcement claimed this node's own owner. It is
/// unverified — anybody may claim it — so it decides ordering and how loudly a
/// failure is reported, and never what the peer is allowed to obtain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The device to pin when dialling. A different answer is refused.
    pub device: DeviceId,
    /// Where it was heard from, with the port it announced.
    pub address: SocketAddr,
    /// Whether it claims to belong to the same user as this node.
    pub mine: bool,
}

/// The bounded set of devices heard from locally.
#[derive(Debug)]
pub struct Neighbours {
    capacity: usize,
    entries: BTreeMap<DeviceId, Neighbour>,
    protected: BTreeSet<DeviceId>,
}

impl Neighbours {
    /// A table holding at most `capacity` devices.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            entries: BTreeMap::new(),
            protected: BTreeSet::new(),
        }
    }

    /// Mark a device as known-real, so a flood of strangers cannot evict it.
    ///
    /// Call this for every peer in the configuration and for every device that
    /// has completed an authenticated connection. Protection is about eviction
    /// only: it grants no trust, and a protected device still has to prove
    /// itself at the TLS layer like anyone else.
    pub fn protect(&mut self, device: DeviceId) {
        self.protected.insert(device);
    }

    /// Whether a device is protected from eviction.
    #[must_use]
    pub fn is_protected(&self, device: &DeviceId) -> bool {
        self.protected.contains(device)
    }

    /// Record a verified announcement heard from `source` at local time `now`.
    ///
    /// Only the port comes from the announcement; the address comes from the
    /// datagram, so a node can never advertise a machine other than itself.
    pub fn record(&mut self, announcement: &Announcement, source: IpAddr, now: u64) -> Heard {
        let address = SocketAddr::new(source, announcement.port);
        let fresh = Neighbour {
            owner: announcement.owner,
            device: announcement.device,
            address,
            sent_unix: announcement.sent_unix,
            heard_unix: now,
        };

        if let Some(existing) = self.entries.get_mut(&announcement.device) {
            let was = existing.address;
            *existing = fresh;
            return if was == address {
                Heard::Refreshed
            } else {
                Heard::Moved { from: was }
            };
        }

        if self.entries.len() >= self.capacity && !self.evict_one() {
            return Heard::Ignored;
        }

        self.entries.insert(announcement.device, fresh);
        Heard::New
    }

    /// Drop the least recently heard unprotected entry. False if none exists.
    fn evict_one(&mut self) -> bool {
        let victim = self
            .entries
            .values()
            .filter(|n| !self.protected.contains(&n.device))
            .min_by_key(|n| (n.heard_unix, n.device))
            .map(|n| n.device);

        match victim {
            Some(device) => {
                self.entries.remove(&device);
                true
            }
            None => false,
        }
    }

    /// Forget entries not heard from since `cutoff`, except protected ones.
    ///
    /// Protected devices survive going quiet on purpose: a laptop that is
    /// switched off has not stopped being your laptop, and forgetting its last
    /// known address means a slower reconnection when it wakes.
    pub fn expire(&mut self, cutoff: u64) -> usize {
        let doomed: Vec<DeviceId> = self
            .entries
            .values()
            .filter(|n| n.heard_unix < cutoff && !self.protected.contains(&n.device))
            .map(|n| n.device)
            .collect();
        for device in &doomed {
            self.entries.remove(device);
        }
        doomed.len()
    }

    /// Every device heard from, ordered by device id so two nodes agree.
    pub fn all(&self) -> impl Iterator<Item = &Neighbour> {
        self.entries.values()
    }

    /// Look up one device.
    #[must_use]
    pub fn get(&self, device: &DeviceId) -> Option<&Neighbour> {
        self.entries.get(device)
    }

    /// Candidates to dial, own machines first, each with the device to expect.
    ///
    /// The ordering is the whole product decision: a node's own devices hold
    /// its own data, so reaching them is what makes a folder appear. Strangers
    /// follow, because they are candidates for hosting rather than for
    /// syncing. Within each group the order is by device id, so it is stable
    /// and two machines produce the same list.
    ///
    /// `owner` is matched against the announcement's unverified owner field, so
    /// a stranger can put themselves in the first group. That costs them a dial
    /// they would have received anyway from the second group, and gains them
    /// nothing: the caller pins the device id, and the peer protocol serves
    /// strangers only sealed and signed objects.
    #[must_use]
    pub fn dial_order(&self, owner: UserId) -> Vec<Candidate> {
        let (mine, theirs): (Vec<&Neighbour>, Vec<&Neighbour>) =
            self.entries.values().partition(|n| n.owner == owner);
        mine.into_iter()
            .map(|n| Candidate {
                device: n.device,
                address: n.address,
                mine: true,
            })
            .chain(theirs.into_iter().map(|n| Candidate {
                device: n.device,
                address: n.address,
                mine: false,
            }))
            .collect()
    }

    /// How many devices are remembered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing has been heard.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The most devices this table will hold.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Default for Neighbours {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use itsanas_crypto::{DeviceKeys, ID_LEN};

    use super::*;
    use crate::beacon::Announcement;

    fn owner_a() -> UserId {
        UserId::from_bytes([1u8; ID_LEN])
    }

    fn owner_b() -> UserId {
        UserId::from_bytes([2u8; ID_LEN])
    }

    fn announce(keys: &DeviceKeys, owner: UserId, port: u16, sent: u64) -> Announcement {
        Announcement::parse(&Announcement::seal(keys, owner, port, sent)).unwrap()
    }

    fn index_as_byte(index: usize) -> u8 {
        u8::try_from(index % 256).unwrap_or(0)
    }

    fn ip(last: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, last))
    }

    #[test]
    fn a_new_device_is_recorded_with_the_address_it_was_heard_from() {
        let mut table = Neighbours::new(8);
        let k = DeviceKeys::generate().unwrap();

        assert_eq!(
            table.record(&announce(&k, owner_a(), 9797, 500), ip(20), 1000),
            Heard::New
        );

        let found = table.get(&k.device_id()).unwrap();
        assert_eq!(found.address, SocketAddr::new(ip(20), 9797));
        assert_eq!(found.heard_unix, 1000);
    }

    #[test]
    fn repeating_the_same_announcement_is_not_reported_as_news() {
        // A beacon arrives every interval, forever. If each one counted as a
        // discovery the daemon's log would be unreadable within an hour, and
        // an unreadable log is the same as no log when something breaks.
        let mut table = Neighbours::new(8);
        let k = DeviceKeys::generate().unwrap();

        table.record(&announce(&k, owner_a(), 9797, 500), ip(20), 1000);
        assert_eq!(
            table.record(&announce(&k, owner_a(), 9797, 560), ip(20), 1030),
            Heard::Refreshed
        );
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn a_device_that_changed_network_is_followed_and_reported() {
        let mut table = Neighbours::new(8);
        let k = DeviceKeys::generate().unwrap();

        table.record(&announce(&k, owner_a(), 9797, 500), ip(20), 1000);
        let moved = table.record(&announce(&k, owner_a(), 9797, 600), ip(31), 1100);

        assert_eq!(
            moved,
            Heard::Moved {
                from: SocketAddr::new(ip(20), 9797)
            }
        );
        assert_eq!(
            table.get(&k.device_id()).unwrap().address,
            SocketAddr::new(ip(31), 9797)
        );
    }

    #[test]
    fn a_rebooted_pi_with_a_reset_clock_is_still_followed_to_its_new_address() {
        // The Raspberry Pi 4 has no real-time clock: after a power cut it
        // announces itself believing it is 1970. If the table superseded by
        // the sender's clock, the Pi would keep its old address forever and be
        // unreachable until NTP ran — which is exactly the moment somebody is
        // waiting for it to come back.
        let mut table = Neighbours::new(8);
        let k = DeviceKeys::generate().unwrap();

        table.record(&announce(&k, owner_a(), 9797, 1_700_000_000), ip(20), 1000);
        table.record(&announce(&k, owner_a(), 9797, 0), ip(45), 2000);

        assert_eq!(
            table.get(&k.device_id()).unwrap().address,
            SocketAddr::new(ip(45), 9797),
            "the reboot was ignored because the sender's clock went backwards"
        );
    }

    #[test]
    fn the_table_never_grows_past_its_capacity() {
        // A device id is a free keypair, so an attacker on the network can
        // mint valid announcements without limit. Unbounded here means an
        // out-of-memory kill on the Pi, triggered by anyone on the LAN.
        let mut table = Neighbours::new(4);
        for index in 0..64u8 {
            let k = DeviceKeys::generate().unwrap();
            table.record(
                &announce(&k, owner_b(), 9797, 0),
                ip(index),
                1000 + u64::from(index),
            );
        }
        assert_eq!(table.len(), 4);
    }

    #[test]
    fn a_flood_of_strangers_cannot_evict_a_known_peer() {
        // The eviction attack. Without protection, an attacker pushes the
        // Raspberry Pi out of every other machine's table and the household
        // stops syncing while every node believes discovery is working.
        let mut table = Neighbours::new(4);
        let pi = DeviceKeys::generate().unwrap();

        table.record(&announce(&pi, owner_a(), 9797, 0), ip(10), 1);
        table.protect(pi.device_id());

        for index in 0..200u8 {
            let attacker = DeviceKeys::generate().unwrap();
            table.record(
                &announce(&attacker, owner_a(), 9797, 0),
                ip(index),
                1000 + u64::from(index),
            );
        }

        assert_eq!(
            table.get(&pi.device_id()).unwrap().address,
            SocketAddr::new(ip(10), 9797),
            "the known peer was evicted by strangers"
        );
        assert_eq!(table.len(), 4);
    }

    #[test]
    fn a_table_full_of_protected_devices_refuses_a_stranger_rather_than_forgetting_one() {
        let mut table = Neighbours::new(2);
        for index in 0..2u8 {
            let k = DeviceKeys::generate().unwrap();
            table.record(&announce(&k, owner_a(), 9797, 0), ip(index), 1);
            table.protect(k.device_id());
        }

        let stranger = DeviceKeys::generate().unwrap();
        assert_eq!(
            table.record(&announce(&stranger, owner_b(), 9797, 0), ip(99), 2),
            Heard::Ignored
        );
        assert_eq!(table.len(), 2);
    }

    #[test]
    fn the_oldest_unprotected_entry_is_the_one_evicted() {
        let mut table = Neighbours::new(2);
        let old = DeviceKeys::generate().unwrap();
        let recent = DeviceKeys::generate().unwrap();

        table.record(&announce(&old, owner_b(), 9797, 0), ip(1), 100);
        table.record(&announce(&recent, owner_b(), 9797, 0), ip(2), 900);

        let newcomer = DeviceKeys::generate().unwrap();
        table.record(&announce(&newcomer, owner_b(), 9797, 0), ip(3), 1000);

        assert!(table.get(&old.device_id()).is_none());
        assert!(table.get(&recent.device_id()).is_some());
        assert!(table.get(&newcomer.device_id()).is_some());
    }

    #[test]
    fn expiry_forgets_the_quiet_but_keeps_your_own_switched_off_machines() {
        // A laptop that is off has not stopped being your laptop. Forgetting
        // its address means a slower reconnection every single time it wakes.
        let mut table = Neighbours::new(8);
        let mine = DeviceKeys::generate().unwrap();
        let stranger = DeviceKeys::generate().unwrap();

        table.record(&announce(&mine, owner_a(), 9797, 0), ip(10), 100);
        table.protect(mine.device_id());
        table.record(&announce(&stranger, owner_b(), 9797, 0), ip(11), 100);

        assert_eq!(table.expire(1000), 1);
        assert!(table.get(&mine.device_id()).is_some());
        assert!(table.get(&stranger.device_id()).is_none());
    }

    #[test]
    fn own_devices_are_dialled_before_strangers() {
        // Reaching your own machines is what makes a folder appear; a stranger
        // is a candidate for hosting, which can wait. Getting this backwards
        // means a node with many neighbours syncs late for no reason.
        let mut table = Neighbours::new(8);
        let stranger = DeviceKeys::generate().unwrap();
        let mine = DeviceKeys::generate().unwrap();

        table.record(&announce(&stranger, owner_b(), 9797, 0), ip(11), 100);
        table.record(&announce(&mine, owner_a(), 9797, 0), ip(10), 100);

        let order = table.dial_order(owner_a());
        assert_eq!(order.len(), 2);
        assert_eq!(order[0].device, mine.device_id());
        assert!(order[0].mine);
        assert_eq!(order[1].device, stranger.device_id());
        assert!(!order[1].mine);
    }

    #[test]
    fn the_dial_order_is_stable_across_two_nodes_with_the_same_view() {
        // Two machines that heard the same announcements must produce the same
        // list, or they retry each other in lockstep and thrash.
        let devices: Vec<DeviceKeys> = (0..6).map(|_| DeviceKeys::generate().unwrap()).collect();

        let mut forwards = Neighbours::new(8);
        for (index, k) in devices.iter().enumerate() {
            forwards.record(
                &announce(k, owner_a(), 9797, 0),
                ip(index_as_byte(index)),
                100,
            );
        }

        let mut backwards = Neighbours::new(8);
        for (index, k) in devices.iter().enumerate().rev() {
            backwards.record(
                &announce(k, owner_a(), 9797, 0),
                ip(index_as_byte(index)),
                100,
            );
        }

        assert_eq!(
            forwards.dial_order(owner_a()),
            backwards.dial_order(owner_a())
        );
    }

    #[test]
    fn a_capacity_of_zero_is_treated_as_one_rather_than_never_recording() {
        // A misconfiguration should degrade, not silently disable discovery.
        let mut table = Neighbours::new(0);
        let k = DeviceKeys::generate().unwrap();
        assert_eq!(
            table.record(&announce(&k, owner_a(), 9797, 0), ip(1), 1),
            Heard::New
        );
        assert_eq!(table.capacity(), 1);
    }
}
