#!/usr/bin/env bash
# The acceptance tests of a test day, run by the machines rather than by hand.
#
#   bash scripts/kit-day.sh <synced folder> [--away] [--minutes N] [--name NAME]
#
# Start it on every machine of the day -- the Mac at a friend's house, the
# laptop, the VM -- each with its own synced folder of the same account, at
# about the same time. Nothing is typed from one machine to another: each one
# writes into the account itself, under `kit-day/<machine>/`, and reads what
# the others wrote there.
#
#   B  each machine writes a file of random bytes; every other machine checks
#      it arrives byte for byte (`acceptance.sh B write` / `B check`)
#   O  with --away (the machine away from home): the coordinator is reached
#      from this network, and a file written here arrives back home (`O away`)
#   F  once every other machine has seen this machine's B file, it is
#      deleted here; every other machine checks it is gone (`F delete` / `F check`)
#
# What is left to hands is said at the end: A (install and enrol without
# editing a file) is the person's own verdict, J needs a power cut, H a day of
# samples, and the phones their own scripts (android-e2e.sh, iphone-watch.sh).
# G and C are the fleet's (docs/FLEET-CI.md): two edits at the same instant,
# and a look inside another account's node, are not a person's to stage.
#
# Each verdict is the kit's own line (scripts/acceptance.sh), appended to
# ~/.itsanas-receipts/acceptance.txt, so "B passes" means here what it means at
# night and by hand. Exit 0 when every check passed, 1 when one failed or never
# finished in the time given.
#
# Environment: ITSANAS_BIN and ITSANAS_HOME as for the kit; KIT_DAY_SYNC, a
# command run before each look (empty: the daemon syncs; the local test runs
# `itsanas scan`); KIT_DAY_POLL, seconds between looks (20).
#
# bash 3.2 (macOS): no associative arrays, no mapfile.
set -uo pipefail

here=$(cd "$(dirname "$0")" && pwd)
KIT=${ACCEPTANCE_KIT:-$here/acceptance.sh}
POLL=${KIT_DAY_POLL:-20}
SYNC=${KIT_DAY_SYNC:-}

usage() {
    sed -n '3,5p' "$0" >&2
    exit 2
}

folder=
away=0
minutes=20
name=$(uname -n 2>/dev/null || hostname)
while [ $# -gt 0 ]; do
    case "$1" in
        --away) away=1 ;;
        --minutes) shift; minutes=${1:-}; case "$minutes" in ''|*[!0-9]*) usage ;; esac ;;
        --name) shift; name=${1:-} ;;
        -*) usage ;;
        *) [ -z "$folder" ] || usage; folder=$1 ;;
    esac
    shift
done
[ -n "$folder" ] && [ -d "$folder" ] || usage
# A machine name is a directory of the account: letters, digits, dot, dash.
me=$(printf '%s' "$name" | tr -c 'A-Za-z0-9.-' '-' | cut -c1-40)
[ -n "$me" ] || usage

day="$folder/kit-day"
mine="$day/$me"
manifest="$mine/manifest.txt"
mkdir -p "$mine"
touch "$manifest"

kit() { bash "$KIT" "$@"; }

# The digest, to wait on a file without calling the kit: a check made while a
# file is still on its way would leave a FAIL in the receipt at every look.
if command -v sha256sum >/dev/null 2>&1; then
    digest() { sha256sum "$1" | cut -d' ' -f1; }
else
    digest() { shasum -a 256 "$1" | cut -d' ' -f1; }
fi

# The kit prints "PASS  B write   <name> sha256 <hex> code ...": the file and
# its digest, from the verdict line itself.
written() { sed -n 's/^PASS .* \(acceptance-[A-Z]-[^ ]*\.bin\) sha256 \([0-9a-f]\{64\}\).*/\1 \2/p'; }

note() { printf '%s\n' "$*" >>"$manifest"; }
noted() { grep -qxF -- "$*" "$manifest" 2>/dev/null; }

echo "== kit day on $me, folder $folder, for $minutes minutes"

# What this machine writes, once per run: a second run the same day adds a new
# file rather than checking an old one again.
# A write that failed fails the day: the checks that follow would otherwise
# pass on what the other machines wrote and say nothing of this one.
broken=0
if ! grep -q '^B ' "$manifest"; then
    line=$(kit B write "$mine" | tee /dev/stderr | written)
    if [ -n "$line" ]; then note "B $line"; else broken=1; fi
