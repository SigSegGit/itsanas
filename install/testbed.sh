#!/usr/bin/env sh
# A test bed in one command per machine: Linux (Pi, VM) and macOS.
#
#   First machine, which creates the test account (an invitation from any
#   member: `itsanas invite` on a machine that already is one):
#
#     sh install/testbed.sh --coordinator HOST:PORT --coordinator-device ID --invite CODE
#
#   Every other machine, which joins it (asks for the 24 words, hidden):
#
#     sh install/testbed.sh --coordinator HOST:PORT --coordinator-device ID
#
#   A machine that stays at home must be reachable from outside, or a laptop
#   away from home finds nobody to sync with: forward a port on the box to it
#   and add --announce PUBLICNAME:PORT (the port as seen from outside).
#   Machines that move announce nothing; they call out.
#
#   Unattended (over SSH, no terminal): --phrase-file PATH reads the 24 words
#   from a file you made mode 600; the script never deletes your file.
#
#   Then, any time, on any of them -- changes nothing:
#
#     sh install/testbed.sh status
#
#   Remove the bed (a dry run; add --yes; --purge-account also archives
#   nothing and deletes the node, so think first):
#
#     sh install/testbed.sh --clean [--yes]
#
# Why this exists (HANDOVER §8 0r)
# --------------------------------
#
# Nicolas asked to *see* the network work on his machines, about 10 GB each,
# with every bit of preparation done by the script and an answer readable at a
# glance. The installers and provisioners already do each step; what was
# missing is the order, the defaults, the Mac, and a verdict. So this is a thin
# wrapper and never a fork: on Linux it calls `provision.sh`, on macOS
# `macos.sh` plus the same configuration commands `provision.sh` runs.
#
# The bed is its own account, `essai`, in its own instance (`~/.itsanas-essai`,
# folder `~/ITSaNAS-essai`). Real accounts on the machine are never touched:
# every command here names the instance. `--fresh` archives an earlier bed to
# `~/itsanas-archive-DATE/` -- moved, never deleted.
#
# Secrets: the passphrase is drawn at random once per machine and lives only
# in ~/.config/itsanas/essai.environment (mode 600), the file provision.sh
# writes and clean.sh removes -- on the Mac too, so one clean-up knows it. The 24 words are typed at a hidden
# prompt, go through a mode-600 temporary file, and are removed after use;
# they never appear on a command line, where every process could read them.
set -u

INSTANCE=essai
ACCOUNT=essai
PLEDGE=10G
KEEP=3G
FOLDER="$HOME/ITSaNAS-$INSTANCE"
NODE_HOME="$HOME/.itsanas-$INSTANCE"
BIN="$HOME/.local/bin/itsanas"
RAW="https://raw.githubusercontent.com/SigSegGit/itsanas/main/install"
# The family install/macos.sh uses (net.itsanas.daemon), so clean.sh finds it.
AGENT_LABEL="net.itsanas.$INSTANCE"
AGENT_PLIST="$HOME/Library/LaunchAgents/$AGENT_LABEL.plist"

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    C_OK=$(printf '\033[32m'); C_ERR=$(printf '\033[31m'); C_OFF=$(printf '\033[0m')
else
    C_OK=''; C_ERR=''; C_OFF=''
fi
step() { printf '\n==> %s\n' "$*"; }
die() {
    printf '\n%serror%s %s\n' "$C_ERR" "$C_OFF" "$1"
    shift
    for line in "$@"; do printf '       %s\n' "$line"; done
    exit 1
}
have() { command -v "$1" >/dev/null 2>&1; }

HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" 2>/dev/null && pwd)
# TESTBED_OS lets check-installers.sh take the Linux path from any host.
OS=${TESTBED_OS:-$(uname -s)}

# A sibling script when run from a checkout, the published one when piped.
sibling() {
    if [ -r "$HERE/$1" ]; then
        printf '%s\n' "$HERE/$1"
        return
    fi
    target="${TMPDIR:-/tmp}/itsanas-$1"
    curl -fsSL "$RAW/$1" -o "$target" || die "could not download $1" "Is the network up?"
    printf '%s\n' "$target"
}

