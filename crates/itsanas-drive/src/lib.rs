//! The account as a folder: everything listed, downloaded when opened.
//!
//! # What this is for
//!
//! A synced folder holds what has been downloaded. An account can be larger
//! than the disk it is read from — the ordinary case for a phone and a common
//! one for a laptop — and the honest way to show the rest is to show it: a
//! name, a size and a date in the file manager, with the bytes fetched the
//! first time somebody opens one. That is what other people's cloud folders do,
//! and it is the difference between "your files" and "the part of your files
//! that fitted".
//!
//! # What is here, and what is not
//!
//! Here: the **projection** — given everything the account knows and a
//! directory somebody is looking at, what should appear. That is where the bugs
//! live (a prefix is not a directory, a path separator is not the same on both
//! sides, an absent file must still have a size), and it is testable without
//! any filesystem driver at all.
//!
//! Not here yet: the binding to the operating system. Two findings decided
//! that, and both are worth writing down rather than rediscovering.
//!
//! ## The Windows mechanism is right, and off by default
//!
//! The **Projected File System** is exactly this shape — placeholders,
//! hydration on read, change notifications — ships with Windows, needs no
//! third-party driver, and is what VFS for Git is built on. It is an optional
//! feature, off unless somebody turns it on:
//!
//! ```text
//! Enable-WindowsOptionalFeature -Online -FeatureName Client-ProjFS -All
//! ```
//!
//! Until then `ProjectedFSLib.dll` does not exist on the machine. That matters
//! more than it sounds: a binary that *links* it will not start at all —
//! measured, `STATUS_DLL_NOT_FOUND` before `main` — so the projection cannot
//! live inside `itsanas.exe`. It belongs in a separate binary, or behind a
//! delayed load. A daemon that stops starting because somebody upgraded is not
//! a trade this project makes.
//!
//! ## The obvious library is licensed incompatibly
//!
//! `windows-projfs` has the best API for this by a distance — a safe trait, no
//! unsafe on our side — and is **GPL-2.0**. This project is AGPL-3.0-or-later,
//! and GPL-2.0-only cannot be combined with the GPLv3 family. It was in the
//! dependency tree for about an hour and is not any more.
//!
//! The copyright holder offered to change the project's licence to make it fit.
//! Declined, and the reason is not sentiment: AGPL's network clause is the one
//! that matters for a system whose whole purpose is other people running nodes.
//! Under GPL-2.0 somebody could run a modified node as a service and owe
//! nothing, and "or later" would be gone as well. That is a large thing to
//! trade for a nicer binding API when `projfs` (MIT) is thinner, workable, and
//! costs only more code — which is the cheap side of that trade.
//!
//! # Linux
//!
//! FUSE, and later. The machines that need a virtual drive are the ones
//! somebody sits in front of; on this fleet the Pi and the VM are servers that
//! hold everything and that nobody browses.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;

/// One thing a file manager should show.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Entry {
    /// A directory. Not a thing this account stores — a prefix several paths
    /// share, which is why it carries no date and no size.
    Directory(String),
    /// A file, with what the log says about it whether or not this machine
    /// holds the bytes.
    File {
        name: String,
        size: u64,
        modified_unix: u64,
        /// Whether the content is on this machine. A file manager does not need
        /// to know; a person does, and it decides whether opening it is instant
        /// or a download.
        here: bool,
    },
}

impl Entry {
    /// The name as it appears in the folder.
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::Directory(name) | Self::File { name, .. } => name,
        }
    }
}

/// One file the account has, as the projection sees it.
#[derive(Clone, Copy, Debug)]
pub struct Known<'a> {
    pub path: &'a str,
    pub size: u64,
    pub modified_unix: u64,
    pub here: bool,
}

