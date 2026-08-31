#!/usr/bin/env sh
# ITSaNAS installer for Linux, including the Raspberry Pi and the Freebox VM.
#
#   curl -fsSL https://raw.githubusercontent.com/SigSegGit/itsanas/main/install/linux.sh | sh
#   sh install/linux.sh --help
#
# What it is for
# --------------
#
# A fresh Raspberry Pi OS image has no Rust, an apt cache that may be days out
# of date, a `cc` that cannot build blake3's NEON assembly without the right
# package, and 1 GB of RAM on a 4B — which is where `cargo build --release`
# usually dies, silently, with the OOM killer taking rustc and leaving cargo to
# report something unrelated.
#
# So this does not assume anything. Every step checks what it found, and every
# failure says what it was doing, what it expected, and what to try instead.
#
# Deliberate constraints
# ----------------------
#
# **POSIX sh, not bash.** Some minimal images ship dash as /bin/sh and no bash.
# A script that needs bash and does not say so fails at the first `[[`.
#
# **No `set -e`.** It hides which command failed and skips cleanup, and its
# interaction with pipelines and subshells is the single most misunderstood
# thing in shell. Every command that matters is checked by hand.
#
# **Nothing is parsed that does not have to be.** Versions come from
# `rustc --version` and are compared numerically after an explicit, checked
# split — not with a regex that a future format change silently breaks.
#
# **Idempotent.** Run it twice and the second run changes nothing. Run it after
# a failure and it resumes rather than starting over.

set -u

VERSION="1.0"
MIN_RUST_MAJOR=1
MIN_RUST_MINOR=88

# ---------------------------------------------------------------- appearance

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

# Every failure says what was being attempted and what to do about it. A
# one-word error on somebody else's machine is a support conversation.
die() {
    printf '\n%serror%s %s\n' "$C_ERR" "$C_OFF" "$1"
    shift
    for line in "$@"; do printf '       %s\n' "$line"; done
    printf '\n'
    exit 1
}

# ---------------------------------------------------------------- arguments

PREFIX="${ITSANAS_PREFIX:-$HOME/.local}"
SOURCE_DIR=""
DO_SERVICE=1
DO_BUILD=1
DO_SMOKE=1
ASSUME_YES=0

usage() {
    cat <<'USAGE'
ITSaNAS installer for Linux

  sh install/linux.sh [options]

Options
  --prefix DIR     where to put the binaries (default: ~/.local)
  --source DIR     build from this checkout instead of cloning
  --no-service     do not install or enable the systemd user unit
  --no-build       check the machine and stop, changing nothing
  --no-smoke       skip storing a test file once it is installed
  --yes            do not ask before installing system packages
  --help           this

Environment
  ITSANAS_PREFIX   same as --prefix
  ITSANAS_REPO     git URL to clone when --source is not given
                   (default: https://github.com/SigSegGit/itsanas)

It is safe to run this twice. Nothing is removed and nothing is overwritten
without saying so first.
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
        --no-smoke) DO_SMOKE=0; shift ;;
        --yes|-y) ASSUME_YES=1; shift ;;
        --help|-h) usage; exit 0 ;;
        *) die "unknown option: $1" "Run with --help for the list." ;;
    esac
done

SELF_URL="https://raw.githubusercontent.com/SigSegGit/itsanas/main/install/linux.sh"

# Ask a yes/no question on the *terminal*, not on standard input.
#
# `curl -fsSL .../linux.sh | sh` makes stdin the script itself, so a bare `read`
# swallows the next lines of the program the shell is still executing and it
# then runs a truncated one. The one-liner at the top of this file would have
# done exactly that at its first prompt. /dev/tty is the terminal whatever stdin
# happens to be, and when there is no terminal there is nobody to ask.
confirm() {
    [ "$ASSUME_YES" -eq 1 ] && return 0
    if [ ! -r /dev/tty ]; then
        warn "nothing to ask on: this is not running from a terminal"
        info "Re-run with --yes to accept, or save the script and run it:"
        info "  curl -fsSL $SELF_URL -o itsanas-install.sh"
        info "  sh itsanas-install.sh"
        return 1
    fi
    printf '  %s [y/N] ' "$1"
    read -r reply < /dev/tty
    case "$reply" in y|Y|yes|YES) return 0 ;; *) return 1 ;; esac
}

ORIGINAL_PATH="$PATH"

printf '%sITSaNAS installer %s%s\n' "$C_DIM" "$VERSION" "$C_OFF"

