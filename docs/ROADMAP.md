# Roadmap vs Current State

**Last updated: 2026-09-01.** This file is updated in the same change as the
code it describes. If it disagrees with the code, the code is right and this is
a bug. For picking the project up cold, read [HANDOVER.md](HANDOVER.md) first.

## Current state at a glance

| Milestone | Crate | Status |
| --- | --- | --- |
| M0 Repository, CI, licence | — | ✅ **done**, and the CI has now actually run |
| M1 Cryptographic core | `itsanas-crypto` | ✅ **done** |
| M1b Published test fixtures | `itsanas-testkit` | ✅ **done** |
| M2 Chunking and local store | `itsanas-store` | ✅ **done** |
| M3 Sync engine | `itsanas-sync` | ✅ **done** |
| M4 Network transport | `itsanas-wire`, `itsanas-tls`, `itsanas-net` | ✅ **done** |
| M4b Local discovery | `itsanas-discover` | ✅ **done** |
| M5 Placement and repair | `itsanas-placement` | 🟨 **decided, not executed** |
| M6 Coordinator | `itsanas-coord`, `itsanas-coordinator` | ✅ **done** |
| M7 Daemon, CLI, synced folder | `itsanas-cli`, `itsanas-folder` | 🟨 **a folder that syncs** |
| M8 Three-device bring-up | — | ⬜ not started |
| M11 A catalogue of known-but-absent files | `itsanas-store` | ✅ **done** |
| M12 Android shell | — | ⬜ core verified, shell not written; `itsanas-policy` is now wired into `itsanas daemon`, so the phone inherits behaviour a desktop has exercised |
| M9 Measurement | `itsanas bench` | ✅ **done**, and it corrected its own conclusion |
| M10 Pack files | `itsanas-store` | ⬜ decided by M9, scheduled after M6 |
| M13 One-click install | `install/` | 🟨 five scripts; run end to end on Windows, Linux x86-64, **Linux aarch64 (the Freebox VM)** and macOS in CI — no Pi, no phone |

This table used to carry a per-milestone test count. Those numbers were a second
copy of what [TESTING.md](TESTING.md) already holds, and a second copy is what
drifts: the store row claimed 115 unit tests against a real 138, the transport
row was short by 19, and the coordinator row by 17. The counts live in one place
now, and `scripts/check-counts.py` reads that place back against the source on
every push.

**645 test functions, 3 of them `#[ignore]`d into the slow job, and 30 of
them red-team tests that pass when an attack fails.**

**Nothing here should hold data you care about yet**, but the reason has
narrowed. The cryptography, the local store, the merge rules, the transport and
the synced folder all work and are tested, and two daemons genuinely keep two
folders identical over an encrypted, mutually authenticated connection.

What is missing before this is a *network* rather than a personal sync tool:

- ~~**No coordinator server.**~~ Built. `itsanas-coordinator` serves the address
  book and the escrow locker; `itsanas coordinator`, `itsanas register` and
  `itsanas login --from` are wired. **Recovering an account from a passphrase
  alone works** — verified with the real binaries, not only in tests.
- **Placement is recorded but not acted on.** A node now knows which peers
  hold each of its chunks, records it on every sync round, and `itsanas status`
  says plainly whether the data exists anywhere else. What is missing is a
  repair loop that *chooses* peers to fix a shortfall — which matters once
  there are more peers than a node pushes to anyway.
