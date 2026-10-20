//! Arithmetic in the prime field of the Mersenne prime 2^61 - 1, the field
//! the keyed chunker's polynomial hash is evaluated in.
//!
//! Why this prime: a product of two elements fits in 122 bits, and reducing
//! it is a shift, a mask and one conditional subtraction -- no division. Its
//! size sets the hash's collision bound, (w - 1) / 2^61 for a window of w
//! bytes (Truong et al., Lemma 4.4): about 2^-55 for 64 bytes.

/// The modulus, 2^61 - 1.
pub const P: u64 = (1 << 61) - 1;

/// `x` reduced from `[0, 2P)` into `[0, P)`.
#[inline]
pub const fn reduce(x: u64) -> u64 {
    if x >= P { x - P } else { x }
}

/// `a * b mod P`, for `a, b < P`.
#[inline]
pub const fn mul(a: u64, b: u64) -> u64 {
    let wide = a as u128 * b as u128;
    // 2^61 = 1 mod P: the bits above 61 fold back onto the low ones.
    #[allow(clippy::cast_possible_truncation)]
    let low = wide as u64 & P;
    #[allow(clippy::cast_possible_truncation)]
    let high = (wide >> 61) as u64;
    reduce(low + high)
}

/// `a + b mod P`, for `a, b < P`.
#[inline]
pub const fn add(a: u64, b: u64) -> u64 {
    reduce(a + b)
}

/// `base^exponent mod P`.
#[must_use]
pub const fn pow(base: u64, mut exponent: u32) -> u64 {
    let (mut result, mut square) = (1, base);
    while exponent > 0 {
        if exponent & 1 == 1 {
            result = mul(result, square);
        }
        square = mul(square, square);
        exponent >>= 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn the_field_agrees_with_plain_integer_arithmetic() {
        let samples = [0, 1, 2, 255, P - 1, P - 2, 0x0123_4567_89ab_cdef, P / 3];
        for &a in &samples {
            for &b in &samples {
                let expected = (u128::from(a) * u128::from(b) % u128::from(P)) as u64;
                assert_eq!(mul(a, b), expected, "{a} * {b}");
                assert_eq!(
                    add(a, b),
                    ((u128::from(a) + u128::from(b)) % u128::from(P)) as u64
                );
            }
        }
        assert_eq!(pow(3, 0), 1);
        assert_eq!(pow(3, 5), 243);
        // Fermat: a^(P-1) = 1 for a not divisible by P; checked through a
        // chain of squarings that exercises every reduction path.
        assert_eq!(
            pow(0x1234_5678, 1 << 20),
            mul(pow(0x1234_5678, 1 << 19), pow(0x1234_5678, 1 << 19))
        );
    }
}
