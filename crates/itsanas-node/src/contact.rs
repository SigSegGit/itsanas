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
//! * **Reading again** is the one connection of its own a read costs: a
//!   device of the account that answered at none of the addresses held for
//!   it may have moved or restarted on another port, so the next round asks,
//!   at first [`REREAD_AFTER_MISS`](crate::contact::REREAD_AFTER_MISS) after
//!   the last read. Without it, a machine that started on a new port was
//!   dialled at the old one until this node's next hourly publication (hand
//!   test, 2026-10-22). Each re-read that brings no new address for a machine
//!   still missing doubles the wait, up to [`PUBLISH_EVERY`](crate::contact::PUBLISH_EVERY), so a machine
//!   that stays off costs a handful of reads before it settles to the hourly
//!   one this node makes anyway.
//!
//! A node that never moves therefore makes one connection an hour instead of
//! twelve.
//!
//! Every time here is this machine's own: an [`Instant`](std::time::Instant)
//! for what lives only in this process, unix seconds of *this machine's* clock
//! for which address last worked, because that outlives a restart in the
//! address book (`<home>/address-book`). Those seconds are compared only with
//! each other. A peer's clock never decides anything (HANDOVER §6), and
//! neither does the coordinator's order: the addresses of a device are a set,
//! dialled in the order of this machine's record of which one last worked.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use itsanas_coord::claim::{SignedClaim, SignedPresence};
use itsanas_crypto::DeviceId;

mod addresses;
mod book;
mod dialling;
mod relay;

pub use dialling::is_this_listener;
use dialling::sort_best_first;
pub use relay::{Board, RELAY_PREFIX, Relayed, SharedBoard};

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

/// How soon after the last read a device of the account that did not answer
/// makes the next round read the account's devices again.
///
/// The hand test of 2026-10-22: a portable key registered at its configured
/// port, then its daemon ran on another one. The daemon published the port it
/// really listened on, but the laptop had read the old one and read again only
/// at its hourly publication, so for up to an hour it dialled a port nobody
/// listened on. A machine of the account that answers at none of the
/// addresses held for it is the sign that the book may be stale, so the next
/// round asks.
///
/// This is the first wait, not every one. A read asked for while a machine is
/// missing doubles the next wait (2 min, 4, 8 ...), up to [`PUBLISH_EVERY`],
/// whether the coordinator answered or not; the wait falls back to this when
/// a read lists a new address for a missing machine, or when every missing
/// machine answers again -- at most once per [`RESET_FLOOR`]. So a laptop shut for the week costs about five
/// reads before this node settles to its hourly one -- about 29 a day, not
/// 288 -- and a machine that comes back on a new port after hours away is
/// found at the latest at that hourly read.
pub const REREAD_AFTER_MISS: Duration = Duration::from_secs(120);

/// How rarely the wait after a miss may go back to [`REREAD_AFTER_MISS`].
///
/// Red team, 2026-10-23: the wait falls back on news -- a read that lists a
/// new address for a missing machine -- and on a missing machine answering
/// again. Both are the word of somebody else: a coordinator can list a new
/// port at every read, and a machine of the account (or whoever holds its
/// key) can answer one round in two. Unfloored, either held this node at a
/// read every two minutes for ever -- 720 a day. At most one fall-back per
/// this long, the doubling wins between them: at most 432 reads a day at a
/// round every 30 s (three per ten minutes: the fall-back's, then two and
/// six minutes after it) and 288 at the default five minutes, the round's
/// own rate, against the 720 an attacker could force before -- 312 and 143
/// measured by the test. A machine that really moves twice in ten minutes
/// is found at the doubled wait, minutes later, not an hour.
pub const RESET_FLOOR: Duration = Duration::from_secs(600);

