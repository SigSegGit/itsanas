//! Storage challenges: making a host prove it still holds what it said it
//! held.

use itsanas_crypto::ChunkId;
use itsanas_store::Store;

use crate::{
    error::{NetError, Result},
    transport::PeerClient,
};

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
    /// Whether the peer was one of this account's own devices, whose missing
    /// chunks were withdrawn without counting against it.
    pub own: bool,
}

impl AuditReport {
    /// Whether anything was found to be missing from another account's host.
    #[must_use]
    pub const fn found_a_liar(&self) -> bool {
        self.failed > 0 && !self.own
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
///
/// One of this account's own devices (a roster member) is audited the same
/// way and its passes count as proof, but a chunk it no longer has is one it
/// let go, a phone or a key over its budget: the record is withdrawn and
/// nothing is held against it ([`Store::note_audit`]).
pub fn audit(store: &Store, client: &mut PeerClient, limit: usize) -> Result<AuditReport> {
    let owner = store.owner();
    let peer = client.peer_device();
    let mut report = AuditReport {
        own: store.is_member(&peer)?,
        ..AuditReport::default()
    };

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

    let mut confirmed: Vec<ChunkId> = Vec::new();
    let mut failed: Vec<ChunkId> = Vec::new();

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
            confirmed.push(chunk);
        } else {
            report.failed += 1;
            failed.push(chunk);
        }
    }

    // Written once, not once per answer.
    //
    // Sixteen challenges used to mean up to sixteen write transactions, and a
    // copy-on-write storage engine charges by the commit rather than by the
    // row: measured, a round that commits a couple of times writes 61 KB and
    // one that commits twenty writes 780 KB, on an account of nine hundred
    // kilobytes where nothing has changed. Batching the answers is the cheapest
    // test of that, and both calls already take slices.
    store.record_holders(&confirmed, &peer)?;
    store.forget_holders(&failed, &peer)?;

    // One outcome per round, not one per chunk. A peer that fails sixteen
    // challenges in a single round has failed once — it is one host in one
    // state — and counting each chunk separately would pause it on the first
    // round rather than the third, which is the whole point of the threshold.
    if report.asked > 0 {
        report.record = Some(store.note_audit(&peer, report.failed == 0)?);
    }

    Ok(report)
}
