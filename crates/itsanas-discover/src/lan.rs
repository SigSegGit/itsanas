//! The UDP socket that carries announcements on the local network.
//!
//! # Why broadcast rather than mDNS
//!
//! mDNS would be the conventional answer and it would mean a dependency that
//! parses a text-based, variable-length, cache-bearing protocol from
//! unauthenticated packets. This crate's whole security argument is that the
//! only unsolicited parser in the project is 147 fixed bytes with a signature
//! over it, and adding mDNS would replace that argument with somebody else's.
//!
//! Broadcast also has a practical advantage on the target hardware: it needs no
//! multicast group membership, which is the part that most often fails silently
//! on consumer routers and inside VM network bridges.
//!
//! The cost is real and worth stating: IPv4 global broadcast leaves by the
//! default route only, so a machine with several interfaces announces itself on
//! one of them. That is fine for a household and wrong for a campus, and it is
//! why the coordinator exists for anything beyond one network.
//!
//! # What is visible to anyone on the same network
//!
//! That ITSaNAS is running here, this device's public key, the user id it
//! claims, and the port. That is a deliberate trade: it is the minimum needed
//! for two machines in one house to find each other with nothing configured,
//! and it is the same order of information a device already leaks by opening a
//! listening port. It is not sent beyond the local link.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use itsanas_crypto::{DeviceKeys, UserId};

use crate::beacon::{Announcement, BEACON_LEN};
use crate::error::{DiscoverError, Result};

/// The UDP port announcements are sent to and heard on.
///
/// Registered with nobody, and chosen to sit clear of Syncthing's 21027 so that
/// two tools on one machine do not spend their time discarding each other's
/// traffic.
pub const DEFAULT_PORT: u16 = 21037;

/// How often a node should announce itself.
///
/// Thirty seconds is a compromise with one side that matters more than the
/// other. Too slow and a machine that just woke is invisible for as long as the
/// interval, which is the moment a user is watching. Too fast and a laptop is
/// kept awake sending packets nobody needs, which is the failure that gets the
/// daemon uninstalled. At this rate the cost is roughly five kilobytes a day.
pub const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(30);

/// How long an unheard device stays in the table before being forgotten.
///
/// Six missed announcements. Generous on purpose: a brief wireless drop is far
/// more common than a machine genuinely leaving, and forgetting an address
/// costs a slower reconnection.
pub const EXPIRY: Duration = Duration::from_secs(30 * 6);

/// Room for a packet larger than any valid announcement.
///
/// Deliberately more than [`BEACON_LEN`] so that an oversized datagram is seen
/// as the wrong length and rejected, rather than being silently truncated to
/// exactly the right size by the kernel and then parsed.
const RECEIVE_BUFFER: usize = 512;

/// Seconds since the Unix epoch, or zero if the clock is before it.
///
/// Zero is a legitimate answer here rather than an error: a Raspberry Pi with
/// no real-time clock genuinely believes it is 1970, and it still needs to be
/// findable. Nothing in discovery makes a decision from this value.
#[must_use]
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A socket for announcing this node and hearing others.
#[derive(Debug)]
pub struct Lan {
    socket: UdpSocket,
    targets: Vec<SocketAddr>,
}

impl Lan {
    /// Listen for announcements on `port`, and broadcast to the same port.
    ///
    /// Binding fails if another process on this machine already holds the port,
    /// which is the honest outcome: two ITSaNAS nodes on one machine is a
    /// configuration mistake, and silently disabling discovery for the second
    /// one would be much harder to diagnose than a refusal at start-up.
    pub fn bind(port: u16) -> Result<Self> {
        Self::open(port, port)
    }

    /// Bind an ephemeral port but announce to the discovery port.
    ///
    /// For a process that wants to be *heard* without taking the port a daemon
    /// needs — a diagnostic, or a second node on a machine that already has one.
    pub fn announcer(announce_to: u16) -> Result<Self> {
        Self::open(0, announce_to)
    }

