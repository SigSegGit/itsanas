//! Accounts: registering a name, looking one up, forgetting, deleting and
//! suspending an account.

use itsanas_crypto::{DeviceId, UserId};
use redb::{ReadableTable, ReadableTableMetadata};

use super::{
    ACCOUNTS, AVAILABILITY, Account, AccountInventory, Admission, BY_ID, CLAIMS, CLAIMS_BY_OWNER,
    DELETED, DELETED_DEVICES, DeviceInventory, Directory, ESCROW, ESCROW_TRIES, Forgotten,
    NAME_RETIRED_FOR, PRESENCE, RETIRED, SUSPENDED, SignedRegistration, USAGE,
    claims::live_claims_in, invitations::redeem_in, owner_device_key,
};
use crate::{
    claim::SignedClaim,
    error::{CoordError, Result},
    invitation::Secret,
};

impl Directory {
    /// Register a username, or update the record for one already held.
    ///
    /// First come, first served, and a name is bound to a key forever. Re-using
    /// a name with a *different* key is refused: usernames are what members
    /// type when they mean a particular person, and a name that can change
    /// hands is a name that can be used to impersonate.
    pub fn register(&self, signed: &SignedRegistration, now: u64) -> Result<Account> {
        self.register_admitted(signed, None, Admission::Open, now)
    }

    /// Register, under a stated admission policy.
    ///
    /// An account that already exists under the same key skips the invitation
    /// entirely: re-registering is how a member refreshes their agreement key
    /// and how a client retries a dropped connection, and demanding a fresh
    /// invitation for either would lock members out of their own accounts.
    pub fn register_admitted(
        &self,
        signed: &SignedRegistration,
        secret: Option<&Secret>,
        admission: Admission,
        now: u64,
    ) -> Result<Account> {
        signed.verify()?;

        let joiner = signed.registration.user.id;
        if self.is_suspended(joiner)? {
            return Err(CoordError::Suspended);
        }
        let held = self.account_of(joiner)?;
        let returning = held.is_some();

        // The first member of an invite-only coordinator has nobody to invite
        // them. Requiring one anyway produces a coordinator that is running,
        // reachable, correct in every detail and impossible to join — which is
        // what the first version of this did, and the chicken-and-egg was
        // written in a doc comment as though naming it were the same as
        // handling it.
        //
        // The only actor who can admit the first member is whoever started the
        // process. The window is exactly one account wide and closes by itself.
        let founding = admission == Admission::Founding && self.is_empty()?;
        let closed = matches!(admission, Admission::ByInvitation | Admission::Founding);

        let needs_invitation = closed && !returning && !founding;
        if needs_invitation && secret.is_none() {
            return Err(CoordError::Rejected(
                "this coordinator admits new members by invitation only",
            ));
        }

        let name = signed.registration.username.as_str();

        // **One key, one account.** Without this, the two questions above are
        // answered from two different tables and they disagree the moment a
        // key asks for a second name: `returning` is decided by user id and
        // `existing`, below, by username. A key that already has an account
        // therefore skipped the invitation gate (it is "returning") and then
        // fell into the branch that mints a *fresh* account -- new username,
        // `registered_unix: now` -- and repointed BY_ID at it. The comment on
        // that branch promises the joining allowance cannot be reset by
        // re-registering, and it could: one signed message a month turned a
        // thirty-day, 10 GiB allowance into a permanent free tier, and on an
        // invite-only coordinator it also minted unlimited accounts from one
        // admitted key.
        //
        // The test that was supposed to hold this varied the key and held the
        // name fixed, and its sibling varied the name and held the key fixed.
        // Neither could reach (same key, different name). That is the same
        // shape as the freshness guard that lived in one branch of three.
        //
        // Refusing rather than carrying the date forward, because a username
        // is bound to a key for ever with no release path: two names for one
        // key would leave `account_of` picking one of them, which is the
        // ambiguity that caused this.
        if let Some(held) = &held
            && held.username != name
        {
            return Err(CoordError::Rejected(
                "this key already has an account under another name",
            ));
        }

        let txn = self.db.begin_write()?;
        let account;

        {
            let mut accounts = txn.open_table(ACCOUNTS)?;
            let mut by_id = txn.open_table(BY_ID)?;

            let existing: Option<Account> = match accounts.get(name)? {
                Some(value) => Some(postcard::from_bytes(value.value())?),
                None => None,
            };

            // The code is spent before the name is looked at, inside the
            // transaction that creates the account, so a refusal below gives
            // it back: nothing is committed. (A first version redeemed in a
            // transaction of its own, and a mistyped name burned the
            // invitation.) After the name, as it was until 2026-11-24, a
            // stranger holding no valid code learnt which names were taken,
            // one refusal at a time.
            if needs_invitation {
                let secret = secret.ok_or(CoordError::Rejected(
                    "this coordinator admits new members by invitation only",
                ))?;
                redeem_in(&txn, &by_id, secret, joiner, now)?;
            }
            // After the code, not before: a retired name told a stranger with
            // none that an account of that name once existed, and until when
            // its name is kept.
            self.refuse_if_retired(name, joiner, now)?;

            account = match existing {
                Some(existing) if existing.user.id != signed.registration.user.id => {
                    return Err(CoordError::NameTaken(name.to_owned()));
                }
                // Same key: refresh the agreement key, keep the original
                // registration date so the joining allowance cannot be reset by
                // re-registering.
                Some(existing) => Account {
                    user: signed.registration.user,
                    ..existing
                },
                None => Account {
                    username: name.to_owned(),
                    user: signed.registration.user,
                    registered_unix: now,
                    escrow_enabled: false,
                },
            };

            accounts.insert(name, postcard::to_stdvec(&account)?.as_slice())?;
            by_id.insert(account.user.id.as_bytes().as_slice(), name)?;
        }

        txn.commit()?;
        Ok(account)
    }

