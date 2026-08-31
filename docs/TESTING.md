# Test Catalogue

**Last updated: 2026-08-31 — 604 test functions across 21 binaries, 3 of them
`#[ignore]`d, plus 2 doctests. Eighteen are red-team tests.**

| Binary | Tests |
| --- | --- |
| `itsanas-crypto` unit | 64 (1 `#[ignore]`d) |
| `itsanas-crypto` property (`tests/properties.rs`) | 15 |
| `itsanas-wire` unit | 19 |
| `itsanas-tls` unit | 6 |
| `itsanas-tls` handshake (`tests/handshake.rs`) | 5 |
| `itsanas-store` unit | 130 |
| `itsanas-store` integration (`tests/store.rs`) | 29 (1 `#[ignore]`d) |
| `itsanas-sync` unit | 12 |
| `itsanas-sync` convergence (`tests/convergence.rs`) | 19 |
| `itsanas-net` unit | 30 |
| `itsanas-net` two-node (`tests/two_nodes.rs`) | 28 |
| `itsanas-placement` unit | 29 |
| `itsanas-coord` unit | 55 |
| `itsanas-coord` integration (`tests/coordinator.rs`) | 12 |
| `itsanas-discover` unit | 36 |
| `itsanas-policy` unit | 15 |
| `itsanas-folder` unit | 31 |
| `itsanas-folder` integration (`tests/folder.rs`) | 22 |
| `itsanas-cli` unit | 39 |
| `itsanas-cli` crash (`tests/crash.rs`) | 1 (`#[ignore]`d) |
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

## Red-team tests

A test whose name begins `red_team_` describes an **attack**. It passes when the
attack fails. Each one names the attacker, what it costs them, and what they get
if the test ever goes green for the wrong reason — because a security test whose
failure message is `assertion failed: !x` teaches nobody anything at three in the
morning.

They exist because an ordinary test found nothing here. The eviction protection
in `itsanas-discover` had a test that passed while the daemon above it was
handing protection to every stranger that dialled: the test **confirmed the
honest peer and nobody else**, encoding the assumption instead of checking it.
The attack was found by reading the code, not by running the suite. These tests
are the answer to that.

| Test | The attack it defeats |
| --- | --- |
| **`red_team_a_flood_of_authenticating_strangers_cannot_take_over_the_table`** | A device id is a free keypair. Mint 600, have every one claim the victim's owner tag so they sort to the front of the dial order, answer every dial correctly and store nothing. If merely authenticating earned protection, they would all become unevictable, fill the table, and the real Raspberry Pi would be refused entry forever while every node reported discovery as working. One laptop on the same wifi could silently stop a household syncing. |
| **`red_team_dialling_strangers_is_rationed_so_a_flood_cannot_eat_the_interval`** | Three hundred minted identities announce themselves. Without a cap the daemon opens three hundred connections per round and spends the whole sync interval shaking hands with machines that store nothing. |
| **`red_team_a_peer_that_only_answered_the_phone_has_earned_nothing`** | The rule underneath both of the above: completing a mutually authenticated handshake proves possession of a keypair generated a second earlier. It identifies a peer; it vouches for nothing. |
| **`red_team_a_failed_round_earns_nothing`** | Offering data a peer never took is not the peer storing it. |
| **`red_team_a_host_that_keeps_discarding_stops_getting_free_uploads`** | The follow-up attack: keep doing it, and let the owner's own repair drain their uplink forever. |
| **`red_team_a_host_that_keeps_discarding_stops_costing_bandwidth`** | The rule underneath it. |
| **`red_team_a_host_that_threw_the_data_away_stops_counting_as_a_holder`** | Accept everything, delete it, keep claiming the space. Free, undetectable without audits, and fatal to the replication guarantee. |
| **`red_team_the_user_id_never_appears_on_the_wire`** | Sit on a café or hotel network and listen. A user id is a public key; broadcasting it every thirty seconds would tell the room whose machine this is. The announcement carries a keyed tag instead. |
| **`red_team_a_stranger_cannot_compute_the_tag_without_knowing_the_user_id`** | Claiming to be one of the victim's own machines buys priority in their dial order. A guessable tag would hand that over for free; a keyed derivation means you must already know who you are targeting. |
| **`red_team_grinding_one_account_is_cut_off_after_a_few_attempts`** | The escrow blob is fetchable by anyone with a username, because a machine recovering from nothing has nothing to prove with. Grinding it is the attack, and the rate limit is the only defence — the single job a central component does better than a distributed one. |
| **`red_team_flooding_invented_names_cannot_reset_a_real_account_counter`** | The limiter is a table a stranger writes into. Evicting to make room would let an attacker clear their own counter. |
| **`red_team_reconnecting_does_not_reset_the_escrow_attempt_budget`** | A per-connection budget is no budget: reconnecting costs a handshake and buys a fresh one. |
| **`red_team_an_unenrolled_device_cannot_overwrite_someone_elses_escrow`** | Substituting a container whose passphrase you chose. |
| **`red_team_a_device_cannot_publish_an_address_for_a_device_it_does_not_own`** | Black-holing a member's machines through the address book. |
| **`red_team_a_name_cannot_be_taken_over_by_a_different_key`** | Sending everyone who looks a member up to an impostor. |
| **`red_team_an_oversized_username_is_refused_before_the_directory_sees_it`** | A megabyte where a name is expected. |

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

