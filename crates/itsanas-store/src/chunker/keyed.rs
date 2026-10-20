//! Where an account cuts its files, decided under its own key: Chk-PHTE,
//! "polynomial hashing then encrypt" (Truong, Merz, Scarlata, Günther,
//! Paterson, "Breaking and Fixing Content-Defined Chunking", CCS 2025,
//! ePrint 2025/558, Fig. 4).
//!
//! The public FastCDC cut lets a host holding a candidate copy of a file
//! recognise it in an account's store from chunk sizes alone. Keying the gear
//! table does not fix that: the paper recovers such a table (Bupstash's) from
//! a known file, and the same for Borg, Restic, Duplicacy and Tarsnap. This
//! construction is the paper's fix, with a proof: the chunk lengths it
//! produces are indistinguishable from random ones, to anybody without the
//! key, whatever files they know or choose.
//!
//! Per byte:
//!
//! 1. the last [`WINDOW`] bytes, as a polynomial evaluated at a secret point
//!    of the field 2^61 - 1 ([`super::field`]), rolled on in one
//!    multiplication: a universal hash, collisions at most (w - 1) / 2^61;
//! 2. that value through `SipHash`-2-4 under a second secret key
//!    ([`super::siphash`]): a PRF, so the output says nothing about the
//!    window. The paper uses AES; any PRF keeps its proof, and `SipHash` needs
//!    no hardware instructions, which every device of an account must share;
//! 3. a cut where the output's masked bits are zero -- FastCDC's two masks
//!    and its min / max are kept above this, so sizes behave as before.
//!
//! Cost, measured by `examples/phte_cost.rs` on a 2.1 GHz Xeon: about
//! 60 MB/s against 1.5 GB/s for the public cut, the same with or without AES
//! instructions -- AES ran at 180 MB/s with them and 16 MB/s without.
//!
//! What it does not hide (the paper, §5): two equal chunks are equal, as any
//! deduplication must leave them, and a file's total length.

use std::sync::Arc;

use itsanas_crypto::SymmetricKey;
use zeroize::Zeroize as _;

use super::{
    field::{self, P},
    siphash,
};

/// Bytes the cut decision looks at. 64, as Restic's window: long enough that
/// a cut depends on real content, short enough to stay local to an edit.
pub const WINDOW: usize = 64;

/// The account's keyed cut: its polynomial point, the table that removes the
/// byte leaving the window, and its PRF key. Wiped when the last holder drops
/// it: the table gives the point away as surely as the point itself.
pub struct KeyedCut {
    point: u64,
    /// `-b * point^(WINDOW - 1)` for every byte `b`: the leaving byte's term,
    /// read rather than multiplied.
    leaving: [u64; 256],
    prf: (u64, u64),
}

impl Drop for KeyedCut {
    fn drop(&mut self) {
        self.point.zeroize();
        self.leaving.zeroize();
        self.prf.0.zeroize();
        self.prf.1.zeroize();
    }
}

impl std::fmt::Debug for KeyedCut {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("KeyedCut(..)")
    }
}

/// Equal when they cut alike: the same point and the same PRF key.
impl PartialEq for KeyedCut {
    fn eq(&self, other: &Self) -> bool {
        (self.point, self.prf) == (other.point, other.prf)
    }
}

/// The bounds a cut obeys, from [`super::ChunkerConfig`].
#[derive(Clone, Copy)]
pub struct Bounds {
    pub min: usize,
    pub avg: usize,
    pub max: usize,
    pub strict: u64,
    pub loose: u64,
}

fn word(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().expect("8 bytes"))
}

/// A point of the field from eight random bytes: reduced into it, and moved
/// off 0 and 1, which would make the polynomial trivial (a bias of 2^-60
/// nobody can see).
fn point_from(raw: u64) -> u64 {
    let point = field::reduce(raw & P);
    if point < 2 { point + 2 } else { point }
}

impl KeyedCut {
    /// The cut for the account whose chunking key is `key`
    /// (`UserKeys::chunking_key`).
    #[must_use]
    pub fn new(key: &SymmetricKey) -> Arc<Self> {
        // Disjoint parts of one uniformly random key: independent.
        let bytes = key.expose();
        let point = point_from(word(&bytes[..8]));
        let top = field::pow(point, u32::try_from(WINDOW - 1).expect("small"));
        let mut leaving = [0u64; 256];
        for (byte, slot) in (0u64..).zip(leaving.iter_mut()) {
            *slot = field::reduce(P - field::mul(byte, top));
        }
        Arc::new(Self {
            point,
            leaving,
            prf: (word(&bytes[16..24]), word(&bytes[24..32])),
        })
    }

