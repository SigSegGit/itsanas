//! A storage that vanished must never read as a deletion.
//!
//! The failure this file is about is not exotic, and it destroys data on every
//! machine of an account at once. An unmounted disk, or a network share that
//! dropped, leaves its mount point behind as an **empty directory**. The scan
//! finds nothing, every file the ledger says this machine holds looks deleted,
//! and those deletions replicate. The disk comes back an hour later with the
//! files still on it, and the account has already agreed they were gone.
//!
//! Nothing in the filesystem distinguishes that from a folder somebody emptied
//! on purpose. So there are two defences, aimed at two different shapes:
//!
//! * A **marker** at the folder root, carrying the device id. It goes away with
//!   the storage it sits on, so its absence beside a ledger full of files means
//!   the storage is not there.
//! * A **guard on the count**, for when the directory really is there and most
//!   of it is not: a half-run restore, a `rm -rf` in the wrong terminal.

use std::path::Path;

use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};
use itsanas_folder::{Folder, scan};
use itsanas_store::Store;

/// A store and a folder that already hold `names`, as a working machine would.
fn machine(home: &Path, folder: &Path, names: &[&str]) -> Store {
    std::fs::create_dir_all(folder).expect("folder");
    let owner = UserKeys::derive(&MasterSecret::from_bytes([7; 32]));
    let device = DeviceKeys::from_seed(&SecretBytes::new([8; 32]));
    let store = Store::open(home, owner, device).expect("store");

    for name in names {
        std::fs::write(folder.join(name), format!("the contents of {name}")).expect("write");
    }
    let opened = Folder::open(folder).expect("open folder");
    let report = opened.reconcile(&store, false).expect("first pass");
    assert_eq!(
        report.imported.len(),
        names.len(),
        "the fixture must import"
    );
    store
}

/// THE ACCIDENT: the disk is not mounted, so the mount point is an empty
/// directory. Every file looks deleted. Without a marker this pass writes those
/// deletions into the log, and they replicate to every machine of the account.
#[test]
fn red_team_an_unmounted_folder_writes_no_deletion_at_all() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().join("node");
    let folder = dir.path().join("ITSaNAS");
    let store = machine(&home, &folder, &["a.txt", "b.txt", "c.txt"]);

    // The disk goes away: the mount point is left behind, empty. The marker
    // was on the disk, so it goes with it.
    for entry in std::fs::read_dir(&folder).expect("read") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            std::fs::remove_dir_all(&path).expect("remove dir");
        } else {
            std::fs::remove_file(&path).expect("remove file");
        }
    }

    let opened = Folder::open(&folder).expect("open folder");
    let outcome = opened.reconcile(&store, false);

    assert!(
        outcome.is_err(),
        "an empty mount point was read as three deletions, which replicate to \
         every machine of this account"
    );
    let said = outcome.unwrap_err().to_string();
    assert!(
        said.contains("storage unreachable"),
        "the refusal must name the cause, or somebody will 'fix' it by deleting \
         the ledger; it said {said:?}"
    );

    // And nothing was written: the account still holds all three.
    let still = store.local_states().expect("ledger");
    assert_eq!(
        still.len(),
        3,
        "the ledger lost files to a folder that was merely not mounted"
    );
}

/// The other shape: the directory is there, the marker with it, and most of the
/// files are not. A restore that wrote into the wrong place, a sync client that
/// half-ran. Deletions replicate, so guessing wrong costs the account's copies
/// and not only this machine's.
#[test]
fn red_team_a_folder_that_emptied_itself_has_its_deletions_held() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().join("node");
    let folder = dir.path().join("ITSaNAS");
    let names = [
        "a.txt", "b.txt", "c.txt", "d.txt", "e.txt", "f.txt", "g.txt",
    ];
    let store = machine(&home, &folder, &names);

    // Six of seven gone, and the marker still there: the storage is present.
    for name in &names[..6] {
        std::fs::remove_file(folder.join(name)).expect("remove");
    }
    assert!(
        folder.join(scan::MARKER).exists(),
        "the fixture is wrong: this test is about a folder that is still there"
    );

    let opened = Folder::open(&folder).expect("open folder");
    let report = opened.reconcile(&store, false).expect("the pass runs");

    assert_eq!(
        report.removed_from_store.len(),
        0,
        "six deletions out of seven were written without anybody looking"
    );
    assert_eq!(report.held_deletions, 6);
    assert!(report.held_anything());

    let still = store.local_states().expect("ledger");
    assert_eq!(still.len(), 7, "the account lost files to a held pass");

    // And when somebody does look, it goes through.
    let confirmed = opened
        .reconcile_confirmed(&store, false)
        .expect("confirmed pass");
    assert_eq!(
        confirmed.removed_from_store.len(),
        6,
        "`itsanas folder --confirm` must be able to say the files really are gone"
    );
}

