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
