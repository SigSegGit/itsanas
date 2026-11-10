//! A phone's trash: what the person deleted here, and what left the chosen folder.
//!
//! # Kept on this phone, on purpose
//!
//! A trashed file stays a live file of the account: same path, same content, same
//! copies, counted in the same space. Only this phone hides it, and only the delay
//! running out, or the person, sends the real delete. So the log and the other
//! machines learn nothing new until then, and losing this file, or the phone,
//! leaves every file in the account: the failure costs room, never a photo.
//!
//! # Never from an empty listing
//!
//! A folder that reads as empty may be a permission lost or a card taken out, and
//! would make every file look gone. Files that left the folder are only ever
//! listed, and only a person sends them away, or keeps them: a file kept is in
//! the account and gone from the phone on purpose, and is not asked about again.

use crate::phone_folder::{self, FolderFile};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// One line per trashed file, `due_unix<TAB>path`, rewritten whole.
const RECORD: &str = "trash";

/// The paths the chosen folder last listed, one per line.
const LISTED: &str = "phone-folder-listed";

/// Account paths a person kept after they left the folder, one per line,
/// appended: each line is one answer, and there are as many as answers given.
const KEPT: &str = "phone-folder-kept";

const DAY: u64 = 24 * 60 * 60;

/// Trashed paths and when each is really deleted.
#[must_use]
pub fn list(home: &Path) -> BTreeMap<String, u64> {
    let text = std::fs::read(home.join(RECORD)).unwrap_or_default();
    String::from_utf8_lossy(&text)
        .lines()
        .filter_map(|line| {
            let (due, path) = line.split_once('\t')?;
            Some((path.to_owned(), due.parse().ok()?))
        })
        .collect()
}

fn write_all(home: &Path, name: &str, text: &str) -> std::io::Result<()> {
    // Renamed into place: a torn file would hide or show the wrong files.
    let temporary = home.join(format!("{name}.new"));
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, home.join(name))
}

fn save(home: &Path, trashed: &BTreeMap<String, u64>) -> std::io::Result<()> {
    let mut text = String::new();
    for (path, due) in trashed {
        text.push_str(&due.to_string());
        text.push('\t');
        text.push_str(path);
        text.push('\n');
    }
    write_all(home, RECORD, &text)
}

/// Put `path` in the trash for `days` days.
///
/// # Errors
///
/// When the record cannot be written; the file then stays where it was.
pub fn put(home: &Path, path: &str, now: u64, days: u64) -> std::io::Result<()> {
    let mut trashed = list(home);
    trashed.insert(
        path.to_owned(),
        now.saturating_add(days.saturating_mul(DAY)),
    );
    save(home, &trashed)
}

/// Take `path` out of the trash: it is a file like any other again.
///
/// # Errors
///
/// When the record cannot be written; the file then stays in the trash.
pub fn take_out(home: &Path, path: &str) -> std::io::Result<()> {
    let mut trashed = list(home);
    if trashed.remove(path).is_some() {
        save(home, &trashed)?;
    }
    Ok(())
}

/// The trashed paths whose delay has run out.
#[must_use]
pub fn due(home: &Path, now: u64) -> Vec<String> {
    list(home)
        .into_iter()
        .filter(|(_, due)| *due <= now)
        .map(|(path, _)| path)
        .collect()
}

/// Remember what the chosen folder holds now.
///
/// # Errors
///
/// When the file cannot be written; the previous listing then stands.
pub fn note_listing(home: &Path, listed: &[FolderFile]) -> std::io::Result<()> {
    let mut text = String::new();
    for file in listed {
        text.push_str(&file.path);
        text.push('\n');
    }
    write_all(home, LISTED, &text)
}

/// Keep `path`, an account path that left the folder: it stays in the account
/// and is no longer listed as gone.
///
/// # Errors
///
/// When the record cannot be written; the file is then listed again.
pub fn keep(home: &Path, path: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    if path.contains(['\n', '\r']) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a path holds no line break",
        ));
    }
    let mut record = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join(KEPT))?;
    writeln!(record, "{path}")
}

