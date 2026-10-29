#!/usr/bin/env bash
# The fleet tests, tested without a fleet: the agent refuses what it must,
# and scripts/fleet/run.sh says FAIL when a file does not cross, a canary is
# readable, or a daemon has stopped -- not only PASS when all is well.
#
#   bash scripts/check-fleet.sh
#
# Why this exists
# ---------------
#
# The fleet run happens at night on machines nobody is watching, behind a key
# that can leak (docs/FLEET-CI.md). Two things must therefore be true before
# it ever runs, and both are cheap to check on every push:
#
# * **the agent is a wall.** Every request a leaked key could send to do
#   something else -- a shell metacharacter, a path, an option, another
#   account, a file the kit did not write, more files than the cap -- is
#   refused with exit 64, and the folder is left as it was.
# * **the run can fail.** Each scenario is run once where it must pass and
#   once where it must not. A fleet run that cannot say FAIL would turn the
#   night's green into the decoration acceptance-local.sh was written to
#   catch.
#
# "Machines" here are agent configurations in a temporary directory; two of
# them share one folder, which is what a sync that worked looks like from the
# outside. Nothing needs the network, a node or cargo.

set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

AGENT=scripts/fleet/agent.sh
RUN=scripts/fleet/run.sh
WORK=$(mktemp -d)
TEST_UID=$(printf 't%.0s' 1 | sha256sum | cut -d' ' -f1)
trap 'rm -rf "$WORK"' EXIT
failures=0

ok() { printf '  ok    %s\n' "$*"; }
bad() { printf '  FAIL  %s\n' "$*"; failures=$((failures + 1)); }

# A machine: an agent configuration, a test folder, a test home with a fresh
# snapshot (or a stale one), and another account's home.
machine() {
    local name=$1 folder=$2 age=${3:-10} other=${4:-$WORK/$1/other}
    mkdir -p "$WORK/$name/test" "$other/vault/owners" "$folder" "$WORK/$name/receipts"
    # What this host holds for test: one chunk, unless the caller made it
    # something else (a leak) or nothing (NOHOST=1).
    if [ -z "${NOHOST:-}" ] && [ ! -e "$other/vault/owners/$TEST_UID" ]; then
        mkdir -p "$other/vault/owners/$TEST_UID"
        head -c 4096 /dev/urandom >"$other/vault/owners/$TEST_UID/chunk"
    fi
    # COPIES: the copies part of the header ("2 of 2", "1 of 2 since T"),
    # empty for none; FILES: the files line.
    local copies=${COPIES-2 of 2}
    printf 'snapshot %s every 300 version %s%s\naccount\n  username        test\nyour data\n  files           %s\n' \
        "$(($(date +%s) - age))" "${VERSION:-0.1.13}" "${copies:+ copies $copies}" "${FILES:-3}" \
        >"$WORK/$name/test/status.snapshot"
    cat >"$WORK/$name/agent.conf" <<EOF
FOLDER="$folder"
KIT="$PWD/scripts/acceptance.sh"
RECEIPTS="$WORK/$name/receipts"
ACCOUNTS="test other"
TEST_UID="$TEST_UID"
HOME_test="$WORK/$name/test"
HOME_other="$other"
MAX_FILES=${MAX_FILES:-50}
EOF
}

agent() {
    local name=$1
    shift
    SSH_ORIGINAL_COMMAND="$*" ITSANAS_FLEET_CONF="$WORK/$name/agent.conf" sh "$AGENT"
}

echo "the agent refuses what a leaked key would try"

