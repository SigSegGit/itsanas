//! A folder on a phone whose new files reach the account by themselves.
//!
//! # Why this lives here
//!
//! Only the application can walk a folder the system lent it, so it lists what
//! is there and copies what it is asked for. Which of those files are new, and
//! which names may enter the account at all, is decided here: a shell draws what
//! it alone can.
//!
//! # One way
//!
//! Files go from the folder to the account. A photo deleted on the phone stays
//! in the account, because freeing room on a phone is the commonest reason to
//! delete one. A file removed from the account does not come back either: it is
//! still recorded here, until it changes.
//!
//! # The record
//!
//! One line per import, `size<TAB>modified<TAB>path`, appended, the last line for
//! a path winning. Appended because a first run may bring thousands of photos,
//! and rewriting the file for each is quadratic. A torn last line is not a file
//! seen: it is imported again, over the same path, which costs a copy and loses
//! nothing. The line is written after the store took the file, never before.

use std::{collections::HashMap, io::Write as _, path::Path};

/// Where this phone records what it imported.
const RECORD: &str = "phone-folder";

/// Where a folder's files go in the account.
const IN_ACCOUNT: &str = "Phone";

/// A file in the chosen folder, as the application lists it. `path` is relative
/// and starts with the folder's own name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderFile {
    pub path: String,
    pub size: u64,
    pub modified: u64,
}

impl FolderFile {
    /// Where this file goes in the account.
    #[must_use]
    pub fn in_account(&self) -> String {
        format!("{IN_ACCOUNT}/{}", self.path)
    }
}

/// Whether a listed name may enter the account. A document provider is another
/// application, and its names are whatever it says. Hidden names stay out for an
/// ordinary reason: Android keeps a photo being taken as `.pending-...`, and
/// thumbnails and its bin under dotted names too.
fn may_enter(path: &str) -> bool {
    !path.contains(['\\', '\t', '\n', '\r', '\0'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.'))
}

fn recorded(home: &Path) -> HashMap<String, (u64, u64)> {
    let text = std::fs::read(home.join(RECORD)).unwrap_or_default();
    String::from_utf8_lossy(&text)
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let size = parts.next()?.parse().ok()?;
            let modified = parts.next()?.parse().ok()?;
            Some((parts.next()?.to_owned(), (size, modified)))
        })
        .collect()
}

/// Which of `listed` the application should copy: new or changed, and allowed in.
#[must_use]
pub fn wanted(home: &Path, listed: Vec<FolderFile>) -> Vec<FolderFile> {
    let seen = recorded(home);
    listed
        .into_iter()
        .filter(|file| {
            may_enter(&file.path) && seen.get(&file.path) != Some(&(file.size, file.modified))
        })
        .collect()
}

/// Record a file the store has taken.
///
/// # Errors
///
/// When the record cannot be written; the file is then imported again next time.
pub fn imported(home: &Path, file: &FolderFile) -> std::io::Result<()> {
    let mut record = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join(RECORD))?;
    writeln!(record, "{}\t{}\t{}", file.size, file.modified, file.path)
}

#[cfg(test)]
mod tests {
    use super::{FolderFile, imported, wanted};

    fn file(path: &str, size: u64, modified: u64) -> FolderFile {
        FolderFile {
            path: path.to_owned(),
            size,
            modified,
        }
    }

    /// Sabotage: compare the path alone, or never record.
    #[test]
    fn red_team_a_folder_file_is_copied_once_and_again_when_it_changes() {
        let home = tempfile::tempdir().expect("temp dir");
        let photo = file("Camera/a.jpg", 10, 100);
        assert_eq!(
            wanted(home.path(), vec![photo.clone()]),
            vec![photo.clone()],
            "a new photo was not asked for: it never reaches the account"
        );
        imported(home.path(), &photo).expect("record");
        assert_eq!(
            wanted(home.path(), vec![photo.clone()]),
            Vec::new(),
            "a photo already in the account was asked for again: every round copies the \
             whole folder"
        );
        for changed in [file("Camera/a.jpg", 11, 100), file("Camera/a.jpg", 10, 101)] {
            assert_eq!(
                wanted(home.path(), vec![changed.clone()]),
                vec![changed],
                "an edited file was not asked for: the account keeps the old version"
            );
        }
        let edited = file("Camera/a.jpg", 11, 101);
        imported(home.path(), &edited).expect("record");
        assert_eq!(
            wanted(home.path(), vec![edited]),
            Vec::new(),
            "the older line won over the newer one: this file is copied every round"
        );
    }

    /// Another application names these files. Sabotage: let everything in.
    #[test]
    fn red_team_a_folder_name_cannot_climb_or_slip_in_hidden() {
        let home = tempfile::tempdir().expect("temp dir");
        for name in [
            "../outside.jpg",
            "Camera/../../outside.jpg",
            "Camera//a.jpg",
            "/Camera/a.jpg",
            "Camera/.pending-1-a.jpg",
            "Camera/.thumbnails/a.jpg",
            "Camera\\a.jpg",
            "Camera/a\tb.jpg",
            "Camera/a\nb.jpg",
            "",
        ] {
            assert_eq!(
                wanted(home.path(), vec![file(name, 1, 1)]),
                Vec::new(),
                "{name:?} would enter the account: a provider's name decides where a file \
                 lands, or half-taken photos sync to every machine"
            );
        }
    }

    #[test]
    fn a_torn_line_in_the_record_is_not_a_file_seen() {
        let home = tempfile::tempdir().expect("temp dir");
        std::fs::write(home.path().join(super::RECORD), "10\t100").expect("record");
        let photo = file("Camera/a.jpg", 10, 100);
        assert_eq!(wanted(home.path(), vec![photo.clone()]), vec![photo]);
    }
}
