//! Noticing that the folder changed, without reacting mid-write.
//!
//! A filesystem watcher is a latency optimisation, never a source of truth.
//! Every platform's watcher drops events under load, misses changes made while
//! the process was not running, and reports a file several times while a
//! program is still writing it. So this is deliberately only half the story:
//! the watcher says "look now", and a periodic full rescan catches whatever it
//! missed. A design that trusted the watcher alone would lose files and take
//! months to notice.
//!
//! # Debounce
//!
//! Saving a file is rarely one event. An editor may truncate, write, rename and
//! set the modification time, and a large copy produces a steady stream for as
//! long as it takes. Importing on the first event would capture a half-written
//! file — and because the store hashes what it reads, that truncated content
//! would become a real version and replicate. Waiting for the folder to fall
//! quiet is what avoids that.

use std::{
    path::Path,
    sync::mpsc::{Receiver, RecvTimeoutError, channel},
    time::{Duration, Instant},
};

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::{error::Result, scan::STAGING_DIR};

/// How long the folder must be quiet before a change is acted on.
pub const DEFAULT_DEBOUNCE: Duration = Duration::from_millis(750);

/// What the watcher reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// Something in the folder changed.
    Touched,
}

/// Watches a folder and reports when it has settled.
#[derive(Debug)]
pub struct Watcher {
    // Held only to keep the watch alive; dropping it stops the notifications.
    _watcher: RecommendedWatcher,
    events: Receiver<Change>,
}

impl Watcher {
    /// Start watching `root` and everything under it.
    pub fn new(root: &Path) -> Result<Self> {
        let (sender, events) = channel();

        let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            let Ok(event) = result else {
                // A watcher error usually means the backend lost track. There
                // is nothing useful to do about it here, and the periodic
                // rescan is what covers it.
                return;
            };

            if !is_interesting(&event) {
                return;
            }

            // The receiver being gone means the daemon is shutting down.
            let _ = sender.send(Change::Touched);
        })?;

        watcher.watch(root, RecursiveMode::Recursive)?;

        Ok(Self {
            _watcher: watcher,
            events,
        })
    }

    /// Wait for a change, then for the folder to fall quiet.
    ///
    /// Returns `true` if something changed. Returns `false` if `timeout`
    /// elapsed with nothing happening, which is the caller's cue to do its
    /// periodic work.
    ///
    /// `settle_limit` caps how long the quiet period can be extended by a
    /// continuous stream of events. Without it, copying a large directory in
    /// would postpone the import for as long as the copy took — correct in
    /// principle, but indistinguishable from a hang.
    #[must_use]
    pub fn wait_for_quiet(
        &self,
        timeout: Duration,
        debounce: Duration,
        settle_limit: Duration,
    ) -> bool {
        match self.events.recv_timeout(timeout) {
            Ok(Change::Touched) => {}
            Err(RecvTimeoutError::Timeout) => return false,
            // The watcher thread is gone. Report a change so the caller does a
            // scan rather than silently going blind.
            Err(RecvTimeoutError::Disconnected) => return true,
        }

        let started = Instant::now();

        // Drain until nothing arrives for a whole debounce period.
        loop {
            match self.events.recv_timeout(debounce) {
                Ok(Change::Touched) => {
                    if started.elapsed() >= settle_limit {
                        return true;
                    }
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return true,
            }
        }
    }

    /// Whether any change is waiting, without blocking.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.events.try_recv().is_ok()
    }
}

/// Whether an event is worth waking up for.
fn is_interesting(event: &Event) -> bool {
    // Our own staging directory changes on every export. Reacting to it would
    // make the folder trigger itself forever.
    if event
        .paths
        .iter()
        .all(|path| path.components().any(|c| c.as_os_str() == STAGING_DIR))
    {
        return false;
    }

    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_debounce_is_long_enough_to_outlast_an_editor_save() {
        // An editor may truncate, write, rename and set the mtime. Acting on
        // the first event would import a half-written file, and because the
        // store hashes what it reads, that truncation would become a real
        // version and replicate.
        assert!(DEFAULT_DEBOUNCE >= Duration::from_millis(250));
        // But short enough that saving a file feels like it synced.
        assert!(DEFAULT_DEBOUNCE <= Duration::from_secs(3));
    }

    #[test]
    fn a_watcher_starts_on_a_real_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Watcher::new(dir.path()).is_ok());
    }

    #[test]
    fn watching_a_missing_directory_is_an_error_rather_than_silence() {
        // Silently watching nothing would mean the daemon believes it is
        // reacting to changes when it is not.
        let dir = tempfile::tempdir().unwrap();
        assert!(Watcher::new(&dir.path().join("absent")).is_err());
    }

    #[test]
    fn a_quiet_folder_times_out_rather_than_blocking_forever() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = Watcher::new(dir.path()).unwrap();

        let started = Instant::now();
        let changed = watcher.wait_for_quiet(
            Duration::from_millis(200),
            Duration::from_millis(50),
            Duration::from_secs(1),
        );

        assert!(!changed, "a quiet folder reported a change");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the timeout was not honoured"
        );
    }

    #[test]
    fn a_real_change_is_noticed() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = Watcher::new(dir.path()).unwrap();

        let path = dir.path().join("appeared.txt");
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            let _ = std::fs::write(path, b"hello");
        });

        assert!(
            watcher.wait_for_quiet(
                Duration::from_secs(10),
                Duration::from_millis(100),
                Duration::from_secs(2),
            ),
            "a new file did not wake the watcher"
        );
    }
}
