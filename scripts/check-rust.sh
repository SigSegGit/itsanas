#!/usr/bin/env bash
# The cargo gates CI runs, in one place, so `scripts/check-all.sh` covers them
# too: fmt, clippy, the documentation build, and cargo deny.
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
# cargo deny joined on 2026-09-14. It had failed CI twice in one week: on
# 2026-09-07 four pushes in a row carried a dependency flagged unsound with no
# fixed version (RUSTSEC-2022-0040), because nothing local ever asked; and on
# 2026-09-14 an advisory published that day against rustls turned main red.
# The second cannot be prevented locally, only noticed sooner (CI now checks
# daily). The first can: a dependency is now vetted before it is pushed.
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
    echo "no cargo on this PATH; fmt, clippy, doc and deny were not run"
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

echo "== cargo deny --all-features check"
if ! cargo deny --version >/dev/null 2>&1; then
    # Skipped, like the toolchain above, and said loudly: this is the gate
    # that exists because nobody ran it.
    echo "   SKIPPED: cargo-deny is not installed (cargo install cargo-deny --locked)"
elif cargo deny --all-features check >/dev/null 2>&1; then
    echo "   advisories, bans, licences and sources all pass"
else
    echo "   FAIL: cargo deny refuses a dependency. For an advisory, try"
    echo "         \`cargo update -p <crate>\`; for a licence, see deny.toml."
    cargo deny --all-features check 2>&1 | grep -aE "^(error|warning)\[|ID:|Solution:" | head -12
    failed=1
fi

exit "$failed"
