//! Moving frames between two machines.
//!
//! # Read this before exposing a node to a network you do not control
//!
//! **This transport is plain TCP. It is not encrypted and it does not
//! authenticate the peer.**
//!
//! What that does *not* cost: confidentiality of your data. Everything that
//! crosses this wire was already sealed by [`itsanas_crypto`] — chunk bodies
//! are ciphertext, segment bodies are ciphertext, and a segment envelope is
//! signed, so a man in the middle can neither read a payload nor forge one that
//! a peer will accept.
//!
//! What it does cost: **metadata**. A passive observer on the path sees chunk
//! identifiers, object sizes, and timing. The threat model already grants a
//! *host* all three — a host stores the chunks, so of course it knows their
//! addresses and sizes — but it does not grant them to an arbitrary network
//! between two of your own machines. An observer who records chunk ids can tell
//! when you touch the same file again, and can correlate two of your devices.
//!
//! Because of that, [`PeerServer::bind`] refuses a non-loopback address unless
//! the caller explicitly opts in. QUIC with TLS and device-key authentication
//! is the remaining M4 work; until it lands, run this over loopback, a VPN, or
//! an SSH tunnel.
//!
//! # Why blocking sockets and threads
//!
//! A node talks to a handful of peers, not ten thousand. A thread per
//! connection costs a few megabytes and buys code that can be read top to
//! bottom. Async would buy scalability this design does not need, at the price
//! of an executor in every signature. The framing and protocol layers are
//! transport-agnostic, so swapping this for QUIC later touches nothing above
//! it.

use std::{
    io::{Read as _, Write as _},
    net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use itsanas_crypto::{ChunkId, DeviceId, ObjectId, UserId};
use itsanas_store::SegmentEnvelope;

use crate::{
    error::{NetError, Result},
    protocol::{Head, PROTOCOL_VERSION, Request, Response},
    service::PeerService,
    wire::{self, FrameReader},
};

/// How long a read or write may stall before the connection is abandoned.
///
/// Without this a single peer that opens a connection and then says nothing
/// holds a thread forever, and enough of them exhaust the node.
pub const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Whether a listener may bind somewhere other than loopback.
///
/// Spelled out as an enum rather than a `bool` so that the decision is legible
/// at the call site: `Exposure::LocalOnly` and `Exposure::Anywhere` say what
/// they mean, where `true` would not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exposure {
    /// Refuse to bind anything but a loopback address.
    LocalOnly,
    /// Bind whatever was asked for, accepting the metadata exposure described
    /// in the module docs.
    Anywhere,
}

/// Accepts peer connections and answers them from a [`PeerService`].
#[derive(Debug)]
pub struct PeerServer {
    listener: TcpListener,
}

impl PeerServer {
    /// Bind a listener.
    ///
    /// Refuses a non-loopback address under [`Exposure::LocalOnly`], because
    /// this transport exposes metadata to anyone on the path.
    pub fn bind(address: impl ToSocketAddrs, exposure: Exposure) -> Result<Self> {
        let resolved: Vec<SocketAddr> = address.to_socket_addrs()?.collect();

        if exposure == Exposure::LocalOnly
            && let Some(public) = resolved.iter().find(|a| !a.ip().is_loopback())
        {
            return Err(NetError::Refused(format!(
                "refusing to bind {public}: this transport is unencrypted and \
                 exposes chunk identifiers and sizes to anyone on the path. \
                 Pass Exposure::Anywhere to override, and prefer a VPN or an \
                 SSH tunnel until QUIC lands"
            )));
        }

        Ok(Self {
            listener: TcpListener::bind(resolved.as_slice())?,
        })
    }

    /// The address actually bound, which matters when port 0 was requested.
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.listener.local_addr()?)
    }

    /// Accept one connection and serve it until the peer closes.
    pub fn serve_one(&self, service: &PeerService<'_>) -> Result<()> {
        let (stream, _) = self.listener.accept()?;
        serve_connection(stream, service)
    }

    /// Serve until `shutdown` is set.
    ///
    /// One connection at a time, deliberately: a node talks to a handful of
    /// peers, and serialising them means the store is never touched
    /// concurrently by two peers doing unrelated things.
    pub fn serve_until(&self, service: &PeerService<'_>, shutdown: &AtomicBool) -> Result<()> {
        self.listener.set_nonblocking(true)?;

        while !shutdown.load(Ordering::Relaxed) {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false)?;
                    // One peer misbehaving must not stop the server. Log-worthy,
                    // not fatal.
                    let _ = serve_connection(stream, service);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => return Err(error.into()),
            }
        }

        Ok(())
    }
}

