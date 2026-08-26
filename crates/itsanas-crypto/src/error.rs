/// Every way a cryptographic operation in ITSaNAS can fail.
///
/// Variants deliberately avoid echoing key material, plaintext, or passphrases
/// so that error values are safe to log on a machine that hosts other people's
/// data.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CryptoError {
    #[error(
        "authenticated decryption failed: wrong key, wrong associated data, or tampered ciphertext"
    )]
    Decrypt,

    #[error("signature verification failed")]
    BadSignature,

    #[error("malformed key material: {0}")]
    MalformedKey(&'static str),

    #[error("invalid recovery phrase: {0}")]
    BadMnemonic(String),

    #[error("unsupported {kind} format version {found} (this build understands up to {supported})")]
    UnsupportedVersion {
        kind: &'static str,
        found: u8,
        supported: u8,
    },

    #[error("malformed {0}")]
    Malformed(&'static str),

    #[error("the operating system refused to supply randomness: {0}")]
    Entropy(#[from] getrandom::Error),

    #[error("key derivation failed: {0}")]
    Kdf(&'static str),
}

pub type Result<T> = core::result::Result<T, CryptoError>;
