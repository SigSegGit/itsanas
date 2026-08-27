//! Published test users and their data, for every test in the workspace.
//!
//! # These keys are public, on purpose, and are banned in production
//!
//! Alice, Bob and Carol have real identities with real recovery phrases, and
//! those phrases are printed in full in `docs/TEST-USERS.md`. Anyone can clone
//! this repository and reproduce every encryption, sync and adversarial test
//! byte for byte, without inventing key material of their own.
//!
//! Three things stop that openness from becoming an attack surface:
//!
//! 1. **The identities are banned in production.**
//!    [`itsanas_crypto::is_published_test_identity`] returns true for all three,
//!    and a real node refuses to host, serve, or accept writes from them. Their
//!    published keys therefore buy an attacker nothing in a live swarm.
//!
//! 2. **The corpus has no files to tamper with.** Every byte is *generated*
//!    from seeds written in this source file. There is no fixture directory to
//!    swap, no archive to poison — changing the test data means changing
//!    reviewed source code.
//!
//! 3. **The corpus is pinned.** Every file's BLAKE3 digest and the digest of
//!    the corpus as a whole are constants here, checked by
//!    [`tests::corpus_matches_its_published_digests`] and republished in the
//!    documentation. Any change to the test data — accidental or hostile —
//!    fails CI and changes a value that a reader can verify by hand.

use itsanas_crypto::{MasterSecret, UserKeys};

/// Context string for fixture entropy.
///
/// Deliberately unlike any production derivation context in
/// [`itsanas_crypto::kdf`], so fixture material can never collide with a real
/// user's keys.
pub const FIXTURE_CONTEXT: &str = "itsanas test fixture entropy - NOT SECRET";

/// Passphrase used for every fixture keystore. Published, like everything else
/// about these users.
pub const FIXTURE_PASSPHRASE: &str = "itsanas-test-users-are-public-do-not-reuse";

/// One file in a test user's data set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestFile {
    /// Path relative to the user's synced folder, always with `/` separators.
    pub path: &'static str,
    /// Full contents.
    pub content: Vec<u8>,
    /// BLAKE3 digest of `content`, pinned in source so tampering is visible.
    pub digest: &'static str,
}

impl TestFile {
    #[must_use]
    pub fn actual_digest(&self) -> String {
        blake3::hash(&self.content).to_hex().to_string()
    }
}

/// A published test user: identity, credentials, and private data.
#[derive(Debug)]
pub struct TestUser {
    /// Account name, as it would be registered with a coordinator.
    pub username: &'static str,
    /// The 24-word BIP-39 recovery phrase. Public.
    pub recovery_phrase: String,
    /// The master secret the phrase encodes.
    pub master: MasterSecret,
    /// The derived key schedule.
    pub keys: UserKeys,
    /// A byte string that appears in this user's plaintext and nowhere else.
    ///
    /// Tests scan a host's on-disk store for it: if it ever turns up in another
    /// user's storage directory, encryption is not actually happening.
    pub canary: &'static str,
    /// This user's private files.
    pub files: Vec<TestFile>,
}

impl TestUser {
    /// Total plaintext size of this user's data, in bytes.
    #[must_use]
    pub fn plaintext_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.content.len() as u64).sum()
    }

    #[must_use]
    pub fn file(&self, path: &str) -> Option<&TestFile> {
        self.files.iter().find(|f| f.path == path)
    }
}

/// Deterministic filler bytes, so a large fixture file costs one line of source
/// rather than a megabyte in git.
#[must_use]
pub fn filler(seed: &str, len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    blake3::Hasher::new_derive_key("itsanas test fixture filler")
        .update(seed.as_bytes())
        .finalize_xof()
        .fill(&mut out);
    out
}