    /// The polynomial over the window ending just before `end`.
    fn primed(&self, data: &[u8], end: usize) -> u64 {
        data[end.saturating_sub(WINDOW)..end]
            .iter()
            .fold(0, |hash, &byte| {
                field::add(field::mul(hash, self.point), u64::from(byte))
            })
    }

    /// Offset of the first cut in `data`, or `data.len()` when the buffer ends
    /// first; the same contract as the public cut.
    #[must_use]
    pub fn cut_point(&self, data: &[u8], bounds: Bounds) -> usize {
        let len = data.len();
        if len <= bounds.min {
            return len;
        }
        let end = len.min(bounds.max);
        let mut hash = self.primed(data, bounds.min);
        for at in bounds.min..end {
            // Before the window has filled (a minimum under WINDOW, in
            // tests), nothing leaves: byte 0's term is zero.
            let old = at.checked_sub(WINDOW).map_or(0, |gone| data[gone]);
            hash = field::add(hash, self.leaving[usize::from(old)]);
            hash = field::add(field::mul(hash, self.point), u64::from(data[at]));
            let mask = if at < bounds.avg {
                bounds.strict
            } else {
                bounds.loose
            };
            if siphash::hash_word(self.prf.0, self.prf.1, hash) & mask == 0 {
                return at + 1;
            }
        }
        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(seed: u8) -> SymmetricKey {
        SymmetricKey::new([seed; 32])
    }

    /// Where a fixed key cuts a fixed MiB, as a digest of the offsets.
    fn pinned_cuts() -> String {
        let cut = KeyedCut::new(&key(9));
        let data: Vec<u8> = (0u32..1 << 20)
            .flat_map(|i| blake3::hash(&i.to_le_bytes()).as_bytes()[..1].to_vec())
            .collect();
        let bounds = Bounds {
            min: 2048,
            avg: 8192,
            max: 32768,
            strict: (1 << 15) - 1,
            loose: (1 << 11) - 1,
        };
        let mut hasher = blake3::Hasher::new();
        let mut offset = 0;
        while offset < data.len() {
            offset += cut.cut_point(&data[offset..], bounds);
            hasher.update(&offset.to_le_bytes());
        }
        hasher.finalize().to_hex().to_string()
    }

    #[test]
    fn the_keyed_cut_is_pinned_forever() {
        // Changing where a key cuts is changing every chunk of every account:
        // the next write re-uploads it, and devices on two versions stop
        // deduplicating against each other. It also catches the construction
        // quietly losing a part, the PRF above all. Change it only on
        // purpose, with a note in ROADMAP.
        assert_eq!(pinned_cuts(), PINNED);
    }

    const PINNED: &str = "ef22baeed1910420dbb93d2e08ccfae86d0c05f567bfcf9c0e3d324a0e174759";

    #[test]
    fn a_point_is_always_inside_the_field_and_off_the_trivial_ones() {
        for raw in [0, 1, 2, P, P + 1, u64::MAX, P - 1] {
            let point = point_from(raw);
            assert!((2..P).contains(&point), "{raw:#x} gave {point:#x}");
        }
    }

    #[test]
    fn the_rolled_hash_is_the_window_polynomial_wherever_it_stands() {
        // The cut must depend on the last WINDOW bytes only (w-locality):
        // rolled on from anywhere, the hash equals the polynomial of exactly
        // that window, computed afresh.
        let cut = KeyedCut::new(&key(5));
        let data: Vec<u8> = (0..500u32).map(|i| (i * 31 % 251) as u8).collect();
        let mut hash = cut.primed(&data, 100);
        for at in 100..400 {
            let old = data[at - WINDOW];
            hash = field::add(hash, cut.leaving[usize::from(old)]);
            hash = field::add(field::mul(hash, cut.point), u64::from(data[at]));
            assert_eq!(hash, cut.primed(&data, at + 1), "at {at}");
        }
    }
}
