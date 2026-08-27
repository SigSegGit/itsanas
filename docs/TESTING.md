# Test Catalogue

**Last updated: 2026-08-27 — 462 tests across 17 binaries, plus 2 doctests.**

| Binary | Tests |
| --- | --- |
| `itsanas-crypto` unit | 64 (1 `#[ignore]`d) |
| `itsanas-crypto` property (`tests/properties.rs`) | 15 |
| `itsanas-wire` unit | 17 |
| `itsanas-tls` unit | 6 |
| `itsanas-tls` handshake (`tests/handshake.rs`) | 5 |
| `itsanas-store` unit | 97 |
| `itsanas-store` integration (`tests/store.rs`) | 29 (1 `#[ignore]`d) |
| `itsanas-sync` unit | 12 |
| `itsanas-sync` convergence (`tests/convergence.rs`) | 19 |
| `itsanas-net` unit | 25 |
| `itsanas-net` two-node (`tests/two_nodes.rs`) | 12 |
| `itsanas-placement` unit | 29 |
| `itsanas-coord` unit | 47 |
| `itsanas-folder` unit | 31 |
| `itsanas-folder` integration (`tests/folder.rs`) | 22 |
| `itsanas-cli` unit | 25 |
| `itsanas-testkit` unit | 7 |

These counts are mechanical — regenerate them with
`cargo test --workspace -- --list`. If this table disagrees with that command,
the table is the bug.

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
| **slow-tests** | `cargo test -- --ignored --test-threads 1` | Two tests are marked `#[ignore]`: the real 64 MiB Argon2id cost, and a 64 MiB streaming round trip that takes ~45s in a debug build. Too slow for every push, far too important to never run. |
| **cross-build** | `cargo build --release --target aarch64-unknown-linux-gnu` | The Raspberry Pi 4B+ is a first-class deployment target. Catching a dependency that does not cross-compile at PR time is much cheaper than at deploy time. |
| **minimum-rust-version** | `cargo check` on the pinned MSRV | Prevents accidentally requiring a newer toolchain than the documented minimum, which would break users on distro Rust. |
| **supply-chain** | `cargo deny check` | Fails on any unpatched advisory, any yanked crate, and any licence not compatible with AGPL-3.0. For a system whose entire value is "your host cannot read your data", a vulnerable crypto dependency is a release blocker. |
| **documentation** | `cargo doc` with `RUSTDOCFLAGS=-D warnings` | Broken intra-doc links are the quietest kind of rot: the documentation keeps claiming a relationship the code no longer has, and nothing fails until a reader clicks it. |
| **coverage** | `cargo llvm-cov` | Not a target to game — used to spot whole modules or error paths with no test at all. |

The workflow also runs **weekly on a schedule**, so a newly published advisory
against a dependency surfaces even when nobody has pushed for a month.

---

# `itsanas-crypto` — unit tests (64)

## `secret` — secret hygiene (4)

| Test | What it proves |
| --- | --- |
| `debug_never_reveals_bytes` | Formatting a secret prints `SecretBytes<32>(redacted)` and no key bytes. Daemon logs on an ITSaNAS node are readable by the machine's owner, who is explicitly not the person whose keys those are. |
| `equality_is_value_based` | Secret comparison is by value via constant-time `ct_eq`, so tests comparing keys are meaningful and comparison does not leak timing. |
| `random_secrets_differ` | Two freshly drawn secrets differ and neither is all-zero. Catches a miswired CSPRNG returning a constant — the failure mode that silently makes every key identical. |
| `from_slice_rejects_wrong_length` | 31- and 33-byte inputs are refused. Prevents a truncated key being silently zero-padded into a weak key. |

## `kdf` — key derivation (7)

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

## `identity` — identity and signatures (17)

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

## `seal` — authenticated encryption (16)

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

## `keystore` — passphrase-protected storage (13)

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
| **`every_fixture_identity_is_banned_in_production`** | All three published users are refused by production. Their keys are printed in [TEST-USERS.md](TEST-USERS.md); this is what stops that being an attack. Enforcement is tested at the store layer by `the_published_test_identities_are_refused_by_the_normal_constructor`. |
| `recovery_phrases_rebuild_the_documented_identities` | Each phrase in the docs reconstructs exactly the user id claimed beside it. Ties documentation to code — a stale doc fails CI. |
| `canaries_are_unique_and_actually_present_in_plaintext` | Each canary is unique, genuinely present in its owner's plaintext, and absent from everyone else's. Without this, the on-disk plaintext-leak tests would pass **vacuously** — searching for a string that was never there. |
| `the_shared_document_gets_a_different_address_for_every_user` | On real corpus data: three users holding byte-identical content derive three unrelated chunk ids. |
| `filler_is_deterministic_and_seed_separated` | Generated content is reproducible and seed-separated, without which every pinned digest is meaningless. |
| `the_corpus_covers_edge_case_sizes` | The corpus contains an empty file, a file over 512 KiB, and a file whose size is not a multiple of 1 KiB. Guards the *test data* against becoming too tidy to catch boundary bugs. |

---

# `itsanas-store` — unit tests (97)

## `chunker` — content-defined chunking (13)

| Test | What it proves |
| --- | --- |
| **`the_gear_table_is_pinned_forever`** | The 256-entry gear table hashes to a fixed digest. If it ever changes, every chunk boundary in the network moves: existing stores stop deduplicating against new writes and every client re-uploads every file. This is the test that makes deriving the table safer than pasting one in. |
| **`inserting_a_byte_at_the_front_shifts_only_local_boundaries`** | Over 90% of chunks survive a one-byte prefix insertion into 4 MiB. This is the entire reason content-defined chunking exists; with fixed-size chunking it finds zero. |
| `editing_the_middle_leaves_both_ends_intact` | The same property for a mid-file insertion. |
| **`the_average_chunk_size_is_close_to_the_target`** | Empirical mean chunk size is within 2× of the configured average. A mistranscribed cut mask still produces valid, reassembling chunks — just at the wrong size, quietly wrecking the dedup/overhead trade-off. Nothing else would catch that. |
| `chunks_reassemble_into_the_original_bytes` | Chunking loses and reorders nothing, across six sizes from 0 to 1 MiB. |
| `chunking_is_deterministic` | Two runs agree on boundaries, so two devices will too. |
| `size_bounds_are_respected` | Every non-final chunk sits within min and max. |
| **`highly_repetitive_data_still_terminates_and_respects_the_maximum`** | Long runs of one byte are the pathological case for a rolling hash — the hash can settle into a state where the mask never matches. Proves the max-size ceiling stops that producing one enormous chunk. |
| `a_buffer_shorter_than_the_minimum_is_one_chunk` | The minimum is enforced. |
| `offsets_are_contiguous_and_start_at_zero` | Chunk offsets tile the input exactly. |
| `an_empty_buffer_produces_no_chunks` | Zero-length input is not a special case that panics. |
| `invalid_configurations_are_rejected` | Out-of-order or zero bounds fail at construction. |
| **`small_configurations_do_not_panic_on_mask_lookup`** | Extreme averages do not index off the end of the mask table. This test found a real bug: the loose mask index overran for any average above 2^27. |