# `itsanas-store` — unit tests (115)

## `reliability` — remembering that a peer failed (6)

| Test | What it proves |
| --- | --- |
| **`red_team_a_host_that_keeps_discarding_stops_costing_bandwidth`** | The decision rule under the test above: three consecutive failures pause new content. |
| `one_failure_is_not_enough_to_stop_sending` | A host mid-restart, a swapped disk, a chunk collected on one side of a race. Reacting to a single failure would make a household stop syncing every time a machine rebooted at the wrong moment. |
| `one_pass_clears_the_record` | A sanction with no exit is a ban. |
| `the_lifetime_totals_survive_a_reset` | Consecutive failures decide the sanction; the totals are for somebody deciding whether to keep a peer at all, and clearing them on every pass would hide a host that fails half the time. |
| `counters_saturate_rather_than_wrapping` | A wrap would turn a peer that failed four billion challenges into a trusted one. |
| `a_paused_peer_explains_itself_and_a_healthy_one_says_nothing` | The message names the way back. |

## `holders` — the placement ledger's key layout (7)

| Test | What it proves |
| --- | --- |
| **`everything_one_device_holds_sorts_together_in_the_other_ordering`** | The reason a second ordering exists at all: without it, "what does this peer hold?" walks every row for every peer on every audit round. |
| `the_two_orderings_describe_the_same_pair` | Both encodings of one fact decode back to it. |
| **`every_holder_of_one_chunk_sorts_together`** | Why the chunk comes first in the composite key. If the device sorted first, "who holds this chunk?" would walk the whole table, which on a Pi with a million chunks is the difference between a repair pass that finishes and one that does not. |
| `a_chunk_held_only_here_is_flagged_as_the_only_copy` | The alert condition is distinguishable from an ordinary shortfall: everything else is background work, this one is a disk failure away from loss. |
| `a_chunk_held_more_widely_than_its_target_has_no_shortfall` | Saturating rather than wrapping. An underflow here would ask the repair loop for four billion pushes. |
| `a_key_round_trips_through_its_two_halves` / `a_key_of_the_wrong_length_is_refused_rather_than_guessed_at` | The encoding, and that a key written by something else is refused rather than reinterpreted. |

## `index` — the placement ledger (15)

Where this node's data actually went. This is what replaced the coordinator's
signed node-set epoch: an owner who already keeps a log of their own chunks can
record where they put them, and then no global membership list has to be agreed
by anybody. See [DESIGN.md](DESIGN.md) §8.

