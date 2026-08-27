//! Finding the other machines in the house without being told where they are.
//!
//! This is the half of discovery that needs no server. `itsanas-discover`
//! carries the packet format and the bounded table; this module is the daemon's
//! use of them — a thread that announces this node on a timer, records what it
//! hears, and hands the sync loop a list of addresses to try.
//!
//! # What it changes for the user
//!
//! Before this, two machines only found each other if somebody typed
//! `itsanas peer add 192.168.1.20:9797` on one of them. On a home network they
//! now find each other with nothing configured, which is the difference between
//! a tool that needs a manual and one that behaves like the thing it is being
//! compared against.
//!
//! It does not remove the coordinator, and is not meant to. It removes the
//! coordinator from the case this project was started for — a laptop, a
//! Raspberry Pi and a virtual machine on one network — and leaves it for
//! reaching a machine somewhere else. `docs/DESIGN.md` §8 has the argument.
//!
//! # Why the log is nearly silent
//!
//! An announcement arrives from every neighbour every thirty seconds, forever.
//! Printing them would produce a journal nobody reads, and an unread journal is
//! the same as no journal on the day something breaks. So only genuine news is
//! printed: a machine appearing, and a machine moving to a new address.
//! Verification failures are counted and summarised rather than printed one by
//! one, because the situation that produces them — somebody spraying the
//! network — is exactly the situation where a per-packet log is a denial of
//! service against the operator's ability to see anything else.

use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use itsanas_crypto::{DeviceId, DeviceKeys, UserId};
use itsanas_discover::{ANNOUNCE_INTERVAL, Candidate, EXPIRY, Heard, Lan, Neighbours, now_unix};

/// How long to wait on the socket before checking for shutdown.
///
/// The thread is asleep in `recv_from` nearly all of its life. A Ctrl-C that
/// waited a full announce interval to be noticed would look like a hang.
const POLL: Duration = Duration::from_millis(500);

/// How often the "packets failed to verify" summary is allowed to print.
const COMPLAINT_INTERVAL: Duration = Duration::from_secs(300);

/// The neighbours this node has heard from, shared between threads.
///
/// A plain `Mutex` rather than a channel: the sync loop wants the *current*
/// picture when it happens to look, not a backlog of every announcement since
/// it last checked. A channel would make a slow sync round accumulate messages
/// that are already out of date by the time they are read.
#[derive(Debug)]
pub struct Neighbourhood {
    table: Mutex<Neighbours>,
}

impl Neighbourhood {
    /// An empty neighbourhood with the default capacity.
    #[must_use]
    pub fn new() -> Self {
        Self {
            table: Mutex::new(Neighbours::default()),
        }
    }

    /// Addresses to try, this node's own machines first.
    ///
    /// Each comes with the device to expect, which is passed to
    /// `PeerClient::connect` so that an address answering as somebody else is
    /// refused. Discovery says who might be there; the TLS layer decides who
    /// actually is.
    #[must_use]
    pub fn dial_order(&self, owner: UserId) -> Vec<Candidate> {
        self.table
            .lock()
            .map(|table| table.dial_order(owner))
            .unwrap_or_default()
    }

    /// Mark a device as one that has genuinely answered, so a flood of
    /// strangers cannot push it out of the table.
    ///
    /// Called after a successful authenticated round, which is the only
    /// evidence available on a bare network that a device is real rather than
    /// a freshly minted keypair.
    pub fn confirm(&self, device: DeviceId) {
        if let Ok(mut table) = self.table.lock() {
            table.protect(device);
        }
    }

    /// Whether a device has already earned a place a stranger cannot take.
    #[must_use]
    pub fn is_confirmed(&self, device: &DeviceId) -> bool {
        self.table
            .lock()
            .is_ok_and(|table| table.is_protected(device))
    }

    /// How many devices are currently known.
    #[must_use]
    pub fn len(&self) -> usize {
        self.table.lock().map_or(0, |table| table.len())
    }

    /// Whether nothing has been heard yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for Neighbourhood {
    fn default() -> Self {
        Self::new()
    }
}

