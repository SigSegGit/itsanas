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
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use itsanas_crypto::DeviceId;
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
    /// Deletions this pass found and did **not** write, because there were
    /// enough of them to look like an accident rather than a decision.
    ///
    /// Nothing was lost: the files are still in the account, and the next pass
    /// will find them missing again. `itsanas folder --confirm` is what says
    /// they really are meant to go.
    pub held_deletions: usize,
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

    /// Whether this pass refused to write deletions it found.
    #[must_use]
    pub const fn held_anything(&self) -> bool {
        self.held_deletions > 0
    }
}

/// Fewer deletions than this are never held, whatever the proportion.
///
/// Deleting two files out of three is a Tuesday. The guard is for the shape of
/// an accident -- a folder that emptied itself -- and a threshold without a
/// floor would hold a perfectly ordinary tidy-up on a small folder and teach
/// its owner to pass `--confirm` out of habit, which is how a guard becomes a
/// formality.
pub const DELETIONS_ALWAYS_ALLOWED: usize = 5;

/// Whether a pass may write the deletions it found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Deletions {
    /// Ordinary: what is gone from disk is gone from the account.
    Apply,
    /// Somebody said so out loud, with `itsanas folder --confirm`.
    ApplyWhateverTheCount,
}

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
        self.reconcile_with(store, deep, Deletions::Apply)
    }

    /// The same, applying deletions however many there are.
    ///
    /// What `itsanas folder --confirm` runs. A person looked at the folder and
    /// said the files really are gone; the guard is for the case where nobody
    /// looked.
    ///
    /// # Errors
    ///
    /// As [`Self::reconcile`].
    pub fn reconcile_confirmed(&self, store: &Store, deep: bool) -> Result<ReconcileReport> {
        self.reconcile_with(store, deep, Deletions::ApplyWhateverTheCount)
    }

    fn reconcile_with(
        &self,
        store: &Store,
        deep: bool,
        deletions: Deletions,
    ) -> Result<ReconcileReport> {
        let mut report = ReconcileReport::default();

        let on_disk = scan::scan(&self.root)?;
        // Before anything is written: is this directory the folder, or is it a
        // mount point with nothing mounted on it?
        self.check_storage(store, &on_disk)?;
        let in_store = store.entries()?;
        let ledger = store.local_states()?;

        // Every path any of the three views knows about. Missing one would mean
        // never noticing a file that exists in only one place — which is
        // exactly the interesting case.
        let mut paths: BTreeSet<String> = BTreeSet::new();
        paths.extend(on_disk.keys().cloned());
        paths.extend(in_store.iter().map(|(path, _)| path.clone()));
        paths.extend(ledger.iter().map(|(path, _)| path.clone()));

        // Decided once for the whole pass, before anything is written: a
        // guard applied file by file would let the first half of an accident
        // through before noticing the shape of it.
        let hold = match deletions {
            Deletions::ApplyWhateverTheCount => None,
            Deletions::Apply => Self::deletions_are_too_many(store, &on_disk)?,
        };
        if let Some(count) = hold {
            report.held_deletions = count;
        }

        for path in paths {
            if let Err(error) = self.reconcile_one(store, &path, deep, hold.is_some(), &mut report)
            {
                // One unreadable file must not stop the rest of the folder.
                report.failed.push((path, error.to_string()));
            }
        }

        // One segment per pass, rather than one per file: a folder of ten
        // thousand files should announce itself once, not ten thousand times.
        store.flush_segment()?;

        Ok(report)
    }

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
    fn check_storage(&self, store: &Store, on_disk: &BTreeMap<String, DiskFile>) -> Result<()> {
        let device = store.device_id();
        match read_marker(&self.root, device) {
            Marker::Ours => return Ok(()),
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
    fn deletions_are_too_many(
        store: &Store,
        on_disk: &BTreeMap<String, DiskFile>,
    ) -> Result<Option<usize>> {
        let ledger = store.local_states()?;
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

    fn reconcile_one(
        &self,
        store: &Store,
        path: &str,
        deep: bool,
        hold_deletions: bool,
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
                // Held, not dropped: the ledger keeps saying this machine has
                // the file, so the next pass sees the same disappearance and
                // asks the same question. Writing the deletion is the one step
                // that cannot be taken back, because it replicates.
                if hold_deletions {
                    return Ok(());
                }
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
