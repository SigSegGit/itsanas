//! A real directory, mirrored into a store and back.
//!
//! This is the layer that turns ITSaNAS from something you drive with commands
//! into a folder that syncs. Files put in it are imported; files deleted from
//! it go to this machine's trash (`itsanas_store::trash`), and are deleted
//! everywhere when its delay runs out; changes arriving from a peer are
//! written out. A file restored from the trash is written back here.
//!
//! # The dangerous case, and what stops it
//!
//! A file missing from disk means one of two opposite things: the user deleted
//! it, or this device never downloaded it. Acting on the wrong one is
//! catastrophic in a way that is worth stating plainly — a brand-new device
//! that treated "absent" as "deleted" would, on its first sync, announce the
//! deletion of every file the user owns, and every other device would obey.
//!
//! What separates them is [`itsanas_store::LocalState`]: the record
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

mod absent;
pub mod decision;
mod disk;
pub mod error;
mod guard;
mod renames;
mod report;
pub mod scan;
mod steady;
pub mod watch;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use itsanas_store::{LocalState, Store, deletion, freed, trash};

pub use decision::{Decision, decide};
pub use error::{FolderError, Result};
pub use guard::DELETIONS_ALWAYS_ALLOWED;
pub use report::{LINES_IN_A_LOG, ReconcileReport};
pub use scan::{DiskFile, STAGING_DIR};
pub use watch::{Change, Watcher};

use renames::{Contents, pair_renames};

