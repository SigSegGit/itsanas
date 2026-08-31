//! User and device identity, and the key schedule hanging off the master
//! secret.
//!
//! The whole trust model of ITSaNAS reduces to one sentence: a host stores
//! bytes it cannot interpret. That works because the only thing that can turn
//! stored bytes back into files is a [`UserKeys`], and a `UserKeys` exists only
//! on machines that hold the [`MasterSecret`].

use core::fmt;

use ed25519_dalek::{Signer as _, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey as AgreementPublic, StaticSecret as AgreementSecret};
use zeroize::{Zeroize as _, Zeroizing};

use crate::{
    error::{CryptoError, Result},
    ids::{ChunkId, DeviceId, ID_LEN, UserId},
    kdf,
    seal::{self, SealContext},
    secret::{SecretBytes, SymmetricKey},
};

/// Number of words in an ITSaNAS recovery phrase. 24 words encodes 256 bits of
/// entropy, matching the master secret exactly — no stretching, no loss.
pub const RECOVERY_PHRASE_WORDS: usize = 24;

/// The one secret that matters.
///
/// Everything a user can ever decrypt descends from these 32 bytes. Lose them
/// and the data is gone; leak them and every host storing your chunks can read
/// them. This is the value the recovery phrase encodes.
#[derive(Clone, PartialEq, Eq)]
pub struct MasterSecret(SymmetricKey);

impl MasterSecret {
    /// Draw a fresh master secret from the operating system CSPRNG.
    pub fn generate() -> Result<Self> {
        Ok(Self(SecretBytes::random()?))
    }

    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(SecretBytes::new(bytes))
    }

    #[must_use]
    pub const fn expose(&self) -> &[u8; 32] {
        self.0.expose()
    }

    /// Render as a 24-word BIP-39 English recovery phrase.
    ///
    /// The phrase *is* the master secret in another encoding, so the returned
    /// [`Zeroizing<String>`] wipes its heap buffer on drop. That is not
    /// airtight — `bip39` builds intermediate `String`s this crate cannot
    /// reach, and any `String` can be reallocated by growth, leaving copies
    /// behind — but it removes the longest-lived copy, which is the one a core
    /// dump or a swapped-out page is most likely to catch.
    pub fn to_recovery_phrase(&self) -> Result<Zeroizing<String>> {
        bip39::Mnemonic::from_entropy(self.0.expose())
            .map(|mnemonic| Zeroizing::new(mnemonic.to_string()))
            .map_err(|err| CryptoError::BadMnemonic(err.to_string()))
    }

    /// Rebuild a master secret from its recovery phrase.
    ///
    /// This is the path a user takes on a brand-new machine after losing every
    /// device they own, so it accepts the phrase with any interior whitespace
    /// and in any case.
    pub fn from_recovery_phrase(phrase: &str) -> Result<Self> {
        // BIP-39 itself is strict about case and spacing. Someone copying 24
        // words off a sheet of paper is not, so normalise before parsing: the
        // checksum still catches every genuine transcription error.
        let normalised = phrase
            .split_whitespace()
            .map(str::to_lowercase)
            .collect::<Vec<_>>()
            .join(" ");

        let mnemonic = bip39::Mnemonic::parse(normalised)
            .map_err(|err| CryptoError::BadMnemonic(err.to_string()))?;

        if mnemonic.word_count() != RECOVERY_PHRASE_WORDS {
            return Err(CryptoError::BadMnemonic(format!(
                "expected {RECOVERY_PHRASE_WORDS} words, found {}",
                mnemonic.word_count()
            )));
        }

        let (mut entropy, len) = mnemonic.to_entropy_array();
        let result = SecretBytes::<32>::from_slice(&entropy[..len]).map(Self);
        entropy.zeroize();
        result
    }
}

impl fmt::Debug for MasterSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MasterSecret(redacted)")
    }
}

/// A domain-separated Ed25519 signature.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature(#[serde(with = "signature_bytes")] [u8; 64]);

