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
is reinstalled by hand. Key rotation is not built: the manifest format has a
`next-key` line and `Trust::learn` can add the key it names, but nothing calls
it, nothing remembers a learned key, and the old key is never dropped. Changing
keys today means pinning the new one in a release and reinstalling by hand any
node that does not take that release.

## Each version (Nicolas)

1. Bump `version` under `[workspace.package]` in `Cargo.toml`, merge.
2. Tag and push: `git tag v0.2.0 && git push origin v0.2.0`.
3. Wait for the **Release** workflow (about 20 minutes). It refuses a tag that
   does not match the workspace version, builds the five targets, and leaves a
   **draft** release with the binaries and `manifest.txt`. Testers see nothing
   yet.
4. Double-click `scripts\sign-release.cmd`. It downloads the draft's binaries
   and runs `itsanas-release check`, which refuses unless every binary matches
   `manifest.txt` and the manifest's version is the tag's, and prints each
   binary's SHA-256 (compare them with the ones the workflow's log printed). A
   draft is writable by anyone with write access to the repository, so this
   check is what makes a tampered draft one nobody signs. It then shows the
   draft's version and targets, asks "Sign it and publish it?", asks the passphrase, signs, uploads
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
catches a damaged download, not a forged release. Self-update is where the
installed binary checks every later release's signature against the key
compiled into it.

## How a node updates itself

`itsanas update [--check]`, and the daemon once a day (jittered, a first look
5 to 65 minutes after it starts) under the `updates` setting -- `notify` by
default (log, and "update available: X" in `itsanas status`), `auto`
(installs), `off` (never looks). Setup's "Updates" step and Settings set it;
`updates = "auto"` in an answers file too. In `crates/itsanas-cli/src/update.rs`:

1. no key pinned (`RELEASE_KEY = None`): says so, fetches nothing;
2. a binary under a cargo `target/` directory, or on a platform no release
   covers, never updates itself;
3. `releases/latest/download/manifest.txt` and `.sig` (HTTPS through `curl`,
   which every supported system ships; the workspace has no HTTP client and
   the signature, not the transport, is what is trusted), verified by
   `itsanas-release`: signature, format, newer than the running version;
4. the running program must be the binary its own version's signed manifest
   lists (`download/vX.Y.Z/manifest.txt`): a source build copied by an
   installer is not, and is never replaced;
5. the binary for this target is downloaded beside the program, checked
   (size, BLAKE3, SHA-256), then put in place by two renames -- the running
   program aside to `.old`, the new one in -- with the first undone if the
   second fails. Never written over: a running file is never modified (#240);
6. `itsanas update` restarts the background service; the daemon under `auto`
   exits with a failure code instead, which its service (the Windows wrapper,
   systemd `Restart=on-failure`, launchd `KeepAlive`) answers by starting the
   new program -- stopping its own service from inside would kill it half way.

**Not verified:** never run against the real GitHub (no signed release exists
yet, and no key is pinned); the Windows rename of a running `itsanas.exe` and
the service restarts were never exercised on a real machine.
