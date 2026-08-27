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
    time::Duration,
};

use itsanas_crypto::{
    ChunkId, DeviceKeys, SealContext, UserId, UserKeys, open_deterministic, seal_deterministic,
};

use crate::{
    blob::BlobStore,
    chunker::ChunkerConfig,
    error::{Result, StoreError},
    index::{Index, now_unix},
    oplog::{FileEntry, LogEntry, Operation, SegmentEnvelope, validate_chain},
    path as logical_path,
};

/// Seal purpose for file chunks.
pub const CHUNK_SEAL_PURPOSE: &str = "chunk";

/// A device's local state.
#[derive(Debug)]
pub struct Store {
    root: PathBuf,
    blobs: BlobStore,
    index: Index,
    chunker: ChunkerConfig,
    user: UserKeys,
    device: DeviceKeys,
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
    pub bytes_on_disk: u64,
    pub segments: u64,
    pub unsealed_entries: usize,
}

impl Store {
    /// Open (creating if needed) a store at `root`.
    pub fn open(root: impl AsRef<Path>, user: UserKeys, device: DeviceKeys) -> Result<Self> {
        Self::open_with_chunker(root, user, device, ChunkerConfig::default())
    }

    /// Open with a non-default chunker, for tests and for tuning experiments.
    pub fn open_with_chunker(
        root: impl AsRef<Path>,
        user: UserKeys,
        device: DeviceKeys,
        chunker: ChunkerConfig,
    ) -> Result<Self> {
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

    /// The sealing context for one chunk.
    fn chunk_context<'a>(&self, address: &'a ChunkId) -> SealContext<'a> {
        SealContext {
            purpose: CHUNK_SEAL_PURPOSE,
            owner: self.user.user_id(),
            address: address.as_bytes(),
        }
    }

    // ----------------------------------------------------------------- write

    /// Chunk, seal and store `plaintext` under the logical path `path`.
    ///
    /// Returns the resulting entry. The write is recorded in the operation log
    /// as a pending entry; call [`Self::flush_segment`] to seal it into a
    /// segment that peers can replicate.
    pub fn write_file(&self, path: &str, plaintext: &[u8]) -> Result<FileEntry> {
        logical_path::validate(path)?;

        let mut chunks = Vec::new();
        for chunk in self.chunker.split(plaintext) {
            let address = self.user.chunk_id(chunk.data);
            let sealed = seal_deterministic(
                self.user.chunk_root(),
                &self.chunk_context(&address),
                chunk.data,
            )?;
            self.blobs.put(&address, &sealed)?;
            chunks.push(address);
        }

        let entry = FileEntry {
            size: plaintext.len() as u64,
            modified_unix: now_unix(),
            content_hash: *blake3::hash(plaintext).as_bytes(),
            chunks,
        };

        // Index first, log second. If the process dies between the two, the
        // file is readable locally and simply has not been announced yet —
        // which the next flush corrects. The reverse order would announce a
        // file this device cannot serve.
        self.index.put_file(path, &entry)?;
        self.index.push_pending(&LogEntry {
            sequence: self.index.next_sequence()?,
            recorded_unix: entry.modified_unix,
            operation: Operation::Upsert {
                path: path.to_owned(),
                entry: entry.clone(),
            },
        })?;

        Ok(entry)
    }

    /// Delete a file, leaving a tombstone in the log.
    pub fn remove_file(&self, path: &str) -> Result<bool> {
        logical_path::validate(path)?;

        if self.index.get_file(path)?.is_none() {
            return Ok(false);
        }

        self.index.remove_file(path)?;
        self.index.push_pending(&LogEntry {
            sequence: self.index.next_sequence()?,
            recorded_unix: now_unix(),
            operation: Operation::Remove {
                path: path.to_owned(),
            },
        })?;

        Ok(true)
    }

    // ------------------------------------------------------------------ read

    /// Read a file back, verifying it end to end.
    ///
    /// Each chunk is authenticated by its AEAD tag, and the reassembled whole is
    /// then checked against the hash recorded at write time. The second check is
    /// not redundant: per-chunk tags prove each chunk is intact and correctly
    /// addressed, but only the whole-file hash proves the *list* was not
    /// reordered or truncated.
    pub fn read_file(&self, path: &str) -> Result<Option<Vec<u8>>> {
        logical_path::validate(path)?;

        let Some(entry) = self.index.get_file(path)? else {
            return Ok(None);
        };

        let mut plaintext = Vec::with_capacity(usize::try_from(entry.size).unwrap_or(0));
        for address in &entry.chunks {
            let sealed = self
                .blobs
                .get(address)?
                .ok_or_else(|| StoreError::MissingChunk(address.short()))?;

            let chunk = open_deterministic(
                self.user.chunk_root(),
                &self.chunk_context(address),
                &sealed,
            )?;
            plaintext.extend_from_slice(&chunk);
        }

        if plaintext.len() as u64 != entry.size {
            return Err(StoreError::Corrupt(format!(
                "{path}: reassembled {} bytes but the index recorded {}",
                plaintext.len(),
                entry.size
            )));
        }
        if blake3::hash(&plaintext).as_bytes() != &entry.content_hash {
            return Err(StoreError::Corrupt(format!(
                "{path}: reassembled content does not match its recorded hash"
            )));
        }

        Ok(Some(plaintext))
    }

    /// Look up a file's metadata without reading its content.
    pub fn stat(&self, path: &str) -> Result<Option<FileEntry>> {
        logical_path::validate(path)?;
        self.index.get_file(path)
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

    /// Segments from `position` onwards, for a peer catching up.
    pub fn segments_from(&self, position: u64) -> Result<Vec<SegmentEnvelope>> {
        self.index.segments_from(position)
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
                match self.read_file(&path) {
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => report.corrupt_files.push(path.clone()),
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

    /// Coarse statistics, cheap enough for a status command.
    pub fn stats(&self) -> Result<StoreStats> {
        Ok(StoreStats {
            files: self.index.file_count()?,
            live_chunks: self.index.referenced_chunks()?.len(),
            pending_collection: self.index.unreferenced_chunks()?.len(),
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
