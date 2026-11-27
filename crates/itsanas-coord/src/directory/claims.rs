//! Device claims: enrolling a machine, the bound per account, and the index
//! that keeps one account's devices together.

use itsanas_crypto::{DeviceId, UserId};
use redb::{ReadableTable, ReadableTableMetadata};

use super::{CLAIMS, CLAIMS_BY_OWNER, Directory, SUSPENDED, owner_device_key};
use crate::{
    claim::{MAX_DEVICES_PER_ACCOUNT, SignedClaim},
    error::{CoordError, Result},
};

impl Directory {
    /// Record a device claim, if it is newer than what is held.
    ///
    /// Returns whether anything changed.
    pub fn claim(&self, signed: &SignedClaim, now: u64) -> Result<bool> {
        signed.verify(now)?;

        // A device may only be claimed by a registered account. Otherwise the
        // node set fills with machines belonging to nobody, and the accounting
        // has no member to attribute them to.
        if self.account_of(signed.claim.owner)?.is_none() {
            return Err(CoordError::NoSuchAccount(signed.claim.owner.short()));
        }
        // A suspended account enrols nothing new. A withdrawal still goes
        // through: it only ever takes a machine out.
        if !signed.claim.revoked && self.is_suspended(signed.claim.owner)? {
            return Err(CoordError::Suspended);
        }

        let key = signed.claim.device.to_bytes();
        let txn = self.db.begin_write()?;
        let changed;

        {
            let mut claims = txn.open_table(CLAIMS)?;
            let existing: Option<SignedClaim> = match claims.get(key.as_slice())? {
                Some(value) => Some(postcard::from_bytes(value.value())?),
                None => None,
            };

            // Only a live claim for a device with no claim here takes a slot.
            // A device already live is re-signing (a new pledge, `register`
            // run again) and holds its slot already; a withdrawn one is
            // refused below whatever the count; a withdrawal takes nothing.
            if existing.is_none() && !signed.claim.revoked && self.bounds_devices() {
                let index = txn.open_table(CLAIMS_BY_OWNER)?;
                let live = live_claims_in(&index, &claims, signed.claim.owner)?;
                if live.len() >= MAX_DEVICES_PER_ACCOUNT {
                    // Returning drops `txn` uncommitted: nothing is written.
                    return Err(CoordError::TooManyDevices {
                        live: live.len(),
                        limit: MAX_DEVICES_PER_ACCOUNT,
                        devices: named(&live),
                    });
                }
            }

            changed = match &existing {
                None => true,
                Some(existing) => {
                    // A device belongs to one account. Letting a second account
                    // claim it would let anyone steal a machine's identity by
                    // asserting it.
                    if existing.claim.owner != signed.claim.owner {
                        return Err(CoordError::Rejected(
                            "that device is already claimed by another account",
                        ));
                    }
                    // Refused out loud rather than ignored. `supersedes` would
                    // already keep the withdrawal, but an `Ok(false)` reaches
                    // the client as `Done`, and `itsanas register` on a
                    // withdrawn machine then printed "enrolled this device".
                    if existing.claim.revoked && !signed.claim.revoked {
                        return Err(CoordError::Rejected(
                            "that device was withdrawn from this account, and a withdrawal is final; log in afresh on that machine, which gives it a new device id",
                        ));
                    }
                    signed.supersedes(existing)
                }
            };

            if changed {
                claims.insert(key.as_slice(), postcard::to_stdvec(signed)?.as_slice())?;
            }
        }

        if changed {
            // In the same transaction as the claim itself. Two writes that can
            // land separately are two tables that can disagree, and the one
            // somebody would notice is the index: a member whose device is in
            // `CLAIMS` and not in the index is a member whose machines have
            // vanished from the address book.
            let mut index = txn.open_table(CLAIMS_BY_OWNER)?;
            index.insert(
                owner_device_key(signed.claim.owner, signed.claim.device).as_slice(),
                (),
            )?;
        }

        txn.commit()?;
        Ok(changed)
    }

