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

## Three house rules

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
