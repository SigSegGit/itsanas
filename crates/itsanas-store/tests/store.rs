//! End-to-end tests for the local store.
//!
//! These are the M2 exit criteria from `docs/ROADMAP.md`, plus the adversarial
//! cases that would make the guarantee in `README.md` a lie if they regressed.
//! Every test here is meant to fail loudly for a specific, nameable reason —
//! if one of them starts passing for a different reason than it was written
//! for, it is a bad test and should be rewritten.

use std::{collections::HashSet, fs, path::Path, time::Duration};

use itsanas_crypto::{ChunkId, DeviceKeys, MasterSecret, UserKeys};
use itsanas_store::{CausalOrder, ChunkerConfig, Operation, Store, validate_chain};
use itsanas_testkit as testkit;

/// Open a store, permitting the published fixture identities.
///
/// Most of these tests deliberately use Alice and Bob, whose recovery phrases
/// are printed in the documentation. `Store::open` refuses those identities —
/// see `the_published_test_identities_are_refused_by_the_normal_constructor` —
/// so the fixtures go through the explicitly-named testing constructor.
fn store_for(master: &MasterSecret, root: &Path) -> Store {
    Store::open_for_testing(
        root,
        UserKeys::derive(master),
        DeviceKeys::generate().expect("device key"),
        ChunkerConfig::default(),
    )
    .expect("store opens")
}

/// A peer cannot make this node decrypt a frame it knows is not a chunk.
///
/// The wire allows eight megabytes; the chunker never emits more than 256 KiB.
/// A repair round asks for thirty-two chunks, so a peer answering every one at
/// the frame limit would have this node decrypting a quarter of a gigabyte for
/// a result known in advance from the length alone. Seconds per round on a
/// Raspberry Pi, for free, from anyone recorded as a holder.
#[test]
fn a_reply_too_large_to_be_a_chunk_is_refused_without_decrypting_it() {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = store_for(&testkit::alice().master, dir.path());
    let entry = store
        .write_file("real.bin", b"a genuine chunk")
        .expect("write");
    let address = entry.chunks[0];

    let genuine = store
        .blobs()
        .get(&address)
        .expect("blobs")
        .expect("the chunk this node just wrote");
    std::fs::remove_file(store.blobs().path_of(&address)).expect("lose it");

    let oversized = vec![0u8; (256 * 1024) + 4096];
    assert!(
        !store.restore_chunk(&address, &oversized).expect("restore"),
        "a reply larger than any chunk the chunker can emit was accepted"
    );
    assert!(
        !store.has_chunk(&address),
        "the oversized reply was written"
    );

    // And the real thing still comes back, so the bound is not simply "no".
    assert!(
        store.restore_chunk(&address, &genuine).expect("restore"),
        "the genuine chunk was refused along with the rubbish"
    );
    assert_eq!(
        store.read_file("real.bin").expect("read").as_deref(),
        Some(&b"a genuine chunk"[..])
    );
}

#[test]
fn the_published_test_identities_are_refused_by_the_normal_constructor() {
    // README.md and SECURITY.md both promise this. Before this test existed the
    // check was defined, exported, and called by nothing.
    for user in testkit::everyone() {
        let dir = tempfile::tempdir().unwrap();
        let opened = Store::open(
            dir.path(),
            UserKeys::derive(&user.master),
            DeviceKeys::generate().unwrap(),
        );

        assert!(
            opened.is_err(),
            "{} opened a normal store despite having a published recovery \
             phrase; real data stored under it would be readable by anyone",
            user.username
        );
    }

    // An ordinary identity is unaffected.
    let dir = tempfile::tempdir().unwrap();
    assert!(
        Store::open(
            dir.path(),
            UserKeys::derive(&MasterSecret::from_bytes([99; 32])),
            DeviceKeys::generate().unwrap(),
        )
        .is_ok(),
        "the ban list rejected an ordinary identity"
    );
}

