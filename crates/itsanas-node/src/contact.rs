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
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use itsanas_coord::claim::{ClaimedPresence, MAX_CLOCK_SKEW, SignedClaim, SignedPresence};
use itsanas_crypto::{DeviceId, UserId};
use serde::{Deserialize, Serialize};

use crate::coordinator::is_private_address;
use crate::{NodeError, Result};

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

/// The address book's version. A file of another version is read as empty,
/// except version 1, which is version 2 with no claims and is read as such:
/// dropping it would forget that the coordinator signs, and re-open the
/// downgrade once on the upgrade.
const BOOK_VERSION: u32 = 2;

/// The address book as written to `<home>/address-book`.
#[derive(Serialize, Deserialize)]
struct BookFile {
    version: u32,
    signs: bool,
    entries: Vec<BookEntry>,
}

#[derive(Serialize, Deserialize)]
struct BookEntry {
    presence: SignedPresence,
    worked: Option<u64>,
    claim: Option<SignedClaim>,
}

/// Version 1 of the file, before claims.
#[derive(Serialize, Deserialize)]
struct BookFileV1 {
    version: u32,
    signs: bool,
    entries: Vec<BookEntryV1>,
}

#[derive(Serialize, Deserialize)]
struct BookEntryV1 {
    presence: SignedPresence,
    worked: Option<u64>,
}

impl From<BookFileV1> for BookFile {
    fn from(old: BookFileV1) -> Self {
        Self {
            version: BOOK_VERSION,
            signs: old.signs,
            entries: old
                .entries
                .into_iter()
                .map(|entry| BookEntry {
                    presence: entry.presence,
                    worked: entry.worked,
                    claim: None,
                })
                .collect(),
        }
    }
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

    /// Take in the coordinator's list of the account's other devices.
    ///
    /// `found` is every `(device, address)` the read kept; `presences` are the
    /// signed ones among them (none when an older coordinator answered
    /// unsigned). An address with its signature in `presences` carries it
    /// into the book, which is what lets it be written to disk. `claimed` are
    /// those that came with this account's claim, already checked
    /// ([`crate::coordinator::verified_claimed`]); none when the coordinator
    /// is older than `ClaimedPeers`.
    ///
    /// A device no longer listed is dropped: it was withdrawn, or has been
    /// silent past the presence's lifetime, and in both cases this node has no
    /// address for it worth dialling. For the others, the listed address joins
    /// the set of candidates. It replaces nothing and takes no rank from being
    /// new: the one that last worked is still dialled first.
    pub fn read(
        &mut self,
        found: &[(DeviceId, String)],
        presences: &[SignedPresence],
        claimed: &[ClaimedPresence],
        now: Instant,
    ) {
        self.read_at = Some(now);
        self.missed = false;
        // News for a machine that is missing -- an address the book did not
        // hold for it -- is what re-reading is for: the next miss may be the
        // same story again, so it is asked about soon, not in an hour.
        let news = found.iter().any(|(device, address)| {
            self.missing.contains(device)
                && self
                    .book
                    .get(device)
                    .is_none_or(|held| !held.iter().any(|candidate| candidate.address == *address))
        });
        if news {
            self.fall_back(now);
        }
        let listed: BTreeSet<DeviceId> = found.iter().map(|(device, _)| *device).collect();
        let before = self.book.len();
        self.book.retain(|device, _| listed.contains(device));
        self.changed |= self.book.len() != before;

        for (device, address) in found {
            let presence = presences
                .iter()
                .find(|signed| {
                    signed.presence.device == *device && signed.presence.address == *address
                })
                .cloned();
            let claim = claimed
                .iter()
                .find(|row| presence.as_ref() == Some(&row.presence))
                .map(|row| row.claim.clone());
            self.insert(*device, address, presence, claim, None);
        }
    }

    /// Add one candidate, within [`MAX_DEVICES`] and [`MAX_ADDRESSES`]. The one
    /// door into the book, for the wire and for the file alike.
    fn insert(
        &mut self,
        device: DeviceId,
        address: &str,
        presence: Option<SignedPresence>,
        claim: Option<SignedClaim>,
        worked: Option<u64>,
    ) {
        if !self.book.contains_key(&device) && self.book.len() >= MAX_DEVICES {
            return;
        }
        let candidates = self.book.entry(device).or_default();
        if let Some(known) = candidates.iter_mut().find(|known| known.address == address) {
            // Signed at last, or signed again later: keep the latest proof.
            // The device's own date, compared only with its own earlier
            // ones. Kept old, the presence this node relays would look stale
            // to the receiver, who could not tell it from a replay.
            if let Some(presence) = presence
                && known
                    .presence
                    .as_ref()
                    .is_none_or(|held| held.presence.at_unix < presence.presence.at_unix)
            {
                known.presence = Some(presence);
                self.changed = true;
            }
            // A claim at last, or a later one: the later is the owner's
            // current word on the device.
            if let Some(claim) = claim
                && known.presence.is_some()
                && known
                    .claim
                    .as_ref()
                    .is_none_or(|held| held.claim.issued_unix < claim.claim.issued_unix)
            {
                known.claim = Some(claim);
                self.changed = true;
            }
            return;
        }
        candidates.push(Candidate {
            address: address.to_owned(),
            worked,
            // Never a claim without the presence it vouches for.
            claim: presence.as_ref().and(claim),
            presence,
            proven: false,
        });
        if candidates.len() > MAX_ADDRESSES {
            sort_best_first(candidates);
            candidates.truncate(MAX_ADDRESSES);
        }
        self.changed = true;
    }

    /// Record that a sync with `device` at `address` completed.
    ///
    /// Only for a device the coordinator listed: a device found on the LAN or
    /// typed into the configuration has its own route and is not this book's
    /// business.
    ///
    /// `now_unix` is this machine's clock, and is only ever compared with
    /// other readings of it. Not even that is trusted to go forward: a
    /// Raspberry Pi with no real-time clock reads 1970 until NTP answers, and
    /// a success recorded then would rank below a stale address recorded
    /// yesterday, costing a connect timeout every round. So a success is
    /// recorded as later than every success already in the book, whatever the
    /// clock says.
    pub fn worked(&mut self, device: DeviceId, address: &str, now_unix: u64) {
        let latest = self
            .book
            .values()
            .flatten()
            .filter_map(|candidate| candidate.worked)
            .max();
        let at = latest.map_or(now_unix, |latest| now_unix.max(latest.saturating_add(1)));
        if let Some(candidates) = self.book.get_mut(&device)
            && let Some(candidate) = candidates.iter_mut().find(|c| c.address == address)
        {
            candidate.worked = Some(at);
            candidate.proven = true;
            self.changed = true;
        }
    }

    /// Record that `device` did not answer at `address` this round: a private
    /// address that had answered goes back to being dialled only as
    /// [`Self::dialable`] allows a machine not heard here.
    pub fn failed(&mut self, device: DeviceId, address: &str) {
        if let Some(candidate) = self
            .book
            .get_mut(&device)
            .and_then(|candidates| candidates.iter_mut().find(|c| c.address == address))
        {
            candidate.proven = false;
        }
    }

    /// Where the address book lives under a node's home.
    #[must_use]
    pub fn path(home: &Path) -> PathBuf {
        home.join("address-book")
    }

