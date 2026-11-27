//! Usage reports and escrow: the sealed recovery container, its fetch budget,
//! and the decoy handed out when there is none.

use itsanas_crypto::UserId;
use redb::{ReadableTable, ReadableTableMetadata};

use super::{
    DECOY_KEY, Directory, ESCROW, ESCROW_TRIES, MAX_ESCROW_LEN, RECOVERY_PAYLOAD_LEN, SECRETS,
    USAGE, UsageRecord, window_over,
};
use crate::error::{CoordError, Result};

impl Directory {
    /// Record how much a member reports storing.
    ///
    /// Self-reported, and the coordinator has no way to check it. A member who
    /// under-reports gains entitlement they have not earned; the cost lands on
    /// the hosts, who can refuse them independently. Verifiable usage needs
    /// hosts to report what they hold, which is deferred rather than pretended.
    pub fn report_usage(&self, user: UserId, bytes: u64, now: u64) -> Result<()> {
        let previous = self.usage(user)?;

        // Track when the member first went over, so sanctions escalate on a
        // schedule rather than the moment a number crosses a line.
        let over_since = match previous {
            Some((_, Some(since))) => Some(since),
            _ => None,
        };

        let record = UsageRecord {
            bytes,
            over_since_unix: over_since,
            reported_unix: now,
        };

        let txn = self.db.begin_write()?;
        {
            txn.open_table(USAGE)?.insert(
                user.as_bytes().as_slice(),
                postcard::to_stdvec(&record)?.as_slice(),
            )?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Mark, or clear, when a member went over their entitlement.
    pub fn set_over_since(&self, user: UserId, since: Option<u64>, now: u64) -> Result<()> {
        let bytes = self.usage(user)?.map_or(0, |(bytes, _)| bytes);
        let record = UsageRecord {
            bytes,
            over_since_unix: since,
            reported_unix: now,
        };

        let txn = self.db.begin_write()?;
        {
            txn.open_table(USAGE)?.insert(
                user.as_bytes().as_slice(),
                postcard::to_stdvec(&record)?.as_slice(),
            )?;
        }
        txn.commit()?;
        Ok(())
    }

    /// A member's reported usage, and when they went over.
    pub fn usage(&self, user: UserId) -> Result<Option<(u64, Option<u64>)>> {
        let txn = self.db.begin_read()?;
        let usage = txn.open_table(USAGE)?;
        match usage.get(user.as_bytes().as_slice())? {
            Some(value) => {
                let record: UsageRecord = postcard::from_bytes(value.value())?;
                Ok(Some((record.bytes, record.over_since_unix)))
            }
            None => Ok(None),
        }
    }

    /// Store a member's sealed escrow blob.
    pub fn put_escrow(&self, user: UserId, blob: &[u8]) -> Result<()> {
        if blob.len() > MAX_ESCROW_LEN {
            return Err(CoordError::Rejected("escrow blob is too large"));
        }

        let txn = self.db.begin_write()?;
        {
            txn.open_table(ESCROW)?
                .insert(user.as_bytes().as_slice(), blob)?;
        }
        txn.commit()?;
        Ok(())
    }

    /// Count one escrow fetch of `username` at `now`, on disk; `false` when
    /// its budget for the window is spent or the table is full.
    ///
    /// The same rule as the in-memory `EscrowLimiter`, kept across restarts.
    /// Full means refuse, never evict (evicting is how a caller would clear
    /// their own count).
    pub fn count_escrow_attempt(
        &self,
        username: &str,
        now: u64,
        allowed: u32,
        window: u64,
        capacity: usize,
    ) -> Result<bool> {
        let expired = |since: u64| window_over(since, now, window);
        let txn = self.db.begin_write()?;
        let allowed_now = {
            let mut table = txn.open_table(ESCROW_TRIES)?;
            let current = table.get(username)?.and_then(|v| escrow_try(v.value()));
            let next = match current {
                Some((_, since)) if expired(since) => Some((1, now)),
                Some((count, _)) if count >= allowed => None,
                Some((count, since)) => Some((count + 1, since)),
                None => {
                    if table.len()? >= capacity as u64 {
                        let stale: Vec<String> = table
                            .iter()?
                            .filter_map(std::result::Result::ok)
                            .filter(|(_, v)| escrow_try(v.value()).is_none_or(|(_, s)| expired(s)))
                            .map(|(k, _)| k.value().to_owned())
                            .collect();
                        for name in &stale {
                            table.remove(name.as_str())?;
                        }
                    }
                    (table.len()? < capacity as u64).then_some((1, now))
                }
            };
            match next {
                Some((count, since)) => {
                    let mut value = count.to_le_bytes().to_vec();
                    value.extend_from_slice(&since.to_le_bytes());
                    table.insert(username, value.as_slice())?;
                    true
                }
                None => false,
            }
        };
        txn.commit()?;
        Ok(allowed_now)
    }

    /// Fetch a member's escrow blob by username.
    ///
    /// Deliberately unauthenticated: somebody recovering an account has lost
    /// every device and every key, so there is nothing left to authenticate
    /// them with. That is the whole point of escrow, and its cost is that
    /// anyone who knows a username can fetch the blob and attack the passphrase
    /// offline. Hence opt-in, and hence the Argon2id cost.
    pub fn escrow(&self, username: &str) -> Result<Option<Vec<u8>>> {
        let Some(account) = self.account(username)? else {
            return Ok(None);
        };
        if !account.escrow_enabled {
            return Ok(None);
        }

        let txn = self.db.begin_read()?;
        let escrow = txn.open_table(ESCROW)?;
        match escrow.get(account.user.id.as_bytes().as_slice())? {
            Some(value) => Ok(Some(value.value().to_vec())),
            None => Ok(None),
        }
    }

    /// What `GetEscrow` hands out for `username`: its recovery container, or
    /// a decoy of the same shape when there is none.
    ///
    /// An unknown name, a deleted one, an account that never lodged a
    /// container and one that withdrew it all get a decoy, and the decoy is
    /// made on every call, real container or not, so the work does not differ
    /// either. A client then fails the same way, after the same Argon2id cost,
    /// for a wrong name as for a wrong passphrase.
    pub fn recovery_answer(&self, username: &str) -> Result<Vec<u8>> {
        let key = {
            let txn = self.db.begin_read()?;
            let secrets = txn.open_table(SECRETS)?;
            let stored = secrets
                .get(DECOY_KEY)?
                .ok_or(CoordError::Rejected("the coordinator has no decoy key"))?;
            let bytes: [u8; 32] = stored
                .value()
                .try_into()
                .map_err(|_| CoordError::Rejected("the coordinator's decoy key is damaged"))?;
            itsanas_crypto::SecretBytes::new(bytes)
        };
        let decoy = itsanas_crypto::Keystore::decoy(
            &key,
            username.as_bytes(),
            RECOVERY_PAYLOAD_LEN,
            itsanas_crypto::KdfParams::RECOMMENDED,
        )
        .to_bytes();
        Ok(self.escrow(username)?.unwrap_or(decoy))
    }
}

/// A row of [`ESCROW_TRIES`]: how many fetches, and since when.
pub(super) fn escrow_try(bytes: &[u8]) -> Option<(u32, u64)> {
    let count = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?);
    let since = u64::from_le_bytes(bytes.get(4..12)?.try_into().ok()?);
    Some((count, since))
}
