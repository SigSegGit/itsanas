//! When the daemon runs its next round: the control file read between
//! rounds, a pause holding or lifted, "sync now", a change left to settle,
//! and application mode's light rounds.

use std::time::{Duration, Instant};

use super::{APP_INTERVAL, APP_SETTLE, Control, MAX_INTERVAL, MIN_INTERVAL, SETTLE};

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
    /// Application mode: send what changed to the other machines in
    /// application mode, and dial nobody else ([`APP_INTERVAL`]).
    Light,
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
    /// When this folder first changed since the last round, if it did: a
    /// new local file goes out [`SETTLE`] later, not at the end of the
    /// interval (hand test, 2026-10-22: five minutes for a file to reach a
    /// key). Never sooner than [`MIN_INTERVAL`] after the last round, so a
    /// folder rewritten every second costs a round per half minute at most.
    changed: Option<Instant>,
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
            changed: None,
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

    /// Whether this machine runs in application mode, as the file said last.
    #[must_use]
    pub fn app_mode(&self) -> bool {
        self.current.app_mode()
    }

    /// How long a change is left to settle: [`APP_SETTLE`] in application
    /// mode, else [`SETTLE`].
    #[must_use]
    pub fn settle(&self) -> Duration {
        if self.app_mode() { APP_SETTLE } else { SETTLE }
    }

    /// The folder changed here at `now`: files to send out. The first change
    /// since the last round is the one that counts, so a folder that never
    /// stops changing still has its round.
    pub fn changed_here(&mut self, now: Instant) {
        self.changed.get_or_insert(now);
    }

    /// What to do now, given when the last round ended.
    ///
    /// A pause that just ended is due at once, like a "sync now". A change
    /// in the folder ([`Self::changed_here`]) is due [`SETTLE`] after it, and
    /// [`MIN_INTERVAL`] at least after the last round (the interval in force,
    /// if it is shorter).
    ///
    /// A "sync now" is consumed whether or not it is acted on: asked while
    /// paused, it is refused at the command line, and one written anyway must
    /// not wait in the file to fire the moment somebody resumes.
    #[cfg(test)]
    pub fn next(&mut self, now: Instant, last_round: Option<Instant>, base: Duration) -> Next {
        self.next_with(
            now,
            Rounds {
                any: last_round,
                full: last_round,
            },
            base,
            false,
        )
    }

    /// [`Self::next`], with light rounds: in application mode, and with
    /// `app_peers` known to dial, a change goes out in a light round rather
    /// than a full one, and a light round is due every [`APP_INTERVAL`]. The
    /// full round keeps its own interval, counted from the last full one.
    /// Without application mode, or with nobody to dial in a light round,
    /// this is [`Self::next`] exactly.
    pub fn next_with(
        &mut self,
        now: Instant,
        rounds: Rounds,
        base: Duration,
        app_peers: bool,
    ) -> Next {
        let last_round = rounds.any;
        let asked = self.current.sync_asked.is_some() && self.current.sync_asked != self.honoured;
        if asked {
            self.honoured = self.current.sync_asked;
        }
        let lifted = std::mem::take(&mut self.lifted);
        let early = self
            .changed
            .is_some_and(|since| now >= since + self.settle())
            && last_round.is_none_or(|last| now >= last + MIN_INTERVAL.min(self.interval(base)));
        let light = self.app_mode() && app_peers;
        let full_due = rounds
            .full
            .is_none_or(|last| now >= last + self.interval(base));
        let due = asked || lifted || full_due || (early && !light);
        let light_due =
            light && (early || last_round.is_none_or(|last| now >= last + APP_INTERVAL));
        match (due, light_due, self.paused()) {
            // Paused, nothing moves: not even to the machines that asked for
            // a minute.
            (false, false, _) | (false, true, true) => Next::Wait,
            (true, _, true) => Next::Publish,
            (true, _, false) => {
                self.changed = None;
                Next::Round
            }
            (false, true, false) => {
                self.changed = None;
                Next::Light
            }
        }
    }
}

/// When the last round of each kind ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rounds {
    /// The last round of either kind, light or full.
    pub any: Option<Instant>,
    /// The last full round.
    pub full: Option<Instant>,
}

