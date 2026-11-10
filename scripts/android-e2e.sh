#!/usr/bin/env bash
# The end-to-end protocol of the phone app, as a person would do it.
#
# Builds the core for an x86_64 emulator (a DEBUG build: the call that pins the
# tests to a local coordinator is refused by a release one, which is the point),
# builds and starts the host's coordinator on a scratch directory, runs the
# instrumented tests against it, and stops what it started even when a test
# fails. A device or emulator must already be attached (`adb devices`).
#
# A real phone works the same way, by USB or by Wi-Fi debugging:
#
#   adb pair <phone's pairing address>     # once, with the code the phone shows
#   adb connect <phone's debugging address>
#   ANDROID_SERIAL=<that address> bash scripts/android-e2e.sh
#
# The tests run in the debug build, `fr.ngas.itsanas.e2e`, beside the installed
# app and its account, which they never touch. The core is built for the phone's
# own processor; the phone reaches this machine's coordinator through
# `adb reverse`; and the animation and error-dialog settings the run changes
# are put back as they were. The phone must stay unlocked during the run.
#
# The tests never see the official coordinator: they are given this one's
# address, and a run without it fails saying so.
#
# Output, in $E2E_OUT (default android/app/build/e2e): logcat.txt, the test
# reports, and screens/ with a screenshot of every step. A screen of secrets
# is black on purpose (FLAG_SECURE).
#
# Needs the same tools as scripts/build-apk.sh (JDK, Android SDK, NDK, cargo-ndk),
# plus `adb` and a running emulator.
#
#   bash scripts/android-e2e.sh                   # all scenarios
#   bash scripts/android-e2e.sh fr.ngas.itsanas.WelcomeTest   # one class
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
export ANDROID_HOME="${ANDROID_HOME:-D:/Android/Sdk}"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/27.3.13750724}"
export JAVA_HOME="${JAVA_HOME:-D:/Android/jdk}"
gradle="${GRADLE:-D:/Android/gradle-8.10.2/bin/gradle}"
adb="${ADB:-adb}"
if ! command -v "$adb" >/dev/null 2>&1 && [ -x "$ANDROID_HOME/platform-tools/adb.exe" ]; then
    adb="$ANDROID_HOME/platform-tools/adb.exe"
elif ! command -v "$adb" >/dev/null 2>&1 && [ -x "$ANDROID_HOME/platform-tools/adb" ]; then
    adb="$ANDROID_HOME/platform-tools/adb"
fi
port="${E2E_PORT:-9899}"
out="${E2E_OUT:-$here/android/app/build/e2e}"
target="${CARGO_TARGET_DIR:-$here/target}"
only="${1:-}"

for tool in cargo-ndk "$adb"; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "$tool is not installed or not on PATH" >&2
        exit 1
    fi
done

if [ "$("$adb" get-state 2>/dev/null)" != "device" ]; then
    echo "no device: attach a phone or start an emulator (adb devices), or set ANDROID_SERIAL" >&2
    exit 1
fi
shell() { "$adb" shell "$@" | tr -d '\r'; }
abi="$(shell getprop ro.product.cpu.abi)"
case "$abi" in
    arm64-v8a | armeabi-v7a | x86_64) ;;
    *) echo "this device's processor ($abi) is not one the app is built for" >&2; exit 1 ;;
esac
real=""
[ "$(shell getprop ro.kernel.qemu)" = "1" ] || real=1

# A real phone keeps its own settings: what the run changes is put back.
settings=(window_animation_scale transition_animation_scale animator_duration_scale hide_error_dialogs)
declare -A before=()
if [ -n "$real" ]; then
    for name in "${settings[@]}"; do
        before[$name]="$(shell settings get global "$name")"
    done
fi

state="$(mktemp -d)"
coordinator_pid=""
cleanup() {
    if [ -n "$coordinator_pid" ]; then
        kill "$coordinator_pid" 2>/dev/null || true
        wait "$coordinator_pid" 2>/dev/null || true
    fi
    if [ -n "$real" ]; then
        "$adb" reverse --remove "tcp:$port" >/dev/null 2>&1 || true
        for name in "${settings[@]}"; do
            if [ "${before[$name]}" = "null" ] || [ -z "${before[$name]}" ]; then
                "$adb" shell settings delete global "$name" >/dev/null 2>&1 || true
            else
                "$adb" shell settings put global "$name" "${before[$name]}" >/dev/null 2>&1 || true
            fi
        done
    fi
    rm -rf "$state"
}
trap cleanup EXIT

