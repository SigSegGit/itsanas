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
//! interval 600           # seconds the person chose; absent = --interval or the policy
//! sync-now 1759750123    # when "sync now" was last asked for
//! ```
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

/// What the file says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Control {
    /// When syncing was paused, in Unix seconds; `None` while it runs.
    pub paused_since: Option<u64>,
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
            ("interval", self.interval),
            ("sync-now", self.sync_asked),
        ] {
            if let Some(value) = value {
                let _ = writeln!(text, "{key} {value}");
            }
        }
        text
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
    Control::read(home).is_ok_and(|control| control.paused_since.is_some())
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
    pub fn start(read: Result<Control, String>) -> (Self, Option<String>) {
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
        };
        (steering, said)
    }

    /// Take a fresh read of the file, and say what changed, for the log.
    ///
    /// A read that fails keeps the last state understood. The alternative is
    /// the default, and the default is "not paused": a file caught in an odd
    /// state by a backup tool or an editor would resume a node somebody paused
    /// on purpose -- on a metered connection, say -- without telling them.
    pub fn refresh(&mut self, read: Result<Control, String>) -> Vec<String> {
        let fresh = match read {
            Ok(fresh) => {
                self.unreadable = false;
                fresh
            }
            Err(why) => {
                if self.unreadable {
                    return Vec::new();
                }
                self.unreadable = true;
                return vec![format!("{why}; keeping what it said before")];
            }
        };
        let mut said = Vec::new();
        match (self.current.paused_since, fresh.paused_since) {
            (None, Some(_)) => {
                said.push("syncing paused (`itsanas resume` to continue)".to_owned());
            }
            (Some(_), None) => said.push("syncing resumed".to_owned()),
            _ => {}
        }
        if self.current.interval != fresh.interval {
            said.push(match fresh.interval {
                Some(_) => format!(
                    "syncing every {}s, as asked (`itsanas interval`)",
                    clamp(fresh.interval).map_or(0, |every| every.as_secs())
                ),
                None => "syncing on the daemon's own interval again".to_owned(),
            });
        }
        self.current = fresh;
        said
    }

    #[must_use]
    pub const fn paused(&self) -> bool {
        self.current.paused_since.is_some()
    }

    /// The interval in force: the person's choice, bounded, else `base`.
    #[must_use]
    pub fn interval(&self, base: Duration) -> Duration {
        clamp(self.current.interval).unwrap_or(base)
    }

    /// What to do now, given when the last round ended.
    ///
    /// A "sync now" is consumed whether or not it is acted on: asked while
    /// paused, it is refused at the command line, and one written anyway must
    /// not wait in the file to fire the moment somebody resumes.
    pub fn next(&mut self, now: Instant, last_round: Option<Instant>, base: Duration) -> Next {
        let asked = self.current.sync_asked.is_some() && self.current.sync_asked != self.honoured;
        if asked {
            self.honoured = self.current.sync_asked;
        }
        let due = asked || last_round.is_none_or(|last| now >= last + self.interval(base));
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
    let (digits, unit) = text
        .find(|c: char| !c.is_ascii_digit())
        .map_or((text, ""), |at| text.split_at(at));
    let scale = match unit.trim() {
        "" | "s" => 1,
        "m" | "min" => 60,
        "h" => 3600,
        "d" => 86_400,
        _ => 0,
    };
    let seconds = digits
        .parse::<u64>()
        .ok()
        .filter(|_| scale > 0)
        .and_then(|n| n.checked_mul(scale))
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

    fn steering(control: Control) -> Steering {
        Steering::start(Ok(control)).0
    }

    #[test]
    fn the_file_round_trips_and_ignores_keys_it_does_not_know() {
        let control = Control {
            paused_since: Some(1_759_750_000),
            interval: Some(600),
            sync_asked: Some(1_759_750_123),
        };
        assert_eq!(Control::parse(&control.render()), Ok(control));
        let newer = "paused 5\nbandwidth 20M\n# a comment\n\ninterval 60 # one minute\n";
        assert_eq!(
            Control::parse(newer),
            Ok(Control {
                paused_since: Some(5),
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
        let (mut steering, _) = Steering::start(Ok(asked));
        let soon = start + Duration::from_secs(2);
        assert_eq!(steering.next(soon, Some(start), BASE), Next::Wait);

        let again = Control {
            sync_asked: Some(101),
            ..Control::default()
        };
        steering.refresh(Ok(again));
        assert_eq!(steering.next(soon, Some(start), BASE), Next::Round);
        for _ in 0..3 {
            steering.refresh(Ok(again));
            assert_eq!(
                steering.next(soon, Some(soon), BASE),
                Next::Wait,
                "the same request ran a second round: one click, a daemon syncing every two seconds"
            );
        }
        // A writer whose clock went back is still a new request.
        steering.refresh(Ok(Control {
            sync_asked: Some(50),
            ..Control::default()
        }));
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
        steering.refresh(Ok(Control {
            paused_since: Some(1),
            sync_asked: Some(9),
            ..Control::default()
        }));
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
        let said = steering.refresh(Ok(Control::default()));
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
        let said = steering.refresh(Err("could not read control".to_owned()));
        assert!(
            steering.paused(),
            "an unreadable file resumed a paused node"
        );
        assert_eq!(said.len(), 1, "the failure was not said");
        assert!(
            steering.refresh(Err("again".to_owned())).is_empty(),
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
        let (mut steering, said) = Steering::start(Err("could not read control".to_owned()));
        assert!(said.is_some_and(|said| said.contains("starting paused")));
        assert_eq!(
            steering.next(Instant::now(), None, BASE),
            Next::Publish,
            "a daemon that could not read its control file started syncing"
        );
        // Once the file can be read, it decides.
        let said = steering.refresh(Ok(Control::default()));
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
}
