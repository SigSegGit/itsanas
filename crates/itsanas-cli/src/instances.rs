//! The nodes of this machine: `instances`, and `migrate` from the unnamed home.

use std::{
    fmt::Write as _,
    path::{Path, PathBuf},
};

use crate::config;
use crate::{
    error::{CliError, Result},
    node::Node,
};

/// Every node home under `base`, one line each, without a passphrase.
///
/// A home is `.itsanas` or a `.itsanas-NAME` **directory holding a keystore**:
/// `~/.itsanas-passphrase` is the default node's passphrase file, and a
/// directory left by `clean --purge-account` is no node. The folder is
/// "reachable" only when its 0l marker is there -- an unmounted disk leaves an
/// empty mount point that `is_dir` alone would call healthy. "running" is the
/// store lock `status` already trusts, not the listen port, which another
/// program may hold.
pub(crate) fn instances_report(base: &Path) -> String {
    let homes = config::node_homes(base);
    if homes.is_empty() {
        return format!("no node under {}\n", base.display());
    }
    instances_lines(homes)
}

pub(crate) fn instances_lines(homes: Vec<(String, PathBuf)>) -> String {
    let mut out = String::new();
    for (name, home) in homes {
        let config = config::Config::load(&Node::config_path(&home));
        let account = config
            .as_ref()
            .map_or_else(|_| "config unreadable".to_owned(), |c| c.username.clone());
        let folder = match config.ok().and_then(|c| c.folder) {
            None => "no folder".to_owned(),
            Some(folder) if folder.join(itsanas_folder::scan::MARKER).is_file() => {
                format!("folder {} reachable", folder.display())
            }
            Some(folder) => format!("folder {} UNREACHABLE", folder.display()),
        };
        let daemon = if itsanas_store::Store::is_locked(Node::store_path(&home)) {
            "daemon running"
        } else {
            "daemon stopped"
        };
        let _ = writeln!(
            out,
            "{name}: account {account}, home {}, {folder}, {daemon}",
            home.display()
        );
    }
    out
}

pub(crate) fn migrate(name: Option<&str>) -> Result<()> {
    let (name, to) = migrate_unnamed(&config::user_home(), name)?;
    print!("{}", migration_advice(&name, &to));
    Ok(())
}

/// Rename `base/.itsanas` to `base/.itsanas-NAME`, NAME the account unless given.
///
/// A rename and nothing else: the keystore, config, store and vault all live
/// inside the home, so one `rename` moves the node whole and atomically, and a
/// copy would leave two nodes with one identity. Refused while a daemon holds
/// the store -- renaming under it would leave it writing to a directory that
/// no longer has the name its service knows -- and refused onto an existing
/// home, which is somebody's node.
pub(crate) fn migrate_unnamed(base: &Path, name: Option<&str>) -> Result<(String, PathBuf)> {
    let from = base.join(".itsanas");
    if !Node::exists(&from) {
        return Err(CliError::NoNode(from));
    }
    if itsanas_store::Store::is_locked(Node::store_path(&from)) {
        return Err(CliError::Usage(format!(
            "a daemon is running on {}; stop it first, then migrate",
            from.display()
        )));
    }
    let name = match name {
        Some(name) => name.to_owned(),
        None => config::Config::load(&Node::config_path(&from))?.username,
    };
    let to = config::instance_home_in(base, &name).map_err(|error| {
        CliError::Usage(format!(
            "{error}; pick one with `itsanas migrate --name NAME`"
        ))
    })?;
    if to.exists() {
        return Err(CliError::Usage(format!(
            "{} already exists; pick another with `itsanas migrate --name NAME`",
            to.display()
        )));
    }
    std::fs::rename(&from, &to).map_err(|error| CliError::Io {
        path: from.clone(),
        source: error,
    })?;
    Ok((name, to))
}

/// What still names the old home after [`migrate_unnamed`], and how to fix it.
///
/// Said rather than done: the service files belong to the installer, and the
/// passphrase file is readable by this account only for a reason.
pub(crate) fn migration_advice(name: &str, to: &Path) -> String {
    format!(
        concat!(
            "moved to {home}; open it with `itsanas --instance {name}`.\n",
            "The service that started it still points at ~/.itsanas: it will now\n",
            "fail at every start (named instances only), and systemd's Restart=\n",
            "or the logon task will keep restarting it until you disable it.\n",
            "Switch it over:\n",
            "  Linux:   systemctl --user disable --now itsanas\n",
            "           mv ~/.config/itsanas/environment ~/.config/itsanas/{name}.environment\n",
            "           systemctl --user enable --now itsanas@{name}\n",
            "  Windows: Unregister-ScheduledTask ITSaNAS -Confirm:$false\n",
            "           then run install\\provision.ps1 -Instance {name}\n",
        ),
        home = to.display(),
        name = name,
    )
}

#[cfg(test)]
mod tests {
    use super::instances_report;
    use super::migrate_unnamed;
    use crate::test_fixtures::fake_node;