    /// The contact a daemon starts with: the address book at `path`, or an
    /// empty one.
    ///
    /// Every entry goes through the door the coordinator's list goes through:
    /// its signature is checked ([`SignedPresence::verify_origin`]) and
    /// [`MAX_DEVICES`] and [`MAX_ADDRESSES`] bound what is kept. The file sits
    /// on disk, where anything with this user's rights can edit it, and an
    /// address in it is dialled before the coordinator is asked anything.
    ///
    /// A claim is checked against `owner`, this node's account
    /// ([`ClaimedPresence::verify_for`]); one that fails is dropped and the
    /// address kept, dialled but never relayed.
    ///
    /// What this does not defend: somebody with this user's rights can also set
    /// `signs` false or mark a genuine but stale address as having worked. Such
    /// a person can rewrite the configuration too; the check is against an
    /// address nobody signed, not against the owner of the home directory.
    ///
    /// A missing file is a first start. An unreadable one is an empty book and
    /// a sentence in the second value, never a failed start: the book is a
    /// cache, and the coordinator refills it on the first round.
    #[must_use]
    pub fn load(path: &Path, owner: UserId) -> (Self, Option<String>) {
        let mut contact = Self::new();
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (contact, None),
            Err(error) => {
                return (
                    contact,
                    Some(format!(
                        "could not read {}: {error}; starting with an empty address book",
                        path.display()
                    )),
                );
            }
        };
        // The version comes first in every version of the file, so it can be
        // read before knowing the shape of the rest.
        let decoded = match postcard::take_from_bytes::<u32>(&bytes) {
            Ok((1, _)) => postcard::from_bytes::<BookFileV1>(&bytes).map(BookFile::from),
            _ => postcard::from_bytes::<BookFile>(&bytes),
        };
        let file: BookFile = match decoded {
            Ok(file) if file.version == BOOK_VERSION => file,
            Ok(file) => {
                return (
                    contact,
                    Some(format!(
                        "{} is version {}, this node reads {BOOK_VERSION}; starting with an empty address book",
                        path.display(),
                        file.version
                    )),
                );
            }
            Err(error) => {
                return (
                    contact,
                    Some(format!(
                        "{} is damaged ({error}); starting with an empty address book",
                        path.display()
                    )),
                );
            }
        };
        contact.signs = file.signs;
        let offered = file.entries.len();
        let mut forged = 0usize;
        let mut foreign = 0usize;
        for entry in file.entries {
            if entry.presence.verify_origin().is_err() {
                forged += 1;
                continue;
            }
            let claim = entry.claim.filter(|claim| {
                let kept = ClaimedPresence {
                    presence: entry.presence.clone(),
                    claim: claim.clone(),
                }
                .verify_for(owner)
                .is_ok();
                foreign += usize::from(!kept);
                kept
            });
            let device = entry.presence.presence.device;
            let address = entry.presence.presence.address.clone();
            contact.insert(device, &address, Some(entry.presence), claim, entry.worked);
        }
        contact.changed = false;
        let mut problems = Vec::new();
        if forged > 0 {
            problems.push(format!(
                "{forged} of {offered} addresses their device never signed, dropped"
            ));
        }
        if foreign > 0 {
            problems.push(format!(
                "{foreign} claims that are not a live device of this account, dropped \
                 (their addresses are kept and never relayed)"
            ));
        }
        let warning = (!problems.is_empty())
            .then(|| format!("{} held {}", path.display(), problems.join("; ")));
        (contact, warning)
    }

    /// Write the address book to `path` if anything in it changed since it was
    /// loaded or last saved. Returns whether it wrote.
    ///
    /// Only signed addresses are written: an unsigned one is on an old
    /// coordinator's word, and a file is no place to launder it into
    /// something [`Self::load`] would have to take on trust. Written to a
    /// temporary file and renamed over the old one, so a power cut leaves the
    /// old book or the new one, never an empty file.
    ///
    /// # Errors
    ///
    /// If the file cannot be written or renamed. The book stays marked
    /// changed, so the next round tries again.
    pub fn save(&mut self, path: &Path) -> Result<bool> {
        if !self.changed {
            return Ok(false);
        }
        let entries = self
            .book
            .values()
            .flatten()
            .filter_map(|candidate| {
                candidate.presence.clone().map(|presence| BookEntry {
                    presence,
                    worked: candidate.worked,
                    claim: candidate.claim.clone(),
                })
            })
            .collect();
        let bytes = postcard::to_stdvec(&BookFile {
            version: BOOK_VERSION,
            signs: self.signs,
            entries,
        })?;
        let temporary = path.with_extension("tmp");
        let io = |error| NodeError::Io {
            path: path.to_owned(),
            source: error,
        };
        // Synced before the rename: without it a crash can leave the new name
        // pointing at data that never reached the disk.
        let mut file = std::fs::File::create(&temporary).map_err(io)?;
        std::io::Write::write_all(&mut file, &bytes).map_err(io)?;
        file.sync_all().map_err(io)?;
        drop(file);
        std::fs::rename(&temporary, path).map_err(io)?;
        self.changed = false;
        Ok(true)
    }

    /// Every address known for the account's other devices, in dialling order:
    /// for each device, the one that last worked first, then the ones that can
    /// work from anywhere before the private ones.
    #[must_use]
    pub fn candidates(&self) -> Vec<(DeviceId, String)> {
        self.ordered()
            .into_iter()
            .map(|(device, candidate)| (device, candidate.address))
            .collect()
    }

    /// Every candidate, in [`Self::candidates`]'s order.
    fn ordered(&self) -> Vec<(DeviceId, Candidate)> {
        let mut out = Vec::new();
        for (device, candidates) in &self.book {
            let mut ordered = candidates.clone();
            sort_best_first(&mut ordered);
            out.extend(ordered.into_iter().map(|candidate| (*device, candidate)));
        }
        out
    }

    /// [`Self::candidates`], less the addresses on a private network (RFC 1918)
    /// of a machine that is plausibly not on this one: `heard_here` says
    /// whether local discovery heard the device, and `None` -- discovery not
    /// running here -- keeps every address, as before.
    ///
    /// The hand test of 2026-10-22: two houses both numbered 192.168.1.x, and
    /// the laptop in one and a Mac in the other dialled each other's
    /// 192.168.1.x address from the coordinator at every round, each dial a
    /// connect timeout. A private address is somebody's own network; the
    /// coordinator, or a peer relaying it, says where a machine is on *its*
    /// network, not that this machine shares it. The evidence that it does is
    /// a beacon heard here. (Both machines having announced from the same
    /// public address would be evidence too, but the coordinator does not
    /// record the address it sees a publication come from.)
    ///
    /// Not hearing a machine is weak evidence, though (red team,
    /// 2026-10-23): another VLAN, a VPN numbered 10.x, a Wi-Fi that filters
    /// multicast, and the first minute of discovery all hide a machine this
    /// one can reach. So the gate has three doors: `heard_here` answers `None`
    /// while discovery warms up (`Neighbourhood::heard_here`); one round in
    /// [`PRIVATE_PROBE_EVERY`] -- the first, then every sixth -- dials every
    /// private address anyway; and an address that answered
    /// ([`Self::worked`]) is dialled at every round until it fails
    /// ([`Self::failed`]). Another house's 192.168.1.x thus costs one connect
    /// timeout every sixth round, not one every round.
    ///
    /// A machine left with no address to dial is missed like any other
    /// ([`Self::round_reached`]): the coordinator is asked again, at the
    /// backed-off rate, in case it moved somewhere this machine can reach.
    #[must_use]
    pub fn dialable(
        &self,
        heard_here: impl Fn(&DeviceId) -> Option<bool>,
    ) -> Vec<(DeviceId, String)> {
        let probe = self.rounds.is_multiple_of(PRIVATE_PROBE_EVERY);
        self.ordered()
            .into_iter()
            .filter(|(device, candidate)| {
                probe
                    || candidate.proven
                    || !only_rfc1918(&candidate.address)
                    || heard_here(device) != Some(false)
            })
            .map(|(device, candidate)| (device, candidate.address))
            .collect()
    }

    /// The machine of the account to park on when this one cannot be
    /// reached (§8 0v): the first, in dialling order, with an address that
    /// works from anywhere and is not itself a relay. Own account only in
    /// this first version: a host of another account relays too (its
    /// `relays_for`), but this node does not choose one yet.
    #[must_use]
    pub fn relay_choice(&self) -> Option<(DeviceId, String)> {
        self.relay_choices().into_iter().next()
    }

    /// Every machine [`Self::relay_choice`] could pick, best first: a
    /// first choice that refuses does not leave this node unreachable while
    /// another would take the park.
    ///
    /// The account's own machines first; then hosts of other accounts the
    /// last round reached ([`Self::hosts_reached`]), which relay for an
    /// account whose claim they confirmed, under a lower cap.
    #[must_use]
    pub fn relay_choices(&self) -> Vec<(DeviceId, String)> {
        let mut choices: Vec<_> = self.candidates();
        for (device, address, _) in &self.hosts {
            if !choices.iter().any(|(known, _)| known == device) {
                choices.push((*device, address.clone()));
            }
        }
        choices.retain(|(_, address)| {
            !address.starts_with(RELAY_PREFIX) && !is_private_address(address)
        });
        choices
    }

    /// The hosts of other accounts this round reached and that proved
    /// themselves, at `now`; kept [`HOST_RELAY_FOR`] after they last did.
    pub fn hosts_reached(&mut self, hosts: Vec<(DeviceId, String)>, now: Instant) {
        for (device, address) in hosts {
            self.hosts.retain(|(known, _, _)| *known != device);
            self.hosts.push((device, address, now));
        }
        self.hosts
            .retain(|(_, _, seen)| now.saturating_duration_since(*seen) < HOST_RELAY_FOR);
    }

    /// The relay behind an address `relay:<device>@<address>`, and where to
    /// dial it: its own address from this book when the relay is a machine
    /// of the account, else the one carried in the address -- a host of
    /// another account is in no book here. Either way the relay's key is
    /// pinned on the dial (`connect_relayed`), so an address that leads
    /// elsewhere reaches nobody.
    #[must_use]
    pub fn through(&self, address: &str) -> Option<(DeviceId, String)> {
        let rest = address.strip_prefix(RELAY_PREFIX)?;
        let (id, carried) = rest
            .split_once('@')
            .map_or((rest, None), |(id, at)| (id, Some(at)));
        let relay: DeviceId = id.parse().ok()?;
        self.candidates()
            .into_iter()
            .find(|(device, address)| *device == relay && !address.starts_with(RELAY_PREFIX))
            .or_else(|| {
                carried
                    .filter(|at| !at.is_empty() && !at.starts_with(RELAY_PREFIX))
                    .map(|at| (relay, at.to_owned()))
            })
    }

    /// What this node may hand a peer of its account: every address it holds
    /// with both the device's signature and the owner's claim.
    ///
    /// An unsigned address is on an old coordinator's word and an unclaimed
    /// one proves where a machine is but not whose; neither leaves this node.
    #[must_use]
    pub fn relayable(&self) -> Vec<ClaimedPresence> {
        self.book
            .values()
            .flatten()
            .filter_map(|candidate| {
                Some(ClaimedPresence {
                    presence: candidate.presence.clone()?,
                    claim: candidate.claim.clone()?,
                })
            })
            .collect()
    }

    /// Take in the rows a machine of this account relayed
    /// ([`itsanas_net::Request::Presences`]), keeping only what the book can
    /// check for itself.
    ///
    /// A row is kept only if all of these hold, and each one is an attack it
    /// refuses:
    ///
    /// * it decodes, and both signatures check out for `owner`
    ///   ([`ClaimedPresence::verify_for`]): a relay cannot forge an address,
    ///   nor pass off another account's genuine machine as one of this one's;
    /// * its device is one the book already holds a claim for, from the
    ///   coordinator or the file. **A relay refreshes addresses; it never
    ///   introduces a machine.** A device the owner withdrew is dropped by the
    ///   next coordinator read ([`Self::read`]), and from then on no relay
    ///   can bring it back with a claim signed before the withdrawal, which
    ///   `verify_for` cannot date. New machines are learnt from the
    ///   coordinator, as before;
    /// * its claim is not older than the one the book holds for that device
    ///   (the owner's clock against itself);
    /// * its presence is not older than the latest the book holds for that
    ///   device (the device's clock against itself): an address the machine
    ///   has since left cannot be replayed into the book.
    ///
    /// A kept row joins as a candidate that has never worked: it is dialled
    /// after every address that has, and [`MAX_ADDRESSES`] drops never-worked
    /// ones first, so a relay cannot push out an address that answers. `me`
    /// is this machine, which the book never lists.
    ///
    /// Dates more than [`MAX_CLOCK_SKEW`] past `now` count for nothing: a row
    /// dated so is refused, and a presence already held with such a date is
    /// not "the latest". Otherwise one device whose clock read 2099 once --
    /// signed genuinely, since it is that device's own clock -- became the
    /// newest presence for good, and every later relayed address for it was
    /// refused as older. The coordinator's read, which has no date filter,
    /// still reaches the machine; the relay no longer goes blind.
    pub fn relayed(&mut self, rows: &[Vec<u8>], owner: UserId, me: DeviceId, now: u64) -> Relayed {
        let horizon = now.saturating_add(MAX_CLOCK_SKEW);
        let mut outcome = Relayed::default();
        for row in rows {
            let Ok(row) = postcard::from_bytes::<ClaimedPresence>(row) else {
                outcome.refused += 1;
                continue;
            };
            let device = row.presence.presence.device;
            if device == me {
                continue;
            }
            if row.verify_for(owner).is_err() || row.presence.presence.at_unix > horizon {
                outcome.refused += 1;
                continue;
            }
            let Some(held) = self.book.get(&device) else {
                outcome.refused += 1;
                continue;
            };
            let latest_claim = held
                .iter()
                .filter_map(|candidate| candidate.claim.as_ref())
                .map(|claim| claim.claim.issued_unix)
                .max();
            let latest_presence = held
                .iter()
                .filter_map(|candidate| candidate.presence.as_ref())
                .map(|presence| presence.presence.at_unix)
                .filter(|at| *at <= horizon)
                .max();
            let fresh = latest_claim.is_some_and(|at| row.claim.claim.issued_unix >= at)
                && latest_presence.is_none_or(|at| row.presence.presence.at_unix >= at);
            if !fresh {
                outcome.refused += 1;
                continue;
            }
            let address = row.presence.presence.address.clone();
            self.insert(device, &address, Some(row.presence), Some(row.claim), None);
            outcome.kept += 1;
        }
        outcome
    }

    /// What this node answers a machine of its account that asks where the
    /// others are. See [`Board`].
    #[must_use]
    pub fn board(&self) -> Board {
        let members = self
            .book
            .iter()
            .filter(|(_, candidates)| candidates.iter().any(|c| c.claim.is_some()))
            .map(|(device, _)| *device)
            .collect();
        let rows = self
            .relayable()
            .into_iter()
            .filter_map(|row| {
                let device = row.presence.presence.device;
                postcard::to_stdvec(&row).ok().map(|bytes| (device, bytes))
            })
            .collect();
        Board {
            members,
            rows,
            relays: self.relay_choices(),
        }
    }
}