    /// Fill [`CLAIMS_BY_OWNER`] from [`CLAIMS`] whenever the two disagree.
    ///
    /// A coordinator that has been running since before this table existed has
    /// claims and no index. Reading that as "this account has no devices" would
    /// be silent and total: every member told their machines are gone, the
    /// address book answering nothing, and the only clue being that it started
    /// at an upgrade. So the file is repaired on open -- the same rule, and for
    /// the same reason, as the holder ledger's second ordering in
    /// `itsanas-store`.
    ///
    /// **The condition is "the two tables hold a different number of rows", not
    /// "the index is empty", and the difference is a real operation on a real
    /// machine.** Downgrading the coordinator binary is a supported manoeuvre --
    /// `install/coordinator.sh` exists to re-run it, and the handover documents
    /// the `cp` and `systemctl` dance. An older binary enrols devices by writing
    /// `CLAIMS` and knowing nothing of this table. Coming back up, "the index is
    /// not empty" would skip the repair, and every machine enrolled during the
    /// downgrade would be **permanently invisible**: `claim_for` knows it, it
    /// can announce, and `peers_of` never returns it. Found by the Rodin audit
    /// of 2026-09-18.
    ///
    /// Row counts are the right comparison because both tables hold the same
    /// devices, revoked ones included: `claim` writes the index on every change,
    /// a withdrawal included, and a device can never change owner.
    pub(super) fn rebuild_owner_index_if_missing(&self) -> Result<()> {
        {
            let txn = self.db.begin_read()?;
            let claims = txn.open_table(CLAIMS)?;
            let index = txn.open_table(CLAIMS_BY_OWNER)?;
            if claims.len()? == index.len()? {
                return Ok(());
            }
        }

        let txn = self.db.begin_write()?;
        {
            let claims = txn.open_table(CLAIMS)?;
            let mut index = txn.open_table(CLAIMS_BY_OWNER)?;
            for row in claims.iter()? {
                let (_, value) = row?;
                let signed: SignedClaim = postcard::from_bytes(value.value())?;
                index.insert(
                    owner_device_key(signed.claim.owner, signed.claim.device).as_slice(),
                    (),
                )?;
            }
        }
        txn.commit()?;
        Ok(())
    }

    /// How many devices are enrolled, and how many the index knows of.
    ///
    /// The same number twice, always: both tables hold every device, revoked
    /// ones included. Printed by the coordinator at every start so that the
    /// invariant is something its operator can see rather than something a
    /// comment asserts -- and so that a file repaired on open says so by the
    /// numbers agreeing afterwards.
    ///
    /// # Errors
    ///
    /// If the database cannot be read.
    pub fn enrolled_counts(&self) -> Result<(u64, u64)> {
        let txn = self.db.begin_read()?;
        Ok((
            txn.open_table(CLAIMS)?.len()?,
            txn.open_table(CLAIMS_BY_OWNER)?.len()?,
        ))
    }

    /// Every live claim of one account, without reading anybody else's.
    ///
    /// What [`Self::live_claims`] gives for the whole network, for one member
    /// and at the cost of their own devices rather than of the fleet. One range
    /// scan over the index, then one point lookup per device.
    ///
    /// Revoked claims are left out, exactly as `live_claims` leaves them out: a
    /// withdrawn machine is not a device of the account any more, and the index
    /// keeps its row so that a withdrawal stays final rather than becoming a
    /// gap somebody can write into.
    ///
    /// A suspended account has none: it is handed out to nobody.
    pub fn live_claims_of(&self, user: UserId) -> Result<Vec<SignedClaim>> {
        let txn = self.db.begin_read()?;
        if txn
            .open_table(SUSPENDED)?
            .get(user.as_bytes().as_slice())?
            .is_some()
        {
            return Ok(Vec::new());
        }
        let index = txn.open_table(CLAIMS_BY_OWNER)?;
        let claims = txn.open_table(CLAIMS)?;
        live_claims_in(&index, &claims, user)
    }

