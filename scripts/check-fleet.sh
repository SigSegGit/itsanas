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
    printf 'snapshot %s every 300\naccount\n  username        test\n' \
        "$(($(date +%s) - age))" >"$WORK/$name/test/status.snapshot"
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
for scenario in "PASS  S pi" "PASS  B pi -> vm" "PASS  F pi -> vm" "PASS  F vm: still absent" "PASS  C vm:other"; do
    grep -qF "$scenario" "$WORK/run.log" || bad "the passing run has no '$scenario'"
done

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

machine stopped "$WORK/synced" 3000
fleet "pi stopped" ""
expect fail "a daemon whose last round is ten intervals old is FAIL S" "^FAIL  S stopped" $?

FLEET_SSH=false fleet "pi gone" ""
expect fail "one machine answering is FAIL: nothing was tested" "fewer than two machines answered" $?
grep -qE '^SKIP  gone does not answer' "$WORK/run.log" || bad "a host that does not answer is not said SKIP"

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

echo "the Windows agent refuses the same requests"

if command -v pwsh >/dev/null 2>&1; then
    mkdir -p "$WORK/winhome"
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
else
    echo "  skip  pwsh is not installed here; agent.ps1 was not tried"
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