/// What [`Contact::relayed`] did with a peer's answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Relayed {
    /// Rows that joined or refreshed the book.
    pub kept: usize,
    /// Rows refused: undecodable, forged, another account's, a machine the
    /// book does not know, or older than what it holds.
    pub refused: usize,
}

/// The book as the listener answers from it: who may ask, and what they get.
///
/// A copy, because the book belongs to the daemon's round and the listener
/// answers on other threads; the daemon replaces it after each round
/// ([`SharedBoard::replace`]).
///
/// **Who may ask:** a device the book holds with this account's claim -- a
/// machine the coordinator listed as one of this account's, checked by the
/// owner's signature. Not a device that merely says it belongs: the request
/// carries nothing, and the caller is the device TLS proved. A machine of the
/// account the coordinator has not listed here yet is refused until it is;
/// it is on the coordinator's list and can read it there.
#[derive(Clone, Debug, Default)]
pub struct Board {
    members: BTreeSet<DeviceId>,
    rows: Vec<(DeviceId, Vec<u8>)>,
    /// [`Contact::relay_choices`], for the thread that parks.
    relays: Vec<(DeviceId, String)>,
}

/// How an address names a relay instead of a socket (§8 0v):
/// `relay:<device id>`, the device a machine nobody can reach has parked on.
pub const RELAY_PREFIX: &str = "relay:";