## `blob` — content-addressed storage (11)

| Test | What it proves |
| --- | --- |
| `round_trips_what_it_was_given` | Sealed bytes come back byte-identical. |
| **`storing_the_same_address_twice_writes_once`** | Deduplication actually saves a write rather than silently rewriting. |
| **`a_blob_lands_at_a_sharded_path_not_a_flat_one`** | Two-level fan-out is real. A flat directory degrades badly at a million chunks on both ext4 and NTFS. |
| **`files_that_are_not_blobs_are_ignored_by_the_scan`** | Garbage collection deletes what the scan reports, so a scan that reported a foreign file would delete a stranger's data. |
| **`no_staging_file_survives_a_successful_write`** | The write-then-rename path cleans up, so writes do not leak a temp file each time. |
| **`sweeping_removes_crash_leftovers_but_not_blobs`** | Crash recovery removes abandoned staging files and touches nothing else. |
| `removal_is_idempotent` | Deleting twice is not an error. |
| `addresses_lists_everything_across_the_fan_out` | The scan finds blobs in every shard directory. |
| `a_missing_address_is_none_not_an_error` | Absence is a value, not a failure. |
| `an_empty_blob_is_storable_and_distinguishable_from_a_missing_one` | Zero-length content is not confused with absence. |
| `total_bytes_counts_stored_bytes` | Size accounting is correct. |

## `index` — transactional metadata (11)

| Test | What it proves |
| --- | --- |
| **`two_files_sharing_a_chunk_both_hold_it`** | Deleting one of two files that share a chunk does not take the other's data with it. |
| **`a_file_that_repeats_a_chunk_counts_each_occurrence`** | A file of ten identical blocks references one chunk ten times. Getting this wrong frees live data on the first delete. |
| **`a_chunk_can_be_resurrected_before_it_is_collected`** | Restoring identical content before GC runs takes the chunk out of the collection queue, so GC does not delete a blob that is live again. |
| `overwriting_a_file_releases_only_the_chunks_it_stopped_using` | An overwrite computes the right delta rather than releasing everything. |
| `adding_a_file_references_its_chunks` | Reference counts go up on write. |
| `a_fresh_index_reads_as_empty_rather_than_erroring` | A brand-new store reads empty instead of failing on a table that was never written. |
| `state_survives_reopening` | Data is durable across a process restart. |
| `files_come_back_sorted_so_two_devices_agree_on_order` | Iteration order is deterministic, which matters once two devices compare listings. |
| `a_file_round_trips` | Entries store and load unchanged. |
| `removing_an_absent_file_is_a_no_op` | Deleting nothing is not an error. |
| `forgetting_a_chunk_clears_both_tables` | Post-GC cleanup leaves no half-state. |

## `oplog` — the operation log (15)

| Test | What it proves |
| --- | --- |
| **`a_host_can_verify_a_segment_without_being_able_to_read_it`** | The entire bargain: hosts police authenticity, owners read. A stranger's key cannot open the body. |
| **`the_sealed_body_does_not_leak_the_path_in_plaintext`** | A filename does not appear in the encoded segment. Without this, hosts learn what their peers store. |
| **`a_host_dropping_a_segment_from_the_middle_is_detected`** | The concrete attack: a host holds segments 1–3 and serves only 1 and 3, hiding whatever change 2 carried. The chain link catches it. |
| **`a_sequence_gap_is_detected_even_when_the_chain_links_up`** | A compromised device cannot skip sequence numbers while chaining correctly and leave a peer believing its history is complete. |
| **`an_envelope_that_lies_about_its_body_is_caught_on_open`** | A validly re-signed envelope claiming the wrong sequence range is still rejected, because the body is cross-checked against the envelope's claims. |
| **`tampering_with_any_envelope_field_invalidates_the_signature`** | Five separate fields, each mutated independently. |
| **`a_segment_signed_by_another_device_is_rejected`** | One device cannot forge a segment attributed to another. |
| **`malformed_bytes_do_not_panic_the_decoder`** | Every truncation and 200 single-byte corruptions of a valid segment return errors rather than panicking. A host controls these bytes entirely. |
| `two_segments_with_identical_entries_are_still_distinct_objects` | Randomised sealing and random object ids stop two identical batches colliding in the blob store. |
| `a_valid_chain_validates` | The honest case passes. |
| `a_chain_whose_first_segment_claims_a_predecessor_is_still_walked` | Starting mid-chain is legitimate for a peer catching up. |
| `an_empty_chain_is_vacuously_valid` | Nothing to check is not an error. |
| `an_empty_segment_is_refused` | No wasting a sequence number and an object id on nothing. |
| `a_segment_round_trips_for_its_owner` | The happy path works. |
| `encoding_round_trips_through_the_wire_format` | Serialisation preserves everything, including verifiability. |

## `path` — logical path validation (9)

Paths arrive from a peer's operation log, so they are attacker-controlled the
moment the sync engine starts materialising files.

