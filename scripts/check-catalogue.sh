#!/usr/bin/env bash
# Every test name cited in the docs must exist in the source.
#
# This exists because it caught real drift. A documentation audit found the
# catalogue listing three `transport` tests that had been deleted along with the
# `Exposure` type — including one in bold, as the security property that
# mattered, whose replacement asserts the opposite — plus a whole section for a
# module that had moved to another crate, and an entry describing behaviour the
# named test did not have.
#
# A catalogue whose entries may or may not correspond to anything is worse than
# no catalogue: it gets read as evidence. The house rule in CONTRIBUTING.md says
# every test gets an entry saying what it proves; this makes the half of that
# rule which can be checked mechanically actually get checked.
#
# HANDOVER.md is checked too, and for a sharper reason: its invariant table
# names, for each rule, the test that pins it down. That is the table somebody
# reads before changing code they did not write. It was citing
# `a_host_that_starts_answering_again_is_sent_data_again` — a name missing one
# word, which had never existed — as the evidence for a rule about not cutting
# a paused peer out of the log. A rule whose evidence is a typo is a rule with
# no evidence, and it read exactly like one with some.
#
# The check is deliberately one-directional. Some crates are catalogued by
# property rather than test by test — `itsanas-coord` is a library with no
# server yet, where what matters is which rule each group of tests pins down —
# so requiring every test to be named would produce noise rather than accuracy.
# Naming a test that does not exist is always wrong; not naming one is not.
set -euo pipefail

cd "$(dirname "$0")/.."

docs=(docs/TESTING.md docs/HANDOVER.md)

# Identifier-shaped tokens in backticks, long enough that a struct field or a
# short method name does not qualify. Test names in this project are sentences,
# so the threshold costs nothing.
#
# Non-test identifiers that clear the length bar are named here rather than
# excluded by a pattern, so that adding one is a decision somebody makes on
# purpose.
not_a_test='^(MAX_SEGMENTS_WALKED|FAILURES_BEFORE_PAUSE|CHALLENGES_PER_ROUND)$'

total=0
missing=()
for doc in "${docs[@]}"; do
    names=$(grep -oE '`[a-z][a-z0-9_]{14,}`' "$doc" | tr -d '`' | sort -u)
    for name in $names; do
        if [[ "$name" =~ $not_a_test ]]; then
            continue
        fi
        total=$((total + 1))
        if ! grep -rqF "fn ${name}(" crates --include='*.rs'; then
            missing+=("$doc: $name")
        fi
    done
done

if [ ${#missing[@]} -ne 0 ]; then
    echo "documentation names tests that do not exist in crates/:"
    printf '  %s\n' "${missing[@]}"
    echo
    echo "Either the test was renamed or deleted and its entry was left behind,"
    echo "or the entry describes a test somebody meant to write. Both make the"
    echo "documentation read as evidence for something nothing is checking."
    exit 1
fi

echo "test catalogue: all ${total} names cited in ${docs[*]} exist"
