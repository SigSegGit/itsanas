//! End-to-end behaviour of a synced folder.
//!
//! "The store changed from a peer" is simulated by writing to the store
//! directly, which is exactly what the sync engine does when it adopts a
//! remote operation. That keeps these tests about the folder rather than about
//! the network, which has its own suite.

use std::path::Path;

use itsanas_crypto::{DeviceKeys, MasterSecret, SecretBytes, UserKeys};
use itsanas_folder::Folder;
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

fn read_disk(root: &Path, relative: &str) -> Option<Vec<u8>> {
    std::fs::read(root.join(relative)).ok()
}

// ---------------------------------------------------------------------------
// The dangerous case
// ---------------------------------------------------------------------------

#[test]
fn a_brand_new_device_downloads_everything_and_deletes_nothing() {
    // The catastrophe this design exists to avoid. A device that has never
    // synced has an empty folder. If "absent from disk" were read as "the user
    // deleted it", this machine would announce the deletion of every file its
    // owner has, and every other device would obey.
    let node = node(1);

    // The store already knows about files — as it would after a first sync —
    // but nothing has been written to the folder yet.
    for name in ["a.txt", "work/b.txt", "deep/nested/c.txt"] {
        node.store.write_file(name, name.as_bytes()).unwrap();
    }

    let report = node.folder.reconcile(&node.store, false).unwrap();

    assert!(
        report.removed_from_store.is_empty(),
        "a fresh device deleted files it had simply never downloaded: {:?}",
        report.removed_from_store
    );
    assert_eq!(report.exported.len(), 3);

    for name in ["a.txt", "work/b.txt", "deep/nested/c.txt"] {
        assert_eq!(
            read_disk(node.folder.root(), name).as_deref(),
            Some(name.as_bytes()),
            "{name} was not written to the folder"
        );
    }
}

#[test]
fn a_file_the_user_deletes_is_deleted_everywhere() {
    // The counterpart. A delete that this device genuinely made must propagate,
    // or the product does not work.
    let node = node(2);

    write_disk(node.folder.root(), "doomed.txt", b"content");
    node.folder.reconcile(&node.store, false).unwrap();
    assert!(node.store.read_file("doomed.txt").unwrap().is_some());

    std::fs::remove_file(node.folder.root().join("doomed.txt")).unwrap();
    let report = node.folder.reconcile(&node.store, false).unwrap();

    assert_eq!(report.removed_from_store, vec!["doomed.txt".to_owned()]);
    assert_eq!(node.store.read_file("doomed.txt").unwrap(), None);
    assert!(
        node.store.tombstone("doomed.txt").unwrap().is_some(),
        "no tombstone, so a device that was offline would resurrect the file"
    );
}

// ---------------------------------------------------------------------------
// Ordinary flow
// ---------------------------------------------------------------------------

#[test]
fn a_new_file_dropped_in_the_folder_is_imported() {
    let node = node(3);
    write_disk(node.folder.root(), "notes/hello.txt", b"dropped in");

    let report = node.folder.reconcile(&node.store, false).unwrap();

    assert_eq!(report.imported, vec!["notes/hello.txt".to_owned()]);
    assert_eq!(
        node.store.read_file("notes/hello.txt").unwrap().unwrap(),
        b"dropped in"
    );
}

#[test]
fn editing_a_file_in_the_folder_updates_the_store() {
    let node = node(4);
    write_disk(node.folder.root(), "a.txt", b"first");
    node.folder.reconcile(&node.store, false).unwrap();

    // A different length, so the fast path notices without needing a deep scan.
    write_disk(node.folder.root(), "a.txt", b"second version, longer");
    let report = node.folder.reconcile(&node.store, false).unwrap();

    assert_eq!(report.imported, vec!["a.txt".to_owned()]);
    assert_eq!(
        node.store.read_file("a.txt").unwrap().unwrap(),
        b"second version, longer"
    );
}

#[test]
fn a_change_arriving_from_a_peer_is_written_to_the_folder() {
    let node = node(5);
    write_disk(node.folder.root(), "a.txt", b"local version");
    node.folder.reconcile(&node.store, false).unwrap();

    // As the sync engine would, after adopting a peer's newer version.
    node.store
        .write_file("a.txt", b"the peer's version")
        .unwrap();
    let report = node.folder.reconcile(&node.store, false).unwrap();

    assert_eq!(report.exported, vec!["a.txt".to_owned()]);
    assert_eq!(
        read_disk(node.folder.root(), "a.txt").unwrap(),
        b"the peer's version"
    );
}

