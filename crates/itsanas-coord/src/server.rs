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
//! - a read timeout, so a connection that opens and says nothing costs one slot
//!   for a few seconds instead of forever;
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
use itsanas_tls::{Authenticated, Identity, accept, connect};
use itsanas_wire::Connection;
use rustls::{ClientConfig, ServerConfig};

use crate::directory::{Admission, Directory};
use crate::error::{CoordError, Result};
use crate::protocol::{COORD_VERSION, MAX_REQUESTS_PER_CONNECTION, Request, Response};
use crate::service::{CoordService, EscrowLimiter};

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
}

impl CoordServer {
    /// Bind to `address`.
    pub fn bind(address: impl ToSocketAddrs) -> Result<Self> {
        let listener = TcpListener::bind(address).map_err(CoordError::from)?;
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

        Ok(Self { listener, config })
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

        let live = AtomicUsize::new(0);
        let service = CoordService::admitting(directory, admission);

        // One limiter for the whole server, not one per connection. A
        // per-connection budget is no budget at all: an attacker reconnects and
        // gets a fresh one, which costs them a handshake and buys them
        // everything. Shared state behind a mutex is the price of the rate
        // limit meaning anything, and it is only touched on escrow fetches.
        let limiter = Mutex::new(EscrowLimiter::new());

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

                        if live.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                            // Closing beats queueing: a queue is a slower way
                            // to run out of memory, and the caller finds out
                            // now rather than after a timeout.
                            drop(stream);
                            on_event("at the connection limit; refused one");
                            continue;
                        }

                        live.fetch_add(1, Ordering::Relaxed);
                        let config = Arc::clone(&self.config);
                        let live = &live;
                        let service = &service;
                        let limiter = &limiter;
                        scope.spawn(move || {
                            let outcome = serve_one(stream, &config, device, service, limiter);
                            live.fetch_sub(1, Ordering::Relaxed);
                            let _ = (outcome, from);
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

/// One connection, from handshake to close.
fn serve_one(
    stream: TcpStream,
    config: &Arc<ServerConfig>,
    device: &DeviceKeys,
    service: &CoordService<'_>,
    limiter: &Mutex<EscrowLimiter>,
) -> Result<()> {
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(CoordError::from)?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(CoordError::from)?;

    let Authenticated {
        peer: caller,
        mut connection,
    } = accept(config, device, stream)
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

        let response = {
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
        let target = address
            .to_socket_addrs()
            .map_err(CoordError::from)?
            .next()
            .ok_or_else(|| CoordError::Transport("no address to connect to".to_owned()))?;

        let stream = TcpStream::connect_timeout(&target, IO_TIMEOUT).map_err(CoordError::from)?;
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
