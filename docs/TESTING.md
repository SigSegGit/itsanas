# Test Catalogue

**Last updated: 2026-08-26 — 78 tests across 3 test binaries.**

Every automated test in ITSaNAS is listed here with the property it establishes.
The rule this project holds itself to: **if you cannot state in one sentence
what a test would catch, it should not exist.** A test that passes whether or
not the system works is worse than no test, because it buys false confidence.

Where a security claim is made anywhere in the documentation, there is a test
here that would fail if the claim were false.

## How to run

```bash
cargo test --workspace                    # the fast suite (~9s)
cargo test --workspace -- --ignored       # expensive tests, real cost parameters
cargo clippy --workspace --all-targets -- -D warnings
cargo deny --all-features check           # advisories and licences
```

## What CI runs, and why each job exists

Defined in [`.github/workflows/ci.yml`](../.github/workflows/ci.yml).

| Job | What it runs | Why it is there |
| --- | --- | --- |
| **lint** | `cargo fmt --check`, `cargo clippy -D warnings` | Style drift and lint debt compound. Clippy's `pedantic` set catches real bugs in crypto code — sign confusion, lossy casts, misused ranges. |
| **test** | `cargo test --workspace` on Ubuntu, Windows, macOS | ITSaNAS must run on a Windows laptop and a Linux Pi simultaneously. Path handling, endianness assumptions and filesystem semantics differ; a Linux-only suite would not notice. |
| **slow-tests** | `cargo test -- --ignored --test-threads 1` | Tests marked `#[ignore]` run *production* cost parameters — real 64 MiB Argon2id, and later large simulated swarms. Too slow for every push, far too important to never run. |
| **cross-build** | `cargo build --release --target aarch64-unknown-linux-gnu` | The Raspberry Pi 4B+ is a first-class deployment target. Catching a dependency that does not cross-compile at PR time is much cheaper than at deploy time. |
| **minimum-rust-version** | `cargo check` on the pinned MSRV | Prevents accidentally requiring a newer toolchain than the documented minimum, which would break users on distro Rust. |
| **supply-chain** | `cargo deny check` | Fails on any unpatched advisory, any yanked crate, and any licence not compatible with AGPL-3.0. For a system whose entire value is "your host cannot read your data", a vulnerable crypto dependency is a release blocker. |
| **coverage** | `cargo llvm-cov` | Not a target to game — used to spot whole modules or error paths with no test at all. |

The workflow also runs **weekly on a schedule**, so a newly published advisory
against a dependency surfaces even when nobody has pushed for a month.

---

# `itsanas-crypto` — unit tests (56)

## `secret` — secret hygiene (4)

| Test | What it proves |
| --- | --- |
| `debug_never_reveals_bytes` | Formatting a secret prints `SecretBytes<32>(redacted)` and no key bytes. Daemon logs on an ITSaNAS node are readable by the machine's owner, who is explicitly not the person whose keys those are. |
| `equality_is_value_based` | Secret comparison is by value via constant-time `ct_eq`, so tests comparing keys are meaningful and comparison does not leak timing. |
| `random_secrets_differ` | Two freshly drawn secrets differ and neither is all-zero. Catches a miswired CSPRNG returning a constant — the failure mode that silently makes every key identical. |
| `from_slice_rejects_wrong_length` | 31- and 33-byte inputs are refused. Prevents a truncated key being silently zero-padded into a weak key. |

## `kdf` — key derivation (6)

| Test | What it proves |
| --- | --- |
| `context_strings_are_unique` | No two derivation contexts collide. A collision would mean two different purposes share a key — for example the signing key equalling the data key. |
| `context_strings_carry_a_version` | Every context contains a version marker, so a future v2 key schedule derives entirely different keys instead of silently reinterpreting v1 material. |
| `different_contexts_yield_different_keys` | All pairs of contexts produce distinct keys from identical input. The empirical counterpart to the uniqueness check above. |
| `derivation_is_deterministic` | Same input, same key, every time. This is what makes recovery from a phrase work at all. |
| `one_bit_of_master_change_changes_every_subkey` | Flipping one bit of the master secret changes every derived subkey. Catches a derivation that accidentally ignores part of its input. |
| `expansion_separates_by_label` / `expansion_separates_by_root_key` | Per-object key expansion is separated by both label and root key, so two objects never share a (key, nonce) pair. |

## `ids` — identifier encoding (4)

| Test | What it proves |
| --- | --- |
| `hex_round_trips` | An identifier survives rendering and re-parsing exactly. These strings appear in CLI output, config and the coordinator API. |
| `short_form_is_twelve_chars` | The abbreviated form used in logs is stable, so log output stays greppable across versions. |
| `parsing_rejects_bad_input` | Empty, non-hex, too-short and too-long inputs are all refused rather than being silently padded or truncated into a valid-looking identifier. |
| `hex_rendering_is_lowercase_and_zero_padded` | Encoding is canonical. Without this, `0x05` could render as `5` and produce a 63-character identifier that fails to round trip. |

