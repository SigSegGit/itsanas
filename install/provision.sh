#!/usr/bin/env sh
# Take a machine with nothing on it to a running ITSaNAS node, in one command.
#
#   curl -fsSL https://raw.githubusercontent.com/SigSegGit/itsanas/main/install/provision.sh |
#     ITSANAS_PASSPHRASE='...' sh -s -- --username nicolas --pledge 100G
#
# What this is, and why it is not `linux.sh`
# ------------------------------------------
#
# `install/linux.sh` compiles and installs. That is all it does, deliberately:
# it touches no keys, creates no account, and writes no secret anywhere. Getting
# from "installed" to "a node that is actually a member of something" was five
# more commands typed by hand, in an order nobody had written down, and half of
# them needed values from another machine.
#
# This is that order, written down and made repeatable. It is the artefact you
# keep when you rebuild the machine: run it again on a fresh install and you get
# the same node back.
#
# **It handles the passphrase, and that is the whole reason it is separate.** A
# daemon cannot be prompted, so the passphrase has to be put somewhere systemd
# can read it. That is a real trade — anything running as you can read that file
# — and it belongs in a script you read before running, not buried in an
# installer's last step.
#
# Why not a container
# -------------------
#
# The question comes up because a Pi carrying several services invites the crash
# that takes the rest down with it, and because a rebuild should be one command.
# The first is answered by cgroups, which the systemd units already set. The
# second is answered by this file.
#
# What a container would add is a prebuilt image — and there is not one, so on
# aarch64 you would build it on the Pi, which costs exactly what building the
# binary costs. You would pay `--network host` (discovery is a UDP broadcast a
# bridge does not carry) and a bind mount (redb memory-maps its index) for a
# benefit that does not arrive until somebody publishes images. `docs/PORTING.md`
# §3b has the long form and the conditions that would reverse it.
#
# Same constraints as the other scripts here: POSIX sh, no `set -e`, idempotent.

set -u

VERSION="1.0"
INSTALLER="https://raw.githubusercontent.com/SigSegGit/itsanas/main/install/linux.sh"

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
have() { command -v "$1" >/dev/null 2>&1; }

USERNAME=""
PHRASE_FILE=""
COORDINATOR=""
COORDINATOR_DEVICE=""
INVITE=""
PLEDGE=""
FOLDER=""
PEER=""
DO_SERVICE=1
SERVICE_OK=0
DO_INSTALL=1

usage() {
    cat <<'USAGE'
Provision an ITSaNAS node

  ITSANAS_PASSPHRASE='...' sh install/provision.sh --username NAME [options]

The account
  --username NAME        the account this machine belongs to (required)
  --phrase-file PATH     restore an existing account from its 24 words
                         instead of creating a new one

The network
  --coordinator HOST:PORT        where members find each other
  --coordinator-device ID        the coordinator's device id, which you pin
  --invite CODE                  an invitation, if the coordinator needs one
  --peer HOST:PORT               a peer to sync with directly (repeatable)

This machine
  --pledge SIZE          space offered to other members, e.g. 100G
  --folder PATH          the directory kept in step with the account
  --no-service           do not enable the systemd user unit
  --no-install           the binary is already here; only configure
  --help                 this

The passphrase comes from ITSANAS_PASSPHRASE and nowhere else. It unlocks this
machine's keystore, and the service needs it at every start, so it is written to
~/.config/itsanas/environment with mode 600. Anything running as you can read
that file. That is the trade a background service makes; it is not a default
this script picked for you.

Run it twice and the second run changes nothing.
USAGE
}

