//! Answering requests against a [`Directory`], and refusing the rest.
//!
//! Split from the server so it can be tested without a socket: every decision
//! about what a caller may obtain lives here, and the transport above only
//! moves bytes.
//!
//! # The rate limit is the point of having a coordinator at all
//!
//! [`Request::GetEscrow`] has to be answerable by a machine with no identity —
//! that is what recovering from nothing means. So it is the one message a
//! stranger can reach that returns something worth having, and the only defence
//! is to make asking repeatedly expensive.
//!
//! A distributed table cannot do this: a blob published to a DHT is fetched
//! once and ground offline forever, with no rate limit and no trace. This is
//! the single job where centralisation is genuinely better, and it is why the
//! coordinator survived the decentralisation audit in `docs/DESIGN.md` §8.

use std::time::{Duration, Instant};

use itsanas_crypto::{DeviceId, UserId};

use crate::claim::SignedClaim;
use crate::directory::{Account, Admission, Directory};
use crate::error::Result;
use crate::protocol::{
    COORD_VERSION, EnrolledDevice, MAX_PEERS_RETURNED, MAX_WIRE_USERNAME, Request, Response,
};

mod limiter;
mod peers;
mod probe;

pub use limiter::EscrowLimiter;

/// How many escrow fetches one username may provoke per window.
///
/// The blob is Argon2id-sealed, so a passphrase that survives a few hundred
/// guesses survives this indefinitely. Five is enough for a person who mistypes
/// and far too few for anyone working through a word list.
pub const ESCROW_ATTEMPTS: u32 = 5;

/// How long that window lasts.
pub const ESCROW_WINDOW: Duration = Duration::from_secs(15 * 60);

/// How long an address stays worth handing out after it was last confirmed.
///
/// A device that announced once and vanished should stop being suggested. This
/// network is built out of machines that are usually off, so the window is
/// generous — but not unbounded, because every stale address is a dial that
/// every peer pays for on every round, forever.
pub const PRESENCE_TTL: u64 = 7 * 24 * 3600;

/// How long an address and the date of its last announcement stay on file.
///
/// Past [`PRESENCE_TTL`] nobody is handed the address, and the only reader left
/// is the owner's own device list, where "heard from 12 days ago" is what lets
/// them recognise a machine they have lost. Past this, the file keeps the
/// enrolment and forgets where the machine was and when.
pub const PRESENCE_KEPT_FOR: u64 = 30 * 24 * 3600;

// Shorter than the time-to-live, and a pass would erase what `peers_of` is still
// handing out.
const _: () = assert!(PRESENCE_KEPT_FOR > PRESENCE_TTL);

/// How many usernames the limiter remembers at once.
///
/// The limiter is itself a table a stranger can write into, one entry per name
/// they invent. Bounded, and full means *refuse* rather than forget: dropping
/// an entry to make room is exactly how an attacker would clear their own.
pub const ESCROW_TRACKED: usize = 4096;

/// How often one device may ask to be probed.
///
/// A probe costs the coordinator an outbound connection to an address a member
/// chose, so it is the one request here that makes the coordinator *act* on the
/// internet. Once an hour per device is far more than the question needs -- a
/// node asks when its published address changed, and an address that changes
/// hourly is a machine nothing can reach anyway -- and it bounds a fleet of
/// three thousand machines at under one probe a second in the worst case
/// where every one of them asks at once.
pub const PROBE_WINDOW: Duration = Duration::from_secs(3600);

/// How many devices the probe limiter remembers.
///
/// Same reasoning as [`ESCROW_TRACKED`], and full means refuse: evicting an
/// entry to make room is how a caller would clear their own counter. Only
/// enrolled devices reach this, so the table is bounded by the fleet rather
/// than by the internet.
pub const PROBE_TRACKED: usize = 8192;

/// What a probe request resolves to, before anything touches the network.
///
/// Separating the decision from the act is what lets every rule about *which*
/// addresses may be probed be tested without a socket -- and those rules are
/// the difference between a diagnostic and a scanner somebody else aims.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// Dial this address and expect this device to answer.
    Address(String),
    /// Do not dial anything; tell the caller this.
    Refuse(String),
}

/// What one [`CoordService::expire`] pass erased.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Expired {
    /// Devices silent past [`PRESENCE_KEPT_FOR`], whose address went.
    pub addresses: usize,
    /// Deleted accounts' names whose retirement is over.
    pub names: usize,
    /// Per-name counts of escrow fetches whose window is over.
    pub escrow_counts: usize,
}

