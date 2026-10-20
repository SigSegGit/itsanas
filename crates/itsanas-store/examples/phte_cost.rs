//! What a provably secure chunker costs against today's FastCDC (HANDOVER §8
//! 3c): `cargo run --release -p itsanas-store --example phte_cost`.
//!
//! Chk-PHTE (Truong et al., "Breaking and Fixing Content-Defined Chunking",
//! CCS 2025, ePrint 2025/558, Fig. 4): a rolling polynomial hash over the last
//! `WINDOW` bytes, at a secret point of the field 2^61 - 1, then a PRF of that
//! value; a cut where the PRF output's masked bits are zero. Measured with
//! the same min / average / max and two-level normalisation as the chunker,
//! so only the hash under them differs.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::print_stdout,
    deprecated
)]

use std::{hash::Hasher as _, time::Instant};

use aes::cipher::{BlockCipherEncrypt as _, KeyInit as _};
use itsanas_store::ChunkerConfig;

const P: u64 = (1 << 61) - 1;
const WINDOW: usize = 64;
const MIN: usize = ChunkerConfig::DEFAULT_MIN;
const AVG: usize = ChunkerConfig::DEFAULT_AVG;
const MAX: usize = ChunkerConfig::DEFAULT_MAX;
const STRICT: u64 = (1 << 18) - 1;
const LOOSE: u64 = (1 << 14) - 1;

const fn reduce(x: u64) -> u64 {
    if x >= P { x - P } else { x }
}

fn mul(a: u64, b: u64) -> u64 {
    let wide = u128::from(a) * u128::from(b);
    let folded = (wide as u64 & P) + (wide >> 61) as u64;
    if folded >= P { folded - P } else { folded }
}

/// The polynomial over `data[start - WINDOW .. start]`, ready to roll on.
fn prime(data: &[u8], start: usize, key: u64) -> u64 {
    data[start.saturating_sub(WINDOW)..start]
        .iter()
        .fold(0, |u, &b| reduce(mul(u, key) + u64::from(b)))
}

/// Cut points with `prf` deciding; same shape as `ChunkerConfig::cut_point`.
fn cut(data: &[u8], key: u64, top: u64, prf: &mut impl FnMut(u64) -> u64) -> usize {
    let len = data.len();
    if len <= MIN {
        return len;
    }
    let end = len.min(MAX);
    let mut u = prime(data, MIN, key);
    for index in MIN..end {
        let old = if index >= WINDOW {
            data[index - WINDOW]
        } else {
            0
        };
        u = reduce(u + P - mul(u64::from(old), top));
        u = reduce(mul(u, key) + u64::from(data[index]));
        let mask = if index < AVG { STRICT } else { LOOSE };
        if prf(u) & mask == 0 {
            return index + 1;
        }
    }
    end
}

fn chunks(data: &[u8], mut next: impl FnMut(&[u8]) -> usize) -> usize {
    let (mut offset, mut count) = (0, 0);
    while offset < data.len() {
        offset += next(&data[offset..]);
        count += 1;
    }
    count
}

fn report(name: &str, bytes: usize, start: Instant, count: usize, base: Option<f64>) -> f64 {
    let secs = start.elapsed().as_secs_f64();
    let rate = bytes as f64 / secs / 1e6;
    let slower = base.map_or(String::new(), |base| {
        format!(", {:+.0}% time", (secs / base - 1.0) * 100.0)
    });
    println!(
        "{name:<24} {rate:>8.0} MB/s  {count:>6} chunks, avg {:>6} B{slower}",
        bytes / count.max(1)
    );
    secs
}

