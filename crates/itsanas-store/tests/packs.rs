//! A whole store on pack files (M10): what the file layout does, without a
//! file per chunk.
//!
//! A store keeps the layout it was created with; one created in packs is
//! made here by opening its root packed before the store opens it.

use std::time::Duration;

use itsanas_crypto::{DeviceKeys, MasterSecret, UserKeys};
use itsanas_store::{BlobStore, ChunkerConfig, Store};

fn packed_store(root: &std::path::Path, master: &MasterSecret, device: DeviceKeys) -> Store {
    drop(BlobStore::open_packed(root).expect("packs"));
    Store::open_for_testing(
        root,
        UserKeys::derive(master),
        device,
        ChunkerConfig::default(),
    )
    .expect("store opens")
}

#[test]
fn a_store_in_packs_writes_reads_checks_collects_and_reopens_with_no_file_per_chunk() {
    let dir = tempfile::tempdir().expect("temp dir");
    let master = MasterSecret::from_bytes([42; 32]);
    let seed = itsanas_crypto::SecretBytes::new([9; 32]);
    let body: Vec<u8> = (0..3_000_000u32).map(|i| (i * 7 % 251) as u8).collect();
    {
        let store = packed_store(dir.path(), &master, DeviceKeys::from_seed(&seed));
        assert!(
            store.blobs().packs().is_some(),
            "the store did not keep its packs"
        );
        store.write_file("big.bin", &body).expect("write");
        store
            .write_file("gone.bin", b"to be removed")
            .expect("write");
        store.remove_file("gone.bin").expect("remove");
        let report = store.collect_garbage(Duration::ZERO).expect("gc");
        assert!(
            report.blobs_removed >= 1,
            "the removed file's chunk was kept"
        );
        let check = store.verify_integrity(true).expect("verify");
        assert!(check.missing_chunks.is_empty() && check.corrupt_files.is_empty());
    }
    let store = packed_store(dir.path(), &master, DeviceKeys::from_seed(&seed));
    assert_eq!(
        store.read_file("big.bin").expect("read").as_deref(),
        Some(body.as_slice())
    );
    assert_eq!(store.read_file("gone.bin").expect("read"), None);
    assert!(
        !dir.path().join("blobs").exists()
            || std::fs::read_dir(dir.path().join("blobs"))
                .unwrap()
                .next()
                .is_none(),
        "a store in packs wrote blob files"
    );
}

#[test]
fn red_team_a_store_converted_to_packs_keeps_every_file_and_survives_a_crash_at_each_step() {
    let dir = tempfile::tempdir().expect("temp dir");
    let master = MasterSecret::from_bytes([43; 32]);
    let seed = itsanas_crypto::SecretBytes::new([10; 32]);
    let open = || {
        Store::open_for_testing(
            dir.path(),
            UserKeys::derive(&master),
            DeviceKeys::from_seed(&seed),
            ChunkerConfig::default(),
        )
        .expect("store opens")
    };
    let body: Vec<u8> = (0..1_500_000u32).map(|i| (i * 13 % 251) as u8).collect();
    // A store already in blob files, whatever ITSANAS_PACKS says: the
    // variable only picks the layout of a store with nothing in it yet (it
    // made this fixture start in packs on Nicolas's run, 2026-10-14).
    std::fs::create_dir_all(dir.path().join("blobs").join("00")).unwrap();
    {
        let store = open();
        assert!(
            store.blobs().packs().is_none(),
            "the fixture is not in blob files"
        );
        store.write_file("kept.bin", &body).expect("write");
    }
    // A crash during the copy: a staging directory nothing reads. The blob
    // store must still be whole, and the next conversion start over.
    std::fs::create_dir_all(dir.path().join("converting").join("packs")).unwrap();
    std::fs::write(
        dir.path()
            .join("converting")
            .join("packs")
            .join("00000001.pack"),
        b"half a copy",
    )
    .unwrap();
    {
        let store = open();
        assert!(
            store.blobs().packs().is_none(),
            "a half-made staging switched the layout"
        );
        assert_eq!(
            store.read_file("kept.bin").unwrap().as_deref(),
            Some(body.as_slice())
        );
    }
    let moved = BlobStore::convert_to_packs(dir.path()).expect("convert");
    assert!(moved > 0);
    assert!(
        !dir.path().join("blobs").exists(),
        "the blob files were left behind"
    );
    assert!(!dir.path().join("converting").exists());
    let store = open();
    assert!(
        store.blobs().packs().is_some(),
        "the store did not reopen in packs"
    );
    assert_eq!(
        store.read_file("kept.bin").unwrap().as_deref(),
        Some(body.as_slice()),
        "a converted store lost a file"
    );
    let check = store.verify_integrity(true).expect("verify");
    assert!(check.missing_chunks.is_empty() && check.corrupt_files.is_empty());
    drop(store);
    // A crash after the rename, before the blob files went: the packs are
    // read, and the next call clears the leftovers.
    std::fs::create_dir_all(dir.path().join("blobs").join("ab")).unwrap();
    assert_eq!(BlobStore::convert_to_packs(dir.path()).expect("again"), 0);
    assert!(!dir.path().join("blobs").exists());
}
