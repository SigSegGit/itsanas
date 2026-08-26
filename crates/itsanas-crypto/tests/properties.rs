//! Property tests for the crypto core.
//!
//! The unit tests pin down specific known-dangerous cases. These check that the
//! same guarantees hold across randomly generated inputs, which is where
//! encoding and length-handling bugs tend to hide.

use itsanas_crypto::{
    ChunkId, KdfParams, Keystore, MasterSecret, SealContext, UserId, UserKeys, open_deterministic,
    open_random, seal_deterministic, seal_random, verify,
};
use proptest::prelude::*;

const TEST_KDF: KdfParams = KdfParams::INSECURE_FOR_TESTS;

fn keys(seed: [u8; 32]) -> UserKeys {
    UserKeys::derive(&MasterSecret::from_bytes(seed))
}

fn chunk_ctx(owner: UserId, address: &[u8]) -> SealContext<'_> {
    SealContext {
        purpose: "chunk",
        owner,
        address,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Anything sealed must come back out byte-identical, at any length.
    #[test]
    fn sealing_round_trips_for_any_plaintext(
        seed in any::<[u8; 32]>(),
        address in prop::collection::vec(any::<u8>(), 0..64),
        plaintext in prop::collection::vec(any::<u8>(), 0..4096),
    ) {
        let user = keys(seed);
        let context = chunk_ctx(user.user_id(), &address);

        let sealed = seal_deterministic(user.chunk_root(), &context, &plaintext).unwrap();
        prop_assert_eq!(open_deterministic(user.chunk_root(), &context, &sealed).unwrap(), plaintext.clone());

        let sealed = seal_random(user.oplog_root(), &context, &plaintext).unwrap();
        prop_assert_eq!(open_random(user.oplog_root(), &context, &sealed).unwrap(), plaintext);
    }

    /// The core promise: hosting someone's bytes grants no read access.
    #[test]
    fn a_host_can_never_open_what_it_stores(
        owner_seed in any::<[u8; 32]>(),
        host_seed in any::<[u8; 32]>(),
        plaintext in prop::collection::vec(any::<u8>(), 1..1024),
    ) {
        prop_assume!(owner_seed != host_seed);

        let owner = keys(owner_seed);
        let host = keys(host_seed);
        let context = chunk_ctx(owner.user_id(), b"address");

        let sealed = seal_deterministic(owner.chunk_root(), &context, &plaintext).unwrap();

        prop_assert!(
            open_deterministic(host.chunk_root(), &context, &sealed).is_err(),
            "a host decrypted stored data"
        );
        prop_assert!(
            open_deterministic(host.oplog_root(), &context, &sealed).is_err()
        );
    }

    /// Corrupting any single byte of a sealed object must be caught.
    #[test]
    fn any_single_byte_corruption_is_detected(
        seed in any::<[u8; 32]>(),
        plaintext in prop::collection::vec(any::<u8>(), 1..512),
        index in any::<prop::sample::Index>(),
        delta in 1u8..=255,
    ) {
        let user = keys(seed);
        let context = chunk_ctx(user.user_id(), b"address");
        let sealed = seal_deterministic(user.chunk_root(), &context, &plaintext).unwrap();

        let mut tampered = sealed.clone();
        let position = index.index(tampered.len());
        tampered[position] = tampered[position].wrapping_add(delta);

        prop_assert!(
            open_deterministic(user.chunk_root(), &context, &tampered).is_err(),
            "corruption at byte {} went undetected",
            position
        );
    }

    /// Truncating a sealed object at any point must be caught.
    #[test]
    fn truncation_at_any_point_is_detected(
        seed in any::<[u8; 32]>(),
        plaintext in prop::collection::vec(any::<u8>(), 1..512),
        index in any::<prop::sample::Index>(),
    ) {
        let user = keys(seed);
        let context = chunk_ctx(user.user_id(), b"address");
        let sealed = seal_deterministic(user.chunk_root(), &context, &plaintext).unwrap();

        let cut = index.index(sealed.len());
        prop_assert!(open_deterministic(user.chunk_root(), &context, &sealed[..cut]).is_err());
    }

    /// Chunk ids must be a stable function of content, and never collide across
    /// distinct content within one user.
    #[test]
    fn chunk_ids_are_stable_and_content_separating(
        seed in any::<[u8; 32]>(),
        left in prop::collection::vec(any::<u8>(), 0..512),
        right in prop::collection::vec(any::<u8>(), 0..512),
    ) {
        let user = keys(seed);

        prop_assert_eq!(user.chunk_id(&left), user.chunk_id(&left));
        if left != right {
            prop_assert_ne!(user.chunk_id(&left), user.chunk_id(&right));
        }
    }

    /// Two users must never derive the same address for the same content, or a
    /// host could tell that they hold identical files.
    #[test]
    fn chunk_ids_never_align_across_users(
        left_seed in any::<[u8; 32]>(),
        right_seed in any::<[u8; 32]>(),
        content in prop::collection::vec(any::<u8>(), 0..512),
    ) {
        prop_assume!(left_seed != right_seed);
        prop_assert_ne!(keys(left_seed).chunk_id(&content), keys(right_seed).chunk_id(&content));
    }

    /// Signatures verify for the signer and fail for anyone else, over any
    /// message and any domain.
    #[test]
    fn signatures_bind_signer_domain_and_message(
        signer_seed in any::<[u8; 32]>(),
        other_seed in any::<[u8; 32]>(),
        domain in "[a-z/]{1,32}",
        other_domain in "[a-z/]{1,32}",
        message in prop::collection::vec(any::<u8>(), 0..256),
    ) {
        prop_assume!(signer_seed != other_seed);

        let signer = keys(signer_seed);
        let other = keys(other_seed);
        let signature = signer.sign(&domain, &message);

        prop_assert!(verify(signer.user_id().as_bytes(), &domain, &message, signature).is_ok());
        prop_assert!(verify(other.user_id().as_bytes(), &domain, &message, signature).is_err());

        if domain != other_domain {
            prop_assert!(
                verify(signer.user_id().as_bytes(), &other_domain, &message, signature).is_err()
            );
        }
    }

    /// A recovery phrase must reconstruct the exact master secret it came from.
    #[test]
    fn recovery_phrases_round_trip(seed in any::<[u8; 32]>()) {
        let master = MasterSecret::from_bytes(seed);
        let phrase = master.to_recovery_phrase().unwrap();

        prop_assert_eq!(phrase.split_whitespace().count(), 24);
        prop_assert!(MasterSecret::from_recovery_phrase(&phrase).unwrap() == master);
    }

    /// A mistyped recovery phrase must never reconstruct the original identity,
    /// and must never reconstruct a *partially* correct one.
    ///
    /// Note what is deliberately **not** asserted here: that a transposition
    /// always fails. A 24-word BIP-39 phrase carries 256 bits of entropy and
    /// only an 8-bit checksum, so about one transposition in 256 passes the
    /// checksum and decodes cleanly to a completely different master secret.
    /// That is inherent to BIP-39 and cannot be fixed in this crate.
    ///
    /// The consequence is a real user-facing hazard: someone restoring with a
    /// typo can land on a valid, empty account and conclude their data is gone.
    /// The mitigation lives at the application layer — the CLI must display the
    /// derived user id after recovery, and must refuse to proceed when it does
    /// not match the identity registered for the account. See
    /// `docs/DESIGN.md`, "Recovery must be verified, not assumed".
    #[test]
    fn a_mistyped_recovery_phrase_never_reconstructs_the_original_identity(
        seed in any::<[u8; 32]>(),
        left in any::<prop::sample::Index>(),
        right in any::<prop::sample::Index>(),
    ) {
        let master = MasterSecret::from_bytes(seed);
        let phrase = master.to_recovery_phrase().unwrap();
        let mut words: Vec<&str> = phrase.split_whitespace().collect();

        let (a, b) = (left.index(words.len()), right.index(words.len()));
        prop_assume!(words[a] != words[b]);
        words.swap(a, b);

        if let Ok(recovered) = MasterSecret::from_recovery_phrase(&words.join(" ")) {
            prop_assert!(
                !(recovered == master),
                "two different word orders decoded to the same master secret, \
                 so the phrase encoding is not injective"
            );
            prop_assert_ne!(
                UserKeys::derive(&recovered).user_id(),
                UserKeys::derive(&master).user_id()
            );
        }
    }

    /// The keystore opens with the right passphrase and never with a wrong one.
    #[test]
    fn keystores_round_trip_and_reject_wrong_passphrases(
        passphrase in ".{0,64}",
        wrong in ".{0,64}",
        label in "[a-z/]{1,32}",
        payload in prop::collection::vec(any::<u8>(), 0..256),
    ) {
        let store = Keystore::lock(&passphrase, &label, &payload, TEST_KDF).unwrap();

        prop_assert_eq!(store.unlock(&passphrase, &label).unwrap(), payload);

        if wrong != passphrase {
            prop_assert!(store.unlock(&wrong, &label).is_err());
        }
    }

    /// A keystore is bound to its label, so containers cannot be swapped.
    #[test]
    fn keystores_are_bound_to_their_label(
        passphrase in ".{1,32}",
        label in "[a-z/]{1,32}",
        other_label in "[a-z/]{1,32}",
        payload in prop::collection::vec(any::<u8>(), 1..128),
    ) {
        prop_assume!(label != other_label);

        let store = Keystore::lock(&passphrase, &label, &payload, TEST_KDF).unwrap();
        prop_assert!(store.unlock(&passphrase, &other_label).is_err());
    }

    /// The on-disk keystore encoding must survive a round trip exactly.
    #[test]
    fn keystore_encoding_round_trips(
        passphrase in ".{0,32}",
        payload in prop::collection::vec(any::<u8>(), 0..256),
    ) {
        let store = Keystore::lock(&passphrase, "label", &payload, TEST_KDF).unwrap();
        let reloaded = Keystore::from_bytes(&store.to_bytes()).unwrap();

        prop_assert!(reloaded == store);
        prop_assert_eq!(reloaded.unlock(&passphrase, "label").unwrap(), payload);
    }

    /// Parsing arbitrary bytes as a keystore must never panic.
    #[test]
    fn arbitrary_bytes_never_panic_the_keystore_parser(
        bytes in prop::collection::vec(any::<u8>(), 0..512),
    ) {
        if let Ok(store) = Keystore::from_bytes(&bytes) {
            let _ = store.unlock("whatever", "label");
        }
    }

    /// Identifier hex encoding must round trip for every possible value.
    #[test]
    fn identifier_hex_round_trips(bytes in any::<[u8; 32]>()) {
        let id = ChunkId::from_bytes(bytes);
        prop_assert_eq!(id.to_hex().parse::<ChunkId>().unwrap(), id);
        prop_assert_eq!(id.to_hex().len(), 64);
    }

    /// Parsing arbitrary text as an identifier must never panic.
    #[test]
    fn arbitrary_text_never_panics_the_identifier_parser(text in ".{0,200}") {
        let _ = text.parse::<ChunkId>();
    }
}
