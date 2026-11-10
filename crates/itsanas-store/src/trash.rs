//! The trash: what a person deleted on this machine, and when it is really deleted.
//!
//! # The same on every machine
//!
//! A delete made here, from a phone's screen, a synced folder, the virtual drive
//! or `itsanas rm`, does not write a tombstone. The file stays a live file of the
//! account: same path, same content, same copies, counted in the same space. Only
//! this machine hides it, and only the delay running out, or the person, sends
//! the real delete. So the log and the other machines learn nothing until then,
//! and losing this record loses no file: the failure costs room, never data.
//!
//! # A file next to the store, not a table in it
//!
//! A phone's screen reads the trash while a round holds the store's database,
//! which a second open would refuse. So the record is a small text file in the
//! store's directory, rewritten whole and renamed into place.

use std::collections::BTreeMap;
use std::path::Path;

use crate::{Store, error::StoreError, vault::Vault};

/// One line per trashed file, `due_unix<TAB>path`.
const RECORD: &str = "trash";

/// How many days a file waits, as a decimal number, when a person set it. Zero
/// turns the trash off: a delete is written at once, as before it existed.
const DAYS: &str = "trash-days";

/// The wait when nobody set one.
pub const DEFAULT_DAYS: u64 = 30;

/// The longest wait a person may set: a little over 27 years, far past any use,
/// and short enough that `days * DAY` never overflows.
pub const MAX_DAYS: u64 = 9_999;

/// Seconds in a day.
pub const DAY: u64 = 24 * 60 * 60;

/// Trashed paths, each with when it is really deleted.
#[must_use]
pub fn list(dir: &Path) -> BTreeMap<String, u64> {
    let text = std::fs::read(dir.join(RECORD)).unwrap_or_default();
    String::from_utf8_lossy(&text)
        .lines()
        .filter_map(|line| {
            let (due, path) = line.split_once('\t')?;
            Some((path.to_owned(), due.parse().ok()?))
        })
        .collect()
}

fn write_all(dir: &Path, name: &str, text: &str) -> std::io::Result<()> {
    // Renamed into place: a torn file would hide or show the wrong files.
    std::fs::create_dir_all(dir)?;
    let temporary = dir.join(format!("{name}.new"));
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, dir.join(name))
}

fn save(dir: &Path, trashed: &BTreeMap<String, u64>) -> std::io::Result<()> {
    let mut text = String::new();
    for (path, due) in trashed {
        text.push_str(&due.to_string());
        text.push('\t');
        text.push_str(path);
        text.push('\n');
    }
    write_all(dir, RECORD, &text)
}

/// Put `path` in the trash for `days` days from `now`.
///
/// # Errors
///
/// When the record cannot be written, or `path` holds a line break, which
/// would make one line say two things. The file then stays where it was.
pub fn put(dir: &Path, path: &str, now: u64, days: u64) -> std::io::Result<()> {
    if path.contains(['\n', '\r', '\t']) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a path in the trash holds no tab or line break",
        ));
    }
    let mut trashed = list(dir);
    trashed.insert(
        path.to_owned(),
        now.saturating_add(days.min(MAX_DAYS).saturating_mul(DAY)),
    );
    save(dir, &trashed)
}

/// Take `path` out of the trash: it is a file like any other again.
///
/// # Errors
///
/// When the record cannot be written; the file then stays in the trash.
pub fn take_out(dir: &Path, path: &str) -> std::io::Result<()> {
    let mut trashed = list(dir);
    if trashed.remove(path).is_some() {
        save(dir, &trashed)?;
    }
    Ok(())
}

/// The trashed paths whose delay has run out.
#[must_use]
pub fn due(dir: &Path, now: u64) -> Vec<String> {
    list(dir)
        .into_iter()
        .filter(|(_, due)| *due <= now)
        .map(|(path, _)| path)
        .collect()
}

/// How many days a file waits on this machine.
#[must_use]
pub fn days(dir: &Path) -> u64 {
    std::fs::read_to_string(dir.join(DAYS))
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .map_or(DEFAULT_DAYS, |days: u64| days.min(MAX_DAYS))
}

/// Set how many days a file waits, up to [`MAX_DAYS`]; zero turns the trash
/// off. Files already in the trash keep the date they were given.
///
/// # Errors
///
/// When the setting cannot be written; the previous one then stands.
pub fn set_days(dir: &Path, days: u64) -> std::io::Result<()> {
    write_all(dir, DAYS, &days.min(MAX_DAYS).to_string())
}

/// Delete `path` as a person on this machine does: into the trash for this
/// machine's days, or written at once when the trash is off. False when the
/// account has no such file, so nothing was trashed.
///
/// # Errors
///
/// When the log cannot be read, or the trash or the store cannot be written.
pub fn delete(store: &Store, vault: &Vault, path: &str, now: u64) -> crate::Result<bool> {
    let wait = days(store.root());
    if wait == 0 {
        let removed = crate::remove_known(store, vault, path)?;
        if removed {
            store.flush_segment()?;
        }
        return Ok(removed);
    }
    let known = crate::catalogue(store, vault)?
        .files
        .iter()
        .any(|file| file.path == path);
    if known {
        put(store.root(), path, now, wait).map_err(|error| StoreError::io(path, error))?;
    }
    Ok(known)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trashed_file_comes_due_after_its_days_and_not_before() {
        let dir = tempfile::tempdir().expect("tempdir");
        put(dir.path(), "a/b.jpg", 1_000, 30).expect("put");
        assert!(
            due(dir.path(), 1_000 + 30 * DAY - 1).is_empty(),
            "deleted before its delay ran out"
        );
        assert_eq!(
            due(dir.path(), 1_000 + 30 * DAY),
            ["a/b.jpg"],
            "never deleted: the space stays taken"
        );
    }

    #[test]
    fn a_restored_file_never_comes_due() {
        let dir = tempfile::tempdir().expect("tempdir");
        put(dir.path(), "a.jpg", 0, 1).expect("put");
        take_out(dir.path(), "a.jpg").expect("take out");
        assert!(
            due(dir.path(), u64::MAX).is_empty(),
            "a restored file was deleted when the delay ran out"
        );
    }

    #[test]
    fn the_wait_is_thirty_days_until_a_person_sets_it_and_stays_in_bounds() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(days(dir.path()), DEFAULT_DAYS);
        set_days(dir.path(), 0).expect("set");
        assert_eq!(
            days(dir.path()),
            0,
            "a person who turned the trash off still has it"
        );
        set_days(dir.path(), u64::MAX).expect("set");
        assert_eq!(days(dir.path()), MAX_DAYS);
        assert!(
            put(dir.path(), "a\nb", 0, 1).is_err(),
            "a line break in a path would trash another file than the one deleted"
        );
    }
}