/// How long a machine of the account that answered at none of its addresses
/// waits before this node dials it again, after its first miss; doubled at
/// each miss after that, up to [`QUIET_AT_MOST`] ([`Contact::dialable`]).
///
/// Most machines of an account are off most of the time -- a laptop shut, a
/// phone asleep, a USB key in a drawer -- and each dial to one costs a connect
/// timeout ([`itsanas_tls::reach::CONNECT_TIMEOUT`], five seconds) and, for a
/// machine that once parked, a session with its relay. Fleet run 40478381966:
/// the runner dialled the laptop's `test` node at every 30 s round, 31 times
/// in twenty minutes. Dialled less often, a machine back on loses little: it
/// dials the others itself as soon as it starts, and whatever says it is back
/// ends the wait ([`Contact::worked`], [`Contact::round_reached`], a new
/// address in [`Contact::read`], discovery hearing it).
pub const QUIET_AFTER_MISS: Duration = Duration::from_secs(60);

/// The longest a machine of the account waits between two dials, however
/// long it has been off: half an hour, so every hour of its ring is still an
/// hour this node tried (`placing::unreached`), and a machine that comes back
/// unannounced is found within the half hour.
pub const QUIET_AT_MOST: Duration = Duration::from_secs(30 * 60);

/// How long [`Contact::dialable`] leaves a device out after its `misses`-th
/// round in a row answering nowhere.
fn quiet_for(misses: u32) -> Duration {
    let doublings = misses.saturating_sub(1).min(16);
    QUIET_AFTER_MISS
        .saturating_mul(1 << doublings)
        .min(QUIET_AT_MOST)
}

/// Every how many rounds a private address of a machine local discovery did
/// not hear here is dialled anyway ([`Contact::dialable`]): every sixth, about
/// half an hour at the default five-minute round.
///
/// Discovery not hearing a machine is weak evidence: another VLAN of the same
/// house, a VPN numbered 10.x, a Wi-Fi that drops multicast, all put a
/// reachable machine at a private address no beacon crosses. One connect
/// timeout every half hour per such address is what finding them costs; an
/// address that answers is then dialled at every round until it fails.
pub const PRIVATE_PROBE_EVERY: u64 = 6;

/// How many devices the address book holds.
///
/// The list comes from the coordinator. Each address in it is signed by its
/// device and checked (`coordinator::verified`), but the coordinator still
/// chooses which devices it lists and which of their past addresses it hands
/// out, and one older than `SignedPeers` answers unsigned. So a hostile
/// coordinator decides what arrives here, and without a bound it would decide
/// how much memory the daemon uses too. An account
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
    /// Whether an unsigned list may be read, from a coordinator that hangs up
    /// on `SignedPeers` as one older than it does.
    ///
    /// True until a coordinator has signed its list in front of this process.
    /// After that, one that hangs up on the question is not old: it is either
    /// failing or trying to talk this node down to addresses it can forge, and
    /// both are a failed read rather than a reason to believe it.
    pub accept_unsigned: bool,
}

impl Due {
    /// Whether the coordinator is dialled at all.
    #[must_use]
    pub fn any(&self) -> bool {
        self.publish || self.read
    }
}

/// A device of the book that answered nowhere at the rounds that dialled it.
#[derive(Clone, Copy, Debug)]
struct Quiet {
    /// Rounds in a row it was dialled and did not answer.
    misses: u32,
    /// The round ([`Contact::rounds`]) the last miss was counted at: a device
    /// failing at three addresses in one round missed once.
    round: u64,
    /// Not dialled before this ([`QUIET_AFTER_MISS`]).
    until: Instant,
    /// An address the book did not hold arrived since the last miss: dialled
    /// at the next round, once, whatever `until` says. Not a reset of
    /// `misses`: a coordinator listing a new port at every read bought three
    /// dials a read that way (the ramp's 1, 2, 4 minutes each time), and buys
    /// one now.
    news: bool,
}

