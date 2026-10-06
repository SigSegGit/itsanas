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
    time::{Duration, Instant},
};

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
/// Every round dials each machine the account knows, so the floor is what
/// keeps a slip of the finger -- `interval 0`, or a file edited by hand -- from
/// becoming a daemon that dials in a tight loop, hammering its peers and
/// spending somebody's mobile data. `--interval`, the operator's flag, keeps
/// its own floor of one second: test rigs need it, and a person never types
/// it into a tray.
pub const MIN_INTERVAL: Duration = Duration::from_secs(30);

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

/// The last second of the year 9999. A pause ending later has no date worth
/// printing, and the date arithmetic below stays far from any overflow.
const LAST_DATED_SECOND: u64 = 253_402_300_799;

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
        ] {
            if let Some(value) = value {
                let _ = writeln!(text, "{key} {value}");
            }
        }
        text
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

/// What the daemon should do this time round its loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Nothing is due.
    Wait,
    /// Paused, and a round is due: only say where this machine is, so the
    /// others can still reach what it hosts for them. No file moves.
    Publish,
    /// A full round.
    Round,
}

/// The daemon's reading of the control file, kept between reads.
#[derive(Debug)]
pub struct Steering {
    current: Control,
    /// The "sync now" stamp already acted on. Compared for difference, not
    /// order, so a writer whose clock went back still gets its round.
    honoured: Option<u64>,
    /// Whether the last read failed, so the failure is said once.
    unreadable: bool,
    /// Whether the pause held at the last refresh: what [`Self::paused`]
    /// answers, so one turn of the daemon's loop sees one answer.
    holding: bool,
    /// A pause just ended, by the clock or by `resume`: the next decision is
    /// a round at once, not a wait for the interval. A person whose pause of
    /// an hour is over expects their files to move, not an hour more.
    lifted: bool,
}

impl Steering {
    /// Start from what the file says now. A "sync now" already in it is taken
    /// as done: the daemon syncs as it starts anyway, and replaying an old
    /// request at every restart would be a round nobody asked for.
    ///
    /// A file that cannot be read starts the daemon **paused**. There is no
    /// earlier state to keep, and the file exists, so somebody wrote it: the
    /// commonest thing written there is a pause. Starting to sync would
    /// override one -- on a metered link, say -- and say nothing. Paused, the
    /// machine still hosts for the others, `status` says `unknown`, and
    /// `itsanas resume` rewrites the file. (#243's review.)
    #[must_use]
    pub fn start(read: Result<Control, String>, now_unix: u64) -> (Self, Option<String>) {
        let (current, unreadable, said) = match read {
            Ok(current) => (current, false, None),
            Err(why) => (
                Control {
                    paused_since: Some(0),
                    ..Control::default()
                },
                true,
                Some(format!(
                    "{why}; starting paused until it can be read (`itsanas resume` rewrites it)"
                )),
            ),
        };
        let steering = Self {
            current,
            honoured: current.sync_asked,
            unreadable,
            holding: current.pause_holds(now_unix),
            lifted: false,
        };
        (steering, said)
    }

    /// Take a fresh read of the file and the clock, and say what changed, for
    /// the log.
    ///
    /// A read that fails keeps the last state understood. The alternative is
    /// the default, and the default is "not paused": a file caught in an odd
    /// state by a backup tool or an editor would resume a node somebody paused
    /// on purpose -- on a metered connection, say -- without telling them. A
    /// pause with an end still ends on time then: the end is what was asked.
    pub fn refresh(&mut self, read: Result<Control, String>, now_unix: u64) -> Vec<String> {
        let mut said = Vec::new();
        match read {
            Ok(fresh) => {
                self.unreadable = false;
                said.extend(self.interval_change(&fresh));
                self.current = fresh;
            }
            Err(why) => {
                if !self.unreadable {
                    self.unreadable = true;
                    said.push(format!("{why}; keeping what it said before"));
                }
            }
        }
        let holds = self.current.pause_holds(now_unix);
        match (self.holding, holds) {
            (false, true) => said.insert(
                0,
                format!(
                    "syncing paused {} (`itsanas resume` to continue)",
                    self.current.lasts(now_unix)
                ),
            ),
            // Still written in the file, but over by the clock.
            (true, false) if self.current.paused_since.is_some() => {
                said.insert(0, "pause over, syncing resumed".to_owned());
            }
            (true, false) => said.insert(0, "syncing resumed".to_owned()),
            _ => {}
        }
        self.lifted |= self.holding && !holds;
        self.holding = holds;
        said
    }

