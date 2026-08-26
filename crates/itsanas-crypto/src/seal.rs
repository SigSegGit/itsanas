//! Authenticated encryption for everything that leaves a machine.
//!
//! Two sealing modes, because ITSaNAS stores two kinds of object:
//!
//! * [`seal_deterministic`] — for content-addressed file chunks. Key and nonce
//!   are both derived from the chunk identifier, so re-sealing identical
//!   content yields byte-identical ciphertext. That is what makes
//!   deduplication work, and it also lets an owner re-derive a host's copy of a
//!   chunk to audit it without storing the ciphertext locally. It is safe
//!   despite the fixed nonce because the chunk id is itself a hash of the
//!   plaintext: one key is never used for two different messages.
//!
//! * [`seal_random`] — for operation-log segments and manifests, which are new
//!   objects each time and carry a freshly drawn nonce.
//!
//! Both bind the ciphertext to its owner, purpose and address through the
//! associated data. A host cannot serve chunk X's bytes when asked for chunk Y
//! and have them decrypt.

use chacha20poly1305::{
    Key, KeyInit as _, XChaCha20Poly1305, XNonce,
    aead::{Aead as _, Payload},
};

use crate::{
    error::{CryptoError, Result},
    ids::UserId,
    kdf,
    secret::{SecretBytes, SymmetricKey},
};

/// On-disk and on-wire format version for sealed objects.
pub const SEAL_VERSION: u8 = 1;

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;

/// Overhead added by [`seal_deterministic`]: version byte plus Poly1305 tag.
pub const DETERMINISTIC_OVERHEAD: usize = 1 + TAG_LEN;
/// Overhead added by [`seal_random`]: version byte, nonce, and Poly1305 tag.
pub const RANDOM_OVERHEAD: usize = 1 + NONCE_LEN + TAG_LEN;

/// What a sealed object *is*, cryptographically bound into its ciphertext.
#[derive(Clone, Copy, Debug)]
pub struct SealContext<'a> {
    /// Coarse kind, e.g. `"chunk"` or `"oplog-segment"`. Prevents an object of
    /// one kind being accepted where another is expected.
    pub purpose: &'a str,
    /// The user the object belongs to.
    pub owner: UserId,
    /// The object's storage address.
    pub address: &'a [u8],
}

impl SealContext<'_> {
    /// Canonical, unambiguous associated-data encoding.
    ///
    /// Every variable-length field is length-prefixed so that no two distinct
    /// contexts can serialise to the same bytes.
    fn associated_data(&self) -> Vec<u8> {
        let purpose = self.purpose.as_bytes();
        let mut out = Vec::with_capacity(1 + 4 + purpose.len() + 32 + 4 + self.address.len());
        out.push(SEAL_VERSION);
        out.extend_from_slice(
            &u32::try_from(purpose.len())
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        out.extend_from_slice(purpose);
        out.extend_from_slice(self.owner.as_bytes());
        out.extend_from_slice(
            &u32::try_from(self.address.len())
                .unwrap_or(u32::MAX)
                .to_le_bytes(),
        );
        out.extend_from_slice(self.address);
        out
    }

    /// Label used to derive this object's key material from the root key.
    fn key_label(&self) -> Vec<u8> {
        self.associated_data()
    }
}

fn cipher_for(root: &SymmetricKey, context: &SealContext<'_>) -> XChaCha20Poly1305 {
    let derived = kdf::expand::<KEY_LEN>(root, &context.key_label());
    XChaCha20Poly1305::new(&Key::from(*derived.expose()))
}

fn deterministic_nonce(root: &SymmetricKey, context: &SealContext<'_>) -> SecretBytes<NONCE_LEN> {
    let mut label = context.key_label();
    label.extend_from_slice(b"/nonce");
    kdf::expand::<NONCE_LEN>(root, &label)
}

fn check_version(sealed: &[u8], kind: &'static str) -> Result<()> {
    match sealed.first() {
        Some(&SEAL_VERSION) => Ok(()),
        Some(&found) => Err(CryptoError::UnsupportedVersion {
            kind,
            found,
            supported: SEAL_VERSION,
        }),
        None => Err(CryptoError::Malformed("sealed object is empty")),
    }
}

