# ITSaNAS

**A peer-to-peer cloud where people trade disk space, and hosting someone's data
gives you no way to read it.**

You pledge some of your disk and bandwidth. Other members do the same. Your
files live encrypted on their machines, theirs live encrypted on yours, and
nobody can open anybody else's. Devices come and go — laptops sleep, a Pi
reboots — and your data stays available and stays in sync.

> ## ⚠️ Under construction. Do not put data you care about in this.
>
> This is a working prototype being built in the open, not a product. It has
> never run on the fleet it is designed for, no version has been released, and
> the on-disk format may change without a migration. **Treat anything you store
> with it as already lost.**
>
> **What runs today.** Two machines keep a folder in sync over an encrypted,
> mutually authenticated connection: drop a file in, it appears on the other;
> delete it, it goes from both. Machines on one network find each other with
> nothing configured; machines elsewhere find each other through a coordinator
> that holds no keys and no file data. An account is recovered on a fresh
> machine from a passphrase alone — **and the files come back**, verified end
> to end with the real binaries. Hosts are audited on chunks drawn in an order
> they cannot predict, and a peer that cannot answer stops being sent data. A
> disk that quietly loses a block gets it back from a peer that still has it —
> **watched happening between two machines**: a host's blob was deleted from its
> disk behind its back, and the owner's next round printed
> `FAILED 1 of 1 storage challenges` and re-uploaded the chunk in the same round.
> Joining is by invitation from an existing member, also tested between two
> machines through a real coordinator.
>
> 641 tests, 30 of them red-team — a red-team test **passes when the attack
> fails**. See [docs/TESTING.md](docs/TESTING.md), which lists every one of them
> with the property it establishes.
>
> **Saving is fast, and faster on the small machine.** A 512 KiB document is
> stored, sealed and announced in **29 ms on an x86-64 laptop and 10 ms on a
> 2-vCPU aarch64 VM** — same binary, same commit, measured by `itsanas bench`.
> The laptop chunks 4.6× faster and reads 4.5× faster, and still loses, because
> what a save costs is dominated by writing one file per chunk and that is where
> NTFS is expensive. It is also the clearest argument for the pack files that
> are planned: filling a terabyte means 14.7 million of those files.
>
> **What is missing, and it is not small.** It has never run on four real
> machines for a week. It **has** now run on real ARM: the whole suite passes on
> Apple silicon in CI on every push, and on an aarch64 Ubuntu VM on a Freebox
> Delta — every test, none failing, on 2026-09-01, installed there by its own
> one-liner, and on a **Raspberry Pi 4B on an SD card** — where it installed,
> stored and returned a file, and turned out to save a note faster than the
> laptop. That Pi's filesystem failed an hour later for reasons of its own, so
> its numbers want repeating on a sound card.
> Repair chooses no peers. Tombstones are never pruned. There is no Android app,
> only a Termux script that builds the command line tool. See
> [docs/ROADMAP.md](docs/ROADMAP.md) for the list and
> [docs/MVP.md](docs/MVP.md) for what would make it worth trusting.

## The idea in one picture

```
        Alice's laptop                 Bob's Raspberry Pi           Carol's VM
   ┌──────────────────────┐        ┌──────────────────────┐   ┌──────────────────────┐
   │ Alice's files  (read)│        │ Bob's files    (read)│   │ Carol's files  (read)│
   │ Bob's chunks  (blind)│  ⇄     │ Alice's chunks(blind)│ ⇄ │ Alice's chunks(blind)│
   │ Carol's chunks(blind)│        │ Carol's chunks(blind)│   │ Bob's chunks  (blind)│
   └──────────────────────┘        └──────────────────────┘   └──────────────────────┘
```

Everyone holds everyone's data. Everyone can only read their own. "Blind" is
literal: a host sees opaque blobs with opaque names, and cannot tell what a
chunk contains, how large the original file was, what it is called, or even
whether two of its users are storing the same file.

It does know **whose** data it holds — the blobs are filed under the owner's
public identifier — because the accounting is bilateral and you cannot charge
for space without knowing who is using it. Verified on two machines rather than
asserted: a document with a canary string written on a Windows laptop, pushed to
a separate account on an ARM VM, whose whole store then contains neither the
canary nor the file's name.