# ------------------------------------------------------------- what is this

step "Looking at this machine"

have_cmd() { command -v "$1" >/dev/null 2>&1; }

UNAME_S=$(uname -s 2>/dev/null || echo unknown)
UNAME_M=$(uname -m 2>/dev/null || echo unknown)

case "$UNAME_S" in
    Linux) ;;
    Darwin) die "this is macOS, not Linux" "Use install/macos.sh instead." ;;
    *) die "unsupported system: $UNAME_S" \
           "This installer covers Linux. See install/README.md for the others." ;;
esac

case "$UNAME_M" in
    aarch64|arm64) ARCH_NOTE="64-bit ARM" ;;
    x86_64|amd64)  ARCH_NOTE="64-bit x86" ;;
    # `armv8l` is the one that matters and the first version of this list did
    # not have it: it is what a 64-bit Pi reports when it is running a 32-bit
    # userland, which is exactly the case the message below describes. It would
    # have been waved through with "untested architecture" and then failed an
    # hour into the build. `armhf` and `armv6l` are the same mistake with
    # different names.
    armv6l|armv7l|armv8l|armhf|arm)
        die "32-bit ARM ($UNAME_M) is not supported" \
            "ITSaNAS needs a 64-bit target: it maps large files and keeps 64-bit" \
            "counters that a 32-bit address space cannot hold." \
            "" \
            "On a Raspberry Pi 3, 4 or 5 this almost always means 64-bit hardware" \
            "running a 32-bit Raspberry Pi OS. Re-image with the 64-bit build, or" \
            "add 'arm_64bit=1' to /boot/firmware/config.txt and reboot." \
            "" \
            "Check which you have with:  getconf LONG_BIT" ;;
    i386|i486|i586|i686)
        die "32-bit x86 ($UNAME_M) is not supported" \
            "Same reason as 32-bit ARM: 64-bit counters and large mapped files." \
            "If this machine is 64-bit, you are running a 32-bit distribution" \
            "on it, and re-installing the 64-bit one is the fix." ;;
    *) ARCH_NOTE="$UNAME_M"; warn "untested architecture: $UNAME_M" ;;
esac
ok "$ARCH_NOTE ($UNAME_M)"

# The model name, when there is one. Purely so the log says which Pi.
if [ -r /proc/device-tree/model ]; then
    MODEL=$(tr -d '\000' < /proc/device-tree/model 2>/dev/null)
    [ -n "$MODEL" ] && ok "$MODEL"
fi

# --------------------------------------------------------------- resources

step "Checking there is enough to build with"

# Memory. `cargo build --release` peaks around 1.5 GB for this workspace, and a
# Pi 4 with 1 GB and no swap will have rustc killed by the OOM reaper — which
# surfaces as "signal: 9, SIGKILL" or, worse, as a linker error that sends
# people looking in the wrong place entirely.
MEM_KB=0
if [ -r /proc/meminfo ]; then
    MEM_KB=$(awk '/^MemTotal:/ {print $2; exit}' /proc/meminfo 2>/dev/null)
    case "$MEM_KB" in ''|*[!0-9]*) MEM_KB=0 ;; esac
fi
SWAP_KB=0
if [ -r /proc/meminfo ]; then
    SWAP_KB=$(awk '/^SwapTotal:/ {print $2; exit}' /proc/meminfo 2>/dev/null)
    case "$SWAP_KB" in ''|*[!0-9]*) SWAP_KB=0 ;; esac
fi
TOTAL_MB=$(( (MEM_KB + SWAP_KB) / 1024 ))

# The right way to add swap depends on the distribution, and getting it wrong
# sends somebody to a command that is not installed. Raspberry Pi OS uses
# dphys-swapfile; Ubuntu on a Pi, and the Freebox VM, use a plain swapfile.
if [ -f /etc/dphys-swapfile ] && have_cmd dphys-swapfile; then
    SWAP_ADVICE_1="Enlarge the swap file (Raspberry Pi OS):"
    SWAP_ADVICE_2="  sudo dphys-swapfile swapoff"
    SWAP_ADVICE_3="  sudo sed -i 's/^CONF_SWAPSIZE=.*/CONF_SWAPSIZE=2048/' /etc/dphys-swapfile"
    SWAP_ADVICE_4="  sudo dphys-swapfile setup && sudo dphys-swapfile swapon"
