#!/usr/bin/env sh
# ITSaNAS installer for macOS, Apple silicon and Intel.
#
#   sh install/macos.sh
#   sh install/macos.sh --help
#
# What is different from Linux
# ----------------------------
#
# **The Command Line Tools are not installed by default and Rust cannot link
# without them.** A fresh Mac has no `cc`; typing `cc` triggers a graphical
# prompt, and a script that ignores it leaves the user staring at a dialog they
# did not ask for while the build fails. So it is checked, and triggered on
# purpose with an explanation.
#
# **launchd, not systemd.** A LaunchAgent under ~/Library/LaunchAgents runs in
# the user's session, which is where a daemon holding the user's keys belongs.
#
# **Gatekeeper.** A binary built here is not signed or notarised. Running it
# from a terminal is fine — Gatekeeper only quarantines things that arrive with
# the quarantine attribute, which a local build does not have. Downloading a
# prebuilt binary would be a different story, and this installer does not.
#
# The same constraints as the Linux one: POSIX sh, no `set -e`, no parsing that
# is not checked, and safe to run twice.

set -u

VERSION="1.0"
MIN_RUST_MAJOR=1
MIN_RUST_MINOR=88

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    C_OK=$(printf '\033[32m'); C_WARN=$(printf '\033[33m')
    C_ERR=$(printf '\033[31m'); C_DIM=$(printf '\033[2m'); C_OFF=$(printf '\033[0m')
else
    C_OK=''; C_WARN=''; C_ERR=''; C_DIM=''; C_OFF=''
fi

step()  { printf '\n%s==>%s %s\n' "$C_DIM" "$C_OFF" "$*"; }
ok()    { printf '  %sok%s   %s\n' "$C_OK" "$C_OFF" "$*"; }
warn()  { printf '  %swarn%s %s\n' "$C_WARN" "$C_OFF" "$*"; }
info()  { printf '       %s\n' "$*"; }
die() {
    printf '\n%serror%s %s\n' "$C_ERR" "$C_OFF" "$1"
    shift
    for line in "$@"; do printf '       %s\n' "$line"; done
    printf '\n'
    exit 1
}

PREFIX="${ITSANAS_PREFIX:-$HOME/.local}"
SOURCE_DIR=""
DO_SERVICE=1
DO_BUILD=1
ASSUME_YES=0

usage() {
    cat <<'USAGE'
ITSaNAS installer for macOS

  sh install/macos.sh [options]

Options
  --prefix DIR     where to put the binaries (default: ~/.local)
  --source DIR     build from this checkout instead of looking for one
  --no-service     do not write the LaunchAgent
  --no-build       check the machine and stop, changing nothing
  --yes            do not ask before installing anything
  --help           this

It is safe to run this twice.
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) [ $# -ge 2 ] || die "--prefix needs a directory"; PREFIX="$2"; shift 2 ;;
        --prefix=*) PREFIX="${1#--prefix=}"; shift ;;
        --source) [ $# -ge 2 ] || die "--source needs a directory"; SOURCE_DIR="$2"; shift 2 ;;
        --source=*) SOURCE_DIR="${1#--source=}"; shift ;;
        --no-service) DO_SERVICE=0; shift ;;
        --no-build) DO_BUILD=0; shift ;;
        --yes|-y) ASSUME_YES=1; shift ;;
        --help|-h) usage; exit 0 ;;
        *) die "unknown option: $1" "Run with --help for the list." ;;
    esac
done

printf '%sITSaNAS installer %s%s\n' "$C_DIM" "$VERSION" "$C_OFF"
have() { command -v "$1" >/dev/null 2>&1; }

# ------------------------------------------------------------- what is this

step "Looking at this machine"

[ "$(uname -s)" = "Darwin" ] || die "this is not macOS" \
    "uname says $(uname -s). Use install/linux.sh instead."

MACOS_VERSION=$(sw_vers -productVersion 2>/dev/null || echo unknown)
MACOS_MAJOR=${MACOS_VERSION%%.*}
case "$MACOS_MAJOR" in
    ''|*[!0-9]*) warn "could not read the macOS version" ;;
    *)
        if [ "$MACOS_MAJOR" -lt 12 ]; then
            warn "macOS $MACOS_VERSION is older than the toolchain is tested on"
            info "Rust supports macOS 10.12 and newer; this may still work."
        else
            ok "macOS $MACOS_VERSION"
        fi ;;
esac

case "$(uname -m)" in
    arm64) ok "Apple silicon (arm64)"; BREW_PREFIX=/opt/homebrew ;;
    x86_64)
        # Rosetta makes an Apple silicon Mac claim to be x86_64 when the shell
        # itself is translated. Building under Rosetta produces an x86 binary
        # that runs, slowly, on hardware that could have run a native one.
        if sysctl -n sysctl.proc_translated 2>/dev/null | grep -q '^1$'; then
            warn "this shell is running under Rosetta on Apple silicon"
            info "The build would produce an x86 binary and run translated."
            info "Open a native terminal (right-click Terminal in Finder, uncheck"
            info "\"Open using Rosetta\") and run this again."
        else
            ok "Intel (x86_64)"
        fi
        BREW_PREFIX=/usr/local ;;
    *) die "unsupported architecture: $(uname -m)" ;;
