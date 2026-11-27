//! The log and the chunks it names: sealing pending entries into segments,
//! opening one, the chunks missing or lost on this disk, accepting a chunk,
//! and how far behind this device is on other devices' chains.

use itsanas_crypto::{ChunkId, DeviceId, ObjectId};

use super::Store;
use crate::{
    error::{Result, StoreError},
    index::now_unix,
    oplog::{SegmentBody, SegmentEnvelope},
};

impl Store {
    // ------------------------------------------------------------------- log

    /// Seal every pending entry into one signed segment.
    ///
    /// Returns `None` when there is nothing to seal.
    pub fn flush_segment(&self) -> Result<Option<SegmentEnvelope>> {
        self.refuse_if_twin()?;
        let pending = self.index.pending_entries()?;
        if pending.is_empty() {
            return Ok(None);
        }

        let previous = self.index.head_segment()?;
        let envelope = SegmentEnvelope::create(
            self.user.oplog_root(),
            self.user.user_id(),
            &self.device,
            previous,
            pending,
        )?;

        self.index.append_segment(&envelope)?;
        Ok(Some(envelope))
    }

    /// This device's whole segment chain, oldest first.
    pub fn segments(&self) -> Result<Vec<SegmentEnvelope>> {
        self.index.segments()
    }

    /// Verify and open a segment belonging to this store's owner.
    ///
    /// Segments from a user's *other* devices open with the same key: the log
    /// root is derived from the master secret, which every device of that user
    /// holds. That is the whole reason a device can catch up on work done
    /// elsewhere.
    pub fn open_segment(&self, envelope: &SegmentEnvelope) -> Result<SegmentBody> {
        if envelope.owner != self.owner() {
            return Err(StoreError::Corrupt(format!(
                "segment {} belongs to {}, not to this store's owner {}",
                envelope.segment_id.short(),
                envelope.owner.short(),
                self.owner().short()
            )));
        }
        envelope.open(self.user.oplog_root())
    }

    /// Chunks this node's own files need and whose bytes are not on this disk.
    ///
    /// # Why this is not what `doctor` does
    ///
    /// [`Self::verify_integrity`] answers the same question exhaustively and is
    /// a command somebody runs. This is for a daemon, so it is bounded: it
    /// examines at most `scan` live chunks, starting wherever `cursor` lands
    /// and wrapping. At a terabyte, stat-ing fourteen million blobs every round
    /// costs far more than the loss it is looking for, and a scan that is too
    /// expensive to run is a scan nobody runs.
    ///
    /// Successive rounds with fresh cursors cover the store. What that buys is
    /// eventual detection rather than immediate — which is the right shape,
    /// because the thing it detects is a disk quietly losing a file, and a disk
    /// quietly losing a file is not an emergency until you need the file.
    pub fn missing_locally(&self, cursor: &ChunkId, scan: usize) -> Result<Vec<ChunkId>> {
        // What is here may have been recorded as lost and then restored by a
        // filesystem repair or a backup underneath us; clearing it stops
        // repair asking peers for ever about something that came back on its
        // own. Settled in one transaction, and in none when nothing changed:
        // this runs every round, and it used to commit once per chunk it
        // looked at -- four of the eleven commits of an idle round, each
        // some twenty kilobytes on a copy-on-write engine (ROADMAP, "An idle
        // node's disk writes").
        let (present, found): (Vec<ChunkId>, Vec<ChunkId>) = self
            .index
            .live_chunks_from(cursor, scan)?
            .into_iter()
            .partition(|address| self.blobs.contains(address));
        self.index.settle_losses(&present, &found, now_unix())?;
        Ok(found)
    }

    /// Check up to `limit` live chunks against the disk, after `after`, and
    /// return how many were missing and where the next call should start
    /// (`None`: the pass reached the end, start again from the top; a slice
    /// that ends exactly on the last chunk still returns a cursor, and the next
    /// call finds nothing after it and returns `None`).
    ///
    /// The same test as [`Self::missing_locally`], recorded in the same loss
    /// queue, but walked in order from a cursor the caller keeps rather than
    /// drawn at random: the daemon has to cover every chunk once per
    /// `REFRESH_AFTER`, because since §8 4c a peer of this account believes
    /// this node's summary ([`Self::held_summary`]) without asking, and a
    /// random draw at a terabyte would take weeks to land on a given chunk.
    ///
    /// # Errors
    ///
    /// If the index cannot be read or written.
    pub fn check_disk(
        &self,
        after: Option<&ChunkId>,
        limit: usize,
    ) -> Result<(usize, Option<ChunkId>)> {
        let limit = limit.min(crate::vault::MAX_CHECK_BATCH);
        let (page, next) = self.index.live_chunks_page(after, limit)?;
        let (present, missing): (Vec<ChunkId>, Vec<ChunkId>) = page
            .into_iter()
            .partition(|address| self.blobs.contains(address));
        // One transaction for the slice: thousands of rows a round, on an SD
        // card, are not thousands of commits.
        self.index.settle_losses(&present, &missing, now_unix())?;
        Ok((missing.len(), next))
    }

