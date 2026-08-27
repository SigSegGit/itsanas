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

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use itsanas_crypto::{DeviceId, UserId};

use crate::claim::Presence;
use crate::directory::Directory;
use crate::error::Result;
use crate::protocol::{COORD_VERSION, MAX_PEERS_RETURNED, MAX_WIRE_USERNAME, Request, Response};

/// How many escrow fetches one username may provoke per window.
///
/// The blob is Argon2id-sealed, so a passphrase that survives a few hundred
/// guesses survives this indefinitely. Five is enough for a person who mistypes
/// and far too few for anyone working through a word list.
pub const ESCROW_ATTEMPTS: u32 = 5;

/// How long that window lasts.
pub const ESCROW_WINDOW: Duration = Duration::from_secs(15 * 60);

/// How many usernames the limiter remembers at once.
///
/// The limiter is itself a table a stranger can write into, one entry per name
/// they invent. Bounded, and full means *refuse* rather than forget: dropping
/// an entry to make room is exactly how an attacker would clear their own.
pub const ESCROW_TRACKED: usize = 4096;

/// Per-username attempt counting for escrow fetches.
#[derive(Debug)]
pub struct EscrowLimiter {
    attempts: BTreeMap<String, (u32, Instant)>,
    allowed: u32,
    window: Duration,
    capacity: usize,
}

impl EscrowLimiter {
    /// A limiter with the default budget.
    #[must_use]
    pub fn new() -> Self {
        Self::with(ESCROW_ATTEMPTS, ESCROW_WINDOW, ESCROW_TRACKED)
    }

    /// A limiter with an explicit budget, for tests.
    #[must_use]
    pub fn with(allowed: u32, window: Duration, capacity: usize) -> Self {
        Self {
            attempts: BTreeMap::new(),
            allowed,
            window,
            capacity: capacity.max(1),
        }
    }

    /// Whether one more attempt on `username` is allowed at `now`.
    ///
    /// Counts the attempt when it allows it. Taking `now` rather than reading
    /// the clock keeps the window testable without sleeping.
    pub fn allow(&mut self, username: &str, now: Instant) -> bool {
        self.forget_expired(now);

        if let Some((count, since)) = self.attempts.get_mut(username) {
            if now.duration_since(*since) >= self.window {
                *count = 1;
                *since = now;
                return true;
            }
            if *count >= self.allowed {
                return false;
            }
            *count += 1;
            return true;
        }

        // Full of live windows. Refusing a name never seen before is the safe
        // direction: the alternative is evicting somebody else's counter, which
        // is exactly how an attacker would clear their own.
        if self.attempts.len() >= self.capacity {
            return false;
        }
        self.attempts.insert(username.to_owned(), (1, now));
        true
    }

    /// Drop windows that have elapsed.
    fn forget_expired(&mut self, now: Instant) {
        let window = self.window;
        self.attempts
            .retain(|_, (_, since)| now.duration_since(*since) < window);
    }

    /// How many usernames are currently being counted.
    #[must_use]
    pub fn tracked(&self) -> usize {
        self.attempts.len()
    }
}

impl Default for EscrowLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// A coordinator's answering half.
#[derive(Debug)]
pub struct CoordService<'a> {
    directory: &'a Directory,
}

impl<'a> CoordService<'a> {
    /// Answer requests against `directory`.
    #[must_use]
    pub const fn new(directory: &'a Directory) -> Self {
        Self { directory }
    }

