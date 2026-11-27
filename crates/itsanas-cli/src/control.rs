//! What a person asked the running daemon to do: pause, resume, sync now, sync
//! every so often.
//!
//! The daemon holds the store's lock for as long as it runs (HANDOVER §9, "one
//! process per node"), so a tray icon, a setup wizard or a second `itsanas`
//! cannot open the store to tell it anything. They do not need to. The home
//! directory is already the boundary of trust -- whoever can write in it can
//! read the keystore -- so a small file there is a control channel with no
//! socket to secure, no port to collide with a second node, and the same
//! behaviour wherever this daemon runs: Windows, macOS, Linux, Termux. (The
//! Android app has a loop of its own, which does not read it yet.)
//!
//! How long a request waits: an idle daemon looks every two seconds
//! (`wait_for_work`). During a round, a pause is looked for again before each
//! machine is dialled ([`paused_on_disk`]), so the transfer under way finishes
//! with the machine it is talking to -- at most `PEER_SESSION_BUDGET`, five
//! minutes -- and no other is started. Pausing is for giving the bandwidth
//! back; "after the round", which can be hours on a first sync, would not.
//!
//! The file is lines of `key value`; a key this version does not know is
//! ignored, so a newer tray can add one without stopping an older daemon:
//!
//! ```text
//! paused 1759750000      # since when, Unix seconds; absent = syncing
//! until 1759757200       # when the pause ends by itself; absent = until resumed
//! interval 600           # seconds the person chose; absent = --interval or the policy
//! sync-now 1759750123    # when "sync now" was last asked for
//! ```
//!
//! A pause with an end is decided by the wall clock, not by a timer in the
//! daemon: "for two hours" is what the person meant, it must survive a restart
//! of the daemon or of the machine, and only a date in the file does. A clock
//! set back makes the pause last longer, never shorter -- the safe side for a
//! pause chosen to spare a metered link.
//!
//! Writers replace it whole, through a temporary name and a rename, so the
//! daemon never reads half of one. Two writers at once (a tray and a terminal)
//! means the last one wins, which for a person clicking twice is the right
//! answer.

use std::{
    fmt::Write as _,
    io::{ErrorKind, Read as _},
    path::Path,
    time::Duration,
};

mod durations;
mod schedule;

pub use durations::{describe_every, describe_moment, describe_wait, parse_every, parse_pause};
pub use schedule::{Next, Rounds, Steering};

/// The file, in the node's home.
pub const CONTROL: &str = "control";

/// The most of the file the daemon reads: three short lines fit many times
/// over. It is read every two seconds, so a file that grew by accident -- a log
/// redirected into it, a tool's output -- would otherwise be read whole, every
/// two seconds. Past this, the whole lines that fit are read and the rest is
/// ignored: refusing the file instead would lose a pause written before the
/// growth, and every reader of a refused file has to guess (#243's review).
pub const MAX_CONTROL_BYTES: u64 = 4096;

/// The shortest interval a person can ask for.
///
/// Every round dials each machine the account knows that is on or due its
/// next try (`contact::QUIET_AFTER_MISS`), so the floor is what
/// keeps a slip of the finger -- `interval 0`, or a file edited by hand -- from
/// becoming a daemon that dials in a tight loop, hammering its peers and
/// spending somebody's mobile data. `--interval`, the operator's flag, keeps
/// its own floor of one second: test rigs need it, and a person never types
/// it into a tray.
pub const MIN_INTERVAL: Duration = Duration::from_secs(30);

/// How long a change found in the folder is left to settle before the round
/// that sends it out: a save is often several writes, and a copy of many
/// files takes a few seconds to land.
pub const SETTLE: Duration = Duration::from_secs(5);

/// The settle in application mode (HANDOVER §8 00): the program writes one
/// small file at a time and closes it, so there is no burst to wait out, and
/// the seconds saved are seconds the file is on one machine only.
pub const APP_SETTLE: Duration = Duration::from_secs(2);

/// How often a machine in application mode dials the other machines in
/// application mode it knows, between its full rounds (a light round: only
/// them, nobody else). A minute: both ends asked for it, and the machines
/// that did not -- a phone, the Pi, a laptop -- are dialled at their own
/// interval as before.
pub const APP_INTERVAL: Duration = Duration::from_secs(60);

/// The longest: a day. Past that a machine is not syncing, it is off, and the
/// snapshot's "stale after two intervals" would take days to say so.
pub const MAX_INTERVAL: Duration = Duration::from_secs(86_400);

