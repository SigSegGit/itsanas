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

/// Offer this device's segments and any chunks the peer lacks.
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

    // A peer that has failed audit after audit has been re-sent this data every
    // round and thrown it away every round. Detecting that and re-uploading
    // anyway is a free, indefinite drain on this node's uplink, so the bulk
    // stops """ + DASH + u""" while segments, which are kilobytes, keep going so the peer can
    // still relay for devices that have done nothing wrong.
    //
    // Not a ban: one chunk still goes. A failed audit withdraws the record for
    // that chunk, so withholding *everything* would leave nothing to challenge,
    // no audit would ever run, and the advertised way back could never be
    // taken. A ban wearing the words of a suspension.
    //
    // Two things about that one chunk, each of which was wrong once.
    //
    // It is **written down**, so the next audit asks about it and nothing else.
    // The first version left the audit to find it in the ledger, where it sat
    // as one fresh record among the thousands the peer is paused for; every
    // question landed on something it had already lost.
    //
    // And **the owner chooses it**. The second version took it from the peer's
    // own answer to "what are you missing?", which handed a host its own
    // examination question: name one small chunk, keep it, buy back the
    // terabyte you threw away. The owner now draws from its own live set and
    // the peer has no say. It is offered whether or not the peer claims to have
    // it, because "I already have that one" is the cheapest lie available.
    if !store.worth_sending_to(&peer)? {
        report.withheld = true;

        let mut raw = [0u8; 32];
        getrandom::fill(&mut raw)
            .map_err(|error| NetError::Refused(format!("could not draw a probe: {error}")))?;
        let Some(address) = store.live_chunk_near(&ChunkId::from_bytes(raw))? else {
            return Ok(report); // nothing of our own to prove anything with
        };
        let Some(sealed) = store.blobs().get(&address)? else {
            return Ok(report); // collected between choosing and sending
        };

        report.chunks_offered += 1;
        let len = sealed.len() as u64;
        if client.store_chunk(owner, address, sealed)? {
            report.chunks_accepted += 1;
            report.bytes_sent = report.bytes_sent.saturating_add(len);
            report.holders_recorded += 1;
            report.probe = Some(address);
            store.record_holders(&[address], &peer)?;
            // Recorded only on acceptance: a peer that will not take even the
            // one chunk offered has been asked nothing, and stays paused.
            store.note_probe(&peer, &address)?;
        }
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
/// At [`Scope::Metadata`] the segments are still fetched, verified and kept, so
/// this node can relay them onwards and the next content round resumes instead
/// of starting over. Every operation whose chunks would have to be downloaded
/// comes back as `deferred`. Nothing is half-written: an operation is either
/// applied with its content or left for later, which is the same guarantee a
/// sleeping peer already gets.
///
/// **What this does not yet do is show you the file.** A deferred operation
/// writes no index entry, so `Store::list` does not report it — the paths are
/// in the returned outcomes and in the vault's segments, and nothing keeps them
/// anywhere a browser could read. Presenting "known but not downloaded", the
/// way a phone client should, needs a catalogue derived from the vault that
/// does not exist yet. Recorded in `docs/ROADMAP.md`.
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
    // Applying an already-applied operation is cheap — the version comparison
    // happens before any chunk is fetched — but the walk itself is O(history),
    // and doing it on every round for every peer turned a per-round cost of
    // "the new segments" into "the whole chain, times the number of peers".
    // That was a regression, introduced with the fix and measured afterwards.
    //
    // So it only happens when something is actually outstanding: the vault
    // holds segments this device has not applied. `Store::has_unapplied` is a
    // cheap comparison of two markers, not a walk.
    //
    // Metadata rounds never replay. They could not complete anything anyway,
    // and walking a whole chain to defer it again is work for nothing.
    if scope.moves_content() && (fetched.is_empty() || store.has_unapplied(vault)?) {
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

    let peer = client.peer_device();
    let (outcome, served) = if scope.moves_content() {
        let source = RemoteChunks {
            client: RefCell::new(client),
            served: RefCell::new(Vec::new()),
        };
        let outcome = apply_segments(store, &fetched, &source);
        let served = source.served.into_inner();
        (outcome, served)
    } else {
        (
            apply_segments(store, &fetched, &itsanas_sync::EmptySource),
            Vec::new(),
        )
    };

    // Before the `?`. A peer that served the bytes held them, whether or not
    // the merge that asked for them then went wrong, and throwing that away
    // because of an unrelated failure would leave the ledger understating
    // replication — which is the direction that hides a real shortage.
    if !served.is_empty() {
        store.record_holders(&served, &peer)?;
    }

    let (report, _) = outcome.map_err(|error| NetError::Refused(error.to_string()))?;

    // Only a round that finished everything may move the markers. One deferral
    // and they stay where they are, so the next content round replays.
    if scope.moves_content() && report.deferred == 0 {
        store.note_all_applied(vault)?;
    }

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

/// How many chunks one audit round checks with one peer.
///
/// A challenge is a round trip and a hash of the sealed bytes on both sides, so
/// it is cheap per chunk and ruinous per million. Sixteen per peer per round is
/// enough that a modest account is fully re-audited within a day at the default
/// interval, and small enough that auditing never competes with syncing for a
/// Raspberry Pi's attention.
pub const CHALLENGES_PER_ROUND: usize = 16;

/// What an audit round found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AuditReport {
    /// Chunks challenged.
    pub asked: usize,
    /// Chunks the peer proved it still holds.
    pub confirmed: usize,
    /// Chunks the peer could not prove, whose records were withdrawn.
    pub failed: usize,
    /// Chunks skipped because this device no longer holds a copy to check
    /// against.
    ///
    /// Not a fault of the peer. Verifying a proof means re-deriving the sealed
    /// bytes locally, and a chunk this device has garbage-collected cannot be
    /// re-derived. Counted rather than hidden, because a node that has become
    /// unable to audit anything should be able to notice.
    pub unverifiable: usize,
    /// The peer's record after this round, when anything was asked.
    pub record: Option<itsanas_store::Reliability>,
    /// Whether this round was the single probe question put to a paused peer.
    ///
    /// A paused peer is asked about one chunk — the one the owner handed it —
    /// and not about the records it is paused for. Answering pays off one of
    /// its outstanding failures; enough of them lift the sanction. This flag is
    /// how a caller tells that round apart from an ordinary one.
    pub probing: bool,
}

impl AuditReport {
    /// Whether anything was found to be missing.
    #[must_use]
    pub const fn found_a_liar(&self) -> bool {
        self.failed > 0
    }
}

/// Ask a peer to prove it still holds what it said it held.
///
/// # Why this exists
///
/// The placement ledger records that a peer *accepted* a chunk. That is
/// evidence, not proof: a host that accepted a chunk and then deleted it looks
/// exactly the same from here. Without this, a node believes its data is safe
/// on three machines while two of them threw it away, and finds out on the day
/// the third disk dies.
///
/// # What a passing challenge does and does not prove
///
/// It proves the peer had the bytes when asked. It does not prove it will have
/// them tomorrow, and a host that fetches a chunk from another replica just in
/// time passes. That is the honest limit, stated in `docs/ECONOMICS.md` §9:
/// challenges raise the cost of lying without eliminating it, and the real
/// protection is replication across parties with no reason to collude.
///
/// # Failure withdraws evidence rather than punishing
///
/// A failed challenge removes that one (chunk, device) record, so the chunk
/// shows as under-replicated and repair can act. Nothing is deleted and nobody
/// is blocked — consistent with the rule in `docs/ECONOMICS.md` §5 that the
/// network never destroys data as a sanction.
///
/// # The questions are drawn, not scheduled
///
/// An audit is worth exactly the host's inability to guess what will be asked.
/// The first version worked through the least recently confirmed records, which
/// sounds diligent and was in fact a fixed list of the sixteen lowest chunk ids
/// asked every round for ever — a host could keep sixteen chunks out of
/// fourteen million and never be caught. Cursors are drawn fresh here and each
/// picks the chunk the ledger holds at or after it, so what is asked this round
/// says nothing about what will be asked next. See
/// [`Index::chunks_to_challenge`](itsanas_store::Index::chunks_to_challenge).
///
/// The one exception is a peer already under sanction, which is asked about the
/// single chunk it was handed as a probe and nothing else — because its other
/// records are the ones it is paused *for*, and drawing from them would make
/// the way back unreachable.
pub fn audit(store: &Store, client: &mut PeerClient, limit: usize) -> Result<AuditReport> {
    let owner = store.owner();
    let peer = client.peer_device();
    let mut report = AuditReport::default();

    // A paused peer answers for its probe alone. Everything else on its record
    // predates the sanction, so asking about any of it guarantees a failure and
    // turns a suspension into a life sentence.
    let targets = match store.probe(&peer)? {
        Some(probe) if !store.worth_sending_to(&peer)? => {
            report.probing = true;
            vec![probe]
        }
        _ => {
            let mut cursors: Vec<itsanas_store::AuditCursor> = Vec::with_capacity(limit);
            for _ in 0..limit {
                let mut raw = itsanas_store::AuditCursor::default();
                getrandom::fill(&mut raw).map_err(|error| {
                    NetError::Refused(format!("could not draw an audit cursor: {error}"))
                })?;
                cursors.push(raw);
            }
            store.chunks_to_challenge(&peer, &cursors)?
        }
    };

    for chunk in targets {
        // Re-derived from this device's own copy. Deterministic sealing is what
        // makes a remote audit possible without keeping a second copy of the
        // ciphertext, and it is why the chunk id is content-addressed.
        let Some(expected) = store.blobs().get(&chunk)? else {
            report.unverifiable += 1;
            continue;
        };

        // A fresh nonce per challenge, so a proof cannot be replayed and a host
        // cannot pre-compute answers.
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce).map_err(|error| {
            NetError::Refused(format!("could not draw a challenge nonce: {error}"))
        })?;

        report.asked += 1;
        if client.challenge(owner, chunk, nonce, &expected)? {
            report.confirmed += 1;
            store.record_holders(&[chunk], &peer)?;
        } else {
            report.failed += 1;
            store.forget_holder(&chunk, &peer)?;
        }
    }

    // One outcome per round, not one per chunk. A peer that fails sixteen
    // challenges in a single round has failed once — it is one host in one
    // state — and counting each chunk separately would pause it on the first
    // round rather than the third, which is the whole point of the threshold.
    if report.asked > 0 {
        report.record = Some(store.note_audit(&peer, report.failed == 0)?);
    }

    Ok(report)
}