    /// Listen on `listen`, broadcast to `announce_to`.
    ///
    /// The two are separate because they were once the same value and it was a
    /// bug: binding an ephemeral port made the broadcast target port zero, so
    /// every announcement was sent precisely nowhere, silently and successfully.
    fn open(listen: u16, announce_to: u16) -> Result<Self> {
        if announce_to == 0 {
            return Err(DiscoverError::NoPort);
        }
        let socket = UdpSocket::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), listen))?;
        socket.set_broadcast(true)?;
        Ok(Self {
            socket,
            targets: vec![SocketAddr::new(
                IpAddr::V4(Ipv4Addr::BROADCAST),
                announce_to,
            )],
        })
    }

    /// Bind to an exact address and send only where told.
    ///
    /// Used by tests, and by anyone who has to pin discovery to one interface.
    /// Broadcast is not enabled, so this cannot reach a whole network by
    /// accident.
    pub fn bind_to(address: SocketAddr, targets: Vec<SocketAddr>) -> Result<Self> {
        if targets.iter().any(|target| target.port() == 0) {
            return Err(DiscoverError::NoPort);
        }
        let socket = UdpSocket::bind(address)?;
        Ok(Self { socket, targets })
    }

    /// The address this socket is actually bound to.
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.socket.local_addr()?)
    }

    /// Where announcements are sent.
    #[must_use]
    pub fn targets(&self) -> &[SocketAddr] {
        &self.targets
    }

    /// Announce this device, serving the peer protocol on `service_port`.
    ///
    /// A send failure is returned rather than swallowed, but a caller in a
    /// daemon loop should log and continue: an interface that is down while a
    /// laptop moves between networks is the normal case, not a fault.
    pub fn announce(&self, keys: &DeviceKeys, owner: UserId, service_port: u16) -> Result<()> {
        let packet = Announcement::seal(keys, owner, service_port, now_unix());
        for target in &self.targets {
            self.socket.send_to(&packet, target)?;
        }
        Ok(())
    }

    /// Wait up to `timeout` for one announcement.
    ///
    /// `Ok(None)` means the timeout elapsed with nothing arriving, which is the
    /// ordinary state of a quiet network and not an error. Anything that did
    /// arrive and failed to verify comes back as an error naming why, so that a
    /// node which never finds its neighbours can be told whether nothing is
    /// arriving, something foreign is, or something of ours is failing to
    /// verify — three different things to go and fix.
    pub fn receive(&self, timeout: Duration) -> Result<Option<(Announcement, IpAddr)>> {
        self.socket.set_read_timeout(Some(timeout))?;

        let mut buffer = [0u8; RECEIVE_BUFFER];
        let (len, from) = match self.socket.recv_from(&mut buffer) {
            Ok(received) => received,
            // A read timeout is reported as WouldBlock on Unix and TimedOut on
            // Windows. Both mean the same thing and neither is a failure.
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(None);
            }
            // Windows returns this when a datagram was larger than the buffer.
            // It is an oversized packet, which is exactly what a wrong-length
            // rejection is for, so report it as one rather than as an I/O fault.
            Err(error) if error.raw_os_error() == Some(10040) => {
                return Err(DiscoverError::WrongLength {
                    got: RECEIVE_BUFFER + 1,
                    expected: BEACON_LEN,
                });
            }
            Err(error) => return Err(error.into()),
        };

        let announcement = Announcement::parse(&buffer[..len])?;
        Ok(Some((announcement, from.ip())))
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use itsanas_crypto::ID_LEN;

    use super::*;

    fn loopback(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    fn owner() -> UserId {
        UserId::from_bytes([3u8; ID_LEN])
    }

    /// A listener on an ephemeral loopback port, and a sender aimed at it.
    ///
    /// Deliberately not broadcast: two sockets on one machine cannot share a
    /// port without `SO_REUSEADDR`, and taking a dependency purely to make a
    /// test resemble production would be a poor trade. Everything below the
    /// destination address is the production path.
    fn pair() -> (Lan, Lan) {
        let listener = Lan::bind_to(loopback(0), Vec::new()).unwrap();
        let port = listener.local_addr().unwrap().port();
        let sender = Lan::bind_to(loopback(0), vec![loopback(port)]).unwrap();
        (listener, sender)
    }

    #[test]
    fn an_announcement_crosses_a_real_socket_and_verifies() {
        let (listener, sender) = pair();
        let keys = DeviceKeys::generate().unwrap();

        sender.announce(&keys, owner(), 9797).unwrap();

        let (heard, from) = listener
            .receive(Duration::from_secs(5))
            .unwrap()
            .expect("the announcement did not arrive");

        assert_eq!(heard.device, keys.device_id());
        assert_eq!(heard.owner, owner());
        assert_eq!(heard.port, 9797);
        assert!(from.is_loopback());
    }

    #[test]
    fn a_quiet_network_times_out_rather_than_blocking_forever() {
        // A daemon polls this in a loop. If a silent network blocked, the
        // daemon would never reach its next scheduled task and would look
        // hung rather than idle.
        let listener = Lan::bind_to(loopback(0), Vec::new()).unwrap();
        assert!(
            listener
                .receive(Duration::from_millis(150))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn foreign_traffic_on_the_port_is_reported_as_foreign_not_as_a_failure() {
        // Something else may be using the port. That is not a discovery fault
        // and must not be logged as one, or a busy network floods the log and
        // hides the failure that matters.
        let (listener, sender) = pair();
        sender
            .socket
            .send_to(b"GET / HTTP/1.1\r\n", sender.targets[0])
            .unwrap();

        let error = listener.receive(Duration::from_secs(5)).unwrap_err();
        assert!(error.is_foreign_traffic(), "unexpected error: {error}");
    }

    #[test]
    fn a_tampered_announcement_is_refused_at_the_socket() {
        let (listener, sender) = pair();
        let keys = DeviceKeys::generate().unwrap();

        let mut packet = Announcement::seal(&keys, owner(), 9797, now_unix());
        packet[100] ^= 0x40;
        sender.socket.send_to(&packet, sender.targets[0]).unwrap();

        let error = listener.receive(Duration::from_secs(5)).unwrap_err();
        assert!(matches!(error, DiscoverError::BadSignature));
        assert!(!error.is_foreign_traffic());
    }

    #[test]
    fn an_oversized_datagram_is_rejected_rather_than_truncated_into_a_valid_one() {
        // The receive buffer is larger than a valid announcement precisely so
        // that a long packet fails the length check. With an exact-sized
        // buffer the kernel would trim the excess and hand up something that
        // parses, which is how a padded packet smuggles data past a parser.
        let (listener, sender) = pair();
        let keys = DeviceKeys::generate().unwrap();

        let mut packet = Announcement::seal(&keys, owner(), 9797, now_unix()).to_vec();
        packet.extend_from_slice(&[0xAA; 64]);
        sender.socket.send_to(&packet, sender.targets[0]).unwrap();

        let error = listener.receive(Duration::from_secs(5)).unwrap_err();
        assert!(matches!(error, DiscoverError::WrongLength { .. }));
    }

    #[test]
    fn an_ephemeral_bind_still_announces_to_the_discovery_port() {
        // Found by running it, not by a test. `bind(0)` used the same number
        // for the local port and the broadcast target, so a diagnostic that
        // asked for an ephemeral port sent every announcement to
        // 255.255.255.255:0 — which the operating system accepts and delivers
        // to nobody. It reported five successful sends and reached nothing.
        let lan = Lan::announcer(DEFAULT_PORT).unwrap();
        assert_ne!(lan.local_addr().unwrap().port(), 0);
        assert_eq!(lan.targets()[0].port(), DEFAULT_PORT);
    }

    #[test]
    fn announcing_to_port_zero_is_refused_rather_than_sent_into_the_void() {
        assert!(matches!(Lan::announcer(0), Err(DiscoverError::NoPort)));
        assert!(matches!(
            Lan::bind_to(loopback(0), vec![loopback(0)]),
            Err(DiscoverError::NoPort)
        ));
    }

    #[test]
    fn a_broadcasting_socket_asks_the_kernel_for_broadcast() {
        // Without SO_BROADCAST the socket sends nothing at all to
        // 255.255.255.255, and does so without an error: discovery would
        // silently never work while every send reported success.
        let lan = Lan::announcer(DEFAULT_PORT).unwrap();
        assert!(lan.socket.broadcast().unwrap());
        assert_eq!(lan.targets().len(), 1);
        assert!(lan.targets()[0].ip().is_ipv4());
        assert!(lan.targets()[0].ip().to_string().ends_with("255"));
    }

    #[test]
    fn two_nodes_on_one_machine_refuse_to_share_a_port() {
        // Better a refusal at start-up than a second node whose discovery
        // quietly never works.
        let first = Lan::bind_to(loopback(0), Vec::new()).unwrap();
        let port = first.local_addr().unwrap().port();
        assert!(Lan::bind_to(loopback(port), Vec::new()).is_err());
    }

    #[test]
    fn the_announce_interval_is_not_expensive_to_leave_running() {
        // An acceptance criterion, not a preference: the first version that
        // keeps a laptop awake gets uninstalled. 147 bytes every 30 seconds is
        // under half a megabyte a day.
        let per_day = (86_400 / ANNOUNCE_INTERVAL.as_secs()) * BEACON_LEN as u64;
        assert!(per_day < 512 * 1024, "{per_day} bytes a day is too much");
        assert!(
            EXPIRY > ANNOUNCE_INTERVAL * 3,
            "one lost packet must not forget a peer"
        );
    }
}