while [ $# -gt 0 ]; do
    case "$1" in
        --username) [ $# -ge 2 ] || die "--username needs a name"; USERNAME="$2"; shift 2 ;;
        --username=*) USERNAME="${1#--username=}"; shift ;;
        --phrase-file) [ $# -ge 2 ] || die "--phrase-file needs a path"; PHRASE_FILE="$2"; shift 2 ;;
        --phrase-file=*) PHRASE_FILE="${1#--phrase-file=}"; shift ;;
        --coordinator) [ $# -ge 2 ] || die "--coordinator needs host:port"; COORDINATOR="$2"; shift 2 ;;
        --coordinator=*) COORDINATOR="${1#--coordinator=}"; shift ;;
        --coordinator-device) [ $# -ge 2 ] || die "--coordinator-device needs an id"; COORDINATOR_DEVICE="$2"; shift 2 ;;
        --coordinator-device=*) COORDINATOR_DEVICE="${1#--coordinator-device=}"; shift ;;
        --invite) [ $# -ge 2 ] || die "--invite needs a code"; INVITE="$2"; shift 2 ;;
        --invite=*) INVITE="${1#--invite=}"; shift ;;
        --pledge) [ $# -ge 2 ] || die "--pledge needs a size"; PLEDGE="$2"; shift 2 ;;
        --pledge=*) PLEDGE="${1#--pledge=}"; shift ;;
        --folder) [ $# -ge 2 ] || die "--folder needs a path"; FOLDER="$2"; shift 2 ;;
        --folder=*) FOLDER="${1#--folder=}"; shift ;;
        --peer) [ $# -ge 2 ] || die "--peer needs host:port"; PEER="$PEER $2"; shift 2 ;;
        --peer=*) PEER="$PEER ${1#--peer=}"; shift ;;
        --no-service) DO_SERVICE=0; shift ;;
        --no-install) DO_INSTALL=0; shift ;;
        --help|-h) usage; exit 0 ;;
        *) die "unknown option: $1" "Run with --help for the list." ;;
    esac
done

printf '%sITSaNAS provisioning %s%s\n' "$C_DIM" "$VERSION" "$C_OFF"

# ------------------------------------------------------------- what is needed

step "Checking what was asked for"

[ -n "$USERNAME" ] || die "--username is required" \
    "It is the account this machine belongs to. On the first machine it is a" \
    "name you choose; on the second it is the same name, with --phrase-file." \
    "" \
    "  sh install/provision.sh --username nicolas --pledge 100G"

# The passphrase is checked here rather than at the moment it is needed, because
# the moment it is needed is after a forty-minute build.
[ -n "${ITSANAS_PASSPHRASE:-}" ] || die "ITSANAS_PASSPHRASE is not set" \
    "The keystore on this machine is sealed under it, and the service needs it" \
    "at every start. Provide it in the environment:" \
    "" \
    "  ITSANAS_PASSPHRASE='...' sh install/provision.sh --username $USERNAME" \
    "" \
    "Use the same one you used on your other machines only if you want to; the" \
    "passphrase is per-machine and protects this machine's copy of the keys."
ok "passphrase supplied"

if [ -n "$PHRASE_FILE" ]; then
    [ -r "$PHRASE_FILE" ] || die "cannot read $PHRASE_FILE" \
        "It should hold the 24 recovery words, separated by spaces."
    words=$(tr -s '[:space:]' ' ' < "$PHRASE_FILE" | wc -w)
    [ "$words" -eq 24 ] || die "$PHRASE_FILE has $words words, not 24" \
        "A recovery phrase is exactly twenty-four words."
    ok "recovery phrase: 24 words, so this machine joins an existing account"
else
    ok "no phrase given, so this machine creates the account"
fi

if [ -n "$COORDINATOR" ] && [ -z "$COORDINATOR_DEVICE" ]; then
    warn "a coordinator without --coordinator-device is not pinned"
    info "A coordinator hands out addresses and is never trusted to say who"
    info "lives at one. Without the device id, this node will believe whatever"
    info "answers at that address. Get it from the coordinator's operator."
fi

# ------------------------------------------------------------------- install

BIN="$HOME/.local/bin/itsanas"