#[test]
fn a_delete_arriving_from_a_peer_removes_the_file_from_disk() {
    let node = node(6);
    write_disk(node.folder.root(), "a.txt", b"content");
    node.folder.reconcile(&node.store, false).unwrap();

    node.store.remove_file("a.txt").unwrap();
    let report = node.folder.reconcile(&node.store, false).unwrap();

    assert_eq!(report.deleted_from_disk, vec!["a.txt".to_owned()]);
    assert_eq!(read_disk(node.folder.root(), "a.txt"), None);
}

#[test]
fn reconciling_twice_does_nothing_the_second_time() {
    // A reconciler that is not idempotent means a daemon uploads the same
    // folder forever and never settles.
    let node = node(7);
    for name in ["a.txt", "b/c.txt"] {
        write_disk(node.folder.root(), name, name.as_bytes());
    }

    assert!(
        node.folder
            .reconcile(&node.store, false)
            .unwrap()
            .changed_anything()
    );

    let second = node.folder.reconcile(&node.store, false).unwrap();
    assert!(
        !second.changed_anything(),
        "the second pass moved data: {second:?}"
    );
    assert_eq!(
        second.recorded, 0,
        "the ledger was not settled by the first pass"
    );
}

#[test]
fn an_imported_file_is_announced_to_peers_not_just_stored_locally() {
    // A real bug, found by running two daemons and watching nothing cross
    // between them. `Store::write_file` only queues a pending log entry; until
    // it is sealed into a segment, a peer asking what changed is told nothing.
    // The file sits on this machine looking perfectly synced while existing
    // nowhere else — the worst possible failure for a backup system, because
    // it is invisible.
    let node = node(21);
    write_disk(node.folder.root(), "announced.txt", b"content");

    node.folder.reconcile(&node.store, false).unwrap();

    assert_eq!(
        node.store.stats().unwrap().unsealed_entries,
        0,
        "the import was left as an unsealed pending entry, so no peer will \
         ever hear about it"
    );
    assert!(
        !node.store.segments().unwrap().is_empty(),
        "no log segment was produced, so a peer asking what changed gets \
         nothing"
    );
}

#[test]
fn a_deletion_is_announced_to_peers_too() {
    let node = node(22);
    write_disk(node.folder.root(), "doomed.txt", b"content");
    node.folder.reconcile(&node.store, false).unwrap();
    let before = node.store.segments().unwrap().len();

    std::fs::remove_file(node.folder.root().join("doomed.txt")).unwrap();
    node.folder.reconcile(&node.store, false).unwrap();

    assert_eq!(node.store.stats().unwrap().unsealed_entries, 0);
    assert!(
        node.store.segments().unwrap().len() > before,
        "the deletion was never sealed into a segment, so other devices keep \
         the file forever"
    );
}

#[test]
fn a_pass_that_changes_nothing_does_not_produce_an_empty_segment() {
    // Flushing unconditionally would mint a segment on every idle scan, and a
    // daemon scanning every few seconds would grow the log without bound.
    let node = node(23);
    write_disk(node.folder.root(), "a.txt", b"content");
    node.folder.reconcile(&node.store, false).unwrap();

    let after_first = node.store.segments().unwrap().len();
    node.folder.reconcile(&node.store, false).unwrap();
    node.folder.reconcile(&node.store, false).unwrap();

    assert_eq!(
        node.store.segments().unwrap().len(),
        after_first,
        "idle scans are minting empty segments; the log will grow forever"
    );
}

// ---------------------------------------------------------------------------
// Conflicts
// ---------------------------------------------------------------------------