/// A [`Board`] shared between the round that writes it and the listener that
/// answers from it.
#[derive(Debug, Default)]
pub struct SharedBoard(std::sync::Mutex<Board>);

impl SharedBoard {
    /// Replace what the listener answers with.
    pub fn replace(&self, board: Board) {
        // A poisoned lock means a listener thread panicked while reading; the
        // board is a plain value, whole either way.
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = board;
    }
}

impl SharedBoard {
    /// The machines to park on, best first, as the last round saw them.
    #[must_use]
    pub fn relays(&self) -> Vec<(DeviceId, String)> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .relays
            .clone()
    }
}

impl itsanas_net::Relay for SharedBoard {
    fn presences_for(&self, caller: DeviceId) -> Option<Vec<Vec<u8>>> {
        let board = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !board.members.contains(&caller) {
            return None;
        }
        // Not the caller's own rows: it knows where it is.
        Some(
            board
                .rows
                .iter()
                .filter(|(device, _)| *device != caller)
                .map(|(_, row)| row.clone())
                .collect(),
        )
    }
}

/// Worked most recently first, then never-worked public before never-worked
/// private. Stable, so ties keep the order they arrived in.
/// Whether every entry of `address` (one address, or an `AddressList`) is an
/// IPv4 address of a private network, RFC 1918. A name, a public or an IPv6
/// address, a `relay:` -- anything else -- is not.
fn only_rfc1918(address: &str) -> bool {
    let entries: Vec<std::net::SocketAddr> = address
        .split(',')
        .map(|entry| entry.trim().parse())
        .collect::<std::result::Result<_, _>>()
        .unwrap_or_default();
    !entries.is_empty()
        && entries
            .iter()
            .all(|entry| matches!(entry.ip(), std::net::IpAddr::V4(v4) if v4.is_private()))
}

fn sort_best_first(candidates: &mut [Candidate]) {
    candidates.sort_by(|a, b| {
        b.worked
            .cmp(&a.worked)
            .then_with(|| is_private_address(&a.address).cmp(&is_private_address(&b.address)))
    });
}

