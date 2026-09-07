# -*- coding: utf-8 -*-
"""Keep the unsafe exception the size the documentation says it is.

Why this exists
---------------

The workspace forbids unsafe code. Two places cannot, for reasons that are not
the same reason, and lumping them together as "the unsafe crates" is how an
exception stops meaning anything:

* **`itsanas-android`** exports symbols a Java virtual machine calls by name.
  Rust treats `#[unsafe(no_mangle)]` as unsafe because a duplicate exported
  symbol is undefined behaviour at link time. That crate relaxes the lint and
  uses the attribute -- and writes no unsafe *code* at all.

* **`itsanas-drive/src/projfs.rs`** binds five Win32 functions. Windows calls
  back into this process with raw pointers; there is no version of that which
  is safe code. It is one file, and the allowance stops at its edge.

That is two doors held open by comments, and a comment is not a control. Three
things could go through them and nobody would notice: an `unsafe` block written
in some other file of a crate that already relaxes the lint, a third crate
copying the `[lints.rust]` stanza because it was the quickest way past an error,
and -- the one that actually matters -- an unsafe block in the file where they
are allowed, with no argument for why it is sound.

The rule
--------

* No `.rs` file in `crates/` may contain `unsafe {`, `unsafe fn`, `unsafe impl`
  or `unsafe trait`, outside a comment or a doc string, **except** the files
  named in `MAY_WRITE_UNSAFE`.
* Every `unsafe {` block and `unsafe impl` in those files must have a
  `SAFETY:` comment in the six lines above it. An unsafe block whose soundness
  nobody wrote down is one nobody checked.
* Only the crates in `MAY_RELAX` may name `unsafe_code` in their own `[lints]`,
  and each of them must, so that removing the line by accident is a failure
  here rather than a silent widening.

`#[unsafe(no_mangle)]` and `#[unsafe(export_name)]` are permitted anywhere in
`itsanas-android`: they are attributes, not code, and they are the whole reason
that exception exists.
"""

import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
CRATES = ROOT / "crates"

# Crates allowed to relax the lint in their own manifest, and why. Both must be
# present: a crate that stops relaxing it has either moved its unsafe somewhere
# this script is not looking, or is now inheriting a `forbid` it cannot satisfy.
MAY_RELAX = {
    "itsanas-android": "the JVM export attribute",
    "itsanas-drive": "the ProjFS callbacks",
}

# The only files that may write unsafe code, as paths under `crates/`. A file,
# not a crate: `itsanas-drive` is eight hundred lines of ordinary Rust and one
# binding, and the binding is what the exception is for.
MAY_WRITE_UNSAFE = {
    "itsanas-drive/src/projfs.rs",
}

PERMITTED_ATTRIBUTES = ("unsafe(no_mangle)", "unsafe(export_name)")

CODE = re.compile(r"\bunsafe\s*(\{|fn\b|impl\b|trait\b)")
# What needs an argument written next to it. A declaration (`unsafe fn`) states
# a requirement on its caller and is documented with `# Safety`; a block and an
# `unsafe impl` are where a claim is actually being made.
NEEDS_REASON = re.compile(r"\bunsafe\s*(\{|impl\b)")
HOW_FAR_BACK = 6


def is_comment(line):
    stripped = line.strip()
    return stripped.startswith("//") or stripped.startswith("*")


def offending_lines(path):
    """Lines of `path` that write unsafe code rather than an export attribute."""
    found = []
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if is_comment(line):
            continue
        if any(form in line for form in PERMITTED_ATTRIBUTES):
            continue
        if CODE.search(line):
            found.append((number, line.strip()))
    return found


def unargued(path):
    """Unsafe blocks in `path` with no `SAFETY:` note above them."""
    lines = path.read_text(encoding="utf-8").splitlines()
    found = []
    for index, line in enumerate(lines):
        if is_comment(line) or not NEEDS_REASON.search(line):
            continue
        if any(form in line for form in PERMITTED_ATTRIBUTES):
            continue
        window = lines[max(0, index - HOW_FAR_BACK) : index]
        if not any("SAFETY:" in earlier for earlier in window):
            found.append((index + 1, line.strip()))
    return found


def main():
    problems = []

    for path in sorted(CRATES.rglob("*.rs")):
        if "target" in path.parts:
            continue
        where = path.relative_to(CRATES).as_posix()
        if where in MAY_WRITE_UNSAFE:
            for number, line in unargued(path):
                problems.append(
                    f"crates/{where}:{number} claims something is sound and does "
                    f"not say why: {line}"
                )
            continue
        for number, line in offending_lines(path):
            problems.append(f"crates/{where}:{number} writes unsafe code: {line}")

    # A named file that no longer exists is a rule pointing at nothing, and the
    # next person reads the list as though it were still holding a door shut.
    for named in sorted(MAY_WRITE_UNSAFE):
        if not (CRATES / named).is_file():
            problems.append(
                f"crates/{named} is named as the place unsafe may be written and "
                "is not there; the exception has outlived its reason"
            )

    relaxed = []
    for manifest in sorted(CRATES.rglob("Cargo.toml")):
        if "target" in manifest.parts:
            continue
        crate = manifest.relative_to(CRATES).parts[0]
        text = manifest.read_text(encoding="utf-8")
        # Only what the crate declares for itself. `workspace = true` inherits
        # the forbid and is the normal case.
        if re.search(r"^\s*unsafe_code\s*=", text, re.MULTILINE) is None:
            continue
        relaxed.append(crate)
        if crate not in MAY_RELAX:
            problems.append(
                f"crates/{crate}/Cargo.toml sets `unsafe_code` itself; only "
                f"{', '.join(sorted(MAY_RELAX))} may"
            )

    for crate, why in sorted(MAY_RELAX.items()):
        if crate not in relaxed:
            problems.append(
                f"crates/{crate}/Cargo.toml no longer names `unsafe_code`. Either "
                f"the exception for {why} moved and this script did not, or the "
                "crate is now inheriting a `forbid` it cannot satisfy"
            )

    if problems:
        print("the unsafe exception is not the size the documentation claims:")
        for problem in problems:
            print(f"  {problem}")
        print()
        print(
            "The workspace forbids unsafe code. Two places cannot -- a JVM calls\n"
            "`extern \"system\"` symbols by name, and Windows calls back into this\n"
            "process with raw pointers -- and the exception is supposed to be those\n"
            "two and nothing else. `docs/PORTING.md` says so in words; this is what\n"
            "makes it true."
        )
        return 1

    files = ", ".join(sorted(MAY_WRITE_UNSAFE))
    print(
        f"unsafe: written only in {files}, every block with a reason; "
        f"only {', '.join(sorted(MAY_RELAX))} relax the lint"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
