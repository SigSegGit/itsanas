//! Where a node's home is: the default one, named instances, the user's home
//! directory on each system, and a node left under the other home variable.

use std::path::{Path, PathBuf};

use crate::error::{NodeError, Result};

/// Where a node keeps its state, if the user did not say: `ITSANAS_HOME`,
/// else [`unnamed_home`] under the user's home directory.
///
/// Deliberately not the OS config directory: this holds bulk data as well as
/// settings, and burying gigabytes of chunks in `AppData` or `~/.config` would
/// surprise people and break backup tooling that treats those as small.
///
/// # Errors
///
/// As [`unnamed_home`]: the machine holds named instances only.
pub fn default_home() -> Result<PathBuf> {
    std::env::var_os("ITSANAS_HOME").map_or_else(
        || unnamed_home(&user_home()),
        |home| Ok(PathBuf::from(home)),
    )
}

/// Every node home under `base`, sorted: `(unnamed)` for `.itsanas`, else NAME.
///
/// A home is `.itsanas` or a `.itsanas-NAME` **directory holding a keystore**,
/// so `~/.itsanas-passphrase` (a file) and a purged home are not listed.
#[must_use]
pub fn node_homes(base: &Path) -> Vec<(String, PathBuf)> {
    let mut homes: Vec<(String, PathBuf)> = std::fs::read_dir(base)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let file_name = entry.file_name();
            let file_name = file_name.to_str()?;
            let name = match file_name.strip_prefix(".itsanas") {
                Some("") => "(unnamed)".to_owned(),
                Some(rest) => rest.strip_prefix('-')?.to_owned(),
                None => return None,
            };
            let home = entry.path();
            crate::Node::exists(&home).then_some((name, home))
        })
        .collect();
    homes.sort();
    homes
}

/// The home a command gets with neither `--instance` nor `--home`.
///
/// `base/.itsanas` while it holds a node, or while the machine has no node at
/// all (the first `init`). Once only named instances exist -- after `itsanas
/// migrate`, or on a machine set up with `provision.sh --instance` -- falling
/// back to `~/.itsanas` would open nothing, or worse, let `init` or `login`
/// make a second identity beside the real ones for a stale service to start.
/// So it refuses and names the instances to choose from.
///
/// # Errors
///
/// When `base` holds named instances and no `.itsanas` node.
pub fn unnamed_home(base: &Path) -> Result<PathBuf> {
    let home = base.join(".itsanas");
    if crate::Node::exists(&home) {
        return Ok(home);
    }
    let named: Vec<String> = node_homes(base).into_iter().map(|(name, _)| name).collect();
    if named.is_empty() {
        return Ok(home);
    }
    Err(NodeError::Config(format!(
        "this machine has named instances only ({}); say which with \
         `--instance NAME` or ITSANAS_INSTANCE",
        named.join(", ")
    )))
}

/// Where the named instance `name` keeps its state: `~/.itsanas-NAME`.
///
/// The same mapping and the same rule as `install/provision.sh --instance`, so
/// a node the script set up is the node `itsanas --instance NAME` opens. The
/// rule is strict because the name becomes a directory, a systemd unit and a
/// file name: anything that needs quoting, or a `/` or `..` that would walk
/// out of the home directory, is refused rather than cleaned.
///
/// # Errors
///
/// If `name` is empty, longer than 32 characters, or not lowercase letters,
/// digits and inner dashes.
pub fn instance_home(name: &str) -> Result<PathBuf> {
    instance_home_in(&user_home(), name)
}

/// [`instance_home`] under `base` rather than the user's home directory.
///
/// `base.join(".itsanas-NAME")`, with the same rule, so the migration and its
/// tests resolve a name exactly as `--instance` does.
///
/// # Errors
///
/// As [`instance_home`].
pub fn instance_home_in(base: &Path, name: &str) -> Result<PathBuf> {
    // `~/.itsanas-passphrase` is the default node's passphrase file, so that
    // one name would point an instance at a file.
    let valid = !name.is_empty()
        && name != "passphrase"
        && name.len() <= 32
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !valid {
        return Err(NodeError::Config(format!(
            concat!(
                "an instance name is 1 to 32 lowercase letters, digits and inner ",
                "dashes, because it names a directory, a service and a file; found {:?}"
            ),
            name
        )));
    }
    Ok(base.join(format!(".itsanas-{name}")))
}

/// The directory every node home hangs off: `~/.itsanas` and `~/.itsanas-NAME`.
///
/// What `itsanas instances` scans. `.` when neither `HOME` nor `USERPROFILE` is
/// set.
#[must_use]
pub fn user_home() -> PathBuf {
    dirs_home().unwrap_or_else(|| PathBuf::from("."))
}

fn dirs_home() -> Option<PathBuf> {
    home_from(
        std::env::var_os("HOME"),
        std::env::var_os("USERPROFILE"),
        cfg!(windows),
    )
}

/// The user's home directory from `HOME` and `USERPROFILE`.
///
/// On Windows, `USERPROFILE` first: it is what `provision.ps1` and
/// `clean.ps1` build every node home from, while `HOME` is not a Windows
/// variable at all -- Git Bash sets one, and some installs make it permanent,
/// pointing elsewhere. Preferring it there made the CLI and the scripts name
/// two different `~/.itsanas-NAME`. Elsewhere `HOME` is the convention.
fn home_from(
    home: Option<std::ffi::OsString>,
    profile: Option<std::ffi::OsString>,
    windows: bool,
) -> Option<PathBuf> {
    let (first, second) = ranked(home, profile, windows);
    first.or(second)
}