else
    SWAP_ADVICE_1="Add 2 GB of swap:"
    SWAP_ADVICE_2="  sudo fallocate -l 2G /swapfile && sudo chmod 600 /swapfile"
    SWAP_ADVICE_3="  sudo mkswap /swapfile && sudo swapon /swapfile"
    SWAP_ADVICE_4="  echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab"
fi

if [ "$MEM_KB" -eq 0 ]; then
    warn "could not read /proc/meminfo; skipping the memory check"
elif [ "$TOTAL_MB" -lt 1200 ]; then
    die "only ${TOTAL_MB} MB of memory and swap" \
        "Building this workspace needs about 1.5 GB. rustc will be killed by the" \
        "OOM reaper, and what you will see is 'signal: 9, SIGKILL' or a linker" \
        "error that has nothing to do with the real cause." \
        "" \
        "$SWAP_ADVICE_1" \
        "$SWAP_ADVICE_2" \
        "$SWAP_ADVICE_3" \
        "$SWAP_ADVICE_4" \
        "" \
        "Or build on another machine and copy the binary across:" \
        "  cargo build --release --target aarch64-unknown-linux-gnu"
elif [ "$TOTAL_MB" -lt 2000 ]; then
    warn "${TOTAL_MB} MB of memory and swap; the build will be slow but should finish"
    info "If rustc is killed, enlarge the swap file and run this again."
else
    ok "${TOTAL_MB} MB of memory and swap"
fi

# Disk. The target directory for a debug + release build of this workspace is
# around 3 GB. A Pi with a full SD card fails halfway through with ENOSPC,
# which cargo reports as a write error against a temporary file nobody
# recognises.
DEST_PARENT=$(dirname "$PREFIX")
[ -d "$DEST_PARENT" ] || DEST_PARENT="$HOME"
FREE_MB=$(df -Pm "$DEST_PARENT" 2>/dev/null | awk 'NR==2 {print $4}')
case "${FREE_MB:-}" in
    ''|*[!0-9]*) warn "could not measure free space on $DEST_PARENT" ;;
    *)
        if [ "$FREE_MB" -lt 4000 ]; then
            warn "${FREE_MB} MB free on $DEST_PARENT; the build wants about 4 GB"
            info "Free some space, or pass --no-build and copy a binary in."
        else
            ok "${FREE_MB} MB free"
        fi ;;
esac

# --------------------------------------------------------------- toolchain

step "Checking the build tools"

have() { command -v "$1" >/dev/null 2>&1; }

# The C compiler. blake3 builds NEON assembly on aarch64 through cc, and its
# build script fails with "failed to find tool" rather than anything a person
# would connect to a missing package.
# Wait, briefly, for whatever is holding the dpkg lock. A fresh Raspberry Pi OS
# boot runs unattended-upgrades and holds it for minutes; an installer that does
# not know this fails on its first action on somebody's first boot.
wait_for_apt() {
    _waited=0
    while [ "$_waited" -lt 180 ]; do
        if ! sudo fuser /var/lib/dpkg/lock-frontend >/dev/null 2>&1 \
           && ! sudo fuser /var/lib/apt/lists/lock >/dev/null 2>&1; then
            return 0
        fi
        [ "$_waited" -eq 0 ] && info "waiting for another package operation to finish"
        sleep 5
        _waited=$((_waited + 5))
    done
    warn "still locked after three minutes; trying anyway"
    return 0
}

MISSING=""
have cc || have gcc || MISSING="$MISSING build-essential"
have git || MISSING="$MISSING git"
have curl || have wget || MISSING="$MISSING curl"
# pkg-config and libssl are not needed today. They are the two packages every
# other Rust installer asks for, and adding them "just in case" is how an
# installer grows a dependency the project does not have.

if [ -n "$MISSING" ]; then
    warn "missing packages:$MISSING"
    if have apt-get; then
        # shellcheck disable=SC2086
        if [ "$ASSUME_YES" -eq 1 ]; then
            info "installing:$MISSING"
            # A fresh Raspberry Pi OS boot runs unattended-upgrades, which holds
            # the dpkg lock for several minutes. Without this wait the installer
            # dies on the very first thing it tries, on the very first boot,
            # with an error about a lock file.
            wait_for_apt
            sudo apt-get update -qq \
                && sudo apt-get install -y $MISSING \
                || die "apt-get failed" \
                       "Two common causes on a fresh image:" \
                       "" \
                       "  A lock held by unattended-upgrades, which runs on first" \
                       "  boot. Wait a few minutes and run this again, or watch it:" \
                       "    sudo systemctl status unattended-upgrades" \
                       "" \
                       "  A cache older than a repository change:" \
                       "    sudo apt-get update && sudo apt-get upgrade"
        else
            die "these packages are needed:$MISSING" \
                "Install them and run this again:" \
                "  sudo apt-get update && sudo apt-get install -y$MISSING" \
                "" \
                "Or re-run with --yes to let this script do it."
        fi
    else
        die "these packages are needed:$MISSING" \
            "This machine has no apt-get, so install them with whatever it does" \
            "use, then run this again."
    fi