machine wall "$WORK/wall-folder"
echo "a note somebody put here" >"$WORK/wall-folder/notes.txt"
before=$(cd "$WORK/wall-folder" && ls -la | sha256sum)
long=$(printf 'B check %0250d' 0)
REQUESTS=$(cat <<EOF
B write; touch $WORK/pwned
B write \$(touch $WORK/pwned)
B write \`touch $WORK/pwned\`
B write | sh
F delete ../../etc/passwd
F delete notes.txt
F delete -rf
F delete acceptance-B-20261023T000000Z-1.bin/../notes.txt
B check acceptance-B-20261023T000000Z-1.bin not-a-sha
B check notes.txt $(printf '%064d' 0)
C scan ITSANAS-CANARY-000000000000000000000000 test
C scan ITSANAS-CANARY-000000000000000000000000 nobody
C scan /etc/passwd other
status nobody
sh
bash -i

$long
EOF
)
while IFS= read -r request; do
    out=$(agent wall "$request" 2>&1)
    status=$?
    if [ "$status" -eq 64 ] && [ "${out%% *}" = REFUSED ]; then
        ok "refused: ${request:0:60}"
    else
        bad "not refused (exit $status): ${request:0:60} -> $(printf '%s' "$out" | head -1)"
    fi
done <<<"$REQUESTS"
[ ! -e "$WORK/pwned" ] || bad "a refused request ran a command: $WORK/pwned exists"
after=$(cd "$WORK/wall-folder" && ls -la | sha256sum)
[ "$before" = "$after" ] && ok "the folder is as it was" || bad "a refused request changed the folder"

out=$(agent wall hello 2>&1) && [ "${out%% *}" = hello ] && ok "hello answers" || bad "hello: $out"
out=$(agent wall B write 2>&1)
case "$out" in PASS*) ok "B write writes" ;; *) bad "B write: $out" ;; esac
read -r _ _ _ name _ sha _ <<<"$out"
out=$(agent wall B check "$name" "$sha" 2>&1)
case "$out" in PASS*) ok "B check reads it back" ;; *) bad "B check: $out" ;; esac
[ -f "$WORK/wall-folder/notes.txt" ] && ok "a file the kit did not write is still there" || bad "notes.txt is gone"

MAX_FILES=1 machine capped "$WORK/capped-folder"
agent capped B write >/dev/null 2>&1
out=$(agent capped B write 2>&1)
status=$?
[ "$status" -eq 64 ] && [ "$(find "$WORK/capped-folder" -type f | wc -l)" -eq 1 ] &&
    ok "past MAX_FILES, B write is refused" || bad "B write past the cap: exit $status, $out"

# A click on Run workflow while the night's run is on: two requests at once
# would each count the other's files. The second is refused, not queued.
if command -v flock >/dev/null 2>&1; then
    # Held shared: an agent that took a shared lock itself would get in
    # beside it, and show here.
    flock -s "$WORK/wall/receipts/agent.lock" sleep 3 &
    sleep 0.5
    out=$(agent wall hello 2>&1)
    status=$?
    wait
    [ "$status" -eq 64 ] && [ "${out%% *}" = REFUSED ] &&
        ok "a second request while one runs is refused" || bad "agent.sh ran a request beside another: exit $status, $out"
fi

echo "the run passes where it must, and fails where it must"

export FLEET_WAIT=4 FLEET_POLL=1 FLEET_TIMEOUT=10 FLEET_SETTLE=1
fleet() {
    local hosts=$1 scan=$2
    shift 2
    local var
    for h in $hosts; do
        var="FLEET_AGENT_$h"
        export "$var=$WORK/$h/agent.conf"
    done
    FLEET_HOSTS="$hosts" FLEET_SCAN="$scan" GITHUB_STEP_SUMMARY='' bash "$RUN" >"$WORK/run.log" 2>&1
}
expect() {
    local want=$1 why=$2 pattern=$3 status=$4
    if [ "$want" = pass ] && [ "$status" -eq 0 ]; then
        ok "$why"
    elif [ "$want" = fail ] && [ "$status" -ne 0 ] && grep -qE "$pattern" "$WORK/run.log"; then
        ok "$why"
    else
        bad "$why (exit $status, wanted $want, pattern '$pattern'):"
        sed 's/^/          /' "$WORK/run.log"
    fi
}

machine pi "$WORK/synced"
machine vm "$WORK/synced"
fleet "pi vm" "vm:other"
status=$?
expect pass "two machines that share the file: S, B, F and C pass" "" "$status"
for scenario in "PASS  V: every machine that says runs 0.1.13" "PASS  S pi" "PASS  B pi -> vm" "PASS  F pi -> vm" "PASS  F vm: still absent" "PASS  C vm:other" "PASS  L pi: the test node counts 2 of 2 copies elsewhere: safe"; do
    grep -qF "$scenario" "$WORK/run.log" || bad "the passing run has no '$scenario'"
done

VERSION=0.1.12 machine behind "$WORK/synced"
fleet "pi behind" ""
expect fail "a machine whose daemon runs another build is FAIL V" "^FAIL  V: the machines run different builds: pi=0.1.13 behind=0.1.12" $?

machine island "$WORK/island"
fleet "pi island" ""
expect fail "a file that never arrives is FAIL B" "^FAIL  B pi -> island" $?

# A host whose chunks for test are the plain files: the canary is readable.
mkdir -p "$WORK/leaky-other/vault/owners"
ln -s "$WORK/synced" "$WORK/leaky-other/vault/owners/$TEST_UID"
machine leaky "$WORK/synced" 10 "$WORK/leaky-other"
fleet "pi leaky" "leaky:other"
expect fail "a canary readable in what a host holds for test is FAIL C" "^FAIL  C leaky:other" $?

# A host that holds nothing for test proves nothing: SKIP, never PASS.
NOHOST=1 machine bare "$WORK/synced"
fleet "pi bare" "bare:other"
expect pass "a host holding nothing for test is not a failure" "" $?
grep -qE '^SKIP  C bare:other: other holds nothing' "$WORK/run.log" ||
    bad "a host holding nothing for test was not said SKIP: $(grep 'C bare' "$WORK/run.log")"
# And why, as far as that account's own daemon says: run 40478381966 and
# every night before it said "nicolas holds nothing for test" and nothing
# else, so nobody could tell a zero pledge from a host test never met.
printf 'snapshot %s every 300\nhosting for other people\n  pledged         0 B\n  peers hosted    0\n' \
    "$(date +%s)" >"$WORK/bare/other/status.snapshot"
fleet "pi bare" "bare:other"
grep -qE '^SKIP  C bare:other: .*pledged 0 B, 0 peer\(s\) hosted' "$WORK/run.log" ||
    bad "a C SKIP did not say the host's pledge and how many it hosts: $(grep 'C bare' "$WORK/run.log")"
printf 'snapshot %s every 300\n  pledged         [x](https://evil.example)|**\n  peers hosted    7`rm`\n' \
    "$(date +%s)" >"$WORK/bare/other/status.snapshot"
fleet "pi bare" "bare:other"
if grep 'C bare' "$WORK/run.log" | grep -qE '\]\(|\||\*\*|`'; then
    bad "another account's snapshot put markdown into the run's summary: $(grep 'C bare' "$WORK/run.log")"
fi
rm "$WORK/bare/other/status.snapshot"
fleet "pi bare" "bare:other"
grep -qE '^SKIP  C bare:other: .*no status\.snapshot' "$WORK/run.log" ||
    bad "a C SKIP on a host whose daemon never ran did not say so: $(grep 'C bare' "$WORK/run.log")"

# L: what each machine's test node counts, which is what its icon draws
# (MVP.md L). Short past the grace is said, and still PASS: the row tests
# that the machine tells, not that the fleet is full.
COPIES="1 of 2" machine short "$WORK/synced"
fleet "pi short" ""
grep -qE '^PASS  L short: the test node counts 1 of 2 copies elsewhere: NOT safe, past the grace, so it reads exposed' "$WORK/run.log" ||
    bad "a machine short of copies was not said NOT safe: $(grep ' L short' "$WORK/run.log")"
COPIES="1 of 2 since $(date +%s)" machine saving "$WORK/synced"
fleet "pi saving" ""
grep -qE '^PASS  L saving: .*1 of 2 .*within the grace' "$WORK/run.log" ||
    bad "a shortfall inside its grace was not told apart: $(grep ' L saving' "$WORK/run.log")"
# At the second the grace ends, the icon reads exposed (`Copies::exposed`
# uses >=): the row must not say "within the grace" then. The agent's clock
# and the snapshot's are the same machine's, read a moment apart.
COPIES="1 of 2 since $(( $(date +%s) - 600 ))" machine edge "$WORK/synced"
fleet "pi edge" ""
grep -qE '^PASS  L edge: .*past the grace' "$WORK/run.log" ||
    bad "a shortfall exactly two rounds old was said within the grace: $(grep ' L edge' "$WORK/run.log")"
# A daemon that names its build and stores files, and counts no copies:
# its icon cannot say the files are not safe. FAIL.
COPIES="" machine blind "$WORK/synced"
fleet "pi blind" ""
expect fail "a daemon storing files that counts no copies is FAIL L" "^FAIL  L blind" $?
# Nothing stored: no question to answer, not a failure.
COPIES="" FILES=0 machine empty "$WORK/synced"
fleet "pi empty" ""
grep -qE '^SKIP  L empty: ' "$WORK/run.log" ||
    bad "a test node storing nothing was not SKIP L: $(grep ' L empty' "$WORK/run.log")"

machine stopped "$WORK/synced" 3000
fleet "pi stopped" ""
expect fail "a daemon whose last round is ten intervals old is FAIL S" "^FAIL  S stopped" $?

FLEET_SSH=false fleet "pi gone" ""
expect fail "one machine answering is FAIL: nothing was tested" "fewer than two machines answered" $?
grep -qE '^SKIP  gone does not answer' "$WORK/run.log" || bad "a host that does not answer is not said SKIP"

echo "a SKIP nobody planned turns red after seven nights; a planned one does not"

# C was SKIP every night from 2026-10-24 to 2026-10-29 over a setup defect,
# because a SKIP counted as green for ever (HANDOVER §8 0x). A SKIP that
# waits on an item of "Waiting on Nicolas" is planned and stays SKIP:
# reddening it would reopen "fleet: red" nightly to say what the list says.
NOHOST=1 machine night "$WORK/synced"
day() { date -u -d "@$(( $(date +%s) - $1 * 86400 ))" +%F; }
printf '# label<TAB>item\n' >"$WORK/planned"
nights() {
    FLEET_SKIPS="$WORK/skips" FLEET_PLANNED="$WORK/planned" FLEET_TODAY=$(day "$1") \
        fleet "pi night" "night:other"
}
rm -f "$WORK/skips"
for n in 6 5 4 3 2 1; do nights "$n" >/dev/null; done
grep -qE '^SKIP  C night:other: ' "$WORK/run.log" ||
    bad "an unplanned SKIP on its sixth night was not still SKIP: $(grep 'C night' "$WORK/run.log")"
first=$(day 6)
nights 0
expect fail "an unplanned SKIP on its seventh night is FAIL" "^FAIL  C night:other: SKIP 7 nights in a row since $first: other holds nothing" $?
# A PASS between restarts the count.
rm -f "$WORK/skips"
for n in 6 5 4; do nights "$n" >/dev/null; done
machine night "$WORK/synced"
nights 3 >/dev/null
rm -rf "$WORK/night"; NOHOST=1 machine night "$WORK/synced"
for n in 2 1 0; do nights "$n" >/dev/null; done
grep -qE '^SKIP  C night:other: ' "$WORK/run.log" ||
    bad "a PASS in between did not restart the count: $(grep 'C night' "$WORK/run.log")"
# Planned: seven nights and more, still SKIP, and it says which item.
printf 'C night:other\t1\n' >>"$WORK/planned"
rm -f "$WORK/skips"
for n in 8 7 6 5 4 3 2 1; do nights "$n" >/dev/null; done
nights 0
expect pass "a planned SKIP eight nights old is not a failure" "" $?
grep -qE '^SKIP  C night:other: .*\(planned: Waiting on Nicolas item 1\)' "$WORK/run.log" ||
    bad "a planned SKIP did not name its item: $(grep 'C night' "$WORK/run.log")"

# The planned skips in the repository each name an item the list still has:
# a skip cannot stay planned after its reason is gone.
items=$(awk '/^\*\*Waiting on Nicolas/{on=1} on && /^## /{exit} on && /^[0-9]+\. /{sub(/\..*/,""); print}' docs/HANDOVER.md)
[ -n "$items" ] || bad "no numbered item under \"Waiting on Nicolas\" in HANDOVER §0"
planned_items() {
    grep -vE '^(#|$)' "$1" | while IFS=$'\t' read -r label item; do
        [ -n "$label" ] && [ -n "$item" ] || { echo "malformed: $label"; continue; }
        printf '%s\n' "$items" | grep -qx "$item" || echo "$label names item $item, which Waiting on Nicolas no longer has"
    done
}
stale=$(planned_items scripts/fleet/planned-skips)
[ -z "$stale" ] && ok "every planned fleet skip names a live item of Waiting on Nicolas" || bad "$stale"
printf 'C night:other\t99\n' >"$WORK/stale"
[ -n "$(planned_items "$WORK/stale")" ] || bad "a planned skip naming a gone item passed the gate"

echo "fleet-docker.sh opens the port discovery listens on"

# Another account's node on the home LAN is found by discovery's UDP beacons
# alone (the coordinator lists only the asker's own account), and test C needs
# it found. fleet-docker.sh opened UDP 9797-9799 "for discovery" from
# 2026-10-14 to 2026-10-29 while discovery listens on 21037, so the port is
# read from the source here, not restated.
port=$(sed -n 's/^pub const DEFAULT_PORT: u16 = \([0-9]*\);.*/\1/p' crates/itsanas-discover/src/lan.rs)
if [ -z "$port" ]; then
    bad "no DEFAULT_PORT in crates/itsanas-discover/src/lan.rs: this check reads the wrong place"
elif grep -qE "ufw allow $port/udp" scripts/fleet-docker.sh; then
    ok "fleet-docker.sh lets discovery's UDP $port through ufw"
else
    bad "fleet-docker.sh does not open UDP $port to ufw (itsanas_discover::DEFAULT_PORT): the accounts on the LAN never hear each other, and C finds its host empty"
fi

echo "the fleet workflow can only run from main"

# What keeps the key to Nicolas's machines out of a stranger's hands is not
# code but three lines of fleet.yml: no pull_request trigger, the `fleet`
# environment (main only), and secrets read nowhere else. A later edit that
# adds a trigger "to test it on the PR" undoes all of it, so it is checked.
fleet_yml=.github/workflows/fleet.yml
if python3 - "$fleet_yml" <<'PY'
import re, sys
try:
    import yaml
except ImportError:
    # No PyYAML on this machine: the same two rules, read off the text.
    text = open(sys.argv[1]).read()
    on = re.search(r"^on:\n((?:[ #].*\n|\n)*)", text, re.M).group(1)
    triggers = set(re.findall(r"^  ([a-z_]+):", on, re.M))
    assert triggers <= {"workflow_dispatch", "schedule"}, f"triggers {sorted(triggers)}"
    jobs = re.search(r"^jobs:\n((?:[ #].*\n|\n)*)", text, re.M).group(1)
    names = re.findall(r"^  ([A-Za-z0-9_-]+):", jobs, re.M)
    assert len(re.findall(r"^    environment: fleet$", jobs, re.M)) == len(names), "a job outside fleet"
    sys.exit(0)
doc = yaml.safe_load(open(sys.argv[1]))
on = doc.get(True, doc.get("on"))  # YAML reads a bare `on:` as true
triggers = set(on) if isinstance(on, dict) else {on} if isinstance(on, str) else set(on)
assert triggers <= {"workflow_dispatch", "schedule"}, f"triggers {sorted(triggers)}"
for name, job in doc["jobs"].items():
    assert job.get("environment") == "fleet", f"job {name} runs outside the fleet environment"
PY
then
    ok "fleet.yml runs on schedule or by hand only, every job in the fleet environment"
else
    bad "fleet.yml can run on something else than a schedule or a click, or outside the fleet environment"
fi
if grep -n 'secrets\.FLEET_\|secrets\.TS_' .github/workflows/*.yml | grep -v "^$fleet_yml:" | grep -q .; then
    bad "a workflow other than fleet.yml reads a fleet secret: $(grep -ln 'secrets.FLEET_' .github/workflows/*.yml | grep -v fleet.yml)"
else
    ok "no other workflow reads a fleet secret"
fi

echo "setup.sh installs the key once, and keeps it when run again"
# Run again after every agent change (it copies agent.sh), and the key it
# needs as its argument was the one thing nobody kept at hand: on
# 2026-10-27 the handover had to tell Nicolas to sed it out of
# authorized_keys. A fake HOME with what fleet-docker.sh leaves behind.
SETUP_HOME="$WORK/setup-home"
mkdir -p "$SETUP_HOME/ITSaNAS-test" "$SETUP_HOME/itsanas-data/test/home"
printf 'snapshot %s every 300\naccount\n  user id         %s\n' "$(date +%s)" "$TEST_UID" \
    >"$SETUP_HOME/itsanas-data/test/home/status.snapshot"
CI_KEY="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakeKeyForCheckFleetOnly0000000000000000 itsanas-fleet-ci"
setup() { HOME="$SETUP_HOME" bash scripts/fleet/setup.sh "$@" >"$WORK/setup.log" 2>&1; }
installed() { grep -c 'restrict,command=".*/\.itsanas-fleet/agent\.sh" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFakeKey' "$SETUP_HOME/.ssh/authorized_keys" 2>/dev/null; }
if setup; then
    bad "setup.sh with no key and none installed did not refuse: $(head -2 "$WORK/setup.log")"
else
    ok "setup.sh with no key and none installed refuses"
fi
setup "$CI_KEY" && [ "$(installed)" = 1 ] &&
    ok "setup.sh installs the key behind the agent" || bad "setup.sh did not install the key: $(head -3 "$WORK/setup.log")"
echo "# an old line" >"$SETUP_HOME/.itsanas-fleet/agent.sh"
if setup && [ "$(installed)" = 1 ] && grep -q 'SSH_ORIGINAL_COMMAND' "$SETUP_HOME/.itsanas-fleet/agent.sh"; then
    ok "setup.sh run again with no key keeps the key installed and refreshes the agent"
else
    bad "setup.sh run again with no key did not keep the key (installed $(installed)): $(head -3 "$WORK/setup.log")"
fi

echo "the Windows agent refuses the same requests"

if command -v pwsh >/dev/null 2>&1; then
    # The receipts too, before the listing below: the agent makes the
    # directory for its lock, and a new directory in $WORK changes the
    # folder's `..` line.
    mkdir -p "$WORK/winhome" "$WORK/winreceipts"
    printf '{ "Folder": "%s", "Kit": "%s", "Receipts": "%s", "Homes": { "test": "%s" } }\n' \
        "$WORK/wall-folder" "$PWD/scripts/acceptance.ps1" "$WORK/winreceipts" "$WORK/winhome" >"$WORK/agent.json"
    winagent() { SSH_ORIGINAL_COMMAND="$1" ITSANAS_FLEET_CONF="$WORK/agent.json" pwsh -NoProfile -File scripts/fleet/agent.ps1; }
    before=$(cd "$WORK/wall-folder" && ls -la | sha256sum)
    refused=0
    while IFS= read -r request; do
        out=$(winagent "$request" 2>&1)
        status=$?
        if [ "$status" -eq 64 ] && [ "${out%% *}" = REFUSED ]; then
            refused=$((refused + 1))
        else
            bad "agent.ps1 did not refuse (exit $status): ${request:0:60} -> $(printf '%s' "$out" | head -1)"
        fi
    done <<<"$REQUESTS"
    ok "agent.ps1 refused $refused requests"
    [ ! -e "$WORK/pwned" ] || bad "a request to agent.ps1 ran a command"
    after=$(cd "$WORK/wall-folder" && ls -la | sha256sum)
    [ "$before" = "$after" ] && ok "and left the folder as it was" || bad "a refused request changed the folder"
    out=$(winagent "B write" 2>&1)
    read -r _ _ _ _ _ _ _ wcode _ <<<"$out"
    out=$(winagent "B check $wcode" 2>&1)
    case "$out" in PASS*) ok "agent.ps1 writes and checks by code" ;; *) bad "agent.ps1 B: $out" ;; esac
    if command -v flock >/dev/null 2>&1; then
        flock -s "$WORK/winreceipts/agent.lock" sleep 6 &
        sleep 0.5
        out=$(winagent hello 2>&1)
        status=$?
        wait
        [ "$status" -eq 64 ] && [ "${out%% *}" = REFUSED ] &&
            ok "agent.ps1 refuses a second request while one runs" || bad "agent.ps1 ran a request beside another: exit $status, $out"
    fi
