//! A full sync round against one peer.
//!
//! This is where the pieces meet: the merge rules from [`itsanas_sync`], the
//! local state from [`itsanas_store`], and a peer on the other end of a socket.
//!
//! A round is deliberately two independent halves:
//!
//! * **Push** — offer this device's segments and any chunks the peer lacks.
//! * **Pull** — fetch what the peer has from *other* devices and merge it.
//!
//! Either half can fail without poisoning the other, and neither is required
//! for the other to be useful. A device with nothing new still pulls; a device
//! whose peer is a pure host still pushes.
//!
//! # Why the pull half writes to the vault as well as the store
//!
//! Segments fetched from a peer are put in this node's vault before being
//! applied. That is not bookkeeping — it is what makes relaying work. Once the
//! laptop holds the Pi's segments, the laptop can serve them to the VM, and the
//! Pi never has to be online at the same time as the VM. It also gives the pull
//! a natural resume point: "everything after what my vault already holds",
//! which costs nothing to track and is correct after a crash.

use std::cell::RefCell;

use itsanas_crypto::{ChunkId, UserId};
use itsanas_store::{Store, Vault};
use itsanas_sync::{ChunkSource, SyncReport};

use crate::{
    error::Result,
    transport::{Offer, PeerClient, Refusal},
};

/// What one push half did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PushReport {
    /// This peer holds a segment signed by this machine's own key that forks
    /// from this machine's chain: another disk holds the same identity. The
    /// store now writes nothing (`Store::note_twin`).
    pub twin: bool,
    pub segments_offered: usize,
    pub segments_accepted: usize,
    pub chunks_offered: usize,
    pub chunks_accepted: usize,
    pub bytes_sent: u64,
    /// Whether bulk content was withheld because this peer keeps failing audits.
    ///
    /// The log was still offered, and so was a single chunk — the probe that
    /// gives the peer something to prove itself on. Nothing was deleted and
    /// nothing is blocked.
    pub withheld: bool,
    /// The chunk offered as a probe and accepted, when this peer was paused.
    ///
    /// `None` with `withheld` set means the peer would not take even the one
    /// chunk it was offered, so there is nothing to ask it about next round.
    pub probe: Option<ChunkId>,
    /// Chunks this peer is now known to hold, whether just sent or already had.
    ///
    /// Counted separately from `chunks_accepted` because the two answer
    /// different questions: how much work this round did, and how much of this
    /// node's data now exists somewhere other than this disk.
    pub holders_recorded: usize,
    /// How many chunk ids this round put on the wire to ask "have you got
    /// these?".
    ///
    /// The number the reconciliation exists to keep at zero. It used to be the
    /// whole account, every round, per peer: a two-thousandth of the account in
    /// bytes, or a hundred and forty gigabytes a day for a terabyte. Reported
    /// so a test can hold it to zero, because an optimisation nothing measures
    /// is an optimisation nobody notices losing.
    pub chunks_asked_about: usize,
    /// Holder records re-stamped without the wire, because the peer's summary
    /// of their bucket agreed with this device's on a due walk (§8 4c).
    ///
    /// Counted apart from [`Self::holders_recorded`], which is what the peer
    /// said: this is what the summary implied, and a test that wants to know
    /// which of the two kept a record fresh has to be able to tell.
    pub holders_restamped: usize,
    /// Offers the peer refused, chunks and segments together.
    ///
    /// Counted apart from "not accepted", which also covers a segment the peer
    /// already had. A host with pledge 0 refuses everything, its own account's
    /// log included, and the round used to print `sent 0 B, 0 segments` -- the
    /// line for "there was nothing to send". The acceptance bench failed E, F
    /// and G that way until every node pledged, and nothing said why.
    pub refused: usize,
    /// Why the first refusal happened, if there was one.
    pub refusal: Option<Refusal>,
    /// Copies this push told a full host it may drop (§E), each one this
    /// device holds and placement keeps with margin elsewhere.
    pub released: usize,
}

impl PushReport {
    /// Count one offer's answer, and say whether the peer took it.
    fn record(&mut self, offer: Offer) -> bool {
        match offer {
            Offer::Taken => true,
            Offer::AlreadyHeld => false,
            Offer::Refused(why) => {
                self.refused += 1;
                self.refusal.get_or_insert(why);
                false
            }
        }
    }
}