| Test | What it proves |
| --- | --- |
| **`traversal_is_rejected_in_every_position`** | `..` is refused leading, trailing and interior. Without this a peer writes `../../../.ssh/authorized_keys`. |
| **`absolute_paths_are_rejected`** | Unix absolute paths and Windows drive-letter prefixes both refused. |
| **`backslashes_are_rejected_rather_than_translated`** | Translating would make `a\b` and `a/b` name one file on Windows and two on Linux, so the devices would diverge. |
| **`windows_device_names_are_rejected`** | The Pi will happily create `com1.txt`; the laptop must never try to open a serial port. Includes negative cases (`console.txt`, `com10`) so the rule is not over-broad. |
| **`trailing_spaces_and_dots_are_rejected`** | Windows silently strips these, so `evil.txt ` and `evil.txt` would collide on one device and not another. |
| `control_characters_are_rejected` | NUL, newline, carriage return and tab refused. |
| `malformed_separators_are_rejected` | Empty, doubled and trailing separators refused. |
| `oversized_paths_and_components_are_rejected` | Bounds enforced, so one log entry cannot make the index enormous. |
| `ordinary_paths_are_accepted` | The rules are not so strict that normal filenames — including Unicode and dotfiles — break. |

---

# `itsanas-store` — integration tests (29)

Full path from plaintext to disk and back. `tests/store.rs`.

| Test | What it proves |
| --- | --- |
| **`alices_entire_corpus_round_trips_byte_identical`** | M2 exit criterion. Every fixture file survives chunking, sealing, storage and reassembly unchanged. |
| **`no_users_plaintext_ever_touches_the_disk`** | M2 exit criterion, and the single most important property in the project. Both canaries are scanned against both stores — a user's own store must not leak their plaintext either, because that laptop can be stolen. Includes a vacuity check proving the canary really is in the plaintext, so the scan cannot pass by scanning nothing. |
| **`an_insertion_at_the_start_of_a_large_file_reuses_almost_every_chunk`** | M2 exit criterion, end to end through the real store. |
| **`two_users_storing_the_same_document_produce_unrelated_chunk_ids`** | Two users storing byte-identical content get disjoint addresses. If addresses were plain content hashes a host could correlate users and confirm guessed files. |
| **`one_users_store_cannot_be_opened_with_another_users_keys`** | Sealing is bound to the owner, not merely to the directory. |
| **`the_published_test_identities_are_refused_by_the_normal_constructor`** | The claim README.md and SECURITY.md both make. Before this test the ban-list function was defined, exported, and called by nothing. |
| **`a_chunk_served_under_the_wrong_address_does_not_decrypt`** | The substitution attack, with two genuine chunks from the same user. |
| **`a_corrupted_blob_is_detected_and_never_returned_as_content`** | A flipped bit in stored ciphertext surfaces as an error, not as data. |
| **`a_deleted_blob_is_reported_rather_than_silently_returning_short_data`** | A missing chunk fails the read instead of returning a truncated file. |
| **`garbage_collection_honours_the_grace_period`** | Nothing is deleted inside the grace window — a peer may still be fetching it — and everything is once the window passes. |
| **`deleting_one_of_two_identical_files_keeps_the_other_readable`** | GC with shared chunks does not destroy live data. |
| **`unsealed_writes_survive_a_restart_and_are_announced_afterwards`** | Simulates a power cut between a write and the next flush. The entry is not lost, so the peer still learns the file exists. |
| **`every_write_is_announced_in_the_log_exactly_once`** | Sequence numbers are dense from 1, and a second flush re-emits nothing — so a peer never replays an entry twice. |
| **`the_segment_chain_links_up_across_many_flushes`** | Five flushes produce a chain with no gaps and non-overlapping sequence ranges. |
| `identical_files_stored_twice_occupy_one_copy_on_disk` | Deduplication measured in bytes on disk, not merely in chunk ids. |
| `overwriting_a_file_eventually_reclaims_the_bytes_it_stopped_using` | 1 MiB overwritten by 1 KiB drops below a tenth of the original size after GC. |
| `a_store_reopens_with_everything_intact` | Everything survives a restart, and the reopened store reports healthy. |
| `a_healthy_store_reports_healthy` | The integrity check does not cry wolf on a good store, and writing files leaves no orphan blobs. |
| `the_store_rejects_paths_that_would_escape_the_sync_root` | Path validation is wired into the store, not merely available. |
| `a_file_larger_than_one_chunk_uses_several_and_still_verifies` | Multi-chunk files reassemble and hash correctly. |
| `an_empty_file_is_stored_and_distinguishable_from_a_missing_one` | Zero chunks is a valid file, distinct from absence. |
| `a_non_default_chunker_still_round_trips` | The tuning knob does not produce unreadable data. |

---

# `itsanas-sync` — unit tests (12)

## `conflict` — deciding who keeps the original path (10)

| Test | What it proves |
| --- | --- |
| **`the_winner_is_the_same_whichever_side_asks`** | The rule is antisymmetric. If it were not, both devices would each believe they won, both would write to the original path, and they would overwrite each other forever. |
| **`the_higher_device_id_wins_regardless_of_write_count`** | A device that has written a thousand times does not thereby beat one that wrote twice — the outcome must not depend on unrelated activity elsewhere. |
| **`the_order_is_total_so_no_pair_is_ever_undecided`** | Strict and antisymmetric across every pair, including a version against itself. |
| **`the_marker_goes_before_the_extension`** | `report.pdf` → `report.conflict-….pdf`, not `report.pdf.conflict`, which Windows would associate with nothing. |
| **`a_dotfile_keeps_its_leading_dot`** | `.bashrc` does not become `.conflict-….bashrc`, which would be a different, no-longer-hidden file. |
| **`only_the_final_component_is_examined_for_an_extension`** | A directory called `my.files` does not swallow the marker. |
| **`two_different_devices_produce_two_different_siblings`** | Three-way conflicts are rare but real; two losers colliding on one sibling path would destroy one of them. |
| **`the_sibling_path_is_a_valid_logical_path`** | The generated name survives the store's own path validation — it is about to be written to a real store. |
| `a_file_with_no_extension_gets_the_marker_appended` | Extension-less names are handled. |
| `a_multi_dot_name_splits_on_the_last_dot` | `archive.tar.gz` splits sensibly. |

## `engine` — sync reporting (2)

