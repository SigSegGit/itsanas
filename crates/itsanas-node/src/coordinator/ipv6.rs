//! This machine's global IPv6 address: which one to announce, kept
//! stable across the rotations of temporary addresses.

use std::net::SocketAddr;

/// The address the router opened (`UPnP`, NAT-PMP), then this machine's global
/// IPv6 address on the listening port, as one list (HANDOVER §8 0u): a peer
/// whose network has no IPv6 dials the first, one behind a box that opened
/// nothing outbound for IPv4 the second. On Windows too, whose listener
/// takes IPv6 beside IPv4 since 2026-10-30 (`itsanas_tls::reach::listen_on`).
#[must_use]
pub fn with_global_ipv6(
    found: &str,
    global: Option<std::net::Ipv6Addr>,
    port: Option<u16>,
) -> String {
    match (global, port) {
        (Some(v6), Some(port)) => {
            format!("{found},{}", SocketAddr::new(v6.into(), port))
        }
        _ => found.to_owned(),
    }
}

/// This machine's global IPv6 address, if it has one: the local end of a UDP
/// "connection" towards a public IPv6 address (no packet is sent), swapped
/// for the stable one beside it on Linux, or for the one announced last
/// elsewhere.
pub(super) fn global_ipv6() -> Option<std::net::Ipv6Addr> {
    let socket = std::net::UdpSocket::bind((std::net::Ipv6Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(("2001:4860:4860::8888", 53)).ok()?;
    let std::net::IpAddr::V6(v6) = socket.local_addr().ok()?.ip() else {
        return None;
    };
    if !is_global_ipv6(v6) {
        return None;
    }
    let table = std::fs::read_to_string("/proc/net/if_inet6").ok();
    Some(announced_ipv6(v6, table.as_deref(), &LAST_IPV6, holds_ipv6))
}

/// [`global_ipv6`] once the kernel has chosen `chosen`: the stable address
/// beside it where Linux's `table` is there, the one announced last
/// (`last`, which this updates) where it is not. Split out so two
/// consecutive calls can be tested without the machine's own addresses.
fn announced_ipv6(
    chosen: std::net::Ipv6Addr,
    table: Option<&str>,
    last: &std::sync::Mutex<Option<std::net::Ipv6Addr>>,
    holds: impl Fn(std::net::Ipv6Addr) -> bool,
) -> std::net::Ipv6Addr {
    let mut last = last
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // Sticky only where the table is missing, as before this existed on
    // Linux: there the kernel's choice is already the stable address, or
    // `stable_ipv6` finds it.
    let address = match table {
        Some(table) => stable_ipv6(chosen, table).unwrap_or(chosen),
        None => sticky_ipv6(*last, chosen, holds),
    };
    *last = Some(address);
    address
}

/// The stable address beside `chosen` when `chosen` is a temporary one
/// (RFC 4941), from Linux's `/proc/net/if_inet6`. The kernel picks the
/// temporary address as the source, and it rotates within a day: announced,
/// it made the machine republish at each rotation and left peers dialling an
/// address it had dropped. Same interface and same /64, global, neither
/// temporary, deprecated nor tentative. `None` when `chosen` is not
/// temporary or nothing stable sits beside it.
fn stable_ipv6(chosen: std::net::Ipv6Addr, table: &str) -> Option<std::net::Ipv6Addr> {
    const TEMPORARY: u32 = 0x01;
    const UNUSABLE: u32 = TEMPORARY | 0x20 | 0x40; // deprecated, tentative
    let rows: Vec<(std::net::Ipv6Addr, &str, u32)> = table
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let [hex, _index, _prefix, scope, flags, name] = fields[..] else {
                return None;
            };
            let bits = u128::from_str_radix(hex, 16).ok()?;
            (scope == "00").then_some(())?;
            Some((
                std::net::Ipv6Addr::from(bits),
                name,
                u32::from_str_radix(flags, 16).ok()?,
            ))
        })
        .collect();
    let (_, interface, flags) = rows.iter().find(|(address, _, _)| *address == chosen)?;
    if flags & TEMPORARY == 0 {
        return None;
    }
    let net = |a: &std::net::Ipv6Addr| a.segments()[..4].to_vec();
    rows.iter()
        .find(|(address, name, flags)| {
            name == interface
                && flags & UNUSABLE == 0
                && net(address) == net(&chosen)
                && is_global_ipv6(*address)
        })
        .map(|(address, _, _)| *address)
}