/// What a whole round did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RoundReport {
    pub push: PushReport,
    pub pull: SyncReport,
}

impl RoundReport {
    /// Whether the peer did something only a real host could do.
    ///
    /// **Not the same as "the round succeeded".** Completing a mutually
    /// authenticated handshake proves possession of a device key, and a device
    /// key is a free keypair anybody can mint a second before dialling. So a
    /// successful connection is an *identification*, never a credential.
    ///
    /// What this asks instead is whether the peer put itself to some cost on
    /// our behalf: it accepted data, or it holds data of ours it did not have
    /// to keep, or it served us work from one of our other devices. Any of
    /// those is expensive to fake at scale, because faking it means actually
    /// storing the data — at which point the peer is a real host and the
    /// distinction has stopped mattering.
    ///
    /// Callers use it to decide who deserves a place that a stranger cannot
    /// take. Getting this wrong turns an anti-flood measure into the flood's
    /// best tool: see `docs/TESTING.md`, `itsanas-cli` red-team tests.
    #[must_use]
    pub const fn peer_earned_trust(&self) -> bool {
        self.push.chunks_accepted > 0
            || self.push.segments_accepted > 0
            || self.push.holders_recorded > 0
            || self.pull.adopted > 0
    }

    /// Whether this round moved anything at all.
    #[must_use]
    pub const fn changed_anything(&self) -> bool {
        self.push.segments_accepted > 0
            || self.push.chunks_accepted > 0
            || self.pull.changed_anything()
    }
}

/// Fetches chunks from a peer on demand.
///
/// [`ChunkSource`] takes `&self` because the merge engine holds it immutably
/// while walking operations; the socket underneath needs `&mut`. The `RefCell`
/// bridges the two. It cannot deadlock: the engine never calls back into itself
/// while a fetch is outstanding, so the borrow is only ever held across one
/// request.
struct RemoteChunks<'a> {
    client: RefCell<&'a mut PeerClient>,
    /// Chunks this peer actually handed over.
    ///
    /// Recorded in the placement ledger afterwards, because a chunk a peer
    /// *served* is better evidence than one it merely claimed in the
    /// have/missing exchange: it produced the bytes.
    ///
    /// Without this a device restored from a passphrase downloads its whole
    /// corpus from a host and then believes not one copy of it exists anywhere
    /// but on its own disk — so `itsanas status` reports every chunk as
    /// unreplicated, and `under_replicated` calls the entire store critical,
    /// on the one day a user most needs to be told the truth.
    served: RefCell<Vec<ChunkId>>,
}

impl ChunkSource for RemoteChunks<'_> {
    fn fetch(&self, owner: UserId, address: &ChunkId) -> itsanas_sync::Result<Option<Vec<u8>>> {
        let fetched = self
            .client
            .borrow_mut()
            .chunk(owner, *address)
            // A peer that fails mid-fetch is a transport problem, not a merge
            // problem. Reporting it as "absent" would let a broken connection
            // masquerade as a device being asleep, and the operation would be
            // deferred forever instead of surfacing the fault.
            .map_err(|error| itsanas_sync::SyncError::Source(error.to_string()))?;

        if fetched.is_some() {
            self.served.borrow_mut().push(*address);
        }
        Ok(fetched)
    }
}

/// How much of a round to do.
///
/// A phone on mobile data, and a laptop tethered to one, both want to know what
/// changed without paying to download it. Exchanging signed log segments is
/// kilobytes; fetching the changes themselves is megabytes.
///
/// The deferred path this relies on was not added for it: applying an operation
/// whose chunks are unavailable already leaves local state untouched and asks
/// to be retried, because that is what a peer being asleep looks like. Choosing
/// not to fetch is indistinguishable from being unable to, which is why this
/// costs almost no new code and no new failure mode.
///
/// `itsanas-policy` decides which one to use and why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Segments only. The file list becomes current; contents do not arrive.
    Metadata,
    /// Segments and chunks.
    Everything,
}

impl Scope {
    /// Whether file contents move.
    #[must_use]
    pub const fn moves_content(self) -> bool {
        matches!(self, Self::Everything)
    }
}