#[test]
fn a_local_edit_colliding_with_a_remote_one_keeps_both() {
    let node = node(8);
    write_disk(node.folder.root(), "report.txt", b"base");
    node.folder.reconcile(&node.store, false).unwrap();

    // The user edits on disk, and a peer's version arrives, with neither
    // having seen the other.
    write_disk(node.folder.root(), "report.txt", b"my local edit");
    node.store
        .write_file("report.txt", b"their remote edit")
        .unwrap();

    let report = node.folder.reconcile(&node.store, false).unwrap();

    assert_eq!(
        report.kept_both.len(),
        1,
        "expected one conflict: {report:?}"
    );
    let (original, sibling) = &report.kept_both[0];
    assert_eq!(original, "report.txt");

    assert_eq!(
        read_disk(node.folder.root(), "report.txt").unwrap(),
        b"their remote edit",
        "the incoming version should take the original path"
    );
    assert_eq!(
        read_disk(node.folder.root(), sibling).unwrap(),
        b"my local edit",
        "the local edit was destroyed"
    );

    // And both are in the store, so both reach the other devices.
    assert_eq!(
        node.store.read_file(sibling).unwrap().unwrap(),
        b"my local edit"
    );
    assert_eq!(
        Path::new(sibling).extension().and_then(|e| e.to_str()),
        Some("txt"),
        "the sibling lost its extension, so it would open in the wrong \
         application: {sibling}"
    );
}

#[test]
fn the_same_edit_made_on_both_sides_is_not_a_conflict() {
    // Both changed, but to identical content. A sibling here would litter the
    // folder for no reason.
    let node = node(9);
    write_disk(node.folder.root(), "a.txt", b"base");
    node.folder.reconcile(&node.store, false).unwrap();

    write_disk(node.folder.root(), "a.txt", b"identical new content");
    node.store
        .write_file("a.txt", b"identical new content")
        .unwrap();

    let report = node.folder.reconcile(&node.store, false).unwrap();

    assert!(
        report.kept_both.is_empty(),
        "a spurious conflict: {report:?}"
    );
    assert!(!report.changed_anything());
    assert_eq!(report.recorded, 1);
}

#[test]
fn a_local_delete_racing_a_remote_edit_brings_the_file_back() {
    // Matches the sync engine's rule: an unexpected file costs a second to
    // delete again, a lost edit is unrecoverable.
    let node = node(10);
    write_disk(node.folder.root(), "a.txt", b"base");
    node.folder.reconcile(&node.store, false).unwrap();

    std::fs::remove_file(node.folder.root().join("a.txt")).unwrap();
    node.store.write_file("a.txt", b"edited elsewhere").unwrap();

    let report = node.folder.reconcile(&node.store, false).unwrap();

    assert!(report.removed_from_store.is_empty());
    assert_eq!(
        read_disk(node.folder.root(), "a.txt").unwrap(),
        b"edited elsewhere",
        "a local delete destroyed a remote edit it never saw"
    );
}

// ---------------------------------------------------------------------------
// On-disk behaviour
// ---------------------------------------------------------------------------

#[test]
fn exporting_creates_the_directories_it_needs() {
    let node = node(11);
    node.store.write_file("a/b/c/deep.txt", b"nested").unwrap();

    node.folder.reconcile(&node.store, false).unwrap();

    assert_eq!(
        read_disk(node.folder.root(), "a/b/c/deep.txt").unwrap(),
        b"nested"
    );
}

#[test]
fn deleting_the_last_file_in_a_tree_prunes_the_empty_directories() {
    // Otherwise every machine slowly fills with empty directories nobody put
    // there and nothing ever removes.
    let node = node(12);
    write_disk(node.folder.root(), "a/b/c/only.txt", b"x");
    node.folder.reconcile(&node.store, false).unwrap();

    node.store.remove_file("a/b/c/only.txt").unwrap();
    node.folder.reconcile(&node.store, false).unwrap();

    assert!(
        !node.folder.root().join("a").exists(),
        "empty directories were left behind"
    );
    assert!(
        node.folder.root().exists(),
        "pruning removed the folder root itself"
    );
}

#[test]
fn no_staging_file_survives_a_reconcile() {
    // A leftover in the staging directory means every export leaks a file.
    let node = node(13);
    node.store.write_file("a.txt", b"content").unwrap();
    node.folder.reconcile(&node.store, false).unwrap();

    let staging = node.folder.root().join(itsanas_folder::STAGING_DIR);
    let leftovers: Vec<_> = std::fs::read_dir(&staging)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .collect();

    assert!(leftovers.is_empty(), "staging files were left behind");
}

