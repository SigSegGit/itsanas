#!/bin/sh
#
# Remove everything an ITSaNAS install put on this machine.
#
#   sh install/clean.sh              # show what would go, change nothing
#   sh install/clean.sh --yes        # remove the programs and the service
#   sh install/clean.sh --yes --purge-account   # and the account itself
#   sudo sh install/clean.sh --yes --purge-coordinator  # a coordinator host
#
# Why this exists
# ---------------
#
# Three scripts install things here — `linux.sh` and `macos.sh` put programs in
# place, `provision.sh` creates the account and the service — and until now
# nothing took them away. Uninstalling meant remembering six paths, two of them
# written by a script the person may never have read, and one of them a file
# holding a passphrase.
#
# An install that cannot be undone is not an install anybody should trust with
# a disk. It is also, in practice, how a machine ends up with two versions of
# the same daemon and a unit pointing at a binary that is no longer there.
#
# What it will not do without being asked twice
# ---------------------------------------------
#
# **The account.** `keystore.bin` holds this machine's sealed copy of the master
# secret, and the store holds the only copy of anything not yet replicated
# elsewhere. Deleting them is not "uninstalling a program", it is losing data,
# so it takes a separate flag and it says what it is about to do first.
#
# The dry run is the default for the same reason: the first thing anybody does
# with an unfamiliar clean-up script is run it to see what it says.
set -eu

DO_IT=0
PURGE=0
PURGE_COORD=0
PREFIX="${ITSANAS_PREFIX:-$HOME/.local}"
NODE_HOME="${ITSANAS_HOME:-$HOME/.itsanas}"

say()  { printf '%s\n' "$*"; }
plan() { printf '  %s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }

usage() {
    sed -n '3,9p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
}

while [ $# -gt 0 ]; do
    case "$1" in
        --yes|-y) DO_IT=1; shift ;;
        --purge-account) PURGE=1; shift ;;
        --purge-coordinator) PURGE_COORD=1; shift ;;
        --prefix) PREFIX="$2"; shift 2 ;;
        --prefix=*) PREFIX="${1#--prefix=}"; shift ;;
        --home) NODE_HOME="$2"; shift 2 ;;
        --home=*) NODE_HOME="${1#--home=}"; shift ;;
        -h|--help) usage ;;
        *) say "unknown option: $1"; exit 2 ;;
    esac
done

UNIT="$HOME/.config/systemd/user/itsanas.service"
COORD_UNIT="$HOME/.config/systemd/user/itsanas-coordinator.service"
PLIST="$HOME/Library/LaunchAgents/fr.ngas.itsanas.plist"

say "ITSaNAS clean-up"
say ""

# ------------------------------------------------------------------ the service

RUNNING=0
if command -v systemctl >/dev/null 2>&1 && systemctl --user is-active itsanas >/dev/null 2>&1; then
    RUNNING=1
fi
if command -v launchctl >/dev/null 2>&1 && launchctl list 2>/dev/null | grep -q 'fr\.ngas\.itsanas'; then
    RUNNING=1
fi

say "the service"
[ "$RUNNING" -eq 1 ] && plan "stop and disable it (it is running now)" || plan "not running"
[ -f "$UNIT" ] && plan "remove $UNIT"
[ -f "$COORD_UNIT" ] && plan "remove $COORD_UNIT"
[ -f "$PLIST" ] && plan "remove $PLIST"

# ----------------------------------------------------------------- the programs

say ""
say "the programs"
for name in itsanas itsanas-coordinator itsanas-drive; do
    [ -f "$PREFIX/bin/$name" ] && plan "remove $PREFIX/bin/$name"
done
[ -d "$HOME/src-itsanas" ] && plan "leave the source checkout at $HOME/src-itsanas (not ours to remove)"

# ------------------------------------------------------------------- the secret

say ""
say "the passphrase"
FOUND_SECRET=0
for secret in "$HOME/.itsanas-passphrase" "$NODE_HOME/passphrase"; do
    if [ -f "$secret" ]; then
        plan "remove $secret"
        FOUND_SECRET=1
    fi
done
[ "$FOUND_SECRET" -eq 0 ] && plan "none found"

# ------------------------------------------------------------------ the account

say ""
say "the account"
if [ -d "$NODE_HOME" ]; then
    if [ "$PURGE" -eq 1 ]; then
        plan "REMOVE $NODE_HOME — the sealed master secret and every chunk on this machine"
        plan "anything here and nowhere else is gone for good"
    else
        plan "keep $NODE_HOME (pass --purge-account to remove it)"
    fi
else
    plan "no node at $NODE_HOME"
fi

# --------------------------------------------------------------- the coordinator
#
# `coordinator.sh` installs somewhere else entirely: a **system** unit in /etc,
# a binary in /usr/local/bin, a system user, and state in /var/lib. None of that
# is under $HOME, so the member clean-up above walks straight past it.
#
# This was found by reading `coordinator.sh` after writing the rest of this
# script, not by running it — which means the first version of this file would
# have reported "everything removed" on a machine still running a coordinator
# as a system service. That is the exact shape of failure this project keeps
# producing: a claim of completeness over a list assembled from memory.
#
# The state directory is the member directory. Removing it does not destroy
# anybody's files, but it does mean members who have not pinned each other
# cannot find each other again, so it takes its own flag and takes the system
# user with it — a user without its files is an orphan uid on the disk.

