#!/usr/bin/env python3
"""What this public repository must never say or allow, about its own rights.

ITSaNAS is public, and its owner may one day publish it widely, license it on
other terms or sell it. Three things written here would weaken that for good,
and nothing else checks them:

1. **A caveat inside a legal text.** On 2026-10-31 a session wrote "Not yet
   reviewed by a lawyer" at the top of CLA.md: the argument a contributor
   would use against the assignment, published by the project itself. A legal
   text (CLA.md, and any TERMS / PRIVACY / LEGAL file) carries no draft
   marker, no doubt about its validity, no mention of a pending review.
2. **A licence the owner cannot relicense.** Every crate is AGPL-3.0-or-later
   through the workspace, and deny.toml admits only licences that allow
   relicensing on other terms (permissive ones). A copyleft or
   "non-commercial" entry there, or a crate with its own licence, would put
   code in the product the owner cannot offer commercially.
3. **Contributions without the agreement.** CLA.md, its check
   (scripts/cla_check.py) and the workflow that runs it must exist.

    python scripts/check-public-claims.py
    python scripts/check-public-claims.py --self-test
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

LEGAL_NAMES = re.compile(r"^(CLA|TERMS|PRIVACY|LEGAL|CGU|EULA)[\w.-]*\.md$", re.I)
# A sentence that weakens the text it sits in. Case-insensitive.
WEAKENING = re.compile(
    r"lawyer|avocat|attorney|juriste|legal advice|conseil juridique|not (yet )?reviewed|"
    r"\bdraft\b|brouillon|enforceab|opposable|may not be valid|pas valable|"
    r"not legally binding|sans valeur",
    re.I,
)
# Licences that would stop the owner relicensing what ships with the product.
FORBIDDEN_LICENCES = re.compile(
    r"\b(GPL|LGPL|AGPL-1|MPL|EPL|CDDL|EUPL|OSL|SSPL|BUSL|CC-BY-NC|CC-BY-SA|Commons-Clause|"
    r"Elastic|Polyform)", re.I,
)
OWN_LICENCE = "AGPL-3.0-or-later"


def legal_texts(root: Path) -> list[Path]:
    found = [p for p in root.glob("*.md") if LEGAL_NAMES.match(p.name)]
    legal_dir = root / "docs" / "legal"
    if legal_dir.is_dir():
        found += sorted(legal_dir.rglob("*.md"))
    return sorted(found)


def weakened(text: str) -> list[str]:
    return [line.strip() for line in text.splitlines() if WEAKENING.search(line)]


def licence_problems(cargo_files: dict[str, str], deny: str) -> list[str]:
    problems = []
    for name, text in cargo_files.items():
        for line in text.splitlines():
            line = line.strip()
            if line.startswith("license") and not line.startswith("license-file"):
                if line in ("license.workspace = true", f'license = "{OWN_LICENCE}"'):
                    continue
                problems.append(f"{name}: '{line}' (every crate is {OWN_LICENCE}, through the workspace)")
    allow = re.search(r"allow\s*=\s*\[(.*?)\]", deny, re.S)
    for entry in re.findall(r'"([^"]+)"', allow.group(1) if allow else ""):
        if entry != OWN_LICENCE and FORBIDDEN_LICENCES.search(entry):
            problems.append(f"deny.toml admits '{entry}': code under it could not be relicensed")
    if not allow:
        problems.append("deny.toml has no [licenses] allow list: any licence could come in")
    return problems


def check(root: Path) -> list[str]:
    failures = []
    for path in legal_texts(root):
        for line in weakened(path.read_text(encoding="utf-8")):
            failures.append(f"{path.relative_to(root)} weakens itself: '{line[:120]}'")
    for needed in ["CLA.md", "scripts/cla_check.py", ".github/workflows/cla.yml"]:
        if not (root / needed).is_file():
            failures.append(f"{needed} is missing: contributions would come in without the agreement")
    cargo = {str(p.relative_to(root)): p.read_text(encoding="utf-8")
             for p in [root / "Cargo.toml", *sorted((root / "crates").glob("*/Cargo.toml"))]}
    workspace = cargo.get("Cargo.toml", "")
    if f'license = "{OWN_LICENCE}"' not in workspace:
        failures.append(f"Cargo.toml: the workspace licence is not {OWN_LICENCE}")
    deny = (root / "deny.toml").read_text(encoding="utf-8") if (root / "deny.toml").is_file() else ""
    failures += licence_problems(cargo, deny)
    return failures


def self_test() -> int:
    bad = 0
    def expect(what, got, want):
        nonlocal bad
        if got != want:
            bad += 1
            print(f"  FAIL  {what}: got {got!r}, wanted {want!r}")
    expect("a draft marker", bool(weakened("*Draft: not yet reviewed by a lawyer.*")), True)
    expect("a French caveat", bool(weakened("Ce texte n'a pas été relu par un avocat.")), True)
    expect("a plain clause", bool(weakened("You assign to the Maintainer all economic rights.")), False)
    expect("a GPL entry", bool(licence_problems({}, 'allow = ["MIT", "GPL-3.0"]')), True)
    expect("a non-commercial entry", bool(licence_problems({}, 'allow = ["CC-BY-NC-4.0"]')), True)
    expect("the permissive list", licence_problems({}, f'allow = ["{OWN_LICENCE}", "MIT", "Apache-2.0", "BSD-3-Clause"]'), [])
    expect("a crate with its own licence",
           bool(licence_problems({"crates/x/Cargo.toml": 'license = "MIT"'}, 'allow = ["MIT"]')), True)
    expect("a crate on the workspace licence",
           licence_problems({"crates/x/Cargo.toml": "license.workspace = true"}, 'allow = ["MIT"]'), [])
    print(f"check-public-claims self-test: {'all right' if not bad else f'{bad} wrong'}")
    return 1 if bad else 0


def main() -> int:
    if "--self-test" in sys.argv:
        return self_test()
    if self_test():
        return 1
    failures = check(ROOT)
    for failure in failures:
        print(f"  FAIL  {failure}")
    if failures:
        print("\nThe repository is public: fix the text, not the check (AGENTS.md, 'Public repository').")
        return 1
    print("public claims: legal texts carry no caveat, every licence can be relicensed, the CLA is enforced")
    return 0


if __name__ == "__main__":
    sys.exit(main())