| Test | What it proves |
| --- | --- |
| `a_report_counts_every_outcome_kind` | Each of the six outcomes is tallied, and a deferred operation asks for another round. |
| **`a_quiet_round_reports_no_work_and_no_retry`** | A round that only recognised things it already knew reports no progress. If it reported progress, the settle loop would never terminate. |

---

# `itsanas-sync` — convergence tests (19)

The M3 exit criteria. Real stores, real chunking, real sealing, real signatures;
only the network is simulated. Nothing uses randomness or wall-clock time, so a
failure reproduces exactly. `tests/convergence.rs`.

| Test | What it proves |
| --- | --- |
| **`a_device_that_never_comes_back_still_gets_its_work_to_everyone_else`** | The scenario the whole architecture exists for. The Pi writes at 3am, publishes, and is switched off permanently; the laptop and VM still converge on its work, having never spoken to it. |
| **`work_propagates_through_a_third_device_that_only_relays`** | The Pi and the VM are never online simultaneously. The work still reaches the VM via the laptop. |
| **`concurrent_edits_produce_both_files_and_lose_neither`** | Two edits during a partition yield two files on every device, with both bodies intact. |
| **`a_three_way_conflict_produces_three_distinct_files`** | All three versions survive a full partition; none is silently dropped. |
| **`a_sequential_edit_is_not_treated_as_a_conflict`** | The common case stays clean. If ordinary edits produced siblings the folder would fill with junk and the feature would be worse than useless. |
| **`a_delete_racing_an_edit_never_destroys_the_edit`** | The asymmetry rule. A concurrent delete loses, because a lost edit is unrecoverable and an unexpected resurrection takes a second to undo. |
| **`a_delete_that_saw_the_edit_is_honoured`** | The counterpart: a normal delete actually deletes, on every device. Without this the product does not work. |
| **`an_offline_device_does_not_resurrect_a_file_deleted_while_it_slept`** | Tombstones do their job. Without them the returning device re-announces what it still holds and the file comes back from the dead everywhere. |
| **`re_creating_a_deleted_file_works_and_converges`** | A path can go live → deleted → live again without ending up both present and deleted. |
| **`the_final_state_does_not_depend_on_the_order_devices_sync_in`** | The same divergence healed in two opposite orders reaches an identical state. Order dependence here would be a permanent, silent disagreement in production. |
| **`syncing_repeatedly_changes_nothing`** | Hosts re-serve segments freely and there is no acknowledgement telling them to stop, so applying an operation twice must be a no-op. |
| **`re_resolving_a_conflict_is_idempotent`** | Guards the specific bug this suite caught: a conflict re-resolved every round means a settle loop that stops when nothing changes never stops. |
| **`a_long_run_of_alternating_partitions_still_converges`** | Ten rounds of rotating partitions, twenty files, full agreement at the end. More history than a hand-built scenario covers. |
| **`an_operation_whose_chunks_are_unavailable_is_deferred_not_half_applied`** | A segment can arrive before its chunks. Materialising anyway would create a file that exists but cannot be read. |
| **`a_deferred_operation_completes_once_its_chunks_show_up`** | And the retry actually completes. |
| **`the_hosts_hold_everything_and_can_read_none_of_it`** | Every byte the simulated hosts hold is scanned for Alice's canary *and* for each of her filenames. Includes a vacuity check proving the canary is really in the data. |
| **`every_segment_a_host_holds_is_verifiable_by_that_host`** | Hosts cannot read segments but must be able to authenticate them, or anyone could flood a host with garbage attributed to a peer. |
| **`a_full_corpus_converges_across_three_devices_with_partitions`** | The realistic end-to-end case: a real data set written across three devices that are never all online together, converging byte-identically. |
| **`version_vectors_order_sequential_writes_and_flag_concurrent_ones`** | The underlying primitive, checked at the level of real stores rather than in isolation. |

---

# `itsanas-store` — the vault (14 of the store's unit tests)

Storage for *other people's* data. The vault holds no keys and no constructor
takes one, so these tests are about accepting, serving and accounting — never
about reading.

| Test | What it proves |
| --- | --- |
| **`a_segment_with_a_bad_signature_is_refused_before_it_is_stored`** | A host that stored unverified envelopes would be a convenient way to attribute garbage to someone else's device. |
| **`a_segment_that_does_not_continue_the_chain_is_refused`** | Otherwise a host can be induced to store a chain with a hole and then serve that hole to a peer as though it were complete. |
| **`re_offering_the_current_tip_is_accepted_as_a_no_op`** | Peers re-offer freely — there is no acknowledgement telling them to stop — so this must neither error nor duplicate. |
| **`an_owner_whose_chunks_are_held_but_whose_log_is_not_still_counts`** | Guards a real bug this suite caught: the owner list was derived from the segment table alone, so a host storing chunks but no segments reported zero bytes and its quota was blind to the bulk of what it held. |
| **`two_owners_chunks_do_not_collide_even_at_the_same_address`** | Chunk ids are blinded per user so a collision should not happen, but correctness must not depend on that. |
| **`one_owners_segments_are_never_served_under_another_owners_name`** | Owner scoping is real, not incidental. |
| **`resuming_after_an_unknown_segment_returns_nothing_rather_than_everything`** | An unrecognised resume point must not cause the whole chain to be re-sent. |
| **`resuming_after_a_segment_skips_what_the_caller_already_has`** | The resume path works, so a catching-up peer does not re-download its own history. |
| `a_chain_is_stored_and_served_in_order` | What comes back validates as a chain. |
| `the_limit_caps_the_response` | One request cannot ask for unbounded work. |
| `heads_are_reported_per_device_and_scoped_to_one_owner` | Head reporting is per device and does not leak across owners. |
| `stats_account_for_every_owner` | Quota accounting sums correctly. |
| `a_chunk_round_trips_without_the_vault_ever_holding_a_key` | The basic path. |
| `everything_survives_reopening` | Durable across a restart. |

---

# `itsanas-net` — unit tests (25)

## `protocol` — messages and challenges (9)

