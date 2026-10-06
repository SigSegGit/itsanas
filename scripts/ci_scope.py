# -*- coding: utf-8 -*-
"""Decide what a CI run has to test, from what it changed.

Why this exists
---------------

On 2026-10-06 Nicolas asked for the CI to stop re-testing what a pull request
did not touch. Every PR ran everything: a docs-only PR (#225) cost 23
runner-minutes and six of wall-clock, the same as one that rewrote the store.

The rule
--------

A pull request tests the crates it changed **and every crate that depends on
them**, directly or not, dev-dependencies included. A change to `itsanas-crypto`
or `itsanas-store` therefore still tests nearly the whole workspace, and that is
correct: the test that catches a broken primitive usually lives in a crate that
uses it. A Windows-only path in `itsanas-drive` breaks on a change to
`itsanas-store`, and the Windows leg runs `itsanas-drive`'s tests for it.

Everything else runs the **full** suite, unconditionally:

- a push to `main`, the nightly schedule, a manual dispatch -- anything that is
  not a pull request;
- a pull request labelled `milestone` (Nicolas's "major milestones");
- a pull request touching what every test depends on without being a crate:
  the workspace manifest, `Cargo.lock`, the toolchain file, `.config/`
  (nextest's budget), `.cargo/`, any workflow, or this file;
- a pull request touching a path this file does not know. Unknown means
  full: a new top-level directory must be classified here before it can save
  anything, never skipped by default.

What it cannot see, and is the price: something that breaks from the
environment -- a new rustc, a runner image, a dependency republished under the
same version -- is caught only by `main` and the nightly run (docs/ROADMAP.md,
"Selective CI").

Lint and every documentation gate always run; they are not decided here.

Two commands
------------

    python scripts/ci_scope.py plan     # in the `changes` job
    python scripts/ci_scope.py verify   # in the last job, with NEEDS=toJSON(needs)

`plan` writes its decision to `$GITHUB_OUTPUT` and, with the reasons, to the
job summary. `verify` is what makes a skipped job acceptable: it fails unless
every job GitHub reports as skipped is one `plan` decided to skip, with a
reason written down (docs/HANDOVER.md §5, the merge rule).

The decision itself is `decide()`, a pure function, so `check-ci-scope.py` can
test it without git, cargo or GitHub.
"""

import json
import os
import subprocess
import sys

# Not a crate, and every test depends on it. A change here runs everything.
FULL_FILES = {
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain",
    "rust-toolchain.toml",
    "scripts/ci_scope.py",
}
FULL_PREFIXES = (".config/", ".cargo/", ".github/workflows/")

# Read by no test and built into nothing. Lint and the doc gates check them,
# and those always run.
INERT_FILES = {
    "LICENSE",
    ".gitignore",
    ".gitattributes",
    "clippy.toml",  # clippy runs in lint, always
    "rustfmt.toml",  # likewise
    "deny.toml",  # cargo deny runs always
    ".github/dependabot.yml",
    ".github/CODEOWNERS",
}
INERT_PREFIXES = ("docs/", ".claude/", ".github/ISSUE_TEMPLATE/")

# `crates/itsanas-android` compiles the Kotlin side's declarations in with
# include_str! and checks the two agree, so the app's sources belong to it.
OTHER_CRATE_DIRS = {"android/": "itsanas-android"}

# The crates the android-core job checks. check-ci-scope.py compares this with
# the job's own `-p` list, so the two cannot drift apart.
ANDROID_CORE = (
    "itsanas-crypto",
    "itsanas-store",
    "itsanas-sync",
    "itsanas-policy",
    "itsanas-discover",
    "itsanas-placement",
    "itsanas-wire",
)

# Job id in ci.yml -> the output key its `if:` reads. `test` is the one job
# never skipped at job level: its three legs are required checks, and a matrix
# job skipped by `if:` reports under an unexpanded name, "Test (${{ matrix.os
# }})", which no required check matches -- the PR would wait forever. Its steps
# are gated instead, and it passes having said why it tested nothing.
JOBS = {
    "test": "tests",
    "slow-tests": "slow",
    "installers-run": "installers",
    "acceptance-local": "acceptance",
    "cross-build": "arm",
    "android-core": "android",
    "minimum-rust-version": "msrv",
    "coverage": "coverage",
}

MILESTONE_LABEL = "milestone"


def full_reason(event, labels, files):
    """Why this run must test everything, or None."""
    if event != "pull_request":
        return f"event `{event}`: everything not a pull request runs in full"
    if MILESTONE_LABEL in labels:
        return f"labelled `{MILESTONE_LABEL}`"
    for path in files:
        if path in FULL_FILES or path.startswith(FULL_PREFIXES):
            return f"`{path}` changed, and every test depends on it"
    return None