    /// Answer one request from `caller`, at wall-clock `now_unix`.
    ///
    /// `caller` is the device that authenticated the connection. It is an
    /// identity, never a permission: everything it authorises is checked
    /// against a signature the coordinator cannot produce.
    pub fn handle(
        &self,
        request: &Request,
        caller: DeviceId,
        now_unix: u64,
        limiter: &mut EscrowLimiter,
        now: Instant,
    ) -> Result<Response> {
        match request {
            Request::Hello { version } => Ok(if *version == COORD_VERSION {
                Response::Welcome {
                    version: COORD_VERSION,
                }
            } else {
                Response::Refused(format!(
                    "this coordinator speaks version {COORD_VERSION}; upgrade one side"
                ))
            }),

            Request::Register(signed) => match self.directory.register(signed, now_unix) {
                Ok(account) => Ok(Response::Account(Box::new(account))),
                Err(error) => Ok(Response::Refused(error.to_string())),
            },

            Request::Lookup { username } => {
                if username.len() > MAX_WIRE_USERNAME {
                    return Ok(Response::Refused("username too long".to_owned()));
                }
                Ok(match self.directory.account(username)? {
                    Some(account) => Response::Account(Box::new(account)),
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

            Request::Peers { user } => Ok(Response::Peers(self.peers_of(*user)?)),

            Request::PutEscrow { blob } => self.put_escrow(caller, blob.as_deref(), now_unix),

            Request::GetEscrow { username } => {
                if username.len() > MAX_WIRE_USERNAME {
                    return Ok(Response::Refused("username too long".to_owned()));
                }
                if !limiter.allow(username, now) {
                    // The same answer whether the name exists or not: a
                    // limiter that only triggered on real accounts would be a
                    // free oracle for which names are worth attacking.
                    return Ok(Response::Refused(
                        "too many recovery attempts for this account; try again later".to_owned(),
                    ));
                }
                Ok(match self.directory.escrow(username)? {
                    Some(blob) => Response::Escrow(blob),
                    None => Response::Missing,
                })
            }
        }
    }

    /// Live, reachable devices for `user`, newest first, bounded.
    fn peers_of(&self, user: UserId) -> Result<Vec<Presence>> {
        let mut out = Vec::new();
        for claim in self.directory.live_claims()? {
            if claim.claim.owner != user {
                continue;
            }
            if let Some(presence) = self.directory.presence_of(claim.claim.device)? {
                out.push(presence.presence);
            }
        }

        // Newest first so a client tries the most likely address, then by
        // device id so two clients asking at the same moment get the same list.
        out.sort_by(|a, b| b.at_unix.cmp(&a.at_unix).then(a.device.cmp(&b.device)));
        out.truncate(MAX_PEERS_RETURNED);
        Ok(out)
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
mod tests {
    use super::*;

    fn limiter() -> EscrowLimiter {
        EscrowLimiter::with(3, Duration::from_secs(60), 8)
    }

    #[test]
    fn red_team_grinding_one_account_is_cut_off_after_a_few_attempts() {
        // THE ATTACK: the escrow blob is the one thing a stranger can ask for
        // without proving anything, because a machine recovering from nothing
        // has nothing to prove with. Fetch it once and the passphrase can be
        // ground offline forever — so the only defence is to make *asking*
        // expensive, which is the single job a central component does better
        // than a distributed one.
        //
        // If this test fails, the recovery story is a password list away from
        // being an account takeover.
        let mut limiter = limiter();
        let now = Instant::now();

        for attempt in 1..=3 {
            assert!(limiter.allow("nicolas", now), "attempt {attempt} refused");
        }
        assert!(
            !limiter.allow("nicolas", now),
            "a fourth attempt got through"
        );
    }

    #[test]
    fn red_team_flooding_invented_names_cannot_reset_a_real_account_counter() {
        // THE ATTACK: the limiter is itself a table a stranger writes into, one
        // entry per invented name. If a full table evicted the oldest entry to
        // make room, an attacker would spend their budget on a real account,
        // then flood invented names until their own counter was forgotten.
        let mut limiter = limiter();
        let now = Instant::now();

        for _ in 0..3 {
            assert!(limiter.allow("victim", now));
        }
        assert!(!limiter.allow("victim", now));

        for index in 0..500 {
            let _ = limiter.allow(&format!("invented-{index}"), now);
        }

        assert!(
            !limiter.allow("victim", now),
            "the flood cleared the attacker's own counter"
        );
        assert!(limiter.tracked() <= 8, "the limiter grew past its capacity");
    }

    #[test]
    fn the_budget_comes_back_after_the_window() {
        // Somebody who mistypes their passphrase five times must not be locked
        // out of their own account for good.
        let mut limiter = limiter();
        let start = Instant::now();
        for _ in 0..3 {
            assert!(limiter.allow("nicolas", start));
        }
        assert!(!limiter.allow("nicolas", start));

        let later = start + Duration::from_secs(61);
        assert!(limiter.allow("nicolas", later), "the window never reopened");
    }

    #[test]
    fn expired_windows_are_forgotten_so_the_table_does_not_fill_with_history() {
        let mut limiter = limiter();
        let start = Instant::now();
        for index in 0..8 {
            assert!(limiter.allow(&format!("user-{index}"), start));
        }
        assert_eq!(limiter.tracked(), 8);

        assert!(limiter.allow("someone-new", start + Duration::from_secs(61)));
        assert_eq!(limiter.tracked(), 1);
    }

    #[test]
    fn a_name_that_was_never_asked_about_is_allowed_once_the_table_has_room() {
        let mut limiter = EscrowLimiter::with(3, Duration::from_secs(60), 1);
        let now = Instant::now();
        assert!(limiter.allow("first", now));
        assert!(
            !limiter.allow("second", now),
            "a full table must refuse rather than evict"
        );
    }
}
