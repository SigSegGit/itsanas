#!/usr/bin/env python3
"""Every test has a deadline, and every way of running them enforces it.

Why this exists
---------------

A job in this project ran for five hours before a person stopped it. `cargo
test` cannot prevent that: it has no per-test timeout, so a test that blocks
blocks the run, and the only thing that ends it is somebody's patience.

`cargo nextest` can, because it runs each test in its own process. The budget
lives in `.config/nextest.toml`. This file checks that the budget is still
there, that it still *terminates* rather than merely warning, and — the part
that actually rots — that every place tests are run still goes through the
runner that enforces it.

The last one is the reason this script exists rather than a comment. A timeout
configured in a file nothing uses is worse than no timeout, because it reads
like a guarantee. One `cargo test` restored to `ci.yml` during a merge, and the
budget silently stops applying to the job that matters, while this file still
sits in the repository saying one minute.

What it does not check
----------------------

Whether any test is actually near the limit. That needs running them, which is
what `cargo nextest run --profile measure` is for, and the measurements are
recorded in `docs/TESTING.md`. This is the cheap static half.
"""

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CONFIG = ROOT / ".config" / "nextest.toml"
WORKFLOWS = ROOT / ".github" / "workflows"

# The ceiling, in seconds. Stated here as well as in the config so that raising
# it is two edits and a decision rather than one edit and a habit.
BUDGET = 60

# Profiles allowed to exceed it, and why. Keeping the reason here rather than
# only in the TOML means the exemption is visible from the gate: running this
# script prints what is exempt, so an exemption cannot quietly become the norm
# by being somewhere nobody looks.
EXEMPT = {
    "ci-emulated": (
        300,
        "runs the suite under qemu-user-static on an x86 runner. At 60s that job "
        "reported eight timeouts and 724s for the suite, while the same suite on "
        "a real Raspberry Pi 4 has nothing near a minute -- so the limit there "
        "measures the emulator, not this code.",
    ),
}

problems = []
exemptions = []


def parse_timeout(profile, name):
    """The seconds after which this profile kills a test, or None."""
    timeout = profile.get("slow-timeout")
    if not isinstance(timeout, dict):
        return None
    after = timeout.get("terminate-after")
    if after is None:
        return None
    period = timeout.get("period", "")
    seconds = re.match(r"^\s*(\d+)\s*s\s*$", str(period))
    if not seconds:
        problems.append(
            f"profile `{name}` has a period of {period!r} this script cannot read; "
            "write it in whole seconds, e.g. \"20s\""
        )
        return None
    return int(seconds.group(1)) * int(after)


if not CONFIG.exists():
    problems.append(f"{CONFIG.relative_to(ROOT)} is gone, so nothing bounds a test")
    print("\n".join(problems))
    sys.exit(1)

config = tomllib.loads(CONFIG.read_text(encoding="utf-8"))
profiles = config.get("profile", {})

# Which profiles CI actually asks for. A profile nothing selects cannot let a
# test run unbounded, and `measure` deliberately does not terminate -- so the
# rule is about the profiles in use, not about every profile that exists.
workflow_text = ""
if WORKFLOWS.is_dir():
    for workflow in sorted(WORKFLOWS.glob("*.yml")) + sorted(WORKFLOWS.glob("*.yaml")):
        workflow_text += workflow.read_text(encoding="utf-8")

used = set(re.findall(r"--profile[= ]([A-Za-z0-9_-]+)", workflow_text))

# `default` is what a person gets locally when they pass no profile at all, so
# it is checked whether or not CI names it: a budget that only the pipeline
# honours stops applying the moment somebody runs the suite by hand.
for name in sorted({"default"} | used):
    profile = profiles.get(name)
    if profile is None:
        problems.append(
            f"a workflow asks for profile `{name}`, which is not in "
            f"{CONFIG.relative_to(ROOT)}; nextest would fall back and the budget "
            "would not apply"
            if name in used
            else f"there is no `{name}` profile, so its runs are unbounded"
        )
        continue

    limit = parse_timeout(profile, name)
    ceiling, reason = EXEMPT.get(name, (BUDGET, None))

    if limit is None:
        problems.append(
            f"profile `{name}` does not terminate a slow test. A `slow-timeout` "
            "without `terminate-after` prints a warning and waits forever, which "
            "is the behaviour this file exists to prevent."
        )
    elif limit > ceiling:
        problems.append(
            f"profile `{name}` allows {limit}s per test; the ceiling for it is "
            f"{ceiling}s"
        )

    if reason and limit is not None:
        exemptions.append(f"`{name}` is allowed {ceiling}s: {reason}")

    if profile.get("retries", 0):
        problems.append(
            f"profile `{name}` retries failed tests. A test that passes on the "
            "second attempt hides the timing bugs the timeout is for."
        )

# Overrides are exemptions. They are allowed -- some test may one day be slow
# for a reason that is the point of it -- but each one must say why, in a
# comment immediately above it, because an exemption nobody can justify is one
# nobody will remove.
text = CONFIG.read_text(encoding="utf-8")
for match in re.finditer(r"^\[\[profile\.[a-z]+\.overrides\]\]", text, re.MULTILINE):
    before = text[: match.start()].rstrip().split("\n")
    if not before or not before[-1].lstrip().startswith("#"):
        line = text[: match.start()].count("\n") + 1
        problems.append(
            f"{CONFIG.relative_to(ROOT)}:{line}: a timeout override with no comment "
            "above it saying why the test's slowness is unavoidable"
        )

# Every test invocation in CI must go through the runner that enforces the
# budget. `cargo test --doc` is the one exception and is named, not inferred:
# nextest does not run doctests, so they need `cargo test`, and leaving them out
# would drop two tests while the summary still said everything passed.
RUNS_TESTS = re.compile(r"\bcargo\s+(?:\+\S+\s+)?test\b(?![^\n]*--doc\b)")

if not WORKFLOWS.is_dir():
    problems.append(".github/workflows is missing")
else:
    workflows = sorted(WORKFLOWS.glob("*.yml")) + sorted(WORKFLOWS.glob("*.yaml"))
    if not workflows:
        problems.append(".github/workflows has no workflow files")
    for workflow in workflows:
        for number, line in enumerate(
            workflow.read_text(encoding="utf-8").split("\n"), start=1
        ):
            if line.lstrip().startswith("#"):
                continue
            if RUNS_TESTS.search(line):
                problems.append(
                    f"{workflow.relative_to(ROOT)}:{number}: `cargo test` runs tests "
                    "with no per-test timeout. Use `cargo nextest run --profile ci`, "
                    "or `cargo test --doc` for the doctests nextest cannot run."
                )

    doctests = any(
        "--doc" in workflow.read_text(encoding="utf-8") for workflow in workflows
    )
    if not doctests:
        problems.append(
            "no workflow runs `cargo test --doc`. nextest does not run doctests, so "
            "moving to it without this stops running them silently."
        )

if problems:
    print("the test budget is not enforced:")
    for problem in problems:
        print(f"  {problem}")
    print()
    print("A timeout configured in a file nothing uses reads like a guarantee")
    print("and is not one. `.config/nextest.toml` explains the budget and why")
    print("it currently has no exceptions.")
    sys.exit(1)

print(
    f"test budget: {BUDGET}s per test, terminating; "
    f"every CI test invocation goes through nextest, and the doctests are run"
)
for exemption in exemptions:
    print(f"  exempt: {exemption}")