/// An ordinary tidy-up is not an accident. A guard that held every deletion
/// would teach its owner to pass `--confirm` out of habit, which is how a guard
/// becomes a formality.
#[test]
fn deleting_a_few_files_is_an_ordinary_thing_to_do() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().join("node");
    let folder = dir.path().join("ITSaNAS");
    let names = [
        "a.txt", "b.txt", "c.txt", "d.txt", "e.txt", "f.txt", "g.txt",
    ];
    let store = machine(&home, &folder, &names);

    for name in &names[..3] {
        std::fs::remove_file(folder.join(name)).expect("remove");
    }

    let opened = Folder::open(&folder).expect("open folder");
    let report = opened.reconcile(&store, false).expect("the pass runs");

    assert_eq!(
        report.removed_from_store.len(),
        3,
        "deleting three files of seven is a Tuesday and must just work"
    );
    assert_eq!(report.held_deletions, 0);
}

/// Two nodes pointed at one directory is the other way a folder empties itself:
/// each one deletes what the other wrote. The marker names a device, so the
/// second node can tell.
#[test]
fn red_team_a_folder_that_belongs_to_another_node_is_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().join("node");
    let folder = dir.path().join("ITSaNAS");
    let store = machine(&home, &folder, &["a.txt"]);

    // Somebody else's marker, on a folder that otherwise looks fine.
    std::fs::write(
        folder.join(scan::MARKER),
        DeviceKeys::from_seed(&SecretBytes::new([99; 32]))
            .device_id()
            .to_hex(),
    )
    .expect("write marker");

    let opened = Folder::open(&folder).expect("open folder");
    let said = opened
        .reconcile(&store, false)
        .expect_err("a folder owned by another device must be refused")
        .to_string();

    assert!(
        said.contains("storage unreachable") && said.contains("marker names device"),
        "the refusal must say whose folder this is; it said {said:?}"
    );
}

/// Upgrading must not stop anybody: a folder that predates markers, whose files
/// are present, gets one and carries on.
#[test]
fn a_folder_from_before_markers_is_adopted_rather_than_refused() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().join("node");
    let folder = dir.path().join("ITSaNAS");
    let store = machine(&home, &folder, &["a.txt", "b.txt"]);

    std::fs::remove_file(folder.join(scan::MARKER)).expect("remove the marker");

    let opened = Folder::open(&folder).expect("open folder");
    let report = opened
        .reconcile(&store, false)
        .expect("an existing folder must keep working across an upgrade");

    assert_eq!(report.removed_from_store.len(), 0);
    assert!(
        folder.join(scan::MARKER).exists(),
        "the folder must be given a marker, or it is defenceless at the next pass"
    );
}

/// The marker is this system's bookkeeping. Syncing it would send one machine's
/// device id to every other machine, where it would name the wrong device and
/// make every folder look foreign.
#[test]
fn the_marker_is_never_synced() {
    let dir = tempfile::tempdir().expect("temp dir");
    let home = dir.path().join("node");
    let folder = dir.path().join("ITSaNAS");
    let store = machine(&home, &folder, &["a.txt"]);

    let listed = store.local_states().expect("ledger");
    assert!(
        !listed.iter().any(|(path, _)| path.contains(scan::MARKER)),
        "the marker was taken into the account as a file"
    );
}