/// How many devices this node has never confirmed it will dial in one round.
///
/// A flood of freshly minted identities costs an attacker nothing. Without a
/// cap, one round would open a connection to every one of them — up to the
/// whole table — and spend the interval doing it instead of syncing with the
/// machines that matter. Confirmed peers are dialled regardless of this: the
/// limit is on strangers, not on work.
pub const NEW_PEERS_PER_ROUND: usize = 4;

/// Announce this node and record what is heard, until shutdown.
///
/// Never returns an error. Discovery is an optimisation: a network that drops
/// broadcast, an interface that goes away when a laptop moves, a firewall that
/// blocks the port — none of those should stop a daemon that can still sync
/// with the peers it already knows about.
pub fn run(
    lan: &Lan,
    device: &DeviceKeys,
    owner: UserId,
    service_port: u16,
    neighbourhood: &Neighbourhood,
    shutdown: &AtomicBool,
) {
    // Announce immediately. Someone who has just started the daemon on a second
    // machine should see it found, not wait out an interval wondering.
    let mut next_announce = Instant::now();
    let mut next_expiry = Instant::now() + EXPIRY;
    let mut refused = 0usize;
    let mut next_complaint = Instant::now() + COMPLAINT_INTERVAL;

    while !shutdown.load(Ordering::Relaxed) {
        if Instant::now() >= next_announce {
            if let Err(error) = lan.announce(device, owner, service_port) {
                // A laptop between networks has no route to broadcast on. That
                // is the normal case, not a fault, and it fixes itself.
                if Instant::now() >= next_complaint {
                    eprintln!("itsanas: could not announce on the local network: {error}");
                    next_complaint = Instant::now() + COMPLAINT_INTERVAL;
                }
            }
            next_announce = Instant::now() + ANNOUNCE_INTERVAL;
        }

        match lan.receive(POLL) {
            Ok(Some((announcement, from))) => {
                if announcement.device == device.device_id() {
                    // Our own broadcast, heard back. Not news.
                    continue;
                }
                report(neighbourhood, &announcement, from, owner);
            }
            Ok(None) => {}
            Err(error) if error.is_foreign_traffic() => {}
            Err(_) => {
                refused += 1;
                if Instant::now() >= next_complaint {
                    eprintln!(
                        "itsanas: {refused} local discovery packets failed to verify. \
                         Either a node here is running a different build, or somebody \
                         on this network is sending packets that are not what they claim."
                    );
                    refused = 0;
                    next_complaint = Instant::now() + COMPLAINT_INTERVAL;
                }
            }
        }

        if Instant::now() >= next_expiry {
            if let Ok(mut table) = neighbourhood.table.lock() {
                table.expire(now_unix().saturating_sub(EXPIRY.as_secs()));
            }
            next_expiry = Instant::now() + EXPIRY;
        }
    }
}

