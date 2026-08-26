//! Domain-separated key derivation.
//!
//! Every key in ITSaNAS descends from a single 32-byte master secret through
//! BLAKE3's `derive_key`, which takes a hardcoded context string. Two rules
//! make that safe, and both are enforced by tests in this module:
//!
//! 1. Context strings are globally unique and never constructed at runtime, so
//!    no two purposes can ever collide on the same derived key.
//! 2. Context strings carry the format version, so a future v2 key schedule
//!    produces entirely different keys rather than silently reinterpreting v1
//!    material.

use crate::secret::{SecretBytes, SymmetricKey};

/// Ed25519 master signing key: the user's permanent identity.
pub const CTX_USER_SIGNING: &str = "itsanas v1 user master signing key";
/// X25519 master agreement key: used to wrap keys to a user's other devices.
pub const CTX_USER_AGREEMENT: &str = "itsanas v1 user master agreement key";
/// Root key for sealing file chunks.
pub const CTX_USER_CHUNK_DATA: &str = "itsanas v1 user chunk data key";
/// Key that blinds plaintext hashes into storage-visible chunk identifiers.
pub const CTX_USER_BLINDING: &str = "itsanas v1 user chunk id blinding key";
/// Root key for sealing operation-log segments and manifests.
pub const CTX_USER_OPLOG: &str = "itsanas v1 user oplog object key";
/// Wrapping key for the passphrase-protected keystore and its escrow copy.
pub const CTX_KEYSTORE_WRAP: &str = "itsanas v1 keystore wrapping key";
/// Prehash context for domain-separated signatures.
pub const CTX_SIGNED_MESSAGE: &str = "itsanas v1 signed message digest";

/// Every context string this version defines. Used by the uniqueness test and
/// by anyone auditing the key schedule.
pub const ALL_CONTEXTS: &[&str] = &[
    CTX_USER_SIGNING,
    CTX_USER_AGREEMENT,
    CTX_USER_CHUNK_DATA,
    CTX_USER_BLINDING,
    CTX_USER_OPLOG,
    CTX_KEYSTORE_WRAP,
    CTX_SIGNED_MESSAGE,
];

/// Derive a 32-byte subkey from key material under a fixed context string.
#[must_use]
pub fn derive(context: &'static str, key_material: &[u8]) -> SymmetricKey {
    SecretBytes::new(blake3::derive_key(context, key_material))
}

/// Expand a key into `N` bytes of output, bound to a per-object label.
///
/// Used to turn one root key plus an object identifier into the exact
/// (key, nonce) pair for that object, without a second round trip to the RNG.
#[must_use]
pub fn expand<const N: usize>(root: &SymmetricKey, label: &[u8]) -> SecretBytes<N> {
    let mut out = [0u8; N];
    blake3::Hasher::new_keyed(root.expose())
        .update(label)
        .finalize_xof()
        .fill(&mut out);
    SecretBytes::new(out)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn context_strings_are_unique() {
        let unique: HashSet<&&str> = ALL_CONTEXTS.iter().collect();
        assert_eq!(
            unique.len(),
            ALL_CONTEXTS.len(),
            "two key-derivation contexts collide, so two different purposes \
             would share a key"
        );
    }

    #[test]
    fn context_strings_carry_a_version() {
        for context in ALL_CONTEXTS {
            assert!(
                context.contains(" v1 "),
                "context {context:?} has no version marker, so a v2 key \
                 schedule could not be distinguished from v1"
            );
        }
    }

    #[test]
    fn different_contexts_yield_different_keys() {
        let material = [42u8; 32];
        let derived: Vec<_> = ALL_CONTEXTS
            .iter()
            .map(|ctx| derive(ctx, &material))
            .collect();

        for (i, a) in derived.iter().enumerate() {
            for (j, b) in derived.iter().enumerate().skip(i + 1) {
                assert_ne!(
                    a, b,
                    "contexts {:?} and {:?} derived the same key",
                    ALL_CONTEXTS[i], ALL_CONTEXTS[j]
                );
            }
        }
    }

    #[test]
    fn derivation_is_deterministic() {
        let material = [7u8; 32];
        assert_eq!(
            derive(CTX_USER_CHUNK_DATA, &material),
            derive(CTX_USER_CHUNK_DATA, &material)
        );
    }

    #[test]
    fn one_bit_of_master_change_changes_every_subkey() {
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        b[31] = 1;

        for context in ALL_CONTEXTS {
            assert_ne!(derive(context, &a), derive(context, &b));
        }

        a[0] = 0x80;
        assert_ne!(
            derive(CTX_USER_SIGNING, &a),
            derive(CTX_USER_SIGNING, &[0u8; 32])
        );
    }

    #[test]
    fn expansion_separates_by_label() {
        let root = SecretBytes::new([3u8; 32]);
        let a = expand::<56>(&root, b"object-a");
        let b = expand::<56>(&root, b"object-b");
        assert_ne!(a, b);
        assert_eq!(a, expand::<56>(&root, b"object-a"));
    }

    #[test]
    fn expansion_separates_by_root_key() {
        let a = expand::<32>(&SecretBytes::new([1u8; 32]), b"same-label");
        let b = expand::<32>(&SecretBytes::new([2u8; 32]), b"same-label");
        assert_ne!(a, b);
    }
}
