#!/usr/bin/env bash
# The CI runner as a member of the `test` account, for the length of one
# fleet run: a machine on a network that is not home, reachable by nobody,
# with no local discovery to make things easy.
#
#   bash scripts/fleet/member.sh start     # restore or log in, daemon, agent.conf
#   bash scripts/fleet/member.sh stop      # stop the daemon, seal the home for next time
#
# Why this exists
# ---------------
#
# The Pi and the VM are one house on one LAN, and MVP.md §2 says what that is
# worth: four machines in one house cannot show that this works. Test O -- a
# file written somewhere that is not home reaches home -- was a person on a
# train. A GitHub runner is a VM in a datacentre behind NAT, on a new address
# every run, that reaches the account only through the coordinator, the
# public forwards and the relays (Rodin, 2026-10-23). As the first host of
# run.sh it is the writer, so B and F become O.
#
# One device, kept from night to night
# ------------------------------------
#
# The member is the same device every night: its home and folder are sealed
# at `stop` (AES-256 under FLEET_HOME_KEY, a secret of the `fleet`
# environment) into FLEET_STATE, which the workflow keeps in the Actions
# cache, and opened at `start`. So each night is also a machine coming back
# after a day away, which is the laptop's life.
#
# Not a new device per run: a host keeps the claim of every device that ever
# presented one, withdrawn or not, so a device a night would grow every
# host's table by one a night for nothing (and, until the sixteen claims a
# host hands out became the sixteen presented last, pushed live devices out
# of the roster). And not the same device with an empty store: it would hand
# out sequence numbers its chain already used.
#
# When there is no archive (the first run, a cache evicted after a week
# unused, no FLEET_HOME_KEY), the member logs in from the 24 words as a new
# device and withdraws the one before it, whose id is kept in FLEET_STATE
# next to the archive. A device cannot withdraw itself (`withdraw_device`).
#
# What the secrets are: FLEET_TEST_PHRASE is the `test` account itself, and
# the archive holds that account's keys under a passphrase kept inside it. The
# account must hold test data and nothing else.
#
# Environment
# -----------
#
#   FLEET_TEST_PHRASE   the 24 words of the test account (needed without an archive)
#   FLEET_HOME_KEY      the key the archive is sealed with (none: a new device each run)
#   FLEET_COORD         host:port of the coordinator
#   FLEET_COORD_ID      the device id it must prove
#   FLEET_STATE         where the archive and the device id live between runs
#                       (default .fleet-member, which the workflow caches)
#   ITSANAS_BIN         the itsanas binary (default target/release/itsanas)
#   FLEET_MEMBER        the member's working directory (default $RUNNER_TEMP/fleet-member)
#   FLEET_MEMBER_LISTEN where its daemon listens (default 0.0.0.0:9797)

set -uo pipefail
cd "$(dirname "$0")/../.." || exit 2

BIN=${ITSANAS_BIN:-target/release/itsanas}
DIR=${FLEET_MEMBER:-${RUNNER_TEMP:-/tmp}/fleet-member}
STATE=${FLEET_STATE:-.fleet-member}
ARCHIVE="$STATE/home.enc"
DEVICES="$STATE/devices"
HOME_DIR="$DIR/home"
FOLDER="$DIR/folder"

die() { echo "fleet member: $*" >&2; exit 1; }
say() { echo "fleet member: $*"; }
node() { ITSANAS_HOME="$HOME_DIR" ITSANAS_PASSPHRASE="$(cat "$DIR/passphrase")" "$BIN" "$@" </dev/null; }
seal() { openssl enc -aes-256-cbc -pbkdf2 -iter 200000 -salt -pass env:FLEET_HOME_KEY "$@"; }

# The device's id, from the snapshot ("device   <id>") or from what `login`
# said ("device   : <id>").
device_of() {
    sed -n 's/^ *device *:\{0,1\} *\([0-9a-f]\{64\}\).*/\1/p' "$@" 2>/dev/null | head -1
}

restore() {
    [ -n "${FLEET_HOME_KEY:-}" ] && [ -s "$ARCHIVE" ] || return 1
    mkdir -p "$DIR"
    chmod 700 "$DIR"
    if ! seal -d -in "$ARCHIVE" 2>"$DIR/restore.err" | tar -xz -C "$DIR" 2>>"$DIR/restore.err"; then
        say "the archive did not open ($(head -1 "$DIR/restore.err")); starting as a new device"
        rm -rf "$HOME_DIR" "$FOLDER" "$DIR/passphrase"
        return 1
    fi
    if [ ! -s "$DIR/passphrase" ] || [ ! -d "$HOME_DIR" ]; then
        rm -rf "$HOME_DIR" "$FOLDER" "$DIR/passphrase"
        return 1
    fi
    say "the same device as last run, back after $(( ($(date +%s) - $(stat -c %Y "$ARCHIVE")) / 60 )) minutes"
}

