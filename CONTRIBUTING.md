# Contributing

Patches and bug reports are welcome. This file is short on purpose: everything
in it is enforced by CI, so nothing here is a matter of taste you have to guess
at.

## Getting a build

**Rust 1.88 or newer.** Not "recent stable" — 1.88 exactly, because the code
uses let-chains. Edition 2024 alone would only need 1.85, so a 1.86 toolchain
gets you a parse error that does not obviously say "upgrade".

```bash
rustup toolchain install 1.88.0
cargo build --workspace
```

`cargo test --workspace` takes about a minute. Two tests are `#[ignore]`d — the
real 64 MiB Argon2id cost and a 64 MiB streaming round trip — and run in a
separate CI job:

```bash
cargo test --workspace --all-features -- --ignored
```

Cross-compiling for the Raspberry Pi needs `gcc-aarch64-linux-gnu`, because
blake3 builds NEON assembly.

## Before opening a pull request

Every one of these runs in CI. Running them locally is faster than waiting:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features
cargo test --workspace --all-features
cargo +1.88.0 check --workspace --all-features
cargo deny --all-features check
```

## Four house rules

**1. New behaviour comes with a test that would fail without it.** A test
asserting that an attack *fails* is worth as much as one asserting a feature
works — arguably more, since this is a storage system whose whole claim is that
a host cannot read what it holds.

**2. Every test gets an entry in [docs/TESTING.md](docs/TESTING.md) saying what
it proves.** Not what it does — what breaks in the real world if it fails. If
that sentence cannot be written, the test should not exist. Test names are
sentences for the same reason: a failure should read as a statement about the
system, not as an identifier.

CI enforces the half of this that a machine can check — every name cited in the
catalogue must exist in `crates/`:

```bash
bash scripts/check-catalogue.sh
```

Rename a test and the catalogue fails until its entry follows. That rule exists
because the catalogue once described three deleted tests, one of them in bold as
the security property that mattered, whose replacement asserts the opposite.

**3. Documentation changes in the same commit as the code it describes.**
Present indicative in a document means "this runs today and a named test proves
it". Anything not yet built carries a visible marker — see the legend at the top
of [docs/ECONOMICS.md](docs/ECONOMICS.md). This rule exists because the project
has already drifted once: mechanisms that had been decided but not built were
written up in the present tense and then quoted as fact by three other
documents.

**4. A mechanism is not finished until something calls it.** Four times in one
session a subsystem was designed, implemented, given tests, documented, and
wired to nothing — the sync policy the phone was meant to inherit, two functions
written to report failing peers that never reached a status line, an accessor
for a public field. Each was found by a person reading the code and noticing,
which is not a process.

This workspace is the only consumer there is, so a `pub fn` nobody calls is
either unfinished work or forgotten work, and the two are indistinguishable from
outside:

```bash
python scripts/check-wired.py
```

Its allowlist is for things deferred by an actual decision, and each entry says
what would wire it, so the list reads as work rather than as excuses.

## The three CI checks, and why each exists

None of them is a style rule. Each was written the day something shipped broken
and nothing in the toolchain noticed.

| Check | What went wrong first |
| --- | --- |
| `scripts/check-catalogue.sh` | The catalogue listed three deleted `transport` tests, one in bold as the security property that mattered. HANDOVER.md later cited an invariant's evidence by a name missing one word, which had never existed. |
| `scripts/check-messages.py` | `cargo fmt` removes a string literal's trailing backslash and keeps the indentation as literal spaces. Every continuation in the repository had been eaten that way, including the one line an operator sees when a peer is sanctioned — and a correction to it was undone by the `cargo fmt` in the same breath and committed. |
| `scripts/check-wired.py` | See house rule 4. Note what it cannot see: a function called only from a path that never runs. The sync policy would have passed it on the day it was written, because its tests called it. |

## Things that need a discussion first

Some decisions are load-bearing and each has a test that fails if it is
reversed. They are listed in [docs/HANDOVER.md](docs/HANDOVER.md) §6 with the
guard for each. If a change touches one — floating point in placement, the
chunker's gear table, the delete-versus-edit asymmetry, the vault's refusal to
accept keys — please open an issue before the patch, so the reasoning gets
argued rather than the diff.

## Code style

- `unsafe_code = "forbid"` across the workspace. No exceptions.
- Comments explain *why*. What the code does should be readable from the code.
- `itsanas-crypto` keeps a deliberately short dependency list so it stays
  auditable in isolation. Adding one there needs a reason in the pull request.

## Reading the code

[docs/HANDOVER.md](docs/HANDOVER.md) §5 has a reading route — six files, about
two hours, after which the rest is predictable. Start there rather than at
`main.rs`.

## Licence

Contributions are accepted under [AGPL-3.0-or-later](LICENSE), the project's
licence.
