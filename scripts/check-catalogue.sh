#!/usr/bin/env bash
# Every test name cited in docs/TESTING.md must exist in the source.
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
# The check is deliberately one-directional. Some crates are catalogued by
# property rather than test by test — `itsanas-coord` is a library with no
# server yet, where what matters is which rule each group of tests pins down —
# so requiring every test to be named would produce noise rather than accuracy.
# Naming a test that does not exist is always wrong; not naming one is not.
set -euo pipefail

cd "$(dirname "$0")/.."

# Identifier-shaped tokens in backticks, long enough that a struct field or a
# short method name does not qualify. Test names in this project are sentences,
# so the threshold costs nothing.
names=$(grep -oE '`[a-z][a-z0-9_]{14,}`' docs/TESTING.md | tr -d '`' | sort -u)

missing=()
for name in $names; do
    if ! grep -rqF "fn ${name}(" crates --include='*.rs'; then
        missing+=("$name")
    fi
done

if [ ${#missing[@]} -ne 0 ]; then
    echo "docs/TESTING.md names tests that do not exist in crates/:"
    printf '  %s\n' "${missing[@]}"
    echo
    echo "Either the test was renamed or deleted and its entry was left behind,"
    echo "or the entry describes a test somebody meant to write. Both make the"
    echo "catalogue read as evidence for something nothing is checking."
    exit 1
fi

echo "test catalogue: all $(echo "$names" | wc -w | tr -d ' ') names cited in docs/TESTING.md exist"
