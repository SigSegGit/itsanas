//! What a file saved, deleted or renamed in the folder does to the account.
//!
//! # The answer
//!
//! The folder is the account, so it does what any synced folder does here: a
//! saved file is a new version on every machine, a deleted one is deleted on
//! every machine. What makes a drive different is that Windows reports each act
//! as it happens, half-written files and editors' scratch files included. So:
//!
//! - **Nothing is read at once.** A change waits until [`SETTLE_SECS`] have gone
//!   by with nothing further on that path ([`Settling`]).
//! - **Scratch files stay out** ([`may_keep`]): Office's `~$` locks, `.tmp` and
//!   half a download would otherwise reach every machine.
//! - **A wave of deletions is refused** ([`Deletions`]), the rule
//!   `itsanas-folder` applies to a synced folder. Something deleting most of the
//!   account through this folder is more often a program than a person, and
//!   here one act reaches every copy. The account keeps the files.
//!
//! Recycling a file is a rename to outside the folder, and Windows reports it
//! so: it is a deletion. Restoring it is a file moved in.
//!
//! All of this is here and not in `projfs.rs` because it can be tested on any
//! system, with no driver.

use itsanas_folder::DELETIONS_ALWAYS_ALLOWED;
use std::io;

/// How long a path stays quiet before its change is read.
pub const SETTLE_SECS: u64 = 2;

/// How far back [`Deletions`] counts: a slow wave is still a wave.
const DELETION_WINDOW_SECS: u64 = 3600;

/// What Windows reported, in this crate's words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Created,
    Modified,
    Deleted,
    Renamed,
}

/// What a report means for the account. Paths are logical (`crate::logical`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Change {
    Written(String),
    Deleted(String),
    Renamed { from: String, to: String },
}

/// The change a report stands for, if any. An account stores paths and not a
/// tree, so a directory created or closed means nothing until a file is in it.
#[must_use]
pub fn change_of(event: Event, path: &str, destination: &str, directory: bool) -> Option<Change> {
    let path = crate::logical(path);
    let destination = crate::logical(destination);
    match event {
        Event::Created | Event::Modified if directory || path.is_empty() => None,
        Event::Created | Event::Modified => Some(Change::Written(path)),
        Event::Deleted if path.is_empty() => None,
        Event::Deleted => Some(Change::Deleted(path)),
        Event::Renamed => match (path.is_empty(), destination.is_empty()) {
            (true, true) => None,
            (true, false) => Some(Change::Written(destination)),
            (false, true) => Some(Change::Deleted(path)),
            (false, false) => Some(Change::Renamed {
                from: path,
                to: destination,
            }),
        },
    }
}

/// Whether a file with this path belongs in the account at all.
#[must_use]
pub fn may_keep(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    !(name.is_empty()
        || name.starts_with("~$")
        || name.starts_with(".~lock")
        || name
            .rsplit_once('.')
            .is_some_and(|(_, last)| matches!(last, "tmp" | "crdownload" | "partial"))
        || name == "desktop.ini"
        || name == "thumbs.db")
}

/// Changes waiting for their path to go quiet, in the order they happened.
#[derive(Debug, Default)]
pub struct Settling {
    waiting: Vec<(Change, u64)>,
}

impl Settling {
    /// Add a change. A file written again waits again, and a file deleted needs
    /// no write that came before it read.
    pub fn push(&mut self, change: Change, now: u64) {
        let superseded = match &change {
            Change::Written(path) | Change::Deleted(path) => Some(path.clone()),
            Change::Renamed { .. } => None,
        };
        if let Some(path) = superseded {
            self.waiting
                .retain(|(before, _)| !matches!(before, Change::Written(p) if *p == path));
        }
        self.waiting.push((change, now));
    }

    /// The changes whose time has come. Only from the front: a rename applied
    /// ahead of the write before it would move the wrong bytes.
    pub fn due(&mut self, now: u64) -> Vec<Change> {
        let ready = self
            .waiting
            .iter()
            .take_while(|(_, at)| now.saturating_sub(*at) >= SETTLE_SECS)
            .count();
        self.waiting
            .drain(..ready)
            .map(|(change, _)| change)
            .collect()
    }
}

