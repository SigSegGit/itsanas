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
//!
//! [`Store::bytes_held_by`]: itsanas_store::Store::bytes_held_by

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use itsanas_coord::claim::SignedClaim;
use itsanas_crypto::{DeviceId, UserId};

mod admission;
mod book;
mod service;

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

/// What a host answers an account past what its pledge earns. Defined in the
/// net crate, whose client and service both read it back.
pub use itsanas_net::service::ACCOUNT_FULL;
/// What a host answers when the share lent on promises is used up.
pub const UNPROVEN_FULL: &str =
    "this host lends no more on promises alone: host something for it to earn more";
/// How long a coordinator's word that a device is live lets it store here.
/// [`ClaimBook::due`] asks again after half of it, ageing confirmations
/// first, so a coordinator that answers every round lets none lapse while
/// the round can reach them all (`coordinator::STANDING_PER_ROUND`, 768 an
/// hour at the default interval).
pub const STANDING_FOR: Duration = Duration::from_secs(2 * 60 * 60);

/// How far a device's claim may be re-dated before the copy a host keeps
/// for its siblings is written again (`Owners::adds_nothing`), in seconds.
///
/// A device signs its claim afresh at every connection, so its date moves
/// every round and nothing else does. The kept copy is what siblings are
/// handed (`Vault::claims_for`), and they only order one device's claims by
/// date (`SignedClaim::supersedes`): a copy up to a quarter of an hour old
/// orders the same, as the contact stamp's `SEEN_STEP` does for its readers.
pub const KEEP_CLAIM_STEP: u64 = 15 * 60;

// What a host answers a device that was withdrawn, unconfirmed, or
// contradicted by its audits. The texts live in `itsanas-net`, beside
// `ACCOUNT_FULL`, so the owner's side matches them exactly and says the real
// reason instead of "a segment that does not verify".
pub use itsanas_net::{CONTRADICTED, UNCONFIRMED, WITHDRAWN};

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

#[cfg(test)]
mod tests {
    use itsanas_coord::claim::NodeClaim;
    use itsanas_crypto::{ChunkId, DeviceKeys, MasterSecret, SecretBytes, UserKeys};
    use itsanas_store::{ChunkerConfig, Store, Vault};

    use super::*;

    pub(super) const NOW: u64 = 1_800_000_000;
    /// The host's own pledge in these tests: the share lent on promises is 300.
    pub(super) const HOST_PLEDGE: u64 = 1_000;

    pub(super) struct Host {
        pub(super) _dir: tempfile::TempDir,
        pub(super) store: Store,
        pub(super) vault: Vault,
        pub(super) book: ClaimBook,
    }

    pub(super) fn host() -> Host {
        host_of(ClaimBook::new())
    }

    pub(super) fn host_of(book: ClaimBook) -> Host {
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

    pub(super) fn account(seed: u8) -> UserKeys {
        UserKeys::derive(&MasterSecret::from_bytes([seed; 32]))
    }

    pub(super) fn device(seed: u8) -> DeviceId {
        DeviceKeys::from_seed(&SecretBytes::new([seed; 32])).device_id()
    }

    pub(super) fn claim(
        account: &UserKeys,
        device_seed: u8,
        pledged: u64,
        revoked: bool,
    ) -> Vec<u8> {
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
    pub(super) fn present(host: &Host, account: &UserKeys, seed: u8, pledged: u64) {
        presents(host, account, seed, pledged);
        host.book.note(device(seed), Verdict::Live);
    }

    /// Present a claim, with no word from the coordinator yet.
    pub(super) fn presents(host: &Host, account: &UserKeys, seed: u8, pledged: u64) {
        host.book
            .take(
                device(seed),
                &claim(account, seed, pledged, false),
                &host.vault,
            )
            .unwrap();
    }

    pub(super) fn signed(bytes: &[u8]) -> SignedClaim {
        postcard::from_bytes(bytes).unwrap()
    }

    /// Put `bytes` of `owner`'s data in the host's vault.
    pub(super) fn holds(host: &Host, owner: UserId, tag: u8, bytes: usize) {
        host.vault
            .put_chunk(owner, &ChunkId::from_bytes([tag; 32]), &vec![tag; bytes])
            .unwrap();
    }

    pub(super) fn admits(
        host: &Host,
        caller: DeviceId,
        owner: UserId,
        incoming: u64,
    ) -> Result<(), String> {
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
    pub(super) fn hosts_for_the_host(host: &Host, device: DeviceId) -> u64 {
        host.store
            .write_file("mine.bin", &vec![7u8; 64 * 1024])
            .unwrap();
        let (chunks, _) = host.store.live_chunks_page(None, 1_000).unwrap();
        host.store.record_holders(&chunks, &device).unwrap();
        host.store.note_audit(&device, true).unwrap();
        host.store.bytes_held_by(&device).unwrap()
    }
}
