# -*- coding: utf-8 -*-
"""Every gate in `scripts/` is actually run by CI.

Why this exists
---------------

`scripts/check-unsafe.py` was written, sabotage-verified against three separate
rules, documented in two places, and **never added to `.github/workflows/ci.yml`**.
For two days the gate holding this project's central claim -- that the only
unsafe code is in two named places -- ran on one laptop and on no push. Anybody
could have added an `unsafe` block anywhere and CI would have been green.

It was found by asking an adversarial reader "which of these gates is
decorative?", which is not a process either.

The cause is not carelessness, it is a **by-name list**. `check-all.sh`
discovers gates with a glob; `ci.yml` names seven of them in seven steps. Adding
a file to `scripts/` is therefore enough to be run locally and not enough to be
run anywhere else, and the gap is invisible because both places look complete.

`check-installers.sh` has this exact warning in its own header, about installers
rather than gates:

    Scripts are **discovered, not listed**. The first version named two files,
    so `install/coordinator.sh` was written, added, and checked by nothing --
    which is exactly the failure this file exists to prevent, made while
    writing it.

The same mistake, one level up, in the file that decides what runs.

Why the steps stay named
------------------------

The obvious fix is for CI to run `check-all.sh` and stop enumerating. It is
rejected: a failure then arrives as one red step called "the gates", and the
seven comments in `ci.yml` explaining *why each gate exists* -- several of them
the only record of the bug that caused it -- would have nowhere to live. Named
steps are worth keeping. What is not worth keeping is nothing noticing when the
list falls behind.

The rule
--------

Every `scripts/check-*.py` and `scripts/check-*.sh` must be named somewhere in
`.github/workflows/ci.yml`, except the runner itself, which is what the others
are run *by*.
"""

import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
SCRIPTS = ROOT / "scripts"
WORKFLOWS = ROOT / ".github" / "workflows"

# The runner is not a gate; it is how a contributor runs the gates by hand.
# Naming it in CI would make CI run everything twice.
NOT_A_GATE = {"check-all.sh"}


def gates():
    """Every gate, discovered rather than listed. That is the whole point."""
    found = []
    for path in sorted(SCRIPTS.glob("check-*")):
        if path.suffix not in (".py", ".sh"):
            continue
        if path.name in NOT_A_GATE:
            continue
        found.append(path.name)
    return found


def main():
    workflows = sorted(WORKFLOWS.glob("*.yml")) + sorted(WORKFLOWS.glob("*.yaml"))
    if not workflows:
        print(f"no workflow files under {WORKFLOWS.relative_to(ROOT).as_posix()}")
        print("Either CI moved and this script did not, or nothing runs on a push.")
        return 1

    text = "\n".join(path.read_text(encoding="utf-8") for path in workflows)

    found = gates()
    if not found:
        print("scripts/ has no gates at all, which is either a move or a deletion")
        return 1

    unwired = [name for name in found if name not in text]

    if unwired:
        print("gates that run on a laptop and on no push:")
        for name in unwired:
            print(f"  scripts/{name}")
        print()
        print(
            "A gate CI does not run is worse than no gate: it is written down as\n"
            "evidence, it passes when somebody runs it by hand, and it lets\n"
            "through everything it was built to stop. `check-unsafe.py` sat like\n"
            "this for two days, holding the claim that this project has no unsafe\n"
            "code outside two named files.\n"
            "\n"
            "Add a step to .github/workflows/ci.yml -- with a comment saying what\n"
            "the gate is for, like the ones already there."
        )
        return 1

    # A name in a comment is not a gate that runs, and this file is full of
    # comments naming gates -- deliberately, since each one records the bug that
    # caused it. So the comments come out before looking again. Checked
    # separately from the first pass so the message can say which of the two
    # failed: "not mentioned at all" and "mentioned only in prose" are different
    # mistakes with different fixes.
    # Cutting at the first `#`, not skipping lines that begin with one. The
    # first version did the latter, and its own sabotage test walked straight
    # through: `run: true  # was: check-unsafe.py` kept the name on a line that
    # does not start with a comment, so the gate reported success over a gate
    # that had just been switched off. A half-working rule that prints a
    # reassuring line is the thing this file exists to stop, and it was in this
    # file.
    #
    # A `#` inside a quoted shell string would truncate a real invocation and
    # fail loudly here. That is the right direction to be wrong in, and there is
    # no such line today.
    commands = [line.split("#", 1)[0] for line in text.splitlines()]
    unrun = [name for name in found if not any(name in line for line in commands)]

    if unrun:
        print("gates named in CI only inside a comment:")
        for name in unrun:
            print(f"  scripts/{name}")
        print()
        print("A mention is not an invocation. Put it behind a `run:`.")
        return 1

    print(f"CI runs all {len(found)} gates in scripts/, discovered rather than listed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
