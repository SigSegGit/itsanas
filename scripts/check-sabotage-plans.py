#!/usr/bin/env python3
"""Every sabotage plan parses, and each defence's anchor is in its file once.

`scripts/sabotage.py` finds both out only in the Sabotage job, a quarter of an
hour after a push. Two plans of #378 went red there on a line break inside a JSON
string, and a third on a defence no test could see. This costs a second.
"""

import glob
import json
import sys

problems = []
for path in sorted(glob.glob("scripts/sabotage/*.json")):
    try:
        with open(path, encoding="utf-8") as handle:
            plan = json.load(handle)
    except ValueError as why:
        problems.append(f"{path}: not JSON: {why}")
        continue
    for name, defence in plan["defences"].items():
        try:
            with open(defence["file"], encoding="utf-8") as handle:
                found = handle.read().count(defence["live"])
        except OSError as why:
            problems.append(f"{path}: {name}: {why}")
            continue
        if found != 1:
            problems.append(f"{path}: {name}: its anchor appears {found} times in {defence['file']}")

if problems:
    print("sabotage plans that cannot run:")
    for problem in problems:
        print(f"  {problem}")
    print("\nA plan is read by a job nobody watches: fix the anchor, or the plan proves nothing.")
    sys.exit(1)
print("check-sabotage-plans: every plan parses and every anchor appears once")
