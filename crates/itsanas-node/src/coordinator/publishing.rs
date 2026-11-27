//! Where this machine says it is: the address published, the presence
//! signed with it, and the relay or router port that address can name.

use std::net::{SocketAddr, ToSocketAddrs};

use itsanas_coord::claim::Presence;
use itsanas_coord::protocol::{Request, Response};
use itsanas_crypto::DeviceId;

use super::ipv6::global_ipv6;
use super::{dial, prefer_global_ipv6, with_global_ipv6};
use crate::node::Node;
use crate::{NodeError as CliError, Result};

/// The address to publish for this device.
///
/// A node listening on `0.0.0.0:9797` — the default, and the right default —
/// must not tell the coordinator that this *is* its address. `0.0.0.0` is where
/// to accept connections from; it is not somewhere to dial. A peer that looked
/// it up got an address it could not use, and `itsanas register` printed
/// `announced 0.0.0.0:9797` as though that had worked. The comment above that
/// call says a device nobody can reach has not really joined anything, which is
/// exactly what it had just arranged.
///
/// So when the configured address is unspecified, publish the local end of the
/// connection that just reached the coordinator: among this machine's
/// addresses, that is the one demonstrably able to talk to it. The port stays
/// the configured one — the listening port, not the ephemeral port this
/// particular connection went out from.
///
/// **This is still wrong behind NAT**, where the address a peer needs is the
/// router's and only the coordinator can observe it. The fix for that is the
/// coordinator recording the source address it saw, which is a protocol change
/// and is safe for the same reason this is: members pin the device id, so an
/// address leading to the wrong machine is refused rather than trusted.
#[must_use]
pub fn reachable_address(configured: &str, local: SocketAddr) -> String {
    let Ok(parsed) = configured.parse::<SocketAddr>() else {
        // Not an address literal, so it is a hostname somebody chose on
        // purpose. Substituting an IP for it would be overruling them.
        return configured.to_owned();
    };
    if parsed.ip().is_unspecified() {
        SocketAddr::new(local.ip(), parsed.port()).to_string()
    } else {
        configured.to_owned()
    }
}

/// What this node tells the coordinator, which is not always where it listens.
///
/// `announce` in the configuration wins, verbatim and including its port: it is
/// the address that reaches this machine from *another* network, and the whole
/// reason it exists is that it differs from the local one. A forwarded port
/// maps an outside port to a different inside port, so replacing the port with
/// the listening one would publish an address that is wrong in exactly the case
/// the setting is for.
///
/// Without it, the old behaviour: the local end of the connection that reached
/// the coordinator. That is right for a house where everyone is on one LAN and
/// right for a machine that moves -- a laptop at a friend's house has no
/// address anybody elsewhere can dial, and saying so honestly is better than
/// inventing one. Such a machine takes part by dialling out; one reachable side
/// per pair is enough.
#[must_use]
pub fn published_address(
    config: &crate::config::Config,
    listen: &str,
    local: SocketAddr,
) -> String {
    publishable(
        config,
        listen,
        on_a_real_adapter(local, &itsanas_discover::LocalNets::of_this_machine()),
        found_address(),
        relayed_address(),
        global_ipv6,
    )
}

/// [`published_address`] with this process's state passed in, so the rule
/// is tested without the statics it reads.
fn publishable(
    config: &crate::config::Config,
    listen: &str,
    local: SocketAddr,
    found: Option<String>,
    relayed: Option<String>,
    ipv6: impl Fn() -> Option<std::net::Ipv6Addr>,
) -> String {
    if let Some(announce) = config.announce.clone() {
        return announce;
    }
    // What `address_now` says, for the same reasons: a park, then the
    // router's port with the global IPv6 address. This one used to return
    // only the router's port, so a 0.1.6 node compared a `relay:` or a list
    // with itself and published neither.
    // Never more than the coordinator takes: an address over its cap was
    // refused at every round, so the machine stopped being findable at all.
    let fits = |address: &String| address.len() <= itsanas_coord::claim::MAX_ADDRESS_LEN;
    if let Some(relayed) = relayed.filter(fits) {
        return relayed;
    }
    match found {
        Some(found) if ANNOUNCE_LISTS => {
            let port = listen.parse::<SocketAddr>().ok().map(|l| l.port());
            Some(with_global_ipv6(&found, ipv6(), port))
                .filter(fits)
                .unwrap_or(found)
        }
        Some(found) => found,
        // The global IPv6 address over a private IPv4 one, as `direct_address`.
        None => reachable_address(listen, prefer_global_ipv6(local, ipv6())),
    }
}

