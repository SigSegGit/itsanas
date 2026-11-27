//! Where the account's chunks went: the holder ledger recorded, read and
//! forgotten, its summaries, and what each peer holds.

use itsanas_crypto::{ChunkId, DeviceId};

use super::Store;
use crate::{
    error::Result,
    holders::{Holder, HolderEvidence},
    index::now_unix,
};

impl Store {
    // ------------------------------------------------------- where it went

    /// Record that `device` acknowledged holding these chunks.
    ///
    /// This is the replacement for a coordinator-published node set: an owner
    /// who already keeps a log of their own chunks writes down where they put
    /// them, and nothing has to agree with anybody about membership. See
    /// `holders.rs`.
    pub fn record_holders(&self, chunks: &[ChunkId], device: &DeviceId) -> Result<()> {
        self.index.record_holders(chunks, device, now_unix())
    }

    /// [`Self::record_holders`] at a given time: what a round re-stamping
    /// agreeing buckets uses, so every record it writes carries the same
    /// "now" the round decided freshness by.
    ///
    /// # Errors
    ///
    /// If the index cannot be written.
    pub fn record_holders_at(&self, chunks: &[ChunkId], device: &DeviceId, now: u64) -> Result<()> {
        self.index.record_holders(chunks, device, now)
    }

    /// Arm the short queue for `measured`, or disarm and empty it for
    /// `everyone`. See [`Index::set_placement_measured`].
    ///
    /// # Errors
    ///
    /// If the index cannot be read or written.
    pub fn set_placement_measured(&self, measured: bool) -> Result<()> {
        self.index.set_placement_measured(measured)
    }

    /// When `device` last passed a storage audit here, if it has.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn last_passed(&self, device: &DeviceId) -> Result<Option<u64>> {
        self.index.last_passed(device)
    }

    /// Record that `device` accepted these chunks in a `StoreChunk` just
    /// now: unlike [`Self::record_holders`], this ends any release of them.
    ///
    /// # Errors
    ///
    /// If the index cannot be written.
    pub fn record_stored_at(&self, chunks: &[ChunkId], device: &DeviceId, now: u64) -> Result<()> {
        self.index.record_stored(chunks, device, now)
    }

    /// Every other device known to hold `chunk`.
    pub fn remote_holders(&self, chunk: &ChunkId) -> Result<Vec<Holder>> {
        self.index.remote_holders(chunk)
    }

    /// Withdraw the record that `device` holds `chunk`.
    ///
    /// For a failed storage challenge: a record is evidence that a host once
    /// accepted a chunk, never proof that it still has it.
    /// Withdraw many records for one device at once.
    ///
    /// Two callers, both of which correct the ledger with something better than
    /// a memory: a peer that answers "I am missing this" during a push, and a
    /// peer that says outright what it has let go of.
    /// What this node can say about who else holds `chunk`, and how recently
    /// they said it about *this* chunk.
    ///
    /// See [`Index::holder_evidence`]. Reported by `itsanas status` and used by
    /// [`Store::release`], which is the operation that must not act on a
    /// machine-level liveness signal alone.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn holder_evidence(&self, chunk: &ChunkId, now: u64) -> Result<HolderEvidence> {
        self.index.holder_evidence(chunk, now)
    }

    /// Whether the whole holder ledger for `device` is due a walk.
    ///
    /// A round that reconciles in one hash does not touch the ledger when the
    /// two sides agree. This is what stops every record quietly ageing out of
    /// countable while that happens.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn ledger_walk_due(&self, device: &DeviceId, now: u64) -> Result<bool> {
        self.index.ledger_walk_due(device, now)
    }

    /// Record that the whole ledger for `device` has just been walked.
    ///
    /// # Errors
    ///
    /// If the index cannot be written.
    pub fn note_ledger_walk(&self, device: &DeviceId, now: u64) -> Result<()> {
        self.index.note_ledger_walk(device, now)
    }

    /// A summary of what this device holds, for reconciling with a peer.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn chunk_summary(&self) -> Result<Vec<crate::summary::Digest>> {
        self.index.chunk_summary()
    }

    /// The summary a peer of the same account is answered with: the live
    /// chunks less the ones known to be missing from this disk. See
    /// [`Index::held_summary`].
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn held_summary(&self) -> Result<Vec<crate::summary::Digest>> {
        self.index.held_summary()
    }

    /// About how many bytes of this node's data `device` is recorded as
    /// holding: its holder records, counted in one lookup, times the mean
    /// size of this node's own chunks.
    ///
    /// An estimate, said so: the ledger counts chunks, not bytes, and a
    /// device holding only this node's largest chunks is credited the mean.
    /// What a host uses it for -- how much an account has proved it hosts
    /// here (§8 1c) -- is bounded by records this node wrote itself after a
    /// peer accepted a chunk or answered for it, never by anything the peer
    /// says about its size.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn bytes_held_by(&self, device: &DeviceId) -> Result<u64> {
        let records = self.index.held_by(device)?;
        if records == 0 {
            return Ok(0);
        }
        let chunks = self.index.live_chunk_count()?.max(1);
        let mean = self.local_bytes()? / chunks;
        Ok(records.saturating_mul(mean))
    }

    /// How many chunks are live, without listing them.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn live_chunk_count(&self) -> Result<u64> {
        self.index.live_chunk_count()
    }

    /// One page of the chunks this device holds, in chunk order.
    ///
    /// What a push walks to ask a peer what it is missing. See
    /// [`Index::live_chunks_page`] for why it is not a directory scan.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn live_chunks_page(
        &self,
        after: Option<&ChunkId>,
        limit: usize,
    ) -> Result<(Vec<ChunkId>, Option<ChunkId>)> {
        self.index.live_chunks_page(after, limit)
    }

    /// A page of the chunks `device` is recorded as holding.
    ///
    /// What a sync round needs in order to keep the ledger honest about content
    /// this device no longer holds itself. See [`Index::holdings_page`].
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn holdings_page(
        &self,
        device: &DeviceId,
        after: Option<&[u8]>,
        limit: usize,
    ) -> Result<(Vec<ChunkId>, Option<Vec<u8>>)> {
        self.index.holdings_page(device, after, limit)
    }

    /// Of `chunks`, the ones `device` has not confirmed holding within
    /// [`crate::holders::REFRESH_AFTER`]. See [`Index::without_fresh_record`].
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn without_fresh_record(
        &self,
        chunks: &[ChunkId],
        device: &DeviceId,
    ) -> Result<Vec<ChunkId>> {
        self.index.without_fresh_record(chunks, device, now_unix())
    }

    /// Of `chunks`, the ones `device` has any record for. See
    /// [`Index::with_record`].
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn with_record(&self, chunks: &[ChunkId], device: &DeviceId) -> Result<Vec<ChunkId>> {
        self.index.with_record(chunks, device)
    }

    /// Record that `device` refused a chunk for a full pledge at `now`.
    ///
    /// # Errors
    ///
    /// If the index cannot be written.
    pub fn note_peer_full(&self, device: &DeviceId, now: u64) -> Result<()> {
        self.index.note_peer_full(device, now)
    }

    /// When `device` last refused a chunk for a full pledge, if ever.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn peer_full_since(&self, device: &DeviceId) -> Result<Option<u64>> {
        self.index.peer_full_since(device)
    }

    pub fn forget_holders(&self, chunks: &[ChunkId], device: &DeviceId) -> Result<()> {
        self.index.forget_holders(chunks, device)
    }

    pub fn forget_holder(&self, chunk: &ChunkId, device: &DeviceId) -> Result<()> {
        self.index.forget_holder(chunk, device)
    }
}
