//! What the keyed chunker costs against the public one (HANDOVER §8 3c):
//! `cargo run --release -p itsanas-store --example phte_cost`.
//!
//! The PRF was chosen with an earlier version of this file, on a 2.1 GHz
//! Xeon, 256 MiB of random bytes (the chunker alone; sealing runs at about
//! 800 MB/s beside it):
//!
//! | cut                                    | with AES-NI | without  |
//! |----------------------------------------|-------------|----------|
//! | public FastCDC (before)                | 1.5 GB/s    | 1.5 GB/s |
//! | polynomial + AES-128, 8 blocks at once | 180 MB/s    | 16 MB/s  |
//! | polynomial + `SipHash`-2-4 (chosen)      | 60 MB/s     | 60 MB/s  |
//! | Rabin over GF(2) + AES (Restic's)      | 290 MB/s    | 16 MB/s  |
//!
//! Every device of an account must cut alike, so one PRF serves them all; a
//! Raspberry Pi 4 has no AES instructions, and AES in software put it an
//! order of magnitude behind `SipHash`.

#![allow(clippy::cast_precision_loss, clippy::print_stdout)]

use std::time::Instant;

use itsanas_crypto::{MasterSecret, UserKeys};
use itsanas_store::ChunkerConfig;

fn rate(name: &str, bytes: usize, start: Instant, chunks: usize) {
    let secs = start.elapsed().as_secs_f64();
    println!(
        "{name:<22} {:>7.0} MB/s  {chunks} chunks, average {} B",
        bytes as f64 / secs / 1e6,
        bytes / chunks.max(1)
    );
}

fn main() {
    let size = 256 << 20;
    let mut data = Vec::with_capacity(size);
    let mut counter = 0u64;
    while data.len() < size {
        data.extend_from_slice(blake3::hash(&counter.to_le_bytes()).as_bytes());
        counter += 1;
    }
    let keys = UserKeys::derive(&MasterSecret::from_bytes([3; 32]));

    let public = ChunkerConfig::default();
    let start = Instant::now();
    let count = public.split(&data).count();
    rate("public FastCDC", size, start, count);

    let keyed = ChunkerConfig::default().keyed(&keys.chunking_key());
    let start = Instant::now();
    let count = keyed.split(&data).count();
    rate("keyed (Chk-PHTE)", size, start, count);

    let start = Instant::now();
    let mut count = 0;
    for chunk in keyed.split(&data) {
        keys.seal_chunk(chunk.data).expect("seal");
        count += 1;
    }
    rate("keyed + seal", size, start, count);
}
