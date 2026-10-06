# -*- coding: utf-8 -*-
"""What CI runs cost: wall-clock and runner-minutes, per run, from GitHub.

Why this exists
---------------

On 2026-10-06 the CI started testing only what a pull request touches
(`scripts/ci_scope.py`). A saving nobody measured is a claim, and the numbers in
docs/ROADMAP.md ("Selective CI") were taken with this script, so they can be
taken again the same way rather than estimated.

Wall-clock is from the run's creation to its last job's end: what a person
waiting for a merge feels. Runner-minutes is the sum over jobs of (end - start),
per OS: what the machines spent. A skipped job costs nothing and counts nothing.

    python scripts/ci-cost.py [how-many] [event]      # defaults: 10 pull_request
    python scripts/ci-cost.py --runs 123 456           # these runs only
"""

import datetime
import json
import subprocess
import sys

WORKFLOW = "ci.yml"


def gh(*args):
    out = subprocess.run(["gh", *args], capture_output=True, text=True, encoding="utf-8")
    if out.returncode != 0:
        raise SystemExit(f"gh {' '.join(args)} failed: {out.stderr.strip()}")
    return json.loads(out.stdout)


def when(text):
    return datetime.datetime.fromisoformat(text.replace("Z", "+00:00"))


def cost(run_id):
    run = gh("run", "view", str(run_id), "--json", "createdAt,jobs,conclusion,headBranch,event")
    minutes = {"Linux": 0.0, "Windows": 0.0, "macOS": 0.0}
    last = None
    ran = 0
    for job in run["jobs"]:
        if not job.get("startedAt") or not job.get("completedAt") or job["conclusion"] == "skipped":
            continue
        start, end = when(job["startedAt"]), when(job["completedAt"])
        if end <= start:
            continue
        ran += 1
        name = job["name"].lower()
        os_ = "Windows" if "windows" in name else "macOS" if "macos" in name else "Linux"
        minutes[os_] += (end - start).total_seconds() / 60
        last = end if last is None or end > last else last
    wall = (last - when(run["createdAt"])).total_seconds() / 60 if last else 0.0
    return run, wall, minutes, ran


def main(argv):
    if argv[:1] == ["--runs"]:
        ids = argv[1:]
    else:
        count = int(argv[0]) if argv else 10
        event = argv[1] if len(argv) > 1 else "pull_request"
        listed = gh("run", "list", "--workflow", WORKFLOW, "--event", event, "--status", "completed",
                    "-L", str(count), "--json", "databaseId")
        ids = [str(r["databaseId"]) for r in listed]
    total_wall = total_min = 0.0
    print("run          conclusion  jobs  wall  linux  windows  macos  total  branch")
    for run_id in ids:
        run, wall, minutes, ran = cost(run_id)
        spent = sum(minutes.values())
        total_wall += wall
        total_min += spent
        print(f"{run_id:<12} {run['conclusion'] or '-':<11} {ran:>4} {wall:5.1f} {minutes['Linux']:6.1f}"
              f" {minutes['Windows']:8.1f} {minutes['macOS']:6.1f} {spent:6.1f}  {run['headBranch']}")
    if ids:
        print(f"mean over {len(ids)} runs: wall {total_wall / len(ids):.1f} min, "
              f"runner {total_min / len(ids):.1f} min")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