mod signature_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8; 64], ser: S) -> Result<S::Ok, S::Error> {
        if ser.is_human_readable() {
            let mut hex = String::with_capacity(128);
            for byte in bytes {
                use core::fmt::Write as _;
                let _ = write!(hex, "{byte:02x}");
            }
            ser.serialize_str(&hex)
        } else {
            ser.serialize_bytes(bytes)
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<[u8; 64], D::Error> {
        let raw = <serde_bytes_64::Raw>::deserialize(de)?;
        Ok(raw.0)
    }

    mod serde_bytes_64 {
        use core::fmt;

        use serde::{Deserialize, Deserializer, de};

        pub struct Raw(pub [u8; 64]);

        impl<'de> Deserialize<'de> for Raw {
            fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
                struct Visitor;

                impl<'de> de::Visitor<'de> for Visitor {
                    type Value = Raw;

                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        write!(f, "a 64-byte signature")
                    }

                    fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                        if value.len() != 128 {
                            return Err(E::invalid_length(value.len(), &self));
                        }
                        let mut out = [0u8; 64];
                        // The length check above makes the split exact.
                        let (pairs, _) = value.as_bytes().as_chunks::<2>();
                        for (slot, &[high, low]) in out.iter_mut().zip(pairs) {
                            let hi = (high as char)
                                .to_digit(16)
                                .ok_or_else(|| E::custom("non-hex digit in signature"))?;
                            let lo = (low as char)
                                .to_digit(16)
                                .ok_or_else(|| E::custom("non-hex digit in signature"))?;
                            *slot = u8::try_from(hi * 16 + lo).expect("nibbles fit in a byte");
                        }
                        Ok(Raw(out))
                    }

                    fn visit_bytes<E: de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
                        value
                            .try_into()
                            .map(Raw)
                            .map_err(|_| E::invalid_length(value.len(), &self))
                    }

                    fn visit_seq<A: de::SeqAccess<'de>>(
                        self,
                        mut seq: A,
                    ) -> Result<Self::Value, A::Error> {
                        let mut out = [0u8; 64];
                        for (index, slot) in out.iter_mut().enumerate() {
                            *slot = seq
                                .next_element()?
                                .ok_or_else(|| de::Error::invalid_length(index, &self))?;
                        }
                        Ok(Raw(out))
                    }
                }

                if de.is_human_readable() {
                    de.deserialize_str(Visitor)
                } else {
                    de.deserialize_bytes(Visitor)
                }
            }
        }
    }
}

impl Signature {
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 64] {
        self.0
    }

    #[must_use]
    pub const fn from_bytes(bytes: [u8; 64]) -> Self {
        Self(bytes)
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Signature({:02x}{:02x}{:02x}{:02x}..)",
            self.0[0], self.0[1], self.0[2], self.0[3]
        )
    }
}

/// Bind a message to a purpose before signing it.
///
/// Without this, a signature produced over a storage receipt could be replayed
/// as a signature over a device certificate. The domain is length-prefixed so
/// `("ab", "c")` and `("a", "bc")` cannot hash to the same digest.
#[must_use]
pub fn message_digest(domain: &str, message: &[u8]) -> [u8; 32] {
    let domain_len =
        u32::try_from(domain.len()).expect("signature domains are compile-time constants");
    blake3::Hasher::new_derive_key(kdf::CTX_SIGNED_MESSAGE)
        .update(&domain_len.to_le_bytes())
        .update(domain.as_bytes())
        .update(message)
        .finalize()
        .into()
}

/// Verify a domain-separated signature against a public identity.
pub fn verify(
    public: &[u8; ID_LEN],
    domain: &str,
    message: &[u8],
    signature: Signature,
) -> Result<()> {
    let verifying = VerifyingKey::from_bytes(public).map_err(|_| CryptoError::BadSignature)?;
    let digest = message_digest(domain, message);
    verifying
        .verify_strict(
            &digest,
            &ed25519_dalek::Signature::from_bytes(&signature.to_bytes()),
        )
        .map_err(|_| CryptoError::BadSignature)
}

/// The public half of an identity: what a user publishes to the coordinator and
/// what peers pin.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct UserPublic {
    pub id: UserId,
    /// X25519 public key, used to wrap secrets to this user.
    pub agreement: [u8; 32],
}

