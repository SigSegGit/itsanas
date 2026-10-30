#!/usr/bin/env bash
# Create, or bring up to date, the public GitHub Project board for this repo.
#
# The board is a view of docs/ROADMAP.md and docs/MVP.md, not a second place to
# keep status: `python3 scripts/check-wiki.py --items` reads the same two tables
# the wiki is drawn from and this script turns each row into a card. Change a
# status in ROADMAP.md or MVP.md, merge, run this, and the board follows. A card
# moved by hand on the board is put back by the next run, on purpose: if the
# board and the documents disagree the documents are right.
#
#     bash scripts/project-sync.sh             # create the board if missing, sync cards
#     bash scripts/project-sync.sh --dry-run   # print what it would do, change nothing
#
# Needs `gh` signed in with the `project` scope, which `gh auth login` does not
# grant by default:
#
#     gh auth refresh -s project
#
# This uses GitHub's GraphQL API underneath, which is why it is a script for a
# signed-in person and not something a CI job or a cloud session can run.
#
# What it does not do: `gh` cannot create *views*. The board opens as a plain
# table. Add a Board view (group by Status) and a Roadmap view in the browser,
# once; they persist and this script does not touch them.
#
# Idempotent: cards are matched by exact title. A card whose title is not in the
# documents any more (a renamed milestone) is reported, never deleted.

set -euo pipefail
cd "$(dirname "$0")/.."

OWNER="${OWNER:-SigSegGit}"
REPO="${REPO:-SigSegGit/itsanas}"
TITLE="${TITLE:-ITSaNAS roadmap}"
dry=0
[ "${1:-}" = "--dry-run" ] && dry=1

run() {
    if [ "$dry" = 1 ]; then
        printf 'would run: gh'
        printf ' %q' "$@"
        printf '\n'
    else
        gh "$@"
    fi
}

items="$(python3 scripts/check-wiki.py --items)"

number="$(gh project list --owner "$OWNER" --limit 100 --format json \
    | jq -r --arg t "$TITLE" '.projects[] | select(.title == $t) | .number' | head -n1)"

if [ -z "$number" ]; then
    echo "project-sync: no project called '$TITLE' under $OWNER, creating it"
    if [ "$dry" = 1 ]; then
        echo "would run: gh project create / edit / link, then add $(echo "$items" | jq length) cards"
        exit 0
    fi
    number="$(gh project create --owner "$OWNER" --title "$TITLE" --format json | jq -r .number)"
    gh project edit "$number" --owner "$OWNER" --visibility PUBLIC \
        --description "Where ITSaNAS stands: milestones, the MVP acceptance tests, and what is deferred." \
        --readme "Cards are generated from docs/ROADMAP.md and docs/MVP.md by scripts/project-sync.sh. Edit the documents, not the cards."
    gh project link "$number" --owner "$OWNER" --repo "$REPO"
fi

project_id="$(gh project view "$number" --owner "$OWNER" --format json | jq -r .id)"
fields="$(gh project field-list "$number" --owner "$OWNER" --format json)"
status_id="$(echo "$fields" | jq -r '.fields[] | select(.name == "Status") | .id')"
[ -n "$status_id" ] || { echo "project-sync: the board has no Status field" >&2; exit 1; }

option_id() {
    echo "$fields" | jq -r --arg n "$1" \
        '.fields[] | select(.name == "Status") | .options[] | select(.name == $n) | .id'
}

existing="$(gh project item-list "$number" --owner "$OWNER" --limit 500 --format json)"

created=0 moved=0 same=0
while IFS= read -r row; do
    title="$(echo "$row" | jq -r .title)"
    want="$(echo "$row" | jq -r .status)"
    body="$(echo "$row" | jq -r .body)"
    opt="$(option_id "$want")"
    [ -n "$opt" ] || { echo "project-sync: the Status field has no option '$want'" >&2; exit 1; }

    have_id="$(echo "$existing" | jq -r --arg t "$title" '.items[] | select(.title == $t) | .id' | head -n1)"
    have_status="$(echo "$existing" | jq -r --arg t "$title" '.items[] | select(.title == $t) | .status // ""' | head -n1)"

    if [ -z "$have_id" ]; then
        echo "  new    [$want] $title"
        created=$((created + 1))
        if [ "$dry" = 0 ]; then
            have_id="$(gh project item-create "$number" --owner "$OWNER" \
                --title "$title" --body "$body" --format json | jq -r .id)"
            gh project item-edit --id "$have_id" --project-id "$project_id" \
                --field-id "$status_id" --single-select-option-id "$opt"
        fi
    elif [ "$have_status" != "$want" ]; then
        echo "  move   [$have_status -> $want] $title"
        moved=$((moved + 1))
        run project item-edit --id "$have_id" --project-id "$project_id" \
            --field-id "$status_id" --single-select-option-id "$opt"
    else
        same=$((same + 1))
    fi
done < <(echo "$items" | jq -c '.[]')

echo "$existing" | jq -r --argjson want "$items" \
    '.items[] | select(.title as $t | ($want | map(.title) | index($t)) | not) | .title' \
    | while IFS= read -r orphan; do
        echo "  orphan (not in the documents, left alone): $orphan"
    done

echo "project-sync: $created new, $moved moved, $same unchanged"