else
    ok "compiler, git and a downloader are present"
fi

# Rust. Version comparison is done on integers after an explicit split, not on
# the string: `rustc --version` prints things like "rustc 1.88.0-nightly
# (abc 2026-01-01)" and every regex written for it eventually meets a form it
# did not expect.
# rustup installs into ~/.cargo/bin and this script passes --no-modify-path, so
# a shell that has not sourced ~/.cargo/env will not see it. Without this, the
# second run of the installer reports "rust is not installed" about the Rust the
# first run installed -- nothing broken, everything looking broken, on a machine
# nobody has set up.
#
# --no-modify-path stays: silently editing somebody's shell profile is not an
# installer's business. Finding what it put there is.
if [ -x "$HOME/.cargo/bin/rustc" ]; then
    case ":$PATH:" in
        *":$HOME/.cargo/bin:"*) ;;
        *) PATH="$HOME/.cargo/bin:$PATH"; export PATH ;;
    esac
fi

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
        info "updating the toolchain with rustup"
        rustup update stable || die "rustup update failed"
        rustup default stable >/dev/null 2>&1
    else
        confirm "Install the Rust toolchain with rustup?" \
            || die "stopping: Rust ${MIN_RUST_MAJOR}.${MIN_RUST_MINOR} or newer is needed"
        info "downloading rustup (this takes a few minutes on a Pi)"
        if have curl; then
            curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs -o /tmp/rustup-init.sh
        else
            wget -q -O /tmp/rustup-init.sh https://sh.rustup.rs
        fi
        [ -s /tmp/rustup-init.sh ] || die "could not download rustup" \
            "Check the network, or install Rust from your distribution and run" \
            "this again. It needs ${MIN_RUST_MAJOR}.${MIN_RUST_MINOR} or newer."
        sh /tmp/rustup-init.sh -y --no-modify-path --profile minimal \
            || die "rustup failed to install the toolchain"
        rm -f /tmp/rustup-init.sh
        # rustup puts it here; PATH is fixed for this shell and reported below.
        PATH="$HOME/.cargo/bin:$PATH"
        export PATH
    fi

    rust_is_new_enough || die "Rust is still older than ${MIN_RUST_MAJOR}.${MIN_RUST_MINOR}" \
        "If rustup just installed it, this shell may still be finding an older" \
        "system rustc first. Open a new shell and run this again."
    ok "rust $(rustc --version 2>/dev/null | awk '{print $2}')"
fi

if [ -x "$HOME/.cargo/bin/rustc" ]; then
    case ":${ORIGINAL_PATH:-$PATH}:" in
        *":$HOME/.cargo/bin:"*) ;;
        *)
            warn "$HOME/.cargo/bin is not on your PATH"
            info "This run found it anyway. For your shells, add to ~/.profile:"
            info "  . \"\$HOME/.cargo/env\"" ;;
    esac
fi

if [ "$DO_BUILD" -eq 0 ]; then
    step "Stopping here (--no-build)"
    ok "this machine can build ITSaNAS"
    exit 0
fi

# ----------------------------------------------------------------- sources

step "Getting the source"

if [ -n "$SOURCE_DIR" ]; then
    [ -d "$SOURCE_DIR" ] || die "no such directory: $SOURCE_DIR"
    [ -f "$SOURCE_DIR/Cargo.toml" ] || die "$SOURCE_DIR is not an ITSaNAS checkout" \
        "Expected to find Cargo.toml there."
    BUILD_DIR="$SOURCE_DIR"
    ok "building from $BUILD_DIR"