| Test | What it proves |
| --- | --- |
| **`a_proof_for_one_nonce_does_not_answer_another`** | Otherwise a host computes one proof, throws the chunk away, and answers every future challenge from cache. |
| **`a_proof_requires_the_actual_bytes`** | A host that discarded the chunk fails. |
| **`a_single_bit_of_difference_fails_the_challenge`** | Corruption is caught, not just deletion. |
| **`an_unbounded_segment_request_is_not_acceptable`** | One request cannot ask a peer to assemble everything it holds. |
| **`a_maximum_size_chunk_fits_in_one_frame`** | The largest legitimate message fits the frame limit, so normal operation does not hit it. |
| `every_request_variant_round_trips_through_the_wire` | A variant that fails to encode is a runtime failure on a live connection. |
| `every_response_variant_round_trips_through_the_wire` | The same, for responses. |
| `a_hello_from_a_different_protocol_version_is_not_acceptable` | Version negotiation is real. |
| `a_refusal_carries_no_secret_material` | Documents that `Refused` is operator-facing only. |

## `service` — what a peer may obtain (14)

| Test | What it proves |
| --- | --- |
| **`what_a_peer_fetches_is_useless_without_the_key`** | The reason there is no access-control list. The served bytes contain no plaintext, and a stranger's keys cannot open them. |
| **`a_node_stores_and_serves_a_strangers_chunk_without_reading_it`** | The mutual-storage bargain in one test: the host serves back exactly what it took, cannot open it, and the guest can. |
| **`a_host_that_discarded_a_chunk_cannot_fake_the_proof`** | Deleting to save space is detected. |
| **`storing_beyond_the_pledge_is_refused`** | Otherwise "pledge 10 GB" is meaningless and the disk fills. |
| **`a_bad_request_never_becomes_a_local_error`** | A peer must not be able to decide when this node reports a fault. |
| **`a_forged_segment_is_refused_rather_than_stored`** | Signature checking is wired into the service, not merely available. |
| **`an_unknown_chunk_is_none_rather_than_an_error`** | "I do not have it" is ordinary. |
| `a_storage_challenge_passes_when_held_and_fails_when_not` | Both directions. |
| `a_node_that_pledged_nothing_still_serves_its_own_data` | Hosting nothing must not break syncing your own devices. |
| `heads_for_an_unknown_owner_are_empty_rather_than_an_error` | No invented chains. |
| `hello_reports_this_nodes_device_and_agrees_on_a_version` | The opening exchange. |
| `a_hello_from_a_future_protocol_version_is_refused_not_guessed_at` | No optimistic guessing. |
| `a_peer_can_fetch_this_nodes_own_segments_and_chunks` | The basic serving path. |
| `the_segment_limit_is_clamped_to_the_protocol_maximum` | Limits are applied. |

## `transport` — binding and serving (2)

| Test | What it proves |
| --- | --- |
| `binding_a_public_address_no_longer_needs_an_override` | Documents a deliberate *removal*. Binding a public address used to be refused, because the transport leaked chunk identifiers and sizes to anyone on the path. TLS closed that, so the refusal became cargo cult and was deleted. The test exists so that nobody restores the refusal believing it was ever a security control. |
| `loopback_still_binds` | The ordinary case did not regress while the above changed. |

Authentication is not tested here. It lives one layer down, in `itsanas-tls`,
and is catalogued with that crate.

---

# `itsanas-net` — two-node tests (12)

Real stores, real chunking, real sealing, real signatures, real TCP.
`tests/two_nodes.rs`.

| Test | What it proves |
| --- | --- |
| **`two_nodes_sync_a_file_over_a_real_socket`** | The M4 exit criterion. |
| **`a_host_stores_a_strangers_data_and_cannot_read_a_byte_of_it`** | Alice's whole corpus pushed to Bob's node, then every byte Bob holds scanned for Alice's canary. |
| **`a_host_relays_one_device_to_another_that_it_never_met`** | The architecture's whole reason for existing, over a socket: the Pi pushes and powers off, the VM pulls the Pi's work from a host it has never met. |
| **`syncing_twice_transfers_nothing_the_second_time`** | Without the have/missing exchange this re-uploads everything every round, which at real sizes saturates the link forever. |
| **`concurrent_edits_on_two_machines_converge_over_a_socket`** | The convergence property, through the real protocol, so a transport bug that lost or reordered work shows up. |
| **`a_peer_cannot_push_a_forged_segment_into_a_host`** | End to end, not just at the service layer. |
| **`a_storage_challenge_works_over_the_wire`** | Including that the owner re-derives the expected bytes rather than keeping a second copy — which is what makes remote audit possible at all. |
| **`a_malformed_request_gets_a_refusal_rather_than_a_dropped_connection`** | A peer cannot kill a sync round by sending something silly. |
| **`a_host_that_has_pledged_nothing_refuses_to_store_but_still_answers`** | Refusing to store does not make a node stop being a peer. |
| `a_larger_file_survives_the_wire_byte_for_byte` | Multi-chunk fetch and reassembly. |
| `a_peer_asking_about_an_unknown_user_gets_an_empty_answer` | No invented chains over the wire either. |

---

# `itsanas-cli` — unit tests (25)

## `daemon` — pacing (2)

| Test | What it proves |
| --- | --- |
| `the_default_interval_is_neither_a_busy_loop_nor_an_hour` | Too short and three machines polling each other is a constant load on a Pi; too long and the thing feels broken. |
| `shutdown_is_noticed_quickly_enough_to_feel_immediate` | A Ctrl-C that took a whole interval to be noticed would be indistinguishable from a hang. |

The daemon's real behaviour — that two nodes converge with nobody running
`sync` — is verified by running it, not by a unit test. The loop itself is
twenty lines around `session::round`, which the two-node suite covers
thoroughly; a test with a fake clock around it would assert that the loop calls
the function, which is not a property worth having a test for.

## `node` — identity on disk (9)

