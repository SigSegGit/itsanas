#!/usr/bin/env sh
# Install ITSaNAS from the latest release, without compiling anything.
# Linux (x86_64, aarch64 incl. the Raspberry Pi) and macOS (Apple silicon, Intel).
#
#   curl -fsSL https://raw.githubusercontent.com/SigSegGit/itsanas/main/install/get.sh | sh
#   sh install/get.sh --help
#
# For a tester who cannot, or should not have to, build Rust: install/linux.sh
# and install/macos.sh build from source and stay the way to install what is on
# `main` before it is released.
#
# What it does: finds the latest *published* release through the GitHub API,
# downloads its manifest.txt and the binary for this machine, checks the
# binary's size and SHA-256 against the manifest, puts it where linux.sh and
# macos.sh put theirs (~/.local/bin), offers to add that to the PATH the way
# they do, and runs `itsanas setup`.
#
# What it trusts, honestly: the manifest is signed by Nicolas's release key, but
# this script cannot check an Ed25519 signature with what a fresh machine has.
# So for this first download the trust root is HTTPS to github.com: the size
# and hash check catches a truncated or corrupted download, not a forged
# release. From then on, the installed binary verifies every update's signature
# against the key compiled into it (docs/RELEASING.md).
#
# POSIX sh, no bashisms, prompts read from /dev/tty (stdin is this script when
# it is piped), and no `set -e`: every failure says what to do in one line.

set -u

REPO="${ITSANAS_REPO:-SigSegGit/itsanas}"
PREFIX="${ITSANAS_PREFIX:-$HOME/.local}"
DO_SETUP=1
ASSUME_YES=0

say()  { printf '%s\n' "$*"; }
fail() { printf '\nerror: %s\n' "$1" >&2; exit 1; }

# Delegation, as in linux.sh: there is one uninstaller, install/clean.sh. Piped
# from the network there is no sibling to run, and `$0` is `sh`, so `dirname`
# would answer the current directory -- whatever clean.sh happens to be there
# would run. `[ -f "$0" ]` is the guard.
run_clean() {
    if [ -f "$0" ]; then
        here=$(dirname -- "$0")
        if [ -f "$here/clean.sh" ]; then
            exec sh "$here/clean.sh" "$@"
        fi
    fi
    fail "--clean needs the checkout: clone the repository and run  sh install/clean.sh"
}

usage() {
    cat <<'USAGE'
Install ITSaNAS from the latest release, without compiling.

  curl -fsSL https://raw.githubusercontent.com/SigSegGit/itsanas/main/install/get.sh | sh
  sh install/get.sh [options]

Options
  --prefix DIR     where to put the binary (default: ~/.local, so ~/.local/bin)
  --no-setup       install only; do not run `itsanas setup` afterwards
  --yes            add the binary's directory to the PATH without asking
  --clean          remove what a previous install put here, then stop
                   (a dry run; add --yes to actually do it)
  --help           this

Environment
  ITSANAS_PREFIX   same as --prefix
  ITSANAS_REPO     owner/name of the repository (default: SigSegGit/itsanas)
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) [ $# -ge 2 ] || fail "--prefix needs a directory"; PREFIX="$2"; shift 2 ;;
        --prefix=*) PREFIX="${1#--prefix=}"; shift ;;
        --no-setup) DO_SETUP=0; shift ;;
        --yes|-y) ASSUME_YES=1; shift ;;
        --clean) shift; run_clean "$@" ;;
        --help|-h) usage; exit 0 ;;
        *) fail "unknown option: $1 (run with --help for the list)" ;;
    esac
done

have() { command -v "$1" >/dev/null 2>&1; }

# A subshell around the probe: dash exits outright on a failed redirection of
# the special built-in `:` (found on a Raspberry Pi by linux.sh).
have_tty() { (exec 2>/dev/null; : < /dev/tty); }

confirm() {
    [ "$ASSUME_YES" -eq 1 ] && return 0
    have_tty || return 1
    printf '%s [y/N] ' "$1"
    reply=""
    read -r reply < /dev/tty || return 1
    case "$reply" in y|Y|yes|YES|o|oui) return 0 ;; *) return 1 ;; esac
}

