//! The operation log, versions and tombstones: every write announced once,
//! the chain linked, and deletes that supersede what they deleted.

use itsanas_crypto::{MasterSecret, UserKeys};
use itsanas_store::{CausalOrder, Operation, validate_chain};
use itsanas_testkit as testkit;

use crate::store_for;

// ---------------------------------------------------------------------------
// The operation log
// ---------------------------------------------------------------------------

#[test]
fn every_write_is_announced_in_the_log_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let alice = testkit::alice();
    let store = store_for(&alice.master, dir.path());

    for file in &alice.files {
        store.write_file(file.path, &file.content).unwrap();
    }
    store.remove_file(alice.files[0].path).unwrap();

    let segment = store.flush_segment().unwrap().expect("pending writes");
    let body = segment
        .open(UserKeys::derive(&alice.master).oplog_root())
        .unwrap();

    let upserts = body
        .entries
        .iter()
        .filter(|e| matches!(e.operation, Operation::Upsert { .. }))
        .count();
    let removes = body
        .entries
        .iter()
        .filter(|e| matches!(e.operation, Operation::Remove { .. }))
        .count();

    assert_eq!(upserts, alice.files.len(), "wrong number of upserts logged");
    assert_eq!(removes, 1, "the delete was not logged");

    let sequences: Vec<u64> = body.entries.iter().map(|e| e.sequence).collect();
    let mut expected: Vec<u64> = (1..=sequences.len() as u64).collect();
    expected.sort_unstable();
    assert_eq!(sequences, expected, "sequence numbers are not dense from 1");

    // A second flush has nothing left to do.
    assert!(
        store.flush_segment().unwrap().is_none(),
        "entries were re-emitted, so a peer would replay them twice"
    );
}

#[test]
fn the_segment_chain_links_up_across_many_flushes() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([26; 32]), dir.path());

    for round in 0..5 {
        store
            .write_file(
                &format!("file-{round}.txt"),
                format!("body {round}").as_bytes(),
            )
            .unwrap();
        store.flush_segment().unwrap().expect("one pending write");
    }

    let chain = store.segments().unwrap();
    assert_eq!(chain.len(), 5);
    validate_chain(&chain).expect("the chain this store built must validate");

    assert_eq!(
        chain[0].previous, None,
        "the first segment has a predecessor"
    );
    for pair in chain.windows(2) {
        assert_eq!(
            pair[1].previous,
            Some(pair[0].segment_id),
            "the chain is broken between two consecutive segments"
        );
        assert!(
            pair[1].first_sequence > pair[0].last_sequence,
            "sequence numbers overlap between segments"
        );
    }
}

#[test]
fn unsealed_writes_survive_a_restart_and_are_announced_afterwards() {
    // Simulates a power cut between a write and the next flush. The entry must
    // not be lost, or the peer never learns the file exists.
    let dir = tempfile::tempdir().unwrap();
    let master = MasterSecret::from_bytes([27; 32]);

    {
        let store = store_for(&master, dir.path());
        store
            .write_file("unannounced.txt", b"written but not flushed")
            .unwrap();
        assert_eq!(store.stats().unwrap().unsealed_entries, 1);
        // Dropped without flushing.
    }

    let store = store_for(&master, dir.path());
    assert_eq!(
        store.stats().unwrap().unsealed_entries,
        1,
        "a pending log entry was lost across a restart"
    );

    let segment = store.flush_segment().unwrap().expect("the pending entry");
    let body = segment
        .open(UserKeys::derive(&master).oplog_root())
        .unwrap();
    assert_eq!(body.entries.len(), 1);
    assert_eq!(body.entries[0].operation.path(), "unannounced.txt");
}

// ---------------------------------------------------------------------------
// Versions and tombstones
// ---------------------------------------------------------------------------

#[test]
fn each_write_advances_this_devices_component_of_the_version() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([40; 32]), dir.path());
    let device = store.device_id();

    let first = store.write_file("a.txt", b"one").unwrap();
    let second = store.write_file("a.txt", b"two").unwrap();

    assert!(
        second.version.get(&device) > first.version.get(&device),
        "a second write did not advance the version, so peers cannot tell \
         which of the two is newer"
    );
    assert_eq!(
        first.version.compare(&second.version),
        CausalOrder::Before,
        "consecutive writes on one device must be causally ordered, never \
         concurrent"
    );
}

#[test]
fn writes_to_different_paths_do_not_make_each_other_look_concurrent() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([41; 32]), dir.path());

    let a = store.write_file("a.txt", b"first").unwrap();
    let b = store.write_file("b.txt", b"second").unwrap();

    // b was written with full knowledge of a, on the same device.
    assert_eq!(a.version.compare(&b.version), CausalOrder::Before);
}

#[test]
fn deleting_leaves_a_tombstone_that_supersedes_the_deleted_version() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([42; 32]), dir.path());

    let entry = store.write_file("doomed.txt", b"content").unwrap();
    assert!(store.remove_file("doomed.txt").unwrap());

    let tombstone = store.tombstone("doomed.txt").unwrap().expect(
        "a delete must leave a tombstone, or an offline device \
                 resurrects the file when it returns",
    );

    assert_eq!(
        entry.version.compare(&tombstone.version),
        CausalOrder::Before,
        "the tombstone does not dominate the version it deleted, so the delete \
         would look concurrent with its own target"
    );
    assert_eq!(store.read_file("doomed.txt").unwrap(), None);
    assert_eq!(store.list().unwrap(), Vec::<String>::new());
}

#[test]
fn recreating_a_deleted_file_supersedes_the_tombstone() {
    // Otherwise the re-creation looks concurrent with its own delete, and the
    // file materialises as a conflict pair against a file that no longer exists.
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([43; 32]), dir.path());

    store.write_file("phoenix.txt", b"first life").unwrap();
    store.remove_file("phoenix.txt").unwrap();
    let tombstone = store.tombstone("phoenix.txt").unwrap().unwrap();

    let reborn = store.write_file("phoenix.txt", b"second life").unwrap();

    assert_eq!(
        tombstone.version.compare(&reborn.version),
        CausalOrder::Before,
        "re-creating a deleted file did not build on the tombstone's version"
    );
    assert_eq!(
        store.tombstone("phoenix.txt").unwrap(),
        None,
        "the tombstone survived the re-creation; the path is now both present \
         and deleted"
    );
    assert_eq!(
        store.read_file("phoenix.txt").unwrap().unwrap(),
        b"second life"
    );
}

#[test]
fn removing_a_file_that_is_not_there_reports_false_and_logs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([44; 32]), dir.path());

    assert!(!store.remove_file("never-existed.txt").unwrap());
    assert_eq!(store.stats().unwrap().unsealed_entries, 0);
    assert_eq!(store.tombstone("never-existed.txt").unwrap(), None);
}
