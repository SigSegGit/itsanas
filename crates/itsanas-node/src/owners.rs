//! How much a host stores for each account (§8 1c).
//!
//! # The hole this closes
//!
//! A host used to bound only itself: `would_exceed_pledge` stops it holding
//! more than its own pledge, and nothing limited what one account stored on
//! it. A rebuilt client that pledged nothing was served until every host was
//! full, and the 30/70 bargain held only on clients honest enough to apply it.
//!
//! # The rule, decided by Nicolas on 2026-10-04
//!
//! * A device presents its account's signed claim ([`SignedClaim`]) before
//!   storing. No claim, no storing: a client that leaves it out to escape the
//!   bound is told to update, like a genuinely old one.
//! * **Credit is immediate on the space offered**: an account may hold here up
//!   to `Split::DEFAULT.room_earned(pledged)`, the sum of the pledges in its
//!   devices' claims -- three sevenths, the 30/70 split.
//! * **An offer that is contradicted earns only what is proved**:
//!   `room_earned(proved)`, the same ratio applied to what the account's
//!   devices are recorded as holding for this host
//!   ([`Store::bytes_held_by`]), so cheating never earns more than playing
//!   straight. Contradicted means this host's own audit paused one of the
//!   account's devices.
//! * **A pledge this host tested and found short counts for what was proved**
//!   (§8 1c (i)): a device that refused this host's own chunks for a full
//!   pledge within `FULL_RETRY` adds `min(pledged, proved)` to its account's
//!   pledge instead of what it claimed. The push path is the test: it offers
//!   every device it dials what that device lacks.
//!
//! # Why a ceiling for the whole host (Rodin, on the plan; redteam, on the code)
//!
//! The claim is signed with the account key, and every node holds that key:
//! a cheater signs a pledge of a petabyte, and immediate credit renews with
//! every throwaway account. So the part of what an account holds here that
//! goes **beyond `room_earned(proved)`** -- credit on a promise alone -- is
//! shared: all accounts together hold at most [`UNPROVEN_SHARE`] of this
//! host's pledge that way. A newcomer stores at once inside that share, and
//! its own credit grows with what it really hosts for this host.
//!
//! The first version exempted an account outright once one of its devices had
//! passed one audit; the redteam agent showed that "host me one chunk once"
//! then opened the whole host to a petabyte claim. Proof is now counted in
//! bytes, never in a flag.
//!
//! # Withdrawals, from the coordinator (§8 1c (ii))
//!
//! Every node holds the account key, so a device its owner withdrew signs
//! itself a fresh live claim. Only the coordinator knows of the withdrawal,
//! and there it is final. So a device of another account stores here only
//! once this host has asked the coordinator about it ([`ClaimBook::due`],
//! [`ClaimBook::note`]) and been answered with a live claim, within
//! [`STANDING_FOR`]. **No answer means no storing** -- Nicolas, 2026-10-05,
//! asked whether a coordinator that is down should let devices through:
//! "Refuse". It matches the coordinator's place (it can refuse a member,
//! never admit one), and its cost is stated: a newcomer is asked about when
//! it presents (or waits for the round), every account but this host's own stops storing here
//! while the coordinator is unreachable for longer than [`STANDING_FOR`], and
//! a host with no coordinator configured, or one not pinned by its device id,
//! stores only for its own account.
//!
//! What this does not bound, stated: an account can keep a paused device from
//! presenting here, so the contradiction this host sees is the one its own
//! audits find among the devices that came.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use itsanas_coord::accounting::Split;
use itsanas_coord::claim::{MAX_DEVICES_PER_ACCOUNT, SignedClaim};
use itsanas_crypto::{DeviceId, UserId};
use itsanas_store::{Store, Vault};

/// The part of its pledge a host lends on promises alone, to every account
/// together: 3 parts in 10.
pub const UNPROVEN_SHARE: (u64, u64) = (3, 10);

/// Most devices whose claims a host remembers. When full, a device whose
/// account holds nothing here is forgotten to make room; when none is, the
/// newcomer is refused. Free keys cost nothing, so a book that never forgot
/// could be filled with empty claims and lock every honest newcomer out.
pub const MAX_CLAIMS: usize = 4096;

/// How long the total lent on promises is trusted between recounts. A store
/// adds its own share as it goes; the recount catches what was freed.
const RECOUNT_EVERY: Duration = Duration::from_secs(60);

/// The least time between two recounts forced by a refusal. A peer that
/// keeps offering past the share would otherwise make every refused offer
/// walk the whole book (redteam).
const RECOUNT_ON_REFUSAL: Duration = Duration::from_secs(1);

/// What a host answers an account past what its pledge earns.
pub const ACCOUNT_FULL: &str = "this account has stored here what its pledge earns";
/// What a host answers when the share lent on promises is used up.
pub const UNPROVEN_FULL: &str =
    "this host lends no more on promises alone: host something for it to earn more";
/// How long a coordinator's word that a device is live lets it store here.
/// [`ClaimBook::due`] asks again after half of it, ageing confirmations
/// first, so a coordinator that answers every round lets none lapse while
/// the round can reach them all (`coordinator::STANDING_PER_ROUND`, 768 an
/// hour at the default interval).
pub const STANDING_FOR: Duration = Duration::from_secs(2 * 60 * 60);

/// What a host answers a device the coordinator says was withdrawn.
pub const WITHDRAWN: &str = "the coordinator says this device was withdrawn by its account";
/// What a host answers a device of another account it has not confirmed
/// with the coordinator lately.
pub const UNCONFIRMED: &str = "this host has not confirmed this device with the coordinator \
     lately; it stores here once it has (the next round, if the coordinator answers)";

/// What a host answers an account whose offer its audits contradicted.
pub const CONTRADICTED: &str = "a device of this account failed this host's storage audits; \
     it stores here only what its devices have proved they hold for this host";

#[derive(Clone, Debug)]
struct Held {
    owner: UserId,
    pledged: u64,
    issued: u64,
    /// The claim as presented, for asking the coordinator about it.
    claim: Vec<u8>,
    standing: Standing,
    /// When it first presented here, which orders who is asked about first.
    presented: Instant,
    /// When the coordinator was last asked about it, and whether it answered
    /// that it holds nothing for it -- which backs the next question off.
    asked: Option<(Instant, bool)>,
}

