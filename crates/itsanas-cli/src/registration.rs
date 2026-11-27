//! The coordinator: setting it, registering and announcing this machine, invitations.

use std::path::Path;

use crate::leaving::refuse_if_departed;
use crate::opening::{open, passphrase};
use crate::{config, coordinator};
use crate::{
    coordinator::mark_registered,
    error::{CliError, Result},
    node::{Node, SNAPSHOT},
};

/// Point `config` at the coordinator `address`, pinned to `device` when
/// given. The id is parsed now rather than at first use, so a mistyped one
/// fails while the person who typed it is still looking at it.
pub(crate) fn apply_coordinator(
    config: &mut config::Config,
    address: &str,
    device: Option<&str>,
) -> Result<()> {
    if let Some(device) = device {
        coordinator::parse_device(device)?;
    }
    config.coordinator = Some(address.to_owned());
    config.coordinator_device = device.map(str::to_owned);
    Ok(())
}

/// Set, show, or forget the coordinator this node uses.
pub(crate) fn coordinator_setting(
    home: &Path,
    address: Option<&str>,
    device: Option<&str>,
    forget: bool,
) -> Result<()> {
    let mut config = config::Config::load(&Node::config_path(home))?;

    if forget {
        config.coordinator = None;
        config.coordinator_device = None;
        config.save(&Node::config_path(home))?;
        println!("no coordinator configured. Machines on this network still find");
        println!("each other; machines elsewhere now need `itsanas peer add`.");
        return Ok(());
    }

    if let Some(address) = address {
        apply_coordinator(&mut config, address, device)?;
        config.save(&Node::config_path(home))?;
        println!("coordinator set to {address}");
        if let Some(device) = device {
            println!("  pinned to device {device}");
        } else {
            println!("  not pinned. Anything answering at that address is trusted to");
            println!("  be the coordinator. Pin it with --device <id> from");
            println!("  `itsanas-coordinator --identity`.");
        }
        return Ok(());
    }

    if let Some(address) = &config.coordinator {
        println!("{address}");
        match &config.coordinator_device {
            Some(device) => println!("  pinned to device {device}"),
            None => println!("  not pinned"),
        }
    } else {
        println!("no coordinator configured");
    }
    Ok(())
}

/// Draw an invitation and print it once.
pub(crate) fn invite(home: &Path, uses: u32, days: u64) -> Result<()> {
    let node = open(home)?;
    let validity = days.saturating_mul(24 * 60 * 60);
    let secret = coordinator::invite(&node, uses, validity, itsanas_discover::now_unix())?;

    println!("invitation code");
    println!();
    println!("  {}", coordinator::encode_secret(&secret));
    println!();
    println!("Send it to whoever is joining. They run:");
    println!();
    println!("  itsanas init --username <their-name>");
    println!("  itsanas coordinator {}", coordinator_address(&node));
    println!("  itsanas register --invite <the code above>");
    println!();
    if uses == 1 {
        println!("Good for one account, for {days} day(s).");
    } else {
        println!("Good for {uses} accounts, for {days} day(s).");
    }
    println!("It is not stored anywhere. Lose it and issue another.");
    Ok(())
}

/// What to tell an invitee to point at.
pub(crate) fn coordinator_address(node: &Node) -> String {
    node.config
        .coordinator
        .clone()
        .unwrap_or_else(|| "<host:port>".to_owned())
}

/// Register this account and device, and optionally lodge a recovery container.
pub(crate) fn register(
    home: &Path,
    recovery: bool,
    withdraw: bool,
    invite: Option<&str>,
) -> Result<()> {
    // `register` also announces this device's address: after `leave` that
    // would hand the peers, told it was gone, a fresh address to dial.
    refuse_if_departed(home)?;
    // Before the passphrase, as `status` asks: with the daemon up, the store
    // is its, and the open below would fail after the prompt with a message
    // about locks that does not say what matters here.
    if Node::exists(home) && itsanas_store::Store::is_locked(Node::store_path(home)) {
        return Err(CliError::Usage(register_beside_daemon(home)));
    }
    let node = open(home)?;
    let published = register_and_announce(&node, invite)?;
    println!(
        "registered {:?} and enrolled this device",
        node.config.username
    );
    match published {
        Ok(published) => println!("announced {published}"),
        Err(error) => println!("could not announce an address: {error}"),
    }

    if withdraw {
        coordinator::set_escrow(&node, None, &[])?;
        println!("recovery container withdrawn. This account can now only be");
        println!("restored with its 24-word phrase.");
        return Ok(());
    }

    if recovery {
        println!();
        println!("Lodging a recovery container. Enter this machine's passphrase again:");
        let secret = passphrase(false)?;
        coordinator::set_escrow(&node, Some(&secret), &node.secrets)?;
        println!("recovery container lodged.");
        println!(
            "  A new machine can now run `itsanas login --username {} \\",
            node.config.username
        );
        println!(
            "    --from {}`",
            node.config
                .coordinator
                .as_deref()
                .unwrap_or("<coordinator>")
        );
        println!("  Anybody who steals the coordinator's database can attack that");
        println!("  passphrase offline. Withdraw it with `itsanas register --withdraw-recovery`.");
    }

    Ok(())
}