fn clamp(seconds: Option<u64>) -> Option<Duration> {
    seconds.map(|seconds| Duration::from_secs(seconds).clamp(MIN_INTERVAL, MAX_INTERVAL))
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

    fn app_steering() -> Steering {
        steering(Control {
            app: Some(1),
            ..Control::default()
        })
    }

    #[test]
    fn red_team_application_mode_sends_a_change_to_its_peers_in_seconds_and_nobody_else() {
        // The program writes a file; it must be on the other machines in
        // application mode within seconds, not at the end of a five-minute
        // interval -- and without dialling every host of the network for it.
        let mut app = app_steering();
        let start = Instant::now();
        let full = Rounds {
            any: Some(start),
            full: Some(start),
        };
        let changed = start + MIN_INTERVAL;
        app.changed_here(changed);
        assert_eq!(
            app.next_with(changed + APP_SETTLE, full, BASE, true),
            Next::Light,
            "a change in application mode waited past its two-second settle, or dialled everybody"
        );
        // Without anybody to dial in a light round, it is the full round, as
        // before: the file must still leave this machine.
        let mut alone = app_steering();
        alone.changed_here(changed);
        assert_eq!(
            alone.next_with(changed + APP_SETTLE, full, BASE, false),
            Next::Round,
            "with no peer in application mode, a change waited for the interval"
        );
        // Off, the settle is the usual one.
        let mut off = steering(Control::default());
        off.changed_here(changed);
        assert_eq!(
            off.next_with(changed + APP_SETTLE, full, BASE, true),
            Next::Wait,
            "a machine not in application mode took the short settle"
        );
    }

    #[test]
    fn red_team_application_mode_dials_its_peers_every_minute_and_the_rest_at_the_interval() {
        let mut app = app_steering();
        let start = Instant::now();
        let mut rounds = Rounds {
            any: Some(start),
            full: Some(start),
        };
        let mut light = 0;
        let mut full = 0;
        for second in 1..=BASE.as_secs() {
            let now = start + Duration::from_secs(second);
            match app.next_with(now, rounds, BASE, true) {
                Next::Light => {
                    light += 1;
                    rounds.any = Some(now);
                }
                Next::Round => {
                    full += 1;
                    rounds = Rounds {
                        any: Some(now),
                        full: Some(now),
                    };
                }
                Next::Wait => {}
                Next::Publish => panic!("not paused"),
            }
        }
        assert_eq!(
            (light, full),
            (4, 1),
            "five minutes in application mode: a light round a minute and one full round"
        );
        // Paused, nothing moves, not even to the machines that asked.
        let mut paused = steering(Control {
            app: Some(1),
            paused_since: Some(1),
            ..Control::default()
        });
        let quiet = Rounds {
            any: Some(start),
            full: Some(start),
        };
        assert_eq!(
            paused.next_with(start + APP_INTERVAL, quiet, BASE, true),
            Next::Wait,
            "a paused machine in application mode dialled its peers"
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

    /// A file saved here goes out a few seconds later, not at the end of the
    /// interval (hand test, 2026-10-22: five minutes to reach a key).
    #[test]
    fn a_local_change_is_sent_after_a_short_settle_not_a_whole_interval() {
        let start = Instant::now();
        let mut steering = steering(Control::default());
        let changed = start + MIN_INTERVAL;
        steering.changed_here(changed);
        assert_eq!(steering.next(changed, Some(start), BASE), Next::Wait);
        assert_eq!(
            steering.next(changed + SETTLE, Some(start), BASE),
            Next::Round,
            "a new local file waited for the whole interval"
        );
        assert_eq!(
            steering.next(changed + SETTLE * 2, Some(changed + SETTLE), BASE),
            Next::Wait,
            "one change ran a second round"
        );
    }

    /// A folder rewritten every second -- a log, a sync tool fighting this
    /// one -- must not become a daemon dialling every machine every second.
    #[test]
    fn red_team_a_folder_that_never_stops_changing_costs_a_round_per_half_minute_at_most() {
        let start = Instant::now();
        let mut steering = steering(Control::default());
        let mut last_round = Some(start);
        let mut rounds = 0;
        for second in 1..=300 {
            let now = start + Duration::from_secs(second);
            steering.changed_here(now);
            if steering.next(now, last_round, BASE) == Next::Round {
                rounds += 1;
                last_round = Some(now);
            }
        }
        assert!(
            rounds <= 10,
            "{rounds} rounds in five minutes of a folder changing every second"
        );
        assert!(
            rounds >= 9,
            "{rounds} rounds: a folder that never settles was never sent out"
        );
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
        assert_eq!(
            lasting.refresh(Ok(forever), u64::MAX - 1),
            Vec::<String>::new()
        );
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
}
