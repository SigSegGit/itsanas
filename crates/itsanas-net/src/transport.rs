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
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use itsanas_crypto::{ChunkId, DeviceId, DeviceKeys, ObjectId, UserId};
use itsanas_store::SegmentEnvelope;
use itsanas_tls::{
    Authenticated, Identity,
    limits::{ConnectionLimits, Slot},
};
use itsanas_wire::Connection;

use crate::{
    error::{NetError, Result},
    protocol::{
        Head, MIN_PROTOCOL_VERSION, PROTOCOL_VERSION, PROTOCOL_WITH_CHUNK_SUMMARY,
        PROTOCOL_WITH_DROP_NOTICES, Request, Response,
    },
    service::PeerService,
};

/// How long a read or write may stall before the connection is abandoned.
///
/// Without this a single peer that opens a connection and then says nothing
/// holds a thread forever, and enough of them exhaust the node.
pub const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Accepts peer connections and answers them from a [`PeerService`].
#[derive(Debug)]
pub struct PeerServer {
    listener: TcpListener,
    config: Arc<rustls_config::Server>,
    handshake_deadline: Duration,
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
            listener: TcpListener::bind(resolved.as_slice())?,
            config: identity.server_config()?,
            handshake_deadline: HANDSHAKE_DEADLINE,
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

        loop {
            let request: Request = match connection.receive()? {
                Some(request) => request,
                // A clean close between requests is how a peer says goodbye.
                None => return Ok(()),
            };

            // Read before refusing, never the other way round: closing a socket
            // with unread data makes Windows reset it, which discards the
            // refusal and leaves the caller reading "connection aborted".
            if !within_limit {
                connection.send(&Response::Refused(TOO_MANY_FROM_DEVICE.to_owned()))?;
                return Ok(());
            }

            // `peer` is who TLS proved is on the other end, and it used to be
            // dropped on the line above with `let _ = peer;`. Answering
            // `Request::Hosted` needs it: recording that somebody holds a chunk
            // is worthless if you cannot say who.
            let response = service.handle(&request, peer)?;
            connection.send(&response)?;

            // Between requests, so a peer that keeps asking cannot keep a
            // stopping daemon alive.
            if shutdown.load(Ordering::Relaxed) {
                return Ok(());
            }
        }
    }
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
}

impl Offer {
    fn from_response(answer: Response) -> Result<Self> {
        match answer {
            Response::Stored { accepted: true } => Ok(Self::Taken),
            Response::Stored { accepted: false } => Ok(Self::AlreadyHeld),
            Response::Refused(reason) if reason == crate::service::PLEDGE_EXHAUSTED => {
                Ok(Self::Refused(Refusal::PledgeFull))
            }
            Response::Refused(_) => Ok(Self::Refused(Refusal::Rejected)),
            _ => Err(NetError::UnexpectedResponse { expected: "stored" }),
        }
    }
}

/// A connection to a peer, from the asking side.
pub struct PeerClient {
    connection: Connection<itsanas_tls::session::ClientStream<TcpStream>>,
    peer_device: DeviceId,
    /// The version both sides agreed to speak.
    ///
    /// Not a formality: a verb added after that version does not exist for this
    /// peer, and sending it would fail the connection rather than the feature.
    /// Every caller of a newer request asks this first.
    spoken: u16,
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
        let address = address
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| NetError::Refused("no address to connect to".to_owned()))?;

        let stream = TcpStream::connect_timeout(&address, IO_TIMEOUT)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        stream.set_nodelay(true)?;

        let identity = Identity::generate()?;
        let Authenticated { peer, connection } =
            itsanas_tls::connect(&identity.client_config()?, device, stream, expect)?;

        let mut client = Self {
            connection,
            peer_device: peer,
            spoken: MIN_PROTOCOL_VERSION,
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
    fn binding_a_public_address_no_longer_needs_an_override() {
        // It used to be refused, because the transport leaked chunk identifiers
        // and sizes to anyone on the path. TLS closed that, so keeping the
        // refusal would be cargo cult.
        let server = PeerServer::bind("0.0.0.0:0").expect("a public bind should work");
        assert_eq!(server.local_addr().unwrap().ip().to_string(), "0.0.0.0");
    }

    #[test]
    fn loopback_still_binds() {
        let server = PeerServer::bind("127.0.0.1:0").unwrap();
        assert!(server.local_addr().unwrap().ip().is_loopback());
    }
}
