#!/usr/bin/env python3
"""Refuse a pull request whose author has not agreed to CLA.md.

Run by .github/workflows/cla.yml on every pull request, with the author and
the description passed in environment variables (PR_AUTHOR, PR_BODY), never
pasted into a shell command: the description is written by whoever opened the
pull request.

Why this exists: Nicolas holds all the rights in ITSaNAS, and CLA.md has
every contribution assigned to him. One external patch merged without that
agreement would leave a part of the code he does not own, for good, and
nothing would say so. This makes "no agreement" a red check instead of a silence.

    python scripts/cla_check.py            # reads PR_AUTHOR and PR_BODY
    python scripts/cla_check.py --self-test
"""
import os
import sys

# The sentence CLA.md §6 asks for, exactly. Whitespace and case are forgiven
# (a description wrapped by an editor is still an agreement), nothing else.
AGREEMENT = "I have read the ITSaNAS Contributor Agreement (CLA.md, version 1.0) and I agree to it."

# Who needs no agreement: the Maintainer, and the bots that propose dependency
# updates (their changes are other projects' code under those projects'
# licences, not contributions of their own).
EXEMPT = {"SigSegGit", "dependabot[bot]"}


def normalise(text: str) -> str:
    return " ".join(text.split()).casefold()


def verdict(author: str, body: str) -> tuple[bool, str]:
    if not author:
        return False, "no pull request author given: the check cannot say who agreed"
    if author in EXEMPT:
        return True, f"{author} needs no contributor agreement"
    if normalise(AGREEMENT) in normalise(body or ""):
        return True, f"{author} agreed to CLA.md version 1.0 in this pull request"
    return False, (
        f"{author} has not agreed to the contributor licence agreement.\n"
        "Read CLA.md, then add this sentence to the pull request's description:\n\n"
        f"    {AGREEMENT}\n\n"
        "Without it the contribution is not accepted (CLA.md)."
    )


def self_test() -> int:
    cases = [
        ("SigSegGit", "", True),
        ("dependabot[bot]", "", True),
        ("stranger", "", False),
        ("stranger", "Fixes a typo.", False),
        ("stranger", "Fixes a typo.\n\n" + AGREEMENT, True),
        ("stranger", "i have read the itsanas contributor agreement (CLA.md, version 1.0)\nand I agree to it.", True),
        ("stranger", "I have read the ITSaNAS Contributor Agreement (CLA.md, version 0.9) and I agree to it.", False),
        ("stranger", "I have read the ITSaNAS Contributor Agreement and I do not agree to it.", False),
        ("", AGREEMENT, False),
        ("sigseggit-impostor", "", False),
    ]
    failed = 0
    for author, body, want in cases:
        got, _ = verdict(author, body)
        if got != want:
            failed += 1
            print(f"  FAIL  author={author!r} body={body[:50]!r}: got {got}, wanted {want}")
    print(f"cla_check self-test: {len(cases) - failed} of {len(cases)} cases right")
    return 1 if failed else 0


def main() -> int:
    if "--self-test" in sys.argv:
        return self_test()
    ok, why = verdict(os.environ.get("PR_AUTHOR", ""), os.environ.get("PR_BODY", ""))
    print(why)
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
