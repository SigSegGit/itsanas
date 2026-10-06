#!/bin/sh
# Sign and publish the ITSaNAS release CI has just built -- the Linux/macOS
# twin of scripts/sign-release.ps1 (double-click scripts/sign-release.cmd on
# Windows).
#
#   sh scripts/sign-release.sh [TAG]
#
# The release workflow (.github/workflows/release.yml) builds the binaries on a
# v* tag and leaves a DRAFT release holding them and an unsigned manifest.txt.
# This script: makes the release key the first time (asking first), finds the
# newest draft with `gh`, shows what it is about to sign, signs it with
# `itsanas-release sign` -- which asks the passphrase itself, hidden; this
# script never sees, prints or stores it -- uploads manifest.txt.sig, publishes
# the release and says what it published.
#
# Environment:
#   ITSANAS_RELEASE_KEY  the sealed key file
#                        (default: ~/itsanas-release-key/release-signing.key;
#                        not under ~/.itsanas*, which install/clean.sh removes)
#   ITSANAS_REPO         owner/name (default: SigSegGit/itsanas)

set -u

KEY="${ITSANAS_RELEASE_KEY:-$HOME/itsanas-release-key/release-signing.key}"
REPO="${ITSANAS_REPO:-SigSegGit/itsanas}"
TAG="${1:-}"
CHECKOUT=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd) || exit 1

fail() { printf '\nerror: %s\n' "$1" >&2; exit 1; }

confirm() {
    printf '%s [y/N] ' "$1"
    read -r answer < /dev/tty || return 1
    case "$answer" in y|Y|yes|YES|o|oui) return 0 ;; *) return 1 ;; esac
}

# Built from this checkout, so the code that touches the key is the code in the
# repository, not a binary downloaded from anywhere.
release_tool() {
    cargo run -q --release --manifest-path "$CHECKOUT/Cargo.toml" -p itsanas-release -- "$@"
}

command -v gh >/dev/null 2>&1 \
    || fail "the GitHub command line (gh) is not installed: https://cli.github.com, then gh auth login"
command -v cargo >/dev/null 2>&1 \
    || fail "cargo is not installed here: install Rust from https://rustup.rs and run this again"
gh auth status >/dev/null 2>&1 \
    || fail "gh is not logged in to GitHub: run  gh auth login  then this again"

# ------------------------------------------------------------ the key, once
if [ ! -f "$KEY" ]; then
    echo "There is no release key at $KEY."
    echo "The release key signs every ITSaNAS version; nodes install only what it signed."
    echo "It is made once, here, sealed under a passphrase you choose."
    confirm "Create it now?" \
        || fail "nothing was done. If your key file is elsewhere, set ITSANAS_RELEASE_KEY to its path."
    release_tool keygen --out "$KEY" \
        || fail "the key was not created (the reason is just above): run this again"
    echo
    echo "Do the offline copy now, before publishing anything: copy the file above to a"
    echo "USB stick, put the stick in a drawer, and keep the passphrase on paper elsewhere."
    printf 'Press Enter once the copy is made. '
    read -r _ < /dev/tty
fi

# ------------------------------------------------------- the draft to sign
if [ -z "$TAG" ]; then
    TAG=$(gh release list -R "$REPO" --limit 30 --json tagName,isDraft \
        --jq '[.[] | select(.isDraft)][0].tagName // empty') \
        || fail "could not list the releases of $REPO: check your network and gh auth status"
    [ -n "$TAG" ] \
        || fail "no draft release in $REPO: push a v* tag and wait for the release workflow to finish"
fi

WORK=$(mktemp -d) || fail "could not make a temporary directory"
trap 'rm -rf "$WORK"' EXIT INT TERM

gh release download "$TAG" -R "$REPO" -p manifest.txt -D "$WORK" \
    || fail "release $TAG has no manifest.txt: did the release workflow finish? Look at its run on GitHub"
MANIFEST="$WORK/manifest.txt"
VERSION=$(sed -n 's/^version //p' "$MANIFEST" | head -n 1)
TARGETS=$(awk '$1 == "file" { print $2 }' "$MANIFEST")

echo
echo "Draft $TAG: ITSaNAS $VERSION, binaries for:"
printf '%s\n' "$TARGETS" | sed 's/^/  /'
confirm "Sign it and publish it?" || fail "nothing was signed or published"

release_tool sign "$MANIFEST" --key "$KEY" \
    || fail "not signed (the reason is just above): nothing was published"
gh release upload "$TAG" -R "$REPO" "$MANIFEST.sig" --clobber \
    || fail "the signature could not be uploaded to $TAG: run this again, it is safe"
gh release edit "$TAG" -R "$REPO" --draft=false \
    || fail "signed and uploaded, but $TAG is still a draft: run this again, or publish it on GitHub"

echo
echo "Published ITSaNAS $VERSION ($TAG), signed, for: $(printf '%s' "$TARGETS" | tr '\n' ' ')"
echo "Testers can now install it: see docs/RELEASING.md."