else
    # Run from inside a checkout, which is the common case.
    HERE=$(CDPATH='' cd -- "$(dirname -- "$0")/.." 2>/dev/null && pwd)
    if [ -n "${HERE:-}" ] && [ -f "$HERE/Cargo.toml" ]; then
        BUILD_DIR="$HERE"
        ok "building from $BUILD_DIR"
    else
        # Defaulted, because the one-liner at the top of this file has no
        # checkout to be run from and died right here with "nothing to
        # build" -- the advertised entry point failing on the advertised
        # path, in a script whose whole point is that it works on a machine
        # nobody has configured.
        REPO="${ITSANAS_REPO:-https://github.com/SigSegGit/itsanas.git}"
        BUILD_DIR="$HOME/.local/src/itsanas"
        if [ -d "$BUILD_DIR/.git" ]; then
            info "updating $BUILD_DIR"
            git -C "$BUILD_DIR" pull --ff-only || warn "could not update; building what is there"
        else
            mkdir -p "$(dirname "$BUILD_DIR")" || die "could not create $(dirname "$BUILD_DIR")"
            git clone --depth 1 "$REPO" "$BUILD_DIR" || die "could not clone $REPO"
        fi
        ok "building from $BUILD_DIR"
    fi
fi

# ------------------------------------------------------------------- build

step "Building (this is the slow part: 15-40 minutes on a Pi 4)"

# One codegen unit fewer than cores, so the machine stays usable and the peak
# memory stays lower. On a 1 GB Pi the difference is finishing and not.
JOBS=$(nproc 2>/dev/null || echo 1)
case "$JOBS" in ''|*[!0-9]*) JOBS=1 ;; esac
if [ "$TOTAL_MB" -lt 2500 ] && [ "$JOBS" -gt 1 ]; then
    JOBS=$((JOBS - 1))
    info "using $JOBS parallel jobs to keep peak memory down"
fi

( cd "$BUILD_DIR" && cargo build --release --locked --jobs "$JOBS" ) || die \
    "the build failed" \
    "If the last thing you saw was 'signal: 9' or the machine froze, it ran out" \
    "of memory: enlarge the swap file and run this again." \
    "" \
    "If it was 'failed to find tool' or an assembler error, the C compiler is" \
    "missing or wrong: sudo apt-get install -y build-essential" \
    "" \
    "If it was a checksum or 'failed to download', the network dropped: just" \
    "run this again, cargo resumes."

ok "built"

# ----------------------------------------------------------------- install

step "Installing"

BIN_DIR="$PREFIX/bin"
mkdir -p "$BIN_DIR" || die "could not create $BIN_DIR"

for prog in itsanas itsanas-coordinator; do
    src="$BUILD_DIR/target/release/$prog"
    [ -x "$src" ] || die "$prog was not produced by the build" \
        "Expected $src. This usually means the build stopped early; scroll up."
    cp -f "$src" "$BIN_DIR/$prog" || die "could not copy $prog to $BIN_DIR"
    ok "$BIN_DIR/$prog"
done

case ":$PATH:" in
    *":$BIN_DIR:"*) ok "$BIN_DIR is already on your PATH" ;;
    *)
        warn "$BIN_DIR is not on your PATH"
        info "Add this to ~/.profile (or ~/.bashrc) and open a new shell:"
        info "  export PATH=\"$BIN_DIR:\$PATH\"" ;;
esac

# ----------------------------------------------------------------- service

if [ "$DO_SERVICE" -eq 1 ]; then
    step "Setting up the background service"

    if ! have systemctl; then
        warn "no systemd here; skipping the service"
        info "Start the daemon by hand with: itsanas daemon"
    else
        UNIT_DIR="$HOME/.config/systemd/user"
        mkdir -p "$UNIT_DIR" || die "could not create $UNIT_DIR"

        # A *user* unit, not a system one. The daemon needs the passphrase to
        # unlock the keystore and it writes into the user's home; running it as
        # root would put the keys somewhere the user cannot read and give a
        # storage daemon privileges it has no use for.
        cat > "$UNIT_DIR/itsanas.service" <<UNIT
[Unit]
Description=ITSaNAS peer-to-peer storage
Documentation=https://github.com/SigSegGit/itsanas
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$BIN_DIR/itsanas daemon
Restart=on-failure
RestartSec=30

# The passphrase. systemd reads this file as the user, so it must not be
# world-readable; the installer creates it with 600 and refuses to continue if
# it cannot. Leaving it out means the unit will not start, which is the honest
# failure: a daemon cannot prompt.
EnvironmentFile=-%h/.config/itsanas/environment