if [ "$DO_INSTALL" -eq 1 ]; then
    step "Installing"
    if [ -x "$BIN" ]; then
        ok "$($BIN --version 2>/dev/null) is already here"
        info "Re-run with --no-install to skip this check entirely."
    fi
    if have curl; then
        curl -fsSL "$INSTALLER" | sh -s -- --yes --no-smoke \
            || die "the installer failed" "Its output is above."
    elif have wget; then
        wget -q -O - "$INSTALLER" | sh -s -- --yes --no-smoke \
            || die "the installer failed" "Its output is above."
    else
        die "neither curl nor wget is here" \
            "Install one, or run install/linux.sh from a checkout and then" \
            "re-run this with --no-install."
    fi
fi

[ -x "$BIN" ] || die "no itsanas binary at $BIN" \
    "The install step did not produce one. Its output is above."
ok "$($BIN --version 2>/dev/null)"

# -------------------------------------------------------------- the account

step "The account"

# `init` refuses to overwrite an existing node, and that refusal is the thing
# that makes this script safe to re-run: it will not destroy a master secret
# because somebody repeated a command.
if $BIN status >/dev/null 2>&1; then
    ok "a node already exists here; leaving it alone"
elif [ -n "$PHRASE_FILE" ]; then
    $BIN login --username "$USERNAME" --phrase-file "$PHRASE_FILE" \
        || die "could not restore the account" \
               "The phrase and the username have to match the ones used when" \
               "the account was created."
    ok "restored $USERNAME on this machine"
else
    PHRASE_OUT=$(mktemp)
    if $BIN init --username "$USERNAME" > "$PHRASE_OUT" 2>&1; then
        cat "$PHRASE_OUT"
        printf '\n'
        warn "WRITE THOSE TWENTY-FOUR WORDS DOWN, ON PAPER, NOW"
        info "They are the only way to recover this account on a new machine."
        info "They are not stored anywhere else and cannot be reissued."
        rm -f "$PHRASE_OUT"
    else
        cat "$PHRASE_OUT"
        rm -f "$PHRASE_OUT"
        die "could not create the account" "The output is above."
    fi
fi

# ------------------------------------------------------------ configuration

step "Configuring this machine"

if [ -n "$PLEDGE" ]; then
    $BIN pledge "$PLEDGE" || die "could not set the pledge to $PLEDGE"
else
    warn "no --pledge, so this node offers nothing and hosts nobody"
    info "A node that pledges nothing is a client, not a member."
fi

if [ -n "$FOLDER" ]; then
    mkdir -p "$FOLDER" || die "could not create $FOLDER"
    $BIN folder "$FOLDER" || die "could not set the synced folder"
fi

for address in $PEER; do
    $BIN peer add "$address" || warn "could not add the peer $address"
done

if [ -n "$COORDINATOR" ]; then
    if [ -n "$COORDINATOR_DEVICE" ]; then
        $BIN coordinator "$COORDINATOR" --device "$COORDINATOR_DEVICE" \
            || die "could not set the coordinator"
    else
        $BIN coordinator "$COORDINATOR" || die "could not set the coordinator"
    fi

    if [ -n "$INVITE" ]; then
        $BIN register --invite "$INVITE" || die "the coordinator refused this account" \
            "An invitation is good for one account and expires. Ask the member" \
            "who issued it for another:  itsanas invite"
    else
        $BIN register || die "the coordinator refused this account" \
            "If it admits by invitation, you need a code from a member:" \
            "  itsanas invite      (on a machine that is already a member)" \
            "then re-run this with --invite <code>."
    fi
fi

# ----------------------------------------------------------------- service