def classify(path, crate_dirs):
    """One changed path -> ("crate", name) | ("inert", None) | ("script", path)
    | ("install", path) | ("unknown", path)."""
    for prefix, name in OTHER_CRATE_DIRS.items():
        if path.startswith(prefix):
            return "crate", name
    # Longest directory first, so a crate nested in another is not swallowed.
    for directory in sorted(crate_dirs, key=len, reverse=True):
        if path.startswith(directory + "/"):
            return "crate", crate_dirs[directory]
    if path in INERT_FILES or path.startswith(INERT_PREFIXES):
        return "inert", None
    if "/" not in path and path.endswith(".md"):
        return "inert", None
    if path.startswith("scripts/"):
        return "script", path
    if path.startswith("install/"):
        return "install", path
    return "unknown", path


def dependents(packages, changed):
    """`changed` plus every package that reaches one of them through its
    dependencies, transitively. `packages` maps name -> {"deps": [...]}."""
    reverse = {name: set() for name in packages}
    for name, info in packages.items():
        for dep in info["deps"]:
            if dep in reverse and dep != name:
                reverse[dep].add(name)
    affected = set()
    queue = [name for name in changed if name in packages]
    while queue:
        name = queue.pop()
        if name in affected:
            continue
        affected.add(name)
        queue.extend(reverse[name])
    return affected


def args_for(names):
    return " ".join(f"-p {name}" for name in sorted(names))


def decide(event, labels, files, packages):
    """The whole decision. `packages` maps crate name -> {"dir": "crates/x",
    "deps": [names], "lib": bool}. Returns a dict; see `plan()` for its use."""
    every = set(packages)
    reason = full_reason(event, labels, files)
    unknown = []
    crates, scripts, install = set(), [], []
    if reason is None:
        crate_dirs = {info["dir"]: name for name, info in packages.items()}
        for path in files:
            kind, what = classify(path, crate_dirs)
            if kind == "crate":
                crates.add(what)
            elif kind == "script":
                scripts.append(what)
            elif kind == "install":
                install.append(what)
            elif kind == "unknown":
                unknown.append(what)
        if unknown:
            reason = f"`{unknown[0]}` is a path ci_scope.py does not classify; unknown means full"

    full = reason is not None
    affected = every if full else dependents(packages, crates)
    rust = bool(affected)
    touched = lambda prefix: any(s.startswith(prefix) for s in scripts)

    def job(run, why_run, why_skip):
        return {"run": bool(full or run), "why": "full run" if full else (why_run if run else why_skip)}

    nothing = "no crate is affected" if not crates else ""
    jobs = {
        "test": job(rust, "affected crates", nothing),
        "slow-tests": job(rust, "affected crates", nothing),
        "installers-run": job(
            bool(install) or touched("scripts/smoke.sh") or "itsanas-cli" in affected,
            "install/, smoke.sh or itsanas-cli affected",
            "neither install/, scripts/smoke.sh nor itsanas-cli is affected",
        ),
        "acceptance-local": job(
            touched("scripts/acceptance") or bool({"itsanas-cli", "itsanas-coordinator"} & affected),
            "acceptance scripts, itsanas-cli or itsanas-coordinator affected",
            "neither the acceptance scripts nor the two binaries are affected",
        ),
        "cross-build": job(rust, "affected crates", nothing),
        "android-core": job(
            bool(set(ANDROID_CORE) & affected),
            "a crate android-core checks is affected",
            "none of the crates android-core checks is affected",
        ),
        "minimum-rust-version": job(rust, "affected crates", nothing),
        # Coverage of a subset is a misleading number, and it gates nothing.
        "coverage": job(False, "", "coverage is measured on full runs only"),
    }
    assert set(jobs) == set(JOBS)

    libs = {name for name in affected if packages[name].get("lib")}
    return {
        "full": full,
        "reason": reason or (f"pull request touching {len(files)} file(s)"),
        "changed": sorted(crates),
        "affected": sorted(affected),
        "jobs": jobs,
        # A partial run may name a crate with no test of the requested kind
        # (most have no #[ignore]d test); nextest calls that an error. A full
        # run keeps that error: an empty workspace would be a real bug.
        "test_args": "--workspace" if full else f"{args_for(affected)} --no-tests=pass",
        "doc_args": "--workspace" if full else args_for(libs),
        "build_args": "--workspace" if full else args_for(affected),
        # The ARM job's smoke test runs the `itsanas` binary it built.
        "smoke": full or "itsanas-cli" in affected,
    }


# ------------------------------------------------------------- the real world


