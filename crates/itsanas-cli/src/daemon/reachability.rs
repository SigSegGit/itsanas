//! Where this machine listens and how the others learn it: the listener and
//! the LAN socket, the enrolment at start, what it publishes, and what its
//! peers say about reaching it.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

use itsanas_discover::Lan;
use itsanas_net::PeerServer;
use itsanas_node::contact::Contact;

use super::{pledge::forget_deleted, snapshot::Outage};
use crate::{
    coordinator,
    error::{CliError, Result},
    node::Node,
};

/// The LAN discovery socket, if wanted and free.
pub(super) fn bind_lan(discover: bool) -> Option<Lan> {
    // Discovery is an optimisation, so a failure to bind is a warning and not
    // an exit. The commonest cause is a second node on the same machine, which
    // is a test rig rather than a mistake, and it must not stop the daemon that
    // can still sync with its configured peers.
    if discover {
        // `ITSANAS_DISCOVERY_PORT` moves a test rig off 21037, so two daemons a
        // test starts hear each other and not a real node on the same machine
        // (`tests/route.rs`). Anything that is not a port is the default.
        let port = std::env::var("ITSANAS_DISCOVERY_PORT")
            .ok()
            .and_then(|port| port.parse::<u16>().ok())
            .filter(|port| *port != 0)
            .unwrap_or(itsanas_discover::DEFAULT_PORT);
        match Lan::bind(port) {
            Ok(lan) => Some(lan),
            Err(error) => {
                eprintln!(
                    "itsanas: local discovery is off ({error}). Peers must be added by hand."
                );
                None
            }
        }
    } else {
        None
    }
}

/// A key's enrolment at its first start, when the copy could not enrol it
/// (`copy_to_key::enrol_at_start`); prints what happened.
///
/// Registers without announcing the configured `listen`, then announces
/// `bound`, the address this daemon really listens on (red team,
/// 2026-10-23): run beside the rounds' own publication, announcing the
/// configured port of a daemon started with `--listen` elsewhere sent the
/// account's machines to a port nobody was on until the next publication.
pub(super) fn enrol_once(node: &Node, bound: std::net::SocketAddr) {
    let enrolled = enrol_at_daemon_start(node, &bound.to_string(), |node, listen| {
        crate::register_only(node)?;
        // Best effort: the rounds publish this address too, and keep at it
        // until the coordinator takes it.
        if let Err(error) = coordinator::announce(node, listen, itsanas_discover::now_unix()) {
            eprintln!("itsanas: enrolled, but could not announce {listen} yet: {error}");
        }
        Ok(())
    });
    if let Some(line) = enrolled {
        println!("{line}");
    }
}

/// [`enrol_once`]'s decision, with the enrolment given: `enrol` is handed the
/// node and `bound`, the address to announce.
///
/// A `registered` marker naming another coordinator than the configured one
/// is stale -- `itsanas coordinator` pointed this node elsewhere since -- and
/// is dropped, so the node enrols with the one it now uses (red team,
/// 2026-10-23); `copy_to_key::enrol_at_start` then does the rest.
pub(super) fn enrol_at_daemon_start(
    node: &Node,
    bound: &str,
    enrol: impl FnOnce(&Node, &str) -> Result<()>,
) -> Option<String> {
    if let Some(configured) = &node.config.coordinator
        && crate::registered_with(&node.home).is_some_and(|marked| marked != *configured)
    {
        let _ = std::fs::remove_file(node.home.join(crate::REGISTERED));
    }
    crate::copy_to_key::enrol_at_start(node, |node| enrol(node, bound))
}

/// What this daemon knows about being reachable, and what it has already said.
///
/// Three facts, none of which a node can work out alone, and the rule for when
/// it is worth asking again. A machine that cannot be reached is not broken --
/// a laptop never can be -- so none of this is an error; it is the difference
/// between a member who knows their forward is wrong and one who thinks the
/// network is quiet.
pub(super) struct Reach {
    /// The address published on the last round that reached the coordinator.
    published: Option<String>,
    /// Whether the coordinator has said it could reach us here.
    verdict: Option<bool>,
    /// Connections from outside at the last check, so a first one is news.
    seen_from_outside: u64,
}

