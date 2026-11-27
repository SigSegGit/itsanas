//! What a host agrees to keep for another account: segments and chunks
//! within the pledge, the disk and the account's bound, and eviction marks.

use itsanas_crypto::{ChunkId, DeviceId, UserId};

use crate::{error::Result, protocol::Response};

use super::{ACCOUNT_FULL, NOT_YOURS_TO_EVICT, PLEDGE_EXHAUSTED, PeerService};

impl PeerService<'_> {
    /// Store one chunk for `owner`, within the host's pledge and the
    /// account's bound, evicting the account's own marked chunks first when
    /// the bound is what refuses it.
    /// Store a segment `caller` pushed, after the pledge and the owner allow it.
    pub(super) fn store_segment(
        &self,
        caller: DeviceId,
        envelope: &itsanas_store::SegmentEnvelope,
    ) -> Result<Response> {
        let _storing = self.storing_lock();
        let size = envelope.sealed_body.len();
        // The account's own log is not lending. A sibling's segments are
        // how a delete, a rename or a trash rule made on another machine
        // reaches this one; a pledge is room for other accounts, and a
        // new node pledges nothing, so a computer at pledge 0 refused
        // the delete for good of its own phone (#409's iPhone run). Only
        // the disk bounds them, and only for a proven member: the owner
        // named in an envelope is the sender's word.
        let sibling =
            envelope.owner == self.store.owner() && self.store.is_member(&caller).unwrap_or(false);
        let refused = if sibling {
            self.would_exceed_disk(size)
        } else {
            self.would_exceed_pledge(size)?
        };
        if refused {
            return Ok(Response::Refused(PLEDGE_EXHAUSTED.to_owned()));
        }
        if let Some(refusal) = self.refused_for_owner(caller, envelope.owner, size) {
            return Ok(refusal);
        }
        // A rejected segment is the peer's problem, not ours: refuse it
        // and say why, rather than failing the connection.
        //
        // `accepted` is what `put_segment` actually did, not `Ok(_)`.
        // Re-offering the tip is normal and stores nothing, and claiming
        // it was stored made every idle round look like a round that had
        // moved something -- so the daemon printed a line every five
        // minutes on a fleet where nothing was happening, which is how an
        // operator learns to stop reading the log.
        match self.vault.put_segment(envelope) {
            Ok(accepted) => Ok(Response::Stored { accepted }),
            Err(error) => Ok(Response::Refused(error.to_string())),
        }
    }

    pub(super) fn store_chunk(
        &self,
        caller: DeviceId,
        owner: UserId,
        address: &ChunkId,
        sealed: &[u8],
    ) -> Result<Response> {
        let _storing = self.storing_lock();
        if self.would_exceed_pledge(sealed.len())? {
            return Ok(Response::Refused(PLEDGE_EXHAUSTED.to_owned()));
        }
        if let Some(refusal) = self.refused_for_owner(caller, owner, sealed.len()) {
            // The account's share here is spent. If the owner marked
            // copies it has to spare, they make room for this one --
            // the account trades its own spare copy for its own short
            // one -- and the bound is asked once more, so eviction
            // never lets an account store past what it earns. Any
            // other refusal evicts nothing: a full pledge is the
            // host's, not the account's, and a device with no claim
            // has no say over the account's chunks.
            let full = matches!(&refusal, Response::Refused(why) if why.starts_with(ACCOUNT_FULL));
            // The real overage, not only the incoming size: an account
            // already past its share (its allowance shrank) is refused again
            // after freeing one chunk's worth, and those copies were evicted
            // for nothing (red team, LOW).
            let needed = match &refusal {
                Response::Refused(why) => overage(why, sealed.len() as u64),
                _ => sealed.len() as u64,
            };
            if !full
                || self
                    .vault
                    .evict_marked(owner, needed, itsanas_store::now_unix())?
                    == 0
            {
                return Ok(refusal);
            }
            if let Some(refusal) = self.refused_for_owner(caller, owner, sealed.len()) {
                return Ok(refusal);
            }
        }
        self.vault.put_chunk(owner, address, sealed)?;
        Ok(Response::Stored { accepted: true })
    }

    /// Mark `chunks` of `owner` evictable for `owner`, if `caller` speaks for
    /// `owner` and `owner` is another account than this node's.
    ///
    /// Fails closed: a host that bounds nobody has no way to tell who speaks
    /// for an account, and a stranger allowed to mark could have the
    /// account's next store evict copies the account still counts. Its own
    /// account is refused too: this node's own chunks are its store's, not
    /// the vault's, and nothing here may remove them.
    pub(super) fn may_evict(
        &self,
        caller: DeviceId,
        owner: UserId,
        chunks: &[ChunkId],
    ) -> Result<Response> {
        let allowed = owner != self.store.owner()
            && self
                .owners
                .is_some_and(|owners| owners.speaks_for(caller, owner));
        if !allowed {
            return Ok(Response::Refused(NOT_YOURS_TO_EVICT.to_owned()));
        }
        // The storing lock, as for a store: a mark lands between a store's
        // check and its write otherwise, and an eviction it triggers then
        // takes a chunk just stored.
        let _storing = self.storing_lock();
        let marked = self
            .vault
            .mark_evictable(owner, chunks, itsanas_store::now_unix())?;
        Ok(Response::MayEvict { marked })
    }

    /// The storing lock, recovered if a panic poisoned it.
    ///
    /// Poisoning here means one request panicked between a check and a write.
    /// Refusing every later store would turn one bug into a host that never
    /// accepts anything again; the check that follows re-reads the vault, so
    /// carrying on is safe.
    pub(super) fn storing_lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.storing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether `incoming` bytes would not fit on the disk itself.
    pub(super) fn would_exceed_disk(&self, incoming: usize) -> bool {
        self.disk_room
            .and_then(|room| room())
            .is_some_and(|room| incoming as u64 > room)
    }

    pub(super) fn would_exceed_pledge(&self, incoming: usize) -> Result<bool> {
        // A running total, not `stats()`: this runs under the storing lock on
        // every offer, and a walk here let a peer sending offers it knew would
        // be refused hold up every honest store behind a pass over the disk.
        if self.would_exceed_disk(incoming) {
            return Ok(true);
        }
        let held = self.vault.held_bytes()?;
        Ok(held.saturating_add(incoming as u64) > self.pledge.bytes)
    }
}

/// Bytes an account-full refusal says must go before `incoming` fits:
/// `mine + incoming - allowance`, read from "`{why}: {mine} of {allowance}
/// bytes ...`". Falls back to `incoming` for a message of another shape,
/// which is what was asked before and never more than the store needs.
pub(super) fn overage(why: &str, incoming: u64) -> u64 {
    let figures = why
        .split_once(": ")
        .map(|(_, rest)| rest.split(' ').collect::<Vec<_>>());
    if let Some(words) = figures
        && words.len() >= 3
        && words[1] == "of"
        && let (Ok(mine), Ok(allowance)) = (words[0].parse::<u64>(), words[2].parse::<u64>())
    {
        let over = mine.saturating_add(incoming).saturating_sub(allowance);
        if over > 0 {
            return over;
        }
    }
    incoming
}
