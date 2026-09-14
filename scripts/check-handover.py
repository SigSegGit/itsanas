# -*- coding: utf-8 -*-
"""The pointer at the top of HANDOVER.md names a real step, from a real commit.

Why this exists
---------------

Every session on this project starts cold, and the first question is always
"where were we". For months the answer was prose in HANDOVER.md §0, and prose
rots the way every other hand-written number here rotted before it had a gate:
§0 said "ten gates, all green" on 2026-09-14 while an eleventh existed and was
red, and it described §8.1(a) as uncommitted work waiting on a receipt long
after that stopped being the useful question.

So the state lives in a machine-readable block, and this checks it:

    <!-- ITSANAS-STATE
    NEXT: 8.1b
    TITLE: Bound writes on the honest client
    WRITTEN-AT: 2026-09-14
    BASE: <sha of main when the block was written>
    -->

What is checked
---------------

* the block exists, once, inside §0, with exactly those four fields;
* `NEXT` names an item that exists in §8 -- `8.1b` is item 1, step (b) -- and
  that item is not marked done;
* `WRITTEN-AT` is a date;
* `BASE` is a commit that is an ancestor of `HEAD`, so the pointer was written
  on this history and not on a branch that was thrown away.

What is deliberately not checked: whether CI is green, whether the tree is
clean. Those are facts about git and GitHub at the moment somebody looks, and a
file claiming to know them is wrong from the next push onwards.

`BASE` is skipped with a warning where there is no git history to ask, such as a
source tarball; CI always has one.
"""

import datetime
import io
import os
import re
import subprocess
import sys

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')
HANDOVER = os.path.join(ROOT, 'docs', 'HANDOVER.md')

BLOCK = re.compile(r'<!-- ITSANAS-STATE\n(.*?)\n-->', re.S)
FIELDS = ('NEXT', 'TITLE', 'WRITTEN-AT', 'BASE')
STEP = re.compile(r'^(\d+)\.(\d+)([a-z])?$')
DONE = re.compile(r'✅|\bDONE\b|\bDone\b|\(done\)|~~', re.I)


def section(text, number):
    """The body of `## N.` up to the next `## ` heading."""
    found = re.search(r'^## %d\. .*?$(.*?)(?=^## \d+\. |\Z)' % number, text, re.M | re.S)
    return found.group(1) if found else None


def item(body, index):
    """Top-level numbered item `index.` of a section, up to the next one."""
    found = re.search(
        r'^%d\. (.*?)(?=^\d+\. |\Z)' % index, body, re.M | re.S
    )
    return found.group(0) if found else None


def step(text, letter):
    """Lettered step `letter.` inside an item, up to the next letter."""
    found = re.search(
        r'^\s+%s\. (.*?)(?=^\s+[a-z]\. |\Z)' % letter, text, re.M | re.S
    )
    return found.group(0) if found else None


def git(*args):
    try:
        return subprocess.run(
            ('git',) + args, cwd=ROOT, capture_output=True, text=True
        )
    except OSError:
        return None


def main():
    # §8 headings carry ✅, and a Windows console defaults to cp1252: without
    # this, the report of a bad pointer is a UnicodeEncodeError, which exits
    # non-zero for the wrong reason and says nothing about what to fix.
    sys.stdout.reconfigure(encoding='utf-8', errors='replace')
    text = io.open(HANDOVER, encoding='utf-8').read().replace('\r\n', '\n')
    problems = []

    blocks = BLOCK.findall(text)
    if len(blocks) != 1:
        print('docs/HANDOVER.md has %d ITSANAS-STATE blocks; it needs exactly one,' % len(blocks))
        print('at the top of §0. Without it the next session has to ask where')
        print('things are, which is the question this file exists to answer.')
        return 1

    zero = section(text, 0)
    if zero is None or '<!-- ITSANAS-STATE' not in zero:
        problems.append('the ITSANAS-STATE block is not inside §0')

    fields = {}
    for line in blocks[0].split('\n'):
        key, _, value = line.partition(':')
        fields[key.strip()] = value.strip()
    if tuple(fields) != FIELDS or not all(fields.values()):
        problems.append(
            'the block must have exactly %s, in that order and non-empty; it has %s'
            % (', '.join(FIELDS), ', '.join(fields) or 'nothing')
        )
        fields = {key: fields.get(key, '') for key in FIELDS}

    try:
        datetime.date.fromisoformat(fields['WRITTEN-AT'])
    except ValueError:
        problems.append('WRITTEN-AT %r is not a YYYY-MM-DD date' % fields['WRITTEN-AT'])

    eight = section(text, 8)
    parsed = STEP.match(fields['NEXT'])
    if eight is None:
        problems.append('docs/HANDOVER.md has no §8, so NEXT points at nothing')
    elif not parsed or parsed.group(1) != '8':
        problems.append('NEXT %r is not a §8 item such as 8.1 or 8.1b' % fields['NEXT'])
    else:
        target = item(eight, int(parsed.group(2)))
        if target and parsed.group(3):
            target = step(target, parsed.group(3))
        if target is None:
            problems.append('NEXT %s names no item in §8' % fields['NEXT'])
        else:
            heading = target.strip().split('\n')[0]
            if DONE.search(heading):
                problems.append(
                    'NEXT %s is marked done in §8 (%r); point at the step after it'
                    % (fields['NEXT'], heading[:80])
                )

    result = git('merge-base', '--is-ancestor', fields['BASE'], 'HEAD')
    if result is None or git('rev-parse', '--git-dir').returncode != 0:
        print('warning: no git history here, so BASE was not checked')
    elif result.returncode != 0:
        problems.append(
            'BASE %s is not an ancestor of HEAD; the pointer was written on '
            'another history' % fields['BASE']
        )

    if problems:
        print('the handover pointer does not point anywhere real:')
        for problem in problems:
            print('  %s' % problem)
        print()
        print('Fix the ITSANAS-STATE block at the top of docs/HANDOVER.md §0.')
        return 1

    print('handover: NEXT %s (%s), written %s on %s'
          % (fields['NEXT'], fields['TITLE'], fields['WRITTEN-AT'], fields['BASE'][:9]))
    return 0


if __name__ == '__main__':
    sys.exit(main())