/// The deletions of the last hour, to tell a person from a wave.
#[derive(Debug, Default)]
pub struct Deletions {
    recent: Vec<u64>,
}

impl Deletions {
    /// Whether `count` more deletions may go ahead on an account that holds
    /// `known` files now, and note them if so. The rule of `itsanas-folder`:
    /// a few always may, more than half of what was there may not.
    pub fn allow(&mut self, count: usize, known: usize, now: u64) -> bool {
        // A save deletes nothing, and a refused wave must not stop saves.
        if count == 0 {
            return true;
        }
        self.recent
            .retain(|at| now.saturating_sub(*at) < DELETION_WINDOW_SECS);
        let total = self.recent.len() + count;
        let held = known + self.recent.len();
        if total > DELETIONS_ALWAYS_ALLOWED && total * 2 > held {
            return false;
        }
        self.recent.extend(std::iter::repeat_n(now, count));
        true
    }
}

/// What applying a change needs from the account.
pub trait Target {
    /// Every path the account has.
    ///
    /// # Errors
    ///
    /// When the account cannot be read.
    fn known(&self) -> io::Result<Vec<String>>;
    /// Read this path from the disk into the account, a directory's files too.
    ///
    /// # Errors
    ///
    /// When it cannot be read or the account will not take it.
    fn keep(&self, path: &str) -> io::Result<()>;
    /// Delete this file from the account.
    ///
    /// # Errors
    ///
    /// When the account cannot be written.
    fn remove(&self, path: &str) -> io::Result<()>;
    /// Copy a file already in the account to a new path. Not from the disk: a
    /// file renamed before it was ever opened has no bytes there.
    ///
    /// # Errors
    ///
    /// When its content cannot be had or the account will not take it.
    fn copy(&self, from: &str, to: &str) -> io::Result<()>;
}

/// Whether trying again can help. A file an editor holds open refuses a read for
/// a moment; a file the account no longer has will not be deleted by asking
/// five times, and the copy a rename made must not be made five times.
#[must_use]
pub fn worth_retrying(error: &io::Error) -> bool {
    error.kind() != io::ErrorKind::Unsupported
}