/// The full key schedule for one user, derived from their master secret.
pub struct UserKeys {
    master: MasterSecret,
    signing: SigningKey,
    agreement: AgreementSecret,
    chunk_root: SymmetricKey,
    blinding: SymmetricKey,
    oplog_root: SymmetricKey,
    audit_order: SymmetricKey,
}

impl UserKeys {
    /// Derive every subkey. Deterministic: the same master secret always yields
    /// the same identity, which is what makes recovery from a phrase work.
    #[must_use]
    pub fn derive(master: &MasterSecret) -> Self {
        let material = master.expose();
        let signing_seed = kdf::derive(kdf::CTX_USER_SIGNING, material);
        let agreement_seed = kdf::derive(kdf::CTX_USER_AGREEMENT, material);

        Self {
            master: master.clone(),
            signing: SigningKey::from_bytes(signing_seed.expose()),
            agreement: AgreementSecret::from(*agreement_seed.expose()),
            chunk_root: kdf::derive(kdf::CTX_USER_CHUNK_DATA, material),
            blinding: kdf::derive(kdf::CTX_USER_BLINDING, material),
            oplog_root: kdf::derive(kdf::CTX_USER_OPLOG, material),
            audit_order: kdf::derive(kdf::CTX_USER_AUDIT_ORDER, material),
        }
    }

    #[must_use]
    pub fn user_id(&self) -> UserId {
        UserId::from_bytes(self.signing.verifying_key().to_bytes())
    }

    #[must_use]
    pub fn public(&self) -> UserPublic {
        UserPublic {
            id: self.user_id(),
            agreement: AgreementPublic::from(&self.agreement).to_bytes(),
        }
    }

    #[must_use]
    pub const fn master(&self) -> &MasterSecret {
        &self.master
    }

    #[must_use]
    pub const fn chunk_root(&self) -> &SymmetricKey {
        &self.chunk_root
    }

    #[must_use]
    pub const fn oplog_root(&self) -> &SymmetricKey {
        &self.oplog_root
    }

    /// Sign a message under a purpose-specific domain.
    #[must_use]
    pub fn sign(&self, domain: &str, message: &[u8]) -> Signature {
        let digest = message_digest(domain, message);
        Signature(self.signing.sign(&digest).to_bytes())
    }

    /// Compute the storage address of a chunk from its plaintext.
    ///
    /// Blinded by this user's secret key, so it is stable for them (enabling
    /// deduplication of repeated content within their own data) while revealing
    /// nothing to the host that stores it. Two users backing up byte-identical
    /// files produce entirely unrelated chunk identifiers.
    #[must_use]
    pub fn chunk_id(&self, plaintext: &[u8]) -> ChunkId {
        let content = blake3::hash(plaintext);
        ChunkId::from_bytes(
            *blake3::keyed_hash(self.blinding.expose(), content.as_bytes()).as_bytes(),
        )
    }

    /// This user's key for scrambling the audit order of a peer's holdings.
    ///
    /// Handed to the placement ledger, which orders each peer's records under a
    /// keyed hash of the chunk id rather than under the chunk id itself. See
    /// [`kdf::CTX_USER_AUDIT_ORDER`] for why that is the difference between a
    /// host being able to choose what to delete and not.
    #[must_use]
    pub const fn audit_order_key(&self) -> &SymmetricKey {
        &self.audit_order
    }

    /// Derive a shared secret with another user, for wrapping keys to them.
    ///
    /// # Errors
    ///
    /// Returns [`CryptoError::Malformed`] if the peer's key is a low-order
    /// point. X25519 accepts such points and produces the identity — 32 zero
    /// bytes — as the "shared" secret. Since the coordinator is explicitly
    /// untrusted and is the thing that hands out other users' public keys, a
    /// malicious one could otherwise serve an all-zero agreement key and know
    /// every secret subsequently wrapped to that peer.
    pub fn agree(&self, their_agreement: &[u8; 32]) -> Result<SymmetricKey> {
        let shared = self
            .agreement
            .diffie_hellman(&AgreementPublic::from(*their_agreement));

        if !shared.was_contributory() {
            return Err(CryptoError::Malformed(
                "agreement key is a low-order point; the shared secret would be \
                 attacker-known",
            ));
        }

        // A raw Diffie-Hellman output and a long-term static secret are
        // different kinds of material, so they get different contexts. Sharing
        // one would mean a value derived from a peer-influenced exchange lands
        // in the same key space as one derived from the master secret alone.
        Ok(kdf::derive(kdf::CTX_USER_DH_OUTPUT, shared.as_bytes()))
    }

