//! Being reachable, and reaching, from a network that is not the one at home.
//!
//! Two sockets' worth of policy, kept here rather than in either transport
//! because the node and the coordinator need exactly the same of it and the
//! coordinator must not grow a dependency on the peer protocol to get it.
//!
//! Both decisions come from the same fact: a member is only usable from outside
//! their own LAN if something can be dialled, and the cheapest thing that can
//! be dialled -- the only one that costs nothing per machine and asks no
//! third party for anything -- is a public IPv6 address, which every French
//! subscriber already has. That works only if a node listens on IPv6 and only
//! if dialling a name tries the IPv6 record *and* the IPv4 one.

use std::{
    io,
    net::{IpAddr, SocketAddr, TcpListener, TcpStream},
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};

/// How long a TCP handshake may take before that address is given up on.
///
/// Short, and deliberately not the timeout a transfer gets. The two are
/// different waits: a transfer may legitimately take thirty seconds, while a
/// SYN unanswered after five is not going to be answered. The difference is
/// what a node away from home spends every round -- a laptop at a friend's
/// house is handed its own account's addresses, every one of them on a network
/// it has left, and at thirty seconds each those dead dials ate two minutes of
/// a five-minute round before anything that could answer had been tried.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether an address is one that only its own network can dial.
///
/// Two callers, which is why it lives here rather than beside either of them: a
/// node orders the addresses it was handed so the ones that can answer are
/// tried first, and a coordinator refuses to *probe* one, because probing an
/// address a caller named is a scanner and probing a private one is a scanner
/// pointed at somebody's own network.
///
/// A name is never private: it is the thing somebody set up precisely so others
/// could reach them, and what it resolves to is a question for the resolver.
///
/// **Where this is deliberately wrong:** an overlay network -- Tailscale,
/// Nebula, any `WireGuard` mesh -- hands out addresses in `100.64.0.0/10`, and
/// inside such a network they are reachable from anywhere. Counted private
/// here, because the same range is what a mobile carrier hands a phone behind
/// CGNAT, where it is reachable from nothing. Anybody running this over an
/// overlay gives the node a **name** for that address, which is what names are
/// for and what this function trusts.
#[must_use]
pub fn is_private_address(address: &str) -> bool {
    // A machine parked on a relay (`relay:<device>`, §8 0v) is not dialable
    // from anywhere: it ranks after every direct address and counts as
    // nothing `doctor` could dial.
    if address.starts_with("relay:") {
        return true;
    }
    // A list (`AddressList`) is private if any entry is: the coordinator's
    // probe refuses it whole, and a dialler ranks it after the public ones.
    address
        .split(',')
        .filter_map(|entry| entry.trim().parse::<SocketAddr>().ok())
        .any(|parsed| is_private_ip(parsed.ip()))
}

/// The same question about an address that is already parsed.
///
/// What the listener asks of every caller: a connection from a public address
/// is proof that this machine is reachable from outside its own network, and
/// that proof is free -- it arrived anyway.
#[must_use]
pub fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                // 100.64.0.0/10, carrier-grade NAT: what a mobile network and
                // an overlay hand out. `Ipv4Addr::is_shared` says this and is
                // still unstable.
                || (a == 100 && (64..128).contains(&b))
        }
        IpAddr::V6(v6) => {
            // An IPv4 caller arriving on a dual-stack socket is that IPv4
            // address, and asking the question of `::ffff:192.168.1.10` as an
            // IPv6 address answers "public", which is the opposite of the
            // truth.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_ip(IpAddr::V4(v4));
            }
            let [a, b, ..] = v6.octets();
            v6.is_loopback()
                || v6.is_unspecified()
                // fc00::/7 unique local, and fe80::/10 link local. Both are
                // `Ipv6Addr` methods that are still unstable.
                || (a & 0xfe) == 0xfc
                || (a == 0xfe && (b & 0xc0) == 0x80)
        }
    }
}

/// How many connections a listener may have waiting to be accepted.
///
/// What `TcpListener::bind` asks for, kept the same so that building the socket
/// by hand does not quietly change the behaviour of a node under load.
#[cfg(not(windows))]
const BACKLOG: i32 = 128;

