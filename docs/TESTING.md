# Test Catalogue

**Last updated: 2026-08-27 — 217 tests across 7 binaries, plus 2 doctests.**

| Binary | Tests |
| --- | --- |
| `itsanas-crypto` unit | 64 (1 `#[ignore]`d) |
| `itsanas-crypto` property (`tests/properties.rs`) | 15 |
| `itsanas-store` unit | 73 |
| `itsanas-store` integration (`tests/store.rs`) | 27 |
| `itsanas-sync` unit | 12 |
| `itsanas-sync` convergence (`tests/convergence.rs`) | 19 |
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
| **slow-tests** | `cargo test -- --ignored --test-threads 1` | One test is currently marked `#[ignore]`: the real 64 MiB Argon2id cost. Too slow for every push, far too important to never run. Large simulated swarms will join it at M5. |
| **cross-build** | `cargo build --release --target aarch64-unknown-linux-gnu` | The Raspberry Pi 4B+ is a first-class deployment target. Catching a dependency that does not cross-compile at PR time is much cheaper than at deploy time. |
| **minimum-rust-version** | `cargo check` on the pinned MSRV | Prevents accidentally requiring a newer toolchain than the documented minimum, which would break users on distro Rust. |
| **supply-chain** | `cargo deny check` | Fails on any unpatched advisory, any yanked crate, and any licence not compatible with AGPL-3.0. For a system whose entire value is "your host cannot read your data", a vulnerable crypto dependency is a release blocker. |
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

# `itsanas-store` — unit tests (59)

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

# `itsanas-store` — integration tests (22)

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
