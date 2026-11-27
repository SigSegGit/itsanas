//! Being reachable from outside: the router port, and parking on a relay
//! while no direct address answers.

use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use itsanas_crypto::DeviceId;
use itsanas_net::PeerServer;
use itsanas_node::contact::SharedBoard;

use super::shutdown::pause;
use crate::{coordinator, node::Node};

/// The home router's port, opened by `UPnP` when nothing was written: what
/// makes a machine behind a box reachable without anybody touching the box.
/// On its own thread, so a silent router costs the start nothing; the
/// address found is announced from the next round.
/// And, beside it, the park on a relay while no port is open (§8 0v).
pub(super) fn open_the_router_port<'scope>(
    scope: &'scope std::thread::Scope<'scope, '_>,
    node: &'scope Node,
    port: Option<u16>,
    parking: (
        &'scope PeerServer,
        &'scope itsanas_net::PeerService<'scope>,
        &'scope SharedBoard,
    ),
    shutdown: &'scope AtomicBool,
) {
    let (server, service, board) = parking;
    scope.spawn(move || park_while_unreachable(node, server, service, board, shutdown));
    let Some(port) = port else { return };
    if node.config.announce.is_some()
        || node.config.coordinator.is_none()
        || std::env::var_os("ITSANAS_NO_UPNP").is_some()
    {
        return;
    }
    open_router(scope, port, shutdown);
}

pub(super) fn open_router<'scope>(
    scope: &'scope std::thread::Scope<'scope, '_>,
    port: u16,
    shutdown: &'scope AtomicBool,
) {
    scope.spawn(move || match crate::upnp::open_port(port) {
        Ok((gateway, address)) => {
            println!("itsanas: the router opened port {port} (UPnP); reachable at {address}");
            coordinator::set_found_address(Some(address.clone()));
            crate::upnp::renew(&gateway, port, address, shutdown, |address| {
                println!("itsanas: the router's address changed; now reachable at {address}");
                coordinator::set_found_address(Some(address));
            });
        }
        // A router that speaks no UPnP may speak NAT-PMP (HANDOVER §8 0u).
        Err(why) => match crate::natpmp::open_port(port) {
            Ok(mapping) => {
                println!(
                    "itsanas: the router opened port {port} (NAT-PMP); reachable at {}",
                    mapping.address
                );
                coordinator::set_found_address(Some(mapping.address.clone()));
                crate::natpmp::renew(mapping, port, shutdown, |address| {
                    println!("itsanas: the router's address changed; now reachable at {address}");
                    coordinator::set_found_address(Some(address));
                });
            }
            Err(also) => {
                println!(
                    "itsanas: no port opened on the router ({why}; {also}); this machine dials out"
                );
            }
        },
    });
}

/// How long the parking thread waits between looks: for the router's answer
/// at the start, for a relay to be known, after a refused park.
pub(super) const PARK_PAUSE: Duration = Duration::from_secs(60);

/// Park on a machine of the account that can be reached, for as long as this
/// one cannot (§8 0v): no `announce`, no port the router opened. A caller who
/// cannot dial this machine reaches it through the park; what it says over it
/// is sealed end to end. Off with no coordinator, or `ITSANAS_NO_RELAY`.
pub(super) fn park_while_unreachable(
    node: &Node,
    server: &PeerServer,
    service: &itsanas_net::PeerService<'_>,
    board: &SharedBoard,
    shutdown: &AtomicBool,
) {
    if node.config.announce.is_some()
        || node.config.coordinator.is_none()
        || std::env::var_os("ITSANAS_NO_RELAY").is_some()
    {
        return;
    }
    let site = Parking {
        node,
        server,
        service,
        board,
        shutdown,
    };
    park_loop(&site, node.store.device_id(), shutdown, || {
        pause(PARK_PAUSE, shutdown)
    });
}

/// What the parking loop needs from the world, so its decisions can be
/// driven by a test without a socket, a coordinator or a minute per look.
pub(super) trait ParkSite {
    /// Whether nothing can dial this machine directly ([`fn@unreachable`]).
    fn unreachable(&self) -> bool;
    /// The relays the book knows, best first.
    fn relays(&self) -> Vec<(DeviceId, String)>;
    /// Park on `relay` until it hands over a caller (`Ok(true)`), goes quiet
    /// or closes the park (`Ok(false)`), or refuses it (`Err`).
    fn park(&self, relay: DeviceId, address: &str) -> itsanas_net::Result<bool>;
    /// The relay this machine announces itself behind, if any.
    fn announce(&self, relay: Option<(DeviceId, String)>);
}

