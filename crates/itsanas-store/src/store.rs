//! The store: the whole of a device's local state, behind one type.
//!
//! A [`Store`] owns a user's identity, a device key, a blob directory and an
//! index, and it is the only place in the codebase where plaintext and
//! ciphertext meet. Everything above it deals in sealed bytes.
//!
//! # What this store does and does not hold
//!
//! It holds **the owner's own data**: their chunks, their index, their log.
//! Chunks this device hosts *for other people* are a different concern and will
//! live in a separate area, because they are opaque bytes with no index, no
//! plaintext and no key. Keeping the two apart means there is no code path in
//! which hosting could accidentally decrypt, and no path in which garbage
//! collection of your own data could reach a peer's.

use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use itsanas_crypto::{
    ChunkId, DeviceId, DeviceKeys, ObjectId, UserId, UserKeys, is_published_test_identity,
};

use crate::{
    blob::BlobStore,
    chunker::ChunkerConfig,
    error::{Result, StoreError},
    holders::{AtRisk, Holder},
    index::{Index, now_unix},
    local::LocalState,
    oplog::{
        FileEntry, LogEntry, Operation, SegmentBody, SegmentEnvelope, Tombstone, validate_chain,
    },
    path as logical_path,
    vault::Vault,
    version::VersionVector,
};

/// Seal purpose for file chunks.
///
/// Re-exported from the crypto crate so there is exactly one definition: a
/// second copy that drifted would derive different keys and silently produce
/// chunks nothing else can read.
pub use itsanas_crypto::seal::CHUNK_PURPOSE as CHUNK_SEAL_PURPOSE;

/// A device's local state.
#[derive(Debug)]
pub struct Store {
    root: PathBuf,
    blobs: BlobStore,
    index: Index,
    chunker: ChunkerConfig,
    user: UserKeys,
    device: DeviceKeys,
    /// Serialises the read-modify-write cycle that produces a new version.
    ///
    /// Every mutating method takes `&self`, so two threads could otherwise read
    /// the same base version, both stamp a successor, and produce a lost
    /// update that looks causally valid — the worst kind, because nothing
    /// downstream would flag it as a conflict.
    write_lock: Mutex<()>,
}

/// What one garbage-collection pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    /// Blobs deleted from disk.
    pub blobs_removed: usize,
    /// Bytes reclaimed.
    pub bytes_reclaimed: u64,
    /// Chunks that were unreferenced but still inside the grace period.
    pub retained_in_grace: usize,
    /// Chunks the index had queued whose blob was already gone.
    pub already_absent: usize,
}

/// What an integrity check found.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IntegrityReport {
    /// Files checked.
    pub files_checked: usize,
    /// Chunks referenced by the index but missing from disk, with the path that
    /// wanted them.
    pub missing_chunks: Vec<(String, ChunkId)>,
    /// Blobs on disk that the index does not account for at all. These are
    /// leaked, not dangerous — usually a crash between writing a blob and
    /// committing its index entry.
    pub orphan_blobs: Vec<ChunkId>,
    /// Files whose reassembled content did not match its recorded hash.
    pub corrupt_files: Vec<String>,
    /// Whether the operation-log chain is intact.
    pub chain_intact: bool,
}

impl IntegrityReport {
    /// Whether the store is fully healthy.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        self.missing_chunks.is_empty() && self.corrupt_files.is_empty() && self.chain_intact
    }
}

/// Coarse size and count statistics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreStats {
    pub files: u64,
    pub live_chunks: usize,
    pub pending_collection: usize,
    /// How many (chunk, device) acknowledgements the placement ledger holds.
    ///
    /// Reported so an operator can see that replication is being recorded at
    /// all. Zero with a non-empty store means every chunk exists only here.
    pub holder_records: u64,
    pub bytes_on_disk: u64,
    pub segments: u64,
    pub unsealed_entries: usize,
}

impl Store {
    /// Open (creating if needed) a store at `root`.
    ///
    /// Refuses the published test identities. Their recovery phrases are
    /// printed in `docs/TEST-USERS.md`, so anyone at all can derive their keys;
    /// a store holding real data under one of them offers no protection
    /// whatsoever.
    pub fn open(root: impl AsRef<Path>, user: UserKeys, device: DeviceKeys) -> Result<Self> {
        Self::open_inner(root, user, device, ChunkerConfig::default(), false)
    }

    /// Open with a non-default chunker, for tuning experiments.
    pub fn open_with_chunker(
        root: impl AsRef<Path>,
        user: UserKeys,
        device: DeviceKeys,
        chunker: ChunkerConfig,
    ) -> Result<Self> {
        Self::open_inner(root, user, device, chunker, false)
    }