#[cfg(test)]
mod tests {
    use itsanas_coord::claim::{NodeClaim, Presence};
    use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};

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

    #[test]
    fn red_team_a_relay_the_account_knows_is_dialled_at_the_books_address() {
        let mut contact = Contact::new();
        let relay = device(3);
        let listed = vec![
            (device(2), format!("{RELAY_PREFIX}{relay}@203.0.113.5:9797")),
            (relay, "203.0.113.5:9797".to_owned()),
            (device(1), "192.168.1.4:9797".to_owned()),
            (
                device(5),
                format!("{RELAY_PREFIX}{relay}@198.51.100.9:9797"),
            ),
        ];
        contact.read(&listed, &[], &[], Instant::now());
        assert_eq!(
            contact.through(&format!("{RELAY_PREFIX}{relay}")),
            Some((relay, "203.0.113.5:9797".to_owned()))
        );
        assert_eq!(
            contact.through(&format!("{RELAY_PREFIX}{relay}@198.51.100.9:9797")),
            Some((relay, "203.0.113.5:9797".to_owned())),
            "the address carried in a presence won over the book's for a relay of \
             the account"
        );
        assert_eq!(
            contact.through(&format!("{RELAY_PREFIX}{}", device(9))),
            None
        );
        // A host of another account is in no book: the carried address is
        // all there is, and the relay's key is pinned on the dial.
        assert_eq!(
            contact.through(&format!("{RELAY_PREFIX}{}@198.51.100.9:9797", device(9))),
            Some((device(9), "198.51.100.9:9797".to_owned()))
        );
        assert_eq!(
            contact.relay_choice(),
            Some((relay, "203.0.113.5:9797".to_owned())),
            "a machine parked on a private address, or on another relay"
        );
    }

    #[test]
    fn red_team_a_host_of_another_account_relays_only_after_the_accounts_own_machines() {
        let mut contact = Contact::new();
        let own = device(3);
        let host = device(1);
        contact.read(
            &[(own, "203.0.113.5:9797".to_owned())],
            &[],
            &[],
            Instant::now(),
        );
        let now = Instant::now();
        contact.hosts_reached(
            vec![
                (host, "198.51.100.7:9797".to_owned()),
                (device(2), "192.168.1.2:9797".to_owned()),
            ],
            now,
        );
        assert_eq!(
            contact.relay_choices(),
            vec![
                (own, "203.0.113.5:9797".to_owned()),
                (host, "198.51.100.7:9797".to_owned())
            ],
            "a stranger's host was put before the account's own machine, or a host \
             on a private address was offered as a relay"
        );
        // A quiet round proves nothing either way: the host stays.
        contact.hosts_reached(Vec::new(), now + Duration::from_secs(300));
        assert_eq!(
            contact.relay_choices().len(),
            2,
            "a round that pushed nothing dropped the relay: the park would flap"
        );
        contact.hosts_reached(Vec::new(), now + HOST_RELAY_FOR);
        assert_eq!(
            contact.relay_choices().len(),
            1,
            "a host unseen for a day stayed"
        );
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
    fn round_against(
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

    /// The hand test of 2026-10-22: the laptop's house and the Mac's are both
    /// 192.168.1.x, and the laptop dialled the Mac's 192.168.1.x address from
    /// the coordinator at every round, timing out each time. A private address
    /// of a machine discovery did not hear is dialled only at a probe round,
    /// one in [`PRIVATE_PROBE_EVERY`]; a public address, a list with one, a
    /// name and a relay always are; and a node whose discovery is off, or
    /// warming up, dials as it always did.
    #[test]
    fn red_team_a_private_address_of_another_house_is_not_dialled() {
        let (mac, pi) = (device(5), device(6));
        let mut contact = Contact::new();
        contact.read(
            &[
                (mac, "192.168.1.20:9797".to_owned()),
                (mac, "10.0.0.20:9797".to_owned()),
                (mac, "172.16.4.20:9797".to_owned()),
                (pi, "192.168.1.30:9797".to_owned()),
                (pi, "203.0.113.30:9797".to_owned()),
                (pi, "192.168.1.30:9797,[2001:db8::30]:9797".to_owned()),
                (pi, "pi.example.net:9797".to_owned()),
            ],
            &[],
            &[],
            Instant::now(),
        );
        let private = [
            "192.168.1.20:9797",
            "10.0.0.20:9797",
            "172.16.4.20:9797",
            "192.168.1.30:9797",
        ];
        let addresses = |contact: &Contact, heard: Option<bool>| -> Vec<String> {
            contact
                .dialable(|_| heard)
                .into_iter()
                .map(|(_, address)| address)
                .collect()
        };

        // Two hours of five-minute rounds, nobody answering at a private
        // address: the other house is dialled at the probe rounds only.
        let start = Instant::now();
        let mut probed = Vec::new();
        for round in 0..24u32 {
            let elsewhere = addresses(&contact, Some(false));
            if elsewhere
                .iter()
                .any(|address| private.contains(&address.as_str()))
            {
                probed.push(round);
            }
            for kept in [
                "203.0.113.30:9797",
                "192.168.1.30:9797,[2001:db8::30]:9797",
                "pi.example.net:9797",
            ] {
                assert!(
                    elsewhere.iter().any(|address| address == kept),
                    "round {round}: {kept} was not dialled"
                );
            }
            for address in private {
                let device = if address.ends_with(".30:9797") {
                    pi
                } else {
                    mac
                };
                contact.failed(device, address);
            }
            contact.round_reached(&BTreeSet::new(), start + ROUND * round);
        }
        assert_eq!(
            probed,
            [0, 6, 12, 18],
            "a private address of a machine discovery never heard here was dialled at \
             rounds {probed:?} of 24, not one in {PRIVATE_PROBE_EVERY}"
        );
        assert_eq!(
            addresses(&contact, Some(true)).len(),
            7,
            "a machine heard on this LAN lost an address"
        );
        assert_eq!(
            addresses(&contact, None).len(),
            7,
            "with discovery off or warming up, an address was dropped"
        );
    }

    /// The other side of the gate (red team, 2026-10-23): a NAS at 10.0.2.5
    /// on another VLAN of the same house, or behind a VPN, is never heard by
    /// discovery and is reachable all the same. It is dialled while discovery
    /// warms up, at every probe round after that, and -- once a probe has
    /// reached it -- at every round, until it fails.
    #[test]
    fn red_team_a_machine_on_another_vlan_is_still_dialled() {
        let nas = device(7);
        let address = "10.0.2.5:9797";
        let mut contact = Contact::new();
        let start = Instant::now();
        contact.read(&[(nas, address.to_owned())], &[], &[], start);
        // Past the first round, which is a probe round of its own.
        contact.round_reached(&BTreeSet::new(), start);
        let dialled = |contact: &Contact, heard: Option<bool>| {
            contact
                .dialable(|_| heard)
                .iter()
                .any(|(device, at)| *device == nas && at == address)
        };

        assert!(
            dialled(&contact, None),
            "the VLAN's NAS was not dialled while discovery warmed up"
        );

        // Discovery warmed up and never hears it; nothing answers.
        let mut probes = Vec::new();
        for round in 1..13u32 {
            if dialled(&contact, Some(false)) {
                probes.push(round);
                contact.failed(nas, address);
            }
            contact.round_reached(&BTreeSet::new(), start + ROUND * round);
        }
        assert_eq!(
            probes,
            [6, 12],
            "the VLAN's NAS was dialled at rounds {probes:?}, not every \
             {PRIVATE_PROBE_EVERY}th"
        );

        // The probe of round 12 answered: dialled at every round from then on.
        contact.worked(nas, address, 1_000);
        for round in 13..20u32 {
            assert!(
                dialled(&contact, Some(false)),
                "round {round}: the NAS answered and was not dialled again"
            );
            contact.round_reached(&BTreeSet::from([nas]), start + ROUND * round);
        }
        // Until it fails: then back to the probes.
        contact.failed(nas, address);
        contact.round_reached(&BTreeSet::new(), start + ROUND * 20);
        assert!(
            !dialled(&contact, Some(false)),
            "an address that stopped answering was still dialled at every round"
        );
    }

    /// The fall-back to two minutes is somebody else's word (red team,
    /// 2026-10-23): a machine of the account answering one round in two, and
    /// a coordinator listing a new port at every read, each put the wait back
    /// to [`REREAD_AFTER_MISS`] and held this node at a read every two minutes
    /// -- 720 a day at a round every 30 s. Floored to one fall-back per
    /// [`RESET_FLOOR`], the doubling wins: never more than 432 a day at
    /// 30 s rounds (three reads per ten minutes: the fall-back's, two and
    /// six minutes after it), and no more than the 288 rounds of a day at
    /// the default five minutes.
    #[test]
    fn red_team_a_machine_toggling_every_round_cannot_hold_the_wait_at_two_minutes() {
        let day = Duration::from_secs(24 * 3600);
        let laptop = device(2);
        for (every, most) in [(Duration::from_secs(30), 432), (ROUND, 288)] {
            let mut contact = Contact::new();
            let start = Instant::now();
            contact.published(Some(HOME), start);
            contact.read(&[(laptop, "192.168.1.2:9000".to_owned())], &[], &[], start);
            let rounds = u32::try_from(day.as_secs() / every.as_secs()).expect("rounds");
            let reads = (1..rounds)
                .filter(|&round| {
                    // An answer one round in two at its address, and a new
                    // port beside it at every read.
                    let home = "192.168.1.2:9000";
                    let listed = [
                        (laptop, home.to_owned()),
                        (laptop, format!("192.168.1.2:{}", 9000 + round)),
                    ];
                    let live: &[&str] = if round % 2 == 0 { &[home] } else { &[] };
                    round_against(&mut contact, &listed, true, live, start + every * round).0
                })
                .count();
            assert!(
                reads <= most,
                "at a round every {every:?}, a machine toggling every round cost {reads} reads \
                 in a day; the floor allows at most {most}"
            );
        }
    }

    /// The coordinator chooses which presences it lists -- stale ones, or
    /// unsigned ones from an older coordinator -- so it decides what arrives
    /// in this table. It must not decide its size.
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
        contact.read(&flood, &[], &[], now);
        assert_eq!(
            contact.book.len(),
            MAX_DEVICES,
            "one answer filled the table"
        );

        let one = flood[0].0;
        let addresses: Vec<(DeviceId, String)> = (0..1000)
            .map(|n| (one, format!("10.1.{}.{}:9797", n >> 8, n & 0xff)))
            .collect();
        contact.read(&addresses, &[], &[], now);
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
        contact.read(&[(pi, "192.168.1.20:9797".to_owned())], &[], &[], start);
        contact.worked(pi, "192.168.1.20:9797", 1_000);

        let stale: Vec<(DeviceId, String)> = (0..20)
            .map(|n| (pi, format!("203.0.113.{n}:9797")))
            .collect();
        contact.read(&stale, &[], &[], start + ROUND);

        let order = contact.candidates();
        assert_eq!(
            order.first().map(|(_, address)| address.as_str()),
            Some("192.168.1.20:9797"),
            "the address that answered was pushed down by addresses that never did"
        );
        assert!(order.len() <= MAX_ADDRESSES, "the replay was kept in full");
    }

    /// A coordinator that pretends to be older than `SignedPeers` hangs up on
    /// it, and a client that then asks `Peers` reads a list the coordinator can
    /// forge at will: the signature check would be advisory against the one
    /// party it exists to check. Once a coordinator has signed in front of this
    /// process, that fallback is closed for good.
    #[test]
    fn red_team_a_coordinator_that_has_signed_cannot_talk_this_node_down_to_an_unsigned_list() {
        let mut contact = Contact::new();
        let start = Instant::now();
        assert!(
            contact.due(Some(HOME), start).accept_unsigned,
            "a coordinator older than signed lists must still be readable"
        );

        contact.published(Some(HOME), start);
        contact.read(&listing(&[2]), &[], &[], start);
        contact.signed();
        for round in 1..=24u32 {
            assert!(
                !contact
                    .due(Some(HOME), start + PUBLISH_EVERY * round)
                    .accept_unsigned,
                "hour {round}: a coordinator that signed may now forge the list by hanging up"
            );
        }
    }

    fn signed_at(seed: u8, address: &str) -> SignedPresence {
        let keys = DeviceKeys::from_seed(&SecretBytes::new([seed; 32]));
        Presence {
            device: keys.device_id(),
            address: address.to_owned(),
            at_unix: 0,
        }
        .sign(&keys)
    }

    fn owner(seed: u8) -> UserKeys {
        UserKeys::derive(&MasterSecret::from_bytes([seed; 32]))
    }

    /// The account these tests' node belongs to.
    fn me() -> UserId {
        owner(1).user_id()
    }

    /// Device `seed`'s presence at `address`, with `account`'s claim on it.
    fn claimed_at(account: &UserKeys, seed: u8, address: &str) -> ClaimedPresence {
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

    /// What 2b.3 needs the book for: the pairs a peer may be handed, kept
    /// across a restart. An address that came without a claim, or unsigned,
    /// stays dialled and never leaves.
    #[test]
    fn claimed_addresses_are_relayable_across_a_restart_and_nothing_else_is() {
        let home = tempfile::tempdir().expect("temporary directory");
        let path = Contact::path(home.path());
        let pi = claimed_at(&owner(1), 2, "192.168.1.20:9797");
        let unclaimed = signed_at(3, "192.168.1.30:9797");
        let unsigned = (device(9), "192.168.1.99:9797".to_owned());

        let mut before = Contact::new();
        let presences = [pi.presence.clone(), unclaimed.clone()];
        let mut listed = found(&presences);
        listed.push(unsigned);
        before.read(
            &listed,
            &presences,
            std::slice::from_ref(&pi),
            Instant::now(),
        );
        assert_eq!(
            before.relayable(),
            vec![pi.clone()],
            "the book would hand a peer an address with no claim of this account on it"
        );
        before.save(&path).expect("save");

        let (after, warning) = Contact::load(&path, me());
        assert_eq!(warning, None, "a book this node wrote was not read cleanly");
        assert_eq!(
            after.relayable(),
            vec![pi],
            "the claim was lost across a restart, so a rebooted machine has nothing to relay"
        );
    }

    /// A machine re-publishes the same address every hour with a new date.
    /// The book keeps the latest signature, not the first: the one it holds
    /// is the one a peer is handed, and a receiver has only the date to tell
    /// a fresh presence from a replayed one.
    #[test]
    fn the_same_address_signed_again_later_keeps_the_later_signature() {
        let keys = DeviceKeys::from_seed(&SecretBytes::new([2; 32]));
        let at = |at_unix| {
            Presence {
                device: keys.device_id(),
                address: "192.168.1.20:9797".to_owned(),
                at_unix,
            }
            .sign(&keys)
        };
        let mut contact = Contact::new();
        for signed in [at(100), at(200), at(150)] {
            contact.read(
                &found(std::slice::from_ref(&signed)),
                std::slice::from_ref(&signed),
                &[],
                Instant::now(),
            );
        }
        let held = contact.book[&keys.device_id()][0]
            .presence
            .as_ref()
            .map(|signed| signed.presence.at_unix);
        assert_eq!(
            held,
            Some(200),
            "the book kept an older signature, which a peer would be handed as if current"
        );
    }

    /// The file is editable by anything running as this user. A claim in it
    /// is checked as the wire's is: another account's genuine claim, put on
    /// one of the listed machines, would otherwise be handed to every peer of
    /// this account as one of its own.
    #[test]
    fn red_team_an_address_book_edited_to_hold_another_accounts_claim_does_not_relay_it() {
        let home = tempfile::tempdir().expect("temporary directory");
        let path = Contact::path(home.path());
        let theirs = claimed_at(&owner(2), 2, "203.0.113.67:9797");
        let file = BookFile {
            version: BOOK_VERSION,
            signs: true,
            entries: vec![BookEntry {
                presence: theirs.presence.clone(),
                worked: None,
                claim: Some(theirs.claim),
            }],
        };
        std::fs::write(&path, postcard::to_stdvec(&file).expect("encode")).expect("write");

        let (contact, warning) = Contact::load(&path, me());
        assert!(
            contact.relayable().is_empty(),
            "another account's machine would be relayed as one of this account's"
        );
        assert!(
            warning.is_some_and(|why| why.contains("1 claims")),
            "a tampered claim was dropped without a word"
        );
    }

    /// The book's format changed under running machines. Read as empty, the
    /// old file would forget that the coordinator signs, and the first round
    /// after the upgrade would accept an unsigned list again.
    #[test]
    fn red_team_the_upgrade_does_not_reopen_the_downgrade() {
        let home = tempfile::tempdir().expect("temporary directory");
        let path = Contact::path(home.path());
        let pi = signed_at(2, "192.168.1.20:9797");
        let old = BookFileV1 {
            version: 1,
            signs: true,
            entries: vec![BookEntryV1 {
                presence: pi.clone(),
                worked: Some(5),
            }],
        };
        std::fs::write(&path, postcard::to_stdvec(&old).expect("encode")).expect("write");

        let (contact, warning) = Contact::load(&path, me());
        assert_eq!(warning, None, "a version-1 book was refused");
        assert!(
            !contact.due(Some(HOME), Instant::now()).accept_unsigned,
            "the upgrade forgot that the coordinator signs: an unsigned list is \
             accepted again on the first round"
        );
        assert_eq!(
            contact.candidates(),
            found(&[pi]),
            "the old addresses were lost"
        );
    }

    fn found(presences: &[SignedPresence]) -> Vec<(DeviceId, String)> {
        presences
            .iter()
            .map(|signed| (signed.presence.device, signed.presence.address.clone()))
            .collect()
    }

    /// 2b.1 closed the fallback to an unsigned list once the coordinator had
    /// signed in front of the process, and left it open at every start: a
    /// hostile coordinator only had to wait for a reboot and hang up. The
    /// memory now lives in the address book.
    #[test]
    fn red_team_a_restart_does_not_reopen_the_downgrade() {
        let home = tempfile::tempdir().expect("temporary directory");
        let path = Contact::path(home.path());
        let pi = signed_at(2, "192.168.1.20:9797");

        let mut before = Contact::new();
        before.read(
            &found(std::slice::from_ref(&pi)),
            std::slice::from_ref(&pi),
            &[],
            Instant::now(),
        );
        before.signed();
        assert!(
            before.save(&path).expect("save"),
            "a changed book was not written"
        );

        let (after, warning) = Contact::load(&path, me());
        assert_eq!(warning, None, "a book this node wrote was not read cleanly");
        assert!(
            !after.due(Some(HOME), Instant::now()).accept_unsigned,
            "after a restart the first round would read an unsigned list from a \
             coordinator that has signed: the downgrade is back at every reboot"
        );
    }

    /// The file sits on disk, where anything running as this user can edit
    /// it, and its addresses are dialled before the coordinator is asked
    /// anything. Loading it goes through the same check as the wire.
    #[test]
    fn red_team_an_address_book_edited_to_hold_a_forged_presence_loses_it_on_load() {
        let home = tempfile::tempdir().expect("temporary directory");
        let path = Contact::path(home.path());
        let genuine = signed_at(2, "192.168.1.20:9797");
        let mut moved = signed_at(3, "192.168.1.30:9797");
        moved.presence.address = "203.0.113.66:9797".to_owned();

        let file = BookFile {
            version: BOOK_VERSION,
            signs: true,
            entries: vec![
                BookEntry {
                    presence: genuine.clone(),
                    worked: None,
                    claim: None,
                },
                BookEntry {
                    presence: moved,
                    worked: Some(u64::MAX),
                    claim: None,
                },
            ],
        };
        std::fs::write(&path, postcard::to_stdvec(&file).expect("encode")).expect("write");

        let (contact, warning) = Contact::load(&path, me());
        assert_eq!(
            contact.candidates(),
            found(&[genuine]),
            "an address its device never signed was loaded and would be dialled first"
        );
        assert!(
            warning.is_some_and(|why| why.contains("1 of 2")),
            "a tampered address book was loaded without a word"
        );
    }

    /// What the book is for: the address that answered is still dialled first
    /// after a restart, and an address an old coordinator handed out unsigned
    /// is dialled but never written, where the next load would have had to
    /// take it on trust.
    #[test]
    fn the_address_that_worked_is_still_first_after_a_restart() {
        let home = tempfile::tempdir().expect("temporary directory");
        let path = Contact::path(home.path());
        let public = signed_at(2, "203.0.113.20:9797");
        let home_lan = signed_at(2, "192.168.1.20:9797");
        let pi = public.presence.device;
        let unsigned = (device(9), "192.168.1.99:9797".to_owned());

        let mut before = Contact::new();
        let mut listed = found(&[public.clone(), home_lan.clone()]);
        listed.push(unsigned.clone());
        before.read(&listed, &[public, home_lan], &[], Instant::now());
        before.worked(pi, "192.168.1.20:9797", 1_000);
        assert!(
            before.candidates().contains(&unsigned),
            "an unsigned address was not dialled"
        );
        before.save(&path).expect("save");
        assert!(
            !before.save(&path).expect("save"),
            "an unchanged book was written again"
        );

        let (after, _) = Contact::load(&path, me());
        let order = after.candidates();
        assert_eq!(
            order.first().map(|(_, address)| address.as_str()),
            Some("192.168.1.20:9797"),
            "the address that worked lost its rank across a restart"
        );
        assert!(
            !order.contains(&unsigned),
            "an unsigned address was laundered through the file"
        );
    }

    /// Found by Rodin on 2026-09-29: a Pi with no real-time clock reads 1970
    /// until NTP answers. The address that just worked must still outrank one
    /// that worked yesterday by a clock that was right then.
    #[test]
    fn red_team_a_clock_back_in_1970_does_not_rank_a_stale_address_first() {
        let pi = device(2);
        let mut contact = Contact::new();
        contact.read(
            &[
                (pi, "203.0.113.20:9797".to_owned()),
                (pi, "192.168.1.20:9797".to_owned()),
            ],
            &[],
            &[],
            Instant::now(),
        );
        contact.worked(pi, "203.0.113.20:9797", 1_700_000_000);
        contact.worked(pi, "192.168.1.20:9797", 40);
        assert_eq!(
            contact
                .candidates()
                .first()
                .map(|(_, address)| address.as_str()),
            Some("192.168.1.20:9797"),
            "a success recorded before NTP ranked below yesterday's address, which \
             now costs a connect timeout every round"
        );
    }

    #[test]
    fn a_damaged_address_book_is_an_empty_one_not_a_failed_start() {
        let home = tempfile::tempdir().expect("temporary directory");
        let path = Contact::path(home.path());
        std::fs::write(&path, [0xff; 3]).expect("write");
        let (contact, warning) = Contact::load(&path, me());
        assert_eq!(
            contact.candidates(),
            [] as [(itsanas_crypto::DeviceId, std::string::String); 0]
        );
        assert!(
            warning.is_some(),
            "a damaged book was dropped without a word"
        );
        assert_eq!(
            Contact::load(&home.path().join("absent"), me()).1,
            None,
            "a first start is not a warning"
        );
    }

    #[test]
    fn a_device_no_longer_listed_is_forgotten() {
        let mut contact = Contact::new();
        let now = Instant::now();
        contact.read(&listing(&[2, 3]), &[], &[], now);
        contact.read(&listing(&[2]), &[], &[], now);
        let devices: BTreeSet<DeviceId> =
            contact.candidates().into_iter().map(|(d, _)| d).collect();
        assert_eq!(
            devices,
            [device(2)].into(),
            "a withdrawn device kept its place and would be dialled every round"
        );
    }

    /// Device `seed` at `address` on its own clock `at`, with `account`'s
    /// claim issued at `issued`.
    fn dated(account: &UserKeys, seed: u8, address: &str, at: u64, issued: u64) -> ClaimedPresence {
        let keys = DeviceKeys::from_seed(&SecretBytes::new([seed; 32]));
        ClaimedPresence {
            presence: Presence {
                device: keys.device_id(),
                address: address.to_owned(),
                at_unix: at,
            }
            .sign(&keys),
            claim: NodeClaim {
                owner: account.user_id(),
                device: keys.device_id(),
                pledged_bytes: 0,
                issued_unix: issued,
                revoked: false,
            }
            .sign(account),
        }
    }

    fn encoded(rows: &[ClaimedPresence]) -> Vec<Vec<u8>> {
        rows.iter()
            .map(|row| postcard::to_stdvec(row).expect("encode"))
            .collect()
    }

    /// A book that got `rows` from the coordinator.
    fn book_of(rows: &[ClaimedPresence]) -> Contact {
        let presences: Vec<_> = rows.iter().map(|row| row.presence.clone()).collect();
        let mut contact = Contact::new();
        contact.read(&found(&presences), &presences, rows, Instant::now());
        contact
    }

    fn this_machine() -> DeviceId {
        device(0xEE)
    }

    const AWAY: &str = "203.0.113.7:9797";
    /// "Now" for the relay tests: far past every date they sign.
    const NOW: u64 = 1_000_000;

    /// The point of the step: the Pi moved while the coordinator was down,
    /// the laptop reached it since, and tells this machine. The new address
    /// joins behind the one that worked, and is relayed onward.
    #[test]
    fn a_newer_address_relayed_by_a_machine_of_the_account_joins_the_book() {
        let pi = dated(&owner(1), 2, HOME, 100, 10);
        let mut contact = book_of(std::slice::from_ref(&pi));
        contact.worked(pi.presence.presence.device, HOME, 1000);

        let moved = dated(&owner(1), 2, AWAY, 200, 10);
        let outcome = contact.relayed(
            &encoded(std::slice::from_ref(&moved)),
            me(),
            this_machine(),
            NOW,
        );

        assert_eq!(
            outcome,
            Relayed {
                kept: 1,
                refused: 0
            }
        );
        let addresses: Vec<_> = contact.candidates().into_iter().map(|(_, a)| a).collect();
        assert_eq!(
            addresses,
            vec![HOME.to_owned(), AWAY.to_owned()],
            "a relayed address must join behind the one that worked, never ahead of it"
        );
        assert!(
            contact.relayable().contains(&moved),
            "a kept row is not passed on"
        );
    }

    /// Sabotage: skip `verify_for` in `relayed`.
    #[test]
    fn red_team_a_peer_cannot_hand_out_a_presence_it_forged() {
        let pi = dated(&owner(1), 2, HOME, 100, 10);
        let mut contact = book_of(std::slice::from_ref(&pi));
        let mut forged = dated(&owner(1), 2, AWAY, 200, 10);
        forged.presence.presence.address = "198.51.100.66:9797".to_owned();

        let outcome = contact.relayed(&encoded(&[forged]), me(), this_machine(), NOW);

        assert_eq!(
            outcome,
            Relayed {
                kept: 0,
                refused: 1
            }
        );
        assert_eq!(
            contact.candidates(),
            vec![(pi.presence.presence.device, HOME.to_owned())],
            "an address the device never signed reached the book: the relay chooses where this \
             machine dials"
        );
    }

    /// A relay holds genuine presences of other accounts' machines, each
    /// signed by its device and claimed by its owner. None of them is ours.
    /// Sabotage: skip `verify_for` in `relayed`.
    #[test]
    fn red_team_a_relay_cannot_pass_off_another_accounts_machine_as_ours() {
        let pi = dated(&owner(1), 2, HOME, 100, 10);
        let mut contact = book_of(std::slice::from_ref(&pi));
        // The same device, claimed by somebody else at a newer date: it would
        // pass every date check, and only the owner's signature refuses it.
        let theirs = dated(&owner(2), 2, AWAY, 200, 20);

        let outcome = contact.relayed(&encoded(&[theirs]), me(), this_machine(), NOW);

        assert_eq!(
            outcome,
            Relayed {
                kept: 0,
                refused: 1
            }
        );
        assert_eq!(
            contact.candidates().len(),
            1,
            "another account's claim was taken as ours"
        );
    }

    /// The owner withdrew the Pi; the coordinator stopped listing it, and the
    /// book dropped it. A relay still holds its old, unrevoked claim, which
    /// `verify_for` cannot date. Sabotage: let `relayed` take a device the
    /// book does not hold.
    #[test]
    fn red_team_a_relay_cannot_bring_back_a_machine_the_owner_withdrew() {
        let pi = dated(&owner(1), 2, HOME, 100, 10);
        let laptop = dated(&owner(1), 3, "192.168.1.30:9797", 100, 10);
        let mut contact = book_of(&[pi.clone(), laptop.clone()]);
        // The next read lists the laptop only: the Pi was withdrawn.
        contact.read(
            &found(std::slice::from_ref(&laptop.presence)),
            std::slice::from_ref(&laptop.presence),
            std::slice::from_ref(&laptop),
            Instant::now(),
        );

        let replayed = dated(&owner(1), 2, AWAY, 300, 10);
        let outcome = contact.relayed(&encoded(&[replayed]), me(), this_machine(), NOW);

        assert_eq!(
            outcome,
            Relayed {
                kept: 0,
                refused: 1
            }
        );
        assert!(
            contact
                .candidates()
                .iter()
                .all(|(d, _)| *d != pi.presence.presence.device),
            "a withdrawn machine is back in the book on a relay's word, and would be dialled \
             and relayed onward"
        );
    }

    /// A clock far ahead once must not blind the relay for good: the Pi's
    /// presence dated 2099 sits in the book, and a relayed presence dated now
    /// is kept all the same; and a row dated past the skew is refused.
    /// Sabotage: drop the horizon filter on the held presences.
    #[test]
    fn red_team_a_presence_from_a_clock_far_ahead_does_not_blind_the_relay() {
        const Y2099: u64 = 4_070_908_800;
        let pi = dated(&owner(1), 2, AWAY, Y2099, 10);
        let mut contact = book_of(std::slice::from_ref(&pi));

        let now_here = dated(&owner(1), 2, HOME, NOW - 5, 10);
        let outcome = contact.relayed(&encoded(&[now_here]), me(), this_machine(), NOW);
        assert_eq!(
            outcome,
            Relayed {
                kept: 1,
                refused: 0
            },
            "a presence dated 2099 in the book refused every honest one after it: the \
             relay stays blind to that machine for good"
        );

        let ahead = dated(&owner(1), 2, AWAY, NOW + MAX_CLOCK_SKEW + 1, 10);
        let outcome = contact.relayed(&encoded(&[ahead]), me(), this_machine(), NOW);
        assert_eq!(outcome.refused, 1, "a row dated past the skew was kept");
    }

    /// The Pi left `AWAY` for `HOME`. A relay replays its genuine, older
    /// presence at `AWAY`. Sabotage: drop the presence-date check.
    #[test]
    fn red_team_a_peer_cannot_strand_a_machine_at_an_address_it_left() {
        let pi = dated(&owner(1), 2, HOME, 200, 10);
        let mut contact = book_of(std::slice::from_ref(&pi));

        let stale = dated(&owner(1), 2, AWAY, 100, 10);
        let outcome = contact.relayed(&encoded(&[stale]), me(), this_machine(), NOW);

        assert_eq!(
            outcome,
            Relayed {
                kept: 0,
                refused: 1
            }
        );
        assert_eq!(
            contact.candidates(),
            vec![(pi.presence.presence.device, HOME.to_owned())],
            "an address the machine left was replayed into the book: a connect timeout every \
             round, and relayed onward"
        );
    }

    /// The owner re-issued the Pi's claim; a relay offers the older one.
    /// Sabotage: drop the claim-date check.
    #[test]
    fn red_team_a_claim_older_than_the_one_held_is_refused() {
        let pi = dated(&owner(1), 2, HOME, 100, 20);
        let mut contact = book_of(std::slice::from_ref(&pi));

        let older = dated(&owner(1), 2, AWAY, 200, 10);
        let outcome = contact.relayed(&encoded(&[older]), me(), this_machine(), NOW);

        assert_eq!(
            outcome,
            Relayed {
                kept: 0,
                refused: 1
            },
            "a claim the owner has replaced was taken back in"
        );
    }

    /// Sabotage: drop the `members` check in `SharedBoard::presences_for`.
    #[test]
    fn red_team_a_stranger_asking_for_the_accounts_presences_is_refused() {
        use itsanas_net::Relay;

        let pi = dated(&owner(1), 2, HOME, 100, 10);
        let laptop = dated(&owner(1), 3, "192.168.1.30:9797", 100, 10);
        let shared = SharedBoard::default();
        shared.replace(book_of(&[pi.clone(), laptop.clone()]).board());

        assert_eq!(
            shared.presences_for(device(0x77)),
            None,
            "a device that is not one of this account's machines was told where they are"
        );
        assert_eq!(
            shared.presences_for(laptop.presence.presence.device),
            Some(encoded(&[pi])),
            "a machine of the account should get the others, and not itself"
        );
    }
}