rm -rf "$out"
mkdir -p "$out/screens"

echo "== the core, debug, for $abi${real:+ (a real phone)}"
cd "$here"
rm -rf android/app/src/main/jniLibs
cargo ndk -t "$abi" -o android/app/src/main/jniLibs build -p itsanas-android
find android/app/src/main/jniLibs -name 'lib*-*.so' -delete

echo "== the coordinator, on this machine"
cargo build -p itsanas-coordinator
coordinator="$target/debug/itsanas-coordinator"
[ -x "$coordinator" ] || coordinator="$coordinator.exe"
device="$("$coordinator" --state "$state" --identity | tail -n 1)"
"$coordinator" --state "$state" --listen "127.0.0.1:$port" >"$out/coordinator.log" 2>&1 &
coordinator_pid=$!
sleep 2
if ! kill -0 "$coordinator_pid" 2>/dev/null; then
    echo "the coordinator did not start:" >&2
    cat "$out/coordinator.log" >&2
    exit 1
fi

echo "== the tests"
"$adb" logcat -c || true
"$adb" shell rm -rf /data/local/tmp/e2e || true
# Animations make every tap a race against the screen.
# A slow runner makes the launcher "not respond", and that dialog takes the
# window focus from the app under test: every test then fails the same way.
"$adb" shell settings put global hide_error_dialogs 1 || true
"$adb" shell input keyevent KEYCODE_HOME || true
for scale in window_animation_scale transition_animation_scale animator_duration_scale; do
    "$adb" shell settings put global "$scale" 0 || true
done

# The emulator reaches this machine at 10.0.2.2; a real phone through a tunnel
# adb opens, which needs no network between the two and opens nothing on it.
coordinator_address="10.0.2.2:$port"
if [ -n "$real" ]; then
    "$adb" reverse "tcp:$port" "tcp:$port"
    # A name, not 127.0.0.1: a member's coordinator is never a loopback
    # address (every member would dial itself), and the check is on the
    # literal. On the phone the name resolves into the tunnel all the same.
    coordinator_address="localhost:$port"
    "$adb" shell input keyevent KEYCODE_WAKEUP || true
fi

args=(
    "-Pandroid.testInstrumentationRunnerArguments.coordinator=$coordinator_address"
    "-Pandroid.testInstrumentationRunnerArguments.coordinatorDevice=$device"
)
if [ -n "$only" ]; then
    args+=("-Pandroid.testInstrumentationRunnerArguments.class=$only")
fi

status=0
(cd "$here/android" && "$gradle" --no-daemon ":app:connectedDebugAndroidTest" "${args[@]}") || status=$?

echo "== what it left"
"$adb" logcat -d >"$out/logcat.txt" || true
"$adb" pull /data/local/tmp/e2e/. "$out/screens/" >/dev/null 2>&1 || true
cp -r "$here/android/app/build/reports/androidTests" "$out/reports" 2>/dev/null || true
cp -r "$here/android/app/build/outputs/androidTest-results" "$out/results" 2>/dev/null || true
echo "screenshots: $(find "$out/screens" -name '*.png' | wc -l), in $out/screens"

# A crash the tests did not notice is still a crash: the app dying under a user
# loses what they were doing, whatever the assertions said.
if grep -A4 "FATAL EXCEPTION" "$out/logcat.txt" | grep -q "Process: fr.ngas.itsanas"; then
    echo "FATAL EXCEPTION from fr.ngas.itsanas in the log:" >&2
    grep -B1 -A12 "FATAL EXCEPTION" "$out/logcat.txt" | head -60 >&2
    status=1
fi

# A real phone that passed every scenario says so on GitHub, for this commit:
# the CI then takes it for an emulator job no runner finished. All scenarios
# only: one class is a look, not a receipt.
if [ "$status" = 0 ] && [ -n "$real" ] && [ -z "$only" ]; then
    model="$(shell getprop ro.product.model)"
    release="$(shell getprop ro.build.version.release)"
    passed="$(grep -ho 'tests="[0-9]*"' "$out"/results/connected/*/*.xml 2>/dev/null | grep -o '[0-9]*' | awk '{ n += $1 } END { print n + 0 }')"
    bash "$here/scripts/device-receipt.sh" android "$model, Android $release: $passed tests of scripts/android-e2e.sh" || true
fi

exit "$status"
