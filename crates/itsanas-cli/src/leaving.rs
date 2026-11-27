//! Leaving the network and coming back: `leave`, `delete-account`, `rejoin`.

use std::{fmt::Write as _, path::Path};

use itsanas_crypto::DeviceId;

use crate::opening::open;
use crate::status::describe_age;
use crate::syncing::connect_peer;
use crate::{config, coordinator};
use crate::{
    error::{CliError, Result},
    node::Node,
};

pub(crate) fn delete_account(home: &Path, confirm: &str, drain_for: u64) -> Result<()> {
    let config = config::Config::load(&Node::config_path(home))?;
    if confirm != config.username {
        return Err(CliError::Usage(format!(
            "--confirm must be this account's username, {:?}: nothing was deleted",
            config.username
        )));
    }
    if config.coordinator.is_none() {
        return Err(CliError::Usage(
            "no coordinator is set: an account only exists there, so there is nothing to delete"
                .to_owned(),
        ));
    }
    leave(home, drain_for)?;
    let node = open(home)?;
    coordinator::delete_account(&node, itsanas_discover::now_unix())?;
    println!(
        "account {} deleted at the coordinator: its hosts free its data at their next rounds, its \
         other machines stop, and the name is retired for a year",
        config.username
    );
    println!("remove this machine's own files with install/clean --purge-account --purge-folder");
    Ok(())
}

pub(crate) fn leave(home: &Path, drain_for: u64) -> Result<()> {
    let node = open(home)?;
    let me = node.store.device_id();
    // Written first: from here the daemon will not start, so nothing new is
    // accepted while the owners re-place what this machine holds.
    record_departure(home, itsanas_discover::now_unix())?;

    let mut targets: Vec<(Option<DeviceId>, String)> = node
        .config
        .peers
        .iter()
        .map(|address| (None, address.clone()))
        .collect();
    if node.config.coordinator.is_some() {
        let mut owners = vec![node.store.owner()];
        for owner in node.vault.owners()? {
            if !owners.contains(&owner) {
                owners.push(owner);
            }
        }
        for owner in owners {
            match coordinator::devices(&node, owner) {
                Ok(found) => targets.extend(
                    found
                        .into_iter()
                        .filter(|(device, _)| *device != me)
                        .map(|(device, address)| (Some(device), address)),
                ),
                Err(error) => println!("could not list the devices of {}: {error}", owner.short()),
            }
        }
    }

    drain(&node, &targets, drain_for);

    let mut told: std::collections::BTreeSet<DeviceId> = std::collections::BTreeSet::new();
    for (expect, address) in &targets {
        if expect.is_some_and(|device| told.contains(&device)) {
            continue;
        }
        let mut client = match connect_peer(address, &node.device, node.store.owner(), *expect) {
            Ok(client) => client,
            Err(error) => {
                println!("{address}: not told, unreachable ({error})");
                continue;
            }
        };
        let device = client.peer_device();
        if !told.insert(device) {
            continue;
        }
        match client.leaving() {
            Ok(true) => println!("{address}: told"),
            Ok(false) => println!("{address}: not told, too old to know the notice"),
            Err(error) => println!("{address}: not told ({error})"),
        }
    }

    if node.config.coordinator.is_some() {
        match coordinator::depart(&node, itsanas_discover::now_unix()) {
            Ok(true) => println!("coordinator: departure recorded"),
            Ok(false) => println!("coordinator: too old to record a departure"),
            Err(error) => println!("coordinator: not recorded ({error})"),
        }
    }
    // Said plainly when nobody heard: a machine found only by the LAN beacons
    // is not in any list this command can read, and "you can switch off now"
    // after telling no one would be the reassurance this notice exists to end.
    if told.is_empty() {
        println!("no peer was told: their ledgers will stop counting this node after LIVE_FOR");
    } else {
        println!("{} peer(s) told; this node can be switched off", told.len());
    }
    println!("the daemon will not start here again until `itsanas rejoin`");
    if node.config.coordinator.is_some() {
        // Leaving is not withdrawing: the claim stays live, so this device
        // still takes one of the account's slots. Said here, because the
        // next person to hit the limit will not think of this machine.
        println!(
            "this device still holds one of the account's {} device slots; to free it, run \
             `itsanas device forget {me}` from another device of the account",
            itsanas_coord::claim::MAX_DEVICES_PER_ACCOUNT
        );
    }
    Ok(())
}

