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
    'crates/itsanas-net/tests/two_nodes.rs': 4972,
    'crates/itsanas-store/src/index.rs': 4454,
    'crates/itsanas-coord/src/directory.rs': 4162,
    'crates/itsanas-store/src/vault.rs': 3268,
    'crates/itsanas-node/src/contact.rs': 3125,
    'crates/itsanas-node/src/coordinator.rs': 2790,
    'crates/itsanas-net/src/service.rs': 2577,
    'crates/itsanas-net/src/session.rs': 2424,
    'crates/itsanas-node/src/node.rs': 2317,
    'crates/itsanas-store/tests/store.rs': 2199,
    'crates/itsanas-store/src/store.rs': 2078,
    'crates/itsanas-android/src/lib.rs': 1924,
    'crates/itsanas-coord/src/service.rs': 1814,
    'crates/itsanas-node/src/owners.rs': 1803,
    'crates/itsanas-node/src/config.rs': 1703,
    'scripts/check-installers.sh': 1650,
    'crates/itsanas-store/src/index/placement.rs': 1508,
    'crates/itsanas-net/src/transport.rs': 1504,
    'crates/itsanas-coord/tests/coordinator.rs': 1455,
    'crates/itsanas-folder/src/lib.rs': 1297,
    'android/app/src/main/java/fr/ngas/itsanas/MainActivity.kt': 1273,
    'crates/itsanas-net/src/protocol.rs': 1228,
    'crates/itsanas-coord/src/server.rs': 1120,
    'crates/itsanas-cli/src/setup/web/i18n.js': 1102,
    'crates/itsanas-sync/tests/convergence.rs': 1068,
    'crates/itsanas-net/tests/placement.rs': 1037,
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