/// A coordinator's answering half.
#[derive(Debug)]
pub struct CoordService<'a> {
    directory: &'a Directory,
    admission: Admission,
}

impl<'a> CoordService<'a> {
    /// Answer requests against `directory`, admitting anybody who asks.
    ///
    /// The right answer for a household: the operator is the only person who
    /// knows the address, and an invitation to admit the *first* member has no
    /// author. See [`Self::admitting`].
    #[must_use]
    pub const fn new(directory: &'a Directory) -> Self {
        Self {
            directory,
            admission: Admission::Open,
        }
    }

    /// Answer requests against `directory` under a stated admission policy.
    ///
    /// `Admission::ByInvitation` is what makes every other defence in this
    /// project mean something. Audits, the reliability pause and the probation
    /// ladder are all aimed at a hostile *host*, and a hostile host is somebody
    /// who joined — so until there was a rule about joining, they were
    /// defences against an adversary with no way in and no reason to exist.
    #[must_use]
    pub const fn admitting(directory: &'a Directory, admission: Admission) -> Self {
        Self {
            directory,
            admission,
        }
    }

    /// Erase what has outlived its use: the addresses of devices silent past
    /// [`PRESENCE_KEPT_FOR`], the names of deleted accounts that are free again,
    /// and the counts of escrow fetches whose [`ESCROW_WINDOW`] is over.
    pub fn expire(&self, now_unix: u64) -> Result<Expired> {
        Ok(Expired {
            addresses: self
                .directory
                .expire_presence(now_unix, PRESENCE_KEPT_FOR)?,
            names: self.directory.expire_retired_names(now_unix)?,
            escrow_counts: self
                .directory
                .expire_escrow_tries(now_unix, ESCROW_WINDOW.as_secs())?,
        })
    }

    /// Answer one request from `caller`, at wall-clock `now_unix`.
    ///
    /// `caller` is the device that authenticated the connection. It is an
    /// identity, never a permission: everything it authorises is checked
    /// against a signature the coordinator cannot produce.
    /// What this coordinator holds for the device `presented` names, for a
    /// host deciding whether to store for it (HANDOVER §8 1c (ii)).
    ///
    /// The caller must hold a claim the owner signed: otherwise this would be
    /// a free oracle for whose any device id is. And the answer is `None`
    /// unless the claim held here is under the same account, for the same
    /// reason: a caller holding a claim it forged under its own key for
    /// somebody else's device learns nothing about that device.
    fn standing(&self, presented: &SignedClaim) -> Result<Response> {
        if presented.verify_origin().is_err() {
            return Ok(Response::Refused(
                "that claim is not signed by the account it names".to_owned(),
            ));
        }
        let held = self
            .directory
            .claim_for(presented.claim.device)?
            .filter(|held| held.claim.owner == presented.claim.owner);
        // A suspended account's machines are answered as unenrolled, so hosts
        // store nothing new for them (`Directory::suspend` says why not more).
        let held = match held {
            Some(held) if self.directory.is_suspended(held.claim.owner)? => None,
            other => other,
        };
        Ok(Response::Standing(held.map(Box::new)))
    }

    fn hello(version: u16) -> Response {
        crate::protocol::agreed(version).map_or_else(
            || {
                Response::Refused(format!(
                    "this coordinator speaks versions {}..={COORD_VERSION}, not {version}; update this app",
                    crate::protocol::MIN_COORD_VERSION
                ))
            },
            |version| Response::Welcome { version },
        )
    }

    /// Deleting an account, and asking which accounts were deleted.
    ///
    /// The account's own key signs a deletion; the caller's device is not
    /// asked to be the account's, because every keystore of the account holds
    /// that key anyway and a lost machine is exactly when it is needed.
    fn deletion(&self, request: &Request, now_unix: u64) -> Result<Response> {
        match request {
            Request::DeleteAccount(signed) => match self.directory.delete_account(signed, now_unix)
            {
                Ok(()) => Ok(Response::Done),
                Err(error) => Ok(Response::Refused(error.to_string())),
            },
            Request::Deleted { users } if users.len() > crate::protocol::MAX_DELETED_ASKED => Ok(
                Response::Refused("too many accounts in one question".to_owned()),
            ),
            Request::Deleted { users } => {
                Ok(Response::Deleted(self.directory.deleted_among(users)?))
            }
            _ => Ok(Response::Refused("not a deletion request".to_owned())),
        }
    }

