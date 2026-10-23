#!/usr/bin/env bash
# What an idle node writes to its disk per sync round, measured.
#
#   bash scripts/idle-writes.sh            (after cargo build --release)
#   R=12 INT=10 bash scripts/idle-writes.sh
#
# A coordinator and two accounts on this machine, each node hosting the
# other, nothing changing: the block-device bytes each daemon wrote
# (/proc/PID/io write_bytes) over R rounds, divided by R. Linux only.
#
# Why this exists: ROADMAP, "An idle node's disk writes". 313 MB a day on
# the Pi for a node with nothing to do was found by sampling the fleet, and
# every cut since was measured, not supposed -- a copy-on-write engine
# charges by the commit, and which commit is idle is not visible from the
# code. 2026-10-23: 218 KB a round on this bench before, about 100 after
# (an idle round's no-op writes removed). STRACE=1 adds which files.
set -u
BIN=${BIN:-target/release/itsanas}
COORDB=${COORDB:-target/release/itsanas-coordinator}
set -u
cd "$(dirname "$0")/.." || exit 2
BIN=${BIN:-target/release/itsanas}
COORDB=${COORDB:-target/release/itsanas-coordinator}
[ -x "$BIN" ] && [ -x "$COORDB" ] || { echo "build first: cargo build --release -p itsanas-cli -p itsanas-coordinator"; exit 2; }
[ -r /proc/self/io ] || { echo "needs Linux (/proc/PID/io)"; exit 2; }
INT=${INT:-10}
R=${R:-6}
W=$(mktemp -d)
trap 'kill $(jobs -p) 2>/dev/null; wait 2>/dev/null; rm -rf "$W"' EXIT
export ITSANAS_PASSPHRASE=idle-writes-bench-only
CP=$((30000 + RANDOM % 2000))
"$COORDB" --state "$W/c" --identity >/dev/null 2>&1
CID=$("$COORDB" --state "$W/c" --identity 2>&1 | grep -oE '[0-9a-f]{64}' | head -1)
"$COORDB" --state "$W/c" --listen "127.0.0.1:$CP" >"$W/c.log" 2>&1 &
sleep 1
n() { local h=$1; shift; ITSANAS_HOME="$W/$h" "$BIN" "$@" </dev/null; }
port() { echo $((CP + $1)); }
for i in 1 2; do
    mkdir -p "$W/f$i"
    n "h$i" init --username "acct$i" >/dev/null 2>&1 || { echo "init failed"; exit 1; }
    n "h$i" coordinator "127.0.0.1:$CP" --device "$CID" >/dev/null
    n "h$i" folder "$W/f$i" >/dev/null
    n "h$i" pledge 1G >/dev/null 2>&1
    n "h$i" listen "127.0.0.1:$(port "$i")" >/dev/null 2>&1
    n "h$i" register >/dev/null 2>&1
    head -c 300000 /dev/urandom >"$W/f$i/data.bin"
done
n h1 peer add "127.0.0.1:$(port 2)" >/dev/null 2>&1
n h2 peer add "127.0.0.1:$(port 1)" >/dev/null 2>&1
pids=()
for i in 1 2; do
    ITSANAS_HOME="$W/h$i" "$BIN" daemon --interval "$INT" --no-discovery >"$W/d$i.log" 2>&1 &
    pids+=($!)
done
# Long enough for the first exchange to settle: what is measured is idle.
sleep $((INT * 4))
written() { awk '/^write_bytes/{print $2}' "/proc/$1/io"; }
a1=$(written "${pids[0]}")
a2=$(written "${pids[1]}")
if [ -n "${STRACE:-}" ] && command -v strace >/dev/null 2>&1; then
    timeout $((INT * R)) strace -f -qq -y -e trace=write,pwrite64,writev,fsync,fdatasync \
        -o "$W/trace" -p "${pids[0]}" 2>/dev/null
    echo "node 1, bytes written by file over $R rounds:"
    sed -nE 's/.*<([^>]*)>.*= ([0-9]+)$/\1 \2/p' "$W/trace" | sed "s|$W/||" |
        awk '{b[$1]+=$2} END {for (f in b) printf "  %8d KB  %s\n", b[f]/1024, f}' | sort -rn
else
    sleep $((INT * R))
fi
b1=$(written "${pids[0]}")
b2=$(written "${pids[1]}")
echo "idle, $R rounds of ${INT}s: node 1 $(((b1 - a1) / R / 1024)) KB/round, node 2 $(((b2 - a2) / R / 1024)) KB/round"
