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
    net::{SocketAddr, TcpListener, TcpStream},
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
/// # Not on Windows, and this is a real limit rather than an oversight
///
/// Clearing `IPV6_V6ONLY` means building the socket by hand, and a socket built
/// by hand on Windows does not get `SO_EXCLUSIVEADDRUSE`, which
/// `TcpListener::bind` sets and which is what stops **another process on the
/// machine binding this node's port and taking its traffic**. `socket2` 0.6
/// exposes no safe way to set it, and this workspace allows `unsafe` in exactly
/// one file, which is not this one. Given the choice between a node that can be
/// hijacked locally and a Windows node that is not dialable over IPv6, the
/// second is the smaller loss: the machines that have to be *dialed* in this
/// fleet are the Linux ones that stay at home, and the Windows machine is the
/// laptop that moves and takes part by dialling out.
///
/// `a_second_listener_cannot_take_a_port_this_one_holds` is what holds this
/// shut. It failed when this function was first written, which is how the hole
/// was found rather than argued about.
pub fn listen_on(resolved: &[SocketAddr]) -> io::Result<TcpListener> {
    #[cfg(not(windows))]
    {
        use std::net::{IpAddr, Ipv6Addr};

        let wildcard = resolved
            .iter()
            .find(|address| matches!(address.ip(), IpAddr::V4(v4) if v4.is_unspecified()));

        if let Some(address) = wildcard {
            let both = SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), address.port());
            if let Ok(listener) = bind_dual_stack(both) {
                return Ok(listener);
            }
        }
    }

    TcpListener::bind(resolved)
}

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
    // means on Windows -- see the note on `listen_on`, which is why this
    // function does not exist there.
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
/// The error returned is the last one, because the last address tried is the
/// one that had no alternative left.
pub fn connect_to_one_of(addresses: &[SocketAddr]) -> io::Result<TcpStream> {
    let mut last: Option<io::Error> = None;

    for address in addresses {
        match TcpStream::connect_timeout(address, CONNECT_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(error) => last = Some(error),
        }
    }

    Err(last
        .unwrap_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no address to connect to")))
}

#[cfg(test)]
mod tests {
    use super::*;
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

        if cfg!(windows) || !has_ipv6() {
            // Windows deliberately keeps the plain IPv4 listener (see
            // `listen_on`), and a machine with IPv6 switched off in the kernel
            // cannot have anything else. Both must still *start*: a node that
            // refused to run would be a regression in exchange for a
            // reachability neither of them can have.
            assert!(
                listener.local_addr().unwrap().is_ipv4(),
                "the listener must fall back to IPv4 rather than refusing to start"
            );
            return;
        }

        assert!(
            listener.local_addr().unwrap().is_ipv6(),
            "a wildcard bind must take the dual-stack socket when IPv6 exists; \
             an IPv4-only listener cannot be dialled between two houses"
        );

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

    /// The failure a caller sees must be the one that has no alternative left,
    /// not the first of several.
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
}
