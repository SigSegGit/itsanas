//! What a peer is asked about: the have/missing sweep, the summary that
//! narrows it to the buckets the two sides disagree on, and the ledger of
//! released chunks corrected from the peer's answers.

use std::collections::BTreeSet;

use itsanas_crypto::{ChunkId, DeviceId, UserId};
use itsanas_store::{Store, summary};

use crate::{
    error::Result,
    protocol::MAX_HAVE_BATCH,
    transport::{Offer, PeerClient, Refusal},
};

use super::PushReport;

/// Ask the peer about the chunks this device holds, and send what it lacks.
///
/// `only` narrows it to the buckets a summary said the two sides disagree
/// about; `None` means all of them, which is what happens against a peer too
/// old to summarise and on the periodic full walk of the ledger.
///
/// # What a narrowed round asks
///
/// Not the whole bucket. A bucket at a terabyte holds some sixty-five thousand
/// chunks and costs 2.1 MB to list, so one saved photograph cost 2.1 MB a peer
/// and the 100 MB/day budget bought about three megabytes of change a day
/// (`docs/DESIGN.md` §6.5). Within a named bucket only the chunks with **no
/// fresh record for this peer** are asked about: the new ones, and the ones it
/// last confirmed more than [`holders::REFRESH_AFTER`] ago. A chunk it confirmed
/// within that window is taken at its word until the next full walk.
///
/// What that gives up, stated: a peer that silently drops a chunk it recently
/// confirmed is not caught by a narrowed round. It is caught by the full walk,
/// which asks about everything whatever the records say and is due at most
/// [`holders::REFRESH_AFTER`] after the last one -- inside
/// [`holders::LIVE_FOR`], the window in which repair would have counted that
/// record anyway -- and by the audit, and by the peer's own drop notice. A
/// chunk not asked about keeps its record and loses nothing.
///
/// [`holders::REFRESH_AFTER`]: itsanas_store::holders::REFRESH_AFTER
/// [`holders::LIVE_FOR`]: itsanas_store::holders::LIVE_FOR
fn sweep(
    store: &Store,
    client: &mut PeerClient,
    report: &mut PushReport,
    only: Option<&[u8]>,
    thorough: bool,
    full: &mut Full,
) -> Result<()> {
    let Some(buckets) = only else {
        return walk(store, client, report, None, None, true, full);
    };

    // Each bucket is a contiguous range of the index, because its number is the
    // first byte of the id. Reading only those ranges is what makes one
    // differing bucket cost one bucket of disk reads: the first version paged
    // the whole live-chunk index and filtered, so one saved file walked every
    // row this device holds, on an SD card, every round.
    let mut buckets = buckets.to_vec();
    buckets.sort_unstable();
    buckets.dedup();
    for bucket in buckets {
        walk(
            store,
            client,
            report,
            Some(bucket),
            bucket_floor(bucket),
            !thorough,
            full,
        )?;
    }
    Ok(())
}

/// The id just below `bucket`, as an exclusive start for a page of the index.
///
/// `None` for bucket 0, which starts at the beginning. The id itself belongs to
/// the previous bucket, so skipping it -- `after` is exclusive -- can only ever
/// skip a chunk this walk would have filtered out anyway.
fn bucket_floor(bucket: u8) -> Option<ChunkId> {
    let below = bucket.checked_sub(1)?;
    let mut bytes = [0xFF; itsanas_crypto::ID_LEN];
    bytes[0] = below;
    Some(ChunkId::from_bytes(bytes))
}

/// Page through the index from `after`, asking the peer about what is found.
///
/// With `bucket`, stops at the end of that bucket and asks only about chunks
/// with no fresh record for this peer; without, walks everything and asks
/// about all of it.
/// Whether the peer is full, and when "now" is, for one round's walks.
pub(crate) struct Full {
    pub(crate) now: u64,
    /// Set at the start of the round from the last refusal, and the moment a
    /// refusal arrives during it.
    pub(crate) yes: bool,
    /// A placed push: a refusal for a full *account share* stops offering
    /// too, and records are stamped at the round's clock. Off for
    /// `everyone`, which must behave exactly as it did before placement.
    pub(crate) placed: bool,
    /// The peer refused for a full account share during this push (§E).
    pub(crate) share_spent: bool,
}