    fn interval_change(&self, fresh: &Control) -> Option<String> {
        (self.current.interval != fresh.interval).then(|| match fresh.interval {
            Some(_) => format!(
                "syncing every {}s, as asked (`itsanas interval`)",
                clamp(fresh.interval).map_or(0, |every| every.as_secs())
            ),
            None => "syncing on the daemon's own interval again".to_owned(),
        })
    }

    /// Whether syncing is paused, as of the last [`Self::refresh`].
    #[must_use]
    pub const fn paused(&self) -> bool {
        self.holding
    }

    /// How long the pause in force lasts, for the log.
    #[must_use]
    pub fn lasts(&self, now_unix: u64) -> String {
        self.current.lasts(now_unix)
    }

    /// The interval in force: the person's choice, bounded, else `base`.
    #[must_use]
    pub fn interval(&self, base: Duration) -> Duration {
        clamp(self.current.interval).unwrap_or(base)
    }

    /// What to do now, given when the last round ended.
    ///
    /// A pause that just ended is due at once, like a "sync now".
    ///
    /// A "sync now" is consumed whether or not it is acted on: asked while
    /// paused, it is refused at the command line, and one written anyway must
    /// not wait in the file to fire the moment somebody resumes.
    pub fn next(&mut self, now: Instant, last_round: Option<Instant>, base: Duration) -> Next {
        let asked = self.current.sync_asked.is_some() && self.current.sync_asked != self.honoured;
        if asked {
            self.honoured = self.current.sync_asked;
        }
        let lifted = std::mem::take(&mut self.lifted);
        let due =
            asked || lifted || last_round.is_none_or(|last| now >= last + self.interval(base));
        match (due, self.paused()) {
            (false, _) => Next::Wait,
            (true, true) => Next::Publish,
            (true, false) => Next::Round,
        }
    }
}

fn clamp(seconds: Option<u64>) -> Option<Duration> {
    seconds.map(|seconds| Duration::from_secs(seconds).clamp(MIN_INTERVAL, MAX_INTERVAL))
}

/// Read `10m`, `1h`, `90s`, `1d` or a bare number of seconds; `auto` is `None`.
///
/// # Errors
///
/// Anything else, and anything outside [`MIN_INTERVAL`]..=[`MAX_INTERVAL`]:
/// said at the command line rather than clamped there, so the person learns
/// the bound instead of getting a number they did not type.
pub fn parse_every(text: &str) -> Result<Option<u64>, String> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    let seconds = seconds_in(text)
        .ok_or_else(|| format!("`{text}` is not a duration: say 30s, 10m, 1h, 1d, or auto"))?;
    if seconds < MIN_INTERVAL.as_secs() || seconds > MAX_INTERVAL.as_secs() {
        return Err(format!(
            "{} is outside what a daemon can be asked for: between {} and {}",
            describe_every(seconds),
            describe_every(MIN_INTERVAL.as_secs()),
            describe_every(MAX_INTERVAL.as_secs()),
        ));
    }
    Ok(Some(seconds))
}

/// Read how long `itsanas pause --for` lasts: `30m`, `2h`, `1d`, in seconds.
///
/// # Errors
///
/// Anything else, and anything outside [`MIN_PAUSE`]..=[`MAX_PAUSE`], said
/// with the bounds and the way to pause with no end.
pub fn parse_pause(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let seconds =
        seconds_in(text).ok_or_else(|| format!("`{text}` is not a duration: say 30m, 2h or 1d"))?;
    if seconds < MIN_PAUSE.as_secs() || seconds > MAX_PAUSE.as_secs() {
        return Err(format!(
            "a pause lasts between {} and {}; `itsanas pause` with no --for lasts until \
             `itsanas resume`",
            describe_every(MIN_PAUSE.as_secs()),
            describe_every(MAX_PAUSE.as_secs()),
        ));
    }
    Ok(seconds)
}

/// `90s`, `10m`/`10min`, `2h`, `1d` or bare seconds; `None` for anything else or
/// a number too large to be seconds.
fn seconds_in(text: &str) -> Option<u64> {
    let (digits, unit) = text
        .find(|c: char| !c.is_ascii_digit())
        .map_or((text, ""), |at| text.split_at(at));
    let scale = match unit.trim() {
        "" | "s" => 1,
        "m" | "min" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => return None,
    };
    digits.parse::<u64>().ok()?.checked_mul(scale)
}