/// `relay:<relay>@<its address>` while this process is parked and has no
/// port of its own, if this build announces it.
fn relayed_address() -> Option<String> {
    let (relay, at) = parked_on().filter(|_| ANNOUNCE_RELAY && found_address().is_none())?;
    // The relay's address rides along for a caller that does not know it (a
    // host of another account); the relay's key is pinned on the dial, so a
    // wrong address reaches nobody else.
    Some(format!("{}{relay}@{at}", crate::contact::RELAY_PREFIX))
}

/// When to date a presence signed now: never before the last one this
/// machine signed (kept in `presence.last` in its home). A Pi rebooted with
/// no clock into 1970 signed presences older than the one its peers hold,
/// which they refused as stale until NTP came back: the machine was
/// unreachable for that long. Its own record moves forward instead.
const CLOCK_FLOOR: u64 = 1_767_225_600; // 2026-01-01

/// [`presence_time`]'s rule, without the file.
fn next_presence_time(last: u64, now: u64) -> u64 {
    // A clock before this code was written is a clock that has not been
    // set (no RTC, no NTP yet): move on from the last presence. A record
    // ahead of a clock that *is* set came from a clock that ran ahead; kept,
    // it would date every later presence past what peers accept, for ever.
    let clock_set = now >= CLOCK_FLOOR;
    if clock_set && last > now.saturating_add(itsanas_coord::claim::MAX_CLOCK_SKEW) {
        now
    } else {
        now.max(last.saturating_add(1))
    }
}

pub(super) fn presence_time(node: &Node, now: u64) -> u64 {
    // One signer at a time in this process: two announces reading the same
    // record would sign the same date.
    static SIGNING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _one = SIGNING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let path = node.home.join("presence.last");
    let last = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| text.trim().parse::<u64>().ok())
        .unwrap_or(0);
    // A clock before this code was written is a clock that has not been
    // set (no RTC, no NTP yet): move on from the last presence. A record
    // ahead of a clock that *is* set came from a clock that ran ahead; kept,
    // it would date every later presence past what peers accept, for ever.
    let at = next_presence_time(last, now);
    // Written aside and renamed: a power cut mid-write left an empty file,
    // the floor read as 0, and the 1970 clock it exists for came back.
    let aside = path.with_extension("last.tmp");
    if let Err(error) =
        std::fs::write(&aside, at.to_string()).and_then(|()| std::fs::rename(&aside, &path))
    {
        eprintln!(
            "itsanas: could not keep {} ({error}); a clock reset may date presences back",
            path.display()
        );
    }
    at
}

/// An address this process found it is reachable at -- the router opened its
/// port by `UPnP` -- used when the configuration names none. A written
/// `announce` always wins: a person who typed one knows their network.
static FOUND_ADDRESS: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Say where this process is reachable from outside, or that it is not.
pub fn set_found_address(address: Option<String>) {
    *FOUND_ADDRESS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = address;
}

