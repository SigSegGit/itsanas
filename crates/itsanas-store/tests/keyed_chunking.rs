//! Where an account cuts its files is its own (HANDOVER §8 3c): a host that
//! holds a candidate copy of a file must not recognise it in an account's
//! store by the sizes of its chunks.
//!
//! The construction and why a keyed gear table would not do are in
//! `src/chunker/keyed.rs`. These tests hold the store to it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use itsanas_crypto::{DeviceKeys, MasterSecret, UserKeys};
use itsanas_store::{ChunkerConfig, Store};

fn store(master: [u8; 32], root: &Path) -> Store {
    Store::open_for_testing(
        root,
        UserKeys::derive(&MasterSecret::from_bytes(master)),
        DeviceKeys::generate().expect("device key"),
        ChunkerConfig::default(),
    )
    .expect("open store")
}

/// Four MiB of reproducible, incompressible bytes: a file a host might hold.
fn candidate() -> Vec<u8> {
    let mut out = Vec::with_capacity(4 << 20);
    let mut counter = 0u64;
    while out.len() < 4 << 20 {
        out.extend_from_slice(blake3::hash(&counter.to_le_bytes()).as_bytes());
        counter += 1;
    }
    out
}

fn sizes(config: &ChunkerConfig, data: &[u8]) -> Vec<usize> {
    config.split(data).map(|chunk| chunk.len()).collect()
}

#[test]
fn red_team_a_host_cannot_recognise_a_file_by_its_chunk_sizes() {
    let dir = tempfile::tempdir().expect("temp dir");
    let file = candidate();
    let mine = store([1; 32], &dir.path().join("mine"));
    let my_laptop = store([1; 32], &dir.path().join("my-laptop"));
    let theirs = store([2; 32], &dir.path().join("theirs"));
    assert!(mine.chunker().is_keyed(), "a real store cuts in public");

    let my_sizes = sizes(mine.chunker(), &file);
    assert!(my_sizes.len() > 20, "too few chunks to say anything");
    assert_eq!(
        my_sizes,
        sizes(my_laptop.chunker(), &file),
        "two devices of one account cut differently: they would not deduplicate"
    );
    assert_ne!(
        my_sizes,
        sizes(theirs.chunker(), &file),
        "two accounts cut the same file alike: one can recognise the other's"
    );

    // The host cuts its candidate copy with everything public it has.
    let public = sizes(&ChunkerConfig::default(), &file);
    let shared = my_sizes.iter().filter(|size| public.contains(size)).count();
    assert!(
        shared * 10 < my_sizes.len(),
        "{shared} of {} chunk sizes reproduced by the public cut",
        my_sizes.len()
    );
}

#[test]
fn keyed_chunks_keep_the_configured_bounds() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mine = store([3; 32], &dir.path().join("mine"));
    let file = candidate();
    let config = mine.chunker();
    let sizes = sizes(config, &file);
    let (last, whole) = sizes.split_last().expect("chunks");
    assert!(*last <= config.max_size());
    for size in whole {
        assert!(
            (config.min_size()..=config.max_size()).contains(size),
            "a chunk of {size} bytes"
        );
    }
    let average = file.len() / sizes.len();
    assert!(
        (config.avg_size() / 2..config.avg_size() * 2).contains(&average),
        "average {average} for a target of {}",
        config.avg_size()
    );
}

#[test]
fn keyed_streaming_and_slicing_agree_on_every_boundary() {
    // Two paths, one cut: a file imported by one and re-imported by the
    // other must deduplicate against itself.
    let dir = tempfile::tempdir().expect("temp dir");
    let mine = store([4; 32], &dir.path().join("mine"));
    let file = candidate();
    let mut streamed = Vec::new();
    itsanas_store::split_stream(mine.chunker(), &file[..], |chunk| {
        streamed.push(chunk.len());
        Ok(())
    })
    .expect("stream");
    assert_eq!(streamed, sizes(mine.chunker(), &file));
}
