//! A real directory, mirrored into a store and back.
//!
//! This is the layer that turns ITSaNAS from something you drive with commands
//! into a folder that syncs. Files put in it are imported; files deleted from
//! it are deleted everywhere; changes arriving from a peer are written out.
//!
//! # The dangerous case, and what stops it
//!
//! A file missing from disk means one of two opposite things: the user deleted
//! it, or this device never downloaded it. Acting on the wrong one is
//! catastrophic in a way that is worth stating plainly — a brand-new device
//! that treated "absent" as "deleted" would, on its first sync, announce the
//! deletion of every file the user owns, and every other device would obey.
//!
//! What separates them is [`LocalState`]: the record
//! of what this device last put on disk. A delete is only ever acted on for a
//! path the ledger says this device genuinely had. That guard is tested
//! exhaustively over every combination of the three views in [`decision`].
//!
//! # What is not attempted
//!
//! Empty directories are not synced — only files are. Permissions, ownership
//! and extended attributes are not preserved: they mean different things on a
//! Windows laptop and a Raspberry Pi, and syncing them would create conflicts
//! that cannot be resolved. Symlinks are skipped rather than followed, which is
//! a security property; see [`scan`].

pub mod decision;
pub mod error;
pub mod scan;
pub mod watch;

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use itsanas_store::{LocalState, Store};

pub use decision::{Decision, decide};
pub use error::{FolderError, Result};
pub use scan::{DiskFile, STAGING_DIR};
pub use watch::{Change, Watcher};

/// What one reconciliation pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Files taken from disk into the store.
    pub imported: Vec<String>,
    /// Files written out of the store onto disk.
    pub exported: Vec<String>,
    /// Files the user deleted, now deleted in the store.
    pub removed_from_store: Vec<String>,
    /// Files a peer deleted, now removed from disk.
    pub deleted_from_disk: Vec<String>,
    /// Paths where both sides had changed differently. The local version was
    /// moved aside; the value is where it went.
    pub kept_both: Vec<(String, String)>,
    /// Paths whose bookkeeping was corrected without moving any data.
    pub recorded: usize,
    /// Paths that could not be handled, with why. One bad file must not stop
    /// the rest of the folder from syncing.
    pub failed: Vec<(String, String)>,
}

impl ReconcileReport {
    /// Whether anything actually moved.
    #[must_use]
    pub fn changed_anything(&self) -> bool {
        !self.imported.is_empty()
            || !self.exported.is_empty()
            || !self.removed_from_store.is_empty()
            || !self.deleted_from_disk.is_empty()
            || !self.kept_both.is_empty()
    }

    /// A one-line summary for a log.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} in, {} out, {} deleted locally, {} deleted remotely, {} conflicts",
            self.imported.len(),
            self.exported.len(),
            self.removed_from_store.len(),
            self.deleted_from_disk.len(),
            self.kept_both.len()
        )
    }
}

/// A directory kept in step with a store.
#[derive(Debug)]
pub struct Folder {
    root: PathBuf,
}