- ~~**Nothing challenges a host on a schedule.**~~ Built. The daemon audits
  every peer it contacts on chunks **drawn at random** from what the ledger says
  that peer took, and a host that cannot prove it still holds one has that
  record withdrawn — after which the chunk counts as under-replicated and the
  same round re-uploads it. Unpredictability is the whole mechanism, and it took
  three attempts: least-recently-confirmed order was a *constant* (the sixteen
  lowest chunk ids, every round), and a plain random cursor was still guessable
  because seeking to the next record weights by the gap before it and the host
  knows every gap. The ledger is now ordered under a keyed hash the host cannot
  compute — measured, that is the difference between a host losing 10% of
  your data and passing 92% of audits, and passing 18%.

  **Watched working between two machines, 2026-09-01.** Two separate accounts,
  a Windows laptop and an aarch64 VM. The host's blob was deleted from its disk
  behind its back; the owner's next daemon round printed `FAILED 1 of 1 storage
  challenges — it is not holding what it said` and re-uploaded the chunk in that
  same round, after which rounds went back to 208 bytes of nothing-new. The
  host's store still contained neither the plaintext canary nor the file's name
  afterwards. Detection, sanction and repair, on hardware rather than in a test
  binary.

  Found while doing it: **the coordinator only finds your own devices.**
  `daemon.rs:495` looks peers up as `coordinator::peers(node, node.store.owner())`,
  so a member cannot discover *other members* to host with — Alice had to be told
  `itsanas peer add` before she could reach Bob at all. That is the same gap as
  "repair chooses no peers" below, seen from the other end, and it is the thing
  standing between this and a network rather than a set of arranged pairs.

  A host that fails three rounds in a row stops being sent new content, keeps
  receiving the log so it can still relay, and is handed one chunk a round —
  chosen by the owner, and the only thing it is then audited on. Each answered
  round pays off one failure, so coming back costs as long as falling did.
- **Tombstones are never pruned.** One small record per file ever deleted,
  kept for the life of the account. `Index::forget_tombstone` exists and cannot
  be called safely: dropping a tombstone before every device has seen the delete
  lets a device that missed it resurrect the file, and "every device" needs a
  membership list this design deliberately does not have — the same problem
  that killed signed node-set epochs. Bounded in practice by how much somebody
  deletes; unbounded in principle. The likely answer is an age threshold well
  past any plausible offline period, which is a decision nobody has taken.
- ~~**Anyone who can reach the coordinator is a member.**~~ Built.
  `--invite-only` admits a stranger only with a secret an existing member
  signed; the coordinator holds the hash, never a working code, and can refuse
  but not admit. This was the front door, and its absence meant every other
  defence in the project — audits, the reliability pause, the probation
  ladder, the keyed audit order — was aimed at an adversary with no way in.
  ECONOMICS.md §6 had named invitation as the answer and left it as a sentence.

  Not built, and the next lever: **a limit on how many people one member may
  invite.** Nothing rate-limits it, so a member can invite themselves as often
  as they like. Attribution makes that visible after the fact; it does not stop
  it.
- **Recovery from username plus passphrase is not wired.** The escrow container
  exists and is tested; `itsanas login` still requires the 24 words.
- ~~**Never run on ARM.**~~ Done, twice over. **The Freebox Delta VM has run
  it**: aarch64 Ubuntu 26.04, 2 vCPU, installed on 2026-09-01 by the `curl | sh`
  one-liner on a machine with no compiler and no Rust, built in 5m37s, then
  **the whole suite passing with none failing** — 640 tests on 2026-09-01, a
  figure that moves as tests are added — the smoke check, and a benchmark that
  saves a document in a third of the laptop's time. `Test (macos-latest)` had
  also been running the whole suite on Apple silicon since CI first ran.

  ~~**Still never run on a Raspberry Pi.**~~ **It has.** A Pi 4 Model B, Debian
  13, aarch64, on an SD card, on 2026-09-01: installed by the same one-liner on
  a machine with no Rust on it, built in about twenty minutes at two jobs, and
  the smoke check passed natively. `itsanas bench` there is in M9 below, and it
  is the answer to a question this project has carried since week one — **the Pi
  saves a note in 0.8 ms, the fastest of the three machines, and the whole
  256 MiB benchmark peaks at 7.6 MiB of memory.** The constants were chosen for
  a machine nobody had measured, and the machine turns out to be comfortable.

  **The test suite could not be run there**, and that is about the machine
  rather than about ARM: `rustc` dies with `SIGBUS` on assorted small
  dependencies, and *which* ones varies between runs, on a Pi whose `ext4`
  reported six `EFSCORRUPTED` block-bitmap errors at boot and whose last `fsck`
  was in June. Release builds succeed; debug builds — which is what `cargo test`
  is — do not. Nothing points at ITSaNAS, and nothing can be concluded about
  ITSaNAS on a Pi from it either, until that filesystem is checked. What stands
  is: it installs, it runs, and it is fast there.

  What that leaves untested everywhere: **redb on an SD card under sustained
  write**, which is the medium's real question and needs a working Pi.

- CI also cross-builds
  the workspace for aarch64 and then runs **the whole test suite** on that
  architecture under `qemu-user-static` — 637 pass, 3 `#[ignore]`d, none fail —
  followed by `scripts/smoke.sh`: an account, a 24-word phrase, a 350 KB file
  across five chunks read back byte for byte, `doctor` clean.

  That answers all three things ARM changes *at the level of instruction
  semantics*: blake3's NEON path hashed every chunk in every store test, redb's
  memory mapping wrote and reopened the index 167 times, and `ring`'s aarch64
  assembly ran a real TLS handshake over a real socket in `itsanas-tls` — which
  the smoke test alone would not have touched, since it never opens one.

  It is not the same as running on ARM, and the first version of this paragraph
  said "settles", which is too strong. `qemu-user` executes aarch64 instructions
  on this runner's x86 kernel, and reports an emulated CPU's features to any
  library that picks its code path from them. `docs/PORTING.md` §3 lists what
  that leaves open.

  **Real ARM silicon was already covered and nobody had noticed.**
  `Test (macos-latest)` runs the whole suite on Apple silicon and has since CI
  first ran — a genuinely weakly-ordered machine with genuine NEON — and
  `install/macos.sh` now runs there too, finishing with
  `PASS: ITSaNAS stored and returned a file -- native arm64`. The job is called
  "Test (macos-latest)", which is why it never read as an ARM job. What the
  emulated Linux run adds is the *Linux* half of `aarch64-unknown-linux-gnu`:
  glibc, ext4, and a kernel that pages differently.

  What emulation cannot touch is the part that made the Pi worth worrying about:
  a 4B has 1 GB of RAM and a runner has 16, and nothing here says the index
  fits. Nor has anything met an SD card. `install/linux.sh` now ends by running
  the same smoke script on the machine it just installed on, so the answer for a
  real Pi is one command away from whoever has one.