/// What the coordinator last said about a device, as far as this host knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Standing {
    /// Never asked, or asked and not answered with a live claim.
    Unconfirmed,
    /// Answered with a live claim at this instant.
    Live(Instant),
    /// Answered with a withdrawal. Final, as it is on the coordinator.
    Withdrawn,
}

/// What one answer of the coordinator's means for one device
/// ([`ClaimBook::verdict`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The coordinator holds a live claim for it under its account.
    Live,
    /// The coordinator holds its withdrawal.
    Withdrawn,
    /// The coordinator holds nothing for it under its account, or answered
    /// with a claim that does not check out. Not live from now.
    Unenrolled,
    /// No answer: down, unreachable, or older than the question. What the
    /// device had stays until it lapses.
    NoAnswer,
}

#[derive(Debug, Default)]
struct Lent {
    total: u64,
    counted: Option<Instant>,
}

/// Asks the coordinator about one presented claim, there and then.
pub type Asker = Box<dyn Fn(&SignedClaim) -> Verdict + Send + Sync>;

/// Most questions [`ClaimBook::take`] puts to the coordinator a minute.
///
/// Asking on presentation is what lets a newcomer store on its first round
/// instead of the host's next one; bounding it is what keeps that from being
/// an amplifier: a device costs nothing to make, and without a bound every
/// connection of a flood of them would be a connection to the coordinator.
/// Past it a device waits for the host's round ([`ClaimBook::due`]).
pub const ASKS_PER_MINUTE: usize = 30;

/// Most inline questions in flight at once. Each holds a connection's thread
/// for a round trip to the coordinator, which is up to its I/O timeout when
/// it is slow; past this a device waits for the host's round (redteam).
pub const ASKING_AT_ONCE: usize = 4;

/// How long a device the coordinator said it holds nothing for waits before
/// it is asked about again. Device keys are free: without this, junk devices
/// presented once are asked about every round, for ever, ahead of real ones
/// (redteam).
pub const ASK_AGAIN_AFTER: Duration = Duration::from_secs(60 * 60);

/// The claims this host has checked, by device.
pub struct ClaimBook {
    claims: Mutex<BTreeMap<DeviceId, Held>>,
    lent: Mutex<Lent>,
    capacity: usize,
    /// This host's own account, whose devices are never asked about, and
    /// how to ask about the others when they present.
    asking: Option<(UserId, Asker)>,
    asked: Mutex<std::collections::VecDeque<Instant>>,
    in_flight: std::sync::atomic::AtomicUsize,
}