impl Reach {
    pub(super) const fn new() -> Self {
        Self {
            published: None,
            verdict: None,
            seen_from_outside: 0,
        }
    }

    /// Whether the coordinator should be asked to try reaching us.
    ///
    /// Only when the answer can have changed: the address is new, or it is the
    /// first round of this daemon and nobody has ever said. A probe costs the
    /// coordinator an outbound connection, and a fleet that asked every round
    /// would turn a diagnostic into the load it is meant to prevent.
    fn should_ask(&self, published: &str) -> bool {
        self.verdict.is_none() || self.published.as_deref() != Some(published)
    }
}

/// Ask the coordinator to try reaching this machine, and say what it found.
///
/// Said once per answer, not once per round: the line matters the first time
/// and is noise the twentieth.
pub(super) fn check_reachable(node: &Node, published: &str, reach: &mut Reach) {
    if !reach.should_ask(published) {
        return;
    }
    reach.published = Some(published.to_owned());

    match coordinator::check_me(node) {
        Ok(answer) => {
            let found = coordinator::found_address();
            let heard = hear_reachability(answer, published, found.as_deref(), reach);
            for line in &heard.lines {
                println!("{line}");
            }
            if heard.withdraw_found {
                coordinator::set_found_address(None);
            }
        }
        Err(error) => eprintln!("itsanas: could not ask whether we are reachable: {error}"),
    }
}

/// What the daemon does with the coordinator's answer about `published`.
pub(super) struct Heard {
    /// What to print, already said once per verdict.
    pub(super) lines: Vec<String>,
    /// Whether the address the router said it opened stops being published.
    pub(super) withdraw_found: bool,
}

/// [`check_reachable`]'s decisions, without the coordinator or the statics.
///
/// `found` is the address `UPnP` or NAT-PMP gave (`None` with an `announce`,
/// which is never found). When the coordinator could not reach this device
/// there, the router's yes was not worth what it said: a hand-made forward of
/// the same port to another machine wins over the router's own mapping (hand
/// test 2026-10-22: the home address's port 9799 answered as the VM), or a
/// firewall drops what the router lets in. Kept, that address sends every
/// member to the wrong machine at every round, and it is also what tells
/// [`fn@unreachable`] that this machine needs no relay. Withdrawn, the next
/// publication is the honest local one and the parking thread finds a relay.
/// For the rest of this run: a NAT-PMP renewal that reports a new address
/// publishes it again, and the new address is probed in its turn.
pub(super) fn hear_reachability(
    answer: Option<coordinator::Reachability>,
    published: &str,
    found: Option<&str>,
    reach: &mut Reach,
) -> Heard {
    let mut heard = Heard {
        lines: Vec::new(),
        withdraw_found: false,
    };
    match answer {
        Some(coordinator::Reachability::Reachable(detail)) => {
            if reach.verdict != Some(true) {
                heard.lines.push(format!("reachable: {detail}"));
            }
            reach.verdict = Some(true);
        }
        Some(coordinator::Reachability::Unreachable(detail)) => {
            if reach.verdict != Some(false) {
                heard
                    .lines
                    .push(format!("NOT reachable from outside: {detail}"));
                heard
                    .lines
                    .push("  Other members cannot dial this machine. It can still take".to_owned());
                heard
                    .lines
                    .push("  part by dialling them, and one reachable side per pair is".to_owned());
                heard.lines.push(
                    "  enough -- but if a forward was meant to work, it does not.".to_owned(),
                );
            }
            reach.verdict = Some(false);
            // The published form is the found address, then perhaps this
            // machine's IPv6 address (`with_global_ipv6`); the probe tried
            // the whole list.
            if let Some(found) = found
                && published.split(',').next() == Some(found)
            {
                heard.lines.push(format!(
                    "  The router said it opened {found}, but that is not this machine: \
                     it is no longer published, and this machine reaches the others \
                     through a relay of the account if one is up."
                ));
                heard.withdraw_found = true;
            }
        }
        // Not an answer, so it does not become one. Leaving the verdict alone
        // means the next round asks again rather than reporting a guess.
        Some(coordinator::Reachability::Unknown(_)) => {}
        // Too old to answer. Not a verdict, and saying nothing is right: the
        // alternative is telling a member their forward is broken when what is
        // out of date is the coordinator.
        None => reach.verdict = None,
    }
    heard
}