else
    echo "  skip  pwsh is not installed here; agent.ps1 was not tried"
fi

echo "the Windows setup writes what the agent reads, and the key runs only the agent"

# setup.ps1 is what Nicolas runs once on the laptop; a wrong agent.json or a
# keys line sshd cannot run is found there at night, by a SKIP row nobody
# reads. So it is run here on a profile laid out as the laptop's is: a path
# with a space, a test node, another account, a home emptied by `clean`, the
# passphrase file, a keys file with another key and an older line of this one.
if command -v pwsh >/dev/null 2>&1; then
    prof="$WORK/win profile"
    keys="$WORK/winkeys/authorized_keys"
    cikey="ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIFleetCiKeyForCheckFleetOnly0000000000000 itsanas-fleet-ci"
    node() { mkdir -p "$prof/$1"; echo sealed >"$prof/$1/keystore.bin"; printf 'username = %s\n%s' "$2" "${3:+folder = $3
}" >"$prof/$1/config"; }
    node .itsanas-test test "$prof/ITSaNAS-test"
    node .itsanas-nicolas nicolas
    node .itsanas-gone gone
    rm "$prof/.itsanas-gone/keystore.bin"
    mkdir -p "$prof/ITSaNAS-test" "$(dirname "$keys")"
    echo secret >"$prof/.itsanas-passphrase"
    printf 'ssh-ed25519 AAAAOtherKeyStaysAsItIs nicolas@laptop\nfrom="10.0.0.1" ssh-ed25519 %s old-options\n' \
        "$(echo "$cikey" | cut -d' ' -f2)" >"$keys"
    setup() { ITSANAS_FLEET_PROFILE="$prof" ITSANAS_FLEET_KEYS="$keys" pwsh -NoProfile -File scripts/fleet/setup.ps1 "$@"; }
    out=$(setup "$cikey" 2>&1) && out=$(setup "$cikey" 2>&1)
    status=$?
    [ "$status" -eq 0 ] && ok "setup.ps1 runs twice" || bad "setup.ps1 (exit $status): $(printf '%s' "$out" | head -3)"
    conf="$prof/.itsanas-fleet/agent.json"
    homes=$(pwsh -NoProfile -Command "(Get-Content -LiteralPath '$conf' -Raw | ConvertFrom-Json).Homes.PSObject.Properties.Name -join ' '" 2>&1)
    [ "$homes" = "nicolas test" ] &&
        ok "agent.json names the two nodes, not the emptied home, the passphrase file or its own folder" ||
        bad "agent.json homes: '$homes'"
    grep -qF "\"Folder\": \"$prof/ITSaNAS-test\"" "$conf" && ok "and the test account's folder" || bad "agent.json folder: $(grep Folder "$conf")"
    grep -qxF 'ssh-ed25519 AAAAOtherKeyStaysAsItIs nicolas@laptop' "$keys" && ok "another key is left as it was" || bad "setup.ps1 changed another key"
    lines=$(grep -cF "$(echo "$cikey" | cut -d' ' -f2)" "$keys")
    [ "$lines" -eq 1 ] && grep -q '^restrict,command=' "$keys" &&
        ok "the CI key is there once, behind restrict,command=" || bad "the CI key is in $lines lines: $(cat "$keys")"
    # What sshd on Linux does with the line: the command, de-quoted, run by
    # the user's shell with the request aside. A quote wrong around the space
    # in the profile shows here.
    cmd=$(grep -F 'itsanas-fleet-ci' "$keys" | sed -n 's/^restrict,command="\(.*\)" ssh-ed25519 .*/\1/p' | sed 's/\\"/"/g')
    out=$(SSH_ORIGINAL_COMMAND=hello sh -c "$cmd" 2>&1)
    case "$out" in "hello "*"accounts nicolas test kit "*) ok "the keys line runs the installed agent" ;; *) bad "the keys line: '$cmd' -> $out" ;; esac
    out=$(SSH_ORIGINAL_COMMAND="sh" sh -c "$cmd" 2>&1)
    [ "${out%% *}" = REFUSED ] && ok "and that agent refuses a shell" || bad "the installed agent ran sh: $out"
    out=$(SSH_ORIGINAL_COMMAND="B write" sh -c "$cmd" 2>&1)
    read -r _ _ _ _ _ _ _ wcode _ <<<"$out"
    out=$(SSH_ORIGINAL_COMMAND="B check $wcode" sh -c "$cmd" 2>&1)
    case "$out" in PASS*) ok "and writes into the test folder through the kit it copied" ;; *) bad "B through the installed agent: $out" ;; esac
    before=$(sha256sum "$keys")
    out=$(setup "ssh-rsa AAAAB3NzaC1yc2E itsanas-fleet-ci" 2>&1)
    status=$?
    out2=$(setup "ssh-ed25519 AAAA\" ssh-ed25519 BBBB" 2>&1)
    status2=$?
    [ "$status" -ne 0 ] && [ "$status2" -ne 0 ] && [ "$before" = "$(sha256sum "$keys")" ] &&
        ok "a key that is not one plain ed25519 key is refused, the keys file untouched" ||
        bad "setup.ps1 took a bad key (exit $status, $status2): $out $out2"
    bad_prof="$WORK/win%PATH%prof"
    mkdir -p "$bad_prof/ITSaNAS-test"
    cp -r "$prof/.itsanas-test" "$bad_prof/"
    sed -i "s|^folder = .*|folder = $bad_prof/ITSaNAS-test|" "$bad_prof/.itsanas-test/config"
    out=$(ITSANAS_FLEET_PROFILE="$bad_prof" ITSANAS_FLEET_KEYS="$keys" pwsh -NoProfile -File scripts/fleet/setup.ps1 "$cikey" 2>&1)
    status=$?
    [ "$status" -ne 0 ] && [ "$before" = "$(sha256sum "$keys")" ] &&
        ok "a profile whose path cmd.exe would expand is refused, the keys file untouched" ||
        bad "setup.ps1 wrote a keys line for a path with a % (exit $status): $out"
    mv "$prof/.itsanas-test" "$prof/.itsanas-test-away"
    rm "$prof/.itsanas-test-away/keystore.bin"
    out=$(setup "$cikey" 2>&1)
    status=$?
    [ "$status" -ne 0 ] && [ "$before" = "$(sha256sum "$keys")" ] && case "$out" in *"no node of the account test"*) true ;; *) false ;; esac &&
        ok "a profile without a test node is refused before the key goes in" || bad "setup.ps1 without a test node (exit $status): $out"
