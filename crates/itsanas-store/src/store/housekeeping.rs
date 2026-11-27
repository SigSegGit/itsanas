//! Housekeeping: collecting chunks unreferenced past their grace, and
//! checking the whole store against its index and its chain.

use std::time::Duration;

use itsanas_crypto::ChunkId;

use super::{GcReport, IntegrityReport, Store};
use crate::{error::Result, index::now_unix, oplog::validate_chain};

impl Store {
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
        // A file another machine deleted and this one protects is not in the
        // index, and its chunks are all that can bring it back.
        let protected = crate::deletion::protected_chunks(&self.root);

        for (address, unreferenced_since) in self.index.unreferenced_chunks()? {
            if unreferenced_since > cutoff || protected.contains(&address) {
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
        // In pack files a removed chunk is dead space until its pack is
        // rewritten; blob files gave the bytes back at `remove`.
        self.blobs.compact()?;

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
                if self.blobs.contains(address) {
                    continue;
                }
                report.missing_chunks.push((path.clone(), *address));
                // Somebody ran `doctor` because a file would not open. That is
                // the fastest and most reliable detector of local loss this
                // system has, and before this its answer went to a terminal and
                // nowhere else: the daemon's sampling scan would have taken
                // fifty-five days to find the same chunk on a terabyte store.
                self.index.note_loss(address, now_unix())?;
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
}
