//! The content half of a push to a host, under placement.
//!
//! [`crate::session::push_placed`] hands a peer here once it is known not to
//! be one of this account's own devices and not paused. What follows is the
//! order the steps run in and why each one is bounded.
//!
//! What this module refuses to do:
//!
//! - **Offer what the placement did not ask for.** A host gets a chunk it is
//!   wanted for, one that has no proven copy anywhere, or one that has gone
//!   without durability for
//!   [`SAFETY_GRACE`](crate::placement::SAFETY_GRACE) ([`Placement::offers`]).
//!   Never "everything it lacks", which is what turned five hosts into five
//!   full copies.
//! - **List the account on a due walk.** Only the chunks recorded for this
//!   host are hashed and, where they disagree, asked about.
//! - **Release on a guess.** §E needs this device's own copy, the strict
//!   evidence of [`Placement::release_choice`], a clock that has not gone
//!   back, and no release of the same copy within [`CONFIRMED_FOR`].
//! - **Speak `MayEvict` to a peer below protocol 10.** The client keeps the
//!   verb back; this module then keeps every record.

use itsanas_crypto::{ChunkId, DeviceId};
use itsanas_store::{Store, holders::CONFIRMED_FOR, summary};

use crate::{
    error::{NetError, Result},
    placement::Placement,
    protocol::MAX_HAVE_BATCH,
    session::{Full, PushReport, ask, bucket_chunks, refresh_released},
    transport::PeerClient,
};

/// The short queue read per session: one have/missing batch.
const SHORT_PAGE: usize = MAX_HAVE_BATCH;

/// Push content to `client`'s peer by `placement`.
pub(crate) fn push(
    store: &Store,
    client: &mut PeerClient,
    report: &mut PushReport,
    placement: &Placement,
) -> Result<()> {
    let peer = client.peer_device();
    let now = placement.now();
    let placed = placement.is_placed(&peer);

    if placed {
        due_walk(store, client, report, &peer, now)?;
    }

    let mut full = Full::at(store, &peer, now, true)?;
    let wanting = short_round(store, client, report, placement, &mut full)?;

    if placed {
        report.holders_recorded += refresh_released(store, client, &peer)?;
        if full.share_spent && wanting > 0 {
            release(store, client, report, placement, wanting)?;
        }
    }
    Ok(())
}

/// The ledger walk for a placed host, when due: per bucket, the chunks
/// recorded for it against its own summary.
///
/// The everyone walk hashes this device's whole bucket, which a host that
/// holds a share of the account never matches, so every due walk listed
/// every bucket. Hashing only what is *recorded* for the host makes the
/// ordinary answer "agree" again; a bucket that disagrees asks about those
/// records and nothing else (Rodin, MUST-FIX 3). No offers here: what the
/// host should take is the short queue's business.
fn due_walk(
    store: &Store,
    client: &mut PeerClient,
    report: &mut PushReport,
    peer: &DeviceId,
    now: u64,
) -> Result<()> {
    if !store.ledger_walk_due(peer, now)? {
        return Ok(());
    }
    // A peer that cannot summarise is asked about every record instead,
    // which is still only this host's records.
    let theirs = client
        .chunk_summary(store.owner())
        .unwrap_or(None)
        .filter(|digests| digests.len() == summary::BUCKETS);
    for bucket in 0..=u8::MAX {
        let recorded = store.with_record(&bucket_chunks(store, bucket)?, peer)?;
        if recorded.is_empty() {
            continue;
        }
        let agrees = theirs.as_ref().is_some_and(|digests| {
            summary::bucket_digest(bucket, &recorded) == digests[usize::from(bucket)]
        });
        if agrees {
            report.holders_restamped += recorded.len();
            store.record_holders_at(&recorded, peer, now)?;
            continue;
        }
        for batch in recorded.chunks(MAX_HAVE_BATCH) {
            ask_only(store, client, report, batch, peer, now)?;
        }
    }
    store.note_ledger_walk(peer, now)?;
    Ok(())
}

/// One have/missing exchange that corrects the ledger and offers nothing.
fn ask_only(
    store: &Store,
    client: &mut PeerClient,
    report: &mut PushReport,
    batch: &[ChunkId],
    peer: &DeviceId,
    now: u64,
) -> Result<()> {
    report.chunks_asked_about += batch.len();
    let missing = client.missing_chunks(store.owner(), batch.to_vec())?;
    store.forget_holders(&missing, peer)?;
    let held: Vec<ChunkId> = batch
        .iter()
        .filter(|chunk| !missing.contains(chunk))
        .copied()
        .collect();
    report.holders_recorded += held.len();
    store.record_holders_at(&held, peer, now)?;
    Ok(())
}