/// Whether a pass may write the deletions it found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Deletions {
    /// Ordinary: what is gone from disk is gone from the account.
    Apply,
    /// Somebody said so out loud, with `itsanas folder --confirm`.
    ApplyWhateverTheCount,
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
        self.check_storage(store, &on_disk, deletions)?;
        let in_store = store.entries()?;
        let ledger = store.local_states()?;

        // Every path any of the three views knows about. Missing one would mean
        // never noticing a file that exists in only one place — which is
        // exactly the interesting case.
        let mut paths: BTreeSet<String> = BTreeSet::new();
        paths.extend(on_disk.keys().cloned());
        paths.extend(in_store.iter().map(|(path, _)| path.clone()));
        paths.extend(ledger.iter().map(|(path, _)| path.clone()));
        if cfg!(any(windows, target_os = "macos")) {
            skip_case_twins(&mut paths, &on_disk, &ledger, &mut report);
        }

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

        let trashed = trash::list(store.root());
        let freed = freed::list(store.root());
        let shared = deletion::shared_known_here(store)?;
        let mut contents = Contents::default();
        for path in paths {
            // In the account's trash, from this machine or another: no plain
            // copy anywhere until it is restored, and no delete read from its
            // absence. A copy edited by hand meanwhile takes it out instead.
            if shared.contains_key(&path) {
                match self.trashed_in_account(store, &path, deep) {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(error) => {
                        report.failed.push((path, error.to_string()));
                        continue;
                    }
                }
            }
            // In this machine's trash: still the account's, hidden here, left
            // alone until its delay runs out or a person restores it, which
            // takes it out and lets the next pass write it back. Put back on
            // disk by hand, it leaves the trash and syncs like any file.
            if trashed.contains_key(&path) {
                if !on_disk.contains_key(&path) {
                    continue;
                }
                if let Err(error) = Self::put_back(store, &path) {
                    report.failed.push((path, error.to_string()));
                    continue;
                }
            }
            // Freed on this machine: absent on purpose, its bytes on the account's
            // other machines, and no ledger entry, so it never reads as a delete.
            // Back on disk, by hand or by `keep_here`, it is a file here again.
            if freed.contains(&path) {
                if !on_disk.contains_key(&path) {
                    continue;
                }
                if let Err(error) = freed::take_out(store.root(), &path) {
                    report.failed.push((path, error.to_string()));
                    continue;
                }
            }
            if let Err(error) = self.reconcile_one(
                store,
                &path,
                deep,
                hold.is_some(),
                &mut report,
                &mut contents,
            ) {
                // One unreadable file must not stop the rest of the folder.
                report.failed.push((path, error.to_string()));
            }
        }
        if contents.could_pair() {
            // How many live paths carry each content after the pass. Read
            // only when there is a pair to judge, so an ordinary pass pays
            // nothing for it.
            // A trashed path is not a live copy: counting it would make every
            // move look like a copy, and leave the old name in the trash.
            let trashed = trash::list(store.root());
            let mut live: BTreeMap<[u8; 32], usize> = BTreeMap::new();
            let shared = deletion::shared_known_here(store)?;
            for (path, entry) in store.entries()? {
                if !trashed.contains_key(&path)
                    && !shared.contains_key(&path)
                    && !deletion::is_reserved(&path)
                {
                    *live.entry(entry.content_hash).or_default() += 1;
                }
            }
            report.renamed_here = pair_renames(&contents.gone_here, &contents.came_here, &live);
            report.renamed_elsewhere =
                pair_renames(&contents.gone_elsewhere, &contents.came_elsewhere, &live);
            // A move is no delete to wait on: the bytes are still here under the
            // new name, and the old one would fill the trash with every rename.
            for (from, _) in &report.renamed_here {
                store.remove_file(from)?;
                trash::take_out(store.root(), from)
                    .map_err(|error| FolderError::io(self.root.clone(), error))?;
                deletion::restore_shared_known(store, from)?;
                freed::take_out(store.root(), from)
                    .map_err(|error| FolderError::io(self.root.clone(), error))?;
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
        hold_deletions: bool,
        report: &mut ReconcileReport,
        contents: &mut Contents,
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
                if !Self::import(store, path, &real)? {
                    report.still_writing.push(path.to_owned());
                    return Ok(());
                }
                report.imported.push(path.to_owned());
                if let Some(hash) = disk_hash {
                    contents.came_here.push((path.to_owned(), hash));
                }
            }

            Decision::RemoveFromStore => {
                // Held, not dropped: the ledger keeps saying this machine has
                // the file, so the next pass sees the same disappearance and
                // asks the same question. Writing the deletion is the one step
                // that cannot be taken back, because it replicates.
                if hold_deletions {
                    return Ok(());
                }
                // The account's own files (its rules, its trash) are nobody's
                // to delete from a folder: gone from disk, as after a move to a
                // new folder, the next pass writes them back.
                if deletion::is_reserved(path) {
                    store.clear_local_state(path)?;
                    return Ok(());
                }
                Self::trash_or_remove(store, path, &real)?;
                store.clear_local_state(path)?;
                report.removed_from_store.push(path.to_owned());
                if let Some(hash) = ledger_hash {
                    contents.gone_here.push((path.to_owned(), hash));
                }
            }

            Decision::Export => {
                self.export(store, path, &real)?;
                report.exported.push(path.to_owned());
                if let Some(hash) = store_hash {
                    contents.came_elsewhere.push((path.to_owned(), hash));
                }
            }

            Decision::DeleteFromDisk => {
                self.delete_from_disk(&real)?;
                store.clear_local_state(path)?;
                report.deleted_from_disk.push(path.to_owned());
                if let Some(hash) = ledger_hash {
                    contents.gone_elsewhere.push((path.to_owned(), hash));
                }
            }

            Decision::KeepBoth => {
                let sibling = self.keep_both(store, path, &real, disk_hash)?;
                report.kept_both.push((path.to_owned(), sibling));
            }
        }

        Ok(())
    }
}

/// On a disk that folds case, `IMG.JPG` and `img.jpg` from a machine that
/// keeps them apart are one file here. Reconciled both, the second spelling
/// reads as the first one edited, and this machine rewrites it for the whole
/// account. So one spelling is kept here (the one on the disk, else one this
/// folder already synced, else the first) and the others are left in the
/// account untouched and reported: no export, no copy, no delete read from
/// their absence.
fn skip_case_twins(
    paths: &mut BTreeSet<String>,
    on_disk: &BTreeMap<String, DiskFile>,
    ledger: &[(String, LocalState)],
    report: &mut ReconcileReport,
) {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in paths.iter() {
        groups
            .entry(path.to_lowercase())
            .or_default()
            .push(path.clone());
    }
    let synced = |path: &str| ledger.iter().any(|(known, _)| known == path);
    for group in groups.into_values().filter(|group| group.len() > 1) {
        let kept = group
            .iter()
            .position(|path| on_disk.contains_key(path))
            .or_else(|| group.iter().position(|path| synced(path)))
            .unwrap_or(0);
        for (index, twin) in group.into_iter().enumerate() {
            if index != kept {
                paths.remove(&twin);
                let why = "differs only in case from another file, and this disk cannot hold both; it stays in the account";
                report.failed.push((twin, why.to_owned()));
            }
        }
    }
}