# `ssh pi 'sh testbed.sh status'` has no XDG_RUNTIME_DIR, and without it
# `systemctl --user` cannot find the bus: a running daemon reads as stopped.
# Same fix as provision.sh.
if [ "$OS" != Darwin ] && [ -z "${XDG_RUNTIME_DIR:-}" ] && [ -d "/run/user/$(id -u)" ]; then
    XDG_RUNTIME_DIR="/run/user/$(id -u)"
    export XDG_RUNTIME_DIR
fi

# ------------------------------------------------------------------ verdict

daemon_running() {
    if [ "$OS" = Darwin ]; then
        launchctl print "gui/$(id -u)/$AGENT_LABEL" 2>/dev/null | grep -q 'state = running'
    else
        systemctl --user is-active "itsanas@$INSTANCE" >/dev/null 2>&1
    fi
}

status() {
    host=$(hostname 2>/dev/null | cut -d. -f1)
    printf '\nITSaNAS test bed on %s\n\n' "$host"
    failures=0

    if [ -x "$BIN" ]; then
        printf '  %s✅%s installed      %s\n' "$C_OK" "$C_OFF" "$("$BIN" --version 2>/dev/null)"
    else
        printf '  %s❌%s installed      no %s -- run the setup command again\n' "$C_ERR" "$C_OFF" "$BIN"
        failures=$((failures + 1))
    fi

    if [ -f "$NODE_HOME/keystore.bin" ]; then
        printf '  %s✅%s account        %s, node in %s\n' "$C_OK" "$C_OFF" "$ACCOUNT" "$NODE_HOME"
    else
        printf '  %s❌%s account        no node in %s -- run the setup command again\n' "$C_ERR" "$C_OFF" "$NODE_HOME"
        failures=$((failures + 1))
    fi

    if daemon_running; then
        printf '  %s✅%s daemon         running\n' "$C_OK" "$C_OFF"
    else
        printf '  %s❌%s daemon         stopped -- log: ' "$C_ERR" "$C_OFF"
        if [ "$OS" = Darwin ]; then
            printf '%s\n' "$NODE_HOME/daemon.log"
        else
            printf 'journalctl --user -u itsanas@%s -n 50\n' "$INSTANCE"
        fi
        failures=$((failures + 1))
    fi

    others=""
    count=0
    for file in "$FOLDER"/bonjour-depuis-*.txt; do
        [ -e "$file" ] || continue
        name=$(basename "$file" .txt)
        name=${name#bonjour-depuis-}
        [ "$name" = "$host" ] && continue
        others="$others $name"
        count=$((count + 1))
    done
    if [ "$count" -gt 0 ]; then
        printf '  %s✅%s other machines %s:%s\n' "$C_OK" "$C_OFF" "$count" "$others"
    else
        printf '  %s❌%s other machines none yet -- normal right after the first setup;\n' "$C_ERR" "$C_OFF"
        printf '                    wait a few minutes after another machine has run it\n'
        failures=$((failures + 1))
    fi

    # A 25-byte greeting proves a path exists; a whole 50 MB file proves it
    # carries data. A file still arriving does not count.
    big=0
    for file in "$FOLDER"/50Mo-depuis-*.bin; do
        [ -e "$file" ] || continue
        case "$file" in *"-depuis-$host.bin") continue ;; esac
        [ "$(wc -c < "$file" | tr -d ' ')" -eq 52428800 ] && big=$((big + 1))
    done
    if [ "$big" -gt 0 ]; then
        printf '  %s✅%s 50 MB files    %s complete from other machines\n' "$C_OK" "$C_OFF" "$big"
    else
        printf '  %s❌%s 50 MB files    none complete yet -- they follow the greetings\n' "$C_ERR" "$C_OFF"
        failures=$((failures + 1))
    fi

    printf '\n  Details: ITSANAS_HOME=%s %s doctor\n' "$NODE_HOME" "$BIN"
    printf '  Your test folder: %s\n\n' "$FOLDER"
    if [ "$failures" -eq 0 ]; then
        printf '  %s==> IT WORKS%s\n\n' "$C_OK" "$C_OFF"
        return 0
    fi
    printf '  %s==> NOT YET (%s line(s) above)%s\n\n' "$C_ERR" "$failures" "$C_OFF"
    return 1
}

