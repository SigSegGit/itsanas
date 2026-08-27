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
| M2 Chunking and local store | `itsanas-store` | ✅ **done** | 88 unit + 27 integration + 1 doc |
| M3 Sync engine | `itsanas-sync` | 🟨 **mostly done** | 12 unit + 19 convergence + 1 doc |
| M4 Network transport | `itsanas-net` | 🟨 **works over TCP; QUIC pending** | 38 unit + 11 two-node |
| M5 Placement and repair | `itsanas-placement` | 🟨 **decided, not yet executed** | 29 unit + vault's 16 |
| M6 Coordinator | `itsanas-coord` | ⬜ not started | — |
| M7 Daemon and CLI | `itsanas-cli` | 🟨 **CLI and daemon work; no file watching** | 23 unit |
| M8 Three-device bring-up | — | ⬜ not started | — |

**Nothing in this repository should hold data you care about yet.** The
cryptographic guarantees, the local store, the merge rules and the peer protocol
are implemented and tested. Two processes now sync a real file over a real
socket, a node hosts another user's data without being able to read it, and a
host relays one device's work to another device it has never met.

There is a working command line: `itsanas init | put | get | sync | serve |
doctor`, walked through in [QUICKSTART.md](QUICKSTART.md).

What is still missing before this is safe to rely on:

- **The transport is plain TCP.** It protects your *data* — everything on the
  wire is sealed — but exposes chunk identifiers, sizes and timing to anyone on
  the network path. It refuses to bind a non-loopback address unless you
  override it. Use a VPN or an SSH tunnel between machines.
- **Nothing runs on its own.** There is no daemon, so `itsanas sync` is a thing
  you run or a thing cron runs, and only one process may hold a node at a time.
- **Placement is decided but not executed.** The rules for which hosts should
  hold a chunk, and the repair plan for one that has too few copies, are
  implemented and tested — but nothing yet carries that plan out.
- **Nothing challenges a host on a schedule.** The proof-of-storage primitive
  works over the wire; a host that quietly discards data is currently caught
  only by accident.
- **There is no coordinator**, so peers must be pointed at each other by hand.

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

### M4 — Network transport 🟨 (`itsanas-net`)

Implemented:

- **Framing** with a fixed header, an explicit length and a hard ceiling.
  Every byte comes from a stranger's computer, so the decoder is deliberately
  boring: no recursion, and no allocation sized by a number the peer chose until
  that number has been checked.
- **Peer protocol**: hello with version negotiation, head announce, segment
  fetch with resume, chunk fetch, batched *which of these do you lack*, chunk
  and segment push, and storage challenges.
- **A service layer** that answers requests from a store and a vault, with no
  sockets in it, so every rule about what a peer may obtain is tested directly.
- **A vault** (`itsanas_store::Vault`) holding other users' sealed objects. It
  takes no keys in any constructor, so there is no code path from "a peer asked
  me something" to "I decrypted something of theirs".
- **A TCP transport** with timeouts, which refuses to bind a non-loopback
  address unless explicitly overridden.
- **Sessions**: push and pull halves that compose into a full sync round.
  Fetched segments are retained in the vault, which is what lets a node relay
  one device's work to another and gives the next pull a free resume point.

**Exit criterion — met for the protocol, partly for the transport.** Two
processes sync a real file over a real socket; a host stores a stranger's data
and cannot read a byte of it; a host relays one device to another that it never
met; concurrent edits converge over the wire. The decoder is exercised against
every truncation and every single-bit corruption of a valid frame, and against
arbitrary garbage — but that is a hand-written adversarial suite, not the fuzzing
campaign the original criterion asked for.

**Still outstanding for M4:**

- **QUIC with TLS and device-key authentication.** The current transport is
  plain TCP. Data confidentiality does not depend on it — everything on the wire
  is already sealed, and segment envelopes are signed — but a passive observer
  sees chunk identifiers, sizes and timing, which the threat model grants to a
  *host* and not to an arbitrary network. `PeerServer::bind` refuses non-loopback
  addresses by default because of this. Until QUIC lands, run over loopback, a
  VPN, or an SSH tunnel.
- NAT hole punching and relay fallback, which QUIC is a prerequisite for.
- A real fuzzing campaign (`cargo-fuzz`) against the decoder.

---

### M5 — Placement and repair 🟨 (`itsanas-placement`)

The **hosting** half arrived early, because M4's protocol needed somewhere to put
other people's data: `Vault` stores and serves foreign sealed objects, verifies
segment signatures before accepting them, refuses a segment that does not
continue the chain it already holds, and enforces a pledged-capacity limit.

Implemented:

