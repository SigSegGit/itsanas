#!/bin/sh
# The one thing the fleet CI key may run on a machine of the fleet.
#
#   authorized_keys:  restrict,command="$HOME/.itsanas-fleet/agent.sh" ssh-ed25519 AAAA... itsanas-fleet-ci
#   from the runner:  ssh pi B write
#
# Why this exists
# ---------------
#
# The hand tests on the fleet took Nicolas a day and were an ordeal for the
# partner testing with him (2026-10-22). Everything in them that needs no hand
# on a cable is run from CI instead (docs/FLEET-CI.md), on the real machines,
# over SSH. A key that CI holds is a key that can leak, so it opens no shell:
# `restrict,command=` makes sshd run this script whatever the caller asked,
# with the request in SSH_ORIGINAL_COMMAND, and this script does one of a
# fixed list of things with it.
#
# What it does is `scripts/acceptance.sh`, the kit Nicolas runs by hand, and
# nothing of its own: a second definition of "B passes" would drift from the
# first (Rodin, 2026-10-23). It adds the allowlist, the scoping, and one read
# the kit has no phase for: a node's status snapshot.
#
# What a leaked key can do, and nothing more
# ------------------------------------------
#
# * write a file of about a mebibyte into the `test` account's folder (`B
#   write`, `C plant`), at most MAX_FILES of them, then refused;
# * delete one of the files it wrote (`F delete`: names this agent made only);
# * read: whether a file is there and its digest, whether a canary is in what
#   another account's node holds for `test` (its vault's directory for that
#   owner and its databases, never the rest of its vault), and a node's
#   status snapshot;
# * one request at a time: a second one while one runs is refused, so a
#   leaked key cannot pile scans onto a Pi that also runs other services.
#
# It never runs a command built from the request, never reads a path from it,
# and never touches an account other than `test` except to scan it read-only.
# No sudo, no docker: the containers run as this user (fleet-docker.sh), so
# their homes and folders are this user's files.
#
# Configuration: agent.conf beside this script, written by setup.sh and owned
# by this user. It is sourced, so it is trusted like this script is.

set -u

HERE=$(dirname "$0")
CONF="${ITSANAS_FLEET_CONF:-$HERE/agent.conf}"
# The test account's folder, the kit, and one HOME_<account> per account on
# this machine (where `C scan` looks and where the snapshot is).
FOLDER=""
KIT="$HERE/acceptance.sh"
ACCOUNTS=""
MAX_FILES=50
RECEIPTS="$HERE/receipts"
TEST_UID=""
[ -r "$CONF" ] || { echo "REFUSED no configuration at $CONF"; exit 64; }
# shellcheck disable=SC1090
. "$CONF"

refuse() { echo "REFUSED $*"; exit 64; }

[ -n "$FOLDER" ] && [ -d "$FOLDER" ] || refuse "the test folder '$FOLDER' does not exist"
[ -r "$KIT" ] || refuse "no acceptance kit at $KIT"

# One request at a time. flock is util-linux, on every Linux this runs on;
# without it the lock is skipped rather than the agent refused.
if command -v flock >/dev/null 2>&1; then
    mkdir -p "$RECEIPTS" 2>/dev/null
    exec 9>"$RECEIPTS/agent.lock"
    flock -n 9 || refuse "another request is running on this machine"
fi

