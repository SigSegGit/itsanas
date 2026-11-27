//! What the coordinator remembers, and for how long.
//!
//! No keys, no plaintext, no chunks. What it does hold, table by table:
//!
//! | Table | Holds | Until |
//! | --- | --- | --- |
//! | `accounts`, `accounts_by_id` | a username, its key, its date, whether escrow is on | the account is deleted or forgotten |
//! | `claims`, `claims_by_owner` | signed device enrolments | the account goes; a withdrawal stays for good, so that it stays final |
//! | `presence`, `availability` | where a device says it is, when this coordinator last heard it | silent for `PRESENCE_KEPT_FOR`, or the account goes |
//! | `escrow` | a blob sealed under a passphrase | withdrawn, or the account goes |
//! | `escrow_tries` | how many fetches of a name, since when | its window is over, or the account goes |
//! | `usage` | what a member says it stores (no message writes it yet) | the account goes |
//! | `invitations` | the hash of a code, who signed it, who came in on it | for good: attribution is what it is for |
//! | `departures` | a device's last signed notice that it is leaving | for good |
//! | `retired_names` | a deleted account's name | it is free again |
//! | `deleted_accounts` | an account's own signed deletion | for good: hosts ask, and free what they keep for it |
//! | `deleted_devices` | the device ids a deleted account had claimed | for good: no other account may claim them |
//! | `suspended_accounts` | when the operator suspended an account | the suspension is lifted |
//! | `emails`, `email_owners` | the address an account bound, whether it is verified, the hash of the code waiting | unbound, or the account goes |
//! | `recovery_codes` | the hash of the code a recovery needs, its end, its wrong guesses | used, ended, or the account goes |
//!
//! [`crate::service::CoordService::expire`] is what ends the ones with a date,
//! as the serving process starts and once a day after that.
//!
//! # Availability is measured, not asserted
//!
//! A device claims an address; it does not get to claim how reliable it is.
//! [`Directory::tick`], which nothing on the live path calls (it says why),
//! asks for each device on a fixed period "did I hear from you since the last
//! tick?" -- then folds the answer into an exponentially weighted average. A
//! node cannot inflate its own uptime by saying so, only by actually being
//! there.
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
    claim::{SignedClaim, SignedDeparture},
    error::{CoordError, Result},
    invitation::SignedInvitation,
};

mod accounts;
mod availability;
mod claims;
mod email;
mod escrow;
mod expiry;
mod invitations;
mod presence;

pub use email::{CODE_LIFETIME_SECS, CODE_TRIES, CodeCheck};

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
/// Inviter's user id -> the places it offered in its current window and when
/// that window opened ([`INVITED_PLACES`], [`INVITE_WINDOW`]).
const INVITED: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("invited");

/// How many places one member may offer in invitations per [`INVITE_WINDOW`],
/// every use of a code counted.
///
/// A member could invite themselves as often as they liked: an invitation is
/// signed by one key, and nothing counted how many it signed. Attribution
/// showed it afterwards and stopped nothing. Ten a month is a household and
/// its friends; a member filling the network with accounts of its own is held
/// to that, and is visible as their source.
pub const INVITED_PLACES: u32 = 10;

/// The window [`INVITED_PLACES`] is counted over: thirty days.
pub const INVITE_WINDOW: u64 = 30 * 24 * 60 * 60;
/// Device id -> the signed notice and when **this coordinator** received it.
///
/// A table of its own rather than a flag on availability, because a silence
/// and a departure are different facts and the regulation that will read them
/// has not been written: keeping them apart now is what leaves it free.
const DEPARTURES: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("departures");
/// A deleted account's name -> the unix time it is free again. Deleting does
/// not free a name at once, so creating and deleting the same name in a loop
/// buys nothing, and a friend's list does not show a stranger under a name it
/// knew until a year has passed (Rodin, 2026-10-12).
const RETIRED: TableDefinition<'_, &str, u64> = TableDefinition::new("retired_names");
/// A deleted account's key -> its own signed request: the tombstone hosts ask
/// about at their rounds, so a host that was offline when it happened still
/// frees what it kept, and checks the account's signature before it does.
const DELETED: TableDefinition<'_, &[u8], &[u8]> = TableDefinition::new("deleted_accounts");
/// The device ids a deleted account had claimed, kept for good: they were
/// public while it lived, and another account claiming one would be listed as
/// that machine's owner while it still announced.
const DELETED_DEVICES: TableDefinition<'_, &[u8], ()> = TableDefinition::new("deleted_devices");
/// A suspended account's key -> the unix time the operator suspended it.
///
/// Set and lifted by the operator only ([`Directory::suspend`]), never by a
/// request on the wire. Nothing else about the account changes: the operator
/// can stop it taking part, and still cannot read a byte it stored.
const SUSPENDED: TableDefinition<'_, &[u8], u64> = TableDefinition::new("suspended_accounts");
/// Escrow fetches per username: `count` (u32 LE) then `since` (unix, u64 LE).
/// On disk because a restart that cleared them handed out a fresh budget to
/// anyone who could provoke one (HANDOVER §9).
const ESCROW_TRIES: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("escrow_tries");
/// Secrets of this coordinator's own, by name: [`DECOY_KEY`] and the key mailed codes are hashed under.
const SECRETS: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("coordinator_secrets");
/// The key a recovery decoy is made from ([`Directory::recovery_answer`]).
/// Written once, on the first open, and kept: a decoy that changed across a
/// restart would be told from a real container by asking either side of it.
const DECOY_KEY: &str = "escrow-decoy";