- **Rendezvous hashing with capacity weights**, and **no floating point**.
  `DESIGN.md` originally specified `weight / -ln(uniform_hash(…))`, which is
  correct mathematics and a latent bug: `f64::ln` is libm-dependent, two
  platforms can differ in the last ulp, and when two candidates land that close
  the Pi and the laptop disagree about where a chunk lives — silently, with no
  error and no way to notice except by losing data. The weighting is done with
  integer slots instead: a node's score is the highest hash across its slots,
  and the chance of holding the swarm's highest hash is exactly its share of the
  slots. Same proportionality, computed identically everywhere.
- **A slot cap**, so one enormous pledge cannot concentrate the network's data
  on the single machine most worth attacking.
- **Owner affinity**: a user's own devices always appear in their own replica
  sets, so a user whose peers have all left can still read their own files.
- **Repair planning**: which chunks are short of the replication floor, what to
  send where, and what is still at risk afterwards. Pure — no sockets, no store,
  no clock — so the cases that matter (nothing holds this chunk; the swarm is
  too small to meet its own floor; a holder has left) are ordinary unit tests.
- **Repair never plans a deletion.** An over-replicated chunk is wasted space; a
  wrongly deleted one is gone. Reclaiming excess needs certainty the other
  copies exist, which needs scheduled storage challenges.
- **An offline node is not a reason to move data.** Placement is computed over
  the whole swarm and only the *sending* is restricted to what is reachable, so
  the network does not churn every time somebody shuts a laptop.

**Exit criteria — met for the decision, not the execution.** Removing a node
from a 20-node swarm disturbs only chunks that node held, and *zero* chunks move
between two surviving nodes; distribution tracks pledged capacity within 20%
across a 1:8 capacity spread. Repair correctly plans the restoration of a chunk
below the floor — but nothing runs that plan yet.

**Still outstanding for M5:**

- **Executing the plan.** The planner is wired to nothing. It needs a loop that
  builds a census by asking peers, runs the plan, and pushes — which needs the
  daemon (M7) to have somewhere to live and the coordinator (M6) to supply an
  agreed node set.
- **Scheduled proof-of-storage.** The challenge primitive works and is tested
  over the wire; nothing issues challenges periodically or records the results,
  so a host that quietly discards data is currently detected only by accident.
- Fair-share accounting beyond the single pledged-bytes ceiling.

---

### M7 — Daemon and CLI 🟨 (`itsanas-cli`, `itsanas-daemon`)

The **CLI** landed early, because without it none of the layers below could be
exercised by a human. See [QUICKSTART.md](QUICKSTART.md) for a walkthrough whose
output was copied from a real session.

`itsanas init | login | whoami | status | ls | put | get | rm | pledge | peer |
serve | sync | doctor | gc`

- Identity created or restored from a 24-word phrase, sealed under a passphrase
  at production Argon2id cost, with the device seed sealed alongside it.
- The recovery phrase is shown once and never written to disk — enforced by a
  test that scans the whole node directory for it.
- Published test identities are refused as real accounts, with an explanation.
- `serve` refuses a non-loopback address unless explicitly overridden.
- `pledge` warns rather than silently dropping data when lowered below what is
  already stored.

The **daemon** (`itsanas daemon`) serves peers and syncs on a timer in one
process. That it is one process is not a convenience: the index is held under an
exclusive lock, so `serve` and `sync` cannot run simultaneously against the same
node and two cron entries would fight. It also means the passphrase is entered
once rather than paying a full Argon2id derivation on every scheduled sync.

Verified: two daemons on one machine, a file written before either started, and
the second node had it without anyone running `sync`.

- An unreachable peer is logged, not fatal — the whole design is built around
  machines that are usually off.
- A quiet round prints nothing. Saying "nothing happened" every five minutes
  fills a journal with noise and trains the operator to ignore it.
- The listener stopping does not take sync down with it: a node that cannot
  accept connections can still push to its peers.

Still to build:

- **File watching** (`notify`) with debounce and a periodic rescan — carried
  over from M3. Files currently enter the store through `itsanas put`, not by
  appearing in a folder, so this is not yet the folder-that-just-syncs
  experience.
- **Repair execution.** The daemon is the loop M5's planner needs, but building
  a census means asking every peer what it holds, and knowing who "every peer"
  is needs M6's node set.
- **Scheduled storage challenges**, for the same reason.
- Windows service and systemd unit definitions.
- Alerting on the conditions in
  [ARCHITECTURE.md §7](ARCHITECTURE.md#7-operational-behaviour). None of them
  are wired to anything yet.
- While the daemon runs, other commands against the same home still refuse to
  start. The daemon covers the common case; a local control socket would be the
  real fix.

**Exit criteria:** the folder-that-just-syncs experience, plus an alert that
actually fires when node count drops below the replication floor or a sync round
stops completing.

## Not started

### M6 — Coordinator (`itsanas-coord`)

- Account directory, presence, signed node-set epochs, escrow blob storage.
- Deployable as a container to an OVH VPS or a Freebox ARM VM.

**Exit criteria:** a new device logs in with username plus passphrase and
recovers the full account; a test proves the coordinator's stored state contains
no plaintext and no usable key material.

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