/// Enrol this account and device with the configured coordinator and publish
/// this device's address. The outer error is the registration; the inner
/// result is the address published, or why none could be -- a device that
/// registered and could not announce is still enrolled.
///
/// For a node this process is about to run, or runs, as the machine it is
/// on: the address announced is the configured `listen` as seen from here.
/// A daemon announces the address it is bound to instead, and a key being
/// copied announces nothing ([`register_only`]).
pub(crate) fn register_and_announce(
    node: &Node,
    invite: Option<&str>,
) -> Result<std::result::Result<String, String>> {
    let now = itsanas_discover::now_unix();
    enrol(node, invite, now)?;

    // Publishing the address is part of registering, not a separate step: a
    // device nobody can reach has not really joined anything. What was
    // published is returned, not what was configured: with `listen` set to
    // every interface -- the default -- those differ, and printing the
    // configured value told the reader an address no peer can dial.
    let listen = node.config.listen.clone();
    Ok(coordinator::announce(node, &listen, now).map_err(|error| error.to_string()))
}

/// Enrol this account and device with the configured coordinator, and
/// announce nothing (red team, 2026-10-23).
///
/// For a node that is not, or not yet, the machine it would announce: a key
/// being copied (`copy-to-key`), whose registration would otherwise publish
/// the copying machine's address under the key's device id, and a daemon,
/// which publishes the address it is bound to itself. Writes the
/// `registered` marker, with the coordinator's address, as
/// [`register_and_announce`] does.
pub(crate) fn register_only(node: &Node) -> Result<()> {
    enrol(node, None, itsanas_discover::now_unix())
}

/// Register with the configured coordinator and write the `registered`
/// marker: what [`register_and_announce`] and [`register_only`] share.
/// Registering again is a refresh at the coordinator, so a node enrolled
/// before the marker existed enrols once more at its first start, harmlessly.
pub(crate) fn enrol(node: &Node, invite: Option<&str>, now: u64) -> Result<()> {
    let secret = invite.map(coordinator::decode_secret).transpose()?;
    coordinator::register_with(node, secret.as_ref(), now)?;
    // `daemon.rs`, `enrol_at_daemon_start`, reads the address it holds.
    mark_registered(node);
    Ok(())
}

/// Why `register` does not run beside this node's daemon, and where that
/// daemon listens.
///
/// `register` announces the configured `listen` (`register_and_announce`).
/// A daemon started with `--listen` elsewhere publishes the address it really
/// listens on, and an announcement of the configured one would send the
/// account's other machines to a port nobody is on (hand test, 2026-10-22).
/// The store is the daemon's while it runs, so `register` cannot open the
/// node anyway; what it can do is say where the daemon is, from the snapshot
/// the daemon writes every round, as `status` does.
pub(crate) fn register_beside_daemon(home: &Path) -> String {
    let configured = config::Config::load(&Node::config_path(home))
        .ok()
        .map(|config| config.listen);
    let listening = match (daemon_listening(home), configured) {
        (Some(bound), Some(configured)) if bound != configured => format!(
            ", listening on {bound} by its last report (not the configured {configured}); it \
             publishes that address itself, at its start and whenever it changes. `itsanas \
             listen {bound}` makes it the configured one, which `register` announces"
        ),
        (Some(bound), _) => format!(
            ", listening on {bound} by its last report; it publishes that address itself, \
             at its start and whenever it changes"
        ),
        (None, _) => String::new(),
    };
    format!(
        "this node's daemon is running{listening}. `register` opens the node, which the \
         daemon holds: stop the daemon, run `itsanas register`, and start the daemon again."
    )
}