/// Text content with the owner's canary embedded, padded to a chosen size.
fn text_with_canary(canary: &str, body: &str, pad_to: usize) -> Vec<u8> {
    let mut content = format!("{body}\n\ncanary: {canary}\n").into_bytes();
    if content.len() < pad_to {
        let extra = filler(canary, pad_to - content.len());
        // Keep it printable so a human inspecting the file sees text, and so a
        // naive "does this look encrypted?" eyeball check is meaningful.
        content.extend(
            extra
                .iter()
                .map(|b| b"0123456789abcdef "[(*b % 17) as usize]),
        );
    }
    content
}

fn build(username: &'static str, canary: &'static str, files: Vec<TestFile>) -> TestUser {
    let master = MasterSecret::from_bytes(blake3::derive_key(FIXTURE_CONTEXT, username.as_bytes()));

    TestUser {
        username,
        // These users are published on purpose, so the phrase is deliberately
        // copied out of its zeroizing wrapper and kept in an ordinary String.
        recovery_phrase: master
            .to_recovery_phrase()
            .expect("fixture entropy is always 32 bytes")
            .as_str()
            .to_owned(),
        keys: UserKeys::derive(&master),
        master,
        canary,
        files,
    }
}

pub const ALICE_CANARY: &str = "ITSANAS-CANARY-ALICE-4f21c8d0";
pub const BOB_CANARY: &str = "ITSANAS-CANARY-BOB-9e73a1b5";
pub const CAROL_CANARY: &str = "ITSANAS-CANARY-CAROL-2c60f8ae";

/// A file every user holds byte-for-byte identically.
///
/// Its purpose is adversarial: because all three users store exactly these
/// bytes, any cross-user deduplication or address collision shows up
/// immediately as a chunk id that two users share.
pub const SHARED_DOCUMENT: &[u8] =
    b"This exact paragraph is stored by Alice, Bob and Carol alike. \
      If a host can tell that, blinding is broken.\n";

/// Alice — a laptop user with documents, a photo, and an empty file.
#[must_use]
pub fn alice() -> TestUser {
    build(
        "alice",
        ALICE_CANARY,
        vec![
            TestFile {
                path: "notes/architecture.md",
                content: text_with_canary(
                    ALICE_CANARY,
                    "# Architecture notes\n\nRendezvous hashing beats consistent hashing here.",
                    0,
                ),
                digest: "703346cc4634b513141d2cdf05cdf8975623a693ea979d6637eb0a0fea6985f9",
            },
            TestFile {
                path: "finance/taxes-2026.csv",
                content: text_with_canary(
                    ALICE_CANARY,
                    "date,amount,category\n2026-01-04,-1299.00,hardware",
                    4096,
                ),
                digest: "917bfd933ae52d158534c541b77e4fd8787be9af5a0c98f3add48e1969f98c3b",
            },
            TestFile {
                path: "photos/holiday.jpg",
                content: filler("alice/photos/holiday.jpg", 512 * 1024),
                digest: "4ebad42d75cd47a195374ca6fadfa6e4b2c841957e3293ccedf3ced5a28e5572",
            },
            TestFile {
                path: "empty.txt",
                content: Vec::new(),
                digest: "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
            },
            TestFile {
                path: "shared/common.txt",
                content: SHARED_DOCUMENT.to_vec(),
                digest: "e1bbbbc9dc8b32d986d3f7b318e7d4ead364a3db7b5740a0a943e0734239564a",
            },
        ],
    )
}