| Test | What it proves |
| --- | --- |
| **`the_two_orderings_never_disagree_whatever_is_done_to_the_ledger`** | The ledger is kept in two key orders, because "who holds this chunk?" and "what does this peer hold?" are range scans under opposite prefixes and answering one with the wrong ordering is a full table walk — fourteen million rows per audit round at a terabyte. That is denormalised state, which this project refuses everywhere else, and the refusal is only earned if every path writing one writes the other in the same transaction. Exercised across recording, batching, forgetting a holder, forgetting a device, and collecting a chunk. |
| **`red_team_the_same_question_is_not_asked_twice_every_round`** | The attack that broke the audit for six commits: keep the chunks that will be asked about, delete the rest. Selection used to sort by when each record was last confirmed — but a push round re-stamps a whole batch from one clock reading, so every timestamp in a batch was equal and the sort fell through to its tie-break, the chunk id. The same sixteen lowest ids, every round, for ever. Sixteen chunks out of fourteen million bought a spotless record. |
| `the_challenges_for_one_device_never_name_another_device_s_chunks` | Exhaustive over every cursor in the space: no draw may wander out of one peer's range into a neighbour's. Auditing a peer on another peer's records fails an innocent host. |
| **`every_holding_is_reachable_by_some_cursor`** | A cursor past the device's highest id wraps rather than being discarded. Without the wrap the lowest-numbered chunks would be the only ones never asked about — a hole an attacker can park its deletions in. |
| **`a_probe_is_remembered_until_the_peer_answers_for_it`** | The probe survives a failed round (the peer still owes an answer) and is cleared by a passing one (the sanction is over). A marker left standing after the pause lifts would misdirect the next round's questions. |
| **`a_ledger_written_before_the_second_ordering_is_rebuilt_on_open`** | Every write path writes both orderings, so they cannot drift while running — but they can *start* apart, on a store written before the device-first table existed. Left alone that is silent and total: challenge selection reads the second table, so no audit would ever ask anything, and a node that has stopped checking its hosts looks exactly like one whose hosts are honest. |
| **`a_target_counts_this_device_so_three_asks_for_two_elsewhere`** | The counting convention, pinned. Off by one here means the repair loop targets two copies while reporting three, and nothing ever says so — it surfaces as data loss after two machines die instead of after three. |
| **`the_chunks_closest_to_being_lost_are_reported_first`** | A repair pass on a laptop is interrupted by the lid closing. Ordered by chunk id, the work done before the interruption would be random with respect to risk, and the chunk with one copy left could wait behind a thousand that had two. |
| **`recording_the_same_holder_twice_refreshes_rather_than_duplicates`** | A peer syncing hourly acknowledges the same chunks every hour. One row per acknowledgement would grow the ledger without bound and inflate the replica count — wrong in the direction that hides a real shortage. |
| **`forgetting_a_device_clears_it_from_every_chunk_and_nothing_else`** | A peer that left stops being evidence for every chunk at once, and every other device's records survive. Otherwise losing one peer looks like losing all of them and the node re-uploads its entire store. |
| **`collecting_a_chunk_takes_its_holder_records_with_it`** | Garbage collection does not leave the repair loop working to restore the replication of a chunk that no longer exists. |
| **`the_ledger_survives_reopening`** | It is the only record of where this node put its data. Losing it on restart would make every node re-upload everything after a reboot. |
| `forgetting_a_holder_leaves_the_others_alone` | One failed storage challenge removes one host, not all of them. Removing more would make a single bad answer start a repair storm. |
| `an_unreferenced_chunk_is_not_reported_as_under_replicated` | Deleted and overwritten data is on its way out; restoring its replication is work done to keep something nobody wants. |
| `holders_come_back_sorted_so_two_devices_agree_on_order` | Two nodes comparing ledgers see the same order. |
| `holders_are_kept_apart_by_chunk` / `a_recorded_holder_comes_back` | The basic paths. |
| `recording_a_batch_matches_recording_one_at_a_time` | A sync round commits once rather than once per chunk, which on an SD card is most of the time spent. |
| `recording_an_empty_batch_does_nothing_rather_than_opening_a_transaction` | A quiet round costs no write. |

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
| **`removing_an_absent_file_still_records_the_tombstone`** | A device deleting a file it never downloaded must still record the deletion, or a machine that was away when a file arrived cannot take part in removing it — and the file comes back on the next sync. |
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

# `itsanas-net` — unit tests (30)

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

## `session` — what a round establishes (5)

| Test | What it proves |
| --- | --- |
| **`red_team_a_peer_that_only_answered_the_phone_has_earned_nothing`** | See **Red-team tests** above. |
| **`red_team_a_failed_round_earns_nothing`** | See **Red-team tests** above. |
| `a_peer_that_already_held_our_data_has_earned_it` | The steady state of a host that has been storing for weeks: nothing to send, nothing to fetch, and still the most valuable peer this node knows. Requiring fresh transfer would demote every long-standing host to stranger the moment it caught up. |
| `a_peer_that_accepted_our_data_has_earned_it` / `a_peer_that_served_us_our_own_work_has_earned_it` | The two ways a peer proves it is real: it stored something, or it gave us something of ours. |

---

# `itsanas-net` — two-node tests (28)

Real stores, real chunking, real sealing, real signatures, real TCP.
`tests/two_nodes.rs`.

