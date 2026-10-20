//! Disk work handed to the running daemon: move the vault, write a cold copy,
//! read one back.
//!
//! The daemon holds the store and the vault for as long as it runs, and it
//! has the keys unlocked. So a terminal or the Settings page does not open
//! either -- which would need the daemon stopped and the passphrase typed
//! again, into a browser for the page -- it writes the request into the home,
//! the boundary of trust `control.rs` already relies on, and the daemon does
//! the work between two rounds:
//!
//! - an export or a restore at once, on the node it has open;
//! - a move once its run has ended and the node is closed (Windows will not
//!   copy a database another handle holds), then it starts again on the new
//!   vault, as after a disk that came back.
//!
//! Two files, each replaced whole through a temporary name and a rename:
//!
//! ```text
//! job          id 1759750000123 / kind export / path D:\backup   (asked)
//! job-result   id 1759750000123 / state running|done|failed / said ...
//! ```
//!
//! One job at a time: a second is refused while the first is waiting.

use std::{
    fmt::Write as _,
    io::ErrorKind,
    path::{Path, PathBuf},
};

/// The request, in the node's home.
pub const JOB: &str = "job";

/// What became of it, in the node's home.
pub const JOB_RESULT: &str = "job-result";

/// The most of either file read: three short lines and a path.
const MAX_JOB_BYTES: u64 = 8192;

/// What the daemon is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Copy the vault to the path, check it, switch to it.
    MoveVault {
        /// The new vault may share a disk with the store.
        same_disk: bool,
    },
    /// Write this account's cold copy into the path (`backup.rs`).
    Export,
    /// Read the cold copy at the path into this node.
    Restore,
}

impl Kind {
    const fn name(self) -> &'static str {
        match self {
            Self::MoveVault { same_disk: false } => "move-vault",
            Self::MoveVault { same_disk: true } => "move-vault-same-disk",
            Self::Export => "export",
            Self::Restore => "restore",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "move-vault" => Self::MoveVault { same_disk: false },
            "move-vault-same-disk" => Self::MoveVault { same_disk: true },
            "export" => Self::Export,
            "restore" => Self::Restore,
            _ => return None,
        })
    }
}

/// A request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub id: u64,
    pub kind: Kind,
    pub path: PathBuf,
}

/// How far a job went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    Done,
    Failed,
}

impl State {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

/// What `job-result` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub id: u64,
    pub state: State,
    pub said: String,
}

/// A path fit to travel in the request: absolute, and on one line. A line
/// break in it would let whoever typed it add lines of their own to the file.
///
/// # Errors
///
/// Why it is not.
pub fn check_path(path: &Path) -> Result<PathBuf, String> {
    let text = path
        .to_str()
        .ok_or_else(|| format!("{} is not valid UTF-8", path.display()))?;
    if text.trim().is_empty() {
        return Err("no directory given".to_owned());
    }
    if text.contains(['\n', '\r', '\0']) {
        return Err("a directory name holds a line break or a NUL".to_owned());
    }
    if !path.is_absolute() {
        return Err(format!(
            "{} is not a full path (start it with / or a drive letter)",
            path.display()
        ));
    }
    Ok(path.to_owned())
}

fn replace(home: &Path, name: &str, text: &str) -> std::io::Result<()> {
    let path = home.join(name);
    let staging = home.join(format!("{name}.tmp"));
    std::fs::write(&staging, text)?;
    std::fs::rename(&staging, &path)
}

fn read_capped(path: &Path) -> std::io::Result<Option<String>> {
    use std::io::Read as _;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut text = String::new();
    file.take(MAX_JOB_BYTES).read_to_string(&mut text)?;
    Ok(Some(text))
}

fn fields(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(key, value)| (key.trim(), value))
}

/// Ask the daemon for `kind` on `path`; the id to wait for.
///
/// # Errors
///
/// A path [`check_path`] refuses, a job already waiting, or the disk.
pub fn ask(home: &Path, kind: Kind, path: &Path) -> Result<u64, String> {
    let path = check_path(path)?;
    if home.join(JOB).exists() {
        return Err(
            "the daemon has not picked up the last disk job yet; try again in a few \
                    seconds"
                .to_owned(),
        );
    }
    if let Some(report) = result(home).filter(|report| report.state == State::Running) {
        return Err(format!(
            "the daemon is still busy with a disk job ({})",
            report.said
        ));
    }
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    let mut text = String::new();
    let _ = writeln!(text, "id {id}");
    let _ = writeln!(text, "kind {}", kind.name());
    let _ = writeln!(text, "path {}", path.display());
    replace(home, JOB, &text).map_err(|error| format!("could not write the job: {error}"))?;
    Ok(id)
}

