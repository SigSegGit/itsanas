//! The listener, and the client that talks to it.
//!
//! Reuses `itsanas-tls` and `itsanas-wire` unchanged, so a coordinator
//! connection is authenticated exactly like a peer connection: anonymous
//! throwaway certificates, identity proved by signing the TLS exporter value.
//!
//! # It is the only thing on a public address
//!
//! Every other component of ITSaNAS talks to machines it chose to dial. This
//! one is dialled by strangers, on a home connection, and will be port-scanned
//! within the hour. So the limits are here rather than in a document:
//!
//! - a cap on concurrent connections, past which new ones are closed rather
//!   than queued, because a queue is just a slower way to run out of memory;
//! - a cap on connections from one address, so one machine cannot hold every
//!   slot and leave the coordinator up and serving nobody else;
//! - a deadline on the whole handshake ([`HANDSHAKE_DEADLINE`]). A read
//!   timeout alone is not one: it bounds a single read, so a caller trickling a
//!   byte a little faster than the timeout held a slot for as long as it liked;
//! - a read timeout once authenticated, so a member who goes quiet gives the
//!   slot back;
//! - a cap on requests per connection, so the expensive part — the handshake —
//!   has to be paid again for more work;
//! - and the framing limits `itsanas-wire` already enforces.
//!
//! Nothing here requires an operator to intervene to stay safe. A coordinator
//! that needed babysitting would not be one.

use std::io;
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use itsanas_crypto::DeviceKeys;
use itsanas_tls::{Authenticated, Identity, accept_within, limits::ConnectionLimits};
use rustls::ServerConfig;

use crate::directory::{Admission, Directory};
use crate::error::{CoordError, Result};
use crate::protocol::{MAX_REQUESTS_PER_CONNECTION, Request, Response};
use crate::service::{CoordService, EscrowLimiter, Expired, PROBE_TRACKED};

mod client;
mod names;
mod probe;

use names::{is_a_question, is_about_a_name};
use probe::answer_check_me;

pub use client::{CoordClient, Deadline};
pub use names::NameBudget;
pub use probe::{MAX_PROBES_IN_FLIGHT, PROBE_TIMEOUT, ProbeBudget};

/// Default port for a coordinator.
pub const DEFAULT_COORD_PORT: u16 = 9898;

/// Longest a connection may sit without sending anything.
pub const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Most connections served at once.
///
/// Each costs a thread and a TLS session. Sized for a coordinator serving a
/// household or a few dozen friends on a small VM, not for a public service:
/// past this, new connections are closed immediately, which is a bad minute for
/// a legitimate caller and a cheap one for the machine.
pub const MAX_CONNECTIONS: usize = 64;

/// Most connections served at once from one IP address.
///
/// A household's machines share one public address, and a machine with two
/// accounts dials twice. Sixteen covers that with room to spare, and leaves
/// three quarters of [`MAX_CONNECTIONS`] to everybody else.
pub const MAX_CONNECTIONS_PER_ADDRESS: usize = 16;

/// How long a caller has, in total, to finish TLS and prove its device key.
pub const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(15);

/// Questions about a name one address may ask per [`NAME_WINDOW`]: every
/// `Lookup` and `GetEscrow`, and every `Register` or `RegisterInvited` that is
/// refused (a refused one says a name is taken or retired; an accepted one is
/// a machine refreshing its own entry, or a name claimed).
///
/// The per-name escrow budget stops a guessing attack on one name; it does
/// nothing against one question each about a million names, which is how a
/// list of accounts is drawn up. A person asks a handful in an hour, so this
/// is far above them; a list of 100 000 names costs over three thousand
/// address-hours. An IPv6 address counts by its /64, the block one
/// subscriber is handed.
pub const NAME_QUESTIONS: u32 = 30;

/// How many times [`NAME_QUESTIONS`] one IPv6 /48 may ask, all its /64s
/// together.
///
/// A /64 is what one subscriber is handed, and a provider or a cloud hands
/// out thousands for nothing: counted by /64 alone, a list of names cost a
/// script one more /64 per thirty names. A /48 is a site's whole allocation,
/// far rarer, so going wide now costs those. Four, so a household or an
/// office whose devices sit in several /64s of one site is not cut short.
pub const NAME_BLOCK_SHARES: u32 = 4;

/// The window [`NAME_QUESTIONS`] is counted over.
pub const NAME_WINDOW: Duration = Duration::from_secs(3600);

/// Addresses counted at once. Full, a new address is refused rather than an
/// old one forgotten, which would be how an attacker cleared their own count.
pub const NAME_TRACKED: usize = 65_536;