/// Bind a listener that answers on both IP versions where the system allows it.
///
/// `0.0.0.0:9797` is the default and it is IPv4 only. That was invisible while
/// every member was on one LAN, and it is the whole question once they are not:
/// a home reachable from outside without asking anybody for anything is usually
/// reached over IPv6, and a node that does not listen on IPv6 cannot be.
///
/// So an unspecified IPv4 address is served by an unspecified IPv6 socket with
/// `IPV6_V6ONLY` cleared, which also accepts IPv4 callers -- they arrive as
/// `::ffff:a.b.c.d`, and [`crate::limits::counted_as`] turns those back into the
/// IPv4 address they are, so the per-address cap still counts one caller once.
///
/// Falling back to the plain IPv4 socket is not a formality: a machine with
/// IPv6 disabled in the kernel cannot bind `[::]` at all, and a node that
/// refused to start there would be a regression for its owner in exchange for a
/// reachability that machine cannot have anyway.
///
/// An address that is not unspecified is bound as asked. Somebody who wrote
/// `127.0.0.1:9797` or one interface's address meant that one.
///
/// # On Windows, two sockets
///
/// The dual-stack socket is built by hand with `SO_REUSEADDR`, which on
/// Windows lets **another program on the machine bind this node's port and
/// take its traffic**. So Windows gets two plain `TcpListener::bind`s on the
/// same port instead, `0.0.0.0` then `[::]`: std leaves `IPV6_V6ONLY` on
/// there, so the two do not clash, and each keeps exactly the options std
/// gives it. Measured on Windows 11 (2026-10-30): a socket asking for
/// `SO_REUSEADDR` is refused either wildcard of the port while they are
/// held, and is let in beside a `[::]` half built with it.
///
/// With port 0 the IPv4 socket picks the port and the IPv6 one takes the
/// same; an IPv6 twin some other program holds is a port picked again. On a
/// port that was named, an IPv6 half something else holds is an error,
/// never a quiet IPv4-only node: this node announces its IPv6 address, and
/// its callers there would reach the other program. A machine with no IPv6
/// keeps the IPv4 socket alone, for the reason above.
///
/// `a_second_listener_cannot_take_a_port_this_one_holds` and
/// `red_team_a_second_listener_cannot_take_the_ipv6_port_this_one_holds` hold
/// this shut. The first failed when this function was first written, which is
/// how the hole was found rather than argued about.
pub fn listen_on(resolved: &[SocketAddr]) -> io::Result<Listeners> {
    let wildcard = resolved
        .iter()
        .find(|address| matches!(address.ip(), IpAddr::V4(v4) if v4.is_unspecified()));

    if let Some(address) = wildcard {
        #[cfg(windows)]
        return bind_side_by_side(address.port());
        #[cfg(not(windows))]
        {
            let both = SocketAddr::new(IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED), address.port());
            if let Ok(listener) = bind_dual_stack(both) {
                return Listeners::of(vec![listener]);
            }
        }
    }

    Listeners::of(vec![TcpListener::bind(resolved)?])
}

/// How often port 0 is picked again when its IPv6 twin is taken.
#[cfg(windows)]
const PORT_PICKS: usize = 8;

/// `0.0.0.0:port` and `[::]:port` as two plain std sockets (Windows).
#[cfg(windows)]
fn bind_side_by_side(port: u16) -> io::Result<Listeners> {
    use std::net::{Ipv4Addr, Ipv6Addr};

    let taken = |error: &io::Error| {
        matches!(
            error.kind(),
            io::ErrorKind::AddrInUse | io::ErrorKind::PermissionDenied
        )
    };
    let mut picks = 0;
    loop {
        let v4 = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port))?;
        let bound = v4.local_addr()?.port();
        match TcpListener::bind((Ipv6Addr::UNSPECIFIED, bound)) {
            Ok(v6) => return Listeners::of(vec![v4, v6]),
            Err(error) if taken(&error) && port == 0 && picks < PORT_PICKS => picks += 1,
            Err(error) if taken(&error) => return Err(error),
            Err(_) => return Listeners::of(vec![v4]),
        }
    }
}