def workspace_packages():
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        capture_output=True, text=True, encoding="utf-8", check=True,
    )
    meta = json.loads(out.stdout)
    root = meta["workspace_root"].replace("\\", "/").rstrip("/") + "/"
    members = set(meta["workspace_members"])
    packages = {}
    for pkg in meta["packages"]:
        if pkg["id"] not in members:
            continue
        manifest = pkg["manifest_path"].replace("\\", "/")
        directory = manifest[len(root):].rsplit("/", 1)[0] if manifest.startswith(root) else ""
        packages[pkg["name"]] = {
            "dir": directory,
            "deps": [d["name"] for d in pkg["dependencies"] if d.get("path")],
            "lib": any(k in ("lib", "rlib", "proc-macro") for t in pkg["targets"] for k in t["kind"]),
        }
    return packages


def changed_files(base_ref):
    out = subprocess.run(
        ["git", "diff", "--name-only", f"origin/{base_ref}...HEAD"],
        capture_output=True, text=True, encoding="utf-8", check=True,
    )
    return [line.strip() for line in out.stdout.splitlines() if line.strip()]


def plan():
    event = os.environ.get("EVENT", "")
    # A push has no pull request, and toJSON of its labels is `null`.
    labels = json.loads(os.environ.get("LABELS") or "[]") or []
    files = []
    try:
        packages = workspace_packages()
    except Exception as error:  # no cargo, a broken manifest: test everything
        print(f"cargo metadata failed ({error}); planning a full run with no crate list")
        packages, event = {}, f"{event} (cargo metadata failed)"
    if event == "pull_request":
        try:
            files = changed_files(os.environ.get("BASE_REF", "main"))
        except Exception as error:
            print(f"git diff failed ({error}); planning a full run")
            event = "pull_request (git diff failed)"
    result = decide(event, labels, files, packages)

    outputs = {key: str(result["jobs"][job]["run"]).lower() for job, key in JOBS.items()}
    outputs.update(
        full=str(result["full"]).lower(),
        reason=result["reason"],
        test_args=result["test_args"],
        doc_args=result["doc_args"],
        build_args=result["build_args"],
        smoke=str(result["smoke"]).lower(),
        plan=json.dumps({job: result["jobs"][job] for job in JOBS}, separators=(",", ":")),
    )
    lines = [f"{k}={v}" for k, v in outputs.items()]
    print("\n".join(lines))
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as out:
            out.write("\n".join(lines) + "\n")

    summary = [
        "## What this run tests",
        "",
        f"**{'Full run' if result['full'] else 'Selective run'}**: {result['reason']}.",
        "",
        f"Changed crates: {', '.join(result['changed']) or 'none'}",
        "",
        f"Tested (changed + every dependent): {', '.join(result['affected']) or 'none'}",
        "",
        "| job | runs | why |",
        "|---|---|---|",
    ]
    summary += [f"| {job} | {'yes' if j['run'] else '**skipped**'} | {j['why']} |"
                for job, j in result["jobs"].items()]
    if files:
        summary += ["", "<details><summary>Changed files</summary>", ""]
        summary += [f"- `{f}`" for f in files] + ["", "</details>"]
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as out:
            out.write("\n".join(summary) + "\n")
    return 0


def check_needs(needs):
    """Every problem with a finished run's jobs, given `toJSON(needs)`."""
    problems = []
    changes = needs.get("changes")
    if not changes or changes.get("result") != "success":
        return ["the `changes` job did not succeed, so no skip in this run is justified"]
    try:
        decided = json.loads(changes["outputs"]["plan"])
    except (KeyError, ValueError):
        return ["the `changes` job wrote no plan, so no skip in this run is justified"]
    for job, info in needs.items():
        result = info.get("result")
        if result in ("failure", "cancelled"):
            problems.append(f"{job}: {result}")
        elif result == "skipped":
            wrote = decided.get(job)
            if wrote is None:
                problems.append(f"{job}: skipped, and the plan never mentions it")
            elif wrote.get("run"):
                problems.append(f"{job}: skipped although the plan said it runs ({wrote.get('why')})")
            elif not wrote.get("why"):
                problems.append(f"{job}: skipped with no reason written down")
    return problems


def verify():
    problems = check_needs(json.loads(os.environ.get("NEEDS") or "{}"))
    if problems:
        print("this run is not green:")
        for problem in problems:
            print(f"  {problem}")
        print("\nA skipped job is green only when `changes` decided to skip it and said why.")
        return 1
    print("every job passed, or was skipped by the plan with a reason written down")
    return 0


if __name__ == "__main__":
    command = sys.argv[1] if len(sys.argv) > 1 else ""
    if command == "plan":
        sys.exit(plan())
    if command == "verify":
        sys.exit(verify())
    print("usage: ci_scope.py plan | verify")
    sys.exit(2)