/// Say the first time something from outside this network arrives.
///
/// The free half of the same question. A connection from a public address is
/// proof the forward works, and it costs nothing to notice -- where the probe
/// costs the coordinator a connection. A node with an `announce` set and none
/// of these has something wrong between it and the internet.
pub(super) fn note_inbound(witness: &itsanas_net::transport::Witness, reach: &mut Reach) {
    let outside = witness.from_outside();
    if outside > reach.seen_from_outside && reach.seen_from_outside == 0 {
        println!("something from outside this network reached us: the way in works");
    }
    reach.seen_from_outside = outside;
}

/// others are -- when `contact` says it is due. Returns the address published.
pub(super) fn publish(
    node: &Node,
    shutdown: &AtomicBool,
    contact: &mut Contact,
    bound: std::net::SocketAddr,
    outage: &mut Outage,
) -> Option<String> {
    let mut announced: Option<String> = None;
    if node.config.coordinator.is_some() && !shutdown.load(Ordering::Relaxed) {
        let listen = bound.to_string();
        let here = coordinator::address_now(&node.config, &listen);
        let due = contact.due(here.as_deref(), Instant::now());
        if due.any() {
            contact.asked(&due, Instant::now());
            match coordinator::contact(node, &listen, itsanas_discover::now_unix(), &due) {
                Ok(contacted) => {
                    outage.succeeded();
                    forget_deleted(node, shutdown);
                    if let Some(published) = contacted.published {
                        contact.published(here.as_deref(), Instant::now());
                        announced = Some(published);
                    }
                    if let Some(found) = contacted.found {
                        contact.read(
                            &found,
                            &contacted.presences,
                            &contacted.claimed,
                            Instant::now(),
                        );
                        if contacted.signed {
                            contact.signed();
                        } else {
                            eprintln!(
                                "itsanas: the coordinator does not sign its list of this \
                                 account's machines (it is older than this node); their \
                                 addresses are taken on its word until it is upgraded"
                            );
                        }
                    }
                    if let Some(error) = contacted.read_failed {
                        eprintln!(
                            "itsanas: published, but could not list the account's machines: {error}"
                        );
                    }
                    if let Some(error) = contacted.pledges_not_kept {
                        eprintln!("itsanas: could not keep the account's pledges: {error}");
                    }
                    if contacted.forged > 0 {
                        eprintln!(
                            "itsanas: the coordinator listed {} address(es) their machine never \
                             signed; ignored. It is lying, or it is not the coordinator you \
                             meant (pin it with its device id).",
                            contacted.forged
                        );
                    }
                }
                Err(error) => outage.failed(&error.to_string()),
            }
        }
    }
    announced
}

/// Whether an address is this node, for [`dial_listed`]: this machine's
/// addresses at the port the daemon is **bound** to -- not the configured
/// `listen`, which a daemon started with `--listen` does not use (the same
/// mistake the enrolment made, 2026-10-23) -- or what it publishes, written
/// (`announce`) or opened by the router (`found`).
pub(super) fn this_listener(
    announce: Option<String>,
    found: Option<String>,
    bound: std::net::SocketAddr,
    here: itsanas_discover::LocalNets,
) -> impl Fn(&str) -> bool {
    let published: Vec<String> = announce.into_iter().chain(found).collect();
    move |address: &str| {
        let published: Vec<&str> = published.iter().map(String::as_str).collect();
        itsanas_node::contact::is_this_listener(address, bound.port(), &here, &published)
    }
}

/// Bind the peer listener, and say what to do when the port is taken.
///
/// Nodes created before `init` chose a free port all listen on 9797, so a
/// second account on such a machine fails here -- and under systemd it
/// restarts every thirty seconds with "address in use" in a journal. The error
/// now names a port that is free and not claimed by another node here, and the
/// two commands that move this node to it.
pub(super) fn bind_listener(node: &Node, address: &str) -> Result<PeerServer> {
    match PeerServer::bind(address) {
        Ok(server) => Ok(server),
        Err(itsanas_net::NetError::Io(error))
            if matches!(
                error.kind(),
                std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied
            ) =>
        {
            let free =
                crate::first_free_port(&crate::sibling_ports(&node.home), crate::port_is_free);
            Err(CliError::Usage(taken_port_message(address, &error, free)))
        }
        Err(other) => Err(other.into()),
    }
}