    /// Only directories holding a keystore are nodes. `~/.itsanas-passphrase`
    /// is the default node's passphrase *file*, and a home emptied by
    /// `clean --purge-account` is no node: listing either would send somebody
    /// to `--instance passphrase`, or to a node that no longer exists.
    #[test]
    fn red_team_instances_lists_only_homes_that_hold_a_node() {
        let base = tempfile::tempdir().expect("temp dir");
        std::fs::write(base.path().join(".itsanas-passphrase"), "secret").expect("file");
        std::fs::create_dir(base.path().join(".itsanas-gone")).expect("empty home");
        fake_node(&base.path().join(".itsanas"), "alice", None);
        fake_node(&base.path().join(".itsanas-bob"), "bob", None);

        let out = instances_report(base.path());
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "exactly the two nodes, got:\n{out}");
        assert!(lines[0].starts_with("(unnamed): account alice,"), "{out}");
        assert!(lines[1].starts_with("bob: account bob,"), "{out}");
        assert!(
            !out.contains("passphrase") && !out.contains("gone"),
            "{out}"
        );
        assert!(
            out.contains("stopped"),
            "no daemon holds these stores: {out}"
        );
    }

    /// The migration moves the node whole: the renamed home opens with the same
    /// passphrase and reads back the file stored before. A migration that made
    /// the new directory and copied the keystore and config but not the store
    /// would open fine and have lost every file -- hence the read, not just
    /// `exists`. And nothing is left at `~/.itsanas`, or a service still
    /// pointing there would run the same identity twice.
    #[test]
    fn red_team_migration_names_the_node_after_its_account_and_keeps_its_data() {
        let base = tempfile::tempdir().expect("temp dir");
        let old = base.path().join(".itsanas");
        {
            let (node, _phrase) =
                itsanas_node::Node::create(&old, "a passphrase", "alice").expect("create");
            node.store
                .write_file("notes/a.txt", b"kept")
                .expect("write");
        }

        let (name, to) = migrate_unnamed(base.path(), None).expect("migrate");
        assert_eq!(name, "alice");
        assert_eq!(to, base.path().join(".itsanas-alice"));
        assert!(!old.exists(), "the old home is still there");

        let node = itsanas_node::Node::open(&to, "a passphrase").expect("open migrated");
        assert_eq!(node.config.username, "alice");
        assert_eq!(
            node.store
                .read_file("notes/a.txt")
                .expect("read")
                .as_deref(),
            Some(&b"kept"[..]),
            "the migrated node lost its files"
        );
    }

    /// After a migration, a bare command must not fall back to `~/.itsanas`:
    /// `init` there would make a second identity beside the real one. It
    /// refuses and names the instance; a machine with no node at all still
    /// gets `~/.itsanas`, or the first `init` could never run.
    #[test]
    fn red_team_after_migration_a_command_without_a_name_refuses() {
        let base = tempfile::tempdir().expect("temp dir");
        assert_eq!(
            itsanas_node::config::unnamed_home(base.path()).expect("empty machine"),
            base.path().join(".itsanas"),
            "the first init needs ~/.itsanas"
        );

        fake_node(&base.path().join(".itsanas"), "alice", None);
        assert_eq!(
            itsanas_node::config::unnamed_home(base.path()).expect("unmigrated"),
            base.path().join(".itsanas")
        );

        migrate_unnamed(base.path(), None).expect("migrate");
        let error = itsanas_node::config::unnamed_home(base.path())
            .expect_err("must refuse after migration");
        assert!(
            error.to_string().contains("alice") && error.to_string().contains("--instance"),
            "the refusal must name the instance and the flag: {error}"
        );
    }

    /// The service left on ~/.itsanas after `migrate` fails at each start;
    /// the advice must say it keeps restarting, or nobody disables it.
    #[test]
    fn migration_advice_says_the_old_unit_restart_loops() {
        let text = super::migration_advice("alice", std::path::Path::new("/h/.itsanas-alice"));
        assert!(text.contains("keep restarting"), "{text}");
    }

    /// Onto an existing home is onto somebody's node: refused, both untouched.
    #[test]
    fn red_team_migration_never_lands_on_an_existing_home() {
        let base = tempfile::tempdir().expect("temp dir");
        fake_node(&base.path().join(".itsanas"), "alice", None);
        fake_node(&base.path().join(".itsanas-alice"), "other", None);

        let error = migrate_unnamed(base.path(), None).expect_err("must refuse");
        assert!(error.to_string().contains("already exists"), "{error}");
        assert!(itsanas_node::Node::exists(&base.path().join(".itsanas")));
        let kept = itsanas_node::config::Config::load(&itsanas_node::Node::config_path(
            &base.path().join(".itsanas-alice"),
        ))
        .expect("config");
        assert_eq!(kept.username, "other", "the existing home was overwritten");
    }

    /// An unmounted disk leaves an empty mount point: the directory exists, the
    /// files are gone. Calling it reachable is the lie 0l exists to prevent, so
    /// only the folder marker makes a folder reachable.
    #[test]
    fn red_team_an_empty_mount_point_is_not_a_reachable_folder() {
        let base = tempfile::tempdir().expect("temp dir");
        let folder = base.path().join("mnt");
        std::fs::create_dir(&folder).expect("mount point");
        fake_node(&base.path().join(".itsanas-carol"), "carol", Some(&folder));

        let out = instances_report(base.path());
        assert!(
            out.contains("UNREACHABLE"),
            "an empty mount point read as healthy: {out}"
        );

        std::fs::write(folder.join(itsanas_folder::scan::MARKER), "device").expect("marker");
        let out = instances_report(base.path());
        assert!(
            out.contains("reachable") && !out.contains("UNREACHABLE"),
            "{out}"
        );
    }
}
