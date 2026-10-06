# -*- coding: utf-8 -*-
"""The CI's scope decision tests what it must, and ci.yml obeys it.

Why this exists
---------------

`scripts/ci_scope.py` decides, for each pull request, which jobs run and which
crates they test (docs/ROADMAP.md, "Selective CI"). A mistake there does not
fail anything: it makes CI green by testing less. That is the one kind of bug a
CI cannot catch by running, so it is caught here, hermetically -- no git, no
GitHub, and cargo only to read the real workspace graph.

What is checked
---------------

**The decision**, against a fixed graph and against the real workspace:

- a change to `itsanas-crypto` alone tests every crate that depends on it,
  computed here a second way (a fixpoint, not the script's queue);
- a docs-only or scripts-only pull request runs no Rust test job;
- `Cargo.lock`, the workspace manifest, the toolchain, `.config/`, a workflow
  and the scope script itself each force the full run, as do every event
  that is not a pull request and the `milestone` label;
- a path nobody classified forces the full run;
- the Android app's Kotlin sources belong to `itsanas-android`.

**The verdict on a finished run** (`check_needs`): a job skipped by the plan
with a reason passes; a job skipped that the plan said would run, a failure, or
a missing plan does not.

**ci.yml itself**, as text: every job the plan decides is gated on its own
output and on nothing else; the `test` matrix is gated by step, never by job
(a skipped matrix job reports under an unexpanded name that no required check
matches); the last job needs every gated job and runs `ci_scope.py verify`;
android-core checks exactly the crates the plan assumes it does.

Each check prints `test <name> ... ok|FAILED`, the shape `scripts/sabotage.py`
reads, so its sabotages run through that script.
"""

import importlib.util
import pathlib
import re
import shutil
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("ci_scope", ROOT / "scripts" / "ci_scope.py")
scope = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scope)

failures = []


def check(name, condition, detail=""):
    print(f"test {name} ... {'ok' if condition else 'FAILED'}")
    if not condition:
        failures.append(f"{name}: {detail}")


# A small graph shaped like the real one: a primitive everything reaches, a
# leaf nobody depends on, a binary, and a crate reached only by a dev-dependency.
FAKE = {
    "itsanas-crypto": {"dir": "crates/itsanas-crypto", "deps": [], "lib": True},
    "itsanas-store": {"dir": "crates/itsanas-store", "deps": ["itsanas-crypto", "itsanas-testkit"], "lib": True},
    "itsanas-testkit": {"dir": "crates/itsanas-testkit", "deps": ["itsanas-crypto"], "lib": True},
    "itsanas-sync": {"dir": "crates/itsanas-sync", "deps": ["itsanas-store"], "lib": True},
    "itsanas-wire": {"dir": "crates/itsanas-wire", "deps": [], "lib": True},
    "itsanas-cli": {"dir": "crates/itsanas-cli", "deps": ["itsanas-sync", "itsanas-wire"], "lib": False},
    "itsanas-coordinator": {"dir": "crates/itsanas-coordinator", "deps": ["itsanas-wire"], "lib": False},
    "itsanas-android": {"dir": "crates/itsanas-android", "deps": ["itsanas-store"], "lib": False},
}


# The two crates in FAKE that a crypto change must *not* reach.
NOT_ON_CRYPTO = {"itsanas-wire", "itsanas-coordinator"}


def pr(files, packages=FAKE, labels=()):
    return scope.decide("pull_request", list(labels), files, packages)


def runs(result):
    return {job for job, j in result["jobs"].items() if j["run"]}


RUST_JOBS = {"test", "slow-tests", "cross-build", "minimum-rust-version"}

# ------------------------------------------------------------- the decision

r = pr(["crates/itsanas-crypto/src/lib.rs"])
check("crypto_alone_tests_every_dependent",
      set(r["affected"]) == set(FAKE) - NOT_ON_CRYPTO and not r["full"],
      f"affected {r['affected']}, expected {sorted(set(FAKE) - NOT_ON_CRYPTO)}")
check("crypto_alone_names_each_crate_to_nextest",
      all(f"-p {name}" in r["test_args"] for name in set(FAKE) - NOT_ON_CRYPTO) and "--workspace" not in r["test_args"],
      r["test_args"])
check("doctests_skip_crates_without_a_library",
      "itsanas-cli" not in r["doc_args"] and "-p itsanas-crypto" in r["doc_args"], r["doc_args"])

