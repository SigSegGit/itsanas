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
use crate::directory::{Admission, Directory};
use crate::error::Result;
use crate::protocol::{
    COORD_VERSION, EnrolledDevice, MAX_PEERS_RETURNED, MAX_WIRE_USERNAME, Request, Response,
};

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

/// Whether a string is plausibly `host:port` with a name rather than a literal.
///
/// The announce path already refuses everything else; this is the second lock
/// on the door that leads to a resolver, because the first one is in another
/// crate and could be relaxed by somebody who has not read this one.
fn looks_like_host_port(address: &str) -> bool {
    let Some((host, port)) = address.rsplit_once(':') else {
        return false;
    };
    !host.is_empty()
        && !host.contains([':', '[', ']', '/', ' ', '@'])
        && port.parse::<u16>().is_ok_and(|port| port != 0)
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

            Request::Peers { user } => Ok(Response::Peers(self.peers_of(*user, now_unix)?)),

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

            Request::Devices { user } => self.devices_of(*user, caller, now_unix),

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

        // Most recently heard from first, never-heard-from last, then by id so
        // two askers see one order.
        out.sort_by(|a, b| {
            a.silent_for
                .unwrap_or(u64::MAX)
                .cmp(&b.silent_for.unwrap_or(u64::MAX))
                .then(a.device.cmp(&b.device))
        });
        out.truncate(MAX_PEERS_RETURNED);
        Ok(Response::Devices(out))
    }

    /// Live, reachable devices for `user`, most recently confirmed first.
    ///
    /// Ordered and expired by **this coordinator's** record of when it last
    /// heard from each device, never by the `at_unix` inside the presence. That
    /// number is the announcing device's opinion of the time: a Raspberry Pi
    /// with no real-time clock reports 1970 and would sort last forever, and
    /// anybody who wanted to sort first could simply say so. The same mistake
    /// was made and removed in `itsanas-discover`; it does not get to come back
    /// here.
    fn peers_of(&self, user: UserId, now: u64) -> Result<Vec<Presence>> {
        let mut out = Vec::new();
        for claim in self.directory.live_claims_of(user)? {
            let device = claim.claim.device;
            let Some(presence) = self.directory.presence_of(device)? else {
                continue;
            };
            let seen = self.directory.last_seen(device)?.unwrap_or(0);
            if now.saturating_sub(seen) > PRESENCE_TTL {
                continue;
            }
            out.push((seen, presence.presence));
        }

        // Then by device id, so two clients asking at the same moment get the
        // same list rather than a shuffling one.
        out.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.device.cmp(&b.1.device)));
        out.truncate(MAX_PEERS_RETURNED);
        Ok(out.into_iter().map(|(_, presence)| presence).collect())
    }

    /// Which address, if any, may be probed on this caller's behalf.
    ///
    /// Four refusals, and each of them closes a way of turning a coordinator
    /// into somebody else's tool:
    ///
    /// * **Not an enrolled device.** A probe is work done for a member.
    /// * **Never announced.** There is nothing to probe, and the alternative --
    ///   letting the caller name an address -- is the scanner this design
    ///   exists to avoid.
    /// * **A private address.** Probing `192.168.1.1:80` or `10.0.0.5:22` from
    ///   the coordinator is a scan of the *coordinator's* own network, which is
    ///   the one network a member has no business reaching. This is the rule
    ///   that would matter if the coordinator ever ran somewhere with company
    ///   on its LAN.
    /// * **An address that is not `host:port`.** Nothing that reaches the
    ///   resolver is shaped by a stranger beyond what `Announce` already
    ///   accepted.
    ///
    /// What remains is: a member, who published a public address, may have that
    /// address dialled once an hour. The residual is written down rather than
    /// argued away -- an enrolled member can make the coordinator open one
    /// connection an hour to a public address of their choosing, which is a
    /// slow and attributable port scanner, and the answer to that is that they
    /// are enrolled and can be un-enrolled.
    pub fn probe_target(&self, caller: DeviceId, now_unix: u64) -> Result<Probe> {
        let Some(claim) = self.directory.claim_for(caller)? else {
            return Ok(Probe::Refuse(
                "this device is not enrolled under any account".to_owned(),
            ));
        };
        if claim.claim.revoked || claim.verify(now_unix).is_err() {
            return Ok(Probe::Refuse(
                "this device's enrolment is revoked or invalid".to_owned(),
            ));
        }

        let Some(presence) = self.directory.presence_of(caller)? else {
            return Ok(Probe::Refuse(
                "this device has not announced an address, so there is nothing to try".to_owned(),
            ));
        };

        let address = presence.presence.address;
        if address.parse::<std::net::SocketAddr>().is_err() && !looks_like_host_port(&address) {
            return Ok(Probe::Refuse(format!(
                "{address:?} is not an address this coordinator will dial"
            )));
        }
        if itsanas_tls::reach::is_private_address(&address) {
            return Ok(Probe::Refuse(format!(
                concat!(
                    "{} is private, so it means something only on the network it ",
                    "was announced from; a coordinator elsewhere cannot tell you ",
                    "anything about it"
                ),
                address
            )));
        }

        Ok(Probe::Address(address))
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
    use crate::claim::{NodeClaim, Presence};

    /// A fixed wall clock, so nothing in these tests depends on today's date.
    const NOW: u64 = 1_700_000_000;

    fn directory() -> Directory {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("directory.redb");
        // The directory owns its file; the handle keeps the directory alive for
        // the length of the test by leaking it, which is what a test process
        // exiting does anyway.
        std::mem::forget(dir);
        Directory::open(path).expect("directory")
    }

    fn user(seed: u8) -> itsanas_crypto::UserKeys {
        itsanas_crypto::UserKeys::derive(&itsanas_crypto::MasterSecret::from_bytes([seed; 32]))
    }

    fn device(seed: u8) -> itsanas_crypto::DeviceKeys {
        itsanas_crypto::DeviceKeys::from_seed(&itsanas_crypto::SecretBytes::new([seed; 32]))
    }

    fn enrol(
        directory: &Directory,
        owner: &itsanas_crypto::UserKeys,
        device: &itsanas_crypto::DeviceKeys,
    ) {
        directory
            .register(
                &crate::directory::Registration {
                    username: format!("member{}", device.device_id().short()),
                    user: owner.public(),
                    issued_unix: NOW,
                }
                .sign(owner),
                NOW,
            )
            .expect("register");
        directory
            .claim(
                &NodeClaim {
                    owner: owner.user_id(),
                    device: device.device_id(),
                    pledged_bytes: 1 << 30,
                    issued_unix: NOW,
                    revoked: false,
                }
                .sign(owner),
                NOW,
            )
            .expect("claim");
    }

    fn announce_address(
        directory: &Directory,
        _owner: &itsanas_crypto::UserKeys,
        device: &itsanas_crypto::DeviceKeys,
        address: &str,
    ) {
        directory
            .announce(
                &Presence {
                    device: device.device_id(),
                    address: address.to_owned(),
                    at_unix: NOW,
                }
                .sign(device),
                NOW,
            )
            .expect("announce");
    }

    /// What one address lookup costs when the coordinator holds a real fleet.
    ///
    /// `#[ignore]`d because it builds a directory of three thousand devices and
    /// is a measurement rather than an assertion -- run it with
    /// `cargo test -p itsanas-coord -- --ignored --nocapture` and read the
    /// numbers.
    ///
    /// **Why it exists.** `peers_of` and `devices_of` walk `live_claims()`,
    /// which deserialises **every claim in the directory** and then filters by
    /// account. The cost of one member asking "where are my machines" is
    /// therefore O(devices in the whole network), not O(devices of that
    /// account) -- so the work grows with the square of the fleet. At the scale
    /// Nicolas asked about, a thousand members and three thousand machines,
    /// that is the wall, and halving the number of *connections* per round does
    /// not touch it. Found by the Rodin audit of 2026-09-18.
    ///
    /// This test does not assert a time. It prints one, so that the decision to
    /// index by account is made against a number instead of an intuition.
    #[test]
    #[ignore = "a measurement over three thousand devices, not an assertion"]
    fn measure_what_one_lookup_costs_across_a_fleet() {
        let directory = directory();
        let mine = user(1);
        let my_laptop = device(1);
        enrol(&directory, &mine, &my_laptop);

        let service = CoordService::new(&directory);
        let mut sizes = Vec::new();

        for fleet in [0usize, 500, 1500, 3000] {
            while sizes.iter().sum::<usize>() < fleet {
                let n = sizes.len();
                // A separate account per device: the shape that matters is the
                // number of claims in the table, whoever owns them.
                let owner = user(u8::try_from(n % 200).unwrap_or(0).wrapping_add(20));
                let machine = itsanas_crypto::DeviceKeys::from_seed(
                    &itsanas_crypto::SecretBytes::new(seed_for(n)),
                );
                let _ = directory.register(
                    &crate::directory::Registration {
                        username: format!("member{n}"),
                        user: owner.public(),
                        issued_unix: NOW,
                    }
                    .sign(&owner),
                    NOW,
                );
                let _ = directory.claim(
                    &NodeClaim {
                        owner: owner.user_id(),
                        device: machine.device_id(),
                        pledged_bytes: 1 << 30,
                        issued_unix: NOW,
                        revoked: false,
                    }
                    .sign(&owner),
                    NOW,
                );
                sizes.push(1);
            }

            let started = Instant::now();
            let rounds = 20;
            for _ in 0..rounds {
                let _ = service
                    .peers_of(mine.user_id(), NOW)
                    .expect("the lookup answers");
            }
            let each = started.elapsed() / rounds;
            println!(
                "  {:>5} devices in the directory -> {each:?} per lookup, \
                 {:.0} lookups/s",
                sizes.iter().sum::<usize>(),
                1.0 / each.as_secs_f64()
            );
        }
        println!("  a fleet of 3000 machines on a 300 s round asks about 10 lookups/s");
    }

    /// Thirty-two bits of variation in a device seed, which is enough for a
    /// measurement and keeps the fixture deterministic.
    fn seed_for(n: usize) -> [u8; 32] {
        let mut seed = [0u8; 32];
        seed[..8].copy_from_slice(&(n as u64).to_le_bytes());
        seed[31] = 0xA5;
        seed
    }

    /// THE ATTACK: a coordinator that dials what a caller names is a port
    /// scanner with somebody else's address on it. This one dials only what the
    /// caller *announced*, which is signed by the caller's own device key --
    /// and even then, only if it is an address the wider internet could reach.
    /// Aiming it at a private range would scan the coordinator's own network,
    /// which is the one network a member has no business reaching.
    #[test]
    fn red_team_a_probe_cannot_be_aimed_at_the_coordinators_own_network() {
        let directory = directory();
        let owner = user(7);
        let laptop = device(7);
        enrol(&directory, &owner, &laptop);

        let service = CoordService::new(&directory);

        for address in [
            "192.168.1.1:80",
            "10.0.0.5:22",
            "127.0.0.1:9797",
            "[::1]:9797",
            "169.254.169.254:80",
            "172.16.0.1:443",
        ] {
            announce_address(&directory, &owner, &laptop, address);
            let decided = service.probe_target(laptop.device_id(), NOW).unwrap();
            assert!(
                matches!(decided, Probe::Refuse(_)),
                "{address} would have been dialled by the coordinator: a member \
                 can point that at the machine it runs on"
            );
        }

        // And the addresses the feature exists for.
        for address in ["ngas.fr:9801", "82.67.35.234:9801", "[2001:db8::1]:9797"] {
            announce_address(&directory, &owner, &laptop, address);
            assert_eq!(
                service.probe_target(laptop.device_id(), NOW).unwrap(),
                Probe::Address(address.to_owned()),
                "{address} is the case this exists for"
            );
        }
    }

    /// A probe is work the coordinator does for a member. A stranger who
    /// completed a handshake is not a member: device keys are free keypairs,
    /// and answering them would let anybody with a socket spend this
    /// coordinator's outbound connections.
    #[test]
    fn red_team_a_device_with_no_account_cannot_make_the_coordinator_dial_anything() {
        let directory = directory();
        let service = CoordService::new(&directory);
        let stranger = device(99);

        let Probe::Refuse(why) = service.probe_target(stranger.device_id(), NOW).unwrap() else {
            panic!("an unenrolled device got the coordinator to dial on its behalf");
        };
        // The reason, not merely the refusal: an unenrolled device also has no
        // presence, so "there is nothing to probe" refuses it by accident. That
        // accident disappears the day anything else writes a presence, and the
        // rule this test is about would be gone with no test failing.
        assert!(
            why.contains("not enrolled"),
            concat!(
                "the refusal must be about enrolment rather than about a ",
                "missing address; it said {:?}"
            ),
            why
        );
    }

    /// A withdrawn machine keeps its key: withdrawal is the account saying it
    /// no longer speaks for them. It must stop buying the coordinator's work
    /// too, or a stolen laptop is a probe budget for as long as it runs.
    #[test]
    fn a_withdrawn_device_stops_being_probed_for() {
        let directory = directory();
        let owner = user(8);
        let laptop = device(8);
        enrol(&directory, &owner, &laptop);
        announce_address(&directory, &owner, &laptop, "ngas.fr:9801");

        let service = CoordService::new(&directory);
        assert!(matches!(
            service.probe_target(laptop.device_id(), NOW).unwrap(),
            Probe::Address(_)
        ));

        directory
            .claim(
                &NodeClaim {
                    owner: owner.user_id(),
                    device: laptop.device_id(),
                    pledged_bytes: 0,
                    issued_unix: NOW + 1,
                    revoked: true,
                }
                .sign(&owner),
                NOW + 1,
            )
            .expect("withdraw");

        assert!(
            matches!(
                service.probe_target(laptop.device_id(), NOW + 2).unwrap(),
                Probe::Refuse(_)
            ),
            "a withdrawn device still spends this coordinator's connections"
        );
    }

    /// A device that has never announced has nothing to probe, and the
    /// alternative -- letting it name an address -- is the scanner.
    #[test]
    fn a_device_that_never_announced_is_told_so_rather_than_probed() {
        let directory = directory();
        let owner = user(9);
        let laptop = device(9);
        enrol(&directory, &owner, &laptop);

        let service = CoordService::new(&directory);
        let Probe::Refuse(why) = service.probe_target(laptop.device_id(), NOW).unwrap() else {
            panic!("a device with no address was given a probe");
        };
        assert!(
            why.contains("announced"),
            "the refusal must say what is missing; it said {why:?}"
        );
    }

    /// THE BUDGET: one probe per device per hour. Without it a member's daemon
    /// -- or a member's script -- turns every round into an outbound connection
    /// the coordinator pays for, which is the load this whole design exists to
    /// keep off it.
    #[test]
    fn red_team_asking_to_be_probed_again_and_again_buys_one_probe_an_hour() {
        let mut limiter = EscrowLimiter::with(1, PROBE_WINDOW, PROBE_TRACKED);
        let device = device(11).device_id().to_hex();
        let start = Instant::now();

        assert!(limiter.allow(&device, start), "the first ask is answered");
        for attempt in 0..50 {
            assert!(
                !limiter.allow(&device, start + Duration::from_secs(attempt)),
                "asking again within the hour bought another outbound connection"
            );
        }
        assert!(
            limiter.allow(&device, start + PROBE_WINDOW + Duration::from_secs(1)),
            "the window must reopen, or a machine that really did move can never \
             find out that it is reachable again"
        );
    }

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
