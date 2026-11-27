# -*- coding: utf-8 -*-
"""Tests for `sabotage.py`: a failed build is proof only when it was asked for.

Until 2026-11-27 any command that exited nonzero with "error" in its output
counted as a defence the compiler enforces. A sabotage that broke the syntax, a
linker failure or a full disk was then reported as "1 test(s) red" and the plan
passed with no test run. Each case below runs `verify()` on a temporary file
with a fake command, so it costs no build.

    python scripts/test_sabotage.py
"""

import io
import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import sabotage as s  # noqa: E402

failures = []


def expect(name, got, want):
    if got != want:
        failures.append(f"{name}: got {got!r}, want {want!r}")


def fake(printed, code):
    return [sys.executable, "-c", f"import sys; print({printed!r}); sys.exit({code})"]


def run(root, printed, code, **extra):
    defence = dict({"file": "guarded.rs", "live": "if guard {", "dead": "if false {"}, **extra)
    return s.verify("guard", defence, fake(printed, code), root)


def main():
    with tempfile.TemporaryDirectory() as root:
        path = os.path.join(root, "guarded.rs")
        with io.open(path, "w", encoding="utf-8", newline="") as out:
            out.write("fn f() { if guard { } }\n")

        red, problem = run(root, "error: could not compile `itsanas-coord`", 101)
        expect("an unrelated build failure is not a verified defence",
               (red, problem is not None and "without a test going red" in problem),
               (None, True))
        red, problem = run(root, "error: linking with `cc` failed", 101, expect="build")
        expect("a linker failure does not stand in for a compile-time defence",
               (red, problem is not None), (None, True))
        red, problem = run(root, "error[E0080]: evaluation of constant value failed", 101,
                           expect="build")
        expect("a const assertion the compiler rejects still verifies",
               (red, problem), (["(the build itself refused it)"], None))
        red, problem = run(root, "test a::b ... FAILED", 101)
        expect("a red test verifies", (red, problem), (["a::b"], None))
        red, problem = run(root, "test a::b ... ok", 0)
        expect("nothing red is reported as nothing red", (red, problem), ([], None))
        with io.open(path, encoding="utf-8") as handle:
            expect("the file is restored", handle.read(), "fn f() { if guard { } }\n")

    if failures:
        print("sabotage tests FAILED:")
        for f in failures:
            print(f"  {f}")
        sys.exit(1)
    print("sabotage: 6 checks passed")


if __name__ == "__main__":
    main()