/// Files this folder sent to the account that it no longer holds, as account
/// paths: only those still in `account`, not trashed, not kept, under a folder
/// it listed.
#[must_use]
pub fn left_the_folder(home: &Path, account: &BTreeSet<String>) -> Vec<String> {
    let text = std::fs::read(home.join(LISTED)).unwrap_or_default();
    let text = String::from_utf8_lossy(&text);
    let listed: BTreeSet<&str> = text.lines().collect();
    let folders: BTreeSet<&str> = listed
        .iter()
        .filter_map(|path| path.split('/').next())
        .collect();
    let trashed = list(home);
    let kept = std::fs::read(home.join(KEPT)).unwrap_or_default();
    let kept = String::from_utf8_lossy(&kept);
    let kept: BTreeSet<&str> = kept.lines().collect();
    phone_folder::recorded(home)
        .into_keys()
        .filter(|path| {
            !listed.contains(path.as_str())
                && path
                    .split('/')
                    .next()
                    .is_some_and(|top| folders.contains(top))
        })
        .map(|path| {
            FolderFile {
                path,
                size: 0,
                modified: 0,
            }
            .in_account()
        })
        .filter(|path| {
            account.contains(path) && !trashed.contains_key(path) && !kept.contains(path.as_str())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> FolderFile {
        FolderFile {
            path: path.to_owned(),
            size: 1,
            modified: 1,
        }
    }

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

    /// Sabotage: drop the check on `folders` in `left_the_folder`.
    #[test]
    fn red_team_an_empty_listing_does_not_make_every_file_look_gone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let kept = file("Shots/a.jpg");
        phone_folder::imported(dir.path(), &kept).expect("record");
        note_listing(dir.path(), std::slice::from_ref(&kept)).expect("listing");
        note_listing(dir.path(), &[]).expect("empty listing");
        let account = BTreeSet::from(["Phone/Shots/a.jpg".to_owned()]);
        assert!(
            left_the_folder(dir.path(), &account).is_empty(),
            "a lost permission reads as an empty folder, and offered every file for deletion"
        );
    }

    #[test]
    fn a_file_kept_after_it_left_the_folder_is_not_asked_about_again() {
        let dir = tempfile::tempdir().expect("tempdir");
        for path in ["Shots/kept.jpg", "Shots/asked.jpg", "Shots/here.jpg"] {
            phone_folder::imported(dir.path(), &file(path)).expect("record");
        }
        note_listing(dir.path(), &[file("Shots/here.jpg")]).expect("listing");
        keep(dir.path(), "Phone/Shots/kept.jpg").expect("keep");
        let account = BTreeSet::from([
            "Phone/Shots/kept.jpg".to_owned(),
            "Phone/Shots/asked.jpg".to_owned(),
            "Phone/Shots/here.jpg".to_owned(),
        ]);
        assert_eq!(
            left_the_folder(dir.path(), &account),
            ["Phone/Shots/asked.jpg"],
            "a file the person kept was offered for deletion again, every time the trash opens"
        );
        assert!(
            keep(dir.path(), "a\nPhone/Shots/asked.jpg").is_err(),
            "a line break in a path would keep another file than the one answered"
        );
    }

    #[test]
    fn a_file_gone_from_the_folder_is_listed_and_another_folders_is_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        for path in ["Shots/gone.jpg", "Shots/here.jpg", "Old/other.jpg"] {
            phone_folder::imported(dir.path(), &file(path)).expect("record");
        }
        note_listing(dir.path(), &[file("Shots/here.jpg")]).expect("listing");
        let account = BTreeSet::from([
            "Phone/Shots/gone.jpg".to_owned(),
            "Phone/Shots/here.jpg".to_owned(),
            "Phone/Old/other.jpg".to_owned(),
        ]);
        assert_eq!(
            left_the_folder(dir.path(), &account),
            ["Phone/Shots/gone.jpg"],
            "choosing another folder must not offer the previous one's files for deletion"
        );
    }
}
