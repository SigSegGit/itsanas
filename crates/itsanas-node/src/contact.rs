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

use itsanas_coord::claim::SignedPresence;
use itsanas_crypto::DeviceId;
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
}

/// The address book's version. A file of another version is read as empty.
const BOOK_VERSION: u32 = 1;

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
    /// Whether the coordinator has answered with a signed list since this
    /// process started. See [`Due::accept_unsigned`].
    signs: bool,
    book: BTreeMap<DeviceId, Vec<Candidate>>,
    /// Whether anything worth writing changed since the last [`Self::save`].
    changed: bool,
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
        Due {
            publish,
            read,
            accept_unsigned: !self.signs,
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
    /// into the book, which is what lets it be written to disk.
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
        now: Instant,
    ) {
        self.read_at = Some(now);
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
            self.insert(*device, address, presence, None);
        }
    }

    /// Add one candidate, within [`MAX_DEVICES`] and [`MAX_ADDRESSES`]. The one
    /// door into the book, for the wire and for the file alike.
    fn insert(
        &mut self,
        device: DeviceId,
        address: &str,
        presence: Option<SignedPresence>,
        worked: Option<u64>,
    ) {
        if !self.book.contains_key(&device) && self.book.len() >= MAX_DEVICES {
            return;
        }
        let candidates = self.book.entry(device).or_default();
        if let Some(known) = candidates.iter_mut().find(|known| known.address == address) {
            // The same address signed again, or signed at last: keep the proof.
            if known.presence.is_none() && presence.is_some() {
                known.presence = presence;
                self.changed = true;
            }
            return;
        }
        candidates.push(Candidate {
            address: address.to_owned(),
            worked,
            presence,
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
            self.changed = true;
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
    /// What this does not defend: somebody with this user's rights can also set
    /// `signs` false or mark a genuine but stale address as having worked. Such
    /// a person can rewrite the configuration too; the check is against an
    /// address nobody signed, not against the owner of the home directory.
    ///
    /// A missing file is a first start. An unreadable one is an empty book and
    /// a sentence in the second value, never a failed start: the book is a
    /// cache, and the coordinator refills it on the first round.
    #[must_use]
    pub fn load(path: &Path) -> (Self, Option<String>) {
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
        let file: BookFile = match postcard::from_bytes::<BookFile>(&bytes) {
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
        for entry in file.entries {
            if entry.presence.verify_origin().is_err() {
                forged += 1;
                continue;
            }
            let device = entry.presence.presence.device;
            let address = entry.presence.presence.address.clone();
            contact.insert(device, &address, Some(entry.presence), entry.worked);
        }
        contact.changed = false;
        let warning = (forged > 0).then(|| {
            format!(
                "{} held {forged} of {offered} addresses their device never signed; they were dropped",
                path.display()
            )
        });
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
    use itsanas_coord::claim::Presence;
    use itsanas_crypto::{DeviceKeys, SecretBytes};

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
                contact.read(&listing(&[2]), &[], now);
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
        contact.read(&listing(&[2, 3]), &[], start);

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
        contact.read(&[], &[], start);

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
        contact.read(&listing(&[2]), &[], start);

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
        contact.read(&listing(&[2, 3]), &[], hour);
        assert!(
            contact.candidates().iter().any(|(d, _)| *d == device(3)),
            "the read did not put the new machine in the book"
        );
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
        contact.read(&flood, &[], now);
        assert_eq!(
            contact.book.len(),
            MAX_DEVICES,
            "one answer filled the table"
        );

        let one = flood[0].0;
        let addresses: Vec<(DeviceId, String)> = (0..1000)
            .map(|n| (one, format!("10.1.{}.{}:9797", n >> 8, n & 0xff)))
            .collect();
        contact.read(&addresses, &[], now);
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
        contact.read(&[(pi, "192.168.1.20:9797".to_owned())], &[], start);
        contact.worked(pi, "192.168.1.20:9797", 1_000);

        let stale: Vec<(DeviceId, String)> = (0..20)
            .map(|n| (pi, format!("203.0.113.{n}:9797")))
            .collect();
        contact.read(&stale, &[], start + ROUND);

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
        contact.read(&listing(&[2]), &[], start);
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
            Instant::now(),
        );
        before.signed();
        assert!(
            before.save(&path).expect("save"),
            "a changed book was not written"
        );

        let (after, warning) = Contact::load(&path);
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
                },
                BookEntry {
                    presence: moved,
                    worked: Some(u64::MAX),
                },
            ],
        };
        std::fs::write(&path, postcard::to_stdvec(&file).expect("encode")).expect("write");

        let (contact, warning) = Contact::load(&path);
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
        before.read(&listed, &[public, home_lan], Instant::now());
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

        let (after, _) = Contact::load(&path);
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
        let (contact, warning) = Contact::load(&path);
        assert!(contact.candidates().is_empty());
        assert!(
            warning.is_some(),
            "a damaged book was dropped without a word"
        );
        assert_eq!(
            Contact::load(&home.path().join("absent")).1,
            None,
            "a first start is not a warning"
        );
    }

    #[test]
    fn a_device_no_longer_listed_is_forgotten() {
        let mut contact = Contact::new();
        let now = Instant::now();
        contact.read(&listing(&[2, 3]), &[], now);
        contact.read(&listing(&[2]), &[], now);
        let devices: BTreeSet<DeviceId> =
            contact.candidates().into_iter().map(|(d, _)| d).collect();
        assert_eq!(
            devices,
            [device(2)].into(),
            "a withdrawn device kept its place and would be dialled every round"
        );
    }
}
