#!/usr/bin/env bash
# The acceptance kit, run between throwaway nodes on one machine, where every
# phase is made to pass once and to fail once.
#
#   bash scripts/acceptance-local.sh [itsanas-binary] [coordinator-binary]
#
# Why this exists
# ---------------
#
# `scripts/acceptance.sh` is what Nicolas runs on the fleet, and a morning spent
# on four machines is too expensive to discover there that a check cannot pass,
# or -- worse -- cannot fail. So CI runs it here first, in two directions:
#
# * **the scenario**: B, D, E, F and G between three homes of one account and a
#   local coordinator, over real sockets, with no credential that exists
#   anywhere else. This is the laboratory version of the fleet runs. It proves
#   the kit and the mechanisms agree; it does not replace the fleet.
# * **the negative controls**: each check pointed at a situation that must be
#   refused -- a wrong hash, a canary sitting in the host's store, a deleted
#   file that is still there, an edit with no conflict copy, a count that
#   moved. A check that passes both ways is decoration, and this is where that
#   would show.
#
# Nothing here needs the network beyond 127.0.0.1, and every node, key and
# passphrase is generated for the run and deleted after it.

set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

BIN=${1:-target/debug/itsanas}
COORD=${2:-target/debug/itsanas-coordinator}
# Overridable so the bench itself can be sabotage-verified: point it at a copy
# of the kit whose verdicts always pass, and this script must go red.
KIT=${ACCEPTANCE_KIT:-scripts/acceptance.sh}
[ -x "$BIN" ] || [ -x "$BIN.exe" ] || { echo "no itsanas binary at $BIN (cargo build -p itsanas-cli)"; exit 2; }
[ -x "$COORD" ] || [ -x "$COORD.exe" ] || { echo "no coordinator binary at $COORD"; exit 2; }