r = pr(["crates/itsanas-wire/src/frame.rs"])
check("a_leaf_change_tests_its_dependents_and_nothing_else",
      r["affected"] == ["itsanas-cli", "itsanas-coordinator", "itsanas-wire"], r["affected"])
r = pr(["crates/itsanas-coordinator/src/main.rs"])
check("a_change_without_android_core_skips_android",
      not r["jobs"]["android-core"]["run"] and r["jobs"]["android-core"]["why"])

r = pr(["crates/itsanas-testkit/src/lib.rs"])
check("a_dev_dependency_counts_as_a_dependency",
      {"itsanas-store", "itsanas-sync", "itsanas-cli"} <= set(r["affected"]), r["affected"])

for files, what in (
    (["docs/ROADMAP.md", "README.md", "FIRST-STEPS.md"], "docs"),
    (["scripts/check-counts.py", "scripts/receipt.sh"], "scripts"),
):
    r = pr(files)
    check(f"{what}_only_runs_no_rust_test_job",
          not (runs(r) & RUST_JOBS) and not r["full"] and not r["affected"],
          f"runs {sorted(runs(r))}, affected {r['affected']}")
    check(f"{what}_only_writes_why_each_job_skips",
          all(j["why"] for j in r["jobs"].values()))

r = pr(["install/linux.sh"])
check("an_installer_change_runs_the_installers_and_no_rust_test",
      runs(r) == {"installers-run"}, sorted(runs(r)))
r = pr(["scripts/acceptance.sh"])
check("an_acceptance_script_change_runs_acceptance", runs(r) == {"acceptance-local"}, sorted(runs(r)))
r = pr(["android/app/src/main/java/fr/ngas/itsanas/Native.kt"])
check("the_kotlin_side_belongs_to_itsanas_android",
      r["affected"] == ["itsanas-android"], r["affected"])

for path in ("Cargo.lock", "Cargo.toml", "rust-toolchain.toml", ".config/nextest.toml",
             ".cargo/config.toml", ".github/workflows/ci.yml", "scripts/ci_scope.py"):
    r = pr(["docs/ROADMAP.md", path])
    check(f"full_run_on_{re.sub(r'[^a-z]+', '_', path.lower()).strip('_')}",
          # Full *because of this file*, not through the unknown-path fallback:
          # the first version only checked `full`, and with Cargo.lock taken
          # off the list the run was still full -- as an "unclassified path".
          # That passes today and fails the day someone classifies root files.
          r["full"] and runs(r) == set(scope.JOBS) and r["test_args"] == "--workspace"
          and f"`{path}` changed, and every test depends on it" in r["reason"],
          f"{path}: full={r['full']}, runs {sorted(runs(r))}, reason {r['reason']}")

for event in ("push", "schedule", "workflow_dispatch"):
    r = scope.decide(event, [], [], FAKE)
    check(f"full_run_on_{event}", r["full"] and runs(r) == set(scope.JOBS))
r = pr(["docs/ROADMAP.md"], labels=["milestone"])
check("full_run_on_the_milestone_label", r["full"] and "coverage" in runs(r))
r = pr(["somewhere-new/thing.txt"])
check("an_unclassified_path_runs_everything", r["full"], r["reason"])

# --------------------------------------------------- against the real workspace

if shutil.which("cargo"):
    real = scope.workspace_packages()
    # The oracle, computed differently on purpose: grow a set until nothing new
    # joins it, rather than walking a reverse graph with a queue.
    expected = {"itsanas-crypto"}
    while True:
        grown = expected | {n for n, info in real.items() if set(info["deps"]) & expected}
        if grown == expected:
            break
        expected = grown
    r = pr(["crates/itsanas-crypto/src/lib.rs"], packages=real)
    check("real_workspace_crypto_reaches_every_dependent",
          set(r["affected"]) == expected and len(expected) >= 10 and "itsanas-cli" in expected,
          f"plan {r['affected']}, fixpoint {sorted(expected)}")
    r = pr(["crates/itsanas-store/src/lib.rs"], packages=real)
    check("real_workspace_store_reaches_the_drive_and_the_cli",
          {"itsanas-drive", "itsanas-cli", "itsanas-node"} <= set(r["affected"]), r["affected"])
    check("every_real_crate_is_under_a_directory_the_plan_maps",
          all(info["dir"].startswith("crates/") for info in real.values()),
          {n: i["dir"] for n, i in real.items()})
else:
    print("skip  the real-workspace checks (no cargo here); CI's lint job has one")

