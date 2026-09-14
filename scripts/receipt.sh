#!/usr/bin/env bash
# One command, one receipt: the whole verification, run on the machine it is
# about, reduced to a dozen lines that can be read without the logs.
#
#   bash scripts/receipt.sh           # the nine gates, then every test
#   bash scripts/receipt.sh --quick   # tests only
#
# Why this exists
# ---------------
#
# Tests were being run by AI agents, one per question, each re-reading the
# whole project to do it. That cost far more than the tests and answered less:
# a green CI run or a receipt from the Raspberry Pi says the same thing for
# nothing. Run this on the Pi or the Freebox VM (`ssh pi 'cd itsanas && git pull
# -q && bash scripts/receipt.sh'`) and paste the receipt, or read CI.
#
# Full logs go to ~/.itsanas-receipts/<stamp>/, the receipt to
# ~/.itsanas-receipts/<stamp>.txt and to standard output. Exit status is 0 only
# when everything passed.

set -uo pipefail
cd "$(dirname "$0")/.." || exit 2

quick=0
[ "${1:-}" = "--quick" ] && quick=1

stamp=$(date -u +%Y%m%dT%H%M%SZ)
root="$HOME/.itsanas-receipts"
logs="$root/$stamp"
mkdir -p "$logs"
started=$(date +%s)

commit=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
if [ -n "$(git status --porcelain 2>/dev/null)" ]; then
    commit="$commit (uncommitted changes)"
fi

result=PASS

gates="skipped (--quick)"
if [ "$quick" -eq 0 ]; then
    if bash scripts/check-all.sh >"$logs/gates.log" 2>&1; then
        gates="ok"
    else
        gates="FAIL: $(grep -E '^FAIL' "$logs/gates.log" | awk '{print $2}' | tr '\n' ' ')"
        result=FAIL
    fi
fi

# nextest and nothing else: it is the runner that enforces each test's deadline
# (.config/nextest.toml), and a plain `cargo test` on a slow machine can hang
# for as long as nobody is watching.
if ! cargo nextest --version >/dev/null 2>&1; then
    tests="not run: cargo-nextest is missing (cargo install cargo-nextest --locked)"
    failed="-"
    result=FAIL
else
    cargo nextest run --workspace >"$logs/tests.log" 2>&1 || result=FAIL
    tests=$(grep -E '^[[:space:]]*Summary' "$logs/tests.log" | tail -1 | sed -E 's/^[[:space:]]+//')
    [ -n "$tests" ] || tests="no summary: the build failed, see tests.log"
    failed=$(grep -E '^[[:space:]]*FAIL \[' "$logs/tests.log" | awk '{print $NF}' | sort -u | head -12 | tr '\n' ' ')
    [ -n "$failed" ] || failed="none"
fi

receipt="ITSaNAS receipt   $stamp
host      $(uname -n) ($(uname -m), $(uname -s) $(uname -r))
commit    $commit
rustc     $(rustc --version 2>/dev/null | awk '{print $2}')
gates     $gates
tests     $tests
failed    $failed
duration  $(( $(date +%s) - started )) s
logs      $logs
RESULT    $result"

printf '%s\n' "$receipt" | tee "$root/$stamp.txt"
[ "$result" = PASS ]