/// `HOME` and `USERPROFILE` in the order [`home_from`] tries them, empty ones
/// dropped.
fn ranked(
    home: Option<std::ffi::OsString>,
    profile: Option<std::ffi::OsString>,
    windows: bool,
) -> (Option<PathBuf>, Option<PathBuf>) {
    let (first, second) = if windows {
        (profile, home)
    } else {
        (home, profile)
    };
    let set = |v: Option<std::ffi::OsString>| v.filter(|v| !v.is_empty()).map(PathBuf::from);
    (set(first), set(second))
}

/// The home directory the variable [`user_home`] did *not* choose names, if it
/// is set and differs.
///
/// A node made while the other variable won -- by a build that preferred
/// `HOME` on Windows, or from a Git Bash whose `HOME` points elsewhere -- lives
/// there, and [`stranded_node`] looks for it.
#[must_use]
pub fn other_user_home() -> Option<PathBuf> {
    let (first, second) = ranked(
        std::env::var_os("HOME"),
        std::env::var_os("USERPROFILE"),
        cfg!(windows),
    );
    let first = first.unwrap_or_else(|| PathBuf::from("."));
    second.filter(|second| *second != first)
}

/// A node at the same place under `other` when `chosen`, under `base`, holds
/// none.
///
/// # Why this exists
///
/// On Windows the CLI prefers `USERPROFILE` over `HOME`; a node created when
/// `HOME` won sits in `HOME/.itsanas` and is invisible from `USERPROFILE`.
/// Proceeding would have `init` or `login` mint a second identity beside it,
/// and every other command report "no node" over a machine that has one. The
/// caller refuses instead and names the path.
#[must_use]
pub fn stranded_node(chosen: &Path, base: &Path, other: Option<&Path>) -> Option<PathBuf> {
    if crate::Node::exists(chosen) {
        return None;
    }
    let there = other?.join(chosen.strip_prefix(base).ok()?);
    crate::Node::exists(&there).then_some(there)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// provision.ps1 puts `-Instance NAME` under `$env:USERPROFILE`; the CLI
    /// must look in the same place on Windows even when a `HOME` exists, or
    /// `itsanas --instance NAME` opens (or `init` creates) a different node.
    #[test]
    fn red_team_on_windows_the_profile_wins_over_home() {
        let home = Some(std::ffi::OsString::from("/c/elsewhere"));
        let profile = Some(std::ffi::OsString::from("C:\\Users\\sam"));
        assert_eq!(
            super::home_from(home.clone(), profile.clone(), true),
            Some(std::path::PathBuf::from("C:\\Users\\sam")),
            "the CLI and provision.ps1 would name two different homes"
        );
        assert_eq!(
            super::home_from(home, profile, false),
            Some(std::path::PathBuf::from("/c/elsewhere"))
        );
        assert_eq!(
            super::home_from(Some("".into()), Some("C:\\Users\\sam".into()), false),
            Some(std::path::PathBuf::from("C:\\Users\\sam")),
            "an empty HOME is no home"
        );
    }

    /// A node under the variable that lost (`HOME` on Windows) is found when
    /// the chosen home holds none, and only then.
    #[test]
    fn red_team_a_node_under_the_other_home_variable_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let (profile, home) = (dir.path().join("profile"), dir.path().join("home"));
        let keystore = |base: &Path| {
            std::fs::create_dir_all(base.join(".itsanas")).unwrap();
            std::fs::write(base.join(".itsanas").join("keystore.bin"), b"k").unwrap();
        };
        keystore(&home);
        let chosen = profile.join(".itsanas");
        assert_eq!(
            stranded_node(&chosen, &profile, Some(&home)),
            Some(home.join(".itsanas")),
            "the node under HOME was not seen: `init` would mint a second identity"
        );
        assert_eq!(stranded_node(&chosen, &profile, None), None);
        keystore(&profile);
        assert_eq!(
            stranded_node(&chosen, &profile, Some(&home)),
            None,
            "a node where the CLI looks is the node; the other is not a reason to refuse"
        );
    }

    #[test]
    fn red_team_an_instance_name_cannot_leave_the_home_directory() {
        // The name is joined onto the home directory: a separator or a `..`
        // accepted here would let `--instance` open, and `clean.sh` delete,
        // a directory that belongs to no instance.
        for name in [
            "",
            "../etc",
            "a/b",
            r"a\b",
            "..",
            "Nicolas",
            "-x",
            "x-",
            "a b",
            "abcdefghijklmnopqrstuvwxyz0123456",
            "passphrase",
        ] {
            assert!(
                instance_home(name).is_err(),
                "{name:?} was accepted as an instance name"
            );
        }
    }

    #[test]
    fn an_instance_lives_where_provision_sh_puts_it() {
        // provision.sh sets ITSANAS_HOME="$HOME/.itsanas-$INSTANCE"; if the CLI
        // mapped a name elsewhere, `itsanas --instance x` would open an empty
        // home beside the node the script installed.
        let home = instance_home("tester-2").expect("a valid name");
        assert_eq!(
            home.file_name(),
            Some(std::ffi::OsStr::new(".itsanas-tester-2"))
        );
        assert_eq!(home.parent(), dirs_home().as_deref());
        assert!(instance_home(&"a".repeat(32)).is_ok());
    }
}