---

## Done

### M0 — Repository, CI, licence ✅

- Cargo workspace, Rust 2024 edition, MSRV pinned and enforced.
- AGPL-3.0-or-later.
- CI with nine jobs: lint (format, clippy at `-D warnings`, rustdoc, and the
  five checking scripts), tests on Linux/Windows/macOS, expensive `#[ignore]`d
  tests, **the installers actually installing** on a machine of each kind, an
  aarch64 build that then runs the whole suite under emulation, an Android
  compile check, MSRV check, dependency advisories and licence audit
  (`cargo-deny`), coverage.
- Weekly scheduled CI run so a new advisory surfaces without waiting for a push.

**It had never run.** The workflow was written in the first week and the
repository was not published until 2026-08-31, so the first execution was also
the first time any of it was tested. Six jobs passed on the first attempt,
including two that had never had any evidence behind them: the macOS test run
and the aarch64 cross-build for the Raspberry Pi. Three failed, and all three
were real:

- **clippy.** The CI toolchain was five releases ahead of the machine this was
  written on, so three of its lints had never been seen here. Six errors: four
  `chunks_exact(2)` on hex digits, now `as_chunks::<2>()`, which reads the pair
  as a `[u8; 2]` and drops two bounds checks with it; one redundant borrow; and
  one `map(...).unwrap_or(true)` in the test asserting a host cannot read its
  guest's files. That last one was worth the trip: the assertion read
  `read_file("anything")` — a name no store would ever hold — inside a loop,
  with an error counted as a pass. It now reads the paths the fixture actually
  wrote and fails on an error, and it was checked by making the host gain the
  file and watching it fail. The local toolchain is updated to match CI,
  because a gate that is weaker on the developer's machine is not a gate.
- **Android.** The job's own comment claimed the crates it checks were "pure
  Rust" and needed no NDK. `blake3` compiles C for its SIMD backends, so even
  `cargo check` needs a cross compiler. The job now resolves one from the
  runner's NDK and fails with a readable message if there is none.
- **`cargo-deny`.** `chacha20 0.10.1` had been yanked. The reason is an SSE4.1
  intrinsic in the SSE2 backend of the RNG and 64-bit-counter variants — code
  this project never calls, since it uses `XChaCha20Poly1305` only, so the
  exposure was nil. Updated to 0.10.2 regardless: `yanked = "deny"` is the
  policy precisely so the question gets asked.

The same pass found the Android step running with its line continuations eaten
and thirteen literal spaces in their place, from the day it was written — the
third time that has happened here, and the first time it was in a file
`cargo fmt` never touches. `check-messages.py` now reads commands as well as
messages, and the step is written as a YAML folded scalar so there is no
backslash left to lose.

### M13 — One-click install 🟨 (`install/`)

Five scripts: `linux.sh` (which covers the Raspberry Pi and the Freebox VM),
`macos.sh`, `windows.ps1`, `android-termux.sh`, and `coordinator.sh` for the
machine with the public address. `install/README.md` carries a table saying, for
each one, whether anybody has run it on the system it claims to install --
because an installer nobody has run is a hypothesis with a shebang.

Three have been run end to end. **Linux on aarch64**, on the Freebox VM, through
the one-liner, on a machine with nothing on it — the case the script was written
for and the first time it met one. **Windows**: built the workspace,
installed both binaries, put them on PATH, changed nothing on a second run, and
stored a 341 KiB file and read it back byte for byte, under PowerShell 5.1 rather
than 7 because 5.1 is what the script promises to support. **Linux**: twice on a
bare Ubuntu with no toolchain, and then through the `curl | sh` one-liner the
script has advertised on its fourth line since the week it was written and which
nobody had ever run.

That one line was broken three ways, and every one of them was invisible from
reading it. The URL was a placeholder. `ITSANAS_REPO` was required rather than
defaulted, so the piped path — which has no checkout — died at "nothing to
build". And under a pipe **stdin is the script**, so the first `read` would have
consumed the lines the shell had not executed yet and run a truncated program.