    /// One more escrow fetch of `username`: memory first (cheap, and what
    /// bounds a flood), then the disk, which a restart does not clear.
    fn escrow_allowed(
        &self,
        limiter: &mut EscrowLimiter,
        username: &str,
        now: Instant,
        now_unix: u64,
    ) -> Result<bool> {
        Ok(limiter.allow(username, now)
            && self.directory.count_escrow_attempt(
                username,
                now_unix,
                ESCROW_ATTEMPTS,
                ESCROW_WINDOW.as_secs(),
                ESCROW_TRACKED,
            )?)
    }

    pub fn handle(
        &self,
        request: &Request,
        caller: DeviceId,
        now_unix: u64,
        limiter: &mut EscrowLimiter,
        now: Instant,
    ) -> Result<Response> {
        match request {
            Request::Hello { version } => Ok(Self::hello(*version)),

            Request::Register(signed) => {
                match self
                    .directory
                    .register_admitted(signed, None, self.admission, now_unix)
                {
                    Ok(account) => Ok(Response::Account(Box::new(account))),
                    Err(error) => Ok(Response::Refused(error.to_string())),
                }
            }

            Request::RegisterInvited {
                registration,
                secret,
            } => {
                match self.directory.register_admitted(
                    registration,
                    Some(secret),
                    self.admission,
                    now_unix,
                ) {
                    Ok(account) => Ok(Response::Account(Box::new(account))),
                    Err(error) => Ok(Response::Refused(error.to_string())),
                }
            }

            Request::Invite(signed) => match self.directory.lodge_invitation(signed, now_unix) {
                Ok(_) => Ok(Response::Done),
                Err(error) => Ok(Response::Refused(error.to_string())),
            },

            Request::Lookup { username } => {
                if username.len() > MAX_WIRE_USERNAME {
                    return Ok(Response::Refused("username too long".to_owned()));
                }
                // Anybody may ask: a machine with no identity yet asks whether
                // a name is free before it makes one (setup's name check), so
                // a members-only rule broke account creation, and on an open
                // coordinator membership costs a registration and a claim
                // anyway. What bounds a list drawn up from this is the
                // server's per-address budget and its answer floor
                // (`server::NAME_QUESTIONS`, `server::NAME_ANSWER_FLOOR`). The
                // date would tell when somebody joined, and the flag which
                // names hold an escrow blob.
                Ok(match self.directory.account(username)? {
                    Some(account) => Response::Account(Box::new(Account {
                        registered_unix: 0,
                        escrow_enabled: false,
                        ..account
                    })),
                    None => Response::Missing,
                })
            }

            Request::Claim(signed) => match self.directory.claim(signed, now_unix) {
                Ok(_) => Ok(Response::Done),
                Err(error) => Ok(Response::Refused(error.to_string())),
            },

            Request::Announce(signed) => {
                // A device may only announce itself. Without this a node could
                // publish an address for somebody else's device and redirect
                // every dial at it — which TLS pinning would refuse, but only
                // after a wasted connection each time, for every peer.
                if signed.presence.device != caller {
                    return Ok(Response::Refused(
                        "a device may only announce its own address".to_owned(),
                    ));
                }
                match self.directory.announce(signed, now_unix) {
                    Ok(()) => Ok(Response::Done),
                    Err(error) => Ok(Response::Refused(error.to_string())),
                }
            }

            Request::Peers { user } => Ok(Response::Peers(self.bare_peers_of(*user, now_unix)?)),

            Request::SignedPeers { user } => {
                Ok(Response::SignedPeers(self.peers_of(*user, now_unix)?))
            }

            Request::ClaimedPeers { user } => Ok(Response::ClaimedPeers(
                self.claimed_peers_of(*user, now_unix)?,
            )),

            Request::Hosts => Ok(Response::Hosts(self.hosts_for(caller, now_unix)?)),

            Request::Standing(presented) => self.standing(presented),
            Request::DeleteAccount(_) | Request::Deleted { .. } => self.deletion(request, now_unix),

            Request::PutEscrow { blob } => self.put_escrow(caller, blob.as_deref(), now_unix),

            Request::GetEscrow { username } => {
                if username.len() > MAX_WIRE_USERNAME {
                    return Ok(Response::Refused("username too long".to_owned()));
                }
                if !self.escrow_allowed(limiter, username, now, now_unix)? {
                    // The same answer whether the name exists or not: a
                    // limiter that only triggered on real accounts would be a
                    // free oracle for which names are worth attacking.
                    return Ok(Response::Refused(
                        "too many recovery attempts for this account; try again later".to_owned(),
                    ));
                }
                // Never `Missing`: a container or its decoy, the same size and
                // shape, so a stranger cannot sort names into "exists and
                // holds one", "exists" and "does not" (`recovery_answer`).
                Ok(Response::Escrow(self.directory.recovery_answer(username)?))
            }

            Request::Devices { user } => self.devices_of(*user, caller, now_unix),

            Request::Depart(signed) => {
                // A device may only say that *it* is leaving. The signature
                // alone does not settle this: a departure signed by device A
                // and relayed over device B's connection is authentic and
                // still not B's to deliver, and accepting it would let any
                // member replay a notice it once saw into somebody else's
                // history. Same rule as `Announce`, for the same reason.
                if signed.departure.device != caller {
                    return Ok(Response::Refused(
                        "a device may only announce its own departure".to_owned(),
                    ));
                }
                match self.directory.depart(signed, now_unix) {
                    Ok(()) => Ok(Response::Done),
                    Err(error) => Ok(Response::Refused(error.to_string())),
                }
            }

            // Answered in `server.rs`, which has the device keys to prove who
            // is calling back and a socket to call with. Reaching here means
            // somebody wired a service without that, and a silent "no" would
            // read to a member as "your forward is broken".
            Request::CheckMe => Ok(Response::Refused(
                "this coordinator cannot probe: it was built without a network".to_owned(),
            )),
        }
    }