## Design goals

- **Zero-knowledge hosting.** A host is treated as actively malicious. It may
  read everything it stores, keep it forever, return corrupted or stale bytes,
  serve one chunk when asked for another, and collude with other hosts. None of
  that yields plaintext, and all of it is detected.
- **Survives churn.** Most people do not leave machines on. Data must remain
  readable and writable when an arbitrary subset of the swarm is offline.
- **Recoverable from nothing.** A 24-word recovery phrase reconstructs your
  entire identity on a brand-new machine, and your data is pulled back from
  whichever peers are up.
- **Fair.** Pledge three times what you store, weighted by how reliably your
  machines are actually reachable. The rules are in [docs/ECONOMICS.md](docs/ECONOMICS.md).
- **Small trusted surface.** Machines on one network need no server at all. A
  coordinator is optional, holds no data, no keys and no plaintext, and carries
  nothing that would be lost if it vanished — the reasoning is in
  [docs/DESIGN.md](docs/DESIGN.md) §8.

## How live sync works when the peer is asleep

This is the part that makes ITSaNAS different from both a backup tool and from
Syncthing.

Each of your devices keeps an **append-only operation log**: `Put{path, meta,
chunks}`, `Delete{path}`, each stamped with a device id and a Lamport clock.
Those log entries are batched into small segments, encrypted, signed, and
replicated to blind hosts exactly like file data. Hosts can order segments by
their signed sequence numbers without being able to read a single field.

So when your Raspberry Pi writes a file and then powers down, its log segments
are already sitting on other people's machines. Your laptop wakes up hours
later, pulls those segments from whoever happens to be online, replays them, and
converges — without the Pi ever coming back. Concurrent edits are detected by
version vector and materialised side by side as
`report.conflict-<device>-<sequence>.pdf`; nothing is silently overwritten and
nothing is lost.

## Repository layout

| Crate | What it does | Status |
| --- | --- | --- |
| `itsanas-crypto` | Identity, key schedule, sealing, blinded addressing, keystore | **implemented** |
| `itsanas-testkit` | The three published test users, their generated corpus and canaries | **implemented** |
| `itsanas-store` | Content-defined chunking, blob store, operation log, local index | **implemented** |
| `itsanas-sync` | Version vectors, log merge, conflict materialisation, convergence simulation | **implemented** |
| `itsanas-wire` | Length-prefixed framing and a stream-agnostic connection | **implemented** |
| `itsanas-discover` | Serverless discovery: signed announcements on the local network | **implemented** |
| `itsanas-tls` | TLS 1.3 with device authentication bound to the channel, no certificate authority | **implemented** |
| `itsanas-net` | Peer protocol, encrypted transport, sync sessions, proof-of-storage challenges | **implemented** |
| `itsanas-placement` | Rendezvous hashing, replication targets, repair planning | **implemented** (execution pending) |
| `itsanas-coord` | Device claims and revocation, measured availability, accounting, directory, coordinator protocol and server | **implemented** |
| `itsanas-coordinator` | The coordinator binary: address book and escrow locker | **implemented** |
| `itsanas-policy` | When and how much to sync: metered connections, battery, attention | **implemented** |
| `itsanas-folder` | A real directory mirrored into the store and back: import, export, delete, file watching | **implemented** |
| `itsanas-cli` (`itsanas`) | Command line and daemon: init, login, folder, put, get, sync, serve, daemon, doctor | **implemented** |

## Documentation

