#!/usr/bin/env bash
# What an idle node costs per sync round, measured: disk writes, CPU time
# (a phone's or a laptop's battery), loopback bytes (the data plan) and the
# resident memory it keeps (a leak).
#
#   bash scripts/idle-writes.sh            (after cargo build --release)
#   R=12 INT=10 bash scripts/idle-writes.sh
#   CHECK=1 BIN=target/debug/itsanas COORDB=target/debug/itsanas-coordinator \
#       INT=2 R=40 bash scripts/idle-writes.sh     (CI: fails over a budget)
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
cd "$(dirname "$0")/.." || exit 2
BIN=${BIN:-target/release/itsanas}
COORDB=${COORDB:-target/release/itsanas-coordinator}
[ -x "$BIN" ] && [ -x "$COORDB" ] || { echo "no $BIN or $COORDB: build them first (cargo build --release -p itsanas-cli -p itsanas-coordinator)"; exit 2; }
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
# utime + stime, in clock ticks: what the process cost the CPU.
ticks() { awk '{print $14 + $15}' "/proc/$1/stat"; }
resident() { awk '/^VmRSS/{print $2 * 1024}' "/proc/$1/status"; }
# Bytes through the loopback interface, both ways: this bench's traffic, the
# coordinator's included (nothing else on a CI runner talks on lo).
looped() { awk '/^ *lo:/{print $2 + $10}' /proc/net/dev; }
a1=$(written "${pids[0]}")
a2=$(written "${pids[1]}")
t1=$(ticks "${pids[0]}")
t2=$(ticks "${pids[1]}")
m1=$(resident "${pids[0]}")
m2=$(resident "${pids[1]}")
l0=$(looped)
if [ -n "${STRACE:-}" ] && command -v strace >/dev/null 2>&1; then
    timeout $((INT * R)) strace -f -qq -y -e trace=write,pwrite64,writev,fsync,fdatasync \
        -o "$W/trace" -p "${pids[0]}" 2>/dev/null
    echo "node 1, bytes written by file over $R rounds:"
    sed -nE 's/.*<([^>]*)>.*= ([0-9]+)$/\1 \2/p' "$W/trace" | sed "s|$W/||" |
        awk '{b[$1]+=$2} END {for (f in b) printf "  %8d KB  %s\n", b[f]/1024, f}' | sort -rn
else
    sleep $((INT * R))
fi
for pid in "${pids[@]}"; do
    kill -0 "$pid" 2>/dev/null || { echo "FAIL  a daemon stopped while idle"; tail -n 20 "$W"/d*.log; exit 1; }
done
b1=$(written "${pids[0]}")
b2=$(written "${pids[1]}")
hz=$(getconf CLK_TCK)
cpu1=$((($(ticks "${pids[0]}") - t1) * 1000 / hz / R))
cpu2=$((($(ticks "${pids[1]}") - t2) * 1000 / hz / R))
grew1=$(($(resident "${pids[0]}") - m1))
grew2=$(($(resident "${pids[1]}") - m2))
net=$((($(looped) - l0) / R))
disk1=$(((b1 - a1) / R / 1024))
disk2=$(((b2 - a2) / R / 1024))
echo "idle, $R rounds of ${INT}s:"
echo "  disk    node 1 ${disk1} KB/round, node 2 ${disk2} KB/round"
echo "  cpu     node 1 ${cpu1} ms/round, node 2 ${cpu2} ms/round"
echo "  memory  node 1 grew $((grew1 / 1024)) KB, node 2 $((grew2 / 1024)) KB over the $R rounds"
echo "  network $((net / 1024)) KB/round on loopback, both nodes and the coordinator"

[ -n "${CHECK:-}" ] || exit 0
# Budgets for the debug build CI runs, measured 2026-11-23 under WSL (31 KB of
# disk and 13 ms of CPU a round, 24-36 KB of memory over 40 rounds) and set
# about three times over: a regression, not noise, is what crosses them. A
# leak of 8 KB a round crosses MEMORY_KB within R=40 (32 KB a round, planted,
# kept 928 KB).
DISK_KB=${DISK_KB:-100}
CPU_MS=${CPU_MS:-60}
MEMORY_KB=${MEMORY_KB:-256}
# Measured on CI's ubuntu-24.04 (#443): 56 KB a round, every byte counted
# twice on lo (sent and received). WSL counts nothing there.
NET_KB=${NET_KB:-160}
failed=0
over() {
    if [ "$1" -gt "$2" ]; then
        echo "FAIL  $3: $1 over the budget of $2"
        failed=1
    fi
}
over "$disk1" "$DISK_KB" "disk KB a round, node 1 (an SD card's life)"
over "$disk2" "$DISK_KB" "disk KB a round, node 2"
over "$cpu1" "$CPU_MS" "CPU ms a round, node 1 (battery)"
over "$cpu2" "$CPU_MS" "CPU ms a round, node 2"
over "$((grew1 / 1024))" "$MEMORY_KB" "KB of memory kept over $R idle rounds, node 1 (a leak)"
over "$((grew2 / 1024))" "$MEMORY_KB" "KB of memory kept, node 2"
# WSL mirrors loopback to Windows and counts none of it on `lo`: a counter
# that did not move measured nothing, which is said, and is a failure where
# REQUIRE_NET asks for the number (CI, a real Linux).
if [ "$net" -eq 0 ]; then
    echo "SKIP  network: loopback counted nothing here"
    [ -z "${REQUIRE_NET:-}" ] || { echo "FAIL  network not measured, and REQUIRE_NET is set"; failed=1; }
else
    over "$((net / 1024))" "$NET_KB" "network KB a round (a data plan)"
fi
[ "$failed" = 0 ] && echo "PASS  an idle node stays inside its disk, CPU, memory and network budgets"
exit "$failed"
