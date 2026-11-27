//! Placement: uptime, networks, the short queue, the census, releases,
//! drains, audit challenges, and how many copies each chunk has.

use itsanas_crypto::{ChunkId, DeviceId};

use super::Store;
use crate::{
    error::Result,
    holders::{AtRisk, AuditCursor, Coverage},
};

impl Store {
    // ------------------------------------------------------------ placement

    /// Note that this node tried `device` and did not reach it. Known devices
    /// only. See [`Index::note_unreached`].
    pub fn note_unreached_at(&self, device: &DeviceId, now: u64) -> Result<bool> {
        self.index.note_unreached(device, now)
    }

    /// [`Store::note_seen_at`] for an inbound session: known devices only.
    pub fn note_seen_if_known_at(&self, device: &DeviceId, now: u64) -> Result<bool> {
        self.index.note_seen_if_known(device, now)
    }

    /// Admit `device` to this account's roster, or mark it revoked: the
    /// caller verified its claim. See [`Index::admit_member`].
    pub fn admit_member(&self, device: &DeviceId, revoked: bool) -> Result<()> {
        self.index.admit_member(device, revoked)
    }

    /// Whether a chain signed by `device` may be applied here: always for
    /// this device, else a live roster member once the roster is armed.
    pub fn may_apply_chain_of(&self, device: &DeviceId) -> Result<bool> {
        Ok(*device == self.device_id()
            || !self.index.roster_armed()?
            || self.index.is_member(device)?)
    }

    /// Record the network `device` was seen from over a direct session. Known
    /// devices only. See [`Index::note_network`].
    pub fn note_network_at(
        &self,
        device: &DeviceId,
        network: itsanas_placement::Network,
        now: u64,
    ) -> Result<bool> {
        self.index.note_network(device, network, now)
    }

    /// The network `device` was last seen from, if at or after `since`.
    pub fn network_since(
        &self,
        device: &DeviceId,
        since: u64,
    ) -> Result<Option<itsanas_placement::Network>> {
        self.index.network_since(device, since)
    }

    /// `device`'s uptime ring, as this node has observed it.
    pub fn uptime(&self, device: &DeviceId) -> Result<Option<itsanas_placement::Uptime>> {
        self.index.uptime(device)
    }

    /// Every uptime ring this node keeps, sorted by device.
    pub fn uptimes(&self) -> Result<Vec<(DeviceId, itsanas_placement::Uptime)>> {
        self.index.uptimes()
    }

    /// The highest hour any ring was written at. See
    /// [`Index::latest_hour_written`].
    pub fn latest_hour_written(&self) -> Result<u64> {
        self.index.latest_hour_written()
    }

    /// Queue `chunks` as possibly short of their placement goal.
    pub fn enqueue_short(&self, chunks: &[ChunkId], now: u64) -> Result<()> {
        self.index.enqueue_short(chunks, now)
    }

    /// Take `chunks` off the short queue.
    pub fn settle_short(&self, chunks: &[ChunkId]) -> Result<()> {
        self.index.settle_short(chunks)
    }

    /// A page of the short queue. See [`Index::short_page`].
    pub fn short_page(
        &self,
        after: Option<&ChunkId>,
        limit: usize,
    ) -> Result<crate::index::ShortPage> {
        self.index.short_page(after, limit)
    }

    /// How many chunks are queued as possibly short.
    pub fn short_count(&self) -> Result<u64> {
        self.index.short_count()
    }

    /// The queued chunk at or after `cursor`, wrapping.
    pub fn short_near(&self, cursor: &ChunkId) -> Result<Option<ChunkId>> {
        self.index.short_near(cursor)
    }

    /// A page of live chunks with their holders. See [`Index::census_page`].
    pub fn census_page(
        &self,
        after: Option<&ChunkId>,
        limit: usize,
    ) -> Result<(Vec<crate::index::CensusRow>, Option<ChunkId>)> {
        self.index.census_page(after, limit)
    }