| Test | What it proves |
| --- | --- |
| **`a_sync_round_records_which_peer_now_holds_this_nodes_data`** | The replacement for a coordinator-published node set, end to end over a socket. Without it the repair loop has no idea whether a chunk exists anywhere but on this disk, and the honest answer to "is my data safe?" is "no idea". |
| **`a_peer_that_already_had_the_data_is_still_recorded_as_holding_it`** | The property that makes the ledger converge rather than only grow. A device restored from its recovery phrase learns where its data lives by *asking*, instead of re-uploading its whole store to find out — and the answer costs nothing extra, since it is the same round trip that decides what to send. |
| **`a_host_that_refuses_to_store_is_not_recorded_as_holding_anything`** | A node that pledged nothing still answers, because refusing to host does not stop it being a peer. Recording it as a holder would let a node believe its data was replicated onto a machine that declined it — the worst possible error, indistinguishable from safety until the local disk dies. |
| **`red_team_a_host_that_keeps_discarding_stops_getting_free_uploads`** | The attack auditing alone does **not** stop. Accept, delete, wait: the audit catches it every round and the owner re-uploads every round, so the host pays nothing and the owner pays a full upload each time. The more data the owner has, the more it costs them. Detection without memory is not a defence. |
| **`a_replacement_device_pulls_a_whole_corpus_back_from_a_stranger`** | MVP acceptance test D, the half nobody had checked. Recovery from a passphrase restores the *account* — the user id, the keys, the ability to speak — and says nothing whatever about whether the files come back, which is the only part the user cares about. A machine writes a corpus, edits one file, deletes another, uploads to a host belonging to somebody else, and is destroyed; a replacement built from the same master secret with a **new device id** pulls. Contents byte for byte, the edit rather than the original, and the deletion as a deletion — that last is the one that fails quietly, because a restore which resurrects everything you ever deleted looks exactly like one that worked. It also asserts the restored device knows *where* its data lives, which is what found that a pull recorded no holders at all. |
| **`red_team_a_host_that_keeps_only_what_it_expects_to_be_asked_is_caught`** | The same attack as the unit test above, end to end over a socket, against the exact set the old rule would have named: the host keeps the sixteen lowest chunk ids out of 117 and deletes the rest. Under the old rule it survived every round for ever. |
| **`a_paused_host_that_starts_answering_again_is_sent_data_again`** | The way back, on a store of a hundred chunks rather than one. A paused peer is offered one chunk a round; the first version left the audit to *find* it in the ledger, where it sat as one fresh record among the thousands the peer is paused for, so every question landed on something it had already lost and the sanction never lifted — a ban wearing the words of a suspension. The probe is now written down and is the only thing a paused peer is asked about: accept, answer, cleared, in two rounds. **The earlier version of this test used a 37-byte file** — one chunk, one record, the single case where finding the probe is guaranteed — so it passed while the mechanism it named did not work. |
| **`red_team_a_host_that_threw_the_data_away_stops_counting_as_a_holder`** | The attack that costs nothing: accept everything offered, delete it immediately, keep claiming the space. A node trusting its own ledger would believe its files were on three machines while two held nothing, and find out on the day the third disk died. The audit withdraws the record, the chunk shows as under-replicated, and the same round re-uploads it. |
| **`an_audit_never_asks_about_a_chunk_it_could_not_check`** | Verifying a proof means re-deriving the sealed bytes locally. Challenging on a chunk this device has collected would fail for a reason that is nothing to do with the peer, and would withdraw an honest record. |
| `an_audit_confirms_a_host_that_is_still_holding_the_data` | The ordinary path: evidence becomes proof, for the moment it is asked. |
| **`a_metadata_round_makes_the_file_listable_before_it_is_downloaded`** | The behaviour everyone expects from a phone: everything listed, tap one to download it. Before the catalogue, a metadata round left the file invisible — deferred means no index entry, and a client on a metered connection could show nothing at all. |
| **`a_metadata_round_learns_what_changed_without_downloading_it`** | The other half: nothing is fetched, nothing is half-written, and a later round on an unmetered connection completes it. Writing this test is what found that deferred work was never retried. |
| **`a_delete_racing_an_edit_still_leaves_the_file_listed`** | The listing applies the same asymmetry as the merge engine. A listing that hid a file the engine is about to keep would tell somebody their edit was lost. |
| **`a_file_deleted_elsewhere_is_never_offered_for_download`** | A client that listed a file deleted last week, and fetched it when tapped, would have resurrected it. |
| `a_metadata_round_offers_the_log_but_sends_no_chunks` | The upload direction: a photo taken on mobile data does not upload itself, and the peer still learns it happened. |
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

