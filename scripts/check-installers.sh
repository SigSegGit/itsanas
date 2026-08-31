#!/usr/bin/env bash
# Parse every installer, and check the claims they make about each other.
#
# Why this exists
# ---------------
#
# An installer is the one program in a project that runs on a machine nobody has
# configured, in a shell nobody chose, in front of somebody who has not read
# anything. A syntax error in it is not a build failure — it is a stranger
# pasting a command and getting `unexpected end of file`.
#
# And it is the code least likely to be exercised: the Rust is compiled on every
# push and covered by six hundred tests; `install/linux.sh` runs when somebody
# installs, which on this project so far means twice.
#
# What is checked
# ---------------
#
# **Syntax**, with the shell each script declares. The `.sh` files are POSIX sh
# on purpose — some minimal images ship dash as /bin/sh and no bash — so they
# are checked with `sh -n`, plus a grep for the bashisms `sh -n` accepts and
# dash then rejects at runtime, which is the worst of both.
#
# **shellcheck**, when it is available. Not required: making a check depend on a
# tool that is absent turns a useful warning into an obstacle.
#
# **The claims.** install/README.md has a table saying which installer has been
# run on the system it installs. That table is the honest part of the whole
# directory, and a table nobody checks becomes wrong the way every other
# unchecked table in this project has.
#
# **The Rust version**, in every script that states one, against Cargo.toml. A
# machine that passes an installer's check and then fails to compile is the
# worst possible order for those two to disagree.
#
# Scripts are **discovered, not listed**. The first version named two files, so
# `install/coordinator.sh` was written, added, and checked by nothing — which is
# exactly the failure this file exists to prevent, made while writing it.

set -uo pipefail

cd "$(dirname "$0")/.."
failed=0

say() { printf '  %s\n' "$*"; }
bad() { printf '  FAIL %s\n' "$*"; failed=1; }