# -------------------------------------------------------------------- args

COORDINATOR=""
COORDINATOR_DEVICE=""
INVITE=""
ANNOUNCE=""
PHRASE_ARG=""
FRESH=0
INSTALL=1
case "${1:-}" in
    status) status; exit $? ;;
    --clean) shift; exec sh "$(sibling clean.sh)" --instance "$INSTANCE" "$@" ;;
esac
while [ $# -gt 0 ]; do
    case "$1" in
        --coordinator) [ $# -ge 2 ] || die "--coordinator needs host:port"; COORDINATOR="$2"; shift 2 ;;
        --coordinator-device) [ $# -ge 2 ] || die "--coordinator-device needs an id"; COORDINATOR_DEVICE="$2"; shift 2 ;;
        --invite) [ $# -ge 2 ] || die "--invite needs a code"; INVITE="$2"; shift 2 ;;
        --phrase-file) [ $# -ge 2 ] || die "--phrase-file needs a path"; PHRASE_ARG="$2"; shift 2 ;;
        --announce) [ $# -ge 2 ] || die "--announce needs host:port"; ANNOUNCE="$2"; shift 2 ;;
        --fresh) FRESH=1; shift ;;
        --no-install) INSTALL=0; shift ;;
        # The whole comment header, so a line added to it never drops off --help.
        --help|-h) awk 'NR > 1 && !/^#/ { exit } NR > 1' "$0" 2>/dev/null; exit 0 ;;
        *) die "unknown option: $1" "Run with --help." ;;
    esac
done
[ -n "$COORDINATOR" ] && [ -n "$COORDINATOR_DEVICE" ] || die \
    "--coordinator and --coordinator-device are both needed" \
    "They are in your fleet notes. The coordinator's id is printed by" \
    "  itsanas-coordinator --identity      (on the coordinator)"

# ------------------------------------------------------------- earlier beds

stop_daemon() {
    if [ "$OS" = Darwin ]; then
        launchctl bootout "gui/$(id -u)/$AGENT_LABEL" 2>/dev/null || true
    elif have systemctl; then
        systemctl --user stop "itsanas@$INSTANCE" 2>/dev/null || true
    fi
}

if [ "$FRESH" -eq 1 ] && [ -e "$NODE_HOME" ]; then
    step "Archiving the earlier test bed"
    stop_daemon
    archive="$HOME/itsanas-archive-$(date +%Y-%m-%d-%H%M%S)"
    mkdir -p "$archive" && chmod 700 "$archive" || die "could not create $archive"
    mv "$NODE_HOME" "$archive/" || die "could not move $NODE_HOME"
    [ -e "$FOLDER" ] && mv "$FOLDER" "$archive/"
    secret="$HOME/.config/itsanas/$INSTANCE.environment"
    [ -e "$secret" ] && mv "$secret" "$archive/"
    printf '  moved to %s (delete it yourself once the new bed works)\n' "$archive"
fi

# --------------------------------------------------------------- passphrase

# Reused when the machine already has one, so a re-run does not lock the
# existing node out of its own keystore.
SECRET_FILE="$HOME/.config/itsanas/$INSTANCE.environment"
if [ -r "$SECRET_FILE" ]; then
    ITSANAS_PASSPHRASE=$(sed -n 's/^ITSANAS_PASSPHRASE=//p' "$SECRET_FILE" | head -1)
fi
if [ -z "${ITSANAS_PASSPHRASE:-}" ]; then
    [ -e "$NODE_HOME/keystore.bin" ] && die "a test node exists but its passphrase file is gone" \
        "Start over with --fresh (the old node is archived, not deleted)."
    ITSANAS_PASSPHRASE=$(od -An -tx1 -N24 /dev/urandom | tr -d ' \n')
