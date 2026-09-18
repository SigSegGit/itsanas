//! What the coordinator remembers.
//!
//! Five tables and nothing else: who is called what, which devices they own,
//! where those devices are, their sealed escrow blobs, and how much each member
//! is storing. No keys, no plaintext, no chunks.
//!
//! # Availability is measured, not asserted
//!
//! A device claims an address; it does not get to claim how reliable it is.
//! The coordinator ticks on a fixed period and asks, for each device, "did I
//! hear from you since the last tick?" — then folds the answer into an
//! exponentially weighted average. A node cannot inflate its own uptime by
//! saying so, only by actually being there.
//!
//! The average is integer per mille throughout. A member must be able to check
//! their own standing and get the same number the coordinator got, and two
//! machines disagreeing in the last bit about whether someone is in default is
//! a dispute nobody can settle.

use std::path::Path;

use itsanas_crypto::{DeviceId, Signature, UserId, UserKeys, UserPublic, verify};
use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::{
    accounting::DeviceContribution,
    claim::{SignedClaim, SignedPresence},
    error::{CoordError, Result},
    invitation::{self, Secret, SignedInvitation},
};

/// Signature domain for claiming a username.
pub const REGISTRATION_DOMAIN: &str = "itsanas v1 account registration";

/// How often availability is folded in.
pub const TICK_SECONDS: u64 = 15 * 60;

/// Weight given to the newest observation, per mille.
///
/// 10‰ over 15-minute ticks gives a half-life of about 18 hours and settles
/// over roughly a week — long enough that one bad afternoon does not change a
/// member's standing, short enough that a machine which genuinely became
/// reliable is credited within days.
pub const SMOOTHING_ALPHA_PER_MILLE: u64 = 10;

/// Longest username accepted.
pub const MAX_USERNAME_LEN: usize = 64;

const ACCOUNTS: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("accounts");
const BY_ID: TableDefinition<'_, &[u8], &str> = TableDefinition::new("accounts_by_id");
const CLAIMS: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("claims");

/// The same claims, keyed by the account that made them.
///
/// `owner` then `device`, so one account's devices are a contiguous range.
/// The value is empty: the claim itself lives once, in [`CLAIMS`], and storing
/// it twice would be two copies to disagree with each other. This table answers
/// *which* devices, and the other answers *what* each claim says.
///
/// **Why it exists.** Every "where are this account's machines" went through
/// `live_claims`, which deserialises the whole table and then filters. The cost
/// of one member's lookup was therefore O(devices in the entire network), so the
/// coordinator's total work grew with the square of the fleet. Measured on
/// 2026-09-18 before this table existed: 5.2 us with an empty directory, 575 us
/// at 500 devices, 2.58 ms at 3000.
///
/// An account keeps its devices for the life of the account and a device can
/// never change owner -- `claim` refuses that outright -- so an entry here is
/// written once and never moves. That is what makes a denormalised second copy
/// defensible: it has no update path to get wrong.
///
/// **Its safety rests on three refusals that live in other functions**, which is
/// worth knowing before any of them is relaxed:
///
/// * `claim` refuses a second account claiming a device, so the key never moves.
/// * `claim` refuses un-revoking a withdrawal, so a row never has to be removed.
/// * `register` refuses re-using a username under a different key, so an
///   account's id -- the leading half of every key here -- is stable for life.
///
/// Loosen any of those and this table needs a delete path it does not have.
const CLAIMS_BY_OWNER: TableDefinition<'_, &[u8], ()> = TableDefinition::new("claims_by_owner");

/// The key of a row in [`CLAIMS_BY_OWNER`].
fn owner_device_key(owner: UserId, device: DeviceId) -> [u8; 64] {
    let mut key = [0u8; 64];
    key[..32].copy_from_slice(&owner.to_bytes());
    key[32..].copy_from_slice(&device.to_bytes());
    key
}
const PRESENCE: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("presence");
const ESCROW: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("escrow");
const USAGE: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("usage");
/// Device → smoothed availability, per mille, plus when it was last folded in.
const AVAILABILITY: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("availability");

/// Code id -> the signed invitation and how much of it is left.
///
/// Filed under the *hash* of the secret, so the directory never holds a working
/// code. Somebody who steals this database gets a list of endorsements they
/// cannot redeem, which is the whole reason the secret stays with the inviter
/// and the invitee.
const INVITATIONS: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("invitations");

/// A member's account, as the coordinator holds it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub username: String,
    pub user: UserPublic,
    pub registered_unix: u64,
    /// Whether the member has opted in to escrow.
    ///
    /// Off by default. An escrow blob can be fetched by anyone who knows the
    /// username, so its security is exactly the strength of the passphrase —
    /// which is a decision the member should make deliberately rather than
    /// inherit.
    pub escrow_enabled: bool,
}

/// A lodged invitation, and what remains of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LodgedInvitation {
    /// What the inviter signed.
    pub signed: SignedInvitation,
    /// Uses not yet spent.
    pub remaining: u32,
    /// Who came in on it, in the order they arrived.
    ///
    /// Kept after the invitation is spent, because attribution is the point:
    /// an endorsement nobody can trace back is not an endorsement. This is what
    /// makes "who let these forty accounts in" a question with an answer.
    pub admitted: Vec<UserId>,
}

/// A member's request to hold a username.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registration {
    pub username: String,
    pub user: UserPublic,
    pub issued_unix: u64,
}

/// What a coordinator demands of somebody who wants to join.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Anybody who can reach it. What every coordinator did until now, and the
    /// right answer for a household: the operator is the only person who knows
    /// the address, and requiring an invitation to admit the *first* member is
    /// a chicken with no egg.
    Open,
    /// Only somebody holding a secret an existing member signed.
    ///
    /// A keypair costs nothing, so without this the answer to "who is a member"
    /// is "anyone who can open a socket". Every other defence in this project
    /// — audits, the reliability pause, the probation ladder — is aimed at a
    /// hostile *host*, and a hostile host is somebody who joined.
    ByInvitation,
    /// By invitation, except that an empty directory admits one account.
    ///
    /// The chicken and the egg: an invitation to admit the first member has no
    /// author, so an invite-only coordinator with nobody in it can never be
    /// joined. Something has to open the door once.
    ///
    /// It is a separate variant rather than a special case of
    /// [`Admission::ByInvitation`] because the difference is a **race**. If an
    /// empty directory always admitted its first caller, then on a public
    /// address the founder is whoever finds the port first, and the operator
    /// discovers this by being refused from their own coordinator. Making it a
    /// flag the operator passes means the window is open only while they are
    /// standing at the terminal, and it still shuts by itself after one
    /// account.
    Founding,
}

/// A [`Registration`] signed by the key it names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedRegistration {
    pub registration: Registration,
    pub signature: Signature,
}

impl Registration {
    fn payload(&self) -> Vec<u8> {
        let name = self.username.as_bytes();
        let mut out = Vec::with_capacity(4 + name.len() + 32 + 32 + 8);
        out.extend_from_slice(&u32::try_from(name.len()).unwrap_or(u32::MAX).to_le_bytes());
        out.extend_from_slice(name);
        out.extend_from_slice(self.user.id.as_bytes());
        out.extend_from_slice(&self.user.agreement);
        out.extend_from_slice(&self.issued_unix.to_le_bytes());
        out
    }

