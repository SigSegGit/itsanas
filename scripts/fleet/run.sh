#!/usr/bin/env bash
# The fleet tests: the acceptance kit's phases across the real machines,
# driven from one place, each ending in PASS, FAIL or SKIP with the numbers.
#
#   FLEET_HOSTS="pi vm" bash scripts/fleet/run.sh
#
# Why this exists
# ---------------
#
# docs/FLEET-CI.md. In short: the hand test of 2026-10-22 took a day and an
# ordeal for the partner beside Nicolas, and most of it needed no hand. Each
# machine runs `scripts/fleet/agent.sh` behind a key that can run nothing
# else; this script asks each agent for one phase at a time, waits for the
# result to cross the network, and says what it saw.
#
# How a host is reached
# ---------------------
#
# By `ssh <host> <request>`, with whatever ~/.ssh/config says about <host>
# (the workflow writes it from a secret: jump host, port, pinned key). Or,
# when FLEET_AGENT_<host> names an agent configuration, by running the agent
# here with that configuration: that is how the runner takes part as a member
# of the account on a network that is not home (test O), and how
# scripts/check-fleet.sh tests this file without any fleet.
#
# Environment
# -----------
#
#   FLEET_HOSTS     the hosts, in order; the first that answers writes (required)
#   FLEET_SCAN      "host:account ..." -- homes of *other* accounts to scan for
#                   the canary (test C); none means C is SKIP
#   FLEET_WAIT      seconds a change has to reach another machine (default 900)
#   FLEET_POLL      seconds between looks (default 20)
#   FLEET_STALE     a snapshot older than this many intervals is a daemon not
#                   taking rounds (default 3)
#   FLEET_SSH       the ssh program (default ssh)
#   FLEET_TIMEOUT   seconds a host has to answer a request (default 20)
#   FLEET_SCAN_TIMEOUT  seconds a C scan may take (default 600)
#   FLEET_SETTLE    seconds after F before looking again that the file stayed
#                   gone: longer than a round (default 360)

set -uo pipefail

HOSTS=${FLEET_HOSTS:?FLEET_HOSTS names the hosts, e.g. "pi vm"}
SCAN=${FLEET_SCAN:-}
WAIT=${FLEET_WAIT:-900}
POLL=${FLEET_POLL:-20}
STALE=${FLEET_STALE:-3}
SSH=${FLEET_SSH:-ssh}
TIMEOUT=${FLEET_TIMEOUT:-20}
SETTLE=${FLEET_SETTLE:-360}
SCAN_TIMEOUT=${FLEET_SCAN_TIMEOUT:-600}
AGENT="$(cd "$(dirname "$0")" && pwd)/agent.sh"

failures=0
tested=0
results=()

say() { printf '%s\n' "$*"; }
# One row of the verdict table. FAIL counts; SKIP is said, with why, and
# never counts as a pass.
row() {
    local result=$1
    shift
    results+=("$result  $*")
    say "$result  $*"
    case "$result" in
        FAIL) failures=$((failures + 1)); tested=$((tested + 1)) ;;
        PASS) tested=$((tested + 1)) ;;
    esac
}

# Ask a host's agent for one request; its output on stdout, its exit status.
ask() {
    local host=$1
    shift
    local var="FLEET_AGENT_$host" conf
    conf=${!var:-}
    if [ -n "$conf" ]; then
        SSH_ORIGINAL_COMMAND="$*" ITSANAS_FLEET_CONF="$conf" timeout "$TIMEOUT" sh "$AGENT"
    else
        # BatchMode: a host asking for a password or a new host key is a
        # host that does not answer, not a run that hangs.
        timeout "$TIMEOUT" "$SSH" -o BatchMode=yes -o ConnectTimeout="$TIMEOUT" "$host" "$*" </dev/null
    fi
}

# Ask until a request's output starts with PASS, or the deadline passes.
# Prints the last answer and how long it took.
until_pass() {
    local host=$1
    shift
    local started out
    started=$(date +%s)
    while :; do
        out=$(ask "$host" "$@" 2>&1)
        case "$out" in
            PASS*) printf '%s (after %ss)' "$(printf '%s' "$out" | head -1 | cut -c7- | cut -d'[' -f1)" "$(( $(date +%s) - started ))"; return 0 ;;
        esac
        if [ $(( $(date +%s) - started + POLL )) -gt "$WAIT" ]; then
            printf '%s (gave up after %ss)' "$(printf '%s' "$out" | head -1 | cut -d'[' -f1)" "$(( $(date +%s) - started ))"
            return 1
        fi
        sleep "$POLL"
    done
}

# ------------------------------------------------------------------ who answers

up=()
for host in $HOSTS; do
    if answer=$(ask "$host" hello 2>&1) && [ "${answer%% *}" = hello ]; then
        up+=("$host")
        say "up    $host: $answer"
    else
        row SKIP "$host does not answer: $(printf '%s' "$answer" | head -1)"
    fi
done