Running it then found two more. The systemd unit is written with an unquoted
heredoc, and a comment inside it explained the leading dash on
`ReadWritePaths` using the phrase "created by `itsanas init`" — in backticks.
The shell ran it: a usage error appeared in the middle of the install and the
unit on disk read "created by , so enabling the service". And the smoke step
read `$SOURCE_DIR`, which only `--source` sets, so the verification every
installer supposedly ends with was skipping itself on both of the usual paths
and announcing "not in this checkout" from inside a checkout.

Five defects on the primary entry point, none of them findable by reading.

The architecture refusals are exercised by faking `uname`, and the coordinator's
`--check` against a free port, a busy one, a present binary, an absent one and an
all-private address list. What none of that touches is the machines the project
is for: the Mac, the Pi, the phone and the Freebox VM are all untried, and
`install/README.md` says so in the column that exists to say it.

Every member installer now ends by proving its own work rather than by running
`itsanas --version`, which only says the kernel can execute the file.
`scripts/smoke.sh` creates an account, checks the recovery phrase is still 24
words, stores a file across several chunks, reads it back and runs `doctor`; CI
runs the same script under aarch64 emulation, so what a person sees on their own
Pi is what CI checked. Windows has no `sh`, so `windows.ps1` writes the same
steps out. The coordinator is the exception: it has no store to exercise.

What `scripts/check-installers.sh` refuses, each item because it happened:

- a script that does not parse, or that uses bash syntax while declaring POSIX
- a script missing from `install/README.md` -- the first version of the checker
  named two files by hand, so `coordinator.sh` was written and checked by
  nothing, which is the failure the checker exists to prevent, committed inside
  the checker
- a Rust version that disagrees with `Cargo.toml`, because a machine that passes
  the installer's check and then fails to compile is the worst possible order
  for those two to disagree
- a systemd `ReadWritePaths` without the leading dash that lets a unit start
  before the path exists -- the member unit listed `~/.itsanas`, which
  `itsanas init` creates, so enabling the service before initialising failed
  with an error about mount namespaces
- a 32-bit userland that `linux.sh` does not refuse. Its list was
  `armv7l|armv6l`, and a 64-bit Raspberry Pi running a 32-bit image reports
  **armv8l** -- the exact case the error message describes, waved through with
  "untested architecture" and left to fail an hour into the build. 32-bit x86
  was let through too. The checker now fakes `uname` and tries all seven
  spellings.

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

Device certificates and revocation moved to `itsanas-coord` (`claim.rs`), where
they belong: they are statements a coordinator records, not primitives. Key
wrapping to other users is deliberately unbuilt — mutual *storage* needs no key
exchange, so `UserKeys::agree` is kept, tested and unused until sharing exists.

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

### M3 — Sync engine ✅ (`itsanas-sync`)

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

### M4 — Network transport ✅ (`itsanas-wire`, `itsanas-tls`, `itsanas-net`)

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

**Encryption and authentication** (`itsanas-tls`). Every connection is TLS 1.3,
with both ends proving which device they are.

Authentication sits *above* TLS rather than in the certificate. The obvious
design puts the device's Ed25519 key in its certificate and compares it against
the expected id, which needs X.509 parsing in the trusted path to get the key
back out. Instead the certificates are anonymous and regenerated every start-up,
and each side signs the TLS session's **exporter value** with its device key. A
man in the middle who terminates TLS has two sessions with two different
exporters, so a proof from one is worthless in the other and they cannot make
their own. Two things fall out: no certificate parser anywhere, and an observer
cannot correlate two connections by their certificates.

`ring` rather than the default `aws-lc-rs` provider: aws-lc-rs needs cmake and a
full C toolchain, which makes cross-compiling for the Pi markedly harder.

This removed `Exposure` and the CLI's `--allow-public`. Both existed only
because the transport leaked chunk identifiers, sizes and timing; keeping them
would be cargo cult. The default listen address is now `0.0.0.0:9797`, because a
node in a network has to be reachable.

**Exit criteria — met.** Two processes sync a real file over a real socket; a
host stores a stranger's data and cannot read a byte of it; a host relays one
device to another it never met; concurrent edits converge over the wire; a
recording of everything written to the socket contains no plaintext; dialling a
device and reaching a different one is refused.

**Still outstanding for M4:**

- **A real fuzzing campaign** (`cargo-fuzz`) against the decoder. It is currently
  exercised against every truncation and every single-bit corruption of a valid
  frame, and against arbitrary garbage — a hand-written adversarial suite, which
  covers the inputs somebody thought of.
