//! Moving frames between two machines, encrypted and mutually authenticated.
//!
//! # What changed, and why the warnings are gone
//!
//! This transport used to be plain TCP. Your *data* was never at risk — chunk
//! bodies and log segments are sealed before they reach the wire — but an
//! observer on the path saw chunk identifiers, object sizes and timing. The
//! threat model grants a host all three, because a host stores the chunks; it
//! does not grant them to an arbitrary network between two of your machines.
//! `PeerServer::bind` therefore refused non-loopback addresses.
//!
//! It no longer needs to. Every connection is TLS 1.3, and both ends prove
//! which device they are by signing the session's exporter value with their
//! device key ([`itsanas_tls`]). A man in the middle who terminates TLS gets a
//! different exporter and cannot forge either signature, so the encryption is
//! bound to the identity rather than sitting beside it.
//!
//! The certificates are anonymous and regenerated every start-up. That is not
//! a weakness — see [`itsanas_tls::session`] — and it means an observer cannot
//! correlate two connections by their certificates either.
//!
//! # Serving strangers is still deliberate
//!
//! A node answers any device that authenticates, including one it has never
//! met. That is what lets somebody offer storage to the network at all.
//! Everything it can serve is sealed or signed, so serving it to the wrong
//! person reveals nothing, and how much it will *store* is bounded by the
//! pledge. What is new is that the node now knows *who* it served, which is
//! what any future policy would need.
//!
//! # Why blocking sockets and threads
//!
//! A node talks to a handful of peers, not ten thousand. A thread per
//! connection costs a few megabytes and buys code that can be read top to
//! bottom. Async would buy scalability this design does not need, at the price
//! of an executor in every signature.

use std::{
    io::{Read, Write},
    net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use itsanas_crypto::{ChunkId, DeviceId, DeviceKeys, ObjectId, UserId};
use itsanas_store::SegmentEnvelope;
use itsanas_tls::{
    Authenticated, Identity, Socket,
    limits::{ConnectionLimits, Slot},
    reach::{Listeners, connect_to_one_of, listen_on},
};
use itsanas_wire::Connection;

use crate::relay::NOT_RELAYING;

use crate::{
    error::{NetError, Result},
    protocol::{
        Head, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION, PROTOCOL_WITH_CHUNK_SUMMARY,
        PROTOCOL_WITH_CLAIMS, PROTOCOL_WITH_DRAINING, PROTOCOL_WITH_DROP_NOTICES,
        PROTOCOL_WITH_EVICTION, PROTOCOL_WITH_LEAVING, PROTOCOL_WITH_PRESENCES, Request, Response,
    },
    service::PeerService,
};

/// How long a read or write may stall before the connection is abandoned.
///
/// Without this a single peer that opens a connection and then says nothing
/// holds a thread forever, and enough of them exhaust the node.
pub const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the daemon gives one peer in a round, everything included.
///
/// [`IO_TIMEOUT`] bounds one read or write, not a conversation: a peer that
/// answers a little at a time, or keeps a slow transfer just alive, holds the
/// round for as long as it likes, and with it the folder pass and every other
/// peer. Seen on 2026-10-06: a laptop's daemon spent 45 minutes on one
/// connection to an older-build node reached through a virtual network adapter, and
/// nothing it or its peers wrote moved in that time. Cutting a session loses
/// nothing -- every chunk is acknowledged on its own and the next round goes
/// on from there -- so the budget is one sync interval: a round with `n`
/// peers ends within `n` intervals whatever any of them does.
pub const PEER_SESSION_BUDGET: Duration = Duration::from_secs(300);

/// A socket whose whole conversation, not just each read, has a deadline.
///
/// Every read and write is given the smaller of [`IO_TIMEOUT`] and what is left
/// of the budget, and fails once nothing is. Without a budget it behaves as a
/// plain socket under [`IO_TIMEOUT`], which is what interactive commands want:
/// somebody watching `itsanas sync` can stop it themselves.
pub struct Budgeted {
    stream: Box<dyn Socket>,
    until: Option<Instant>,
}

impl std::fmt::Debug for Budgeted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Budgeted")
            .field("until", &self.until)
            .finish_non_exhaustive()
    }
}