/// The sentence for a listen address something else already holds.
pub(crate) fn taken_port_message(
    address: &str,
    error: &dyn std::fmt::Display,
    free: Option<u16>,
) -> String {
    match free {
        Some(port) => format!(
            "cannot serve on {address} ({error}): another node or program has it, or the \
             system reserves it. \
             Port {port} is free here: `itsanas listen 0.0.0.0:{port}`, then \
             `itsanas register` if a coordinator is configured, then start again."
        ),
        None => format!(
            "cannot serve on {address} ({error}), and no port from 9797 to 9896 is free \
             here; choose one with `itsanas listen`."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The daemon's side of `red_team_an_address_that_is_this_node_is_never_dialled_for_another_device`
    /// (contact.rs): the port that makes an address this node is the one the
    /// daemon is bound to, and both published addresses count. A node
    /// configured for 9797 and started on 9811 is at 9811; 9797 on the same
    /// machine is the next account.
    #[test]
    fn the_daemon_takes_itself_at_its_bound_port_and_its_published_addresses() {
        let ip = |text: &str| text.parse::<std::net::IpAddr>().unwrap();
        let laptop = itsanas_discover::LocalNets::from_interfaces([(
            "Wi-Fi".to_owned(),
            ip("192.168.1.142"),
            ip("255.255.255.0"),
        )]);
        let is_me = this_listener(
            Some("ngas.fr:9811".to_owned()),
            Some("86.238.26.80:9811".to_owned()),
            "0.0.0.0:9811".parse().expect("bound"),
            laptop,
        );
        for itself in ["192.168.1.142:9811", "ngas.fr:9811", "86.238.26.80:9811"] {
            assert!(
                is_me(itself),
                "{itself} is this daemon and would be dialled for another device"
            );
        }
        assert!(
            !is_me("192.168.1.142:9797"),
            "the configured port was taken for the bound one: the next account on this \
             machine would never be dialled"
        );
    }

    /// Red team, 2026-10-23. A key that enrols at its first daemon start ran
    /// `register_and_announce`, which announced the configured `listen`
    /// beside the rounds' publication of the bound one: a daemon started on
    /// another port sent the account to the configured port until the next
    /// publication. It announces the bound address. And a `registered` marker
    /// is per coordinator: pointed at another one, the node enrols there at
    /// its next start; pointed at the same, it does not enrol again.
    #[test]
    fn red_team_a_daemon_enrols_at_its_bound_address_with_the_coordinator_it_uses() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (mut node, _) = Node::create(
            &dir.path().join("key"),
            "the passphrase of this key",
            "camille",
        )
        .expect("node");
        node.config.coordinator = Some("198.51.100.7:9797".to_owned());
        node.config.listen = "0.0.0.0:9797".to_owned();

        let mut announced = None;
        let said = enrol_at_daemon_start(&node, "0.0.0.0:9811", |_, listen| {
            announced = Some(listen.to_owned());
            Ok(())
        });
        assert!(said.is_some_and(|line| line.contains("198.51.100.7:9797")));
        assert_eq!(
            announced.as_deref(),
            Some("0.0.0.0:9811"),
            "the enrolment announced another address than the one the daemon is bound to"
        );
        assert_eq!(
            crate::registered_with(&node.home).as_deref(),
            Some("198.51.100.7:9797"),
            "the marker does not name the coordinator enrolled with"
        );
        assert!(
            enrol_at_daemon_start(&node, "0.0.0.0:9811", |_, _| panic!("enrolled twice")).is_none(),
            "a node enrolled with its coordinator enrolled again"
        );

        node.config.coordinator = Some("203.0.113.9:9797".to_owned());
        let mut asked = 0;
        let said = enrol_at_daemon_start(&node, "0.0.0.0:9811", |_, _| {
            asked += 1;
            Ok(())
        });
        assert_eq!(
            asked, 1,
            "pointed at another coordinator, the node did not enrol there"
        );
        assert!(said.is_some_and(|line| line.contains("203.0.113.9:9797")));
        assert_eq!(
            crate::registered_with(&node.home).as_deref(),
            Some("203.0.113.9:9797")
        );
    }
}
