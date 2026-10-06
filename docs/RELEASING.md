# Releasing ITSaNAS

A release is five binaries and a manifest listing their sizes and hashes,
**signed by Nicolas's release key**. CI builds; Nicolas signs on his PC; the
key never reaches the repository or CI (decided 2026-10-06: a key in GitHub
Actions would let whoever takes the GitHub account push a binary onto every
member's machine).

The manifest format, the verification order and the red-team tests are in
`crates/itsanas-release` (its `lib.rs` header says why each step comes where it
does).

## Once: the release key (Nicolas)

Double-click `scripts\sign-release.cmd` (or `sh scripts/sign-release.sh`). With
no key yet, it offers to create one:

- it asks a passphrase twice, hidden (12 characters or more);
- it writes the sealed key to `%USERPROFILE%\itsanas-release-key\release-signing.key`
  (`~/itsanas-release-key/` elsewhere; `ITSANAS_RELEASE_KEY` to change it --
  never under `.itsanas*`, which the uninstallers delete);
- it prints the **public key**.

Then, before anything else:

1. copy the key file to a USB stick and put the stick in a drawer;
2. write the passphrase on paper, kept somewhere else than the stick;
3. paste the public key into `RELEASE_KEY` in `crates/itsanas-release/src/lib.rs`
   and update the test `the_release_key_is_pinned_until_nicolas_changes_it_on_purpose`
   in the same pull request. Until then every build says "no release key
   pinned yet" and trusts no download.

The file is sealed with the node keystore's own scheme (Argon2id, then
XChaCha20-Poly1305): useless without the passphrase. Losing both copies, or the
passphrase, means nodes can no longer be updated by a signed release until each
is reinstalled by hand. Changing keys later is a release signed by the old key
whose manifest names the new one (`next-key`).

## Each version (Nicolas)

1. Bump `version` under `[workspace.package]` in `Cargo.toml`, merge.
2. Tag and push: `git tag v0.2.0 && git push origin v0.2.0`.
3. Wait for the **Release** workflow (about 20 minutes). It refuses a tag that
   does not match the workspace version, builds the five targets, and leaves a
   **draft** release with the binaries and `manifest.txt`. Testers see nothing
   yet.
4. Double-click `scripts\sign-release.cmd`. It shows the draft's version and
   targets, asks "Sign it and publish it?", asks the passphrase, signs, uploads
   `manifest.txt.sig`, publishes, and says what it published.

It needs `gh` logged in (`gh auth login`) and Rust (it builds the signing tool
from the checkout, so the code that touches the key is the reviewed code).

## Installing a release (testers)

Windows, in PowerShell:

    irm https://raw.githubusercontent.com/SigSegGit/itsanas/main/install/get.ps1 | iex

Linux (x86_64, Raspberry Pi 64-bit) and macOS (Apple silicon, Intel):

    curl -fsSL https://raw.githubusercontent.com/SigSegGit/itsanas/main/install/get.sh | sh

Each downloads the latest published release for the machine, checks the size
and SHA-256 against the manifest, installs where the build-from-source
installers do (`%LOCALAPPDATA%\Programs\itsanas\bin`, `~/.local/bin`), puts
that on the PATH, and runs `itsanas setup`.

**What that first download trusts:** HTTPS to github.com. The scripts cannot
check an Ed25519 signature with what a fresh machine has, so their hash check
catches a damaged download, not a forged release. Self-update (HANDOVER §8 0w
(6)) is where the installed binary checks every later release's signature
against the key compiled into it.
