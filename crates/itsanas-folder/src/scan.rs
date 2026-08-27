//! Reading the folder, and mapping between disk paths and logical paths.
//!
//! # Two rules that are not negotiable
//!
//! **A logical path may never resolve outside the folder.** These strings
//! arrive in a peer's operation log, which means they are attacker-controlled
//! the moment the sync engine materialises anything. `itsanas_store::path`
//! already rejects traversal, absolute paths, backslashes, drive letters and
//! Windows device names; this module checks the *result* as well, because a
//! defence that is only applied once is a defence that stops working the day
//! somebody adds a second caller.
//!
//! **Symlinks are skipped, never followed.** Following one would let a symlink
//! inside the folder pointing at `~/.ssh` quietly upload a private key to the
//! network — the user would see a harmless-looking link in their synced folder
//! and never suspect it. Materialising a symlink on the other side is equally
//! unhelpful, since the target almost certainly does not exist there.

use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};

use crate::error::{FolderError, Result};

/// Directory used for staging atomic writes.
///
/// Lives inside the folder so a rename into place stays on one filesystem —
/// a rename across devices is not atomic and would reintroduce torn files.
pub const STAGING_DIR: &str = ".itsanas-tmp";

/// Names that are never synced.
///
/// Operating-system and editor debris. Syncing these achieves nothing and
/// causes constant spurious conflicts: every machine writes its own
/// `.DS_Store` or `Thumbs.db`, so they would fight forever.
const IGNORED_NAMES: [&str; 4] = [".DS_Store", "Thumbs.db", "desktop.ini", "ehthumbs.db"];

/// Prefixes that mark a file as an in-progress save by some other program.
const IGNORED_PREFIXES: [&str; 2] = ["~$", ".~lock."];

/// Suffixes that mark a file as transient.
const IGNORED_SUFFIXES: [&str; 3] = [".tmp", ".crdownload", ".part"];

/// What the filesystem says about one file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskFile {
    pub size: u64,
    pub modified_unix: u64,
}

/// Whether a file name should be ignored entirely.
#[must_use]
pub fn is_ignored(name: &str) -> bool {
    if IGNORED_NAMES.iter().any(|ignored| ignored == &name) {
        return true;
    }
    if IGNORED_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
    {
        return true;
    }
    IGNORED_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

/// Turn a logical path into a real one, refusing anything that escapes.
pub fn to_filesystem(root: &Path, logical: &str) -> Result<PathBuf> {
    // First line of defence: the logical path's own grammar.
    itsanas_store::path::validate(logical)
        .map_err(|_| FolderError::Escapes(PathBuf::from(logical)))?;

    let joined = root.join(logical);

    // Second line: whatever the join actually produced. `validate` already
    // rejects `..`, but this catches anything a future change to either side
    // lets through, and costs nothing.
    if joined
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(FolderError::Escapes(joined));
    }
    if !joined.starts_with(root) {
        return Err(FolderError::Escapes(joined));
    }

    Ok(joined)
}

/// Turn a real path under `root` into a logical path.
///
/// Returns `None` for anything that should not be synced: files outside the
/// root, ignored names, the staging directory, and any path whose components
/// are not valid UTF-8.
#[must_use]
pub fn to_logical(root: &Path, file: &Path) -> Option<String> {
    let relative = file.strip_prefix(root).ok()?;

    let mut parts = Vec::new();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            // Anything that is not a plain name — a prefix, a root, a `..` —
            // means this path does not describe a file inside the folder.
            return None;
        };

        let part = part.to_str()?;
        if part == STAGING_DIR || is_ignored(part) {
            return None;
        }
        parts.push(part);
    }

    if parts.is_empty() {
        return None;
    }

    let logical = parts.join("/");

    // The store will reject it anyway; refusing here means a file with an
    // impossible name is skipped quietly rather than failing the whole scan.
    itsanas_store::path::validate(&logical).ok()?;
    Some(logical)
}

/// Walk `root` and report every syncable file.
///
/// Symlinks are skipped — see the module documentation for why that is a
/// security property rather than a limitation.
pub fn scan(root: &Path) -> Result<BTreeMap<String, DiskFile>> {
    if !root.is_dir() {
        return Err(FolderError::NoFolder(root.to_owned()));
    }

    let mut found = BTreeMap::new();
    walk(root, root, &mut found)?;
    Ok(found)
}