/// The address announced last, while it is still this machine's, global and
/// on the /64 of `chosen`; `chosen` otherwise.
///
/// Where `stable_ipv6` has nothing to read (macOS, Windows) the kernel's
/// choice is a temporary address (RFC 4941) that it replaces every day. The
/// one it replaced stays assigned, deprecated, until its valid lifetime runs
/// out, so keeping it moves the announcement less often than every day, and
/// never to an address the machine has dropped. Not measured: that Windows
/// still accepts a connection on a deprecated temporary address (RFC 4862
/// says it should; no rotation was watched here).
///
/// Only the interface ID is kept, never the prefix: after a renumbering the
/// old prefix's address also stays assigned, deprecated, for hours or days,
/// and kept it sent every peer from another house to a prefix the ISP no
/// longer routes.
fn sticky_ipv6(
    previous: Option<std::net::Ipv6Addr>,
    chosen: std::net::Ipv6Addr,
    still_assigned: impl Fn(std::net::Ipv6Addr) -> bool,
) -> std::net::Ipv6Addr {
    previous
        // Global follows: `chosen` is (`global_ipv6` checks), and the /64
        // carries what makes an address global.
        .filter(|previous| {
            previous.segments()[..4] == chosen.segments()[..4] && still_assigned(*previous)
        })
        .unwrap_or(chosen)
}

/// The IPv6 address `global_ipv6` returned last, for `sticky_ipv6`. Per
/// process: a one-shot command (`itsanas register`) starts with nothing and
/// may publish the kernel's newer temporary address, until the daemon's next
/// publication puts back the one it kept. Both still answer; the command
/// publishes once, so this is not a loop.
static LAST_IPV6: std::sync::Mutex<Option<std::net::Ipv6Addr>> = std::sync::Mutex::new(None);

/// Whether `address` is still one of this machine's: a socket binds to it.
/// Asked by binding rather than by listing the adapters, because
/// `if_addrs` leaves out a deprecated address on Windows (it keeps only
/// `IpDadStatePreferred`), and deprecated is what a rotated one is.
fn holds_ipv6(address: std::net::Ipv6Addr) -> bool {
    std::net::UdpSocket::bind((address, 0)).is_ok()
}

/// Global unicast (`2000::/3`), not documentation (`2001:db8::/32`).
fn is_global_ipv6(address: std::net::Ipv6Addr) -> bool {
    let first = address.segments()[0];
    (first & 0xe000) == 0x2000 && !(first == 0x2001 && address.segments()[1] == 0x0db8)
}

