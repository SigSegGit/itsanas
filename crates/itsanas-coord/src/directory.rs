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
use redb::{Database, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};

use crate::{
    accounting::DeviceContribution,
    claim::{SignedClaim, SignedPresence},
    error::{CoordError, Result},
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

/// A member's request to hold a username.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registration {
    pub username: String,
    pub user: UserPublic,
    pub issued_unix: u64,
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
        signed.verify()?;

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

            accounts.insert(name, postcard::to_stdvec(&account)?.as_slice())?;
            by_id.insert(account.user.id.as_bytes().as_slice(), name)?;
        }

        txn.commit()?;
        Ok(account)
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

#[cfg(test)]
mod tests {
    use super::*;
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
