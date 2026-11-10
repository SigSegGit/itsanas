#!/usr/bin/env bash
# Say on GitHub that a real phone passed on this commit, so CI accepts it for
# the phone job no runner finished (scripts/ci_scope.py, RECEIPTS).
#
#   bash scripts/device-receipt.sh android "<what ran>"   # after android-e2e.sh on a phone
#   bash scripts/device-receipt.sh iphone "<what ran>"    # after iphone-watch.sh PASS
#   bash scripts/device-receipt.sh post                   # the ones kept until a push
#
# The scripts that run on a phone call it themselves when they pass; it is a
# command of its own so a run made before the push still counts after it.
#
# A receipt is for the exact commit that ran, and only for a committed tree: a
# run on uncommitted changes proves nothing about any commit, and is refused.
# It is a commit status (`real-device/android`, `real-device/iphone`), posted
# with the owner's own `gh` login. The CI takes it for a phone job that was
# cancelled or that no runner took, never for one that failed, and skips the
# Mac job when an iPhone receipt is there before the run plans.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
pending="$HOME/.itsanas-receipts/devices-pending.txt"
mkdir -p "$(dirname "$pending")"

pushed() { git -C "$here" branch -r --contains "$1" 2>/dev/null | grep -q .; }

post() {
    local sha=$1 kind=$2 description=$3 repo
    repo="$(cd "$here" && gh repo view --json nameWithOwner -q .nameWithOwner)"
    gh api "repos/$repo/statuses/$sha" -f state=success -f "context=real-device/$kind" \
        -f "description=${description:0:140}" >/dev/null
    echo "receipt posted: real-device/$kind on ${sha:0:12} ($description)"
}

if [ "${1:-}" = post ]; then
    git -C "$here" fetch -q origin || true
    [ -s "$pending" ] || { echo "no receipt waits for a push"; exit 0; }
    left="$(mktemp)"
    while IFS="$(printf '\t')" read -r sha kind description; do
        if pushed "$sha"; then post "$sha" "$kind" "$description"; else printf '%s\t%s\t%s\n' "$sha" "$kind" "$description" >>"$left"; fi
    done <"$pending"
    mv "$left" "$pending"
    [ -s "$pending" ] && echo "still waiting for a push: $(wc -l <"$pending" | tr -d ' ') receipts"
    exit 0
fi

kind="${1:-}"
description="${2:-}"
case "$kind" in
    android | iphone) ;;
    *) echo "usage: device-receipt.sh android|iphone \"<what ran>\" | post" >&2; exit 2 ;;
esac
[ -n "$description" ] || { echo "say what ran, in a few words" >&2; exit 2; }

if ! git -C "$here" diff --quiet HEAD --; then
    echo "no receipt: the tree has uncommitted changes, so this run tested no commit. Commit, run again." >&2
    exit 1
fi
sha="$(git -C "$here" rev-parse HEAD)"
git -C "$here" fetch -q origin || true
if pushed "$sha"; then
    post "$sha" "$kind" "$description"
else
    printf '%s\t%s\t%s\n' "$sha" "$kind" "$description" >>"$pending"
    echo "receipt kept for ${sha:0:12}: push it, then run  bash scripts/device-receipt.sh post"
fi
