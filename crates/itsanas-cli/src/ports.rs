//! The port this node listens on: one no other node here takes, and `listen`.

use std::{net::SocketAddr, path::Path};

use crate::config;
use crate::opening::open;
use crate::{error::Result, node::Node};

/// The first port a new node may serve on: from 9797, and no further than this.
pub(crate) const PORT_SEARCH: std::ops::Range<u16> = 9797..9897;

/// Ports the other nodes on this machine are configured to serve on.
///
/// A sibling is any directory beside `home` holding a keystore, which is what
/// "there is a node here" means everywhere else in this program. Asking the
/// kernel alone is not enough: a node whose daemon is stopped holds no socket,
/// so a second account created while the first is down would be handed the
/// same port and the two daemons would fight over it at the next boot.
pub(crate) fn sibling_ports(home: &Path) -> std::collections::BTreeSet<u16> {
    let mut taken = std::collections::BTreeSet::new();
    let Some(parent) = home.parent() else {
        return taken;
    };
    let Ok(entries) = std::fs::read_dir(parent) else {
        return taken;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == home || !path.join("keystore.bin").is_file() {
            continue;
        }
        if let Ok(other) = config::Config::load(&Node::config_path(&path))
            && let Ok(address) = crate::config::parse_listen(&other.listen)
        {
            taken.insert(address.port());
        }
    }
    taken
}

/// The first port in [`PORT_SEARCH`] nobody claims and this machine can bind.
/// Why this node did not get the port it asked for.
///
/// The second account on a machine skipped 9797 *and* 9798 and was told only
/// that "9797 is used by another node", which reads as one sibling when there
/// are two. Somebody counting their instances from this line counts wrong, and
/// the line exists precisely because ports here are allocated behind the
/// person's back.
pub(crate) fn ports_skipped(chosen: u16) -> String {
    match chosen.saturating_sub(PORT_SEARCH.start) {
        0 => "chosen for this node".to_owned(),
        1 => format!(
            "{} is used by another node on this machine",
            PORT_SEARCH.start
        ),
        _ => format!(
            "{}-{} are used by other nodes on this machine",
            PORT_SEARCH.start,
            chosen - 1
        ),
    }
}

pub(crate) fn first_free_port(
    taken: &std::collections::BTreeSet<u16>,
    bindable: impl Fn(u16) -> bool,
) -> Option<u16> {
    PORT_SEARCH
        .clone()
        .find(|port| !taken.contains(port) && bindable(*port))
}

/// Whether a node could listen on every interface at `port` now: the bind
/// the daemon makes, so on Windows the IPv6 half is asked about too.
pub(crate) fn port_is_free(port: u16) -> bool {
    itsanas_net::listen_on(&[SocketAddr::from(([0, 0, 0, 0], port))]).is_ok()
}

/// Give a new node a port no other node on this machine serves on.
///
/// Two accounts on one machine are two daemons, and every node used to be
/// created listening on 9797: the second daemon failed to bind, exited, and
/// under systemd restarted every thirty seconds with the reason in a journal.
/// Only the default is moved -- an address somebody chose with `itsanas listen`
/// is left alone.
pub(crate) fn settle_listen_port(node: &mut Node) -> Result<()> {
    let Ok(current) = crate::config::parse_listen(&node.config.listen) else {
        return Ok(());
    };
    if !current.ip().is_unspecified() {
        return Ok(());
    }
    let taken = sibling_ports(&node.home);
    if !taken.contains(&current.port()) && port_is_free(current.port()) {
        return Ok(());
    }
    match first_free_port(&taken, port_is_free) {
        Some(port) => {
            let chosen = SocketAddr::new(current.ip(), port);
            node.config.listen = chosen.to_string();
            node.save_config()?;
            println!("  listen   : {chosen} ({})", ports_skipped(port));
        }
        None => println!(
            "warning: every port from {} to {} is taken here; choose one with `itsanas listen`",
            PORT_SEARCH.start,
            PORT_SEARCH.end - 1
        ),
    }
    Ok(())
}