/// Where this node's daemon said it listens, from the `listen` line of its
/// snapshot (`render_status`), if it left one.
pub(crate) fn daemon_listening(home: &Path) -> Option<String> {
    let text = std::fs::read_to_string(home.join(SNAPSHOT)).ok()?;
    text.lines()
        .find_map(|line| line.trim_start().strip_prefix("listen "))
        .and_then(|rest| rest.split_whitespace().next())
        .map(str::to_owned)
}

pub(crate) fn announce_as(home: &Path, address: Option<&str>, forget: bool) -> Result<()> {
    let mut node = open(home)?;

    if forget {
        match node.config.announce.take() {
            Some(previous) => {
                node.save_config()?;
                println!("no longer announcing {previous}");
                println!("  this node now publishes the address it reaches the");
                println!("  coordinator from, which only its own network can dial");
            }
            None => println!("nothing was being announced"),
        }
        refresh_hint(&node);
        return Ok(());
    }

    let Some(address) = address else {
        match node.config.announce.as_deref() {
            Some(announce) => println!("{announce}"),
            None => println!(
                "nothing announced: this node publishes the address it reaches the coordinator from"
            ),
        }
        return Ok(());
    };

    // Validated here and not merely stored, for the same reason `listen` is:
    // a value that cannot work must fail in front of the person who typed it,
    // not in a daemon log after the next restart.
    let announce = crate::config::parse_announce(address)?;
    let previous = node.config.announce.replace(announce.clone());
    node.save_config()?;

    match previous {
        Some(previous) if previous != announce => {
            println!("announcing {announce} from now on (was {previous})");
        }
        _ => println!("announcing {announce} from now on"),
    }
    println!("  nothing checks that this address reaches this machine: that is");
    println!("  your router's forward, or your IPv6 firewall, and a wrong one");
    println!("  makes this node unreachable rather than noisy");

    refresh_hint(&node);
    Ok(())
}

/// The coordinator keeps handing out the previous address until it is told.
///
/// Said rather than done: refreshing needs the passphrase and the coordinator
/// to be up, and a command that half-worked silently is worse than one that
/// says what is left.
pub(crate) fn refresh_hint(node: &Node) {
    if node.config.coordinator.is_some() {
        println!("  the coordinator still publishes the old address; refresh it with:");
        println!("    itsanas register");
    }
}

#[cfg(test)]
mod tests {
    use super::register;
    use crate::node::SNAPSHOT;

    /// Hand test, 2026-10-22: `register` announces the configured `listen`,
    /// and a daemon run with `--listen` elsewhere is on another port. With
    /// that daemon up, `register` says so before asking for a passphrase,
    /// and names the port the daemon really listens on, read back from the
    /// snapshot it writes -- which must therefore carry that port, not the
    /// configured one. Sabotaged (no check before the open; the snapshot
    /// printing the configured port): red.
    #[test]
    fn register_beside_a_running_daemon_names_the_port_it_really_listens_on() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        // Held open for the whole test, as a running daemon holds it.
        let (node, _) =
            itsanas_node::Node::create(&home, "a passphrase", "camille").expect("create");
        assert_eq!(node.config.listen, "0.0.0.0:9797");
        let snapshot = crate::status::render_status(&node, Some("0.0.0.0:9811")).expect("render");
        assert!(
            snapshot.contains("listen          0.0.0.0:9811"),
            "the daemon's snapshot names the configured port, not the one it listens on:\n\
             {snapshot}"
        );
        std::fs::write(
            home.join(SNAPSHOT),
            format!(
                "snapshot {} every 60\n{snapshot}",
                itsanas_discover::now_unix()
            ),
        )
        .expect("snapshot");

        let refused =
            register(&home, false, false, None).expect_err("register beside a running daemon");
        let said = refused.to_string();
        assert!(
            said.contains("daemon is running")
                && said.contains("0.0.0.0:9811")
                && said.contains("itsanas listen 0.0.0.0:9811"),
            "register did not say where the running daemon listens: {said}"
        );

        // `status` itself runs no listener: the configured port, unqualified.
        let own = crate::status::render_status(&node, None).expect("render");
        assert!(own.contains("listen          0.0.0.0:9797\n"), "{own}");
    }
}
