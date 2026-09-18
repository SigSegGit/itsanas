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
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use itsanas_crypto::{DeviceId, DeviceKeys};
use itsanas_tls::{Authenticated, Identity, accept_within, connect, limits::ConnectionLimits};
use itsanas_wire::Connection;
use rustls::{ClientConfig, ServerConfig};

use crate::directory::{Admission, Directory};
use crate::error::{CoordError, Result};
use crate::protocol::{COORD_VERSION, MAX_REQUESTS_PER_CONNECTION, Request, Response};
use crate::service::{CoordService, EscrowLimiter, PROBE_TRACKED, PROBE_WINDOW, Probe};

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

/// Seconds since the Unix epoch.
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A listening coordinator.
#[derive(Debug)]
pub struct CoordServer {
    listener: TcpListener,
    config: Arc<ServerConfig>,
    handshake_deadline: Duration,
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

        // One limiter for the whole server, not one per connection. A
        // per-connection budget is no budget at all: an attacker reconnects and
        // gets a fresh one, which costs them a handshake and buys them
        // everything. Shared state behind a mutex is the price of the rate
        // limit meaning anything, and it is only touched on escrow fetches.
        let limiter = Mutex::new(EscrowLimiter::new());

        // A second budget, for the one request that makes this process act on
        // the internet rather than answer about it. Shared for the same reason
        // as the first: a per-connection budget is no budget, because
        // reconnecting is cheap.
        let probes = Mutex::new(EscrowLimiter::with(1, PROBE_WINDOW, PROBE_TRACKED));
        let in_flight = AtomicUsize::new(0);