## `identity` — identity and signatures (13)

| Test | What it proves |
| --- | --- |
| `recovery_phrase_round_trips` | A generated master secret produces a 24-word phrase that reconstructs it exactly. The single most important recovery path in the system. |
| `recovery_phrase_tolerates_untidy_input` | A phrase retyped in capitals, with line breaks and stray whitespace, still works. **This test found a real bug**: raw BIP-39 parsing rejected it, which would have locked out a user typing 24 words off paper correctly. |
| `a_corrupted_recovery_phrase_is_rejected_not_silently_accepted` | Transposed words, non-words and an empty string are refused rather than producing a valid-but-wrong identity. |
| `short_phrases_are_rejected` | A valid *12-word* BIP-39 phrase is refused. 12 words carries 128 bits; ITSaNAS requires 256. Without this check a user could unknowingly halve their key strength. |
| `identity_is_a_pure_function_of_the_master_secret` | The same master secret always yields the same user id and public keys, on any machine. If this failed, recovery on a new device would produce a stranger. |
| `different_masters_give_different_identities` | Distinct masters give distinct identities — no accidental collapse to a shared identity. |
| `signatures_verify_and_reject_tampering` | Valid signatures verify; altered messages and flipped signature bits do not. |
| `a_signature_cannot_be_replayed_under_another_domain` | A signature over an oplog head is **not** accepted as a device certificate. Without domain separation, any signature could be replayed in any other context. |
| `domain_prefix_is_unambiguous` | `("ab", "c")` and `("a", "bc")` hash differently. Catches the classic concatenation ambiguity that makes domain separation decorative. |
| `one_users_signature_does_not_verify_under_another` | Alice's signature fails under Bob's key. Basic, and the thing that stops a host forging log entries. |
| `chunk_ids_deduplicate_within_a_user_but_not_across_users` | One user gets a stable address for identical content (deduplication works) while two users get unrelated addresses for the *same* content (a host cannot tell they hold the same file). |
| `chunk_id_does_not_expose_the_plaintext_hash` | The chunk id is not the raw content hash. Otherwise a host holding a guess of the plaintext could confirm it by hashing — a confirmation-of-file attack. |
| `diffie_hellman_agrees_in_both_directions` | X25519 agreement is symmetric, the prerequisite for wrapping keys to another user. |
| `device_keys_are_independent_of_the_user_master` | Device keys are independently random and restorable from their seed, so revoking a stolen laptop never requires rotating the user's identity. |
| `secrets_are_redacted_in_debug_output` | `MasterSecret` and `UserKeys` never print key material even when logged wholesale. |

## `seal` — authenticated encryption (13)

The security core. Most of these assert that an **attack fails**.

| Test | What it proves |
| --- | --- |
| `deterministic_seal_round_trips` | Content-addressed sealing recovers the plaintext exactly, with the documented overhead. |
| `deterministic_seal_is_byte_stable` | Sealing identical content twice gives identical bytes. Deduplication depends on it, and so does remote audit — an owner re-derives what a host should hold rather than storing a second copy. |
| `random_seal_round_trips_and_is_never_byte_stable` | Randomised sealing round trips and never repeats a nonce. Nonce reuse under XChaCha20-Poly1305 is catastrophic. |
| **`another_user_cannot_open_your_chunk`** | Bob, holding Alice's sealed chunk, cannot decrypt it. **This is the load-bearing test of the entire project.** If it fails, ITSaNAS has no reason to exist. |
| `a_host_cannot_substitute_one_chunk_for_another` | A chunk sealed for address A does not open at address B, so a host cannot serve stale or swapped content undetected. |
| `purpose_confusion_is_rejected` | A chunk is not accepted where an oplog segment is expected. Type confusion across object kinds is a real source of protocol attacks. |
| `owner_confusion_is_rejected` | Ciphertext is bound to its owner, so objects cannot be attributed to the wrong user. |
| `every_single_bit_flip_is_detected` | **Exhaustive**: every bit of every byte of a sealed object is flipped in turn, and every one is rejected. Not sampled — all of them. A malicious host cannot corrupt a single bit silently. |
| `truncation_is_detected` | Cutting the ciphertext at any length fails to open, so a host cannot serve a partial chunk. |
| `an_unknown_format_version_is_refused_not_guessed_at` | An unknown version byte produces an explicit version error rather than a misparse. Forward compatibility that fails loudly. |
| `empty_and_undersized_inputs_do_not_panic` | Empty and short inputs at every length below the minimum return errors instead of panicking. A panic here is a remote denial of service, since these bytes come from an untrusted peer. |
| `empty_plaintext_is_a_valid_object` | A zero-byte file is a legitimate object, not an edge case that errors. |
| `associated_data_encoding_is_unambiguous` | Length-prefixing makes `("ab","c")` and `("a","bc")` distinct contexts, so binding cannot be bypassed by shifting a field boundary. |