impl Budgeted {
    fn left(&self) -> std::io::Result<Option<Duration>> {
        let Some(until) = self.until else {
            return Ok(None);
        };
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "this peer used up its time for this round; the rest waits for the next",
            ));
        }
        Ok(Some(left.min(IO_TIMEOUT)))
    }
}

impl Socket for Budgeted {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.stream.set_read_timeout(timeout)
    }
    fn set_write_timeout(&self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.stream.set_write_timeout(timeout)
    }
}

impl Read for Budgeted {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(limit) = self.left()? {
            self.stream.set_read_timeout(Some(limit))?;
        }
        self.stream.read(buf)
    }
}

impl Write for Budgeted {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(limit) = self.left()? {
            self.stream.set_write_timeout(Some(limit))?;
        }
        self.stream.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

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

/// What a peer did with something offered for storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Offer {
    /// It stored it.
    Taken,
    /// It already had it, which is ordinary: re-offering a log tip is.
    AlreadyHeld,
    /// It said no.
    ///
    /// Kept apart from `AlreadyHeld`, which it used to share a `false` with, so
    /// a host refusing everything read exactly like a round with nothing to do.
    Refused(Refusal),
}

/// Why a peer refused an offer.
///
/// Two cases rather than the peer's sentence, so a report stays `Copy` and a
/// hostile peer's text never reaches a log line through this path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// It has no room left under its pledge, or pledged nothing.
    PledgeFull,
    /// Any other refusal: a segment that does not verify or does not chain.
    Rejected,
    /// It has room, but not for this account: the account has stored there
    /// what its pledge earns (§8 1c).
    ///
    /// Kept apart from [`Self::Rejected`], which it used to read as, because
    /// the two call for opposite answers. A rejection says something was
    /// wrong with what was sent. This says the account's share on that host
    /// is spent: the owner can pledge more, or let the host evict copies it
    /// has to spare ([`PeerClient::may_evict`]), and the host is no worse a
    /// place for the account's data than before.
    AccountFull,
    /// The host has not confirmed this device of another account with its
    /// coordinator lately (`UNCONFIRMED`): nothing was wrong with what was
    /// sent. Read as [`Self::Rejected`], it sent a session after segments and
    /// chains when the answer was a coordinator (`tests/route.rs`, 2026-10-30).
    Unconfirmed,
    /// The coordinator says this device was withdrawn by its account.
    Withdrawn,
    /// This account's devices failed the host's storage audits.
    Contradicted,
}

impl Offer {
    fn from_response(answer: Response) -> Result<Self> {
        match answer {
            Response::Stored { accepted: true } => Ok(Self::Taken),
            Response::Stored { accepted: false } => Ok(Self::AlreadyHeld),
            Response::Refused(reason) if reason == crate::service::PLEDGE_EXHAUSTED => {
                Ok(Self::Refused(Refusal::PledgeFull))
            }
            // A prefix, not an equality: the host appends what the account
            // holds and what it earned, for the operator's log.
            Response::Refused(reason) if reason.starts_with(crate::service::ACCOUNT_FULL) => {
                Ok(Self::Refused(Refusal::AccountFull))
            }
            Response::Refused(reason) if reason == crate::service::UNCONFIRMED => {
                Ok(Self::Refused(Refusal::Unconfirmed))
            }
            Response::Refused(reason) if reason == crate::service::WITHDRAWN => {
                Ok(Self::Refused(Refusal::Withdrawn))
            }
            Response::Refused(reason) if reason == crate::service::CONTRADICTED => {
                Ok(Self::Refused(Refusal::Contradicted))
            }
            Response::Refused(_) => Ok(Self::Refused(Refusal::Rejected)),
            _ => Err(NetError::UnexpectedResponse { expected: "stored" }),
        }
    }
}

/// A connection to a peer, from the asking side.
pub struct PeerClient {
    connection: Connection<itsanas_tls::session::ClientStream<Budgeted>>,
    peer_device: DeviceId,
    /// The version both sides agreed to speak.
    ///
    /// Not a formality: a verb added after that version does not exist for this
    /// peer, and sending it would fail the connection rather than the feature.
    /// Every caller of a newer request asks this first.
    spoken: u16,
    /// The address the socket reached, for a direct session only: over a
    /// splice it would be the relay's, which says nothing about the peer.
    direct: Option<std::net::IpAddr>,
}

