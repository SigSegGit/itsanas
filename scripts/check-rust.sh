#!/usr/bin/env bash
# The three cargo gates CI runs, in one place, so `scripts/check-all.sh` covers
# them too.
#
# Why this exists
# ---------------
#
# The other scripts here check documentation, messages, installers and test
# budgets, and running them all felt like running everything. It was not:
# `cargo fmt --check`, `cargo clippy -D warnings` and the documentation build
# live only in CI, so a push could pass every local gate and fail on
# formatting -- which is exactly what happened on 2026-09-06, on a commit whose
# author had run the local gates and read the clippy output and still not run
# `fmt`.
#
# A checklist a person has to remember is a checklist that has already failed.
#
# Skipping rather than failing
# ----------------------------
#
# `scripts/check-all.sh` is often run from a shell that has no Rust toolchain --
# WSL, on a repository checked out on the Windows side. A gate that fails there
# would teach people to ignore it, so this one says it was skipped and exits 0.
# The failure it prevents is a slip, not a compromise, and CI runs the same
# three unconditionally.

cd "$(dirname "$0")/.."

if ! command -v cargo >/dev/null 2>&1; then
    echo "no cargo on this PATH; fmt, clippy and doc were not run"
    echo "(CI runs all three on every push)"
    exit 0
fi

failed=0

echo "== cargo fmt --all --check"
if cargo fmt --all --check; then
    echo "   formatting is what rustfmt would write"
else
    echo "   FAIL: run \`cargo fmt --all\`"
    failed=1
fi

echo "== cargo clippy --workspace --all-targets --all-features -- -D warnings"
if cargo clippy --workspace --all-targets --all-features -- -D warnings 2>&1 |
    grep -E '^(error|warning)' | head -20; then
    :
fi
if cargo clippy --workspace --all-targets --all-features -- -D warnings >/dev/null 2>&1; then
    echo "   no clippy findings"
else
    echo "   FAIL: clippy has findings (shown above)"
    failed=1
fi

echo "== cargo doc --workspace --no-deps --all-features"
if RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features >/dev/null 2>&1; then
    echo "   the documentation builds with no warnings"
else
    echo "   FAIL: rustdoc has warnings. Four-space indentation in a /// comment"
    echo "         is a Rust code block, which rustdoc then tries to compile."
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features 2>&1 |
        grep -E '^(error|warning)' | head -10
    failed=1
fi

exit "$failed"