fn main() {
    let size = 256 << 20;
    let mut data = vec![0u8; size];
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    for byte in &mut data {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }
    let key = 0x0123_4567_89ab_cdef % P;
    let top = (1..WINDOW).fold(1, |acc, _| mul(acc, key));

    // The rest of the pipeline each chunk goes through: address and seal.
    let keys = itsanas_crypto::UserKeys::derive(&itsanas_crypto::MasterSecret::from_bytes([3; 32]));
    let start = Instant::now();
    let mut sealed = 0;
    for piece in data.chunks(AVG) {
        sealed += keys.seal_chunk(piece).expect("seal").1.len();
    }
    let seal = start.elapsed().as_secs_f64();
    println!(
        "{:<24} {:>8.0} MB/s  (address + seal, {sealed} B)",
        "seal_chunk (the rest)",
        size as f64 / seal / 1e6
    );

    let config = ChunkerConfig::default();
    let start = Instant::now();
    let count = chunks(&data, |rest| config.cut_point(rest));
    let base = report("FastCDC gear (today)", size, start, count, None);

    let start = Instant::now();
    let count = chunks(&data, |rest| cut(rest, key, top, &mut |u| u));
    report("PHTE, no PRF (insecure)", size, start, count, Some(base));

    let keys = (0x0706_0504_0302_0100_u64, 0x0f0e_0d0c_0b0a_0908_u64);
    let start = Instant::now();
    let count = chunks(&data, |rest| {
        cut(rest, key, top, &mut |u| {
            let mut hasher = std::hash::SipHasher::new_with_keys(keys.0, keys.1);
            hasher.write_u64(u);
            hasher.finish()
        })
    });
    report("PHTE + SipHash-2-4", size, start, count, Some(base));

    let cipher = aes::Aes128::new(&[7u8; 16].into());
    let start = Instant::now();
    let count = chunks(&data, |rest| {
        cut(rest, key, top, &mut |u| {
            let mut block = [0u8; 16];
            block[..8].copy_from_slice(&u.to_le_bytes());
            let mut block = block.into();
            cipher.encrypt_block(&mut block);
            u64::from_le_bytes(block[..8].try_into().expect("8 bytes"))
        })
    });
    report("PHTE + AES-128", size, start, count, Some(base));

    let start = Instant::now();
    let count = chunks(&data, |rest| cut_batched(rest, key, top, &cipher));
    report("PHTE + AES-128 x8", size, start, count, Some(base));

    let rabin = Rabin::new(0x3D_A3_35_8B_4D_C1_73);
    let start = Instant::now();
    let count = chunks(&data, |rest| rabin.cut(rest, &cipher));
    report("Rabin GF(2) + AES x8", size, start, count, Some(base));

    let leaving: Vec<u64> = (0..256u64).map(|b| P - mul(b, top)).collect();
    let start = Instant::now();
    let count = chunks(&data, |rest| cut_tabled(rest, key, &leaving, &cipher));
    report("PHTE + AES x8 + table", size, start, count, Some(base));
}

/// As [`cut_batched`], the leaving byte's term read from a table: one field
/// multiplication per byte instead of two.
fn cut_tabled(data: &[u8], key: u64, leaving: &[u64], cipher: &aes::Aes128) -> usize {
    const LANES: usize = 8;
    let len = data.len();
    if len <= MIN {
        return len;
    }
    let end = len.min(MAX);
    let mut u = prime(data, MIN, key);
    let mut index = MIN;
    let mut blocks = [aes::Block::default(); LANES];
    while index < end {
        let lanes = LANES.min(end - index);
        for (lane, block) in blocks.iter_mut().enumerate().take(lanes) {
            let at = index + lane;
            let old = if at >= WINDOW { data[at - WINDOW] } else { 0 };
            u = reduce(mul(reduce(u + leaving[old as usize]), key) + u64::from(data[at]));
            *block = [0u8; 16].into();
            block[..8].copy_from_slice(&u.to_le_bytes());
        }
        cipher.encrypt_blocks(&mut blocks[..lanes]);
        for (lane, block) in blocks.iter().enumerate().take(lanes) {
            let at = index + lane;
            let mask = if at < AVG { STRICT } else { LOOSE };
            if u64::from_le_bytes(block[..8].try_into().expect("8 bytes")) & mask == 0 {
                return at + 1;
            }
        }
        index += lanes;
    }
    end
}