/// What an address past [`NAME_QUESTIONS`] is told, whatever the name.
pub const TOO_MANY_NAME_QUESTIONS: &str =
    "too many questions about names from this address; try again in an hour";

/// No answer about a name leaves sooner than this after its question.
///
/// A name that exists costs a read more than one that does not, and a
/// container a little more than its decoy; over enough samples a stranger
/// times the difference. Held to a floor far above that work, the time says
/// nothing about the name. A person recovering an account does not see a
/// fifth of a second.
pub const NAME_ANSWER_FLOOR: Duration = Duration::from_millis(200);

/// Everything one [`CoordClient`] may take, handshake included.
///
/// A client is short-lived: one batch of requests. Without a bound, a path
/// that lets a byte through now and then -- a censor throttling a foreign
/// address to nothing, or a hostile middlebox -- held a daemon's round, or a
/// recovery typed by a person, for as long as it liked, each read arriving
/// just inside [`IO_TIMEOUT`]. Two minutes is a hundred round trips across a
/// continent and a slow firewall.
pub const COORD_SESSION_BUDGET: Duration = Duration::from_secs(120);

/// How often a serving coordinator erases what has expired, after doing so as
/// it starts.
///
/// A day because nothing it erases is urgent: a row a few hours past its date
/// is read by nobody, and each pass holds the directory's one write lock.
pub const EXPIRE_EVERY: Duration = Duration::from_secs(24 * 3600);

/// Seconds since the Unix epoch.
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A listening coordinator.
#[derive(Debug)]
pub struct CoordServer {
    listener: itsanas_tls::reach::Listeners,
    config: Arc<ServerConfig>,
    handshake_deadline: Duration,
    expire_every: Duration,
    name_questions: u32,
    outbox: Option<crate::mail::Outbox>,
}

impl CoordServer {
    /// Bind to `address`.
    ///
    /// Through [`itsanas_tls::reach::listen_on`], so that a coordinator asked
    /// for every interface answers IPv6 callers too. It is the one machine in a
    /// fleet that everybody must be able to reach from anywhere, so it is the
    /// one where being IPv4-only costs the most.
    pub fn bind(address: impl ToSocketAddrs) -> Result<Self> {
        let resolved: Vec<SocketAddr> = address
            .to_socket_addrs()
            .map_err(CoordError::from)?
            .collect();
        let listener = itsanas_tls::reach::listen_on(&resolved).map_err(CoordError::from)?;
        listener.set_nonblocking(false).map_err(CoordError::from)?;
        // One anonymous certificate for the life of the process. It
        // authenticates nobody — identity is proved a layer up, by signing the
        // TLS exporter — so regenerating it per connection would cost key
        // generation for no gain.
        let identity = Identity::generate()
            .map_err(|error| CoordError::Transport(format!("no TLS identity: {error}")))?;
        let config = identity
            .server_config()
            .map_err(|error| CoordError::Transport(format!("no TLS config: {error}")))?;

        Ok(Self {
            listener,
            config,
            handshake_deadline: HANDSHAKE_DEADLINE,
            expire_every: EXPIRE_EVERY,
            name_questions: NAME_QUESTIONS,
            outbox: None,
        })
    }

    /// Give callers `limit` instead of [`HANDSHAKE_DEADLINE`] to authenticate.
    ///
    /// For tests, which cannot wait fifteen seconds to watch a deadline pass.
    #[must_use]
    pub const fn with_handshake_deadline(mut self, limit: Duration) -> Self {
        self.handshake_deadline = limit;
        self
    }

    /// Erase what has expired every `period` instead of every [`EXPIRE_EVERY`].
    ///
    /// For tests, which cannot wait a day to watch a second pass.
    #[must_use]
    pub const fn with_expiry_every(mut self, period: Duration) -> Self {
        self.expire_every = period;
        self
    }

    /// Questions about names one address may ask per [`NAME_WINDOW`], in
    /// place of [`NAME_QUESTIONS`]; for tests.
    #[must_use]
    pub const fn with_name_questions(mut self, allowed: u32) -> Self {
        self.name_questions = allowed;
        self
    }

    /// Mail through `outbox` ([`crate::mail`]): addresses can be bound, and a
    /// verified one asks a mailed code of every recovery. Without it the
    /// coordinator refuses to bind one.
    #[must_use]
    pub fn with_outbox(mut self, outbox: crate::mail::Outbox) -> Self {
        self.outbox = Some(outbox);
        self
    }

