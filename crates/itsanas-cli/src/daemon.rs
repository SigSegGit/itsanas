//! Running as a service: serve peers and sync on a timer, in one process.
//!
//! Two things need to happen continuously and neither can happen alone. A node
//! that only serves is a host nobody syncs with; a node that only syncs is a
//! peer nobody can reach. So the daemon does both, in one process, against one
//! open store.
//!
//! # Why one process matters more than it looks
//!
//! A node's index is held under an exclusive file lock, so `itsanas serve` and
//! `itsanas sync` cannot run at the same time against the same home — the
//! second refuses to start. Two separate cron entries would therefore fight,
//! and which one won would depend on timing. Doing both from one process is not
//! a convenience; it is the only arrangement that works.
//!
//! It also means the passphrase is entered once and the keys are unlocked once,
//! rather than paying a full Argon2id derivation on every scheduled sync.
//!
//! # The loop
//!
//! ```text
//! reconcile   folder → store, store → folder
//! sync        push and pull with each peer, on the interval
//! reconcile   write out whatever the sync just pulled
//! wait        until the folder changes, or the next sync is due
//! ```
//!
//! Waiting on the *watcher* rather than on a plain sleep is what makes a local
//! edit feel instant while a network round still happens on a timer. The
//! watcher is never trusted on its own: every platform's drops events under
//! load and none of them report changes made while the process was stopped, so
//! a full rescan runs on the interval regardless, and a slower **deep** rescan
//! periodically re-hashes everything to catch what size-and-mtime comparison
//! cannot see.
//!
//! # Finding peers
//!
//! Three ways, in order of how much they need. A third thread announces this
//! node on the local network and records what it hears, so machines in one
//! house find each other with nothing configured. Addresses in the
//! configuration are dialled first, because somebody typed them. And if a
//! coordinator is configured, each round publishes this node's address there
//! and asks where the account's other devices are — which is the only one of
//! the three that reaches a machine on a different network.
//!
//! Every candidate is dialled with its device id pinned, so an address that
//! answers as somebody else is refused. Discovery of any kind says who *might*
//! be there; the TLS layer decides who is.
//!
//! # What it does not do yet
//!
//! It does not execute repair plans. Building a census means asking every peer
//! what it holds, and knowing who "every peer" is needs the coordinator's node
//! set. Recorded in `docs/ROADMAP.md`.

use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use itsanas_crypto::DeviceId;
use itsanas_discover::Lan;
use itsanas_folder::{Folder, Watcher, watch};
use itsanas_net::{PeerClient, PeerServer, PeerService, Pledge, session};

use crate::{
    config::format_size,
    coordinator,
    discovery::{self, Neighbourhood},
    error::{CliError, Result},
    node::Node,
};

/// How often to sync when the operator did not say.
///
/// Five minutes is a compromise: short enough that a change made on the laptop
/// is on the Pi before you have walked to it, long enough that three machines
/// polling each other is not a constant background load on a Raspberry Pi.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(300);

/// How finely the sleep between rounds is chopped.
///
/// The daemon spends nearly all its life asleep, and a Ctrl-C that took five
/// minutes to be noticed would be indistinguishable from a hang.
const SHUTDOWN_POLL: Duration = Duration::from_millis(200);

/// How often every file is re-hashed rather than trusted by size and mtime.
///
/// The fast path misses a file rewritten within the same second at exactly the
/// same length. Rare, but silent, and the only thing that finds it is reading
/// the bytes. Hourly is cheap enough for a folder of any sane size and short
/// enough that such an edit is not lost for a working day.
const DEEP_SCAN_EVERY: Duration = Duration::from_secs(3600);

/// Longest a continuous stream of file events may postpone a reconcile.
///
/// Copying a large directory in produces events for as long as the copy takes.
/// Waiting for true quiet is correct in principle and indistinguishable from a
/// hang in practice, so the wait is capped and the reconcile runs on what has
/// landed so far. Anything still arriving is caught by the next pass.
const SETTLE_LIMIT: Duration = Duration::from_secs(10);

/// How long a continuing coordinator outage stays quiet between complaints.
///
/// A coordinator that is down is down for hours, not for one round. Saying so
/// every round produces roughly six hundred identical lines a day, and a
/// journal nobody reads is the same as no journal on the morning something else
/// breaks. Found by running the daemon against a coordinator that was not
/// there, which is exactly the state MVP acceptance test I describes as normal.
const OUTAGE_QUIET: Duration = Duration::from_secs(30 * 60);

