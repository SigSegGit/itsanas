//! The write budget: how much the account may hold, set by the node, and
//! the room a write or a pull is checked against before anything is stored.

use super::{FolderCopy, Store, WriteBudget};
use crate::error::{Result, StoreError};

impl Store {
    // ----------------------------------------------------------------- write

    /// Bound every later write by `budget`, or lift the bound with `None`.
    ///
    /// # Errors
    ///
    /// If a panic poisoned the lock.
    pub fn set_write_budget(&self, budget: Option<WriteBudget>) -> Result<()> {
        *self.lock_budget()? = budget;
        Ok(())
    }

    /// The bound writes are held to, if any.
    ///
    /// # Errors
    ///
    /// If a panic poisoned the lock.
    pub fn write_budget(&self) -> Result<Option<WriteBudget>> {
        Ok(*self.lock_budget()?)
    }

    fn lock_budget(&self) -> Result<std::sync::MutexGuard<'_, Option<WriteBudget>>> {
        self.budget.lock().map_err(|_| {
            StoreError::Corrupt("the store's write budget was poisoned by a panic".to_owned())
        })
    }

    /// Refuse, before a byte is read, a file of `incoming` bytes that would not
    /// fit.
    ///
    /// [`Self::write_stream`] enforces the same bound whether or not this was
    /// asked, but it can only find out as the bytes go past: a refusal there
    /// comes after reading up to the limit. A caller that knows the size --
    /// a file on disk has one -- asks here first, so that a 3 GB video offered
    /// to an account with 100 MB left costs a `stat` and not a 100 MB copy.
    ///
    /// # Errors
    ///
    /// [`StoreError::OverBudget`] with the numbers, or if the index cannot be
    /// read.
    pub fn check_room(&self, path: &str, incoming: u64) -> Result<()> {
        let Some(budget) = self.write_budget()? else {
            return Ok(());
        };
        let account = self.account_besides(path, budget)?;
        Self::refuse_past(path, budget, account, account.saturating_add(incoming))
    }

    /// Whether another device's file of `incoming` bytes fits on this disk,
    /// before any of its chunks is fetched: `None` if it fits, or the room
    /// left if it does not.
    ///
    /// # Why a pull has its own check
    ///
    /// A pull adds nothing to the *account* -- the file is already in it,
    /// counted in [`WriteBudget::elsewhere`] -- so the account bound does not
    /// apply. The disk does: what this machine's pledge still owes is space
    /// promised to other people, and downloading the rest of the account into
    /// it breaks that promise exactly as writing new files would. Asked per
    /// file and before the first chunk, because a refusal part-way through
    /// would leave the chunks already fetched on disk with no index entry to
    /// count them: the disk fills while the ceiling reads as untouched.
    ///
    /// `replacing` says whether the file lands on `path` in place of what is
    /// there (an update, charged its growth) or beside it (a conflict keeps
    /// both, so nothing is given back). Measured in plaintext bytes, and
    /// chunks already here are charged again: both err towards refusing.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn pull_room(&self, path: &str, incoming: u64, replacing: bool) -> Result<Option<u64>> {
        let Some(budget) = self.write_budget()? else {
            return Ok(None);
        };
        let replaced = if replacing {
            self.index.get_file(path)?.map_or(0, |entry| entry.size)
        } else {
            0
        };
        let local = self.index.local_bytes()?.saturating_sub(replaced);
        match budget.folder_copy {
            FolderCopy::SameVolume { local_then } => {
                // Every byte pulled since the budget was set, and this file,
                // twice: the store's copy and the folder's.
                let Some(ceiling) = budget.local_ceiling else {
                    return Ok(None);
                };
                let room = ceiling.saturating_sub(local_then);
                let spent = local.saturating_sub(local_then).saturating_mul(2);
                if spent.saturating_add(incoming.saturating_mul(2)) > room {
                    return Ok(Some(room.saturating_sub(spent) / 2));
                }
                Ok(None)
            }
            FolderCopy::OtherVolume { local_then, room } => {
                let spent = local.saturating_sub(local_then);
                if spent.saturating_add(incoming) > room {
                    return Ok(Some(room.saturating_sub(spent)));
                }
                Ok(Self::past_ceiling(budget.local_ceiling, local, incoming))
            }
            FolderCopy::None => Ok(Self::past_ceiling(budget.local_ceiling, local, incoming)),
        }
    }

    fn past_ceiling(ceiling: Option<u64>, local: u64, incoming: u64) -> Option<u64> {
        let ceiling = ceiling?;
        (local.saturating_add(incoming) > ceiling).then(|| ceiling.saturating_sub(local))
    }

    /// The account's bytes, not counting what is at `path` now -- a write
    /// there replaces it, so an edit that grows a file by a kilobyte is charged
    /// a kilobyte.
    pub(super) fn account_besides(&self, path: &str, budget: WriteBudget) -> Result<u64> {
        let replaced = self.index.get_file(path)?.map_or(0, |entry| entry.size);
        Ok(budget
            .elsewhere
            .saturating_add(self.index.local_bytes()?)
            .saturating_sub(replaced))
    }

    pub(super) fn refuse_past(
        path: &str,
        budget: WriteBudget,
        account: u64,
        total: u64,
    ) -> Result<()> {
        if total > budget.allowed {
            return Err(StoreError::OverBudget {
                path: path.to_owned(),
                account,
                total,
                allowed: budget.allowed,
            });
        }
        // The account may have room and this disk not: what the pledge still
        // owes is space promised to other people, and filling it with our own
        // files breaks that promise the day they send their data.
        let local_before = account.saturating_sub(budget.elsewhere);
        let local_after = total.saturating_sub(budget.elsewhere);
        if let Some(ceiling) = budget
            .local_ceiling
            .filter(|ceiling| local_after > *ceiling)
        {
            return Err(StoreError::DiskFull {
                path: path.to_owned(),
                incoming: total.saturating_sub(account),
                room: ceiling.saturating_sub(local_before),
            });
        }
        Ok(())
    }
}