    /// Whether [`Self::claim`] applies [`MAX_DEVICES_PER_ACCOUNT`].
    ///
    /// Always, except where a test has turned it off to play a coordinator
    /// built before the bound existed ([`Self::bound_devices`]).
    #[cfg_attr(not(any(test, feature = "hostile")), allow(clippy::unused_self))]
    fn bounds_devices(&self) -> bool {
        #[cfg(any(test, feature = "hostile"))]
        {
            !self.unbounded.load(std::sync::atomic::Ordering::Relaxed)
        }
        #[cfg(not(any(test, feature = "hostile")))]
        {
            true
        }
    }

    /// Play a coordinator older than [`MAX_DEVICES_PER_ACCOUNT`]: with `false`,
    /// any number of devices is admitted. For two kinds of test only -- that a
    /// client refuses a device beyond the bound on its own, and an account enrolled
    /// above the bound before it existed keeps working. Nothing built to run
    /// has it.
    #[cfg(any(test, feature = "hostile"))]
    #[doc(hidden)]
    pub fn bound_devices(&self, bound: bool) {
        self.unbounded
            .store(!bound, std::sync::atomic::Ordering::Relaxed);
    }

    /// The current claim for a device.
    pub fn claim_for(&self, device: DeviceId) -> Result<Option<SignedClaim>> {
        let txn = self.db.begin_read()?;
        let claims = txn.open_table(CLAIMS)?;
        match claims.get(device.to_bytes().as_slice())? {
            Some(value) => Ok(Some(postcard::from_bytes(value.value())?)),
            None => Ok(None),
        }
    }

    /// Every live (unrevoked) claim of an account not suspended.
    pub fn live_claims(&self) -> Result<Vec<SignedClaim>> {
        let txn = self.db.begin_read()?;
        let claims = txn.open_table(CLAIMS)?;
        let suspended = txn.open_table(SUSPENDED)?;

        let mut out = Vec::new();
        for row in claims.iter()? {
            let (_, value) = row?;
            let signed: SignedClaim = postcard::from_bytes(value.value())?;
            if !signed.claim.revoked
                && suspended
                    .get(signed.claim.owner.as_bytes().as_slice())?
                    .is_none()
            {
                out.push(signed);
            }
        }
        Ok(out)
    }
}

/// Every live claim of `user`, read through the owner index.
///
/// One function for [`Directory::live_claims_of`] and for the device bound in
/// [`Directory::claim`], so "a device of the account" means one thing to the
/// address book and to the count: a withdrawn device is not one.
pub(super) fn live_claims_in(
    index: &impl ReadableTable<&'static [u8], ()>,
    claims: &impl ReadableTable<&'static [u8], &'static [u8]>,
    user: UserId,
) -> Result<Vec<SignedClaim>> {
    let first = owner_device_key(user, DeviceId::from_bytes([0x00; 32]));
    let last = owner_device_key(user, DeviceId::from_bytes([0xff; 32]));

    let mut out = Vec::new();
    for row in index.range(first.as_slice()..=last.as_slice())? {
        let (key, _) = row?;
        let device = &key.value()[32..];
        let Some(value) = claims.get(device)? else {
            // The index names a device the claims table does not hold. That
            // cannot happen through `claim`, which writes both in one
            // transaction; skipping rather than failing keeps a damaged
            // file readable, and `live_claims` would skip it too.
            continue;
        };
        let signed: SignedClaim = postcard::from_bytes(value.value())?;
        if !signed.claim.revoked {
            out.push(signed);
        }
    }
    Ok(out)
}

/// The full ids of `live`, for a refusal a person reads. Full, not the short
/// form: the machine reading this is not enrolled, so it cannot list the
/// account's devices to resolve a short id, and when every machine of a full
/// account was lost or reinstalled it is the only machine there is -- the
/// full id is what lets it run `itsanas device forget` at all. At most a
/// screenful: an account enrolled far above the bound before it existed would
/// otherwise put an unbounded list into one wire message.
fn named(live: &[SignedClaim]) -> String {
    const SHOWN: usize = MAX_DEVICES_PER_ACCOUNT + 3;
    let mut out: Vec<String> = live
        .iter()
        .take(SHOWN)
        .map(|claim| claim.claim.device.to_string())
        .collect();
    if live.len() > SHOWN {
        out.push(format!("and {} more", live.len() - SHOWN));
    }
    out.join(", ")
}