/// The shortest pause with an end: a minute. Shorter is a typo for minutes.
pub const MIN_PAUSE: Duration = Duration::from_secs(60);

/// The longest: thirty days. A pause forgotten on a backup is somebody who
/// believes their files are safe (the reason `OneDrive` forces a duration); a
/// month covers a trip, and past it `itsanas pause` with no end says outright
/// that it lasts until resumed.
pub const MAX_PAUSE: Duration = Duration::from_secs(30 * 86_400);

/// What the file says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Control {
    /// When syncing was paused, in Unix seconds; `None` while it runs.
    pub paused_since: Option<u64>,
    /// When the pause ends by itself, in Unix seconds; `None` lasts until
    /// `itsanas resume`. Meaningless without `paused_since`.
    pub until: Option<u64>,
    /// The interval the person chose, in seconds; `None` leaves the daemon's
    /// own (`--interval`, else the sync policy).
    pub interval: Option<u64>,
    /// When "sync now" was last asked for, in Unix seconds.
    pub sync_asked: Option<u64>,
    /// `1` while this machine runs in application mode (`itsanas app on`);
    /// anything else, or nothing, is off.
    pub app: Option<u64>,
}

impl Control {
    /// Read the file. A missing one is the default: nothing asked.
    ///
    /// # Errors
    ///
    /// A file that exists and cannot be read or understood. The caller decides
    /// what that means; the daemon keeps what it last understood
    /// ([`Steering::refresh`]).
    pub fn read(home: &Path) -> Result<Self, String> {
        let path = home.join(CONTROL);
        let unreadable =
            |error: std::io::Error| format!("could not read {}: {error}", path.display());
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(unreadable(error)),
        };
        let mut bytes = Vec::new();
        file.take(MAX_CONTROL_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(unreadable)?;
        let rewrite = "`itsanas pause` or `itsanas resume` rewrites it";
        if bytes.len() as u64 > MAX_CONTROL_BYTES {
            // Cut at the last whole line, so a character split by the limit
            // is never what decides whether the file can be read.
            bytes.truncate(usize::try_from(MAX_CONTROL_BYTES).unwrap_or(usize::MAX));
            let Some(end) = bytes.iter().rposition(|&byte| byte == b'\n') else {
                return Err(format!(
                    "{} has no line end in its first {MAX_CONTROL_BYTES} bytes; {rewrite}",
                    path.display()
                ));
            };
            bytes.truncate(end + 1);
        }
        // UTF-16, which PowerShell 5's `Out-File` writes, is NUL between every
        // letter: `p\0a\0u\0s\0e\0d\0` would parse as an unknown key and
        // read as "not paused".
        if bytes.contains(&0) {
            return Err(format!(
                "{} holds NUL bytes (written as UTF-16?); {rewrite}",
                path.display()
            ));
        }
        let text = String::from_utf8(bytes)
            .map_err(|_| format!("{} is not UTF-8 text; {rewrite}", path.display()))?;
        Self::parse(&text)
    }

    /// # Errors
    ///
    /// A key this version knows with a value that is not a number. Refused
    /// rather than skipped: `paused yes` skipped would read as "not paused".
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut control = Self::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let mut words = line.split_whitespace();
            let (Some(key), value) = (words.next(), words.next()) else {
                continue;
            };
            let slot = match key {
                "paused" => &mut control.paused_since,
                "until" => &mut control.until,
                "interval" => &mut control.interval,
                "sync-now" => &mut control.sync_asked,
                "app" => &mut control.app,
                _ => continue,
            };
            let number = value
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| {
                    format!("the control file says `{line}`, which is not `{key} <number>`")
                })?;
            *slot = Some(number);
        }
        Ok(control)
    }

    #[must_use]
    pub fn render(&self) -> String {
        let mut text = String::new();
        for (key, value) in [
            ("paused", self.paused_since),
            ("until", self.until),
            ("interval", self.interval),
            ("sync-now", self.sync_asked),
            ("app", self.app),
        ] {
            if let Some(value) = value {
                let _ = writeln!(text, "{key} {value}");
            }
        }
        text
    }

    /// Whether this machine runs in application mode.
    #[must_use]
    pub fn app_mode(&self) -> bool {
        self.app == Some(1)
    }

    /// Whether the pause holds at `now_unix`: written, and either open-ended
    /// or not over yet. `now < until`, never `now + something`: an `until` of
    /// `u64::MAX`, typed or corrupted, is a pause that lasts, not an overflow.
    #[must_use]
    pub fn pause_holds(&self, now_unix: u64) -> bool {
        self.paused_since.is_some() && self.until.is_none_or(|until| now_unix < until)
    }

    /// How long the pause lasts, in words: `until you resume`, or `until
    /// 2026-10-06 14:30 UTC (in 2 h)`.
    #[must_use]
    pub fn lasts(&self, now_unix: u64) -> String {
        match self.until {
            None => "until you resume".to_owned(),
            Some(until) => match describe_moment(until) {
                Some(moment) => format!(
                    "until {moment} (in {})",
                    describe_wait(until.saturating_sub(now_unix))
                ),
                None => "until you resume (the end it was given is past any date)".to_owned(),
            },
        }
    }

    /// Replace the file whole.
    ///
    /// # Errors
    ///
    /// The home cannot be written.
    pub fn write(&self, home: &Path) -> std::io::Result<()> {
        // Through a temporary name: the daemon reads this every two seconds,
        // and a truncated file would read as "nothing asked" -- a paused node
        // resuming because it was looked at mid-write.
        let pending = home.join(format!("{CONTROL}.new"));
        std::fs::write(&pending, self.render())?;
        std::fs::rename(&pending, home.join(CONTROL))
    }
}

