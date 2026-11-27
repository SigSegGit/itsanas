//! The commands that write to another drive: `backup`, `portable`, `copy-to-key`,
//! and `vault`, which moves this node's vault.

use std::path::Path;

use crate::cli::{BackupAction, Command};
use crate::opening::{daemon_holds, hand_to_daemon, open};
use crate::{backup, config, copy_to_key, drives, jobs, portable};
use crate::{
    config::format_size,
    error::{CliError, Result},
    node::Node,
};

/// Tell whoever keeps a ledger about this device that it is going.
///
/// Who that is: this account's other devices, which keep the ledger of its own
/// files, and the owners of whatever this machine hosts, which keep theirs.
/// The configured peers are told too, because somebody typed them in. Each is
/// told at most once, and a peer that cannot be reached is named rather than
/// retried: its records age out of `LIVE_FOR` like a crash's would.
/// `itsanas delete-account`: check the typed name, drain, then delete at the
/// coordinator. The name check comes first and alone: a mistyped command must
/// cost nothing.
/// `itsanas vault [PATH]`.
pub(crate) fn backup_command(home: &Path, action: BackupAction) -> Result<()> {
    if let BackupAction::Export { prune: true, .. } = &action
        && daemon_holds(home)
    {
        return Err(CliError::Usage(
            "--prune deletes from the copy, so it runs with the store in hand: stop the daemon (itsanas stop), or refresh the copy without --prune"
                .to_owned(),
        ));
    }
    if daemon_holds(home) {
        let (kind, dir) = match &action {
            BackupAction::Export { dir, .. } => (jobs::Kind::Export, dir),
            BackupAction::Restore { dir } => (jobs::Kind::Restore, dir),
        };
        return hand_to_daemon(home, kind, dir);
    }
    let node = match open(home) {
        // A daemon that started between the two looks.
        Err(_) if daemon_holds(home) => {
            let (kind, dir) = match &action {
                BackupAction::Export { dir, .. } => (jobs::Kind::Export, dir),
                BackupAction::Restore { dir } => (jobs::Kind::Restore, dir),
            };
            return hand_to_daemon(home, kind, dir);
        }
        other => other?,
    };
    match action {
        BackupAction::Export { dir, prune } => {
            let moved = backup::export_with(&node, &dir, prune)?;
            println!(
                "{}",
                backup::export_summary(&moved, &dir.display().to_string())
            );
            if let Some(warning) = drives::warning_for(&dir, "the backup") {
                println!("note: {warning}");
            }
        }
        BackupAction::Restore { dir } => {
            let (moved, report) = backup::restore(&node, &dir)?;
            println!(
                "{}",
                backup::restore_summary(&moved, &report, &dir.display().to_string())
            );
        }
    }
    Ok(())
}

/// `portable`, `copy-to-key` and `vault`: what `run` sends to a drive.
pub(crate) fn to_a_drive(home: &Path, command: Command) -> Result<()> {
    match command {
        Command::Portable { dir } => make_portable(&dir),
        Command::CopyToKey {
            dir,
            detach,
            passphrases_on_stdin,
        } => copy_to_key::command(home, &dir, detach, passphrases_on_stdin),
        Command::Vault {
            path,
            same_disk,
            move_data,
        } => vault(home, path.as_deref(), same_disk, move_data),
        _ => unreachable!("only portable, copy-to-key and vault are sent here"),
    }
}

/// Whether a running daemon holds this node's store: disk work then goes
/// through it (`jobs.rs`), which has the keys and the vault open already.
/// `itsanas portable DIR`.
pub(crate) fn make_portable(dir: &Path) -> Result<()> {
    let dir = std::path::absolute(dir).map_err(|source| CliError::Io {
        path: dir.to_owned(),
        source,
    })?;
    println!("{}", portable::make(&dir)?);
    Ok(())
}

