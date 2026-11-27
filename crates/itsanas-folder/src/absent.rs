//! Files absent on purpose: freed on this machine, or waiting in a trash.

use std::path::Path;

use itsanas_store::{LocalState, Store, deletion, freed, trash};

use crate::{Folder, FolderError, Result, scan};

impl Folder {
    /// Free `path` on this machine: its plain copy leaves this folder, the account
    /// keeps it, and later passes leave it absent instead of reading a delete.
    /// Only a file this folder holds as the account does: freeing an edit not yet
    /// imported would lose it.
    ///
    /// The record is written before the ledger is cleared and the file removed,
    /// so a crash between the steps leaves a file that syncs, never a delete.
    ///
    /// # Errors
    ///
    /// [`FolderError::NotSynced`] when the file is absent or differs from the
    /// account's version; an I/O or store error when a step cannot be written.
    pub fn free(&self, store: &Store, path: &str) -> Result<()> {
        let real = scan::to_filesystem(&self.root, path)?;
        let ledger = store.local_state(path)?.map(|state| state.content_hash);
        let disk = Self::hash_on_disk(&real, None, true)?;
        let held = store.stat(path)?.map(|entry| entry.content_hash);
        if disk.is_none() || disk != held || disk != ledger {
            return Err(FolderError::NotSynced(path.to_owned()));
        }
        freed::add(store.root(), path).map_err(|error| FolderError::io(real.clone(), error))?;
        store.clear_local_state(path)?;
        std::fs::remove_file(&real).map_err(|error| FolderError::io(real, error))
    }

    /// Keep `path` on this machine again: the next pass writes the account's
    /// version into the folder. False when it was not freed here.
    ///
    /// # Errors
    ///
    /// When the record cannot be written.
    pub fn keep_here(store: &Store, path: &str) -> Result<bool> {
        if !freed::list(store.root()).contains(path) {
            return Ok(false);
        }
        freed::take_out(store.root(), path).map_err(|error| FolderError::io(path, error))?;
        Ok(true)
    }

    /// A trashed path found on disk again: out of the trash, and the ledger set to
    /// the version that was deleted, so what is on disk now reads as an edit of
    /// it rather than a conflict with it. The time is left at zero so the file
    /// is hashed, never trusted on its size.
    pub(crate) fn put_back(store: &Store, path: &str) -> Result<()> {
        trash::take_out(store.root(), path).map_err(|error| FolderError::io(path, error))?;
        if let Some(entry) = store.stat(path)? {
            store.set_local_state(
                path,
                &LocalState {
                    size: entry.size,
                    modified_unix: 0,
                    content_hash: entry.content_hash,
                },
            )?;
        }
        Ok(())
    }

    /// A path in the account's trash: true when this pass has nothing more to
    /// do with it. Gone from disk, it stays gone. On disk as the ledger last
    /// saw it, the trash was emptied of it elsewhere: the plain copy goes, and
    /// the ledger with it, so its absence is never read as a delete. On disk
    /// changed, a person put it back by hand: out of the trash, and it syncs.
    pub(crate) fn trashed_in_account(&self, store: &Store, path: &str, deep: bool) -> Result<bool> {
        let real = scan::to_filesystem(&self.root, path)?;
        let ledger = store.local_state(path)?;
        let Some(disk) = Self::hash_on_disk(&real, ledger.as_ref(), deep)? else {
            return Ok(true);
        };
        if ledger.is_some_and(|state| state.content_hash == disk) {
            self.delete_from_disk(&real)?;
            store.clear_local_state(path)?;
            return Ok(true);
        }
        deletion::restore_shared_known(store, path)?;
        Self::put_back(store, path)?;
        Ok(false)
    }

    /// A file the person deleted from disk, by this machine's rules
    /// ([`deletion::rules_known_here`]): into the account's trash, waiting here
    /// for a confirmation, gone from this machine only, or out of the store at
    /// once.
    pub(crate) fn trash_or_remove(store: &Store, path: &str, real: &Path) -> Result<()> {
        let rules = deletion::rules_known_here(store)?;
        let now = itsanas_store::now_unix();
        let off = trash::days(store.root()) == 0;
        match rules.mode {
            deletion::Mode::Direct if rules.ask_auth && !off => {
                trash::put(store.root(), path, now, trash::MAX_DAYS)
                    .map_err(|error| FolderError::io(real.to_owned(), error))
            }
            _ if off || rules.mode == deletion::Mode::Direct => {
                store.remove_file(path)?;
                Ok(())
            }
            deletion::Mode::Local => freed::add(store.root(), path)
                .map_err(|error| FolderError::io(real.to_owned(), error)),
            deletion::Mode::Trash | deletion::Mode::Direct => {
                deletion::trash_shared(store, path, now, rules.days)?;
                Ok(())
            }
        }
    }
}