## `keystore` — passphrase-protected storage (12)

| Test | What it proves |
| --- | --- |
| `round_trips_a_master_secret` | The primary use: a master secret sealed under a passphrase comes back intact. |
| `survives_serialisation` | The on-disk encoding round trips and still opens. Catches a header layout bug that would brick every existing keystore. |
| `a_wrong_passphrase_fails` | Wrong, empty, and trailing-whitespace passphrases all fail. |
| **`downgrading_the_kdf_cost_is_detected`** | An attacker rewriting `memory_kib` from 64 MiB down to 8 KiB in a stolen container cannot then open it. Without binding the KDF parameters into the associated data, a stolen escrow blob could be made trivially brute-forceable. |
| `an_escrow_blob_cannot_be_passed_off_as_a_local_keystore` | Container labels are bound, so a stolen escrow blob cannot be dropped in as a device keystore. |
| `one_users_escrow_blob_cannot_be_served_for_another` | A malicious coordinator cannot hand Bob's client Alice's blob and have it open, even if they share a passphrase. |
| `tampering_with_the_salt_is_detected` | Salt modification is caught. |
| `tampering_with_the_ciphertext_is_detected` | Ciphertext modification is caught. |
| `each_lock_uses_a_fresh_salt` | Two locks of the same payload under the same passphrase differ. Salt reuse would let an attacker attack many containers with one dictionary pass. |
| `malformed_input_is_rejected_without_panicking` | Every truncation length, and unknown version and KDF bytes, produce errors not panics. This parser reads data supplied by the coordinator. |
| `recommended_parameters_actually_work` *(`#[ignore]`)* | The real 64 MiB / 3-pass Argon2id parameters function end to end. Fast tests use deliberately weak parameters; this is the one that exercises what ships. Runs in CI's **slow-tests** job. |

## `wellknown` — published-identity ban list (3)

| Test | What it proves |
| --- | --- |
| **`the_ban_list_matches_the_actual_fixture_identities`** | The three banned user ids are exactly the fixture identities, derived independently in this test. If the test kit and the ban list drift apart, the ban list would silently protect nothing while appearing to work. |
| `ordinary_identities_are_not_banned` | Normal users are unaffected — the ban list is not accidentally matching everything. |
| `the_ban_list_has_no_duplicate_or_empty_entries` | No all-zero or duplicated entries, so the list covers as many identities as it appears to. |

---

# `itsanas-crypto` — property tests (15)

Unit tests pin down specific known-dangerous cases; property tests check the
same guarantees hold across randomly generated inputs, which is where encoding
and length bugs hide. 256 cases each, in
[`tests/properties.rs`](../crates/itsanas-crypto/tests/properties.rs).