else
    echo "  skip  pwsh is not installed here; setup.ps1 was not tried"
fi

echo "the Windows kit writes what the shell kit reads, and the other way round"

# The laptop and Windows partners run scripts/acceptance.ps1, a Mac or the
# fleet the shell kit: a code one prints must be found by the other, and a
# canary planted on Windows must have the digest run.sh works out for it.
if command -v pwsh >/dev/null 2>&1; then
    both="$WORK/both"
    mkdir -p "$both"
    export ITSANAS_RECEIPTS="$WORK/receipts"
    ps() { pwsh -NoProfile -File scripts/acceptance.ps1 "$@"; }
    out=$(ps B write "$both" 2>&1)
    read -r _ _ _ pname _ psha _ pcode _ <<<"$out"
    bash scripts/acceptance.sh B check "$both" "$pcode" >/dev/null 2>&1 &&
        ok "the shell kit finds a Windows write by its code" || bad "shell B check of a Windows code: $out"
    bash scripts/acceptance.sh B check "$both" "$pname" "$psha" >/dev/null 2>&1 &&
        ok "and by its name and digest" || bad "shell B check of a Windows name: $out"
    out=$(bash scripts/acceptance.sh B write "$both" 2>&1)
    read -r _ _ _ _ _ _ _ scode _ <<<"$out"
    ps B check "$both" "$scode" >/dev/null 2>&1 &&
        ok "the Windows kit finds a shell write by its code" || bad "Windows B check of a shell code: $out"
    if ps B check "$both" ffff-ffff >/dev/null 2>&1; then
        bad "the Windows kit passed a code no file has"
    else
        ok "the Windows kit fails a code no file has"
    fi
    out=$(ps C plant "$both" 2>&1)
    read -r _ _ _ cfile _ canary _ <<<"$out"
    want=$(printf 'A private note.\n%s\n' "$canary" | sha256sum | cut -d' ' -f1)
    [ "$(sha256sum "$both/$cfile" | cut -d' ' -f1)" = "$want" ] &&
        ok "a Windows canary has the digest run.sh expects" || bad "a Windows canary differs from the shell kit's: $out"
    ps F delete "$both" "$pname" >/dev/null 2>&1 && [ ! -e "$both/$pname" ] &&
        ps F check "$both" "$pname" >/dev/null 2>&1 &&
        ok "the Windows kit deletes and checks the deletion" || bad "Windows F delete/check"
else
    echo "  skip  pwsh is not installed here; the Windows kit was not compared"
fi

echo
if [ "$failures" -eq 0 ]; then
    echo "check-fleet: the agent refuses and the run fails where it must"
fi
[ "$failures" -eq 0 ]
