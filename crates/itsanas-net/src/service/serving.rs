//! What a peer reads from this node: chain tips, segments, chunks and the
//! summary of what is held.

use itsanas_crypto::{DeviceId, UserId};

use crate::{
    error::Result,
    protocol::{Head, Response},
};

use super::PeerService;

impl PeerService<'_> {
    /// Chain tips, from both this node's own log and its vault.
    ///
    /// Both sources matter. Asking a peer "what do you have for Alice" must
    /// include Alice's own devices if this *is* one of Alice's devices, and the
    /// segments it holds for her as a host if it is not.
    pub(super) fn heads(&self, owner: UserId) -> Result<Response> {
        let mut heads: Vec<Head> = self
            .vault
            .heads_for(owner)?
            .into_iter()
            .map(|(device, head, length)| Head {
                device,
                head,
                length,
            })
            .collect();

        if owner == self.store.owner()
            && let Some(head) = self.store.head_segment()?
        {
            heads.push(Head {
                device: self.store.device_id(),
                head,
                length: self.store.chain_length()?,
            });
        }

        // Deterministic order, so two identical nodes answer identically and a
        // difference in a test is a real difference.
        heads.sort_by_key(|head| head.device.to_bytes());
        Ok(Response::Heads(heads))
    }

    pub(super) fn segments(
        &self,
        owner: UserId,
        device: DeviceId,
        after: Option<itsanas_crypto::ObjectId>,
        limit: usize,
    ) -> Result<Vec<itsanas_store::SegmentEnvelope>> {
        // This node's own chain, if that is what was asked for.
        if owner == self.store.owner() && device == self.store.device_id() {
            let mine = self.store.segments()?;
            let start = match after {
                None => 0,
                Some(id) => match mine.iter().position(|s| s.segment_id == id) {
                    Some(index) => index + 1,
                    // A resume point we do not recognise. Returning everything
                    // would re-send history the caller already has.
                    None => return Ok(Vec::new()),
                },
            };
            return Ok(mine.into_iter().skip(start).take(limit).collect());
        }

        Ok(self.vault.segments_for(owner, device, after, limit)?)
    }

    pub(super) fn chunk(
        &self,
        owner: UserId,
        address: &itsanas_crypto::ChunkId,
    ) -> Result<Option<Vec<u8>>> {
        // Our own chunks first: if this is our data, the store is authoritative
        // and the vault would not hold it.
        if owner == self.store.owner()
            && let Some(sealed) = self.store.blobs().get(address)?
        {
            return Ok(Some(sealed));
        }

        Ok(self.vault.get_chunk(owner, address)?)
    }

    /// Whether this node holds `address` for `owner`, from the same places
    /// [`Self::chunk`] would read it, without reading it.
    pub(super) fn holds(&self, owner: UserId, address: &itsanas_crypto::ChunkId) -> Result<bool> {
        if owner == self.store.owner() && self.store.blobs().contains(address) {
            return Ok(true);
        }
        Ok(self.vault.has_chunk(owner, address)?)
    }

    /// What this node holds of `owner`'s chunks, summarised.
    ///
    /// Over the same set this node would answer `HaveChunks` from: its own
    /// store when the account is its own, its vault when it is holding for
    /// somebody else. Any other set and the summary would disagree with the
    /// exchange that follows it, which is the one way this mechanism can be
    /// worse than none. `held_summary`, not `chunk_summary`: since §8 4c an
    /// agreeing bucket re-stamps holder records without asking, so this node
    /// must not say it holds a chunk its own disk is known to have lost. The
    /// vault's index is kept honest by its rolling check.
    pub(super) fn summary_of(&self, owner: UserId) -> Result<Vec<itsanas_store::summary::Digest>> {
        Ok(if owner == self.store.owner() {
            self.store.held_summary()?
        } else {
            self.vault
                .chunk_summary_at(owner, itsanas_store::now_unix())?
        })
    }
}
