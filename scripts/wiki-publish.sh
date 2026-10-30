#!/usr/bin/env bash
# Publish docs/wiki/ to the GitHub wiki of this repository.
#
# Why a script, and why by hand: a wiki is a separate git repository
# (SigSegGit/itsanas.wiki.git) that CI and cloud sessions have no credentials
# for, so this runs where Nicolas is already signed in to git. The pages are
# kept in docs/wiki/ rather than only in the wiki so that they are reviewed in
# a pull request, and so that scripts/check-wiki.py can hold them to
# docs/ROADMAP.md and docs/MVP.md.
#
#     bash scripts/wiki-publish.sh             # clone, copy, commit, push
#     bash scripts/wiki-publish.sh --dry-run   # show what would change, push nothing
#
# It refuses to publish pages that check-wiki.py says are stale: the wiki is the
# public face, and a roadmap page that disagrees with ROADMAP.md is worse than
# none.
#
# It never deletes a page it did not write, so anything added through the web
# editor survives. A page removed from docs/wiki/ must be removed from the wiki
# by hand.
#
# GitHub creates the wiki repository when its first page is saved. If the clone
# below fails with "not found", open the Wiki tab, press "Create the first
# page", save anything, and run this again; it overwrites that page.

set -euo pipefail
cd "$(dirname "$0")/.."

dry=0
[ "${1:-}" = "--dry-run" ] && dry=1

python3 scripts/check-wiki.py

src="$(git rev-parse --short HEAD)"
url="${WIKI_URL:-https://github.com/SigSegGit/itsanas.wiki.git}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

if ! git clone --quiet "$url" "$work/wiki" 2>"$work/err"; then
    cat "$work/err" >&2
    echo "wiki-publish: could not clone $url" >&2
    echo "  If the wiki has no page yet, create the first one in the Wiki tab and re-run." >&2
    exit 1
fi

cp docs/wiki/*.md "$work/wiki/"
cd "$work/wiki"
git add -A

if git diff --cached --quiet; then
    echo "wiki-publish: the wiki already matches docs/wiki/"
    exit 0
fi

git diff --cached --stat
if [ "$dry" = 1 ]; then
    echo "wiki-publish: dry run, nothing pushed"
    exit 0
fi

git commit --quiet -m "Sync from docs/wiki at $src"
git push --quiet origin HEAD
echo "wiki-publish: pushed ($src)"