/// Every byte on disk under `root`, concatenated. Used for leak scans.
fn all_bytes_under(root: &Path) -> Vec<u8> {
    fn walk(directory: &Path, out: &mut Vec<u8>) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if let Ok(bytes) = fs::read(&path) {
                out.extend_from_slice(&bytes);
            }
        }
    }

    let mut out = Vec::new();
    walk(root, &mut out);
    out
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.len() <= haystack.len() && haystack.windows(needle.len()).any(|w| w == needle)
}

// ---------------------------------------------------------------------------
// M2 exit criteria
// ---------------------------------------------------------------------------

#[test]
fn alices_entire_corpus_round_trips_byte_identical() {
    let alice = testkit::alice();
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&alice.master, dir.path());

    for file in &alice.files {
        store.write_file(file.path, &file.content).unwrap();
    }

    for file in &alice.files {
        let read_back = store
            .read_file(file.path)
            .unwrap()
            .unwrap_or_else(|| panic!("{} vanished from the store", file.path));

        assert_eq!(
            read_back.len(),
            file.content.len(),
            "{}: length changed on the round trip",
            file.path
        );
        assert!(
            read_back == file.content,
            "{}: content changed on the round trip",
            file.path
        );
    }

    let mut listed = store.list().unwrap();
    listed.sort();
    let mut expected: Vec<String> = alice.files.iter().map(|f| f.path.to_owned()).collect();
    expected.sort();
    assert_eq!(listed, expected, "the store lost or invented a path");
}

#[test]
fn no_users_plaintext_ever_touches_the_disk() {
    // The single most important property in the project. Both canaries are
    // checked against both stores: a user's own store must not leak their
    // plaintext either, because that store lives on a laptop that can be stolen.
    let alice = testkit::alice();
    let bob = testkit::bob();

    let alice_dir = tempfile::tempdir().unwrap();
    let bob_dir = tempfile::tempdir().unwrap();

    let alice_store = store_for(&alice.master, alice_dir.path());
    let bob_store = store_for(&bob.master, bob_dir.path());

    for file in &alice.files {
        alice_store.write_file(file.path, &file.content).unwrap();
    }
    for file in &bob.files {
        bob_store.write_file(file.path, &file.content).unwrap();
    }
    alice_store.flush_segment().unwrap();
    bob_store.flush_segment().unwrap();

    let alice_disk = all_bytes_under(alice_dir.path());
    let bob_disk = all_bytes_under(bob_dir.path());

    assert!(
        !alice_disk.is_empty() && !bob_disk.is_empty(),
        "the leak scan read nothing, so it proves nothing"
    );

    for (label, disk) in [("Alice's", &alice_disk), ("Bob's", &bob_disk)] {
        for (whose, canary) in [
            ("Alice's", testkit::ALICE_CANARY),
            ("Bob's", testkit::BOB_CANARY),
        ] {
            assert!(
                !contains(disk, canary.as_bytes()),
                "{whose} canary was found in plaintext inside {label} store; \
                 encryption at rest is not working"
            );
        }
    }

    // The canary must genuinely be in the plaintext, or the scan above is
    // vacuous — it would "pass" against a store containing nothing at all.
    let alice_plaintext: Vec<u8> = alice.files.iter().flat_map(|f| f.content.clone()).collect();
    assert!(
        contains(&alice_plaintext, testkit::ALICE_CANARY.as_bytes()),
        "the canary is not present in Alice's plaintext, so the leak scan \
         cannot detect anything"
    );
}

#[test]
fn an_insertion_at_the_start_of_a_large_file_reuses_almost_every_chunk() {
    // Without content-defined chunking this test finds zero reuse and every
    // edit costs a full re-upload.
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([21; 32]), dir.path());

    let original = testkit::filler("m2-insertion", 4 * 1024 * 1024);
    store.write_file("big.bin", &original).unwrap();
    let before: HashSet<ChunkId> = store
        .stat("big.bin")
        .unwrap()
        .unwrap()
        .chunks
        .into_iter()
        .collect();

    let mut edited = Vec::with_capacity(original.len() + 1);
    edited.push(b'!');
    edited.extend_from_slice(&original);
    store.write_file("big.bin", &edited).unwrap();

    let after: HashSet<ChunkId> = store
        .stat("big.bin")
        .unwrap()
        .unwrap()
        .chunks
        .into_iter()
        .collect();

    let reused = before.intersection(&after).count();
    let total = before.len();

    assert!(
        reused * 10 > total * 9,
        "only {reused} of {total} chunks survived a one-byte prefix insertion"
    );
    assert_eq!(
        store.read_file("big.bin").unwrap().unwrap(),
        edited,
        "the edited file did not read back correctly"
    );
}