/// Bob — a Raspberry Pi user with source code, media, and a password database.
#[must_use]
pub fn bob() -> TestUser {
    build(
        "bob",
        BOB_CANARY,
        vec![
            TestFile {
                path: "code/main.rs",
                content: text_with_canary(
                    BOB_CANARY,
                    "fn main() {\n    println!(\"hello from the pi\");\n}",
                    0,
                ),
                digest: "54ad78937a0dec1f1e4a78760fa49783fe22f0d661417e9712f53eb2fec4f7de",
            },
            TestFile {
                path: "secrets/passwords.kdbx",
                content: filler("bob/secrets/passwords.kdbx", 17 * 1024 + 7),
                digest: "b29337a12662c2d275e0b32c4a01b01e3b71d7681796ba86c6f53885894dd5dd",
            },
            TestFile {
                path: "music/track.flac",
                content: filler("bob/music/track.flac", 1024 * 1024),
                digest: "b2ae8395456a3bb4482f3f97176ef9ebedf8e923355d7846b917def820db3936",
            },
            TestFile {
                path: "shared/common.txt",
                content: SHARED_DOCUMENT.to_vec(),
                digest: "e1bbbbc9dc8b32d986d3f7b318e7d4ead364a3db7b5740a0a943e0734239564a",
            },
        ],
    )
}

/// Carol — a VM user with a thesis, measurements, and rotating logs.
#[must_use]
pub fn carol() -> TestUser {
    build(
        "carol",
        CAROL_CANARY,
        vec![
            TestFile {
                path: "thesis/chapter-1.tex",
                content: text_with_canary(
                    CAROL_CANARY,
                    "\\chapter{Storage under churn}\nAvailability is not durability.",
                    8192,
                ),
                digest: "d668f7764c267d34db15542481e18bb465d9f88ea12ef0768c178fe902ac923a",
            },
            TestFile {
                path: "data/measurements.parquet",
                content: filler("carol/data/measurements.parquet", 256 * 1024),
                digest: "9976f7b7a7daf0bce8e04dd256e765d1d5e893f2eb60ed9470627b01b7449fa7",
            },
            TestFile {
                path: "logs/system.log",
                content: text_with_canary(CAROL_CANARY, "boot: ok\nnetwork: ok", 64 * 1024),
                digest: "e1f5617b13e8500293a27d9ac1fde94289b464bc8e2f5e8dcbc31db36861b26a",
            },
            TestFile {
                path: "shared/common.txt",
                content: SHARED_DOCUMENT.to_vec(),
                digest: "e1bbbbc9dc8b32d986d3f7b318e7d4ead364a3db7b5740a0a943e0734239564a",
            },
        ],
    )
}

/// All three published test users.
#[must_use]
pub fn everyone() -> Vec<TestUser> {
    vec![alice(), bob(), carol()]
}

/// BLAKE3 digest over the whole corpus: every user, path and byte.
///
/// One value a reader can check by hand to confirm the test data has not been
/// altered. Republished in `docs/TEST-USERS.md`.
#[must_use]
pub fn corpus_digest() -> String {
    let mut hasher = blake3::Hasher::new_derive_key("itsanas test corpus digest");
    for user in everyone() {
        hasher.update(user.username.as_bytes());
        hasher.update(b"\0");
        for file in &user.files {
            hasher.update(file.path.as_bytes());
            hasher.update(b"\0");
            hasher.update(&(file.content.len() as u64).to_le_bytes());
            hasher.update(blake3::hash(&file.content).as_bytes());
        }
    }
    hasher.finalize().to_hex().to_string()
}

/// Pinned value of [`corpus_digest`].
pub const CORPUS_DIGEST: &str = "72a9f85576aaf16ecfe6a7ad8079c00690a03a9d7c9d3aec9ec05895ca88ae02";

#[cfg(test)]
mod tests {
    use super::*;
    use itsanas_crypto::is_published_test_identity;

    /// Every fixture file must hash to the digest pinned beside it.
    ///
    /// This is the tamper check: the corpus is generated from seeds in this
    /// file, so the only way to change the test data is to edit reviewed source
    /// — and doing so moves a digest that is also published in the docs.
    #[test]
    fn corpus_matches_its_published_digests() {
        for user in everyone() {
            for file in &user.files {
                assert_eq!(
                    file.actual_digest(),
                    file.digest,
                    "content of {}/{} does not match its pinned digest",
                    user.username,
                    file.path
                );
            }
        }
        assert_eq!(
            corpus_digest(),
            CORPUS_DIGEST,
            "the corpus as a whole changed"
        );
    }