/// Ask every owner to stop counting this machine and re-place what rests on it
/// alone, until each says nothing does or `minutes` pass (§8 0w (7)).
///
/// Never refuses to finish: Nicolas, 2026-10-07, nobody is kept from switching
/// off because the network is thin. What it says is counts, never who: the
/// owners are other members, and their names are not this machine's to print.
pub(crate) fn drain(node: &Node, targets: &[(Option<DeviceId>, String)], minutes: u64) {
    use std::{collections::BTreeMap, time::Instant};
    let minutes = minutes.min(itsanas_store::holders::MAX_DRAIN / 60);
    if minutes == 0 || targets.is_empty() {
        return;
    }
    let deadline = Instant::now() + std::time::Duration::from_secs(minutes * 60);
    println!("draining: asking the owners to re-place what rests on this machine alone");

    // Keyed by device where the coordinator named one, so a machine listed at
    // two addresses is one answer, and a dead address of a machine that
    // answered elsewhere is not "could not be asked". A typed-in peer has
    // only its address.
    let key = |expect: &Option<DeviceId>, address: &str| {
        expect.map_or_else(|| address.to_owned(), |device| device.to_string())
    };
    let mut last: BTreeMap<String, Asked> = BTreeMap::new();
    // Only machines that cannot be reached are left after this many passes:
    // waiting the whole drain for them would only keep a person at a screen.
    let mut passes_with_only_unreachable = 0;
    loop {
        for (expect, address) in targets {
            let slot = key(expect, address);
            if matches!(last.get(&slot), Some(Asked::Exposed(0) | Asked::TooOld)) {
                continue;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let asked = match connect_peer(address, &node.device, node.store.owner(), *expect)
                .and_then(|mut client| client.draining(left.as_secs() + 60))
            {
                Ok(Some(chunks)) => Asked::Exposed(chunks),
                Ok(None) => Asked::TooOld,
                Err(_) => Asked::Unreachable,
            };
            // One answer from any address of a machine stands over a failure
            // at another.
            if asked != Asked::Unreachable || !last.contains_key(&slot) {
                last.insert(slot, asked);
            }
        }
        let waiting = last
            .values()
            .filter(|asked| matches!(asked, Asked::Exposed(n) if *n > 0))
            .count();
        let unreachable = last
            .values()
            .filter(|asked| **asked == Asked::Unreachable)
            .count();
        if waiting == 0 && unreachable > 0 {
            passes_with_only_unreachable += 1;
        }
        if waiting + unreachable == 0
            || passes_with_only_unreachable >= 3
            || Instant::now() >= deadline
        {
            break;
        }
        println!("draining: {waiting} owner(s) still re-placing, {unreachable} unreachable");
        std::thread::sleep(
            std::time::Duration::from_secs(60)
                .min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    println!("{}", drain_summary(last.values().copied()));
}

/// What one owner last said to [`drain`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Asked {
    /// How many of its chunks rest on this machine alone.
    Exposed(u64),
    /// Older than the drain; its records go at `Leaving`, as before.
    TooOld,
    Unreachable,
}

/// The drain's last word, in counts only.
///
/// An owner that could not be asked is never folded into "done": that would
/// be the one number in this sentence that lies.
pub(crate) fn drain_summary(answers: impl Iterator<Item = Asked>) -> String {
    let (mut short, mut chunks, mut unknown) = (0usize, 0u64, 0usize);
    for asked in answers {
        match asked {
            Asked::Exposed(0) => {}
            Asked::Exposed(n) => {
                short += 1;
                chunks += n;
            }
            Asked::TooOld | Asked::Unreachable => unknown += 1,
        }
    }
    if short == 0 && unknown == 0 {
        return "drained: every owner has another machine holding what this one held for them"
            .to_owned();
    }
    let mut out = String::from("drain ended: ");
    if short > 0 {
        let _ = write!(
            out,
            "{short} owner(s) left one replica fewer, for {chunks} chunk(s) no other \
             machine held; they re-place them from their own copy, and any they no longer \
             have themselves may be lost"
        );
    }
    if unknown > 0 {
        if short > 0 {
            out.push_str("; ");
        }
        let _ = write!(
            out,
            "{unknown} owner(s) could not be asked and may be one replica fewer"
        );
    }
    out
}

/// The file in a node home that says `itsanas leave` ran, and when.
///
/// Why a file and not a stop hook: the service stops at every reboot and
/// every upgrade, so running `leave` from `ExecStop=` would announce a
/// departure each time and have the peers re-replicate the whole machine
/// after a restart. The contradiction to prevent is the reverse -- a machine
/// that left, then came back because systemd or the logon task started it --
/// so the departure is recorded where the daemon looks before starting.
pub(crate) const DEPARTED: &str = "departed";

pub(crate) fn record_departure(home: &Path, now: u64) -> Result<()> {
    let path = home.join(DEPARTED);
    std::fs::write(&path, format!("{now}\n")).map_err(|error| CliError::Io {
        path,
        source: error,
    })
}

/// Why this node must not talk to its peers, if `leave` ran here.
pub(crate) fn departure_refusal(home: &Path) -> Option<String> {
    let text = std::fs::read_to_string(home.join(DEPARTED)).ok()?;
    let when = text.trim().parse::<u64>().map_or_else(
        |_| "at an unreadable time".to_owned(),
        |at| {
            format!(
                "{} ago",
                describe_age(itsanas_discover::now_unix().saturating_sub(at))
            )
        },
    );
    Some(format!(
        "this node left the network {when} (`itsanas leave`), and its peers \
         have stopped counting it; starting it again would contradict that. \
         Run `itsanas rejoin` if it is coming back."
    ))
}

pub(crate) fn refuse_if_departed(home: &Path) -> Result<()> {
    departure_refusal(home).map_or(Ok(()), |why| Err(CliError::Usage(why)))
}

/// Undo `leave` locally. The peers count this node again once they confirm
/// it holds their chunks, as for any machine that was away.
pub(crate) fn rejoin(home: &Path) -> Result<()> {
    if !Node::exists(home) {
        return Err(CliError::NoNode(home.to_path_buf()));
    }
    let path = home.join(DEPARTED);
    match std::fs::remove_file(&path) {
        Ok(()) => {
            println!("this node may start again: `itsanas daemon`, or start its service");
            println!("with a coordinator, `itsanas register` publishes it again");
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            println!("this node had not left; nothing to do");
            Ok(())
        }
        Err(error) => Err(CliError::Io {
            path,
            source: error,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::Asked;
    use super::departure_refusal;
    use super::drain_summary;
    use super::record_departure;
    use super::refuse_if_departed;
    use super::rejoin;
    use crate::registration::register;
    use crate::test_fixtures::fake_node;

    #[test]
    fn red_team_a_drain_never_counts_an_owner_it_could_not_ask_as_done() {
        // The drain's last sentence is what a person reads before switching
        // off for good. An owner that was unreachable, or too old to be asked,
        // folded into "drained" would be the one number in it that lies.
        let all_done = drain_summary([Asked::Exposed(0), Asked::Exposed(0)].into_iter());
        assert!(all_done.starts_with("drained:"), "{all_done}");

        let out = drain_summary(
            [
                Asked::Exposed(0),
                Asked::Unreachable,
                Asked::TooOld,
                Asked::Exposed(4),
            ]
            .into_iter(),
        );
        assert!(
            !out.starts_with("drained:"),
            "owners that were never asked were reported as done: {out}"
        );
        assert!(
            out.contains("1 owner(s) left one replica fewer, for 4 chunk(s)"),
            "{out}"
        );
        assert!(out.contains("2 owner(s) could not be asked"), "{out}");
    }

    /// Deleting an account is for good: a mistyped name, or no coordinator to
    /// delete it at, must cost nothing -- not even this machine's departure.
    #[test]
    fn red_team_delete_account_does_nothing_without_the_typed_name() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        fake_node(&home, "alice", None);
        for typed in ["alic", "Alice", ""] {
            let refused = super::delete_account(&home, typed, 0)
                .expect_err("a mistyped name went ahead")
                .to_string();
            assert!(
                refused.contains("--confirm"),
                "{typed:?} got past the name check (refused later, for another reason: {refused})"
            );
        }
        assert!(
            super::delete_account(&home, "alice", 0).is_err(),
            "an account with no coordinator was 'deleted' with nothing to delete it at"
        );
        assert!(
            refuse_if_departed(&home).is_ok(),
            "a refused deletion still made this machine leave"
        );
    }

    /// A machine that ran `leave` must not come back by itself: systemd's
    /// `Restart=` or the logon task would start the daemon, and its peers,
    /// told it was gone, would meet it again. After the departure the daemon's
    /// check, `serve` and `sync` all refuse and name `rejoin`; after `rejoin`
    /// they do not.
    #[test]
    fn red_team_a_departed_node_stays_departed_until_it_rejoins() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        fake_node(&home, "alice", None);
        assert!(refuse_if_departed(&home).is_ok(), "a node that never left");

        record_departure(&home, itsanas_discover::now_unix()).expect("record");
        let why = departure_refusal(&home).expect("the daemon must see the departure");
        assert!(why.contains("itsanas rejoin"), "{why}");
        assert!(
            refuse_if_departed(&home).is_err(),
            "serve and sync would still talk to the peers"
        );

        rejoin(&home).expect("rejoin");
        assert!(departure_refusal(&home).is_none(), "rejoin did not undo it");
    }

    /// `register` publishes this device's address as well as its claim. After
    /// `leave` it must refuse before touching the network, or a departed
    /// node hands the peers that were told it had gone a fresh address to
    /// dial -- the return `leave` exists to prevent.
    #[test]
    fn red_team_a_departed_node_cannot_register_again() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        fake_node(&home, "alice", None);
        record_departure(&home, itsanas_discover::now_unix()).expect("record");

        let refused = register(&home, false, false, None)
            .expect_err("a departed node registered and announced itself");
        assert!(
            refused.to_string().contains("itsanas rejoin"),
            "register reached the node (or the network) instead of naming rejoin: {refused}"
        );
    }
}