# Every token: short, and nothing a shell, a path or an option could use.
# Checked before anything is split further, so a request is either all
# plain words or refused whole.
REQUEST="${SSH_ORIGINAL_COMMAND:-}"
[ ${#REQUEST} -le 256 ] || refuse "request longer than 256 characters"
case "$REQUEST" in
    *[!A-Za-z0-9._:\ -]*) refuse "a character outside [A-Za-z0-9._: -]" ;;
esac
# shellcheck disable=SC2086
set -- $REQUEST
[ $# -gt 0 ] || refuse "an empty request (try hello)"
for token in "$@"; do
    case "$token" in
        -*) refuse "a token starting with '-'" ;;
        *..*) refuse "a token containing '..'" ;;
    esac
    [ ${#token} -le 64 ] || refuse "a token longer than 64 characters"
done

is_sha() { printf '%s' "$1" | grep -Eq '^[0-9a-f]{64}$'; }
# The names the kit itself makes (acceptance.sh write_phase, c_plant); a
# request can name nothing else, so it can neither read nor delete a file
# somebody put in the folder.
is_kit_name() {
    printf '%s' "$1" | grep -Eq '^acceptance-[A-Z]-[0-9]{8}T[0-9]{6}Z-[0-9]+\.bin$' ||
        printf '%s' "$1" | grep -Eq '^ITSANAS-CANARY-[0-9a-f]{24}\.txt$'
}
is_code() { printf '%s' "$1" | grep -Eq '^[0-9a-f]{4}-?[0-9a-f]{4}$'; }
is_canary() { printf '%s' "$1" | grep -Eq '^ITSANAS-CANARY-[0-9a-f]{24}$'; }
account_home() {
    # Names from agent.conf, compared whole: the request never reaches eval.
    for a in $ACCOUNTS; do
        printf '%s' "$a" | grep -Eq '^[a-z0-9_]+$' || continue
        if [ "$a" = "$1" ]; then
            eval "printf '%s' \"\${HOME_$a:-}\""
            return 0
        fi
    done
    return 1
}
# VAULT_<account> in agent.conf, for a vault moved off the home (`vault --move`).
vault_of() { eval "printf '%s' \"\${VAULT_$1:-}\""; }
kit_files() {
    find "$FOLDER" -maxdepth 1 -type f \( -name 'acceptance-*.bin' -o -name 'ITSANAS-CANARY-*.txt' \) | wc -l | tr -d ' '
}
room_for_one_more() {
    [ "$(kit_files)" -lt "$MAX_FILES" ] ||
        refuse "already $MAX_FILES files from the kit in the test folder; F delete some"
}
kit() { ITSANAS_RECEIPTS="$RECEIPTS" bash "$KIT" "$@"; }

case "$*" in
    hello)
        printf 'hello %s agent 1 accounts %s kit %s\n' "$(uname -n)" "$ACCOUNTS" \
            "$(sha256sum "$KIT" | cut -c1-16)"
        ;;
    "B write" | "E write")
        room_for_one_more
        kit "$1" write "$FOLDER"
        ;;
    "C plant")
        room_for_one_more
        kit C plant "$FOLDER"
        ;;
    *)
        verb="$1 ${2:-}"
        case "$verb" in
            "B check" | "E check")
                if [ $# -eq 3 ] && is_code "$3"; then
                    kit "$1" check "$FOLDER" "$3"
                    exit
                fi
                [ $# -eq 4 ] && is_kit_name "$3" && is_sha "$4" || refuse "usage: $1 check <kit file> <sha256> | <code>"
                kit "$1" check "$FOLDER" "$3" "$4"
                ;;
            "F delete" | "F check")
                [ $# -eq 3 ] && is_kit_name "$3" || refuse "usage: F $2 <kit file>"
                kit F "$2" "$FOLDER" "$3"
                ;;
            "C scan")
                [ $# -eq 4 ] && is_canary "$3" || refuse "usage: C scan <canary> <account>"
                [ "$4" != test ] || refuse "the test account's own home holds its file names in the clear; scan another account"
                home=$(account_home "$4") || refuse "no account '$4' on this machine"
                [ -d "$home" ] || refuse "the home of '$4' is not at $home"
                printf '%s' "$TEST_UID" | grep -Eq '^[0-9a-f]{64}$' ||
                    refuse "agent.conf has no TEST_UID (the test account's user id); run setup.sh again"
                vault=$(vault_of "$4")
                vault=${vault:-$home/vault}
                # What this node holds for `test`: the chunks in its vault's
                # directory for that owner. Not the rest of the vault, which
                # on the Pi is a terabyte of other people's chunks to read
                # every night for nothing.
                held="$vault/owners/$TEST_UID"
                if [ ! -d "$held" ] || [ -z "$(find -H "$held" -type f | head -1)" ]; then
                    # Why, as far as that account's daemon says: a zero
                    # pledge and a host `test` never met read the same
                    # otherwise (C SKIP every night until 2026-10-27).
                    snapshot="$home/status.snapshot"
                    if [ -r "$snapshot" ]; then
                        # Another account's file, and the line ends up in the
                        # run's public summary: letters, digits, dots and
                        # spaces only, and short.
                        pledged=$(sed -n 's/^  pledged  *//p' "$snapshot" | head -1 | tr -cd '0-9A-Za-z. ' | cut -c1-16)
                        hosted=$(sed -n 's/^  peers hosted  *//p' "$snapshot" | head -1 | tr -cd '0-9' | cut -c1-6)
                        why="its daemon says pledged ${pledged:-unknown}, ${hosted:-unknown} peer(s) hosted"
                    else
                        why="no status.snapshot: its daemon has not run here"
                    fi
                    echo "NOTHOSTED $4 holds nothing for test in $vault: there is nothing here to read ($why)"
                    exit 3
                fi
                kit C scan "$3" "$held" || exit 1
                # And its databases (its own index, the vault's), which must
                # not have learnt the name either.
                for db in "$home"/*.redb "$vault"/*.redb; do
                    [ -f "$db" ] || continue
                    # -z: lines end at NUL, which a database is full of, so
                    # grep never holds a run of a binary file between two
                    # newlines -- megabytes of it -- as one line.
                    if grep -a -z -F -q -- "$3" "$db"; then
                        echo "FAIL  C scan    the canary is readable in $db"
                        exit 1
                    fi
                done
                ;;
            "status "*)
                [ $# -eq 2 ] || refuse "usage: status <account>"
                home=$(account_home "$2") || refuse "no account '$2' on this machine"
                snapshot="$home/status.snapshot"
                [ -r "$snapshot" ] || { echo "NOSNAPSHOT $2 has no status.snapshot: its daemon has not run here"; exit 1; }
                taken=$(sed -n '1s/^snapshot \([0-9]*\) every \([0-9]*\).*/\1 \2/p' "$snapshot")
                printf 'snapshot %s now %s\n' "$taken" "$(date +%s)"
                # The running daemon's version, from the release after 0.1.12 on.
                ver=$(sed -n '1s/.* version \([0-9A-Za-z.+-]*\).*/\1/p' "$snapshot")
                printf 'version %s\n' "${ver:-unknown}"
                grep -E '^  (username|device|files|live chunks)' "$snapshot"
                ;;
            *)
                refuse "unknown request '$*' (hello, B|E write|check, C plant|scan, F delete|check, status)"
                ;;
        esac
        ;;
esac
