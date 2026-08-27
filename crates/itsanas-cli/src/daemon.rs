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
//! # What it does not do yet
//!
//! It does not watch the filesystem — files enter the store through
//! `itsanas put`, not by appearing in a folder. It does not execute repair
//! plans, because building a census means asking every peer what it holds and
//! that needs the coordinator's node set to know who "every peer" is. Both are
//! recorded in `docs/ROADMAP.md`.

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

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

/// Sync with every configured peer, forever.
fn sync_loop(node: &Node, interval: Duration, shutdown: &AtomicBool) {
    // Sync immediately on start rather than waiting a full interval. A daemon
    // restarted after a config change should act on it now, and someone
    // watching the first run should see something happen.
    let mut next = Instant::now();

    while !shutdown.load(Ordering::Relaxed) {
        if Instant::now() < next {
            std::thread::sleep(SHUTDOWN_POLL);
            continue;
        }

        if node.config.peers.is_empty() {
            // Nothing to do, but keep the loop alive: the listener is still
            // serving, and a peer may be added and the daemon restarted.
            next = Instant::now() + interval;
            continue;
        }

        for peer in &node.config.peers {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }
            sync_once(node, peer);
        }

        next = Instant::now() + interval;
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