// ---------------------------------------------------------------------------
// Confidentiality between users
// ---------------------------------------------------------------------------

#[test]
fn two_users_storing_the_same_document_produce_unrelated_chunk_ids() {
    // If addresses were plain content hashes, a host could tell that two of its
    // peers hold the same file — and confirm a guessed file by hashing it.
    let alice = testkit::alice();
    let bob = testkit::bob();

    let alice_dir = tempfile::tempdir().unwrap();
    let bob_dir = tempfile::tempdir().unwrap();

    let alice_store = store_for(&alice.master, alice_dir.path());
    let bob_store = store_for(&bob.master, bob_dir.path());

    alice_store
        .write_file("shared.txt", testkit::SHARED_DOCUMENT)
        .unwrap();
    bob_store
        .write_file("shared.txt", testkit::SHARED_DOCUMENT)
        .unwrap();

    let alice_chunks: HashSet<ChunkId> = alice_store
        .stat("shared.txt")
        .unwrap()
        .unwrap()
        .chunks
        .into_iter()
        .collect();
    let bob_chunks: HashSet<ChunkId> = bob_store
        .stat("shared.txt")
        .unwrap()
        .unwrap()
        .chunks
        .into_iter()
        .collect();

    assert!(
        alice_chunks.is_disjoint(&bob_chunks),
        "two users storing identical bytes produced the same chunk address; a \
         host can now correlate users and confirm guessed content"
    );
    assert!(!alice_chunks.is_empty(), "nothing was stored");
}

#[test]
fn one_users_store_cannot_be_opened_with_another_users_keys() {
    let dir = tempfile::tempdir().unwrap();
    let alice = testkit::alice();

    {
        let store = store_for(&alice.master, dir.path());
        store
            .write_file("secret.txt", b"alice's private notes")
            .unwrap();
    }

    // Same directory, Bob's keys: the index is readable (it is local metadata)
    // but the content must not be.
    let bob = testkit::bob();
    let impostor = store_for(&bob.master, dir.path());

    match impostor.read_file("secret.txt") {
        Err(_) => {}
        Ok(other) => panic!(
            "another user's keys read the store and got {other:?}; sealing is \
             not bound to the owner"
        ),
    }
}

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

// ---------------------------------------------------------------------------
// Integrity and tamper detection
// ---------------------------------------------------------------------------

#[test]
fn a_healthy_store_reports_healthy() {
    let dir = tempfile::tempdir().unwrap();
    let alice = testkit::alice();
    let store = store_for(&alice.master, dir.path());

    for file in &alice.files {
        store.write_file(file.path, &file.content).unwrap();
    }
    store.flush_segment().unwrap();

    let report = store.verify_integrity(true).unwrap();
    assert!(
        report.is_healthy(),
        "a freshly written store reported problems: {report:?}"
    );
    assert_eq!(report.files_checked, alice.files.len());
    assert!(
        report.orphan_blobs.is_empty(),
        "writing files left orphaned blobs behind: {:?}",
        report.orphan_blobs
    );
}

#[test]
fn a_deleted_blob_is_reported_rather_than_silently_returning_short_data() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([28; 32]), dir.path());

    let payload = testkit::filler("victim", 512 * 1024);
    store.write_file("victim.bin", &payload).unwrap();

    let chunks = store.stat("victim.bin").unwrap().unwrap().chunks;
    assert!(chunks.len() > 1, "need a multi-chunk file for this test");
    store.blobs().remove(&chunks[0]).unwrap();

    let report = store.verify_integrity(false).unwrap();
    assert!(!report.is_healthy());
    assert_eq!(report.missing_chunks.len(), 1);
    assert_eq!(report.missing_chunks[0].1, chunks[0]);

    assert!(
        store.read_file("victim.bin").is_err(),
        "a file with a missing chunk read back successfully; the caller would \
         have silently received truncated data"
    );
}

