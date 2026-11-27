//! An email address bound to an account: the code that proves it is the
//! member's, the code that a recovery from it needs, and the index that
//! answers "which account is this address's" for a reminder of the name.
//!
//! The address is the only thing here a person could be found by, so it is
//! kept no longer than the account and erased with it ([`forget_rows`]). A
//! code is never stored: only a hash keyed by a secret of this coordinator's,
//! so a copy of the database names no code that would open anything.

use itsanas_crypto::{SecretBytes, UserId};
use redb::{ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use super::{Directory, SECRETS};
use crate::error::{CoordError, Result};

/// An account's address, verified or waiting for its code.
pub(super) const EMAILS: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("emails");

/// A verified address, lower-cased, and the account it belongs to.
pub(super) const EMAIL_OWNERS: TableDefinition<'_, &str, &[u8]> =
    TableDefinition::new("email_owners");

/// A recovery code waiting to be used, by account.
pub(super) const RECOVERY_CODES: TableDefinition<'_, &[u8], &[u8]> =
    TableDefinition::new("recovery_codes");

/// The key the codes are hashed under, in [`SECRETS`].
pub(super) const CODE_KEY: &str = "email-code";

/// How long a mailed code opens anything.
pub const CODE_LIFETIME_SECS: u64 = 15 * 60;

/// Wrong codes one mailed code survives. The ninth digit of chance an
/// attacker gets is this, out of a hundred million.
pub const CODE_TRIES: u32 = 5;

/// A code waiting, hashed, with its end and the wrong guesses it has had.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Pending {
    hash: [u8; 32],
    expires_unix: u64,
    tries: u32,
}

/// An account's address as the coordinator keeps it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Bound {
    address: String,
    verified: bool,
    pending: Option<Pending>,
}

/// What a code given back does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeCheck {
    /// It was the code: it is spent.
    Accepted,
    /// Not the code, or none is waiting, or it is over.
    Refused,
}

