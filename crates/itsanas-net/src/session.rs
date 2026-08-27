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
use std::collections::BTreeSet;

use itsanas_crypto::{ChunkId, UserId};
use itsanas_store::{SegmentEnvelope, Store, Vault};
use itsanas_sync::{ChunkSource, SyncReport, apply_segments};

use crate::{
    error::{NetError, Result},
    protocol::{MAX_HAVE_BATCH, MAX_SEGMENTS_PER_REQUEST},
    transport::PeerClient,
};

/// What one push half did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PushReport {
    pub segments_offered: usize,
    pub segments_accepted: usize,
    pub chunks_offered: usize,
    pub chunks_accepted: usize,
    pub bytes_sent: u64,
    /// Chunks this peer is now known to hold, whether just sent or already had.
    ///
    /// Counted separately from `chunks_accepted` because the two answer
    /// different questions: how much work this round did, and how much of this
    /// node's data now exists somewhere other than this disk.
    pub holders_recorded: usize,
}

/// What a whole round did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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
}

impl ChunkSource for RemoteChunks<'_> {
    fn fetch(&self, owner: UserId, address: &ChunkId) -> itsanas_sync::Result<Option<Vec<u8>>> {
        self.client
            .borrow_mut()
            .chunk(owner, *address)
            // A peer that fails mid-fetch is a transport problem, not a merge
            // problem. Reporting it as "absent" would let a broken connection
            // masquerade as a device being asleep, and the operation would be
            // deferred forever instead of surfacing the fault.
            .map_err(|error| itsanas_sync::SyncError::Source(error.to_string()))
    }
}

/// Offer this device's segments and any chunks the peer lacks.
/// How much of a round to do.
///
/// A phone on mobile data, and a laptop tethered to one, both want the file
/// list without the files. Learning *what* changed means exchanging signed log
/// segments — kilobytes. Fetching the changes themselves is megabytes.
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
pub fn round_scoped(
    store: &Store,
    vault: &Vault,
    client: &mut PeerClient,
    scope: Scope,
) -> Result<RoundReport> {
    Ok(RoundReport {
        push: push_scoped(store, client, scope)?,
        pull: pull_scoped(store, vault, client, scope)?,
    })
}

/// Offer this node's work to a peer, moving everything.
pub fn push(store: &Store, client: &mut PeerClient) -> Result<PushReport> {
    push_scoped(store, client, Scope::Everything)
}

/// Offer this node's work to a peer, at `scope`.
pub fn push_scoped(store: &Store, client: &mut PeerClient, scope: Scope) -> Result<PushReport> {
    let owner = store.owner();
    let peer = client.peer_device();
    let mut report = PushReport::default();

    for envelope in store.segments()? {
        report.segments_offered += 1;
        if client.store_segment(&envelope)? {
            report.segments_accepted += 1;
            report.bytes_sent = report
                .bytes_sent
                .saturating_add(envelope.sealed_body.len() as u64);
        }
    }

    if !scope.moves_content() {
        // Segments have been offered; the peer now knows what this device has
        // done. Sending the bytes is the expensive half and it can wait for a
        // connection that does not cost money.
        return Ok(report);
    }

    // Ask before sending. Re-uploading a hundred thousand chunks every round
    // because we never asked is the difference between a usable system and one
    // that saturates the link forever.
    let addresses = store.blobs().addresses()?;
    for batch in addresses.chunks(MAX_HAVE_BATCH) {
        let missing = client.missing_chunks(owner, batch.to_vec())?;
        let wanted: BTreeSet<ChunkId> = missing.iter().copied().collect();

        // What the peer did *not* ask for, it already has. That answer costs
        // nothing extra — it is the same round trip that decides what to send —
        // and it is what makes the placement ledger converge on every sync
        // rather than only recording chunks this node happened to upload. A
        // node restored from its recovery phrase learns where its data lives by
        // asking, instead of re-uploading everything to find out.
        let mut confirmed: Vec<ChunkId> = batch
            .iter()
            .filter(|address| !wanted.contains(address))
            .copied()
            .collect();

        for address in missing {
            let Some(sealed) = store.blobs().get(&address)? else {
                // Collected between listing and sending. Not an error.
                continue;
            };

            report.chunks_offered += 1;
            let len = sealed.len() as u64;
            if client.store_chunk(owner, address, sealed)? {
                report.chunks_accepted += 1;
                report.bytes_sent = report.bytes_sent.saturating_add(len);
                confirmed.push(address);
            }
        }

        report.holders_recorded += confirmed.len();
        store.record_holders(&confirmed, &peer)?;
    }

    Ok(report)
}

/// Fetch what the peer has from this user's *other* devices, and merge it.
pub fn pull(store: &Store, vault: &Vault, client: &mut PeerClient) -> Result<SyncReport> {
    pull_scoped(store, vault, client, Scope::Everything)
}