/// Length of what a recovery container seals: a master secret and a device
/// seed, 32 bytes each, as `itsanas-node`'s `NodeSecrets` encodes them. A
/// decoy of any other length would be told from a real one by its size alone;
/// `the_coordinators_decoy_has_the_shape_of_a_real_recovery_container` in
/// `itsanas-node` holds the two together.
pub const RECOVERY_PAYLOAD_LEN: usize = 64;

/// How long a deleted account's name stays retired.
pub const NAME_RETIRED_FOR: u64 = 365 * 24 * 3600;

/// What [`Directory::forget_account`] removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Forgotten {
    /// The account's id.
    pub user: UserId,
    /// Devices whose live claim, presence and availability were removed.
    pub devices: usize,
    /// Withdrawn devices, kept so they stay withdrawn.
    pub withdrawals_kept: usize,
}

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

/// One account and its live claims, including claims hidden from members
/// while the account is suspended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountInventory {
    /// The registered account.
    pub account: Account,
    /// Whether the coordinator operator suspended this account.
    pub suspended: bool,
    /// Its unrevoked device claims, in device-id order.
    pub devices: Vec<DeviceInventory>,
}

/// One stored live claim and the locally recorded presence for its device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInventory {
    /// The signed, unrevoked device claim.
    pub claim: SignedClaim,
    /// The last recorded address, if the device announced one.
    pub address: Option<String>,
    /// When the coordinator last heard from the device, in Unix seconds.
    pub last_seen_unix: Option<u64>,
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

/// A departure notice, and when this coordinator received it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct DepartureRecord {
    notice: SignedDeparture,
    received_unix: u64,
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
    /// See [`Directory::play_old`].
    #[cfg(feature = "hostile")]
    plays_old: std::sync::atomic::AtomicBool,
    /// See [`Directory::bound_devices`].
    #[cfg(any(test, feature = "hostile"))]
    unbounded: std::sync::atomic::AtomicBool,
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
            let _ = txn.open_table(DEPARTURES)?;
            let _ = txn.open_table(RETIRED)?;
            let _ = txn.open_table(DELETED)?;
            let _ = txn.open_table(SUSPENDED)?;
            let _ = txn.open_table(email::EMAILS)?;
            let _ = txn.open_table(email::EMAIL_OWNERS)?;
            let _ = txn.open_table(email::RECOVERY_CODES)?;
            let mut secrets = txn.open_table(SECRETS)?;
            for name in [DECOY_KEY, email::CODE_KEY] {
                if secrets.get(name)?.is_none() {
                    let key = itsanas_crypto::SymmetricKey::random()?;
                    secrets.insert(name, key.expose().as_slice())?;
                }
            }
        }
        txn.commit()?;

        let directory = Self {
            db,
            #[cfg(feature = "hostile")]
            plays_old: std::sync::atomic::AtomicBool::new(false),
            #[cfg(any(test, feature = "hostile"))]
            unbounded: std::sync::atomic::AtomicBool::new(false),
        };
        directory.rebuild_owner_index_if_missing()?;
        Ok(directory)
    }

    /// Read accounts, live device claims and recorded presence without
    /// creating tables, rebuilding indexes or writing to the Directory.
    ///
    /// Intended for local operator inspection while the coordinator service
    /// is stopped. Suspended accounts are included and marked as suspended.
    pub fn read_inventory(path: impl AsRef<Path>) -> Result<Vec<AccountInventory>> {
        let db = Database::open(path.as_ref())?;
        let directory = Self {
            db,
            #[cfg(feature = "hostile")]
            plays_old: std::sync::atomic::AtomicBool::new(false),
            #[cfg(any(test, feature = "hostile"))]
            unbounded: std::sync::atomic::AtomicBool::new(false),
        };
        directory.inventory()
    }
}

/// Largest escrow blob accepted.
///
/// A keystore holding a master secret and a device seed is a few hundred bytes.
/// 4 KiB is generous and stops the directory being used as free storage.
pub const MAX_ESCROW_LEN: usize = 4096;

/// Whether a window opened at `since` is over at `now`. A clock that went
/// backwards keeps it open: the cautious side.
const fn window_over(since: u64, now: u64, window: u64) -> bool {
    now >= since && now - since >= window
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct UsageRecord {
    bytes: u64,
    over_since_unix: Option<u64>,
    reported_unix: u64,
}

#[cfg(test)]
mod disk_bytes;

#[cfg(test)]
mod tests;
