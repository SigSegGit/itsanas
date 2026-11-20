#!/usr/bin/env bash
# Isolated lifecycle tests for scripts/fleet/member.sh. No network, real
# coordinator, account phrase, or production state is used.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
BIN="$WORK/itsanas"
OUT="$WORK/output"
OLD_A=$(printf 'a%.0s' {1..64})
OLD_B=$(printf 'b%.0s' {1..64})
CURRENT=$(printf 'c%.0s' {1..64})
RESTORED=$(printf 'd%.0s' {1..64})
OTHER=$(printf 'e%.0s' {1..64})
failures=0

ok() { printf '  ok    %s\n' "$*"; }
bad() {
    printf '  FAIL  %s\n' "$*"
    printf 'test check-fleet-member: %s ... FAILED\n' "$*"
    failures=$((failures + 1))
}

cat >"$BIN" <<'FAKE'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$FLEET_TEST_LOG"
case "$1" in
    login)
        mkdir -p "$ITSANAS_HOME"
        printf 'device : %s\n' "$TEST_DEVICE_ID"
        ;;
    coordinator) ;;
    register)
        if [ "${TEST_REGISTER_FAIL:-}" = 1 ]; then
            echo "fake registration refusal" >&2
            exit 1
        fi
        echo "registered"
        ;;
    device)
        [ "$2" = forget ] || exit 2
        [ "$3" != "$OTHER_DEVICE_ID" ] || exit 99
        ;;
    folder)
        [ "${TEST_FOLDER_FAIL:-}" != 1 ] || exit 1
        mkdir -p "$2"
        ;;
    daemon)
        mkdir -p "$ITSANAS_HOME"
        if [ "${TEST_DAEMON_HANG:-}" = 1 ]; then
            exec sleep 60
        fi
        printf 'device : %s\n' "$TEST_DEVICE_ID" >"$ITSANAS_HOME/status.snapshot"
        ;;
    *) exit 2 ;;
esac
FAKE
chmod +x "$BIN"

set_case() {
    local name=$1
    CASE="$WORK/$name"
    mkdir -p "$CASE/state"
    LOG="$CASE/calls"
    : >"$LOG"
}

run_member() {
    ITSANAS_BIN="$BIN" \
        FLEET_MEMBER="$CASE/member" \
        FLEET_STATE="$CASE/state" \
        FLEET_COORD=test.invalid:9898 \
        FLEET_COORD_ID="$OTHER" \
        FLEET_TEST_PHRASE="fixture phrase only" \
        FLEET_TEST_LOG="$LOG" \
        TEST_DEVICE_ID="$CURRENT" \
        OTHER_DEVICE_ID="$OTHER" \
        TEST_REGISTER_FAIL="${TEST_REGISTER_FAIL:-}" \
        TEST_FOLDER_FAIL="${TEST_FOLDER_FAIL:-}" \
        TEST_DAEMON_HANG="${TEST_DAEMON_HANG:-}" \
        bash scripts/fleet/member.sh start >"$OUT" 2>&1
}

echo "fresh registration removes only valid Fleet-state IDs before registration"
set_case fresh
printf '%s\nnot-an-id\n%s\n%s\n' "$OLD_A" "$OLD_B" "$CURRENT" >"$CASE/state/devices"
if run_member; then
    old_a_line=$(grep -nFx "device forget $OLD_A" "$LOG" | cut -d: -f1)
    old_b_line=$(grep -nFx "device forget $OLD_B" "$LOG" | cut -d: -f1)
    register_line=$(grep -nFx "register" "$LOG" | cut -d: -f1)
    if [ -n "$old_a_line" ] && [ -n "$old_b_line" ] &&
        [ "$old_a_line" -lt "$register_line" ] && [ "$old_b_line" -lt "$register_line" ] &&
        ! grep -Fxq "device forget not-an-id" "$LOG" &&
        ! grep -Fxq "device forget $CURRENT" "$LOG" &&
        ! grep -Fxq "device forget $OTHER" "$LOG" &&
        [ "$(cat "$CASE/state/devices")" = "$CURRENT" ]; then
        ok "old IDs withdrawn first; invalid, current and unrelated IDs untouched; state replaced after startup"
    else
        bad "fresh cleanup order or ID scope was wrong"
    fi
else
    bad "fresh member did not start: $(cat "$OUT")"
fi

echo "registration failure preserves Fleet state and does not claim success"
set_case registration-failure
printf '%s\n%s\n' "$OLD_A" "$OLD_B" >"$CASE/state/devices"
cp "$CASE/state/devices" "$CASE/state/before"
TEST_REGISTER_FAIL=1
if run_member; then
    bad "registration failure returned success"
