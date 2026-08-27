# ITSaNAS

**A peer-to-peer cloud where people trade disk space, and hosting someone's data
gives you no way to read it.**

You pledge some of your disk and bandwidth. Other members do the same. Your
files live encrypted on their machines, theirs live encrypted on yours, and
nobody can open anybody else's. Devices come and go — laptops sleep, a Pi
reboots — and your data stays available and stays in sync.

> **Status: early, but it runs.** Two machines keep a folder in sync over an
> encrypted, mutually authenticated connection: drop a file in, it appears on
> the other; delete it, it goes from both. 462 tests.
>
> What is missing before it is a *network* rather than a personal sync tool:
> there is no coordinator server, so members cannot find each other and peers
> are configured by hand. See [docs/ROADMAP.md](docs/ROADMAP.md).
> Nothing here is ready to hold data you care about yet.

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
- **Small trusted surface.** There is an optional coordinator, and it is
  deliberately not trusted with data, keys, or plaintext of any kind.

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
| `itsanas-tls` | TLS 1.3 with device authentication bound to the channel, no certificate authority | **implemented** |
| `itsanas-net` | Peer protocol, encrypted transport, sync sessions, proof-of-storage challenges | **implemented** |
| `itsanas-placement` | Rendezvous hashing, replication targets, repair planning | **implemented** (execution pending) |
| `itsanas-coord` | Device certificates and revocation, measured availability, accounting, directory | **library implemented**, server pending |
| `itsanas-folder` | A real directory mirrored into the store and back: import, export, delete, file watching | **implemented** |
| `itsanas-cli` (`itsanas`) | Command line and daemon: init, login, folder, put, get, sync, serve, daemon, doctor | **implemented** |

## Documentation

| Document | What is in it |
| --- | --- |
| [docs/HANDOVER.md](docs/HANDOVER.md) | **Start here to pick the project up cold** — state, decisions that must not be reversed, what is next |
| [docs/ECONOMICS.md](docs/ECONOMICS.md) | **The bargain**: what a member gives, what they get, and what happens when they stop |
| [docs/QUICKSTART.md](docs/QUICKSTART.md) | **Get two machines syncing** — every command shown has been run, with its real output |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Global architecture: layers, data model, placement, coordinator, transport |
| [docs/ROADMAP.md](docs/ROADMAP.md) | **Roadmap versus current state** — what is built, what is not, with exit criteria |
| [docs/DESIGN.md](docs/DESIGN.md) | Every mechanism explained in detail, including what was rejected and why |
| [docs/TESTING.md](docs/TESTING.md) | **Every test catalogued** with the property it proves, and what each CI job is for |
| [docs/TEST-USERS.md](docs/TEST-USERS.md) | The three published test identities, keys in plaintext |
| [SECURITY.md](SECURITY.md) | Threat model and how to report a vulnerability |

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

## Building

Requires a recent stable Rust toolchain.

```bash
cargo test --workspace
```

Two tests — the real 64 MiB Argon2id cost and a 64 MiB streaming round trip —
are marked `#[ignore]` so the normal suite stays fast. CI runs them separately:

```bash
cargo test --workspace -- --ignored
```

## Contributing

Bug reports and patches welcome. Three house rules, all enforced by CI:

- `cargo clippy --workspace --all-targets` must be clean at `-D warnings`.
- New behaviour comes with a test that would fail without it. Tests that assert
  an attack *fails* are as valuable as tests that assert a feature works.
- Every test gets an entry in [docs/TESTING.md](docs/TESTING.md) stating what it
  proves. If that sentence cannot be written, the test should not exist.

## Licence

[GNU Affero General Public License v3.0 or later](LICENSE).

AGPL rather than a permissive licence on purpose: ITSaNAS is a networked storage
system, and the copyleft should follow it when it is run as a service, not only
when the source is redistributed.