fi
export ITSANAS_PASSPHRASE

# ------------------------------------------------------------ the 24 words

PHRASE_FILE=""
# Only the temporary file this script made: a --phrase-file is the caller's.
OWN_PHRASE=0
cleanup() { [ "$OWN_PHRASE" -eq 1 ] && rm -f "$PHRASE_FILE"; }
trap cleanup EXIT INT TERM
if [ -e "$NODE_HOME/keystore.bin" ] || [ -n "$INVITE" ]; then
    :
elif [ -n "$PHRASE_ARG" ]; then
    [ -r "$PHRASE_ARG" ] || die "cannot read $PHRASE_ARG"
    [ "$(wc -w < "$PHRASE_ARG" | tr -d ' ')" -eq 24 ] || die "$PHRASE_ARG does not hold 24 words"
    PHRASE_FILE="$PHRASE_ARG"
else
    step "Joining the test account"
    printf '  Paste the 24 words the first machine printed, then Enter (hidden):\n  '
    # stty acts on its stdin, and under `curl | sh` that is the pipe: without
    # </dev/tty the words were echoed in clear on Mandarine's Mac (2026-10-06).
    stty -echo </dev/tty 2>/dev/null
    read -r WORDS </dev/tty
    stty echo </dev/tty 2>/dev/null
    printf '\n'
    [ "$(printf '%s\n' "$WORDS" | wc -w | tr -d ' ')" -eq 24 ] || die "that is not 24 words" \
        "Copy all of them, in order, from the first machine's output."
    PHRASE_FILE=$(mktemp) || die "could not create a temporary file"
    OWN_PHRASE=1
    chmod 600 "$PHRASE_FILE"
    printf '%s\n' "$WORDS" > "$PHRASE_FILE"
    WORDS=""
fi

# ------------------------------------------------------------ setting it up

if [ "$OS" = Linux ]; then
    step "Installing and setting up (provision.sh)"
    set -- --instance "$INSTANCE" --username "$ACCOUNT" --pledge "$PLEDGE" --keep "$KEEP" \
        --folder "$FOLDER" --coordinator "$COORDINATOR" --coordinator-device "$COORDINATOR_DEVICE"
    [ -n "$INVITE" ] && set -- "$@" --invite "$INVITE"
    [ -n "$ANNOUNCE" ] && set -- "$@" --announce "$ANNOUNCE"
    [ -n "$PHRASE_FILE" ] && set -- "$@" --phrase-file "$PHRASE_FILE"
    [ "$INSTALL" -eq 1 ] || set -- "$@" --no-install
    sh "$(sibling provision.sh)" "$@" || die "provisioning failed" "Its output is above."