| Test | What it proves |
| --- | --- |
| **`the_phrase_is_not_written_anywhere_under_the_node_directory`** | Scans every file under the node's home for the phrase. A recovery phrase stored on the machine it protects is not a backup, it is an extra copy for an attacker to find. |
| **`the_phrase_does_not_leak_through_debug`** | The single most likely way for a phrase to escape is a stray `dbg!` or a derived `Debug`. |
| **`a_published_test_phrase_is_refused_as_a_real_account`** | Restoring Alice's published phrase as a real account is refused, with an explanation. |
| **`the_device_identity_also_survives_a_restart`** | If the device key changed on every start, every restart would look like a new device to the version vectors and history would fragment. |
| **`creating_over_an_existing_node_is_refused`** | Overwriting would destroy the master secret and make every chunk stored under it permanently unreadable. |
| **`a_phrase_round_trips_through_restore`** | Same account, *different* device id — two machines sharing a device identity would share a sequence counter and fork the log. |
| **`opening_a_missing_node_says_what_to_do_about_it`** | The error names both `init` and `login`. |
| `a_created_node_reopens_with_the_same_identity` | Reopening does not orphan the data. |
| `the_wrong_passphrase_does_not_open_the_node` | Indistinguishable from a tampered keystore, on purpose. |

## `config` — settings (12)

| Test | What it proves |
| --- | --- |
| **`an_unknown_setting_is_an_error_rather_than_being_ignored`** | A silently discarded typo is how a node ends up pledging nothing while its operator believes it pledged a terabyte. |
| **`defaults_are_safe`** | Pledge defaults to zero and listen defaults to loopback. A node that has not said what it offers has not offered any. |
| **`a_nonsense_size_is_refused_rather_than_read_as_zero`** | Reading "ten gigabytes" as 0 would silently disable hosting. |
| `a_malformed_line_names_its_line_number` | Errors are actionable. |
| `an_overflowing_size_is_refused` | `999999999999T` does not wrap. |
| `sizes_parse_the_way_people_write_them` | `500`, `1K`, `2MB`, `10G`, `1TiB`. |
| `sizes_format_readably` / `formatting_never_panics_at_the_extremes` | Output is legible at every magnitude. |
| `a_config_round_trips` / `comments_and_blank_lines_are_ignored` / `several_peers_accumulate` / `a_missing_file_reads_as_defaults` | The format works. |

---

# `itsanas-placement` — unit tests (29)

## `nodeset` — where a chunk belongs (16)

| Test | What it proves |
| --- | --- |
| **`removing_a_node_moves_only_that_nodes_share`** | The M5 exit criterion, and stronger than the usual phrasing: **zero** chunks move between two *surviving* nodes. With modulo hashing almost everything moves, which at real scale means re-uploading the whole network. |
| **`adding_a_node_only_pulls_in_its_own_share`** | The same property in the other direction. |
| **`distribution_matches_pledged_capacity`** | A node pledging 4× holds roughly 4× as many chunks, measured over 20 000 chunks across a 1:8 spread. Without this the "mutual" in mutual storage is a fiction and the small nodes carry the network. |
| **`no_floating_point_is_involved`** | Greps the module's own source for `f64`, `.ln(`, `.powf`. `f64::ln` is libm-dependent and two platforms can differ in the last ulp, which would make two machines disagree about where a chunk lives — silently, with no error. |
| **`placement_is_deterministic`** / **`the_answer_does_not_depend_on_the_order_the_set_was_built_in`** | Two peers given the same membership by different routes reach the same answer. |
| **`a_users_own_devices_always_hold_their_own_data`** | A user whose peers have all left must still be able to read their own files. |
| **`owner_affinity_does_not_starve_a_user_with_many_devices`** | Documents the deliberate current behaviour when a user has more devices than the replication factor — right for availability, wrong for durability, and the fix belongs with the repair loop. |
| **`one_enormous_node_cannot_take_over_the_swarm`** | The slot cap bounds how much of the network's data can be concentrated on the single machine most worth attacking. |
| **`different_owners_get_different_placements_for_the_same_chunk_id`** | Placement must not reintroduce the cross-user correlation that blinded chunk ids remove. |
| **`a_replica_set_never_contains_the_same_node_twice`** | Three replicas on one machine is one replica with extra steps, and would make the durability accounting a lie. |
| `identical_capacities_distribute_evenly` | No node is starved; none is favoured. |
| `a_swarm_smaller_than_the_replication_factor_returns_everyone` / `an_empty_swarm_places_nothing_rather_than_panicking` / `asking_for_zero_replicas_returns_none` | Edges. |
| `duplicate_and_zero_capacity_nodes_are_refused` | Malformed membership is rejected at construction. |

## `repair` — noticing a chunk is running out of copies (13)

| Test | What it proves |
| --- | --- |
| **`a_chunk_nobody_holds_is_planned_for_rather_than_overlooked`** | The case that loses data. A chunk absent from the census would be invisible to repair. |
| **`a_swarm_too_small_to_meet_the_floor_raises_an_alert`** | Silence would mean a user believing they have three replicas when the network can only ever give them two. |
| **`a_chunk_with_a_single_copy_left_is_flagged_as_critical`** | The difference between "a node is having an evening off" and "one more failure and this is gone". |
| **`an_offline_node_is_not_a_reason_to_move_data`** | A sleeping node will come back. Re-placing its chunks would mean the network churns every time somebody shuts a laptop — but the shortfall is still reported. |
| **`repair_never_plans_a_deletion`** | An over-replicated chunk is wasted space; a wrongly deleted one is gone. The plan type has no deletion variant, and this test makes adding one a deliberate act. |
| **`repair_never_sends_a_chunk_to_a_node_that_should_not_hold_it`** | Otherwise repair slowly spreads every chunk to every node and capacity accounting stops meaning anything. |
| **`a_holder_that_has_left_the_swarm_does_not_count_towards_the_floor`** | Counting a decommissioned machine means believing in a replica that no longer exists. |
| **`the_census_counts_distinct_holders_not_repeated_claims`** | A peer answering twice must not inflate the replica count into a false sense of safety. |
| `one_missing_replica_produces_exactly_one_push_to_the_right_node` | The ordinary case, exactly. |
| `a_fully_replicated_chunk_needs_nothing` | No make-work. |
| `a_plan_is_deterministic_and_ordered` | Two nodes produce comparable plans, so an operator can diff two logs. |
| `an_empty_census_produces_an_empty_plan` / `nothing_is_planned_when_no_node_is_reachable` | Edges. |

---

# `itsanas-folder` — unit tests (31)

## `decision` — what should happen to one path (15)

A pure function of three content hashes: what is on disk, what the store says,
and what this device last put there. Every branch is a unit test because
several of them are hard to stage on a real filesystem and all of them are
destructive if wrong.

