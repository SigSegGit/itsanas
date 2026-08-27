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
use itsanas_tls::{Authenticated, Identity};
use itsanas_wire::Connection;

use crate::{
    error::{NetError, Result},
    protocol::{Head, PROTOCOL_VERSION, Request, Response},
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
        })
    }

    /// The address actually bound, which matters when port 0 was requested.
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.listener.local_addr()?)
    }

    /// Accept one connection and serve it until the peer closes.
    pub fn serve_one(&self, service: &PeerService<'_>, device: &DeviceKeys) -> Result<()> {
        let (stream, _) = self.listener.accept()?;
        self.serve_connection(stream, service, device)
    }

    /// Serve until `shutdown` is set.
    ///
    /// One connection at a time, deliberately: a node talks to a handful of
    /// peers, and serialising them means the store is never touched
    /// concurrently by two peers doing unrelated things.
    pub fn serve_until(
        &self,
        service: &PeerService<'_>,
        device: &DeviceKeys,
        shutdown: &AtomicBool,
    ) -> Result<()> {
        self.listener.set_nonblocking(true)?;

        while !shutdown.load(Ordering::Relaxed) {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false)?;
                    // One peer misbehaving must not stop the server. A failed
                    // handshake is the most ordinary thing on a public port.
                    let _ = self.serve_connection(stream, service, device);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => return Err(error.into()),
            }
        }

        Ok(())
    }

    fn serve_connection(
        &self,
        stream: TcpStream,
        service: &PeerService<'_>,
        device: &DeviceKeys,
    ) -> Result<()> {
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;

        let Authenticated {
            peer,
            mut connection,
        } = itsanas_tls::accept(&self.config, device, stream)?;
        let _ = peer;

        loop {
            let request: Request = match connection.receive()? {
                Some(request) => request,
                // A clean close between requests is how a peer says goodbye.
                None => return Ok(()),
            };

            let response = service.handle(&request)?;
            connection.send(&response)?;
        }
    }
}

/// A connection to a peer, from the asking side.
pub struct PeerClient {
    connection: Connection<itsanas_tls::session::ClientStream<TcpStream>>,
    peer_device: DeviceId,
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
        };

        // Version negotiation after authentication, so an incompatible peer
        // fails legibly rather than on some later message.
        match client.request(&Request::Hello {
            protocol: PROTOCOL_VERSION,
            device: device.device_id(),
            owner,
        })? {
            Response::Hello { protocol, .. } => {
                if protocol != PROTOCOL_VERSION {
                    return Err(NetError::UnsupportedProtocolVersion {
                        found: protocol,
                        supported: PROTOCOL_VERSION,
                    });
                }
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

    /// Offer a sealed chunk for storage. Returns whether the peer took it.
    pub fn store_chunk(
        &mut self,
        owner: UserId,
        address: ChunkId,
        sealed: Vec<u8>,
    ) -> Result<bool> {
        match self.request(&Request::StoreChunk {
            owner,
            address,
            sealed,
        })? {
            Response::Stored { accepted } => Ok(accepted),
            Response::Refused(_) => Ok(false),
            _ => Err(NetError::UnexpectedResponse { expected: "stored" }),
        }
    }

    /// Offer a segment for storage. Returns whether the peer took it.
    pub fn store_segment(&mut self, envelope: &SegmentEnvelope) -> Result<bool> {
        match self.request(&Request::StoreSegment {
            envelope: Box::new(envelope.clone()),
        })? {
            Response::Stored { accepted } => Ok(accepted),
            Response::Refused(_) => Ok(false),
            _ => Err(NetError::UnexpectedResponse { expected: "stored" }),
        }
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
