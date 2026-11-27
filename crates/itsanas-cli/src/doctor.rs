//! `doctor`, `convert-packs` and `gc`: the network check, and the store's upkeep.

use std::path::{Path, PathBuf};

use crate::opening::{daemon_holds, hand_to_daemon, open, passphrase};
use crate::{
    config::format_size,
    error::{CliError, Result},
    node::Node,
};
use crate::{coordinator, jobs};

/// What the coordinator said of this account's other machines.
#[derive(Debug)]
pub(crate) struct Peers {
    /// This device is among those the coordinator lists.
    pub(crate) me_listed: bool,
    /// Other machines of the account that published an address.
    pub(crate) elsewhere: usize,
    /// Of those, the ones at an address dialable from another network.
    dialable: usize,
}

/// What the coordinator saw when asked to dial this machine back.
#[derive(Debug)]
pub(crate) enum Inbound {
    Answered(coordinator::Reachability),
    /// The coordinator predates the dial-back.
    TooOld,
    Failed(String),
}

/// The network half of `doctor`, as data: `doctor` prints it, `setup`
/// judges it. `inbound` is `None` when the coordinator was not reached.
#[derive(Debug)]
pub(crate) struct NetworkCheck {
    pub(crate) coordinator: Option<String>,
    pub(crate) outbound: std::result::Result<Peers, String>,
    pub(crate) inbound: Option<Inbound>,
}

/// `dial_back` asks the coordinator to connect to this machine, which costs it a
/// connection and proves something only while this machine listens: a phone,
/// which never does, asks `false`.
pub(crate) fn network_check(
    identity: &itsanas_node::node::Identity,
    dial_back: bool,
) -> NetworkCheck {
    let coordinator = identity.config.coordinator.clone();
    if coordinator.is_none() {
        return NetworkCheck {
            coordinator,
            outbound: Err("no coordinator configured".to_owned()),
            inbound: None,
        };
    }
    let user = identity.user.user_id();
    let outbound = coordinator::devices_as(&identity.config, &identity.device, user)
        .map(|found| {
            let me = identity.device.device_id();
            Peers {
                me_listed: found.iter().any(|(device, _)| *device == me),
                elsewhere: found.iter().filter(|(device, _)| *device != me).count(),
                dialable: found
                    .iter()
                    .filter(|(device, candidate)| {
                        *device != me && !coordinator::is_private_address(candidate)
                    })
                    .count(),
            }
        })
        .map_err(|error| error.to_string());
    let inbound = (outbound.is_ok() && dial_back).then(|| {
        match coordinator::check_me_as(&identity.config, &identity.device, user) {
            Ok(Some(answer)) => Inbound::Answered(answer),
            Ok(None) => Inbound::TooOld,
            Err(error) => Inbound::Failed(error.to_string()),
        }
    });
    NetworkCheck {
        coordinator,
        outbound,
        inbound,
    }
}

