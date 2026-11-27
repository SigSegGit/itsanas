//! Stopping, pausing and restarting the daemon: the process-wide flags, the
//! signal handler that raises them, and the checks a round makes before it
//! goes on (a vault that came or went, a pause asked for).

use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use super::pledge::vault_present;
use crate::{
    control,
    error::{CliError, Result},
    node::Node,
};

/// Raises the shutdown flag however the scope is left, panic included.
///
/// See the comment at the `thread::scope` in [`run`]: without it a panic in the
/// sync loop leaves the listener thread waiting on a flag nobody set, and the
/// process hangs instead of dying.
pub(super) struct StopEverythingOnDrop<'a>(pub(super) &'a AtomicBool);

impl Drop for StopEverythingOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Set by the interrupt handler.
///
/// A process-wide `static` rather than something threaded through, because the
/// signal handler is process-wide: it is installed once, outlives any particular
/// call, and cannot borrow a local.
pub(super) static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Set with [`SHUTDOWN`] when a run stops because the vault's disk went, not
/// because somebody asked: `itsanas daemon` then waits for the disk and runs
/// again, instead of exiting.
pub(super) static PAUSED_FOR_VAULT: AtomicBool = AtomicBool::new(false);

/// Whether the run that just returned stopped for the vault's disk; clears
/// the stop so the next run starts. A Ctrl-C during the pause still wins.
pub(crate) fn resume_after_vault_pause() -> bool {
    if PAUSED_FOR_VAULT.swap(false, Ordering::SeqCst) {
        SHUTDOWN.store(false, Ordering::SeqCst);
        return true;
    }
    false
}

/// Set by the daily update look when it installed a new program: the daemon
/// then exits with a failure code so its service starts the new one.
/// Set when this run stopped because `itsanas update` asked for a restart
/// (`lifecycle.rs`): `main` then exits with the code that restarts it.
pub(super) static RESTART_ASKED: AtomicBool = AtomicBool::new(false);

/// Whether the run that just ended was asked to restart.
pub(crate) fn restart_asked() -> bool {
    RESTART_ASKED.load(Ordering::SeqCst)
}

/// Sleep `length`, a second at a time; `false` once stopping.
pub(super) fn pause(length: Duration, shutdown: &AtomicBool) -> bool {
    let until = Instant::now() + length;
    while Instant::now() < until {
        if shutdown.load(Ordering::Relaxed) {
            return false;
        }
        std::thread::sleep(Duration::from_secs(1).min(until - Instant::now()));
    }
    !shutdown.load(Ordering::Relaxed)
}

/// Looked at every couple of seconds, not once a round: a disk plugged back
/// in should be hosting again in seconds, not in an hour. And the other way:
/// unplugged, this run stops at once and the next one serves this account
/// alone, rather than an hour of rounds onto the bare mount point.
pub(super) fn stop_if_the_vault_came_or_went(node: &Node, shutdown: &AtomicBool) {
    let present = vault_present(&node.vault_dir());
    if node.away == present {
        if present {
            eprintln!(
                "itsanas: the vault's disk is back at {}; hosting again",
                node.vault_dir().display()
            );
        } else {
            eprintln!(
                "itsanas: the vault at {} lost its marker (was the disk unplugged?); pausing",
                node.vault_dir().display()
            );
        }
        PAUSED_FOR_VAULT.store(true, Ordering::SeqCst);
        shutdown.store(true, Ordering::SeqCst);
    }
}

/// Whether this run stops for its vault's disk. It dropped under a running
/// daemon: stop rather than run a round that writes hosted data to the local
/// mount point; `itsanas daemon` then waits for the disk and starts again by
/// itself. Away, the disk coming back is the news: this run stops, and
/// `itsanas daemon` opens the node again on its real vault.
pub(super) fn stops_for_vault(node: &Node, shutdown: &AtomicBool) -> bool {
    let present = vault_present(&node.vault_dir());
    if node.away == present {
        if present {
            eprintln!(
                "itsanas: the vault's disk is back at {}; hosting again",
                node.vault_dir().display()
            );
        } else {
            eprintln!(
                "itsanas: the vault at {} lost its marker (was the disk unplugged?); pausing",
                node.vault_dir().display()
            );
        }
        PAUSED_FOR_VAULT.store(true, Ordering::SeqCst);
        shutdown.store(true, Ordering::SeqCst);
        return true;
    }
    false
}

/// Stop dialling: the process is ending, or somebody paused syncing during
/// this round. Read before each machine, so a pause gives the bandwidth back
/// once the session under way ends rather than when the whole round does.
pub(super) fn halted(node: &Node, shutdown: &AtomicBool) -> bool {
    shutdown.load(Ordering::Relaxed) || control::paused_on_disk(&node.home)
}

pub(super) fn install_signal_handler() -> Result<()> {
    // Once per process: a run that resumes after its vault's disk came back
    // installs nothing new, and `ctrlc` refuses a second handler.
    static INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if INSTALLED.get().is_some() {
        return Ok(());
    }
    ctrlc::set_handler(|| {
        PAUSED_FOR_VAULT.store(false, Ordering::SeqCst);
        SHUTDOWN.store(true, Ordering::Relaxed);
        println!();
        println!("stopping — finishing the current round…");
    })
    .map_err(|error| CliError::Usage(format!("could not install a signal handler: {error}")))?;
    let _ = INSTALLED.set(());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::SHUTDOWN_POLL;
    use super::*;

    #[test]
    fn a_panic_anywhere_in_the_scope_raises_the_shutdown_flag() {
        // Without this the daemon hangs instead of dying: the scope joins the
        // listener thread, the listener waits on a flag the panic skipped, and
        // the process stays alive, serving, never syncing, and looking healthy
        // to systemd. A supervisor that cannot tell a working daemon from a
        // hung one is not supervising anything.
        let flag = AtomicBool::new(false);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _stop = StopEverythingOnDrop(&flag);
            panic!("the sync loop died");
        }));
        assert!(outcome.is_err(), "the panic did not reach the caller");
        assert!(
            flag.load(Ordering::Relaxed),
            "the daemon would have hung rather than exited"
        );
    }

    #[test]
    fn shutdown_is_noticed_quickly_enough_to_feel_immediate() {
        // A Ctrl-C that took a whole interval to be noticed would be
        // indistinguishable from a hang.
        assert!(SHUTDOWN_POLL <= Duration::from_millis(500));
    }
}