    /// Seal a file chunk at the address derived from its own content.
    ///
    /// This is the API every caller should use. [`seal::seal_deterministic`]
    /// accepts an arbitrary address, and its fixed-nonce construction is only
    /// safe while that address is a function of the plaintext; deriving the
    /// address here makes the unsafe combination unreachable rather than merely
    /// discouraged.
    ///
    /// Returns the blinded address and the sealed bytes. Identical content
    /// always yields both identically, which is what makes deduplication and
    /// remote storage audits work.
    pub fn seal_chunk(&self, plaintext: &[u8]) -> Result<(ChunkId, Vec<u8>)> {
        let address = self.chunk_id(plaintext);
        let sealed = seal::seal_deterministic(
            &self.chunk_root,
            &SealContext {
                purpose: seal::CHUNK_PURPOSE,
                owner: self.user_id(),
                address: address.as_bytes(),
            },
            plaintext,
        )?;
        Ok((address, sealed))
    }

    /// Open a chunk sealed by [`Self::seal_chunk`].
    ///
    /// Fails if `sealed` was not the chunk stored at `address` — which is
    /// exactly what a host substituting one chunk for another produces.
    pub fn open_chunk(&self, address: &ChunkId, sealed: &[u8]) -> Result<Vec<u8>> {
        seal::open_deterministic(
            &self.chunk_root,
            &SealContext {
                purpose: seal::CHUNK_PURPOSE,
                owner: self.user_id(),
                address: address.as_bytes(),
            },
            sealed,
        )
    }
}

impl fmt::Debug for UserKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UserKeys")
            .field("user_id", &self.user_id())
            .finish_non_exhaustive()
    }
}

/// A single machine's signing key.
///
/// Generated locally and never derived from the master secret, so that revoking
/// a lost laptop is a matter of dropping one certificate rather than rotating
/// the user's entire identity.
pub struct DeviceKeys {
    signing: SigningKey,
}

impl DeviceKeys {
    pub fn generate() -> Result<Self> {
        let seed = SecretBytes::<32>::random()?;
        Ok(Self {
            signing: SigningKey::from_bytes(seed.expose()),
        })
    }

    #[must_use]
    pub fn from_seed(seed: &SymmetricKey) -> Self {
        Self {
            signing: SigningKey::from_bytes(seed.expose()),
        }
    }

    #[must_use]
    pub fn device_id(&self) -> DeviceId {
        DeviceId::from_bytes(self.signing.verifying_key().to_bytes())
    }

    #[must_use]
    pub fn sign(&self, domain: &str, message: &[u8]) -> Signature {
        let digest = message_digest(domain, message);
        Signature(self.signing.sign(&digest).to_bytes())
    }

    /// The seed bytes, for persisting the device key in the local keystore.
    #[must_use]
    pub fn seed(&self) -> SymmetricKey {
        SecretBytes::new(*self.signing.as_bytes())
    }
}

