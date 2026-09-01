#!/usr/bin/env bash
# Run every gate in this directory and report which ones failed.
#
# The gates are separate programs on purpose -- each one answers a different
# question and CI runs them as separate jobs so a failure names itself. This is
# for the other case: a person, or an agent, about to commit, who wants one
# command rather than five and wants the failures collected instead of stopping
# at the first.
#
# It discovers them rather than listing them. A gate added to this directory and
# not added to a list here would be a gate nobody runs, which is the exact
# failure `check-installers.sh` was written to prevent for the installers.

cd "$(dirname "$0")/.."
failed=0
log=$(mktemp)

for gate in scripts/check-*.sh scripts/check-*.py; do
    case "$gate" in
        scripts/check-all.sh) continue ;;
    esac

    case "$gate" in
        *.py) runner=python3 ;;
        *)    runner=bash ;;
    esac

    if command -v "$runner" >/dev/null 2>&1; then
        if "$runner" "$gate" >"$log" 2>&1; then
            printf 'ok    %s\n' "$gate"
        else
            printf 'FAIL  %s\n' "$gate"
            sed 's/^/        /' "$log"
            failed=1
        fi
    else
        printf 'skip  %s (no %s here)\n' "$gate" "$runner"
    fi
done

rm -f "$log"
exit "$failed"