    /// The published identities must be exactly the ones production bans.
    #[test]
    fn every_fixture_identity_is_banned_in_production() {
        for user in everyone() {
            assert!(
                is_published_test_identity(&user.keys.user_id()),
                "{} is usable in production despite having a published key",
                user.username
            );
        }
    }

    /// A phrase printed in the docs must rebuild the exact identity it claims.
    #[test]
    fn recovery_phrases_rebuild_the_documented_identities() {
        for user in everyone() {
            let restored = MasterSecret::from_recovery_phrase(&user.recovery_phrase).unwrap();
            assert!(restored == user.master, "{} phrase mismatch", user.username);
            assert_eq!(
                UserKeys::derive(&restored).user_id(),
                user.keys.user_id(),
                "{} recovers to a different identity",
                user.username
            );
        }
    }

    /// Canaries must be unique, and must genuinely appear in the plaintext —
    /// otherwise the "no plaintext on a host's disk" tests would pass
    /// vacuously.
    #[test]
    fn canaries_are_unique_and_actually_present_in_plaintext() {
        let users = everyone();

        for user in &users {
            let hits = user
                .files
                .iter()
                .filter(|f| String::from_utf8_lossy(&f.content).contains(user.canary))
                .count();
            assert!(
                hits > 0,
                "{}'s canary appears in none of their files, so any test \
                 searching a host's disk for it would pass without proving \
                 anything",
                user.username
            );
        }

        for user in &users {
            for other in &users {
                if user.username == other.username {
                    continue;
                }
                assert_ne!(user.canary, other.canary);
                for file in &other.files {
                    assert!(
                        !String::from_utf8_lossy(&file.content).contains(user.canary),
                        "{}'s canary leaked into {}'s fixture data",
                        user.username,
                        other.username
                    );
                }
            }
        }
    }

    /// Identical bytes held by different users must never share a chunk
    /// address. This is the blinding guarantee, checked on real corpus data.
    #[test]
    fn the_shared_document_gets_a_different_address_for_every_user() {
        let users = everyone();

        for user in &users {
            let shared = user.file("shared/common.txt").expect("every user holds it");
            assert_eq!(shared.content, SHARED_DOCUMENT);
        }

        for (i, user) in users.iter().enumerate() {
            for other in users.iter().skip(i + 1) {
                assert_ne!(
                    user.keys.chunk_id(SHARED_DOCUMENT),
                    other.keys.chunk_id(SHARED_DOCUMENT),
                    "{} and {} address identical content identically, so a host \
                     storing both could tell they hold the same file",
                    user.username,
                    other.username
                );
            }
        }
    }

    /// Filler must be deterministic, or every digest above is meaningless.
    #[test]
    fn filler_is_deterministic_and_seed_separated() {
        assert_eq!(filler("seed", 64), filler("seed", 64));
        assert_ne!(filler("seed-a", 64), filler("seed-b", 64));
        assert_eq!(filler("seed", 0).len(), 0);
        assert_eq!(filler("seed", 1000).len(), 1000);
    }

    /// The corpus should exercise the awkward sizes, not just tidy ones.
    #[test]
    fn the_corpus_covers_edge_case_sizes() {
        let users = everyone();
        let sizes: Vec<usize> = users
            .iter()
            .flat_map(|u| u.files.iter().map(|f| f.content.len()))
            .collect();

        assert!(sizes.contains(&0), "no empty file in the corpus");
        assert!(
            sizes.iter().any(|s| *s > 512 * 1024),
            "no file large enough to span many chunks"
        );
        assert!(
            sizes.iter().any(|s| s % 1024 != 0),
            "every file is a round number of kibibytes, which would hide \
             off-by-one bugs at chunk boundaries"
        );
    }
}