impl std::fmt::Debug for ClaimBook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaimBook")
            .field("claims", &self.lock().len())
            .field("capacity", &self.capacity)
            .field("asking", &self.asking.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for ClaimBook {
    fn default() -> Self {
        Self::with_capacity(MAX_CLAIMS)
    }
}

/// What one account has here, as the rule reads it.
struct Account {
    pledged: u64,
    proved: u64,
    contradicted: bool,
}

impl ClaimBook {
    /// An empty book of [`MAX_CLAIMS`] places.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty book of `capacity` places, for a test that has to fill one.
    #[must_use]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            claims: Mutex::new(BTreeMap::new()),
            lent: Mutex::new(Lent::default()),
            capacity,
            asking: None,
            asked: Mutex::new(std::collections::VecDeque::new()),
            in_flight: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Ask `asker` about a device of another account than `own` when it
    /// presents a claim this book has not confirmed, up to
    /// [`ASKS_PER_MINUTE`]. Without it a device waits for [`ClaimBook::due`].
    #[must_use]
    pub fn asking(mut self, own: UserId, asker: Asker) -> Self {
        self.asking = Some((own, asker));
        self
    }

    /// Whether one more question fits in the last minute's budget, counted
    /// if it does.
    fn may_ask(&self) -> bool {
        let mut asked = self
            .asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while asked
            .front()
            .is_some_and(|at| at.elapsed() >= Duration::from_secs(60))
        {
            asked.pop_front();
        }
        if asked.len() >= ASKS_PER_MINUTE {
            return false;
        }
        asked.push_back(Instant::now());
        true
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<DeviceId, Held>> {
        self.claims
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Take a claim `caller` presented.
    ///
    /// Checked for its signer, never its date: a Raspberry Pi with no
    /// real-time clock reads 1970 and would refuse every claim as from the
    /// future, which would make every peer "update required" (redteam). The
    /// date only orders a device's claims against each other.
    ///
    /// # Errors
    ///
    /// When it does not decode, is not signed by the account it names, names
    /// another device than the one TLS proved, is a withdrawal, would be a
    /// sixth device of its account, or the book is full of accounts that hold
    /// something here.
    pub fn take(&self, caller: DeviceId, bytes: &[u8], vault: &Vault) -> Result<(), String> {
        let signed: SignedClaim =
            postcard::from_bytes(bytes).map_err(|_| "not a claim".to_owned())?;
        signed.verify_origin().map_err(|error| error.to_string())?;
        let claim = &signed.claim;
        if claim.device != caller {
            return Err("the claim is for another device than the one connected".to_owned());
        }
        let mut book = self.lock();
        if claim.revoked {
            book.remove(&caller);
            return Err("this device was withdrawn by its account".to_owned());
        }
        let mut standing = Standing::Unconfirmed;
        let mut presented = Instant::now();
        let mut asked = None;
        match book.get(&caller) {
            Some(held) if held.owner != claim.owner => {
                return Err("this device is already claimed by another account".to_owned());
            }
            Some(held) if held.standing == Standing::Withdrawn => {
                return Err(WITHDRAWN.to_owned());
            }
            Some(held) if held.issued > claim.issued_unix => return Ok(()),
            Some(held) => {
                standing = held.standing;
                presented = held.presented;
                asked = held.asked;
            }
            None => {
                let siblings = book
                    .values()
                    .filter(|held| held.owner == claim.owner)
                    .count();
                if siblings >= MAX_DEVICES_PER_ACCOUNT {
                    return Err("this account already has its five devices here".to_owned());
                }
                if book.len() >= self.capacity {
                    let empty = book.iter().find_map(|(device, held)| {
                        vault
                            .held_bytes_for(held.owner)
                            .is_ok_and(|bytes| bytes == 0)
                            .then_some(*device)
                    });
                    match empty {
                        Some(device) => {
                            book.remove(&device);
                        }
                        None => return Err("this host remembers no more devices".to_owned()),
                    }
                }
            }
        }
        book.insert(
            caller,
            Held {
                owner: claim.owner,
                pledged: claim.pledged_bytes,
                issued: claim.issued_unix,
                claim: bytes.to_vec(),
                standing,
                presented,
                asked,
            },
        );
        drop(book);
        // Asked outside the lock: it is a round trip to the coordinator, and
        // every other connection's `admits` waits on this lock.
        if let Some((own, asker)) = &self.asking {
            // Asked again at half of `STANDING_FOR`, as `due` does, so a withdrawal
            // reaches this host within the hour whichever comes first.
            let confirmed =
                matches!(standing, Standing::Live(at) if at.elapsed() < STANDING_FOR / 2);
            // Only a device never asked about: the rest wait for the round,
            // which asks oldest first, so presenting again buys nothing.
            if claim.owner != *own && !confirmed && asked.is_none() && self.may_ask() {
                use std::sync::atomic::Ordering;
                if self.in_flight.fetch_add(1, Ordering::AcqRel) < ASKING_AT_ONCE {
                    let verdict = asker(&signed);
                    self.in_flight.fetch_sub(1, Ordering::AcqRel);
                    self.note(caller, verdict);
                } else {
                    self.in_flight.fetch_sub(1, Ordering::AcqRel);
                }
            }
        }
        Ok(())
    }

    /// Up to `at_most` devices of other accounts than `own` to ask the
    /// coordinator about, with the claim each presented, most pressing first.
    ///
    /// First, confirmations past half of [`STANDING_FOR`], oldest first:
    /// those are real devices about to be refused. Then devices never
    /// answered, by when they presented. Last, devices the coordinator said
    /// it holds nothing for, once [`ASK_AGAIN_AFTER`] has passed. A withdrawn
    /// device is never asked again. Never in id order: ids are free to grind,
    /// and a book read in id order let eight junk devices take every round's
    /// questions for ever (redteam).
    #[must_use]
    pub fn due(&self, own: UserId, at_most: usize) -> Vec<(DeviceId, Vec<u8>)> {
        let book = self.lock();
        let mut due: Vec<((u8, Instant), DeviceId)> = book
            .iter()
            .filter(|(_, held)| held.owner != own)
            .filter_map(|(device, held)| {
                let key = match (held.standing, held.asked) {
                    (Standing::Live(at), _) if at.elapsed() >= STANDING_FOR / 2 => (0, at),
                    (Standing::Unconfirmed, None | Some((_, false))) => (1, held.presented),
                    (Standing::Unconfirmed, Some((at, true)))
                        if at.elapsed() >= ASK_AGAIN_AFTER =>
                    {
                        (2, at)
                    }
                    // Withdrawn for good, confirmed lately, or disowned lately.
                    _ => return None,
                };
                Some((key, *device))
            })
            .collect();
        due.sort_unstable();
        due.into_iter()
            .take(at_most)
            .filter_map(|(_, device)| book.get(&device).map(|held| (device, held.claim.clone())))
            .collect()
    }

    /// What the coordinator answered about `device`, heard now.
    pub fn note(&self, device: DeviceId, verdict: Verdict) {
        self.note_at(device, verdict, Instant::now());
    }

    fn note_at(&self, device: DeviceId, verdict: Verdict, at: Instant) {
        let mut book = self.lock();
        let Some(held) = book.get_mut(&device) else {
            return;
        };
        held.asked = Some((at, verdict == Verdict::Unenrolled));
        held.standing = match (held.standing, verdict) {
            (Standing::Withdrawn, _) | (_, Verdict::Withdrawn) => Standing::Withdrawn,
            (_, Verdict::Live) => Standing::Live(at),
            (_, Verdict::Unenrolled) => Standing::Unconfirmed,
            (kept, Verdict::NoAnswer) => kept,
        };
    }

    /// Read the coordinator's answer to [`Request::Standing`] about the
    /// claim `presented`, checked: a claim it hands back must be signed by
    /// the account, for the device and the account asked about. Anything
    /// else is [`Verdict::Unenrolled`] -- a coordinator can refuse a member,
    /// never vouch for one.
    ///
    /// [`Request::Standing`]: itsanas_coord::protocol::Request::Standing
    #[must_use]
    pub fn verdict(presented: &SignedClaim, answer: Option<&SignedClaim>) -> Verdict {
        match answer {
            Some(held)
                if held.verify_origin().is_ok()
                    && held.claim.owner == presented.claim.owner
                    && held.claim.device == presented.claim.device =>
            {
                if held.claim.revoked {
                    Verdict::Withdrawn
                } else {
                    Verdict::Live
                }
            }
            _ => Verdict::Unenrolled,
        }
    }

    /// Whether `caller` may store `incoming` more bytes for `owner` on the
    /// host whose store, vault and pledge are given.
    ///
    /// # Errors
    ///
    /// The refusal to send back.
    pub fn admits(
        &self,
        caller: DeviceId,
        owner: UserId,
        incoming: u64,
        store: &Store,
        vault: &Vault,
        host_pledge: u64,
    ) -> Result<(), String> {
        let book = self.lock();
        let Some(held) = book.get(&caller) else {
            return Err(itsanas_net::UPDATE_REQUIRED.to_owned());
        };
        if held.owner != owner {
            return Err("this device's claim is for another account".to_owned());
        }
        // The host's own account: its devices replicate to each other, and
        // the bargain is between accounts.
        if owner == store.owner() {
            return Ok(());
        }
        match held.standing {
            Standing::Withdrawn => return Err(WITHDRAWN.to_owned()),
            Standing::Live(at) if at.elapsed() < STANDING_FOR => {}
            Standing::Live(_) | Standing::Unconfirmed => return Err(UNCONFIRMED.to_owned()),
        }

        let standing = standing(&book, owner, store)?;
        let earned_by_proof = Split::DEFAULT.room_earned(standing.proved);
        let allowance = if standing.contradicted {
            earned_by_proof
        } else {
            Split::DEFAULT.room_earned(standing.pledged)
        };
        let mine = vault
            .held_bytes_for(owner)
            .map_err(|error| error.to_string())?;
        let after = mine.saturating_add(incoming);
        if after > allowance {
            let why = if standing.contradicted {
                CONTRADICTED
            } else {
                ACCOUNT_FULL
            };
            return Err(format!(
                "{why}: {mine} of {allowance} bytes (pledged {}, proved {})",
                standing.pledged, standing.proved
            ));
        }

        // Only what goes beyond what proof earns is lent on a promise.
        let on_promise =
            after.saturating_sub(earned_by_proof) - mine.saturating_sub(earned_by_proof);
        if on_promise > 0 {
            let share = u64::try_from(
                u128::from(host_pledge) * u128::from(UNPROVEN_SHARE.0)
                    / u128::from(UNPROVEN_SHARE.1),
            )
            .unwrap_or(u64::MAX);
            let mut lent = self
                .lent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let stale = lent.counted.is_none_or(|at| at.elapsed() >= RECOUNT_EVERY);
            // Recounted when old, and always before refusing: the running
            // total only ever grows between recounts, so a stale one can only
            // refuse wrongly -- an account that proved more, or data freed --
            // and a refusal is the rare path -- at most once a second, so a
            // peer offering past the share cannot make each refusal a walk.
            let over = lent.total.saturating_add(on_promise) > share;
            let recent = lent
                .counted
                .is_some_and(|at| at.elapsed() < RECOUNT_ON_REFUSAL);
            if stale || (over && !recent) {
                lent.total = lent_on_promise(&book, store, vault)?;
                lent.counted = Some(Instant::now());
            }
            if lent.total.saturating_add(on_promise) > share {
                return Err(UNPROVEN_FULL.to_owned());
            }
            lent.total = lent.total.saturating_add(on_promise);
        }
        Ok(())
    }
}

/// One account's pledge, proof and contradiction, from its devices here.
fn standing(
    book: &BTreeMap<DeviceId, Held>,
    owner: UserId,
    store: &Store,
) -> Result<Account, String> {
    let mut standing = Account {
        pledged: 0,
        proved: 0,
        contradicted: false,
    };
    let now = now_unix();
    for (device, held) in book.iter().filter(|(_, held)| held.owner == owner) {
        let record = store
            .reliability(device)
            .map_err(|error| error.to_string())?;
        standing.contradicted |= record.paused;
        // Records this host wrote itself, for a device its audits have not
        // paused. Never a flag: one passed audit proves one chunk.
        let proved = if record.paused {
            0
        } else {
            store
                .bytes_held_by(device)
                .map_err(|error| error.to_string())?
        };
        standing.proved = standing.proved.saturating_add(proved);
        standing.pledged =
            standing
                .pledged
                .saturating_add(if refused_lately(store, device, now)? {
                    // Tested and found short (§8 1c (i)): it offers what it holds.
                    held.pledged.min(proved)
                } else {
                    held.pledged
                });
    }
    Ok(standing)
}

/// Whether `device` refused this host's own chunks for a full pledge within
/// [`FULL_RETRY`](itsanas_net::session::FULL_RETRY).
///
/// The one test a host can put a claim to. The claim is self-signed, so its
/// pledge says what the device would like to be credited with; the push path
/// offers the device this host's chunks every round it dials it and stamps a
/// `PledgeFull` refusal (`Store::note_peer_full`). A device so stamped has
/// been asked for room and said it has none: its pledge counts here for what
/// it proved it holds for this host, and no more. Not a sanction -- an honest
/// device full of other accounts' data earns at the same ratio, and its
/// siblings keep their pledges -- and not for ever: the push path probes
/// again after the same window, and a refusal older than it no longer counts.
///
/// What this cannot test, stated: a device this host never dials. One behind
/// a router presents its claim, stores, and is offered nothing; its credit is
/// still a promise, bounded only by the share lent on promises.
fn refused_lately(store: &Store, device: &DeviceId, now: u64) -> Result<bool, String> {
    Ok(store
        .peer_full_since(device)
        .map_err(|error| error.to_string())?
        .is_some_and(|since| since >= now.saturating_sub(itsanas_net::session::FULL_RETRY)))
}

/// The host's clock, as the push path stamps refusals with it. Zero on a
/// clock before 1970, which makes every refusal recent -- the cautious side.
fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// What this host holds, for every account in the book, beyond what its
/// proof earns: the credit outstanding on promises alone.
fn lent_on_promise(
    book: &BTreeMap<DeviceId, Held>,
    store: &Store,
    vault: &Vault,
) -> Result<u64, String> {
    let mut owners: Vec<UserId> = book.values().map(|held| held.owner).collect();
    owners.sort_unstable();
    owners.dedup();
    let mut total = 0u64;
    for owner in owners.into_iter().filter(|owner| *owner != store.owner()) {
        let proved = standing(book, owner, store)?.proved;
        let held = vault
            .held_bytes_for(owner)
            .map_err(|error| error.to_string())?;
        total = total.saturating_add(held.saturating_sub(Split::DEFAULT.room_earned(proved)));
    }
    Ok(total)
}

impl itsanas_net::Owners for ClaimBook {
    fn present(&self, caller: DeviceId, claim: &[u8], vault: &Vault) -> Result<(), String> {
        self.take(caller, claim, vault)
    }

    fn admits(
        &self,
        caller: DeviceId,
        owner: UserId,
        incoming: u64,
        store: &Store,
        vault: &Vault,
        host_pledge: u64,
    ) -> Result<(), String> {
        ClaimBook::admits(self, caller, owner, incoming, store, vault, host_pledge)
    }
}

#[cfg(test)]
mod tests {
    use itsanas_coord::claim::NodeClaim;
    use itsanas_crypto::{ChunkId, DeviceKeys, MasterSecret, SecretBytes, UserKeys};
    use itsanas_store::{ChunkerConfig, FAILURES_BEFORE_PAUSE};

    use super::*;

    const NOW: u64 = 1_800_000_000;
    /// The host's own pledge in these tests: the share lent on promises is 300.
    const HOST_PLEDGE: u64 = 1_000;

    struct Host {
        _dir: tempfile::TempDir,
        store: Store,
        vault: Vault,
        book: ClaimBook,
    }

    fn host() -> Host {
        host_of(ClaimBook::new())
    }

    fn host_of(book: ClaimBook) -> Host {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_for_testing(
            dir.path().join("store"),
            UserKeys::derive(&MasterSecret::from_bytes([0x01; 32])),
            DeviceKeys::from_seed(&SecretBytes::new([0x01; 32])),
            ChunkerConfig::default(),
        )
        .unwrap();
        let vault = Vault::open(dir.path().join("vault")).unwrap();
        Host {
            _dir: dir,
            store,
            vault,
            book,
        }
    }

    fn account(seed: u8) -> UserKeys {
        UserKeys::derive(&MasterSecret::from_bytes([seed; 32]))
    }

    fn device(seed: u8) -> DeviceId {
        DeviceKeys::from_seed(&SecretBytes::new([seed; 32])).device_id()
    }

    fn claim(account: &UserKeys, device_seed: u8, pledged: u64, revoked: bool) -> Vec<u8> {
        let signed = NodeClaim {
            owner: account.user_id(),
            device: device(device_seed),
            pledged_bytes: pledged,
            issued_unix: NOW,
            revoked,
        }
        .sign(account);
        postcard::to_stdvec(&signed).unwrap()
    }

    /// Present a claim and have the coordinator confirm it, as a round does.
    fn present(host: &Host, account: &UserKeys, seed: u8, pledged: u64) {
        presents(host, account, seed, pledged);
        host.book.note(device(seed), Verdict::Live);
    }

    /// Present a claim, with no word from the coordinator yet.
    fn presents(host: &Host, account: &UserKeys, seed: u8, pledged: u64) {
        host.book
            .take(
                device(seed),
                &claim(account, seed, pledged, false),
                &host.vault,
            )
            .unwrap();
    }

    fn signed(bytes: &[u8]) -> SignedClaim {
        postcard::from_bytes(bytes).unwrap()
    }

    /// Put `bytes` of `owner`'s data in the host's vault.
    fn holds(host: &Host, owner: UserId, tag: u8, bytes: usize) {
        host.vault
            .put_chunk(owner, &ChunkId::from_bytes([tag; 32]), &vec![tag; bytes])
            .unwrap();
    }

    fn admits(host: &Host, caller: DeviceId, owner: UserId, incoming: u64) -> Result<(), String> {
        host.book.admits(
            caller,
            owner,
            incoming,
            &host.store,
            &host.vault,
            HOST_PLEDGE,
        )
    }

    /// `device` hosts every chunk of a file of this host's, and answered an
    /// audit: what proof is. Returns about how many bytes that proves.
    fn hosts_for_the_host(host: &Host, device: DeviceId) -> u64 {
        host.store
            .write_file("mine.bin", &vec![7u8; 64 * 1024])
            .unwrap();
        let (chunks, _) = host.store.live_chunks_page(None, 1_000).unwrap();
        host.store.record_holders(&chunks, &device).unwrap();
        host.store.note_audit(&device, true).unwrap();
        host.store.bytes_held_by(&device).unwrap()
    }

    #[test]
    fn red_team_a_device_that_presents_no_claim_stores_nothing() {
        // What a rebuilt client does first: leave the claim out and claim to
        // be old. Sabotage: admit a caller the book does not know.
        let host = host();
        assert_eq!(
            admits(&host, device(0x11), account(0xA1).user_id(), 1),
            Err(itsanas_net::UPDATE_REQUIRED.to_owned()),
            "a device with no claim was let store: the bound is one omitted message away"
        );
    }

    #[test]
    fn red_team_a_claim_for_another_device_is_refused() {
        // A claim is public once presented; replaying a friend's claim from
        // one's own key must not borrow the friend's pledge. Sabotage: drop
        // the device check.
        let host = host();
        let alice = account(0xA1);
        let theirs = claim(&alice, 0x11, 7_000, false);
        assert!(host.book.take(device(0x22), &theirs, &host.vault).is_err());
        assert!(admits(&host, device(0x22), alice.user_id(), 1).is_err());
    }

    #[test]
    fn red_team_an_account_stores_at_most_three_sevenths_of_its_pledge() {
        // The 30/70 bargain, held by the host. Pledge 700 earns 300, inside
        // the share lent on promises. Sabotage: no allowance.
        let host = host();
        let alice = account(0xA1);
        present(&host, &alice, 0x11, 700);
        holds(&host, alice.user_id(), 1, 299);
        assert_eq!(admits(&host, device(0x11), alice.user_id(), 1), Ok(()));
        let past = admits(&host, device(0x11), alice.user_id(), 2);
        assert!(
            past.as_ref()
                .is_err_and(|why| why.starts_with(ACCOUNT_FULL)),
            "an account stored past what its pledge earns: {past:?}"
        );
    }

    #[test]
    fn red_team_a_giant_self_signed_claim_takes_at_most_the_share_lent_on_promises() {
        // Found by Rodin on the plan: every node holds its account key, so a
        // petabyte pledge costs nothing to sign. Sabotage: skip the share.
        let host = host();
        let mallory = account(0xEE);
        present(&host, &mallory, 0x66, 1 << 50);
        holds(&host, mallory.user_id(), 1, 300);
        assert_eq!(
            admits(&host, device(0x66), mallory.user_id(), 1),
            Err(UNPROVEN_FULL.to_owned()),
            "a petabyte claim nobody tested took more than the share lent on promises"
        );
    }

    #[test]
    fn red_team_one_passed_audit_does_not_open_the_host_to_a_giant_claim() {
        // Found by the redteam agent on the first version, which exempted an
        // account from the share once a device had passed one audit: "host
        // me one chunk once" then let a petabyte claim take the host.
        // Sabotage: count a passed audit as proof, whatever it holds.
        let host = host();
        let mallory = account(0xEE);
        present(&host, &mallory, 0x66, 1 << 50);
        host.store.note_audit(&device(0x66), true).unwrap();
        holds(&host, mallory.user_id(), 1, 300);
        assert_eq!(
            admits(&host, device(0x66), mallory.user_id(), 1),
            Err(UNPROVEN_FULL.to_owned())
        );
    }

    #[test]
    fn red_team_throwaway_accounts_share_one_quota_lent_on_promises() {
        // Immediate credit renews with every new account. Two of them, 200
        // each, against a share of 300: the second is cut at 100.
        let host = host();
        let (one, two) = (account(0xE1), account(0xE2));
        present(&host, &one, 0x61, 7_000);
        present(&host, &two, 0x62, 7_000);
        holds(&host, one.user_id(), 1, 200);
        holds(&host, two.user_id(), 2, 100);
        assert!(
            admits(&host, device(0x62), two.user_id(), 1).is_err(),
            "a second throwaway account got a fresh share of its own"
        );
    }

    #[test]
    fn a_newcomer_stores_at_once_and_earns_more_by_hosting() {
        // Nicolas's rule: the space offered gives credit immediately, inside
        // the share; and what an account hosts for this host earns room on
        // its own, past the share, at the same ratio.
        let host = host();
        let alice = account(0xA1);
        present(&host, &alice, 0x11, 1 << 30);
        assert_eq!(admits(&host, device(0x11), alice.user_id(), 300), Ok(()));
        holds(&host, alice.user_id(), 1, 300);
        assert!(admits(&host, device(0x11), alice.user_id(), 1).is_err());
        let proved = hosts_for_the_host(&host, device(0x11));
        let earned = Split::DEFAULT.room_earned(proved);
        assert!(earned > 0, "fixture: the device proved nothing");
        // A refusal recounts at most once a second (`RECOUNT_ON_REFUSAL`).
        std::thread::sleep(RECOUNT_ON_REFUSAL + Duration::from_millis(50));
        assert_eq!(admits(&host, device(0x11), alice.user_id(), earned), Ok(()));
    }

    #[test]
    fn red_team_a_contradicted_account_keeps_only_what_it_proved() {
        // Nicolas's rule for a contradicted offer: room_earned(proved), the
        // same ratio, so cheating never earns more than playing straight.
        // Sabotage: ignore `paused`.
        let host = host();
        let alice = account(0xA1);
        present(&host, &alice, 0x11, 1 << 30);
        present(&host, &alice, 0x12, 1 << 30);
        let earned = Split::DEFAULT.room_earned(hosts_for_the_host(&host, device(0x11)));
        for _ in 0..FAILURES_BEFORE_PAUSE {
            host.store.note_audit(&device(0x12), false).unwrap();
        }
        assert_eq!(admits(&host, device(0x11), alice.user_id(), earned), Ok(()));
        let past = admits(&host, device(0x11), alice.user_id(), earned + 1);
        assert!(
            past.as_ref()
                .is_err_and(|why| why.starts_with(CONTRADICTED)),
            "a contradicted account stored past what it proved: {past:?}"
        );
    }

    #[test]
    fn red_team_a_device_paused_for_its_audits_proves_nothing() {
        // Host a lot, throw it away, fail the audits: the records the host
        // wrote before the pause are still on its ledger, because a failed
        // challenge withdraws only the chunk it asked about. Counting them as
        // proof would let the cheat keep the credit it was paused for.
        // Sabotage: count a paused device's records as proof.
        let host = host();
        let mallory = account(0xEE);
        present(&host, &mallory, 0x66, 1 << 30);
        let recorded = hosts_for_the_host(&host, device(0x66));
        assert!(
            recorded > 0,
            "fixture: the device was recorded as holding nothing"
        );
        for _ in 0..FAILURES_BEFORE_PAUSE {
            host.store.note_audit(&device(0x66), false).unwrap();
        }
        assert!(
            host.store.reliability(&device(0x66)).unwrap().paused,
            "fixture: the device is not paused"
        );
        let stored = admits(&host, device(0x66), mallory.user_id(), 1);
        assert!(
            stored
                .as_ref()
                .is_err_and(|why| why.starts_with(CONTRADICTED)),
            "a device paused for failing audits was still credited with what it no longer holds: {stored:?}"
        );
    }

    /// Now, as the host's clock reads it: what `note_peer_full` is stamped
    /// with on the push path.
    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn red_team_a_terabyte_claim_from_a_device_that_refused_this_host_loses_its_credit() {
        // §8 1c (i). The claim is self-signed, so its pledge is a promise this
        // host can test only one way: by offering the device its own chunks.
        // The push path does that every round it dials the device, and a
        // `PledgeFull` refusal is stamped (`note_peer_full`). A device that
        // claims a terabyte and refuses this host's data has been tested and
        // found to offer less than it said; it keeps only what it proved.
        // Sabotage: ignore the refusal.
        let host = host();
        let mallory = account(0xEE);
        present(&host, &mallory, 0x66, 1 << 40);
        assert_eq!(
            admits(&host, device(0x66), mallory.user_id(), 1),
            Ok(()),
            "fixture: an untested terabyte claim should get credit at once"
        );
        host.store.note_peer_full(&device(0x66), now()).unwrap();
        let refused = admits(&host, device(0x66), mallory.user_id(), 1);
        assert!(
            refused
                .as_ref()
                .is_err_and(|why| why.starts_with(ACCOUNT_FULL)),
            "a device that refused this host's chunks for a full pledge was still \
             credited with the terabyte it claimed: {refused:?}"
        );
    }

    #[test]
    fn a_device_that_refused_this_host_keeps_what_it_proved_and_its_siblings_pledges() {
        // Not a sanction: an honest device that is full of other accounts'
        // data is credited here with what it holds for this host, the same
        // ratio, and the account's other devices keep their own pledges. The
        // red-team test above must not pass on a book that refuses everything.
        let host = host();
        let alice = account(0xA1);
        present(&host, &alice, 0x11, 1 << 40);
        let earned = Split::DEFAULT.room_earned(hosts_for_the_host(&host, device(0x11)));
        assert!(earned > 0, "fixture: the device proved nothing");
        host.store.note_peer_full(&device(0x11), now()).unwrap();
        assert_eq!(admits(&host, device(0x11), alice.user_id(), earned), Ok(()));
        assert!(
            admits(&host, device(0x11), alice.user_id(), earned + 1).is_err(),
            "a full device was credited past what it proved"
        );
        present(&host, &alice, 0x12, 700);
        assert_eq!(
            admits(&host, device(0x12), alice.user_id(), earned + 300),
            Ok(()),
            "one full device took its siblings' pledges with it"
        );
    }

    #[test]
    fn a_refusal_older_than_the_retry_no_longer_counts() {
        // A device whose owner raised its pledge, or that freed room, is
        // probed again after `FULL_RETRY`; until it refuses again its claim
        // is what it was. Otherwise one full afternoon is a life sentence.
        let host = host();
        let alice = account(0xA1);
        present(&host, &alice, 0x11, 700);
        let long_ago = now() - itsanas_net::session::FULL_RETRY - 1;
        host.store.note_peer_full(&device(0x11), long_ago).unwrap();
        assert_eq!(admits(&host, device(0x11), alice.user_id(), 300), Ok(()));
    }

    #[test]
    fn red_team_a_device_the_coordinator_says_was_withdrawn_stores_nothing() {
        // §8 1c (ii). Every node holds the account key, so a withdrawn device
        // signs itself a fresh live claim; the coordinator's withdrawal is the
        // only word against it, and it is final. Sabotage: skip the verdict.
        let host = host();
        let alice = account(0xA1);
        present(&host, &alice, 0x11, 700);
        assert_eq!(admits(&host, device(0x11), alice.user_id(), 1), Ok(()));
        host.book.note(device(0x11), Verdict::Withdrawn);
        assert_eq!(
            admits(&host, device(0x11), alice.user_id(), 1),
            Err(WITHDRAWN.to_owned()),
            "a device the coordinator says was withdrawn still stored here"
        );
        let resigned = postcard::to_stdvec(
            &NodeClaim {
                owner: alice.user_id(),
                device: device(0x11),
                pledged_bytes: 700,
                issued_unix: NOW + 1_000,
                revoked: false,
            }
            .sign(&alice),
        )
        .unwrap();
        assert_eq!(
            host.book.take(device(0x11), &resigned, &host.vault),
            Err(WITHDRAWN.to_owned()),
            "a later live claim brought a withdrawn device back"
        );
        host.book.note(device(0x11), Verdict::Live);
        assert_eq!(
            admits(&host, device(0x11), alice.user_id(), 1),
            Err(WITHDRAWN.to_owned()),
            "a later answer undid a withdrawal, which is final"
        );
    }

    #[test]
    fn red_team_no_word_from_the_coordinator_means_no_storing() {
        // Nicolas, 2026-10-05: a device this host has not confirmed with the
        // coordinator stores nothing here, and neither does one whose
        // confirmation has lapsed while the coordinator does not answer.
        // Otherwise a withdrawn device stores wherever the coordinator is out
        // of reach. Sabotage: admit an unconfirmed device; never let a
        // confirmation lapse.
        let host = host();
        let alice = account(0xA1);
        presents(&host, &alice, 0x11, 700);
        assert_eq!(
            admits(&host, device(0x11), alice.user_id(), 1),
            Err(UNCONFIRMED.to_owned()),
            "a device nobody asked the coordinator about stored here"
        );
        let long_ago = Instant::now()
            .checked_sub(STANDING_FOR + Duration::from_secs(1))
            .expect("a monotonic clock older than two hours");
        host.book.note_at(device(0x11), Verdict::Live, long_ago);
        host.book.note(device(0x11), Verdict::NoAnswer);
        assert_eq!(
            admits(&host, device(0x11), alice.user_id(), 1),
            Err(UNCONFIRMED.to_owned()),
            "a confirmation outlived STANDING_FOR while the coordinator was silent"
        );
    }

    #[test]
    fn a_silent_coordinator_keeps_a_fresh_confirmation_and_an_empty_answer_ends_it() {
        let host = host();
        let alice = account(0xA1);
        present(&host, &alice, 0x11, 700);
        host.book.note(device(0x11), Verdict::NoAnswer);
        assert_eq!(admits(&host, device(0x11), alice.user_id(), 1), Ok(()));
        host.book.note(device(0x11), Verdict::Unenrolled);
        assert_eq!(
            admits(&host, device(0x11), alice.user_id(), 1),
            Err(UNCONFIRMED.to_owned())
        );
    }

    #[test]
    fn this_hosts_own_devices_need_no_word_from_the_coordinator() {
        // A household keeps replicating to itself with the coordinator down,
        // or with none configured: the bargain is between accounts.
        let host = host();
        let own = account(0x01);
        presents(&host, &own, 0x31, 0);
        assert_eq!(admits(&host, device(0x31), own.user_id(), 1 << 30), Ok(()));
        assert!(host.book.due(own.user_id(), 8).is_empty());
    }

    #[test]
    fn due_names_the_unconfirmed_and_the_ageing_and_never_the_withdrawn() {
        let host = host();
        let alice = account(0xA1);
        presents(&host, &alice, 0x11, 700);
        present(&host, &alice, 0x12, 700);
        presents(&host, &alice, 0x13, 700);
        host.book.note(device(0x13), Verdict::Withdrawn);
        let due: Vec<DeviceId> = host
            .book
            .due(host.store.owner(), 8)
            .into_iter()
            .map(|(device, _)| device)
            .collect();
        assert_eq!(due, vec![device(0x11)]);
        let ageing = Instant::now()
            .checked_sub(STANDING_FOR / 2)
            .expect("a monotonic clock older than an hour");
        host.book.note_at(device(0x12), Verdict::Live, ageing);
        assert_eq!(host.book.due(host.store.owner(), 8).len(), 2);
        assert_eq!(host.book.due(host.store.owner(), 1).len(), 1, "unbounded");
    }

    #[test]
    fn red_team_junk_devices_do_not_take_every_question_from_a_real_one() {
        // Found by the redteam agent: device keys are free, and the round
        // asked about the book in id order, eight a round. Twenty junk
        // devices the coordinator does not know, presented first, then one
        // real device: the round must reach the real one. Sabotage: no
        // back-off for a device answered `Unenrolled`.
        let host = host();
        for seed in 0x40..0x54u8 {
            presents(&host, &account(seed), seed, 1);
            host.book.note(device(seed), Verdict::Unenrolled);
        }
        let bob = account(0xB2);
        presents(&host, &bob, 0x22, 700);
        let due: Vec<DeviceId> = host
            .book
            .due(host.store.owner(), 8)
            .into_iter()
            .map(|(device, _)| device)
            .collect();
        assert_eq!(
            due,
            vec![device(0x22)],
            "junk the coordinator already disowned was asked about again ahead of a real device"
        );
    }

    #[test]
    fn red_team_a_flood_of_newcomers_does_not_let_a_real_confirmation_lapse() {
        // A confirmation past half its life is a real device about to be
        // refused; it goes before every never-asked newcomer, however many.
        // Sabotage: order `due` by id, or by presentation alone.
        let host = host();
        // The newcomers present first, so ordering by presentation alone
        // would put the real device last.
        for seed in 0x40..0x54u8 {
            presents(&host, &account(seed), seed, 1);
        }
        let bob = account(0xB2);
        present(&host, &bob, 0xF0, 700);
        let ageing = Instant::now()
            .checked_sub(STANDING_FOR / 2)
            .expect("a monotonic clock older than an hour");
        host.book.note_at(device(0xF0), Verdict::Live, ageing);
        let due = host.book.due(host.store.owner(), 1);
        assert_eq!(
            due.first().map(|(device, _)| *device),
            Some(device(0xF0)),
            "newcomers were asked about before a live device whose confirmation was ageing"
        );
    }

    #[test]
    fn red_team_a_coordinator_cannot_vouch_with_a_claim_for_another_device_or_account() {
        // The answer is checked like any claim: a coordinator that answers
        // with a live claim of another device, or a forged one, confirms
        // nothing. Sabotage: drop either check in `verdict`.
        let alice = account(0xA1);
        let presented = signed(&claim(&alice, 0x11, 700, false));
        let other_device = signed(&claim(&alice, 0x12, 700, false));
        let other_account = signed(&claim(&account(0xB2), 0x11, 700, false));
        let mut forged = signed(&claim(&alice, 0x11, 700, true));
        forged.claim.revoked = false;
        for (answer, what) in [
            (&other_device, "another device"),
            (&other_account, "another account"),
            (&forged, "a forged claim"),
        ] {
            assert_eq!(
                ClaimBook::verdict(&presented, Some(answer)),
                Verdict::Unenrolled,
                "a coordinator vouched with {what}"
            );
        }
        assert_eq!(
            ClaimBook::verdict(&presented, Some(&signed(&claim(&alice, 0x11, 1, true)))),
            Verdict::Withdrawn
        );
        assert_eq!(
            ClaimBook::verdict(&presented, Some(&presented)),
            Verdict::Live
        );
        assert_eq!(ClaimBook::verdict(&presented, None), Verdict::Unenrolled);
    }

    #[test]
    fn a_device_is_asked_about_when_it_presents_so_it_stores_on_its_first_round() {
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = asked.clone();
        let own = account(0x01).user_id();
        let book = ClaimBook::new().asking(
            own,
            Box::new(move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Verdict::Live
            }),
        );
        let host = host_of(book);
        let alice = account(0xA1);
        presents(&host, &alice, 0x11, 700);
        assert_eq!(admits(&host, device(0x11), alice.user_id(), 1), Ok(()));
        presents(&host, &alice, 0x11, 700);
        presents(&host, &account(0x01), 0x31, 0);
        assert_eq!(
            asked.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "asked again about a confirmed device, or about this host's own"
        );
    }

    #[test]
    fn red_team_presenting_again_does_not_buy_another_question() {
        // One junk device presenting on every connection would otherwise spend
        // the whole minute's budget on itself. Answered once, it waits for the
        // round's back-off. Sabotage: ask whenever it is not confirmed.
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = asked.clone();
        let book = ClaimBook::new().asking(
            account(0x01).user_id(),
            Box::new(move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Verdict::Unenrolled
            }),
        );
        let host = host_of(book);
        for _ in 0..10 {
            presents(&host, &account(0xEE), 0x66, 1);
        }
        assert_eq!(
            asked.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "a device the coordinator disowned bought a question with every presentation"
        );
    }

    #[test]
    fn red_team_a_flood_of_presentations_does_not_become_a_flood_of_questions() {
        // Device keys are free, so asking on every presentation would make
        // this host an amplifier against its coordinator. Sabotage: no budget.
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = asked.clone();
        let book = ClaimBook::new().asking(
            account(0x01).user_id(),
            Box::new(move |_| {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                Verdict::Unenrolled
            }),
        );
        let host = host_of(book);
        let flood = u8::try_from(ASKS_PER_MINUTE).expect("a small budget") + 20;
        for seed in 0..flood {
            presents(
                &host,
                &account(0x80u8.wrapping_add(seed)),
                0x80u8.wrapping_add(seed),
                1,
            );
        }
        assert_eq!(
            asked.load(std::sync::atomic::Ordering::Relaxed),
            ASKS_PER_MINUTE,
            "every presentation became a question to the coordinator"
        );
    }

    #[test]
    fn red_team_a_withdrawn_device_is_forgotten_and_a_sixth_is_refused() {
        let host = host();
        let alice = account(0xA1);
        present(&host, &alice, 0x11, 7_000);
        assert!(
            host.book
                .take(device(0x11), &claim(&alice, 0x11, 7_000, true), &host.vault)
                .is_err()
        );
        assert!(
            admits(&host, device(0x11), alice.user_id(), 1).is_err(),
            "a device its account withdrew still stores on the strength of its old claim"
        );
        for seed in 0x20..0x25u8 {
            present(&host, &alice, seed, 1);
        }
        assert!(
            host.book
                .take(device(0x25), &claim(&alice, 0x25, 1, false), &host.vault)
                .is_err(),
            "a sixth device added its pledge to the account's"
        );
    }

    #[test]
    fn red_team_a_device_cannot_store_under_another_accounts_name() {
        // Including the host's own: a stranger writing under the host's
        // owner id would take the exemption the host's own devices get.
        let host = host();
        present(&host, &account(0xA1), 0x11, 7_000);
        assert!(admits(&host, device(0x11), host.store.owner(), 1).is_err());
        assert!(admits(&host, device(0x11), account(0xB2).user_id(), 1).is_err());
    }

    #[test]
    fn red_team_empty_claims_cannot_lock_newcomers_out() {
        // Found by the redteam agent: free keys fill the book with claims
        // that store nothing, and every honest newcomer is told the host
        // remembers no more devices. Sabotage: refuse when full.
        let host = host_of(ClaimBook::with_capacity(5));
        let squatter = account(0xEE);
        for seed in 0x60..0x65u8 {
            present(&host, &squatter, seed, 1);
        }
        let alice = account(0xA1);
        assert_eq!(
            host.book.take(
                device(0x11),
                &claim(&alice, 0x11, 7_000, false),
                &host.vault
            ),
            Ok(()),
            "a book full of claims holding nothing refused an honest newcomer"
        );
    }

    #[test]
    fn a_claim_is_checked_for_its_signer_not_its_date() {
        // Found by the redteam agent: a host on a Pi with no clock reads 1970
        // and would refuse every claim as from the future. A claim dated far
        // ahead of any clock is still taken.
        let host = host();
        let alice = account(0xA1);
        let ahead = NodeClaim {
            owner: alice.user_id(),
            device: device(0x11),
            pledged_bytes: 7_000,
            issued_unix: u64::MAX / 2,
            revoked: false,
        }
        .sign(&alice);
        assert_eq!(
            host.book.take(
                device(0x11),
                &postcard::to_stdvec(&ahead).unwrap(),
                &host.vault
            ),
            Ok(())
        );
    }
}