else
    if cmp -s "$CASE/state/before" "$CASE/state/devices" &&
        grep -Fq "the runner did not join the account" "$OUT" &&
        grep -Fxq "device forget $CURRENT" "$LOG"; then
        ok "error is explicit, prior state unchanged, only the new ID is cleaned up"
    else
        bad "registration failure changed state or skipped cleanup"
    fi
fi
unset TEST_REGISTER_FAIL

echo "post-registration failure withdraws only the newly created device"
set_case post-registration-failure
printf '%s\n' "$OLD_A" >"$CASE/state/devices"
cp "$CASE/state/devices" "$CASE/state/before"
TEST_FOLDER_FAIL=1
if run_member; then
    bad "folder initialization failure returned success"
else
    if grep -Fxq "device forget $CURRENT" "$LOG" &&
        cmp -s "$CASE/state/before" "$CASE/state/devices" &&
        ! grep -Fxq "device forget $OTHER" "$LOG"; then
        ok "new ID withdrawn and saved IDs preserved"
    else
        bad "post-registration cleanup targeted the wrong IDs or changed state"
    fi
fi
unset TEST_FOLDER_FAIL

echo "restored-device failure never withdraws the persistent device"
set_case restored-failure
mkdir -p "$CASE/archive/home" "$CASE/archive/folder"
printf 'restored passphrase\n' >"$CASE/archive/passphrase"
printf 'kept home data\n' >"$CASE/archive/home/keep"
printf 'kept folder data\n' >"$CASE/archive/folder/keep"
printf '%s\n' "$RESTORED" >"$CASE/state/devices"
cp "$CASE/state/devices" "$CASE/state/before"
tar -cz -C "$CASE/archive" home folder passphrase |
    FLEET_HOME_KEY=fixture-key-only openssl enc -aes-256-cbc -pbkdf2 -iter 200000 -salt \
        -pass env:FLEET_HOME_KEY -out "$CASE/state/home.enc"
TEST_FOLDER_FAIL=1
if FLEET_HOME_KEY=fixture-key-only run_member; then
    bad "restored folder failure returned success"
else
    if grep -Fq "device forget " "$LOG"; then
        bad "a restored device was passed to device forget"
    elif cmp -s "$CASE/state/before" "$CASE/state/devices" &&
        [ -f "$CASE/member/home/keep" ] &&
        [ -f "$CASE/member/folder/keep" ]; then
        ok "restored state and device list preserved; no device forget attempted"
    else
        bad "restored state was deleted or altered"
    fi
fi
unset TEST_FOLDER_FAIL

echo "TERM during fresh startup runs bounded cleanup"
set_case signal
printf '%s\n' "$OLD_A" >"$CASE/state/devices"
cp "$CASE/state/devices" "$CASE/state/before"
TEST_DAEMON_HANG=1
env ITSANAS_BIN="$BIN" \
    FLEET_MEMBER="$CASE/member" FLEET_STATE="$CASE/state" \
    FLEET_COORD=test.invalid:9898 FLEET_COORD_ID="$OTHER" \
    FLEET_TEST_PHRASE="fixture phrase only" FLEET_TEST_LOG="$LOG" \
    TEST_DEVICE_ID="$CURRENT" OTHER_DEVICE_ID="$OTHER" \
    TEST_DAEMON_HANG=1 bash scripts/fleet/member.sh start >"$OUT" 2>&1 &
runner_pid=$!
for _ in $(seq 100); do
    grep -Fxq "daemon --interval 30 --listen 0.0.0.0:9797" "$LOG" && break
    sleep 0.1
done
kill -TERM "$runner_pid" 2>/dev/null || true
wait "$runner_pid" 2>/dev/null
runner_status=$?
if [ "$runner_status" -ne 0 ] &&
    grep -Fxq "device forget $CURRENT" "$LOG" &&
    cmp -s "$CASE/state/before" "$CASE/state/devices" &&
    ! kill -0 "$(cat "$CASE/member/daemon.pid")" 2>/dev/null; then
    ok "signal stopped the daemon, withdrew only the fresh ID, and preserved state"
else
    bad "signal cleanup failed (exit $runner_status): $(cat "$OUT")"
fi
unset TEST_DAEMON_HANG

echo
if [ "$failures" -eq 0 ]; then
    echo "check-fleet-member: all runner lifecycle checks passed"
fi
[ "$failures" -eq 0 ]