# >>> path-line: identical in linux.sh and macos.sh; check-installers.sh
# compares the two copies and runs this one in a throwaway HOME.
#
# One line, marked, so `clean.sh` can remove exactly it and nothing a person
# wrote. Found on the first real Mac (2026-10-05): the install succeeded and
# `itsanas` was "command not found", because this only warned. A newcomer
# reads a warning after the fact, if at all.
PATH_MARK='# added by the ITSaNAS installer'

# The file a new login shell of this user reads: zsh reads ~/.zprofile; bash
# reads ~/.bash_profile and ignores ~/.profile when that exists; anything else
# gets ~/.profile, which every POSIX login shell reads.
path_profile() {
    case "${SHELL:-}" in
        */zsh) printf '%s\n' "$HOME/.zprofile" ;;
        */bash)
            if [ -f "$HOME/.bash_profile" ]; then
                printf '%s\n' "$HOME/.bash_profile"
            else
                printf '%s\n' "$HOME/.profile"
            fi ;;
        *) printf '%s\n' "$HOME/.profile" ;;
    esac
}

# add_path_line PROFILE DIR: 0 added, 1 already there, 2 could not write.
# "Already there" also covers Debian's stock ~/.profile, which puts
# "$HOME/.local/bin" on the PATH at login if it exists: a second line would
# only put it there twice.
add_path_line() {
    _profile=$1
    _dir=$2
    _line="export PATH=\"$_dir:\$PATH\" $PATH_MARK"
    if [ -f "$_profile" ]; then
        grep -qxF -- "$_line" "$_profile" && return 1
        if [ "$_dir" = "$HOME/.local/bin" ] \
            && grep -qF -- '$HOME/.local/bin' "$_profile"; then
            return 1
        fi
        # A file whose last line has no newline would glue ours onto it.
        if [ -s "$_profile" ] && [ -n "$(tail -c 1 "$_profile")" ]; then
            printf '\n' >> "$_profile" || return 2
        fi
    fi
    printf '%s\n' "$_line" >> "$_profile" || return 2
    return 0
}
# <<< path-line

# ------------------------------------------------------------- this machine

# The release names its binaries by Rust target; these are the five it builds.
# Linux binaries are glibc ones, so a musl system (Alpine) is told plainly
# rather than handed a binary that fails with "not found".
os=$(uname -s)
arch=$(uname -m)
case "$arch" in
    x86_64|amd64) arch=x86_64 ;;
    aarch64|arm64) arch=aarch64 ;;
    *) fail "no release is built for a $arch processor: build from source with install/linux.sh" ;;
esac
case "$os" in
    Linux)
        if have ldd && ldd --version 2>&1 | grep -qi musl; then
            fail "this Linux uses musl (Alpine?), and releases are built for glibc: build from source with install/linux.sh"
        fi
        TARGET="$arch-unknown-linux-gnu" ;;
    Darwin)
        # A Terminal running under Rosetta reports x86_64 on Apple silicon;
        # the native binary is the one to install there.
        if [ "$arch" = x86_64 ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null)" = 1 ]; then
            arch=aarch64
        fi
        TARGET="$arch-apple-darwin" ;;
    *) fail "no release is built for $os: on Windows, use install/get.ps1" ;;
esac

if have curl; then
    fetch() { curl -fsSL --retry 3 -o "$2" "$1"; }
elif have wget; then
    fetch() { wget -q -O "$2" "$1"; }
else
    fail "neither curl nor wget is installed: install one (sudo apt-get install -y curl) and run this again"
fi

if have sha256sum; then
    sha256_of() { sha256sum "$1" | cut -d' ' -f1; }
elif have shasum; then
    sha256_of() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
    fail "no sha256sum or shasum here to check the download with: install coreutils and run this again"
fi

WORK=$(mktemp -d) || fail "could not make a temporary directory: is /tmp full?"
trap 'rm -rf "$WORK"' EXIT INT TERM

# ------------------------------------------------------------- the release

say "Looking for the latest ITSaNAS release..."
fetch "https://api.github.com/repos/$REPO/releases/latest" "$WORK/release.json" \
    || fail "no published release found for $REPO (or GitHub is unreachable): try again later, or build from source with install/linux.sh"