pub(crate) fn vault(
    home: &Path,
    path: Option<&Path>,
    same_disk: bool,
    move_data: bool,
) -> Result<()> {
    if let Some(path) = path.filter(|_| move_data && daemon_holds(home)) {
        return hand_to_daemon(home, jobs::Kind::MoveVault { same_disk }, path);
    }
    let mut node = match open(home) {
        // A daemon that started between the two looks.
        Err(_) if move_data && path.is_some() && daemon_holds(home) => {
            let path = path.unwrap_or(home);
            return hand_to_daemon(home, jobs::Kind::MoveVault { same_disk }, path);
        }
        other => other?,
    };
    let Some(path) = path else {
        let dir = node.vault_dir();
        println!(
            "vault {} ({})",
            dir.display(),
            if node.config.vault.is_some() {
                "set"
            } else {
                "the default"
            }
        );
        return Ok(());
    };
    let wanted = std::path::absolute(path).map_err(|source| CliError::Io {
        path: path.to_owned(),
        source,
    })?;
    if wanted == node.vault_dir() {
        println!("vault already at {}", wanted.display());
        return Ok(());
    }
    if move_data {
        println!("{}", move_vault(node, &wanted, same_disk)?);
        println!("the daemon uses it from its next start");
        return Ok(());
    }
    switch_empty_vault(&mut node, &wanted, same_disk)?;
    println!(
        "vault set to {}; the daemon uses it from its next start",
        wanted.display()
    );
    Ok(())
}

/// Copy this node's vault to `wanted`, check the copy, and switch to it.
///
/// Copied under a temporary name and renamed into place only when complete,
/// so a disk that drops mid-copy leaves no half vault that a later start
/// could take for a whole one. Checked twice: every file and byte arrived,
/// and the copy opens and holds what the original held. The original is left
/// in place: deleting tens of GB is the person's decision, once the node has
/// run from the new disk.
pub(crate) fn move_vault(node: Node, wanted: &Path, same_disk: bool) -> Result<String> {
    check_vault_target(&node, wanted, same_disk)?;
    let from = node.vault_dir();
    let held = node.vault.held_bytes()?;
    let config_path = Node::config_path(&node.home);
    // Closed before copying: Windows will not read a database another handle
    // has locked, and a copy of a file still open for writing is a guess.
    drop(node);
    // What an earlier, interrupted move left is ours to clear; anything else
    // in the target is somebody's.
    let _ = std::fs::remove_dir_all(wanted.join(STAGING));
    if std::fs::read_dir(wanted).is_ok_and(|mut entries| entries.next().is_some()) {
        return Err(CliError::Usage(format!(
            "{} is not empty; the vault moves into an empty or new directory",
            wanted.display()
        )));
    }
    // Staged inside the target, not beside it: a mount point or a drive root
    // has no sibling on its own disk, and a rename across disks fails after
    // the whole copy (#297's red team). Each entry is then renamed up, on the
    // same disk, and the marker last: until it lands the target reads as no
    // vault, never as a whole one.
    let original = tree_size(&from)?;
    room_for_vault(original.1, drives::free_bytes(wanted), wanted)?;
    let staging = wanted.join(STAGING);
    let copied = match copy_tree(&from, &staging) {
        Ok(copied) => copied,
        Err(error) => {
            // A drive that filled or went away mid-copy keeps nothing of it.
            let _ = std::fs::remove_dir_all(&staging);
            return Err(error);
        }
    };
    if copied != original {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(CliError::Usage(format!(
            "the copy to {} came out at {} files, {} bytes against {} files, {} bytes: nothing \
             was switched",
            staging.display(),
            copied.0,
            copied.1,
            original.0,
            original.1
        )));
    }
    let switched = (|| {
        let mut names: Vec<_> = std::fs::read_dir(&staging)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<std::io::Result<_>>()?;
        names.sort_by_key(|name| name == itsanas_node::node::VAULT_MARKER);
        for name in names {
            std::fs::rename(staging.join(&name), wanted.join(&name))?;
        }
        std::fs::remove_dir(&staging)
    })();
    if let Err(source) = switched {
        return Err(CliError::Io {
            path: wanted.to_owned(),
            source,
        });
    }
    let reopened = itsanas_store::Vault::open(wanted)?.held_bytes()?;
    if reopened != held {
        return Err(CliError::Usage(format!(
            "the copy at {} holds {reopened} bytes against {held}: the node still uses {}",
            wanted.display(),
            from.display()
        )));
    }
    let mut config = config::Config::load(&config_path)?;
    config.vault = Some(wanted.to_owned());
    config.save(&config_path)?;
    let mut said = format!(
        "vault moved to {} ({} files, {}); the old copy is still at {}: delete it once the \
         node runs from the new one",
        wanted.display(),
        copied.0,
        format_size(copied.1),
        from.display()
    );
    if let Some(warning) = drives::warning_for(wanted, "the vault") {
        said = format!("{said}. Note: {warning}");
    }
    Ok(said)
}

