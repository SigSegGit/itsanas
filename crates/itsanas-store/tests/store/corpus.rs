//! The M2 exit criteria and confidentiality between users: a corpus round
//! trip, no plaintext on disk, chunks reused across an insertion, and two
//! users' stores kept apart.

use std::{collections::HashSet, fs, path::Path};

use itsanas_crypto::{ChunkId, MasterSecret};
use itsanas_testkit as testkit;

use crate::store_for;

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