| Test | What it proves |
| --- | --- |
| **`a_file_that_was_never_downloaded_is_exported_not_deleted`** | The most dangerous confusion in the design. A device that has never had a file must not read its absence as a deletion — that would announce the removal of everything its owner has. |
| **`deleting_from_the_store_only_ever_follows_a_recorded_local_file`** | Exhaustive over all 27 combinations: the destructive action is unreachable unless the ledger says this device genuinely had the file. |
| **`a_stale_ledger_alone_never_moves_data`** | Whatever the ledger says, if disk and store agree the answer is bookkeeping — never an upload, download or delete. |
| **`the_same_edit_made_twice_is_not_a_conflict`** | Both sides changed to identical content. A sibling here would litter the folder for nothing. |
| **`a_delete_racing_an_edit_brings_the_file_back`** / **`an_edit_racing_a_delete_keeps_the_edit`** | Matches the sync engine: an unexpected file costs a second, a lost edit is unrecoverable. |
| **`no_input_combination_panics_or_is_undecided`** | All 27 shapes of the problem are decided. |
| `a_new_local_file_is_imported` / `an_edited_local_file_is_imported` / `a_file_the_user_deleted_is_removed_from_the_store` / `a_remote_edit_is_written_out` / `a_remotely_deleted_file_is_removed_from_disk` / `two_different_edits_keep_both` / `both_sides_deleting_agrees` / `nothing_to_do_when_all_three_agree` | The ordinary cases. |

## `scan` — reading the folder safely (12)

| Test | What it proves |
| --- | --- |
| **`a_logical_path_can_never_escape_the_folder`** | Traversal, absolute paths, drive letters and Windows device names all refused. These strings arrive in a peer's log, so a path that escaped would let a peer write anywhere on the disk. |
| **`symlinks_are_skipped_rather_than_followed`** (unix) | A link inside the folder pointing at `~/.ssh` would otherwise quietly upload a private key, and the user would see only a harmless-looking file. |
| **`operating_system_debris_is_ignored`** | Every machine writes its own `.DS_Store`; syncing them means they fight forever. |
| **`the_staging_directory_is_never_synced`** | Otherwise the folder syncs its own scratch space back and forth. |
| **`nested_directories_become_slash_separated_logical_paths`** | Backslashes must not leak into logical paths, or one file has two names on two machines. |
| `round_tripping_a_path_through_the_filesystem_and_back_is_stable` | The mapping is a genuine round trip. |
| Others | Flat scans, empty folders, missing folders, sizes and mtimes, paths outside the root. |

## `watch` — noticing changes (4)

| Test | What it proves |
| --- | --- |
| **`the_debounce_is_long_enough_to_outlast_an_editor_save`** | Acting on the first event would import a half-written file, and because the store hashes what it reads, that truncation would become a real version and replicate. |
| **`watching_a_missing_directory_is_an_error_rather_than_silence`** | Silently watching nothing would mean the daemon believes it is reacting when it is not. |
| `a_real_change_is_noticed` / `a_quiet_folder_times_out_rather_than_blocking_forever` | It works, and it does not hang. |

---

# `itsanas-folder` — integration tests (22)

| Test | What it proves |
| --- | --- |
| **`a_brand_new_device_downloads_everything_and_deletes_nothing`** | The catastrophe. An empty folder on a device that has never synced must produce downloads, not a mass deletion. |
| **`a_file_the_user_deletes_is_deleted_everywhere`** | The counterpart — a genuine delete must propagate, with a tombstone so an offline device does not resurrect it. |
| **`an_imported_file_is_announced_to_peers_not_just_stored_locally`** | A real bug found by running two daemons: the reconciler wrote to the store but never sealed a log segment, so files looked synced on the machine that had them and existed nowhere else. |
| **`a_deletion_is_announced_to_peers_too`** | The same, for deletes. |
| **`a_pass_that_changes_nothing_does_not_produce_an_empty_segment`** | Flushing unconditionally would mint a segment on every idle scan and grow the log without bound. |
| **`reconciling_twice_does_nothing_the_second_time`** | A non-idempotent reconciler means a daemon uploads the folder forever and never settles. |
| **`a_local_edit_colliding_with_a_remote_one_keeps_both`** | Both survive, and the sibling keeps its extension so it still opens in the right application. |
| **`a_deep_pass_catches_an_edit_the_fast_path_misses`** | Documents the size-and-mtime gap and proves the deep scan closes it. |
| **`deleting_the_last_file_in_a_tree_prunes_the_empty_directories`** | Without it, every machine slowly fills with empty directories nothing removes. |
| **`no_staging_file_survives_a_reconcile`** | Every export would otherwise leak a temp file. |
| `a_second_device_reproduces_the_folder_exactly` | Two machines, one folder content, byte for byte. |
| `a_full_corpus_round_trips_through_a_folder_byte_for_byte` | A real data set, unchanged. |
| Others | New files, edits, remote changes, remote deletes, nested directories, delete/edit races, identical concurrent edits, empty folders. |

---

# `itsanas-wire` — framing (17)

Every byte parsed here comes from a stranger's computer.

| Test | What it proves |
| --- | --- |
| **`an_oversized_length_is_rejected_before_anything_is_allocated`** | Five bytes on the wire asking the peer to reserve four gigabytes. On a Raspberry Pi a handful of these is fatal. |
| **`every_truncation_of_a_valid_frame_is_an_error_and_never_a_panic`** | Every prefix of a valid frame, rejected rather than half-parsed. |
| **`corrupting_any_byte_never_panics`** | Every single-bit corruption of every byte. The decoder may reject; it may not abort the process. |
| **`arbitrary_garbage_never_panics`** | Random bytes fed to both the one-shot decoder and the streaming reader. |
| **`the_reader_does_not_grow_without_bound_on_a_stalled_frame`** | A peer that sends a header then trickles bytes forever cannot make the buffer exceed one maximum frame. |
| **`a_frame_split_across_reads_is_reassembled`** | The normal case on a real stream, byte by byte. |
| **`an_unknown_wire_version_is_refused_not_guessed_at`** | No silent reinterpretation of a future format. |
| `a_frame_exactly_at_the_limit_is_accepted_and_one_byte_over_is_not` | The boundary is where it is documented to be. |
| `several_frames_in_one_read_are_all_returned` | Batched arrivals are all delivered, leaving nothing buffered. |
| `the_header_is_exactly_as_documented` | The layout matches the doc comment. |
| `a_frame_round_trips` / `an_empty_payload_is_a_valid_frame` | The basic paths. |