/// Say, in order, everything this machine can find out about its connectivity.
///
/// The question a member actually has when nothing is syncing is "whose fault
/// is this", and until this existed the answer was a log line saying the
/// coordinator was unreachable, or -- worse -- silence, because a node with a
/// broken forward looks exactly like a node whose peers are all switched off.
///
/// Three questions, in the order that makes the next one worth asking:
///
/// 1. **Can I get out?** Dial the coordinator. The failure carries the reason,
///    and the reasons are different problems: a name that does not resolve is
///    DNS, a refused connection is a port, a timeout is usually a firewall.
/// 2. **Can anybody get in?** Only somebody outside can answer, so ask the
///    coordinator to try. Costs it one connection, which is why this is a
///    command a person runs rather than something on a timer.
/// 3. **What would I dial?** The account's other machines, and whether their
///    addresses are ones this machine could use from where it is standing.
pub(crate) fn network_report(identity: &itsanas_node::node::Identity) {
    println!();
    println!("network");

    let check = network_check(identity, true);
    let Some(address) = check.coordinator.as_deref() else {
        println!("  no coordinator configured, so this machine can only meet peers on");
        println!("  its own network, or ones added by hand with `itsanas peer add`.");
        return;
    };

    match &check.outbound {
        Ok(peers) => {
            println!("  out    the coordinator at {address} answered");
            println!(
                "  peers  {} other machine(s) of this account have published an address",
                peers.elsewhere
            );
            if peers.elsewhere > 0 && peers.dialable == 0 {
                println!("         none of them is an address this machine could dial from");
                println!("         another network. On one LAN that is right and costs nothing;");
                println!("         from anywhere else nothing of this account can be reached.");
            }
        }
        Err(error) => {
            println!("  out    the coordinator at {address} could NOT be reached");
            println!("         {error}");
            println!("         A name that does not resolve is DNS; a refused connection is a");
            println!("         port that is closed or forwarded nowhere; a timeout is usually a");
            println!("         firewall. Syncing continues with peers already known.");
            return;
        }
    }

    match &check.inbound {
        Some(Inbound::Answered(coordinator::Reachability::Reachable(detail))) => {
            println!("  in     {detail}");
        }
        Some(Inbound::Answered(coordinator::Reachability::Unreachable(detail))) => {
            println!("  in     NOTHING can reach this machine: {detail}");
            if let Some(announce) = identity.config.announce.as_deref() {
                println!("         This machine announces {announce}, so something was meant");
                println!("         to reach it there: check the forward, and that it points at");
                println!("         this machine's listening port.");
            } else {
                println!("         This machine announces nothing, so this is expected: it");
                println!("         takes part by dialling out, and one reachable side per");
                println!("         pair is enough. Set `itsanas announce` only if a forward");
                println!("         or an IPv6 route really does reach this machine.");
            }
        }
        // Nothing was tried, so nothing is known. Printing this as a verdict is
        // what sends somebody to rewire a router that works.
        Some(Inbound::Answered(coordinator::Reachability::Unknown(why))) => {
            println!("  in     not checked this time: {why}");
        }
        Some(Inbound::TooOld) => {
            println!("  in     not checked: this coordinator is too old to try reaching back");
        }
        Some(Inbound::Failed(error)) => println!("  in     not checked: {error}"),
        None => {}
    }
}

pub(crate) fn doctor(home: &Path, deep: bool) -> Result<()> {
    // The network half first, and from the keys alone. Only one process may
    // hold the store, and the daemon holds it on every machine that is
    // working -- so a `doctor` that needed the store refused to run on exactly
    // the machines somebody asks about, and stopping the daemon to ask changed
    // the answer, because a node that is not running is not listening.
    let identity = itsanas_node::node::Identity::open(home, &passphrase(false)?)?;
    network_report(&identity);
    println!();

    if itsanas_store::Store::is_locked(Node::store_path(home)) {
        println!("the stored data was not checked: this node is running, and only");
        println!("one process at a time may hold its state. Stop the daemon and run");
        println!("`itsanas doctor` again to check the data itself.");
        return Ok(());
    }

    let node = open(home)?;
    let report = node.store.verify_integrity(deep)?;

    println!(
        "checked {} files{}",
        report.files_checked,
        if deep { " (deep)" } else { "" }
    );

    if report.is_healthy() && report.orphan_blobs.is_empty() {
        println!("the stored data checks out.");
        return Ok(());
    }

    if !report.missing_chunks.is_empty() {
        println!();
        println!(
            "{} chunks are referenced but missing from disk:",
            report.missing_chunks.len()
        );
        for (path, chunk) in report.missing_chunks.iter().take(20) {
            println!("  {path} needs {}", chunk.short());
        }
        println!("  These files cannot be read until the chunks are refetched from a peer.");
    }

    if !report.corrupt_files.is_empty() {
        println!();
        println!("{} files failed verification:", report.corrupt_files.len());
        for path in report.corrupt_files.iter().take(20) {
            println!("  {path}");
        }
    }

    if !report.chain_intact {
        println!();
        println!("the operation log has a gap. Peers may not have the full history.");
    }

    if !report.orphan_blobs.is_empty() {
        println!();
        println!(
            "{} chunks on disk are not accounted for. These are leaked, not \
             dangerous — usually a crash between writing a chunk and committing \
             its index entry. `itsanas gc` reclaims them.",
            report.orphan_blobs.len()
        );
    }

    // Orphans alone are not a failure, and saying they are is worse than
    // saying nothing. A machine that lost power mid-write leaves them every
    // time; exiting non-zero for that means a monitoring wrapper reports a
    // healthy node as broken until somebody runs garbage collection by hand —
    // and a check that cries wolf after every power cut stops being read.
    //
    // Found by the crash test, which killed the process mid-write and then
    // could not tell "the store is damaged" from "the store is exactly as
    // expected after a crash".
    if report.is_healthy() {
        println!();
        println!("nothing is damaged. Those chunks are waiting for `itsanas gc`.");
        return Ok(());
    }

    // A report is information, not a crash. Exit non-zero so a monitoring
    // system notices, but say everything first.
    Err(CliError::Usage(
        "integrity problems found (see above)".to_owned(),
    ))
}