/// The daemon's [`ParkSite`]: its listener, its book, the coordinator's
/// announcement.
pub(super) struct Parking<'a> {
    node: &'a Node,
    server: &'a PeerServer,
    service: &'a itsanas_net::PeerService<'a>,
    board: &'a SharedBoard,
    shutdown: &'a AtomicBool,
}

impl ParkSite for Parking<'_> {
    fn unreachable(&self) -> bool {
        unreachable(self.node, self.server)
    }

    fn relays(&self) -> Vec<(DeviceId, String)> {
        self.board.relays()
    }

    fn park(&self, relay: DeviceId, address: &str) -> itsanas_net::Result<bool> {
        self.server.serve_parked(
            itsanas_net::AddressList(address),
            relay,
            self.service,
            &self.node.device,
            self.shutdown,
        )
    }

    fn announce(&self, relay: Option<(DeviceId, String)>) {
        coordinator::set_parked_on(relay);
    }
}

/// The parking loop: one look per `wait()` (which says `false` once
/// stopping), parked for as long as this machine is unreachable.
pub(super) fn park_loop(
    site: &impl ParkSite,
    me: DeviceId,
    shutdown: &AtomicBool,
    mut wait: impl FnMut() -> bool,
) {
    let mut said: Option<String> = None;
    // Relays that refused or closed the park, skipped until every one has.
    let mut failed: BTreeSet<DeviceId> = BTreeSet::new();
    while wait() {
        if !site.unreachable() {
            continue;
        }
        let choices: Vec<_> = site
            .relays()
            .into_iter()
            .filter(|(d, _)| *d != me)
            .collect();
        if choices.iter().all(|(d, _)| failed.contains(d)) {
            failed.clear();
        }
        let Some((relay, address)) = choices.into_iter().find(|(d, _)| !failed.contains(d)) else {
            continue;
        };
        // Parked again after each caller, for as long as this machine is
        // still unreachable and the relay still the book's.
        while site.unreachable()
            && site
                .relays()
                .iter()
                .any(|(d, a)| *d == relay && *a == address)
        {
            // Kept set across re-parks, so the address announced does not
            // flip at every caller; cleared once the park is given up.
            site.announce(Some((relay, address.clone())));
            match site.park(relay, &address) {
                Ok(true) if !shutdown.load(Ordering::Relaxed) => {}
                Ok(true) => return,
                Ok(false) => {
                    site.announce(None);
                    failed.insert(relay);
                    break;
                }
                Err(error) => {
                    site.announce(None);
                    failed.insert(relay);
                    let line = format!("itsanas: could not park on {address} ({error})");
                    if said.as_deref() != Some(&line) {
                        println!("{line}");
                        said = Some(line);
                    }
                    break;
                }
            }
        }
        site.announce(None);
    }
}

/// Whether nothing can dial this machine directly ([`needs_relay`]), from
/// the router's port, the listener's witness and the direct address.
pub(super) fn unreachable(node: &Node, server: &PeerServer) -> bool {
    let listen = server
        .local_addr()
        .map(|bound| bound.to_string())
        .unwrap_or_default();
    needs_relay(
        coordinator::found_address().as_deref(),
        server.witness().from_outside(),
        coordinator::direct_address(&node.config, &listen).as_deref(),
    )
}

/// Whether nothing can dial this machine directly: no port the router
/// opened (`found`), no caller from outside its network ever arrived
/// (`from_outside`), and the address it would publish (`direct`) proves
/// nothing by itself. A machine with a public IPv4 address, or a forward
/// somebody made by hand, never parks: a relay is slower than a direct path,
/// and an address replaced by `relay:` is one same-LAN machines would
/// hairpin through.
///
/// A global IPv6 address is not proof. It is this machine's own, with no NAT
/// in front, but a home router's IPv6 firewall drops unsolicited callers
/// (RFC 6092 asks it to by default) and so does Windows Defender Firewall
/// without a rule, which the installer only suggests. Taken as reachable, a
/// laptop with IPv6 stopped parking and was dialled at an address that
/// answered nobody from another house. So it parks until a caller from
/// outside has arrived: the coordinator's probe of the first publication
/// (which names the IPv6 address, before the first park a minute later),
/// or a peer, over IPv6 or over a forward. A caller from this machine's own
/// /64 does not count (`itsanas_net::transport::Witness`). Once parked, the
/// presence names the relay only, so a machine whose way in does work but
/// whose coordinator shares its /64 stays parked: slower, never unreachable.
pub(super) fn needs_relay(found: Option<&str>, from_outside: u64, direct: Option<&str>) -> bool {
    let proves_nothing = |address: &str| {
        itsanas_node::coordinator::is_private_address(address)
            || address
                .split(',')
                .any(|entry| matches!(entry.trim().parse(), Ok(std::net::SocketAddr::V6(_))))
    };
    found.is_none() && from_outside == 0 && direct.is_none_or(proves_nothing)
}