if [ ${#up[@]} -lt 2 ]; then
    row FAIL "fewer than two machines answered (${up[*]:-none}): nothing crossed a network, so nothing was tested"
else
    writer=${up[0]}
    others=("${up[@]:1}")

    # ---------------------------------------------------- S: daemons take rounds
    # A snapshot is written after every round; one older than a few intervals
    # is a daemon that stopped, hung, or is not running at all -- which is
    # what the laptop's three daemons on 0.1.8 looked like (hand test).
    versions=""
    for host in "${up[@]}"; do
        out=$(ask "$host" status test 2>&1)
        read -r _ taken every _ now <<<"$(printf '%s' "$out" | head -1)"
        # The runner (member.sh) runs this checkout, not a release: only the
        # machines' own daemons are compared below.
        if [ "$host" != runner ]; then
            version=$(printf '%s\n' "$out" | sed -n 's/^version //p' | head -1)
            versions="$versions $host=${version:-unknown}"
        fi
        if [[ "$taken" =~ ^[0-9]+$ && "$every" =~ ^[0-9]+$ && "$now" =~ ^[0-9]+$ ]]; then
            age=$((now - taken))
            if [ "$age" -le $((STALE * every + 60)) ]; then
                row PASS "S $host: the test node's last round was ${age}s ago (every ${every}s)"
            else
                row FAIL "S $host: the test node's last round was ${age}s ago, more than $STALE intervals of ${every}s: its daemon is not taking rounds"
            fi
        else
            row FAIL "S $host: no snapshot: $(printf '%s' "$out" | head -1)"
        fi
    done

    # ------------------------------------------- V: every machine runs one build
    # An update that restarted nothing leaves daemons on the old program
    # while the new one is on disk (the laptop, 2026-10-22). The snapshot
    # says which build the running daemon is.
    known=$(printf '%s\n' $versions | sed -n 's/^[^=]*=//p' | grep -v '^unknown$' | sort -u)
    if [ -z "$versions" ]; then
        :
    elif [ -z "$known" ]; then
        row SKIP "V: no machine's snapshot names its version (daemons from 0.1.12 or before)"
    elif [ "$(printf '%s\n' "$known" | wc -l)" -gt 1 ]; then
        row FAIL "V: the machines run different builds:$versions"
    else
        row PASS "V: every machine that says runs $known:$versions"
    fi

    # ------------------------------------------------- B: a file appears elsewhere
    out=$(ask "$writer" B write 2>&1)
    read -r verdict _ _ name _ sha _ <<<"$(printf '%s' "$out" | head -1)"
    if [ "$verdict" != PASS ] || [ -z "${sha:-}" ]; then
        row FAIL "B $writer could not write: $(printf '%s' "$out" | head -1)"
    else
        b_ok=1
        for host in "${others[@]}"; do
            if said=$(until_pass "$host" B check "$name" "$sha"); then
                row PASS "B $writer -> $host: $said"
            else
                row FAIL "B $writer -> $host: $said"
                b_ok=0
            fi
        done

        # ------------------------------------------- F: deleting means deleting
        if [ "$b_ok" = 1 ]; then
            out=$(ask "$writer" F delete "$name" 2>&1)
            if [ "${out%% *}" != PASS ]; then
                row FAIL "F $writer could not delete $name: $(printf '%s' "$out" | head -1)"
            else
                for host in "${others[@]}"; do
                    if said=$(until_pass "$host" F check "$name"); then
                        row PASS "F $writer -> $host: $said"
                    else
                        row FAIL "F $writer -> $host: $said"
                    fi
                done
                # Gone once is half of F: a peer that had not heard of the
                # delete can push the file back at its next round. Looked at
                # again after a settle long enough for every daemon's round.
                sleep "$SETTLE"
                for host in "${others[@]}"; do
                    out=$(ask "$host" F check "$name" 2>&1)
                    case "$out" in
                        PASS*) row PASS "F $host: still absent ${SETTLE}s later" ;;
                        *) row FAIL "F $host: came back within ${SETTLE}s: $(printf '%s' "$out" | head -1)" ;;
                    esac
                done
            fi
        else
            row SKIP "F: B did not arrive everywhere, so there is nothing to delete everywhere"
        fi
    fi

    # ------------------------------------------- C: the host cannot read it
    if [ -z "$SCAN" ]; then
        row SKIP "C: FLEET_SCAN names no other account's home to scan"
    else
        out=$(ask "$writer" C plant 2>&1)
        read -r verdict _ _ file _ canary _ <<<"$(printf '%s' "$out" | head -1)"
        if [ "$verdict" != PASS ] || [ -z "${canary:-}" ]; then
            row FAIL "C $writer could not plant a canary: $(printf '%s' "$out" | head -1)"
        else
            # The canary's chunks have left the writer once the file is whole
            # on another machine of the account: wait for that first, or the
            # scan below looks at a host that has not been sent anything yet.
            # Its digest is known here: c_plant writes exactly this text.
            canary_sha=$(printf 'A private note.\n%s\n' "$canary" | sha256sum | cut -d' ' -f1)
            if said=$(until_pass "${others[0]}" B check "$file" "$canary_sha"); then
                say "      the canary reached ${others[0]}: $said"
            else
                row FAIL "C: the canary never reached ${others[0]}, so no host can be said to hold it: $said"
            fi
            for target in $SCAN; do
                host=${target%%:*}
                account=${target#*:}
                if [[ " ${up[*]} " != *" $host "* ]]; then
                    row SKIP "C $host:$account: $host does not answer"
                    continue
                fi
                out=$(TIMEOUT=$SCAN_TIMEOUT ask "$host" C scan "$canary" "$account" 2>&1)
                case "$out" in
                    PASS*) row PASS "C $host:$account: $(printf '%s' "$out" | head -1 | cut -c7- | cut -d'[' -f1)" ;;
                    NOTHOSTED*) row SKIP "C $host:$account: ${out#NOTHOSTED }" ;;
                    *) row FAIL "C $host:$account: $(printf '%s' "$out" | tail -1)" ;;
                esac
            done
            ask "$writer" F delete "$file" >/dev/null 2>&1 || true
        fi
    fi
fi

# ------------------------------------------------------------------ verdict

say
say "fleet: $tested checked, $failures failed, ${#up[@]} of $(wc -w <<<"$HOSTS" | tr -d ' ') machines answered"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
        echo "### Fleet tests"
        echo
        echo '```'
        printf '%s\n' "${results[@]}"
        echo '```'
    } >>"$GITHUB_STEP_SUMMARY"
fi
[ "$failures" -eq 0 ]