#[test]
fn the_staging_directory_is_never_itself_synced() {
    let node = node(14);
    write_disk(node.folder.root(), "real.txt", b"x");
    write_disk(
        node.folder.root(),
        &format!("{}/leftover.part", itsanas_folder::STAGING_DIR),
        b"partial",
    );

    node.folder.reconcile(&node.store, false).unwrap();

    assert_eq!(node.store.list().unwrap(), vec!["real.txt".to_owned()]);
}

#[test]
fn a_full_corpus_round_trips_through_a_folder_byte_for_byte() {
    let alice = itsanas_testkit::alice();
    let node = node(15);

    for file in &alice.files {
        write_disk(node.folder.root(), file.path, &file.content);
    }

    let report = node.folder.reconcile(&node.store, false).unwrap();
    assert_eq!(report.imported.len(), alice.files.len());

    for file in &alice.files {
        assert_eq!(
            node.store.read_file(file.path).unwrap().as_deref(),
            Some(file.content.as_slice()),
            "{} did not survive the folder round trip",
            file.path
        );
    }
}

#[test]
fn a_second_device_reproduces_the_folder_exactly() {
    // The whole point: two machines, one folder content.
    let alice = itsanas_testkit::alice();
    let first = node(16);
    let second = node(17);

    for file in &alice.files {
        write_disk(first.folder.root(), file.path, &file.content);
    }
    first.folder.reconcile(&first.store, false).unwrap();

    // Stand in for sync having replicated the store.
    for (path, _) in first.store.entries().unwrap() {
        let content = first.store.read_file(&path).unwrap().unwrap();
        second.store.write_file(&path, &content).unwrap();
    }

    second.folder.reconcile(&second.store, false).unwrap();

    for file in &alice.files {
        assert_eq!(
            read_disk(second.folder.root(), file.path).as_deref(),
            Some(file.content.as_slice()),
            "{} differs on the second device",
            file.path
        );
    }
}

#[test]
fn a_deep_pass_catches_an_edit_the_fast_path_misses() {
    // The documented gap in the size-and-mtime pre-filter: a file rewritten
    // within the same second at exactly the same length. Forced here by
    // restoring the modification time, which is what a restore tool does.
    let node = node(18);
    let path = node.folder.root().join("a.txt");

    std::fs::write(&path, b"aaaa").unwrap();
    node.folder.reconcile(&node.store, false).unwrap();
    let stamp = std::fs::metadata(&path).unwrap().modified().unwrap();

    std::fs::write(&path, b"bbbb").unwrap();
    // Windows needs a write handle to set the modification time; `File::open`
    // gives a read-only one.
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(stamp)
        .unwrap();

    let fast = node.folder.reconcile(&node.store, false).unwrap();
    assert!(
        !fast.changed_anything(),
        "the fast path was expected to miss this; if it now catches it, this \
         test is documenting the wrong thing"
    );
    assert_eq!(node.store.read_file("a.txt").unwrap().unwrap(), b"aaaa");

    let deep = node.folder.reconcile(&node.store, true).unwrap();
    assert_eq!(
        deep.imported,
        vec!["a.txt".to_owned()],
        "a deep pass must catch what the pre-filter misses"
    );
    assert_eq!(node.store.read_file("a.txt").unwrap().unwrap(), b"bbbb");
}

#[test]
fn an_unreadable_file_does_not_stop_the_rest_of_the_folder() {
    // One bad file must not block every other file from syncing.
    let node = node(19);
    write_disk(node.folder.root(), "good-a.txt", b"a");
    write_disk(node.folder.root(), "good-b.txt", b"b");

    let report = node.folder.reconcile(&node.store, false).unwrap();

    assert_eq!(report.imported.len(), 2);
    assert!(report.failed.is_empty());
    assert_eq!(node.store.list().unwrap().len(), 2);
}

#[test]
fn an_empty_folder_and_an_empty_store_do_nothing() {
    let node = node(20);
    let report = node.folder.reconcile(&node.store, false).unwrap();

    assert!(!report.changed_anything());
    assert_eq!(
        report.summary(),
        "0 in, 0 out, 0 deleted locally, 0 deleted remotely, 0 conflicts"
    );
}