---

# `itsanas-cli` — crash consistency (1, `#[ignore]`d)

`tests/crash.rs`. Spawns the real binary, kills it mid-write, and checks what
survived. Ignored because every invocation pays a full production Argon2id
derivation and it spawns a dozen; the `slow-tests` CI job runs it.

| Test | What it proves |
| --- | --- |
| **`a_store_killed_mid_write_never_lists_a_file_it_cannot_read`** | MVP acceptance test J for the half a test can reach. A dozen hard kills at measured points inside a real write, each followed by `doctor --deep`, and after every one the store must be readable, undamaged, and need no repair. A file that never appeared is fine; orphaned chunks are fine and expected. A file the store *lists* and cannot read is not. |

Two things this test is careful about, both learned the hard way:

- **It calibrates rather than guesses.** The first version killed at fixed
  millisecond delays, passed, and exercised nothing: every kill landed inside
  the Argon2id derivation that precedes any store access, and zero chunks were
  written in any round. It now times one complete write and kills at fractions
  of it, and an assertion fails the test if no round managed to
  interrupt real work.
- **It does not claim to cover a power cut.** Killing a process discards the
  process, not the kernel's page cache. Verified rather than assumed: the suite
  was re-run with `blob.rs`'s per-chunk `sync_all` removed and passed
  identically, so this cannot distinguish a store that flushes from one that
  does not. `docs/MVP.md` records the ten-second experiment on real hardware
  that would.

---

# `itsanas-cli` — unit tests (39)

## `bench` — measuring this machine (4)

`itsanas bench` exists because "will this work on a Raspberry Pi" can only be
answered by the person holding one. Its own correctness matters more than most:
a benchmark that measures a broken path produces a confident wrong number.

| Test | What it proves |
| --- | --- |
| **`the_generator_produces_exactly_what_was_asked_for`** | Every throughput figure divides by this. A generator quietly delivering fewer bytes would inflate all of them. |
| **`the_generator_is_deterministic_so_the_check_at_the_end_is_meaningful`** | The round-trip check compares what was read back against a second run of the generator. Non-deterministic and every run fails; constant and the check proves nothing. |
| `a_stage_that_took_no_measurable_time_reports_zero_rather_than_infinity` | Dividing by a zero duration gives `inf`, which formats as a nonsense size and reads as a spectacular result. |
| `durations_are_reported_in_units_a_person_can_act_on` | "15.3 hours" is a decision; "55080 seconds" is arithmetic homework. |

## `discovery` — the daemon's use of local discovery (7)

| Test | What it proves |
| --- | --- |
| **`a_confirmed_device_survives_a_flood_of_strangers`** | The eviction attack at the layer the daemon actually uses. Without confirming a device after a successful authenticated round, anyone on the network can push the Raspberry Pi out of the laptop's table and the two stop finding each other while both believe discovery is working. |
| `a_discovered_device_becomes_something_to_dial` | Discovery produces an address *and* the device to pin, which is what stops an address answering as somebody else being trusted. |
| **`red_team_a_flood_of_authenticating_strangers_cannot_take_over_the_table`** | See **Red-team tests** above. This is the one that found a real bug. |
| **`red_team_dialling_strangers_is_rationed_so_a_flood_cannot_eat_the_interval`** | A flood cannot consume the interval real syncing needs. |
| `a_confirmed_peer_is_still_dialled_every_round_however_many_strangers_arrive` | The ration limits strangers, never work: three real machines still sync every round on a noisy network. |
| `the_neighbourhood_is_empty_until_something_is_heard` | No invented peers. |
| `the_poll_is_short_enough_that_shutdown_feels_immediate` | A Ctrl-C must not wait out an announce interval. |

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

# `itsanas-policy` — when to sync, and how much (15)

`src/lib.rs`. A decision table with an argument attached to every row, and no
dependency on anything — so the phone, the Mac shell and `itsanas daemon` reach
the same schedule instead of each keeping its own number. The daemon is what
uses it today: `itsanas daemon` prints the interval, the scope and the reason.