    /// Every live claim of `user`, for one of `user`'s own live devices.
    ///
    /// The caller is checked against a claim the coordinator cannot forge, not
    /// against anything it sent: a free keypair authenticates a connection and
    /// vouches for nothing.
    fn devices_of(&self, user: UserId, caller: DeviceId, now: u64) -> Result<Response> {
        let member = self.directory.claim_for(caller)?.is_some_and(|claim| {
            claim.claim.owner == user && !claim.claim.revoked && claim.verify(now).is_ok()
        });
        if !member {
            return Ok(Response::Refused(
                "only an enrolled device of an account may list its devices; run `itsanas register` on this machine first".to_owned(),
            ));
        }

        let mut out = Vec::new();
        for claim in self.directory.live_claims_of(user)? {
            let device = claim.claim.device;
            out.push(EnrolledDevice {
                device,
                pledged_bytes: claim.claim.pledged_bytes,
                silent_for: self
                    .directory
                    .last_seen(device)?
                    .map(|seen| now.saturating_sub(seen)),
                address: self
                    .directory
                    .presence_of(device)?
                    .map(|presence| presence.presence.address),
            });
        }

        // Most recently heard from first, those with no record last, then by id
        // so two askers see one order.
        out.sort_by(|a, b| {
            a.silent_for
                .unwrap_or(u64::MAX)
                .cmp(&b.silent_for.unwrap_or(u64::MAX))
                .then(a.device.cmp(&b.device))
        });
        out.truncate(MAX_PEERS_RETURNED);
        Ok(Response::Devices(out))
    }

    /// Whether this coordinator is pretending to be older than `SignedPeers`
    /// ([`Directory::play_old`]).
    #[cfg(feature = "hostile")]
    #[must_use]
    pub fn plays_old(&self) -> bool {
        self.directory.plays_old()
    }

    /// Store or withdraw an escrow blob, if the caller is a device of the owner.
    fn put_escrow(&self, caller: DeviceId, blob: Option<&[u8]>, now_unix: u64) -> Result<Response> {
        let Some(claim) = self.directory.claim_for(caller)? else {
            return Ok(Response::Refused(
                "this device is not enrolled under any account".to_owned(),
            ));
        };
        if claim.claim.revoked || claim.verify(now_unix).is_err() {
            return Ok(Response::Refused(
                "this device's enrolment is revoked or invalid".to_owned(),
            ));
        }

        let owner = claim.claim.owner;
        let outcome = match blob {
            // Storing a blob *is* the opt-in. Requiring a separate message to
            // enable it afterwards would leave every member who uploaded one
            // unable to recover, and the failure would only be discovered on
            // the day they needed it.
            Some(blob) => self
                .directory
                .put_escrow(owner, blob)
                .and_then(|()| self.directory.set_escrow_enabled(owner, true)),
            // Withdrawing turns recovery off first, so a crash between the two
            // leaves the account unrecoverable-by-passphrase rather than
            // recoverable from a blob the member asked to remove.
            None => self
                .directory
                .set_escrow_enabled(owner, false)
                .and_then(|()| self.directory.put_escrow(owner, &[])),
        };

        match outcome {
            Ok(()) => Ok(Response::Done),
            Err(error) => Ok(Response::Refused(error.to_string())),
        }
    }
}

#[cfg(test)]
mod tests;
