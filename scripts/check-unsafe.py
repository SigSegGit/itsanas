# -*- coding: utf-8 -*-
"""Keep the unsafe exception the size the documentation says it is.

Why this exists
---------------

The workspace forbids unsafe code. Exactly one crate cannot: a Java virtual
machine calls `extern "system"` symbols by name, and Rust treats
`#[unsafe(no_mangle)]` as unsafe because a duplicate exported symbol is
undefined behaviour at link time. So `itsanas-android` relaxes the lint and
allows it at the crate root.

That is a wide door held open by a comment, and a comment is not a control. Two
things could go through it and nobody would notice: an `unsafe` block written
inside that crate, where the allowance already applies, and a second crate
copying the `[lints.rust]` stanza because it was the quickest way past an error.

`docs/PORTING.md` and the crate's own documentation both say "the only unsafe in
the project is the export attribute". This is what makes that sentence a fact
rather than an intention.

The rule
--------

* No `.rs` file in `crates/` may contain `unsafe {`, `unsafe fn`, `unsafe impl`
  or `unsafe trait`, outside a comment or a doc string.
* Only `itsanas-android` may name `unsafe_code` in its own `[lints]`.
* `itsanas-android` must still name it, so that removing the line by accident is
  a failure here rather than a silent widening.

`#[unsafe(no_mangle)]` and `#[unsafe(export_name)]` are the permitted forms, and
only in that crate: they are attributes, not code, and they are the whole reason
the exception exists.
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CRATES = ROOT / "crates"

# The crate that is allowed to relax the lint, and the attribute forms it may
# use. Anything else is a finding.
EXEMPT = "itsanas-android"
PERMITTED_ATTRIBUTES = ("unsafe(no_mangle)", "unsafe(export_name)")

CODE = re.compile(r"\bunsafe\s*(\{|fn\b|impl\b|trait\b)")


def offending_lines(path):
    """Lines of `path` that write unsafe code rather than an export attribute."""
    found = []
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        stripped = line.strip()
        if stripped.startswith("//") or stripped.startswith("*"):
            continue
        if any(form in line for form in PERMITTED_ATTRIBUTES):
            continue
        if CODE.search(line):
            found.append((number, stripped))
    return found


def main():
    problems = []

    for path in sorted(CRATES.rglob("*.rs")):
        if "target" in path.parts:
            continue
        crate = path.relative_to(CRATES).parts[0]
        for number, line in offending_lines(path):
            where = path.relative_to(ROOT).as_posix()
            problems.append(f"{where}:{number} writes unsafe code: {line}")

    relaxed = []
    for manifest in sorted(CRATES.rglob("Cargo.toml")):
        if "target" in manifest.parts:
            continue
        crate = manifest.relative_to(CRATES).parts[0]
        text = manifest.read_text(encoding="utf-8")
        # Only what the crate declares for itself. `workspace = true` inherits
        # the forbid and is the normal case.
        names_it = re.search(r"^\s*unsafe_code\s*=", text, re.MULTILINE) is not None
        if names_it:
            relaxed.append(crate)
            if crate != EXEMPT:
                problems.append(
                    f"crates/{crate}/Cargo.toml sets `unsafe_code` itself; only "
                    f"{EXEMPT} may, and only for the JVM export attribute"
                )

    if EXEMPT not in relaxed:
        problems.append(
            f"crates/{EXEMPT}/Cargo.toml no longer names `unsafe_code`. Either "
            "the exception moved and this script did not, or the crate is now "
            "inheriting a `forbid` it cannot satisfy"
        )

    if problems:
        print("the unsafe exception is not the size the documentation claims:")
        for problem in problems:
            print(f"  {problem}")
        print()
        print(
            "The workspace forbids unsafe code and one crate cannot: a JVM calls\n"
            "`extern \"system\"` symbols by name. That exception is supposed to be\n"
            "the export attribute and nothing else. `docs/PORTING.md` says so in\n"
            "words; this is what makes it true."
        )
        return 1

    print(
        f"unsafe: no crate writes it; only {EXEMPT} relaxes the lint, "
        "for the JVM export attribute"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
