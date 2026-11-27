//! Whether this directory is the folder at all, and whether a pass deletes
//! too much to do without somebody looking.

use std::{collections::BTreeMap, path::Path};

use itsanas_crypto::DeviceId;
use itsanas_store::{Store, deletion};

use crate::{Deletions, DiskFile, Folder, FolderError, Result, scan};

/// Fewer deletions than this are never held, whatever the proportion.
///
/// Deleting two files out of three is a Tuesday. The guard is for the shape of
/// an accident -- a folder that emptied itself -- and a threshold without a
/// floor would hold a perfectly ordinary tidy-up on a small folder and teach
/// its owner to pass `--confirm` out of habit, which is how a guard becomes a
/// formality.
pub const DELETIONS_ALWAYS_ALLOWED: usize = 5;

/// What a folder's marker file says about who owns this directory.
#[derive(Debug)]
enum Marker {
    /// Present and naming this device.
    Ours,
    /// Present and naming somebody else.
    Foreign(String),
    /// Not there at all.
    Missing,
}

fn read_marker(root: &Path, device: DeviceId) -> Marker {
    match std::fs::read_to_string(root.join(scan::MARKER)) {
        Ok(text) => {
            let named = text.trim().to_owned();
            if named == device.to_hex() {
                Marker::Ours
            } else {
                Marker::Foreign(named)
            }
        }
        Err(_) => Marker::Missing,
    }
}

/// Write the marker, so that this directory can say what it is next time.
///
/// Best effort on purpose: a read-only folder is a folder somebody is using, and
/// refusing to sync it because a marker could not be written would be a worse
/// failure than the one the marker prevents.
fn write_marker(root: &Path, device: DeviceId) {
    let _ = std::fs::write(root.join(scan::MARKER), device.to_hex());
}

impl Folder {
    /// Decide whether this directory is really the synced folder.
    ///
    /// **The failure this exists for.** An unmounted disk, or a network share
    /// that dropped, leaves its mount point behind as an *empty directory*.
    /// `scan` finds nothing, every file in the ledger looks deleted, and the
    /// next pass writes those deletions into the log -- where they replicate,
    /// as deletions, to every other machine of the account. The disk comes back
    /// an hour later with the files still on it, and the account has already
    /// agreed they were gone. Nothing in the filesystem distinguishes that from
    /// a folder somebody emptied on purpose, which is why there is a marker.
    ///
    /// Three answers:
    ///
    /// * **The marker is there and names this device.** Ordinary.
    /// * **The marker is there and names another device.** Two nodes pointed at
    ///   one directory, which is the other way a folder gets emptied: each one
    ///   deletes what the other wrote. Refused.
    /// * **The marker is missing.** Either this folder predates markers, or the
    ///   storage is not mounted. The ledger decides: if any file it knows about
    ///   is present on disk, the directory is real and gets a marker; if it
    ///   knows files and *none* of them is there, the storage is gone. An empty
    ///   ledger is a new folder, and gets a marker.
    pub(crate) fn check_storage(
        &self,
        store: &Store,
        on_disk: &BTreeMap<String, DiskFile>,
        deletions: Deletions,
    ) -> Result<()> {
        let device = store.device_id();
        match read_marker(&self.root, device) {
            Marker::Ours => return Ok(()),
            // Somebody typed `itsanas folder --confirm` about this directory:
            // it is the folder, emptied or moved on purpose. The refusal names
            // that command, so the command must get past it.
            Marker::Missing if deletions == Deletions::ApplyWhateverTheCount => {
                write_marker(&self.root, device);
                return Ok(());
            }
            Marker::Foreign(named) => {
                return Err(FolderError::StorageUnreachable {
                    root: self.root.clone(),
                    why: format!(
                        "its marker names device {}, not this one; two nodes syncing one directory delete each other's files",
                        &named[..named.len().min(12)]
                    ),
                });
            }
            Marker::Missing => {}
        }

        let ledger = store.local_states()?;
        let known: Vec<&String> = ledger.iter().map(|(path, _)| path).collect();

        if known.is_empty() {
            write_marker(&self.root, device);
            return Ok(());
        }

        // One is enough. This is not a health check on the folder -- files go
        // missing for ordinary reasons -- it is the difference between "the
        // disk is here" and "the disk is not here", and one file answers that.
        if known.iter().any(|path| on_disk.contains_key(*path)) {
            write_marker(&self.root, device);
            return Ok(());
        }

        Err(FolderError::StorageUnreachable {
            root: self.root.clone(),
            why: format!(
                "not one of the {} files this machine holds is in it, and it carries no marker",
                known.len()
            ),
        })
    }

    /// The paths this pass would delete from the account, and whether that is
    /// too many to do without somebody looking.
    ///
    /// The marker catches a storage that vanished whole. This catches the other
    /// shape: the directory is genuinely there, the marker with it, and most of
    /// what was in it is not -- a sync client that half-ran, a restore that
    /// wrote into the wrong place, a `rm -rf` in the wrong terminal. Deletions
    /// replicate, so the cost of guessing wrong is the account's copies as well
    /// as this machine's.
    pub(crate) fn deletions_are_too_many(
        store: &Store,
        on_disk: &BTreeMap<String, DiskFile>,
    ) -> Result<Option<usize>> {
        // The account's own files are written back when they go missing, never
        // deleted, so they count for neither side.
        let ledger: Vec<_> = store
            .local_states()?
            .into_iter()
            .filter(|(path, _)| !deletion::is_reserved(path))
            .collect();
        let held = ledger.len();
        let vanished = ledger
            .iter()
            .filter(|(path, _)| !on_disk.contains_key(path))
            .count();

        if vanished > DELETIONS_ALWAYS_ALLOWED && vanished * 2 > held {
            return Ok(Some(vanished));
        }
        Ok(None)
    }
}
