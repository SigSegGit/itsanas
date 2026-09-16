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
#   anywhere else, plus C, K and M against a **second account that really
#   hosts** -- the host takes 2.6 MiB of the first account's chunks, is scanned
#   for the canary, lists none of them, and then has its vault deleted under it.
#   This is the laboratory version of the fleet runs. It proves the kit and the
#   mechanisms agree; it does not replace the fleet.
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
[ -x "$COORD" ] || [ -x "$COORD.exe" ] || { echo "no coordinator binary at $COORD (cargo build -p itsanas-coordinator)"; exit 2; }

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
# A property of the bench itself rather than a phase of the kit.
check() {
    local what=$1
    shift
    if "$@" >"$WORK/check.log" 2>&1; then printf '  ok    %s\n' "$what"
    else printf '  WRONG %s\n' "$what"; sed 's/^/        /' "$WORK/check.log"; failures=$((failures + 1)); fi
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
# `--invite-only --admit-first`, because that is what the Pi runs. Until
# 2026-09-16 this bench started an *open* coordinator, so every acceptance run
# exercised a configuration the fleet does not use, and the one path a new
# person actually needs -- being invited -- was covered only by unit tests in
# `itsanas-coord::directory`.
"$COORD" --state "$WORK/coord" --listen "127.0.0.1:$CPORT" --invite-only --admit-first >"$WORK/coord.log" 2>&1 &
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
# A recovery that forgets the coordinator it recovered from restores an identity
# and nothing else: `register` then has nowhere to go. It did, until 2026-09-15.
check "login --from keeps the coordinator it recovered from" \
    sh -c "ITSANAS_HOME='$WORK/m2' '$BIN' coordinator </dev/null | grep -q '127.0.0.1:$CPORT'"
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

say "A host that refuses everything says so"
# The visible half of the fix, which the unit test cannot see: a host that has
# pledged nothing refuses every byte, and the round used to print `sent 0 B`,
# the line for "nothing to send". Every node above pledges, precisely because of
# that, so this host is made on purpose and the round must say "refused".
must node stingy init --username stingy-host
stingy_out=$(serving stingy "$((CPORT + 9))" node m1 sync "127.0.0.1:$((CPORT + 9))" 2>&1)
check "a round against a host with pledge 0 prints why nothing was stored" \
    sh -c "printf '%s\n' \"\$1\" | grep -q 'refused [0-9]* offer(s): its pledge is full or zero'" _ "$stingy_out"

say "C as written: a host of another account holds the data and cannot read it"
# The C section above has only its negative control. The test is about a host of
# *another account*, and scanning the owner's own home finds the canary --
# correctly, because an owner's index holds names in the clear. That needs a
# second account which actually pledges and actually accepts chunks, which is
# what this builds. Until it existed, the one test whose failure stops the
# project was the one the bench could not run.
HOSTPORT=$((CPORT + 11))
must node host2 init --username neighbour-host
must node host2 pledge 1G
host_out=$(serving host2 "$HOSTPORT" node m1 sync "127.0.0.1:$HOSTPORT" 2>&1)
printf '        %s\n' "$(printf '%s\n' "$host_out" | grep -E 'sent|received' | tail -1)"
# Without this the scan below passes on a host that was sent nothing at all,
# which is the failure mode the F phase already had once.
check "a host of another account accepted chunks" \
    sh -c "printf '%s\n' \"\$1\" | grep -Eq 'sent [1-9]'" _ "$host_out"
# Hosted data lives in the node's vault, not in its own store: `store/blobs`
# is this account's own chunks and is empty on a pure host. Checking the wrong
# directory made this read zero while the host held 2.6 MiB.
sealed=$(find "$WORK/host2/vault" -type f 2>/dev/null | wc -l)
check "the host has sealed data in its vault to search" test "$sealed" -gt 0
expect_pass C scan "$canary" "$WORK/host2"

say "M: the host lists none of the account it is storing for"
check "the host's own listing does not name the owner's file" \
    sh -c "! ITSANAS_HOME='$WORK/host2' ITSANAS_PASSPHRASE='$PASSPHRASE' '$BIN' ls </dev/null 2>&1 | grep -q '$canary'"

say "K: a host that throws the data away stops counting as a holder"
# The sanction every economic claim rests on, exercised end to end for the first
# time: challenges, Reliability, FAILURES_BEFORE_PAUSE. Deleting the blobs and
# leaving the ledger is exactly the attack -- keep claiming the space, hold
# nothing -- and it is what test K asks Nicolas to do by hand.
# **`itsanas sync` does not audit.** `session::audit` is called from the daemon
# loop and nowhere else, so a one-shot round pushes and pulls and never
# challenges anybody. The first version of this phase ran three `sync` rounds
# against a host whose vault had been deleted and watched the placement count
# sit still -- which reads as "the sanction does not work" and is really "the
# sanction was never asked to run". Test K needs the daemon up on the owner's
# side, and `docs/MVP.md` and `docs/BRIEFING-MVP.md` say so because of this.
# A second host, because the half of K that matters is not "the owner
# complains" but "the data ends up somewhere else". With one host there is
# nowhere else, so that half was structurally untestable -- and the first
# version of this phase quietly lowered the criterion to match what a
# one-host bench could see. MVP.md §4 forbids exactly that.
HOST3PORT=$((CPORT + 13))
must node host3 init --username spare-host
must node host3 pledge 1G
must node m1 peer add "127.0.0.1:$HOSTPORT"
must node m1 peer add "127.0.0.1:$HOST3PORT"
before=$(node m1 status 2>&1 | grep -oE 'placements +[0-9]+' | awk '{print $2}')
rm -rf "$WORK/host2/vault"
ITSANAS_HOME="$WORK/host2" ITSANAS_PASSPHRASE="$PASSPHRASE" "$BIN" serve --listen "127.0.0.1:$HOSTPORT" \
    </dev/null >>"$WORK/host2-serve.log" 2>&1 &
hsrv=$!
pids+=("$hsrv")
wait_port "$HOSTPORT" || { cat "$WORK/host2-serve.log"; echo "host2 did not serve"; exit 1; }
ITSANAS_HOME="$WORK/host3" ITSANAS_PASSPHRASE="$PASSPHRASE" "$BIN" serve --listen "127.0.0.1:$HOST3PORT" \
    </dev/null >>"$WORK/host3-serve.log" 2>&1 &
h3srv=$!
pids+=("$h3srv")
wait_port "$HOST3PORT" || { cat "$WORK/host3-serve.log"; echo "host3 did not serve"; exit 1; }
ITSANAS_HOME="$WORK/m1" ITSANAS_PASSPHRASE="$PASSPHRASE" "$BIN" daemon --interval 2 \
    --listen "0.0.0.0:$((CPORT + 12))" </dev/null >"$WORK/daemon-audit.log" 2>&1 &
daudit=$!
pids+=("$daudit")
# Challenges are drawn at random, so one round may miss; wait for the line the
# daemon prints when a host fails, and give up after a bounded time either way.
for _ in $(seq 60); do
    grep -q 'storage challenges' "$WORK/daemon-audit.log" && break
    sleep 1
done
kill "$daudit" "$hsrv" "$h3srv" 2>/dev/null
wait "$daudit" "$hsrv" "$h3srv" 2>/dev/null
for port in "$HOSTPORT" "$HOST3PORT"; do
    for _ in $(seq 50); do (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null || break; sleep 0.1; done
done
spare=$(find "$WORK/host3/vault" -type f 2>/dev/null | wc -l)
check "the data reached a second host, so a disqualified one can be replaced" \
    test "$spare" -gt 0
check "the daemon says the host failed its storage challenges" \
    grep -q 'storage challenges' "$WORK/daemon-audit.log"
m1_status=$(node m1 status 2>&1)
after=$(printf '%s\n' "$m1_status" | grep -oE 'placements +[0-9]+' | awk '{print $2}')
printf '        placements %s -> %s\n' "${before:-?}" "${after:-?}"
printf '%s\n' "$m1_status" | sed -n '/failed a storage challenge/,+2p' | sed 's/^/        /'
# Two observables, and the second is the one that matters: the owner must name
# the machine that cheated, and the data must end up somewhere else. With only
# one host the second could not happen at all, and a flat placement count was
# briefly mistaken for a broken sanction. With a spare host it rises.
check "the owner names the host that discarded the data" \
    sh -c 'printf "%s\n" "$1" | grep -q "failed a storage challenge"' _ "$m1_status"

say "A second person joins an invite-only coordinator, and cannot without a code"
# The path the next real member walks, end to end over a socket for the first
# time. `--admit-first` let machine 1 in; everybody after needs a code, and the
# refusal is half the test: a coordinator that admits strangers is the whole
# threat model.
must node newcomer init --username newcomer
must node newcomer coordinator "127.0.0.1:$CPORT" --device "$COORD_ID"
check "an uninvited stranger is refused by an invite-only coordinator" \
    sh -c "! ITSANAS_HOME='$WORK/newcomer' ITSANAS_PASSPHRASE='$PASSPHRASE' '$BIN' register </dev/null 2>&1"
invite_out=$(node m1 invite 2>&1)
# 64 hexadecimal characters, and nothing else. A looser pattern picked an
# 8-character fragment out of the surrounding prose, and `register` then
# refused it for the wrong reason -- which read as the invite path failing.
code=$(printf '%s\n' "$invite_out" | grep -oE '[0-9a-f]{64}' | head -1)
check "an existing member can mint an invitation" test -n "$code"
if [ -n "$code" ]; then
    check "the invited newcomer is admitted" \
        env ITSANAS_HOME="$WORK/newcomer" ITSANAS_PASSPHRASE="$PASSPHRASE" "$BIN" register --invite "$code"
    # Re-registering is how a member refreshes keys; it must not need a
    # second code, or every key rotation would cost an invitation.
    check "a member re-registers without a fresh code" \
        env ITSANAS_HOME="$WORK/newcomer" ITSANAS_PASSPHRASE="$PASSPHRASE" "$BIN" register
    # A single-use code is spent. If it were not, one leaked code would admit
    # the internet to this coordinator.
    must node stranger2 init --username stranger2
    must node stranger2 coordinator "127.0.0.1:$CPORT" --device "$COORD_ID"
    check "a single-use code cannot admit a second stranger" \
        sh -c "! ITSANAS_HOME='$WORK/stranger2' ITSANAS_PASSPHRASE='$PASSPHRASE' '$BIN' register --invite '$code' </dev/null 2>&1"
fi

say "Two accounts on one machine: separate ports, and both hear the local network"
# A second account on this machine is a second node home and a second daemon.
# Two things used to break it: every node was created listening on 9797, so the
# second daemon could not bind; and discovery refused to share UDP 21037, so the
# second ran with discovery silently off. Machine 1 is already a node beside
# this one, configured for 9797.
must node other init --username second-account
other_listen=$(node other listen 2>&1)
check "a second account's node is not given the port machine 1 is configured for" \
    sh -c "printf '%s\n' '$other_listen' | grep -Eq '^0\.0\.0\.0:[0-9]+$' && ! printf '%s\n' '$other_listen' | grep -q ':9797$'"
ITSANAS_HOME="$WORK/m1" ITSANAS_PASSPHRASE="$PASSPHRASE" "$BIN" daemon --interval 5 --listen "0.0.0.0:$((CPORT + 7))" \
    </dev/null >"$WORK/daemon-m1.log" 2>&1 &
d1=$!
pids+=("$d1")
ITSANAS_HOME="$WORK/other" ITSANAS_PASSPHRASE="$PASSPHRASE" "$BIN" daemon --interval 5 \
    </dev/null >"$WORK/daemon-other.log" 2>&1 &
d2=$!
pids+=("$d2")
# Announcements go out every thirty seconds; three of them is generous.
for _ in $(seq 90); do
    grep -q "found another user's device" "$WORK/daemon-m1.log" &&
        grep -q "found another user's device" "$WORK/daemon-other.log" && break
    sleep 1
done
check "the first account's daemon hears the second on the shared discovery port" \
    grep -q "found another user's device" "$WORK/daemon-m1.log"
check "the second account's daemon hears the first" \
    grep -q "found another user's device" "$WORK/daemon-other.log"
check "neither daemon ran with local discovery off" \
    sh -c "! grep -h 'local discovery is off' '$WORK/daemon-m1.log' '$WORK/daemon-other.log'"
kill "$d1" "$d2" 2>/dev/null
wait "$d1" "$d2" 2>/dev/null

say "Accounts: a restored machine is enrolled, listed, withdrawn for good"
# Last, because it withdraws machine 2 and changes machine 3's passphrase.
must node m2 listen "127.0.0.1:$PORT2"
must node m2 register
# Machine 3 has no peer configured and has never been told an address. `sync`
# used to read only configured peers and refuse; the account's machines are in
# the coordinator, and a restored machine must find them there.
check "sync with nothing configured reaches the account's machines through the coordinator" \
    serving m2 "$PORT2" node m3 sync
enrolled() { node m1 device list 2>&1 | grep -c ' pledges '; }
check "the device list names both enrolled machines" test "$(enrolled)" -eq 2
m2_id=$(node m2 device list 2>&1 | awk '/this machine/ {print $1}')
check "machine 2 finds itself in the list" test -n "$m2_id"
must node m1 device forget "${m2_id:0:12}"
check "a withdrawn machine leaves the list" test "$(enrolled)" -eq 1
check "a withdrawn machine cannot enrol itself again" \
    sh -c "out=\$(ITSANAS_HOME='$WORK/m2' ITSANAS_PASSPHRASE='$PASSPHRASE' '$BIN' register </dev/null 2>&1) && { printf '%s\n' \"\$out\"; exit 1; }; printf '%s\n' \"\$out\" | grep -q 'withdrawn from this account'"
NEW_PASSPHRASE=$(head -c 24 /dev/urandom | od -An -tx1 | tr -d ' \n')
must env ITSANAS_HOME="$WORK/m3" ITSANAS_PASSPHRASE="$PASSPHRASE" ITSANAS_NEW_PASSPHRASE="$NEW_PASSPHRASE" "$BIN" passphrase
check "the new passphrase opens machine 3" \
    env ITSANAS_HOME="$WORK/m3" ITSANAS_PASSPHRASE="$NEW_PASSPHRASE" "$BIN" whoami
check "the old passphrase no longer does" \
    sh -c "! ITSANAS_HOME='$WORK/m3' ITSANAS_PASSPHRASE='$PASSPHRASE' '$BIN' whoami </dev/null"
# A passphrase change that leaves the recovery container under the old one is a
# recovery that fails months later, with the passphrase the owner now uses.
must env ITSANAS_HOME="$WORK/m1" ITSANAS_PASSPHRASE="$PASSPHRASE" ITSANAS_NEW_PASSPHRASE="$NEW_PASSPHRASE" \
    "$BIN" passphrase --recovery
check "after passphrase --recovery, a fresh machine recovers with the new passphrase" \
    env ITSANAS_HOME="$WORK/fresh-new" ITSANAS_PASSPHRASE="$NEW_PASSPHRASE" \
    "$BIN" login --username acceptance --from "127.0.0.1:$CPORT" --device "$COORD_ID"
check "and no longer with the old one" \
    sh -c "! ITSANAS_HOME='$WORK/fresh-old' ITSANAS_PASSPHRASE='$PASSPHRASE' '$BIN' login --username acceptance --from '127.0.0.1:$CPORT' --device '$COORD_ID' </dev/null"

echo
if [ "$failures" -eq 0 ]; then
    echo "acceptance-local: every phase passed where it must and failed where it must"
else
    echo "acceptance-local: $failures phases gave the wrong verdict"
fi
[ "$failures" -eq 0 ]
