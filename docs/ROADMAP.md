# Roadmap vs Current State

**Last updated: 2026-08-27.** This file is updated in the same change as the
code it describes. If it disagrees with the code, the code is right and this is
a bug.

## Current state at a glance

| Milestone | Crate | Status | Tests |
| --- | --- | --- | --- |
| M0 Repository, CI, licence | — | ✅ **done** | CI runs 7 jobs |
| M1 Cryptographic core | `itsanas-crypto` | ✅ **done** | 64 unit + 15 property |
| M1b Published test fixtures | `itsanas-testkit` | ✅ **done** | 7 |
| M2 Chunking and local store | `itsanas-store` | ✅ **done** | 73 unit + 27 integration + 1 doc |
| M3 Sync engine | `itsanas-sync` | 🟨 **mostly done** | 12 unit + 19 convergence + 1 doc |
| M4 Network transport | `itsanas-net` | ⬜ not started | — |
| M5 Placement and repair | `itsanas-placement` | ⬜ not started | — |
| M6 Coordinator | `itsanas-coord` | ⬜ not started | — |
| M7 Daemon and CLI | `itsanas-daemon`, `itsanas` | ⬜ not started | — |
| M8 Three-device bring-up | — | ⬜ not started | — |

**Nothing in this repository should hold data you care about yet.** The
cryptographic guarantees, the local store and the merge rules are implemented
and tested, and three simulated devices converge correctly through every
adversarial scenario the suite throws at them. But no byte has ever crossed a
real network: there is no transport, no placement, no coordinator and no daemon.
Until M4 exists, "sync" means "sync between processes on one machine".

---

## Done

### M0 — Repository, CI, licence ✅

- Cargo workspace, Rust 2024 edition, MSRV pinned and enforced.
- AGPL-3.0-or-later.
- CI with seven jobs: format, clippy at `-D warnings`, tests on
  Linux/Windows/macOS, expensive `#[ignore]`d tests, aarch64 cross-build for the
  Raspberry Pi, MSRV check, dependency advisories and licence audit
  (`cargo-deny`), coverage.
- Weekly scheduled CI run so a new advisory surfaces without waiting for a push.

### M1 — Cryptographic core ✅ (`itsanas-crypto`)

Implemented:

- **Identity.** 32-byte master secret ⇄ 24-word BIP-39 recovery phrase.
  Domain-separated key schedule via BLAKE3 `derive_key`: Ed25519 signing,
  X25519 agreement, chunk root, blinding key, oplog root.
- **Device keys.** Generated per machine, independent of the master secret, so a
  lost laptop is revoked without rotating the user's identity.
- **Sealing.** XChaCha20-Poly1305 in two modes — deterministic (content-addressed
  chunks, enabling deduplication and remote audit) and randomised (log segments).
  Ciphertext bound to owner, purpose, address and format version.
- **Blinded addressing.** Chunk ids are keyed hashes of the content hash, so they
  deduplicate for the owner and reveal nothing to a host.
- **Keystore.** Argon2id-sealed container for the master secret, used both as the
  on-device keystore and as the coordinator-hosted escrow blob. KDF parameters
  are bound into the associated data, so cost downgrade attacks fail.
- **Published-identity ban list.** Production refuses the three fixture users
  whose keys are printed in the docs.

Not yet implemented in this crate: key wrapping to other users' devices (the
X25519 agreement primitive exists, the certificate format does not), and device
revocation certificates.

### M1b — Published test fixtures ✅ (`itsanas-testkit`)

Alice, Bob and Carol, with real recovery phrases published in
[TEST-USERS.md](TEST-USERS.md), a generated and digest-pinned data corpus, and
per-user canary strings for on-disk plaintext-leak detection.

---

### M2 — Chunking and local store ✅ (`itsanas-store`)

Implemented:

- **FastCDC chunking** with normalised cut masks, 16 KiB / 64 KiB / 256 KiB by
  default. The 256-entry gear table is *derived* from BLAKE3 under a fixed
  domain string rather than pasted in as a magic constant, and pinned by a
  digest test — because if two devices ever disagree about chunk boundaries,
  deduplication silently stops working across the whole network.
- **Content-addressed blob store**, two-level 256-way fan-out, atomic
  write-then-rename, staging swept on open so a power cut leaks nothing.
  It only ever sees ciphertext; it has no access to a key.
- **Operation log** with sealed bodies in signed plaintext envelopes, so a blind
  host can order and serve segments it cannot read. Segments are **chained** —
  each names its predecessor — so a host cannot drop one from the middle of a
  chain undetected.
- **Transactional index** (`redb`): path → chunk list and chunk → refcount move
  in one transaction, because a crash between them would let garbage collection
  delete live data.
- **Garbage collection** with a grace period, and an integrity check that
  reassembles and re-hashes every file.
- **Path validation** that rejects traversal, absolute paths, backslashes and
  Windows device names — the paths in a peer's log are attacker-controlled input
  the moment the sync engine starts materialising files.