pub(crate) fn listen_on(home: &Path, address: Option<&str>) -> Result<()> {
    let mut node = open(home)?;

    let Some(address) = address else {
        println!("{}", node.config.listen);
        return Ok(());
    };

    // Parsed, not merely stored. An address that does not parse is not found
    // out here but at `serve`, which on a machine running the daemon under
    // systemd means a unit that restarts every thirty seconds with the reason
    // in a journal nobody is reading.
    let parsed: SocketAddr = crate::config::parse_listen(address)?;

    let previous = std::mem::replace(&mut node.config.listen, parsed.to_string());
    node.save_config()?;

    println!("serving on {parsed} from now on (was {previous})");

    // The coordinator is still handing out the old one until it is told. Say
    // so, rather than leaving a node that is reachable and unreachable at the
    // same time depending on who you ask.
    if node.config.coordinator.is_some() {
        println!("  the coordinator still publishes the old address; refresh it with:");
        println!("    itsanas register");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::first_free_port;
    use super::sibling_ports;

    #[test]
    fn a_port_another_node_on_this_machine_is_configured_for_is_not_chosen() {
        // The case the kernel cannot see: the first account's daemon is
        // stopped, so 9797 binds, and handing it to the second account puts
        // two daemons on one port at the next boot.
        let taken = std::collections::BTreeSet::from([9797]);
        assert_eq!(first_free_port(&taken, |_| true), Some(9798));
    }

    /// The line says how many siblings there are, because that is what it is for.
    ///
    /// The second account on this machine skipped 9797 and 9798 and was told
    /// "9797 is used by another node on this machine" -- singular, and naming
    /// one of the two. Ports here are handed out without asking, so this line
    /// is the only place the person learns what happened.
    #[test]
    fn the_ports_a_node_had_to_skip_are_all_named_not_just_the_first() {
        assert!(
            super::ports_skipped(9798).contains("9797 is used"),
            "one skipped port should be named in the singular"
        );
        let two = super::ports_skipped(9799);
        assert!(
            two.contains("9797-9798") && two.contains("are used"),
            "two skipped ports were reported as one, which undercounts the \
             instances on this machine: {two}"
        );
    }

    #[test]
    fn a_port_something_already_holds_is_skipped_and_exhaustion_says_so() {
        let taken = std::collections::BTreeSet::new();
        assert_eq!(first_free_port(&taken, |port| port > 9799), Some(9800));
        assert_eq!(
            first_free_port(&taken, |_| false),
            None,
            "a port nothing can bind was offered as free"
        );
    }

    // Windows only: elsewhere std's `[::]` is dual-stack, so holding it
    // already blocks `0.0.0.0`, and a probe of `0.0.0.0` alone passed there
    // too. Run, it proved nothing it was named for.
    #[test]
    #[cfg(windows)]
    fn red_team_a_port_whose_ipv6_half_is_held_is_not_offered_as_free() {
        // On Windows a node takes `0.0.0.0` and `[::]` as two sockets. A
        // probe of `0.0.0.0` alone offered a port another program held over
        // IPv6, and the node told to move there refused to start again.
        let Ok(held) = std::net::TcpListener::bind((std::net::Ipv6Addr::UNSPECIFIED, 0)) else {
            return;
        };
        let port = held.local_addr().unwrap().port();
        assert!(
            !super::port_is_free(port),
            "port {port}, held over IPv6 by another program, was offered to a node"
        );
        drop(held);
        assert!(super::port_is_free(port), "a free port was refused");
    }

    #[test]
    fn the_ports_of_the_other_nodes_beside_this_one_are_found_and_its_own_is_not() {
        // A sibling is a directory holding a keystore. A directory without one
        // is not a node, and this node's own configuration must not count
        // against it, or re-running `init` logic would move it off its port.
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, listen: &str, keystore: bool| {
            let home = dir.path().join(name);
            std::fs::create_dir_all(&home).unwrap();
            if keystore {
                std::fs::write(home.join("keystore.bin"), b"sealed").unwrap();
            }
            let config = crate::config::Config {
                listen: listen.to_owned(),
                ..crate::config::Config::default()
            };
            config.save(&crate::node::Node::config_path(&home)).unwrap();
            home
        };
        write(".itsanas", "0.0.0.0:9797", true);
        write("not-a-node", "0.0.0.0:9799", false);
        let this = write(".itsanas-bob", "0.0.0.0:9798", true);

        assert_eq!(
            sibling_ports(&this),
            std::collections::BTreeSet::from([9797])
        );
    }

    #[test]
    fn a_taken_listen_port_is_answered_with_a_free_one_and_the_commands_to_move() {
        // Nodes made before `init` chose ports all sit on 9797. The daemon of
        // the second one used to exit with "address in use" -- under systemd,
        // every thirty seconds -- and nothing said which port would work or
        // how to move there.
        let told = crate::daemon::taken_port_message("0.0.0.0:9797", &"address in use", Some(9798));
        assert!(told.contains("itsanas listen 0.0.0.0:9798"), "{told}");
        assert!(told.contains("itsanas register"), "{told}");
        let none = crate::daemon::taken_port_message("0.0.0.0:9797", &"address in use", None);
        assert!(
            none.contains("itsanas listen") && !none.contains("0.0.0.0:9798"),
            "a port was offered where none is free: {none}"
        );
    }
}