shopt -s nullglob
SH_SCRIPTS=(install/*.sh)
PS_SCRIPTS=(install/*.ps1)
shopt -u nullglob

if [ ${#SH_SCRIPTS[@]} -eq 0 ] && [ ${#PS_SCRIPTS[@]} -eq 0 ]; then
    bad "install/ has no scripts at all"
fi

# ------------------------------------------------------------------- syntax

for script in "${SH_SCRIPTS[@]}"; do
    if sh -n "$script" 2>/tmp/itsanas-shcheck; then
        say "$script parses as POSIX sh"
    else
        bad "$script does not parse:"
        sed 's/^/       /' /tmp/itsanas-shcheck
    fi
done
rm -f /tmp/itsanas-shcheck

# Comment lines are stripped first. The first version did not, and flagged the
# sentence in linux.sh explaining why bashisms are avoided -- the same mistake
# as a dead-code check that counts a doc link as a call site, made twice in one
# week by the same person.
BASHISM='\[\[|^[[:space:]]*local |^[[:space:]]*function [A-Za-z_]'
for script in "${SH_SCRIPTS[@]}"; do
    hits=$(grep -vE '^[[:space:]]*#' "$script" | grep -nE "$BASHISM")
    if [ -n "$hits" ]; then
        bad "$script uses bash-only syntax:"
        printf '%s\n' "$hits" | sed 's/^/       /'
    fi
done
say "no bash-only syntax in the POSIX scripts"

if command -v shellcheck >/dev/null 2>&1; then
    for script in "${SH_SCRIPTS[@]}"; do
        if shellcheck -s sh -S warning "$script"; then
            say "$script passes shellcheck"
        else
            bad "$script has shellcheck warnings"
        fi
    done
else
    say "shellcheck is not installed here; skipped"
fi

for script in "${PS_SCRIPTS[@]}"; do
    if command -v pwsh >/dev/null 2>&1; then
        if pwsh -NoProfile -Command "
            \$errors = \$null
            \$null = [System.Management.Automation.Language.Parser]::ParseFile(
                (Resolve-Path '$script'), [ref]\$null, [ref]\$errors)
            if (\$errors) { \$errors | ForEach-Object { \$_.Message }; exit 1 }
        "; then
            say "$script parses"
        else
            bad "$script does not parse"
        fi
    else
        say "pwsh is not installed here; $script was not parsed"
    fi
done

# ------------------------------------------------------------- systemd units

# An invalid unit fails at `systemctl start` with a message naming a line in a
# file the user has never seen. systemd will say so now instead, when it is here
# to ask: it is on the CI runner and on any Linux this installs to, and absent
# on a Mac or on Windows, where skipping is correct rather than lax.
if command -v systemd-analyze >/dev/null 2>&1; then
    units=$(mktemp -d)

    sed -n '/^\[Unit\]/,/^WantedBy=default.target$/p' install/linux.sh \
        | sed 's|\$BIN_DIR|/usr/local/bin|g' \
        > "$units/itsanas.service"

    sed -n '/^\[Unit\]/,/^WantedBy=multi-user.target$/p' install/coordinator.sh \
        | sed 's|\$SERVICE_USER|itsanas-coord|g' \
        | sed 's|\$STATE_DIR|/var/lib/itsanas-coordinator|g' \
        | sed 's|\$PORT|9898|g' \
        | sed 's|\$ADMIT||g' \
        > "$units/itsanas-coordinator.service"

    # A ReadWritePaths entry without a leading dash refuses to start the unit
    # when the path does not exist yet. That is not hypothetical: the member
    # unit listed ~/.itsanas, which `itsanas init` creates, so enabling the
    # service before initialising -- exactly what somebody does when an
    # installer says "enable this" -- failed with an error about mount
    # namespaces. systemd-analyze does not catch it, because the unit is valid.
    for unit in "$units"/*.service; do
        # Split the entry into its paths, keep the ones that are not prefixed
        # with a dash. A trailing pipe continues the line on its own, which is
        # the point: this pipeline was written with backslashes and reached the
        # repository with them eaten and the indentation left in place.
        risky=$(grep -oE '^ReadWritePaths=.*' "$unit" |
            tr ' ' '\n' |
            grep -E '^(/|%h|ReadWritePaths=[^-])' |
            grep -vE '^ReadWritePaths=-|^-')
        if [ -n "$risky" ]; then
            bad "$(basename "$unit") has a ReadWritePaths without a leading dash:"
            printf '%s\n' "$risky" | sed 's/^/       /'
            say "  Without it the unit refuses to start if the path is not there yet."
        fi
    done

    for unit in "$units"/*.service; do
        # The binary is not installed on the machine doing the checking, so that
        # one complaint is expected and is not what is being asked about.
        problems=$(systemd-analyze verify "$unit" 2>&1 | grep -v 'is not executable')
        if [ -z "$problems" ]; then
            say "$(basename "$unit") is a valid unit"
        else
            bad "$(basename "$unit") is not:"
            printf '%s\n' "$problems" | sed 's/^/       /'
        fi
    done
    rm -rf "$units"
else
    say "systemd-analyze is not here; the units were not verified"
fi


# -------------------------------------------------------------- the claims

# Every script must be named in the README. One that nothing links to is one
# nobody runs, and `install/coordinator.sh` reached the repository that way.
for script in "${SH_SCRIPTS[@]}" "${PS_SCRIPTS[@]}" install/*.md; do
    name=$(basename "$script")
    [ "$name" = "README.md" ] && continue
    if grep -q "$name" install/README.md; then
        say "README.md mentions $name"
    else
        bad "install/$name exists and install/README.md does not mention it"
    fi
done

# ---------------------------------------------------------- the Rust version

msrv=$(grep -m1 '^rust-version' Cargo.toml | cut -d'"' -f2)
if [ -z "$msrv" ]; then
    bad "could not read rust-version from Cargo.toml"
else
    major=${msrv%%.*}
    minor=${msrv#*.}
    minor=${minor%%.*}
    stated=0

    for script in "${SH_SCRIPTS[@]}"; do
        got_major=$(grep -m1 '^MIN_RUST_MAJOR=' "$script" | cut -d= -f2)
        # A script that does not build anything states no version, and that is
        # correct rather than missing: install/coordinator.sh installs a binary
        # somebody else compiled.
        [ -z "$got_major" ] && continue
        stated=$((stated + 1))
        got_minor=$(grep -m1 '^MIN_RUST_MINOR=' "$script" | cut -d= -f2)
        if [ "$got_major" = "$major" ] && [ "$got_minor" = "$minor" ]; then
            say "$script wants Rust $got_major.$got_minor, same as Cargo.toml"
        else
            bad "$script wants Rust $got_major.$got_minor but Cargo.toml says $msrv"
        fi
    done

    for script in "${PS_SCRIPTS[@]}"; do
        got_major=$(grep -m1 '^\$MinRustMajor' "$script" | tr -dc '0-9')
        [ -z "$got_major" ] && continue
        stated=$((stated + 1))
        got_minor=$(grep -m1 '^\$MinRustMinor' "$script" | tr -dc '0-9')
        if [ "$got_major" = "$major" ] && [ "$got_minor" = "$minor" ]; then
            say "$script wants Rust $got_major.$got_minor, same as Cargo.toml"
        else
            bad "$script wants Rust $got_major.$got_minor but Cargo.toml says $msrv"
        fi
    done

    if [ "$stated" -eq 0 ]; then
        bad "no installer states a minimum Rust version"
        say "  If they all stopped building, say so here; otherwise one has lost its check."
    fi
fi

# ------------------------------------------------------------ reading a reply
#
# `install/linux.sh` advertises `curl -fsSL … | sh` on its fourth line. Under
# that pipe, **stdin is the script**: a bare `read` consumes the lines the shell
# has not executed yet, and the shell then runs a truncated program. Every
# prompt in these scripts did exactly that, so the entry point the file offers a
# stranger would have destroyed itself at the first question.
#
# `< /dev/tty` reads the terminal whatever stdin happens to be.

for script in "${SH_SCRIPTS[@]}"; do
    bare=$(grep -n '^[^#]*\bread\b' "$script" |
        grep -v '/dev/tty' |
        grep -vE 'could not read|does not let|read this file|read the' || true)
    if [ -n "$bare" ]; then
        bad "$script reads a reply from standard input:"
        printf '%s\n' "$bare" | sed 's/^/         /'
        say "  Piped to sh, stdin is the script. Use: read -r x < /dev/tty"
    fi
done
say "no installer reads a reply from standard input"

# ------------------------------------------------------- what it refuses to do
#
# The one branch of an installer that has to be right on a machine nobody here
# owns is the one that refuses. `install/linux.sh` gets its answer from
# `uname -m`, so a fake `uname` earlier on PATH exercises every case.
#
# This found a real hole. The list was `armv7l|armv6l`, and a 64-bit Raspberry
# Pi running a 32-bit userland reports **armv8l** — the exact case the error
# message describes, waved through with "untested architecture" and left to fail
# an hour into the build. 32-bit x86 was let through too, with the same 64-bit
# counters the message cites as the reason.

fake=$(mktemp -d)
cat > "$fake/uname" <<'FAKEUNAME'
#!/bin/sh
for arg in "$@"; do
    if [ "$arg" = "-m" ]; then
        echo "$FAKE_ARCH"
        exit 0
    fi
done
exec /usr/bin/uname "$@"
FAKEUNAME
chmod +x "$fake/uname"

refused=0
for arch in armv6l armv7l armv8l armhf arm i386 i686; do
    output=$(FAKE_ARCH="$arch" PATH="$fake:$PATH" sh install/linux.sh --no-build 2>&1)
    if printf '%s' "$output" | grep -q "is not supported"; then
        refused=$((refused + 1))
    else
        bad "install/linux.sh does not refuse a 32-bit userland reporting $arch"
        say "  A 64-bit machine running a 32-bit image is the common case, not"
        say "  an exotic one, and the build fails an hour later without this."
    fi
done
[ "$refused" -eq 7 ] && say "install/linux.sh refuses all 7 spellings of a 32-bit userland"

for arch in aarch64 arm64 x86_64; do
    # It may still stop later for want of a compiler on this machine, so look
    # for the line rather than the exit status.
    output=$(FAKE_ARCH="$arch" PATH="$fake:$PATH" sh install/linux.sh --no-build 2>&1)
    if printf '%s' "$output" | grep -q "is not supported"; then
        bad "install/linux.sh refuses $arch, which is one of its targets"
    fi
done
say "install/linux.sh accepts aarch64, arm64 and x86_64"
rm -rf "$fake"

if [ "$failed" -ne 0 ]; then
    echo
    echo "An installer is the one program here that runs on a machine nobody has"
    echo "configured, in front of somebody who has read nothing. It is also the"
    echo "least exercised code in the project."
    exit 1
fi

echo "installers: parse, no bashisms, all listed, MSRV agrees, 32-bit refused"
