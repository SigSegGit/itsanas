//! Sending this device's log and chunks to a peer, and recording who now
//! holds them.

use itsanas_crypto::ChunkId;
use itsanas_store::Store;

use crate::{
    error::{NetError, Result},
    placement::Placement,
    transport::PeerClient,
};

use super::pulling::forked_segment;
use super::sweep::sweep_everything;
use super::{PushReport, Scope};

/// Offer this node's work to a peer, moving everything.
pub fn push(store: &Store, client: &mut PeerClient) -> Result<PushReport> {
    push_scoped(store, client, Scope::Everything)
}

/// Offer this node's work to a peer, at `scope`.
pub fn push_scoped(store: &Store, client: &mut PeerClient, scope: Scope) -> Result<PushReport> {
    push_placed(store, client, scope, None)
}

/// Offer this node's work to a peer, at `scope`, placing content by
/// `placement` when there is one.
///
/// `None`, or a peer that is one of this account's own devices, is the
/// `everyone` behaviour: every chunk the peer lacks, as [`push_scoped`] has
/// always done. Own devices get everything because they are where the account
/// is *used*, not where it is kept safe.
///
/// A peer in the round's roster gets, after the log and the paused probe that
/// every peer gets:
///
/// 1. its **due walk**, over only the chunks recorded for it: a bucket whose
///    digest of those chunks agrees with the peer's summary is re-stamped
///    without the wire, a differing one asks about only those chunks. The
///    everyone walk compares the peer's summary with *this device's* whole
///    set, which a host that is meant to hold a fifth of the account
///    disagrees with in every bucket, for ever; listing all of it then is the
///    537 MB walk §8 4c removed.
/// 2. **one page of the short queue**: a chunk goes to it when the placement
///    wants it there, when no proven remote copy exists at all (the first
///    host reached takes it, without delay: a new photograph should not wait
///    a census for its first copy), or when the chunk has lacked
///    *durability* for [`SAFETY_GRACE`](crate::placement::SAFETY_GRACE) and the peer is eligible. Never the
///    safety net for a chunk short only on availability or only because it
///    reached the cap: that is the case where "send it to whoever answers"
///    sends it everywhere.
/// 3. the released-chunk refresh, as for everyone;
/// 4. **§E**, when it refused for a full account share while chunks still
///    wanted it: it is told which of this account's copies it may drop.
///
/// A peer outside the roster that is not an own device gets the log, and the
/// chunks that have no proven copy anywhere.
///
/// # Errors
///
/// If the peer fails, or the store cannot be written.
pub fn push_placed(
    store: &Store,
    client: &mut PeerClient,
    scope: Scope,
    placement: Option<&Placement>,
) -> Result<PushReport> {
    let mut report = PushReport::default();
    offer_segments(store, client, &mut report)?;

    if !scope.moves_content() {
        // Segments have been offered; the peer now knows what this device has
        // done. Sending the bytes is the expensive half and it can wait for a
        // connection that does not cost money.
        return Ok(report);
    }

    if probe_if_paused(store, client, &mut report)? {
        return Ok(report);
    }

    let peer = client.peer_device();
    match placement.filter(|placement| !placement.is_own(&peer)) {
        None => sweep_everything(store, client, &mut report)?,
        Some(placement) => crate::placed::push(store, client, &mut report, placement)?,
    }
    Ok(report)
}

/// Offer the peer the part of this device's log it does not hold.
fn offer_segments(store: &Store, client: &mut PeerClient, report: &mut PushReport) -> Result<()> {
    let owner = store.owner();
    // Resume from what the peer already holds, exactly as `pull` does in the
    // other direction.
    //
    // Without this a push offered the whole chain on every round, for ever. The
    // peer refused each segment it already had -- a segment that is neither the
    // tip nor the next link answers `SegmentChainBroken` -- and `store_segment`
    // maps every refusal to `false`, so the waste was invisible from here.
    // Found by reading three machines' daemon logs after an upgrade and noticing
    // that "sent 400 B, 1 segments" never stopped on a fleet where nothing was
    // happening.
    //
    // The cost it removes grows without bound: a segment is a few hundred bytes,
    // the chain gains one per batch of edits and is never compacted, so a
    // thousand segments is roughly 350 KB re-uploaded per round per peer -- a
    // hundred megabytes a day against one peer, for nothing.
    //
    // One extra round trip buys it. `heads` is a verb this protocol already has.
    let already = client
        .heads(owner)?
        .into_iter()
        .find(|head| head.device == store.device_id())
        .map(|head| head.head);

    let chain = store.segments()?;
    // A head this chain does not hold, signed by this machine's key and
    // forking from it: a twin. Pushing on would fork the account's log, and
    // the peer would refuse every segment for ever, in silence.
    if let Some(head) = already
        && !chain.iter().any(|envelope| envelope.segment_id == head)
        && let Some(proof) = forked_segment(client, owner, store.device_id(), &chain)?
    {
        store.note_twin(&proof)?;
        report.twin = true;
        return Ok(());
    }
    let after = match already {
        // A head this device does not recognise means the peer is holding
        // something this chain does not contain, which is not a state a push can
        // repair. Offer everything and let the peer's own chain check decide.
        Some(head) => chain
            .iter()
            .position(|envelope| envelope.segment_id == head)
            .map_or(0, |index| index + 1),
        None => 0,
    };

    for envelope in &chain[after..] {
        report.segments_offered += 1;
        if report.record(client.store_segment(envelope)?) {
            report.segments_accepted += 1;
            report.bytes_sent = report
                .bytes_sent
                .saturating_add(envelope.sealed_body.len() as u64);
        }
    }

    Ok(())
}

/// The paused-peer probe; `true` when the peer is paused and the push of
/// content is over.
fn probe_if_paused(
    store: &Store,
    client: &mut PeerClient,
    report: &mut PushReport,
) -> Result<bool> {
    let owner = store.owner();
    let peer = client.peer_device();
    // A peer that has failed audit after audit has been re-sent this data every
    // round and thrown it away every round. Detecting that and re-uploading
    // anyway is a free, indefinite drain on this node's uplink, so the bulk
    // stops — while segments, which are kilobytes, keep going so the peer can
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
    if store.worth_sending_to(&peer)? {
        return Ok(false);
    }
    report.withheld = true;

    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw)
        .map_err(|error| NetError::Refused(format!("could not draw a probe: {error}")))?;
    let Some(address) = store.live_chunk_near(&ChunkId::from_bytes(raw))? else {
        return Ok(true); // nothing of our own to prove anything with
    };
    let Some(sealed) = store.blobs().get(&address)? else {
        return Ok(true); // collected between choosing and sending
    };

    report.chunks_offered += 1;
    let len = sealed.len() as u64;
    if report.record(client.store_chunk(owner, address, sealed)?) {
        report.chunks_accepted += 1;
        report.bytes_sent = report.bytes_sent.saturating_add(len);
        report.holders_recorded += 1;
        report.probe = Some(address);
        store.record_stored_at(&[address], &peer, itsanas_store::now_unix())?;
        // Recorded only on acceptance: a peer that will not take even the
        // one chunk offered has been asked nothing, and stays paused.
        store.note_probe(&peer, &address)?;
    }
    Ok(true)
}
