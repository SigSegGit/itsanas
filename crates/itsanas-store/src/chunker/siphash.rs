//! `SipHash`-2-4 of one 64-bit word: the PRF of the keyed chunker.
//!
//! Chosen over AES (the paper's instantiation) by measurement: every device
//! of an account must use the same PRF to cut alike, and AES without
//! hardware instructions -- a Raspberry Pi 4 has none -- ran ten times
//! slower than `SipHash`, which needs nothing but adds, rotations and XORs.
//! `examples/phte_cost.rs` has the figures.
//!
//! Specialised to an 8-byte message: one compression of the word, one of the
//! length block, four finalisation rounds. The test holds it to the standard
//! implementation.

/// The four state words, as `SipHash` names them.
struct State([u64; 4]);

impl State {
    const fn new(k0: u64, k1: u64) -> Self {
        Self([
            k0 ^ 0x736f_6d65_7073_6575,
            k1 ^ 0x646f_7261_6e64_6f6d,
            k0 ^ 0x6c79_6765_6e65_7261,
            k1 ^ 0x7465_6462_7974_6573,
        ])
    }

    #[inline]
    fn round(&mut self) {
        let [v0, v1, v2, v3] = &mut self.0;
        *v0 = v0.wrapping_add(*v1);
        *v1 = v1.rotate_left(13) ^ *v0;
        *v0 = v0.rotate_left(32);
        *v2 = v2.wrapping_add(*v3);
        *v3 = v3.rotate_left(16) ^ *v2;
        *v0 = v0.wrapping_add(*v3);
        *v3 = v3.rotate_left(21) ^ *v0;
        *v2 = v2.wrapping_add(*v1);
        *v1 = v1.rotate_left(17) ^ *v2;
        *v2 = v2.rotate_left(32);
    }

    #[inline]
    fn absorb(&mut self, word: u64) {
        self.0[3] ^= word;
        self.round();
        self.round();
        self.0[0] ^= word;
    }
}

/// `SipHash`-2-4 under `(k0, k1)` of the eight little-endian bytes of `word`.
#[inline]
#[must_use]
pub fn hash_word(k0: u64, k1: u64, word: u64) -> u64 {
    let mut state = State::new(k0, k1);
    state.absorb(word);
    // The final block of an 8-byte message: its length in the top byte.
    state.absorb(8 << 56);
    state.0[2] ^= 0xff;
    for _ in 0..4 {
        state.round();
    }
    let [v0, v1, v2, v3] = state.0;
    v0 ^ v1 ^ v2 ^ v3
}

#[cfg(test)]
mod tests {
    use std::hash::Hasher as _;

    use super::*;

    #[test]
    #[allow(deprecated)]
    fn the_word_hash_is_standard_siphash_2_4() {
        for (k0, k1, word) in [
            (0, 0, 0),
            (1, 2, 0x0102_0304_0506_0708),
            (u64::MAX, 0x0f0e_0d0c_0b0a_0908, u64::MAX),
        ] {
            let mut standard = std::hash::SipHasher::new_with_keys(k0, k1);
            standard.write(&word.to_le_bytes());
            assert_eq!(hash_word(k0, k1, word), standard.finish());
        }
    }
}