| Test | What it proves |
| --- | --- |
| **`every_combination_produces_a_plan_with_a_reason`** | Totality. A sync tool that silently does nothing in some unconsidered corner is the failure this crate exists to prevent, and "silently" is the operative word: every state has to be explainable to the person looking at it. It walks `Network::ALL`, `Power::ALL` and `Attention::ALL` rather than lists written out here, because the list written out here is the one somebody forgets — it went on checking two `Attention` variants after a third was added, and passed. |
| **`a_service_on_ethernet_does_not_inherit_a_phone_s_interval`** | The Pi in the cupboard is not a backgrounded app. Two hours is not a considered choice about ethernet; it is the smallest number that survives Android Doze, and applying it to a permanently-powered machine would make an edit take up to two hours to cross a household through a node that was awake the whole time. |
| **`a_service_on_a_metered_link_is_no_less_careful_than_a_phone`** | Being a service buys freedom from the *platform*, not from the data plan. A laptop tethered to a phone must not start uploading forty gigabytes because it is technically a daemon. |
| **`a_service_still_stops_when_the_battery_is_nearly_gone`** | Nothing about being a service makes the battery bigger. |
| **`switching_background_syncing_off_does_not_stop_a_daemon_somebody_started`** | That switch means "do not work unless I am looking at the app". Starting a daemon *is* the deliberate act it exists to require. A daemon silently doing nothing because of a phone setting is a support case nobody could diagnose. |
| **`nothing_is_moved_over_a_metered_connection_unless_it_was_asked_for`** | The row that decides whether somebody trusts this on their phone. A tool that silently spent a data allowance would be uninstalled once and remembered for years. |
| **`the_file_list_still_arrives_on_a_metered_connection`** | The other half: knowing *what* changed is kilobytes and always happens. Metadata and content are separate purchases. |
| `allowing_metered_downloads_actually_allows_them` | Somebody with an unlimited plan who says so is believed. |
| **`a_low_battery_stops_background_work_but_never_stops_a_person`** | Refusing to work while somebody is watching is how a tool gets a reputation for being broken. They can see the battery indicator themselves. |
| **`the_button_works_even_when_the_schedule_would_not`** | A button that does nothing teaches people the application is broken. Neither a low battery nor a metered connection overrides a deliberate act; only having no network does. |
| `an_open_application_on_free_wifi_syncs_almost_live` | The case everybody judges the product on. |
| `no_network_means_no_plan_and_no_button` | The one state where there is nothing to honour. |
| `switching_background_syncing_off_leaves_the_foreground_alone` | The setting is about the background, and only the background. |
| **`background_intervals_sit_above_every_platform_floor`** | Every mobile platform imposes a fifteen-minute floor on periodic background work. An interval below it is not a schedule, it is a number the operating system ignores. |
| **`a_day_of_metered_checking_is_not_measurable_on_a_data_plan`** | The arithmetic behind the once-a-day metadata round, so the claim in the module documentation is checked rather than asserted. |

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

---

# `itsanas-discover` — serverless local discovery (36)

The only parser in the project fed unsolicited packets by anybody, with no
handshake in front of it. Everything else sits behind TLS and behind a peer that
has already proved which device it is, so this crate is tested the way a network
edge has to be: every corruption, every truncation, and the failure modes of the
hardware it will actually run on.

## `beacon` — the announcement (14)