- **NAT traversal.** A node behind NAT can push but cannot be dialled. Partly
  mitigated already: `session::drain_vault` means a node that only ever accepts
  connections still learns what was pushed to it. Hole punching and relay
  fallback would want QUIC, which is now an optimisation rather than a
  prerequisite for security.

---

### M4b — Local discovery ✅ (`itsanas-discover`)

The first piece of the decentralisation decision recorded in
[DESIGN.md](DESIGN.md) §8: the job the coordinator does *not* need to do.

A node broadcasts a **fixed 147-byte announcement**, signed by its device key,
on UDP 21037. Because a `DeviceId` is the Ed25519 verifying key, a receiver
checks the signature with no prior contact and no key distribution, so nobody
can advertise a device they do not hold.

Deliberate properties, each with a test:

- **The address is not in the packet.** Only the port is; the address comes from
  the UDP source, so a node cannot advertise a different machine.
- **The sender's clock decides nothing.** A Raspberry Pi 4 has no RTC and
  announces itself believing it is 1970. Superseding by sender clock would make
  a rebooted Pi unreachable until NTP ran.
- **The table is bounded and known peers are protected.** Device ids are free
  keypairs, so a flood is cheap; a flood can deny discovery of *new* peers and
  cannot evict one that has already answered.
- **The owner field is a hint, never an authorisation.** Binding a device to a
  user needs an owner-signed claim, which a bare LAN cannot supply. It orders
  the dial list and nothing else.

Verified by running it, not only by tests: a daemon on this machine reported
`found another user's device d56e4ff7dca9 at 192.168.19.1:9797` from a real
IPv4 broadcast sent by `cargo run -p itsanas-discover --example probe`.

**One bug found by running rather than testing.** `Lan::bind(0)` used the same
number for the local port and the broadcast target, so an ephemeral bind sent
every announcement to `255.255.255.255:0` — accepted by the operating system,
delivered to nobody, reported as five successful sends. Now
`announcing_to_port_zero_is_refused_rather_than_sent_into_the_void`.

**Not covered:** IPv6 multicast, and interface selection — IPv4 global broadcast
leaves by the default route only, which is right for a house and wrong for a
machine with several networks.

---

### M5a — The placement ledger ✅ (`itsanas-store`)

The coordinator's hardest job, removed rather than implemented.

Placement was going to come from a coordinator-published **signed node-set
epoch**, so every peer computed identical rendezvous placement "with no
agreement protocol". That phrasing hid the problem: requiring every peer to hold
the same membership list *is* an agreement protocol. It was consensus by decree.

It is also unnecessary. A global content store must answer "who holds this
block?" for an arbitrary asker; ITSaNAS never asks that. Every chunk has exactly
one owner, who already keeps an operation log of it — so the owner can simply
record where they put it.

- `HOLDERS` table, keyed `chunk_id || device_id`, so every holder of one chunk
  is a contiguous range.
- Filled by `session::push`, from information the round already had: whatever a
  peer did *not* ask for, it already holds. The ledger therefore **converges on
  every sync** rather than only recording new uploads, which is what lets a
  restored device learn where its data lives by asking instead of re-uploading.
- `under_replicated(target)` counts this device, so a target of three asks for
  two elsewhere. That convention is pinned by a test, because off-by-one here is
  invisible until two machines die instead of three.
- `itsanas status` answers the question a backup tool exists for:

```text
is it anywhere else?
  NO             1 of 1 chunks exist only on this machine
  below target   1 chunks are on fewer than 3 machines
                 run `itsanas sync`, or add a peer, to spread it
  placements     0 recorded
```

Verified by running two real nodes: before a sync the answer is `NO`, after one
it is `partly`, and a second round sends nothing while the ledger stays intact.

**Cancelled as a result:** signed node-set epochs. Nothing needs them.

---

### M5b — Placement and repair 🟨 (`itsanas-placement`)

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

- ~~**Getting back what this disk lost.**~~ Built. `session::repair` asks a
  peer for chunks whose bytes are gone from local storage — a dropped block, an
  inode lost to a power cut, a partial restore. That is the failure the
  placement ledger exists to survive, and the one `push` cannot reach: push
  offers a peer what the peer lacks and can put nothing back here.

  Three rules make it safe rather than merely useful:

  - **A peer is asked only about chunks the ledger records it as holding.** A
    repair request says "I no longer have this", and while the ids are blinded,
    which chunks exist only on hosts is exactly the list to delete to destroy
    somebody's data.
  - **Every byte is opened and re-addressed before it is written.** A host
    cannot read what it stores, so answering with noise is its one route to
    destroying data: written unverified it would set `has_chunk`, stop the scan
    looking, and make a recoverable loss permanent.
  - **A reply longer than the chunker can emit is refused on its length**,
    before decryption, so a peer cannot make this node decrypt a quarter of a
    gigabyte a round for nothing.

  Detection has two speeds and one queue. `doctor` finds every loss in a single
  pass — that is a person asking because a file would not open, and it is the
  fastest signal this system has. The daemon also samples 2048 live chunks a
  round, which reaches a given chunk in nine hours on a ten-gigabyte store and
  in **fifty-five days** on a terabyte. Both write to the same queue and repair
  drains it before it samples anything.