/// One page of the short queue, from a random start: offer the peer its
/// share, then settle what that satisfied. Returns how many chunks wanted
/// this peer and are still not recorded on it.
fn short_round(
    store: &Store,
    client: &mut PeerClient,
    report: &mut PushReport,
    placement: &Placement,
    full: &mut Full,
) -> Result<usize> {
    if full.yes {
        // Refused within FULL_RETRY: offering is bytes to be refused. The
        // round after the retry is the probe, as for everyone.
        return Ok(0);
    }
    let peer = client.peer_device();

    let mut offer = Vec::new();
    let mut wanted = Vec::new();
    for (chunk, since) in short_page(store)? {
        if !store.has_chunk(&chunk) {
            continue; // released here: nothing to send
        }
        let evaluation = placement.evaluate(store, &chunk)?;
        if placement.offers(&peer, &evaluation, since) {
            offer.push(chunk);
        }
        if evaluation.want.hosts.contains(&peer) && evaluation.holders.binary_search(&peer).is_err()
        {
            wanted.push(chunk);
        }
    }
    if offer.is_empty() {
        return Ok(0);
    }

    for batch in offer.chunks(MAX_HAVE_BATCH) {
        ask(store, client, report, batch, full)?;
    }

    let mut settled = Vec::new();
    for chunk in &offer {
        if placement.evaluate(store, chunk)?.verdict.satisfied() {
            settled.push(*chunk);
        }
    }
    store.settle_short(&settled)?;

    let recorded = store.with_record(&wanted, &peer)?;
    Ok(wanted.len() - recorded.len())
}

/// Up to [`SHORT_PAGE`] queued chunks from a random point, wrapping, never
/// one twice: a fixed start would serve the head of the queue first on every
/// session and starve the tail whenever the queue is longer than a page.
pub(crate) fn short_page(store: &Store) -> Result<Vec<(ChunkId, u64)>> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| NetError::Refused(format!("no entropy for a queue start: {error}")))?;
    let start = ChunkId::from_bytes(bytes);
    let (mut page, next) = store.short_page(Some(&start), SHORT_PAGE)?;
    if next.is_none() && page.len() < SHORT_PAGE {
        let (wrapped, _) = store.short_page(None, SHORT_PAGE - page.len())?;
        page.extend(wrapped.into_iter().take_while(|(chunk, _)| *chunk <= start));
    }
    Ok(page)
}

/// §E: tell a host whose account share is full which of this account's
/// copies on it may go, so the chunks that want it can come in.
///
/// One page of its records, at most `wanting` (and one batch) released: room
/// for what is waiting, not a clear-out. The owner stops counting the copy at
/// once and queues the chunk, so a host that evicts, and one that does not,
/// both leave the ledger conservative.
fn release(
    store: &Store,
    client: &mut PeerClient,
    report: &mut PushReport,
    placement: &Placement,
    wanting: usize,
) -> Result<()> {
    let peer = client.peer_device();
    let now = placement.now();
    // A clock behind what the rings were written at reads every host as
    // recently seen, which is the evidence release trusts.
    if now / 3600 < store.latest_hour_written()? {
        return Ok(());
    }
    let limit = wanting.min(MAX_HAVE_BATCH);
    let (page, _) = store.holdings_page(&peer, None, MAX_HAVE_BATCH)?;
    let mut batch = Vec::new();
    for chunk in page {
        if batch.len() >= limit {
            break;
        }
        // Without a copy here, the remaining copies are all there is, and
        // this device could not put one back.
        if !store.has_chunk(&chunk) {
            continue;
        }
        if store
            .released_at(&chunk, &peer)?
            .is_some_and(|at| now.saturating_sub(at) < CONFIRMED_FOR)
        {
            continue;
        }
        if placement.release_choice(store, &chunk)? == Some(peer) {
            batch.push(chunk);
        }
    }
    if batch.is_empty() {
        return Ok(());
    }
    // `None` is a peer below protocol 10, never asked; a refusal is a host
    // that does not count this device as speaking for the account. Either
    // way nothing was marked and every record stands.
    let Ok(Some(_)) = client.may_evict(store.owner(), batch.clone()) else {
        return Ok(());
    };
    store.forget_holders(&batch, &peer)?;
    store.note_released(&batch, &peer, now)?;
    store.enqueue_short(&batch, now)?;
    report.released += batch.len();
    Ok(())
}