impl Full {
    /// The state at the start of a push: full if the peer refused within
    /// [`FULL_RETRY`].
    pub(crate) fn at(store: &Store, peer: &DeviceId, now: u64, placed: bool) -> Result<Self> {
        Ok(Self {
            now,
            yes: store
                .peer_full_since(peer)?
                .is_some_and(|since| since >= now.saturating_sub(FULL_RETRY)),
            placed,
            share_spent: false,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn walk(
    store: &Store,
    client: &mut PeerClient,
    report: &mut PushReport,
    bucket: Option<u8>,
    after: Option<ChunkId>,
    only_unconfirmed: bool,
    full: &mut Full,
) -> Result<()> {
    let peer = client.peer_device();
    let mut cursor = after;
    loop {
        let (mut page, next) = store.live_chunks_page(cursor.as_ref(), MAX_HAVE_BATCH)?;
        let mut finished = next.is_none();

        if let Some(bucket) = bucket {
            // Pages are in id order, so the first chunk past the bucket ends it.
            if let Some(past) = page
                .iter()
                .position(|chunk| !summary::in_bucket(chunk, bucket))
            {
                page.truncate(past);
                finished = true;
            }
            // Not on a due walk: there a differing bucket is asked about in
            // full, because the agreeing ones are about to be taken on the
            // summary's word and this is the only listing left (§8 4c).
            if only_unconfirmed {
                page = store.without_fresh_record(&page, &peer)?;
            }
        }

        // A full peer would refuse what it lacks, so asking about chunks it has
        // no record for buys nothing. What it is recorded as holding is still
        // asked, on the same rules: that is what keeps those records honest.
        if full.yes {
            page = store.with_record(&page, &peer)?;
        }

        if !page.is_empty() {
            ask(store, client, report, &page, full)?;
        }

        match next {
            Some(next) if !finished => cursor = Some(next),
            _ => break,
        }
    }
    Ok(())
}

/// One have/missing exchange over `batch`: send what the peer lacks, and
/// correct the ledger from its answer in both directions.
pub(crate) fn ask(
    store: &Store,
    client: &mut PeerClient,
    report: &mut PushReport,
    batch: &[ChunkId],
    full: &mut Full,
) -> Result<()> {
    let owner = store.owner();
    let peer = client.peer_device();

    report.chunks_asked_about += batch.len();
    let missing = client.missing_chunks(owner, batch.to_vec())?;
    let wanted: BTreeSet<ChunkId> = missing.iter().copied().collect();

    // What the peer did *not* ask for, it already has. That answer costs
    // nothing extra — it is the same round trip that decides what to send —
    // and it is what makes the placement ledger converge on every sync
    // rather than only recording chunks this node happened to upload. A
    // node restored from its recovery phrase learns where its data lives by
    // asking, instead of re-uploading everything to find out.
    let confirmed: Vec<ChunkId> = batch
        .iter()
        .filter(|address| !wanted.contains(address))
        .copied()
        .collect();

    // And what it *did* ask for, it does not have -- whatever this node's
    // ledger says. Free, exact, and immediate: the same round trip that
    // decides what to send also withdraws every record this peer has
    // outgrown, which matters now that a device with a storage budget lets
    // go of content on purpose. Waiting for the audit to notice would mean
    // sixteen chunks per round against an account of millions.
    store.forget_holders(&missing, &peer)?;

    // Kept apart from `confirmed`: a chunk the peer just accepted is a new
    // copy and ends any release of the old one, while a "have" for a copy
    // this node released is the host's word before it evicts, and must not
    // bring the record back (red team, HIGH 1).
    let mut stored = Vec::new();
    for address in missing {
        // The peer said it is full. Every further offer is a chunk's bytes on
        // the wire to be refused, which is what this stops.
        if full.yes {
            break;
        }
        let Some(sealed) = store.blobs().get(&address)? else {
            // Collected between listing and sending. Not an error.
            continue;
        };

        report.chunks_offered += 1;
        let len = sealed.len() as u64;
        let offer = client.store_chunk(owner, address, sealed)?;
        let pledge_full = matches!(offer, Offer::Refused(Refusal::PledgeFull));
        // Placed: an account whose share is spent is as full as a host whose
        // pledge is, for this account. Offering on would send every byte to
        // be refused, and §E needs to know it happened.
        let account_full = full.placed && matches!(offer, Offer::Refused(Refusal::AccountFull));
        if report.record(offer) {
            report.chunks_accepted += 1;
            report.bytes_sent = report.bytes_sent.saturating_add(len);
            stored.push(address);
        }
        if pledge_full || account_full {
            store.note_peer_full(&peer, full.now)?;
            full.yes = true;
        }
        full.share_spent |= account_full;
    }

    report.holders_recorded += confirmed.len() + stored.len();
    let now = if full.placed {
        full.now
    } else {
        itsanas_store::now_unix()
    };
    store.record_holders_at(&confirmed, &peer, now)?;
    store.record_stored_at(&stored, &peer, now)?;
    Ok(())
}

/// Re-stamp, without the wire, the records of every bucket the peer's summary
/// agrees with; return the buckets that turned out not to agree after all.
///
/// # Why each bucket is hashed again here
///
/// The verdict came from this device's summary, computed before this function
/// reads a single row. A chunk written in between lands in a bucket that
/// "agreed", and re-stamping it would record the peer as holding a chunk it
/// has never seen -- a copy believed for `LIVE_FOR`. Found by Rodin on the
/// plan. So each bucket's digest is recomputed from exactly the rows about to
/// be re-stamped and compared with the peer's; a bucket that no longer
/// matches is handed back to be listed instead. It costs nothing the walk
/// was not already paying: the rows are read either way.
///
/// What this believes, stated: the peer's digest. A host that replays an old
/// summary is believed past `REFRESH_AFTER` (decided, `docs/HANDOVER.md` §10
/// 8); the host's own disk check (`Vault::check_disk`) is what keeps an
/// honest host's summary true, and the storage audit is the only witness
/// against a dishonest one.
fn restamp_agreeing(
    store: &Store,
    peer: &DeviceId,
    theirs: &[summary::Digest],
    differing: &[u8],
    now: u64,
    report: &mut PushReport,
) -> Result<Vec<u8>> {
    let mut relist = Vec::new();
    if theirs.len() != summary::BUCKETS {
        // Not the same dialect: nothing agrees, so everything is listed.
        return Ok((0..=u8::MAX).collect());
    }
    for bucket in 0..=u8::MAX {
        if differing.contains(&bucket) {
            continue;
        }
        let chunks = bucket_chunks(store, bucket)?;
        if summary::bucket_digest(bucket, &chunks) != theirs[usize::from(bucket)] {
            relist.push(bucket);
            continue;
        }
        let stale = store.without_fresh_record(&chunks, peer)?;
        report.holders_restamped += stale.len();
        store.record_holders_at(&stale, peer, now)?;
    }
    Ok(relist)
}

/// Every live chunk of one bucket, in id order: one contiguous range of the
/// index. At a terabyte about sixty-five thousand ids, 2 MB, and never more
/// than one bucket at a time.
pub(crate) fn bucket_chunks(store: &Store, bucket: u8) -> Result<Vec<ChunkId>> {
    let mut out = Vec::new();
    let mut cursor = bucket_floor(bucket);
    loop {
        let (page, next) = store.live_chunks_page(cursor.as_ref(), MAX_HAVE_BATCH)?;
        let end = page
            .iter()
            .position(|chunk| !summary::in_bucket(chunk, bucket));
        match end {
            Some(end) => {
                out.extend_from_slice(&page[..end]);
                return Ok(out);
            }
            None => out.extend(page),
        }
        match next {
            Some(next) => cursor = Some(next),
            None => return Ok(out),
        }
    }
}

/// How long a peer that refused a chunk for a full pledge is treated as full.
///
/// While it is, a round asks it only about chunks it is recorded as holding and
/// offers it nothing. Without this a full peer was asked about every chunk it
/// lacked and sent each one's bytes, refused, every round: against a phone
/// holding a tenth of a terabyte that is fifteen million ids -- 480 MB -- and
/// the bytes behind them, every five minutes. Once this has passed, the next
/// round is ordinary and serves as the probe: it asks one page of what the peer
/// lacks and offers until the first refusal, so finding out whether the peer
/// has room again costs about 32 KB and one chunk, an hour. A peer whose owner
/// raised its pledge waits at most this long to be offered more.
pub const FULL_RETRY: u64 = 60 * 60;

/// What a summary exchange concluded.
enum Reconciled {
    /// The two sides hold the same set. Nothing to list. Carries the peer's
    /// digests, which a due walk checks bucket by bucket before re-stamping.
    Identical(Vec<summary::Digest>),
    /// They differ, in these buckets of the chunk id space; the peer's digests
    /// beside them.
    Buckets(Vec<u8>, Vec<summary::Digest>),
    /// The peer is too old to be asked, so nothing is known and everything is
    /// listed — which is what every round did before this existed.
    Unknown,
}

/// Compare what the two sides hold, in one hash.
///
/// The whole point of the exercise: the ordinary answer is "the same", and
/// saying so should not cost a two-thousandth of the account.
///
/// A failure to summarise is not a failure of the round. A peer that refuses
/// the question, or answers something unexpected, is treated exactly as one too
/// old to be asked: the sweep runs in full, correctly and expensively. This is
/// an optimisation, and an optimisation that can break a sync is not one.
fn reconcile(store: &Store, client: &mut PeerClient, owner: UserId) -> Result<Reconciled> {
    let ours = store.chunk_summary()?;
    // The root alone first: the ordinary answer is "the same", and 32 bytes
    // say it. Equal roots are equal digests, so this side's stand in for the
    // peer's when a due walk checks them bucket by bucket.
    if client.summary_root(owner).unwrap_or(None) == Some(summary::root(&ours)) {
        return Ok(Reconciled::Identical(ours));
    }
    let Some(theirs) = client.chunk_summary(owner).unwrap_or(None) else {
        return Ok(Reconciled::Unknown);
    };

    if summary::root(&ours) == summary::root(&theirs) {
        return Ok(Reconciled::Identical(theirs));
    }

    let differing = summary::differing(&ours, &theirs);
    Ok(Reconciled::Buckets(differing, theirs))
}

/// Ask this peer about chunks it is recorded as holding that this device no
/// longer holds itself, and correct the ledger from the answer.
///
/// # The hole this closes
///
/// The have/missing sweep above starts from `store.blobs().addresses()`. A
/// released chunk is not there, so it was never offered, never came back in a
/// "missing" answer, and was never withdrawn. The audit could not reach it
/// either: `session::audit` re-derives the expected ciphertext from this
/// device's own copy, and a device that let go of a chunk has no copy to derive
/// from, so those challenges come back `unverifiable`.
///
/// Both together meant that **once a device released a chunk, nothing could
/// ever again tell it that the holders had lost that chunk** — leaving only the
/// peer's voluntary drop notice, which is the honesty of the party the whole
/// mechanism exists not to have to trust. And the device where it mattered most
/// was the one that had released the most: the phone.
///
/// It also fixes the three-machine case with no new message. A releases and
/// tells B; C never hears it, and goes on counting A. Now C's own next round
/// asks A about the chunks it thinks A holds, A answers "missing", and C
/// corrects itself.
///
/// # What it costs, and what it does not
///
/// Nothing at all on a machine that holds its whole account: every recorded
/// chunk is one this device also has, and those are filtered out before a single
/// question is asked. On a device short of room it is 32 bytes per released
/// chunk per round, the same order as the sweep it complements — and it is
/// paged with a cursor rather than a fixed prefix, because a fixed prefix is a
/// fixed list and this project has already been caught by one.
///
/// The answer is a *claim*, not a proof. A peer that says "I still have it"
/// cannot be challenged on a chunk this device no longer holds; that limit is
/// real and is stated in `docs/DESIGN.md` §6.4. What this restores is the
/// ability to hear "no".
pub(crate) fn refresh_released(
    store: &Store,
    client: &mut PeerClient,
    peer: &DeviceId,
) -> Result<usize> {
    let mut recorded = 0usize;
    let mut cursor: Option<Vec<u8>> = None;

    loop {
        let (page, next) = store.holdings_page(peer, cursor.as_deref(), MAX_HAVE_BATCH)?;
        if page.is_empty() {
            break;
        }

        recorded += refresh_released_page(store, client, peer, page)?;

        match next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    Ok(recorded)
}

/// One page of [`refresh_released`]: ask about the chunks of `page` this
/// node no longer holds, and correct their records from the answer.
pub(crate) fn refresh_released_page(
    store: &Store,
    client: &mut PeerClient,
    peer: &DeviceId,
    page: Vec<ChunkId>,
) -> Result<usize> {
    let ask: Vec<ChunkId> = page
        .into_iter()
        .filter(|chunk| !store.has_chunk(chunk))
        .collect();
    if ask.is_empty() {
        return Ok(0);
    }
    let missing = client.missing_chunks(store.owner(), ask.clone())?;
    let gone: BTreeSet<ChunkId> = missing.iter().copied().collect();
    store.forget_holders(&missing, peer)?;
    let still: Vec<ChunkId> = ask.into_iter().filter(|c| !gone.contains(c)).collect();
    store.record_holders(&still, peer)?;
    Ok(still.len())
}

/// Everything the peer lacks: the `everyone` content push.
pub(super) fn sweep_everything(
    store: &Store,
    client: &mut PeerClient,
    report: &mut PushReport,
) -> Result<()> {
    let owner = store.owner();
    let peer = client.peer_device();
    // Ask before sending. Re-uploading a hundred thousand chunks every round
    // because we never asked is the difference between a usable system and one
    // that saturates the link forever.
    // Paged from the index, in chunk order, rather than from a directory walk.
    //
    // This loop used to begin `store.blobs().addresses()?`, whose own
    // documentation says it "walks the fan-out directories [...] never on a hot
    // path" -- and this is the hottest path there is, every round, per peer. At
    // a terabyte that is a recursive `readdir` over sixteen million files every
    // five minutes and a `Vec` of sixteen million ids, 537 MB resident in one
    // allocation, on machines whose measured peak is 17 MiB. The account size at
    // which this sweep becomes expensive on the wire is far past the size at
    // which it kills the process; the bandwidth was the second problem.
    //
    // And before any of that: ask whether there is anything to reconcile at
    // all. One hash, whatever the account weighs. If the two sides hold the
    // same set — which is the answer on almost every round of almost every day
    // — the sweep is skipped entirely and the round costs nothing. If they
    // differ, the summary says *where*, and only those buckets are listed.
    //
    // A differing hash is a question, not a verdict: what follows is the same
    // have/missing exchange as before, over a slice.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    // Is the ledger due a full walk, whatever the summary says?
    //
    // This question has to come **before** the verdict, not inside the branch
    // where the two sides agree. A peer whose storage budget is smaller than
    // this account disagrees on every round, for ever, by design -- and the
    // first version only consulted this in the `Identical` arm, so against such
    // a peer the walk was never due, never performed, never stamped, and the
    // chunks in the agreeing buckets were re-stamped by nobody. Fourteen days
    // later `coverage` reports no copies and `release` refuses to free
    // anything, on an account where nothing has gone wrong and against a host
    // that is behaving perfectly. Silent, and it breaks the two things this is
    // for: knowing where your data is, and being able to make room.
    let due = store.ledger_walk_due(&peer, now)?;

    let reconciled = reconcile(store, client, owner)?;
    let mut full = Full::at(store, &peer, now, false)?;

    match reconciled {
        // Nothing to send and nothing owed to the ledger: the round is over,
        // and it cost one hash.
        Reconciled::Identical(_) if !due => {
            report.holders_recorded += refresh_released(store, client, &peer)?;
            return Ok(());
        }
        // A due walk against a peer that summarises (§8 4c). The buckets it
        // agrees about are re-stamped here without a word on the wire; only
        // the ones it differs about are listed, in full. Before 4c this listed
        // the whole account per peer every REFRESH_AFTER: 537 MB every three
        // and a half days at a terabyte, over the 100 MB/day budget by itself.
        Reconciled::Identical(theirs) if due => {
            let relist = restamp_agreeing(store, &peer, &theirs, &[], now, report)?;
            sweep(store, client, report, Some(&relist), true, &mut full)?;
            store.note_ledger_walk(&peer, now)?;
        }
        Reconciled::Buckets(differing, theirs) if due => {
            let mut relist = restamp_agreeing(store, &peer, &theirs, &differing, now, report)?;
            relist.extend_from_slice(&differing);
            sweep(store, client, report, Some(&relist), true, &mut full)?;
            store.note_ledger_walk(&peer, now)?;
        }
        // An unanswerable peer means nothing is known, so everything is listed;
        // that is also its due walk.
        Reconciled::Unknown | Reconciled::Identical(_) => {
            sweep(store, client, report, None, false, &mut full)?;
            store.note_ledger_walk(&peer, now)?;
        }
        // Not due: only the differing buckets, and within them only what the
        // peer has not confirmed lately (§8 4a).
        Reconciled::Buckets(differing, _) => {
            sweep(store, client, report, Some(&differing), false, &mut full)?;
        }
    }

    report.holders_recorded += refresh_released(store, client, &peer)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_team_a_chunk_written_after_the_summary_is_not_restamped() {
        // Found by Rodin on the 4c plan. The verdict "this bucket agrees" comes
        // from a summary taken before the re-stamp reads its rows; a chunk
        // written in between would be recorded as held by a peer that has
        // never seen it -- a copy believed for LIVE_FOR. Sabotage: re-stamp
        // without recomputing the bucket's digest.
        use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};

        let dir = tempfile::tempdir().unwrap();
        let store = Store::open_for_testing(
            dir.path(),
            UserKeys::derive(&MasterSecret::from_bytes([0xB7; 32])),
            DeviceKeys::from_seed(&SecretBytes::new([0xB8; 32])),
            itsanas_store::ChunkerConfig::default(),
        )
        .unwrap();
        store
            .write_file("before.bin", &itsanas_testkit::filler("before", 256 * 1024))
            .unwrap();
        // What an agreeing peer said, a moment before the write below.
        let theirs = store.chunk_summary().unwrap();
        let before: BTreeSet<ChunkId> = store
            .live_chunks_page(None, 10_000)
            .unwrap()
            .0
            .into_iter()
            .collect();

        store
            .write_file("after.bin", &itsanas_testkit::filler("after", 256 * 1024))
            .unwrap();
        let late: Vec<ChunkId> = store
            .live_chunks_page(None, 10_000)
            .unwrap()
            .0
            .into_iter()
            .filter(|chunk| !before.contains(chunk))
            .collect();
        assert!(!late.is_empty(), "fixture: the second write added nothing");

        let peer = DeviceId::from_bytes([0x77; 32]);
        let mut report = PushReport::default();
        let relist = restamp_agreeing(&store, &peer, &theirs, &[], 1_000, &mut report).unwrap();

        for chunk in &late {
            assert!(
                store.with_record(&[*chunk], &peer).unwrap().is_empty(),
                "a chunk written after the summary was recorded as held by a peer that has \
                 never seen it"
            );
            assert!(
                relist.contains(&summary::bucket_of(chunk)),
                "the bucket the late chunk landed in was not handed back to be listed"
            );
        }
        assert_eq!(
            report.holders_restamped,
            before
                .iter()
                .filter(|chunk| !relist.contains(&summary::bucket_of(chunk)))
                .count(),
            "the buckets that still agree must be re-stamped, or the step saves nothing"
        );
    }
}