#[cfg(test)]
mod tests {
    use super::super::reachability::{Reach, hear_reachability};
    use super::super::tests::id;
    use super::*;

    /// Red team on #346. Windows' listener took IPv6 on 2026-10-30, so the
    /// laptop at home now has a global IPv6 direct address, and that address
    /// is not private: taken as proof, it stopped parking on a relay. Windows
    /// Defender Firewall drops an unsolicited caller without a rule (the
    /// installer only suggests one), and so does a home router's IPv6
    /// firewall, so nobody from another house reached it at all. Sabotage:
    /// drop the IPv6 clause of `proves_nothing` in `needs_relay`.
    #[test]
    fn red_team_a_global_ipv6_address_alone_does_not_stop_a_machine_parking() {
        let laptop = "[2a01:e0a:1045:9d30:4839:531c:38a0:648f]:9797";
        assert!(
            needs_relay(None, 0, Some(laptop)),
            "a laptop whose only proof is its own global IPv6 address stopped parking: \
             behind a firewall that drops inbound IPv6, no other house can reach it"
        );
        assert!(
            !needs_relay(None, 1, Some(laptop)),
            "a caller from outside reached the IPv6 address and the machine still parked: \
             its peers took a slower relay for a way in that works"
        );
        assert!(
            !needs_relay(None, 0, Some("88.170.12.34:9797")),
            "a public IPv4 address parked: everybody can dial it, and a relay only slows them"
        );
        assert!(needs_relay(None, 0, Some("192.168.1.142:9797")));
        assert!(needs_relay(None, 0, None));
        assert!(
            !needs_relay(
                Some("88.170.12.34:9797"),
                0,
                Some(&format!("88.170.12.34:9797,{laptop}"))
            ),
            "a machine whose router opened its port parked because the list also named IPv6"
        );
    }

    /// Red team, hand test 2026-10-22. The laptop's third account listens on
    /// 9799, the router said yes to opening 9799 for it, and a hand-made
    /// forward of 9799 to the VM answered instead: the coordinator's probe
    /// said so, and the daemon went on publishing the address. Every member
    /// dialled the VM there at every round and was refused, and the laptop,
    /// believing itself reachable, never parked on a relay. An address the
    /// router gave and the probe disproves is withdrawn; an `announce`
    /// somebody wrote is theirs to fix, and stays.
    #[test]
    fn red_team_a_router_port_that_reaches_another_machine_is_withdrawn() {
        let other = || {
            Some(coordinator::Reachability::Unreachable(
                "203.0.113.9:9799 answered, but not as this device".to_owned(),
            ))
        };
        let mut reach = Reach::new();
        let heard = hear_reachability(
            other(),
            "203.0.113.9:9799",
            Some("203.0.113.9:9799"),
            &mut reach,
        );
        assert!(
            heard.withdraw_found,
            "a router port the coordinator found another machine behind stayed published: \
             every member keeps dialling the wrong machine, and this one never parks on a relay"
        );
        assert!(
            heard
                .lines
                .iter()
                .any(|line| line.contains("no longer published")),
            "the withdrawal was not said: {:?}",
            heard.lines
        );

        // With this machine's IPv6 address after it, the probe tried both.
        let mut reach = Reach::new();
        let listed = hear_reachability(
            other(),
            "203.0.113.9:9799,[2001:db8::7]:9799",
            Some("203.0.113.9:9799"),
            &mut reach,
        );
        assert!(
            listed.withdraw_found,
            "the list form kept a disproved router port"
        );

        // A written `announce` is never found: told, not withdrawn.
        let mut reach = Reach::new();
        let written = hear_reachability(other(), "ngas.fr:9799", None, &mut reach);
        assert!(!written.withdraw_found);
        assert!(
            !written.lines.is_empty(),
            "an unreachable announce went unsaid"
        );

        // A router port that does reach this machine stays.
        let mut reach = Reach::new();
        let good = hear_reachability(
            Some(coordinator::Reachability::Reachable(
                "it is this device".to_owned(),
            )),
            "203.0.113.9:9799",
            Some("203.0.113.9:9799"),
            &mut reach,
        );
        assert!(!good.withdraw_found, "a working router port was withdrawn");

        // No verdict is no reason either.
        for answer in [
            Some(coordinator::Reachability::Unknown(
                "probed within the hour".to_owned(),
            )),
            None,
        ] {
            let mut reach = Reach::new();
            let unsure = hear_reachability(
                answer,
                "203.0.113.9:9799",
                Some("203.0.113.9:9799"),
                &mut reach,
            );
            assert!(
                !unsure.withdraw_found,
                "a router port was withdrawn on no verdict"
            );
        }
    }