esac

# --------------------------------------------------------------- resources

step "Checking there is enough to build with"

MEM_BYTES=$(sysctl -n hw.memsize 2>/dev/null || echo 0)
case "$MEM_BYTES" in ''|*[!0-9]*) MEM_BYTES=0 ;; esac
if [ "$MEM_BYTES" -gt 0 ]; then
    ok "$((MEM_BYTES / 1024 / 1024 / 1024)) GB of memory"
else
    warn "could not read the memory size"
fi

FREE_MB=$(df -Pm "$HOME" 2>/dev/null | awk 'NR==2 {print $4}')
case "${FREE_MB:-}" in
    ''|*[!0-9]*) warn "could not measure free space" ;;
    *)
        if [ "$FREE_MB" -lt 4000 ]; then
            warn "${FREE_MB} MB free; the build wants about 4 GB"
        else
            ok "${FREE_MB} MB free"
        fi ;;
esac

# --------------------------------------------------------------- toolchain

step "Checking the build tools"

# The Command Line Tools. A fresh Mac has none, and `cc` is a stub that pops a
# graphical installer. A script that just runs cargo leaves the user looking at
# a dialog with no idea where it came from.
if xcode-select -p >/dev/null 2>&1; then
    ok "Xcode command line tools at $(xcode-select -p)"
else
    warn "the Xcode command line tools are missing"
    info "Rust links with Apple's linker, which comes from these tools."
    if [ "$ASSUME_YES" -eq 0 ]; then
        printf '  Trigger the installer now? [y/N] '
        read -r reply
        case "$reply" in y|Y|yes|YES) ;; *) die "stopping: the command line tools are needed" \
            "Install them with:  xcode-select --install" ;; esac
    fi
    xcode-select --install 2>/dev/null
    die "finish the graphical installer, then run this again" \
        "A window should have appeared. It downloads about 1 GB." \
        "If no window appeared, the tools may be installing already, or you can" \
        "get them from https://developer.apple.com/download/all/"
fi

# Rust. Compared as integers after an explicit split, for the same reason as
# everywhere else: the version string's shape is not a promise.
rust_is_new_enough() {
    have rustc || return 1
    _v=$(rustc --version 2>/dev/null | awk '{print $2}')
    [ -n "$_v" ] || return 1
    _major=${_v%%.*}
    _rest=${_v#*.}
    _minor=${_rest%%.*}
    case "$_major$_minor" in ''|*[!0-9]*) return 1 ;; esac
    [ "$_major" -gt "$MIN_RUST_MAJOR" ] && return 0
    [ "$_major" -eq "$MIN_RUST_MAJOR" ] && [ "$_minor" -ge "$MIN_RUST_MINOR" ] && return 0
    return 1
}

if rust_is_new_enough; then
    ok "rust $(rustc --version 2>/dev/null | awk '{print $2}')"
else
    if have rustc; then
        warn "rust $(rustc --version 2>/dev/null | awk '{print $2}') is older than ${MIN_RUST_MAJOR}.${MIN_RUST_MINOR}"
    else
        warn "rust is not installed"
    fi

    if have rustup; then
        rustup update stable || die "rustup update failed"
    else
        # Homebrew's rust is a possibility and is deliberately not used: it
        # lags, and mixing it with a rustup toolchain later is a source of
        # confusion nobody enjoys. rustup is the supported path.
        if [ "$ASSUME_YES" -eq 0 ]; then
            printf '  Install the Rust toolchain with rustup? [y/N] '
            read -r reply
            case "$reply" in y|Y|yes|YES) ;; *) die "stopping: Rust ${MIN_RUST_MAJOR}.${MIN_RUST_MINOR} or newer is needed" ;; esac
        fi
        curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o /tmp/rustup-init.sh \
            || die "could not download rustup" "Check the network and try again."
        sh /tmp/rustup-init.sh -y --no-modify-path --profile minimal \
            || die "rustup failed to install the toolchain"
        rm -f /tmp/rustup-init.sh
        PATH="$HOME/.cargo/bin:$PATH"; export PATH
    fi
    rust_is_new_enough || die "Rust is still older than ${MIN_RUST_MAJOR}.${MIN_RUST_MINOR}" \
        "Open a new shell and run this again."
    ok "rust $(rustc --version 2>/dev/null | awk '{print $2}')"
fi

if [ "$DO_BUILD" -eq 0 ]; then
    step "Stopping here (--no-build)"
    ok "this machine can build ITSaNAS"
    exit 0
fi