pub(crate) fn convert_packs(home: &Path) -> Result<()> {
    if daemon_holds(home) {
        return hand_to_daemon(home, jobs::Kind::ConvertPacks, home);
    }
    println!("{}", convert_stores(home)?);
    Ok(())
}

/// This machine's store and every store it hosts.
pub(crate) fn store_roots(home: &Path) -> Result<Vec<PathBuf>> {
    let config = itsanas_node::config::Config::load(&itsanas_node::node::Node::config_path(home))?;
    let mut roots = vec![itsanas_node::node::Node::store_path(home)];
    let owners = itsanas_node::node::Node::vault_path(home, &config).join("owners");
    if let Ok(entries) = std::fs::read_dir(&owners) {
        roots.extend(entries.filter_map(|entry| entry.ok().map(|e| e.path())));
    }
    Ok(roots)
}

/// Whether the daemon converts `roots` to pack files on its own before it
/// opens the node: one is still one file per chunk (14.7 million files a
/// terabyte, all walked every round: ROADMAP M10), packs are not turned off
/// (`ITSANAS_PACKS=0`), and each such store's disk, as `free_of` measures
/// it, has room for the copy the conversion makes before it deletes, plus a
/// quarter and a gigabyte. A disk too full is left as it is, never filled.
pub(crate) fn conversion_due(
    roots: &[PathBuf],
    packs_off: bool,
    free_of: &dyn Fn(&Path) -> Option<u64>,
) -> bool {
    if packs_off {
        return false;
    }
    let mut any = false;
    for root in roots {
        let Ok(blobs) = itsanas_store::BlobStore::open(root) else {
            return false;
        };
        if blobs.packs().is_some() {
            continue;
        }
        let Ok(bytes) = blobs.total_bytes() else {
            return false;
        };
        let needed = bytes.saturating_add(bytes / 4).saturating_add(1 << 30);
        if free_of(root).is_none_or(|free| free < needed) {
            return false;
        }
        any = true;
    }
    any
}

/// Move this machine's store and every store it hosts into pack files; what
/// it says. The stores must be closed.
pub(crate) fn convert_stores(home: &Path) -> Result<String> {
    let store = itsanas_node::node::Node::store_path(home);
    if itsanas_store::Store::is_locked(&store) {
        return Err(CliError::Usage(
            "the daemon holds this node: stop it, then run convert-packs again".to_owned(),
        ));
    }
    let roots = store_roots(home)?;
    let mut moved = 0u64;
    for root in &roots {
        moved += itsanas_store::BlobStore::convert_to_packs(root)?;
    }
    Ok(format!(
        "{moved} chunks moved into pack files, across {} stores (this machine's and the ones it hosts)",
        roots.len()
    ))
}

pub(crate) fn gc(home: &Path, grace: u64) -> Result<()> {
    let node = open(home)?;
    let report = node
        .store
        .collect_garbage(std::time::Duration::from_secs(grace))?;

    println!(
        "reclaimed {} from {} chunks",
        format_size(report.bytes_reclaimed),
        report.blobs_removed
    );
    if report.retained_in_grace > 0 {
        println!(
            "{} chunks are unreferenced but still inside the {grace}s grace period",
            report.retained_in_grace
        );
    }

    Ok(())
}
