#!/usr/bin/env sh
# ITSaNAS on Android, through Termux.
#
#   pkg install git && git clone https://github.com/SigSegGit/itsanas && cd itsanas
#   sh install/android-termux.sh
#
# Read this first
# ---------------
#
# **This does not install a sync app, and there is no app to install.** There is
# no APK, no JNI bridge, no file picker and no background service. What this
# gives you is the command-line tool running on your phone's own processor, and
# a check that it stores a file and reads it back there. See `android.md` for
# what a real app would take and why none of it is written.
#
# That is worth doing anyway, and it is the reason this script exists rather
# than a paragraph telling you to type six commands. Half the constants in this
# project — the chunk size, the memory the key derivation may use, how much work
# an audit round is allowed — are chosen for ARM devices, and a phone is the ARM
# device most people have. Until something runs there, those numbers are
# guesses. CI runs the same check under emulation; your phone is the real thing.
#
# What will disappoint you
# ------------------------
#
# Android kills background processes, and Samsung's One UI is stricter than
# stock. A daemon left running overnight will usually be dead by morning even
# with a wake-lock. Termux also cannot see your photos or documents without
# `termux-setup-storage`, and even then it is a scoped view. So: a way to find
# out whether the core works on your hardware. Not a way to keep files synced.
#
# Deliberate constraints, same as the other installers here
# ---------------------------------------------------------
#
# **POSIX sh.** Termux ships bash, but the other installers in this repository
# are POSIX and one checking script reads all of them.
#
# **No `set -e`.** Every command that matters is checked by hand, so a failure
# can say what it was doing rather than stopping at a line number.
#
# **The package mirror is assumed to be broken.** Termux's default mirror is
# frequently down or stale, and the resulting error ("E: Unable to locate
# package rust") reads as if the package does not exist. That is the single
# most common way this fails, so it is handled rather than hit.

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

die() {
    printf '\n%serror%s %s\n' "$C_ERR" "$C_OFF" "$1"
    shift
    for line in "$@"; do printf '       %s\n' "$line"; done
    printf '\n'
    exit 1
}

# ---------------------------------------------------------------- arguments

DO_BUILD=1
DO_SMOKE=1
ASSUME_YES=0
# Whether --check found everything it needs. The first version said "this phone
# can build ITSaNAS" unconditionally, including on a phone it had just told to
# install two packages.
READY=1

usage() {
    cat <<'USAGE'
ITSaNAS for Android, through Termux

  sh install/android-termux.sh [options]

Options
  --yes            do not ask before installing packages
  --check          look at the phone and stop, changing nothing
  --no-smoke       build, but do not store a test file afterwards
  --help           this

What you get is the `itsanas` command-line tool built for your phone's
processor, and a check that it works there. There is no app.
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --yes|-y)   ASSUME_YES=1 ;;
        --check)    DO_BUILD=0; DO_SMOKE=0 ;;
        --no-smoke) DO_SMOKE=0 ;;
        --help|-h)  usage; exit 0 ;;
        *) die "unknown option: $1" "Run with --help to see what there is." ;;
    esac
    shift
done

# Ask on the terminal, not on standard input. A script fed to `sh` through a
# pipe has the script itself on stdin, so a bare `read` eats the lines the shell
# has not run yet. Termux is the least likely place to meet that, and the habit
# belongs in all of these scripts rather than only in the one where it bites.
#
# The test opens it rather than asking about it: `[ -r /dev/tty ]` reads the
# permission bits of a device node that exists whether or not this process has a
# controlling terminal, and answers yes on a machine where the open then fails
# with ENXIO.
ask() {
    [ "$ASSUME_YES" -eq 1 ] && return 0
    # A subshell: `:` is a POSIX special built-in, and a redirection error on
    # one makes a non-interactive shell exit. dash obeys that and bash does not,
    # so the brace-group form killed install/linux.sh outright on Debian.
    if ! (exec 2>/dev/null; : < /dev/tty); then
        warn "nothing to ask on: this is not running from a terminal"
        info "Re-run with --yes to accept."
        return 1
    fi
    printf '       %s [y/N] ' "$1"
    answer=""
    read -r answer < /dev/tty || return 1
    case "$answer" in y|Y|yes|YES) return 0 ;; *) return 1 ;; esac
}

printf '%sITSaNAS for Termux %s%s\n' "$C_DIM" "$VERSION" "$C_OFF"

# ---------------------------------------------------------------- the phone

step "Looking at this phone"

