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
        Board { members, rows }
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
}

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