/// Whether `candidate` is `path` or under it.
fn is_or_under(candidate: &str, path: &str) -> bool {
    candidate
        .strip_prefix(path)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// How many of the account's files a change would delete.
#[must_use]
pub fn deleted_by(change: &Change, known: &[String]) -> usize {
    match change {
        Change::Written(_) => 0,
        Change::Deleted(path) => known.iter().filter(|k| is_or_under(k, path)).count(),
        Change::Renamed { from, .. } => known.iter().filter(|k| is_or_under(k, from)).count(),
    }
}

/// Apply one change to the account.
///
/// # Errors
///
/// Whatever the account or the disk answered; what was done before it stays done.
pub fn apply(change: &Change, known: &[String], target: &impl Target) -> io::Result<()> {
    match change {
        Change::Written(path) => {
            if may_keep(path) {
                target.keep(path)?;
            }
        }
        Change::Deleted(path) => {
            for file in known.iter().filter(|k| is_or_under(k, path)) {
                target.remove(file)?;
            }
        }
        Change::Renamed { from, to } => {
            let moved: Vec<&String> = known.iter().filter(|k| is_or_under(k, from)).collect();
            // Not in the account: an editor's scratch file becoming the real
            // one, which is how Office saves. The bytes are on the disk.
            if moved.is_empty() && may_keep(to) {
                target.keep(to)?;
            }
            for file in moved {
                let new = format!("{to}{}", &file[from.len()..]);
                // Copied before it is removed, so a failure loses nothing.
                if may_keep(&new) {
                    target.copy(file, &new)?;
                }
                target.remove(file)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Change, Deletions, Event, SETTLE_SECS, Settling, Target, apply, change_of, deleted_by,
        may_keep,
    };
    use std::{cell::RefCell, io};

    fn written(path: &str) -> Change {
        Change::Written(path.to_owned())
    }

    #[derive(Default)]
    struct Account {
        log: RefCell<Vec<String>>,
        refuse_copies: bool,
    }

    impl Target for Account {
        fn known(&self) -> io::Result<Vec<String>> {
            Ok(Vec::new())
        }
        fn keep(&self, path: &str) -> io::Result<()> {
            self.log.borrow_mut().push(format!("keep {path}"));
            Ok(())
        }
        fn remove(&self, path: &str) -> io::Result<()> {
            self.log.borrow_mut().push(format!("remove {path}"));
            Ok(())
        }
        fn copy(&self, from: &str, to: &str) -> io::Result<()> {
            if self.refuse_copies {
                return Err(io::Error::other("no machine is up"));
            }
            self.log.borrow_mut().push(format!("copy {from} {to}"));
            Ok(())
        }
    }

    fn known(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| (*p).to_owned()).collect()
    }

    /// Sabotage: read an empty destination as a real rename.
    #[test]
    fn red_team_recycling_a_file_is_a_deletion_and_restoring_it_a_write() {
        assert_eq!(
            change_of(Event::Renamed, "a\\b.txt", "", false),
            Some(Change::Deleted("a/b.txt".to_owned())),
            "a file sent to the recycle bin stays in the account, on every machine"
        );
        assert_eq!(
            change_of(Event::Renamed, "", "a\\b.txt", false),
            Some(written("a/b.txt")),
            "a file restored from the recycle bin never comes back to the account"
        );
        assert_eq!(
            change_of(Event::Renamed, "a.txt", "b\\a.txt", false),
            Some(Change::Renamed {
                from: "a.txt".to_owned(),
                to: "b/a.txt".to_owned()
            })
        );
    }

    #[test]
    fn a_directory_alone_is_nothing_to_keep() {
        assert_eq!(change_of(Event::Created, "photos", "", true), None);
        assert_eq!(change_of(Event::Modified, "photos", "", true), None);
        assert_eq!(
            change_of(Event::Modified, "photos\\a.jpg", "", false),
            Some(written("photos/a.jpg"))
        );
    }

    /// Sabotage: let them all in.
    #[test]
    fn red_team_an_editors_scratch_files_never_reach_the_account() {
        for scratch in [
            "docs/~$report.docx",
            "docs/~WRL0001.tmp",
            "docs/.~lock.report.odt#",
            "down/film.mkv.crdownload",
            "down/film.mkv.partial",
            "Photos/Thumbs.db",
            "Photos/desktop.ini",
        ] {
            assert!(
                !may_keep(scratch),
                "{scratch} would reach every machine, and vanish again a moment later"
            );
        }
        for real in ["docs/report.docx", "tmp/notes.txt", "a.tmp.txt"] {
            assert!(may_keep(real), "{real} would silently never be kept");
        }
    }

    /// Sabotage: read at once, or do not move a rewritten file to the back.
    #[test]
    fn red_team_a_file_still_being_written_is_not_read() {
        let mut settling = Settling::default();
        settling.push(written("a"), 100);
        assert_eq!(
            settling.due(100),
            Vec::new(),
            "a file read the moment it changed: half of it reaches every machine"
        );
        settling.push(written("a"), 100 + SETTLE_SECS);
        assert_eq!(
            settling.due(100 + SETTLE_SECS),
            Vec::new(),
            "a file written again was read before it went quiet"
        );
        assert_eq!(settling.due(100 + 2 * SETTLE_SECS), vec![written("a")]);
        assert_eq!(settling.due(1000), Vec::new(), "the same change came twice");
    }

    /// A write then a rename must reach the account in that order. Sabotage:
    /// take everything old enough, wherever it is in the line.
    #[test]
    fn red_team_a_change_never_overtakes_the_one_before_it() {
        let mut settling = Settling::default();
        settling.push(written("a"), 100);
        let rename = Change::Renamed {
            from: "b".to_owned(),
            to: "c".to_owned(),
        };
        settling.push(rename.clone(), 100);
        settling.push(written("d"), 105);
        assert_eq!(settling.due(102), vec![written("a"), rename]);
        assert_eq!(settling.due(106), Vec::new());

        // The clock stepped back: the second is old enough and the first is not.
        settling.due(1000);
        settling.push(written("first"), 2000);
        settling.push(written("second"), 1000);
        assert_eq!(
            settling.due(1500),
            Vec::new(),
            "a later change was applied ahead of an earlier one: the account ends on the \
             wrong bytes"
        );
    }

    /// A deletion that follows a write: the write is gone, or a read of a file
    /// no longer there is retried for nothing. Sabotage: keep the write.
    #[test]
    fn a_deleted_file_needs_no_read() {
        let mut settling = Settling::default();
        settling.push(written("a"), 100);
        settling.push(Change::Deleted("a".to_owned()), 100);
        assert_eq!(settling.due(200), vec![Change::Deleted("a".to_owned())]);
    }

    /// Sabotage: forget earlier deletions, or never refuse.
    #[test]
    fn red_team_a_wave_of_deletions_is_refused_however_slowly_it_comes() {
        let mut deletions = Deletions::default();
        assert!(
            deletions.allow(1, 100, 1000),
            "one file could not be deleted"
        );
        let mut deletions = Deletions::default();
        assert!(
            !deletions.allow(60, 100, 1000),
            "most of the account deleted at once, on every machine: a program does that"
        );
        let mut deletions = Deletions::default();
        let mut known = 20;
        let mut refused = false;
        for second in 0..20 {
            if deletions.allow(1, known, 1000 + second) {
                known -= 1;
            } else {
                refused = true;
                break;
            }
        }
        assert!(
            refused,
            "the same wave one file at a time deleted the account: a limit per batch \
             stops nothing"
        );
        assert!(
            // Few files left, so the count alone would read as a wave.
            deletions.allow(0, 1, 1000 + 20),
            "during a refused wave a saved file was refused too: it never reaches the account"
        );
        assert!(
            deletions.allow(1, known, 1000 + 2 * 3600),
            "a refused wave blocks deletions for ever: nothing the person does clears it"
        );
    }

    /// Sabotage: retry everything.
    #[test]
    fn red_team_a_deletion_that_cannot_happen_is_said_once_and_not_retried() {
        assert!(
            !super::worth_retrying(&io::Error::from(io::ErrorKind::Unsupported)),
            "a deletion the store cannot do was tried again: its renamed copy is written five times"
        );
        assert!(
            super::worth_retrying(&io::Error::from(io::ErrorKind::PermissionDenied)),
            "a file an editor still held open was given up on at once: the save never arrives"
        );
    }

    /// Sabotage: remove before copying.
    #[test]
    fn red_team_a_rename_that_cannot_copy_deletes_nothing() {
        let account = Account {
            refuse_copies: true,
            ..Account::default()
        };
        let change = Change::Renamed {
            from: "a.txt".to_owned(),
            to: "b.txt".to_owned(),
        };
        assert!(apply(&change, &known(&["a.txt"]), &account).is_err());
        assert_eq!(
            account.log.borrow().len(),
            0,
            "a file renamed with no machine up to fetch it from is gone from the account"
        );
    }

    /// How Office saves: new bytes in a scratch file, the old one renamed away,
    /// the scratch file renamed to the real name. Sabotage: drop the disk read.
    #[test]
    fn red_team_a_save_by_renaming_a_scratch_file_keeps_the_new_bytes() {
        let account = Account::default();
        let change = Change::Renamed {
            from: "~WRD0001.tmp".to_owned(),
            to: "report.docx".to_owned(),
        };
        apply(&change, &known(&["report.docx"]), &account).unwrap();
        assert_eq!(
            account.log.borrow().as_slice(),
            ["keep report.docx"],
            "a document saved from Word never reaches the account"
        );
    }

    #[test]
    fn a_folder_renamed_or_deleted_takes_its_files_and_no_neighbours() {
        let files = known(&["a/1", "a/sub/2", "ab/3"]);
        let account = Account::default();
        let rename = Change::Renamed {
            from: "a".to_owned(),
            to: "z".to_owned(),
        };
        apply(&rename, &files, &account).unwrap();
        assert_eq!(
            account.log.borrow().as_slice(),
            [
                "copy a/1 z/1",
                "remove a/1",
                "copy a/sub/2 z/sub/2",
                "remove a/sub/2"
            ]
        );
        let delete = Change::Deleted("a".to_owned());
        assert_eq!(deleted_by(&delete, &files), 2, "`ab/3` is not under `a`");
    }
}