#[derive(Clone, Debug)]
struct Candidate {
    address: String,
    /// When this machine last completed a sync at this address, in unix
    /// seconds of this machine's clock.
    worked: Option<u64>,
    /// The device's own signature over this address. `None` for an address an
    /// older coordinator handed out unsigned: it is dialled, and never written
    /// to the address book or relayed, because nothing proves it.
    presence: Option<SignedPresence>,
    /// The owner's claim on the device, checked against this account
    /// ([`ClaimedPresence::verify_for`]). Only a candidate with both a
    /// presence and a claim is handed to a peer ([`Contact::relayable`]): a
    /// presence alone says where a machine is, not whose.
    claim: Option<SignedClaim>,
    /// Whether this address answered since this process started and has not
    /// failed since ([`Contact::worked`], [`Contact::failed`]): a private
    /// address that answered is dialled although discovery did not hear its
    /// machine ([`Contact::dialable`]). Not kept on disk: what answered at
    /// another house last week proves nothing here, and a restart's warm-up
    /// and probes find it again.
    proven: bool,
}

/// This node's side of its coordinator: what it last published, when it last
/// read, and where the account's other devices were last known to be.
///
/// The daemon keeps the book and the memory that the coordinator signs in
/// `<home>/address-book` ([`Self::load`], [`Self::save`]), so a restart
/// neither re-opens the downgrade to an unsigned list nor forgets which
/// address worked. What it last published and read is not kept: a start
/// publishes anyway, and the read rides on that connection.
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
    /// When a read was last asked for, whether or not the coordinator
    /// answered: what [`REREAD_AFTER_MISS`] is counted from when it is down.
    asked_at: Option<Instant>,
    /// Whether a device of the account in the book answered at none of its
    /// addresses at the last round, and no read has happened since
    /// ([`Self::round_reached`]).
    missed: bool,
    /// The devices of the book the last complete round reached nowhere. Not
    /// cleared by a read: it is what a read's news, and the next round's
    /// answers, are compared with.
    missing: BTreeSet<DeviceId>,
    /// How long after the last read a miss makes the next round read again:
    /// [`REREAD_AFTER_MISS`] doubled at each read asked for while a device is
    /// missing, up to [`PUBLISH_EVERY`]. `None` is the first wait.
    miss_wait: Option<Duration>,
    /// Whether the coordinator has answered with a signed list since this
    /// process started. See [`Due::accept_unsigned`].
    signs: bool,
    book: BTreeMap<DeviceId, Vec<Candidate>>,
    /// Whether anything worth writing changed since the last [`Self::save`].
    changed: bool,
    /// Hosts of other accounts the last round reached and that stored for
    /// this account or served it its own work, at the address that worked:
    /// relays when no machine of the account can be reached (§8 0v). Not
    /// kept on disk: a round finds them again.
    hosts: Vec<(DeviceId, String, Instant)>,
    /// Complete rounds since this process started ([`Self::round_reached`]):
    /// what [`PRIVATE_PROBE_EVERY`] counts.
    rounds: u64,
    /// When the wait after a miss last fell back to [`REREAD_AFTER_MISS`]
    /// ([`RESET_FLOOR`]).
    reset_at: Option<Instant>,
    /// The devices of the book that answered nowhere when last dialled, and
    /// how long they wait before the next dial ([`QUIET_AFTER_MISS`]). Not
    /// kept on disk: a restart dials everybody once, which is how it learns
    /// who is on.
    quiet: BTreeMap<DeviceId, Quiet>,
    /// Hosts of other accounts the coordinator named that answered nowhere
    /// when last dialled: their misses in a row, and the instant before which
    /// they are not dialled ([`Self::introduced_due`]). At most
    /// [`MAX_DEVICES`]; not kept on disk, as `quiet` is not.
    host_quiet: BTreeMap<DeviceId, (u32, Instant)>,
    /// The longest chain each machine of the account signed in a presence a
    /// sibling relayed this round ([`Self::relayed`]), for the store to keep
    /// ([`Self::take_relayed_heads`]): how a machine learns how long the
    /// others' logs are with no coordinator at all.
    relayed_heads: BTreeMap<DeviceId, u64>,
}

