//! The client that talks to a coordinator, and the socket deadline it runs
//! under.

use std::io;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use itsanas_crypto::{DeviceId, DeviceKeys};
use itsanas_tls::{Authenticated, Identity, connect};
use itsanas_wire::Connection;
use rustls::ClientConfig;

use super::{COORD_SESSION_BUDGET, IO_TIMEOUT};
use crate::error::{CoordError, Result};
use crate::protocol::{COORD_VERSION, Request, Response};

/// A socket with a deadline on the whole conversation ([`COORD_SESSION_BUDGET`]).
///
/// Each read and write gets the smaller of [`IO_TIMEOUT`] and what is left,
/// and fails once nothing is.
#[derive(Debug)]
pub struct Deadline {
    stream: TcpStream,
    until: Instant,
}

impl Deadline {
    fn left(&self) -> io::Result<Duration> {
        let left = self.until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the coordinator took longer than this client gives it; the path to it may be throttled",
            ));
        }
        Ok(left.min(IO_TIMEOUT))
    }
}

impl itsanas_tls::Socket for Deadline {
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_read_timeout(timeout)
    }
    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.stream.set_write_timeout(timeout)
    }
}

impl Read for Deadline {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.set_read_timeout(Some(self.left()?))?;
        self.stream.read(buf)
    }
}

impl Write for Deadline {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.set_write_timeout(Some(self.left()?))?;
        self.stream.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

/// A client for one coordinator.
#[derive(Debug)]
pub struct CoordClient {
    connection: Connection<itsanas_tls::session::ClientStream<Deadline>>,
    /// The local end of the socket, kept because the TLS wrapper consumes the
    /// `TcpStream` and a caller cannot ask it afterwards.
    ///
    /// It answers "which of this machine's addresses reaches the coordinator",
    /// which is the only sensible thing to announce when a node is configured
    /// to listen on every interface. See `itsanas-cli`'s `reachable_address`.
    local: SocketAddr,
    /// The version agreed with this coordinator ([`crate::protocol::agreed`]).
    version: u16,
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
        Self::connect_within(address, device, expect, COORD_SESSION_BUDGET)
    }

    /// [`Self::connect`], with `budget` for everything this client will do,
    /// in place of [`COORD_SESSION_BUDGET`].
    pub fn connect_within(
        address: impl ToSocketAddrs,
        device: &DeviceKeys,
        expect: Option<DeviceId>,
        budget: Duration,
    ) -> Result<Self> {
        let until = Instant::now() + budget;
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
        let stream = Deadline { stream, until };

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

        let mut client = Self {
            connection,
            local,
            version: COORD_VERSION,
        };

        match client.ask(&Request::Hello {
            version: COORD_VERSION,
        })? {
            // A coordinator older than this build answers at its own version,
            // which this build still speaks when it is in the window.
            Response::Welcome { version } if crate::protocol::agreed(version) == Some(version) => {
                client.version = version;
                Ok(client)
            }
            Response::Refused(why) => Err(CoordError::Transport(why)),
            other => Err(CoordError::Transport(format!(
                "expected a version agreement, got {other:?}"
            ))),
        }
    }

    /// The version this coordinator and this build agreed to speak: a
    /// message newer than it is not sent, since the coordinator would hang up.
    #[must_use]
    pub const fn version(&self) -> u16 {
        self.version
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
