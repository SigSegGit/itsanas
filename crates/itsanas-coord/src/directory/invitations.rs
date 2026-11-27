//! Invitations: lodging a signed code, counting what a member offers, and
//! spending one use of it when somebody joins.

use itsanas_crypto::UserId;
use redb::ReadableTable;

use super::{
    Directory, INVITATIONS, INVITE_WINDOW, INVITED, INVITED_PLACES, LodgedInvitation, SUSPENDED,
    escrow::escrow_try, window_over,
};
use crate::{
    error::{CoordError, Result},
    invitation::{self, Secret, SignedInvitation},
};

impl Directory {
    /// File an invitation an existing member has signed.
    ///
    /// Refuses one whose inviter is not a member of this coordinator: an
    /// endorsement from somebody nobody has heard of endorses nothing, and
    /// accepting it would let anyone with a keypair fill the table.
    ///
    /// Re-lodging the same code is a no-op that keeps whatever remains, so a
    /// client retrying after a dropped connection cannot refill a spent
    /// invitation.
    pub fn lodge_invitation(
        &self,
        signed: &SignedInvitation,
        now: u64,
    ) -> Result<LodgedInvitation> {
        signed.verify()?;
        if signed.invitation.expires_unix <= now {
            return Err(CoordError::Rejected("that invitation has already expired"));
        }
        if self.account_of(signed.invitation.inviter)?.is_none() {
            return Err(CoordError::Rejected(
                "the inviter is not a member of this coordinator",
            ));
        }
        if self.is_suspended(signed.invitation.inviter)? {
            return Err(CoordError::Suspended);
        }

        let key = signed.invitation.code;
        let txn = self.db.begin_write()?;
        let lodged;
        {
            let mut table = txn.open_table(INVITATIONS)?;
            lodged = if let Some(value) = table.get(key.as_slice())? {
                postcard::from_bytes(value.value())?
            } else {
                count_invited(&txn, signed.invitation.inviter, signed.invitation.uses, now)?;
                LodgedInvitation {
                    signed: signed.clone(),
                    remaining: signed.invitation.uses,
                    admitted: Vec::new(),
                }
            };
            table.insert(key.as_slice(), postcard::to_stdvec(&lodged)?.as_slice())?;
        }
        txn.commit()?;
        Ok(lodged)
    }

    /// What was lodged under this code, if anything.
    pub fn invitation(&self, code: &[u8; 32]) -> Result<Option<LodgedInvitation>> {
        let txn = self.db.begin_read()?;
        match txn.open_table(INVITATIONS)?.get(code.as_slice())? {
            Some(value) => Ok(Some(postcard::from_bytes(value.value())?)),
            None => Ok(None),
        }
    }
}

/// Count `places` more offered by `inviter` at `now`, or refuse: past
/// [`INVITED_PLACES`] in its [`INVITE_WINDOW`], nothing is written.
fn count_invited(
    txn: &redb::WriteTransaction,
    inviter: UserId,
    places: u32,
    now: u64,
) -> Result<()> {
    let mut table = txn.open_table(INVITED)?;
    let current = table
        .get(inviter.as_bytes().as_slice())?
        .and_then(|value| escrow_try(value.value()));
    let (offered, since) = match current {
        Some((offered, since)) if !window_over(since, now, INVITE_WINDOW) => (offered, since),
        _ => (0, now),
    };
    let offered = offered.saturating_add(places);
    if offered > INVITED_PLACES {
        return Err(CoordError::Rejected(
            "this member has offered all the places it may invite this month; an invitation already given still works",
        ));
    }
    let mut value = offered.to_le_bytes().to_vec();
    value.extend_from_slice(&since.to_le_bytes());
    table.insert(inviter.as_bytes().as_slice(), value.as_slice())?;
    Ok(())
}

/// Spend one use of the invitation `secret` opens, for `joiner`.
///
/// Takes the caller's open write transaction rather than opening its own, so
/// that redeeming and creating the account either both happen or neither does.
/// With two transactions, a registration that failed after the redemption
/// burned the invitation and created nothing.
///
/// Every reason for refusal is deliberately the same error. A coordinator that
/// distinguished "no such code" from "expired" from "spent" would let anybody
/// enumerate which codes exist, and the codes are what keeps strangers out.
pub(super) fn redeem_in(
    txn: &redb::WriteTransaction,
    by_id: &redb::Table<'_, &[u8], &str>,
    secret: &Secret,
    joiner: UserId,
    now: u64,
) -> Result<UserId> {
    let refused = || CoordError::Rejected("that invitation cannot be used");
    let key = invitation::code_id(secret);

    let mut table = txn.open_table(INVITATIONS)?;
    let Some(value) = table.get(key.as_slice())? else {
        return Err(refused());
    };
    let mut lodged: LodgedInvitation = postcard::from_bytes(value.value())?;
    drop(value);

    // Re-checked here rather than trusted from lodging time: the row has been
    // on disk since, and the signature is the only thing that makes any of it
    // mean anything.
    lodged.signed.verify().map_err(|_| refused())?;
    if !lodged.signed.opens_with(secret)
        || lodged.remaining == 0
        || lodged.signed.invitation.expires_unix <= now
    {
        return Err(refused());
    }

    // The inviter's standing now, not at lodging: a member suspended or
    // deleted since admits nobody with the codes it handed out.
    let inviter = lodged.signed.invitation.inviter.as_bytes();
    if by_id.get(inviter.as_slice())?.is_none()
        || txn
            .open_table(SUSPENDED)?
            .get(inviter.as_slice())?
            .is_some()
    {
        return Err(refused());
    }

    // Somebody re-registering on a code they already used spends nothing.
    // Without this, a retry after a dropped connection eats a use, and a member
    // enrolling a machine twice locks themselves out.
    if !lodged.admitted.contains(&joiner) {
        lodged.remaining -= 1;
        lodged.admitted.push(joiner);
    }
    let inviter = lodged.signed.invitation.inviter;
    table.insert(key.as_slice(), postcard::to_stdvec(&lodged)?.as_slice())?;
    Ok(inviter)
}
