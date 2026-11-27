//! Deduplication and garbage collection: one copy of identical content,
//! and the grace period before unreferenced chunks go.

use std::time::Duration;

use itsanas_crypto::MasterSecret;
use itsanas_testkit as testkit;

use crate::store_for;

// ---------------------------------------------------------------------------
// Deduplication
// ---------------------------------------------------------------------------

#[test]
fn identical_files_stored_twice_occupy_one_copy_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([22; 32]), dir.path());

    let payload = testkit::filler("dedup", 512 * 1024);
    store.write_file("first.bin", &payload).unwrap();
    let after_first = store.stats().unwrap();

    store.write_file("second.bin", &payload).unwrap();
    let after_second = store.stats().unwrap();

    assert_eq!(
        after_first.bytes_on_disk, after_second.bytes_on_disk,
        "storing the same bytes under a second path consumed more disk; \
         deduplication is not working"
    );
    assert_eq!(after_second.files, 2);
    assert_eq!(
        store.read_file("second.bin").unwrap().unwrap(),
        payload,
        "the deduplicated second file did not read back"
    );
}

#[test]
fn deleting_one_of_two_identical_files_keeps_the_other_readable() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([23; 32]), dir.path());

    let payload = testkit::filler("shared-chunks", 256 * 1024);
    store.write_file("a.bin", &payload).unwrap();
    store.write_file("b.bin", &payload).unwrap();

    store.remove_file("a.bin").unwrap();
    store.collect_garbage(Duration::ZERO).unwrap();

    assert_eq!(
        store.read_file("b.bin").unwrap().unwrap(),
        payload,
        "garbage collection deleted chunks that a surviving file still needs"
    );
}

// ---------------------------------------------------------------------------
// Garbage collection
// ---------------------------------------------------------------------------

#[test]
fn garbage_collection_honours_the_grace_period() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([24; 32]), dir.path());

    let payload = testkit::filler("gc", 256 * 1024);
    store.write_file("doomed.bin", &payload).unwrap();
    let before = store.stats().unwrap().bytes_on_disk;
    assert!(before > 0);

    store.remove_file("doomed.bin").unwrap();

    // Inside the grace period nothing may be deleted: a peer could still be
    // fetching these chunks.
    let report = store.collect_garbage(Duration::from_secs(3600)).unwrap();
    assert_eq!(report.blobs_removed, 0);
    assert!(
        report.retained_in_grace > 0,
        "nothing was queued for collection"
    );
    assert_eq!(
        store.stats().unwrap().bytes_on_disk,
        before,
        "a blob was deleted inside its grace period"
    );

    // With no grace period the same chunks go.
    let report = store.collect_garbage(Duration::ZERO).unwrap();
    assert!(report.blobs_removed > 0, "nothing was collected");
    assert!(report.bytes_reclaimed > 0);
    assert_eq!(
        store.stats().unwrap().bytes_on_disk,
        0,
        "garbage collection left orphaned bytes behind"
    );
}

#[test]
fn overwriting_a_file_eventually_reclaims_the_bytes_it_stopped_using() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([25; 32]), dir.path());

    store
        .write_file("v.bin", &testkit::filler("version-one", 1024 * 1024))
        .unwrap();
    // Entirely different content, so no chunk survives the overwrite.
    store
        .write_file("v.bin", &testkit::filler("version-two", 1024))
        .unwrap();

    let before = store.stats().unwrap().bytes_on_disk;
    store.collect_garbage(Duration::ZERO).unwrap();
    let after = store.stats().unwrap().bytes_on_disk;

    assert!(
        after < before / 10,
        "overwriting a 1 MiB file with a 1 KiB one left {after} bytes on disk \
         (was {before}); superseded chunks are not being reclaimed"
    );
    assert_eq!(store.read_file("v.bin").unwrap().unwrap().len(), 1024);
}