/// How long a host found by [`Contact::hosts_reached`] stays a relay
/// choice: a round that pushes nothing does not prove a host, and a relay
/// that dropped out of the list at every quiet round would flap the park.
pub const HOST_RELAY_FOR: Duration = Duration::from_secs(24 * 3600);

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
        //
        // A device that did not answer at any address held for it may have
        // moved, or restarted on another port, since the last read: asked
        // again, at the earliest [`Self::miss_wait`] after the last time, so a
        // machine that is simply off does not turn every round into a
        // connection.
        let last_asked = self.read_at.max(self.asked_at);
        let wait = self.miss_wait.unwrap_or(REREAD_AFTER_MISS);
        let reread =
            self.missed && last_asked.is_none_or(|at| now.saturating_duration_since(at) >= wait);
        let read = publish || self.read_at.is_none() || reread;
        Due {
            publish,
            read,
            accept_unsigned: !self.signs,
        }
    }

    /// Record that the coordinator is about to be asked what `due` says,
    /// answered or not: a read asked for counts against the wait after a
    /// miss even when the coordinator is down, and one asked for while a
    /// device is missing doubles that wait ([`REREAD_AFTER_MISS`]). If the
    /// read brings a new address for it, [`Self::read`] undoes the doubling.
    pub fn asked(&mut self, due: &Due, now: Instant) {
        if due.read {
            self.asked_at = Some(now);
            if self.missed {
                let wait = self.miss_wait.unwrap_or(REREAD_AFTER_MISS);
                self.miss_wait = Some(wait.saturating_mul(2).min(PUBLISH_EVERY));
            }
        }
    }

    /// Record who the round reached, once it has dialled everybody it was
    /// going to: a device of the account in the book that is not in `reached`
    /// answered at none of the addresses held for it, and makes the next
    /// round read again ([`Self::due`], [`REREAD_AFTER_MISS`]).
    ///
    /// Not for a round cut short by a pause or a shutdown: it did not dial
    /// everybody, so a device it skipped did not fail to answer.
    ///
    /// A device that was missing and answered again, or a round that missed
    /// nobody, puts the wait back to [`REREAD_AFTER_MISS`]: the next time a
    /// machine goes missing, it may be because it moved.
    ///
    /// The fall-back on an answer is floored, as the one on news is
    /// ([`RESET_FLOOR`]): a machine that answers one round in two would
    /// otherwise hold the wait at two minutes.
    pub fn round_reached(&mut self, reached: &BTreeSet<DeviceId>, now: Instant) {
        self.rounds = self.rounds.saturating_add(1);
        // Reached on the LAN or as a configured peer: it is on, whatever its
        // book addresses said ([`QUIET_AFTER_MISS`]).
        self.quiet.retain(|device, _| !reached.contains(device));
        self.host_quiet
            .retain(|device, _| !reached.contains(device));
        let missing: BTreeSet<DeviceId> = self
            .book
            .keys()
            .filter(|device| !reached.contains(device))
            .copied()
            .collect();
        if missing.is_empty() || self.missing.iter().any(|device| reached.contains(device)) {
            self.fall_back(now);
        }
        self.missed = !missing.is_empty();
        self.missing = missing;
    }

    /// Put the wait after a miss back to [`REREAD_AFTER_MISS`], unless it fell
    /// back less than [`RESET_FLOOR`] ago. A wait already at its first value
    /// is not a fall-back and does not start the floor.
    fn fall_back(&mut self, now: Instant) {
        if self.miss_wait.is_none() {
            return;
        }
        if self
            .reset_at
            .is_none_or(|at| now.saturating_duration_since(at) >= RESET_FLOOR)
        {
            self.miss_wait = None;
            self.reset_at = Some(now);
        }
    }

    /// Record a publication the coordinator accepted, for `address` as it was
    /// given to [`Self::due`].
    pub fn published(&mut self, address: Option<&str>, now: Instant) {
        self.published = Some((address.map(str::to_owned), now));
    }

    /// Record that the coordinator answered with a signed list, so that it is
    /// never again read unsigned by this process.
    pub fn signed(&mut self) {
        self.changed |= !self.signs;
        self.signs = true;
    }
}