impl fmt::Debug for DeviceKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeviceKeys")
            .field("device_id", &self.device_id())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn master(byte: u8) -> MasterSecret {
        MasterSecret::from_bytes([byte; 32])
    }

    #[test]
    fn recovery_phrase_round_trips() {
        let original = MasterSecret::generate().unwrap();
        let phrase = original.to_recovery_phrase().unwrap();

        assert_eq!(phrase.split_whitespace().count(), RECOVERY_PHRASE_WORDS);
        assert_eq!(
            MasterSecret::from_recovery_phrase(&phrase).unwrap(),
            original
        );
    }

    #[test]
    fn recovery_phrase_tolerates_untidy_input() {
        let original = MasterSecret::generate().unwrap();
        let phrase = original.to_recovery_phrase().unwrap();
        let untidy = format!("  {}  ", phrase.to_uppercase().replace(' ', "\n"));

        assert_eq!(
            MasterSecret::from_recovery_phrase(&untidy).unwrap(),
            original,
            "a user retyping their phrase from paper must not be defeated by \
             capitalisation or line breaks"
        );
    }

    #[test]
    fn a_corrupted_recovery_phrase_is_rejected_not_silently_accepted() {
        let phrase = MasterSecret::generate()
            .unwrap()
            .to_recovery_phrase()
            .unwrap();
        let mut words: Vec<&str> = phrase.split_whitespace().collect();

        // Swap two words: still all-valid vocabulary, but the checksum must fail.
        words.swap(0, 1);
        let swapped = words.join(" ");
        if swapped != *phrase {
            assert!(
                MasterSecret::from_recovery_phrase(&swapped).is_err(),
                "a transposed recovery phrase decoded successfully, so a user \
                 typo would silently restore the wrong identity"
            );
        }

        assert!(MasterSecret::from_recovery_phrase("not even words").is_err());
        assert!(MasterSecret::from_recovery_phrase("").is_err());
    }

    #[test]
    fn short_phrases_are_rejected() {
        // A valid 12-word BIP-39 phrase carries only 128 bits; we require 256.
        let twelve = bip39::Mnemonic::from_entropy(&[0u8; 16])
            .unwrap()
            .to_string();
        assert_eq!(twelve.split_whitespace().count(), 12);
        assert!(MasterSecret::from_recovery_phrase(&twelve).is_err());
    }

    #[test]
    fn identity_is_a_pure_function_of_the_master_secret() {
        let a = UserKeys::derive(&master(9));
        let b = UserKeys::derive(&master(9));
        assert_eq!(a.user_id(), b.user_id());
        assert_eq!(a.public(), b.public());
    }

    #[test]
    fn different_masters_give_different_identities() {
        let a = UserKeys::derive(&master(1));
        let b = UserKeys::derive(&master(2));
        assert_ne!(a.user_id(), b.user_id());
        assert_ne!(a.public().agreement, b.public().agreement);
    }

    #[test]
    fn signatures_verify_and_reject_tampering() {
        let keys = UserKeys::derive(&master(3));
        let signature = keys.sign("test/domain", b"payload");

        assert!(
            verify(
                keys.user_id().as_bytes(),
                "test/domain",
                b"payload",
                signature
            )
            .is_ok()
        );
        assert!(
            verify(
                keys.user_id().as_bytes(),
                "test/domain",
                b"payloae",
                signature
            )
            .is_err()
        );

        let mut forged = signature.to_bytes();
        forged[0] ^= 1;
        assert!(
            verify(
                keys.user_id().as_bytes(),
                "test/domain",
                b"payload",
                Signature::from_bytes(forged)
            )
            .is_err()
        );
    }

    #[test]
    fn a_signature_cannot_be_replayed_under_another_domain() {
        let keys = UserKeys::derive(&master(4));
        let signature = keys.sign("oplog/head", b"same bytes");

        assert!(
            verify(
                keys.user_id().as_bytes(),
                "oplog/head",
                b"same bytes",
                signature
            )
            .is_ok()
        );
        assert!(
            verify(
                keys.user_id().as_bytes(),
                "device/certificate",
                b"same bytes",
                signature
            )
            .is_err(),
            "a head signature was accepted as a device certificate; domain \
             separation is not working"
        );
    }

    #[test]
    fn domain_prefix_is_unambiguous() {
        // Without length-prefixing, ("ab","c") and ("a","bc") would collide.
        assert_ne!(message_digest("ab", b"c"), message_digest("a", b"bc"));
    }

    #[test]
    fn one_users_signature_does_not_verify_under_another() {
        let alice = UserKeys::derive(&master(5));
        let bob = UserKeys::derive(&master(6));
        let signature = alice.sign("d", b"m");

        assert!(verify(bob.user_id().as_bytes(), "d", b"m", signature).is_err());
    }

    #[test]
    fn chunk_ids_deduplicate_within_a_user_but_not_across_users() {
        let alice = UserKeys::derive(&master(7));
        let bob = UserKeys::derive(&master(8));
        let content = b"the exact same holiday photo";

        assert_eq!(
            alice.chunk_id(content),
            alice.chunk_id(content),
            "identical content must map to one address so deduplication works"
        );
        assert_ne!(
            alice.chunk_id(content),
            bob.chunk_id(content),
            "two users storing the same file produced the same chunk id, which \
             would let a host prove they hold identical content"
        );
        assert_ne!(
            alice.chunk_id(content),
            alice.chunk_id(b"different content")
        );
    }

    #[test]
    fn chunk_id_does_not_expose_the_plaintext_hash() {
        let keys = UserKeys::derive(&master(10));
        let content = b"guessable content";
        let plain_hash = blake3::hash(content);

        assert_ne!(
            keys.chunk_id(content).as_bytes(),
            plain_hash.as_bytes(),
            "chunk id equals the unblinded content hash, so a host holding a \
             guess of the plaintext could confirm it"
        );
    }

    #[test]
    fn diffie_hellman_agrees_in_both_directions() {
        let alice = UserKeys::derive(&master(11));
        let bob = UserKeys::derive(&master(12));

        assert_eq!(
            alice.agree(&bob.public().agreement).unwrap(),
            bob.agree(&alice.public().agreement).unwrap()
        );
    }

    #[test]
    fn a_low_order_agreement_key_is_refused() {
        // The coordinator is untrusted and it is what hands out other users'
        // public keys. If it could serve a low-order point, X25519 would return
        // the identity — 32 zero bytes — and the coordinator would know every
        // secret wrapped to that peer.
        let alice = UserKeys::derive(&master(13));

        // The canonical small-order Curve25519 points.
        let all_zero = [0u8; 32];
        let one = {
            let mut point = [0u8; 32];
            point[0] = 1;
            point
        };
        let order_eight = [
            0xe0, 0xeb, 0x7a, 0x7c, 0x3b, 0x41, 0xb8, 0xae, 0x16, 0x56, 0xe3, 0xfa, 0xf1, 0x9f,
            0xc4, 0x6a, 0xda, 0x09, 0x8d, 0xeb, 0x9c, 0x32, 0xb1, 0xfd, 0x86, 0x62, 0x05, 0x16,
            0x5f, 0x49, 0xb8, 0x00,
        ];

        for (name, point) in [
            ("all-zero", all_zero),
            ("one", one),
            ("order-eight", order_eight),
        ] {
            assert!(
                alice.agree(&point).is_err(),
                "the {name} low-order point was accepted as an agreement key; \
                 the resulting 'shared' secret is known to the attacker"
            );
        }

        // A genuine peer key still works.
        let bob = UserKeys::derive(&master(14));
        assert!(alice.agree(&bob.public().agreement).is_ok());
    }

    #[test]
    fn the_shared_secret_is_not_the_raw_diffie_hellman_output() {
        // A raw DH result and a long-term secret must not share a derivation
        // context, or a peer-influenced value lands in the same key space as
        // one derived from the master secret alone.
        let alice = UserKeys::derive(&master(15));
        let bob = UserKeys::derive(&master(16));

        let agreed = alice.agree(&bob.public().agreement).unwrap();
        let raw = x25519_dalek::StaticSecret::from(
            *kdf::derive(kdf::CTX_USER_AGREEMENT, master(15).expose()).expose(),
        )
        .diffie_hellman(&AgreementPublic::from(bob.public().agreement));

        assert_ne!(
            agreed.expose(),
            raw.as_bytes(),
            "the wrapping key is the unhashed Diffie-Hellman output"
        );
    }

    #[test]
    fn device_keys_are_independent_of_the_user_master() {
        let a = DeviceKeys::generate().unwrap();
        let b = DeviceKeys::generate().unwrap();
        assert_ne!(a.device_id(), b.device_id());

        let restored = DeviceKeys::from_seed(&a.seed());
        assert_eq!(restored.device_id(), a.device_id());
    }

    #[test]
    fn secrets_are_redacted_in_debug_output() {
        let secret = MasterSecret::from_bytes([0xCD; 32]);
        assert_eq!(format!("{secret:?}"), "MasterSecret(redacted)");

        let keys = UserKeys::derive(&secret);
        let rendered = format!("{keys:?}");
        assert!(rendered.contains("user_id"));
        assert!(!rendered.contains("cdcdcdcdcdcdcdcd"));
    }
}