# ----------------------------------------------------------------- sources

step "Getting the source"

if [ -n "$SOURCE_DIR" ]; then
    [ -f "$SOURCE_DIR/Cargo.toml" ] || die "$SOURCE_DIR is not an ITSaNAS checkout"
    BUILD_DIR="$SOURCE_DIR"
else
    HERE=$(CDPATH='' cd -- "$(dirname -- "$0")/.." 2>/dev/null && pwd)
    [ -n "${HERE:-}" ] && [ -f "$HERE/Cargo.toml" ] || die "nothing to build" \
        "Run this from inside a checkout, or pass --source DIR."
    BUILD_DIR="$HERE"
fi
ok "building from $BUILD_DIR"

# ------------------------------------------------------------------- build

step "Building (5-15 minutes the first time)"

( cd "$BUILD_DIR" && cargo build --release --locked ) || die "the build failed" \
    "If it mentioned a missing linker or SDK, the command line tools are" \
    "incomplete: xcode-select --install" \
    "" \
    "If it mentioned a checksum or a download, the network dropped: run this" \
    "again, cargo resumes."
ok "built"

# ----------------------------------------------------------------- install

step "Installing"

BIN_DIR="$PREFIX/bin"
mkdir -p "$BIN_DIR" || die "could not create $BIN_DIR"
for prog in itsanas itsanas-coordinator; do
    src="$BUILD_DIR/target/release/$prog"
    [ -x "$src" ] || die "$prog was not produced by the build" "Expected $src."
    cp -f "$src" "$BIN_DIR/$prog" || die "could not copy $prog to $BIN_DIR"
    ok "$BIN_DIR/$prog"
done

case ":$PATH:" in
    *":$BIN_DIR:"*) ok "$BIN_DIR is already on your PATH" ;;
    *)
        warn "$BIN_DIR is not on your PATH"
        info "Add this to ~/.zprofile and open a new terminal:"
        info "  export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac

# ----------------------------------------------------------------- service

if [ "$DO_SERVICE" -eq 1 ]; then
    step "Writing the LaunchAgent"

    AGENT_DIR="$HOME/Library/LaunchAgents"
    mkdir -p "$AGENT_DIR" || die "could not create $AGENT_DIR"
    PLIST="$AGENT_DIR/net.itsanas.daemon.plist"

    if [ -f "$PLIST" ]; then
        ok "$PLIST already exists; left alone"
    else
        # Written but deliberately not loaded. The daemon needs the passphrase,
        # and a plist that carries one is a decision the user should make on
        # purpose rather than inherit from an installer: everything in
        # ~/Library/LaunchAgents is readable by anything running as them.
        cat > "$PLIST" <<PLIST_END
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>net.itsanas.daemon</string>
  <key>ProgramArguments</key>
  <array>
    <string>$BIN_DIR/itsanas</string>
    <string>daemon</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict><key>SuccessfulExit</key><false/></dict>
  <key>StandardOutPath</key>
  <string>$HOME/Library/Logs/itsanas.log</string>
  <key>StandardErrorPath</key>
  <string>$HOME/Library/Logs/itsanas.log</string>
  <!-- The passphrase. Uncomment and fill in, or leave it out and run
       \`itsanas daemon\` by hand so it can ask. Anything running as you can
       read this file. -->
  <!--
  <key>EnvironmentVariables</key>
  <dict>
    <key>ITSANAS_PASSPHRASE</key>
    <string>your-passphrase-here</string>
  </dict>
  -->
</dict>
</plist>
PLIST_END
        ok "$PLIST (written, not loaded)"

        # plutil is part of macOS and validates the file. A malformed plist is
        # rejected by launchd with a message that says nothing useful.
        if have plutil; then
            if plutil -lint "$PLIST" >/dev/null 2>&1; then
                ok "the plist parses"
            else
                warn "plutil says the plist is malformed; launchd will refuse it"
                plutil -lint "$PLIST"
            fi
        fi

        info "Load it once you have decided about the passphrase:"
        info "  launchctl load -w $PLIST"
        info "  tail -f ~/Library/Logs/itsanas.log"
    fi
fi

# ------------------------------------------------------------------- check

step "Checking what was installed"
INSTALLED_VERSION=$("$BIN_DIR/itsanas" --version 2>/dev/null)
[ -n "$INSTALLED_VERSION" ] || die "the installed binary does not run" \
    "Tried: $BIN_DIR/itsanas --version"
ok "$INSTALLED_VERSION"

cat <<NEXT

${C_OK}Installed.${C_OFF}

Next:

  itsanas init --username <your-name>     create an account, print the 24 words
  itsanas pledge 100G                     offer space to other members
  itsanas folder ~/Sync                   the directory to keep in step
  itsanas peer add <host:port>            or point at a coordinator, see below
  itsanas daemon                          run it

  itsanas coordinator <host:port> --device <its-id>
  itsanas register

NEXT
