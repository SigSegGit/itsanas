//! This account's device roster: who is a member, who was revoked.

use itsanas_crypto::DeviceId;
use redb::{ReadableTable, ReadableTableMetadata};

use super::{Index, ROSTER};
use crate::error::Result;

impl Index {
    /// Admit `device` to this account's roster, or mark it revoked. The
    /// caller has verified the claim's signature against this account; a
    /// revocation is never undone by a later admission.
    ///
    /// # Errors
    ///
    /// If the index cannot be written.
    pub fn admit_member(&self, device: &DeviceId, revoked: bool) -> Result<()> {
        // Every round admits the same members again: a row that already says
        // what this would write is not written (no commit).
        let current = self
            .db
            .begin_read()?
            .open_table(ROSTER)?
            .get(device.as_bytes().as_slice())?
            .map(|row| row.value());
        if current == Some(0) || current == Some(u8::from(!revoked)) {
            return Ok(());
        }
        let txn = self.db.begin_write()?;
        {
            let mut roster = txn.open_table(ROSTER)?;
            let key = device.as_bytes().as_slice();
            let already_revoked = roster.get(key)?.is_some_and(|row| row.value() == 0);
            if !already_revoked {
                roster.insert(key, u8::from(!revoked))?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    /// Whether the roster has any row: an unarmed roster (a store no claim
    /// was ever admitted to, which a node never is once open) gates nothing.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn roster_armed(&self) -> Result<bool> {
        let txn = self.db.begin_read()?;
        Ok(!txn.open_table(ROSTER)?.is_empty()?)
    }

    /// Whether `device` was withdrawn from this account: its roster row says
    /// revoked, for good.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn is_revoked(&self, device: &DeviceId) -> Result<bool> {
        let txn = self.db.begin_read()?;
        Ok(txn
            .open_table(ROSTER)?
            .get(device.as_bytes().as_slice())?
            .is_some_and(|row| row.value() == 0))
    }

    /// The live members of this account's roster.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn members(&self) -> Result<Vec<DeviceId>> {
        let txn = self.db.begin_read()?;
        let mut out = Vec::new();
        for row in txn.open_table(ROSTER)?.iter()? {
            let (key, value) = row?;
            if value.value() == 1 {
                out.push(DeviceId::from_slice(key.value())?);
            }
        }
        Ok(out)
    }

    /// Whether `device` is a live member of this account's roster.
    ///
    /// # Errors
    ///
    /// If the index cannot be read.
    pub fn is_member(&self, device: &DeviceId) -> Result<bool> {
        let txn = self.db.begin_read()?;
        Ok(txn
            .open_table(ROSTER)?
            .get(device.as_bytes().as_slice())?
            .is_some_and(|row| row.value() == 1))
    }
}