- **Executing the plan.** The planner is wired to nothing. It needs a loop that
  builds a census by asking peers, runs the plan, and pushes — which needs the
  daemon (M7) to have somewhere to live and the coordinator (M6) to supply an
  agreed node set.
- **Scheduled proof-of-storage.** The challenge primitive works and is tested
  over the wire; nothing issues challenges periodically or records the results,
  so a host that quietly discards data is currently detected only by accident.
- Fair-share accounting beyond the single pledged-bytes ceiling.

---

### M7 — Daemon, CLI and synced folder 🟨 (`itsanas-cli`, `itsanas-folder`)

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

The **synced folder** (`itsanas-folder`) is the thing that makes this a product
rather than a set of commands. Point it at a directory with `itsanas folder
<path>`, and files put in it are uploaded, files deleted from it are deleted
everywhere, and changes from other devices appear in it.

Verified by running two daemons: a file dropped into one folder appeared in the
other, an edit propagated, a file created on the *other* side came back, and a
deletion removed it from both. Both folders ended byte-identical.

- **The ledger.** A file missing from disk means either "the user deleted it" or
  "this device never downloaded it", and the filesystem cannot tell them apart.
  `LocalState` records what this device last put on disk, and a delete is only
  ever acted on for a path that record covers. Without it, a brand-new device
  would announce the deletion of every file its owner has on its first pass —
  and every other device would obey.
- **File watching** (`notify`) with debounce, plus a periodic rescan, because
  every platform's watcher drops events under load and none report changes made
  while the process was stopped. A slower **deep** rescan re-hashes everything
  hourly, catching a file rewritten within the same second at the same length,
  which size-and-mtime comparison cannot see.
- **Conflicts keep both.** A local edit colliding with an incoming one moves the
  local version aside rather than overwriting it.
- **Symlinks are skipped, never followed** — a link inside the folder pointing
  at `~/.ssh` would otherwise quietly upload a private key.
- **Atomic writes.** A torn file in a synced folder is worse than a missing one:
  the next scan would hash the partial content and replicate the truncation.

Two bugs found by running it rather than by reasoning about it, both now with
regression tests:

1. The reconciler wrote to the store but never called `flush_segment`, so
   imports were never sealed into a log segment. Files looked perfectly synced
   on the machine that had them and existed nowhere else.
2. A push lands in the receiving node's *vault*, and only `pull` applies
   segments to the store — so a node that never dialled anybody held its own
   data and never looked at it. `session::drain_vault` fixes it. Not a corner
   case: a device behind NAT can push and cannot be dialled.

Still to build:

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

---

### M9 — Measurement, and what it found ✅ then ❌ (`itsanas bench`)

`itsanas bench` ships as a command rather than living in `cargo bench`, because
the question was never "is the laptop fast enough". It generates incompressible
data on the fly, pushes it through chunking, sealing, a real store write and a
real store read, checks the round trip by hash, and prints how long a full disk
would take.

It was run once and immediately found two things, which is what it was for.

#### Finding 1 — the write path is dominated by per-chunk file operations

Windows laptop, 256 MiB, 3585 chunks. Sealing alone runs at 229 MiB/s, so
cryptography is not the bottleneck by a factor of twelve:

| Configuration | Write | Extrapolated to 1 TB |
| --- | --- | --- |
| `fsync` + staging file + rename (**what ships**) | 19.0 MiB/s | 15.3 hours |
| `fsync`, written straight to its final path | 27.9 MiB/s | 10.4 hours |
| staging + rename, no `fsync` | 37.0 MiB/s | 7.9 hours |
| straight to final path, no `fsync` | 66.9 MiB/s | 4.4 hours |

Both costs are per **chunk**, and a chunk averages 73 KiB.

**Nothing was changed on the strength of this.** Each variant was measured and
then reverted: `blob.rs` is the write path of a backup system, and a 47 % gain is
not a reason to alter its durability argument at the end of a long session.
Dropping the staging file needs the existence check to become size-aware, or a
half-written blob is silently accepted as complete and never rewritten.

#### Finding 2 — the layout does not reach a terabyte, and that is the real one

At a 73 KiB average, **1 TB is 14.7 million files**. Two consequences, neither of
which any amount of tuning fixes:

- `session::push` calls `blobs().addresses()` **on every sync round**, which
  walks the whole tree and returns every identifier in one `Vec` — about 470 MB
  of resident memory at that scale, on a machine chosen partly because it is
  cheap. This directly contradicts the bounded-memory property the store claims
  everywhere else.
- 14.7 million inodes on the Pi's array, every one of them touched by a full
  verification pass.


#### Finding 3 — the correction: saving a document *is* instant

The first two findings were measured with the wrong question. Throughput on
256 MiB answers "how long does the archive take"; nobody waits for the archive.
What a person waits for is a save. Nicolas put it exactly: *if copying a film
takes two hours I do not care; if you cannot save a Word document on the fly,
that is serious.*

So `itsanas bench` now measures that too — repeated saves of realistic sizes,
each to a fresh path so deduplication cannot answer for free, timed from the
caller handing over bytes to the change being sealed into a log segment peers
can pull. Same laptop:

| What | Size | Typical | p95 | Worst |
| --- | --- | --- | --- | --- |
| a note | 4 KiB | 6.6 ms | 7.8 ms | 11 ms |
| a spreadsheet | 64 KiB | 7.8 ms | 11 ms | 12 ms |
| a Word document | 512 KiB | 28 ms | 32 ms | 34 ms |
| a big PDF | 4 MiB | 167 ms | 187 ms | 204 ms |
| a photo burst | 32 MiB | 1.45 s | 1.56 s | 1.56 s |

**Everything a person saves by hand is under 200 ms, and most of it is under
30 ms.** The per-chunk `fsync` that costs a factor of two on a 256 MiB archive
costs four milliseconds on a spreadsheet, which is nothing.

### The same benchmark on ARM, and the surprise in it

Run again on 2026-09-01 on the Freebox Delta VM — aarch64 Ubuntu 26.04, 2 vCPU,
11 GB — and on the laptop, from the same commit and the same release profile, so
the two columns are comparable:

> **The Pi column was measured on a machine that failed within the hour.** Its
> root filesystem began returning `EUCLEAN` shortly afterwards; `docs/PORTING.md`
> §3 has the detail. The figures agree with the VM's, which is why they are kept,
> and they need repeating on a Pi with a sound card before anything is built on
> them.

| | laptop, x86-64 | VM, aarch64 2 vCPU | **Pi 4B, aarch64, SD card** |
| --- | --- | --- | --- |
| chunking | 848.5 MiB/s | 185.0 MiB/s | 142.5 MiB/s |
| **store write** | **27.2 MiB/s** | **54.1 MiB/s** | **44.1 MiB/s** |
| store read | 436.6 MiB/s | 96.0 MiB/s | 74.8 MiB/s |
| a note, 4 KiB | 9.2 ms | 1.1 ms | **0.8 ms** |
| a Word document, 512 KiB | 29 ms | 10 ms | **12 ms** |
| a big PDF, 4 MiB | 159 ms | 75 ms | **93 ms** |
| a photo burst, 32 MiB | 1.29 s | 617 ms | **731 ms** |
| 1 TB, extrapolated | 10.7 h | 5.4 h | 6.6 h |
| peak memory | — | 9.2 MiB | **7.6 MiB** |

The small machines win on the number that matters and lose on everything else.
The laptop chunks six times faster than the Pi and reads six times faster, and
still takes 29 ms to save a document where the Pi takes 12. Writing is what a
save is made of.

**The Pi is comfortable, which was the open question.** Every constant in this
repository that says "on a Raspberry Pi" was chosen against a machine nobody had
measured. Saving a note takes 0.8 ms there — the fastest of the three — and the
whole 256 MiB benchmark peaks at 7.6 MiB of memory, on a machine with 3.8 GB.
The worry was whether a Pi could keep up. It is not close.

The store puts **one file per chunk**, so a 256 MiB archive is 3585 file
creations. That is the operation NTFS is expensive at and ext4 is not, and it is
the only stage where the ordering flips — if this were CPU, read and write would
move together, and they move opposite ways. Real-time virus scanning on every
newly created file is the obvious additional suspect on Windows and has not been
isolated here, so it stays a suspect.

Two consequences worth writing down. **Pack files (M10) are a Windows problem
before they are a scale problem** — the same change buys roughly twice as much
there. And the figure this project has quoted since M9, "a document saves in
28 ms", was measured on the slower of the two platforms without ever saying
which, which is how a number becomes a fact by repetition.

That reorders the plan. Pack files remain the right answer to 14.7 million
files, and they are an **archive** problem — the first terabyte, and the
`blobs().addresses()` walk on every sync round — not a "cannot use it" problem.
The daily experience is already good, and the first two findings were quietly
scoped as though it were not.

**Decision: pack files, and not urgently.** Chunks append into large segment files, with an index
in redb mapping chunk to (pack, offset, length). One `fsync` per closed pack
rather than per chunk; the have/missing exchange reads the index instead of the
filesystem; garbage collection becomes compaction. This is what git packfiles,
restic, Borg and casync all converged on, for these reasons.

