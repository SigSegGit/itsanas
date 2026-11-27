//! The listening side: binding, the connection limits, serving a session,
//! relaying and parking, and what reached the listener from where.

use std::{
    io::{Read, Write},
    net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use itsanas_crypto::{DeviceId, DeviceKeys};
use itsanas_tls::{
    Authenticated, Identity, Socket,
    limits::{ConnectionLimits, Slot},
    reach::{Listeners, listen_on},
};
use itsanas_wire::Connection;

use crate::relay::NOT_RELAYING;

use crate::{
    error::{NetError, Result},
    protocol::{Request, Response},
    service::PeerService,
};

use super::client::PeerClient;
use super::{IO_TIMEOUT, PEER_SESSION_BUDGET};

/// What has actually reached this listener, and from where.
///
/// The cheapest reachability evidence there is, because it costs nothing: a
/// connection that arrives from a public address **is** proof that this machine
/// can be reached from outside its own network, and it arrived whether or not
/// anybody was counting. A node that has announced a public address and has
/// never seen one of these has a forward that does not work, and can say so
/// without asking anybody anything.
///
/// The counts are of *accepted connections*, before any handshake: a scanner on
/// a public port counts, and should -- the question here is whether packets
/// arrive, not whether friends do. Nothing here is a security decision.
#[derive(Debug, Default)]
pub struct Witness {
    /// Connections from an address outside any private range.
    from_outside: AtomicU64,
    /// Connections from a private, loopback or link-local address, or from
    /// the IPv6 /64 the connection arrived on ([`on_link`]).
    from_inside: AtomicU64,
}

impl Witness {
    /// Count one accepted connection from `from`, which arrived on `to`.
    fn saw(&self, from: IpAddr, to: Option<IpAddr>) {
        let counter = if itsanas_tls::reach::is_private_ip(from) || on_link(from, to) {
            &self.from_inside
        } else {
            &self.from_outside
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// How many connections have arrived from outside this network.
    #[must_use]
    pub fn from_outside(&self) -> u64 {
        self.from_outside.load(Ordering::Relaxed)
    }

    /// How many have arrived from this network, or from this machine.
    #[must_use]
    pub fn from_inside(&self) -> u64 {
        self.from_inside.load(Ordering::Relaxed)
    }
}

/// Whether `from` is an IPv6 address in the same /64 as `to`, the address it
/// reached: a machine on this LAN, which crossed no router.
///
/// A global IPv6 address is not private, so a neighbour dialling this
/// machine's global address used to count as a caller from outside. That
/// count is what tells the daemon its way in works and it needs no relay
/// (HANDOVER §8 0v): a home machine behind a router that drops inbound IPv6
/// stopped parking as soon as a machine of its own house dialled it, and
/// was then unreachable from every other house. IPv4 callers on a
/// dual-stack socket arrive as `::ffff:a.b.c.d`, all of them under one
/// /64, so the comparison is between real IPv6 addresses only.
fn on_link(from: IpAddr, to: Option<IpAddr>) -> bool {
    match (from.to_canonical(), to.map(|to| to.to_canonical())) {
        (IpAddr::V6(from), Some(IpAddr::V6(to))) => from.segments()[..4] == to.segments()[..4],
        _ => false,
    }
}

/// Accepts peer connections and answers them from a [`PeerService`].
#[derive(Debug)]
pub struct PeerServer {
    listener: Listeners,
    config: Arc<rustls_config::Server>,
    handshake_deadline: Duration,
    witness: Arc<Witness>,
    desk: crate::relay::Desk,
}

/// Keeps the rustls type out of this module's public signatures.
mod rustls_config {
    pub type Server = rustls::ServerConfig;
}

impl PeerServer {
    /// Bind a listener.
    ///
    /// Any address is acceptable. The transport is encrypted and both ends are
    /// authenticated, so exposing it to a network is a normal thing to do.
    pub fn bind(address: impl ToSocketAddrs) -> Result<Self> {
        let resolved: Vec<SocketAddr> = address.to_socket_addrs()?.collect();
        let identity = Identity::generate()?;

        Ok(Self {
            listener: listen_on(&resolved)?,
            config: identity.server_config()?,
            handshake_deadline: HANDSHAKE_DEADLINE,
            witness: Arc::new(Witness::default()),
            desk: crate::relay::Desk::default(),
        })
    }

    /// Give callers `limit` instead of [`HANDSHAKE_DEADLINE`] to authenticate.
    ///
    /// For tests, which cannot wait fifteen seconds to watch a deadline pass.
    /// Nothing in the shipped binaries changes it.
    #[must_use]
    pub const fn with_handshake_deadline(mut self, limit: Duration) -> Self {
        self.handshake_deadline = limit;
        self
    }

    /// What has reached this listener, shared with whoever wants to report it.
    ///
    /// Cloned rather than borrowed because the serving thread owns the server
    /// for its lifetime, and the thread that reports is the one that does not.
    #[must_use]
    pub fn witness(&self) -> Arc<Witness> {
        Arc::clone(&self.witness)
    }

    /// The address actually bound, which matters when port 0 was requested.
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.listener.local_addr()?)
    }

    /// Accept one connection and serve it until the peer closes.
    pub fn serve_one(&self, service: &PeerService<'_>, device: &DeviceKeys) -> Result<()> {
        let (stream, from) = self.listener.accept()?;
        let limits = limits();
        let Some(slot) = limits.admit(from.ip()) else {
            return Ok(());
        };
        self.serve_connection(stream, service, device, slot, &AtomicBool::new(false))
    }

    /// Serve until `shutdown` is set.
    ///
    /// # One thread per connection, and why that changed
    ///
    /// This used to serve one connection at a time, until the peer closed it.
    /// On a home network that was a simplification; on a public port it is an
    /// off switch. A single TCP connection that said nothing held the listener
    /// for the thirty-second read timeout, so opening one every thirty seconds
    /// made the node undialable to everybody else, at no cost to whoever did it.
    ///
    /// Now each connection has its own thread, and what an anonymous caller can
    /// occupy is bounded three ways: a deadline on the whole handshake
    /// ([`HANDSHAKE_DEADLINE`]), a cap on connections from one address
    /// ([`MAX_CONNECTIONS_PER_ADDRESS`]) and a cap overall ([`MAX_CONNECTIONS`]).
    /// Once a caller has proved a device key it is also held to
    /// [`MAX_CONNECTIONS_PER_DEVICE`].
    ///
    /// What still happens one at a time is *storing*: see
    /// [`PeerService::handle`]. Reads never needed the old serialisation, and
    /// the sync loop was already touching the store alongside the listener.
    pub fn serve_until(
        &self,
        service: &PeerService<'_>,
        device: &DeviceKeys,
        shutdown: &AtomicBool,
    ) -> Result<()> {
        self.listener.set_nonblocking(true)?;
        let limits = limits();

        std::thread::scope(|scope| {
            while !shutdown.load(Ordering::Relaxed) {
                match self.listener.accept() {
                    Ok((stream, from)) => {
                        // Counted before anything can refuse it: what this
                        // answers is "do packets from outside arrive here",
                        // and a connection dropped at the limit arrived.
                        // The address it reached, for the /64 it shares
                        // with a caller on this LAN (`on_link`).
                        let to = stream.local_addr().ok().map(|to| to.ip());
                        self.witness.saw(from.ip(), to);
                        // Windows hands back a socket that inherited the
                        // listener's non-blocking mode, which fails the TLS
                        // handshake with an error that reads like a firewall.
                        if stream.set_nonblocking(false).is_err() {
                            continue;
                        }
                        // Over a limit, the connection is closed at once:
                        // queueing is a slower way to run out of threads.
                        let Some(slot) = limits.admit(from.ip()) else {
                            continue;
                        };
                        scope.spawn(move || {
                            // One peer misbehaving must not stop the server. A
                            // failed handshake is the most ordinary thing on a
                            // public port.
                            let _ = self.serve_connection(stream, service, device, slot, shutdown);
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    // Not fatal, whatever it is. `accept` fails for reasons a
                    // caller controls -- a connection reset before it was
                    // taken (ECONNABORTED on Linux, WSAECONNRESET on Windows)
                    // -- and for passing ones, such as running out of file
                    // descriptors. Returning here stopped the listener for
                    // good while the daemon carried on without one, which on
                    // a forwarded port is an off switch a SYN and a RST can
                    // reach. The coordinator has always carried on; this did
                    // not. Found by the Rodin audit of 2026-09-17; the error
                    // was not reproduced, and no test here can provoke it.
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            }
            Ok(())
        })
    }

    fn serve_connection(
        &self,
        stream: TcpStream,
        service: &PeerService<'_>,
        device: &DeviceKeys,
        mut slot: Slot<'_>,
        shutdown: &AtomicBool,
    ) -> Result<()> {
        let from = stream.peer_addr().ok().map(|address| address.ip());
        let Authenticated {
            peer,
            mut connection,
        } = itsanas_tls::accept_within(
            &self.config,
            device,
            stream,
            self.handshake_deadline,
            IO_TIMEOUT,
        )?;

        let within_limit = slot.claim_device(peer);
        if within_limit {
            service.note_session(peer, from);
        }

        loop {
            let Some(request) = connection.receive::<Request>()? else {
                // A clean close between requests is how a peer says goodbye.
                return Ok(());
            };
            match request {
                Request::Park | Request::Splice { .. } if within_limit => {
                    // The connection stops carrying requests either way.
                    return self.relay(connection, request, peer, service, shutdown);
                }
                request => {
                    if !answer(
                        &mut connection,
                        &request,
                        peer,
                        within_limit,
                        service,
                        shutdown,
                    )? {
                        return Ok(());
                    }
                }
            }
        }
    }

    /// A park or a splice (§8 0v), from a caller this node relays for; the
    /// same refusal to anybody else, parked or not.
    fn relay(
        &self,
        mut connection: Connection<crate::relay::Outer>,
        request: Request,
        peer: DeviceId,
        service: &PeerService<'_>,
        shutdown: &AtomicBool,
    ) -> Result<()> {
        let class = service.relays_for(peer);
        match (request, class) {
            (Request::Park, Some(class)) => {
                let Some((waiting, _ticket)) = self.desk.park(peer, class) else {
                    connection.send(&Response::Refused(NOT_RELAYING.to_owned()))?;
                    return Ok(());
                };
                connection.send(&Response::Parked)?;
                let mut said = Instant::now();
                // The parked machine says nothing after `Park`: a read that
                // ends is a machine gone, seen within a second rather than
                // at the next keepalive, so a caller is never spliced onto
                // a dead park. A read that times out is the park holding.
                connection.stream().set_read_timeout(Some(RELAY_LISTEN))?;
                loop {
                    match connection.receive::<Request>() {
                        Err(itsanas_wire::StreamError::Io(error)) if timed_out(&error) => {}
                        Ok(None) | Err(_) => return Ok(()),
                        Ok(Some(_)) => {
                            connection.send(&Response::Refused(NOT_RELAYING.to_owned()))?;
                            return Ok(());
                        }
                    }
                    // Woken every second so a stopping daemon lets its parks go.
                    match waiting.handed.try_recv().map_err(|error| match error {
                        std::sync::mpsc::TryRecvError::Empty => {
                            std::sync::mpsc::RecvTimeoutError::Timeout
                        }
                        std::sync::mpsc::TryRecvError::Disconnected => {
                            std::sync::mpsc::RecvTimeoutError::Disconnected
                        }
                    }) {
                        Ok(handoff) => {
                            let _splicing = waiting.carrying();
                            connection.send(&Response::Incoming { from: handoff.from })?;
                            let (mut parked, parked_rest) = connection.into_parts();
                            let mut caller = handoff.stream;
                            parked.write_all(&handoff.rest)?;
                            caller.write_all(&parked_rest)?;
                            self.desk.pump(
                                &mut caller,
                                &mut parked,
                                crate::relay::SPLICE_BUDGET,
                                crate::relay::SPLICE_IDLE,
                            );
                            return Ok(());
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            if shutdown.load(Ordering::Relaxed) {
                                return Ok(());
                            }
                            if said.elapsed() >= crate::relay::KEEPALIVE {
                                connection.send(&Response::Parked)?;
                                said = Instant::now();
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                    }
                }
            }
            (Request::Splice { to }, Some(class)) if to != peer => {
                let Some(reserved) = self.desk.reserve(to, class) else {
                    connection.send(&Response::Refused(NOT_RELAYING.to_owned()))?;
                    return Ok(());
                };
                connection.send(&Response::Spliced)?;
                let (stream, rest) = connection.into_parts();
                reserved.hand(crate::relay::Handoff {
                    stream,
                    rest,
                    from: peer,
                });
                Ok(())
            }
            _ => {
                connection.send(&Response::Refused(NOT_RELAYING.to_owned()))?;
                Ok(())
            }
        }
    }

    /// Park on `relay` and serve, over the splice, the one caller it hands
    /// on (§8 0v); returns once that caller is done, however its session
    /// ended (`true`), or the relay closed the park with nobody (`false`, which the caller backs off on:
    /// a relay that closes every park must not be dialled in a loop).
    /// The daemon calls it again, with a pause, for as long as this node is
    /// reachable only that way.
    ///
    /// The caller proves itself again over the splice: who the relay says
    /// it is ([`Response::Incoming`]) is not taken.
    ///
    /// # Errors
    ///
    /// A relay that refuses, is too old, or cannot be reached, and a caller
    /// handed on that fails to prove itself over the splice.
    pub fn serve_parked(
        &self,
        relay: impl ToSocketAddrs,
        relay_device: DeviceId,
        service: &PeerService<'_>,
        device: &DeviceKeys,
        shutdown: &AtomicBool,
    ) -> Result<bool> {
        let mut client = PeerClient::connect(relay, device, service.owner(), Some(relay_device))?;
        if client.spoken < crate::protocol::PROTOCOL_WITH_RELAY {
            return Err(NetError::Refused(crate::relay::RELAY_TOO_OLD.to_owned()));
        }
        match client.request(&Request::Park)? {
            Response::Parked => {}
            Response::Refused(reason) => return Err(NetError::Refused(reason)),
            _ => return Err(NetError::UnexpectedResponse { expected: "parked" }),
        }
        // Woken every second, so a stopping daemon lets go at once rather
        // than after a read timeout; a relay silent past two keepalives is
        // a relay gone.
        client
            .connection
            .stream()
            .set_read_timeout(Some(PARK_LISTEN))?;
        let mut heard = Instant::now();
        loop {
            if shutdown.load(Ordering::Relaxed) {
                return Ok(false);
            }
            match client.connection.receive::<Response>() {
                Err(itsanas_wire::StreamError::Io(error)) if timed_out(&error) => {
                    if heard.elapsed() > 2 * crate::relay::KEEPALIVE + PARK_LISTEN {
                        return Ok(false);
                    }
                }
                Err(error) => return Err(error.into()),
                Ok(Some(Response::Parked)) => heard = Instant::now(),
                Ok(Some(Response::Incoming { .. })) => break,
                Ok(Some(_)) => {
                    return Err(NetError::UnexpectedResponse {
                        expected: "incoming",
                    });
                }
                Ok(None) => return Ok(false),
            }
        }
        let (stream, rest) = client.into_parts();
        let Authenticated {
            peer,
            mut connection,
        } = itsanas_tls::accept_within(
            &self.config,
            device,
            crate::relay::Prefixed::new(rest, stream),
            self.handshake_deadline,
            IO_TIMEOUT,
        )?;
        // Spliced: the socket is the relay's, so the caller's network is not
        // known here.
        service.note_session(peer, None);
        // An ordinary session's budget: a caller, or the relay, trickling
        // requests does not keep this machine from parking again.
        let until = Instant::now() + PEER_SESSION_BUDGET;
        // However the caller's session ends, the park did its job. No client
        // sends TLS close_notify, so a caller that is done ends it with an
        // end of stream rustls reports as an error; taken for the relay
        // failing, it set the relay aside and kept this machine unparked
        // for a pause, refusing every caller meanwhile (fleet run
        // 40176979039: 23 refusals to 3 splices). A direct session's end is
        // not reported either (`serve_until`).
        while let Ok(Some(request)) = connection.receive::<Request>() {
            if !answer(&mut connection, &request, peer, true, service, shutdown).unwrap_or(false)
                || Instant::now() >= until
            {
                break;
            }
        }
        Ok(true)
    }
}

/// How long a park's side waits on its socket before looking at anything
/// else: the other side gone, a caller handed, the daemon stopping.
const PARK_LISTEN: Duration = Duration::from_secs(1);

/// The relay's side of the same: short, because a caller handed to the park
/// waits for it, inner handshake running.
const RELAY_LISTEN: Duration = Duration::from_millis(50);

/// A read that ran out of time, which on a park is the ordinary case.
fn timed_out(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// Answer one request; `false` when the connection is to close.
fn answer<S: Read + Write>(
    connection: &mut Connection<S>,
    request: &Request,
    peer: DeviceId,
    within_limit: bool,
    service: &PeerService<'_>,
    shutdown: &AtomicBool,
) -> Result<bool> {
    // Read before refusing, never the other way round: closing a socket
    // with unread data makes Windows reset it, which discards the
    // refusal and leaves the caller reading "connection aborted".
    if !within_limit {
        connection.send(&Response::Refused(TOO_MANY_FROM_DEVICE.to_owned()))?;
        return Ok(false);
    }

    // `peer` is who TLS proved is on the other end, and it used to be
    // dropped on the line above with `let _ = peer;`. Answering
    // `Request::Hosted` needs it: recording that somebody holds a chunk
    // is worthless if you cannot say who.
    let response = service.handle(request, peer)?;
    connection.send(&response)?;

    // Between requests, so a peer that keeps asking cannot keep a
    // stopping daemon alive.
    Ok(!shutdown.load(Ordering::Relaxed))
}

/// How long a caller has, in total, to complete TLS and prove its device key.
///
/// Generous for a phone on a bad link -- the handshake is a few kilobytes and
/// two signatures -- and short enough that holding a slot costs an attacker a
/// fresh connection every quarter of a minute.
pub const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(15);

/// Connections served at once, from everybody.
///
/// A node talks to a handful of machines; this is several times that, and a
/// thread each is a few megabytes, which a Raspberry Pi can afford.
pub const MAX_CONNECTIONS: usize = 32;

/// Connections served at once from one IP address.
///
/// Not one: every machine behind a household's router shares an address, and
/// two accounts on one machine share it too. Eight covers a house full of them
/// and still leaves most of [`MAX_CONNECTIONS`] to everybody else.
pub const MAX_CONNECTIONS_PER_ADDRESS: usize = 8;

/// Connections served at once for one proven device.
///
/// A daemon's round and an `itsanas sync` typed while it runs are two; four
/// leaves room without letting one key fill the server.
pub const MAX_CONNECTIONS_PER_DEVICE: usize = 4;

/// What a device over [`MAX_CONNECTIONS_PER_DEVICE`] is told.
pub const TOO_MANY_FROM_DEVICE: &str = "too many connections from this device at once";

fn limits() -> ConnectionLimits {
    ConnectionLimits::new(
        MAX_CONNECTIONS,
        MAX_CONNECTIONS_PER_ADDRESS,
        MAX_CONNECTIONS_PER_DEVICE,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_team_a_neighbour_on_the_same_ipv6_network_is_not_a_caller_from_outside() {
        // The attack is the house itself: the home coordinator's probe, or a
        // machine of this LAN, dials the laptop's global IPv6 address. Counted
        // from outside, it told the daemon its way in works, so the laptop
        // stopped parking on a relay while the router dropped every caller
        // from another house. Sabotage: drop `|| on_link(from, to)` in
        // `Witness::saw`, or the `to_canonical` calls in `on_link`.
        let here: IpAddr = "2a01:e0a:1045:9d30:4839:531c:38a0:648f".parse().unwrap();
        let witness = Witness::default();

        witness.saw("2a01:e0a:1045:9d30::17".parse().unwrap(), Some(here));
        assert_eq!(
            witness.from_outside(),
            0,
            "a machine of this house dialling the global IPv6 address counted as \
             proof that other houses can: the node would stop parking on a relay \
             and nobody elsewhere could reach it"
        );
        assert_eq!(witness.from_inside(), 1);

        witness.saw("2a01:cb00:1:2::5".parse().unwrap(), Some(here));
        assert_eq!(
            witness.from_outside(),
            1,
            "a caller from another IPv6 network did not count: a node whose way \
             in works would keep its peers on a slower relay"
        );

        // Two IPv4 callers on a dual-stack socket share the /64 `::ffff:0:0`
        // and say nothing about each other.
        let mapped_here: IpAddr = "::ffff:203.0.113.5".parse().unwrap();
        witness.saw("::ffff:198.51.100.9".parse().unwrap(), Some(mapped_here));
        assert_eq!(
            witness.from_outside(),
            2,
            "a public IPv4 caller was counted as a neighbour because both ends were \
             IPv4-mapped: a forwarded port that works would look closed"
        );
    }

    #[test]
    fn binding_a_public_address_no_longer_needs_an_override() {
        // A wildcard bind on a developer's Windows asks the firewall at every new
        // test binary (Nicolas, 2026-11-18: a window per build); CI runs it.
        if cfg!(windows) && std::env::var_os("CI").is_none() {
            return;
        }
        // It used to be refused, because the transport leaked chunk identifiers
        // and sizes to anyone on the path. TLS closed that, so keeping the
        // refusal would be cargo cult.
        let server = PeerServer::bind("0.0.0.0:0").expect("a public bind should work");
        // Unspecified rather than `0.0.0.0` exactly: since 2026-09-18 a wildcard
        // bind takes a dual-stack socket where the machine has IPv6, so it
        // reports `::` and accepts both families. What this test is about is
        // that the bind is not refused.
        assert!(server.local_addr().unwrap().ip().is_unspecified());
    }

    #[test]
    fn loopback_still_binds() {
        let server = PeerServer::bind("127.0.0.1:0").unwrap();
        assert!(server.local_addr().unwrap().ip().is_loopback());
    }
}