#[test]
fn a_corrupted_blob_is_detected_and_never_returned_as_content() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([29; 32]), dir.path());

    let payload = testkit::filler("corrupt-me", 512 * 1024);
    store.write_file("data.bin", &payload).unwrap();

    let chunks = store.stat("data.bin").unwrap().unwrap().chunks;
    let mut sealed = store.blobs().get(&chunks[0]).unwrap().unwrap();
    // Flip a bit in the ciphertext body, leaving the version byte alone.
    sealed[5] ^= 0x01;
    store.blobs().remove(&chunks[0]).unwrap();
    store.blobs().put(&chunks[0], &sealed).unwrap();

    assert!(
        store.read_file("data.bin").is_err(),
        "a corrupted chunk decrypted successfully; a malicious host could \
         alter stored data undetected"
    );

    let report = store.verify_integrity(true).unwrap();
    assert!(!report.is_healthy());
    assert_eq!(report.corrupt_files, vec!["data.bin".to_owned()]);
}

#[test]
fn a_chunk_served_under_the_wrong_address_does_not_decrypt() {
    // The substitution attack: a host returns chunk B's bytes when asked for
    // chunk A. Both are genuine chunks belonging to the same user.
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([30; 32]), dir.path());

    store
        .write_file("a.bin", &testkit::filler("alpha", 128 * 1024))
        .unwrap();
    store
        .write_file("b.bin", &testkit::filler("beta", 128 * 1024))
        .unwrap();

    let a = store.stat("a.bin").unwrap().unwrap().chunks[0];
    let b = store.stat("b.bin").unwrap().unwrap().chunks[0];

    let b_bytes = store.blobs().get(&b).unwrap().unwrap();
    store.blobs().remove(&a).unwrap();
    store.blobs().put(&a, &b_bytes).unwrap();

    assert!(
        store.read_file("a.bin").is_err(),
        "one chunk was accepted in another's place; a host can swap stored \
         content undetected"
    );
}

// ---------------------------------------------------------------------------
// Durability and edge cases
// ---------------------------------------------------------------------------

#[test]
fn a_store_reopens_with_everything_intact() {
    let dir = tempfile::tempdir().unwrap();
    let alice = testkit::alice();

    {
        let store = store_for(&alice.master, dir.path());
        for file in &alice.files {
            store.write_file(file.path, &file.content).unwrap();
        }
        store.flush_segment().unwrap();
    }

    let store = store_for(&alice.master, dir.path());
    for file in &alice.files {
        assert_eq!(
            store.read_file(file.path).unwrap().unwrap(),
            file.content,
            "{} did not survive a reopen",
            file.path
        );
    }
    assert_eq!(store.segments().unwrap().len(), 1);
    assert!(store.verify_integrity(true).unwrap().is_healthy());
}

#[test]
fn an_empty_file_is_stored_and_distinguishable_from_a_missing_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([31; 32]), dir.path());

    store.write_file("empty.txt", b"").unwrap();

    assert_eq!(store.read_file("empty.txt").unwrap(), Some(Vec::new()));
    assert_eq!(store.read_file("absent.txt").unwrap(), None);
    assert_eq!(store.stat("empty.txt").unwrap().unwrap().chunks.len(), 0);
    assert!(store.verify_integrity(true).unwrap().is_healthy());
}

#[test]
fn the_store_rejects_paths_that_would_escape_the_sync_root() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([32; 32]), dir.path());

    for hostile in [
        "../../../etc/passwd",
        "/etc/passwd",
        "C:/Windows/System32/config/SAM",
        "..\\..\\Windows",
        "nul",
        "a/../../b",
        "",
    ] {
        assert!(
            store.write_file(hostile, b"payload").is_err(),
            "the store accepted {hostile:?}; a peer's log could write outside \
             the sync root"
        );
        assert!(store.read_file(hostile).is_err());
    }
}