/// The sockets one listen address is served by: one where the system takes
/// both IP versions on one socket, two on Windows (see [`listen_on`]).
///
/// Used like a `TcpListener`. With two, both are non-blocking underneath and
/// a blocking [`Self::accept`] polls them, so a caller that waits on one
/// socket's accept cannot leave the other's callers queued for ever.
#[derive(Debug)]
pub struct Listeners {
    sockets: Vec<TcpListener>,
    blocking: AtomicBool,
    /// Where the next accept starts, so a flood on one family does not
    /// starve the other.
    next: AtomicUsize,
}

impl Listeners {
    fn of(sockets: Vec<TcpListener>) -> io::Result<Self> {
        if sockets.len() > 1 {
            for socket in &sockets {
                // Blocking, an accept on one waits there while the other's
                // callers queue.
                socket.set_nonblocking(true)?;
            }
        }
        Ok(Self {
            sockets,
            blocking: AtomicBool::new(true),
            next: AtomicUsize::new(0),
        })
    }

    /// The address bound: the IPv4 socket's on Windows, which is the port
    /// both share.
    ///
    /// # Errors
    ///
    /// What `TcpListener::local_addr` returns.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.sockets[0].local_addr()
    }

    /// `TcpListener::set_nonblocking`, for every socket.
    ///
    /// # Errors
    ///
    /// What `TcpListener::set_nonblocking` returns.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        if let [only] = &self.sockets[..] {
            only.set_nonblocking(nonblocking)?;
        }
        self.blocking.store(!nonblocking, Ordering::Relaxed);
        Ok(())
    }

    /// The next caller on any of the sockets, as `TcpListener::accept`:
    /// waiting for one unless [`Self::set_nonblocking`] said not to.
    ///
    /// # Errors
    ///
    /// What `TcpListener::accept` returns, and `WouldBlock` from a
    /// non-blocking listener with nobody waiting.
    pub fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        if let [only] = &self.sockets[..] {
            return only.accept();
        }
        loop {
            let start = self.next.load(Ordering::Relaxed);
            for turn in 0..self.sockets.len() {
                let index = (start + turn) % self.sockets.len();
                match self.sockets[index].accept() {
                    Ok((stream, from)) => {
                        self.next.store(index + 1, Ordering::Relaxed);
                        if self.blocking.load(Ordering::Relaxed) {
                            // Accepted from a non-blocking socket, it is one
                            // on Windows; a blocking caller would read
                            // `WouldBlock` as the peer failing.
                            stream.set_nonblocking(false)?;
                        }
                        return Ok((stream, from));
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => return Err(error),
                }
            }
            if !self.blocking.load(Ordering::Relaxed) {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            std::thread::sleep(ACCEPT_POLL);
        }
    }
}

/// How long a blocking [`Listeners::accept`] over two sockets sleeps when
/// neither has a caller: what the servers' own accept loops sleep.
const ACCEPT_POLL: Duration = Duration::from_millis(20);

/// One socket for both IP versions, or an error if this machine has no IPv6.
#[cfg(not(windows))]
fn bind_dual_stack(address: SocketAddr) -> io::Result<TcpListener> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV6,
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )?;
    socket.set_only_v6(false)?;
    // What `TcpListener::bind` sets on this platform: without it a restarted
    // node cannot re-bind its own port while the previous connections drain,
    // and that is every restart of a busy node. It does not mean here what it
    // means on Windows -- see the note on `listen_on`, which is why Windows
    // gets two std sockets instead of this one.
    socket.set_reuse_address(true)?;
    socket.bind(&address.into())?;
    socket.listen(BACKLOG)?;
    Ok(socket.into())
}

/// Connect to the first of `addresses` that answers, each within
/// [`CONNECT_TIMEOUT`].
///
/// Every address, not the first one. A name is how a member is reached from
/// another network -- it is the only thing that survives a public address
/// changing -- and a dual-stack name resolves to an AAAA record *and* an A
/// record. Taking only the first meant a name whose IPv6 route is blocked (a
/// friend's wifi, a hotel, a mobile network that drops it) failed outright
/// while the IPv4 record beside it would have worked, and the failure looked
/// like the peer being down rather than like one of two routes being shut.
///
/// When none answers, the error names every address and what it answered,
/// with the kind of the first failure about the peer rather than about this
/// machine's routes: one address's error alone hid the others.
pub fn connect_to_one_of(addresses: &[SocketAddr]) -> io::Result<TcpStream> {
    connect_within(addresses, CONNECT_TIMEOUT)
}