impl std::fmt::Debug for PeerClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerClient")
            .field("peer_device", &self.peer_device)
            .finish_non_exhaustive()
    }
}

impl PeerClient {
    /// Connect, authenticate, and complete the opening exchange.
    ///
    /// `expect` pins which device must answer. Pass it whenever the identity is
    /// known — addresses come from the coordinator, and the coordinator is not
    /// trusted to say who lives at one.
    pub fn connect(
        address: impl ToSocketAddrs,
        device: &DeviceKeys,
        owner: UserId,
        expect: Option<DeviceId>,
    ) -> Result<Self> {
        Self::connect_within(address, device, owner, expect, None)
    }

    /// [`Self::connect`], with the whole conversation bounded by `budget`,
    /// handshake included: once it is spent every read and write fails, so a
    /// peer that stalls cannot hold the caller past it. The daemon passes
    /// [`PEER_SESSION_BUDGET`].
    pub fn connect_within(
        address: impl ToSocketAddrs,
        device: &DeviceKeys,
        owner: UserId,
        expect: Option<DeviceId>,
        budget: Option<Duration>,
    ) -> Result<Self> {
        let addresses: Vec<SocketAddr> = address.to_socket_addrs()?.collect();

        let stream = connect_to_one_of(&addresses)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        stream.set_nodelay(true)?;
        let direct = stream.peer_addr().ok().map(|address| address.ip());
        let until = budget.map(|budget| Instant::now() + budget);
        Self::establish(
            Budgeted {
                stream: Box::new(stream),
                until,
            },
            device,
            owner,
            expect,
            direct,
        )
    }

    /// Reach `target` through `relay`, a peer it has parked on (§8 0v).
    ///
    /// The relay is dialled and proven like any peer, asked to splice, and
    /// then a second, end-to-end session is run over the splice, with
    /// `target` pinned: the relay passes ciphertext it cannot read, and a
    /// relay answering in `target`'s place fails that session's proof.
    ///
    /// # Errors
    ///
    /// The relay's refusal, a relay older than
    /// [`crate::protocol::PROTOCOL_WITH_RELAY`], or anything `connect` fails on.
    pub fn connect_relayed(
        relay: impl ToSocketAddrs,
        relay_device: DeviceId,
        target: DeviceId,
        device: &DeviceKeys,
        owner: UserId,
        budget: Option<Duration>,
    ) -> Result<Self> {
        let mut outer = Self::connect_within(relay, device, owner, Some(relay_device), budget)?;
        if outer.spoken < crate::protocol::PROTOCOL_WITH_RELAY {
            return Err(NetError::Refused(crate::relay::RELAY_TOO_OLD.to_owned()));
        }
        match outer.request(&Request::Splice { to: target })? {
            Response::Spliced => {}
            Response::Refused(reason) => return Err(NetError::Refused(reason)),
            _ => {
                return Err(NetError::UnexpectedResponse {
                    expected: "spliced",
                });
            }
        }
        let (stream, rest) = outer.connection.into_parts();
        let until = budget.map(|budget| Instant::now() + budget);
        Self::establish(
            Budgeted {
                stream: Box::new(crate::relay::Prefixed::new(rest, stream)),
                until,
            },
            device,
            owner,
            Some(target),
            None,
        )
    }

    /// The stream back, and what was read past the last message: a park
    /// becomes a splice (§8 0v).
    pub(crate) fn into_parts(self) -> (itsanas_tls::session::ClientStream<Budgeted>, Vec<u8>) {
        self.connection.into_parts()
    }