| Test | What it proves |
| --- | --- |
| **`a_device_cannot_advertise_a_device_it_does_not_own`** | The reason the packet is signed at all. Without it, anyone on the network claims to be the Raspberry Pi and every node dials them instead. |
| **`corrupting_any_single_byte_is_refused_and_never_panics`** | Every single-bit flip of every one of 147 bytes. The decoder may reject; it may not take the daemon down, and it may not accept a mutated field. |
| **`every_truncation_and_extension_is_refused_before_anything_is_read`** | The length is fixed, so every other length is rejected before a field is touched. There is no size on the wire for an attacker to lie about. |
| **`arbitrary_garbage_never_panics`** | Anything at all arrives on a UDP port, including another protocol's traffic on a machine that reuses the number. |
| **`a_signature_from_another_domain_does_not_verify_here`** | Domain separation checked rather than assumed: a signature the device made for the peer protocol must not be replayable as a presence announcement. |
| **`an_ancient_clock_still_produces_a_valid_announcement`** | A Raspberry Pi 4 has no real-time clock and announces itself believing it is 1970. It must still be findable, or a machine that just came back is invisible until NTP runs. |
| **`red_team_the_user_id_never_appears_on_the_wire`** | See **Red-team tests** above. |
| **`red_team_a_stranger_cannot_compute_the_tag_without_knowing_the_user_id`** | See **Red-team tests** above. |
| `the_tag_is_stable_so_a_household_keeps_recognising_itself` | The tag is deliberately not rotated on a clock: a Pi 4 has no RTC and boots in 1970, and a daily tag would make its own household treat it as a stranger exactly when it came back from a power cut. |
| `the_layout_is_exactly_as_documented` | The wire format is a compatibility commitment. If it drifts, an older build on another machine stops finding this one and the symptom is "discovery silently does nothing". |
| `an_unknown_version_is_refused_not_guessed_at` | No optimistic reinterpretation of a future format, whose fields may mean something else entirely at these offsets. |
| `foreign_traffic_is_discarded_on_the_magic_rather_than_the_signature` | Sharing a port with something else costs one comparison, not a signature check per packet. |
| `a_zero_port_is_refused` | An announcement nothing can serve is either a bug or bait for a connection that cannot succeed. |
| `an_announcement_round_trips` | The basic path. |

## `neighbours` — the bounded table (12)

| Test | What it proves |
| --- | --- |
| **`a_rebooted_pi_with_a_reset_clock_is_still_followed_to_its_new_address`** | Why the *receiver's* clock decides and the sender's is ignored. Superseding by sender clock would leave a rebooted Pi pinned to a stale address until NTP ran — exactly when someone is waiting for it to come back. |
| **`the_table_never_grows_past_its_capacity`** | A device id is a free keypair, so anyone on the network can mint valid announcements without limit. Unbounded means an out-of-memory kill on the Pi, triggered by a stranger. |
| **`a_flood_of_strangers_cannot_evict_a_known_peer`** | The eviction attack. Without protection, a flood pushes the Pi out of every table and the household stops syncing while every node believes discovery is working. |
| **`own_devices_are_dialled_before_strangers`** | Reaching your own machines is what makes a folder appear; a stranger is a hosting candidate and can wait. |
| `a_table_full_of_protected_devices_refuses_a_stranger_rather_than_forgetting_one` | The bound is never satisfied by discarding something known real. |
| `the_oldest_unprotected_entry_is_the_one_evicted` | Eviction is least-recently-heard, not arbitrary. |
| `expiry_forgets_the_quiet_but_keeps_your_own_switched_off_machines` | A laptop that is off has not stopped being your laptop; forgetting its address costs a slower reconnection every time it wakes. |
| `the_dial_order_is_stable_across_two_nodes_with_the_same_view` | Two machines that heard the same announcements produce the same list, so they do not retry each other in lockstep. |
| `a_device_that_changed_network_is_followed_and_reported` | A laptop moving between networks is followed, and the move is news. |
| `repeating_the_same_announcement_is_not_reported_as_news` | A beacon arrives every thirty seconds forever. Logging each one makes an unreadable journal, which is the same as no journal on the day something breaks. |
| `a_new_device_is_recorded_with_the_address_it_was_heard_from` | The address comes from the datagram, never from the packet. |
| `a_capacity_of_zero_is_treated_as_one_rather_than_never_recording` | A misconfiguration degrades rather than silently disabling discovery. |

## `lan` — the socket (10)

| Test | What it proves |
| --- | --- |
| **`announcing_to_port_zero_is_refused_rather_than_sent_into_the_void`** | **Found by running it, not by a test.** `bind(0)` used one number for both the local port and the broadcast target, so an ephemeral bind sent every announcement to `255.255.255.255:0` — accepted by the operating system, delivered to nobody, reported as five successful sends. |
| **`an_ephemeral_bind_still_announces_to_the_discovery_port`** | The other half of the same bug: listening and announcing are separate numbers. |
| **`an_oversized_datagram_is_rejected_rather_than_truncated_into_a_valid_one`** | The receive buffer is larger than a valid announcement on purpose. With an exact-sized buffer the kernel trims the excess and hands up something that parses, which is how a padded packet smuggles data past a parser. |
| **`a_tampered_announcement_is_refused_at_the_socket`** | Verification happens on the real path, not only on hand-built byte arrays. |
| `an_announcement_crosses_a_real_socket_and_verifies` | End to end over a real UDP socket. |
| `a_quiet_network_times_out_rather_than_blocking_forever` | A daemon polls this in a loop; a silent network must leave it idle, not hung. |
| `foreign_traffic_on_the_port_is_reported_as_foreign_not_as_a_failure` | A busy network must not flood the log and hide the failure that matters. |
| `a_broadcasting_socket_asks_the_kernel_for_broadcast` | Without `SO_BROADCAST` nothing reaches 255.255.255.255 and every send still reports success. |
| `two_nodes_on_one_machine_refuse_to_share_a_port` | Better a refusal at start-up than a second node whose discovery quietly never works. |
| `the_announce_interval_is_not_expensive_to_leave_running` | An acceptance criterion, not a preference: the first version that keeps a laptop awake gets uninstalled. Under half a megabyte a day, and one lost packet never forgets a peer. |

