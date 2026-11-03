#!/usr/bin/env bash
# What CI's "Format and lint" refuses most often, before a push and not after.
#
# Why this exists: on one batch (#365) four pushes went red one after the other,
# on clippy, on the format, on rustdoc and on the catalogue. Each failure costs a
# CI round of twenty minutes and cancels the sabotage run, and each takes under a
# minute to find here. `check-all.sh` is the complete list and too slow to run
# before every push: this is the fast part of it, and it collects every failure
# rather than stopping at the first, so one push fixes them all.
#
# Not here, because they are slow and rarely the cause: check-wired.py and the
# shell gates other than the catalogue. Tests stay with CI.
#
# Usage: bash scripts/preflight.sh [base]   (default origin/main)
# Run it by hand before a push, or from a local .git/hooks/pre-push.

cd "$(dirname "$0")/.." || exit 2
base=${1:-origin/main}
python=python3
python3 -c '' >/dev/null 2>&1 || python=python
failed=0

run() {
    local name=$1
    shift
    if output=$("$@" 2>&1); then
        printf 'ok    %s\n' "$name"
    else
        printf 'FAIL  %s\n' "$name"
        printf '%s\n' "$output" | grep -E '^(error|warning)|^ +--> |^Diff in ' | head -20 | sed 's/^/        /'
        printf '%s\n' "$output" | tail -5 | sed 's/^/        /'
        failed=1
    fi
}

# Committed since the base, and what is not committed yet.
changed=$( { git diff --name-only "$base"...HEAD; git diff --name-only HEAD; } | sort -u)
crates=$(printf '%s\n' "$changed" | sed -n 's#^crates/\([^/]*\)/.*#-p \1#p' | sort -u | tr '\n' ' ')

run 'cargo fmt' cargo fmt --all --check
if [ -n "$crates" ]; then
    # shellcheck disable=SC2086 # one word per flag and per crate, on purpose
    run 'cargo clippy' cargo clippy $crates --all-targets --all-features -- -D warnings
    # shellcheck disable=SC2086
    RUSTDOCFLAGS='-D warnings' run 'cargo doc' cargo doc $crates --no-deps --all-features
fi

run 'check-catalogue.sh' bash scripts/check-catalogue.sh
for gate in scripts/check-*.py; do
    case "$gate" in
        scripts/check-wired.py) continue ;;
    esac
    run "$(basename "$gate")" "$python" "$gate"
done

if [ "$failed" -ne 0 ]; then
    echo 'preflight: not ready to push. A gate that rewrites (check-wiki.py --write): rewrite, commit.'
fi
exit "$failed"