impl Folder {
    /// Open (creating if needed) a folder at `root`.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_owned();
        std::fs::create_dir_all(&root).map_err(|error| FolderError::io(root.clone(), error))?;
        std::fs::create_dir_all(root.join(STAGING_DIR))
            .map_err(|error| FolderError::io(root.clone(), error))?;
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Bring disk and store into agreement.
    ///
    /// `deep` re-hashes every file rather than trusting size and modification
    /// time. The fast path misses a file rewritten within the same second at
    /// exactly the same size; a periodic deep pass closes that gap.
    ///
    /// Anything imported or deleted is **sealed into a log segment before this
    /// returns**. `Store::write_file` only queues a pending entry; until it is
    /// flushed, peers asking what changed are told nothing, and the file sits
    /// on this machine looking perfectly synced while existing nowhere else.
    /// Flushing here rather than leaving it to the caller means no path that
    /// mutates the store through a folder can forget to announce it — which is
    /// exactly the bug this call was added to fix.
    pub fn reconcile(&self, store: &Store, deep: bool) -> Result<ReconcileReport> {
        let mut report = ReconcileReport::default();

        let on_disk = scan::scan(&self.root)?;
        let in_store = store.entries()?;
        let ledger = store.local_states()?;

        // Every path any of the three views knows about. Missing one would mean
        // never noticing a file that exists in only one place — which is
        // exactly the interesting case.
        let mut paths: BTreeSet<String> = BTreeSet::new();
        paths.extend(on_disk.keys().cloned());
        paths.extend(in_store.iter().map(|(path, _)| path.clone()));
        paths.extend(ledger.iter().map(|(path, _)| path.clone()));

        for path in paths {
            if let Err(error) = self.reconcile_one(store, &path, deep, &mut report) {
                // One unreadable file must not stop the rest of the folder.
                report.failed.push((path, error.to_string()));
            }
        }

        // One segment per pass, rather than one per file: a folder of ten
        // thousand files should announce itself once, not ten thousand times.
        store.flush_segment()?;

        Ok(report)
    }

    fn reconcile_one(
        &self,
        store: &Store,
        path: &str,
        deep: bool,
        report: &mut ReconcileReport,
    ) -> Result<()> {
        let ledger = store.local_state(path)?;
        let entry = store.stat(path).ok().flatten();
        let real = scan::to_filesystem(&self.root, path)?;

        let disk_hash = Self::hash_on_disk(&real, ledger.as_ref(), deep)?;
        let store_hash = entry.as_ref().map(|entry| entry.content_hash);
        let ledger_hash = ledger.as_ref().map(|state| state.content_hash);

        match decide(disk_hash, store_hash, ledger_hash) {
            Decision::Nothing => {}

            Decision::RecordOnly => {
                match disk_hash {
                    Some(hash) => Self::record(store, path, &real, hash)?,
                    None => store.clear_local_state(path)?,
                }
                report.recorded += 1;
            }

            Decision::Import => {
                Self::import(store, path, &real)?;
                report.imported.push(path.to_owned());
            }

            Decision::RemoveFromStore => {
                store.remove_file(path)?;
                store.clear_local_state(path)?;
                report.removed_from_store.push(path.to_owned());
            }

            Decision::Export => {
                self.export(store, path, &real)?;
                report.exported.push(path.to_owned());
            }

            Decision::DeleteFromDisk => {
                self.delete_from_disk(&real)?;
                store.clear_local_state(path)?;
                report.deleted_from_disk.push(path.to_owned());
            }

            Decision::KeepBoth => {
                let sibling = self.keep_both(store, path, &real, disk_hash)?;
                report.kept_both.push((path.to_owned(), sibling));
            }
        }

        Ok(())
    }

    /// The content hash of a file on disk, or `None` if it is not there.
    ///
    /// Uses the ledger's size and modification time as a pre-filter unless
    /// `deep` is set, because re-hashing a large folder on every pass would
    /// make watching it unusable.
    fn hash_on_disk(
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

    fn import(store: &Store, path: &str, real: &Path) -> Result<()> {
        let file =
            std::fs::File::open(real).map_err(|error| FolderError::io(real.to_owned(), error))?;

        let entry = store.write_stream(path, std::io::BufReader::new(file))?;
        Self::record(store, path, real, entry.content_hash)
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
    fn export(&self, store: &Store, path: &str, real: &Path) -> Result<()> {
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

    fn delete_from_disk(&self, real: &Path) -> Result<()> {
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
    fn keep_both(
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
        Self::import(store, &sibling, &sibling_real)?;

        // Now the incoming version takes the original path.
        self.export(store, path, real)?;

        Ok(sibling)
    }

    /// Record what is now on disk, so the next pass does not re-examine it.
    fn record(store: &Store, path: &str, real: &Path, hash: [u8; 32]) -> Result<()> {
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
