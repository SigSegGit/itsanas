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

# The checks, one file per concern, run in this order in this shell: they
# share `say`, `bad`, `failed` and the variables one sets for the next.
for part in parse undo script-text refusals throwaway-homes trays release; do
    # shellcheck source=/dev/null
    . "scripts/check-installers/$part.sh"
done

if [ "$failed" -ne 0 ]; then
    echo
    echo "An installer is the one program here that runs on a machine nobody has"
    echo "configured, in front of somebody who has read nothing. It is also the"
    echo "least exercised code in the project."
    exit 1
fi

echo "installers: parse, no bashisms, all listed, MSRV agrees, 32-bit refused, coordinator re-runs, sudo rule matches, a cleaned instance spares its sibling, and the tray icons on every platform, and get.sh and get.ps1 refuse a bad download"