It is not the next piece of work. Finding 3 showed the daily experience is
already fine, so packs are scheduled for when the archive matters: before the
Pi's 1 TB array is filled, and before `blobs().addresses()` walks a tree that
large on every round. The coordinator comes first, because escrow recovery is an
acceptance test and this is not.

---

### M6 — Coordinator 🟨 (`itsanas-coord`)

**This is where the next session should start.** The library is complete and
tested; nothing serves it, so no member can yet learn that another exists.

Implemented:

- **Device certificates** (`claim.rs`) — the piece M1 listed as missing. Two
  signatures because they change at different rates: a *claim* ("this device is
  mine, it pledges this much") signed by the master key and changing rarely, and
  a *presence* ("reachable here, now") signed by the device key and changing
  constantly. One message carrying both would put the key that can revoke every
  device into use every few minutes.

  Revocation falls out of it: claims are signed by the user's key, not the
  device's, so whoever holds a stolen laptop cannot un-revoke it or move it. A
  one-hour clock-skew ceiling stops a claim dated in the future being
  unreplaceable — including by its owner trying to revoke it.

- **Accounting** (`accounting.rs`) — [ECONOMICS.md](ECONOMICS.md) made
  executable, in integers for the same reason placement is. A quarter-uptime
  laptop earns a quarter of what the same disk earns always-on. Availability has
  a floor so a holiday is not a default and a ceiling so a dishonest coordinator
  cannot mint entitlement. Only the harshest state permits reclaiming, and even
  then a member's data survives on their own machines.

- **Directory** (`directory.rs`) — accounts, claims, presence, escrow, usage.
  Availability is *measured* (did I hear from you since the last tick, folded
  into a weighted average), not asserted, so a node cannot inflate its uptime by
  saying so; a single heartbeat buys only the floor. A coordinator that was
  itself offline for a year does not come back and annihilate everyone's
  standing. Usernames are lowercase ASCII only, because a directory is read
  aloud and typed back in. Escrow is opt-in and off by default.

**Still outstanding for M6, in order:**

1. **A protocol and a server.** Request/response enum, a `service.rs` handling
   requests against `Directory`, and a TLS server reusing `itsanas-tls` and
   `itsanas_wire::Connection`. Then an `itsanas-coordinator` binary.
2. **Signed node-set epochs.** Does not exist yet. The coordinator signs, peers
   pin, `itsanas-placement` consumes. This is what makes placement usable.
3. **CLI wiring**: `itsanas register`, a coordinator address in the config, peer
   lookup by username, and passing the discovered device id as `PeerClient`'s
   `expect` argument — currently always `None`.
4. **Escrow recovery**: `itsanas login --username X` fetching the blob. The
   container works and is tested; only the wiring is missing. This is the
   recovery story originally asked for.

**Exit criteria:** a new device logs in with username plus passphrase and
recovers the full account; a test proves the coordinator's stored state contains
no plaintext and no usable key material.

---

### M11 — Knowing about a file you have not downloaded ✅ (`itsanas-store`)

The behaviour everyone expects from a phone: everything listed, tap one to
download it. `catalogue.rs`.

A metadata round keeps the signed log segments without downloading content, so
every operation comes back deferred — nothing half-written. But deferred means
no index entry, so `Store::list` reported nothing and a client on a metered
connection could show an empty screen for an account full of files.

**Derived from the vault, not recorded.** A table updated as segments arrive
would read faster and would drift, and the day the two disagreed the one the
user sees is the wrong one. The walk is read-only, cannot be stale, and has no
repair path to write because there is nothing to repair.

Deliberately *not* done: writing an index entry for an absent file. Faster
still, and it breaks the invariant the rest of the store leans on — a listed
file is a readable file, which the conflict and delete logic both assume.

The listing applies the same delete-versus-edit asymmetry as the merge engine,
because a listing that hid a file the engine is about to keep would tell
somebody their edit was lost.

`itsanas ls` now shows it, and `itsanas sync --metadata-only` reaches the mode
from the command line — a laptop tethered to a phone wants it as much as a phone
does:

```text
         8 B  not here  notes/todo.txt
   292.9 KiB  not here  photos/holiday.jpg

2 file(s) are known but not downloaded. `itsanas sync` fetches them.
```

Verified by running it: two real devices of one account, a metadata round
reporting two deferred operations and downloading nothing, the listing above,
then a full round after which both files are local and the content matches by
SHA-256.

**Cost:** O(history) of segment decoding per call, no network. The same walk
`pull` already does on every content round. It joins the queue behind pack
files rather than being a new class of problem.

---

## Not started

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