fresh() {
    [ -n "${FLEET_TEST_PHRASE:-}" ] || die "no archive to restore and FLEET_TEST_PHRASE is empty: no member this run"
    mkdir -p "$DIR" "$FOLDER"
    chmod 700 "$DIR"
    (umask 077; head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n' >"$DIR/passphrase")
    (umask 077; printf '%s\n' "$FLEET_TEST_PHRASE" >"$DIR/phrase")
    node login --username test --phrase-file "$DIR/phrase" >"$DIR/login.log" 2>&1 ||
        { rm -f "$DIR/phrase"; cat "$DIR/login.log"; die "login failed"; }
    rm -f "$DIR/phrase"
    node coordinator "$FLEET_COORD" --device "$FLEET_COORD_ID" >/dev/null || die "coordinator"
    node register >"$DIR/register.log" 2>&1 || { cat "$DIR/register.log"; die "register"; }
    me=$(device_of "$DIR/login.log")
    [ -n "$me" ] || { cat "$DIR/login.log"; die "login did not say which device it made"; }
    say "a new device, ${me:0:12}"
    # The device of the run before, withdrawn now that a device of the account
    # is here to do it. Said, never fatal: an id already forgotten is no
    # reason to skip the night's tests.
    if [ -s "$DEVICES" ]; then
        while read -r old; do
            [[ "$old" =~ ^[0-9a-f]{64}$ ]] && [ "$old" != "$me" ] || continue
            if node device forget "$old" >/dev/null 2>&1; then
                say "withdrew the device of an earlier run, ${old:0:12}"
            else
                say "could not withdraw ${old:0:12} (already gone?)"
            fi
        done <"$DEVICES"
    fi
    mkdir -p "$STATE"
    printf '%s\n' "$me" >"$DEVICES"
    [ -n "${FLEET_HOME_KEY:-}" ] ||
        say "no FLEET_HOME_KEY: this device is not kept, and the next run is a new one"
}

start() {
    : "${FLEET_COORD:?FLEET_COORD is the coordinator, host:port}"
    : "${FLEET_COORD_ID:?FLEET_COORD_ID is the device id the coordinator must prove}"
    [ -x "$BIN" ] || die "no itsanas binary at $BIN"
    [ ! -e "$HOME_DIR" ] || die "$HOME_DIR exists: one member per run"
    restore || fresh
    mkdir -p "$FOLDER"
    # The runner's temporary directory need not be the same path every night.
    node folder "$FOLDER" >/dev/null || die "folder"

    rm -f "$HOME_DIR/status.snapshot"
    ITSANAS_HOME="$HOME_DIR" ITSANAS_PASSPHRASE="$(cat "$DIR/passphrase")" \
        nohup "$BIN" daemon --interval 30 --listen "${FLEET_MEMBER_LISTEN:-0.0.0.0:9797}" \
        </dev/null >"$DIR/daemon.log" 2>&1 &
    echo $! >"$DIR/daemon.pid"
    for _ in $(seq 120); do
        [ -s "$HOME_DIR/status.snapshot" ] && break
        sleep 1
    done
    [ -s "$HOME_DIR/status.snapshot" ] || { tail -30 "$DIR/daemon.log"; die "the daemon took no round in two minutes"; }

    cat >"$DIR/agent.conf" <<EOF
FOLDER="$FOLDER"
KIT="$PWD/scripts/acceptance.sh"
RECEIPTS="$DIR/receipts"
ACCOUNTS="test"
HOME_test="$HOME_DIR"
EOF
    say "up as $(device_of "$HOME_DIR/status.snapshot" | cut -c1-12); FLEET_AGENT_runner=$DIR/agent.conf"
    [ -z "${GITHUB_ENV:-}" ] || echo "FLEET_AGENT_runner=$DIR/agent.conf" >>"$GITHUB_ENV"
}

stop() {
    if [ -s "$DIR/daemon.pid" ]; then
        pid=$(cat "$DIR/daemon.pid")
        kill "$pid" 2>/dev/null
        # The store is closed when the process is gone, not when it is told.
        for _ in $(seq 30); do kill -0 "$pid" 2>/dev/null || break; sleep 1; done
        echo "fleet member: the daemon's last lines:"
        tail -40 "$DIR/daemon.log" 2>/dev/null | sed 's/^/  /'
    fi
    if [ -n "${FLEET_HOME_KEY:-}" ] && [ -d "$HOME_DIR" ] && [ -s "$DIR/passphrase" ]; then
        mkdir -p "$STATE"
        # Every night writes a mebibyte and deletes it; without this the
        # archive, and the cache it lives in, grows by that much a night.
        node gc >/dev/null 2>&1 || say "gc failed; sealing as it is"
        if tar -cz -C "$DIR" home folder passphrase | seal -out "$ARCHIVE.new" && mv "$ARCHIVE.new" "$ARCHIVE"; then
            say "sealed for the next run ($(du -h "$ARCHIVE" | cut -f1))"
        else
            say "could not seal the home; the next run starts as a new device"
            rm -f "$ARCHIVE.new"
        fi
    fi
    rm -f "$DIR/passphrase"
}

case "${1:-}" in
    start) start ;;
    stop) stop ;;
    *) echo "usage: member.sh start|stop"; exit 2 ;;
esac
