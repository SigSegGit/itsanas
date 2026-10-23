//! Asking a running daemon to stop, or to restart on the program now on disk.
//!
//! Found during the hand test of 2026-10-22: `itsanas update` installed 0.1.9
//! and said "the background service was restarted" while the three daemons of
//! the laptop went on running 0.1.8 for hours, and `itsanas signout` could not
//! stop a daemon its logon task had started. Both asked the service manager,
//! which on Windows ends the task's console and leaves the PowerShell wrapper
//! and the daemon behind.
//!
//! So the daemon is asked directly, through a file in its home -- the
//! boundary of trust `control.rs` already relies on -- and it exits by
//! itself between two rounds:
//!
//! - **stop**: exit code 0, which every service manager here (the Windows
//!   wrapper, systemd's `Restart=on-failure`, launchd's `SuccessfulExit`)
//!   reads as "do not start it again";
//! - **restart**: exit code [`RESTART_EXIT`], which each of them restarts --
//!   on the program now installed, which is the point of an update.
//!
//! The asker then watches the store's lock: released, then (for a restart)
//! held again, and says what it saw, never what it hoped.

use std::{
    path::Path,
    time::{Duration, Instant},
};

/// The request, in the node's home: `stop` or `restart`.
pub const LIFECYCLE: &str = "lifecycle";

/// The exit code that asks the service manager for a fresh start.
pub const RESTART_EXIT: i32 = 75;

/// What a daemon is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Stop,
    Restart,
}

/// Ask the daemon of `home` for `request`.
///
/// # Errors
///
/// The home cannot be written.
pub fn ask(home: &Path, request: Request) -> std::io::Result<()> {
    let word = match request {
        Request::Stop => "stop",
        Request::Restart => "restart",
    };
    let pending = home.join(format!("{LIFECYCLE}.new"));
    std::fs::write(&pending, word)?;
    std::fs::rename(&pending, home.join(LIFECYCLE))
}

/// The daemon's side: the request waiting in `home`, removed so it is acted
/// on once. Anything but the two words is removed and ignored.
pub fn take(home: &Path) -> Option<Request> {
    let path = home.join(LIFECYCLE);
    // Two words at most: a file that grew is read no further than this.
    let mut text = String::new();
    {
        use std::io::Read as _;
        let file = std::fs::File::open(&path).ok()?;
        let _ = file.take(64).read_to_string(&mut text);
    }
    let _ = std::fs::remove_file(&path);
    match text.trim() {
        "stop" => Some(Request::Stop),
        "restart" => Some(Request::Restart),
        _ => None,
    }
}

fn running(home: &Path) -> bool {
    itsanas_store::Store::is_locked(itsanas_node::Node::store_path(home))
}

/// Wait until `still` turns false, polling; whether it did within `limit`.
fn wait_until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    done()
}

/// How long a daemon may take to notice: it looks every two seconds when
/// idle, after the transfer under way in a round otherwise.
pub const NOTICE: Duration = Duration::from_secs(120);

/// How long a service manager may take to start it again: the Windows
/// wrapper waits ten seconds before a restart.
pub const COMEBACK: Duration = Duration::from_secs(60);

/// Ask the daemon of `home` to stop and wait for it to let go of the store.
/// Whether it did.
pub fn stop_and_wait(home: &Path) -> bool {
    if !running(home) {
        return true;
    }
    if ask(home, Request::Stop).is_err() {
        return false;
    }
    wait_until(NOTICE, || !running(home))
}

/// What became of a restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restarted {
    /// It let go of the store and holds it again.
    Yes,
    /// It stopped and nothing started it again.
    StoppedOnly,
    /// It never let go.
    No,
    /// It was not running.
    WasNotRunning,
}

/// Ask the daemon of `home` to restart, and watch it go and come back.
pub fn restart_and_wait(home: &Path) -> Restarted {
    if !running(home) {
        return Restarted::WasNotRunning;
    }
    if ask(home, Request::Restart).is_err() || !wait_until(NOTICE, || !running(home)) {
        return Restarted::No;
    }
    if wait_for_comeback(home) {
        Restarted::Yes
    } else {
        Restarted::StoppedOnly
    }
}

/// Wait up to [`COMEBACK`] for the daemon of `home` to hold its store again.
pub fn wait_for_comeback(home: &Path) -> bool {
    wait_until(COMEBACK, || running(home))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_taken_once_and_nonsense_is_dropped() {
        let home = tempfile::tempdir().expect("home");
        assert_eq!(take(home.path()), None);
        ask(home.path(), Request::Restart).expect("ask");
        assert_eq!(take(home.path()), Some(Request::Restart));
        assert_eq!(take(home.path()), None, "acted on twice");
        ask(home.path(), Request::Stop).expect("ask");
        assert_eq!(take(home.path()), Some(Request::Stop));
        std::fs::write(home.path().join(LIFECYCLE), "reboot the world").expect("write");
        assert_eq!(take(home.path()), None);
        assert!(
            !home.path().join(LIFECYCLE).exists(),
            "nonsense is not met again"
        );
    }

    #[test]
    fn a_node_that_is_not_running_is_said_so_rather_than_restarted() {
        let home = tempfile::tempdir().expect("home");
        assert_eq!(restart_and_wait(home.path()), Restarted::WasNotRunning);
        assert!(stop_and_wait(home.path()));
    }
}