/// Refuse a move onto a drive without room for the vault, before a byte is
/// copied: a USB key that fills halfway is a wasted hour. A tenth on top for
/// the file system's own overhead per file. A drive that will not say how
/// much is free is not refused: the copy then finds out, and cleans up.
pub(crate) fn room_for_vault(held: u64, free: Option<u64>, wanted: &Path) -> Result<()> {
    let needed = held + held / 10;
    match free {
        Some(free) if free < needed => Err(CliError::Usage(format!(
            "the vault holds {}; {} has {} free, and it needs about {}: choose a larger \
             drive, or lower what this machine offers first (`itsanas space --pledge ..`)",
            format_size(held),
            wanted.display(),
            format_size(free),
            format_size(needed)
        ))),
        _ => Ok(()),
    }
}

/// Where a moving vault is copied, inside its target.
pub(crate) const STAGING: &str = ".itsanas-incoming";

/// Copy the directory tree `from` to `to`; (files, bytes) copied.
pub(crate) fn copy_tree(from: &Path, to: &Path) -> Result<(u64, u64)> {
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| CliError::Io { path, source }
    };
    let mut count = (0, 0);
    let mut stack = vec![(from.to_path_buf(), to.to_path_buf())];
    while let Some((source, target)) = stack.pop() {
        std::fs::create_dir_all(&target).map_err(io(&target))?;
        for entry in std::fs::read_dir(&source).map_err(io(&source))? {
            let entry = entry.map_err(io(&source))?;
            let (inner, outer) = (entry.path(), target.join(entry.file_name()));
            if entry.file_type().map_err(io(&inner))?.is_dir() {
                stack.push((inner, outer));
            } else {
                count.0 += 1;
                count.1 += std::fs::copy(&inner, &outer).map_err(io(&inner))?;
            }
        }
    }
    // On the target's disk, not in a cache: the switch follows.
    Ok(count)
}

/// (files, bytes) under `root`.
pub(crate) fn tree_size(root: &Path) -> Result<(u64, u64)> {
    let mut count = (0, 0);
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|source| CliError::Io {
            path: dir.clone(),
            source,
        })? {
            let entry = entry.map_err(|source| CliError::Io {
                path: dir.clone(),
                source,
            })?;
            let meta = entry.metadata().map_err(|source| CliError::Io {
                path: entry.path(),
                source,
            })?;
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                count.0 += 1;
                count.1 += meta.len();
            }
        }
    }
    Ok(count)
}

/// Point `node` at an empty or new directory `wanted`, marked as its own.
/// A directory that holds anything, or another device's vault, is refused:
/// adopting it would mix what two nodes hold, or another node's hosted data.
/// Point this node at a vault elsewhere, while its own holds nothing. A vault
/// that holds data moves only with `--move`, which copies and checks before
/// switching: a bare path change would strand it. The setup's disk question
/// comes here too.
pub(crate) fn switch_empty_vault(node: &mut Node, wanted: &Path, same_disk: bool) -> Result<()> {
    if node.vault.held_bytes()? > 0 {
        return Err(CliError::Usage(format!(
            "the vault at {} holds data: `itsanas vault {} --move` copies it there and switches once the copy checks out",
            node.vault_dir().display(),
            wanted.display()
        )));
    }
    set_vault(node, wanted, same_disk)
}