    /// The address actually bound, after a port of zero.
    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.listener.local_addr().map_err(CoordError::from)
    }

    /// Serve until `shutdown` is set.
    ///
    /// One thread per connection, bounded by [`MAX_CONNECTIONS`]. A connection
    /// that fails is logged by the caller's own means and never brings the loop
    /// down: a coordinator that exited because one stranger sent nonsense would
    /// be trivial to switch off.
    pub fn serve_until(
        &self,
        directory: &Directory,
        device: &DeviceKeys,
        shutdown: &AtomicBool,
        on_event: impl FnMut(&str) + Send,
    ) -> Result<()> {
        self.serve_admitting(directory, device, Admission::Open, shutdown, on_event)
    }

    /// Serve until `shutdown` is set, under a stated admission policy.
    ///
    /// `Admission::ByInvitation` is what makes the rest of this project's
    /// defences describe a real adversary: audits, the reliability pause and
    /// the probation ladder are all aimed at a hostile *host*, and a hostile
    /// host is somebody who joined.
    ///
    /// Erases what has expired ([`CoordService::expire`]) before it accepts
    /// anything, and every [`EXPIRE_EVERY`] after that. In this loop rather than
    /// on a thread of its own: it is a few short write transactions once a day,
    /// and here it needs no stopping and can use `on_event`.
    pub fn serve_admitting(
        &self,
        directory: &Directory,
        device: &DeviceKeys,
        admission: Admission,
        shutdown: &AtomicBool,
        mut on_event: impl FnMut(&str) + Send,
    ) -> Result<()> {
        self.listener
            .set_nonblocking(true)
            .map_err(CoordError::from)?;

        // No per-device cap here: members make one short connection per
        // request batch, and the per-address cap already bounds a flood.
        let limits =
            ConnectionLimits::new(MAX_CONNECTIONS, MAX_CONNECTIONS_PER_ADDRESS, usize::MAX);
        let service = CoordService::admitting(directory, admission);
        let service = match &self.outbox {
            Some(outbox) => service.with_outbox(outbox),
            None => service,
        };

        // One limiter for the whole server, not one per connection. A
        // per-connection budget is no budget at all: an attacker reconnects and
        // gets a fresh one, which costs them a handshake and buys them
        // everything. Shared state behind a mutex is the price of the rate
        // limit meaning anything, and only an escrow fetch locks it, for its
        // count in memory and not across the disk write after it.
        let limiter = Mutex::new(EscrowLimiter::new());

        // A second budget, for the one request that makes this process act on
        // the internet rather than answer about it. Shared for the same reason
        // as the first: a per-connection budget is no budget, because
        // reconnecting is cheap.
        let probes = Mutex::new(ProbeBudget::new(PROBE_TRACKED));
        let in_flight = AtomicUsize::new(0);

        // A third, per address rather than per name: see [`NAME_QUESTIONS`].
        let names = Mutex::new(NameBudget::new(self.name_questions));

        expire_once(&service, &mut on_event);
        let mut last_expired = Instant::now();

        std::thread::scope(|scope| {
            while !shutdown.load(Ordering::Relaxed) {
                if last_expired.elapsed() >= self.expire_every {
                    expire_once(&service, &mut on_event);
                    last_expired = Instant::now();
                }

                match self.listener.accept() {
                    Ok((stream, from)) => {
                        // Windows hands back a socket that inherited the
                        // listener's non-blocking mode, and the TLS handshake
                        // then fails with "connection aborted by your host
                        // software" — an error that reads like a firewall and
                        // is not. Put it back before anything touches it.
                        if stream.set_nonblocking(false).is_err() {
                            continue;
                        }

                        let Some(slot) = limits.admit(from.ip()) else {
                            // Closing beats queueing: a queue is a slower way
                            // to run out of memory, and the caller finds out
                            // now rather than after a timeout.
                            drop(stream);
                            on_event("at the connection limit; refused one");
                            continue;
                        };

                        let config = Arc::clone(&self.config);
                        let deadline = self.handshake_deadline;
                        let service = &service;
                        let limiter = &limiter;
                        let probes = &probes;
                        let names = &names;
                        let in_flight = &in_flight;
                        scope.spawn(move || {
                            let outcome = serve_one(
                                stream,
                                from.ip(),
                                &config,
                                deadline,
                                device,
                                service,
                                Budgets {
                                    escrow: limiter,
                                    probes,
                                    names,
                                },
                                in_flight,
                            );
                            // Given back however the connection ended.
                            drop(slot);
                            let _ = outcome;
                        });
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(error) => {
                        on_event(&format!("accept failed: {error}"));
                        std::thread::sleep(Duration::from_millis(200));
                    }
                }
            }
        });

        Ok(())
    }
}