# What this does and does not buy, because the difference is not obvious.
#
# The daemon runs as you and can already write everything you can, so these
# directives do not contain it the way they would a system service. Measured
# rather than assumed: with a *user* unit, ProtectSystem=strict leaves $HOME
# writable — it is /etc, /usr and /boot that become read-only. That is worth
# having and it is not a sandbox.
#
# The leading dash on ReadWritePaths is load-bearing. Without it, a path that
# does not exist yet makes the unit refuse to start, with an error about mount
# namespaces. ~/.itsanas is created by `itsanas init`, so enabling the service
# before initialising — which is exactly what somebody does after an installer
# says "enable this" — produced a failure nobody could read.
PrivateTmp=true
ProtectSystem=strict
ProtectHome=false
NoNewPrivileges=true
ReadWritePaths=-%h/.itsanas -%h/.config/itsanas

[Install]
WantedBy=default.target
UNIT
        ok "$UNIT_DIR/itsanas.service"

        ENV_DIR="$HOME/.config/itsanas"
        mkdir -p "$ENV_DIR" && chmod 700 "$ENV_DIR" || die "could not create $ENV_DIR"
        if [ ! -f "$ENV_DIR/environment" ]; then
            umask 077
            printf '# ITSANAS_PASSPHRASE=your-passphrase-here\n' > "$ENV_DIR/environment" \
                || die "could not write $ENV_DIR/environment"
            chmod 600 "$ENV_DIR/environment"
            ok "$ENV_DIR/environment (commented out until you fill it in)"
        else
            ok "$ENV_DIR/environment already exists; left alone"
        fi

        systemctl --user daemon-reload 2>/dev/null \
            || warn "systemctl --user daemon-reload failed; is there a user session?"

        # Lingering, so the daemon runs when nobody is logged in. On a headless
        # Pi this is the difference between a storage node and a program that
        # only runs while you have an ssh session open.
        if have loginctl; then
            if loginctl show-user "$(id -un)" 2>/dev/null | grep -q '^Linger=yes'; then
                ok "lingering is on: the daemon will run without a login session"
            else
                warn "lingering is off, so the daemon stops when you log out"
                info "Turn it on with:  sudo loginctl enable-linger $(id -un)"
            fi
        fi
    fi
fi

# ------------------------------------------------------------------- check

step "Checking what was installed"

INSTALLED_VERSION=$("$BIN_DIR/itsanas" --version 2>/dev/null)
[ -n "$INSTALLED_VERSION" ] || die "the installed binary does not run" \
    "Tried: $BIN_DIR/itsanas --version" \
    "On a Pi this is usually a 32-bit userland running a 64-bit binary or the" \
    "other way round. Check with: file $BIN_DIR/itsanas"
ok "$INSTALLED_VERSION"

# ------------------------------------------------------------------- smoke

# `--version` proves the kernel can execute the file. It says nothing about
# whether the data path works on this machine, and on the machines this project
# is for -- a Raspberry Pi, a VM on a Freebox -- that is the entire question.
# blake3 uses different code on aarch64, redb memory-maps its index, and neither
# announces itself by failing to link.
if [ "$DO_SMOKE" -eq 1 ]; then
    step "Storing a file and reading it back, on this machine"
    if [ -r "$SOURCE_DIR/scripts/smoke.sh" ]; then
        if ! sh "$SOURCE_DIR/scripts/smoke.sh" "$BIN_DIR/itsanas"; then
            die "it installed but did not work" \
                "The binary runs and the data path does not, which is the" \
                "interesting kind of failure. The output above says which" \
                "step it got to." \
                "" \
                "Skip this check with --no-smoke if you need the install anyway."
        fi
    else
        warn "scripts/smoke.sh is not in this checkout; skipped"
    fi
fi

# ------------------------------------------------------------------- next

cat <<NEXT

${C_OK}Installed.${C_OFF}

Next, on this machine:

  itsanas init --username <your-name>     create an account, print the 24 words
  itsanas pledge 100G                     offer space to other members
  itsanas folder ~/Sync                   the directory to keep in step

Then either point it at a coordinator so machines elsewhere can find it:

  itsanas coordinator <host:port> --device <its-id>
  itsanas register

or add a peer on this network directly:

  itsanas peer add <host:port>

And start it:

  systemctl --user enable --now itsanas     (after filling in the passphrase in
                                             ~/.config/itsanas/environment)
  journalctl --user -u itsanas -f           to watch it

Or run it in the foreground, which asks for the passphrase properly:

  itsanas daemon

NEXT