/// Record one announcement and print it only if it is news.
fn report(
    neighbourhood: &Neighbourhood,
    announcement: &itsanas_discover::Announcement,
    from: std::net::IpAddr,
    owner: UserId,
) {
    let Ok(mut table) = neighbourhood.table.lock() else {
        return;
    };

    let mine = announcement.owner_tag == itsanas_discover::beacon::owner_tag(owner);
    let whose = if mine { "your" } else { "another user's" };

    match table.record(announcement, from, now_unix()) {
        Heard::New => println!(
            "found {whose} device {} at {}:{}",
            announcement.device.short(),
            from,
            announcement.port
        ),
        Heard::Moved { from: was } => println!(
            "{} moved from {was} to {}:{}",
            announcement.device.short(),
            from,
            announcement.port
        ),
        Heard::Refreshed | Heard::Ignored => {}
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use itsanas_crypto::ID_LEN;
    use itsanas_discover::Announcement;

    use super::*;

    fn owner() -> UserId {
        UserId::from_bytes([5u8; ID_LEN])
    }

    fn heard(keys: &DeviceKeys, owner: UserId, port: u16) -> Announcement {
        Announcement::parse(&Announcement::seal(keys, owner, port, now_unix())).unwrap()
    }

    #[test]
    fn a_discovered_device_becomes_something_to_dial() {
        let hood = Neighbourhood::new();
        let keys = DeviceKeys::generate().unwrap();

        report(
            &hood,
            &heard(&keys, owner(), 9797),
            "192.168.1.20".parse().unwrap(),
            owner(),
        );

        let order = hood.dial_order(owner());
        assert_eq!(order.len(), 1);
        assert_eq!(order[0].device, keys.device_id());
        assert_eq!(order[0].address.port(), 9797);
        assert!(order[0].mine);
    }

    #[test]
    fn a_confirmed_device_survives_a_flood_of_strangers() {
        // The mechanism in isolation: a protected entry is not evicted.
        //
        // This test confirms only the honest device, which is an *assumption*
        // about what the daemon does rather than a check of it — and the
        // assumption was wrong when this was written: the daemon confirmed
        // every peer that authenticated, strangers included, so the protection
        // could be turned against the table it protected.
        // `red_team_a_flood_of_authenticating_strangers_cannot_take_over_the_table`
        // is the test that checks the rule instead of assuming it.
        let hood = Neighbourhood::new();
        let real = DeviceKeys::generate().unwrap();

        report(
            &hood,
            &heard(&real, owner(), 9797),
            "192.168.1.20".parse().unwrap(),
            owner(),
        );
        hood.confirm(real.device_id());

        for index in 0..400u16 {
            let attacker = DeviceKeys::generate().unwrap();
            report(
                &hood,
                &heard(&attacker, owner(), 9797),
                std::net::IpAddr::V4(std::net::Ipv4Addr::new(
                    10,
                    (index >> 8) as u8,
                    (index & 0xff) as u8,
                    1,
                )),
                owner(),
            );
        }

        assert!(
            hood.dial_order(owner())
                .iter()
                .any(|candidate| candidate.device == real.device_id()),
            "a confirmed peer was evicted by strangers"
        );
    }

    #[test]
    fn the_neighbourhood_is_empty_until_something_is_heard() {
        let hood = Neighbourhood::new();
        assert!(hood.is_empty());
        assert!(hood.dial_order(owner()).is_empty());
    }

    /// One round of the daemon's dialling rule, without the sockets.
    ///
    /// Mirrors `daemon::sync_loop`: dial in the table's order, ration how many
    /// unconfirmed devices are contacted, and confirm only those that earned
    /// it. `useful` decides what each dialled peer turns out to be.
    fn one_round(hood: &Neighbourhood, owner: UserId, useful: &dyn Fn(&DeviceId) -> bool) -> usize {
        let mut strangers = 0usize;
        let mut dialled = 0usize;
        for candidate in hood.dial_order(owner) {
            let known = hood.is_confirmed(&candidate.device);
            if !known {
                if strangers >= NEW_PEERS_PER_ROUND {
                    continue;
                }
                strangers += 1;
            }
            dialled += 1;
            if useful(&candidate.device) {
                hood.confirm(candidate.device);
            }
        }
        dialled
    }

    #[test]
    fn red_team_a_flood_of_authenticating_strangers_cannot_take_over_the_table() {
        // THE ATTACK, end to end at the layer the daemon uses.
        //
        // Device ids are free keypairs. An attacker mints more of them than the
        // table holds, has every one of them claim the victim's owner id — an
        // unauthenticated field, so that is free too — and answers every dial
        // correctly while storing nothing.
        //
        // Claiming the owner id puts them at the FRONT of the dial order, which
        // is the ordering meant to reach your own machines first. If merely
        // authenticating earned protection, they would all become unevictable,
        // fill the table, and the real Raspberry Pi would be refused entry
        // forever — while every node reported discovery as working.
        //
        // If this test fails, one laptop on the same wifi can silently stop a
        // household from syncing.
        let hood = Neighbourhood::new();
        let pi = DeviceKeys::generate().unwrap();

        report(
            &hood,
            &heard(&pi, owner(), 9797),
            "192.168.1.20".parse().unwrap(),
            owner(),
        );

        let attackers: Vec<DeviceKeys> =
            (0..600).map(|_| DeviceKeys::generate().unwrap()).collect();
        let attacker_ids: BTreeSet<DeviceId> =
            attackers.iter().map(DeviceKeys::device_id).collect();

        for (index, attacker) in attackers.iter().enumerate() {
            report(
                &hood,
                // Claiming the victim's own owner id: nothing prevents it, and
                // it is what buys priority in the dial order.
                &heard(attacker, owner(), 9797),
                std::net::IpAddr::V4(std::net::Ipv4Addr::new(
                    10,
                    u8::try_from((index >> 8) & 0xff).unwrap(),
                    u8::try_from(index & 0xff).unwrap(),
                    1,
                )),
                owner(),
            );
            // The Pi does its job whenever it is reached; the attackers never
            // store anything, which is the only cheap way to run this attack.
            one_round(&hood, owner(), &|device| !attacker_ids.contains(device));
        }

        assert!(
            hood.is_confirmed(&pi.device_id()),
            "the honest peer never earned protection"
        );
        assert!(
            hood.dial_order(owner())
                .iter()
                .any(|candidate| candidate.device == pi.device_id()),
            "600 strangers evicted the only machine holding the data"
        );

        let protected_attackers = attacker_ids
            .iter()
            .filter(|device| hood.is_confirmed(device))
            .count();
        assert_eq!(
            protected_attackers, 0,
            "{protected_attackers} strangers became unevictable by answering the phone"
        );
    }

    #[test]
    fn red_team_dialling_strangers_is_rationed_so_a_flood_cannot_eat_the_interval() {
        // THE ATTACK: a thousand minted identities announce themselves. Without
        // a cap the daemon opens a thousand connections in one round, spending
        // the whole sync interval on handshakes with machines that store
        // nothing, while the real peers wait.
        let hood = Neighbourhood::new();
        for index in 0..300u16 {
            let attacker = DeviceKeys::generate().unwrap();
            report(
                &hood,
                &heard(&attacker, owner(), 9797),
                std::net::IpAddr::V4(std::net::Ipv4Addr::new(
                    10,
                    u8::try_from(index >> 8).unwrap(),
                    u8::try_from(index & 0xff).unwrap(),
                    1,
                )),
                owner(),
            );
        }

        let dialled = one_round(&hood, owner(), &|_| false);
        assert!(
            dialled <= NEW_PEERS_PER_ROUND,
            "dialled {dialled} unknown peers in one round; the cap is {NEW_PEERS_PER_ROUND}"
        );
    }

    #[test]
    fn a_confirmed_peer_is_still_dialled_every_round_however_many_strangers_arrive() {
        // The cap rations strangers. It must not ration work: a household with
        // three real machines and a noisy network still syncs every round.
        let hood = Neighbourhood::new();
        let mine: Vec<DeviceKeys> = (0..3).map(|_| DeviceKeys::generate().unwrap()).collect();
        for (index, device) in mine.iter().enumerate() {
            report(
                &hood,
                &heard(device, owner(), 9797),
                std::net::IpAddr::V4(std::net::Ipv4Addr::new(
                    192,
                    168,
                    1,
                    u8::try_from(index).unwrap() + 10,
                )),
                owner(),
            );
            hood.confirm(device.device_id());
        }
        for index in 0..200u16 {
            let stranger = DeviceKeys::generate().unwrap();
            report(
                &hood,
                &heard(&stranger, owner(), 9797),
                std::net::IpAddr::V4(std::net::Ipv4Addr::new(
                    10,
                    u8::try_from(index >> 8).unwrap(),
                    u8::try_from(index & 0xff).unwrap(),
                    1,
                )),
                owner(),
            );
        }

        let dialled = one_round(&hood, owner(), &|_| false);
        assert!(
            dialled >= mine.len(),
            "only {dialled} peers were dialled; the three real machines must always be"
        );
    }

    #[test]
    fn the_poll_is_short_enough_that_shutdown_feels_immediate() {
        assert!(POLL <= Duration::from_secs(1));
        assert!(ANNOUNCE_INTERVAL > POLL);
    }
}