fn walk(root: &Path, directory: &Path, out: &mut BTreeMap<String, DiskFile>) -> Result<()> {
    let entries = std::fs::read_dir(directory)
        .map_err(|error| FolderError::io(directory.to_owned(), error))?;

    for entry in entries {
        let entry = entry.map_err(|error| FolderError::io(directory.to_owned(), error))?;
        let path = entry.path();

        // `symlink_metadata` does not follow the link, which is the whole
        // point: `is_symlink` on followed metadata would always be false.
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            // Vanished between listing and stat. A folder people use is a
            // moving target; that is not an error.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(FolderError::io(path, error)),
        };

        if metadata.is_symlink() {
            continue;
        }

        if metadata.is_dir() {
            if path.file_name().and_then(|n| n.to_str()) == Some(STAGING_DIR) {
                continue;
            }
            walk(root, &path, out)?;
            continue;
        }

        let Some(logical) = to_logical(root, &path) else {
            continue;
        };

        let modified_unix = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| since.as_secs());

        out.insert(
            logical,
            DiskFile {
                size: metadata.len(),
                modified_unix,
            },
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp dir")
    }

    fn write(root: &Path, relative: &str, content: &[u8]) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn a_flat_folder_scans() {
        let dir = folder();
        write(dir.path(), "a.txt", b"one");
        write(dir.path(), "b.txt", b"two");

        let found = scan(dir.path()).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(found["a.txt"].size, 3);
    }

    #[test]
    fn nested_directories_become_slash_separated_logical_paths() {
        // Backslashes on Windows must not leak into logical paths, or the same
        // file would have two different names on two machines.
        let dir = folder();
        write(dir.path(), "work/reports/q3.txt", b"x");

        let found = scan(dir.path()).unwrap();
        assert!(
            found.contains_key("work/reports/q3.txt"),
            "got {:?}",
            found.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn operating_system_debris_is_ignored() {
        // Every machine writes its own .DS_Store or Thumbs.db, so syncing them
        // means they fight forever and generate constant conflicts.
        let dir = folder();
        write(dir.path(), "real.txt", b"x");
        for junk in [".DS_Store", "Thumbs.db", "desktop.ini", "~$report.docx"] {
            write(dir.path(), junk, b"junk");
        }
        write(dir.path(), "half-downloaded.crdownload", b"junk");
        write(dir.path(), "editor.tmp", b"junk");

        let found = scan(dir.path()).unwrap();
        assert_eq!(
            found.keys().collect::<Vec<_>>(),
            vec!["real.txt"],
            "junk was not filtered: {:?}",
            found.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_staging_directory_is_never_synced() {
        // Otherwise a half-written file is uploaded, and the folder syncs its
        // own scratch space back and forth forever.
        let dir = folder();
        write(dir.path(), "real.txt", b"x");
        write(dir.path(), &format!("{STAGING_DIR}/abc.tmp"), b"partial");

        let found = scan(dir.path()).unwrap();
        assert_eq!(found.keys().collect::<Vec<_>>(), vec!["real.txt"]);
    }

    #[test]
    fn an_empty_folder_scans_to_nothing_rather_than_failing() {
        let dir = folder();
        assert!(scan(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn scanning_a_folder_that_does_not_exist_says_so() {
        let dir = folder();
        assert!(matches!(
            scan(&dir.path().join("nope")),
            Err(FolderError::NoFolder(_))
        ));
    }

    #[test]
    fn a_logical_path_can_never_escape_the_folder() {
        // These arrive in a peer's operation log. If one could resolve outside
        // the folder, a peer could write anywhere on the disk.
        let dir = folder();
        let root = dir.path();

        for hostile in [
            "../outside.txt",
            "../../etc/passwd",
            "a/../../b",
            "/etc/passwd",
            "C:/Windows/System32/config/SAM",
            "..\\..\\Windows",
            "nul",
            "",
        ] {
            let result = to_filesystem(root, hostile);
            assert!(
                result.is_err(),
                "{hostile:?} resolved to {:?}, which is outside the folder",
                result.ok()
            );
        }
    }

    #[test]
    fn ordinary_logical_paths_resolve_inside_the_folder() {
        let dir = folder();
        let root = dir.path();

        for ordinary in ["a.txt", "work/reports/q3.pdf", ".bashrc", "Ünïcode.txt"] {
            let resolved = to_filesystem(root, ordinary).expect("should resolve");
            assert!(
                resolved.starts_with(root),
                "{ordinary:?} resolved outside the folder"
            );
        }
    }

    #[test]
    fn round_tripping_a_path_through_the_filesystem_and_back_is_stable() {
        let dir = folder();
        let root = dir.path();

        for logical in ["a.txt", "work/q3.pdf", "deep/nested/path/file.bin"] {
            let real = to_filesystem(root, logical).unwrap();
            assert_eq!(
                to_logical(root, &real).as_deref(),
                Some(logical),
                "the mapping is not a round trip"
            );
        }
    }

    #[test]
    fn a_path_outside_the_root_has_no_logical_name() {
        let dir = folder();
        let outside = dir.path().parent().unwrap().join("elsewhere.txt");
        assert_eq!(to_logical(dir.path(), &outside), None);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_skipped_rather_than_followed() {
        // A symlink inside the folder pointing at ~/.ssh would otherwise
        // quietly upload a private key to the network, and the user would see
        // only a harmless-looking link.
        let dir = folder();
        write(dir.path(), "real.txt", b"x");

        let secret = dir.path().parent().unwrap().join("secret-key");
        std::fs::write(&secret, b"PRIVATE KEY").unwrap();
        std::os::unix::fs::symlink(&secret, dir.path().join("innocent.txt")).unwrap();

        let found = scan(dir.path()).unwrap();
        assert_eq!(
            found.keys().collect::<Vec<_>>(),
            vec!["real.txt"],
            "a symlink was followed; it could point anywhere on the disk"
        );
    }

    #[test]
    fn size_and_modification_time_are_reported() {
        let dir = folder();
        write(dir.path(), "sized.txt", &vec![0u8; 1234]);

        let found = scan(dir.path()).unwrap();
        assert_eq!(found["sized.txt"].size, 1234);
        assert!(
            found["sized.txt"].modified_unix > 1_600_000_000,
            "the modification time looks wrong: {}",
            found["sized.txt"].modified_unix
        );
    }
}
