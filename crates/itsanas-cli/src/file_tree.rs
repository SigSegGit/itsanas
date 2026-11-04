//! An account's flat paths, read one folder at a time.
//!
//! The log knows paths and nothing else: a folder exists because a file's path
//! goes through it. So a folder cannot be empty, and one whose last file was
//! deleted is gone: [`level`] then answers for the nearest folder still there,
//! because a screen left on a folder that no longer exists has no way out.

use std::collections::BTreeMap;

/// A file, as a shell shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Its whole path in the account.
    pub path: String,
    /// Its size in bytes.
    pub size: u64,
    /// When it last changed, in Unix seconds.
    pub modified_unix: u64,
    /// Whether this device holds its content.
    pub here: bool,
}

/// A folder inside the one being read, with everything below it counted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// Its own name.
    pub name: String,
    /// Its whole path in the account.
    pub path: String,
    /// The files below it, at any depth.
    pub files: u64,
    /// Their sizes together.
    pub size: u64,
    /// How many of them this device does not hold.
    pub not_here: u64,
}

/// One folder of the account.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Level {
    /// The folder read: the one asked for, or the nearest one still there.
    pub at: String,
    /// The folders in it, by name.
    pub folders: Vec<Folder>,
    /// The files directly in it, by name.
    pub files: Vec<Entry>,
}

/// What `entry` is below `folder`, or `None` when it is elsewhere. The separator
/// is part of the match: `Phone2/a` is not in `Phone`.
fn below<'a>(entry: &'a Entry, folder: &str) -> Option<&'a str> {
    if folder.is_empty() {
        return Some(&entry.path);
    }
    entry.path.strip_prefix(folder)?.strip_prefix('/')
}

/// The nearest folder at or above `wanted` that some file still goes through.
fn nearest<'a>(entries: &[Entry], wanted: &'a str) -> &'a str {
    let mut folder = wanted.trim_matches('/');
    while !folder.is_empty() && !entries.iter().any(|entry| below(entry, folder).is_some()) {
        folder = folder.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
    folder
}

/// Read the account at `wanted`.
#[must_use]
pub fn level(entries: &[Entry], wanted: &str) -> Level {
    let at = nearest(entries, wanted);
    let mut folders: BTreeMap<&str, Folder> = BTreeMap::new();
    let mut files = Vec::new();
    for entry in entries {
        let Some(rest) = below(entry, at) else {
            continue;
        };
        let Some((name, _)) = rest.split_once('/') else {
            files.push(entry.clone());
            continue;
        };
        let folder = folders.entry(name).or_insert_with(|| Folder {
            name: name.to_owned(),
            path: if at.is_empty() {
                name.to_owned()
            } else {
                format!("{at}/{name}")
            },
            files: 0,
            size: 0,
            not_here: 0,
        });
        folder.files += 1;
        folder.size = folder.size.saturating_add(entry.size);
        folder.not_here += u64::from(!entry.here);
    }
    let mut folders: Vec<Folder> = folders.into_values().collect();
    folders.sort_by_cached_key(|folder| folder.name.to_lowercase());
    files.sort_by_cached_key(|entry| entry.path.to_lowercase());
    Level {
        at: at.to_owned(),
        folders,
        files,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, size: u64, here: bool) -> Entry {
        Entry {
            path: path.to_owned(),
            size,
            modified_unix: 0,
            here,
        }
    }

    fn account() -> Vec<Entry> {
        vec![
            file("note.txt", 1, true),
            file("Phone/Screenshots/a.jpg", 10, true),
            file("Phone/Screenshots/b.jpg", 20, false),
            file("Phone/Camera/trip/c.jpg", 40, true),
            file("Phone2/d.jpg", 80, true),
            file("older data/e.txt", 160, true),
        ]
    }

    fn names(level: &Level) -> Vec<&str> {
        level.folders.iter().map(|f| f.name.as_str()).collect()
    }

    fn paths(level: &Level) -> Vec<&str> {
        level.files.iter().map(|f| f.path.as_str()).collect()
    }

    #[test]
    fn red_team_the_root_shows_folders_and_not_every_file_of_the_account() {
        let root = level(&account(), "");
        assert_eq!(
            names(&root),
            ["older data", "Phone", "Phone2"],
            "a folder is missing at the root, or is drawn twice, so its files cannot be reached"
        );
        assert_eq!(
            paths(&root),
            ["note.txt"],
            "files inside folders were listed at the root: a phone folder reads as thousands of loose files"
        );
    }

    #[test]
    fn red_team_a_folder_counts_everything_below_it() {
        let root = level(&account(), "");
        let phone = root.folders.iter().find(|f| f.name == "Phone").unwrap();
        assert_eq!(
            (phone.files, phone.size, phone.not_here),
            (3, 70, 1),
            "the line under a folder misstates what it holds, so it hides files that are not on this phone"
        );
        assert_eq!(
            phone.path, "Phone",
            "tapping this folder would open another"
        );
    }

    #[test]
    fn red_team_a_folder_with_a_longer_sibling_does_not_take_its_files() {
        let phone = level(&account(), "Phone");
        assert_eq!(
            names(&phone),
            ["Camera", "Screenshots"],
            "Phone2's files were shown inside Phone, so deleting there deletes the wrong folder's files"
        );
        assert!(
            phone.files.is_empty(),
            "a file of a deeper folder was listed one level too high"
        );
    }

    #[test]
    fn red_team_a_folder_that_lost_its_last_file_leaves_the_nearest_one_still_there() {
        let mut entries = account();
        entries.retain(|entry| entry.path != "Phone/Camera/trip/c.jpg");
        let left = level(&entries, "Phone/Camera/trip");
        assert_eq!(
            left.at, "Phone",
            "the screen stayed on a folder that no longer exists, empty, with nothing to tap"
        );
        assert_eq!(names(&left), ["Screenshots"]);
    }

    #[test]
    fn a_folder_that_never_existed_reads_as_the_root() {
        let root = level(&account(), "no/such/place");
        assert_eq!(root.at, "");
        assert_eq!(paths(&root), ["note.txt"]);
    }

    #[test]
    fn an_empty_account_has_an_empty_root() {
        assert_eq!(level(&[], "Phone"), Level::default());
    }

    #[test]
    fn a_deep_folder_names_its_whole_path() {
        let camera = level(&account(), "Phone/Camera");
        assert_eq!(camera.at, "Phone/Camera");
        assert_eq!(camera.folders[0].path, "Phone/Camera/trip");
        assert_eq!(
            paths(&level(&account(), "Phone/Camera/trip")),
            ["Phone/Camera/trip/c.jpg"]
        );
    }

    #[test]
    fn a_size_no_real_account_has_does_not_panic() {
        let entries = [file("a/b", u64::MAX, true), file("a/c", u64::MAX, true)];
        assert_eq!(level(&entries, "").folders[0].size, u64::MAX);
    }
}
