#!/usr/bin/env bash
# The hand test of docs/IOS.md, steps 1 to 4, 8 and 9, on an iPhone simulator.
#
# Starts what the hand test needs around the phone, on this Mac: a coordinator
# on a scratch directory, and a computer of the account (a node made with the
# command line, `register --recovery`, holding from-computer.txt and
# to-trash.txt, served by `itsanas daemon`). Then the UI tests
# (ios/ITSaNASUITests) drive the app on the simulator, logged in to that
# account; and last, what step 3 says of the computer is checked on it: the
# file deleted for good on the phone is gone, the other is still there.
# Everything started is stopped, failing or not.
#
# The app is the debug build: only its core takes the tests' coordinator
# (itsanas_use_coordinator_for_tests), and a run without one fails saying so,
# before any name reaches the official coordinator.
#
#   bash scripts/iphone-e2e.sh <simulator id>                 # every step
#   bash scripts/iphone-e2e.sh <simulator id> HandTest/test9_signingOutLocksTheAppUntilThePassphrase
#
# Called by `scripts/build-ios.sh test` (and `ci`), after the core and the
# project are built. Output, in $E2E_OUT (default target/ios-e2e): the test
# results (result.xcresult, with a screenshot of every failure), the logs of
# the coordinator and the computer.
#
# Steps 6 and 7 (iOS choosing when a background round runs) and 10 (Windows
# Hello) are not here: no simulator decides like a phone on its charger.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
simulator="${1:?the simulator to run on, by its id (xcrun simctl list devices)}"
only="${2:-}"
port="${E2E_PORT:-9898}"
desk_port=$((port + 1))
out="${E2E_OUT:-$here/target/ios-e2e}"
target="${CARGO_TARGET_DIR:-$here/target}"

# Test values, used only against this run's coordinator.
username="e2eiphone"
passphrase="e2e iphone passphrase"

work="$(mktemp -d)"
pids=""
cleanup() {
    for pid in $pids; do kill "$pid" 2>/dev/null || true; done
    for pid in $pids; do wait "$pid" 2>/dev/null || true; done
    rm -rf "$work"
}
trap cleanup EXIT

wait_port() {
    for _ in $(seq 150); do
        (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null && return 0
        sleep 0.2
    done
    return 1
}

rm -rf "$out"
mkdir -p "$out"
cd "$here"

echo "== the coordinator and a computer, on this Mac"
cargo build --locked -p itsanas-cli -p itsanas-coordinator
coordinator="$target/debug/itsanas-coordinator"
itsanas="$target/debug/itsanas"
device="$("$coordinator" --state "$work/coord" --identity 2>&1 | grep -oE '[0-9a-f]{64}' | head -n 1)"
"$coordinator" --state "$work/coord" --listen "127.0.0.1:$port" >"$out/coordinator.log" 2>&1 &
pids="$pids $!"
wait_port "$port" || { cat "$out/coordinator.log" >&2; echo "the coordinator did not start" >&2; exit 1; }

desk() { ITSANAS_HOME="$work/desk" ITSANAS_PASSPHRASE="$passphrase" "$itsanas" "$@" </dev/null; }
must() { "$@" >"$work/last.log" 2>&1 || { cat "$work/last.log" >&2; echo "setting up the computer failed: $*" >&2; exit 1; }; }
must desk init --username "$username"
must desk coordinator "127.0.0.1:$port" --device "$device"
must desk listen "127.0.0.1:$desk_port"
# What the phone deletes for good leaves this computer at once, not in 30 days.
must desk trash days 0
printf 'written on a computer\n' >"$work/from-computer.txt"
printf 'to be deleted from the phone\n' >"$work/to-trash.txt"
must desk put from-computer.txt "$work/from-computer.txt"
must desk put to-trash.txt "$work/to-trash.txt"
# The recovery copy is what lets the phone log in with a name and a passphrase.
must desk register --recovery
ITSANAS_HOME="$work/desk" ITSANAS_PASSPHRASE="$passphrase" "$itsanas" daemon --interval 2 --no-discovery \
    </dev/null >"$out/computer.log" 2>&1 &
daemon=$!
pids="$pids $daemon"
wait_port "$desk_port" || { cat "$out/computer.log" >&2; echo "the computer did not serve" >&2; exit 1; }

echo "== the hand test, on $simulator"
# TEST_RUNNER_X reaches the UI tests as X. The phone names the coordinator,
# not 127.0.0.1: a member's coordinator is never a loopback literal.
export TEST_RUNNER_E2E_COORDINATOR="localhost:$port"
export TEST_RUNNER_E2E_COORDINATOR_DEVICE="$device"
export TEST_RUNNER_E2E_USERNAME="$username"
export TEST_RUNNER_E2E_PASSPHRASE="$passphrase"
tests="ITSaNASUITests"
[ -z "$only" ] || tests="ITSaNASUITests/$only"
status=0
xcodebuild -project ios/ITSaNAS.xcodeproj -scheme ITSaNAS \
    -destination "platform=iOS Simulator,id=$simulator" \
    -derivedDataPath target/ios-test -resultBundlePath "$out/result.xcresult" \
    -only-testing:"$tests" test || status=$?

# What the app itself said (its NSLog lines: the trash's failures, the
# fingers' state at each unlock): a failed UI test shows the screen, not why.
if [ "$status" != 0 ]; then
    echo "== the app's own lines"
    # xcodebuild may leave the simulator shut down; its log outlives that,
    # but `simctl spawn` needs it booted ("device is not booted", #409).
    xcrun simctl boot "$simulator" 2>/dev/null || true
    xcrun simctl spawn "$simulator" log show --last 60m --style compact --predicate 'process == "ITSaNAS" AND eventMessage BEGINSWITH "ITSaNAS "' 2>&1 | tail -n 80 || true
    echo "== the computer's last lines"
    tail -n 40 "$out/computer.log" 2>/dev/null || true
fi

# Step 3's end, on the computer: read once its daemon has let the store go.
if [ "$status" = 0 ] && [ -z "$only" ]; then
    echo "== what the computer holds after the phone's delete"
    # The daemon applies what the phone pushed at the top of its loop
    # (`drain_vault`) and says "pushed to us:" when that changed anything.
    # Waited for, up to 30 s, rather than a fixed six seconds; its absence is
    # said, and the computer's log is shown below when the check fails.
    for _ in $(seq 30); do
        grep -q '^pushed to us:' "$out/computer.log" 2>/dev/null && break
        sleep 1
    done
    if ! grep -q '^pushed to us:' "$out/computer.log" 2>/dev/null; then
        echo "the computer applied nothing the phone pushed (no 'pushed to us:' in its log)" >&2
    fi
    kill "$daemon" 2>/dev/null || true
    wait "$daemon" 2>/dev/null || true
    listing="$(desk ls 2>&1)" || { printf '%s\n' "$listing" >&2; status=1; }
    printf '%s\n' "$listing" | sed 's/^/    /'
    if printf '%s\n' "$listing" | grep -q 'to-trash.txt'; then
        echo "to-trash.txt, deleted from every machine on the phone, is still on the computer" >&2
        status=1
        echo "== the computer's last lines" >&2
        tail -n 40 "$out/computer.log" >&2 || true
    fi
    if ! printf '%s\n' "$listing" | grep -q 'from-computer.txt'; then
        echo "from-computer.txt left the computer, and nobody deleted it" >&2
        status=1
    fi
fi

exit "$status"