/// What has already been said about the coordinator, so it is not said again.
#[derive(Debug)]
struct Outage {
    /// Rounds that have failed since the last time anything was printed.
    silent_rounds: usize,
    /// When the next complaint is allowed.
    next_complaint: Instant,
    /// Whether the last round reached it.
    was_reachable: bool,
}

impl Outage {
    fn new() -> Self {
        Self {
            silent_rounds: 0,
            next_complaint: Instant::now(),
            was_reachable: true,
        }
    }

    /// Report a failed round, at most once per [`OUTAGE_QUIET`].
    fn failed(&mut self, why: &str) {
        self.silent_rounds += 1;
        if Instant::now() < self.next_complaint {
            return;
        }
        if self.silent_rounds == 1 {
            println!("coordinator: unreachable ({why})");
            println!("  Peers already known keep syncing. New machines cannot be found.");
        } else {
            println!(
                "coordinator: still unreachable after {} rounds ({why})",
                self.silent_rounds
            );
        }
        self.next_complaint = Instant::now() + OUTAGE_QUIET;
        self.was_reachable = false;
    }

    /// Report a round that reached it, but only if the last one did not.
    fn succeeded(&mut self) {
        if !self.was_reachable {
            println!("coordinator: reachable again");
        }
        self.silent_rounds = 0;
        self.next_complaint = Instant::now();
        self.was_reachable = true;
    }
}