fi
if [ "$away" = 1 ] && ! grep -q '^O ' "$manifest"; then
    line=$(kit O away "$mine" | tee /dev/stderr | written)
    if [ -n "$line" ]; then note "O $line"; else broken=1; fi
fi

# The other machines: every directory of kit-day/ but this one with a manifest.
peers() {
    local dir
    for dir in "$day"/*/; do
        dir=${dir%/}
        [ "$dir" = "$mine" ] && continue
        [ -f "$dir/manifest.txt" ] && basename "$dir"
    done
}

# One look: check what the others wrote, delete this machine's B file once
# they all saw it, check what they deleted. Prints how many checks are still
# open; the loop ends at zero.
look() {
    local open=0 peer kind file sha everyone mine_b
    for peer in $(peers); do
        while read -r kind file sha; do
            case "$kind" in
                B|O)
                    noted "seen $peer $file" && continue
                    if [ -f "$day/$peer/$file" ] && [ "$(digest "$day/$peer/$file")" = "$sha" ]; then
                        kit "$kind" check "$day/$peer" "$file" "$sha" >&2
                        note "seen $peer $file"
                    else
                        open=$((open + 1))
                    fi
                    ;;
                deleted)
                    noted "gone $peer $file" && continue
                    if [ ! -e "$day/$peer/$file" ]; then
                        kit F check "$day/$peer" "$file" >&2
                        note "gone $peer $file"
                    else
                        open=$((open + 1))
                    fi
                    ;;
            esac
        done <"$day/$peer/manifest.txt"
    done

    # This machine's B file goes once every other machine has noted it.
    mine_b=$(sed -n 's/^B \([^ ]*\) .*/\1/p' "$manifest" | head -n 1)
    if [ -n "$mine_b" ] && ! grep -q "^deleted $mine_b\$" "$manifest"; then
        everyone=1
        for peer in $(peers); do
            grep -qxF "seen $me $mine_b" "$day/$peer/manifest.txt" || everyone=0
        done
        if [ "$everyone" = 1 ] && [ -n "$(peers)" ]; then
            kit F delete "$mine" "$mine_b" >&2 && note "deleted $mine_b"
        else
            open=$((open + 1))
        fi
    fi
    # Every peer must also see this machine's delete.
    if grep -q "^deleted " "$manifest"; then
        for peer in $(peers); do
            grep -q "^gone $me " "$day/$peer/manifest.txt" || open=$((open + 1))
        done
    fi
    printf '%s\n' "$open"
}

deadline=$(( $(date +%s) + minutes * 60 ))
open=1
while :; do
    [ -n "$SYNC" ] && sh -c "$SYNC" >/dev/null 2>&1
    open=$(look | tail -n 1)
    [ "$open" = 0 ] && [ -n "$(peers)" ] && break
    [ "$(date +%s)" -ge "$deadline" ] && break
    sleep "$POLL"
done

echo "== kit day on $me: $(peers | wc -l | tr -d ' ') other machine(s): $(peers | tr '\n' ' ')"
if [ -z "$(peers)" ]; then
    kit_line="no other machine wrote into kit-day/ in $minutes minutes: start this script on another machine of the account"
    printf 'FAIL  kit-day  %s\n' "$kit_line"
    exit 1
fi
if [ "$broken" = 1 ]; then
    printf 'FAIL  kit-day  a write on this machine failed (its line is above): the day is not passed\n'
    exit 1
fi
if [ "$open" != 0 ]; then
    printf 'FAIL  kit-day  %s check(s) still open after %s minutes: a file that never arrived, or a delete that never crossed\n' "$open" "$minutes"
    exit 1
fi
cat <<EOF
PASS  kit-day  every file of every machine arrived, and every delete crossed
Left to hands (docs/MVP.md): A, your own verdict on the install; J, a power cut;
H, a day of samples (acceptance.sh H sample); the phones (android-e2e.sh,
iphone-watch.sh). Receipts: ${ITSANAS_RECEIPTS:-$HOME/.itsanas-receipts}/acceptance.txt
EOF
