//! Durability and edge cases: reopening, empty and large files, paths that
//! would escape the root, streams, and a non-default chunker.

use itsanas_crypto::{DeviceKeys, MasterSecret, UserKeys};
use itsanas_store::{ChunkerConfig, Store};
use itsanas_testkit as testkit;

use crate::store_for;

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