    /// Whether no account has ever been registered here.
    ///
    /// Only the founding case reads this: an invite-only coordinator admits its
    /// first member, because an invitation to admit them would have no author.
    pub fn is_empty(&self) -> Result<bool> {
        let txn = self.db.begin_read()?;
        Ok(txn.open_table(ACCOUNTS)?.len()? == 0)
    }

    /// Look a member up by name.
    pub fn account(&self, username: &str) -> Result<Option<Account>> {
        let txn = self.db.begin_read()?;
        let accounts = txn.open_table(ACCOUNTS)?;
        match accounts.get(username)? {
            Some(value) => Ok(Some(postcard::from_bytes(value.value())?)),
            None => Ok(None),
        }
    }

    /// Look a member up by user id.
    pub fn account_of(&self, user: UserId) -> Result<Option<Account>> {
        let txn = self.db.begin_read()?;
        let by_id = txn.open_table(BY_ID)?;
        let Some(name) = by_id.get(user.as_bytes().as_slice())? else {
            return Ok(None);
        };
        let name = name.value().to_owned();
        drop(by_id);
        drop(txn);
        self.account(&name)
    }

    /// List accounts and their live claims for local operator inventory.
    ///
    /// Unlike [`Self::live_claims_of`], this includes a suspended account's
    /// unrevoked claims so the operator can identify devices without making
    /// them visible through the member protocol. Results are ordered by
    /// username; each account's claims are ordered by full device id.
    pub(super) fn inventory(&self) -> Result<Vec<AccountInventory>> {
        let txn = self.db.begin_read()?;
        let accounts = txn.open_table(ACCOUNTS)?;
        let by_owner = txn.open_table(CLAIMS_BY_OWNER)?;
        let claims = txn.open_table(CLAIMS)?;
        let suspended = txn.open_table(SUSPENDED)?;
        let mut inventory = Vec::new();

        for row in accounts.iter()? {
            let (_, value) = row?;
            let account: Account = postcard::from_bytes(value.value())?;
            let owner = account.user.id;
            let mut devices = Vec::new();
            for claim in live_claims_in(&by_owner, &claims, owner)? {
                let device = claim.claim.device;
                devices.push(DeviceInventory {
                    claim,
                    address: self
                        .presence_of(device)?
                        .map(|presence| presence.presence.address),
                    last_seen_unix: self.last_seen(device)?,
                });
            }
            inventory.push(AccountInventory {
                suspended: suspended.get(owner.as_bytes().as_slice())?.is_some(),
                devices,
                account,
            });
        }

        inventory.sort_by(|left, right| left.account.username.cmp(&right.account.username));
        Ok(inventory)
    }

