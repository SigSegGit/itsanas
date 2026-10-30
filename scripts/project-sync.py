# -*- coding: utf-8 -*-
"""Create, or bring up to date, the public GitHub Project board for this repo.

    python scripts/project-sync.py             # create the board if missing, sync cards
    python scripts/project-sync.py --dry-run   # print what it would do, change nothing

The board is a view of docs/ROADMAP.md and docs/MVP.md, not a second place to
keep status: it takes its cards from `items()` in scripts/check-wiki.py, the same
parse the wiki is drawn from. Change a status in ROADMAP.md or MVP.md, merge, run
this, and the board follows. A card moved by hand on the board is put back by the
next run, on purpose: if the board and the documents disagree the documents are
right.

Needs `gh` signed in with the `project` scope, which `gh auth login` does not
grant by default:

    gh auth refresh -s project

Python rather than shell for the reason given in wiki-publish.py: the owner's
Windows machine has no `jq`, and its `bash` is WSL. `gh` is the only external
tool. It uses GitHub's GraphQL API underneath, which is why a CI job or a cloud
session cannot run it.

What it does not do: `gh` cannot create *views*. The board opens as a plain table.
Add a Board view (group by Status) and a Roadmap view in the browser, once; they
persist and this does not touch them.

Idempotent: cards are matched by exact title. A card whose title is not in the
documents any more (a renamed milestone) is reported, never deleted.
"""

import importlib.util
import json
import os
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
OWNER = os.environ.get("OWNER", "SigSegGit")
REPO = os.environ.get("REPO", "SigSegGit/itsanas")
TITLE = os.environ.get("TITLE", "ITSaNAS roadmap")


def load_items():
    spec = importlib.util.spec_from_file_location("check_wiki", ROOT / "scripts" / "check-wiki.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.items()


def gh(*args, parse=False):
    result = subprocess.run(["gh", *args], capture_output=True, text=True,
                            encoding="utf-8", errors="replace")
    if result.returncode:
        sys.exit(f"project-sync: gh {' '.join(args[:2])} failed:\n{result.stderr.strip()}")
    return json.loads(result.stdout) if parse else result.stdout


def main(argv):
    dry = "--dry-run" in argv
    items = load_items()

    projects = gh("project", "list", "--owner", OWNER, "--limit", "100",
                  "--format", "json", parse=True)["projects"]
    number = next((p["number"] for p in projects if p["title"] == TITLE), None)

    if number is None:
        print(f"project-sync: no project called '{TITLE}' under {OWNER}, creating it")
        if dry:
            print(f"would: create it, make it public, link {REPO}, add {len(items)} cards")
            return 0
        number = gh("project", "create", "--owner", OWNER, "--title", TITLE,
                    "--format", "json", parse=True)["number"]
        gh("project", "edit", str(number), "--owner", OWNER, "--visibility", "PUBLIC",
           "--description", "Where ITSaNAS stands: milestones, the MVP acceptance tests, "
                            "and what is deferred.",
           "--readme", "Cards are generated from docs/ROADMAP.md and docs/MVP.md by "
                       "scripts/project-sync.py. Edit the documents, not the cards.")
        gh("project", "link", str(number), "--owner", OWNER, "--repo", REPO)

    number = str(number)
    project_id = gh("project", "view", number, "--owner", OWNER, "--format", "json",
                    parse=True)["id"]
    fields = gh("project", "field-list", number, "--owner", OWNER, "--format", "json",
                parse=True)["fields"]
    status = next((f for f in fields if f["name"] == "Status"), None)
    if status is None:
        sys.exit("project-sync: the board has no Status field")
    options = {o["name"]: o["id"] for o in status["options"]}

    existing = gh("project", "item-list", number, "--owner", OWNER, "--limit", "500",
                  "--format", "json", parse=True)["items"]
    by_title = {i["title"]: i for i in existing}

    def set_status(item_id, want):
        if want not in options:
            sys.exit(f"project-sync: the Status field has no option '{want}'")
        gh("project", "item-edit", "--id", item_id, "--project-id", project_id,
           "--field-id", status["id"], "--single-select-option-id", options[want])

    created = moved = same = 0
    for card in items:
        have = by_title.get(card["title"])
        if have is None:
            print(f"  new    [{card['status']}] {card['title']}")
            created += 1
            if not dry:
                new = gh("project", "item-create", number, "--owner", OWNER,
                         "--title", card["title"], "--body", card["body"],
                         "--format", "json", parse=True)
                set_status(new["id"], card["status"])
        elif (have.get("status") or "") != card["status"]:
            print(f"  move   [{have.get('status') or ''} -> {card['status']}] {card['title']}")
            moved += 1
            if not dry:
                set_status(have["id"], card["status"])
        else:
            same += 1

    wanted = {c["title"] for c in items}
    for title in by_title:
        if title not in wanted:
            print(f"  orphan (not in the documents, left alone): {title}")

    print(f"project-sync: {created} new, {moved} moved, {same} unchanged")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
