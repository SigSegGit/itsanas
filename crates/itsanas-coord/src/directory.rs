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
            let _ = txn.open_table(PRESENCE)?;
            let _ = txn.open_table(ESCROW)?;
            let _ = txn.open_table(USAGE)?;
            let _ = txn.open_table(AVAILABILITY)?;
        }
        txn.commit()?;

        Ok(Self { db })
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
        let returning = self.account_of(joiner)?.is_some();

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
                    signed.supersedes(existing)
                }
            };

            if changed {
                claims.insert(key.as_slice(), postcard::to_stdvec(signed)?.as_slice())?;
            }
        }

        txn.commit()?;
        Ok(changed)
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
        let txn = self.db.begin_write()?;
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
        for signed in self.live_claims()? {
            if signed.claim.owner != user {
                continue;
            }

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
