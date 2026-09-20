#!/usr/bin/env bash
# Merge a pull request only when *every* check has passed, and refuse otherwise.
#
# Why this exists
# ---------------
#
# `docs/HANDOVER.md` §5 says it plainly: one check that is red, pending or
# skipped is not a green. On 2026-09-18 a pull request was merged with two
# checks still running -- "Builds and runs on ARM" and "Expensive tests", the
# two slowest and the two most likely to catch a portability bug. Nothing broke,
# which is luck rather than evidence.
#
# The mistake was not impatience. It was this shell:
#
#     gh pr checks 59 | grep -civ pass && gh pr merge 59 ...
#
# `grep -c` exits 0 when it *finds* matches, so finding three non-passing lines
# ran the merge. A negated count is the wrong shape for a gate: it has to say
# "all of them passed", never "I did not see a failure".
#
# So this script counts what passed, counts what exists, and merges only if the
# two are equal and there are enough of them to be the real suite. It says which
# check is holding it up when it refuses, because "not green" without a name
# sends somebody to the web interface.
#
#   bash scripts/merge-when-green.sh <pr-number> [minimum-checks]

set -uo pipefail

PR=${1:-}
MINIMUM=${2:-15}

[ -n "$PR" ] || { echo "usage: merge-when-green.sh <pr-number> [minimum-checks]"; exit 2; }

checks=$(gh pr checks "$PR" 2>&1) || true

# No checks at all is the state a pull request is in for the first minute of its
# life, and it reads identically to a repository with no CI. Refusing is the
# only safe reading: a merge here is a merge with nothing verified.
if printf '%s' "$checks" | grep -q "no checks reported"; then
    echo "PR $PR: no checks have reported yet. Nothing has been verified."
    exit 1
fi

total=$(printf '%s\n' "$checks" | grep -c .)
passing=$(printf '%s\n' "$checks" | grep -c $'\tpass\t')

if [ "$total" -lt "$MINIMUM" ]; then
    echo "PR $PR: only $total checks are reporting, and this suite has $MINIMUM."
    echo "Some have not started. Waiting is the answer; merging is not."
    exit 1
fi

if [ "$passing" -ne "$total" ]; then
    echo "PR $PR: $passing of $total checks passed. These are not green:"
    printf '%s\n' "$checks" | grep -v $'\tpass\t' | sed 's/^/  /'
    echo ""
    echo "Red, pending and skipped are all 'not green'. Fix or wait."
    exit 1
fi

echo "PR $PR: $passing of $total checks passed. Merging."
gh pr merge "$PR" --squash --delete-branch
