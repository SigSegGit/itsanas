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
# **Syntax**, with the shell each script declares. `linux.sh` and `macos.sh` are
# POSIX sh on purpose — some minimal images ship dash as /bin/sh and no bash —
# so they are checked with `sh -n`, which fails on a bashism the author did not
# notice they used.
#
# **shellcheck**, when it is available. It is not required, because requiring a
# tool that is absent on the machine of whoever is checking turns a useful
# warning into an obstacle.
#
# **The claims.** README.md has a table saying which installer has been run on
# the system it installs. That table is the honest bit of the whole directory,
# and a table nobody checks becomes wrong the way every other unchecked table in
# this project has.

set -uo pipefail

cd "$(dirname "$0")/.."
failed=0

say()  { printf '  %s\n' "$*"; }
bad()  { printf '  FAIL %s\n' "$*"; failed=1; }

# ------------------------------------------------------------------- syntax

for script in install/linux.sh install/macos.sh; do
    if [ ! -f "$script" ]; then
        bad "$script is missing"
        continue
    fi
    if sh -n "$script" 2>/tmp/itsanas-shcheck; then
        say "$script parses as POSIX sh"
    else
        bad "$script does not parse:"
        sed 's/^/       /' /tmp/itsanas-shcheck
    fi
done
rm -f /tmp/itsanas-shcheck

# A bashism check that `sh -n` cannot do: dash accepts `[[` at parse time in
# some builds and fails at runtime, which is the worst of both.
#
# Comment lines are stripped first. The first version did not, and flagged the
# sentence in linux.sh explaining why bashisms are avoided -- the same mistake
# as a dead-code check that counts a doc link as a call site, made twice in one
# week by the same person.
BASHISM='\[\[|^[[:space:]]*local |^[[:space:]]*function [A-Za-z_]'
for script in install/linux.sh install/macos.sh; do
    [ -f "$script" ] || continue
    hits=$(grep -vE '^[[:space:]]*#' "$script" | grep -nE "$BASHISM")
    if [ -n "$hits" ]; then
        bad "$script uses bash-only syntax:"
        printf '%s\n' "$hits" | sed 's/^/       /'
    fi
done
say "no bash-only syntax in the POSIX scripts"

if command -v shellcheck >/dev/null 2>&1; then
    for script in install/linux.sh install/macos.sh; do
        [ -f "$script" ] || continue
        if shellcheck -s sh -S warning "$script"; then
            say "$script passes shellcheck"
        else
            bad "$script has shellcheck warnings"
        fi
    done
else
    say "shellcheck is not installed here; skipped"
fi

if command -v pwsh >/dev/null 2>&1; then
    if pwsh -NoProfile -Command '
        $errors = $null
        $null = [System.Management.Automation.Language.Parser]::ParseFile(
            (Resolve-Path "install/windows.ps1"), [ref]$null, [ref]$errors)
        if ($errors) { $errors | ForEach-Object { $_.Message }; exit 1 }
    '; then
        say "install/windows.ps1 parses"
    else
        bad "install/windows.ps1 does not parse"
    fi
else
    say "pwsh is not installed here; the Windows installer was not parsed"
fi

# -------------------------------------------------------------- the claims

# Every installer named in the README must exist, and every installer must be
# named. A script nobody links to is a script nobody runs.
for script in install/*.sh install/*.ps1 install/*.md; do
    [ -f "$script" ] || continue
    name=$(basename "$script")
    [ "$name" = "README.md" ] && continue
    if grep -q "$name" install/README.md; then
        say "README.md mentions $name"
    else
        bad "install/$name exists and install/README.md does not mention it"
    fi
done

# The MSRV. Three places state it and they drift: the workspace manifest, and a
# constant in each installer. A machine that passes the installer's check and
# then fails to compile is the worst possible order for those two to disagree.
msrv=$(grep -m1 '^rust-version' Cargo.toml | cut -d'"' -f2)
if [ -z "$msrv" ]; then
    bad "could not read rust-version from Cargo.toml"
else
    major=${msrv%%.*}
    minor=${msrv#*.}
    minor=${minor%%.*}
    for script in install/linux.sh install/macos.sh; do
        [ -f "$script" ] || continue
        got_major=$(grep -m1 '^MIN_RUST_MAJOR=' "$script" | cut -d= -f2)
        got_minor=$(grep -m1 '^MIN_RUST_MINOR=' "$script" | cut -d= -f2)
        if [ "$got_major" = "$major" ] && [ "$got_minor" = "$minor" ]; then
            say "$script wants Rust $got_major.$got_minor, same as Cargo.toml"
        else
            bad "$script wants Rust $got_major.$got_minor but Cargo.toml says $msrv"
        fi
    done
    got_major=$(grep -m1 '^\$MinRustMajor' install/windows.ps1 | tr -dc '0-9')
    got_minor=$(grep -m1 '^\$MinRustMinor' install/windows.ps1 | tr -dc '0-9')
    if [ "$got_major" = "$major" ] && [ "$got_minor" = "$minor" ]; then
        say "install/windows.ps1 wants Rust $got_major.$got_minor, same as Cargo.toml"
    else
        bad "install/windows.ps1 wants Rust $got_major.$got_minor but Cargo.toml says $msrv"
    fi
fi

if [ "$failed" -ne 0 ]; then
    echo
    echo "An installer is the one program here that runs on a machine nobody has"
    echo "configured, in front of somebody who has read nothing. It is also the"
    echo "least exercised code in the project."
    exit 1
fi

echo "installers: parse, no bashisms, all listed, MSRV agrees with Cargo.toml"