    /// Open a store for one of the published test identities.
    ///
    /// Exists so the fixture users in `itsanas-testkit` can be exercised
    /// end to end. Named to be impossible to reach for by accident, and never
    /// called from a code path that handles a real user's data.
    pub fn open_for_testing(
        root: impl AsRef<Path>,
        user: UserKeys,
        device: DeviceKeys,
        chunker: ChunkerConfig,
    ) -> Result<Self> {
        Self::open_inner(root, user, device, chunker, true)
    }

    fn open_inner(
        root: impl AsRef<Path>,
        user: UserKeys,
        device: DeviceKeys,
        chunker: ChunkerConfig,
        allow_published_test_identity: bool,
    ) -> Result<Self> {
        if !allow_published_test_identity && is_published_test_identity(&user.user_id()) {
            return Err(StoreError::PublishedTestIdentity(user.user_id().short()));
        }

        let root = root.as_ref().to_owned();
        std::fs::create_dir_all(&root).map_err(|error| StoreError::io(root.clone(), error))?;

        let blobs = BlobStore::open(&root)?;
        // A previous run may have died mid-write. Clearing staging on open is
        // the only moment we can be sure no write is in flight.
        blobs.sweep_staging()?;

        let index = Index::open(root.join("index.redb"))?;

        Ok(Self {
            root,
            blobs,
            index,
            chunker,
            user,
            device,
            write_lock: Mutex::new(()),
        })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn owner(&self) -> UserId {
        self.user.user_id()
    }

    #[must_use]
    pub const fn chunker(&self) -> &ChunkerConfig {
        &self.chunker
    }

    // ----------------------------------------------------------------- write

    /// Chunk, seal and store everything `reader` produces, under `path`.
    ///
    /// **This is the real write path.** Memory use is bounded by the chunker's
    /// window — about half a megabyte at the default settings — regardless of
    /// how large the file is. That matters more than it sounds: the deployment
    /// target is a 1 TB array on a Raspberry Pi with a few gigabytes of RAM, and
    /// a design that materialises each file before storing it does not fail
    /// slowly on a 6 GB video, it gets killed by the kernel.
    ///
    /// Returns the resulting entry. The write is recorded in the operation log
    /// as a pending entry; call [`Self::flush_segment`] to seal it into a
    /// segment that peers can replicate.
    pub fn write_stream<R: std::io::Read>(&self, path: &str, reader: R) -> Result<FileEntry> {
        logical_path::validate(path)?;

        let _guard = self.write_lock.lock().map_err(|_| {
            StoreError::Corrupt("the store write lock was poisoned by a panic".to_owned())
        })?;

        let mut chunks = Vec::new();
        let mut hasher = blake3::Hasher::new();
        let mut size: u64 = 0;

        crate::chunker::split_stream(&self.chunker, reader, |chunk| {
            // Hash as we go rather than over a reassembled copy, so the
            // whole-file digest costs no extra memory and no second pass.
            hasher.update(chunk);
            size += chunk.len() as u64;

            // `seal_chunk` derives the address from the content itself, so the
            // fixed-nonce construction's precondition cannot be violated here.
            let (address, sealed) = self.user.seal_chunk(chunk)?;
            self.blobs.put(&address, &sealed)?;
            chunks.push(address);
            Ok(())
        })?;

        // Build on whatever this path's history already is: the live entry if
        // there is one, otherwise the tombstone left by a delete, otherwise
        // nothing. Skipping the tombstone would make re-creating a deleted file
        // look concurrent with the delete rather than after it.
        let sequence = self.index.next_sequence()?;
        let base = self.base_version(path)?;

        let entry = FileEntry {
            size,
            modified_unix: now_unix(),
            content_hash: *hasher.finalize().as_bytes(),
            chunks,
            version: base.advanced(self.device.device_id(), sequence),
            author: self.device.device_id(),
        };

        // Index first, log second. If the process dies between the two, the
        // file is readable locally and simply has not been announced yet —
        // which the next flush corrects. The reverse order would announce a
        // file this device cannot serve.
        self.index.put_file(path, &entry)?;
        self.index.push_pending(&LogEntry {
            sequence,
            recorded_unix: entry.modified_unix,
            operation: Operation::Upsert {
                path: path.to_owned(),
                entry: entry.clone(),
            },
        })?;

        Ok(entry)
    }

    /// Store a whole buffer under `path`.
    ///
    /// Convenience over [`Self::write_stream`] for content that is already in
    /// memory and small enough to belong there — a note, a config file, a test
    /// fixture. Anything read from a file should use `write_stream` and let the
    /// bytes flow past rather than pile up.
    pub fn write_file(&self, path: &str, plaintext: &[u8]) -> Result<FileEntry> {
        self.write_stream(path, plaintext)
    }

    /// Delete a file, leaving a tombstone behind and in the log.
    pub fn remove_file(&self, path: &str) -> Result<bool> {
        logical_path::validate(path)?;

        let _guard = self.write_lock.lock().map_err(|_| {
            StoreError::Corrupt("the store write lock was poisoned by a panic".to_owned())
        })?;

        if self.index.get_file(path)?.is_none() {
            return Ok(false);
        }

        let sequence = self.index.next_sequence()?;
        let tombstone = Tombstone {
            version: self
                .base_version(path)?
                .advanced(self.device.device_id(), sequence),
            removed_unix: now_unix(),
            author: self.device.device_id(),
        };

        self.index.remove_file(path, &tombstone)?;
        self.index.push_pending(&LogEntry {
            sequence,
            recorded_unix: tombstone.removed_unix,
            operation: Operation::Remove {
                path: path.to_owned(),
                version: tombstone.version.clone(),
            },
        })?;

        Ok(true)
    }

    /// The version currently stamped on `path`, live or deleted.
    fn base_version(&self, path: &str) -> Result<VersionVector> {
        if let Some(entry) = self.index.get_file(path)? {
            return Ok(entry.version);
        }
        if let Some(tombstone) = self.index.get_tombstone(path)? {
            return Ok(tombstone.version);
        }
        Ok(VersionVector::new())
    }

    /// This device's identity, as it appears in version vectors.
    #[must_use]
    pub fn device_id(&self) -> DeviceId {
        self.device.device_id()
    }

    /// The tombstone at `path`, if it was deleted.
    pub fn tombstone(&self, path: &str) -> Result<Option<Tombstone>> {
        logical_path::validate(path)?;
        self.index.get_tombstone(path)
    }

    /// Every tombstone this store holds.
    pub fn tombstones(&self) -> Result<Vec<(String, Tombstone)>> {
        self.index.tombstones()
    }

    /// What this device last saw on disk at `path`.
    pub fn local_state(&self, path: &str) -> Result<Option<LocalState>> {
        self.index.get_local_state(path)
    }

    /// Record what this device just wrote to, or read from, disk.
    pub fn set_local_state(&self, path: &str, state: &LocalState) -> Result<()> {
        self.index.set_local_state(path, state)
    }

    /// Forget a path's disk state, after removing it from disk.
    pub fn clear_local_state(&self, path: &str) -> Result<()> {
        self.index.clear_local_state(path)
    }

    /// Every path this device believes it has on disk.
    pub fn local_states(&self) -> Result<Vec<(String, LocalState)>> {
        self.index.local_states()
    }

    /// Install an entry that arrived from a peer, without logging it again.
    ///
    /// The peer's own segment already announces this operation; re-logging it
    /// would attribute another device's write to this one and make the two
    /// devices' histories disagree about who did what.
    ///
    /// Conflict resolution is the sync engine's job — by the time this is
    /// called, the decision has been made.
    pub fn adopt_entry(&self, path: &str, entry: &FileEntry) -> Result<()> {
        logical_path::validate(path)?;
        self.index.put_file(path, entry)?;
        Ok(())
    }

    /// Install a tombstone that arrived from a peer, without logging it again.
    pub fn adopt_tombstone(&self, path: &str, tombstone: &Tombstone) -> Result<()> {
        logical_path::validate(path)?;
        self.index.remove_file(path, tombstone)?;
        Ok(())
    }

    // ------------------------------------------------------------------ read

    /// Write a file's content to `writer`, verifying it end to end.
    ///
    /// Returns `false` if the path is unknown. Memory use is one chunk at a
    /// time, so a 6 GB file costs a quarter of a megabyte rather than 6 GB.
    ///
    /// Each chunk is authenticated by its AEAD tag, and the whole is then
    /// checked against the hash recorded at write time. The second check is not
    /// redundant: per-chunk tags prove each chunk is intact and correctly
    /// addressed, but only the whole-file hash proves the *list* was not
    /// reordered or truncated.
    ///
    /// # The caller must not publish the output until this returns `Ok`
    ///
    /// Verification necessarily happens at the end — there is nothing to hash
    /// until the last byte has gone past. So bytes reach `writer` *before* they
    /// are known to be correct. Write to a temporary file and rename on success,
    /// which is what [`itsanas_folder`](https://docs.rs/itsanas-folder) does.
    /// Writing straight to the destination would leave corrupt content in place
    /// on failure, and the next scan would hash it and replicate the corruption.
    pub fn read_stream<W: std::io::Write>(&self, path: &str, mut writer: W) -> Result<bool> {
        logical_path::validate(path)?;

        let Some(entry) = self.index.get_file(path)? else {
            return Ok(false);
        };

        let mut hasher = blake3::Hasher::new();
        let mut written: u64 = 0;

        for address in &entry.chunks {
            let sealed = self
                .blobs
                .get(address)?
                .ok_or_else(|| StoreError::MissingChunk(address.short()))?;

            let chunk = self.user.open_chunk(address, &sealed)?;
            hasher.update(&chunk);
            written += chunk.len() as u64;
            writer.write_all(&chunk)?;
        }

        if written != entry.size {
            return Err(StoreError::Corrupt(format!(
                "{path}: reassembled {written} bytes but the index recorded {}",
                entry.size
            )));
        }
        if hasher.finalize().as_bytes() != &entry.content_hash {
            return Err(StoreError::Corrupt(format!(
                "{path}: reassembled content does not match its recorded hash"
            )));
        }

        writer.flush()?;
        Ok(true)
    }

    /// Read a whole file into memory.
    ///
    /// Convenience over [`Self::read_stream`] for content small enough to
    /// belong in memory. Anything destined for a file should use `read_stream`.
    pub fn read_file(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let mut out = Vec::new();
        if self.read_stream(path, &mut out)? {
            Ok(Some(out))
        } else {
            Ok(None)
        }
    }

    /// Look up a file's metadata without reading its content.
    pub fn stat(&self, path: &str) -> Result<Option<FileEntry>> {
        logical_path::validate(path)?;
        self.index.get_file(path)
    }

    /// Every path and its entry, sorted by path.
    pub fn entries(&self) -> Result<Vec<(String, FileEntry)>> {
        self.index.files()
    }

    /// Every path in the store, sorted.
    pub fn list(&self) -> Result<Vec<String>> {
        Ok(self
            .index
            .files()?
            .into_iter()
            .map(|(path, _)| path)
            .collect())
    }

    // ------------------------------------------------------------------- log

    /// Seal every pending entry into one signed segment.
    ///
    /// Returns `None` when there is nothing to seal.
    pub fn flush_segment(&self) -> Result<Option<SegmentEnvelope>> {
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

    /// Store a sealed chunk fetched from a peer.
    ///
    /// The bytes are authenticated when they are next opened, not here — a
    /// chunk that fails to open is caught by [`Self::read_file`], which is the
    /// only place it could do harm.
    pub fn put_sealed_chunk(&self, address: &ChunkId, sealed: &[u8]) -> Result<bool> {
        self.blobs.put(address, sealed)
    }

    /// Whether this store already holds the sealed bytes for `address`.
    #[must_use]
    pub fn has_chunk(&self, address: &ChunkId) -> bool {
        self.blobs.contains(address)
    }

    /// Segments from `position` onwards, for a peer catching up.
    pub fn segments_from(&self, position: u64) -> Result<Vec<SegmentEnvelope>> {
        self.index.segments_from(position)
    }

    /// The most recent segment this device wrote, if any.
    pub fn head_segment(&self) -> Result<Option<ObjectId>> {
        self.index.head_segment()
    }

    /// How many segments this device's chain holds.
    pub fn chain_length(&self) -> Result<u64> {
        self.index.chain_length()
    }

    // --------------------------------------------------------- housekeeping

    /// Delete chunks that have been unreferenced for longer than `grace`.
    ///
    /// The grace period exists because "unreferenced" is a local judgement made
    /// with incomplete information: a peer may still be fetching a chunk whose
    /// file this device just deleted. Collecting immediately would race that
    /// fetch and turn an ordinary delete into an unrecoverable hole.
    pub fn collect_garbage(&self, grace: Duration) -> Result<GcReport> {
        let cutoff = now_unix().saturating_sub(grace.as_secs());
        let mut report = GcReport::default();

        for (address, unreferenced_since) in self.index.unreferenced_chunks()? {
            if unreferenced_since > cutoff {
                report.retained_in_grace += 1;
                continue;
            }

            match self.blobs.size_of(&address)? {
                Some(size) => {
                    if self.blobs.remove(&address)? {
                        report.blobs_removed += 1;
                        report.bytes_reclaimed = report.bytes_reclaimed.saturating_add(size);
                    }
                }
                None => report.already_absent += 1,
            }

            self.index.forget_chunk(&address)?;
        }

        Ok(report)
    }

    /// Check that everything the index promises is actually on disk and intact.
    ///
    /// `deep` additionally reassembles and hashes every file, which is O(data)
    /// rather than O(metadata) and is meant for `itsanas doctor`, not for a
    /// routine loop.
    pub fn verify_integrity(&self, deep: bool) -> Result<IntegrityReport> {
        let mut report = IntegrityReport {
            chain_intact: true,
            ..IntegrityReport::default()
        };

        let mut referenced: std::collections::BTreeSet<ChunkId> = std::collections::BTreeSet::new();

        for (path, entry) in self.index.files()? {
            report.files_checked += 1;

            for address in &entry.chunks {
                referenced.insert(*address);
                if !self.blobs.contains(address) {
                    report.missing_chunks.push((path.clone(), *address));
                }
            }

            if deep && !report.missing_chunks.iter().any(|(p, _)| p == &path) {
                // Into a sink: the point is to verify, not to keep. Reading
                // into a buffer would make `doctor --deep` need as much memory
                // as the largest file in the store.
                match self.read_stream(&path, std::io::sink()) {
                    Ok(true) => {}
                    Ok(false) | Err(_) => report.corrupt_files.push(path.clone()),
                }
            }
        }

        // Chunks queued for collection are legitimately on disk without being
        // referenced, so they are not orphans.
        for (address, _) in self.index.unreferenced_chunks()? {
            referenced.insert(address);
        }

        for address in self.blobs.addresses()? {
            if !referenced.contains(&address) {
                report.orphan_blobs.push(address);
            }
        }

        if let Err(error) = validate_chain(&self.index.segments()?) {
            report.chain_intact = false;
            // The specific break is worth surfacing, but a report is a value,
            // not a failure — the caller decides how loud to be.
            let _ = error;
        }

        Ok(report)
    }

    // ---------------------------------------------------- outstanding work

    /// Whether the vault holds work this device has not managed to apply.
    ///
    /// Two lookups per device, not a walk. The answer is *conservative*: a
    /// device whose head has moved is reported as outstanding even if the new
    /// segments would apply cleanly, because finding out costs exactly the walk
    /// this is here to avoid. Being wrong in that direction means one extra
    /// replay; being wrong in the other means a file that never arrives.
    pub fn has_unapplied(&self, vault: &Vault) -> Result<bool> {
        let mine = self.device_id();
        for (device, head, _) in vault.heads_for(self.owner())? {
            if device == mine {
                continue;
            }
            if self.index.applied_head(&device)? != Some(head) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Record that everything the vault currently holds has been applied.
    ///
    /// Call only after a round that deferred nothing. A round with even one
    /// deferral must leave the markers alone, or the work it could not finish
    /// is never looked at again — which is the bug this whole mechanism exists
    /// to make impossible.
    pub fn note_all_applied(&self, vault: &Vault) -> Result<()> {
        let mine = self.device_id();
        for (device, head, _) in vault.heads_for(self.owner())? {
            if device == mine {
                continue;
            }
            self.index.set_applied_head(&device, &head)?;
        }
        Ok(())
    }

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

    /// Every other device known to hold `chunk`.
    pub fn remote_holders(&self, chunk: &ChunkId) -> Result<Vec<Holder>> {
        self.index.remote_holders(chunk)
    }

    /// Withdraw the record that `device` holds `chunk`.
    ///
    /// For a failed storage challenge: a record is evidence that a host once
    /// accepted a chunk, never proof that it still has it.
    pub fn forget_holder(&self, chunk: &ChunkId, device: &DeviceId) -> Result<()> {
        self.index.forget_holder(chunk, device)
    }

    /// Withdraw every record naming `device`, for a peer that has left.
    pub fn forget_device(&self, device: &DeviceId) -> Result<usize> {
        self.index.forget_device(device)
    }

    /// Live chunks held by fewer than `target` devices, worst first.
    ///
    /// `target` counts this device: a target of three asks for two elsewhere.
    pub fn under_replicated(&self, target: usize) -> Result<Vec<AtRisk>> {
        self.index.under_replicated(target)
    }

    /// Coarse statistics, cheap enough for a status command.
    pub fn stats(&self) -> Result<StoreStats> {
        Ok(StoreStats {
            files: self.index.file_count()?,
            live_chunks: self.index.referenced_chunks()?.len(),
            pending_collection: self.index.unreferenced_chunks()?.len(),
            holder_records: self.index.holder_records()?,
            bytes_on_disk: self.blobs.total_bytes()?,
            segments: self.index.chain_length()?,
            unsealed_entries: self.index.pending_entries()?.len(),
        })
    }

    /// The blob store, for the network layer to serve sealed bytes from.
    #[must_use]
    pub const fn blobs(&self) -> &BlobStore {
        &self.blobs
    }
}
