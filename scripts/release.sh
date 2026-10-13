#!/usr/bin/env bash
# One command from main to a signed release, for Git Bash on Nicolas's PC:
#
#   alias itsanas-release='bash /d/GitHub/itsanas/scripts/release.sh'
#
# 1. goes to the clone (ITSANAS_DIR, default /d/GitHub/itsanas) and refuses
#    uncommitted changes rather than guess what to do with them;
# 2. brings main up to origin/main (fast-forward only);
# 3. reads the version from Cargo.toml's [workspace.package] -- the tag *is*
#    that version, so the release workflow's check cannot refuse it; bumping
#    the version stays a change in a PR, reviewed like any other;
# 4. cleans a tag of that version left on another commit (a tag pushed before
#    its PR merged) and its draft release, after asking; refuses to touch one
#    already published;
# 5. pushes the tag and waits for the Release workflow to finish;
# 6. signs and publishes with sign-release.ps1, which asks the passphrase.
set -euo pipefail

repo="${ITSANAS_REPO:-SigSegGit/itsanas}"
cd "${ITSANAS_DIR:-/d/GitHub/itsanas}"

say() { printf '\n== %s\n' "$*"; }
die() { printf 'release: %s\n' "$*" >&2; exit 1; }

command -v gh >/dev/null || die "gh is not installed (winget install GitHub.cli, then gh auth login)"
if [ -n "$(git status --porcelain)" ]; then
    git status --short
    die "uncommitted changes in $(pwd): commit or stash them first"
fi

say "Updating main"
git fetch --prune --prune-tags --tags --force origin
git checkout -q main
git merge -q --ff-only origin/main || die "local main has commits origin/main does not: push or drop them"

version=$(sed -n '/^\[workspace\.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "no version found in Cargo.toml ([workspace.package])"
tag="v$version"
head=$(git rev-parse HEAD)
echo "main is $(git log -1 --format='%h %s'); version $version, tag $tag"

if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    tagged=$(git rev-list -n1 "$tag")
    draft=$(gh release view "$tag" -R "$repo" --json isDraft -q .isDraft 2>/dev/null || echo none)
    [ "$draft" = false ] && die "$tag is already published: bump the version in Cargo.toml (in a PR) first"
    if [ "$tagged" != "$head" ]; then
        say "Cleaning: $tag points to $(git log -1 --format='%h %s' "$tagged"), not main"
        read -r -p "Delete that tag (and its draft release, if any) and tag main instead? [y/N] " yes
        [[ "$yes" =~ ^[yYoO] ]] || die "stopped; nothing changed"
        [ "$draft" = true ] && gh release delete "$tag" -R "$repo" --yes
        git push -q origin ":refs/tags/$tag" || true
        git tag -d "$tag" >/dev/null
    fi
fi

if ! git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    say "Tagging $tag"
    git tag "$tag"
    git push -q origin "$tag"
    pushed_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
else
    pushed_at=""
fi

say "Waiting for the Release workflow on $tag"
run=""
for _ in $(seq 60); do
    run=$(gh run list -R "$repo" --workflow release.yml --branch "$tag" --limit 1 \
        --json databaseId,createdAt -q ".[] | select(.createdAt >= \"$pushed_at\") | .databaseId")
    [ -n "$run" ] && break
    sleep 5
done
[ -n "$run" ] || die "no Release run started for $tag after 5 minutes: see https://github.com/$repo/actions"
gh run watch "$run" -R "$repo" --exit-status --interval 20 \
    || die "the Release workflow failed: gh run view $run -R $repo --log-failed"

say "Signing and publishing $tag"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$(cygpath -w scripts/sign-release.ps1 2>/dev/null || echo scripts/sign-release.ps1)" -Tag "$tag"
