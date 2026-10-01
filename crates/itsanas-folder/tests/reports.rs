//! What a folder pass tells the person reading it.
//!
//! The store's view of a rename is a deletion and an addition, and that is
//! what replicates. A report that says the same thing reads as "a file was
//! lost and an unknown one appeared", which is how somebody ends up restoring
//! a file they only moved. These tests hold the report to naming the files and
//! to saying "renamed" exactly when the bytes say so -- and never at the cost
//! of what the store actually did.

use std::path::Path;

use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};
use itsanas_folder::{Folder, LINES_IN_A_LOG};
use itsanas_store::{ChunkerConfig, Store};

struct Node {
    _dir: tempfile::TempDir,
    store: Store,
    folder: Folder,
}

fn node(seed: u8) -> Node {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Store::open_for_testing(
        dir.path().join("store"),
        UserKeys::derive(&MasterSecret::from_bytes([0xA1; 32])),
        DeviceKeys::from_seed(&SecretBytes::new([seed; 32])),
        ChunkerConfig::default(),
    )
    .expect("store");
    let folder = Folder::open(dir.path().join("folder")).expect("folder");

    Node {
        _dir: dir,
        store,
        folder,
    }
}

fn write_disk(root: &Path, relative: &str, content: &[u8]) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, content).unwrap();
}

fn rename_disk(root: &Path, from: &str, to: &str) {
    let target = root.join(to);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::rename(root.join(from), target).unwrap();
}

#[test]
fn a_file_renamed_in_the_folder_is_reported_as_one_rename() {
    let node = node(1);
    let root = node.folder.root();
    write_disk(root, "holiday.jpg", b"the bytes of a photograph");
    write_disk(root, "other.txt", b"untouched");
    node.folder.reconcile(&node.store, false).unwrap();

    rename_disk(root, "holiday.jpg", "photos/2026/holiday.jpg");
    let report = node.folder.reconcile(&node.store, false).unwrap();

    assert_eq!(
        report.renamed_here,
        vec![(
            "holiday.jpg".to_owned(),
            "photos/2026/holiday.jpg".to_owned()
        )],
        "a move inside the folder was not recognised: {report:?}"
    );
    assert_eq!(
        report.summary(),
        "0 in, 0 out, 0 deleted locally, 0 deleted remotely, 0 conflicts, 1 renamed",
        "the summary still counts a move as a loss and an addition"
    );
    assert_eq!(
        report.lines(None),
        vec![
            "  mv   holiday.jpg -> photos/2026/holiday.jpg (renamed here, renamed everywhere)"
                .to_owned()
        ],
        "the report names the move as anything but one line"
    );

    // Recognising it changed nothing the store did: the old path is deleted
    // and the new one is there, which is what the other devices will apply.
    assert_eq!(report.removed_from_store, vec!["holiday.jpg".to_owned()]);
    assert_eq!(report.imported, vec!["photos/2026/holiday.jpg".to_owned()]);
    assert!(node.store.stat("holiday.jpg").unwrap().is_none());
    assert_eq!(
        node.store
            .read_file("photos/2026/holiday.jpg")
            .unwrap()
            .as_deref(),
        Some(&b"the bytes of a photograph"[..])
    );
}

#[test]
fn a_rename_made_on_another_device_is_reported_as_one_rename() {
    let node = node(2);
    let root = node.folder.root();
    write_disk(root, "report.odt", b"a document somebody wrote");
    node.folder.reconcile(&node.store, false).unwrap();

    // What the sync engine does when it adopts a peer's rename: the old path
    // deleted, the same bytes under the new one.
    node.store
        .write_file("archive/report.odt", b"a document somebody wrote")
        .unwrap();
    node.store.remove_file("report.odt").unwrap();

    let report = node.folder.reconcile(&node.store, false).unwrap();

    assert_eq!(
        report.renamed_elsewhere,
        vec![("report.odt".to_owned(), "archive/report.odt".to_owned())],
        "a peer's move was not recognised: {report:?}"
    );
    assert_eq!(
        report.lines(None),
        vec![
            "  mv   report.odt -> archive/report.odt (renamed elsewhere, renamed here)".to_owned()
        ]
    );
    assert!(!root.join("report.odt").exists());
    assert_eq!(
        std::fs::read(root.join("archive/report.odt")).unwrap(),
        b"a document somebody wrote"
    );
}