/// Seal a content-addressed object. Identical inputs produce identical output.
pub fn seal_deterministic(
    root: &SymmetricKey,
    context: &SealContext<'_>,
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    let nonce = deterministic_nonce(root, context);
    let aad = context.associated_data();

    let ciphertext = cipher_for(root, context)
        .encrypt(
            &XNonce::from(*nonce.expose()),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| CryptoError::Kdf("deterministic sealing failed"))?;

    let mut out = Vec::with_capacity(1 + ciphertext.len());
    out.push(SEAL_VERSION);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Open an object sealed by [`seal_deterministic`].
pub fn open_deterministic(
    root: &SymmetricKey,
    context: &SealContext<'_>,
    sealed: &[u8],
) -> Result<Vec<u8>> {
    check_version(sealed, "sealed chunk")?;
    let body = sealed
        .get(1..)
        .ok_or(CryptoError::Malformed("sealed chunk"))?;

    let nonce = deterministic_nonce(root, context);
    let aad = context.associated_data();

    cipher_for(root, context)
        .decrypt(
            &XNonce::from(*nonce.expose()),
            Payload {
                msg: body,
                aad: &aad,
            },
        )
        .map_err(|_| CryptoError::Decrypt)
}

/// Seal a one-off object under a freshly drawn nonce.
pub fn seal_random(
    root: &SymmetricKey,
    context: &SealContext<'_>,
    plaintext: &[u8],
) -> Result<Vec<u8>> {
    let key = kdf::expand::<KEY_LEN>(root, &context.key_label());
    seal_with_key(&key, &context.associated_data(), plaintext)
}

/// Open an object sealed by [`seal_random`].
pub fn open_random(
    root: &SymmetricKey,
    context: &SealContext<'_>,
    sealed: &[u8],
) -> Result<Vec<u8>> {
    let key = kdf::expand::<KEY_LEN>(root, &context.key_label());
    open_with_key(&key, &context.associated_data(), sealed)
}

/// Seal under a caller-supplied key and associated data, with a fresh nonce.
///
/// The escape hatch for the keystore, which must seal the master secret before
/// any identity — and therefore any [`SealContext`] — exists.
pub fn seal_with_key(key: &SymmetricKey, aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(CryptoError::Entropy)?;

    let ciphertext = XChaCha20Poly1305::new(&Key::from(*key.expose()))
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Kdf("sealing failed"))?;

    let mut out = Vec::with_capacity(RANDOM_OVERHEAD + plaintext.len());
    out.push(SEAL_VERSION);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Open an object sealed by [`seal_with_key`].
pub fn open_with_key(key: &SymmetricKey, aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>> {
    check_version(sealed, "sealed object")?;
    if sealed.len() < RANDOM_OVERHEAD {
        return Err(CryptoError::Malformed("sealed object is too short"));
    }

    let nonce: [u8; NONCE_LEN] = sealed[1..=NONCE_LEN]
        .try_into()
        .map_err(|_| CryptoError::Malformed("sealed object nonce"))?;

    XChaCha20Poly1305::new(&Key::from(*key.expose()))
        .decrypt(
            &XNonce::from(nonce),
            Payload {
                msg: &sealed[NONCE_LEN + 1..],
                aad,
            },
        )
        .map_err(|_| CryptoError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{MasterSecret, UserKeys};

    fn user(byte: u8) -> UserKeys {
        UserKeys::derive(&MasterSecret::from_bytes([byte; 32]))
    }

    fn ctx(owner: UserId, address: &[u8]) -> SealContext<'_> {
        SealContext {
            purpose: "chunk",
            owner,
            address,
        }
    }

    #[test]
    fn deterministic_seal_round_trips() {
        let keys = user(1);
        let root = keys.chunk_root();
        let context = ctx(keys.user_id(), b"addr");
        let plaintext = b"hello p2p world";

        let sealed = seal_deterministic(root, &context, plaintext).unwrap();
        assert_eq!(
            open_deterministic(root, &context, &sealed).unwrap(),
            plaintext
        );
        assert_eq!(sealed.len(), plaintext.len() + DETERMINISTIC_OVERHEAD);
    }

    #[test]
    fn deterministic_seal_is_byte_stable() {
        let keys = user(1);
        let context = ctx(keys.user_id(), b"addr");

        let first = seal_deterministic(keys.chunk_root(), &context, b"same").unwrap();
        let second = seal_deterministic(keys.chunk_root(), &context, b"same").unwrap();
        assert_eq!(
            first, second,
            "deterministic sealing must be reproducible, otherwise \
             deduplication and storage audits both break"
        );
    }

    #[test]
    fn random_seal_round_trips_and_is_never_byte_stable() {
        let keys = user(2);
        let root = keys.oplog_root();
        let context = SealContext {
            purpose: "oplog-segment",
            owner: keys.user_id(),
            address: b"seg-1",
        };

        let first = seal_random(root, &context, b"log entries").unwrap();
        let second = seal_random(root, &context, b"log entries").unwrap();

        assert_ne!(first, second, "random sealing reused a nonce");
        assert_eq!(open_random(root, &context, &first).unwrap(), b"log entries");
        assert_eq!(
            open_random(root, &context, &second).unwrap(),
            b"log entries"
        );
    }

    #[test]
    fn another_user_cannot_open_your_chunk() {
        let alice = user(3);
        let bob = user(4);
        let context = ctx(alice.user_id(), b"addr");

        let sealed =
            seal_deterministic(alice.chunk_root(), &context, b"alice's tax return").unwrap();

        assert!(
            open_deterministic(bob.chunk_root(), &context, &sealed).is_err(),
            "Bob decrypted Alice's chunk while merely hosting it; the entire \
             trust model of ITSaNAS is broken"
        );
    }

    #[test]
    fn a_host_cannot_substitute_one_chunk_for_another() {
        let keys = user(5);
        let root = keys.chunk_root();

        let sealed_a =
            seal_deterministic(root, &ctx(keys.user_id(), b"address-a"), b"payload").unwrap();

        assert!(
            open_deterministic(root, &ctx(keys.user_id(), b"address-b"), &sealed_a).is_err(),
            "a chunk opened under the wrong address, so a host could serve \
             stale or swapped content undetected"
        );
    }

    #[test]
    fn purpose_confusion_is_rejected() {
        let keys = user(6);
        let root = keys.chunk_root();
        let owner = keys.user_id();

        let as_chunk = seal_deterministic(
            root,
            &SealContext {
                purpose: "chunk",
                owner,
                address: b"x",
            },
            b"payload",
        )
        .unwrap();

        assert!(
            open_deterministic(
                root,
                &SealContext {
                    purpose: "oplog-segment",
                    owner,
                    address: b"x",
                },
                &as_chunk,
            )
            .is_err(),
            "a chunk was accepted as an oplog segment"
        );
    }

    #[test]
    fn owner_confusion_is_rejected() {
        let alice = user(7);
        let bob = user(8);
        let root = alice.chunk_root();

        let sealed = seal_deterministic(root, &ctx(alice.user_id(), b"x"), b"payload").unwrap();

        assert!(
            open_deterministic(root, &ctx(bob.user_id(), b"x"), &sealed).is_err(),
            "ciphertext is not bound to its owner"
        );
    }

    #[test]
    fn every_single_bit_flip_is_detected() {
        let keys = user(9);
        let root = keys.chunk_root();
        let context = ctx(keys.user_id(), b"addr");
        let sealed = seal_deterministic(root, &context, b"integrity matters").unwrap();

        // Skip the version byte: flipping it is caught by the version check,
        // which is tested separately.
        for byte_index in 1..sealed.len() {
            for bit in 0..8u8 {
                let mut tampered = sealed.clone();
                tampered[byte_index] ^= 1 << bit;
                assert!(
                    open_deterministic(root, &context, &tampered).is_err(),
                    "flipping bit {bit} of byte {byte_index} went undetected; a \
                     malicious host could corrupt data silently"
                );
            }
        }
    }

    #[test]
    fn truncation_is_detected() {
        let keys = user(10);
        let root = keys.chunk_root();
        let context = ctx(keys.user_id(), b"addr");
        let sealed = seal_deterministic(root, &context, b"a reasonably long payload").unwrap();

        for cut in 1..sealed.len() {
            assert!(open_deterministic(root, &context, &sealed[..cut]).is_err());
        }
    }

    #[test]
    fn an_unknown_format_version_is_refused_not_guessed_at() {
        let keys = user(11);
        let root = keys.chunk_root();
        let context = ctx(keys.user_id(), b"addr");
        let mut sealed = seal_deterministic(root, &context, b"payload").unwrap();
        sealed[0] = 99;

        match open_deterministic(root, &context, &sealed) {
            Err(CryptoError::UnsupportedVersion { found, .. }) => assert_eq!(found, 99),
            other => panic!("expected a version error, got {other:?}"),
        }
    }

    #[test]
    fn empty_and_undersized_inputs_do_not_panic() {
        let keys = user(12);
        let root = keys.chunk_root();
        let context = ctx(keys.user_id(), b"addr");

        assert!(open_deterministic(root, &context, &[]).is_err());
        assert!(open_random(root, &context, &[]).is_err());
        assert!(open_random(root, &context, &[SEAL_VERSION]).is_err());
        for len in 0..RANDOM_OVERHEAD {
            let mut short = vec![SEAL_VERSION; len];
            if !short.is_empty() {
                short[0] = SEAL_VERSION;
            }
            assert!(open_random(root, &context, &short).is_err());
        }
    }

    #[test]
    fn empty_plaintext_is_a_valid_object() {
        let keys = user(13);
        let root = keys.chunk_root();
        let context = ctx(keys.user_id(), b"addr");

        let sealed = seal_deterministic(root, &context, b"").unwrap();
        assert_eq!(open_deterministic(root, &context, &sealed).unwrap(), b"");
    }

    #[test]
    fn associated_data_encoding_is_unambiguous() {
        let owner = user(14).user_id();
        let a = SealContext {
            purpose: "ab",
            owner,
            address: b"c",
        };
        let b = SealContext {
            purpose: "a",
            owner,
            address: b"bc",
        };
        assert_ne!(a.associated_data(), b.associated_data());
    }
}
