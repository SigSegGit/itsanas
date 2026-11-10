#!/usr/bin/env bash
# Read a real iPhone while the hand test runs (docs/IOS.md), and say what it saw.
#
# The app on the phone is the build signed and installed by its owner: nothing
# here installs, signs or drives it. Plugged in by cable and trusted, the phone
# gives its system log and its crash reports to this machine, and that is what
# a person cannot read by eye: whether a background round ran, and whether the
# app died.
#
#   bash scripts/iphone-watch.sh            # until Ctrl-C, then the verdict
#   bash scripts/iphone-watch.sh 900        # fifteen minutes, then the verdict
#   IPHONE_RECEIPT="hand test 0-10, iPhone SE" bash scripts/iphone-watch.sh
#                                           # and, on PASS, a receipt for this
#                                           # commit (scripts/device-receipt.sh)
#
# Needs one of: pymobiledevice3 (`pip install pymobiledevice3`), or
# libimobiledevice's `idevicesyslog` and `idevicecrashreport`. On Windows,
# Apple's device drivers too (Apple Devices, or iTunes).
#
# Output, in $IPHONE_OUT (default ios/build/watch): syslog.txt (the app's lines
# only), crashes/ (its crash reports since the start), and the verdict, also
# appended to ~/.itsanas-receipts/iphone.txt.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
seconds="${1:-}"
out="${IPHONE_OUT:-$here/ios/build/watch}"
receipts="$HOME/.itsanas-receipts"
mkdir -p "$out/crashes" "$receipts"
: >"$out/syslog.txt"

if command -v pymobiledevice3 >/dev/null 2>&1; then
    tool=pymobiledevice3
    devices="$(pymobiledevice3 usbmux list 2>/dev/null | grep -c '"Identifier"' || true)"
elif command -v idevicesyslog >/dev/null 2>&1; then
    tool=libimobiledevice
    devices="$(idevice_id -l 2>/dev/null | grep -c . || true)"
else
    echo "neither pymobiledevice3 nor idevicesyslog is installed: pip install pymobiledevice3" >&2
    exit 1
fi
if [ "${devices:-0}" -lt 1 ]; then
    echo "no iPhone: plug it in by cable, unlock it and answer Trust on it" >&2
    exit 1
fi

started="$(date +%s)"
echo "== reading the iPhone's log ($tool); the hand test can start"
filter() { grep --line-buffered -i 'itsanas' >>"$out/syslog.txt" || true; }
if [ "$tool" = pymobiledevice3 ]; then
    pymobiledevice3 syslog live 2>/dev/null | filter &
else
    idevicesyslog 2>/dev/null | filter &
fi
reader=$!
stop() { kill "$reader" 2>/dev/null || true; wait "$reader" 2>/dev/null || true; }
trap stop EXIT
if [ -n "$seconds" ]; then
    sleep "$seconds"
else
    trap 'echo; echo "== stopped"' INT
    wait "$reader" || true
fi
stop
trap - EXIT

echo "== the crash reports"
if [ "$tool" = pymobiledevice3 ]; then
    pymobiledevice3 crash pull "$out/crashes" >/dev/null 2>&1 || true
else
    idevicecrashreport -k -e "$out/crashes" >/dev/null 2>&1 || true
fi
# Only the app's, and only those written since the start: an old crash is not
# this run's.
crashes=0
while IFS= read -r -d '' report; do
    if [ "$(stat -c %Y "$report" 2>/dev/null || echo 0)" -ge "$started" ]; then
        crashes=$((crashes + 1))
        echo "  $report"
    fi
done < <(find "$out/crashes" -iname 'ITSaNAS*' -print0)

rounds="$(grep -c 'ITSaNAS background' "$out/syslog.txt" || true)"
panics="$(grep -ci 'panicked' "$out/syslog.txt" || true)"
lines="$(wc -l <"$out/syslog.txt" | tr -d ' ')"

verdict=PASS
why="$lines lines of the app, $rounds background rounds, no crash, no panic"
if [ "$crashes" -gt 0 ] || [ "$panics" -gt 0 ]; then
    verdict=FAIL
    why="$crashes crash reports and $panics panics since the start; see $out"
elif [ "$lines" -eq 0 ]; then
    verdict=FAIL
    why="nothing from the app in the log: was it opened, and is it this phone?"
fi
line="$(date -u +%Y-%m-%dT%H:%M:%SZ) iphone watch $verdict $why"
echo "$line" | tee -a "$receipts/iphone.txt"
# A PASS says so on GitHub for this commit, when the hand test was run on its
# build: IPHONE_RECEIPT names what ran, and its absence posts nothing, since
# this script cannot know which build the phone carries.
if [ "$verdict" = PASS ] && [ -n "${IPHONE_RECEIPT:-}" ]; then
    bash "$here/scripts/device-receipt.sh" iphone "$IPHONE_RECEIPT: $why" || true
fi
[ "$verdict" = PASS ]