#[cfg(test)]
mod tests {
    use itsanas_coord::claim::{ClaimedPresence, NodeClaim, Presence};
    use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserId, UserKeys};

    use super::*;

    pub(super) const ROUND: Duration = Duration::from_secs(300);

    pub(super) const HOME: &str = "192.168.1.10:9797";

    pub(super) fn device(byte: u8) -> DeviceId {
        DeviceId::from_bytes([byte; 32])
    }

    pub(super) fn listing(devices: &[u8]) -> Vec<(DeviceId, String)> {
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
                contact.read(&listing(&[2]), &[], &[], now);
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
        contact.read(&listing(&[2, 3]), &[], &[], start);

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
        contact.read(&[], &[], &[], start);

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
        contact.read(&listing(&[2]), &[], &[], start);

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
        contact.read(&listing(&[2, 3]), &[], &[], hour);
        assert!(
            contact.candidates().iter().any(|(d, _)| *d == device(3)),
            "the read did not put the new machine in the book"
        );
    }

    /// One round as the daemon runs it, against a coordinator that lists
    /// `listed` and, when `answering` is false, is down: ask what is due,
    /// dial every candidate until its device answers -- only the addresses in
    /// `live` do -- and record who was reached. Returns whether a read was
    /// asked for, and who answered.
    pub(super) fn round_against(
        contact: &mut Contact,
        listed: &[(DeviceId, String)],
        answering: bool,
        live: &[&str],
        now: Instant,
    ) -> (bool, BTreeSet<DeviceId>) {
        let due = contact.due(Some(HOME), now);
        contact.asked(&due, now);
        if answering {
            if due.publish {
                contact.published(Some(HOME), now);
            }
            if due.read {
                contact.read(listed, &[], &[], now);
            }
        }
        let mut reached = BTreeSet::new();
        for (device, address) in contact.candidates() {
            if !reached.contains(&device) && live.contains(&address.as_str()) {
                reached.insert(device);
                contact.worked(device, &address, 1_000);
            }
        }
        contact.round_reached(&reached, now);
        (due.read, reached)
    }

    /// The hand test of 2026-10-22: a portable key registered while its
    /// configuration said port 9798, then its daemon ran with `--listen` on
    /// 9811 and published that. The laptop had read 9798 and read again only
    /// at its hourly publication, so it dialled a port nobody listened on for
    /// up to an hour. A machine of the account that answers nowhere now makes
    /// the next round read again: the new port is dialled within two rounds,
    /// at a portable copy's minute as at the default five, and once it
    /// answers nothing more is asked before the hour.
    #[test]
    fn red_team_a_machine_that_restarted_on_another_port_is_dialled_there_within_two_rounds() {
        let key = device(13);
        let (old, new) = ("192.168.1.13:9798", "192.168.1.13:9811");
        for every in [Duration::from_secs(60), ROUND] {
            let mut contact = Contact::new();
            let start = Instant::now();
            contact.published(Some(HOME), start);
            contact.read(&[(key, old.to_owned())], &[], &[], start);
            // From here on the coordinator lists where the key really is.
            let listed = [(key, new.to_owned())];

            let rounds = u32::try_from(PUBLISH_EVERY.as_secs() / every.as_secs()).expect("rounds");
            let found = (1..rounds).find(|&round| {
                round_against(&mut contact, &listed, true, &[new], start + every * round)
                    .1
                    .contains(&key)
            });
            assert!(
                found.is_some_and(|round| round <= 2),
                "at a round every {every:?}, the key on its new port was first reached at \
                 round {found:?}: the old port was dialled until the hourly read"
            );
            let found = found.expect("found");
            for round in found + 1..rounds {
                assert!(
                    !round_against(&mut contact, &listed, true, &[new], start + every * round).0,
                    "round {round} read again although every machine answered"
                );
            }
        }
    }

    /// The rounds, from 1 to `rounds - 1`, `every` apart, at which a read was
    /// asked for while no machine of the account answers anywhere, against a
    /// coordinator that lists `listed(round)`.
    fn reads_over(
        contact: &mut Contact,
        start: Instant,
        every: Duration,
        rounds: u32,
        answering: bool,
        listed: &dyn Fn(u32) -> Vec<(DeviceId, String)>,
    ) -> Vec<u32> {
        (1..rounds)
            .filter(|&round| {
                round_against(
                    contact,
                    &listed(round),
                    answering,
                    &[],
                    start + every * round,
                )
                .0
            })
            .collect()
    }

    /// The other side of the rule above: a machine of the account that is
    /// really off -- a laptop shut for the week -- must not turn every round
    /// into a connection to the coordinator. With a fixed two-minute wait it
    /// cost 288 reads a day at the default round and 720 at a round every
    /// 30 s (`MIN_INTERVAL`), against the 24 this node makes anyway. Doubling
    /// the wait after each read that brings nothing new settles it in about
    /// five reads to the hourly one: never two reads within
    /// `REREAD_AFTER_MISS`, at most 30 in the day, and exactly one an hour
    /// over its second half.
    #[test]
    fn red_team_a_machine_that_stays_off_for_a_day_costs_about_thirty_reads_not_288() {
        let day = Duration::from_secs(24 * 3600);
        for every in [Duration::from_secs(30), ROUND] {
            let mut contact = Contact::new();
            let start = Instant::now();
            contact.published(Some(HOME), start);
            contact.read(&listing(&[2]), &[], &[], start);
            let rounds = u32::try_from(day.as_secs() / every.as_secs()).expect("rounds");
            let reads = reads_over(&mut contact, start, every, rounds, true, &|_| listing(&[2]));

            let gaps = reads.windows(2).map(|pair| every * (pair[1] - pair[0]));
            assert!(
                gaps.clone().all(|gap| gap >= REREAD_AFTER_MISS),
                "at a round every {every:?}, two reads came closer than {REREAD_AFTER_MISS:?}"
            );
            assert!(
                reads.len() <= 30,
                "at a round every {every:?}, a laptop that stays off for a day made {} reads; \
                 one an hour plus a handful while the wait doubles is about 29",
                reads.len()
            );
            let second_half = reads
                .iter()
                .filter(|&&round| every * round >= day / 2)
                .count();
            assert_eq!(
                second_half, 12,
                "at a round every {every:?}, the wait had not settled to one read an hour \
                 after twelve hours of a machine that is off"
            );
        }
    }

    /// A coordinator that is down is asked with the same back-off: a read
    /// asked for counts whether or not it was answered, or a node would dial
    /// a dead coordinator every two minutes for as long as a laptop is shut.
    /// Rounds every 30 s for most of an hour: reads at 2, 6, 14 and 30
    /// minutes, not 29.
    #[test]
    fn red_team_a_coordinator_that_is_down_is_asked_with_the_same_back_off() {
        let every = Duration::from_secs(30);
        let mut contact = Contact::new();
        let start = Instant::now();
        contact.published(Some(HOME), start);
        contact.read(&listing(&[2]), &[], &[], start);
        let reads = reads_over(&mut contact, start, every, 120, false, &|_| listing(&[2]));
        let minutes: Vec<u64> = reads
            .iter()
            .map(|&round| (every * round).as_secs() / 60)
            .collect();
        assert_eq!(
            minutes,
            [2, 6, 14, 30],
            "a dead coordinator was asked at minutes {minutes:?} of the hour"
        );
    }

    /// The wait goes back to two minutes when it has reason to: a read that
    /// lists a new address for the missing machine (it moved, and may move
    /// again before it answers), or the machine answering again (the next
    /// time it goes missing may be a move, not a shutdown).
    #[test]
    fn red_team_news_or_an_answer_puts_the_wait_after_a_miss_back_to_two_minutes() {
        let every = Duration::from_secs(30);
        let laptop = device(2);
        let (old, new) = ("192.168.1.2:9797", "192.168.1.2:9811");

        // News: reads at 2, 6 and 14 minutes bring nothing; the one at 30
        // lists the new port, where the laptop does not answer yet. The next
        // read comes two minutes later, not 32.
        let mut contact = Contact::new();
        let start = Instant::now();
        contact.published(Some(HOME), start);
        contact.read(&[(laptop, old.to_owned())], &[], &[], start);
        let listed = |round: u32| {
            let at = if round < 60 { old } else { new };
            vec![(laptop, at.to_owned())]
        };
        let reads = reads_over(&mut contact, start, every, 70, true, &listed);
        let minutes: Vec<u64> = reads
            .iter()
            .map(|&round| (every * round).as_secs() / 60)
            .collect();
        assert_eq!(
            minutes,
            [2, 6, 14, 30, 32],
            "after a read that listed a new address, the next came at {minutes:?}"
        );

        // An answer: off for three hours, the wait is an hour. The laptop
        // answers one round, then is off again: the next read comes two
        // minutes after that miss, not an hour after the last read.
        let mut contact = Contact::new();
        let start = Instant::now();
        contact.published(Some(HOME), start);
        contact.read(&[(laptop, old.to_owned())], &[], &[], start);
        let off = 3 * 120;
        reads_over(
            &mut contact,
            start,
            every,
            off,
            true,
            &listed_at(laptop, old),
        );
        let (_, reached) = round_against(
            &mut contact,
            &listed_at(laptop, old)(0),
            true,
            &[old],
            start + every * off,
        );
        assert!(
            reached.contains(&laptop),
            "the laptop at its address was not reached"
        );
        let next = (off + 1..off + 120).find(|&round| {
            round_against(
                &mut contact,
                &listed_at(laptop, old)(round),
                true,
                &[],
                start + every * round,
            )
            .0
        });
        assert!(
            next.is_some_and(|round| every * (round - off) <= REREAD_AFTER_MISS + every * 2),
            "the laptop answered and went missing again; the next read came {:?} later",
            next.map(|round| every * (round - off))
        );
    }

    fn listed_at(device: DeviceId, address: &str) -> impl Fn(u32) -> Vec<(DeviceId, String)> {
        let address = address.to_owned();
        move |_| vec![(device, address.clone())]
    }

    pub(super) fn signed_at(seed: u8, address: &str) -> SignedPresence {
        let keys = DeviceKeys::from_seed(&SecretBytes::new([seed; 32]));
        Presence {
            device: keys.device_id(),
            address: address.to_owned(),
            at_unix: 0,
            head: 0,
        }
        .sign(&keys)
    }

    pub(super) fn owner(seed: u8) -> UserKeys {
        UserKeys::derive(&MasterSecret::from_bytes([seed; 32]))
    }

    /// The account these tests' node belongs to.
    pub(super) fn me() -> UserId {
        owner(1).user_id()
    }

    /// Device `seed`'s presence at `address`, with `account`'s claim on it.
    pub(super) fn claimed_at(account: &UserKeys, seed: u8, address: &str) -> ClaimedPresence {
        let keys = DeviceKeys::from_seed(&SecretBytes::new([seed; 32]));
        ClaimedPresence {
            presence: signed_at(seed, address),
            claim: NodeClaim {
                owner: account.user_id(),
                device: keys.device_id(),
                pledged_bytes: 0,
                issued_unix: 0,
                revoked: false,
            }
            .sign(account),
        }
    }

    pub(super) fn found(presences: &[SignedPresence]) -> Vec<(DeviceId, String)> {
        presences
            .iter()
            .map(|signed| (signed.presence.device, signed.presence.address.clone()))
            .collect()
    }
}
