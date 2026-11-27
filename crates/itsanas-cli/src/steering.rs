//! Steering the running daemon through its control file: `pause`, `resume`,
//! `sync-now`, `interval` and `app`.

use std::path::Path;

use crate::cli::Command;
use crate::control;
use crate::{
    error::{CliError, Result},
    node::Node,
};

/// `itsanas pause [--for D]` applied to what the control file says.
///
/// A pause already holding keeps its start, and takes the new end: "for 8
/// hours" over "for 1 hour" extends it, plain `pause` over a timed one makes
/// it last until resumed. Only the same open-ended pause twice is a no-op.
pub(crate) fn pause(
    control: &mut control::Control,
    duration: Option<&str>,
    now: u64,
) -> Result<String> {
    let until = duration
        .map(control::parse_pause)
        .transpose()
        .map_err(CliError::Usage)?
        .map(|seconds| now.saturating_add(seconds));
    let holding = control.pause_holds(now);
    if holding && control.until.is_none() && until.is_none() {
        return Ok("Syncing was already paused.".to_owned());
    }
    if !holding {
        control.paused_since = Some(now);
    }
    control.until = until;
    Ok(format!(
        "Syncing paused on this machine {}. Nothing is lost: your changes here wait until \
         syncing resumes, and this machine keeps hosting for the others. A transfer under way \
         finishes with the machine it is talking to (five minutes at most); no other \
         starts.\nResume sooner with: itsanas resume",
        control.lasts(now)
    ))
}

/// Write what was asked into the control file the daemon reads (`control.rs`).
///
/// Opens nothing and asks no passphrase: these are the commands a tray runs
/// while the daemon holds the store, which is every moment they are useful.
pub(crate) fn steer(home: &Path, command: &Command) -> Result<()> {
    println!("{}", steer_said(home, command)?);
    Ok(())
}

/// [`steer`], returning what it would print: the Settings page of
/// `setup::web` says the same words as the terminal.
pub(crate) fn steer_said(home: &Path, command: &Command) -> Result<String> {
    if !Node::exists(home) {
        return Err(CliError::Usage(format!(
            "no node in {}; set one up first",
            home.display()
        )));
    }
    let running = itsanas_store::Store::is_locked(Node::store_path(home));
    let now = itsanas_discover::now_unix();
    // An unreadable file is rewritten only by `pause` and `resume`, which say
    // outright what it should hold. Anything else would start from the
    // default -- "not paused" -- and erase a pause it could not read
    // (#243's review: `interval 10m` resumed a paused node that way).
    let (mut control, rewritten) = match control::Control::read(home) {
        Ok(control) => (control, false),
        Err(_) if matches!(command, Command::Pause { .. } | Command::Resume) => {
            (control::Control::default(), true)
        }
        Err(why) => {
            return Err(CliError::Usage(format!(
                "{why}\nNothing else is changed until it is rewritten, so a pause it may hold is kept."
            )));
        }
    };
    let said = match command {
        Command::Pause { duration } => pause(&mut control, duration.as_deref(), now)?,
        Command::Resume if !control.pause_holds(now) && !rewritten => {
            // A pause over by the clock leaves its lines behind; clear them.
            control.paused_since = None;
            control.until = None;
            "Syncing was not paused.".to_owned()
        }
        Command::Resume => {
            control.paused_since = None;
            control.until = None;
            // Resuming is a request to catch up, not to wait for the interval.
            control.sync_asked = Some(now);
            "Syncing resumed; a round starts within two seconds.".to_owned()
        }
        Command::SyncNow if control.pause_holds(now) => {
            return Err(CliError::Usage(
                "syncing is paused on this machine; `itsanas resume` resumes and syncs at once"
                    .to_owned(),
            ));
        }
        Command::SyncNow if !running => {
            return Err(CliError::Usage(
                "no daemon is running this node; start it, and it syncs as it starts".to_owned(),
            ));
        }
        Command::SyncNow => {
            // A second click in the same second is the same request.
            control.sync_asked = Some(now);
            "Asked; the round starts within two seconds.".to_owned()
        }
        Command::Interval { every: None } => {
            return Ok(match control.interval {
                Some(every) => format!(
                    "every {} (set with `itsanas interval`; `itsanas interval auto` undoes it)",
                    control::describe_every(every)
                ),
                None => {
                    "auto: the daemon's own setting (`--interval`, else the sync policy)".to_owned()
                }
            });
        }
        Command::Interval { every: Some(every) } => {
            let every = control::parse_every(every).map_err(CliError::Usage)?;
            control.interval = every;
            match every {
                Some(every) => {
                    format!("The daemon syncs every {}.", control::describe_every(every))
                }
                None => "The daemon syncs on its own setting again.".to_owned(),
            }
        }
        Command::App { state } => app_mode(&mut control, state.as_deref())?,
        _ => unreachable!("only pause, resume, sync-now, interval and app are dispatched here"),
    };
    control.write(home).map_err(|source| CliError::Io {
        path: home.join(control::CONTROL),
        source,
    })?;
    let mut said = said;
    if rewritten {
        said.push_str(
            "\n(The control file could not be read, so it was written afresh: an interval set \
             before is back to auto.)",
        );
    }
    if !running && !matches!(command, Command::SyncNow) {
        said.push_str("\n(No daemon is running this node now; it reads this when it starts.)");
    }
    Ok(said)
}