    /// A scripted world for [`park_loop`]: each park pops the next answer
    /// (and may make the machine reachable), every park and announcement is
    /// written down.
    struct FakeSite {
        unreachable: std::cell::Cell<bool>,
        relays: Vec<(DeviceId, String)>,
        answers: std::cell::RefCell<std::collections::VecDeque<(Option<bool>, bool)>>,
        parks: std::cell::RefCell<Vec<DeviceId>>,
        announced: std::cell::RefCell<Vec<Option<DeviceId>>>,
    }

    impl FakeSite {
        /// `answers`: per park, `Some(served)` or `None` for a refusal, and
        /// whether the machine is reachable once it returns.
        fn new(relays: &[DeviceId], answers: &[(Option<bool>, bool)]) -> Self {
            Self {
                unreachable: std::cell::Cell::new(true),
                relays: relays
                    .iter()
                    .map(|d| (*d, format!("203.0.113.{}:9797", d.as_bytes()[0])))
                    .collect(),
                answers: std::cell::RefCell::new(answers.iter().copied().collect()),
                parks: std::cell::RefCell::default(),
                announced: std::cell::RefCell::default(),
            }
        }

        fn parks(&self) -> Vec<DeviceId> {
            self.parks.borrow().clone()
        }
    }

    impl ParkSite for FakeSite {
        fn unreachable(&self) -> bool {
            self.unreachable.get()
        }

        fn relays(&self) -> Vec<(DeviceId, String)> {
            self.relays.clone()
        }

        fn park(&self, relay: DeviceId, _address: &str) -> itsanas_net::Result<bool> {
            self.parks.borrow_mut().push(relay);
            let (served, reachable) = self
                .answers
                .borrow_mut()
                .pop_front()
                .unwrap_or((Some(false), false));
            if reachable {
                self.unreachable.set(false);
            }
            served.ok_or_else(|| {
                itsanas_net::NetError::Io(std::io::Error::other("refused by the relay"))
            })
        }

        fn announce(&self, relay: Option<(DeviceId, String)>) {
            self.announced.borrow_mut().push(relay.map(|(d, _)| d));
        }
    }

    /// Run `looks` looks of the loop over `site`, as device `me`.
    fn park_looks(site: &FakeSite, me: DeviceId, looks: usize) {
        let stop = AtomicBool::new(false);
        let mut left = looks;
        park_loop(site, me, &stop, || {
            let more = left > 0;
            left = left.saturating_sub(1);
            more
        });
    }

    #[test]
    fn red_team_a_relay_that_closes_the_park_is_passed_over_for_the_next() {
        // A relay that keeps closing parks (full, or going down) would be
        // dialled at every look while a second one sat unused: this machine
        // unreachable for as long as the first stays broken.
        let site = FakeSite::new(&[id(1), id(2)], &[(Some(false), false), (None, false)]);
        park_looks(&site, id(9), 3);
        assert_eq!(
            site.parks()[..2],
            [id(1), id(2)],
            "the relay that closed the park was asked again before the other"
        );
    }

    #[test]
    fn red_team_once_every_relay_has_failed_the_first_is_tried_again() {
        // The skip list is a rotation, not a ban: a relay that was down an
        // hour ago is the one to use when it is the only one.
        let site = FakeSite::new(&[id(1)], &[(None, false), (Some(false), false)]);
        park_looks(&site, id(9), 3);
        assert!(
            site.parks().len() >= 2,
            "a machine with one relay stopped parking for good after one refusal: {:?}",
            site.parks()
        );
    }

    #[test]
    fn red_team_a_machine_never_parks_on_itself() {
        // The book lists the account's machines, this one included; parking
        // on itself is a socket to its own listener and no way in.
        let site = FakeSite::new(&[id(9), id(1)], &[]);
        park_looks(&site, id(9), 2);
        assert!(
            !site.parks().contains(&id(9)),
            "the machine parked on itself: {:?}",
            site.parks()
        );
    }

    #[test]
    fn red_team_a_parked_machine_parks_again_after_each_caller_and_withdraws_when_reached() {
        // One caller per park: without parking again the second caller finds
        // nobody. And once a direct path exists (the router opened a port),
        // the `relay:` address must leave the presence, or callers keep
        // taking the slow road through a third machine.
        let site = FakeSite::new(
            &[id(1)],
            &[(Some(true), false), (Some(true), false), (Some(true), true)],
        );
        park_looks(&site, id(9), 2);
        assert_eq!(
            site.parks(),
            vec![id(1); 3],
            "the machine did not park again after a caller, or kept parking once reachable"
        );
        assert_eq!(
            site.announced.borrow().last().copied(),
            Some(None),
            "a machine reachable again still announces itself behind a relay"
        );
    }
}