/// A wait in words, rounded up to the minute so "in 0 min" is never said of a
/// pause that still holds: `5 min`, `2 h 5 min`, `3 days`.
#[must_use]
pub fn describe_wait(seconds: u64) -> String {
    let minutes = seconds.div_ceil(60);
    match minutes {
        0..=119 => format!("{minutes} min"),
        120..=2879 if minutes.is_multiple_of(60) => format!("{} h", minutes / 60),
        120..=2879 => format!("{} h {} min", minutes / 60, minutes % 60),
        _ => format!("{} days", minutes / 1440),
    }
}

/// A Unix time as `2026-10-06 14:30 UTC`; `None` past the year 9999.
///
/// UTC, and said so: the daemon of a headless machine has no reliable local
/// zone, and a tray shows the person's own clock itself. Days to a civil date
/// by Howard Hinnant's algorithm, in whole numbers only (no date crate for
/// one line of output).
#[must_use]
pub fn describe_moment(unix: u64) -> Option<String> {
    if unix > LAST_DATED_SECOND {
        return None;
    }
    let (days, second) = (unix / 86_400, unix % 86_400);
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    Some(format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        second / 3600,
        second % 3600 / 60
    ))
}

/// `600` -> `10 min`, for messages and the tray.
#[must_use]
pub fn describe_every(seconds: u64) -> String {
    match seconds {
        86_400 => "1 day".to_owned(),
        s if s % 86_400 == 0 && s > 0 => format!("{} days", s / 86_400),
        s if s % 3600 == 0 && s > 0 => format!("{} h", s / 3600),
        s if s % 60 == 0 && s > 0 => format!("{} min", s / 60),
        s => format!("{s} s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: Duration = Duration::from_secs(300);

    /// A wall clock for the tests: 2025-10-06, give or take.
    const NOW: u64 = 1_759_750_000;

    fn steering(control: Control) -> Steering {
        Steering::start(Ok(control), NOW).0
    }

    #[test]
    fn the_file_round_trips_and_ignores_keys_it_does_not_know() {
        let control = Control {
            paused_since: Some(1_759_750_000),
            until: Some(1_759_757_200),
            interval: Some(600),
            sync_asked: Some(1_759_750_123),
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

    /// `interval 0` in the file -- typed, or a tray's bug -- would be a daemon
    /// dialling every machine of the account in a tight loop.
    #[test]
    fn red_team_a_control_file_cannot_make_the_daemon_dial_in_a_tight_loop() {
        let start = Instant::now();
        for asked in [0, 1, 29] {
            let mut steering = steering(Control {
                interval: Some(asked),
                ..Control::default()
            });
            assert_eq!(steering.interval(BASE), MIN_INTERVAL);
            assert_eq!(
                steering.next(start + Duration::from_secs(2), Some(start), BASE),
                Next::Wait,
                "`interval {asked}` started a round two seconds after the last: \
                 a daemon hammering its peers"
            );
        }
        let steering = steering(Control {
            interval: Some(u64::MAX),
            ..Control::default()
        });
        assert_eq!(steering.interval(BASE), MAX_INTERVAL);
    }

    /// One click on "sync now" is one round, not one per loop.
    #[test]
    fn red_team_one_sync_now_is_one_round_never_a_loop() {
        let start = Instant::now();
        let asked = Control {
            sync_asked: Some(100),
            ..Control::default()
        };
        // A request left in the file from before this daemon started is not
        // replayed: the daemon synced as it started.
        let (mut steering, _) = Steering::start(Ok(asked), NOW);
        let soon = start + Duration::from_secs(2);
        assert_eq!(steering.next(soon, Some(start), BASE), Next::Wait);

        let again = Control {
            sync_asked: Some(101),
            ..Control::default()
        };
        steering.refresh(Ok(again), NOW);
        assert_eq!(steering.next(soon, Some(start), BASE), Next::Round);
        for _ in 0..3 {
            steering.refresh(Ok(again), NOW);
            assert_eq!(
                steering.next(soon, Some(soon), BASE),
                Next::Wait,
                "the same request ran a second round: one click, a daemon syncing every two seconds"
            );
        }
        // A writer whose clock went back is still a new request.
        steering.refresh(
            Ok(Control {
                sync_asked: Some(50),
                ..Control::default()
            }),
            NOW,
        );
        assert_eq!(steering.next(soon, Some(soon), BASE), Next::Round);
    }

    /// Paused means no file moves, whatever else is asked.
    #[test]
    fn red_team_a_paused_node_never_runs_a_full_round() {
        let start = Instant::now();
        let mut steering = steering(Control {
            paused_since: Some(1),
            ..Control::default()
        });
        assert_eq!(
            steering.next(start, None, BASE),
            Next::Publish,
            "a daemon started paused synced files"
        );
        steering.refresh(
            Ok(Control {
                paused_since: Some(1),
                sync_asked: Some(9),
                ..Control::default()
            }),
            NOW,
        );
        assert_eq!(
            steering.next(start + Duration::from_secs(2), Some(start), BASE),
            Next::Publish,
            "a `sync-now` written while paused moved files"
        );
        assert_eq!(
            steering.next(start + BASE, Some(start), BASE),
            Next::Publish,
            "a due round moved files while paused"
        );
        let said = steering.refresh(Ok(Control::default()), NOW);
        assert_eq!(said, vec!["syncing resumed".to_owned()]);
        assert_eq!(steering.next(start + BASE, Some(start), BASE), Next::Round);
    }

    /// A file caught mid-edit, or locked by a backup tool, must not resume a
    /// node somebody paused on purpose.
    #[test]
    fn red_team_an_unreadable_control_file_never_resumes_a_paused_node() {
        let mut steering = steering(Control {
            paused_since: Some(1),
            ..Control::default()
        });
        let said = steering.refresh(Err("could not read control".to_owned()), NOW);
        assert!(
            steering.paused(),
            "an unreadable file resumed a paused node"
        );
        assert_eq!(said.len(), 1, "the failure was not said");
        assert!(
            steering.refresh(Err("again".to_owned()), NOW).is_empty(),
            "the same failure was logged every two seconds"
        );
        assert_eq!(steering.next(Instant::now(), None, BASE), Next::Publish);
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

    /// No earlier state to keep at start: a file that cannot be read must not
    /// turn into "syncing", which would override the pause it most likely holds.
    #[test]
    fn red_team_an_unreadable_control_file_at_start_never_syncs() {
        let (mut steering, said) = Steering::start(Err("could not read control".to_owned()), NOW);
        assert!(said.is_some_and(|said| said.contains("starting paused")));
        assert_eq!(
            steering.next(Instant::now(), None, BASE),
            Next::Publish,
            "a daemon that could not read its control file started syncing"
        );
        // Once the file can be read, it decides.
        let said = steering.refresh(Ok(Control::default()), NOW);
        assert_eq!(said, vec!["syncing resumed".to_owned()]);
        assert_eq!(steering.next(Instant::now(), None, BASE), Next::Round);
    }

    #[test]
    fn a_person_types_durations_and_learns_the_bounds() {
        assert_eq!(parse_every("10m"), Ok(Some(600)));
        assert_eq!(parse_every("1h"), Ok(Some(3600)));
        assert_eq!(parse_every("90"), Ok(Some(90)));
        assert_eq!(parse_every("1d"), Ok(Some(86_400)));
        assert_eq!(parse_every("AUTO"), Ok(None));
        let short = parse_every("5s").expect_err("below the floor");
        assert!(short.contains("30 s"), "{short}");
        assert!(parse_every("2d").is_err());
        assert!(parse_every("ten minutes").is_err());
        assert_eq!(describe_every(600), "10 min");
        assert_eq!(describe_every(90), "90 s");
        assert_eq!(describe_every(86_400), "1 day");
    }

    /// The pause the tray offers for an hour, ended by the clock alone: the
    /// file still says `paused`, nobody ran `resume`, and the machine must
    /// sync -- at once, not an interval later. A pause that never ends by
    /// itself is somebody who believes their files are safe.
    #[test]
    fn red_team_a_timed_pause_that_has_expired_syncs_by_itself() {
        let start = Instant::now();
        let hour = Control {
            paused_since: Some(NOW),
            until: Some(NOW + 3600),
            ..Control::default()
        };
        let mut steering = steering(hour);
        assert!(steering.paused());
        assert_eq!(steering.next(start, None, BASE), Next::Publish);

        let later = NOW + 3600;
        let said = steering.refresh(Ok(hour), later);
        assert_eq!(
            said,
            vec!["pause over, syncing resumed".to_owned()],
            "the end of a pause was not said in the log"
        );
        assert!(
            !steering.paused(),
            "an hour's pause still held after the hour: files left unsynced nobody knows about"
        );
        let soon = start + Duration::from_secs(2);
        assert_eq!(
            steering.next(soon, Some(start), BASE),
            Next::Round,
            "the pause ended and the machine waited a whole interval before syncing"
        );
        assert_eq!(
            steering.next(soon, Some(soon), BASE),
            Next::Wait,
            "the end of one pause ran a round every loop"
        );
        // The same file read at a daemon's start, after the hour: syncing.
        let (restarted, _) = Steering::start(Ok(hour), later + 1);
        assert!(
            !restarted.paused(),
            "a restart revived a pause that was over"
        );
        assert!(!hour.pause_holds(later));
    }

    /// The other side: a pause whose end has not come holds, whatever is due,
    /// and an `until` of `u64::MAX` -- typed, or a corrupted file -- is a pause
    /// that lasts, never an overflow that panics or wraps into "over".
    #[test]
    fn red_team_a_timed_pause_holds_until_its_end() {
        let start = Instant::now();
        let hour = Control {
            paused_since: Some(NOW),
            until: Some(NOW + 3600),
            ..Control::default()
        };
        let mut steering = steering(hour);
        for now in [NOW, NOW + 1, NOW + 3599] {
            steering.refresh(Ok(hour), now);
            assert_eq!(
                steering.next(start + BASE, Some(start), BASE),
                Next::Publish,
                "files moved {}s into an hour's pause",
                now - NOW
            );
        }
        let forever = Control {
            paused_since: Some(NOW),
            until: Some(u64::MAX),
            ..Control::default()
        };
        assert!(forever.pause_holds(NOW));
        assert!(forever.pause_holds(u64::MAX - 1));
        // The end is exclusive, at the last second there is as anywhere else.
        assert!(!forever.pause_holds(u64::MAX));
        let mut lasting = Steering::start(Ok(forever), NOW).0;
        assert!(lasting.refresh(Ok(forever), u64::MAX - 1).is_empty());
        assert_eq!(lasting.next(start, None, BASE), Next::Publish);
        assert_eq!(
            forever.lasts(NOW),
            "until you resume (the end it was given is past any date)"
        );
        assert_eq!(
            Control::parse(&forever.render()),
            Ok(forever),
            "u64::MAX did not survive the file"
        );
        assert!(
            !Control {
                until: Some(u64::MAX),
                ..Control::default()
            }
            .pause_holds(NOW),
            "an `until` with no pause paused the node"
        );
    }

    #[test]
    fn a_pause_is_asked_for_in_words_and_said_back_in_dates() {
        assert_eq!(parse_pause("1h"), Ok(3600));
        assert_eq!(parse_pause("8h"), Ok(8 * 3600));
        assert_eq!(parse_pause("1m"), Ok(60));
        assert_eq!(parse_pause("30d"), Ok(30 * 86_400));
        let short = parse_pause("30s").expect_err("below a minute");
        assert!(
            short.contains("1 min") && short.contains("30 days"),
            "{short}"
        );
        assert!(parse_pause("31d").is_err());
        assert!(parse_pause("99999999999999999999d").is_err());
        assert!(parse_pause("soon").is_err());
        assert_eq!(describe_moment(0).as_deref(), Some("1970-01-01 00:00 UTC"));
        assert_eq!(
            describe_moment(951_825_600).as_deref(),
            Some("2000-02-29 12:00 UTC")
        );
        assert_eq!(
            describe_moment(1_759_750_000).as_deref(),
            Some("2025-10-06 11:26 UTC")
        );
        assert_eq!(
            describe_moment(LAST_DATED_SECOND).as_deref(),
            Some("9999-12-31 23:59 UTC")
        );
        assert_eq!(describe_moment(u64::MAX), None);
        assert_eq!(describe_wait(1), "1 min");
        assert_eq!(describe_wait(3600), "60 min");
        assert_eq!(describe_wait(7200), "2 h");
        assert_eq!(describe_wait(7500), "2 h 5 min");
        assert_eq!(
            describe_wait(u64::MAX),
            format!("{} days", u64::MAX.div_ceil(60) / 1440)
        );
        let hour = Control {
            paused_since: Some(NOW),
            until: Some(NOW + 3600),
            ..Control::default()
        };
        assert_eq!(hour.lasts(NOW), "until 2025-10-06 12:26 UTC (in 60 min)");
        assert_eq!(Control::default().lasts(NOW), "until you resume");
    }
}