| Test | What it proves |
| --- | --- |
| `sealing_round_trips_for_any_plaintext` | Both sealing modes round trip for arbitrary content up to 4 KiB and arbitrary address lengths. |
| **`a_host_can_never_open_what_it_stores`** | For arbitrary key pairs and arbitrary content, a host cannot decrypt what it holds — with either of its own root keys. The generalised form of the project's central claim. |
| `any_single_byte_corruption_is_detected` | Corruption at a random position with a random non-zero delta is always caught. |
| `truncation_at_any_point_is_detected` | Truncation at any random offset is always caught. |
| `chunk_ids_are_stable_and_content_separating` | Addresses are stable per content and never collide across distinct content for one user. |
| `chunk_ids_never_align_across_users` | Two arbitrary users never derive the same address for the same content. |
| `signatures_bind_signer_domain_and_message` | Across arbitrary domains and messages, a signature verifies only for its signer and only under its own domain. |
| `recovery_phrases_round_trip` | Every possible 32-byte master secret produces a 24-word phrase that reconstructs it. |
| `a_mistyped_recovery_phrase_never_reconstructs_the_original_identity` | A transposed phrase never decodes back to the original master secret. **Documents a real limitation found by this test**: BIP-39 gives a 24-word phrase only an 8-bit checksum, so roughly 1 transposition in 256 decodes cleanly to a *different* valid identity. That cannot be fixed in this crate; the mitigation is at the CLI layer — see [DESIGN.md](DESIGN.md#recovery-must-be-verified-not-assumed). |
| `keystores_round_trip_and_reject_wrong_passphrases` | Arbitrary passphrases including empty and Unicode round trip, and any different passphrase fails. |
| `keystores_are_bound_to_their_label` | Arbitrary distinct labels never open each other's containers. |
| `keystore_encoding_round_trips` | The on-disk encoding is exact for arbitrary payloads. |
| `arbitrary_bytes_never_panic_the_keystore_parser` | Fuzz-style: random bytes never panic the parser. This input arrives from the coordinator. |
| `arbitrary_text_never_panics_the_identifier_parser` | Random text never panics identifier parsing. This input arrives from peers and config files. |
| `identifier_hex_round_trips` | Hex encoding round trips for every possible 32-byte value. |

---

# `itsanas-testkit` — fixture integrity (7)

These protect the test data itself. See [TEST-USERS.md](TEST-USERS.md).

| Test | What it proves |
| --- | --- |
| **`corpus_matches_its_published_digests`** | Every fixture file hashes to its pinned digest, and the corpus digest matches. The tamper check: since the corpus is generated from seeds in source, altering the test data requires editing reviewed code *and* moves a digest published in the documentation. |
| **`every_fixture_identity_is_banned_in_production`** | All three published users are refused by production. Their keys are printed in the README; this is what stops that being an attack. |
| `recovery_phrases_rebuild_the_documented_identities` | Each phrase in the docs reconstructs exactly the user id claimed beside it. Ties documentation to code — a stale doc fails CI. |
| `canaries_are_unique_and_actually_present_in_plaintext` | Each canary is unique, genuinely present in its owner's plaintext, and absent from everyone else's. Without this, the on-disk plaintext-leak tests would pass **vacuously** — searching for a string that was never there. |
| `the_shared_document_gets_a_different_address_for_every_user` | On real corpus data: three users holding byte-identical content derive three unrelated chunk ids. |
| `filler_is_deterministic_and_seed_separated` | Generated content is reproducible and seed-separated, without which every pinned digest is meaningless. |
| `the_corpus_covers_edge_case_sizes` | The corpus contains an empty file, a file over 512 KiB, and a file whose size is not a multiple of 1 KiB. Guards the *test data* against becoming too tidy to catch boundary bugs. |

---

# Planned tests

Listed here so the gap between what is claimed and what is verified stays
visible. These land with the milestones in [ROADMAP.md](ROADMAP.md).

## M2 — store

- Alice's full corpus written and read back byte-identical, digest for digest.
- **Plaintext-leak scan**: after Alice replicates to Bob, scan every byte of
  Bob's storage directory for `ITSANAS-CANARY-ALICE-4f21c8d0`. Any hit fails.
  Repeated for all six ordered user pairs.
- **Storage accounting**: pledged bytes, bytes actually on disk, and bytes
  accounted for in the index agree within the documented overhead — so a node
  cannot silently under-provide or over-report.
- **Data-presence audit**: after replication, assert Bob's store contains the
  expected chunk count for Alice, that each is byte-identical to what Alice
  would re-derive, and that Bob cannot decrypt any of them.
- Chunk-boundary stability: inserting a byte at the start of a 1 MiB file
  re-chunks only the neighbourhood, not the whole file.
- Empty file, 1-byte file, and file exactly on a chunk boundary.
- Garbage collection never removes a referenced chunk; a crash mid-write leaves
  no torn object.

## M3 — sync

- Deterministic three-device simulation with injectable partitions and power
  cycles; convergence asserted in every scenario.
- **The offline-device scenario**: Bob's Pi writes, pushes, powers off
  permanently; Alice and Carol must still converge on Bob's change.
- Concurrent edits on two devices produce both files, neither lost.
- Delete racing an edit never destroys the edit.
- Rename detection does not re-upload chunk data.
- Clock skew and a device whose clock runs backwards do not break ordering.

## M4 — network

- Wire-decoder fuzzing (`cargo-fuzz`), no panics.
- A malicious peer returning garbage, wrong-but-valid, or truncated chunks is
  detected and the chunk refetched elsewhere.
- A peer that fails a storage challenge is marked unreliable.

## M5 — placement

- Removing one node from a 20-node simulated swarm moves only that node's share
  of chunks (rendezvous hashing's minimal-disruption property, measured).
- Weighted distribution matches pledged capacity within tolerance.
- A chunk dropping below the replication floor is repaired without intervention.
- Owner affinity: a user's own devices always appear in their replica set.

## M6 — coordinator

- New-device login via username and passphrase recovers the full account.
- **Coordinator-compromise test**: dump the coordinator's entire stored state
  and assert it contains no plaintext, no canary, and no usable key material.
- A malicious coordinator serving a wrong node set cannot cause data loss.

## M7 — daemon

- The low-node-count alert actually fires when peers drop below the replication
  floor, and clears when they return.
- The sync-stalled alert fires when no round completes within the threshold.
- End-to-end: a file dropped in the folder appears on a second device with no
  user action.