fn serve_connection(stream: TcpStream, service: &PeerService<'_>) -> Result<()> {
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))?;

    let mut connection = Connection::new(stream);

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

/// One end of a framed conversation.
#[derive(Debug)]
struct Connection {
    stream: TcpStream,
    reader: FrameReader,
}

impl Connection {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            reader: FrameReader::new(),
        }
    }

    fn send<T: serde::Serialize>(&mut self, message: &T) -> Result<()> {
        let frame = wire::encode(message)?;
        self.stream.write_all(&frame)?;
        self.stream.flush()?;
        Ok(())
    }

    /// Read until one whole message has arrived.
    ///
    /// `Ok(None)` means the peer closed cleanly between messages. A close
    /// *mid-message* is an error, because silently discarding a partial frame
    /// would let a peer truncate a response and have it treated as complete.
    fn receive<T: for<'de> serde::Deserialize<'de>>(&mut self) -> Result<Option<T>> {
        loop {
            if let Some(message) = self.reader.next_message()? {
                return Ok(Some(message));
            }

            let mut buffer = [0u8; 16 * 1024];
            match self.stream.read(&mut buffer)? {
                0 => {
                    return if self.reader.buffered() == 0 {
                        Ok(None)
                    } else {
                        Err(NetError::ConnectionClosed)
                    };
                }
                read => self.reader.push(&buffer[..read]),
            }
        }
    }
}

/// A connection to a peer, from the asking side.
#[derive(Debug)]
pub struct PeerClient {
    connection: Connection,
    peer_device: DeviceId,
}

impl PeerClient {
    /// Connect and complete the opening exchange.
    pub fn connect(address: impl ToSocketAddrs, device: DeviceId, owner: UserId) -> Result<Self> {
        let address = address
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| NetError::Refused("no address to connect to".to_owned()))?;

        let stream = TcpStream::connect_timeout(&address, IO_TIMEOUT)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        stream.set_nodelay(true)?;

        let mut client = Self {
            connection: Connection::new(stream),
            peer_device: DeviceId::from_bytes([0; 32]),
        };

        // Version negotiation before anything else, so an incompatible peer
        // fails immediately and legibly rather than on some later message.
        match client.request(&Request::Hello {
            protocol: PROTOCOL_VERSION,
            device,
            owner,
        })? {
            Response::Hello { protocol, device } => {
                if protocol != PROTOCOL_VERSION {
                    return Err(NetError::UnsupportedProtocolVersion {
                        found: protocol,
                        supported: PROTOCOL_VERSION,
                    });
                }
                client.peer_device = device;
            }
            Response::Refused(reason) => return Err(NetError::Refused(reason)),
            _ => {
                return Err(NetError::UnexpectedResponse { expected: "hello" });
            }
        }

        Ok(client)
    }

    /// The device on the other end, as it identified itself.
    #[must_use]
    pub const fn peer_device(&self) -> DeviceId {
        self.peer_device
    }

    /// Send a request and wait for its response.
    pub fn request(&mut self, request: &Request) -> Result<Response> {
        self.connection.send(request)?;
        self.connection.receive()?.ok_or(NetError::ConnectionClosed)
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
    fn binding_a_public_address_is_refused_by_default() {
        // The default has to be the safe one. Someone bringing up a node on the
        // Pi will type an address and press enter, and the failure mode of
        // getting this wrong is silent metadata exposure, which nobody notices.
        let error = PeerServer::bind("0.0.0.0:0", Exposure::LocalOnly)
            .expect_err("binding a wildcard address should be refused");

        assert!(
            matches!(&error, NetError::Refused(reason) if reason.contains("unencrypted")),
            "the refusal did not explain why: {error}"
        );
    }

    #[test]
    fn loopback_binds_without_an_override() {
        let server = PeerServer::bind("127.0.0.1:0", Exposure::LocalOnly).unwrap();
        assert!(server.local_addr().unwrap().ip().is_loopback());
    }

    #[test]
    fn an_explicit_override_allows_a_public_bind() {
        let server = PeerServer::bind("0.0.0.0:0", Exposure::Anywhere)
            .expect("an explicit override must work, or the escape hatch is a lie");
        assert_eq!(server.local_addr().unwrap().ip().to_string(), "0.0.0.0");
    }
}
