# Roadmap vs Current State

**Last updated: 2026-08-26.** This file is updated in the same change as the
code it describes. If it disagrees with the code, the code is right and this is
a bug.

## Current state at a glance

| Milestone | Crate | Status | Tests |
| --- | --- | --- | --- |
| M0 Repository, CI, licence | — | ✅ **done** | CI runs 7 jobs |
| M1 Cryptographic core | `itsanas-crypto` | ✅ **done** | 56 unit + 15 property |
| M1b Published test fixtures | `itsanas-testkit` | ✅ **done** | 7 |
| M2 Chunking and local store | `itsanas-store` | ⬜ not started | — |
| M3 Sync engine | `itsanas-sync` | ⬜ not started | — |
| M4 Network transport | `itsanas-net` | ⬜ not started | — |
| M5 Placement and repair | `itsanas-placement` | ⬜ not started | — |
| M6 Coordinator | `itsanas-coord` | ⬜ not started | — |
| M7 Daemon and CLI | `itsanas-daemon`, `itsanas` | ⬜ not started | — |
| M8 Three-device bring-up | — | ⬜ not started | — |

**Nothing in this repository should hold data you care about yet.** The
cryptographic guarantees are implemented and tested; everything that would
actually move a file between two machines is still to be written.

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

## Not started

### M2 — Chunking and local store (`itsanas-store`)

- FastCDC content-defined chunking with configurable min/avg/max.
- Content-addressed blob store, sharded by chunk-id prefix, with refcounts.
- Operation-log segments: append, seal, sign, read back.
- Local index database (`redb`) mapping paths → metadata → chunk lists.
- Mark-and-sweep garbage collection with a grace period.

**Exit criteria:** Alice's full fixture corpus can be written to a store and read
back byte-identical; her canary appears nowhere in a store belonging to Bob;
chunk boundaries are stable across an insertion at the start of a large file.

### M3 — Sync engine (`itsanas-sync`)

- Version vectors, happens-before comparison, concurrency detection.
- Log merge and replay; conflict materialisation as sibling files.
- Tombstones and delete/edit race handling.
- File watching (`notify`) with debounce, plus a periodic full rescan, because
  filesystem watchers drop events under load.

**Exit criteria:** a deterministic simulation of three devices with injectable
partitions and power cycles converges to an identical file tree in every
scenario, including the one where the writing device never comes back online.

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