/// Most addresses one announcement may list.
pub const MAX_LISTED: usize = 4;

/// An announced address that may list several, comma-separated, best first:
/// `203.0.113.7:9797,[2a01:e0a::5]:9797` (HANDOVER §8 0u). One signed string,
/// so the presence a device signs and the coordinator stores are unchanged;
/// a dialler tries every entry, as it already tries every record of a name.
#[derive(Clone, Copy, Debug)]
pub struct AddressList<'a>(pub &'a str);

impl std::net::ToSocketAddrs for AddressList<'_> {
    type Iter = std::vec::IntoIter<SocketAddr>;

    fn to_socket_addrs(&self) -> io::Result<Self::Iter> {
        let mut out = Vec::new();
        let mut last = None;
        for entry in self
            .0
            .split(',')
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .take(MAX_LISTED)
        {
            match std::net::ToSocketAddrs::to_socket_addrs(entry) {
                Ok(found) => out.extend(found),
                Err(error) => last = Some(error),
            }
        }
        if out.is_empty() {
            return Err(last.unwrap_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "no address to connect to")
            }));
        }
        Ok(out.into_iter())
    }
}

/// The same, with the caller choosing how long to wait.
///
/// For a diagnostic probe, which wants an answer rather than a connection: "it
/// did not answer within three seconds" and "it did not answer" read the same
/// to the person whose router is misconfigured, and the coordinator running the
/// probe has members waiting.
pub fn connect_within(addresses: &[SocketAddr], timeout: Duration) -> io::Result<TcpStream> {
    if addresses.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no address to connect to",
        ));
    }

    let mut failures = Vec::with_capacity(addresses.len());
    for address in addresses {
        match TcpStream::connect_timeout(address, timeout) {
            Ok(stream) => return Ok(stream),
            Err(error) => failures.push((*address, error)),
        }
    }

    Err(every_address_failed(failures))
}

