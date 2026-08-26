//! Identities whose private keys are published, and therefore banned.
//!
//! ITSaNAS ships three test users — Alice, Bob and Carol — whose recovery
//! phrases are printed in full in the project documentation. That is
//! deliberate: anyone should be able to clone the repository and reproduce
//! every encryption, sync and adversarial test without inventing their own key
//! material.
//!
//! The consequence is that their private keys belong to the entire internet.
//! Anyone can sign as Alice. So a production node must never store data for
//! these identities, never honour a pledge from them, and never accept an
//! operation-log segment signed by them — otherwise the fixtures become a free
//! way to push arbitrary content into a real swarm.
//!
//! [`is_published_test_identity`] is the check that enforces that, and it lives
//! here rather than in the test kit so that production code can call it without
//! depending on test-only crates.

use crate::ids::UserId;

/// Raw Ed25519 verifying keys of the three published fixture users.
///
/// Regenerate with `cargo run -p itsanas-testkit --bin generate-fixtures`.
/// If this list ever needs editing, the key schedule has changed and every
/// existing user's identity has changed with it — that is a breaking change,
/// not a routine edit.
pub const PUBLISHED_TEST_USER_IDS: [[u8; 32]; 3] = [
    // alice
    [
        0x9b, 0xac, 0x48, 0x12, 0x19, 0x94, 0x63, 0x0c, 0x0f, 0x43, 0x6b, 0xb2, 0x0c, 0xf6, 0x32,
        0xda, 0xef, 0xb9, 0xa9, 0x41, 0xb2, 0x8c, 0x23, 0x9c, 0x37, 0x49, 0x1f, 0x6b, 0x9f, 0xa5,
        0x8f, 0xfe,
    ],
    // bob
    [
        0x36, 0xa3, 0x2e, 0x8e, 0xad, 0x6b, 0xff, 0x89, 0xbd, 0xb6, 0xa6, 0xe9, 0x5a, 0x6e, 0xbe,
        0x65, 0x86, 0xd4, 0xd4, 0x20, 0xd2, 0xe8, 0x1f, 0x41, 0x41, 0x95, 0x2e, 0x4d, 0x2a, 0x36,
        0xa8, 0x6b,
    ],
    // carol
    [
        0xa9, 0xb0, 0x1e, 0xf6, 0x2c, 0x7f, 0xf2, 0x43, 0x3c, 0x2a, 0xdb, 0x77, 0x21, 0xba, 0x23,
        0xb7, 0xa3, 0xda, 0x02, 0x1b, 0x70, 0xc1, 0x89, 0xe8, 0x03, 0x1c, 0xf1, 0xb4, 0x40, 0xec,
        0x7d, 0x13,
    ],
];

/// True if this identity's private key is published in the documentation.
///
/// A production node must refuse to host, serve, or accept writes from any
/// identity for which this returns true.
#[must_use]
pub fn is_published_test_identity(id: &UserId) -> bool {
    PUBLISHED_TEST_USER_IDS
        .iter()
        .any(|published| published == id.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{MasterSecret, UserKeys};

    /// Mirrors `itsanas-testkit`'s fixture derivation. Duplicated on purpose:
    /// if the test kit and this ban list ever drift apart, this test fails and
    /// the ban list stops silently protecting nothing.
    const FIXTURE_CONTEXT: &str = "itsanas test fixture entropy - NOT SECRET";

    fn fixture(name: &str) -> UserKeys {
        UserKeys::derive(&MasterSecret::from_bytes(blake3::derive_key(
            FIXTURE_CONTEXT,
            name.as_bytes(),
        )))
    }

    #[test]
    fn the_ban_list_matches_the_actual_fixture_identities() {
        for name in ["alice", "bob", "carol"] {
            assert!(
                is_published_test_identity(&fixture(name).user_id()),
                "fixture user {name} is not on the published-identity ban list, \
                 so a production node would happily host data signed with a key \
                 that is printed in the README"
            );
        }
    }

    #[test]
    fn ordinary_identities_are_not_banned() {
        for seed in 0u8..32 {
            let keys = UserKeys::derive(&MasterSecret::from_bytes([seed; 32]));
            assert!(
                !is_published_test_identity(&keys.user_id()),
                "a normal identity was rejected as a published fixture"
            );
        }
        assert!(!is_published_test_identity(&UserId::from_bytes([0u8; 32])));
    }

    #[test]
    fn the_ban_list_has_no_duplicate_or_empty_entries() {
        for (index, id) in PUBLISHED_TEST_USER_IDS.iter().enumerate() {
            assert_ne!(id, &[0u8; 32], "entry {index} is all zeroes");
            assert_eq!(
                PUBLISHED_TEST_USER_IDS.iter().filter(|o| *o == id).count(),
                1,
                "entry {index} is duplicated, so the list covers fewer \
                 identities than it appears to"
            );
        }
    }
}