/// One round, both directions, moving everything.
pub fn round(store: &Store, vault: &Vault, client: &mut PeerClient) -> Result<RoundReport> {
    round_scoped(store, vault, client, Scope::Everything)
}

/// One round, both directions, at `scope`.
///
/// Record that `client`'s peer answered: its ring and last contact, and,
/// for a direct session, the network it answered from (§8 7c).
///
/// # Errors
///
/// If the store cannot be written.
pub fn note_contact(store: &Store, client: &PeerClient) -> Result<()> {
    let device = client.peer_device();
    let now = itsanas_store::now_unix();
    store.note_seen(&device)?;
    if let Some(network) = client
        .network_address()
        .and_then(itsanas_placement::Network::of)
    {
        store.note_network_at(&device, network, now)?;
    }
    Ok(())
}

/// # Errors
///
/// If the peer fails, or the store cannot be written.
pub fn round_scoped(
    store: &Store,
    vault: &Vault,
    client: &mut PeerClient,
    scope: Scope,
) -> Result<RoundReport> {
    // A round that got this far spoke to the peer. Whether it had anything to
    // say is beside the point: the acknowledgements it made in the past count
    // as copies only while it is known to be there, and being answered is what
    // knowing consists of.
    note_contact(store, client)?;

    let push = push_scoped(store, client, scope)?;
    let pull = pull_scoped(store, vault, client, scope)?;
    Ok(RoundReport { push, pull })
}

mod audit;
mod fetching;
mod host_for;
mod pulling;
mod pushing;
mod repair;
mod sweep;

pub use audit::{AuditReport, CHALLENGES_PER_ROUND, audit};
pub use fetching::{fetch_only, fetch_only_noting, fetch_version};
pub use host_for::{HostReport, host_for, host_for_bounded};
pub use pulling::{drain_vault, pull, pull_scoped, refresh, refresh_tolerant};
pub use pushing::{push, push_placed, push_scoped};
pub use repair::{REPAIR_FETCH_PER_ROUND, REPAIR_SCAN_PER_ROUND, RepairReport, repair};
pub use sweep::FULL_RETRY;
pub(crate) use sweep::{Full, ask, bucket_chunks, refresh_released_page};

#[cfg(test)]
mod tests {
    use super::*;

    fn nothing() -> RoundReport {
        RoundReport::default()
    }

    #[test]
    fn red_team_a_peer_that_only_answered_the_phone_has_earned_nothing() {
        // THE ATTACK: a device key is a free keypair. An attacker mints one,
        // answers a dial, completes a mutually authenticated handshake, and
        // does nothing else. If that counted as evidence of being a real host,
        // the attacker would earn a place in the neighbour table that no
        // stranger can take — and could then repeat it until the table held
        // nothing else, evicting the machines that actually hold data.
        //
        // If this test fails, the anti-flood measure has become the flood's
        // best tool.
        assert!(
            !nothing().peer_earned_trust(),
            "authenticating alone was treated as trustworthy"
        );
    }

    #[test]
    fn red_team_a_failed_round_earns_nothing() {
        // Connecting and then falling over is not a contribution.
        let mut report = RoundReport::default();
        report.push.chunks_offered = 500;
        report.push.segments_offered = 20;
        assert!(
            !report.peer_earned_trust(),
            "offering data the peer never took was treated as the peer storing it"
        );
    }

    #[test]
    fn a_peer_that_accepted_our_data_has_earned_it() {
        let mut report = RoundReport::default();
        report.push.chunks_accepted = 1;
        assert!(report.peer_earned_trust());
    }

    #[test]
    fn a_peer_that_already_held_our_data_has_earned_it() {
        // The steady state for a peer that has been hosting for weeks: nothing
        // to send, nothing to fetch, and it is still the most valuable node
        // this device knows. Requiring fresh transfer would demote every
        // long-standing host to stranger the moment it caught up.
        let mut report = RoundReport::default();
        report.push.holders_recorded = 40;
        assert!(report.peer_earned_trust());
    }

    #[test]
    fn a_peer_that_served_us_our_own_work_has_earned_it() {
        let mut report = RoundReport::default();
        report.pull.adopted = 3;
        assert!(report.peer_earned_trust());
    }
}
