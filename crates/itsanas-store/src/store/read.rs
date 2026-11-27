//! Reads: a file's content verified end to end, its entry, the account's
//! paths, and the twin this device found signing its own chain.

use super::{Store, TWIN};
use crate::{
    error::{Result, StoreError},
    oplog::{FileEntry, SegmentEnvelope},
    path as logical_path,
};

impl Store {
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

    /// What [`Self::note_twin`] recorded, when another disk holds this
    /// machine's identity: this store then writes nothing.
    #[must_use]
    pub fn twin(&self) -> Option<String> {
        std::fs::read_to_string(self.root.join(TWIN))
            .ok()
            .map(|text| text.trim().to_owned())
    }

    pub(super) fn refuse_if_twin(&self) -> Result<()> {
        self.twin().map_or(Ok(()), |why| Err(StoreError::Twin(why)))
    }

    /// Record that another disk holds this machine's identity, from `proof`:
    /// a segment signed by this machine's own key that its own chain does not
    /// hold, at a position where it holds another -- the two copies of one
    /// key image have each written their own changes. From then on this
    /// store writes nothing and seals no segment, and deletes nothing.
    ///
    /// Checked here, not by the caller: a peer's word that "you have a twin"
    /// would let any peer stop a machine writing; a signature by this
    /// machine's own key cannot be made by anybody else.
    ///
    /// # Errors
    ///
    /// When the proof is not one, or the record cannot be written.
    pub fn note_twin(&self, proof: &SegmentEnvelope) -> Result<()> {
        proof.verify_signature()?;
        if proof.device != self.device_id() || proof.owner != self.owner() {
            return Err(StoreError::Corrupt(
                "a segment of another device or account is no twin of this one".to_owned(),
            ));
        }
        if self
            .segments()?
            .iter()
            .any(|own| own.segment_id == proof.segment_id)
        {
            return Err(StoreError::Corrupt(
                "a segment this machine wrote itself is no twin".to_owned(),
            ));
        }
        let text = format!(
            "segment {} signed by this machine's key, not written here, seen {}",
            proof.segment_id.short(),
            crate::now_unix()
        );
        std::fs::write(self.root.join(TWIN), text).map_err(|error| StoreError::io(TWIN, error))
    }

    /// Let go of the traces of files deleted before `cutoff` (unix seconds),
    /// those dated by a clock never set excepted ([`crate::traces`]); how
    /// many went.
    ///
    /// # Errors
    ///
    /// When the index cannot be written.
    pub fn forget_traces_before(&self, cutoff: u64) -> Result<usize> {
        self.index
            .forget_tombstones_between(crate::traces::CLOCK_FLOOR, cutoff)
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
}