/// One error for a dial where every address failed, naming what each said.
///
/// Returning one address's error alone hid the others: a runner with no IPv6
/// dialling `v4,[v6]` printed only "Network is unreachable", which is this
/// machine's excuse, never the IPv4 answer that says what is wrong between the
/// two machines (fleet run 40478381966, 31 times). The kind is the first one
/// that is about the peer rather than about this machine's routes, so a caller
/// sorting by kind reads "it did not answer", not "no IPv6 here".
fn every_address_failed(mut failures: Vec<(SocketAddr, io::Error)>) -> io::Error {
    if failures.len() == 1 {
        // The caller names the address already; repeating it reads as two.
        return failures.remove(0).1;
    }

    let only_this_machine = |kind: io::ErrorKind| {
        matches!(
            kind,
            io::ErrorKind::NetworkUnreachable
                | io::ErrorKind::HostUnreachable
                | io::ErrorKind::AddrNotAvailable
        )
    };
    let kind = failures
        .iter()
        .map(|(_, error)| error.kind())
        .find(|kind| !only_this_machine(*kind))
        .unwrap_or_else(|| failures[0].1.kind());
    let said = failures
        .iter()
        .map(|(address, error)| format!("{address}: {error}"))
        .collect::<Vec<_>>()
        .join("; ");
    io::Error::new(kind, said)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_team_a_relayed_address_is_never_counted_as_dialable() {
        // §8 0v: `relay:<device>@<at>` reaches a machine only through its
        // relay; counted public, it sorted before a LAN address that works
        // and `doctor` counted it as a machine anybody could dial.
        assert!(is_private_address(
            "relay:0101010101010101010101010101010101010101010101010101010101010101@203.0.113.5:9797"
        ));
        assert!(!is_private_address("203.0.113.5:9797"));
    }
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, TcpListener as StdListener};

    /// Whether this machine can use IPv6 at all, asked by doing it.
    ///
    /// CI runners and containers differ, and a test that assumed either answer
    /// would be a test of the runner.
    fn has_ipv6() -> bool {
        StdListener::bind((Ipv6Addr::LOCALHOST, 0)).is_ok()
    }

    /// THE REGRESSION: a node listening on the default `0.0.0.0` was invisible
    /// to every IPv6 caller, which is the one route between two houses that
    /// needs no port forward and costs nothing per machine.
    #[test]
    fn a_node_asked_for_every_interface_is_reachable_over_ipv6_too() {
        let listener = listen_on(&[SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)])
            .expect("binding every interface must work");
        let port = listener.local_addr().unwrap().port();

        if !has_ipv6() {
            // A machine with IPv6 switched off in the kernel cannot have
            // anything else, and must still *start*: a node that refused to
            // run would be a regression in exchange for a reachability it
            // cannot have.
            assert!(
                listener.local_addr().unwrap().is_ipv4(),
                "the listener must fall back to IPv4 rather than refusing to start"
            );
            return;
        }

        // On Windows too, since 2026-10-30: its listener was IPv4 only, so
        // the laptop at home was dialable from another house through a
        // relay or nothing.
        TcpStream::connect_timeout(
            &SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), port),
            CONNECT_TIMEOUT,
        )
        .expect("an IPv6 caller must reach a node that asked for every interface");
        TcpStream::connect_timeout(
            &SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
            CONNECT_TIMEOUT,
        )
        .expect("and the IPv4 callers it already had must keep reaching it");
    }

    /// THE ATTACK ON WINDOWS: `SO_REUSEADDR` there does not mean what it means
    /// on Unix -- it lets a *second* process bind a port a first one already
    /// holds and take its traffic. Building the listener by hand is where that
    /// option would be set without thinking, and the node's port is the one an
    /// attacker on the same machine would want.
    #[test]
    fn a_second_listener_cannot_take_a_port_this_one_holds() {
        let held = listen_on(&[SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)]).unwrap();
        let port = held.local_addr().unwrap().port();

        assert!(
            listen_on(&[SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)]).is_err(),
            "a second listener took port {port} from the one already serving it"
        );
    }

    /// THE SAME ATTACK, ON THE IPV6 HALF: on Windows the IPv6 side of the
    /// port is a socket of its own. Built with `SO_REUSEADDR`, or not built
    /// at all, it let another program on the machine bind `[::]` on the
    /// node's port and answer every IPv6 caller in its place.
    #[test]
    fn red_team_a_second_listener_cannot_take_the_ipv6_port_this_one_holds() {
        let held = listen_on(&[SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)]).unwrap();
        let port = held.local_addr().unwrap().port();
        if !has_ipv6() {
            return;
        }
        let v6 = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), port);

        assert!(
            StdListener::bind(v6).is_err(),
            "another program took [::]:{port} beside this node and answered its IPv6 callers"
        );
        let reusing = socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .unwrap();
        reusing.set_only_v6(true).unwrap();
        reusing.set_reuse_address(true).unwrap();
        assert!(
            reusing.bind(&v6.into()).is_err(),
            "a program asking for SO_REUSEADDR took [::]:{port} from this node"
        );
        assert!(
            listen_on(&[SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)]).is_err(),
            "a second node started on port {port} beside the one already serving it"
        );
    }

    /// An address somebody named is the address they get. Substituting the
    /// dual-stack socket for `127.0.0.1` would put a node meant to be private
    /// on every interface of the machine.
    #[test]
    fn an_address_that_names_one_interface_is_not_widened_to_all_of_them() {
        let listener = listen_on(&[SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0)]).unwrap();
        assert_eq!(
            listener.local_addr().unwrap().ip(),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            "binding loopback must not silently become binding everything"
        );
    }

    /// THE ATTACK IS A TUESDAY: the first record of a dual-stack name is the
    /// IPv6 one, and plenty of networks drop IPv6. Trying only the first
    /// address made every such network look like the peer being down.
    #[test]
    fn an_address_that_does_not_answer_does_not_hide_the_one_that_does() {
        let listener = StdListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let live = listener.local_addr().unwrap();

        // Port 1 on loopback: nothing listens there, and loopback refuses
        // immediately rather than dropping, so this is fast on every runner.
        let dead = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1);

        let stream = connect_to_one_of(&[dead, live])
            .expect("a dead first address must not stop the live second one");
        assert_eq!(stream.peer_addr().unwrap(), live);
    }

    /// Several addresses that all fail are still one failure, and none at all
    /// is a failure too.
    #[test]
    fn nothing_answering_anywhere_is_still_an_error() {
        let dead = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1);
        assert!(
            connect_to_one_of(&[dead, dead]).is_err(),
            "connecting to nothing must fail rather than return a socket"
        );
        assert!(
            connect_to_one_of(&[]).is_err(),
            "an empty address list is a failure, not a connection"
        );
    }

    /// Fleet run 40478381966: 31 dials to a `v4,[v6]` pair from a runner with
    /// no IPv6 printed only "Network is unreachable", the IPv6 attempt's
    /// excuse, so the IPv4 failure -- the one that says what is wrong between
    /// the two machines -- was never seen.
    #[test]
    fn a_failure_on_every_address_says_what_each_one_answered() {
        let v4: SocketAddr = "198.51.100.7:9811".parse().unwrap();
        let v6: SocketAddr = "[2001:db8::7]:9811".parse().unwrap();
        let error = every_address_failed(vec![
            (
                v4,
                io::Error::new(io::ErrorKind::TimedOut, "connection timed out"),
            ),
            (
                v6,
                io::Error::new(io::ErrorKind::NetworkUnreachable, "Network is unreachable"),
            ),
        ]);
        let said = error.to_string();
        assert!(
            said.contains("198.51.100.7:9811: connection timed out"),
            "the IPv4 failure was hidden behind the IPv6 one: {said}"
        );
        assert!(
            said.contains("[2001:db8::7]:9811: Network is unreachable"),
            "the IPv6 failure must still be named: {said}"
        );
        assert_eq!(
            error.kind(),
            io::ErrorKind::TimedOut,
            concat!(
                "a caller sorting failures by kind read 'this machine has no IPv6' ",
                "instead of 'the peer did not answer'"
            )
        );
    }

    /// One address: its own error, untouched, since the caller already names
    /// the address and a message that repeats it reads as two failures.
    #[test]
    fn a_failure_on_one_address_is_that_address_s_own_error() {
        let only: SocketAddr = "198.51.100.7:9811".parse().unwrap();
        let error = every_address_failed(vec![(
            only,
            io::Error::new(io::ErrorKind::ConnectionRefused, "connection refused"),
        )]);
        assert_eq!(
            error.kind(),
            io::ErrorKind::ConnectionRefused,
            "a lone failure lost its kind, so a refused port no longer reads as refused"
        );
        assert_eq!(
            error.to_string(),
            "connection refused",
            "a lone failure was rewrapped, and the log names its address twice"
        );
    }

    /// The same, through the real dialler: two closed ports, both named.
    #[test]
    fn a_dial_to_two_closed_ports_names_both() {
        // Both held while the ports are read, so the system cannot hand the
        // same one out twice; dropped before dialling, so both are closed.
        let a = StdListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let b = StdListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let (first, second) = (a.local_addr().unwrap(), b.local_addr().unwrap());
        drop((a, b));
        let said = connect_to_one_of(&[first, second])
            .expect_err("nothing listens on either port")
            .to_string();
        assert!(
            said.contains(&first.to_string()) && said.contains(&second.to_string()),
            "a dial over two addresses must say what each one answered: {said}"
        );
    }

    #[test]
    fn red_team_an_address_list_is_capped_and_skips_what_does_not_parse() {
        use std::net::ToSocketAddrs as _;
        let long = (1..=50)
            .map(|i| format!("127.0.0.{i}:9"))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            AddressList(&long).to_socket_addrs().unwrap().count(),
            MAX_LISTED,
            "a presence listing fifty addresses made the dialler try fifty connections"
        );
        assert_eq!(
            AddressList("not an address,127.0.0.1:9")
                .to_socket_addrs()
                .unwrap()
                .count(),
            1,
            "one bad entry hid the good one beside it"
        );
        assert!(AddressList(" , ").to_socket_addrs().is_err());
    }
}
