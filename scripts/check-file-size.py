# -*- coding: utf-8 -*-
"""No source file over a thousand lines, and the ones already over it only shrink.

Why this exists
---------------

On 2026-11-27 thirty-seven tracked source files were over a thousand lines,
84,707 lines between them; `crates/itsanas-cli/src/lib.rs` alone held 8,307.
Each function stayed under the hundred lines clippy allows, so no gate saw the
module itself turn into a drawer: a change went where the last one had gone,
not where it belonged. A reader, a reviewer that sees only a diff, or an agent
with a bounded context, cannot hold such a file, so nobody reviews it whole.

The rule
--------

* A tracked `.rs`, `.kt`, `.swift`, `.py`, `.sh`, `.ps1` or `.js` file has at
  most `LIMIT` lines.
* The files in `CEILING` were over it on 2026-11-27. Each may not grow past the
  count written next to it; splitting one lowers it. One back under `LIMIT`
  must leave the list, so the list only ever gets shorter.
* A file not in `CEILING` over `LIMIT` fails: split it before it lands.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LIMIT = 1000
KINDS = ('.rs', '.kt', '.swift', '.py', '.sh', '.ps1', '.js')

CEILING = {
}


def tracked():
    names = subprocess.run(
        ['git', 'ls-files'], cwd=ROOT, capture_output=True, text=True, check=True
    ).stdout.splitlines()
    return [name for name in names if name.endswith(KINDS)]


def lines(name):
    with io.open(os.path.join(ROOT, name), encoding='utf-8', errors='replace') as handle:
        return sum(1 for _ in handle)


def main():
    problems = []
    seen = set()
    for name in tracked():
        if not os.path.exists(os.path.join(ROOT, name)):
            continue
        count = lines(name)
        ceiling = CEILING.get(name)
        if ceiling is None:
            if count > LIMIT:
                problems.append('%s has %d lines, over %d: split it' % (name, count, LIMIT))
            continue
        seen.add(name)
        if count > ceiling:
            problems.append(
                '%s grew to %d lines, past its ceiling of %d: it may only shrink'
                % (name, count, ceiling)
            )
        elif count <= LIMIT:
            problems.append(
                '%s is down to %d lines: take it out of CEILING in scripts/check-file-size.py'
                % (name, count)
            )
        elif count < ceiling:
            problems.append(
                '%s shrank to %d lines: lower its CEILING from %d to %d'
                % (name, count, ceiling, count)
            )
    for name in sorted(set(CEILING) - seen):
        problems.append('%s is in CEILING but no longer tracked: take it out' % name)
    if problems:
        print('\n'.join(problems))
        return 1
    print('ok: no file over %d lines beyond the %d still being split' % (LIMIT, len(CEILING)))
    return 0


if __name__ == '__main__':
    sys.exit(main())
