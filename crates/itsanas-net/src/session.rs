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

use itsanas_crypto::{ChunkId, DeviceId, UserId};
use itsanas_store::{SegmentEnvelope, Store, Vault, summary};
use itsanas_sync::{ChunkSource, SyncReport, apply_segments};

use crate::{
    error::{NetError, Result},
    protocol::{MAX_HAVE_BATCH, MAX_SEGMENTS_PER_REQUEST},
    service::Pledge,
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
    /// How many chunk ids this round put on the wire to ask "have you got
    /// these?".
    ///
    /// The number the reconciliation exists to keep at zero. It used to be the
    /// whole account, every round, per peer: a two-thousandth of the account in
    /// bytes, or a hundred and forty gigabytes a day for a terabyte. Reported
    /// so a test can hold it to zero, because an optimisation nothing measures
    /// is an optimisation nobody notices losing.
    pub chunks_asked_about: usize,
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
    store.note_seen(&client.peer_device())?;

    let push = push_scoped(store, client, scope)?;
    let pull = pull_scoped(store, vault, client, scope)?;
    Ok(RoundReport { push, pull })
}

/// Ask the peer about the chunks this device holds, and send what it lacks.
///
/// `only` narrows it to the buckets a summary said the two sides disagree
/// about; `None` means all of them, which is what happens against a peer too
/// old to summarise and on the periodic full walk of the ledger.
fn sweep(
    store: &Store,
    client: &mut PeerClient,
    report: &mut PushReport,
    only: Option<&[u8]>,
) -> Result<()> {
    let owner = store.owner();
    let peer = client.peer_device();

    let wanted: Option<[bool; summary::BUCKETS]> = only.map(|buckets| {
        let mut table = [false; summary::BUCKETS];
        for bucket in buckets {
            table[usize::from(*bucket)] = true;
        }
        table
    });

    let mut cursor: Option<ChunkId> = None;
    loop {
        let (page, next) = store.live_chunks_page(cursor.as_ref(), MAX_HAVE_BATCH)?;
        if page.is_empty() {
            break;
        }

        let filtered: Vec<ChunkId> = match &wanted {
            // A lookup rather than a scan of the bucket list per chunk: at a
            // terabyte and a full disagreement that difference is four billion
            // comparisons a round, on a Raspberry Pi.
            Some(wanted) => page
                .iter()
                .copied()
                .filter(|chunk| wanted[usize::from(summary::bucket_of(chunk))])
                .collect(),
            None => page.clone(),
        };

        if filtered.is_empty() {
            match next {
                Some(next) => {
                    cursor = Some(next);
                    continue;
                }
                None => break,
            }
        }

        let batch = filtered.as_slice();
        report.chunks_asked_about += batch.len();
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

        // And what it *did* ask for, it does not have -- whatever this node's
        // ledger says. Free, exact, and immediate: the same round trip that
        // decides what to send also withdraws every record this peer has
        // outgrown, which matters now that a device with a storage budget lets
        // go of content on purpose. Waiting for the audit to notice would mean
        // sixteen chunks per round against an account of millions.
        store.forget_holders(&missing, &peer)?;

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

        match next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    Ok(())
}

/// What a summary exchange concluded.
enum Reconciled {
    /// The two sides hold the same set. Nothing to list.
    Identical,
    /// They differ, in these buckets of the chunk id space.
    Buckets(Vec<u8>),
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
    let Some(theirs) = client.chunk_summary(owner).unwrap_or(None) else {
        return Ok(Reconciled::Unknown);
    };

    let ours = store.chunk_summary()?;
    if summary::root(&ours) == summary::root(&theirs) {
        return Ok(Reconciled::Identical);
    }

    Ok(Reconciled::Buckets(summary::differing(&ours, &theirs)))
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
fn refresh_released(store: &Store, client: &mut PeerClient, peer: &DeviceId) -> Result<usize> {
    let owner = store.owner();
    let mut recorded = 0usize;
    let mut cursor: Option<Vec<u8>> = None;

    loop {
        let (page, next) = store.holdings_page(peer, cursor.as_deref(), MAX_HAVE_BATCH)?;
        if page.is_empty() {
            break;
        }

        let ask: Vec<ChunkId> = page
            .into_iter()
            .filter(|chunk| !store.has_chunk(chunk))
            .collect();

        if !ask.is_empty() {
            let missing = client.missing_chunks(owner, ask.clone())?;
            let gone: BTreeSet<ChunkId> = missing.iter().copied().collect();
            store.forget_holders(&missing, peer)?;

            let still: Vec<ChunkId> = ask.into_iter().filter(|c| !gone.contains(c)).collect();
            recorded += still.len();
            store.record_holders(&still, peer)?;
        }

        match next {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    Ok(recorded)
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
        if client.store_segment(envelope)? {
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

    let only = match reconcile(store, client, owner)? {
        // Nothing to send and nothing owed to the ledger: the round is over,
        // and it cost one hash.
        Reconciled::Identical if !due => {
            report.holders_recorded += refresh_released(store, client, &peer)?;
            return Ok(report);
        }
        // A due walk covers everything, whatever the summary said about where
        // the two sides differ.
        _ if due => None,
        // An unanswerable peer means nothing is known, so everything is listed.
        Reconciled::Identical | Reconciled::Unknown => None,
        Reconciled::Buckets(buckets) => Some(buckets),
    };

    sweep(store, client, &mut report, only.as_deref())?;

    // The clock restarts only after a walk that really was full: a round that
    // listed a few buckets has said nothing about the rest, so claiming it had
    // would be the same rot by a slower route.
    if only.is_none() {
        store.note_ledger_walk(&peer, now)?;
    }

    report.holders_recorded += refresh_released(store, client, &peer)?;

    Ok(report)
}

/// Fetch exactly the chunks named, from a peer, and apply what they complete.
///
/// # The gap this closes
///
/// A device with a storage budget, or one that synced over a metered link, ends
/// up with files it knows about and has not downloaded --
/// `itsanas_store::catalogue` lists them, which is what a phone should show.
/// Until this existed there was no way to then *open* one: `itsanas get`
/// answered "no such file" for a file the account plainly had, and the design
/// that justified the budget had no mechanism behind it.
///
/// # Why a filtered source rather than a plain pull
///
/// The merge engine drives which chunks it asks for, so restricting a fetch to
/// one file means restricting what the *source* will serve. Everything outside
/// `wanted` is declined, the engine defers those operations exactly as it does
/// for a sleeping peer, and nothing else is downloaded. Opening one document on
/// a phone does not pull somebody's photo library.
///
/// # Errors
///
/// If the peer fails, or the store cannot be written.
pub fn fetch_only(
    store: &Store,
    vault: &Vault,
    client: &mut PeerClient,
    wanted: &BTreeSet<ChunkId>,
) -> Result<SyncReport> {
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

    // And this device's own chain, which the ordinary pull skips because the
    // index is the authority for anything this machine wrote. That stops being
    // true once content can be *released*: the file is still in the account and
    // its operation is still in this chain, but there is no index entry, so
    // without this `itsanas get` answers "no such file" for a file two other
    // machines are holding. Measured on a Raspberry Pi, after the catalogue had
    // been fixed to list it -- listing it and being unable to fetch it is the
    // worse of the two failures.
    //
    // Safe to replay because the filter decides: everything outside `wanted`
    // is declined and deferred, and a `Remove` later in the same chain still
    // wins, so nothing this device deleted comes back.
    segments.extend(store.segments()?);

    if segments.is_empty() {
        return Ok(SyncReport::default());
    }

    let source = SelectedChunks {
        client: RefCell::new(client),
        wanted,
        served: RefCell::new(Vec::new()),
    };
    // Replaying this device's own chain as well, which the ordinary pull does
    // not: see the comment above `segments.extend(store.segments()?)`. Safe
    // here and only here, because this source serves a chosen set and declines
    // everything else -- replaying with a source that serves everything would
    // undo every release on the next round.
    let (report, _) = itsanas_sync::apply_replaying(
        store,
        &segments,
        &source,
        itsanas_sync::Replay::IncludingOwn,
    )
    .map_err(|error| NetError::Refused(error.to_string()))?;

    let served = source.served.into_inner();
    if !served.is_empty() {
        let peer = source.client.into_inner().peer_device();
        store.record_holders(&served, &peer)?;
    }

    Ok(report)
}

/// A remote source that serves only the chunks of one file.
struct SelectedChunks<'a> {
    client: RefCell<&'a mut PeerClient>,
    wanted: &'a BTreeSet<ChunkId>,
    served: RefCell<Vec<ChunkId>>,
}

impl ChunkSource for SelectedChunks<'_> {
    fn fetch(&self, owner: UserId, address: &ChunkId) -> itsanas_sync::Result<Option<Vec<u8>>> {
        if !self.wanted.contains(address) {
            return Ok(None);
        }

        let fetched = self
            .client
            .borrow_mut()
            .chunk(owner, *address)
            .map_err(|error| itsanas_sync::SyncError::Source(error.to_string()))?;

        if fetched.is_some() {
            self.served.borrow_mut().push(*address);
        }
        Ok(fetched)
    }
}

/// What one round of hosting for a peer did./// What one round of hosting for a peer did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostReport {
    /// How many chunks the peer asked this node to hold.
    pub wanted: usize,
    /// How many were taken into this node's vault.
    pub taken: usize,
    /// How many were already held, so nothing was transferred.
    pub already_held: usize,
    pub bytes_taken: u64,
    /// Whether the pledge, not the peer, is what limited this round.
    pub pledge_full: bool,
}

impl HostReport {
    #[must_use]
    pub const fn changed_anything(&self) -> bool {
        self.taken > 0
    }
}

/// Take on some of a peer's data, over the connection this node dialled.
///
/// # The reason this exists
///
/// Every other exchange here runs one way. `push` offers this node's work and
/// the peer stores it; `pull` fetches what the peer holds and merges it. Both
/// are initiated by whoever dialled, and in both of them the *dialled* side is
/// the one that ends up hosting. So a member behind a router they do not
/// control could have their data held by others and could hold nothing in
/// return -- which in a system built on mutual storage is not an inconvenience,
/// it is being unable to keep your half of the bargain.
///
/// Nothing about networks required that. It was a question nobody asked. This
/// asks it: *have you anything you would like me to hold?* One reachable side
/// per pair is now enough for hosting to go both ways, and the reachable side
/// can be the one machine somebody has a port forwarded to.
///
/// # What bounds it
///
/// The peer names what it wants, and this node decides what it takes. The
/// pledge is checked here rather than trusted to the peer: a peer that asked
/// for a terabyte gets what this node offered the network and not a byte more.
///
/// # Errors
///
/// If the peer refuses, or the vault cannot be written.
/// How many chunks one round offers to take on.
///
/// A page rather than everything: a round that took an unbounded amount would
/// make one sync unpredictable in length, and the next round is never far away.
const PER_ROUND: u32 = 32;

pub fn host_for(vault: &Vault, client: &mut PeerClient, pledge: Pledge) -> Result<HostReport> {
    let mut report = HostReport::default();

    let held = vault.stats()?.bytes;
    if held >= pledge.bytes {
        report.pledge_full = true;
        return Ok(report);
    }

    let (owner, wanted) = client.want_hosted(PER_ROUND)?;
    report.wanted = wanted.len();

    let mut room = pledge.bytes.saturating_sub(held);
    let mut taken = Vec::new();

    for address in wanted {
        if vault.has_chunk(owner, &address)? {
            report.already_held += 1;
            // Still worth telling them: a holder they have forgotten about is
            // a copy they think they do not have.
            taken.push(address);
            continue;
        }

        let Some(sealed) = client.chunk(owner, address)? else {
            // The peer asked for this to be held and then would not hand it
            // over. Not an error -- it may have been collected between the two
            // messages -- and not this node's problem to solve.
            continue;
        };

        let size = sealed.len() as u64;
        if size > room {
            report.pledge_full = true;
            break;
        }

        if vault.put_chunk(owner, &address, &sealed)? {
            room = room.saturating_sub(size);
            report.taken += 1;
            report.bytes_taken = report.bytes_taken.saturating_add(size);
        }
        taken.push(address);
    }

    if !taken.is_empty() {
        // The owner keeps the placement ledger, so it has to be told. It is a
        // claim, and the owner's storage challenges are what turn it into
        // evidence -- the same treatment a chunk pushed the other way gets.
        client.hosted(taken)?;
    }

    Ok(report)
}

/// Fetch what the peer has from this user's *other* devices, and merge it.
///
/// # Errors
///
/// If the peer fails, or the store cannot be written.
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
/// Bring this node's vault up to date with what the peer knows, without
/// applying anything.
///
/// Kilobytes: signed log segments, not content. What it buys is the ability to
/// *decide* — `itsanas_store::catalogue` reads the vault, so after this a device
/// knows every file the account has, with its size and date, and can choose
/// which ones are worth its remaining room before spending a byte of it on
/// content.
///
/// Returns the segments newly fetched in this call, which is what the callers
/// that go on to apply them need.
///
/// # Errors
///
/// If the peer fails, or the vault cannot be written.
pub fn refresh(
    store: &Store,
    vault: &Vault,
    client: &mut PeerClient,
) -> Result<Vec<SegmentEnvelope>> {
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

    Ok(fetched)
}

/// Fetch and merge everything the peer has, at `scope`.
///
/// A deferred operation writes no index entry, so `Store::list` does not report
/// it. [`catalogue`](mod@itsanas_store::catalogue) is what shows it anyway: it derives the
/// account's whole file list from the vault's segments, marking what this device
/// has not downloaded, and [`fetch_only`] brings one down on demand.
///
/// A device that cannot hold its whole account does not call this: it decides
/// what belongs on it and calls [`fetch_only`]. The two used to be one function
/// with a byte budget, which stopped downloading when the allowance ran out and
/// so kept whatever the log happened to replay first -- a limit on the quantity
/// with no say over the choice.
///
/// # Errors
///
/// If the peer fails, or the store cannot be written.
pub fn pull_scoped(
    store: &Store,
    vault: &Vault,
    client: &mut PeerClient,
    scope: Scope,
) -> Result<SyncReport> {
    let owner = store.owner();
    let mine = store.device_id();

    let mut fetched = refresh(store, vault, client)?;

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

/// How many live chunks one repair round samples for local loss.
///
/// A bounded walk from a fresh random start, because at a terabyte there are
/// fourteen million and stat-ing all of them every round would cost far more
/// than the loss it is looking for.
///
/// # What this actually guarantees, which is less than it sounds
///
/// Each round examines 2048 chunks out of `N`, so the chance a given chunk is
/// looked at is `2048/N`, and reaching a nine-in-ten chance of having examined
/// one particular chunk takes:
///
/// | live chunks | rounds | at the five-minute service beat |
/// | --- | --- | --- |
/// | 2 000 | 1 | five minutes |
/// | 100 000 (~10 GB) | 111 | nine hours |
/// | 14 000 000 (1 TB) | 15 739 | **fifty-five days** |
///
/// An earlier version of this comment said "covers a household store in a
/// handful of rounds", which is wrong by a factor of twenty on the middle row
/// and by three orders of magnitude on the row it cites as the reason for the
/// bound in the first place.
///
/// Fifty-five days is defensible on its own terms — a block silently lost is
/// not an emergency until somebody reads the file — but only because it is not
/// the only detector. `doctor` finds every loss in one pass and now writes what
/// it finds where repair will drain it first. The sampling scan is the
/// background sweep for losses nobody has noticed yet.
pub const REPAIR_SCAN_PER_ROUND: usize = 2_048;

/// How many losses are considered for each one that can be asked about.
///
/// A loss is only worth raising with a peer the ledger records as holding it,
/// so most of the window is skipped. Eight times the fetch budget means a round
/// still finds work when seven in eight of the losses in view belong to peers
/// that are not this one.
const LOSS_WINDOW: usize = 8;

/// How many lost chunks one repair round tries to fetch back.
///
/// Small, because the peer is doing this for free and a node that has just lost
/// a disk would otherwise open with a demand for its entire store. Losing a
/// whole disk is a restore, not a repair; this is for the handful of blocks a
/// filesystem quietly drops.
pub const REPAIR_FETCH_PER_ROUND: usize = 32;

/// What one repair round found and fixed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RepairReport {
    /// Chunks this node's files need and whose bytes are not on this disk.
    ///
    /// Everything known, not only what this round sampled.
    pub lost: usize,
    /// Chunks asked of this peer, because its own record says it holds them.
    pub asked: usize,
    /// Chunks that came back, opened, and re-addressed to what was asked for.
    pub restored: usize,
    /// Chunks that came back as something else.
    ///
    /// Not a transport error. A peer that answers a repair request with bytes
    /// that do not open is either broken or trying to turn a recoverable loss
    /// into a permanent one, and either way its record for that chunk is
    /// withdrawn.
    pub forged: usize,
    /// Losses this peer is not recorded as holding, so it was not told about.
    pub not_asked: usize,
}

impl RepairReport {
    /// Whether this round did anything worth printing.
    #[must_use]
    pub const fn changed_anything(&self) -> bool {
        self.restored > 0 || self.forged > 0
    }
}

/// Fetch back chunks this node has lost, from a peer that already holds them.
///
/// # Why this is not the same as pushing
///
/// `push` restores *replication*: it offers a peer everything the peer lacks.
/// It cannot restore anything to this disk, and a chunk missing from this disk
/// is the one failure the placement ledger was built to survive. A file becomes
/// unreadable, `doctor` reports it, and until this existed the only cure was a
/// human noticing.
///
/// A disk drops a block, a filesystem loses an inode after a power cut, a
/// backup restore comes back partial. The chunk is still on three other
/// machines and its address is still in the index; nothing was reaching for it.
///
/// # A repair request is a disclosure, and that decides who may be asked
///
/// Asking a peer "do you have chunk X?" tells it that this node does not. The
/// ids are blinded, so it learns nothing about the *content* — but it learns
/// which chunks now exist only on hosts, which is precisely the list to delete
/// if you want to destroy somebody's data. A healing mechanism that publishes
/// the map of the wounds.
///
/// The first version asked every peer it connected to about everything it had
/// lost, including strangers the local-discovery loop had dialled for the first
/// time that round.
///
/// So a peer is asked **only about chunks the ledger already records it as
/// holding**, and a chunk no peer is recorded as holding is asked of nobody:
/// nobody has it, so the question buys nothing and costs the answer.
///
/// What that rule buys, stated honestly rather than generously. It does not
/// reduce the disclosure to nothing. The peer knew "I was given X"; it now
/// learns "the owner no longer has X", which is new, and if the ledger names it
/// as the only holder it has just learned it is the last copy. That is the
/// short list of what it could destroy, and the rule does not remove it.
///
/// What the rule does is cut the audience from *every peer this node dials* —
/// strangers the local-discovery loop met thirty seconds ago included — down
/// to the peers already holding the chunk, which is the smallest audience that
/// can repair it at all. Anything smaller repairs nothing.
///
/// One channel is left open and is worth naming rather than discovering later:
/// **silence**. A host holding a thousand chunks that is never asked about any
/// of them learns the owner has lost none of them. Cheap to observe, hard to
/// close without asking for chunks that are not missing, and the cost of that
/// cover traffic is the bandwidth this whole subsystem exists to conserve.
///
/// # What comes back is verified before it is written
///
/// See [`Store::accept_chunk`]. Accepting unverified bytes would set
/// `has_chunk`, stop the scan looking, and turn a recoverable loss into a
/// permanent one.
pub fn repair(store: &Store, client: &mut PeerClient, scan: usize) -> Result<RepairReport> {
    let owner = store.owner();
    let peer = client.peer_device();
    let mut report = RepairReport::default();

    // Sample a slice of the live set, which records anything newly missing.
    // Its return value is deliberately ignored: what matters is the queue it
    // maintains, which also holds everything `doctor` found in one pass.
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw)
        .map_err(|error| NetError::Refused(format!("could not draw a repair cursor: {error}")))?;
    let _ = store.missing_locally(&ChunkId::from_bytes(raw), scan)?;

    // A bounded slice from a moving start, not the whole queue from the top.
    // A node that lost a disk has millions of these, and a run of losses no
    // reachable peer holds would otherwise sit at the front of the key order
    // for ever and starve everything behind it.
    let losses = store.known_losses_from(
        &ChunkId::from_bytes(raw),
        REPAIR_FETCH_PER_ROUND * LOSS_WINDOW,
    )?;
    report.lost = usize::try_from(store.loss_count()?).unwrap_or(usize::MAX);

    for address in losses {
        if report.asked == REPAIR_FETCH_PER_ROUND {
            break;
        }

        // The disclosure test, and the only one: has this peer already told
        // this node it holds this chunk?
        let holds = store
            .remote_holders(&address)?
            .iter()
            .any(|holder| holder.device == peer);
        if !holds {
            report.not_asked += 1;
            continue;
        }

        report.asked += 1;
        let Some(sealed) = client.chunk(owner, address)? else {
            continue;
        };
        if store.accept_chunk(&address, &sealed)? {
            report.restored += 1;
        } else {
            report.forged += 1;
            // The peer said it held this and produced something else. Whether
            // that is corruption or malice, the record is not evidence of
            // anything any more.
            store.forget_holder(&address, &peer)?;
        }
    }

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