/// One pass of [`CoordService::expire`], logged when it found something.
///
/// A failure is logged and the next pass tries again: a coordinator that stopped
/// answering members because it could not tidy up would be a worse one. The log
/// gets counts and nothing else, so what was erased is not kept there instead.
fn expire_once(service: &CoordService<'_>, on_event: &mut impl FnMut(&str)) {
    match service.expire(now_unix()) {
        Ok(expired) if expired == Expired::default() => {}
        Ok(expired) => on_event(&format!(
            "erased {} silent address(es), {} retired name(s), {} escrow count(s)",
            expired.addresses, expired.names, expired.escrow_counts
        )),
        Err(error) => on_event(&format!("could not erase what has expired: {error}")),
    }
}

/// One connection, from handshake to close.
#[allow(clippy::too_many_arguments)]
fn serve_one(
    stream: TcpStream,
    from: IpAddr,
    config: &Arc<ServerConfig>,
    deadline: Duration,
    device: &DeviceKeys,
    service: &CoordService<'_>,
    budgets: Budgets<'_>,
    in_flight: &AtomicUsize,
) -> Result<()> {
    let Budgets {
        escrow: limiter,
        probes,
        names,
    } = budgets;
    let Authenticated {
        peer: caller,
        mut connection,
    } = accept_within(config, device, stream, deadline, IO_TIMEOUT)
        .map_err(|error| CoordError::Transport(format!("handshake failed: {error}")))?;

    for served in 0..=MAX_REQUESTS_PER_CONNECTION {
        // The request is read *before* the budget is checked, and that ordering
        // is load-bearing rather than tidy. Closing a socket that still has
        // unread incoming data makes Windows send an RST, which discards
        // whatever was in the send buffer — so a refusal written before reading
        // never arrives, and the caller sees "connection aborted by your host
        // software" instead. Which reads like a firewall, and is not.
        let request: Request = match connection.receive() {
            // A clean close between messages is how a well-behaved client
            // leaves, and a malformed frame is how a scanner does. Neither is
            // worth a log line on a public address.
            Ok(Some(request)) => request,
            Ok(None) | Err(_) => return Ok(()),
        };

        // What a coordinator older than `SignedPeers` does with it -- it cannot
        // decode it, so it hangs up -- done on purpose, by one that wants its
        // clients on the unsigned list. Only in tests of those clients.
        #[cfg(feature = "hostile")]
        if matches!(
            request,
            Request::SignedPeers { .. } | Request::ClaimedPeers { .. }
        ) && service.plays_old()
        {
            return Ok(());
        }

        if served == MAX_REQUESTS_PER_CONNECTION {
            let _ = connection.send(&Response::Refused(format!(
                concat!("this connection has made its {} requests; ", "open another"),
                MAX_REQUESTS_PER_CONNECTION
            )));
            return Ok(());
        }

        let asked_at = Instant::now();
        let about_a_name = is_about_a_name(&request);
        let spend = |names: &Mutex<NameBudget>| {
            names
                .lock()
                .is_ok_and(|mut names| names.allow(from, asked_at))
        };
        let mut response = if matches!(request, Request::CheckMe) {
            answer_check_me(service, caller, device, probes, in_flight)
        } else if is_a_question(&request) && !spend(names) {
            Response::Refused(TOO_MANY_NAME_QUESTIONS.to_owned())
        } else {
            // Locked inside, by an escrow fetch alone, and for its count in
            // memory alone: every other request runs beside the others.
            service
                .handle(&request, caller, now_unix(), limiter, Instant::now())
                .unwrap_or_else(|error| Response::Refused(error.to_string()))
        };

        // A registration is counted only when refused: the refusal is what
        // tells a name is taken or retired, and a household's machines
        // refreshing their own entry behind one address must never run out.
        if about_a_name
            && !is_a_question(&request)
            && matches!(response, Response::Refused(_))
            && !spend(names)
        {
            response = Response::Refused(TOO_MANY_NAME_QUESTIONS.to_owned());
        }

        if about_a_name {
            // After every lock is given back: a sleeping thread holds a
            // connection slot, never a budget.
            std::thread::sleep(NAME_ANSWER_FLOOR.saturating_sub(asked_at.elapsed()));
        }

        if connection.send(&response).is_err() {
            return Ok(());
        }
    }

    Ok(())
}

/// The three shared budgets [`serve_one`] charges.
#[derive(Clone, Copy)]
struct Budgets<'a> {
    escrow: &'a Mutex<EscrowLimiter>,
    probes: &'a Mutex<ProbeBudget>,
    names: &'a Mutex<NameBudget>,
}