impl Directory {
    /// The address bound to `owner`, and whether it is verified.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn email_of(&self, owner: UserId) -> Result<Option<(String, bool)>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(EMAILS)?;
        Ok(match table.get(owner.as_bytes().as_slice())? {
            Some(row) => {
                let bound: Bound = postcard::from_bytes(row.value())?;
                Some((bound.address, bound.verified))
            }
            None => None,
        })
    }

    /// Bind `address` to `owner`, waiting for `code`; the address bound
    /// before, verified or not, is let go.
    ///
    /// # Errors
    ///
    /// The address is another account's verified one, or the database's.
    pub fn bind_email(&self, owner: UserId, address: &str, code: &str, now: u64) -> Result<()> {
        let folded = address.to_lowercase();
        if self
            .owner_of_email(&folded)?
            .is_some_and(|other| other != owner)
        {
            return Err(CoordError::Rejected(
                "this address is another account's; each address names one account",
            ));
        }
        let pending = Pending {
            hash: self.code_hash(owner, code)?,
            expires_unix: now.saturating_add(CODE_LIFETIME_SECS),
            tries: 0,
        };
        let txn = self.db.begin_write()?;
        {
            let mut emails = txn.open_table(EMAILS)?;
            let mut owners = txn.open_table(EMAIL_OWNERS)?;
            if let Some(row) = emails.get(owner.as_bytes().as_slice())? {
                let before: Bound = postcard::from_bytes(row.value())?;
                drop(row);
                owners.remove(before.address.to_lowercase().as_str())?;
            }
            let bound = Bound {
                address: address.to_owned(),
                verified: false,
                pending: Some(pending),
            };
            emails.insert(
                owner.as_bytes().as_slice(),
                postcard::to_stdvec(&bound)?.as_slice(),
            )?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Let `owner`'s address go: a recovery no longer needs a mailed code.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn unbind_email(&self, owner: UserId) -> Result<()> {
        let txn = self.db.begin_write()?;
        forget_rows(&txn, owner)?;
        txn.commit()?;
        Ok(())
    }

    /// Check the code a [`Directory::bind_email`] mailed; the right one
    /// verifies the address.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn confirm_email(&self, owner: UserId, code: &str, now: u64) -> Result<CodeCheck> {
        let hash = self.code_hash(owner, code)?;
        let txn = self.db.begin_write()?;
        let check = {
            let mut emails = txn.open_table(EMAILS)?;
            let Some(row) = emails.get(owner.as_bytes().as_slice())? else {
                return Ok(CodeCheck::Refused);
            };
            let mut bound: Bound = postcard::from_bytes(row.value())?;
            drop(row);
            let check = spend(&mut bound.pending, &hash, now);
            if check == CodeCheck::Accepted {
                bound.verified = true;
                txn.open_table(EMAIL_OWNERS)?.insert(
                    bound.address.to_lowercase().as_str(),
                    owner.as_bytes().as_slice(),
                )?;
            }
            emails.insert(
                owner.as_bytes().as_slice(),
                postcard::to_stdvec(&bound)?.as_slice(),
            )?;
            check
        };
        txn.commit()?;
        Ok(check)
    }

    /// The account whose verified address is `folded` (lower-cased).
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn owner_of_email(&self, folded: &str) -> Result<Option<UserId>> {
        let txn = self.db.begin_read()?;
        let owners = txn.open_table(EMAIL_OWNERS)?;
        Ok(match owners.get(folded)? {
            Some(row) => Some(user_id(row.value())?),
            None => None,
        })
    }

    /// Lodge `code` as the one a recovery of `owner` needs, replacing any.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn set_recovery_code(&self, owner: UserId, code: &str, now: u64) -> Result<()> {
        let pending = Pending {
            hash: self.code_hash(owner, code)?,
            expires_unix: now.saturating_add(CODE_LIFETIME_SECS),
            tries: 0,
        };
        let txn = self.db.begin_write()?;
        txn.open_table(RECOVERY_CODES)?.insert(
            owner.as_bytes().as_slice(),
            postcard::to_stdvec(&pending)?.as_slice(),
        )?;
        txn.commit()?;
        Ok(())
    }

    /// Spend the recovery code of `owner` if `code` is it.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn take_recovery_code(&self, owner: UserId, code: &str, now: u64) -> Result<CodeCheck> {
        let hash = self.code_hash(owner, code)?;
        let txn = self.db.begin_write()?;
        let check = {
            let mut codes = txn.open_table(RECOVERY_CODES)?;
            let mut pending = match codes.get(owner.as_bytes().as_slice())? {
                Some(row) => Some(postcard::from_bytes::<Pending>(row.value())?),
                None => None,
            };
            let check = spend(&mut pending, &hash, now);
            match pending {
                Some(pending) => {
                    codes.insert(
                        owner.as_bytes().as_slice(),
                        postcard::to_stdvec(&pending)?.as_slice(),
                    )?;
                }
                None => {
                    codes.remove(owner.as_bytes().as_slice())?;
                }
            }
            check
        };
        txn.commit()?;
        Ok(check)
    }

    /// The hash a code of `owner` is kept as: keyed by this coordinator's
    /// secret, so the database alone tells no code.
    fn code_hash(&self, owner: UserId, code: &str) -> Result<[u8; 32]> {
        let txn = self.db.begin_read()?;
        let secrets = txn.open_table(SECRETS)?;
        let stored = secrets
            .get(CODE_KEY)?
            .ok_or(CoordError::Rejected("the coordinator has no code key"))?;
        let mut material = stored.value().to_vec();
        material.extend_from_slice(owner.as_bytes());
        material.extend_from_slice(code.trim().as_bytes());
        let key = itsanas_crypto::kdf::derive("itsanas v1 email code", &material);
        Ok(*key.expose())
    }
}

/// Erase every email row of `owner` inside `txn`: what unbinding and
/// forgetting the account both do.
pub(super) fn forget_rows(txn: &redb::WriteTransaction, owner: UserId) -> Result<()> {
    let mut emails = txn.open_table(EMAILS)?;
    if let Some(row) = emails.remove(owner.as_bytes().as_slice())? {
        let bound: Bound = postcard::from_bytes(row.value())?;
        txn.open_table(EMAIL_OWNERS)?
            .remove(bound.address.to_lowercase().as_str())?;
    }
    txn.open_table(RECOVERY_CODES)?
        .remove(owner.as_bytes().as_slice())?;
    Ok(())
}

/// One guess at a waiting code: the right one before its end spends it; a
/// wrong one costs a try, and the last try, like the end, drops it.
fn spend(pending: &mut Option<Pending>, hash: &[u8; 32], now: u64) -> CodeCheck {
    let Some(waiting) = pending.as_mut() else {
        return CodeCheck::Refused;
    };
    if now > waiting.expires_unix {
        *pending = None;
        return CodeCheck::Refused;
    }
    if SecretBytes::new(waiting.hash) == SecretBytes::new(*hash) {
        *pending = None;
        return CodeCheck::Accepted;
    }
    waiting.tries += 1;
    if waiting.tries >= CODE_TRIES {
        *pending = None;
    }
    CodeCheck::Refused
}

fn user_id(bytes: &[u8]) -> Result<UserId> {
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| CoordError::Rejected("a damaged account id"))?;
    Ok(UserId::from_bytes(bytes))
}