COORD_UNIT_SYS="/etc/systemd/system/itsanas-coordinator.service"
COORD_BIN="/usr/local/bin/itsanas-coordinator"
COORD_STATE="/var/lib/itsanas-coordinator"
COORD_USER="itsanas-coord"

COORD_FOUND=0
[ -f "$COORD_UNIT_SYS" ] && COORD_FOUND=1
[ -f "$COORD_BIN" ] && COORD_FOUND=1
[ -d "$COORD_STATE" ] && COORD_FOUND=1

say ""
say "the coordinator"
if [ "$COORD_FOUND" -eq 0 ]; then
    plan "no coordinator installed here"
else
    [ -f "$COORD_UNIT_SYS" ] && plan "stop and remove $COORD_UNIT_SYS"
    [ -f "$COORD_BIN" ] && plan "remove $COORD_BIN"
    if [ "$PURGE_COORD" -eq 1 ]; then
        [ -d "$COORD_STATE" ] && plan "REMOVE $COORD_STATE - the member directory"
        plan "remove the system user $COORD_USER"
    else
        [ -d "$COORD_STATE" ] && plan "keep $COORD_STATE (pass --purge-coordinator to remove it)"
    fi
    # Saying it now, while nothing has happened yet, rather than failing on the
    # first rm and leaving a half-removed service.
    if [ "$(id -u)" -ne 0 ]; then
        plan "NOTE: this needs root. Re-run with sudo, or the coordinator stays."
    fi
fi

if [ "$DO_IT" -eq 0 ]; then
    say ""
    say "Nothing was changed. Add --yes to do it."
    exit 0
fi

say ""

# The service first. Removing a binary out from under a running daemon leaves a
# process with a deleted executable, which restarts into nothing and reports it
# in a journal nobody is reading.
if command -v systemctl >/dev/null 2>&1; then
    systemctl --user disable --now itsanas >/dev/null 2>&1 || true
    systemctl --user disable --now itsanas-coordinator >/dev/null 2>&1 || true
fi
if command -v launchctl >/dev/null 2>&1 && [ -f "$PLIST" ]; then
    launchctl unload "$PLIST" >/dev/null 2>&1 || true
fi

rm -f "$UNIT" "$COORD_UNIT" "$PLIST"
if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload >/dev/null 2>&1 || true
fi
say "service removed"

for name in itsanas itsanas-coordinator itsanas-drive; do
    rm -f "$PREFIX/bin/$name"
done
say "programs removed from $PREFIX/bin"

for secret in "$HOME/.itsanas-passphrase" "$NODE_HOME/passphrase"; do
    [ -f "$secret" ] && rm -f "$secret"
done
say "passphrase files removed"

if [ "$PURGE" -eq 1 ] && [ -d "$NODE_HOME" ]; then
    rm -rf "$NODE_HOME"
    say "account removed from $NODE_HOME"
elif [ -d "$NODE_HOME" ]; then
    say "account left at $NODE_HOME"
fi

if [ "$COORD_FOUND" -eq 1 ]; then
    if [ "$(id -u)" -ne 0 ]; then
        warn "not root: the coordinator was left alone"
        say "  Re-run with sudo to remove $COORD_UNIT_SYS and $COORD_BIN."
    else
        if command -v systemctl >/dev/null 2>&1; then
            systemctl disable --now itsanas-coordinator >/dev/null 2>&1 || true
        fi
        rm -f "$COORD_UNIT_SYS" "$COORD_BIN"
        if command -v systemctl >/dev/null 2>&1; then
            systemctl daemon-reload >/dev/null 2>&1 || true
        fi
        say "coordinator service and binary removed"

        if [ "$PURGE_COORD" -eq 1 ]; then
            rm -rf "$COORD_STATE"
            # The user last, and only with its files: a system user whose
            # directory is gone is tidy, a directory whose owner is gone is an
            # orphan uid that the next system user to be created inherits.
            if command -v userdel >/dev/null 2>&1; then
                userdel "$COORD_USER" >/dev/null 2>&1 || true
            elif command -v deluser >/dev/null 2>&1; then
                deluser --system "$COORD_USER" >/dev/null 2>&1 || true
            fi
            say "coordinator state and system user removed"
        elif [ -d "$COORD_STATE" ]; then
            say "coordinator state left at $COORD_STATE"
        fi
    fi
fi

# A daemon that is gone is still a holder in somebody else's ledger until they
# notice. Saying so is the difference between "I uninstalled it" and "my
# friend's replica count silently dropped".
say ""
say "Note: other members still count this machine as holding their data until"
say "their next audit withdraws it. If this was a host for somebody, tell them."