/// `itsanas app [on|off]`: what it says back, with the control file changed
/// as asked.
pub(crate) fn app_mode(control: &mut control::Control, state: Option<&str>) -> Result<String> {
    match state {
        None if control.app_mode() => Ok(concat!(
            "on: changes go out two seconds after they are written, and the other ",
            "machines in application mode are dialled first and every minute"
        )
        .to_owned()),
        None => Ok("off (`itsanas app on` turns it on)".to_owned()),
        Some("on") => {
            control.app = Some(1);
            Ok(concat!(
                "Application mode is on. The other machines in application mode ",
                "are found at the next round; `status.snapshot` says `newest`."
            )
            .to_owned())
        }
        Some("off") => {
            control.app = None;
            Ok("Application mode is off.".to_owned())
        }
        Some(other) => Err(CliError::Usage(format!(
            "`itsanas app {other}`: say `on` or `off`"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::steer;
    use crate::cli::Command;
    use crate::control;
    use crate::node::SNAPSHOT;
    use crate::status::brief_status;
    use crate::test_fixtures::fake_node;

    /// A paused node says so; a hung one is stale whatever it was asked, or a
    /// pause would hide a dead daemon behind a calm icon.
    #[test]
    fn a_paused_node_says_paused_and_a_hung_one_still_says_stale() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        fake_node(&home, "alice", None);
        let now = 1_000_000;
        let write = |taken: u64| {
            std::fs::write(
                home.join(SNAPSHOT),
                format!("snapshot {taken} every 300\n  files 3\n"),
            )
            .expect("snapshot");
        };
        steer(&home, &Command::Pause { duration: None }).expect("pause");
        write(now - 60);
        assert_eq!(brief_status(&home, true, now), "paused 60");
        write(now - 3 * 300);
        assert_eq!(brief_status(&home, true, now), "stale 900");
        steer(&home, &Command::Resume).expect("resume");
        write(now - 60);
        assert_eq!(brief_status(&home, true, now), "healthy 60");
        let resumed = control::Control::read(&home).expect("control");
        assert!(
            resumed.sync_asked.is_some(),
            "resuming did not ask for a round: the node waits a full interval to catch up"
        );
        assert!(
            steer(&home, &Command::SyncNow).is_err(),
            "sync-now with no daemon running said it was asked"
        );
    }

    /// A setting written over a control file that cannot be read would start
    /// from "not paused" and erase the pause it holds: `interval 10m` from a
    /// tray resuming a node paused on a metered link.
    #[test]
    fn red_team_a_setting_never_erases_a_pause_it_cannot_read() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        fake_node(&home, "alice", None);
        let garbled = "paused 1\ninterval ten minutes\n";
        std::fs::write(home.join(control::CONTROL), garbled).expect("control");

        let refused = steer(
            &home,
            &Command::Interval {
                every: Some("10m".to_owned()),
            },
        );
        assert!(
            refused.is_err(),
            "a setting was written over an unreadable control file"
        );
        assert!(steer(&home, &Command::SyncNow).is_err());
        assert_eq!(
            std::fs::read_to_string(home.join(control::CONTROL)).expect("control"),
            garbled,
            "the file holding the pause was rewritten"
        );
        let now = 1_000_000;
        std::fs::write(
            home.join(SNAPSHOT),
            format!("snapshot {} every 300\n", now - 60),
        )
        .expect("snapshot");
        assert_eq!(
            brief_status(&home, true, now),
            "unknown 60",
            "a node whose control file cannot be read showed healthy"
        );

        // `resume` says outright what the file should hold, so it repairs it.
        steer(&home, &Command::Resume).expect("resume rewrites it");
        assert!(control::Control::read(&home).is_ok_and(|read| read.paused_since.is_none()));
    }

    /// The tray's "pause for 1 hour": the icon says paused for the hour and
    /// healthy after it, by the clock alone -- a blue icon over a node that
    /// syncs again would send somebody to resume what is not paused, and a
    /// blue icon forever is the forgotten pause the end exists to prevent.
    #[test]
    fn a_timed_pause_shows_paused_only_while_it_holds() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join(".itsanas");
        fake_node(&home, "alice", None);
        steer(
            &home,
            &Command::Pause {
                duration: Some("1h".to_owned()),
            },
        )
        .expect("pause for an hour");
        let asked = control::Control::read(&home).expect("control");
        let (since, until) = (
            asked.paused_since.expect("paused"),
            asked.until.expect("an end"),
        );
        assert_eq!(until - since, 3600, "`--for 1h` did not end an hour later");
        std::fs::write(
            home.join(SNAPSHOT),
            format!("snapshot {} every 300\n", until - 60),
        )
        .expect("snapshot");
        assert_eq!(brief_status(&home, true, until - 1), "paused 59");
        assert_eq!(
            brief_status(&home, true, until),
            "healthy 60",
            "the icon still said paused once the hour was over"
        );
        assert!(
            steer(
                &home,
                &Command::Pause {
                    duration: Some("31d".to_owned())
                }
            )
            .is_err(),
            "a pause of a month and more was taken"
        );
        // Plain `pause` over a timed one lasts until resumed; `resume` clears both.
        steer(&home, &Command::Pause { duration: None }).expect("pause");
        let asked = control::Control::read(&home).expect("control");
        assert_eq!((asked.paused_since, asked.until), (Some(since), None));
        steer(&home, &Command::Resume).expect("resume");
        let asked = control::Control::read(&home).expect("control");
        assert_eq!((asked.paused_since, asked.until), (None, None));
    }
}