if [ "$DO_SERVICE" -eq 1 ]; then
    step "The service"

    if ! have systemctl; then
        warn "no systemd here; start the daemon by hand with: itsanas daemon"
    else
        ENV_DIR="$HOME/.config/itsanas"
        mkdir -p "$ENV_DIR" || die "could not create $ENV_DIR"

        # 600 before the secret goes in, not after. Writing it first and
        # chmodding second leaves a window in which it is world-readable, and
        # the window is exactly as long as the machine is slow.
        ENV_FILE="$ENV_DIR/environment"
        : > "$ENV_FILE" || die "could not create $ENV_FILE"
        chmod 600 "$ENV_FILE" || die "could not restrict $ENV_FILE"
        printf 'ITSANAS_PASSPHRASE=%s\n' "$ITSANAS_PASSPHRASE" >> "$ENV_FILE"
        ok "$ENV_FILE (mode 600)"

        # The unit itself comes from install/linux.sh. It used to be told
        # --no-service here, on the reasoning that the service must not start
        # before the passphrase is in place. That reasoning was wrong in a way
        # that made this whole branch dead code: linux.sh *writes* the unit and
        # reloads systemd, and never enables or starts anything. So the flag
        # suppressed the only step that creates the file, this branch found no
        # unit on every run, and the summary at the bottom still told the reader
        # to run `journalctl --user -u itsanas`. Found by reading the Pi after a
        # green run -- the script reported success and the unit was not there.
        UNIT="$HOME/.config/systemd/user/itsanas.service"
        if [ -f "$UNIT" ]; then
            ok "the unit is already installed"
        else
            warn "no unit at $UNIT"
            info "Run install/linux.sh without --no-service to write it, then"
            info "re-run this with --no-install."
        fi

        if [ -f "$UNIT" ]; then
            systemctl --user daemon-reload 2>/dev/null
            if systemctl --user enable --now itsanas 2>/dev/null; then
                ok "itsanas.service is enabled and running"
                SERVICE_OK=1
            else
                warn "could not enable the service"
                info "Look at:  systemctl --user status itsanas"
            fi

            # Without lingering, a user service stops when the last session
            # ends. On a headless machine that means it runs only while
            # somebody is logged in over ssh, which is the opposite of what a
            # storage node is for.
            if have loginctl; then
                if loginctl show-user "$(id -un)" 2>/dev/null | grep -q '^Linger=yes'; then
                    ok "lingering is on: it keeps running with nobody logged in"
                else
                    warn "lingering is off, so the node stops when you log out"
                    info "  sudo loginctl enable-linger $(id -un)"
                fi
            fi
        fi
    fi
fi

# -------------------------------------------------------------------- check

step "Does it work here?"

HERE=$(CDPATH='' cd -- "$(dirname -- "$0")" 2>/dev/null && pwd)
SMOKE=""
for candidate in "$HERE/../scripts/smoke.sh" "$HOME/.local/src/itsanas/scripts/smoke.sh"; do
    [ -r "$candidate" ] && SMOKE="$candidate" && break
done

if [ -n "$SMOKE" ]; then
    sh "$SMOKE" "$BIN" || die "it installed and it does not work" \
        "The output above says which step. This is the interesting kind of" \
        "failure and is worth reporting."
else
    # The smoke script needs a home of its own, so falling back to --version is
    # a real loss rather than an equivalent check. Say so.
    warn "scripts/smoke.sh was not found; only checked that the binary runs"
    info "It is in the checkout at ~/.local/src/itsanas."
fi

# --------------------------------------------------------------------- next

step "Done"

$BIN status 2>/dev/null | head -12

# What this prints is what the machine is, not what the script meant to do.
# The first version printed the journalctl line unconditionally, on a machine
# where the unit had never been written -- a summary that reports intent is a
# summary that lies on exactly the runs you needed it not to.
if [ "$SERVICE_OK" -eq 1 ]; then
    cat <<NEXT

       Watch it:      journalctl --user -u itsanas -f
       Stop it:       systemctl --user stop itsanas
       Ask it:        itsanas status

       To rebuild this machine, keep the command you just ran. That is the
       whole point of this script: it is the artefact, not the machine.
NEXT
else
    cat <<NEXT

       There is no background service on this machine. Run the daemon by
       hand when you want the node up:

           itsanas daemon

       Ask it:        itsanas status

       To rebuild this machine, keep the command you just ran. That is the
       whole point of this script: it is the artefact, not the machine.
NEXT
fi