    /// The next `limit` chunks of the rolling census from a caller-drawn
    /// start. See [`Index::census_next`]. The daemon sizes the page with
    /// [`rows_per_round`](crate::holders::rows_per_round) and the caller
    /// draws the start, so a test can fix it.
    pub fn census_next(
        &self,
        limit: usize,
        random: &ChunkId,
    ) -> Result<Vec<crate::index::CensusRow>> {
        self.index.census_next(limit, random)
    }

    /// Every other device recorded as holding `chunk`, with its stamp.
    pub fn holders_with_stamps(&self, chunk: &ChunkId) -> Result<Vec<(DeviceId, u64)>> {
        self.index.holders_with_stamps(chunk)
    }

    /// Record that this node released `device`'s copy of `chunks`.
    pub fn note_released(&self, chunks: &[ChunkId], device: &DeviceId, now: u64) -> Result<()> {
        self.index.note_released(chunks, device, now)
    }

    /// When this node last released `device`'s copy of `chunk`.
    pub fn released_at(&self, chunk: &ChunkId, device: &DeviceId) -> Result<Option<u64>> {
        self.index.released_at(chunk, device)
    }

    /// Stop counting `device` towards replication until `until` (capped at
    /// [`crate::holders::MAX_DRAIN`] from `now`): it said it is draining.
    pub fn note_draining(&self, device: &DeviceId, until: u64, now: u64) -> Result<bool> {
        self.index.note_draining(device, until, now)
    }

    /// Whether `device` is draining at `now`.
    pub fn is_draining(&self, device: &DeviceId, now: u64) -> Result<bool> {
        self.index.is_draining(device, now)
    }

    /// Live chunks `device` holds that no other live, non-draining machine
    /// holds. See [`Index::exposed_by`].
    pub fn exposed_by(&self, device: &DeviceId, now: u64) -> Result<u64> {
        self.index.exposed_by(device, now)
    }

    /// Withdraw every record naming `device`, for a peer that has left.
    pub fn forget_device(&self, device: &DeviceId) -> Result<usize> {
        self.index.forget_device(device)
    }

    /// Chunks to challenge `holder` on, one drawn per cursor.
    ///
    /// The cursors must be unpredictable to the host being audited. A proof of
    /// storage is worth exactly the host's inability to guess the question, and
    /// the ordered version this replaced asked the same sixteen questions every
    /// round for ever. See [`Index::chunks_to_challenge`].
    pub fn chunks_to_challenge(
        &self,
        holder: &DeviceId,
        cursors: &[AuditCursor],
    ) -> Result<Vec<ChunkId>> {
        self.index.chunks_to_challenge(holder, cursors)
    }

    /// One live chunk of this node's own, chosen by where `cursor` lands.
    ///
    /// The probe offered to a paused peer. Drawn by the owner from its own live
    /// set, never taken from the peer's answer about what it is missing: a host
    /// that picks its own examination question can name one small chunk, keep
    /// it, and buy back the terabyte it threw away.
    pub fn live_chunk_near(&self, cursor: &ChunkId) -> Result<Option<ChunkId>> {
        self.index.live_chunk_near(cursor)
    }

    /// Note the one chunk `device` was offered while its sanction was in force.
    pub fn note_probe(&self, device: &DeviceId, chunk: &ChunkId) -> Result<()> {
        self.index.note_probe(device, chunk)
    }

    /// The chunk `device` owes an answer on, if it is paused and has been given
    /// one.
    ///
    /// A paused peer is audited on **this** and nothing else. Drawing its
    /// questions from the ledger instead would draw them from the thousands of
    /// stale records it is paused *for*, so it would fail, so the sanction
    /// would never lift — a ban wearing the words of a suspension.
    pub fn probe(&self, device: &DeviceId) -> Result<Option<ChunkId>> {
        self.index.probe(device)
    }

    /// Live chunks held by fewer than `target` devices, worst first.
    ///
    /// `target` counts this device: a target of three asks for two elsewhere.
    pub fn under_replicated(&self, target: usize, now: u64) -> Result<Vec<AtRisk>> {
        self.index.under_replicated(target, now)
    }

    /// How many complete copies exist on other machines.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn coverage(&self, now: u64) -> Result<Coverage> {
        self.index.coverage(now)
    }
}