/// What should appear inside `directory`.
///
/// `directory` is a logical path with no trailing separator; the empty string
/// is the top. Sub-directories are synthesised from the paths that share a
/// prefix, because an account stores paths and not a tree — nothing anywhere
/// records that `Documents` exists, only that `Documents/report.pdf` does.
///
/// Directories first, then files, both sorted, so two machines showing the same
/// account show it the same way round.
#[must_use]
pub fn listing<'a>(files: impl IntoIterator<Item = Known<'a>>, directory: &str) -> Vec<Entry> {
    let prefix = if directory.is_empty() {
        String::new()
    } else {
        format!("{}/", directory.trim_end_matches('/'))
    };

    let mut directories: BTreeSet<String> = BTreeSet::new();
    let mut entries: Vec<Entry> = Vec::new();

    for file in files {
        let Some(rest) = file.path.strip_prefix(prefix.as_str()) else {
            continue;
        };
        if rest.is_empty() {
            continue;
        }

        if let Some((directory, _)) = rest.split_once('/') {
            directories.insert(directory.to_owned());
            continue;
        }

        entries.push(Entry::File {
            name: rest.to_owned(),
            size: file.size,
            modified_unix: file.modified_unix,
            here: file.here,
        });
    }

    entries.sort();
    let mut out: Vec<Entry> = directories.into_iter().map(Entry::Directory).collect();
    out.extend(entries);
    out
}

/// Turn a path the operating system handed over into the account's form.
///
/// Windows speaks in backslashes and the account speaks in slashes, with no
/// leading separator and an empty string at the top. In one place because
/// getting it wrong does not fail: the listing comes back empty, which reads as
/// "the account is empty" rather than as "the path did not match".
#[must_use]
pub fn logical(path: &str) -> String {
    path.replace('\\', "/").trim_matches('/').to_owned()
}

#[cfg(test)]
mod tests {
    use super::{Entry, Known, listing, logical};

    fn file(path: &str) -> Known<'_> {
        Known {
            path,
            size: 10,
            modified_unix: 1_700_000_000,
            here: true,
        }
    }

    #[test]
    fn a_directory_is_a_prefix_several_files_share() {
        // Nothing in the account records that `Documents` exists. If the
        // projection does not invent it, a file manager shows an account that
        // has sub-folders as an empty one.
        let files = [
            file("Documents/2026/report.pdf"),
            file("Documents/notes.txt"),
            file("photo.jpg"),
        ];

        assert_eq!(
            listing(files, ""),
            vec![
                Entry::Directory("Documents".to_owned()),
                Entry::File {
                    name: "photo.jpg".to_owned(),
                    size: 10,
                    modified_unix: 1_700_000_000,
                    here: true,
                },
            ]
        );

        assert_eq!(
            listing(files, "Documents")
                .iter()
                .map(Entry::name)
                .collect::<Vec<_>>(),
            ["2026", "notes.txt"]
        );
    }

    #[test]
    fn a_file_that_is_not_here_is_still_a_file_with_a_size() {
        // The whole point. A placeholder with no size shows as zero bytes, and
        // somebody concludes their file is damaged rather than absent.
        let absent = Known {
            path: "film.mkv",
            size: 4_000_000_000,
            modified_unix: 42,
            here: false,
        };

        assert_eq!(
            listing([absent], ""),
            vec![Entry::File {
                name: "film.mkv".to_owned(),
                size: 4_000_000_000,
                modified_unix: 42,
                here: false,
            }]
        );
    }

    #[test]
    fn a_name_that_merely_starts_the_same_is_not_inside_it() {
        let files = [file("Photos/one.jpg"), file("Photos-old/two.jpg")];
        assert_eq!(
            listing(files, "Photos")
                .iter()
                .map(Entry::name)
                .collect::<Vec<_>>(),
            ["one.jpg"]
        );
    }

    #[test]
    fn the_separators_the_operating_system_uses_are_not_the_accounts() {
        assert_eq!(
            logical(r"Documents\2026\report.pdf"),
            "Documents/2026/report.pdf"
        );
        assert_eq!(logical(""), "");
        assert_eq!(logical(r"\"), "");
        assert_eq!(logical("notes.txt"), "notes.txt");
    }
}