/// Set by the interrupt handler.
///
/// A process-wide `static` rather than something threaded through, because the
/// signal handler is process-wide: it is installed once, outlives any particular
/// call, and cannot borrow a local.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Run until interrupted.
pub fn run(node: &Node, listen: Option<&str>, interval: Duration, discover: bool) -> Result<()> {
    let address = listen.unwrap_or(&node.config.listen);
    let server = PeerServer::bind(address)?;
    let bound = server.local_addr()?;

    let service = PeerService::new(
        &node.store,
        &node.vault,
        Pledge {
            bytes: node.config.pledge_bytes,
        },
    );

    install_signal_handler()?;
    let shutdown = &SHUTDOWN;

    let neighbourhood = Neighbourhood::new();

    // Discovery is an optimisation, so a failure to bind is a warning and not
    // an exit. The commonest cause is a second node on the same machine, which
    // is a test rig rather than a mistake, and it must not stop the daemon that
    // can still sync with its configured peers.
    let lan = if discover {
        match Lan::bind(itsanas_discover::DEFAULT_PORT) {
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
    };

    println!("itsanas daemon");
    println!("  serving   {bound}");
    println!("  user id   {}", node.store.owner());
    println!("  device    {}", node.store.device_id());
    println!("  pledged   {}", format_size(node.config.pledge_bytes));
    println!("  interval  {}s", interval.as_secs());
    match &node.config.folder {
        Some(folder) => println!("  folder    {}", folder.display()),
        None => println!("  folder    none configured (`itsanas folder <path>`)"),
    }
    if node.config.peers.is_empty() {
        println!("  peers     none configured");
    } else {
        println!("  peers     {}", node.config.peers.join(", "));
    }
    match &node.config.coordinator {
        Some(address) => println!("  coordinator {address}"),
        None => println!("  coordinator none (`itsanas coordinator <host:port>`)"),
    }
    match &lan {
        Some(_) => println!(
            "  discovery on, udp {} — machines on this network find each other",
            itsanas_discover::DEFAULT_PORT
        ),
        None if discover => println!("  discovery unavailable — peers must be added by hand"),
        None => println!("  discovery off (--no-discovery)"),
    }
    println!();
    println!("Ctrl-C to stop.");
    println!();

    std::thread::scope(|scope| {
        scope.spawn(|| {
            if let Err(error) = server.serve_until(&service, &node.device, shutdown) {
                // The sync loop can carry on without the listener; a node that
                // cannot accept connections can still push to its peers. Say so
                // loudly rather than exiting and taking sync down with it.
                eprintln!("itsanas: the listener stopped: {error}");
            }
        });

        if let Some(lan) = &lan {
            scope.spawn(|| {
                discovery::run(
                    lan,
                    &node.device,
                    node.store.owner(),
                    bound.port(),
                    &neighbourhood,
                    shutdown,
                );
            });
        }

        sync_loop(node, interval, shutdown, &neighbourhood, bound);
    });

    println!("stopped.");
    Ok(())
}

/// Reconcile the folder, sync with peers, and wait for whichever comes first.
fn sync_loop(
    node: &Node,
    interval: Duration,
    shutdown: &AtomicBool,
    neighbourhood: &Neighbourhood,
    bound: std::net::SocketAddr,
) {
    let folder = match open_folder(node) {
        Ok(folder) => folder,
        Err(error) => {
            eprintln!("itsanas: {error}");
            eprintln!("itsanas: continuing without a synced folder");
            None
        }
    };

    // Sync immediately on start rather than waiting a full interval. A daemon
    // restarted after a config change should act on it now, and someone
    // watching the first run should see something happen.
    let mut next_sync = Instant::now();
    let mut next_deep = Instant::now();
    let mut warned_alone = false;
    let mut outage = Outage::new();

    while !shutdown.load(Ordering::Relaxed) {
        let deep = Instant::now() >= next_deep;
        if deep {
            next_deep = Instant::now() + DEEP_SCAN_EVERY;
        }

        if let Some((folder, _)) = &folder {
            reconcile_once(node, folder, deep);
        }

        // Anything a peer pushed into this node's vault while it was serving.
        // Without this a node that never dials anybody — because it has no
        // peers configured, or because its peers are behind NAT and can only
        // push — would hold its own data and never look at it.
        match session::drain_vault(&node.store, &node.vault) {
            Ok(report) if report.changed_anything() => {
                println!(
                    "pushed to us: {} files, {} conflicts",
                    report.adopted, report.conflicted
                );
                if let Some((folder, _)) = &folder {
                    reconcile_once(node, folder, false);
                }
            }
            Ok(_) => {}
            Err(error) => eprintln!("itsanas: could not apply pushed data: {error}"),
        }

        if Instant::now() >= next_sync {
            let reached = one_round(node, shutdown, neighbourhood, bound, &mut outage);
            next_sync = Instant::now() + interval;

            // Say it once, rather than leaving someone watching a silent
            // terminal wondering whether anything is happening. Silence is the
            // correct output for a working daemon and the worst possible
            // output for one that has nobody to talk to.
            if reached.is_empty() && !warned_alone {
                warned_alone = true;
                if node.config.peers.is_empty() && neighbourhood.is_empty() {
                    println!("no other machines found yet.");
                    println!(
                        "  on this network: start the daemon on another machine and it is found"
                    );
                    println!("  elsewhere:       itsanas peer add <host:port>");
                } else {
                    println!(
                        "{} machine(s) known, none reachable this round.",
                        node.config.peers.len() + neighbourhood.len()
                    );
                }
            } else if !reached.is_empty() {
                warned_alone = false;
            }

            // Write out whatever just arrived, rather than making the user
            // wait for the next loop to see their peer's changes.
            if let Some((folder, _)) = &folder {
                reconcile_once(node, folder, false);
            }
        }

        wait_for_work(folder.as_ref(), next_sync, shutdown);
    }
}

/// One pass over every way this node knows of reaching a peer.
///
/// In order: addresses somebody typed into the configuration; the account's
/// other devices as the coordinator reports them, if one is configured; and
/// whatever announced itself on the local network. Each is dialled with its
/// device id pinned wherever one is known, so an address that answers as
/// somebody else is refused rather than trusted.
fn one_round(
    node: &Node,
    shutdown: &AtomicBool,
    neighbourhood: &Neighbourhood,
    bound: std::net::SocketAddr,
    outage: &mut Outage,
) -> BTreeSet<DeviceId> {
    // Configured peers first: somebody typed those in, so they are
    // wanted even if they are also on the local network — and being in
    // the configuration is itself the evidence that they are real, so
    // they are confirmed on contact without having to earn it.
    let mut reached: BTreeSet<DeviceId> = BTreeSet::new();
    for peer in &node.config.peers {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        if let Some(outcome) = sync_once(node, peer, None, true) {
            neighbourhood.confirm(outcome.device);
            reached.insert(outcome.device);
        }
    }

    // Then whatever announced itself here. Each is pinned to the device
    // that announced it, so an address answering as somebody else is
    // refused rather than trusted: discovery says who might be there,
    // never who is.
    // The coordinator, if there is one. Failures here are ordinary:
    // it may be down, and a node whose peers are already known keeps
    // working without it. That is the point of it carrying nothing
    // vital.
    let mut from_coordinator: Vec<(DeviceId, String)> = Vec::new();
    if node.config.coordinator.is_some() {
        let now = itsanas_discover::now_unix();
        let listen = bound.to_string();
        // Announcing and listing are one outage, not two. Reporting them
        // separately doubled the noise for a single cause.
        let published = coordinator::announce(node, &listen, now);
        let discovered = coordinator::peers(node, node.store.owner());

        match (published, discovered) {
            (Ok(()), Ok(found)) => {
                outage.succeeded();
                from_coordinator = found;
            }
            (Err(error), _) | (_, Err(error)) => outage.failed(&error.to_string()),
        }
    }

    for (device, address) in &from_coordinator {
        if shutdown.load(Ordering::Relaxed) || reached.contains(device) {
            continue;
        }
        // Pinned: the coordinator supplies addresses and is not trusted
        // to say who lives at one.
        if let Some(outcome) = sync_once(node, address, Some(*device), true) {
            reached.insert(outcome.device);
            if outcome.earned_trust {
                neighbourhood.confirm(outcome.device);
            }
        }
    }

    let mut strangers_dialled = 0usize;
    for candidate in neighbourhood.dial_order(node.store.owner()) {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        if reached.contains(&candidate.device) {
            continue;
        }

        // Dialling costs a handshake and a round trip, and minting the
        // identity that provoked it cost an attacker nothing. Confirmed
        // peers are always dialled; unconfirmed ones are rationed, so a
        // flood cannot consume the interval that real syncing needs.
        let known = neighbourhood.is_confirmed(&candidate.device);
        if !known {
            if strangers_dialled >= discovery::NEW_PEERS_PER_ROUND {
                continue;
            }
            strangers_dialled += 1;
        }

        // A stranger that does not answer is not news — this network is
        // built out of machines that are usually off. One of your own
        // machines failing to answer is worth saying.
        let outcome = sync_once(
            node,
            &candidate.address.to_string(),
            Some(candidate.device),
            candidate.mine,
        );

        if let Some(outcome) = outcome {
            reached.insert(outcome.device);
            // Authenticating proves possession of a keypair that cost
            // nothing to generate. Only doing something a real host
            // does — storing our data, or serving us our own work —
            // earns a place a stranger cannot take.
            if outcome.earned_trust {
                neighbourhood.confirm(outcome.device);
            }
        }
    }

    reached
}

/// Block until the folder changes, the next sync is due, or shutdown.
fn wait_for_work(
    folder: Option<&(Folder, Option<Watcher>)>,
    next_sync: Instant,
    shutdown: &AtomicBool,
) {
    let budget = next_sync.saturating_duration_since(Instant::now());

    if let Some((_, Some(watcher))) = folder {
        // Chopped into slices so a Ctrl-C during a long quiet period is still
        // noticed promptly.
        let slice = budget.min(Duration::from_secs(2));
        if watcher.wait_for_quiet(slice, watch::DEFAULT_DEBOUNCE, SETTLE_LIMIT) {
            return;
        }
        if shutdown.load(Ordering::Relaxed) {
            return;
        }
        return;
    }

    // No watcher: plain sleep, still chopped for responsiveness.
    let deadline = Instant::now() + budget.min(Duration::from_secs(2));
    while Instant::now() < deadline && !shutdown.load(Ordering::Relaxed) {
        std::thread::sleep(SHUTDOWN_POLL);
    }
}

/// Open the configured folder and start watching it, if one is configured.
fn open_folder(node: &Node) -> Result<Option<(Folder, Option<Watcher>)>> {
    let Some(path) = &node.config.folder else {
        return Ok(None);
    };

    let folder = Folder::open(path)?;

    // A watcher is a latency optimisation. If the platform will not give us
    // one — too many watches, an unusual filesystem — the periodic rescan
    // still does the job, so this is a warning rather than a failure.
    let watcher = match Watcher::new(folder.root()) {
        Ok(watcher) => Some(watcher),
        Err(error) => {
            eprintln!(
                "itsanas: could not watch {}: {error}. Falling back to periodic \
                 scanning only, so local edits will take up to one interval to \
                 be noticed.",
                folder.root().display()
            );
            None
        }
    };

    Ok(Some((folder, watcher)))
}

/// One folder pass. Never propagates an error.
fn reconcile_once(node: &Node, folder: &Folder, deep: bool) {
    match folder.reconcile(&node.store, deep) {
        Ok(report) => {
            if report.changed_anything() {
                println!("folder: {}", report.summary());
                for (original, sibling) in &report.kept_both {
                    println!("  conflict: {original} — your version kept as {sibling}");
                }
            }
            for (path, why) in &report.failed {
                eprintln!("itsanas: {path}: {why}");
            }
        }
        Err(error) => eprintln!("itsanas: folder scan failed: {error}"),
    }
}

/// What one round against one peer established.
#[derive(Clone, Copy, Debug)]
struct Outcome {
    /// Which device actually answered.
    device: DeviceId,
    /// Whether it did something only a real host does.
    ///
    /// Deliberately separate from "it answered". A device key is a free
    /// keypair, so authenticating identifies a peer and vouches for nothing.
    earned_trust: bool,
}

/// One round against one peer. Never propagates an error.
///
/// A peer being unreachable is the normal state of this network, not a fault:
/// the whole design is built around machines that are usually off. Treating it
/// as an error would mean the daemon exits every time someone shuts a laptop.
///
/// `expect` pins which device must answer. The device is reported even when the
/// round then failed — reaching a machine and falling out with it says nothing
/// about whether the machine exists — but `earned_trust` is only set when the
/// peer stored something of ours or served us our own work.
fn sync_once(
    node: &Node,
    peer: &str,
    expect: Option<DeviceId>,
    announce_failure: bool,
) -> Option<Outcome> {
    let mut client = match PeerClient::connect(peer, &node.device, node.store.owner(), expect) {
        Ok(client) => client,
        Err(error) => {
            if announce_failure {
                println!("{peer}: unreachable ({error})");
            }
            return None;
        }
    };
    let answered = client.peer_device();

    // Make the peer prove it still holds what the ledger says it took, a
    // handful of chunks per round. Without this the ledger is a list of
    // promises with nothing checking any of them, and a node believes its data
    // is on three machines while two of them threw it away.
    //
    // A failure is news: it withdraws that record, so the chunk shows as
    // under-replicated, and the operator should know a peer is not holding what
    // it said. Nothing is deleted and nobody is blocked.
    match session::audit(&node.store, &mut client, session::CHALLENGES_PER_ROUND) {
        Ok(report) if report.found_a_liar() => {
            println!(
                "{peer}: FAILED {} of {} storage challenges — it is not holding \
                 what it said. Those chunks now count as unreplicated.",
                report.failed, report.asked
            );
        }
        Ok(_) => {}
        // An audit that could not run is not an accusation. The peer may have
        // hung up, or this device may no longer hold a copy to check against.
        Err(error) => println!("{peer}: could not audit ({error})"),
    }

    let earned_trust = match session::round(&node.store, &node.vault, &mut client) {
        Ok(report) => {
            if report.changed_anything() {
                println!(
                    "{peer}: sent {} ({} chunks, {} segments), received {} files, {} conflicts{}",
                    format_size(report.push.bytes_sent),
                    report.push.chunks_accepted,
                    report.push.segments_accepted,
                    report.pull.adopted,
                    report.pull.conflicted,
                    if report.pull.deferred > 0 {
                        format!(", {} deferred", report.pull.deferred)
                    } else {
                        String::new()
                    }
                );
            }
            // A quiet round is the common case. Saying so every five minutes
            // would fill a journal with nothing and train the operator to
            // ignore it.
            report.peer_earned_trust()
        }
        Err(error) => {
            println!("{peer}: failed ({error})");
            false
        }
    };

    Some(Outcome {
        device: answered,
        earned_trust,
    })
}

fn install_signal_handler() -> Result<()> {
    ctrlc::set_handler(|| {
        SHUTDOWN.store(true, Ordering::Relaxed);
        println!();
        println!("stopping — finishing the current round…");
    })
    .map_err(|error| CliError::Usage(format!("could not install a signal handler: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_interval_is_neither_a_busy_loop_nor_an_hour() {
        // Too short and three machines polling each other is a constant load on
        // a Raspberry Pi; too long and the thing feels broken.
        assert!(DEFAULT_INTERVAL >= Duration::from_secs(60));
        assert!(DEFAULT_INTERVAL <= Duration::from_secs(900));
    }

    #[test]
    fn shutdown_is_noticed_quickly_enough_to_feel_immediate() {
        // A Ctrl-C that took a whole interval to be noticed would be
        // indistinguishable from a hang.
        assert!(SHUTDOWN_POLL <= Duration::from_millis(500));
    }
}