/// Fetch what the peer has from this user's *other* devices, and merge it, at
/// `scope`.
///
/// At [`Scope::Metadata`] the segments are still fetched, verified and kept —
/// so the file list is current and this node can relay them onwards — and every
/// operation whose chunks would have to be downloaded comes back as `deferred`.
/// Nothing is half-written: an operation is either applied with its content or
/// left for later, which is the same guarantee a sleeping peer already gets.
pub fn pull_scoped(
    store: &Store,
    vault: &Vault,
    client: &mut PeerClient,
    scope: Scope,
) -> Result<SyncReport> {
    let owner = store.owner();
    let mine = store.device_id();

    let heads = client.heads(owner)?;
    let mut fetched: Vec<SegmentEnvelope> = Vec::new();

    for head in heads {
        if head.device == mine {
            continue;
        }

        // Resume from whatever this node's vault already holds for that device.
        let local_head = vault
            .heads_for(owner)?
            .into_iter()
            .find(|(device, _, _)| *device == head.device)
            .map(|(_, head, _)| head);

        if local_head == Some(head.head) {
            // Already current with this device. Nothing to ask for.
            continue;
        }

        let segments = client.segments(owner, head.device, local_head, MAX_SEGMENTS_PER_REQUEST)?;

        for envelope in &segments {
            // Retained so this node can relay them onwards, and so the next
            // pull has a resume point. put_segment verifies the signature and
            // refuses a chain with a hole.
            vault.put_segment(envelope)?;
        }

        fetched.extend(segments);
    }

    // A round that can move content applies from the **vault**, not from what
    // this round happened to fetch.
    //
    // Two reasons, and the second is the one that was actually broken. The
    // vault is a superset — every segment fetched above was just written into
    // it — and it is ordered, which matters because chain validation refuses a
    // segment that arrives before the one it follows. Splicing newly fetched
    // segments onto vault ones produced exactly that: "claims to follow X, but
    // the previous segment on this chain is none".
    //
    // And without it, work deferred by an earlier round is never retried. The
    // segments were kept, so the next pull sees the head as already current,
    // asks for nothing and applies nothing — the file that could not be
    // downloaded the first time is then never downloaded at all, silently, on
    // a node reporting a clean sync. That is the ordinary "the device holding
    // the chunks was asleep" case, which QUICKSTART describes as something a
    // later sync resolves, and which no later sync resolved.
    //
    // Applying an already-applied operation is cheap: the version comparison
    // happens before any chunk is fetched, so a replayed round costs local
    // index lookups and no network. It is still O(history) per round, which is
    // the same cost `drain_vault` already pays every round, and the same thing
    // pack files will have to address.
    //
    // Metadata rounds do not replay. They could not complete anything anyway,
    // and walking a whole chain to defer it again is work for nothing.
    if scope.moves_content() {
        fetched.clear();
        for (device, _, _) in vault.heads_for(owner)? {
            if device == mine {
                continue;
            }
            fetched.extend(vault.segments_for(
                owner,
                device,
                None,
                usize::from(MAX_SEGMENTS_PER_REQUEST),
            )?);
        }
    }

    if fetched.is_empty() {
        return Ok(SyncReport::default());
    }

    let (report, _) = if scope.moves_content() {
        let source = RemoteChunks {
            client: RefCell::new(client),
        };
        apply_segments(store, &fetched, &source)
    } else {
        apply_segments(store, &fetched, &itsanas_sync::EmptySource)
    }
    .map_err(|error| NetError::Refused(error.to_string()))?;

    Ok(report)
}

/// Apply this user's own segments that peers have pushed into the vault.
///
/// A push puts segments and chunks into the *receiving* node's vault, where
/// they sit ready to be relayed. Nothing else picks them up: only [`pull`]
/// applies segments to the local store, and a node that never dials anybody
/// never pulls. Without this, a node that only ever accepts connections stays
/// permanently ignorant of the very data it is holding.
///
/// That is not a corner case. Any device behind NAT can push and cannot be
/// dialled, so for its peers this is the *only* way its work arrives.
///
/// Chunks come from the vault, not the network: a peer that pushed a segment
/// pushed the chunks with it, so nothing here needs anyone to be online.
pub fn drain_vault(store: &Store, vault: &Vault) -> Result<SyncReport> {
    let owner = store.owner();
    let mine = store.device_id();

    let mut segments = Vec::new();
    for (device, _, _) in vault.heads_for(owner)? {
        if device == mine {
            continue;
        }
        segments.extend(vault.segments_for(
            owner,
            device,
            None,
            usize::from(MAX_SEGMENTS_PER_REQUEST),
        )?);
    }

    if segments.is_empty() {
        return Ok(SyncReport::default());
    }

    let source = VaultChunks { vault, owner };
    let (report, _) = apply_segments(store, &segments, &source)
        .map_err(|error| NetError::Refused(error.to_string()))?;

    Ok(report)
}

/// Serves chunks out of the local vault.
struct VaultChunks<'a> {
    vault: &'a Vault,
    owner: UserId,
}

impl ChunkSource for VaultChunks<'_> {
    fn fetch(&self, owner: UserId, address: &ChunkId) -> itsanas_sync::Result<Option<Vec<u8>>> {
        if owner != self.owner {
            return Ok(None);
        }
        self.vault
            .get_chunk(owner, address)
            .map_err(|error| itsanas_sync::SyncError::Source(error.to_string()))
    }
}

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