| Document | What is in it |
| --- | --- |
| [docs/HANDOVER.md](docs/HANDOVER.md) | **Start here to pick the project up cold** — state, decisions that must not be reversed, what is next |
| [docs/MVP.md](docs/MVP.md) | **What has to be true before this is worth using**, as tests you run on your own machines |
| [docs/ECONOMICS.md](docs/ECONOMICS.md) | **The bargain**: what a member gives, what they get, and what happens when they stop |
| [docs/PORTING.md](docs/PORTING.md) | **Which machines this runs on**, what is verified on each, and what a phone would still need |
| [docs/QUICKSTART.md](docs/QUICKSTART.md) | **Get two machines syncing** — every command shown has been run, with its real output |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Global architecture: layers, data model, placement, coordinator, transport |
| [docs/ROADMAP.md](docs/ROADMAP.md) | **Roadmap versus current state** — what is built, what is not, with exit criteria |
| [docs/DESIGN.md](docs/DESIGN.md) | Every mechanism explained in detail, including what was rejected and why |
| [docs/TESTING.md](docs/TESTING.md) | **Every test catalogued** with the property it proves, and what each CI job is for |
| [docs/TEST-USERS.md](docs/TEST-USERS.md) | The three published test identities, keys in plaintext |
| [SECURITY.md](SECURITY.md) | Threat model and how to report a vulnerability |
| [CONTRIBUTING.md](CONTRIBUTING.md) | Build requirements, the CI gates, and the three house rules |

## Test users

Three test users — **Alice**, **Bob** and **Carol** — ship with the project,
with real identities and real data. Their recovery phrases are printed in full
in [docs/TEST-USERS.md](docs/TEST-USERS.md) so anyone can clone the repository
and reproduce every encryption, sync and adversarial test byte for byte.

Publishing working private keys is safe here because of three enforced
mechanisms: `Store::open` **refuses** those identities outright — the fixtures
have to go through an explicitly named testing constructor — the corpus is
**generated from source** so there is no data file to tamper with, and every
byte is **pinned by digest** and checked in CI.

```bash
cargo run -p itsanas-testkit --bin generate-fixtures
```

## Installing

On a Linux machine with nothing on it, including a Raspberry Pi:

```sh
curl -fsSL https://raw.githubusercontent.com/SigSegGit/itsanas/main/install/linux.sh | sh
```

Or from a checkout. One script per system, each of which checks the machine
before it changes anything, says what it found, and finishes by storing a file
and reading it back to prove the result works there:

```sh
sh install/linux.sh          # Linux, Raspberry Pi, the Freebox VM
sh install/macos.sh          # macOS, Apple silicon and Intel
sh install/android-termux.sh # Android, through Termux — a command line, not an app
sudo sh install/coordinator.sh   # the machine with a public address
```

```powershell
powershell -ExecutionPolicy Bypass -File install\windows.ps1
```

[install/README.md](install/README.md) has a column saying which of those has
actually been executed on the system it claims to install, because an installer
nobody has run is a hypothesis with a shebang. Today that column says Windows
and Linux, both on x86-64, and nothing else.

The Android script installs the **command line tool**, not an app: there is no
APK, no file picker and no background service, and Android will kill a daemon
left running overnight whatever you do about it. What it is for is the one thing
a phone is good for here — half the constants in this project are chosen for ARM
devices, and a phone is the ARM device most people own.
[install/android.md](install/android.md) says what a real client would take and
why none of it is written.

## Building

**Requires Rust 1.88 or newer** — the code uses let-chains, so edition 2024 on
its own is not enough and an older toolchain fails with a parse error rather
than a version message.

```bash
cargo test --workspace
```

Two tests — the real 64 MiB Argon2id cost and a 64 MiB streaming round trip —
are marked `#[ignore]` so the normal suite stays fast. CI runs them separately:

```bash
cargo test --workspace -- --ignored
```

## Contributing

Bug reports and patches welcome. [CONTRIBUTING.md](CONTRIBUTING.md) has the
build requirements, the CI gates you can run locally, and four house rules
— a test that would fail without the change, an entry in
[docs/TESTING.md](docs/TESTING.md) saying what each proves, documentation
updated in the same commit as the code, and nothing merged that no code calls.

The last three are enforced by scripts rather than by review, because each was
written the day something shipped broken and nothing in the toolchain noticed:

```bash
bash scripts/check-catalogue.sh    # every test named in the docs exists
python scripts/check-messages.py   # `cargo fmt` eats line continuations in strings
python scripts/check-wired.py      # every public function has a caller
bash scripts/check-installers.sh   # the installers parse, and agree with Cargo.toml
```

## Licence

[GNU Affero General Public License v3.0 or later](LICENSE).

AGPL rather than a permissive licence on purpose: ITSaNAS is a networked storage
system, and the copyleft should follow it when it is run as a service, not only
when the source is redistributed.