elif [ "$OS" = Darwin ]; then
    step "Installing (macos.sh)"
    if [ "$INSTALL" -eq 1 ]; then
        # macos.sh builds from a checkout and, unlike linux.sh, never clones one,
        # so under `curl | sh` it had nothing to build (Mandarine's Mac,
        # 2026-10-06). Use the checkout this script sits in, or keep one where
        # linux.sh keeps its own.
        if [ -f "$HERE/../Cargo.toml" ]; then
            SRC=$(CDPATH='' cd -- "$HERE/.." && pwd)
        else
            SRC="$HOME/.local/src/itsanas"
            if [ -d "$SRC/.git" ]; then
                git -C "$SRC" pull -q --ff-only || die "could not update $SRC" "Remove it and run this again."
            else
                mkdir -p "$(dirname "$SRC")"
                git clone -q --depth 1 https://github.com/SigSegGit/itsanas "$SRC" || die "could not clone the source"
            fi
        fi
        sh "$SRC/install/macos.sh" --source "$SRC" --yes --no-service --no-smoke \
            || die "the installer failed" "Its output is above."
    fi
    [ -x "$BIN" ] || die "no itsanas binary at $BIN after installing"

    step "Setting up the account"
    stop_daemon
    export ITSANAS_HOME="$NODE_HOME"
    if [ -f "$NODE_HOME/keystore.bin" ]; then
        printf '  a test node exists here; leaving it alone\n'
    elif [ -n "$PHRASE_FILE" ]; then
        "$BIN" login --username "$ACCOUNT" --phrase-file "$PHRASE_FILE" || die "could not restore the account" \
            "Check the 24 words; they must be the first machine's."
    else
        "$BIN" init --username "$ACCOUNT" || die "could not create the account"
        printf '\n  WRITE THOSE 24 WORDS DOWN: every other machine asks for them.\n'
    fi
    "$BIN" space --pledge "$PLEDGE" --keep "$KEEP" --apply || die "these numbers do not fit this disk" \
        "The reason is above. Free some space, or tell Nicolas's session to lower --pledge."
    mkdir -p "$FOLDER" && "$BIN" folder "$FOLDER" || die "could not set the synced folder"
    "$BIN" coordinator "$COORDINATOR" --device "$COORDINATOR_DEVICE" || die "could not set the coordinator"
    # Before register, which is what publishes the address.
    if [ -n "$ANNOUNCE" ]; then
        "$BIN" announce "$ANNOUNCE" || die "could not set the announced address to $ANNOUNCE"
    fi
    if [ -n "$INVITE" ]; then
        "$BIN" register --invite "$INVITE" || die "the coordinator refused this account" \
            "An invitation is good for one account and expires: ask for another."
    else
        "$BIN" register || die "the coordinator refused this device" "The reason is above."
    fi

    step "Starting the daemon (a LaunchAgent, loaded)"
    # 600 before the secret goes in: the window between writing and
    # restricting is exactly as long as the machine is slow.
    mkdir -p "$(dirname "$SECRET_FILE")"
    : > "$SECRET_FILE" && chmod 600 "$SECRET_FILE" || die "could not create $SECRET_FILE"
    printf 'ITSANAS_PASSPHRASE=%s\n' "$ITSANAS_PASSPHRASE" >> "$SECRET_FILE"
    mkdir -p "$(dirname "$AGENT_PLIST")"
    : > "$AGENT_PLIST" && chmod 600 "$AGENT_PLIST" || die "could not create $AGENT_PLIST"
    cat >> "$AGENT_PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>$AGENT_LABEL</string>
  <key>ProgramArguments</key><array><string>$BIN</string><string>daemon</string></array>
  <key>EnvironmentVariables</key><dict>
    <key>ITSANAS_HOME</key><string>$NODE_HOME</string>
    <key>ITSANAS_PASSPHRASE</key><string>$ITSANAS_PASSPHRASE</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>StandardOutPath</key><string>$NODE_HOME/daemon.log</string>
  <key>StandardErrorPath</key><string>$NODE_HOME/daemon.log</string>
</dict></plist>
PLIST
    launchctl bootstrap "gui/$(id -u)" "$AGENT_PLIST" || die "launchctl refused the agent" \
        "Try: launchctl bootstrap gui/$(id -u) $AGENT_PLIST"
else
    die "this script is for Linux and macOS" \
        "Windows: install\\testbed.ps1. Android: install the APK and restore the" \
        "account from the 24 words in the app (install/android.md)."
fi

# --------------------------------------------------------- something to see

step "Dropping this machine's files into the test folder"
host=$(hostname 2>/dev/null | cut -d. -f1)
mkdir -p "$FOLDER"
printf 'Bonjour depuis %s (%s), %s\n' "$host" "$OS" "$(date)" > "$FOLDER/bonjour-depuis-$host.txt"
if [ ! -e "$FOLDER/50Mo-depuis-$host.bin" ]; then
    dd if=/dev/urandom of="$FOLDER/50Mo-depuis-$host.bin" bs=1048576 count=50 2>/dev/null
fi
printf '  bonjour-depuis-%s.txt and 50Mo-depuis-%s.bin\n' "$host" "$host"
printf '  The other machines see them within a few minutes; this one shows theirs.\n'

sleep 5
status
exit 0