pub fn found_address() -> Option<String> {
    FOUND_ADDRESS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// The relay this process is parked on (§8 0v), set by the daemon's parking
/// thread while the park holds.
static PARKED_ON: std::sync::Mutex<Option<(DeviceId, String)>> = std::sync::Mutex::new(None);

/// Say which relay this process is parked on, and at what address, or that
/// it is not.
pub fn set_parked_on(relay: Option<(DeviceId, String)>) {
    *PARKED_ON
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = relay;
}

fn parked_on() -> Option<(DeviceId, String)> {
    PARKED_ON
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Whether this build announces `relay:<device>` while parked. On, with
/// [`ANNOUNCE_LISTS`], from the release the whole fleet is reinstalled with
/// for its test (Nicolas, 2026-10-13): a peer older than protocol 9 cannot
/// dial it, and an address it cannot parse is an address it loses -- so no
/// machine older than this release belongs in that fleet.
pub const ANNOUNCE_RELAY: bool = true;

/// Publish where this device can be reached, and return what was published.
///
/// Signed by the device, not by the owner: a laptop moving between networks
/// announces constantly and must never need the key that can revoke everything.
///
/// Returns the address rather than the unit, so that a caller reporting what
/// happened reports what was sent instead of what it asked for. `register`
/// printed the configured value and was wrong whenever the two differed.
pub fn announce(node: &Node, address: &str, now: u64) -> Result<String> {
    let mut client = dial(node)?;
    let address = published_address(&node.config, address, client.local_addr());
    let presence = Presence {
        device: node.store.device_id(),
        address: address.clone(),
        at_unix: presence_time(node, now),
        head: node.store.chain_length().unwrap_or(0),
    }
    .sign(&node.device);

    match client.ask(&Request::Announce(Box::new(presence)))? {
        Response::Done => Ok(address),
        Response::Refused(why) => Err(CliError::Usage(why)),
        other => Err(CliError::Usage(format!("unexpected answer: {other:?}"))),
    }
}

/// Where this machine would publish itself now, found without dialling.
///
/// `announce`, and a listening address that names an interface, are what they
/// are. Otherwise the answer is the local end of a route to the coordinator,
/// which a UDP socket finds by `connect` while sending nothing: a laptop that
/// changed network sees its address change without costing the coordinator a
/// connection. It is not always the address a TCP connection would publish --
/// the two can pick different families of a dual-stack name -- which is why
/// [`Contact`](crate::contact::Contact) compares it only with itself.
///
/// `None` when there is no route: no network, or a name that does not resolve.
#[must_use]
pub fn address_now(config: &crate::config::Config, listen: &str) -> Option<String> {
    if let Some(announce) = config.announce.clone() {
        return Some(announce);
    }
    if let Some(relayed) = relayed_address() {
        return Some(relayed);
    }
    direct_address(config, listen)
}

/// [`address_now`] as if this machine were parked nowhere: where it can be
/// dialled directly, which is what decides whether it needs a relay at all.
#[must_use]
pub fn direct_address(config: &crate::config::Config, listen: &str) -> Option<String> {
    if let Some(announce) = config.announce.clone() {
        return Some(announce);
    }
    if let Some(found) = found_address() {
        if ANNOUNCE_LISTS {
            let port = listen.parse::<SocketAddr>().ok().map(|l| l.port());
            return Some(with_global_ipv6(&found, global_ipv6(), port));
        }
        return Some(found);
    }
    match listen.parse::<SocketAddr>() {
        Ok(parsed) if parsed.ip().is_unspecified() => {}
        // An interface, or a name somebody chose: published as it stands.
        _ => return Some(listen.to_owned()),
    }
    let coordinator = config.coordinator.as_deref()?;
    coordinator
        .to_socket_addrs()
        .ok()?
        .find_map(|target| {
            let any: SocketAddr = if target.is_ipv4() {
                (std::net::Ipv4Addr::UNSPECIFIED, 0).into()
            } else {
                (std::net::Ipv6Addr::UNSPECIFIED, 0).into()
            };
            let socket = std::net::UdpSocket::bind(any).ok()?;
            socket.connect(target).ok()?;
            socket.local_addr().ok()
        })
        .map(|local| on_a_real_adapter(local, &itsanas_discover::LocalNets::of_this_machine()))
        .map(|local| reachable_address(listen, prefer_global_ipv6(local, global_ipv6())))
}

/// `local`, unless it is an address of one of this machine's virtual
/// adapters -- a hypervisor's host network, a container bridge, Tailscale or
/// another overlay (`itsanas_discover::is_virtual_adapter`) -- in which case
/// the first address of a real one, on the same port.
///
/// The route to the coordinator can leave by an overlay: a laptop whose
/// Tailscale carries everything published its 100.x address, which nobody at
/// home or elsewhere dials, and a machine whose route flips between adapters
/// publishes a new address at every flip. Both `address_now` and the address
/// published go through this, so the two keep agreeing. A machine with no
/// real adapter keeps the virtual one rather than publish nothing.
///
/// "Virtual" is the adapter's name (`LocalNets::is_virtual`), with a Hyper-V
/// external switch counted real (red team, 2026-10-23): the LAN address of a
/// machine with one lives on `vEthernet (External ...)`, and swapping it for
/// another card's -- a second NIC on another network -- published an address
/// the house could not dial. A carrier-grade NAT address (100.64.0.0/10) on a
/// network card is that card's own and stays too.
#[must_use]
pub fn on_a_real_adapter(local: SocketAddr, nets: &itsanas_discover::LocalNets) -> SocketAddr {
    match nets.physical_v4() {
        Some(real) if nets.is_virtual(local.ip()) => SocketAddr::new(real.into(), local.port()),
        _ => local,
    }
}

/// Whether this build announces a list of addresses. Every build since
/// 0u *dials* a list (`itsanas_tls::reach::AddressList`); announcing one
/// waited, because a peer older than that cannot dial `"v4,[v6]"` and would
/// lose the IPv4 route it has today, and `updates = notify`, the default,
/// means the fleet does not update by itself (Rodin, 2026-10-13). On from the
/// release the whole fleet is reinstalled with (Nicolas, 2026-10-13).
pub const ANNOUNCE_LISTS: bool = true;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn red_team_what_is_published_is_the_relay_or_the_list_not_only_the_port() {
        // 0.1.6 compared `relay:` and lists with itself and published only
        // the router's port: nobody could reach a parked machine.
        let config = crate::config::Config::default();
        let local: SocketAddr = "192.168.1.5:9797".parse().unwrap();
        let relay = "relay:aa@203.0.113.5:9797".to_owned();
        assert_eq!(
            publishable(
                &config,
                "0.0.0.0:9797",
                local,
                None,
                Some(relay.clone()),
                || None
            ),
            relay
        );
        let v6: std::net::Ipv6Addr = "2a01:e0a0:1:2::42".parse().unwrap();
        let listed = publishable(
            &config,
            "0.0.0.0:9797",
            local,
            Some("198.51.100.1:9797".to_owned()),
            None,
            || Some(v6),
        );
        assert_eq!(
            listed, "198.51.100.1:9797,[2a01:e0a0:1:2::42]:9797",
            "the IPv6 half was left out, so a peer with no IPv4 route had nothing to dial"
        );
        let long = format!("relay:aa@{}.example:9797", "h".repeat(250));
        assert_eq!(
            publishable(&config, "0.0.0.0:9797", local, None, Some(long), || None),
            "192.168.1.5:9797",
            "an address over the coordinator's cap was published, and refused every round"
        );
    }

    #[test]
    fn red_team_what_is_published_with_no_router_port_is_what_the_probe_names() {
        // `direct_address`, the probe, takes the global IPv6 address over a
        // private IPv4 one; the publication took the IPv4 one. A machine at
        // home with IPv6 then differed from itself every round and
        // republished every round.
        let config = crate::config::Config::default();
        let local: SocketAddr = "192.168.1.5:9797".parse().unwrap();
        let v6: std::net::Ipv6Addr = "2a01:e0a0:1:2::42".parse().unwrap();
        assert_eq!(
            publishable(&config, "0.0.0.0:9797", local, None, None, || Some(v6)),
            prefer_global_ipv6(local, Some(v6)).to_string(),
            "the publication and the probe named two addresses for one machine"
        );
    }

    #[test]
    fn red_team_a_machine_rebooted_into_1970_still_dates_its_presence_after_the_last() {
        // Its peers hold a presence from before the reboot and refuse an
        // older one as stale: it was unreachable until NTP came back.
        let before = 1_800_000_000;
        assert_eq!(next_presence_time(before, 12), before + 1);
        assert_eq!(next_presence_time(before, before + 60), before + 60);
        assert_eq!(
            next_presence_time(before + 100_000, before),
            before,
            "a record left by a clock that ran ahead dated every later presence \
             past what peers accept"
        );
    }

    /// The hand test of 2026-10-22, on the publishing side: a laptop with
    /// VMware's host networks, WSL's switch and Tailscale must publish the
    /// Wi-Fi address, whichever adapter the route to the coordinator left by;
    /// a real adapter's address stands as it is, and a machine with nothing
    /// but virtual adapters keeps what it has.
    #[test]
    fn red_team_an_address_on_a_virtual_adapter_is_not_published() {
        let ip = |text: &str| text.parse::<std::net::IpAddr>().unwrap();
        let mask = ip("255.255.255.0");
        let laptop = itsanas_discover::LocalNets::from_interfaces([
            ("Wi-Fi".to_owned(), ip("192.168.1.142"), mask),
            (
                "VMware Network Adapter VMnet1".to_owned(),
                ip("192.168.19.1"),
                mask,
            ),
            (
                "VMware Network Adapter VMnet8".to_owned(),
                ip("192.168.117.1"),
                mask,
            ),
            (
                "vEthernet (WSL)".to_owned(),
                ip("172.27.160.1"),
                ip("255.255.240.0"),
            ),
            (
                "Tailscale".to_owned(),
                ip("100.101.102.103"),
                ip("255.192.0.0"),
            ),
        ]);
        for route in [
            "192.168.19.1:9811",
            "192.168.117.1:9811",
            "172.27.160.1:9811",
            "100.101.102.103:9811",
        ] {
            assert_eq!(
                on_a_real_adapter(route.parse().unwrap(), &laptop).to_string(),
                "192.168.1.142:9811",
                "a route by {route} would publish a virtual adapter's address"
            );
        }
        let wifi: SocketAddr = "192.168.1.142:9811".parse().unwrap();
        assert_eq!(on_a_real_adapter(wifi, &laptop), wifi);
        let only_virtual = itsanas_discover::LocalNets::from_interfaces([(
            "vmnet1".to_owned(),
            ip("192.168.19.1"),
            mask,
        )]);
        let route: SocketAddr = "192.168.19.1:9811".parse().unwrap();
        assert_eq!(on_a_real_adapter(route, &only_virtual), route);
    }

    /// Red team, 2026-10-23: the routed address on a Hyper-V external switch
    /// is the machine's LAN address, and a CGNAT address on the Wi-Fi card
    /// is the card's own. Neither is swapped for another adapter's.
    #[test]
    fn red_team_an_external_switch_or_a_cgnat_card_keeps_its_routed_address() {
        let ip = |text: &str| text.parse::<std::net::IpAddr>().unwrap();
        let mask = ip("255.255.255.0");
        let hyper_v = itsanas_discover::LocalNets::from_interfaces([
            ("vEthernet (External)".to_owned(), ip("192.168.1.5"), mask),
            ("Wi-Fi".to_owned(), ip("10.9.8.7"), mask),
        ]);
        let route: SocketAddr = "192.168.1.5:9797".parse().unwrap();
        assert_eq!(
            on_a_real_adapter(route, &hyper_v),
            route,
            "the external switch's LAN address was swapped for another card's"
        );
        let hotspot = itsanas_discover::LocalNets::from_interfaces([
            ("Wi-Fi".to_owned(), ip("100.70.1.2"), ip("255.192.0.0")),
            ("Ethernet".to_owned(), ip("192.168.1.9"), mask),
        ]);
        let route: SocketAddr = "100.70.1.2:9797".parse().unwrap();
        assert_eq!(
            on_a_real_adapter(route, &hotspot),
            route,
            "a CGNAT address on the Wi-Fi card was swapped for another card's"
        );
    }

    #[test]
    fn a_configured_announce_is_published_instead_of_the_local_address() {
        // The whole point of the setting. Without it a node behind a router
        // publishes its address on the LAN it is on, which is what every
        // machine in another house cannot use.
        let config = crate::config::Config {
            announce: Some("ngas.fr:9801".to_owned()),
            ..crate::config::Config::default()
        };
        let local = "192.168.1.11:41999".parse().unwrap();
        assert_eq!(
            published_address(&config, "0.0.0.0:9797", local),
            "ngas.fr:9801"
        );
    }

    #[test]
    fn the_announced_port_is_not_replaced_by_the_listening_one() {
        // THE BUG THIS PREVENTS: a port forward exists to map an outside port
        // to a different inside one. `ngas.fr:9801 -> 192.168.1.11:9797` is the
        // normal shape of one. Substituting the listening port here would
        // publish ngas.fr:9797, where the router forwards nothing, and the
        // failure would look like the peer being offline.
        let config = crate::config::Config {
            announce: Some("ngas.fr:9801".to_owned()),
            ..crate::config::Config::default()
        };
        let local = "192.168.1.11:41999".parse().unwrap();
        assert!(published_address(&config, "0.0.0.0:9797", local).ends_with(":9801"));
    }

    #[test]
    fn a_machine_that_moves_still_publishes_where_it_is() {
        // No announce is the right configuration for a laptop: it has no
        // address another network can dial. It must still publish something --
        // announcing is also the heartbeat the coordinator counts availability
        // from, and a node that stopped announcing would be counted as gone.
        let config = crate::config::Config::default();
        let friends_house = "10.42.0.7:41999".parse().unwrap();
        // No IPv6 at the friend's: this machine's own would be taken over it.
        assert_eq!(
            publishable(&config, "0.0.0.0:9797", friends_house, None, None, || None),
            "10.42.0.7:9797",
            "a laptop away from home published nothing, and was counted as gone"
        );
    }

    #[test]
    fn an_unspecified_listen_address_is_not_what_gets_published() {
        // The default is `0.0.0.0:9797`, and it is the right default: a node
        // should accept from every interface. Publishing it is a different
        // statement, and a false one — nobody can dial 0.0.0.0. A peer that
        // looked this device up got that string, and `register` printed
        // "announced 0.0.0.0:9797" as though something had been achieved.
        let local = "192.168.1.81:54321".parse().unwrap();
        assert_eq!(
            reachable_address("0.0.0.0:9797", local),
            "192.168.1.81:9797"
        );
    }

    #[test]
    fn the_published_port_is_the_listening_one_not_the_one_dialled_from() {
        // The local end of the connection to the coordinator carries an
        // ephemeral source port. Taking the address from it and the port with
        // it would publish somewhere nothing is listening — which fails later,
        // elsewhere, and looks like a network problem.
        let local = "10.0.0.5:41999".parse().unwrap();
        assert_eq!(reachable_address("0.0.0.0:9797", local), "10.0.0.5:9797");
        assert_eq!(reachable_address("[::]:9797", local), "10.0.0.5:9797");
    }

    #[test]
    fn an_address_somebody_chose_is_left_alone() {
        // Substitution is for the case where the configuration says "anywhere".
        // A specific address, or a hostname, is a decision, and overruling it
        // with whatever interface happened to reach the coordinator would break
        // exactly the setups that were configured on purpose.
        let local = "192.168.1.81:54321".parse().unwrap();
        assert_eq!(
            reachable_address("203.0.113.7:9797", local),
            "203.0.113.7:9797"
        );
        assert_eq!(
            reachable_address("nas.example.org:9797", local),
            "nas.example.org:9797"
        );
    }
}