    /// Forget the account registered under `username`, so the name can be
    /// registered again -- by anyone, under any key. For the operator, with
    /// the coordinator stopped: a test account left behind otherwise holds
    /// its name for ever, and the only other way out was editing this file
    /// by hand.
    ///
    /// Removes the account, its escrow, its count of escrow fetches, its usage,
    /// and its devices' live claims, presences and availability. **Keeps every
    /// withdrawal**, with its index row: a withdrawal is final for its device
    /// id (HANDOVER §6), and the same keys registering again must not bring a
    /// withdrawn machine back. Departures are kept for the same reason.
    ///
    /// `None` when no account has that name.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn forget_account(&self, username: &str) -> Result<Option<Forgotten>> {
        let txn = self.db.begin_write()?;
        let forgotten = {
            let mut accounts = txn.open_table(ACCOUNTS)?;
            let account: Account = match accounts.get(username)? {
                Some(value) => postcard::from_bytes(value.value())?,
                None => return Ok(None),
            };
            let owner = account.user.id;
            let mut index = txn.open_table(CLAIMS_BY_OWNER)?;
            let mut claims = txn.open_table(CLAIMS)?;
            let mut presence = txn.open_table(PRESENCE)?;
            let mut availability = txn.open_table(AVAILABILITY)?;
            let first = owner_device_key(owner, DeviceId::from_bytes([0x00; 32]));
            let last = owner_device_key(owner, DeviceId::from_bytes([0xff; 32]));
            let mut devices = Vec::new();
            for row in index.range(first.as_slice()..=last.as_slice())? {
                let (key, _) = row?;
                devices.push(key.value().to_vec());
            }
            let mut forgotten = Forgotten {
                user: owner,
                devices: 0,
                withdrawals_kept: 0,
            };
            for key in devices {
                let device = &key[32..];
                let revoked = match claims.get(device)? {
                    Some(value) => {
                        postcard::from_bytes::<SignedClaim>(value.value())?
                            .claim
                            .revoked
                    }
                    None => false,
                };
                if revoked {
                    forgotten.withdrawals_kept += 1;
                    continue;
                }
                claims.remove(device)?;
                index.remove(key.as_slice())?;
                presence.remove(device)?;
                availability.remove(device)?;
                forgotten.devices += 1;
            }
            txn.open_table(ESCROW)?
                .remove(owner.as_bytes().as_slice())?;
            txn.open_table(ESCROW_TRIES)?.remove(username)?;
            txn.open_table(USAGE)?.remove(owner.as_bytes().as_slice())?;
            txn.open_table(BY_ID)?.remove(owner.as_bytes().as_slice())?;
            accounts.remove(username)?;
            forgotten
        };
        txn.commit()?;
        Ok(Some(forgotten))
    }

    /// Refuse a deleted account's key for ever, and its name until it is free.
    fn refuse_if_retired(&self, name: &str, user: UserId, now: u64) -> Result<()> {
        let txn = self.db.begin_read()?;
        if txn
            .open_table(DELETED)?
            .get(user.as_bytes().as_slice())?
            .is_some()
        {
            return Err(CoordError::Deleted);
        }
        if let Some(until) = txn.open_table(RETIRED)?.get(name)?.map(|v| v.value())
            && until > now
        {
            return Err(CoordError::Retired {
                name: name.to_owned(),
                until,
            });
        }
        Ok(())
    }

    /// Delete an account at its own request (HANDOVER §8 0w (8), option A).
    ///
    /// The tombstone and the retired name are written first, in their own
    /// transaction: a crash after them leaves an account that cannot come back
    /// and whose hosts free its chunks, never one half-forgotten and free to
    /// be taken. Then the account is forgotten as `forget_account` does.
    ///
    /// # Errors
    ///
    /// A bad or stale signature, a name that is not this key's account, the
    /// database's.
    pub fn delete_account(&self, signed: &crate::claim::SignedDeletion, now: u64) -> Result<()> {
        signed.verify(now)?;
        let user = signed.deletion.user;
        let name = signed.deletion.username.as_str();
        match self.account_of(user)? {
            Some(account) if account.username == name => {}
            _ => return Err(CoordError::NoSuchAccount(name.to_owned())),
        }
        let txn = self.db.begin_write()?;
        {
            txn.open_table(DELETED)?.insert(
                user.as_bytes().as_slice(),
                postcard::to_stdvec(signed)?.as_slice(),
            )?;
            txn.open_table(RETIRED)?
                .insert(name, now.saturating_add(NAME_RETIRED_FOR))?;
            let index = txn.open_table(CLAIMS_BY_OWNER)?;
            let mut gone = txn.open_table(DELETED_DEVICES)?;
            let first = owner_device_key(user, DeviceId::from_bytes([0x00; 32]));
            let last = owner_device_key(user, DeviceId::from_bytes([0xff; 32]));
            for row in index.range(first.as_slice()..=last.as_slice())? {
                let (key, _) = row?;
                gone.insert(&key.value()[32..], ())?;
            }
        }
        txn.commit()?;
        self.forget_account(name)?;
        Ok(())
    }

    /// Which of `users` were deleted: what a host asks before freeing what it
    /// keeps for them.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn deleted_among(&self, users: &[UserId]) -> Result<Vec<crate::claim::SignedDeletion>> {
        let txn = self.db.begin_read()?;
        let table = txn.open_table(DELETED)?;
        let mut out = Vec::new();
        for user in users {
            if let Some(value) = table.get(user.as_bytes().as_slice())? {
                out.push(postcard::from_bytes(value.value())?);
            }
        }
        Ok(out)
    }

    /// Suspend an account, by its operator (an abuse report, a legal
    /// request). Returns whether it was not suspended already.
    ///
    /// Its machines are then answered as unenrolled to a host asking whether
    /// to store for them, its registrations, enrolments and announcements are
    /// refused, and it is listed to nobody. Hosts stop taking new data for it;
    /// they do **not** free what they keep, unlike after a deletion: a
    /// suspension may be lifted ([`Self::unsuspend`]) and a deletion cannot
    /// be undone, so freeing here would turn a reversible decision of the
    /// operator into a permanent loss for the member. Unenrolled rather than
    /// withdrawn for the same reason: a host keeps a withdrawal for good.
    ///
    /// # Errors
    ///
    /// No account with this key; the database's.
    pub fn suspend(&self, user: UserId, now: u64) -> Result<bool> {
        if self.account_of(user)?.is_none() {
            return Err(CoordError::NoSuchAccount(user.short()));
        }
        let txn = self.db.begin_write()?;
        let fresh = {
            let mut table = txn.open_table(SUSPENDED)?;
            let fresh = table.get(user.as_bytes().as_slice())?.is_none();
            if fresh {
                table.insert(user.as_bytes().as_slice(), now)?;
            }
            fresh
        };
        txn.commit()?;
        Ok(fresh)
    }

    /// Lift a suspension. Returns whether the account was suspended.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn unsuspend(&self, user: UserId) -> Result<bool> {
        let txn = self.db.begin_write()?;
        let was = txn
            .open_table(SUSPENDED)?
            .remove(user.as_bytes().as_slice())?
            .is_some();
        txn.commit()?;
        Ok(was)
    }

    /// Whether the operator suspended `user`.
    ///
    /// # Errors
    ///
    /// The database's.
    pub fn is_suspended(&self, user: UserId) -> Result<bool> {
        let txn = self.db.begin_read()?;
        Ok(txn
            .open_table(SUSPENDED)?
            .get(user.as_bytes().as_slice())?
            .is_some())
    }

    /// Turn escrow on or off for a member.
    pub fn set_escrow_enabled(&self, user: UserId, enabled: bool) -> Result<()> {
        let Some(mut account) = self.account_of(user)? else {
            return Err(CoordError::NoSuchAccount(user.short()));
        };
        account.escrow_enabled = enabled;

        let txn = self.db.begin_write()?;
        {
            txn.open_table(ACCOUNTS)?.insert(
                account.username.as_str(),
                postcard::to_stdvec(&account)?.as_slice(),
            )?;
        }
        txn.commit()?;
        Ok(())
    }
}