The five remaining tests cover `Connection`, the generic `Read + Write` wrapper:

| Test | What it proves |
| --- | --- |
| **`a_close_part_way_through_a_message_is_an_error`** | A peer that hangs up mid-frame must not have its partial response treated as complete. This is the difference between a truncated answer and a short one. |
| `a_clean_close_between_messages_is_not_an_error` | The legitimate case is not turned into a failure. |
| `an_oversized_frame_is_refused_rather_than_buffered_towards` | The limit is enforced on the streaming path too, not only the one-shot decoder. |
| `a_message_round_trips` / `several_messages_come_back_in_order` | The basic paths, and that nothing is left buffered between messages. |

---

# `itsanas-tls` — device authentication (11)

Six unit tests in `auth.rs`, five integration tests in `tests/handshake.rs`.

| Test | What it proves |
| --- | --- |
| **`a_proof_from_one_session_is_worthless_in_another`** | The property the whole transport rests on. A man in the middle who terminates TLS has two sessions with two different exporters, so a captured proof cannot be relayed into the other one. If this test is ever weakened, authentication becomes replayable and nothing else in the crate catches it. |
| **`the_payload_never_reaches_the_socket_in_plaintext`** | Records every byte actually written to the socket and scans it for a canary. Proves the encryption is on the wire, not merely configured. |
| **`dialling_a_device_and_reaching_a_different_one_is_refused`** / `dialling_a_known_peer_refuses_a_different_answer` | An address that resolves to the wrong machine is refused rather than trusted — the coordinator hands out addresses and is not trusted to say who lives at one. Tested at both the proof layer and the socket layer. |
| `claiming_to_be_another_device_fails` / `a_tampered_signature_is_refused` | The two direct forgeries. |
| `an_honest_proof_identifies_the_device` / `the_proof_round_trips_through_the_wire` | The mechanism works, and works through framing. |
| `a_server_learns_who_called_without_being_told_in_advance` | A node can serve a device it has never met, which is what lets anyone offer storage. |
| `every_process_presents_a_different_certificate` | Certificates are anonymous and disposable, so an observer cannot correlate two connections by them. |
| `two_devices_authenticate_each_other_and_exchange_a_message` | End to end over a real socket. |

---

# `itsanas-coord` — claims, directory, accounting (47)

Catalogued by property rather than test by test: the crate is a library with no
server yet, and what matters is which rule each group of tests pins down.

**Claims and revocation.** Device claims cannot be forged, retimed or stripped of
their revocation; a claim dated far in the future is refused, because
supersession is by timestamp and such a claim could never be replaced; replaying
an old enrolment cannot un-revoke a stolen laptop; presence is signed by the
device and a claim by the owner, so a laptop changing networks never needs the
key that can revoke everything; a username cannot be taken over by another key
and re-registering cannot reset the joining date; a device cannot be claimed
without an account or by two accounts; **a node cannot inflate its own
availability by saying so** — a single heartbeat buys only the floor; a
coordinator that was itself offline for a year does not annihilate everyone's
standing; escrow is off until asked for; the accounting floors entitlement
against the member, clamps availability at both ends, and permits reclaiming
only in the harshest state.

---

# Planned tests

Listed here so the gap between what is claimed and what is verified stays
visible. These land with the milestones in [ROADMAP.md](ROADMAP.md).

## M2 — remaining

Deferred to the milestone that makes them meaningful — they need a second node,
which does not exist until M4:

- **Storage accounting**: pledged bytes, bytes on disk and bytes in the index
  agree within the documented overhead, so a node cannot silently under-provide.
- **Data-presence audit**: after replication, Bob's store holds the expected
  chunk count for Alice, each byte-identical to what Alice would re-derive, and
  Bob can decrypt none of them.

## M3 — remaining

The convergence suite above covers the simulation, the offline-device scenario,
concurrent edits and the delete/edit race. Still outstanding:

- **Rename detection** does not re-upload chunk data. Deduplication already makes
  a rename cheap in bytes — the chunks are identical, so nothing is re-stored —
  but the operation log currently records it as a delete plus a create rather
  than as a rename, which costs a log entry and loses the user's intent.
- **File watching**, once M7 gives it a daemon to live in: dropped `notify`
  events under load are covered by the periodic rescan, and that rescan needs a
  test that removes events deliberately.

Deliberately *not* planned: a clock-skew test. Ordering never consults a clock —
the version vectors carry no timestamps and `recorded_unix` is advisory and read
by nothing that decides anything. A test asserting that a wrong clock changes
nothing would be asserting the absence of code that does not exist, which is the
kind of test this project treats as worse than none.

## M4 — remaining

The decoder is already exercised against every truncation and every single-bit
corruption of a valid frame, and against arbitrary garbage. Still outstanding:

- **A real fuzzing campaign** (`cargo-fuzz`). The hand-written adversarial suite
  covers the inputs someone thought of, which is exactly the set a fuzzer is
  needed to go beyond.
- **Refetch elsewhere**: a peer returning a chunk that fails to open is detected
  today (the AEAD tag catches it), but nothing yet retries the fetch against a
  different host — there is no placement layer to supply one.
- **Reputation**: a peer that fails a storage challenge should be marked
  unreliable. The challenge works; nothing records the result yet.
- **QUIC**: the transport is TLS 1.3 over TCP and fully tested; QUIC is now only
  wanted for NAT hole punching. Everything above the transport is
  transport-agnostic, so these tests should port unchanged — which is the point
  of the split, and worth checking rather than assuming.

## M5 — remaining

Minimal disruption, weighted distribution and owner affinity are all measured
above. What is not yet tested, because it is not yet built:

- **A chunk dropping below the floor is repaired without intervention.** The
  plan is computed and tested; nothing executes it. The end-to-end test needs
  the daemon.
- **A peer that fails a storage challenge is recorded as unreliable**, and
  repair stops counting it towards the floor.

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