TAG=$(sed -n 's/^ *"tag_name" *: *"\([^"]*\)".*/\1/p' "$WORK/release.json" | head -n 1)
# The tag goes into URLs below; anything but a plain version tag is refused.
case "$TAG" in
    v[0-9]*) ;;
    *) fail "GitHub answered without a release version: try again in a minute" ;;
esac
case "$TAG" in
    *[!A-Za-z0-9._-]*) fail "GitHub answered with an odd release name ($TAG): try again in a minute" ;;
esac

BASE="https://github.com/$REPO/releases/download/$TAG"
fetch "$BASE/manifest.txt" "$WORK/manifest.txt" \
    || fail "release $TAG has no manifest.txt yet: it may still be being published, try again in a few minutes"

# file <target> <name> <size> <blake3> <sha256>
LINE=$(awk -v t="$TARGET" '$1 == "file" && $2 == t { print; exit }' "$WORK/manifest.txt")
[ -n "$LINE" ] || fail "release $TAG has no binary for $TARGET: build from source, or wait for the next release"
NAME=$(printf '%s\n' "$LINE" | awk '{ print $3 }')
SIZE=$(printf '%s\n' "$LINE" | awk '{ print $4 }')
SHA=$(printf '%s\n' "$LINE" | awk '{ print $6 }')
[ "$NAME" = "itsanas-$TARGET" ] || fail "the manifest of $TAG is damaged: try again later"

say "Downloading ITSaNAS ${TAG#v} for $TARGET..."
fetch "$BASE/$NAME" "$WORK/$NAME" \
    || fail "the download of $NAME stopped: check the network and run this again"

GOT_SIZE=$(wc -c < "$WORK/$NAME" | tr -d ' ')
[ "$GOT_SIZE" = "$SIZE" ] \
    || fail "the download is incomplete ($GOT_SIZE of $SIZE bytes): run this again"
[ "$(sha256_of "$WORK/$NAME")" = "$SHA" ] \
    || fail "the download does not match the release (SHA-256 differs): run this again; if it persists, tell Nicolas"
say "  ok   size and SHA-256 match the release manifest"

# ----------------------------------------------------------------- install

BIN_DIR="$PREFIX/bin"
mkdir -p "$BIN_DIR" || fail "could not create $BIN_DIR: choose another place with --prefix"
# Beside it, then renamed over it: a new file, never new bytes in the old one.
# A running daemon keeps its old file, and on Apple silicon a binary rewritten
# in place keeps a stale code signature and is killed at launch (#240).
cp "$WORK/$NAME" "$BIN_DIR/.itsanas.new" \
    && chmod 755 "$BIN_DIR/.itsanas.new" \
    && mv -f "$BIN_DIR/.itsanas.new" "$BIN_DIR/itsanas" \
    || fail "could not install into $BIN_DIR: check its permissions, or choose another place with --prefix"
say "  ok   $BIN_DIR/itsanas (${TAG#v})"

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
        PROFILE=$(path_profile)
        if confirm "Add $BIN_DIR to your PATH in $PROFILE (one marked line)?"; then
            add_path_line "$PROFILE" "$BIN_DIR"
            case $? in
                0|1) say "  ok   $PROFILE puts it on the PATH; open a new terminal to type \`itsanas\`" ;;
                *) say "  Could not write $PROFILE. Add this line to it:  export PATH=\"$BIN_DIR:\$PATH\"" ;;
            esac
        else
            say "  To type \`itsanas\` anywhere, add to $PROFILE:  export PATH=\"$BIN_DIR:\$PATH\""
        fi ;;
esac

# ------------------------------------------------------------------- setup

if [ "$DO_SETUP" -eq 0 ]; then
    say "Installed. Next: $BIN_DIR/itsanas setup"
    exit 0
fi
if ! have_tty; then
    say "Installed. There is no terminal here to set it up in; next, run: $BIN_DIR/itsanas setup"
    exit 0
fi
say ""
say "Installed. Starting the setup..."
"$BIN_DIR/itsanas" setup < /dev/tty
