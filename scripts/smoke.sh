#!/bin/sh
# Store a file on this machine and read it back.
#
# Why this exists
# ---------------
#
# Every installer here used to finish by running `itsanas --version` and calling
# that a verification. It is not one: it proves the file is executable by this
# kernel and nothing about whether the data path works. On the machines this
# project is actually for -- a Raspberry Pi, a phone, a VM on a Freebox -- that
# gap is the whole question.
#
# The Pi justifies half the constants in this repository: the chunk size, the
# memory the key derivation is allowed, the audit budget per round. Until this
# script ran, no line of ITSaNAS had ever executed on an ARM processor. The
# workspace was cross-compiled for aarch64 on every push, which proves the types
# line up and nothing else. A build is not a run.
#
# There is a real difference to catch. aarch64 is where `blake3` switches to its
# NEON backend and where `char` is unsigned by default in the C compiled
# alongside it. Either can produce a binary that links and then hashes wrong.
#
# Not, as an earlier version of this comment claimed, "where alignment
# requirements are stricter". Linux on aarch64 permits unaligned access to
# normal memory, and this workspace sets `unsafe_code = "forbid"`, so safe Rust
# could not produce a misaligned access even where it mattered. Inventing a risk
# to justify a measure is how a project ends up with measures nobody can
# evaluate.
#
# What it does not prove
# -----------------------
#
# Under emulation this is one instruction set standing in for another, on this
# machine's kernel, with a fast disk and no thermal limit. Three things it
# therefore cannot say:
#
#   - whether a Pi 4B with 1 GB of RAM can hold a terabyte's worth of index,
#     which is the actual open question;
#   - anything about aarch64's weaker memory ordering, which the emulator does
#     not reproduce when the host is x86;
#   - which code path a library that detects CPU features at runtime will take
#     on real silicon, since it is asking an emulated CPU.
#
# It is the floor, not the ceiling: the same script runs on a real Pi with no
# runner set, and until it has, the claim is only that the instructions
# execute correctly.

set -eu

BIN=${1:?usage: smoke.sh <path-to-itsanas>}

# The command that runs the binary. Empty on a real Pi or phone; on CI it is
# `qemu-aarch64-static`, which finds the aarch64 loader through QEMU_LD_PREFIX
# pointing at the sysroot the cross compiler already installed.
run() {
    if [ -n "${ITSANAS_RUNNER:-}" ]; then
        # ITSANAS_RUNNER is a command prefix and has to word-split to be one.
        # shellcheck disable=SC2086
        $ITSANAS_RUNNER "$BIN" "$@"
    else
        "$BIN" "$@"
    fi
}

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM

echo "== which machine"
run --version
if [ -n "${ITSANAS_RUNNER:-}" ]; then
    WHERE="emulated aarch64 on $(uname -m), via $ITSANAS_RUNNER"
else
    WHERE="native $(uname -m)"
fi
echo "   $WHERE"

echo "== an account"
ITSANAS_PASSPHRASE='itsanas-smoke-passphrase-9931'
export ITSANAS_PASSPHRASE
run --home "$WORK/home" init --username armsmoke >"$WORK/init.txt" 2>&1 || {
    cat "$WORK/init.txt"
    echo "FAIL: init did not complete"
    exit 1
}
grep -E 'user id' "$WORK/init.txt" | sed 's/^/   /'

# A recovery phrase that is not 24 words means the key schedule produced
# something different here, which would make an account created on a Pi
# unrecoverable anywhere else.
words=$(grep -oE '[0-9]{1,2}\. +[a-z]+' "$WORK/init.txt" | wc -l)
echo "   recovery phrase: $words words"
[ "$words" -eq 24 ] || { echo "FAIL: expected 24 words"; exit 1; }

echo "== a file, in and out"
# Larger than one chunk, so this exercises the chunker and the manifest rather
# than a single sealed blob.
head -c 350000 /dev/urandom >"$WORK/payload.bin"
run --home "$WORK/home" put "docs/smoke.bin" "$WORK/payload.bin" | sed 's/^/   /'
run --home "$WORK/home" ls | sed 's/^/   /'
run --home "$WORK/home" get "docs/smoke.bin" "$WORK/back.bin" | sed 's/^/   /'

before=$(sha256sum <"$WORK/payload.bin" | cut -d' ' -f1)
after=$(sha256sum <"$WORK/back.bin" | cut -d' ' -f1)
echo "   wrote $before"
echo "   read  $after"
[ "$before" = "$after" ] || { echo "FAIL: the bytes changed"; exit 1; }

echo "== the store agrees with itself"
run --home "$WORK/home" doctor | sed 's/^/   /'

# Say where this actually ran. An earlier version ended with "on aarch64"
# whatever it had run on, which would have reported a pass on ARM from a run
# that never left x86.
echo "PASS: ITSaNAS stored and returned a file -- $WHERE"