---

---

# `itsanas-coord` — the coordinator server (12 integration, 8 unit)

A real coordinator on a real socket: real directory, real TLS with device
authentication, real signatures, real framing. `tests/coordinator.rs`.

| Test | What it proves |
| --- | --- |
| **`escrow_is_stored_by_an_enrolled_device_and_recovered_by_name_alone`** | MVP acceptance test D at the protocol layer. A machine with no device, no account and no key fetches the sealed container using only the username, and the passphrase is what opens it. |
| **`red_team_reconnecting_does_not_reset_the_escrow_attempt_budget`** | The escrow blob is the one thing reachable without proving anything, so the rate limit is the whole defence. A per-connection counter would be no counter: an attacker reconnects, pays one handshake, and works through a word list. |
| **`red_team_an_unenrolled_device_cannot_overwrite_someone_elses_escrow`** | Replacing a member's container with one whose passphrase you chose would either take their account or — quieter — destroy their ability to recover, discovered on the day they needed it. |
| **`red_team_a_device_cannot_publish_an_address_for_a_device_it_does_not_own`** | Announcing somebody else's device at an address you control black-holes their machines. TLS pinning stops data being exposed; nothing else stops the denial of service. |
| **`red_team_a_name_cannot_be_taken_over_by_a_different_key`** | A username pointing at the wrong key sends everyone looking that member up to an impostor. |
| **`red_team_an_oversized_username_is_refused_before_the_directory_sees_it`** | Nothing downstream has to be robust against a caller-chosen length. |
| `escrow_is_off_until_a_blob_is_stored_and_can_be_withdrawn_again` | Passphrase recovery is a trade, so it is opt-in *and* reversible. Without the second half the only safe choice would be never to use it. |
| `a_member_registers_enrols_a_device_and_is_then_findable_by_name` | The ordinary path end to end: after this, somebody who knows only a username can reach the machines. |
| `a_connection_that_asks_too_much_is_told_why_rather_than_cut_off` | A silent close surfaces as "connection aborted by your host software", which reads like a firewall and sends whoever is debugging it an hour in the wrong direction. |
| `the_peer_list_is_bounded_however_many_devices_a_user_enrols` | A member with a thousand devices is not a way to make the coordinator send a thousand records to anybody who asks. |
| `a_version_mismatch_is_refused_rather_than_guessed_at` / `asking_about_an_unknown_name_says_so_rather_than_inventing_one` | No optimistic guessing, no invented answers. |

Unit tests in `service.rs` and `protocol.rs` cover the limiter's arithmetic and
the open-request list:

| Test | What it proves |
| --- | --- |
| **`red_team_grinding_one_account_is_cut_off_after_a_few_attempts`** | The limiter does what the whole centralisation argument rests on. |
| **`red_team_flooding_invented_names_cannot_reset_a_real_account_counter`** | The limiter is itself a table a stranger writes into. A full table that evicted the oldest entry would let an attacker clear their own counter by inventing names. |
| **`only_hello_and_escrow_retrieval_are_reachable_without_proving_anything`** | The hostile-internet argument rests on this list being two items long, so it is asserted rather than described. |
| `a_log_line_never_contains_anything_a_caller_wrote` | Otherwise a stranger picks their username and writes into the operator's journal. |
| `the_budget_comes_back_after_the_window` | Somebody who mistypes five times is not locked out of their own account for good. |
| `expired_windows_are_forgotten_so_the_table_does_not_fill_with_history` / `a_name_that_was_never_asked_about_is_allowed_once_the_table_has_room` | The bound holds without leaking history. |
| `a_request_round_trips_through_postcard` | The encoding. |

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