#[test]
fn only_an_unambiguous_move_of_the_same_bytes_is_called_a_rename() {
    let node = node(3);
    let root = node.folder.root();
    // Two identical copies, one file that will be edited while moved, and an
    // empty file: none of them may come out of this pass as a "rename".
    write_disk(root, "copy-a.bin", b"twin content");
    write_disk(root, "copy-b.bin", b"twin content");
    write_disk(root, "draft.txt", b"first version");
    write_disk(root, "empty.log", b"");
    node.folder.reconcile(&node.store, false).unwrap();

    std::fs::remove_file(root.join("copy-a.bin")).unwrap();
    std::fs::remove_file(root.join("copy-b.bin")).unwrap();
    write_disk(root, "copy-c.bin", b"twin content");

    std::fs::remove_file(root.join("draft.txt")).unwrap();
    write_disk(root, "final.txt", b"second version");

    std::fs::remove_file(root.join("empty.log")).unwrap();
    write_disk(root, "unrelated-empty.cfg", b"");

    let report = node.folder.reconcile(&node.store, false).unwrap();

    assert!(
        report.renamed_here.is_empty(),
        "a rename was claimed where the bytes do not say which file became which: {:?}",
        report.renamed_here
    );
    assert_eq!(
        report.summary(),
        "3 in, 0 out, 4 deleted locally, 0 deleted remotely, 0 conflicts"
    );
}

#[test]
fn a_log_names_the_first_files_and_counts_the_rest() {
    let node = node(4);
    let root = node.folder.root();
    let total = LINES_IN_A_LOG + 7;
    for index in 0..total {
        write_disk(
            root,
            &format!("file-{index:03}.txt"),
            format!("{index}").as_bytes(),
        );
    }

    let report = node.folder.reconcile(&node.store, false).unwrap();

    let bounded = report.lines(Some(LINES_IN_A_LOG));
    assert_eq!(
        bounded.len(),
        LINES_IN_A_LOG + 1,
        "the log bound did not hold: {bounded:?}"
    );
    assert_eq!(bounded[0], "  in   file-000.txt");
    assert_eq!(
        bounded.last().unwrap(),
        "  ... and 7 more (`itsanas scan` lists them all)",
        "files past the bound vanished from the log without being counted"
    );
    assert_eq!(
        report.lines(None).len(),
        total,
        "`scan` must list every file"
    );
}

#[test]
fn deleting_one_of_two_copies_is_a_deletion_not_a_rename() {
    // Found by the redteam pass: `a` and `c` hold the same bytes; the user
    // deletes `a` and copies `c` to `b`. One went and one came with those
    // bytes, but `a` was a distinct file and it is gone. "mv a -> b" would
    // hide that deletion from the person reading the report.
    let node = node(5);
    let root = node.folder.root();
    write_disk(root, "a.txt", b"shared bytes");
    write_disk(root, "c.txt", b"shared bytes");
    node.folder.reconcile(&node.store, false).unwrap();

    std::fs::remove_file(root.join("a.txt")).unwrap();
    std::fs::copy(root.join("c.txt"), root.join("b.txt")).unwrap();
    let report = node.folder.reconcile(&node.store, false).unwrap();

    assert!(
        report.renamed_here.is_empty(),
        "a deletion was shown as a rename while another copy still holds the bytes: {:?}",
        report.renamed_here
    );
    assert!(
        report
            .lines(None)
            .contains(&"  del  a.txt (deleted here, will be deleted everywhere)".to_owned()),
        "the deletion of a.txt is not named: {:?}",
        report.lines(None)
    );
}

#[test]
fn a_bounded_log_still_names_every_deletion_and_conflict() {
    // Found by the redteam pass: the bound cut from the tail, so a pass with
    // many imports pushed its deletions and conflicts into "N more".
    let node = node(6);
    let root = node.folder.root();
    write_disk(root, "gone-elsewhere.txt", b"to be deleted by a peer");
    write_disk(root, "contested.txt", b"original");
    node.folder.reconcile(&node.store, false).unwrap();

    for index in 0..LINES_IN_A_LOG + 10 {
        write_disk(
            root,
            &format!("new-{index:03}.txt"),
            format!("n{index}").as_bytes(),
        );
    }
    node.store.remove_file("gone-elsewhere.txt").unwrap();
    // A conflict: changed here and in the store, differently.
    write_disk(root, "contested.txt", b"edited here");
    node.store
        .write_file("contested.txt", b"edited elsewhere")
        .unwrap();

    let report = node.folder.reconcile(&node.store, false).unwrap();
    let bounded = report.lines(Some(LINES_IN_A_LOG));

    assert!(
        bounded
            .iter()
            .any(|line| line.starts_with("  rm   gone-elsewhere.txt")),
        "a deletion vanished into the count of a bounded log: {bounded:?}"
    );
    assert!(
        bounded
            .iter()
            .any(|line| line.starts_with("  !!   contested.txt")),
        "a conflict vanished into the count of a bounded log: {bounded:?}"
    );
    assert!(
        bounded.last().unwrap().starts_with("  ... and "),
        "the routine lines were not bounded: {bounded:?}"
    );
}
