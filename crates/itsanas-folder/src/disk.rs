//! Reading the disk and writing it: hashing, importing, exporting atomically,
//! deleting, and keeping both sides of a conflict.

use std::{
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use itsanas_store::{LocalState, Store};

use crate::steady::Steady;
use crate::{Folder, FolderError, Result, STAGING_DIR, scan};

impl Folder {
    /// The content hash of a file on disk, or `None` if it is not there.
    ///
    /// Uses the ledger's size and modification time as a pre-filter unless
    /// `deep` is set, because re-hashing a large folder on every pass would
    /// make watching it unusable.
    pub(crate) fn hash_on_disk(
        real: &Path,
        ledger: Option<&LocalState>,
        deep: bool,
    ) -> Result<Option<[u8; 32]>> {
        let metadata = match std::fs::symlink_metadata(real) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(FolderError::io(real.to_owned(), error)),
        };

        // A symlink is not a file as far as this folder is concerned, and
        // following one could reach anywhere on the disk.
        if metadata.is_symlink() || metadata.is_dir() {
            return Ok(None);
        }

        if !deep && let Some(ledger) = ledger {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_secs());

            if ledger.probably_matches(metadata.len(), modified) {
                return Ok(Some(ledger.content_hash));
            }
        }

        // Streamed, not slurped: a deep scan over a folder of videos would
        // otherwise read each one entirely into memory just to hash it.
        let file =
            std::fs::File::open(real).map_err(|error| FolderError::io(real.to_owned(), error))?;

        let mut hasher = blake3::Hasher::new();
        hasher
            .update_reader(file)
            .map_err(|error| FolderError::io(real.to_owned(), error))?;

        Ok(Some(*hasher.finalize().as_bytes()))
    }

    /// Take `real` into the store as `path`. False, with nothing taken, when
    /// the file changed while it was read: something is still writing it.
    pub(crate) fn import(store: &Store, path: &str, real: &Path) -> Result<bool> {
        let file =
            std::fs::File::open(real).map_err(|error| FolderError::io(real.to_owned(), error))?;

        // Asked before a byte is read. `write_stream` refuses too, but only as
        // the bytes go past, and a folder pass retries a refused file every
        // round: without this, each round re-reads and re-seals the part of it
        // that fits before throwing that work away.
        let opened = file
            .metadata()
            .map_err(|error| FolderError::io(real.to_owned(), error))?;
        store.check_room(path, opened.len())?;

        let steady = Steady::new(std::io::BufReader::new(file), real, &opened);
        let moved = steady.moved();
        match store.write_stream(path, steady) {
            Ok(entry) => {
                Self::record(store, path, real, entry.content_hash)?;
                Ok(true)
            }
            // A file still being written (a database dumped, a large copy)
            // read half-way was a version of it that never existed, sent to
            // every machine until the next pass replaced it, and the only one
            // left if this machine went first. Nothing of it is recorded: the
            // error comes before `write_stream` writes its entry.
            Err(_) if moved.get() => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    /// Write a file out of the store, streamed and atomically.
    ///
    /// Two properties, both load-bearing:
    ///
    /// * **Streamed.** One chunk at a time, so exporting a 6 GB video costs a
    ///   quarter of a megabyte rather than 6 GB.
    /// * **Atomic.** `read_stream` can only verify the whole-file hash once the
    ///   last byte has gone past, so bytes reach the staging file *before* they
    ///   are known to be correct. If verification fails the staging file is
    ///   discarded and the destination is never touched. Writing straight to
    ///   the destination would leave corrupt content in place, and the next
    ///   scan would hash it, import it as a genuine edit, and replicate the
    ///   corruption to every other machine.
    pub(crate) fn export(&self, store: &Store, path: &str, real: &Path) -> Result<()> {
        if let Some(parent) = real.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| FolderError::io(parent.to_owned(), error))?;
        }

        let temporary = self.staging_path()?;
        let file = std::fs::File::create(&temporary)
            .map_err(|error| FolderError::io(temporary.clone(), error))?;

        let produced = {
            let mut writer = std::io::BufWriter::new(&file);
            store.read_stream(path, &mut writer)
        };

        let produced = match produced {
            Ok(produced) => produced,
            Err(error) => {
                // Corrupt or incomplete. The destination keeps whatever it had.
                drop(file);
                let _ = std::fs::remove_file(&temporary);
                return Err(error.into());
            }
        };

        if !produced {
            // Decided to export something the store cannot produce. The usual
            // cause is a chunk that has not arrived yet, and the right response
            // is to leave the ledger alone and try again next pass.
            drop(file);
            let _ = std::fs::remove_file(&temporary);
            return Ok(());
        }

        // Durable before it is visible: a crash after the rename must not
        // expose a file whose bytes never reached the disk.
        file.sync_all()
            .map_err(|error| FolderError::io(temporary.clone(), error))?;
        drop(file);

        std::fs::rename(&temporary, real).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            FolderError::io(real.to_owned(), error)
        })?;

        let entry = store
            .stat(path)?
            .ok_or_else(|| FolderError::io(real.to_owned(), std::io::Error::other("vanished")))?;

        Self::record(store, path, real, entry.content_hash)
    }

    /// A unique name in the staging directory.
    ///
    /// Process id and a counter rather than the content hash, because with
    /// streaming the hash is not known until the bytes have already been
    /// written. Two processes against one folder would be a mistake anyway —
    /// the index lock prevents it — but the process id costs nothing and makes
    /// a leftover traceable.
    fn staging_path(&self) -> Result<PathBuf> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let staging = self.root.join(STAGING_DIR);
        std::fs::create_dir_all(&staging)
            .map_err(|error| FolderError::io(staging.clone(), error))?;

        Ok(staging.join(format!(
            "{}-{}.part",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        )))
    }

    /// Copy a file within the folder without materialising it.
    fn copy_aside(&self, from: &Path, to: &Path) -> Result<()> {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| FolderError::io(parent.to_owned(), error))?;
        }

        let temporary = self.staging_path()?;
        std::fs::copy(from, &temporary).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            FolderError::io(from.to_owned(), error)
        })?;

        std::fs::rename(&temporary, to).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            FolderError::io(to.to_owned(), error)
        })?;

        Ok(())
    }

    pub(crate) fn delete_from_disk(&self, real: &Path) -> Result<()> {
        match std::fs::remove_file(real) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(FolderError::io(real.to_owned(), error)),
        }

        self.prune_empty_parents(real);
        Ok(())
    }

    /// Remove directories left empty by a deletion, up to but never including
    /// the folder root.
    ///
    /// Without this, deleting the last file from a deep tree leaves the tree
    /// behind on every machine forever, and the folder slowly fills with empty
    /// directories nobody put there.
    fn prune_empty_parents(&self, real: &Path) {
        let mut current = real.parent().map(Path::to_path_buf);

        while let Some(directory) = current {
            if directory == self.root || !directory.starts_with(&self.root) {
                return;
            }
            // Fails harmlessly if the directory is not empty, which is the
            // common case and the reason there is no emptiness check first.
            if std::fs::remove_dir(&directory).is_err() {
                return;
            }
            current = directory.parent().map(Path::to_path_buf);
        }
    }

    /// Move the local version aside and write the store's version out.
    ///
    /// Both survive. The local one is renamed rather than overwritten, because
    /// the alternative is destroying work somebody did on this machine — the
    /// exact thing this project refuses to do anywhere else.
    pub(crate) fn keep_both(
        &self,
        store: &Store,
        path: &str,
        real: &Path,
        disk_hash: Option<[u8; 32]>,
    ) -> Result<String> {
        let hash = disk_hash.ok_or_else(|| {
            FolderError::io(
                real.to_owned(),
                std::io::Error::other("a conflict was reported for a file that is not on disk"),
            )
        })?;

        // Named by content, so the same conflict resolved twice produces the
        // same name instead of a second copy.
        let marker = format!("local-{}", blake3::Hash::from(hash).to_hex().split_at(12).0);
        let sibling = itsanas_sync_naming::with_marker(path, &marker);
        let sibling_real = scan::to_filesystem(&self.root, &sibling)?;

        // Copy on disk first, then import from the copy. Both steps stream, so
        // a conflict on a large file does not need the file in memory — and
        // the original stays untouched until the copy has landed.
        self.copy_aside(real, &sibling_real)?;
        if !Self::import(store, &sibling, &sibling_real)? {
            return Err(FolderError::io(
                sibling_real,
                std::io::Error::other("the copy kept aside changed while it was read"),
            ));
        }

        // Now the incoming version takes the original path.
        self.export(store, path, real)?;

        Ok(sibling)
    }

    /// Record what is now on disk, so the next pass does not re-examine it.
    pub(crate) fn record(store: &Store, path: &str, real: &Path, hash: [u8; 32]) -> Result<()> {
        let metadata =
            std::fs::metadata(real).map_err(|error| FolderError::io(real.to_owned(), error))?;

        let modified_unix = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_secs());

        store.set_local_state(
            path,
            &LocalState {
                size: metadata.len(),
                modified_unix,
                content_hash: hash,
            },
        )?;

        Ok(())
    }
}

/// Sibling naming, borrowed from the sync engine so the rules cannot drift.
mod itsanas_sync_naming {
    pub use itsanas_sync::conflict::with_marker;
}