**Exit criteria — met.** `alices_entire_corpus_round_trips_byte_identical`,
`no_users_plaintext_ever_touches_the_disk` (both canaries scanned against both
stores, with a vacuity check proving the canary is really in the plaintext), and
`an_insertion_at_the_start_of_a_large_file_reuses_almost_every_chunk`.

Known gap, deliberately deferred: a host can still truncate the *tail* of a
segment chain and serve an internally consistent prefix. Detecting that needs
signed, timestamped head records gossiped between peers, which is M3/M4 work.
The limitation is documented at the top of `oplog.rs` rather than papered over.

---

### M3 — Sync engine 🟨 (`itsanas-sync`)

Implemented:

- **Version vectors** with happens-before comparison and concurrency detection.
  Ordering never consults a clock: the machines disagree about the time, and the
  cost of getting the order wrong is silently discarding someone's work.
- **Log merge and replay** across devices, driven by a decision table covering
  every combination of local state and incoming claim.
- **Conflict materialisation** as sibling files. Both versions always survive.
  The winner of the original path is chosen by a deterministic total order on
  `(device, sequence)`, because a rule two devices could disagree about would
  make them overwrite each other forever instead of converging.
- **Tombstones**, so a device that was asleep during a delete does not resurrect
  the file when it returns.
- **Delete/edit races** resolved asymmetrically: a delete that demonstrably saw
  the edit is honoured, a delete merely concurrent with one loses. An unexpected
  resurrection costs a second to undo; a lost edit is unrecoverable.
- **Deferred operations**, for when a segment arrives before the chunks it names.
  Local state is left untouched and the operation is retried, rather than
  materialising a file whose content cannot be read.
- **A deterministic multi-device simulation** (`sim`) with injectable partitions
  and power cycles. Real stores, real chunking, real sealing, real signatures;
  only the network is simulated, by a `Cloud` that stands in for blind hosts.

**Exit criteria — met.** 19 convergence scenarios, including
`a_device_that_never_comes_back_still_gets_its_work_to_everyone_else`,
`the_final_state_does_not_depend_on_the_order_devices_sync_in`, and
`a_long_run_of_alternating_partitions_still_converges`. Nothing uses randomness
or wall-clock time, so a failure reproduces exactly.

One bug the tests caught while being written: a resolved conflict was re-resolved
on every subsequent sync round, so a settle loop that stops when nothing changes
would never have stopped.

**Still outstanding for M3:**

- File watching (`notify`) with debounce and a periodic full rescan. Deferred to
  M7, where the daemon that would own the watcher actually exists — until then
  there is no long-running process for it to run inside.

---

## Not started

### M4 — Network transport (`itsanas-net`)

- QUIC transport with Ed25519 device identity, hole punching, relay fallback.
- Peer protocol: head announce, segment fetch, chunk push/pull, storage
  challenge.
- Wire decoder fuzzing.

**Exit criteria:** two processes on different hosts sync a file; the decoder
survives a fuzzing campaign without a panic.

### M5 — Placement and repair (`itsanas-placement`)

- Rendezvous hashing with capacity weights; owner affinity.
- Replica target tracking; repair loop; proof-of-storage verification.
- Quota and fair-share accounting.

**Exit criteria:** removing a node from a simulated swarm moves only that node's
share of chunks; a chunk that drops below the replication floor is restored
without operator action.

### M6 — Coordinator (`itsanas-coord`)

- Account directory, presence, signed node-set epochs, escrow blob storage.
- Deployable as a container to an OVH VPS or a Freebox ARM VM.

**Exit criteria:** a new device logs in with username plus passphrase and
recovers the full account; a test proves the coordinator's stored state contains
no plaintext and no usable key material.

### M7 — Daemon and CLI (`itsanas-daemon`, `itsanas`)

- Background service; Windows service and systemd unit.
- `itsanas init | login | pledge | status | doctor`.
- Alerting on the conditions listed in
  [ARCHITECTURE.md §7](ARCHITECTURE.md#7-operational-behaviour).

**Exit criteria:** the folder-that-just-syncs experience, plus an alert that
actually fires when node count drops below the replication floor or a sync round
stops completing.

### M8 — Three-device bring-up

Windows laptop (local SSD), Raspberry Pi 4B+ (1 TB RAID1 via Freebox Delta), and
a VMware Workstation VM on dedicated external SSDs. Real NAT, real power cycles,
real network.

---

## Deferred beyond v1

| Item | Why it is deferred |
| --- | --- |
| Reed–Solomon erasure coding | Needs ≥6 independent nodes to beat replication; the interface is already shaped for it |
| Virtual drive mount (WinFsp/FUSE) | Best daily UX by a wide margin, and a large source of subtle bugs — wants a proven storage layer underneath |
| Tray / desktop GUI | The Pi needs the CLI regardless, so the CLI is the surface that must exist first |
| Fully decentralised discovery (DHT) | The coordinator is control-plane only and swappable; building both at once doubles the surface |
| Traffic padding and cover traffic | Would hide object sizes and access timing, currently accepted as visible |