# ------------------------------------------------------- the verdict on a run

plan = {job: {"run": False, "why": "nothing affected"} for job in scope.JOBS}
plan["test"] = {"run": True, "why": "affected crates"}
ok_changes = {"result": "success", "outputs": {"plan": __import__("json").dumps(plan)}}
needs = {"changes": ok_changes, "test": {"result": "success"}, "coverage": {"result": "skipped"}}
check("verdict_accepts_a_skip_the_plan_explained", scope.check_needs(needs) == [])
needs = {"changes": ok_changes, "test": {"result": "skipped"}}
check("verdict_refuses_a_skip_the_plan_did_not_decide", scope.check_needs(needs) != [])
needs = {"changes": ok_changes, "coverage": {"result": "failure"}}
check("verdict_refuses_a_failure", scope.check_needs(needs) != [])
needs = {"changes": {"result": "failure", "outputs": {}}, "coverage": {"result": "skipped"}}
check("verdict_refuses_any_skip_without_a_plan", scope.check_needs(needs) != [])
blank = dict(plan, coverage={"run": False, "why": ""})
needs = {"changes": {"result": "success", "outputs": {"plan": __import__("json").dumps(blank)}},
         "coverage": {"result": "skipped"}}
check("verdict_refuses_a_skip_with_no_reason", scope.check_needs(needs) != [])

# ------------------------------------------------------------- ci.yml obeys

workflow = (ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
jobs_text = workflow.split("\njobs:\n", 1)[1]
blocks = dict(re.findall(r"^  ([a-z][a-z0-9-]*):\n(.*?)(?=^  [a-z][a-z0-9-]*:\n|\Z)", jobs_text, re.M | re.S))

check("ci_has_a_changes_job_running_the_plan",
      "changes" in blocks and "ci_scope.py plan" in blocks["changes"])
for job, key in scope.JOBS.items():
    body = blocks.get(job, "")
    gate = f"needs.changes.outputs.{key} == 'true'"
    job_if = re.search(r"^    if: (.*)$", body, re.M)
    if job == "test":
        steps_ran = re.findall(r"^      - (?:run|name): .*?(?=^      - |\Z)", body, re.M | re.S)
        test_steps = [s for s in steps_ran if "cargo nextest" in s or "cargo test" in s]
        check("ci_test_matrix_is_gated_by_step_never_by_job",
              job_if is None and test_steps and all(gate in s for s in test_steps),
              "a job-level `if:` on a matrix skips under the name `Test (${{ matrix.os }})`, "
              "which no required check matches")
    else:
        check(f"ci_{job.replace('-', '_')}_is_gated_on_its_own_output",
              job_if is not None and gate in job_if.group(1) and "needs: changes" in body,
              f"expected `if: ... {gate}` and `needs: changes` on job {job}")
    if job not in ("coverage", "android-core", "installers-run", "acceptance-local"):
        uses_args = any(a in body for a in ("test_args", "build_args", "doc_args"))
        check(f"ci_{job.replace('-', '_')}_tests_the_planned_crates_not_the_workspace",
              uses_args and "--workspace" not in re.sub(r"#.*", "", body),
              f"job {job} must take its crates from the plan")

last = blocks.get("no-warnings", "")
needed = set(re.findall(r"^      - ([a-z-]+)$", last, re.M))
check("ci_last_job_needs_every_gated_job",
      set(scope.JOBS) | {"changes", "lint", "supply-chain"} <= needed, sorted(needed))
check("ci_last_job_runs_even_after_skips_and_verifies_them",
      "ci_scope.py verify" in last and "!cancelled()" in last)
android = re.findall(r"-p (itsanas-[a-z]+)", blocks.get("android-core", ""))
check("ci_android_core_checks_what_the_plan_assumes",
      sorted(android) == sorted(scope.ANDROID_CORE), f"{sorted(android)} vs {sorted(scope.ANDROID_CORE)}")
labels = re.search(r"pull_request:\n\s+types: \[([^\]]*)\]", workflow)
check("ci_reruns_when_a_label_is_added",
      labels is not None and "labeled" in labels.group(1),
      "without `labeled`, adding `milestone` to an open PR changes nothing until the next push")

if failures:
    print("\nthe CI scope is wrong:")
    for failure in failures:
        print(f"  {failure}")
    print("\nA scope bug makes CI green by testing less. See scripts/ci_scope.py.")
    sys.exit(1)
print("\nci scope: every check passed")