/// Whether the file says paused, read now. For the middle of a round, where
/// [`Steering`] is not consulted. A file that cannot be read does not stop a
/// round: interrupting is the change, and a change needs a file that says so.
#[must_use]
pub fn paused_on_disk(home: &Path) -> bool {
    let now = itsanas_discover::now_unix();
    Control::read(home).is_ok_and(|control| control.pause_holds(now))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_round_trips_and_ignores_keys_it_does_not_know() {
        let control = Control {
            paused_since: Some(1_759_750_000),
            until: Some(1_759_757_200),
            interval: Some(600),
            sync_asked: Some(1_759_750_123),
            app: Some(1),
        };
        assert_eq!(Control::parse(&control.render()), Ok(control));
        let newer = "paused 5\nbandwidth 20M\n# a comment\n\ninterval 60 # one minute\n";
        assert_eq!(
            Control::parse(newer),
            Ok(Control {
                paused_since: Some(5),
                until: None,
                interval: Some(60),
                sync_asked: None,
                app: None,
            }),
            "a key from a newer tray stopped an older daemon reading the rest"
        );
        assert!(
            Control::parse("paused yes").is_err(),
            "`paused yes` read as not paused"
        );
    }

    #[test]
    fn writing_replaces_the_file_whole_and_a_missing_file_asks_nothing() {
        let home = tempfile::tempdir().expect("temp dir");
        assert_eq!(Control::read(home.path()), Ok(Control::default()));
        let paused = Control {
            paused_since: Some(7),
            ..Control::default()
        };
        paused.write(home.path()).expect("write");
        Control::default().write(home.path()).expect("overwrite");
        assert_eq!(Control::read(home.path()), Ok(Control::default()));
        assert!(
            !home.path().join(format!("{CONTROL}.new")).exists(),
            "the temporary file was left behind"
        );
    }

    /// A file that grew -- a log redirected into it -- is read as far as the
    /// limit, never whole, and still says what was written first: refusing
    /// it would leave every reader guessing whether the node is paused.
    #[test]
    fn red_team_a_control_file_that_grew_still_says_paused() {
        let home = tempfile::tempdir().expect("temp dir");
        let path = home.path().join(CONTROL);
        let paused = |home: &Path| Control::read(home).map(|read| read.paused_since);

        let mut grown = "paused 1\n".to_owned();
        grown.push_str(&"# a log redirected here by mistake\n".repeat(30_000));
        std::fs::write(&path, grown).expect("write");
        assert_eq!(
            paused(home.path()),
            Ok(Some(1)),
            "a megabyte appended after `paused 1` lost the pause"
        );
        // A multi-byte character split by the limit does not make it unreadable.
        std::fs::write(&path, format!("paused 2\n#{}", "é".repeat(3000))).expect("write");
        assert_eq!(paused(home.path()), Ok(Some(2)));
        // Exactly at the limit, the last line is read too.
        let exact = format!("{}\npaused 3\n", "#".repeat(4096 - 10));
        assert_eq!(exact.len(), 4096);
        std::fs::write(&path, &exact).expect("write");
        assert_eq!(paused(home.path()), Ok(Some(3)));
        // UTF-16 is refused rather than read as "nothing asked".
        let utf16: Vec<u8> = "paused 4\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        std::fs::write(&path, utf16).expect("write");
        assert!(
            paused(home.path()).is_err(),
            "UTF-16 `paused 4` read as not paused"
        );
    }
}