/// Which local address to announce when nothing better is known (no
/// `announce`, no port opened by UPnP): a global IPv6 address over a private
/// IPv4 one (HANDOVER §8 0u, first step). Behind a home NAT the IPv4 address
/// is reachable from this LAN only, where peers find each other by beacon
/// anyway; a global IPv6 address reaches this machine from any house with
/// IPv6, with no forward, and the listener already takes both versions
/// (`itsanas_tls::reach::listen_on`). A public IPv4 address
/// (a server with no NAT) stays: everybody can dial it.
#[must_use]
pub fn prefer_global_ipv6(local: SocketAddr, global: Option<std::net::Ipv6Addr>) -> SocketAddr {
    let private_v4 = match local.ip() {
        std::net::IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 64
        }
        std::net::IpAddr::V6(_) => false,
    };
    match global {
        Some(v6) if private_v4 => SocketAddr::new(v6.into(), local.port()),
        _ => local,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_team_a_temporary_ipv6_address_is_replaced_by_the_stable_one_beside_it() {
        // §8 0u: announcing the system's temporary address made a machine
        // republish at every rotation, and peers dial an address it dropped.
        let table = "\
20010db8aaaa00000000000000000001 02 40 00 00 eth0
2a01e0a0000100020000000000000099 02 40 00 01 eth0
2a01e0a0000100020000000000000043 02 40 00 20 eth0
2a01e0a0000100020000000000000042 02 40 00 00 eth0
fe800000000000000000000000000001 02 40 20 80 eth0
";
        let temporary: std::net::Ipv6Addr = "2a01:e0a0:1:2::99".parse().unwrap();
        assert_eq!(
            stable_ipv6(temporary, table),
            Some("2a01:e0a0:1:2::42".parse().unwrap()),
            "the temporary address was kept, or a deprecated one taken"
        );
        let stable: std::net::Ipv6Addr = "2a01:e0a0:1:2::42".parse().unwrap();
        assert_eq!(
            stable_ipv6(stable, table),
            None,
            "a stable choice was changed"
        );
    }

    #[test]
    fn red_team_a_rotated_temporary_ipv6_does_not_move_the_announced_address() {
        // §8 0u: macOS and Windows have no `/proc/net/if_inet6`, so the
        // temporary address the kernel prefers was announced, and it rotates
        // every day: a republish a day, and peers dialling what was dropped.
        let announced: std::net::Ipv6Addr = "2a01:e0a0:1:2::aaaa".parse().unwrap();
        let rotated: std::net::Ipv6Addr = "2a01:e0a0:1:2::bbbb".parse().unwrap();
        let held = |list: &'static [&'static str]| {
            move |address: std::net::Ipv6Addr| {
                list.iter()
                    .any(|held| held.parse::<std::net::Ipv6Addr>().unwrap() == address)
            }
        };
        let both = held(&["2a01:e0a0:1:2::aaaa", "2a01:e0a0:1:2::bbbb"]);
        assert_eq!(
            sticky_ipv6(Some(announced), rotated, both),
            announced,
            "the kernel's new temporary address replaced one still answering: \
             the machine republished at every rotation"
        );
        assert_eq!(
            sticky_ipv6(Some(announced), rotated, held(&["2a01:e0a0:1:2::bbbb"])),
            rotated,
            "an address the machine dropped stayed announced, and nobody could dial it"
        );
        // Red team on #346: a renumbered prefix. The old address stays
        // assigned, deprecated, for hours or days, and binds. Sabotage: drop
        // the /64 comparison in `sticky_ipv6`.
        let renumbered: std::net::Ipv6Addr = "2a01:e0a0:9:9::bbbb".parse().unwrap();
        assert_eq!(
            sticky_ipv6(Some(announced), renumbered, |_| true),
            renumbered,
            "the address on the old prefix stayed announced after a renumbering: \
             every peer from another house dialled a prefix nobody routes any more"
        );
        assert_eq!(sticky_ipv6(None, rotated, both), rotated);

        // The wiring `global_ipv6` goes through, two rounds in a row.
        // Sabotage: drop `*last = Some(address)` in `announced_ipv6`, or
        // return `chosen` where the table is missing.
        let last = std::sync::Mutex::new(None);
        assert_eq!(announced_ipv6(announced, None, &last, both), announced);
        assert_eq!(
            announced_ipv6(rotated, None, &last, both),
            announced,
            "the second round forgot the address the first announced: a macOS or \
             Windows machine republished at every rotation"
        );
        // Where Linux's table is there, its answer stands, as before this
        // existed. Sabotage: apply `sticky_ipv6` with a table too.
        assert_eq!(
            announced_ipv6(rotated, Some(""), &last, both),
            rotated,
            "on Linux an address kept from an earlier round overrode the kernel's own \
             stable choice"
        );
    }

    #[test]
    fn an_address_this_machine_holds_binds_and_one_it_does_not_never_does() {
        // What `sticky_ipv6` asks to know whether a kept address is still
        // this machine's. Sabotage: `holds_ipv6` always true, or always false.
        assert!(
            !holds_ipv6("2001:db8::1".parse().unwrap()),
            "an address of no machine counted as this one's: a dropped address \
             would stay announced, and nobody could dial it"
        );
        // No IPv6 in the kernel (some CI containers): nothing is held.
        if std::net::TcpListener::bind((std::net::Ipv6Addr::LOCALHOST, 0)).is_ok() {
            assert!(
                holds_ipv6(std::net::Ipv6Addr::LOCALHOST),
                "an address this machine has was taken for a dropped one: the \
                 announcement would move at every rotation"
            );
        }
    }

    #[test]
    fn red_team_a_home_node_announces_its_global_ipv6_not_its_private_ipv4() {
        let v6: std::net::Ipv6Addr = "2a01:e0a:1:2::5".parse().unwrap();
        let home: SocketAddr = "192.168.1.20:9797".parse().unwrap();
        let server: SocketAddr = "203.0.113.7:9797".parse().unwrap();
        assert_eq!(
            prefer_global_ipv6(home, Some(v6)),
            SocketAddr::new(v6.into(), 9797),
            "a node behind a home NAT announced the address only its own LAN can dial, \
             though it has a global IPv6 one that needs no forward"
        );
        assert_eq!(
            prefer_global_ipv6(server, Some(v6)),
            server,
            "a public IPv4 address, dialable by everybody, was replaced"
        );
        assert_eq!(prefer_global_ipv6(home, None), home);
        for not_global in ["fe80::1", "fd00::1", "2001:db8::1", "::1"] {
            assert!(
                !is_global_ipv6(not_global.parse().unwrap()),
                "{not_global} was taken for a global address and announced"
            );
        }
    }

    #[test]
    fn red_team_a_node_with_an_opened_port_and_ipv6_offers_both_and_every_one_is_dialable() {
        let v6: std::net::Ipv6Addr = "2a01:e0a:1:2::5".parse().unwrap();
        let listed = with_global_ipv6("203.0.113.7:41000", Some(v6), Some(9797));
        assert_eq!(
            listed, "203.0.113.7:41000,[2a01:e0a:1:2::5]:9797",
            "the IPv6 route was not offered beside the opened port, so a house with no \
             IPv4 path to it could not dial this node"
        );
        let resolved: Vec<SocketAddr> =
            std::net::ToSocketAddrs::to_socket_addrs(&itsanas_net::AddressList(&listed))
                .unwrap()
                .collect();
        assert_eq!(
            resolved.len(),
            2,
            "a dialler saw one route of two: a peer with no IPv4 path, or no IPv6 one, \
             would fail where the other would have worked"
        );
        assert!(
            listed.len() <= itsanas_coord::claim::MAX_ADDRESS_LEN,
            "the list no longer fits the signed presence"
        );
        assert_eq!(
            with_global_ipv6("203.0.113.7:41000", None, Some(9797)),
            "203.0.113.7:41000"
        );
    }
}