    fn establish(
        stream: Budgeted,
        device: &DeviceKeys,
        owner: UserId,
        expect: Option<DeviceId>,
        direct: Option<std::net::IpAddr>,
    ) -> Result<Self> {
        let identity = Identity::generate()?;
        let Authenticated { peer, connection } =
            itsanas_tls::connect(&identity.client_config()?, device, stream, expect)?;

        let mut client = Self {
            connection,
            peer_device: peer,
            spoken: MIN_PROTOCOL_VERSION,
            direct,
        };

        // Version negotiation after authentication, so an incompatible peer
        // fails legibly rather than on some later message.
        match client.request(&Request::Hello {
            protocol: PROTOCOL_VERSION,
            device: device.device_id(),
            owner,
        })? {
            Response::Hello { protocol, .. } => {
                if protocol < MIN_PROTOCOL_VERSION {
                    return Err(NetError::UnsupportedProtocolVersion {
                        found: protocol,
                        supported: PROTOCOL_VERSION,
                    });
                }
                // Capped at what this node knows how to speak. A peer answering
                // with something higher is newer than us and has agreed to come
                // down; believing its number would have us using verbs we do
                // not have.
                client.spoken = protocol.min(PROTOCOL_VERSION);
            }
            Response::Refused(reason) => return Err(NetError::Refused(reason)),
            _ => {
                return Err(NetError::UnexpectedResponse { expected: "hello" });
            }
        }

        Ok(client)
    }

    /// The device on the other end, as it *proved* itself — not as it claimed.
    #[must_use]
    pub const fn peer_device(&self) -> DeviceId {
        self.peer_device
    }

    /// The address this session reached the peer at, if it reached it
    /// directly; `None` through a relay.
    #[must_use]
    pub const fn direct_address(&self) -> Option<std::net::IpAddr> {
        self.direct
    }

    /// The protocol version both sides settled on.
    #[must_use]
    pub const fn spoken(&self) -> u16 {
        self.spoken
    }

    /// Send a request and wait for its response.
    pub fn request(&mut self, request: &Request) -> Result<Response> {
        Ok(self.connection.exchange(request)?)
    }