        std::thread::scope(|scope| {
            while !shutdown.load(Ordering::Relaxed) {
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
                        let in_flight = &in_flight;
                        scope.spawn(move || {
                            let outcome = serve_one(
                                stream, &config, deadline, device, service, limiter, probes,
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

/// Decide whether to probe this caller, and probe them.
///
/// The decision is `CoordService`'s and is tested without a socket; everything
/// here is the acting on it, and every line between the two is a bound:
/// one probe per device per hour, at most [`MAX_PROBES_IN_FLIGHT`] at once, and
/// a three-second timeout. A member whose router is dead costs this coordinator
/// three seconds of one thread, once an hour.
fn answer_check_me(
    service: &CoordService<'_>,
    caller: DeviceId,
    device: &DeviceKeys,
    probes: &Mutex<EscrowLimiter>,
    in_flight: &AtomicUsize,
) -> Response {
    let target = match service.probe_target(caller, now_unix()) {
        Ok(Probe::Address(address)) => address,
        Ok(Probe::Refuse(why)) => {
            return Response::Reachable {
                reachable: false,
                detail: why,
            };
        }
        Err(error) => return Response::Refused(error.to_string()),
    };

    {
        let Ok(mut probes) = probes.lock() else {
            return Response::Refused("this coordinator is not answering probes".to_owned());
        };
        // Keyed by device **and address**: the budget exists to stop a daemon
        // asking every round, and a *new* address is precisely the case where
        // the answer can have changed. Found by using it -- repointing a node
        // during a fleet migration, the second question was refused as a
        // repeat of the first, which is the moment somebody most needs it.
        if !probes.allow(&format!("{}@{target}", caller.to_hex()), Instant::now()) {
            return Response::Reachable {
                reachable: false,
                detail:
                    "this device has already been probed within the hour; the last answer stands"
                        .to_owned(),
            };
        }
    }

    // Taken before the dial and given back however it ends, including on an
    // early return: a counter that leaks on a failure path is a limit that
    // shrinks to zero, and every probe of an unreachable member is a failure
    // path.
    let Some(_slot) = InFlight::take(in_flight) else {
        return Response::Reachable {
            reachable: false,
            detail: "this coordinator is already probing as many members as it will at once; ask again shortly".to_owned(),
        };
    };

    let targets = match resolve_probe_target(&target) {
        Ok(targets) => targets,
        Err(why) => {
            return Response::Reachable {
                reachable: false,
                detail: why,
            };
        }
    };

    let (reachable, detail) = probe(&target, &targets, caller, device);
    Response::Reachable { reachable, detail }
}

/// One of the concurrent probe slots, given back when it is dropped.
struct InFlight<'a>(&'a AtomicUsize);

impl<'a> InFlight<'a> {
    fn take(count: &'a AtomicUsize) -> Option<Self> {
        let taken = count
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                (current < MAX_PROBES_IN_FLIGHT).then_some(current + 1)
            })
            .is_ok();
        // `then`, not `then_some`: `then_some` takes its argument by value, so
        // the guard would be *constructed* even when no slot was taken -- and
        // dropped an instant later, giving back a slot that was never held.
        // The counter underflowed to `usize::MAX` and the next probe panicked
        // on the increment. Caught by
        // `a_probe_slot_is_given_back_however_the_probe_ends`, which was
        // written for the opposite leak.
        taken.then(|| Self(count))
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// How many probes may be in flight at once, across the whole coordinator.
///
/// A probe holds an outbound socket for as long as a handshake takes, and the
/// coordinator's job is to answer members, not to wait on their routers. Four
/// is enough that a fleet never queues behind one unreachable member and small
/// enough that the count cannot become the load.
pub const MAX_PROBES_IN_FLIGHT: usize = 4;

/// How long a probe may take before it counts as unreachable.
///
/// Deliberately shorter than a member's own dial timeout: this is a
/// diagnostic, and "your router did not answer within three seconds" is the
/// same answer as "your router did not answer", for anybody reading it.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Resolve an address to probe, refusing anything that is not out on the
/// internet.
///
/// Split from the dialling for one reason: `probe_target` guards the announced
/// **string**, and a name is never private to that check -- correctly, because
/// what a name resolves to is the resolver's business and not a dialler's. Here
/// it becomes this process's business. `nas.example.org` pointing at
/// `192.168.1.10` walked straight through the string guard and had this
/// coordinator dial its own network, which is the one thing that guard exists
/// to prevent. Found by the Rodin audit of 2026-09-18, in a guard written the
/// same afternoon, and demonstrated by the sabotage run reaching the real
/// daemon on the machine the test ran on.
fn resolve_probe_target(address: &str) -> std::result::Result<Vec<SocketAddr>, String> {
    let targets: Vec<SocketAddr> = match address.to_socket_addrs() {
        Ok(found) => found.collect(),
        Err(error) => return Err(format!("{address} does not resolve: {error}")),
    };
    if targets.is_empty() {
        return Err(format!("{address} resolves to no address at all"));
    }

    if let Some(private) = targets
        .iter()
        .find(|target| itsanas_tls::reach::is_private_ip(target.ip()))
    {
        return Err(format!(
            "{address} resolves to {private}, a private address: dialling it would reach a network of this coordinator's own rather than tell you anything about yours"
        ));
    }

    Ok(targets)
}

/// Dial `targets`, expecting `expect` to answer, and say what happened.
///
/// A **device-authenticated handshake**, not a bare connection. An open port
/// proves that something is listening; completing this proves the address leads
/// to that member's machine, which is the question they actually asked. The
/// connection is dropped the moment it is proved: the coordinator has nothing
/// to say over the peer protocol and does not speak it.
///
/// Takes resolved addresses rather than a string, so that the refusal to dial
/// anything private happens once, in `resolve_probe_target`, on the only path
/// that reaches here in production.
fn probe(
    address: &str,
    targets: &[SocketAddr],
    expect: DeviceId,
    device: &DeviceKeys,
) -> (bool, String) {
    let stream = match itsanas_tls::reach::connect_within(targets, PROBE_TIMEOUT) {
        Ok(stream) => stream,
        Err(error) => {
            return (
                false,
                format!(
                    "nothing answered at {address}: {error}. A forward or a firewall rule is missing, or it points somewhere else"
                ),
            );
        }
    };

    let identity = match Identity::generate() {
        Ok(identity) => identity,
        Err(error) => {
            return (
                false,
                format!("this coordinator has no TLS identity: {error}"),
            );
        }
    };
    let config = match identity.client_config() {
        Ok(config) => config,
        Err(error) => {
            return (
                false,
                format!("this coordinator has no TLS config: {error}"),
            );
        }
    };

    match connect(&config, device, stream, Some(expect)) {
        Ok(_) => (true, format!("{address} answered, and it is this device")),
        Err(error) => (
            false,
            format!(
                "{address} answered, but not as this device: {error}. The address reaches some other machine -- a forward pointing at the wrong host, or an address that is not yours"
            ),
        ),
    }
}

/// One connection, from handshake to close.
#[allow(clippy::too_many_arguments)]
fn serve_one(
    stream: TcpStream,
    config: &Arc<ServerConfig>,
    deadline: Duration,
    device: &DeviceKeys,
    service: &CoordService<'_>,
    limiter: &Mutex<EscrowLimiter>,
    probes: &Mutex<EscrowLimiter>,
    in_flight: &AtomicUsize,
) -> Result<()> {
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

        if served == MAX_REQUESTS_PER_CONNECTION {
            let _ = connection.send(&Response::Refused(format!(
                concat!("this connection has made its {} requests; ", "open another"),
                MAX_REQUESTS_PER_CONNECTION
            )));
            return Ok(());
        }

        let response = if matches!(request, Request::CheckMe) {
            answer_check_me(service, caller, device, probes, in_flight)
        } else {
            // Held only across one request, and only escrow fetches touch it.
            // A poisoned lock means another thread panicked mid-request; the
            // safe answer is to refuse rather than to ignore the limit.
            let Ok(mut limiter) = limiter.lock() else {
                return Ok(());
            };
            service
                .handle(&request, caller, now_unix(), &mut limiter, Instant::now())
                .unwrap_or_else(|error| Response::Refused(error.to_string()))
        };

        if connection.send(&response).is_err() {
            return Ok(());
        }
    }

    Ok(())
}

/// A client for one coordinator.
#[derive(Debug)]
pub struct CoordClient {
    connection: Connection<itsanas_tls::session::ClientStream<TcpStream>>,
    /// The local end of the socket, kept because the TLS wrapper consumes the
    /// `TcpStream` and a caller cannot ask it afterwards.
    ///
    /// It answers "which of this machine's addresses reaches the coordinator",
    /// which is the only sensible thing to announce when a node is configured
    /// to listen on every interface. See `itsanas-cli`'s `reachable_address`.
    local: SocketAddr,
}

impl CoordClient {
    /// `expect` pins which device must answer where one is known. A coordinator
    /// address is configuration, and configuration is not a promise about who
    /// lives there.
    ///
    ///
    /// Takes no user id, deliberately, unlike `PeerClient::connect`. A
    /// coordinator connection is authenticated by *device*, and every request
    /// that concerns an account carries its own signature — so an owner
    /// parameter here would be decoration, and a parameter that decorates is
    /// one a reader assumes is checked.
    pub fn connect(
        address: impl ToSocketAddrs,
        device: &DeviceKeys,
        expect: Option<DeviceId>,
    ) -> Result<Self> {
        // Every resolved address, not the first: `coordinator = ngas.fr:9898`
        // is how a member away from home finds one, and a name resolves to an
        // IPv6 record as well as an IPv4 one. On a network that drops IPv6,
        // stopping at the first meant the coordinator was unreachable while the
        // address beside it would have answered.
        let targets: Vec<SocketAddr> = address
            .to_socket_addrs()
            .map_err(CoordError::from)?
            .collect();

        let stream = itsanas_tls::reach::connect_to_one_of(&targets).map_err(CoordError::from)?;
        let local = stream.local_addr().map_err(CoordError::from)?;
        stream
            .set_read_timeout(Some(IO_TIMEOUT))
            .map_err(CoordError::from)?;
        stream
            .set_write_timeout(Some(IO_TIMEOUT))
            .map_err(CoordError::from)?;

        // A fresh identity per client process, same as a peer connection: the
        // certificate is a key transport for the handshake and nothing else, so
        // an observer cannot correlate two connections by it.
        let identity = Identity::generate()
            .map_err(|error| CoordError::Transport(format!("no TLS identity: {error}")))?;
        let config: Arc<ClientConfig> = identity
            .client_config()
            .map_err(|error| CoordError::Transport(format!("no TLS config: {error}")))?;

        let Authenticated { peer, connection } = connect(&config, device, stream, expect)
            .map_err(|error| CoordError::Transport(format!("handshake failed: {error}")))?;
        let _ = peer;

        let mut client = Self { connection, local };

        match client.ask(&Request::Hello {
            version: COORD_VERSION,
        })? {
            Response::Welcome { version } if version == COORD_VERSION => Ok(client),
            Response::Refused(why) => Err(CoordError::Transport(why)),
            other => Err(CoordError::Transport(format!(
                "expected a version agreement, got {other:?}"
            ))),
        }
    }

    /// Which of this machine's addresses reached the coordinator.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    /// Send one request and read one answer.
    pub fn ask(&mut self, request: &Request) -> Result<Response> {
        self.connection
            .exchange(request)
            .map_err(|error| CoordError::Transport(format!("coordinator: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use itsanas_crypto::SecretBytes;

    fn keys(seed: u8) -> DeviceKeys {
        DeviceKeys::from_seed(&SecretBytes::new([seed; 32]))
    }

    /// A listener that answers one connection as `device` and then stops.
    ///
    /// Everything a node's listener does that a probe can observe: TLS with a
    /// device proof. It deliberately does *not* speak the peer protocol,
    /// because the probe must not need it -- a coordinator that had to
    /// understand the peer protocol to answer this question would be a
    /// coordinator that knows what members store.
    fn listening_as(device: DeviceKeys) -> (SocketAddr, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let handle = std::thread::spawn(move || {
            let identity = Identity::generate().expect("identity");
            let config = identity.server_config().expect("server config");
            if let Ok((stream, _)) = listener.accept() {
                let _ = accept_within(
                    &config,
                    &device,
                    stream,
                    Duration::from_secs(5),
                    Duration::from_secs(5),
                );
            }
        });
        (address, handle)
    }

    #[test]
    fn a_probe_that_reaches_the_device_says_so() {
        // The same seed twice rather than a clone: a device key is not `Clone`
        // on purpose, because two copies of one are two machines sharing a
        // sequence counter.
        let expected = keys(0x21).device_id();
        let (address, handle) = listening_as(keys(0x21));

        let (reachable, detail) = probe(&address.to_string(), &[address], expected, &keys(0xC0));

        assert!(
            reachable,
            "a listening device was reported unreachable: {detail}"
        );
        let _ = handle.join();
    }

    /// THE POINT OF THE HANDSHAKE: an open port proves something is there. A
    /// forward pointing at the wrong machine -- the other Raspberry Pi, the
    /// printer, a neighbour on the same public address -- is an open port, and
    /// reporting it as success would tell a member their setup works while
    /// every peer that dialled them got refused by the pinning in
    /// `PeerClient::connect`.
    #[test]
    fn red_team_a_probe_that_reaches_a_different_machine_is_not_a_success() {
        let somebody_else = keys(0x22);
        let (address, handle) = listening_as(somebody_else);

        let (reachable, detail) = probe(
            &address.to_string(),
            &[address],
            keys(0x23).device_id(),
            &keys(0xC0),
        );

        assert!(
            !reachable,
            "a port answering as another device was reported as this one being reachable"
        );
        assert!(
            detail.contains("not as this device"),
            "the answer must say what is wrong, so a person can fix the forward; it said {detail:?}"
        );
        let _ = handle.join();
    }

    #[test]
    fn a_probe_of_an_address_where_nothing_listens_says_nothing_answered() {
        // Loopback port 1: refused immediately on every runner, so this is
        // fast and does not depend on a timeout.
        let dead: SocketAddr = "127.0.0.1:1".parse().unwrap();
        let (reachable, detail) =
            probe("127.0.0.1:1", &[dead], keys(0x24).device_id(), &keys(0xC0));

        assert!(!reachable);
        assert!(
            detail.contains("forward") || detail.contains("firewall"),
            "the answer must name what a person should go and look at; it said {detail:?}"
        );
    }

    /// THE BYPASS, and it was live for an afternoon. The guard in
    /// `probe_target` reads the announced *string*, and a name is never private
    /// to it -- correctly, because what a name resolves to is not a dialler's
    /// business. Then `probe` resolves it. So `nas.example.org` pointing at
    /// `192.168.1.10` walked straight through a guard written to stop exactly
    /// that, and turned the coordinator into a scanner of its own network by
    /// way of DNS. The check that counts is on the resolved address.
    #[test]
    fn red_team_a_name_that_resolves_into_a_private_network_is_not_dialled() {
        // `localhost` is the one name every machine resolves to a private
        // address, so this is the bypass without a resolver anybody controls.
        // Nothing is dialled here: the refusal happens before any socket, which
        // is the whole point -- during the sabotage run that proved this hole,
        // the probe reached the real daemon on the machine running the test.
        let refused = resolve_probe_target("localhost:9797")
            .expect_err("a name resolving into a private network was accepted");

        assert!(
            refused.contains("private address"),
            "the refusal must say why, so nobody 'fixes' it by widening the guard; it said {refused:?}"
        );
    }

    #[test]
    fn a_probe_of_a_name_that_does_not_resolve_says_that_rather_than_timing_out() {
        let refused = resolve_probe_target("this-name-does-not-exist.invalid:9797")
            .expect_err("a name that does not resolve is not somewhere to dial");

        assert!(
            refused.contains("does not resolve"),
            "DNS and a closed port are different problems, fixed in different places, and must read differently; it said {refused:?}"
        );
    }

    /// THE BUDGET, as a property of the counter rather than of the caller: a
    /// slot that is not given back on a failure path is a limit that shrinks to
    /// zero, and every probe of an unreachable member *is* a failure path.
    #[test]
    fn a_probe_slot_is_given_back_however_the_probe_ends() {
        let count = AtomicUsize::new(0);

        for _ in 0..MAX_PROBES_IN_FLIGHT * 3 {
            let slot = InFlight::take(&count).expect("a slot must be free");
            drop(slot);
        }
        assert_eq!(count.load(Ordering::SeqCst), 0);

        let held: Vec<_> = (0..MAX_PROBES_IN_FLIGHT)
            .map(|_| InFlight::take(&count).expect("within the bound"))
            .collect();
        assert!(
            InFlight::take(&count).is_none(),
            "more probes ran at once than the bound allows"
        );
        drop(held);
        assert!(
            InFlight::take(&count).is_some(),
            "the bound never reopened, so this coordinator stops probing for ever"
        );
    }
}
