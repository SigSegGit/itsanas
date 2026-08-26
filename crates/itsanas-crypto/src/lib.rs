//! Cryptographic foundation for ITSaNAS.
//!
//! ITSaNAS lets people trade disk space with each other. The bargain only works
//! if hosting someone's data grants you no ability to read it, and this crate is
//! where that guarantee lives. Everything above it — the store, the sync engine,
//! the network layer — moves bytes that have already been sealed here.
//!
//! # The one-paragraph threat model
//!
//! A host is assumed to be *honest but curious at best, and actively malicious
//! at worst*. It may read every byte it stores, keep copies forever, return
//! stale or corrupted chunks, serve one chunk when asked for another, lie about
//! what it holds, or collude with other hosts. It may not, at any point, learn
//! anything about the plaintext beyond its approximate size and access pattern.
//! The coordinator is assumed to be able to lie about who exists and who is
//! online; it is never trusted with data or keys.
//!
//! # What that buys, concretely
//!
//! * Chunk contents are sealed with XChaCha20-Poly1305 under keys that only the
//!   owner can derive ([`seal`]).
//! * Chunk *addresses* are blinded, so a host cannot confirm a guessed
//!   plaintext or spot that two users hold the same file ([`identity::UserKeys::chunk_id`]).
//! * Ciphertext is bound to its owner, purpose and address, so substitution is
//!   detected rather than silently tolerated ([`seal::SealContext`]).
//! * Every signature is domain-separated, so no signature can be replayed in a
//!   context it was not issued for ([`identity::message_digest`]).
//!
//! # What it explicitly does not hide
//!
//! Object sizes, object counts, and the timing of reads and writes are visible
//! to a host. Hiding those needs padding and cover traffic, which is a
//! deliberate non-goal for now.
//!
//! # Key schedule
//!
//! ```text
//! recovery phrase (24 BIP-39 words)
//!   └── master secret (32 bytes)
//!         ├── Ed25519 signing key ──── user id, signatures
//!         ├── X25519 agreement key ─── wrapping secrets to other users
//!         ├── chunk data root ──────── per-chunk keys and nonces
//!         ├── blinding key ─────────── chunk ids that leak nothing
//!         └── oplog root ───────────── per-segment keys for the sync log
//! ```
//!
//! Device keys sit deliberately outside this tree: they are generated locally
//! and certified by the master key, so a stolen laptop is revoked by dropping
//! one certificate rather than by rotating the user's whole identity.

pub mod error;
pub mod identity;
pub mod ids;
pub mod kdf;
pub mod keystore;
pub mod seal;
pub mod secret;
pub mod wellknown;

pub use error::{CryptoError, Result};
pub use identity::{
    DeviceKeys, MasterSecret, RECOVERY_PHRASE_WORDS, Signature, UserKeys, UserPublic,
    message_digest, verify,
};
pub use ids::{ChunkId, DeviceId, ID_LEN, ObjectId, UserId};
pub use keystore::{KdfParams, Keystore};
pub use seal::{SealContext, open_deterministic, open_random, seal_deterministic, seal_random};
pub use secret::{SecretBytes, SymmetricKey};
pub use wellknown::{PUBLISHED_TEST_USER_IDS, is_published_test_identity};
