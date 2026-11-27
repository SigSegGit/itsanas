//! Who holds this node's own data: a host's word that it holds chunks, a
//! machine draining, and what this node would like a caller to hold for it.

use itsanas_crypto::{ChunkId, DeviceId};

use crate::{error::Result, protocol::Response};

use super::PeerService;

impl PeerService<'_> {
    /// A host says it holds `chunks` of this account.
    pub(super) fn hosted(&self, caller: DeviceId, chunks: &[ChunkId]) -> Result<Response> {
        // A claim, recorded and then checked. The storage challenges
        // this node already runs are what turn it into evidence: a peer
        // that says it stored something and did not fails the next one
        // and the chunk is sent somewhere else. Believing it for now
        // costs one audit round and is what makes the offer worth
        // making at all. Not from a host paused for lying, whose word
        // is what it was paused for, and only for chunks this account
        // uses.
        if !self.store.worth_sending_to(&caller)? {
            return Ok(Response::Stored { accepted: false });
        }
        let ours = self.store.referenced_among(chunks)?;
        if !ours.is_empty() {
            self.store.record_holders(&ours, &caller)?;
        }
        Ok(Response::Stored { accepted: true })
    }

    /// Stop counting `caller` for `seconds`, and say how much rests on it alone.
    ///
    /// Scoped to the caller, as `Leaving` is: a device can only ever ask to be
    /// uncounted itself, so nobody can make an owner stop counting a third
    /// machine. The store caps the duration; the caller's number is a wish.
    pub(super) fn draining(&self, caller: DeviceId, seconds: u64) -> Result<Response> {
        let now = itsanas_store::now_unix();
        // Whether a grant was written changes nothing in the answer: the
        // count is the same question either way.
        self.store
            .note_draining(&caller, now.saturating_add(seconds), now)?;
        let chunks = self.store.exposed_by(&caller, now)?;
        Ok(Response::Exposed { chunks })
    }

    /// Chunks this node would like the caller to hold for it.
    ///
    /// The ones with fewest copies first, which is what
    /// [`Store::under_replicated`] already orders by, so the scarcest coverage
    /// is what a round of reciprocal hosting buys.
    ///
    /// Bounded twice: by what the caller asked for, and by a ceiling here. A
    /// peer that asks for everything gets a page, because the alternative is
    /// letting a caller decide how much work this node does in one answer.
    pub(super) fn want_hosted(&self, limit: u32, caller: DeviceId) -> Result<Response> {
        const CEILING: usize = 64;

        let wanted = (limit as usize).min(CEILING);
        // Cloned out of the lock at once: evaluating holds no lock the
        // daemon's swap would wait on.
        let placement = self
            .placement
            .and_then(|slot| slot.read().ok().and_then(|held| held.clone()))
            // Own devices get everything, as before placement existed.
            .filter(|placement| !placement.is_own(&caller));
        let chunks = match placement {
            // A host of the roster, once per round: each answer evaluates a
            // page of the queue, and a caller allowed to ask on every
            // connection, or a stranger with no share at all, would set this
            // node's CPU to work as often as it liked (red team, MEDIUM 6).
            Some(placement) if placement.take_pull(&caller) => {
                self.share_of(&placement, caller, wanted)?
            }
            Some(_) => Vec::new(),
            None => self
                .store
                .under_replicated(itsanas_store::REPLICATION_TARGET, itsanas_store::now_unix())?
                .into_iter()
                .take(wanted)
                .map(|risk| risk.chunk)
                .collect(),
        };

        Ok(Response::WantHosted {
            owner: self.store.owner(),
            chunks,
        })
    }

    /// `caller`'s share of the short queue under `placement`: what a push to
    /// it would offer, at most `wanted`. One page of the queue, as a push
    /// reads: a host that pulls is not sent everything short, which is what
    /// turned five hosts into five full copies.
    pub(super) fn share_of(
        &self,
        placement: &crate::placement::Placement,
        caller: DeviceId,
        wanted: usize,
    ) -> Result<Vec<ChunkId>> {
        let mut share = Vec::new();
        for (chunk, since) in crate::placed::short_page(self.store)? {
            if share.len() >= wanted {
                break;
            }
            if !self.store.has_chunk(&chunk) {
                continue; // released here: nothing to hand over
            }
            if placement.offers(&caller, &placement.evaluate(self.store, &chunk)?, since) {
                share.push(chunk);
            }
        }
        Ok(share)
    }
}