/// The daemon's side: the waiting request, removed so it runs once. `None`
/// when there is none; `Some(Err)` for a file that is no job, which is
/// removed too, so it is not met again every two seconds.
pub fn take(home: &Path) -> Option<Result<Job, String>> {
    let path = home.join(JOB);
    // Claimed by a rename before it is read: a job asked between a read and
    // a removal would otherwise be removed unread.
    let claimed = home.join(format!("{JOB}.taken"));
    match std::fs::rename(&path, &claimed) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return None,
        Err(error) => return Some(Err(format!("could not take {}: {error}", path.display()))),
    }
    let text = read_capped(&claimed);
    let _ = std::fs::remove_file(&claimed);
    let text = match text {
        Ok(Some(text)) => text,
        Ok(None) => return None,
        Err(error) => return Some(Err(format!("could not read {}: {error}", path.display()))),
    };
    let (mut id, mut kind, mut target) = (None, None, None);
    for (key, value) in fields(&text) {
        match key {
            "id" => id = value.trim().parse().ok(),
            "kind" => kind = Kind::parse(value.trim()),
            "path" => target = Some(PathBuf::from(value)),
            _ => {}
        }
    }
    let job = (|| {
        let path = check_path(&target?).ok()?;
        Some(Job {
            id: id?,
            kind: kind?,
            path,
        })
    })();
    Some(job.ok_or_else(|| format!("{} is no disk job; ignored", path.display())))
}

/// At a daemon's start: a job its last process left running will never
/// finish, and would refuse every later one as busy.
pub fn clear_stale(home: &Path) {
    if let Some(report) = result(home).filter(|report| report.state == State::Running) {
        report_line(
            home,
            report.id,
            State::Failed,
            &format!(
                "the service stopped during it ({}); nothing was switched unless it says so, \
                 try again",
                report.said
            ),
        );
    }
}

/// Say how a job went.
pub fn report(home: &Path, id: u64, state: State, said: &str) {
    report_line(home, id, state, said);
}

fn report_line(home: &Path, id: u64, state: State, said: &str) {
    let mut text = String::new();
    let _ = writeln!(text, "id {id}");
    let _ = writeln!(text, "state {}", state.name());
    // One line: the reader takes the rest of it.
    let _ = writeln!(text, "said {}", said.replace(['\n', '\r'], " "));
    if let Err(error) = replace(home, JOB_RESULT, &text) {
        eprintln!("itsanas: could not write the disk job's result: {error}");
    }
}

/// The last job's result, if any.
pub fn result(home: &Path) -> Option<Report> {
    let text = read_capped(&home.join(JOB_RESULT)).ok()??;
    let (mut id, mut state, mut said) = (None, None, String::new());
    for (key, value) in fields(&text) {
        match key {
            "id" => id = value.trim().parse().ok(),
            "state" => {
                state = match value.trim() {
                    "running" => Some(State::Running),
                    "done" => Some(State::Done),
                    "failed" => Some(State::Failed),
                    _ => None,
                }
            }
            "said" => value.clone_into(&mut said),
            _ => {}
        }
    }
    Some(Report {
        id: id?,
        state: state?,
        said,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn absolute(dir: &Path, name: &str) -> PathBuf {
        std::path::absolute(dir.join(name)).expect("absolute")
    }

    #[test]
    fn a_job_asked_is_taken_once_and_its_result_read_back() {
        let home = tempfile::tempdir().expect("home");
        let target = absolute(home.path(), "backup dir");
        let id = ask(home.path(), Kind::Export, &target).expect("asked");
        assert!(
            ask(home.path(), Kind::Restore, &target).is_err(),
            "a second job while one waits"
        );
        let job = take(home.path()).expect("one").expect("a job");
        assert_eq!(
            job,
            Job {
                id,
                kind: Kind::Export,
                path: target.clone()
            }
        );
        assert!(take(home.path()).is_none(), "taken twice");

        report(home.path(), id, State::Running, "exporting");
        assert!(
            ask(home.path(), Kind::Export, &target).is_err(),
            "a second job while one runs"
        );
        report(home.path(), id, State::Done, "line one\nline two");
        let read = result(home.path()).expect("a result");
        assert_eq!((read.id, read.state), (id, State::Done));
        assert_eq!(read.said, "line one line two");
        for kind in [
            Kind::MoveVault { same_disk: false },
            Kind::MoveVault { same_disk: true },
            Kind::Restore,
        ] {
            ask(home.path(), kind, &target).expect("asked");
            assert_eq!(take(home.path()).expect("one").expect("job").kind, kind);
        }
    }

    #[test]
    fn a_job_left_running_by_a_dead_daemon_does_not_block_the_next() {
        let home = tempfile::tempdir().expect("home");
        report(home.path(), 7, State::Running, "moving the vault");
        let target = absolute(home.path(), "elsewhere");
        assert!(ask(home.path(), Kind::Export, &target).is_err());
        clear_stale(home.path());
        assert_eq!(result(home.path()).expect("result").state, State::Failed);
        ask(home.path(), Kind::Export, &target).expect("asked after the clear");
    }

    #[test]
    fn red_team_a_path_cannot_smuggle_lines_into_the_job() {
        let home = tempfile::tempdir().expect("home");
        let sneaky = absolute(home.path(), "x\nkind restore");
        assert!(ask(home.path(), Kind::Export, &sneaky).is_err());
        assert!(!home.path().join(JOB).exists(), "nothing was written");
        assert!(ask(home.path(), Kind::Export, Path::new("relative/dir")).is_err());

        // A file written by hand with the same trick is refused by the daemon.
        std::fs::write(home.path().join(JOB), "id 1\nkind export\npath relative\n").expect("write");
        assert!(take(home.path()).expect("one").is_err());
        assert!(
            !home.path().join(JOB).exists(),
            "a bad job is not met again"
        );
    }
}