    /// Chunks already known to be missing from this disk, worst problem first.
    ///
    /// Written by whoever found them — `doctor` in one exhaustive pass, the
    /// daemon's sampling scan a slice at a time — and drained by repair. A human
    /// running `doctor` because a file would not open is the fastest detector
    /// this system has, and until these shared a queue their answer had nowhere
    /// to go.
    pub fn known_losses_from(&self, cursor: &ChunkId, limit: usize) -> Result<Vec<ChunkId>> {
        self.index.known_losses_from(cursor, limit)
    }

    /// How many chunks this node is known to be missing.
    pub fn loss_count(&self) -> Result<u64> {
        self.index.loss_count()
    }

    /// Store a sealed chunk that came from a peer, if it is genuine.
    ///
    /// # The rule, and why there is only one method
    ///
    /// **Nothing may make [`Self::has_chunk`] true without proof.** It reads
    /// like belt and braces and it is not: a blob on disk under an address is
    /// how every other part of this system decides it does not need to go
    /// looking. Write noise under a missing chunk's address and the repair scan
    /// stops searching, no other peer is ever asked, the loss queue clears the
    /// entry `doctor` put there, and a file that was **recoverable** becomes
    /// permanently unreadable. A host cannot read what it stores, so that is
    /// its one route to destroying data: answer a request with rubbish.
    ///
    /// This existed as two methods with two different answers. `restore_chunk`
    /// verified, with the argument above written out above it. `put_sealed_chunk`
    /// did not, and argued that authentication could wait because "a chunk that
    /// fails to open is caught by `read_file`, the only place it could do harm"
    /// — which was already untrue when it was written, and stayed untrue
    /// through the commit that spelled out why. The unverified one was on the
    /// **ordinary** path: every chunk pulled from every peer, every round.
    ///
    /// So there is one method. The cost is decrypting each chunk as it arrives
    /// rather than when it is first read, which is work this node was going to
    /// do anyway, moved earlier.
    ///
    /// Returns whether the chunk was accepted.
    pub fn accept_chunk(&self, address: &ChunkId, sealed: &[u8]) -> Result<bool> {
        // Length first, because it is free and decryption is not. The chunker
        // never emits more than its maximum, so anything larger is bogus by
        // construction — and the wire allows eight megabytes a frame, so a
        // peer answering every request at that size would have this node
        // decrypting a quarter of a gigabyte a round for a result known in
        // advance. On a Raspberry Pi that is seconds, every round, for free.
        if sealed.len() > self.chunker.max_size() + itsanas_crypto::seal::DETERMINISTIC_OVERHEAD {
            return Ok(false);
        }

        let Ok(plaintext) = self.user.open_chunk(address, sealed) else {
            return Ok(false);
        };
        if self.user.chunk_id(&plaintext) != *address {
            return Ok(false);
        }
        self.blobs.put(address, sealed)?;
        // It is here, so it is not lost. The queue must shrink on success or
        // repair would keep asking about chunks it has already recovered.
        self.index.clear_loss(address)?;
        Ok(true)
    }

    /// Whether this store already holds the sealed bytes for `address`.
    #[must_use]
    pub fn has_chunk(&self, address: &ChunkId) -> bool {
        self.blobs.contains(address)
    }

    /// The most recent segment this device wrote, if any.
    pub fn head_segment(&self) -> Result<Option<ObjectId>> {
        self.index.head_segment()
    }

    /// How many segments this device's chain holds.
    pub fn chain_length(&self) -> Result<u64> {
        self.index.chain_length()
    }

    /// Record that `device` signed a chain at least `length` segments long
    /// (its presence's `head`). Only grows.
    ///
    /// # Errors
    ///
    /// If the index cannot be written.
    pub fn note_known_head(&self, device: &DeviceId, length: u64) -> Result<()> {
        self.index.note_known_head(device, length)
    }

    /// The account's devices whose chain this vault holds less of than they
    /// signed, with what is held and what they said: a host served only the
    /// start of the chain, or the rest has not reached any host this machine
    /// met yet. Never this device.
    ///
    /// # Errors
    ///
    /// If the index or the vault cannot be read.
    pub fn behind(&self, vault: &crate::Vault) -> Result<Vec<(DeviceId, u64, u64)>> {
        let held: std::collections::BTreeMap<DeviceId, u64> = vault
            .heads_for(self.owner())?
            .into_iter()
            .map(|(device, _, length)| (device, length))
            .collect();
        let mine = self.device_id();
        Ok(self
            .index
            .known_heads()?
            .into_iter()
            .filter(|(device, _)| *device != mine)
            .filter_map(|(device, said)| {
                let have = held.get(&device).copied().unwrap_or(0);
                (have < said).then_some((device, have, said))
            })
            .collect())
    }
}
