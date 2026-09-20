# -*- coding: utf-8 -*-
"""Break one defence, watch its test go red, put the file back exactly.

Why this exists
---------------

`docs/HANDOVER.md` §4 requires every red-team test to be sabotage-verified:
cancel the defence, watch the test fail, restore. A test that passes both ways
is decoration. The rule is old; doing it by hand is what kept going wrong.

On 2026-09-18, three sabotages were applied to `directory.rs` by text
replacement and restored the same way. Breaking the first block made a second
anchor ambiguous, the restore matched nothing, and the half-restored file then
**looked like a working one** while three unrelated tests failed for a reason
that was not in the code. The file had to be taken back from `main` and the
afternoon's work reapplied.

The failure is not carelessness, it is the method: *editing* a sabotage back is
a second edit that can fail on its own. Copying the file back cannot.

What it does
------------

For each defence, in isolation:

1. copy the file aside, byte for byte;
2. replace the live text with the sabotaged text -- refusing unless the anchor
   appears **exactly once**, because an ambiguous anchor is how the above
   happened;
3. run the command;
4. copy the pristine file back, whatever happened, including on a crash or a
   Ctrl-C;
5. report which tests went red.

A defence whose sabotage turns nothing red is reported loudly: that is the
finding, not a detail. It means the test passes whether or not the system
works.

Usage
-----

Write a small file describing the defences and run it::

    python scripts/sabotage.py my-defences.json

where the file is::

    {
      "command": ["cargo", "test", "-p", "itsanas-coord", "--lib"],
      "defences": {
        "the private-address guard": {
          "file": "crates/itsanas-coord/src/service.rs",
          "live": "if itsanas_tls::reach::is_private_address(&address) {",
          "dead": "if false {"
        }
      }
    }

Exit status is 0 only if every defence turned at least one test red.
"""

import io
import json
import os
import shutil
import subprocess
import sys
import tempfile


def repo_root():
    return os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')


def red_tests(output):
    """The names cargo printed as failures.

    Matching on the line shape rather than on a summary: a summary says how
    many failed, and what this needs to know is *which*, because a sabotage
    that turns the wrong test red has told you nothing about the defence.
    """
    names = []
    for line in output.splitlines():
        if line.startswith('test ') and line.rstrip().endswith('FAILED'):
            names.append(line.split()[1])
    return sorted(set(names))


def verify(name, defence, command, root):
    path = os.path.join(root, defence['file'])
    live, dead = defence['live'], defence['dead']

    with io.open(path, encoding='utf-8') as handle:
        pristine = handle.read()

    found = pristine.count(live)
    if found != 1:
        return None, ('the anchor appears %d times in %s, and an ambiguous anchor '
                      'is how a restore silently fails' % (found, defence['file']))

    # The copy is what restores, so it is made before anything is written and
    # kept outside the tree.
    handle, backup = tempfile.mkstemp(suffix='.pristine')
    os.close(handle)
    shutil.copyfile(path, backup)

    try:
        with io.open(path, 'w', encoding='utf-8', newline='') as out:
            out.write(pristine.replace(live, dead))

        run = subprocess.run(command, cwd=root, capture_output=True, text=True)
        return red_tests(run.stdout + run.stderr), None
    finally:
        # Whatever happened -- a failed build, a panic, a Ctrl-C -- the file goes
        # back byte for byte. This is the whole point of the script.
        shutil.copyfile(backup, path)
        os.unlink(backup)


def main():
    if len(sys.argv) != 2:
        print(__doc__.strip().split('Usage')[-1])
        return 2

    root = repo_root()
    with io.open(sys.argv[1], encoding='utf-8') as handle:
        plan = json.load(handle)

    command = plan['command']
    failures = 0

    for name, defence in plan['defences'].items():
        red, problem = verify(name, defence, command, root)
        if problem:
            print('%-40s REFUSED: %s' % (name, problem))
            failures += 1
            continue
        if not red:
            print('%-40s NOTHING WENT RED' % name)
            print('    Sabotaging this changed no test outcome, so no test is')
            print('    checking it. That is the finding.')
            failures += 1
            continue
        print('%-40s %d test(s) red' % (name, len(red)))
        for test in red:
            print('    %s' % test)

    print('')
    if failures:
        print('%d defence(s) are not verified by any test.' % failures)
        return 1
    print('every defence turned at least one test red, and every file was')
    print('restored from a byte-for-byte copy.')
    return 0


if __name__ == '__main__':
    sys.exit(main())