# Termux sets PREFIX to its own tree. Checking for `pkg` alone would also match
# a few desktop distributions that have a different program of that name, and
# the failure that produces is confusing rather than clear.
case "${PREFIX:-}" in
    */com.termux/*) ok "Termux at $PREFIX" ;;
    *)
        die "this is not Termux" \
            "The script is for the Termux app on Android." \
            "" \
            "On a desktop or a Raspberry Pi use install/linux.sh instead;" \
            "on macOS, install/macos.sh." \
            "" \
            "If you are in Termux and see this, PREFIX is unset, which means" \
            "the shell was started in a way that skipped Termux's profile." \
            "Open a new Termux session and run it again."
        ;;
esac

ARCH=$(uname -m 2>/dev/null || echo unknown)
case "$ARCH" in
    aarch64|arm64) ok "64-bit ARM ($ARCH)" ;;
    armv6l|armv7l|armv8l|armhf|arm)
        # `armv8l` is a 64-bit phone running a 32-bit userland, which is the
        # usual shape of this on Android rather than genuinely old hardware.
        die "this is a 32-bit ARM userland ($ARCH)" \
            "ITSaNAS needs 64-bit. On a 64-bit phone this usually means Termux" \
            "was installed from an old APK; reinstall it from F-Droid." \
            "" \
            "Note that Google Play's Termux build is unmaintained and known to" \
            "be broken. F-Droid is the one that works."
        ;;
    x86_64|i686)
        warn "$ARCH — an emulator rather than a phone"
        info "This will work, but it does not test what a phone tests."
        ;;
    *) warn "unrecognised architecture: $ARCH" ;;
esac

if [ -r /proc/meminfo ]; then
    MEM_KB=$(awk '/^MemTotal:/ {print $2; exit}' /proc/meminfo 2>/dev/null)
    case "${MEM_KB:-}" in
        ''|*[!0-9]*) warn "could not read the memory size" ;;
        *)
            ok "$((MEM_KB / 1024)) MB of memory"
            if [ "$MEM_KB" -lt 2000000 ]; then
                warn "under 2 GB; the build may be killed by Android"
                info "If it dies without an error, that is what happened."
            fi
            ;;
    esac
else
    # Some Android builds restrict /proc. Not a reason to stop.
    warn "this Android build does not let Termux read /proc/meminfo"
fi

FREE_MB=$(df -Pm "$HOME" 2>/dev/null | awk 'NR==2 {print $4}')
case "${FREE_MB:-}" in
    ''|*[!0-9]*) warn "could not measure free space" ;;
    *)
        if [ "$FREE_MB" -lt 3000 ]; then
            warn "${FREE_MB} MB free in Termux's storage; the build wants about 3 GB"
            info "Termux uses the app's private storage, not the SD card."
        else
            ok "${FREE_MB} MB free"
        fi
        ;;
esac

# ---------------------------------------------------------------- packages

step "Checking the build tools"

NEEDED=""
for tool in git rustc cargo clang; do
    command -v "$tool" >/dev/null 2>&1 || NEEDED="$NEEDED $tool"
done

# Termux package names differ from the command names: rustc and cargo both come
# from `rust`, and `clang` carries the linker that `binutils` does not.
PACKAGES=""
case " $NEEDED " in *" git "*) PACKAGES="$PACKAGES git" ;; esac
case " $NEEDED " in *" rustc "*|*" cargo "*) PACKAGES="$PACKAGES rust" ;; esac
case " $NEEDED " in *" clang "*) PACKAGES="$PACKAGES clang" ;; esac

if [ -n "$PACKAGES" ] && [ "$DO_BUILD" -eq 0 ]; then
    # --check promises to change nothing, and offering to install is already a
    # change: the first version of this asked before honouring the flag, and
    # only left the phone alone because the answer happened to be no.
    warn "missing:$NEEDED"
    info "This phone would need:  pkg install$PACKAGES"
    READY=0
elif [ -n "$PACKAGES" ]; then
    info "missing:$NEEDED"
    if ask "install$PACKAGES with pkg?"; then
        # Termux's default mirror goes down often enough that "Unable to locate
        # package" almost always means the mirror, not the package. Update
        # first, and if that fails say the thing that actually fixes it.
        if ! pkg update -y >/dev/null 2>&1; then
            warn "pkg update failed"
            info "This is usually the mirror rather than your connection."
            info "Run  termux-change-repo  , pick a different mirror, retry."
        fi
        # PACKAGES is a list of package names and has to word-split here.
        # shellcheck disable=SC2086
        if ! pkg install -y $PACKAGES; then
            die "could not install:$PACKAGES" \
                "If the message said 'Unable to locate package', the mirror is" \
                "stale rather than the package missing. Run:" \
                "" \
                "  termux-change-repo" \
                "" \
                "pick a mirror in your region, then run this script again."
        fi
    else
        die "stopping, nothing was changed" \
            "Install them yourself with:  pkg install$PACKAGES"
    fi
else
    ok "git, rust and clang are present"
fi

RUST_VERSION=$(rustc --version 2>/dev/null | awk '{print $2}')
case "${RUST_VERSION:-}" in
    '')
        # Under --check with rust not installed there is nothing to read, and
        # that is not an error: the missing package was already reported.
        if [ "$DO_BUILD" -eq 0 ]; then
            info "rust is not installed, so its version cannot be checked"
        else
            die "rustc is installed but did not report a version" \
                "Try:  pkg reinstall rust"
        fi
        ;;
    *)
        # Split on dots and compare as integers. A regex over the whole version
        # string breaks the day the format gains a suffix.
        RUST_MAJOR=${RUST_VERSION%%.*}
        RUST_REST=${RUST_VERSION#*.}
        RUST_MINOR=${RUST_REST%%.*}
        case "$RUST_MAJOR$RUST_MINOR" in
            *[!0-9]*)
                warn "could not read the rust version from '$RUST_VERSION'"
                info "Continuing; the build will say if it is too old."
                ;;
            *)
                if [ "$RUST_MAJOR" -lt "$MIN_RUST_MAJOR" ] ||
                   { [ "$RUST_MAJOR" -eq "$MIN_RUST_MAJOR" ] &&
                     [ "$RUST_MINOR" -lt "$MIN_RUST_MINOR" ]; }; then
                    die "rust $RUST_VERSION is too old; $MIN_RUST_MAJOR.$MIN_RUST_MINOR is needed" \
                        "Termux's rust package lags the release channel by a" \
                        "few weeks at times. Try:" \
                        "" \
                        "  pkg upgrade rust" \
                        "" \
                        "and if it is still behind, there is nothing this" \
                        "script can do about it: rustup does not support" \
                        "Termux, so waiting for the package is the option."
                    READY=0
                else
                    ok "rust $RUST_VERSION"
                fi
                ;;
        esac
        ;;
esac

# ---------------------------------------------------------------- source

step "Finding the source"

SOURCE_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")/.." 2>/dev/null && pwd)
if [ -z "${SOURCE_DIR:-}" ] || [ ! -f "$SOURCE_DIR/Cargo.toml" ]; then
    die "this script is not inside an ITSaNAS checkout" \
        "Run it from the repository:" \
        "" \
        "  git clone https://github.com/SigSegGit/itsanas && cd itsanas" \
        "  sh install/android-termux.sh"
fi
ok "$SOURCE_DIR"

if [ "$DO_BUILD" -eq 0 ]; then
    step "Stopping here (--check)"
    if [ "$READY" -eq 1 ]; then
        ok "this phone can build ITSaNAS"
        exit 0
    fi
    warn "this phone is not ready yet; see what is missing above"
    exit 1
fi

# ---------------------------------------------------------------- build

step "Building"

info "This takes 10 to 40 minutes on a phone and gets warm."
info "Keep Termux in the foreground, or Android will stop it."
info "A wake-lock helps: pull down the Termux notification, tap ACQUIRE WAKELOCK."

# Only the command-line tool. Building the whole workspace on a phone doubles
# the time for crates nothing here runs.
if ! ( cd "$SOURCE_DIR" && cargo build --release -p itsanas-cli ); then
    warn "the build failed"
    info ""
    info "If it stopped without an error message, Android killed it: that is"
    info "what an out-of-memory kill looks like from inside Termux. Close other"
    info "apps and run it again — cargo resumes rather than starting over."
    info ""
    info "If it failed inside 'ring' or 'blake3', those are the two crates that"
    info "compile C here, and clang is what they need. Check:  pkg install clang"
    info ""
    info "Either way, the parts of ITSaNAS that have nothing to do with the"
    info "network can still be checked on this phone, which is most of what is"
    info "interesting about running it here:"
    info ""
    info "  cargo test -p itsanas-crypto -p itsanas-store -p itsanas-sync"
    exit 1
fi

BIN="$SOURCE_DIR/target/release/itsanas"
[ -x "$BIN" ] || die "the build reported success but produced no binary" \
    "Expected: $BIN"
ok "built"

VERSION_LINE=$("$BIN" --version 2>&1)
ok "$VERSION_LINE"

# ---------------------------------------------------------------- smoke

if [ "$DO_SMOKE" -eq 1 ]; then
    step "Storing a file and reading it back, on this phone"
    if ! sh "$SOURCE_DIR/scripts/smoke.sh" "$BIN"; then
        die "it built but did not work" \
            "This is the interesting kind of failure: the code compiles for" \
            "this processor and does not behave on it. Please report it with" \
            "the output above and the line from 'uname -a'."
    fi
fi

# ---------------------------------------------------------------- what next

step "Done"

cat <<'NEXT'
       You have the command-line tool, built for this phone.

         ./target/release/itsanas init --username <your-name>
         ./target/release/itsanas put notes/thought.txt ~/thought.txt
         ./target/release/itsanas ls

       To reach your other machines, Termux needs to see them:

         ./target/release/itsanas peer add <host:port>
         ./target/release/itsanas daemon

       Two things that will bite you, in order of how soon:

       1. Android stops Termux when it is not in front. Pull down the Termux
          notification and tap ACQUIRE WAKELOCK, and exempt Termux from
          battery optimisation in Settings. On Samsung, also remove it from
          "Sleeping apps" — One UI puts it there on its own, repeatedly.

       2. This cannot see your photos or your Documents folder. Run
          termux-setup-storage for a scoped view of shared storage. A real
          app would use the Storage Access Framework; there is no real app.

       What you have proved by getting here is that the ITSaNAS data path
       runs on ARM hardware. That was an open question until now, and it is
       the reason this script exists. It is not a phone client.
NEXT