pub(crate) fn set_vault(node: &mut Node, wanted: &Path, same_disk: bool) -> Result<()> {
    check_vault_target(node, wanted, same_disk)?;
    let io = |path: &Path| {
        let path = path.to_owned();
        move |source| CliError::Io { path, source }
    };
    let marker = wanted.join(itsanas_node::node::VAULT_MARKER);
    let mine = node.store.device_id().to_hex();
    match std::fs::read_to_string(&marker) {
        Ok(text) if text.trim() == mine => {}
        Ok(_) => {
            return Err(CliError::Usage(format!(
                "{} is another device's vault; refusing to share it",
                wanted.display()
            )));
        }
        Err(_) => {
            let occupied = match std::fs::read_dir(wanted) {
                Ok(mut entries) => entries.next().is_some(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                // Unreadable is not empty.
                Err(_) => true,
            };
            if occupied {
                return Err(CliError::Usage(format!(
                    "{} is not empty and is no vault of this node's; choose an empty or new directory",
                    wanted.display()
                )));
            }
            std::fs::create_dir_all(wanted).map_err(io(wanted))?;
            std::fs::write(&marker, &mine).map_err(io(&marker))?;
            // The index now, so a later open finds marker and index together.
            itsanas_store::Vault::open(wanted)?;
        }
    }
    node.config.vault = Some(wanted.to_owned());
    node.save_config()?;
    Ok(())
}

/// Where a vault may not go: inside the home or the synced folder, or on the
/// home's disk without `--same-disk`.
pub(crate) fn check_vault_target(node: &Node, wanted: &Path, same_disk: bool) -> Result<()> {
    // Inside the home, the store or the synced folder, other people's sealed
    // chunks would be synced as this account's files, or mixed with its own.
    let inside = |outer: &Path| wanted.starts_with(outer);
    // The default place is the one exception inside the home: it is where a
    // vault starts, and a vault moved away must be able to come back to it
    // (hand test, 2026-10-22: the way back was refused).
    let default_place = wanted == node.home.join("vault");
    if (inside(&node.home) && !default_place) || node.config.folder.as_deref().is_some_and(inside) {
        return Err(CliError::Usage(format!(
            "{} is inside this node's home or synced folder; the vault goes elsewhere",
            wanted.display()
        )));
    }
    // Inside the vault it leaves, the copy would copy itself until the disk
    // filled; around it, the old vault would be inside the new one.
    let vault = node.vault_dir();
    if wanted.starts_with(&vault) || vault.starts_with(wanted) {
        return Err(CliError::Usage(format!(
            "{} is inside the vault at {}, or holds it; the vault moves to a place of its own",
            wanted.display(),
            vault.display()
        )));
    }
    if !same_disk && Node::same_disk(&node.home, wanted).unwrap_or(true) {
        return Err(CliError::Usage(format!(
            "{} is on the same disk as this node's home: if it is a NAS mount point, the NAS is \
             not mounted. Mount it and retry, or add --same-disk if this is meant",
            wanted.display()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    /// A cold copy in a drawer is refreshed now and then: the same directory
    /// again, writing only what it lacks, deleting nothing a person did not
    /// ask to delete, dated, and saying what it left out. It used to refuse
    /// any directory that was not empty, so the person kept a second copy
    /// beside the first, or deleted the first to write the second (HANDOVER
    /// §8 0e step 8). Sabotage: delete stale chunks without `--prune`; or
    /// rewrite every chunk.
    #[test]
    fn red_team_a_backup_is_refreshed_in_place_and_deletes_only_when_pruned() {
        // Bytes that do not repeat, so the chunker cuts several chunks.
        let varied = |seed: u64, len: usize| {
            let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
            (0..len)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    state.to_le_bytes()[0]
                })
                .collect::<Vec<u8>>()
        };
        let base = tempfile::tempdir().expect("temp dir");
        let (laptop, _) =
            itsanas_node::Node::create(&base.path().join("laptop"), "a passphrase", "camille")
                .expect("create");
        let file = varied(1, 300 * 1024);
        laptop.store.write_file("keep.bin", &file).expect("write");
        laptop
            .store
            .write_file("goes.bin", &varied(2, 300 * 1024))
            .expect("write");
        laptop.store.flush_segment().expect("flush");
        let copy = base.path().join("drawer");
        let first = crate::backup::export(&laptop, &copy).expect("first copy");
        // A chunk of an older copy that no file of the account uses any more.
        let gone = copy.join("chunks").join("ab".repeat(32));
        std::fs::write(&gone, b"sealed bytes of a file long deleted").expect("old chunk");

        laptop.store.remove_file("goes.bin").expect("remove");
        laptop
            .store
            .write_file("new.bin", b"written since")
            .expect("write");
        laptop.store.flush_segment().expect("flush");
        let second =
            crate::backup::export(&laptop, &copy).expect("a refresh of this account's copy");
        assert!(
            second.kept > 0,
            "the refresh wrote every chunk again: {second:?}"
        );
        assert!(
            second.chunks >= 1 && second.chunks < first.chunks,
            "{second:?}"
        );
        assert!(
            second.stale > 0,
            "the deleted file's chunks were not counted: {second:?}"
        );
        assert_eq!(
            second.pruned, 0,
            "a refresh without --prune deleted from the copy"
        );
        let on_disk = std::fs::read_dir(copy.join("chunks"))
            .expect("chunks")
            .count();
        assert_eq!(
            on_disk,
            first.chunks + second.chunks + 1,
            "a chunk went missing from the copy without --prune"
        );
        assert!(gone.is_file(), "an old chunk was deleted without --prune");
        assert!(copy.join("written").is_file(), "the copy is not dated");
        assert!(
            copy.join(crate::backup::MARKER).is_file(),
            "the refreshed copy lost its marker"
        );

        let third = crate::backup::export_with(&laptop, &copy, true).expect("pruned");
        assert_eq!(third.pruned, second.stale, "{third:?}");
        assert!(!gone.exists(), "--prune left the chunk no file uses");
        assert!(crate::backup::export_summary(&third, "the drawer").contains("deleted (--prune)"));
    }

    #[test]
    fn red_team_a_cold_copy_rebuilds_the_account_on_a_new_device_and_nowhere_else() {
        // A disk in a drawer: the laptop exports, a new machine recovered
        // from the 24 words restores with no peer anywhere, and gets the
        // files back. A node of another account is refused the same copy
        // before it touches anything.
        let base = tempfile::tempdir().expect("temp dir");
        let (laptop, phrase) =
            itsanas_node::Node::create(&base.path().join("laptop"), "a passphrase", "camille")
                .expect("create");
        laptop
            .store
            .write_file("notes/plan.txt", b"the only copy")
            .expect("write");
        laptop.store.flush_segment().expect("flush");
        let copy = base.path().join("drawer");
        let moved = crate::backup::export(&laptop, &copy).expect("export");
        assert!(moved.chunks > 0 && moved.segments > 0, "{moved:?}");
        let stray = base.path().join("photos");
        std::fs::create_dir_all(&stray).expect("dir");
        std::fs::write(stray.join("holiday.jpg"), b"not a backup").expect("file");
        assert!(
            crate::backup::export(&laptop, &stray).is_err(),
            "an export wrote into a directory of somebody's own files"
        );

        let (stranger, _) =
            itsanas_node::Node::create(&base.path().join("stranger"), "a passphrase", "mallory")
                .expect("create");
        assert!(
            crate::backup::restore(&stranger, &copy).is_err(),
            "another account's node took this account's backup"
        );
        assert!(
            stranger
                .store
                .read_file("notes/plan.txt")
                .expect("read")
                .is_none()
        );

        let rebuilt = itsanas_node::Node::restore(
            &base.path().join("rebuilt"),
            "a passphrase",
            "camille",
            phrase.as_str(),
        )
        .expect("recover from the words");
        // A chunk swapped on the backup disk is refused, not kept.
        let forged = copy.join("chunks").join("ff".repeat(32));
        std::fs::write(&forged, b"not a chunk of this account").expect("forge");
        let (moved, report) = crate::backup::restore(&rebuilt, &copy).expect("restore");
        assert_eq!(
            moved.chunks + 1,
            std::fs::read_dir(copy.join("chunks")).unwrap().count()
        );
        assert_eq!(report.adopted, 1, "{report:?}");
        assert_eq!(
            rebuilt
                .store
                .read_file("notes/plan.txt")
                .expect("read")
                .as_deref(),
            Some(&b"the only copy"[..]),
            "the cold copy did not bring the file back"
        );
    }

    #[test]
    fn red_team_a_vault_moves_onto_an_existing_empty_mount_point_and_never_into_itself() {
        // A mount point or a drive root already exists, empty, and has no
        // sibling on its own disk: the copy is staged inside it and nothing
        // is written beside it. A target inside the vault, or holding it, is
        // refused before anything is copied.
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join("home");
        let (node, _) =
            itsanas_node::Node::create(&home, "a passphrase", "camille").expect("create");
        let mount = base.path().join("mnt").join("usb");
        std::fs::create_dir_all(&mount).expect("mount point");
        let beside = base.path().join("mnt").join("usb.x");
        std::fs::create_dir(&beside).expect("a neighbour");
        super::move_vault(node, &mount, true).expect("move onto the mount point");
        assert!(mount.join(itsanas_node::node::VAULT_MARKER).is_file());
        assert!(!mount.join(super::STAGING).exists(), "staging left behind");
        let siblings: Vec<_> = std::fs::read_dir(mount.parent().expect("parent"))
            .expect("list")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(
            siblings.len(),
            2,
            "something was written beside the mount point: {siblings:?}"
        );
        let node = itsanas_node::Node::open(&home, "a passphrase").expect("reopen");
        let err = super::move_vault(node, &mount.join("sub"), true).expect_err("into itself");
        assert!(err.to_string().contains("inside the vault"), "{err}");
        let node = itsanas_node::Node::open(&home, "a passphrase").expect("reopen");
        assert!(super::check_vault_target(&node, &base.path().join("mnt"), true).is_err());
    }

    #[test]
    fn red_team_a_vault_is_not_moved_onto_a_key_too_small_for_it() {
        let key = std::path::Path::new("/media/key");
        let refused = super::room_for_vault(2_000_000_000, Some(1_000_000_000), key)
            .expect_err("a 2 GB vault onto 1 GB free");
        assert!(refused.to_string().contains("free"), "{refused}");
        assert!(
            super::room_for_vault(2_000_000_000, Some(2_100_000_000), key).is_err(),
            "no margin for the file system's overhead"
        );
        assert!(super::room_for_vault(2_000_000_000, Some(3_000_000_000), key).is_ok());
        assert!(
            super::room_for_vault(2_000_000_000, None, key).is_ok(),
            "unknown: tried"
        );
    }

    #[test]
    fn red_team_a_vault_moved_away_can_come_back_to_its_default_place() {
        // The way back was refused in the hand test of 2026-10-22: the default
        // place is inside the home, which a move refuses for anything else.
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join("home");
        let (node, _) =
            itsanas_node::Node::create(&home, "a passphrase", "camille").expect("create");
        let default_place = node.vault_dir();
        let away = base.path().join("elsewhere").join("vault");
        super::move_vault(node, &away, true).expect("away");
        let node = itsanas_node::Node::open(&home, "a passphrase").expect("reopen");
        assert!(
            super::check_vault_target(&node, &home.join("other"), true).is_err(),
            "anywhere else in the home is still refused"
        );
        std::fs::remove_dir_all(&default_place).expect("the old copy deleted, as advised");
        super::move_vault(node, &default_place, true).expect("back to the default place");
        let node = itsanas_node::Node::open(&home, "a passphrase").expect("reopen");
        assert_eq!(node.vault_dir(), default_place);
    }

    #[test]
    fn red_team_a_backup_is_not_written_into_the_home_vault_or_synced_folder() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join("home");
        let (node, _) =
            itsanas_node::Node::create(&home, "a passphrase", "camille").expect("create");
        for dir in [home.join("backup"), node.vault_dir().join("backup")] {
            assert!(
                crate::backup::export(&node, &dir).is_err(),
                "a backup went into {}",
                dir.display()
            );
            assert!(!dir.exists());
        }
    }

    #[test]
    fn red_team_moving_a_vault_copies_checks_switches_and_keeps_the_original() {
        // Changing disks: the hosted data arrives whole on the new one, the
        // node uses it from then on, and the old copy stays until a person
        // deletes it. A target that already holds something is refused and
        // nothing is switched.
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join("home");
        let (node, _) =
            itsanas_node::Node::create(&home, "a passphrase", "camille").expect("create");
        let owner =
            itsanas_crypto::UserKeys::derive(&itsanas_crypto::MasterSecret::from_bytes([9; 32]))
                .user_id();
        let address = itsanas_crypto::ChunkId::from_bytes([4; 32]);
        node.vault
            .put_chunk(owner, &address, b"someone's sealed chunk")
            .expect("store");
        let old = node.vault_dir();

        let occupied = base.path().join("occupied");
        std::fs::create_dir(&occupied).expect("dir");
        std::fs::write(occupied.join("photo.jpg"), b"x").expect("file");
        assert!(
            super::move_vault(node, &occupied, true).is_err(),
            "a vault was moved into a directory that held something"
        );
        assert!(
            !occupied.join(super::STAGING).exists(),
            "a refused move copied the whole vault before refusing"
        );

        let node = itsanas_node::Node::open(&home, "a passphrase").expect("reopen");
        assert_eq!(
            node.vault_dir(),
            old,
            "a refused move switched the vault anyway"
        );
        let new = base.path().join("new-disk").join("vault");
        std::fs::create_dir_all(new.parent().expect("parent")).expect("disk");
        super::move_vault(node, &new, true).expect("move");

        let node = itsanas_node::Node::open(&home, "a passphrase").expect("reopen");
        assert_eq!(node.vault_dir(), new);
        assert_eq!(
            node.vault
                .get_chunk(owner, &address)
                .expect("read")
                .as_deref(),
            Some(&b"someone's sealed chunk"[..]),
            "the moved vault lost what it held"
        );
        assert!(
            old.join("vault.redb").is_file(),
            "the original was deleted by the move"
        );
        assert!(
            !new.with_extension("itsanas-copy").exists(),
            "the staging copy was left behind"
        );
    }

    /// `itsanas vault PATH` never adopts a directory that holds something
    /// else, nor another device's vault; an empty one is marked as this node's
    /// and the next open uses it.
    #[test]
    fn red_team_setting_the_vault_never_adopts_a_foreign_directory() {
        let base = tempfile::tempdir().expect("temp dir");
        let home = base.path().join("node");
        let (mut node, _) =
            itsanas_node::Node::create(&home, "a long enough passphrase", "alice").expect("node");
        let full = base.path().join("photos");
        std::fs::create_dir_all(&full).expect("dir");
        std::fs::write(full.join("holiday.jpg"), b"x").expect("file");
        let theirs = base.path().join("theirs");
        std::fs::create_dir_all(&theirs).expect("dir");
        std::fs::write(
            theirs.join(itsanas_node::node::VAULT_MARKER),
            "someone-else",
        )
        .expect("m");
        for (path, why) in [
            (&full, "a directory full of photos"),
            (&theirs, "another device's vault"),
        ] {
            assert!(
                super::set_vault(&mut node, path, true).is_err(),
                "{why} was adopted as this node's vault"
            );
        }
        assert_eq!(node.config.vault, None, "a refused path was saved anyway");

        let nas = base.path().join("nas").join("vault");
        assert!(
            super::set_vault(&mut node, &nas, false).is_err(),
            "a path on the home's own disk was taken without --same-disk: an unmounted NAS mount \
             point would become a vault on the local disk"
        );
        assert!(
            super::set_vault(&mut node, &home.join("inside"), true).is_err(),
            "a vault inside the node's home was accepted"
        );
        super::set_vault(&mut node, &nas, true).expect("an empty, new directory");
        drop(node);
        let reopened = itsanas_node::Node::open(&home, "a long enough passphrase").expect("reopen");
        assert_eq!(
            reopened.vault_dir(),
            nas,
            "the node did not use the vault it was given"
        );
    }
}
