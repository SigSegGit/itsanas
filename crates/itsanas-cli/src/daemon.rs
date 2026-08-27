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
//! # What it does not do yet
//!
//! It does not execute repair plans. Building a census means asking every peer
//! what it holds, and knowing who "every peer" is needs the coordinator's node
//! set. Recorded in `docs/ROADMAP.md`.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use itsanas_folder::{Folder, Watcher, watch};
use itsanas_net::{Exposure, PeerClient, PeerServer, PeerService, Pledge, session};

use crate::{
    config::format_size,
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

/// Set by the interrupt handler.
///
/// A process-wide `static` rather than something threaded through, because the
/// signal handler is process-wide: it is installed once, outlives any particular
/// call, and cannot borrow a local.
static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Run until interrupted.
pub fn run(
    node: &Node,
    listen: Option<&str>,
    allow_public: bool,
    interval: Duration,
) -> Result<()> {
    let address = listen.unwrap_or(&node.config.listen);
    let exposure = if allow_public {
        Exposure::Anywhere
    } else {
        Exposure::LocalOnly
    };

    let server = PeerServer::bind(address, exposure)?;
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
        println!("  peers     none configured — this node will serve but never initiate");
    } else {
        println!("  peers     {}", node.config.peers.join(", "));
    }
    if allow_public {
        println!();
        println!("WARNING: this transport is not encrypted. Your data stays sealed, but");
        println!("anyone on the network path sees chunk identifiers, sizes and timing.");
    }
    println!();
    println!("Ctrl-C to stop.");
    println!();

    std::thread::scope(|scope| {
        scope.spawn(|| {
            if let Err(error) = server.serve_until(&service, shutdown) {
                // The sync loop can carry on without the listener; a node that
                // cannot accept connections can still push to its peers. Say so
                // loudly rather than exiting and taking sync down with it.
                eprintln!("itsanas: the listener stopped: {error}");
            }
        });

        sync_loop(node, interval, shutdown);
    });

    println!("stopped.");
    Ok(())
}

/// Reconcile the folder, sync with peers, and wait for whichever comes first.
fn sync_loop(node: &Node, interval: Duration, shutdown: &AtomicBool) {
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
            for peer in &node.config.peers {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                sync_once(node, peer);
            }
            next_sync = Instant::now() + interval;

            // Write out whatever just arrived, rather than making the user
            // wait for the next loop to see their peer's changes.
            if let Some((folder, _)) = &folder {
                reconcile_once(node, folder, false);
            }
        }

        wait_for_work(folder.as_ref(), next_sync, shutdown);
    }
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

/// One round against one peer. Never propagates an error.
///
/// A peer being unreachable is the normal state of this network, not a fault:
/// the whole design is built around machines that are usually off. Treating it
/// as an error would mean the daemon exits every time someone shuts a laptop.
fn sync_once(node: &Node, peer: &str) {
    let mut client = match PeerClient::connect(peer, node.store.device_id(), node.store.owner()) {
        Ok(client) => client,
        Err(error) => {
            println!("{peer}: unreachable ({error})");
            return;
        }
    };

    match session::round(&node.store, &node.vault, &mut client) {
        Ok(report) if report.changed_anything() => {
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
        // A quiet round is the common case. Saying so every five minutes would
        // fill a journal with nothing and train the operator to ignore it.
        Ok(_) => {}
        Err(error) => println!("{peer}: failed ({error})"),
    }
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