#[test]
fn a_file_larger_than_one_chunk_uses_several_and_still_verifies() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([33; 32]), dir.path());

    let payload = testkit::filler("multichunk", 3 * 1024 * 1024);
    let entry = store.write_file("large.bin", &payload).unwrap();

    assert!(
        entry.chunks.len() >= 10,
        "a 3 MiB file produced only {} chunks",
        entry.chunks.len()
    );
    assert_eq!(entry.size, payload.len() as u64);
    assert_eq!(store.read_file("large.bin").unwrap().unwrap(), payload);
}

#[test]
#[ignore = "seals 64 MiB; ~45s in a debug build. Runs in the slow-tests job."]
fn a_file_far_larger_than_any_buffer_streams_through_without_being_materialised() {
    // The deployment target is a 1 TB array on a Raspberry Pi with a few
    // gigabytes of RAM. A design that materialises each file before storing it
    // does not fail slowly on a large video — the kernel kills it.
    //
    // 64 MiB is small enough to keep the suite quick and far larger than every
    // buffer in the write path, so a regression to slurping would show up as a
    // measurable memory jump rather than silently surviving.
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([50; 32]), dir.path());

    let source = dir.path().join("source.bin");
    {
        let mut file = std::fs::File::create(&source).unwrap();
        // Written in pieces so the test never holds it all either.
        for block in 0..64u32 {
            std::io::Write::write_all(
                &mut file,
                &testkit::filler(&format!("big-{block}"), 1 << 20),
            )
            .unwrap();
        }
    }
    let expected = std::fs::metadata(&source).unwrap().len();

    let entry = store
        .write_stream("huge.bin", std::fs::File::open(&source).unwrap())
        .unwrap();

    assert_eq!(entry.size, expected);
    assert!(
        entry.chunks.len() > 200,
        "expected many chunks for {expected} bytes, got {}",
        entry.chunks.len()
    );

    // And back out, streamed, verified against the source byte for byte.
    let recovered = dir.path().join("recovered.bin");
    {
        let file = std::fs::File::create(&recovered).unwrap();
        assert!(
            store
                .read_stream("huge.bin", std::io::BufWriter::new(file))
                .unwrap()
        );
    }

    let mut source_hash = blake3::Hasher::new();
    source_hash
        .update_reader(std::fs::File::open(&source).unwrap())
        .unwrap();
    let mut recovered_hash = blake3::Hasher::new();
    recovered_hash
        .update_reader(std::fs::File::open(&recovered).unwrap())
        .unwrap();

    assert_eq!(
        source_hash.finalize(),
        recovered_hash.finalize(),
        "a large file did not survive the streaming round trip"
    );
}

#[test]
fn a_stream_and_a_buffer_produce_identical_stores() {
    // The two entry points must agree, or the same file stored through
    // different paths deduplicates against nothing and occupies twice the room.
    let dir = tempfile::tempdir().unwrap();
    let store = store_for(&MasterSecret::from_bytes([51; 32]), dir.path());

    let payload = testkit::filler("two-paths", 2 * 1024 * 1024);

    let buffered = store.write_file("a.bin", &payload).unwrap();
    let streamed = store.write_stream("b.bin", payload.as_slice()).unwrap();

    assert_eq!(buffered.chunks, streamed.chunks);
    assert_eq!(buffered.content_hash, streamed.content_hash);
    assert_eq!(buffered.size, streamed.size);
}

#[test]
fn a_non_default_chunker_still_round_trips() {
    // Guards the tuning knob: a future device with different memory limits must
    // still produce readable data.
    let dir = tempfile::tempdir().unwrap();
    let config = ChunkerConfig::new(1024, 4096, 16 * 1024).unwrap();
    let store = Store::open_with_chunker(
        dir.path(),
        UserKeys::derive(&MasterSecret::from_bytes([34; 32])),
        DeviceKeys::generate().unwrap(),
        config,
    )
    .unwrap();

    let payload = testkit::filler("small-chunks", 256 * 1024);
    let entry = store.write_file("tuned.bin", &payload).unwrap();

    assert!(entry.chunks.len() > 20, "the custom chunker was ignored");
    assert_eq!(store.read_file("tuned.bin").unwrap().unwrap(), payload);
}