/// The same, with eight windows' values ciphered at once: AES-NI pipelines
/// independent blocks, and the polynomial does not depend on the cipher.
fn cut_batched(data: &[u8], key: u64, top: u64, cipher: &aes::Aes128) -> usize {
    const LANES: usize = 8;
    let len = data.len();
    if len <= MIN {
        return len;
    }
    let end = len.min(MAX);
    let mut u = prime(data, MIN, key);
    let mut index = MIN;
    let mut blocks = [aes::Block::default(); LANES];
    while index < end {
        let lanes = LANES.min(end - index);
        for (lane, block) in blocks.iter_mut().enumerate().take(lanes) {
            let at = index + lane;
            let old = if at >= WINDOW { data[at - WINDOW] } else { 0 };
            u = reduce(u + P - mul(u64::from(old), top));
            u = reduce(mul(u, key) + u64::from(data[at]));
            *block = [0u8; 16].into();
            block[..8].copy_from_slice(&u.to_le_bytes());
        }
        cipher.encrypt_blocks(&mut blocks[..lanes]);
        for (lane, block) in blocks.iter().enumerate().take(lanes) {
            let at = index + lane;
            let mask = if at < AVG { STRICT } else { LOOSE };
            if u64::from_le_bytes(block[..8].try_into().expect("8 bytes")) & mask == 0 {
                return at + 1;
            }
        }
        index += lanes;
    }
    end
}

/// Restic's rolling hash: the window's bytes as a polynomial over GF(2),
/// reduced modulo a degree-53 polynomial (secret in the real thing), updated
/// with two table reads and no multiplication.
struct Rabin {
    reduce: [u64; 256],
    leave: [u64; 256],
}

const DEGREE: u32 = 53;

fn gf2_mod(mut x: u128, poly: u64) -> u64 {
    let poly = u128::from(poly);
    while x >> DEGREE != 0 {
        let shift = x.ilog2() - DEGREE;
        x ^= poly << shift;
    }
    x as u64
}

impl Rabin {
    fn new(poly: u64) -> Self {
        let mut reduce = [0u64; 256];
        let mut leave = [0u64; 256];
        for b in 0..256u64 {
            // The top byte that spills past the degree, folded back.
            let spilled = u128::from(b) << DEGREE;
            reduce[b as usize] = gf2_mod(spilled, poly) | (b << DEGREE);
            // A byte that entered `WINDOW` bytes ago, as it stands now.
            let mut h = 0u64;
            for byte in std::iter::once(b).chain(std::iter::repeat_n(0, WINDOW - 1)) {
                h = gf2_mod((u128::from(h) << 8) | u128::from(byte), poly);
            }
            leave[b as usize] = h;
        }
        Self { reduce, leave }
    }

    fn cut(&self, data: &[u8], cipher: &aes::Aes128) -> usize {
        const LANES: usize = 8;
        let len = data.len();
        if len <= MIN {
            return len;
        }
        let end = len.min(MAX);
        let mut h = 0u64;
        for &b in &data[MIN.saturating_sub(WINDOW)..MIN] {
            let top = (h >> (DEGREE - 8)) as usize;
            h = ((h << 8) | u64::from(b)) ^ self.reduce[top];
        }
        let mut index = MIN;
        let mut blocks = [aes::Block::default(); LANES];
        while index < end {
            let lanes = LANES.min(end - index);
            for (lane, block) in blocks.iter_mut().enumerate().take(lanes) {
                let at = index + lane;
                h ^= self.leave[data[at - WINDOW] as usize];
                let top = (h >> (DEGREE - 8)) as usize;
                h = ((h << 8) | u64::from(data[at])) ^ self.reduce[top];
                *block = [0u8; 16].into();
                block[..8].copy_from_slice(&h.to_le_bytes());
            }
            cipher.encrypt_blocks(&mut blocks[..lanes]);
            for (lane, block) in blocks.iter().enumerate().take(lanes) {
                let at = index + lane;
                let mask = if at < AVG { STRICT } else { LOOSE };
                if u64::from_le_bytes(block[..8].try_into().expect("8 bytes")) & mask == 0 {
                    return at + 1;
                }
            }
            index += lanes;
        }
        end
    }
}
