//! The trash, seen from a machine's home: the record itself is
//! `itsanas_store::trash`, shared by every way a file is deleted here, and what
//! is only a phone's is below: what left the chosen folder.
//!
//! # Never from an empty listing
//!
//! A folder that reads as empty may be a permission lost or a card taken out, and
//! would make every file look gone. Files that left the folder are only ever
//! listed, and only a person sends them away, or keeps them: a file kept is in
//! the account and gone from the phone on purpose, and is not asked about again.

use crate::phone_folder::{self, FolderFile};
use itsanas_store::trash as record;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// The paths the chosen folder last listed, one per line.
const LISTED: &str = "phone-folder-listed";

/// Account paths a person kept after they left the folder, one per line,
/// appended: each line is one answer, and there are as many as answers given.
const KEPT: &str = "phone-folder-kept";

/// Where this home's trash is kept: next to its store. Up to 0.1.19 a phone
/// kept it in its home; it is moved on first use, so a file trashed then is
/// neither shown again nor forgotten.
#[must_use]
pub fn record_dir(home: &Path) -> PathBuf {
    let dir = itsanas_node::Node::store_path(home);
    let old = home.join("trash");
    if old.is_file() && dir.is_dir() && !dir.join("trash").exists() {
        let _ = std::fs::rename(&old, dir.join("trash"));
    }
    dir
}

/// Trashed paths and when each is really deleted.
#[must_use]
pub fn list(home: &Path) -> BTreeMap<String, u64> {
    record::list(&record_dir(home))
}

/// Put `path` in the trash for `days` days.
///
/// # Errors
///
/// When the record cannot be written; the file then stays where it was.
pub fn put(home: &Path, path: &str, now: u64, days: u64) -> std::io::Result<()> {
    record::put(&record_dir(home), path, now, days)
}

/// Take `path` out of the trash: it is a file like any other again.
///
/// # Errors
///
/// When the record cannot be written; the file then stays in the trash.
pub fn take_out(home: &Path, path: &str) -> std::io::Result<()> {
    record::take_out(&record_dir(home), path)
}

/// The trashed paths whose delay has run out.
#[must_use]
pub fn due(home: &Path, now: u64) -> Vec<String> {
    record::due(&record_dir(home), now)
}

fn write_all(home: &Path, name: &str, text: &str) -> std::io::Result<()> {
    // Renamed into place: a torn file would hide or show the wrong files.
    let temporary = home.join(format!("{name}.new"));
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, home.join(name))
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

    /// Every way a file is deleted here goes through
    /// `itsanas_store::trash::delete`, which follows this machine's rules
    /// (HANDOVER §8 13). Sabotage: make it always write the tombstone.
    #[test]
    fn red_team_a_delete_follows_this_machines_rules_and_the_default_keeps_it() {
        use itsanas_store::{
            deletion::{self, Mode, Rules},
            trash::Deleted,
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let home = dir.path().join("me");
        let (node, _) = itsanas_node::Node::create(&home, "a passphrase", "camille").expect("node");
        for path in ["kept.txt", "asked.txt", "here.txt", "gone.txt"] {
            node.store.write_stream(path, &b"bytes"[..]).expect("write");
        }
        node.store.flush_segment().expect("seal");
        let delete = |path: &str| {
            itsanas_store::trash::delete(&node.store, &node.vault, path, 1_000).expect("delete")
        };
        let set = |rules: Rules| {
            deletion::set_rules_here(&node.store, &node.vault, Some(&rules)).expect("rules");
        };

        assert_eq!(delete("kept.txt"), Deleted::Trashed);
        assert!(
            node.store.tombstone("kept.txt").expect("read").is_none(),
            "a delete by default was written at once: every machine drops the file, no undo"
        );
        assert!(
            deletion::shared(&node.store, &node.vault)
                .expect("shared")
                .contains_key("kept.txt"),
            "the deleted file is not in the account's trash"
        );
        assert_eq!(
            delete("never.txt"),
            Deleted::Unknown,
            "a file the account never had was put in the trash"
        );

        set(Rules {
            mode: Mode::Direct,
            ..Rules::default()
        });
        assert_eq!(delete("asked.txt"), Deleted::Waiting);
        assert!(
            node.store.tombstone("asked.txt").expect("read").is_none(),
            "a direct delete went out before anybody said who they were"
        );

        set(Rules {
            mode: Mode::Local,
            ..Rules::default()
        });
        assert_eq!(delete("here.txt"), Deleted::HereOnly);
        assert!(
            node.store.tombstone("here.txt").expect("read").is_none(),
            "a local delete reached the other machines"
        );

        set(Rules {
            mode: Mode::Direct,
            ask_auth: false,
            ..Rules::default()
        });
        assert_eq!(delete("gone.txt"), Deleted::Everywhere);
        assert!(
            node.store.tombstone("gone.txt").expect("read").is_some(),
            "a direct delete with no question still waits: the person asked for it at once"
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