    /// Sign with the master key of the identity being registered.
    #[must_use]
    pub fn sign(self, owner: &UserKeys) -> SignedRegistration {
        let signature = owner.sign(REGISTRATION_DOMAIN, &self.payload());
        SignedRegistration {
            registration: self,
            signature,
        }
    }
}

impl SignedRegistration {
    /// Check the signature, and that the username is usable.
    pub fn verify(&self) -> Result<()> {
        validate_username(&self.registration.username)?;

        verify(
            self.registration.user.id.as_bytes(),
            REGISTRATION_DOMAIN,
            &self.registration.payload(),
            self.signature,
        )
        .map_err(|_| CoordError::BadSignature("registration"))
    }
}

/// Whether a username is acceptable.
///
/// Deliberately narrow. A directory is a place people read names out of and
/// type them back in, so anything that looks like something else is a problem:
/// mixed case invites two accounts that differ only in capitalisation, and
/// non-ASCII invites homoglyphs.
pub fn validate_username(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > MAX_USERNAME_LEN {
        return Err(CoordError::Rejected(
            "username must be between 1 and 64 characters",
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
    {
        return Err(CoordError::Rejected(
            "username may contain only lowercase ASCII letters, digits, '-' and '.'",
        ));
    }
    if name.starts_with(['-', '.']) || name.ends_with(['-', '.']) {
        return Err(CoordError::Rejected(
            "username may not start or end with '-' or '.'",
        ));
    }
    Ok(())
}

/// Smoothed availability for one device.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct AvailabilityRecord {
    per_mille: u16,
    /// When it was last folded in.
    last_tick_unix: u64,
    /// When the device was last heard from.
    last_seen_unix: u64,
}

/// The coordinator's storage.
#[derive(Debug)]
pub struct Directory {
    db: Database,
}

impl Directory {
    /// Open or create the directory at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let db = Database::create(path.as_ref())?;

        let txn = db.begin_write()?;
        {
            let _ = txn.open_table(ACCOUNTS)?;
            let _ = txn.open_table(BY_ID)?;
            let _ = txn.open_table(CLAIMS)?;
            let _ = txn.open_table(CLAIMS_BY_OWNER)?;
            let _ = txn.open_table(PRESENCE)?;
            let _ = txn.open_table(ESCROW)?;
            let _ = txn.open_table(USAGE)?;
            let _ = txn.open_table(AVAILABILITY)?;
        }
        txn.commit()?;

        let directory = Self { db };
        directory.rebuild_owner_index_if_missing()?;
        Ok(directory)
    }

    // ------------------------------------------------------------- accounts

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

            // Spending the use goes *here*, inside the transaction that
            // creates the account and after the name has been found free.
            //
            // The first version redeemed first, committed, and then opened a
            // second transaction for the account. A registration that failed
            // after that — `NameTaken` is trivial to provoke, and a mistyped
            // name provokes it by accident — burned the invitation without
            // creating anything. Free denial of service against the inviter,
            // and an invitee locked out by their own typing error.
            if needs_invitation {
                let secret = secret.ok_or(CoordError::Rejected(
                    "this coordinator admits new members by invitation only",
                ))?;
                redeem_in(&txn, secret, joiner, now)?;
            }

            accounts.insert(name, postcard::to_stdvec(&account)?.as_slice())?;
            by_id.insert(account.user.id.as_bytes().as_slice(), name)?;
        }

        txn.commit()?;
        Ok(account)
    }

    // ------------------------------------------------------------ invitations

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

        let key = signed.invitation.code;
        let txn = self.db.begin_write()?;
        let lodged;
        {
            let mut table = txn.open_table(INVITATIONS)?;
            lodged = match table.get(key.as_slice())? {
                Some(value) => postcard::from_bytes(value.value())?,
                None => LodgedInvitation {
                    signed: signed.clone(),
                    remaining: signed.invitation.uses,
                    admitted: Vec::new(),
                },
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

    // --------------------------------------------------------------- claims

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

        let key = signed.claim.device.to_bytes();
        let txn = self.db.begin_write()?;
        let changed;

        {
            let mut claims = txn.open_table(CLAIMS)?;
            let existing: Option<SignedClaim> = match claims.get(key.as_slice())? {
                Some(value) => Some(postcard::from_bytes(value.value())?),
                None => None,
            };

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
    fn rebuild_owner_index_if_missing(&self) -> Result<()> {
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
    pub fn live_claims_of(&self, user: UserId) -> Result<Vec<SignedClaim>> {
        let txn = self.db.begin_read()?;
        let index = txn.open_table(CLAIMS_BY_OWNER)?;
        let claims = txn.open_table(CLAIMS)?;

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

    /// The current claim for a device.
    pub fn claim_for(&self, device: DeviceId) -> Result<Option<SignedClaim>> {
        let txn = self.db.begin_read()?;
        let claims = txn.open_table(CLAIMS)?;
        match claims.get(device.to_bytes().as_slice())? {
            Some(value) => Ok(Some(postcard::from_bytes(value.value())?)),
            None => Ok(None),
        }
    }

    /// Every live (unrevoked) claim.
    pub fn live_claims(&self) -> Result<Vec<SignedClaim>> {
        let txn = self.db.begin_read()?;
        let claims = txn.open_table(CLAIMS)?;

        let mut out = Vec::new();
        for row in claims.iter()? {
            let (_, value) = row?;
            let signed: SignedClaim = postcard::from_bytes(value.value())?;
            if !signed.claim.revoked {
                out.push(signed);
            }
        }
        Ok(out)
    }

    // ------------------------------------------------------------- presence

    /// Record where a device says it is.
    ///
    /// Refused for a device with no live claim: an unclaimed machine announcing
    /// an address is either a mistake or somebody trying to get into the node
    /// set without an account to be held responsible.
    pub fn announce(&self, signed: &SignedPresence, now: u64) -> Result<()> {
        signed.verify(now)?;

        let claim = self
            .claim_for(signed.presence.device)?
            .filter(|claim| !claim.claim.revoked)
            .ok_or_else(|| CoordError::UnclaimedDevice(signed.presence.device.short()))?;
        let _ = claim;

        let key = signed.presence.device.to_bytes();
        let mut txn = self.db.begin_write()?;

        // An announcement is a heartbeat, and this is the one write here that
        // does not need to survive a power cut.
        //
        // Presence is re-sent every round: losing the last few seconds of it
        // costs a member being unfindable until its next announcement, which is
        // the same state it is in between announcements anyway. Availability is
        // a rolling score recomputed from the same stream, so a lost update is
        // a rounding error in a number that is already an estimate. Everything
        // that cannot be reconstructed -- registrations, enrolments, escrow,
        // invitations -- keeps the default durability, and any one of those
        // commits flushes these along with it.
        //
        // What this buys is not a micro-optimisation. Every announce was an
        // fsync, so a coordinator's presence throughput was one round trip to
        // the disk per heartbeat per member. Measured through the tests that
        // model a month of them: two of them took over sixty seconds on a CI
        // Windows runner and under three on a laptop, which is the same code
        // meeting a slower disk.
        txn.set_durability(redb::Durability::None);

        {
            let mut presence = txn.open_table(PRESENCE)?;
            let mut availability = txn.open_table(AVAILABILITY)?;

            presence.insert(key.as_slice(), postcard::to_stdvec(signed)?.as_slice())?;

            let mut record: AvailabilityRecord = match availability.get(key.as_slice())? {
                Some(value) => postcard::from_bytes(value.value())?,
                // A device seen for the first time starts at the floor rather
                // than at zero or at full marks. Zero would put a brand-new
                // machine in default; full marks would let a node inflate its
                // credit by announcing once and vanishing.
                None => AvailabilityRecord {
                    per_mille: crate::accounting::AVAILABILITY_FLOOR_PER_MILLE,
                    last_tick_unix: now,
                    last_seen_unix: now,
                },
            };
            record.last_seen_unix = now;

            availability.insert(key.as_slice(), postcard::to_stdvec(&record)?.as_slice())?;
        }
        txn.commit()?;
        Ok(())
    }

    /// When **this coordinator** last heard from a device, by its own clock.
    ///
    /// Distinct from the `at_unix` inside a presence, which is the announcing
    /// device's opinion of the time and therefore not evidence: a Raspberry Pi
    /// with no real-time clock says 1970, and anybody who wants to sort first
    /// can say whatever they like. Ordering and expiry use this instead.
    pub fn last_seen(&self, device: DeviceId) -> Result<Option<u64>> {
        let txn = self.db.begin_read()?;
        let availability = txn.open_table(AVAILABILITY)?;
        match availability.get(device.to_bytes().as_slice())? {
            Some(value) => {
                let record: AvailabilityRecord = postcard::from_bytes(value.value())?;
                Ok(Some(record.last_seen_unix))
            }
            None => Ok(None),
        }
    }

    /// Where a device was last seen.
    pub fn presence_of(&self, device: DeviceId) -> Result<Option<SignedPresence>> {
        let txn = self.db.begin_read()?;
        let presence = txn.open_table(PRESENCE)?;
        match presence.get(device.to_bytes().as_slice())? {
            Some(value) => Ok(Some(postcard::from_bytes(value.value())?)),
            None => Ok(None),
        }
    }

    /// Fold one period's observation into every device's availability.
    ///
    /// Called on a timer. A device heard from since its last tick counts as up
    /// for that period; one that was not counts as down. Nothing a node says
    /// about itself enters this calculation.
    pub fn tick(&self, now: u64) -> Result<usize> {
        let txn = self.db.begin_write()?;
        let mut folded = 0;

        {
            let mut availability = txn.open_table(AVAILABILITY)?;

            let mut updates: Vec<(Vec<u8>, AvailabilityRecord)> = Vec::new();
            for row in availability.iter()? {
                let (key, value) = row?;
                let mut record: AvailabilityRecord = postcard::from_bytes(value.value())?;

                let elapsed = now.saturating_sub(record.last_tick_unix);
                if elapsed < TICK_SECONDS {
                    continue;
                }

                // Fold one observation per elapsed period, capped so a
                // coordinator that was itself down for a month does not
                // annihilate everyone's standing in a single pass.
                let periods = (elapsed / TICK_SECONDS).min(32);
                for period in 0..periods {
                    let period_end = record
                        .last_tick_unix
                        .saturating_add((period + 1) * TICK_SECONDS);
                    let seen = record.last_seen_unix.saturating_add(TICK_SECONDS) >= period_end;
                    record.per_mille = fold(record.per_mille, seen);
                }

                record.last_tick_unix = now;
                updates.push((key.value().to_vec(), record));
                folded += 1;
            }

            for (key, record) in updates {
                availability.insert(key.as_slice(), postcard::to_stdvec(&record)?.as_slice())?;
            }
        }

        txn.commit()?;
        Ok(folded)
    }

    /// What each of a member's live devices contributes.
    pub fn contributions(&self, user: UserId) -> Result<Vec<DeviceContribution>> {
        let txn = self.db.begin_read()?;
        let availability = txn.open_table(AVAILABILITY)?;

        let mut out = Vec::new();
        for signed in self.live_claims_of(user)? {
            let key = signed.claim.device.to_bytes();
            let per_mille = match availability.get(key.as_slice())? {
                Some(value) => postcard::from_bytes::<AvailabilityRecord>(value.value())?.per_mille,
                None => crate::accounting::AVAILABILITY_FLOOR_PER_MILLE,
            };

            out.push(DeviceContribution {
                device: signed.claim.device,
                pledged_bytes: signed.claim.pledged_bytes,
                availability_per_mille: per_mille,
            });
        }

        out.sort_by_key(|contribution| contribution.device.to_bytes());
        Ok(out)
    }

    // -------------------------------------------------------- usage, escrow

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
}

/// Largest escrow blob accepted.
///
/// A keystore holding a master secret and a device seed is a few hundred bytes.
/// 4 KiB is generous and stops the directory being used as free storage.
pub const MAX_ESCROW_LEN: usize = 4096;

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct UsageRecord {
    bytes: u64,
    over_since_unix: Option<u64>,
    reported_unix: u64,
}

/// Fold one observation into a smoothed per-mille average.
fn fold(current: u16, seen: bool) -> u16 {
    let observation: u64 = if seen { 1000 } else { 0 };
    let smoothed = (u64::from(current) * (1000 - SMOOTHING_ALPHA_PER_MILLE)
        + observation * SMOOTHING_ALPHA_PER_MILLE)
        / 1000;

    u16::try_from(smoothed.min(1000)).unwrap_or(1000)
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
fn redeem_in(
    txn: &redb::WriteTransaction,
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

#[cfg(test)]
mod tests {

    /// THE LEAK THIS SHAPE INVITES: one account's devices are now a *range*
    /// rather than a filtered scan, so the filter is the key layout. Get the
    /// boundary wrong and a member's lookup returns the neighbouring account's
    /// machines -- which is both a privacy failure and an address book that
    /// tells people to dial strangers.
    ///
    /// Tested on the property that makes the range safe, with adjacent account
    /// ids constructed on purpose: every key of account *n* sorts below every
    /// key of account *n+1*, whatever devices either of them holds.
    #[test]
    fn red_team_one_accounts_range_cannot_reach_into_the_next_accounts_devices() {
        let mut lower = [0x42u8; 32];
        lower[31] = 0xfe;
        let mut upper = lower;
        upper[31] = 0xff;

        let lower = UserId::from_bytes(lower);
        let upper = UserId::from_bytes(upper);

        let highest_of_lower = owner_device_key(lower, DeviceId::from_bytes([0xff; 32]));
        let lowest_of_upper = owner_device_key(upper, DeviceId::from_bytes([0x00; 32]));

        assert!(
            highest_of_lower < lowest_of_upper,
            "the last device of one account sorts at or above the first device \
             of the next, so a range scan spills between accounts"
        );
        assert_eq!(
            &highest_of_lower[..32],
            &lower.to_bytes(),
            "the account must be the leading part of the key, or the range is \
             not a range over one account at all"
        );
    }

    /// The two tables answer the same question and must never disagree, however
    /// a claim got there: a first enrolment, a superseding claim, a withdrawal.
    /// The one this catches is a write path that updates one and not the other,
    /// which reads as a member's machines vanishing from the address book.
    #[test]
    fn the_index_and_the_claims_never_disagree_whatever_is_done_to_them() {
        let (_dir, directory) = directory();
        let alice = user(1);
        let bob = user(2);
        register(&directory, "alice", &alice);
        register(&directory, "bob", &bob);

        let laptop = DeviceKeys::from_seed(&SecretBytes::new([11; 32]));
        let pi = DeviceKeys::from_seed(&SecretBytes::new([12; 32]));
        let bobs = DeviceKeys::from_seed(&SecretBytes::new([13; 32]));

        let claim_it = |owner: &UserKeys, device: &DeviceKeys, pledged, revoked, at| {
            directory.claim(
                &NodeClaim {
                    owner: owner.user_id(),
                    device: device.device_id(),
                    pledged_bytes: pledged,
                    issued_unix: at,
                    revoked,
                }
                .sign(owner),
                at,
            )
        };

        claim_it(&alice, &laptop, 1 << 30, false, NOW).expect("enrol the laptop");
        claim_it(&alice, &pi, 2 << 30, false, NOW).expect("enrol the pi");
        claim_it(&bob, &bobs, 1 << 30, false, NOW).expect("enrol bob's machine");

        // A superseding claim: same device, new pledge.
        claim_it(&alice, &pi, 4 << 30, false, NOW + 1).expect("re-pledge the pi");

        let mine: Vec<_> = directory
            .live_claims_of(alice.user_id())
            .expect("alice's devices")
            .into_iter()
            .map(|claim| claim.claim.device)
            .collect();
        assert_eq!(mine.len(), 2, "alice has two live machines, got {mine:?}");
        assert!(mine.contains(&laptop.device_id()) && mine.contains(&pi.device_id()));
        assert!(
            !mine.contains(&bobs.device_id()),
            "another account's machine appeared in this account's list"
        );

        // The whole-table walk and the indexed lookup must agree, always.
        let by_scan: Vec<_> = directory
            .live_claims()
            .expect("every claim")
            .into_iter()
            .filter(|claim| claim.claim.owner == alice.user_id())
            .map(|claim| claim.claim.device)
            .collect();
        assert_eq!(
            sorted(by_scan),
            sorted(mine),
            "the index and the table disagree about who owns what"
        );

        // A withdrawal, which keeps its row and must stop being live.
        claim_it(&alice, &laptop, 1 << 30, true, NOW + 2).expect("withdraw the laptop");
        let after: Vec<_> = directory
            .live_claims_of(alice.user_id())
            .expect("alice's devices")
            .into_iter()
            .map(|claim| claim.claim.device)
            .collect();
        assert_eq!(
            after,
            vec![pi.device_id()],
            "a withdrawn machine is still listed as live through the index"
        );
    }

    /// THE OPERATION THAT BREAKS IT, and it is one Nicolas has performed:
    /// downgrading the coordinator binary. An older build enrols devices by
    /// writing `CLAIMS` and knowing nothing of the index. Coming back up, a
    /// repair conditioned on "the index is empty" would skip -- the index is not
    /// empty, it is *stale* -- and every machine enrolled during the downgrade
    /// would be permanently invisible: `claim_for` knows it, it can announce,
    /// and `peers_of` never returns it. Silent, permanent, and it looks to the
    /// member like their machine never joined.
    #[test]
    fn a_device_enrolled_by_an_older_binary_is_found_again_at_the_next_start() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("directory.redb");

        let alice = user(5);
        let first = DeviceKeys::from_seed(&SecretBytes::new([21; 32]));
        let during_downgrade = DeviceKeys::from_seed(&SecretBytes::new([22; 32]));

        let enrol = |directory: &Directory, device: &DeviceKeys, at: u64| {
            directory
                .claim(
                    &NodeClaim {
                        owner: alice.user_id(),
                        device: device.device_id(),
                        pledged_bytes: 1 << 30,
                        issued_unix: at,
                        revoked: false,
                    }
                    .sign(&alice),
                    at,
                )
                .expect("enrol");
        };

        {
            let directory = Directory::open(&path).expect("directory");
            register(&directory, "alice", &alice);
            enrol(&directory, &first, NOW);

            // What the older binary does: a claim written to `CLAIMS` alone.
            // The index keeps the row it already had, so it is stale rather
            // than empty -- which is the whole point of this test.
            let txn = directory.db.begin_write().expect("write");
            {
                let mut claims = txn.open_table(CLAIMS).expect("claims");
                let signed = NodeClaim {
                    owner: alice.user_id(),
                    device: during_downgrade.device_id(),
                    pledged_bytes: 1 << 30,
                    issued_unix: NOW + 1,
                    revoked: false,
                }
                .sign(&alice);
                claims
                    .insert(
                        during_downgrade.device_id().to_bytes().as_slice(),
                        postcard::to_stdvec(&signed).expect("encode").as_slice(),
                    )
                    .expect("insert");
            }
            txn.commit().expect("commit");

            let found = directory.live_claims_of(alice.user_id()).expect("devices");
            assert_eq!(
                found.len(),
                1,
                "the fixture is wrong: the older binary's claim was already indexed"
            );
        }

        let directory = Directory::open(&path).expect("reopen");
        let found: Vec<_> = directory
            .live_claims_of(alice.user_id())
            .expect("devices")
            .into_iter()
            .map(|claim| claim.claim.device)
            .collect();

        assert_eq!(
            found.len(),
            2,
            "a machine enrolled while the coordinator ran an older binary stayed \
             invisible after the upgrade; it announces and nobody is ever told"
        );
        assert!(found.contains(&during_downgrade.device_id()));
    }

    /// THE UPGRADE: a coordinator that has been running since before this table
    /// existed holds claims and no index. Reading that as "this account has no
    /// devices" would be silent and total -- every member told their machines
    /// are gone, the address book answering nothing, and the only clue being
    /// that it started at an upgrade.
    #[test]
    fn a_directory_written_before_the_index_existed_is_repaired_on_open() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("directory.redb");

        let alice = user(3);
        let laptop = DeviceKeys::from_seed(&SecretBytes::new([14; 32]));
        {
            let directory = Directory::open(&path).expect("directory");
            register(&directory, "alice", &alice);
            directory
                .claim(
                    &NodeClaim {
                        owner: alice.user_id(),
                        device: laptop.device_id(),
                        pledged_bytes: 1 << 30,
                        issued_unix: NOW,
                        revoked: false,
                    }
                    .sign(&alice),
                    NOW,
                )
                .expect("enrol");

            // Make it look like a file written by the older code: the claims
            // are there and the index is not.
            let txn = directory.db.begin_write().expect("write");
            {
                let mut index = txn.open_table(CLAIMS_BY_OWNER).expect("index");
                index.retain(|_, ()| false).expect("empty the index");
            }
            txn.commit().expect("commit");
            assert!(
                directory
                    .live_claims_of(alice.user_id())
                    .unwrap()
                    .is_empty(),
                "the fixture is wrong: the index was not emptied"
            );
        }

        let directory = Directory::open(&path).expect("reopen");
        let found = directory
            .live_claims_of(alice.user_id())
            .expect("alice's devices");
        assert_eq!(
            found.len(),
            1,
            "an upgrade lost every machine of every account"
        );
        assert_eq!(found[0].claim.device, laptop.device_id());
    }

    fn sorted(mut ids: Vec<DeviceId>) -> Vec<DeviceId> {
        ids.sort_unstable();
        ids
    }

    use super::*;
    use crate::invitation::SECRET_LEN;

    // ------------------------------------------------------------ invitations

    fn open_registration(who: &UserKeys, name: &str, now: u64) -> SignedRegistration {
        Registration {
            username: name.to_owned(),
            user: who.public(),
            issued_unix: now,
        }
        .sign(who)
    }

    fn invite(inviter: &UserKeys, secret: &Secret, uses: u32, now: u64) -> SignedInvitation {
        crate::invitation::Invitation {
            inviter: inviter.user_id(),
            code: invitation::code_id(secret),
            issued_unix: now,
            expires_unix: now + crate::invitation::DEFAULT_VALIDITY,
            uses,
        }
        .sign(inviter)
    }

    fn found(directory: &Directory, who: &UserKeys, name: &str, now: u64) -> Result<Account> {
        let signed = Registration {
            username: name.to_owned(),
            user: who.public(),
            issued_unix: now,
        }
        .sign(who);
        directory.register_admitted(&signed, None, Admission::Founding, now)
    }

    fn join(
        directory: &Directory,
        who: &UserKeys,
        name: &str,
        secret: Option<&Secret>,
        now: u64,
    ) -> Result<Account> {
        let signed = Registration {
            username: name.to_owned(),
            user: who.public(),
            issued_unix: now,
        }
        .sign(who);
        directory.register_admitted(&signed, secret, Admission::ByInvitation, now)
    }

    #[test]
    fn the_first_member_of_an_invite_only_coordinator_can_join() {
        // The chicken and the egg. An invitation to admit the first member has
        // no author, so requiring one produces a coordinator that is running,
        // reachable, correct in every detail and impossible to join. The first
        // version of this did exactly that and named the problem in a doc
        // comment, as though naming it were the same as handling it.
        let (_dir, directory) = directory();
        found(&directory, &user(1), "alice", 1_000).expect("the founder");
    }

    #[test]
    fn red_team_the_founding_window_is_asked_for_and_shuts_by_itself() {
        // THE ATTACK, and it is one the fix introduced. If an empty directory
        // always admitted its first caller, then on a public address the
        // founder is whoever finds the port first — and the operator learns
        // this by being refused from their own coordinator, with a stranger
        // already inside holding the only account that can invite.
        //
        // So the window is a flag the operator passes, open only while they are
        // standing at the terminal, and it still admits exactly one account.
        let (_dir, directory) = directory();

        // Without the flag, an empty directory admits nobody.
        assert!(
            join(&directory, &user(9), "mallory", None, 900).is_err(),
            "an empty invite-only coordinator admitted a stranger unasked"
        );

        found(&directory, &user(1), "alice", 1_000).expect("the founder");

        assert!(
            found(&directory, &user(2), "bob", 1_100).is_err(),
            "the second caller walked in through the founding window"
        );
        assert!(
            found(&directory, &user(3), "carol", 1_200).is_err(),
            "the window reopened"
        );
    }

    #[test]
    fn red_team_a_registration_that_fails_does_not_burn_the_invitation() {
        // Redeeming and creating the account used to be two transactions: spend
        // the use, commit, then write the account. Anything that failed after
        // the first — and `NameTaken` is trivial to provoke on purpose and easy
        // to provoke by accident — destroyed the invitation and created
        // nothing.
        //
        // Free denial of service against the inviter, and an invitee locked out
        // of the network by their own typing error.
        let (_dir, directory) = directory();
        let alice = user(1);
        let code = [0xBBu8; SECRET_LEN];
        directory
            .register(&open_registration(&alice, "alice", 1_000), 1_000)
            .expect("alice");
        directory
            .lodge_invitation(&invite(&alice, &code, 1, 1_000), 1_000)
            .expect("lodge");

        // Bob mistypes and asks for a name Alice already holds.
        assert!(
            join(&directory, &user(2), "alice", Some(&code), 1_100).is_err(),
            "the name was not actually taken, so this proves nothing"
        );

        let lodged = directory
            .invitation(&invitation::code_id(&code))
            .expect("read")
            .expect("filed");
        assert_eq!(
            lodged.remaining, 1,
            "a failed registration spent the invitation"
        );

        // And the same code still works once he types his own name.
        join(&directory, &user(2), "bob", Some(&code), 1_200).expect("bob, correctly");
    }

    #[test]
    fn an_invited_stranger_joins_and_an_uninvited_one_does_not() {
        let (_dir, directory) = directory();
        let alice = user(1);
        let bob = user(2);
        let carol = user(3);
        let code = [0x11u8; SECRET_LEN];

        // Alice is already a member: the first one is admitted by the operator,
        // because an invitation to admit the first member has no author.
        directory
            .register(&open_registration(&alice, "alice", 1_000), 1_000)
            .expect("alice, openly");

        directory
            .lodge_invitation(&invite(&alice, &code, 1, 1_000), 1_000)
            .expect("lodge");

        join(&directory, &bob, "bob", Some(&code), 1_100).expect("bob was invited");
        assert!(
            join(&directory, &carol, "carol", None, 1_100).is_err(),
            "a stranger with no code joined a coordinator that admits by invitation"
        );
    }

    #[test]
    fn red_team_one_invitation_admits_one_stranger_however_many_try_it() {
        // THE ATTACK: a code posted in a group chat, or leaked by the person it
        // was sent to. If uses were not spent, one endorsement would admit
        // everybody who ever saw it, and "membership costs a member's
        // endorsement" would be false for every account after the first.
        //
        // If this test fails, one leaked code is an open door.
        let (_dir, directory) = directory();
        let alice = user(1);
        let code = [0x22u8; SECRET_LEN];
        directory
            .register(&open_registration(&alice, "alice", 1_000), 1_000)
            .expect("alice");
        directory
            .lodge_invitation(&invite(&alice, &code, 1, 1_000), 1_000)
            .expect("lodge");

        join(&directory, &user(2), "first", Some(&code), 1_100).expect("the invited one");
        for (seed, name) in [(3u8, "second"), (4, "third"), (5, "fourth")] {
            assert!(
                join(&directory, &user(seed), name, Some(&code), 1_100).is_err(),
                "{name} joined on an invitation that had already been spent"
            );
        }

        let lodged = directory
            .invitation(&invitation::code_id(&code))
            .expect("read")
            .expect("still filed");
        assert_eq!(lodged.remaining, 0);
        assert_eq!(lodged.admitted.len(), 1, "more than one use was recorded");
    }

    #[test]
    fn red_team_re_lodging_a_spent_invitation_does_not_refill_it() {
        // The retry path, and a way round the previous test if it were missed.
        // A client whose connection dropped re-sends what it signed; if lodging
        // reset the counter, an inviter could refill their own code for ever
        // and one endorsement would again admit everybody.
        let (_dir, directory) = directory();
        let alice = user(1);
        let code = [0x33u8; SECRET_LEN];
        directory
            .register(&open_registration(&alice, "alice", 1_000), 1_000)
            .expect("alice");
        let signed = invite(&alice, &code, 1, 1_000);
        directory.lodge_invitation(&signed, 1_000).expect("lodge");
        join(&directory, &user(2), "bob", Some(&code), 1_100).expect("bob");

        let again = directory
            .lodge_invitation(&signed, 1_200)
            .expect("re-lodge");
        assert_eq!(again.remaining, 0, "re-lodging refilled a spent invitation");
        assert!(
            join(&directory, &user(3), "mallory", Some(&code), 1_300).is_err(),
            "a refilled invitation admitted a second stranger"
        );
    }

    #[test]
    fn red_team_a_stranger_cannot_vouch_for_a_stranger() {
        // Otherwise invitation buys nothing: an attacker mints one keypair,
        // signs invitations with it, and admits as many accounts as it likes.
        // The endorsement has to come from somebody already inside.
        let (_dir, directory) = directory();
        let outsider = user(9);
        let code = [0x44u8; SECRET_LEN];

        assert!(
            directory
                .lodge_invitation(&invite(&outsider, &code, 5, 1_000), 1_000)
                .is_err(),
            "a coordinator accepted an endorsement from somebody it has never heard of"
        );
        assert!(
            join(&directory, &user(10), "mallory", Some(&code), 1_100).is_err(),
            "the unlodged invitation admitted somebody anyway"
        );
    }

    #[test]
    fn an_expired_invitation_admits_nobody() {
        let (_dir, directory) = directory();
        let alice = user(1);
        let code = [0x55u8; SECRET_LEN];
        directory
            .register(&open_registration(&alice, "alice", 1_000), 1_000)
            .expect("alice");
        directory
            .lodge_invitation(&invite(&alice, &code, 1, 1_000), 1_000)
            .expect("lodge");

        let long_after = 1_000 + crate::invitation::DEFAULT_VALIDITY + 1;
        assert!(
            join(&directory, &user(2), "bob", Some(&code), long_after).is_err(),
            "a code from last year still opened the door"
        );
    }

    #[test]
    fn a_member_re_registering_needs_no_new_invitation() {
        // Re-registering is how a member refreshes their agreement key and how
        // a client retries a dropped connection. Demanding a fresh invitation
        // for either would lock members out of their own accounts, on a
        // coordinator whose whole job is to let them back in.
        let (_dir, directory) = directory();
        let alice = user(1);
        let bob = user(2);
        let code = [0x66u8; SECRET_LEN];
        directory
            .register(&open_registration(&alice, "alice", 1_000), 1_000)
            .expect("alice");
        directory
            .lodge_invitation(&invite(&alice, &code, 1, 1_000), 1_000)
            .expect("lodge");
        join(&directory, &bob, "bob", Some(&code), 1_100).expect("bob joins");

        join(&directory, &bob, "bob", None, 1_200).expect("bob comes back without a code");

        let lodged = directory
            .invitation(&invitation::code_id(&code))
            .expect("read")
            .expect("filed");
        assert_eq!(
            lodged.admitted.len(),
            1,
            "the return visit spent a second use"
        );
    }

    #[test]
    fn who_let_them_in_has_an_answer_afterwards() {
        // Attribution is what an endorsement is for. A member who admits forty
        // accounts that all fail their audits has to be findable, or inviting
        // is free in the only sense that matters.
        let (_dir, directory) = directory();
        let alice = user(1);
        let code = [0x77u8; SECRET_LEN];
        directory
            .register(&open_registration(&alice, "alice", 1_000), 1_000)
            .expect("alice");
        directory
            .lodge_invitation(&invite(&alice, &code, 3, 1_000), 1_000)
            .expect("lodge");

        for (seed, name) in [(2u8, "bob"), (3, "carol"), (4, "dave")] {
            join(&directory, &user(seed), name, Some(&code), 1_100).expect(name);
        }

        let lodged = directory
            .invitation(&invitation::code_id(&code))
            .expect("read")
            .expect("filed");
        assert_eq!(lodged.signed.invitation.inviter, alice.user_id());
        assert_eq!(lodged.admitted.len(), 3);
        assert_eq!(lodged.remaining, 0);
    }

    #[test]
    fn every_refusal_reads_the_same_so_codes_cannot_be_enumerated() {
        // A coordinator that said "no such code" for one and "already spent"
        // for another would let anybody probe which codes exist, and the codes
        // are the thing keeping strangers out. Same sentence, every time.
        let (_dir, directory) = directory();
        let alice = user(1);
        let spent = [0x88u8; SECRET_LEN];
        let expired = [0x99u8; SECRET_LEN];
        let unknown = [0xAAu8; SECRET_LEN];
        directory
            .register(&open_registration(&alice, "alice", 1_000), 1_000)
            .expect("alice");
        directory
            .lodge_invitation(&invite(&alice, &spent, 1, 1_000), 1_000)
            .expect("lodge");
        directory
            .lodge_invitation(&invite(&alice, &expired, 1, 1_000), 1_000)
            .expect("lodge");
        join(&directory, &user(2), "bob", Some(&spent), 1_100).expect("bob");

        let after = 1_000 + crate::invitation::DEFAULT_VALIDITY + 1;
        let reasons: Vec<String> = [&spent, &expired, &unknown]
            .iter()
            .enumerate()
            .map(|(index, code)| {
                let name = format!("probe{index}");
                let seed = u8::try_from(20 + index).unwrap_or(20);
                join(&directory, &user(seed), &name, Some(code), after)
                    .expect_err("all three must be refused")
                    .to_string()
            })
            .collect();

        assert_eq!(
            reasons
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            1,
            "three different refusals let a stranger tell live codes from dead \
             ones: {reasons:?}"
        );
    }

    use crate::claim::{NodeClaim, Presence};
    use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes};

    const NOW: u64 = 1_800_000_000;

    fn directory() -> (tempfile::TempDir, Directory) {
        let dir = tempfile::tempdir().expect("temp dir");
        let directory = Directory::open(dir.path().join("coord.redb")).expect("open");
        (dir, directory)
    }

    fn user(byte: u8) -> UserKeys {
        UserKeys::derive(&MasterSecret::from_bytes([byte; 32]))
    }

    fn device(byte: u8) -> DeviceKeys {
        DeviceKeys::from_seed(&SecretBytes::new([byte; 32]))
    }

    fn register(directory: &Directory, name: &str, owner: &UserKeys) -> Account {
        let signed = Registration {
            username: name.to_owned(),
            user: owner.public(),
            issued_unix: NOW,
        }
        .sign(owner);
        directory.register(&signed, NOW).expect("register")
    }

    fn enrol(directory: &Directory, owner: &UserKeys, dev: &DeviceKeys, pledged: u64) {
        let claim = NodeClaim {
            owner: owner.user_id(),
            device: dev.device_id(),
            pledged_bytes: pledged,
            issued_unix: NOW,
            revoked: false,
        }
        .sign(owner);
        directory.claim(&claim, NOW).expect("claim");
    }

    #[test]
    fn a_registration_round_trips() {
        let (_dir, directory) = directory();
        let owner = user(1);

        let account = register(&directory, "nicolas", &owner);
        assert_eq!(account.user.id, owner.user_id());
        assert!(!account.escrow_enabled, "escrow must be off by default");

        assert_eq!(directory.account("nicolas").unwrap(), Some(account.clone()));
        assert_eq!(
            directory.account_of(owner.user_id()).unwrap(),
            Some(account)
        );
    }

    #[test]
    fn a_username_cannot_be_taken_over_by_another_key() {
        // Usernames are what members type when they mean a particular person.
        // A name that can change hands is a name that can impersonate.
        let (_dir, directory) = directory();
        register(&directory, "nicolas", &user(1));

        let impostor = user(2);
        let signed = Registration {
            username: "nicolas".to_owned(),
            user: impostor.public(),
            issued_unix: NOW,
        }
        .sign(&impostor);

        assert!(matches!(
            directory.register(&signed, NOW),
            Err(CoordError::NameTaken(_))
        ));
        assert_eq!(
            directory.account("nicolas").unwrap().unwrap().user.id,
            user(1).user_id()
        );
    }

    #[test]
    fn re_registering_with_the_same_key_cannot_reset_the_joining_date() {
        // Otherwise the joining allowance is renewable forever by
        // re-registering, and it stops being an allowance.
        let (_dir, directory) = directory();
        let owner = user(3);
        register(&directory, "nicolas", &owner);

        let later = Registration {
            username: "nicolas".to_owned(),
            user: owner.public(),
            issued_unix: NOW + 999_999,
        }
        .sign(&owner);
        let account = directory.register(&later, NOW + 999_999).unwrap();

        assert_eq!(account.registered_unix, NOW);
    }

    #[test]
    fn red_team_a_second_username_cannot_renew_the_joining_allowance() {
        // The hole the test above could not reach, and it is the same shape as
        // the freshness guard that lived in one branch of three.
        //
        // `register_admitted` answered two questions from two tables:
        // "has this key been here before?" from BY_ID, and "does this account
        // exist?" from ACCOUNTS keyed by *name*. They agree until one key asks
        // for a second name. Then the key counts as returning -- so no
        // invitation is demanded -- and the name is unknown, so the branch that
        // preserves `registered_unix` is skipped and a fresh account is minted
        // with today's date. BY_ID is then repointed at it, so `account_of`
        // reports the new date from that moment.
        //
        // One signed message every thirty days turned a bounded joining
        // allowance -- 10 GiB regardless of what you pledge -- into a permanent
        // free tier. That is the "grand profiteur" the accounting exists to
        // stop, and it cost an attacker one round trip a month.
        //
        // The two sibling tests covered (same key, same name) and (different
        // key, same name). Nobody wrote (same key, different name), and the
        // catalogue recorded the property as established.
        let (_dir, directory) = directory();
        let owner = user(9);
        register(&directory, "alice", &owner);

        let after = NOW + crate::accounting::JOINING_PERIOD_SECONDS + 1;
        let second = Registration {
            username: "alice2".to_owned(),
            user: owner.public(),
            issued_unix: after,
        }
        .sign(&owner);

        let minted = directory.register(&second, after);
        assert!(
            minted.is_err(),
            "one key took a second username, and with it a fresh joining date"
        );

        // The record of when this member joined is the input to the whole
        // allowance, so assert on it directly rather than on the refusal alone.
        let held = directory
            .account_of(owner.user_id())
            .expect("read")
            .expect("the account is still there");
        assert_eq!(held.username, "alice");
        assert_eq!(held.registered_unix, NOW);
    }

    #[test]
    fn red_team_one_admitted_key_cannot_mint_accounts_on_an_invite_only_coordinator() {
        // The same defect on its other axis. `needs_invitation` is false for a
        // key that already has an account, which is right for somebody
        // re-registering the name they hold and wrong for anything else: an
        // admitted member could open unlimited further accounts without ever
        // presenting an invitation, and usernames here are bound to a key for
        // ever with no release path. One member could squat every short name on
        // the coordinator.
        let (_dir, directory) = directory();
        let founder = user(10);
        found(&directory, &founder, "alice", NOW).expect("the founder");

        let more = join(&directory, &founder, "alice-again", None, NOW + 10);
        assert!(
            more.is_err(),
            "an admitted key opened a second account with no invitation"
        );
    }

    #[test]
    fn a_registration_signed_by_someone_else_is_refused() {
        let (_dir, directory) = directory();
        let victim = user(4);
        let attacker = user(5);

        let mut forged = Registration {
            username: "victim".to_owned(),
            user: victim.public(),
            issued_unix: NOW,
        }
        .sign(&attacker);
        forged.registration.user = victim.public();

        assert!(directory.register(&forged, NOW).is_err());
    }

    #[test]
    fn usernames_are_narrow_on_purpose() {
        // A directory is read out loud and typed back in. Anything that can
        // look like something else is a problem.
        for good in ["nicolas", "pi-4", "a.b.c", "user123"] {
            validate_username(good).unwrap_or_else(|e| panic!("{good:?} rejected: {e}"));
        }
        for bad in [
            "",
            "Nicolas",  // mixed case invites two near-identical accounts
            "nicolas ", // trailing space
            "nicolàs",  // non-ASCII invites homoglyphs
            "-nicolas",
            "nicolas.",
            "under_score",
            &"a".repeat(MAX_USERNAME_LEN + 1),
        ] {
            assert!(validate_username(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn a_device_cannot_be_claimed_without_an_account() {
        // Otherwise the node set fills with machines belonging to nobody and
        // the accounting has no member to attribute them to.
        let (_dir, directory) = directory();
        let owner = user(6);

        let claim = NodeClaim {
            owner: owner.user_id(),
            device: device(6).device_id(),
            pledged_bytes: 1024,
            issued_unix: NOW,
            revoked: false,
        }
        .sign(&owner);

        assert!(matches!(
            directory.claim(&claim, NOW),
            Err(CoordError::NoSuchAccount(_))
        ));
    }

    #[test]
    fn a_device_cannot_be_claimed_by_two_accounts() {
        let (_dir, directory) = directory();
        let first = user(7);
        let second = user(8);
        register(&directory, "first", &first);
        register(&directory, "second", &second);

        enrol(&directory, &first, &device(7), 1024);

        let stolen = NodeClaim {
            owner: second.user_id(),
            device: device(7).device_id(),
            pledged_bytes: 1024,
            issued_unix: NOW + 10,
            revoked: false,
        }
        .sign(&second);

        assert!(matches!(
            directory.claim(&stolen, NOW + 10),
            Err(CoordError::Rejected(_))
        ));
    }

    #[test]
    fn a_revoked_device_leaves_the_live_set() {
        let (_dir, directory) = directory();
        let owner = user(9);
        register(&directory, "nicolas", &owner);
        enrol(&directory, &owner, &device(9), 1024);

        assert_eq!(directory.live_claims().unwrap().len(), 1);

        let revoked = NodeClaim {
            owner: owner.user_id(),
            device: device(9).device_id(),
            pledged_bytes: 0,
            issued_unix: NOW + 100,
            revoked: true,
        }
        .sign(&owner);
        assert!(directory.claim(&revoked, NOW + 100).unwrap());

        assert!(directory.live_claims().unwrap().is_empty());
    }

    #[test]
    fn red_team_a_machine_holding_the_master_key_cannot_bring_a_withdrawn_device_back() {
        // THE ATTACK: a laptop is stolen with its passphrase -- on Windows the
        // logon task reads it from a file beside the keystore. The owner runs
        // `itsanas device forget` from another machine. The thief runs
        // `itsanas register`, which signs a fresh claim with the master secret
        // every keystore holds, dated after the withdrawal. By timestamp alone
        // that claim won, the device was enrolled again, and the coordinator
        // said `Done`.
        let (_dir, directory) = directory();
        let owner = user(12);
        let stolen = device(12);
        register(&directory, "nicolas", &owner);
        enrol(&directory, &owner, &stolen, 1024);

        let withdrawal = NodeClaim {
            owner: owner.user_id(),
            device: stolen.device_id(),
            pledged_bytes: 0,
            issued_unix: NOW + 100,
            revoked: true,
        }
        .sign(&owner);
        assert!(directory.claim(&withdrawal, NOW + 100).unwrap());

        let re_enrolment = NodeClaim {
            owner: owner.user_id(),
            device: stolen.device_id(),
            pledged_bytes: 1024,
            issued_unix: NOW + 200,
            revoked: false,
        }
        .sign(&owner);

        assert!(
            matches!(
                directory.claim(&re_enrolment, NOW + 200),
                Err(CoordError::Rejected(_))
            ),
            "a re-enrolment of a withdrawn device was not refused out loud, so \
             `itsanas register` on the stolen machine reports success"
        );
        assert!(
            directory.live_claims().unwrap().is_empty(),
            "the withdrawn device is enrolled again"
        );
    }

    #[test]
    fn an_unclaimed_device_cannot_announce_an_address() {
        let (_dir, directory) = directory();
        let dev = device(10);

        let announced = Presence {
            device: dev.device_id(),
            address: "10.0.0.1:9797".to_owned(),
            at_unix: NOW,
        }
        .sign(&dev);

        assert!(matches!(
            directory.announce(&announced, NOW),
            Err(CoordError::UnclaimedDevice(_))
        ));
    }

    #[test]
    fn presence_is_recorded_for_a_claimed_device() {
        let (_dir, directory) = directory();
        let owner = user(11);
        let dev = device(11);
        register(&directory, "nicolas", &owner);
        enrol(&directory, &owner, &dev, 1024);

        let announced = Presence {
            device: dev.device_id(),
            address: "10.0.0.1:9797".to_owned(),
            at_unix: NOW,
        }
        .sign(&dev);
        directory.announce(&announced, NOW).unwrap();

        assert_eq!(
            directory
                .presence_of(dev.device_id())
                .unwrap()
                .unwrap()
                .presence
                .address,
            "10.0.0.1:9797"
        );
    }

    #[test]
    fn a_node_cannot_inflate_its_own_availability_by_saying_so() {
        // Availability is measured by the coordinator noticing heartbeats, not
        // asserted by the node. Announcing once must not buy full marks.
        let (_dir, directory) = directory();
        let owner = user(12);
        let dev = device(12);
        register(&directory, "nicolas", &owner);
        enrol(&directory, &owner, &dev, 100 * 1024 * 1024 * 1024);

        directory
            .announce(
                &Presence {
                    device: dev.device_id(),
                    address: "a:1".to_owned(),
                    at_unix: NOW,
                }
                .sign(&dev),
                NOW,
            )
            .unwrap();

        let contribution = directory.contributions(owner.user_id()).unwrap();
        assert_eq!(contribution.len(), 1);
        assert_eq!(
            contribution[0].availability_per_mille,
            crate::accounting::AVAILABILITY_FLOOR_PER_MILLE,
            "a single heartbeat bought more than the floor"
        );
    }

    #[test]
    fn staying_up_raises_availability_and_going_away_lowers_it() {
        let (_dir, directory) = directory();
        let owner = user(13);
        let dev = device(13);
        register(&directory, "nicolas", &owner);
        enrol(&directory, &owner, &dev, 1024);

        let announce_at = |at: u64| {
            directory
                .announce(
                    &Presence {
                        device: dev.device_id(),
                        address: "a:1".to_owned(),
                        at_unix: at,
                    }
                    .sign(&dev),
                    at,
                )
                .unwrap();
        };
        let availability =
            || directory.contributions(owner.user_id()).unwrap()[0].availability_per_mille;

        announce_at(NOW);
        let start = availability();

        // Heartbeat every period for a good while.
        let mut clock = NOW;
        for _ in 0..200 {
            clock += TICK_SECONDS;
            announce_at(clock);
            directory.tick(clock).unwrap();
        }
        let after_uptime = availability();
        assert!(
            after_uptime > start,
            "staying up did not raise availability: {start} -> {after_uptime}"
        );

        // Now vanish for the same stretch.
        for _ in 0..200 {
            clock += TICK_SECONDS;
            directory.tick(clock).unwrap();
        }
        assert!(
            availability() < after_uptime,
            "going away did not lower availability"
        );
    }

    #[test]
    fn a_coordinator_that_was_itself_down_does_not_annihilate_everyone() {
        // A coordinator offline for a month must not come back and fold a
        // month of "absent" into every member at once — the members were fine,
        // the coordinator was not.
        let (_dir, directory) = directory();
        let owner = user(14);
        let dev = device(14);
        register(&directory, "nicolas", &owner);
        enrol(&directory, &owner, &dev, 1024);

        directory
            .announce(
                &Presence {
                    device: dev.device_id(),
                    address: "a:1".to_owned(),
                    at_unix: NOW,
                }
                .sign(&dev),
                NOW,
            )
            .unwrap();

        let mut clock = NOW;
        for _ in 0..300 {
            clock += TICK_SECONDS;
            directory
                .announce(
                    &Presence {
                        device: dev.device_id(),
                        address: "a:1".to_owned(),
                        at_unix: clock,
                    }
                    .sign(&dev),
                    clock,
                )
                .unwrap();
            directory.tick(clock).unwrap();
        }
        let healthy = directory.contributions(owner.user_id()).unwrap()[0].availability_per_mille;

        // One tick, a year later.
        directory.tick(clock + 365 * 24 * 3600).unwrap();
        let after = directory.contributions(owner.user_id()).unwrap()[0].availability_per_mille;

        assert!(
            after > healthy / 2,
            "a year-long coordinator outage cut availability from {healthy} to \
             {after}; the cap on folded periods is not working"
        );
    }

    #[test]
    fn escrow_is_off_until_it_is_asked_for() {
        // Anyone who knows a username can fetch the blob, so its security is
        // exactly the passphrase. That is a decision to make deliberately.
        let (_dir, directory) = directory();
        let owner = user(15);
        register(&directory, "nicolas", &owner);

        directory.put_escrow(owner.user_id(), b"sealed").unwrap();
        assert_eq!(
            directory.escrow("nicolas").unwrap(),
            None,
            "an escrow blob was served for an account that never enabled it"
        );

        directory.set_escrow_enabled(owner.user_id(), true).unwrap();
        assert_eq!(directory.escrow("nicolas").unwrap().unwrap(), b"sealed");
    }

    #[test]
    fn an_oversized_escrow_blob_is_refused() {
        let (_dir, directory) = directory();
        let owner = user(16);
        register(&directory, "nicolas", &owner);

        assert!(
            directory
                .put_escrow(owner.user_id(), &vec![0u8; MAX_ESCROW_LEN + 1])
                .is_err(),
            "the directory can be used as free storage"
        );
    }

    #[test]
    fn escrow_for_an_unknown_username_is_absent_rather_than_an_error() {
        // A recovery attempt with a typo should say "no such account", not leak
        // whether the name exists through a different error shape.
        let (_dir, directory) = directory();
        assert_eq!(directory.escrow("nobody").unwrap(), None);
    }

    #[test]
    fn contributions_are_sorted_so_two_readers_agree() {
        let (_dir, directory) = directory();
        let owner = user(17);
        register(&directory, "nicolas", &owner);
        for byte in [30u8, 10, 20] {
            enrol(&directory, &owner, &device(byte), 1024);
        }

        let contributions = directory.contributions(owner.user_id()).unwrap();
        assert_eq!(contributions.len(), 3);
        assert!(
            contributions
                .windows(2)
                .all(|pair| pair[0].device.to_bytes() <= pair[1].device.to_bytes())
        );
    }

    #[test]
    fn everything_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coord.redb");
        let owner = user(18);

        {
            let directory = Directory::open(&path).unwrap();
            register(&directory, "nicolas", &owner);
            enrol(&directory, &owner, &device(18), 4096);
        }

        let directory = Directory::open(&path).unwrap();
        assert!(directory.account("nicolas").unwrap().is_some());
        assert_eq!(directory.live_claims().unwrap().len(), 1);
    }
}
