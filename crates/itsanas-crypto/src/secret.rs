use core::fmt;

use subtle::{Choice, ConstantTimeEq};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::{CryptoError, Result};

/// A fixed-size secret that is wiped on drop, never printed, and only
/// comparable in constant time.
///
/// The `Debug` impl prints `SecretBytes<32>(redacted)`. That is not paranoia
/// for its own sake: daemon logs on an ITSaNAS node are readable by whoever
/// owns that machine, and that is explicitly not the person whose keys these
/// are.
#[derive(Clone)]
pub struct SecretBytes<const N: usize>([u8; N]);

impl<const N: usize> Zeroize for SecretBytes<N> {
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl<const N: usize> Drop for SecretBytes<N> {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl<const N: usize> ZeroizeOnDrop for SecretBytes<N> {}

impl<const N: usize> SecretBytes<N> {
    #[must_use]
    pub const fn new(bytes: [u8; N]) -> Self {
        Self(bytes)
    }

    /// Fill a new secret from the operating system CSPRNG.
    pub fn random() -> Result<Self> {
        let mut bytes = [0u8; N];
        getrandom::fill(&mut bytes).map_err(CryptoError::Entropy)?;
        Ok(Self(bytes))
    }

    /// Borrow the raw bytes.
    ///
    /// Named `expose` rather than `as_bytes` so that every place secret
    /// material escapes the wrapper is greppable.
    #[must_use]
    pub const fn expose(&self) -> &[u8; N] {
        &self.0
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let array: [u8; N] = bytes
            .try_into()
            .map_err(|_| CryptoError::MalformedKey("wrong key length"))?;
        Ok(Self(array))
    }
}

impl<const N: usize> fmt::Debug for SecretBytes<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretBytes<{N}>(redacted)")
    }
}

impl<const N: usize> ConstantTimeEq for SecretBytes<N> {
    fn ct_eq(&self, other: &Self) -> Choice {
        self.0.ct_eq(&other.0)
    }
}

impl<const N: usize> PartialEq for SecretBytes<N> {
    fn eq(&self, other: &Self) -> bool {
        self.ct_eq(other).into()
    }
}

impl<const N: usize> Eq for SecretBytes<N> {}

/// A 256-bit symmetric key.
pub type SymmetricKey = SecretBytes<32>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_reveals_bytes() {
        let secret = SecretBytes::<32>::new([0xAB; 32]);
        let rendered = format!("{secret:?}");
        assert_eq!(rendered, "SecretBytes<32>(redacted)");
        assert!(!rendered.contains("ab"), "debug output leaked key bytes");
        assert!(!rendered.contains("171"), "debug output leaked key bytes");
    }

    #[test]
    fn equality_is_value_based() {
        let a = SecretBytes::<32>::new([7; 32]);
        let b = SecretBytes::<32>::new([7; 32]);
        let c = SecretBytes::<32>::new([8; 32]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn random_secrets_differ() {
        let a = SecretBytes::<32>::random().unwrap();
        let b = SecretBytes::<32>::random().unwrap();
        assert_ne!(a, b, "two random secrets collided; the CSPRNG is broken");
        assert_ne!(a.expose(), &[0u8; 32], "random secret was all zeroes");
    }

    #[test]
    fn from_slice_rejects_wrong_length() {
        assert!(SecretBytes::<32>::from_slice(&[0u8; 31]).is_err());
        assert!(SecretBytes::<32>::from_slice(&[0u8; 33]).is_err());
        assert!(SecretBytes::<32>::from_slice(&[0u8; 32]).is_ok());
    }
}
