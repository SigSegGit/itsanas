#!/usr/bin/env python3
"""Read docs/SESSIONS.md and answer the two questions it exists for.

1. Which sessions stopped before the end? A session cut by the usage limit
   leaves no trace of its own; its row is written by the next session (`Fin` =
   `interrompue`). Listing them is how a half-done step gets spotted instead of
   silently restarted from scratch.
2. What did a step cost? Rows are grouped by the `Étape` column (the §8 id of
   HANDOVER.md, or `hors-§8`) and their 5-hour-window points summed. That sum,
   interrupted sessions included, is the price of the feature -- the number to
   compare against how far it moved the project.

Points are the unit because they are what the plan actually meters; context
tokens are shown but not summed (a session's context is not what it billed).
`n/d` counts as unknown, not as zero, and the step is flagged as a lower bound.

Not a gate: nothing here can be wrong in a way CI should refuse. A malformed row
is reported and skipped.
"""
import re
import sys
from collections import OrderedDict
from pathlib import Path

JOURNAL = Path(__file__).resolve().parent.parent / "docs" / "SESSIONS.md"
POINTS = re.compile(r"\+\s*(\d+)\s*pts")


def rows(text):
    header = None
    for line in text.splitlines():
        if not line.startswith("|") or line.startswith("|---"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if header is None:
            header = cells
            continue
        if len(cells) != len(header):
            print(f"skipped malformed row: {line}", file=sys.stderr)
            continue
        yield dict(zip(header, cells))


def main():
    # Windows consoles default to cp1252 and would mangle "Étape" and "§".
    sys.stdout.reconfigure(encoding="utf-8")
    steps = OrderedDict()
    interrupted = []
    for r in rows(JOURNAL.read_text(encoding="utf-8")):
        step = r.get("Étape", "?")
        s = steps.setdefault(step, {"sessions": 0, "points": 0, "unknown": 0, "cut": 0})
        s["sessions"] += 1
        m = POINTS.search(r.get("Tokens", ""))
        if m:
            s["points"] += int(m.group(1))
        else:
            s["unknown"] += 1
        if r.get("Fin") == "interrompue":
            s["cut"] += 1
            interrupted.append(r)

    print("Interrupted sessions (resume these before starting anything new):")
    if not interrupted:
        print("  none")
    for r in interrupted:
        print(f"  {r['Début']}  [{r.get('Étape', '?')}]  {r['Focus']}  PR {r['PR']}")

    print("\nCost per step (5 h window points, newest step first):")
    print(f"  {'Étape':<10} {'sessions':>8} {'cut':>4} {'points':>7}")
    for step, s in steps.items():
        floor = "+" if s["unknown"] else ""
        print(f"  {step:<10} {s['sessions']:>8} {s['cut']:>4} {str(s['points']) + floor:>7}")
    if any(s["unknown"] for s in steps.values()):
        print("\n  '+' = at least one session with unknown cost; the sum is a lower bound.")


if __name__ == "__main__":
    main()
