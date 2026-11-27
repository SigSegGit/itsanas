//! Writes: a file chunked, sealed and logged, removed, placed back, brought
//! back from an earlier version, released, or adopted from a peer.

use itsanas_crypto::{ChunkId, DeviceId};

use super::{Release, ReleaseReport, Store};
use crate::{
    error::{Result, StoreError},
    index::now_unix,
    local::LocalState,
    oplog::{FileEntry, LogEntry, Operation, Tombstone},
    path as logical_path,
    version::VersionVector,
};

impl Store {
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
    ///
    /// # A refused write leaves nothing behind
    ///
    /// Under a [`WriteBudget`](crate::store::WriteBudget), the chunk that would take the account past it
    /// is never stored, and the blobs this call already created are removed
    /// again, with no index entry and nothing in the log. Every other failure
    /// part-way through does the same. Before, it left those blobs as orphans
    /// for `verify` to report: harmless one at a time, and a disk filling with
    /// the first 100 MB of a file refused once per folder pass.
    pub fn write_stream<R: std::io::Read>(&self, path: &str, reader: R) -> Result<FileEntry> {
        logical_path::validate(path)?;
        self.refuse_if_twin()?;

        let _guard = self.write_lock.lock().map_err(|_| {
            StoreError::Corrupt("the store write lock was poisoned by a panic".to_owned())
        })?;

        let mut chunks = Vec::new();
        let mut hasher = blake3::Hasher::new();
        let mut size: u64 = 0;

        // Read once, under the write lock: nothing else writing here can move
        // it, and a stream is refused against the account as it stood when the
        // write began.
        let budget = self.write_budget()?;
        let account = match budget {
            Some(budget) => self.account_besides(path, budget)?,
            None => 0,
        };
        // Blobs this call brought into existence, and only those. A chunk that
        // was already here belongs to some other file too, and removing it on
        // the way out would destroy that file.
        let mut created: Vec<ChunkId> = Vec::new();

        let streamed = crate::chunker::split_stream(&self.chunker, reader, |chunk| {
            // Hash as we go rather than over a reassembled copy, so the
            // whole-file digest costs no extra memory and no second pass.
            hasher.update(chunk);
            size += chunk.len() as u64;

            if let Some(budget) = budget {
                Self::refuse_past(path, budget, account, account.saturating_add(size))?;
            }

            // `seal_chunk` derives the address from the content itself, so the
            // fixed-nonce construction's precondition cannot be violated here.
            let (address, sealed) = self.user.seal_chunk(chunk)?;
            if self.blobs.put(&address, &sealed)? {
                created.push(address);
            }
            chunks.push(address);
            Ok(())
        });

        if let Err(error) = streamed {
            for address in &created {
                // The refusal is what the caller needs to hear; a blob that
                // will not go is an orphan `verify` reports, as before.
                let _ = self.blobs.remove(address);
            }
            return Err(error);
        }

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

    /// Publish `kept` again under `path`, as a new version written here: a file
    /// another machine deleted, that this one protected
    /// ([`crate::deletion::restore_protected`]). Its chunks are the ones it had;
    /// those not here are fetched like any file's.
    ///
    /// # Errors
    ///
    /// When the path is not valid or the index cannot be written.
    pub fn republish(&self, path: &str, kept: &FileEntry) -> Result<FileEntry> {
        logical_path::validate(path)?;
        let _guard = self.write_lock.lock().map_err(|_| {
            StoreError::Corrupt("the store write lock was poisoned by a panic".to_owned())
        })?;
        let sequence = self.index.next_sequence()?;
        let entry = FileEntry {
            version: self
                .base_version(path)?
                .advanced(self.device.device_id(), sequence),
            author: self.device.device_id(),
            modified_unix: now_unix(),
            ..kept.clone()
        };
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

    /// Delete a file, leaving a tombstone behind and in the log.
    pub fn remove_file(&self, path: &str) -> Result<bool> {
        logical_path::validate(path)?;
        self.refuse_if_twin()?;

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

    /// Delete a file this device knows only from the log: no index entry, no
    /// content, and `seen` the newest version it has read.
    ///
    /// The tombstone advances `seen`, so it beats exactly what this device saw.
    /// An edit it did not see is concurrent with it and wins: an unexpected file
    /// costs a second, a lost edit cannot be undone.
    pub(crate) fn remove_unheld(&self, path: &str, seen: &VersionVector) -> Result<bool> {
        logical_path::validate(path)?;

        let _guard = self.write_lock.lock().map_err(|_| {
            StoreError::Corrupt("the store write lock was poisoned by a panic".to_owned())
        })?;

        let sequence = self.index.next_sequence()?;
        let mut base = self.base_version(path)?;
        base.merge(seen);
        let tombstone = Tombstone {
            version: base.advanced(self.device.device_id(), sequence),
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

    /// Write `kept` under `path` as this device's newest version, keeping its
    /// date, size and chunks: what a rename writes under the new name. The
    /// content is the same, so it is not an edit, and nothing is read or
    /// chunked again.
    ///
    /// `held` says whether the content is on this device. When it is not, as on
    /// a phone that lists the account without downloading it, nothing enters
    /// the index (as [`Self::remove_unheld`]): the log alone carries the name,
    /// and the listing shows it as not here, which it is not.
    pub(crate) fn place(&self, path: &str, kept: &FileEntry, held: bool) -> Result<FileEntry> {
        logical_path::validate(path)?;
        let _guard = self.write_lock.lock().map_err(|_| {
            StoreError::Corrupt("the store write lock was poisoned by a panic".to_owned())
        })?;
        let sequence = self.index.next_sequence()?;
        let entry = FileEntry {
            version: self
                .base_version(path)?
                .advanced(self.device.device_id(), sequence),
            author: self.device.device_id(),
            ..kept.clone()
        };
        if held {
            self.index.put_file(path, &entry)?;
        }
        self.index.push_pending(&LogEntry {
            sequence,
            recorded_unix: now_unix(),
            operation: Operation::Upsert {
                path: path.to_owned(),
                entry: entry.clone(),
            },
        })?;
        Ok(entry)
    }

    /// Make `kept`, an earlier version of `path` whose chunks are all here, the
    /// newest, dated now: what bringing a version back writes. Its version
    /// advances both what this device holds and `seen`, the newest the log
    /// shows, so it replaces the current version on every machine rather than
    /// standing beside it as a conflict.
    pub(crate) fn bring_back(
        &self,
        path: &str,
        kept: &FileEntry,
        seen: &crate::VersionVector,
    ) -> Result<FileEntry> {
        logical_path::validate(path)?;
        let _guard = self.write_lock.lock().map_err(|_| {
            StoreError::Corrupt("the store write lock was poisoned by a panic".to_owned())
        })?;
        let sequence = self.index.next_sequence()?;
        let mut base = self.base_version(path)?;
        base.merge(seen);
        let entry = FileEntry {
            version: base.advanced(self.device.device_id(), sequence),
            author: self.device.device_id(),
            modified_unix: now_unix(),
            ..kept.clone()
        };
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

    /// Stop holding the content of `path` on this device, without deleting it
    /// from the account.
    ///
    /// # Why a budget needs this to be a budget
    ///
    /// Refusing to download more is only half of a limit. A device that fills
    /// its allowance and can never let anything go is a ratchet: from that
    /// moment the file edited this morning never arrives, because the one from
    /// six years ago is still sitting there. Measured on the trial device --
    /// told to keep 200 KiB, holding 907 KiB of blobs after a single explicit
    /// fetch, with nothing in the system able to bring it back down.
    ///
    /// # The refusal is the important part
    ///
    /// This is the one operation in the store that destroys data if it is
    /// wrong, so it will not act on a memory. Every chunk that would actually
    /// be deleted -- a chunk another kept file still references is not deleted
    /// and does not need to qualify -- must clear two bars at once:
    ///
    /// * [`SAFE_TO_RELEASE`](crate::holders::SAFE_TO_RELEASE) holders whose
    ///   **machine** has been heard from within
    ///   [`CONFIRMED_FOR`](crate::holders::CONFIRMED_FOR), and
    /// * at least one holder whose record **for this chunk** was refreshed
    ///   inside the same window.
    ///
    /// The second bar is the one that was missing, and its absence was not
    /// theoretical: a per-machine rule cannot tell a peer that still has your
    /// data from one that emptied its disk and stayed online. A record from a
    /// machine nobody has seen in a fortnight is not a copy; a record nobody has
    /// re-confirmed about *this chunk* is not much better. One copy left
    /// standing is not a floor but the last one. If any chunk fails, nothing at
    /// all is released and the caller is told which chunk stopped it and what
    /// the evidence was.
    ///
    /// # What the rest of the system then sees
    ///
    /// The file leaves the index, so `stat` and `list` no longer report it, and
    /// [`catalogue`](mod@crate::catalogue) lists it as [`Presence::Absent`] because
    /// the log in the vault still describes it. `itsanas get` fetches it back on
    /// demand. If this device mirrors a real directory, the folder layer's next
    /// pass sees content in the ledger and none in the store and removes the
    /// file from disk -- which is the intended meaning of "this device keeps
    /// two gigabytes", and is safe precisely because of the refusal above.
    ///
    /// # The clock is an argument
    ///
    /// Like [`Store::coverage`], because the answer depends on it and a test
    /// that cannot move it cannot reach the case that matters: a holder
    /// recorded and then silent for a fortnight. Recording an acknowledgement
    /// *is* contact -- `record_holders` stamps `DEVICE_SEEN` -- so the stale
    /// case is unreachable at the current time by construction.
    ///
    /// # Errors
    ///
    /// If the index or the blob store cannot be read or written.
    ///
    /// [`Presence::Absent`]: crate::catalogue::Presence::Absent
    pub fn release(&self, path: &str, now: u64) -> Result<Release> {
        logical_path::validate(path)?;

        let _guard = self.write_lock.lock().map_err(|_| {
            StoreError::Corrupt("the store write lock was poisoned by a panic".to_owned())
        })?;

        let Some(entry) = self.index.get_file(path)? else {
            return Ok(Release::NotHere);
        };

        for address in &entry.chunks {
            // A chunk another file still references survives this release, so
            // it is not the release that would lose it and it does not have to
            // qualify. Counted against this file's own references rather than
            // one, because a file may legitimately contain the same chunk
            // twice.
            let own = entry
                .chunks
                .iter()
                .filter(|chunk| *chunk == address)
                .count() as u64;
            if self.index.reference_count(address)? > own {
                continue;
            }

            // `proved`, not `live`. A live holder is one that has been heard
            // from; a proved one has answered a storage challenge. Anybody can
            // be heard from -- `Request::Hosted` takes a claim from any device
            // that completes a handshake, and a device that only ever dials in
            // is never audited, so its claim would have stayed unexamined for
            // ever. Releasing is the one operation that reduces the number of
            // copies on purpose, so it is the one that must not run on a claim.
            let evidence = self.index.holder_evidence(address, now)?;
            if evidence.proved < crate::holders::SAFE_TO_RELEASE || evidence.fresh == 0 {
                return Ok(Release::NotSafeYet {
                    chunk: *address,
                    evidence,
                });
            }
        }

        let dropped = self.index.release_file(path)?;
        let mut report = ReleaseReport::default();
        for address in &dropped {
            if let Some(size) = self.blobs.size_of(address)?
                && self.blobs.remove(address)?
            {
                report.chunks += 1;
                report.bytes = report.bytes.saturating_add(size);
            }
            // `release_chunk`, not `forget_chunk`: the file is still in the
            // account and the copies elsewhere are what made this safe. Erasing
            // the ledger would leave this device unable to say how replicated
            // its own account is -- and unable ever to release that file again
            // after fetching it back, because the record of the second holder
            // would be gone.
            self.index.release_chunk(address)?;
        }

        Ok(Release::Gone(report))
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
}
