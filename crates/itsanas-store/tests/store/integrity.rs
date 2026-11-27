//! Integrity and tamper detection: what does not decrypt, what is missing
//! or corrupt, and what is refused before it is decrypted.

use itsanas_crypto::MasterSecret;
use itsanas_testkit as testkit;

use crate::store_for;

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
    store.blobs().damage(&address).expect("lose it");

    let oversized = vec![0u8; (256 * 1024) + 4096];
    assert!(
        !store.accept_chunk(&address, &oversized).expect("restore"),
        "a reply larger than any chunk the chunker can emit was accepted"
    );
    assert!(
        !store.has_chunk(&address),
        "the oversized reply was written"
    );

    // And the real thing still comes back, so the bound is not simply "no".
    assert!(
        store.accept_chunk(&address, &genuine).expect("restore"),
        "the genuine chunk was refused along with the rubbish"
    );
    assert_eq!(
        store.read_file("real.bin").expect("read").as_deref(),
        Some(&b"a genuine chunk"[..])
    );
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