WORK=$(mktemp -d)
export ITSANAS_RECEIPTS="$WORK/receipts"
export ITSANAS_BIN="$BIN"
PASSPHRASE=$(head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n')
CPORT=$((20000 + RANDOM % 5000))
PORT2=$((CPORT + 1))
failures=0
pids=()

cleanup() {
    for pid in "${pids[@]}"; do kill "$pid" 2>/dev/null; done
    wait 2>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT

say() { printf '\n== %s\n' "$*"; }
node() { local home=$1; shift; ITSANAS_HOME="$WORK/$home" ITSANAS_PASSPHRASE="$PASSPHRASE" "$BIN" "$@" </dev/null; }
must() { "$@" >"$WORK/last.log" 2>&1 || { cat "$WORK/last.log"; echo "setup step failed: $*"; exit 1; }; }
wait_port() {
    for _ in $(seq 150); do
        (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null && return 0
        sleep 0.2
    done
    return 1
}

# A phase that must pass, and a phase that must fail. Both count.
expect_pass() {
    local out
    if out=$(bash "$KIT" "$@" 2>&1); then printf '  ok    %s\n' "$out"
    else printf '  WRONG expected PASS: %s\n' "$out"; failures=$((failures + 1)); fi
    LAST=$out
}
expect_fail() {
    local out
    if out=$(bash "$KIT" "$@" 2>&1); then printf '  WRONG expected FAIL: %s\n' "$out"; failures=$((failures + 1))
    else printf '  ok    refused: %s\n' "$out"; fi
}

# Serve one node in the background for the length of one sync by another.
serving() {
    local home=$1 port=$2
    shift 2
    ITSANAS_HOME="$WORK/$home" ITSANAS_PASSPHRASE="$PASSPHRASE" "$BIN" serve --listen "127.0.0.1:$port" \
        </dev/null >>"$WORK/$home-serve.log" 2>&1 &
    local pid=$!
    pids+=("$pid")
    wait_port "$port" || { kill "$pid" 2>/dev/null; cat "$WORK/$home-serve.log"; echo "$home did not serve on $port"; exit 1; }
    "$@"
    local status=$?
    kill "$pid" 2>/dev/null
    wait "$pid" 2>/dev/null
    # The index lock is released when the process is gone, not when it is sent
    # a signal; the next command on that node needs it.
    for _ in $(seq 50); do (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null || break; sleep 0.1; done
    return $status
}
# One sync round, and the line it ends with: what was sent and what arrived is
# the first thing to read when a phase gives the wrong verdict.
round() {
    local from=$1 to=$2 port=$3 out
    if ! out=$(serving "$to" "$port" node "$from" sync "127.0.0.1:$port" 2>&1); then
        printf '%s\n' "$out"
        echo "sync $from -> $to failed"
        exit 1
    fi
    printf '        sync %s -> %s: %s\n' "$from" "$to" "$(printf '%s\n' "$out" | grep -E 'sent|received' | tail -1)"
}
listing() { printf '        %s holds: %s\n' "$1" "$(node "$1" ls 2>&1 | awk '{print $NF}' | tr '\n' ' ')"; }

say "setup: a coordinator and three machines of one account"
must "$COORD" --state "$WORK/coord" --identity
COORD_ID=$("$COORD" --state "$WORK/coord" --identity 2>&1 | grep -oE '[0-9a-f]{64}' | head -1)
"$COORD" --state "$WORK/coord" --listen "127.0.0.1:$CPORT" >"$WORK/coord.log" 2>&1 &
pids+=($!)
wait_port "$CPORT" || { cat "$WORK/coord.log"; exit 1; }

for m in m1 m2 m3; do mkdir -p "$WORK/folder-$m"; done
must node m1 init --username acceptance
must node m1 coordinator "127.0.0.1:$CPORT" --device "$COORD_ID"
must node m1 folder "$WORK/folder-m1"
must node m1 register --recovery

say "D: machines 2 and 3 restored from the passphrase alone"
for m in m2 m3; do
    must node "$m" login --username acceptance --from "127.0.0.1:$CPORT" --device "$COORD_ID"
    must node "$m" folder "$WORK/folder-$m"
done
# Test A's procedure pledges on every machine, and a host refuses to store past
# its pledge -- which is zero until somebody says otherwise.
for m in m1 m2 m3; do must node "$m" pledge 1G; done
d_file="$WORK/d-source.bin"
head -c 700000 /dev/urandom >"$d_file"
must node m1 put d/restored.bin "$d_file"
round m3 m1 "$PORT2"
d_sha=$(sha256sum "$d_file" | cut -d' ' -f1)
ITSANAS_HOME="$WORK/m3" ITSANAS_PASSPHRASE="$PASSPHRASE" expect_pass D check d/restored.bin "$d_sha"
ITSANAS_HOME="$WORK/m3" ITSANAS_PASSPHRASE="$PASSPHRASE" expect_fail D check d/restored.bin "$(printf '0%.0s' $(seq 64))"
ITSANAS_HOME="$WORK/m3" ITSANAS_PASSPHRASE="$PASSPHRASE" expect_fail D check d/never-stored.bin "$d_sha"

say "B: a file written on machine 1 appears on machine 2"
expect_pass B write "$WORK/folder-m1"
b_name=$(printf '%s\n' "$LAST" | awk '{print $4}')
b_sha=$(printf '%s\n' "$LAST" | awk '{print $6}')
must node m1 scan
round m2 m1 "$PORT2"
must node m2 scan
expect_pass B check "$WORK/folder-m2" "$b_name" "$b_sha"
expect_fail B check "$WORK/folder-m2" "$b_name" "$(printf 'f%.0s' $(seq 64))"
expect_fail B check "$WORK/folder-m2" "no-such-file.bin" "$b_sha"

say "E: machine 3 gets a file from machine 2, never meeting machine 1 again"
expect_pass E write "$WORK/folder-m1"
e_name=$(printf '%s\n' "$LAST" | awk '{print $4}')
e_sha=$(printf '%s\n' "$LAST" | awk '{print $6}')
must node m1 scan
round m1 m2 "$PORT2"
listing m2
# Machine 1 is off from here: nothing below starts it.
round m3 m2 "$PORT2"
listing m3
must node m3 scan
expect_pass E check "$WORK/folder-m3" "$e_name" "$e_sha"

say "F: a delete on machine 1 reaches machine 3 through machine 2, and stays"
# F is only a test if the file was on machine 3 to begin with. Without this a
# relay that never delivered anything reads as a delete that worked.
if [ ! -f "$WORK/folder-m3/$e_name" ]; then
    echo "  WRONG $e_name never reached machine 3, so F below would prove nothing"
    failures=$((failures + 1))
fi
expect_pass F delete "$WORK/folder-m1" "$e_name"
must node m1 scan
round m1 m2 "$PORT2"
round m3 m2 "$PORT2"
must node m3 scan
expect_pass F check "$WORK/folder-m3" "$e_name"
round m3 m2 "$PORT2"
must node m3 scan
expect_pass F check "$WORK/folder-m3" "$e_name"
expect_fail F check "$WORK/folder-m3" "$b_name"
[ -f "$WORK/folder-m3/$b_name" ] || { echo "  WRONG the delete took $b_name with it"; failures=$((failures + 1)); }

say "G: machines 1 and 3 edit the same file apart, then meet through machine 2"
g_name="notes-g.txt"
printf 'the original\n' >"$WORK/folder-m1/$g_name"
must node m1 scan
round m1 m2 "$PORT2"
round m3 m2 "$PORT2"
must node m3 scan
expect_fail G check "$WORK/folder-m3" "$g_name"
expect_pass G edit "$WORK/folder-m1" "$g_name" from-machine-1
expect_pass G edit "$WORK/folder-m3" "$g_name" from-machine-3
must node m1 scan
must node m3 scan
round m1 m2 "$PORT2"
round m3 m2 "$PORT2"
round m1 m2 "$PORT2"
must node m1 scan
must node m3 scan
expect_pass G check "$WORK/folder-m1" "$g_name"
g1=$(printf '%s\n' "$LAST" | grep -oE 'digest [0-9a-f]+')
expect_pass G check "$WORK/folder-m3" "$g_name"
g3=$(printf '%s\n' "$LAST" | grep -oE 'digest [0-9a-f]+')
if [ -n "$g1" ] && [ "$g1" = "$g3" ]; then echo "  ok    both machines agree: $g1"
else echo "  WRONG machines disagree on the conflict: '$g1' against '$g3'"; failures=$((failures + 1)); fi

say "C: the scan finds a canary where one is, and a control it cannot skip"
expect_pass C plant "$WORK/folder-m1"
# Once: the canary is the file's name too, so the plant line carries it twice.
canary=$(printf '%s\n' "$LAST" | grep -oE 'ITSANAS-CANARY-[0-9a-f]+' | head -1)
mkdir -p "$WORK/fake-host"
cp "$WORK/folder-m1/$canary.txt" "$WORK/fake-host/"
expect_fail C scan "$canary" "$WORK/fake-host"
must node m1 scan
# Deliberately not scanned: the owner's own state. Its index holds file names in
# the clear -- the first version of this bench asserted otherwise and the scan
# found the canary in store/index.redb, which is correct on the owner's machine
# and is exactly why test C is run on a host of another account. That needs a
# second account and cross-account hosting, which this bench does not set up;
# locally C has its negative control and nothing more.
expect_fail C scan "$canary" "$WORK/m1"

say "I and J: the checks refuse what they must"
printf 'round 1: received 0 files\n' >"$WORK/daemon-up.log"
expect_fail I check "$WORK/daemon-up.log"
printf 'coordinator: unreachable (refused)\n' >"$WORK/daemon-down.log"
expect_fail I check "$WORK/daemon-down.log"
printf 'coordinator: unreachable (refused)\n127.0.0.1:1: sent 0 B (0 chunks, 0 segments), received 1 files, 0 conflicts\n' >>"$WORK/daemon-down2.log"
expect_pass I check "$WORK/daemon-down2.log"
expect_pass J count "$WORK/folder-m3"
j_count=$(printf '%s\n' "$LAST" | awk '{print $4}')
ITSANAS_HOME="$WORK/m3" ITSANAS_PASSPHRASE="$PASSPHRASE" expect_pass J check "$WORK/folder-m3" "$j_count"
ITSANAS_HOME="$WORK/m3" ITSANAS_PASSPHRASE="$PASSPHRASE" expect_fail J check "$WORK/folder-m3" "$((j_count + 1))"

echo
if [ "$failures" -eq 0 ]; then
    echo "acceptance-local: every phase passed where it must and failed where it must"
else
    echo "acceptance-local: $failures phases gave the wrong verdict"
fi
[ "$failures" -eq 0 ]