    /// Ask what chains the peer holds for `owner`.
    pub fn heads(&mut self, owner: UserId) -> Result<Vec<Head>> {
        match self.request(&Request::Heads { owner })? {
            Response::Heads(heads) => Ok(heads),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse { expected: "heads" }),
        }
    }

    /// Fetch a run of segments from one device's chain.
    pub fn segments(
        &mut self,
        owner: UserId,
        device: DeviceId,
        after: Option<ObjectId>,
        limit: u16,
    ) -> Result<Vec<SegmentEnvelope>> {
        match self.request(&Request::Segments {
            owner,
            device,
            after,
            limit,
        })? {
            Response::Segments(segments) => Ok(segments),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse {
                expected: "segments",
            }),
        }
    }

    /// Fetch one sealed chunk.
    pub fn chunk(&mut self, owner: UserId, address: ChunkId) -> Result<Option<Vec<u8>>> {
        match self.request(&Request::Chunk { owner, address })? {
            Response::Chunk(chunk) => Ok(chunk),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse { expected: "chunk" }),
        }
    }

    /// Ask the peer whether it has anything it would like this node to hold.
    ///
    /// The question that makes hosting mutual over a single outbound
    /// connection. Everything else here runs one way -- this node offers its
    /// work and the peer stores it -- which made hosting something only the
    /// dialled side could do, and so shut out every member behind a router they
    /// do not control.
    ///
    /// Returns whose data it is along with the list, because the opening
    /// exchange carries this node's owner to the peer and not the other way
    /// round.
    ///
    /// # Errors
    ///
    /// If the peer refuses, or answers something else.
    pub fn want_hosted(&mut self, limit: u32) -> Result<(UserId, Vec<ChunkId>)> {
        match self.request(&Request::WantHosted { limit })? {
            Response::WantHosted { owner, chunks } => Ok((owner, chunks)),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse {
                expected: "chunks wanted",
            }),
        }
    }

    /// Tell the peer which of its chunks this node has taken.
    ///
    /// So the peer can record who holds them. It is the owner's ledger, and
    /// this is a claim rather than a proof -- the owner's storage challenges
    /// are what make it evidence.
    ///
    /// # Errors
    ///
    /// If the peer refuses, or answers something else.
    pub fn hosted(&mut self, chunks: Vec<ChunkId>) -> Result<bool> {
        match self.request(&Request::Hosted { chunks })? {
            Response::Stored { accepted } => Ok(accepted),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse { expected: "stored" }),
        }
    }

    /// Ask whether this peer holds the same chunks for `owner`.
    ///
    /// `None` when the peer is too old to be asked, which is not a failure:
    /// the caller then does what every round did before, and lists everything.
    pub fn chunk_summary(&mut self, owner: UserId) -> Result<Option<Vec<[u8; 32]>>> {
        if self.spoken < PROTOCOL_WITH_CHUNK_SUMMARY {
            return Ok(None);
        }
        match self.request(&Request::ChunkSummary { owner })? {
            // A summary of the wrong length is not a peer that holds different
            // data; it is a peer that is not answering the question. Treating
            // it as "everything differs" would let eight bytes of nonsense buy
            // a full listing of the account, every round, for ever -- the best
            // amplification ratio available in this protocol. It is a refusal.
            Response::ChunkSummary(digests) if digests.len() == itsanas_store::summary::BUCKETS => {
                Ok(Some(digests))
            }
            Response::ChunkSummary(_) => Err(NetError::UnexpectedResponse {
                expected: "a chunk summary with one digest per bucket",
            }),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse {
                expected: "chunk summary",
            }),
        }
    }

    /// Tell the peer this device no longer holds these chunks.
    ///
    /// Answers `false`, having sent nothing, when the peer is too old to know
    /// the verb. That is not a failure: the audit still catches the same
    /// staleness, more slowly, which is what happened before this existed.
    ///
    /// Batched by the caller: the request carries at most `MAX_HAVE_BATCH`
    /// addresses, the same ceiling the have/missing exchange uses.
    pub fn dropped(&mut self, owner: UserId, chunks: Vec<ChunkId>) -> Result<bool> {
        if self.spoken < PROTOCOL_WITH_DROP_NOTICES || chunks.is_empty() {
            return Ok(false);
        }
        match self.request(&Request::Dropped { owner, chunks })? {
            Response::Stored { accepted } => Ok(accepted),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse { expected: "stored" }),
        }
    }

    /// Tell the peer this device is going and should no longer be counted.
    ///
    /// Answers `false`, having sent nothing, when the peer is too old to know
    /// the verb: its records age out of `LIVE_FOR` instead, which is what
    /// happened before this existed.
    pub fn leaving(&mut self) -> Result<bool> {
        if self.spoken < PROTOCOL_WITH_LEAVING {
            return Ok(false);
        }
        match self.request(&Request::Leaving)? {
            Response::Stored { accepted } => Ok(accepted),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse { expected: "stored" }),
        }
    }

    /// Ask the peer to stop counting this device for `seconds`, and how many
    /// of its chunks rest on this device alone.
    ///
    /// `None`, having sent nothing, when the peer is too old to know the verb:
    /// the caller counts it as an owner it could not ask, never as 0.
    pub fn draining(&mut self, seconds: u64) -> Result<Option<u64>> {
        if self.spoken < PROTOCOL_WITH_DRAINING {
            return Ok(None);
        }
        match self.request(&Request::Draining { seconds })? {
            Response::Exposed { chunks } => Ok(Some(chunks)),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse {
                expected: "exposed",
            }),
        }
    }

    /// Tell the peer which of the chunks it holds for `owner` it may evict to
    /// make room for `owner` ([`Request::MayEvict`]), and learn how many of
    /// them it now holds marked.
    ///
    /// `None`, having sent nothing, when the peer is too old to know the verb
    /// ([`PROTOCOL_WITH_EVICTION`]): an older peer closes the connection on a
    /// request it cannot decode, and the round still needed it. The caller
    /// then keeps counting those copies, which is what happened before this
    /// existed. `Some(0)`, having sent nothing, for an empty list.
    ///
    /// Batched by the caller: the request carries at most `MAX_HAVE_BATCH`
    /// chunks, and a longer one is refused as malformed.
    ///
    /// # Errors
    ///
    /// If the peer refuses -- it does not count this device as one that
    /// speaks for `owner`, or bounds nobody -- or answers something else.
    pub fn may_evict(&mut self, owner: UserId, chunks: Vec<ChunkId>) -> Result<Option<u32>> {
        if self.spoken < PROTOCOL_WITH_EVICTION {
            return Ok(None);
        }
        if chunks.is_empty() {
            return Ok(Some(0));
        }
        match self.request(&Request::MayEvict { owner, chunks })? {
            Response::MayEvict { marked } => Ok(Some(marked)),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse {
                expected: "chunks marked evictable",
            }),
        }
    }

    /// Ask the peer where the account's machines are, as it knows them.
    ///
    /// `None` when the peer cannot tell: it is older than
    /// [`PROTOCOL_WITH_PRESENCES`] (and is not asked), or it refused -- it
    /// keeps no book, or does not count this device as one of its account.
    /// Neither is an error: the coordinator remains the other source.
    ///
    /// The rows are the peer's word until checked, and the caller checks
    /// every one (`itsanas_node::contact::Contact::relayed`). What is bounded
    /// here is only their number and size, past which the answer is refused
    /// whole rather than trimmed: a peer that pads is not one to believe.
    ///
    /// # Errors
    ///
    /// If the connection fails or the answer is not a list of presences.
    pub fn presences(&mut self) -> Result<Option<Vec<Vec<u8>>>> {
        if self.spoken < PROTOCOL_WITH_PRESENCES {
            return Ok(None);
        }
        match self.request(&Request::Presences)? {
            Response::Presences(rows)
                if rows.len() <= crate::protocol::MAX_RELAYED_ROWS
                    && rows
                        .iter()
                        .all(|row| row.len() <= crate::protocol::MAX_RELAYED_ROW_BYTES) =>
            {
                Ok(Some(rows))
            }
            Response::Presences(_) => Err(NetError::UnexpectedResponse {
                expected: "a bounded list of presences",
            }),
            Response::Refused(_) => Ok(None),
            _ => Err(NetError::UnexpectedResponse {
                expected: "presences",
            }),
        }
    }

    /// Present this device's account claim, so a host that bounds what it
    /// stores per account (§8 1c) will store for it.
    ///
    /// `Ok(Some(claim))`: the host took it and answered with its own claim,
    /// which the caller holds the host to when pulling its chunks
    /// ([`crate::session::host_for_bounded`]). `Ok(None)`: the host is older
    /// than [`PROTOCOL_WITH_CLAIMS`] (not asked), bounds nobody, or took it
    /// without answering with its own. A refusal is an error, because the
    /// stores that follow will be refused too and the operator should read
    /// why once, not once per chunk.
    ///
    /// # Errors
    ///
    /// If the host refused the claim, or the connection failed.
    pub fn present_claim(&mut self, claim: Vec<u8>) -> Result<Option<Vec<u8>>> {
        if self.spoken < PROTOCOL_WITH_CLAIMS {
            return Ok(None);
        }
        match self.request(&Request::Claim { claim })? {
            Response::Claim(theirs) if theirs.len() <= crate::protocol::MAX_CLAIM_BYTES => {
                Ok(Some(theirs))
            }
            Response::Stored { .. } => Ok(None),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse { expected: "stored" }),
        }
    }

    /// The claims devices of `owner` presented to this host, unverified:
    /// the caller checks every signature before believing one. Empty from a
    /// host older than [`crate::protocol::PROTOCOL_WITH_ROSTER`] (not asked),
    /// one that keeps none, or one this device has not presented a claim to
    /// for `owner` in this session.
    ///
    /// # Errors
    ///
    /// If the connection failed or the answer was out of bounds.
    pub fn claims(&mut self, owner: UserId) -> Result<Vec<Vec<u8>>> {
        if self.spoken < crate::protocol::PROTOCOL_WITH_ROSTER {
            return Ok(Vec::new());
        }
        match self.request(&Request::Claims { owner })? {
            Response::Claims(claims)
                if claims.len() <= itsanas_store::vault::MAX_CLAIMS_PER_ANSWER
                    && claims
                        .iter()
                        .all(|claim| claim.len() <= crate::protocol::MAX_CLAIM_BYTES) =>
            {
                Ok(claims)
            }
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse { expected: "claims" }),
        }
    }

    /// Ask which of `addresses` the peer lacks.
    pub fn missing_chunks(
        &mut self,
        owner: UserId,
        addresses: Vec<ChunkId>,
    ) -> Result<Vec<ChunkId>> {
        match self.request(&Request::HaveChunks { owner, addresses })? {
            Response::Missing(missing) => Ok(missing),
            Response::Refused(reason) => Err(NetError::Refused(reason)),
            _ => Err(NetError::UnexpectedResponse {
                expected: "missing chunks",
            }),
        }
    }

    /// Offer a sealed chunk for storage, and say what the peer did with it.
    pub fn store_chunk(
        &mut self,
        owner: UserId,
        address: ChunkId,
        sealed: Vec<u8>,
    ) -> Result<Offer> {
        let answer = self.request(&Request::StoreChunk {
            owner,
            address,
            sealed,
        })?;
        Offer::from_response(answer)
    }

    /// Offer a segment for storage, and say what the peer did with it.
    pub fn store_segment(&mut self, envelope: &SegmentEnvelope) -> Result<Offer> {
        let answer = self.request(&Request::StoreSegment {
            envelope: Box::new(envelope.clone()),
        })?;
        Offer::from_response(answer)
    }

    /// Challenge the peer to prove it still holds a chunk.
    ///
    /// `expected` is the sealed bytes the verifier already knows — an owner can
    /// re-derive them, because chunk sealing is deterministic, without keeping
    /// a second copy.
    pub fn challenge(
        &mut self,
        owner: UserId,
        address: ChunkId,
        nonce: [u8; 32],
        expected: &[u8],
    ) -> Result<bool> {
        match self.request(&Request::Challenge {
            owner,
            address,
            nonce,
        })? {
            Response::ChallengeProof(proof) => {
                Ok(crate::protocol::challenge_holds(&nonce, expected, &proof))
            }
            // Away is not lost: the round ends without an audit verdict.
            Response::Refused(why) if why == crate::service::HOSTING_AWAY => {
                Err(NetError::Refused(why))
            }
            Response::Refused(_) => Ok(false),
            _ => Err(NetError::UnexpectedResponse {
                expected: "challenge proof",
            }),
        }
    }
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
    fn a_host_that_has_no_room_for_this_account_is_not_reported_as_rejecting_it() {
        // The two refusals call for opposite answers: a rejection means what
        // was sent is wrong, a full account means its share there is spent,
        // and only the second is answered by letting the host evict spares.
        // Sabotage: drop the `ACCOUNT_FULL` arm in `Offer::from_response`.
        let full = format!(
            "{}: 300 of 300 bytes (pledged 700, proved 0)",
            crate::service::ACCOUNT_FULL
        );
        assert_eq!(
            Offer::from_response(Response::Refused(full)).unwrap(),
            Offer::Refused(Refusal::AccountFull),
            "a full account read as a host that rejects what it is sent"
        );
        assert_eq!(
            Offer::from_response(Response::Refused(
                crate::service::PLEDGE_EXHAUSTED.to_owned()
            ))
            .unwrap(),
            Offer::Refused(Refusal::PledgeFull)
        );
        assert_eq!(
            Offer::from_response(Response::Refused(
                crate::service::UPDATE_REQUIRED.to_owned()
            ))
            .unwrap(),
            Offer::Refused(Refusal::Rejected),
            "a refusal that is not about room read as one that is"
        );
    }

    #[test]
    fn red_team_a_host_waiting_on_its_coordinator_is_not_reported_as_rejecting_the_segments() {
        // tests/route.rs, 2026-10-30: a host of another account refused every
        // offer because no coordinator had confirmed the owner's device, and
        // the owner's log said "a segment that does not verify, or does not
        // follow its chain" -- an investigation went after segments and chains
        // when the answer was `itsanas coordinator --device`. Sabotage: drop
        // any one of the three arms in `Offer::from_response`.
        for (text, want) in [
            (crate::service::UNCONFIRMED, Refusal::Unconfirmed),
            (crate::service::WITHDRAWN, Refusal::Withdrawn),
            (crate::service::CONTRADICTED, Refusal::Contradicted),
        ] {
            assert_eq!(
                Offer::from_response(Response::Refused(text.to_owned())).unwrap(),
                Offer::Refused(want),
                "the owner was told its segments were bad when the host said: {text}"
            );
        }
    }

    #[test]
    fn loopback_still_binds() {
        let server = PeerServer::bind("127.0.0.1:0").unwrap();
        assert!(server.local_addr().unwrap().ip().is_loopback());
    }
}
